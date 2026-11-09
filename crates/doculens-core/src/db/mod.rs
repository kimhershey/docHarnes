//! Workspace database (`<workspace>/.doculens/workspace.db`, design §3.6):
//! SQLite via rusqlite (bundled), schema migrations embedded at compile time,
//! WAL journal on local disks and `DELETE` on network / sync folders where WAL
//! shared memory is unreliable.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::Duration;

use chrono::Utc;
use rusqlite::{Connection, OpenFlags, Transaction};

use crate::error::{CoreError, Result};

/// Embedded migrations, applied in order; `PRAGMA user_version` records the
/// last one applied.
const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("schema/0001_init.sql")),
    (2, include_str!("schema/0002_knowledge.sql")),
    (3, include_str!("schema/0003_pipeline.sql")),
    (4, include_str!("schema/0004_jobs_journal.sql")),
    (5, include_str!("schema/0005_projects.sql")),
    (6, include_str!("schema/0006_page_layout.sql")),
];

/// Schema version the current build expects.
pub const SCHEMA_VERSION: u32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalMode {
    Wal,
    Delete,
}

impl JournalMode {
    fn pragma(self) -> &'static str {
        match self {
            JournalMode::Wal => "WAL",
            JournalMode::Delete => "DELETE",
        }
    }
}

/// Picks the journal mode for a workspace root. WAL needs shared memory that
/// network redirectors and cloud-sync clients do not provide reliably, so UNC
/// paths, remote drives and well-known sync folders fall back to `DELETE`.
pub fn journal_mode_for(root: &Path) -> JournalMode {
    if is_network_or_sync_folder(root) {
        JournalMode::Delete
    } else {
        JournalMode::Wal
    }
}

/// Heuristic shared by all platforms: UNC paths and sync-client folder names.
pub fn is_network_or_sync_folder(root: &Path) -> bool {
    let s = root.to_string_lossy();
    if s.starts_with("\\\\") || s.starts_with("//") {
        return true;
    }
    let lower = s.to_lowercase();
    const SYNC_MARKERS: [&str; 7] =
        ["onedrive", "dropbox", "google drive", "googledrive", "icloud", "sharepoint", "box sync"];
    if SYNC_MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    #[cfg(windows)]
    {
        if is_remote_drive_windows(root) {
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn is_remote_drive_windows(root: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Component;
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    /// `DRIVE_REMOTE` (Win32 `WinBase.h`); lives in `Win32::System::WindowsProgramming` in windows-sys.
    const DRIVE_REMOTE: u32 = 4;
    // Only paths with a drive / UNC prefix can be asked; a bare `\foo` (no drive) would
    // turn into `\\`, which GetDriveTypeW reports as a remote root.
    let Some(Component::Prefix(prefix)) = root.components().next() else { return false };
    // `GetDriveTypeW` wants the root with a trailing separator (`C:\`, `\\server\share\`).
    let mut drive = prefix.as_os_str().to_os_string();
    drive.push("\\");
    let wide: Vec<u16> = drive.encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
    unsafe { GetDriveTypeW(wide.as_ptr()) == DRIVE_REMOTE }
}

/// One open workspace database. All access goes through the mutex-guarded
/// connection; SQLite serialises writers anyway and the P2 workload (import
/// jobs + UI reads) is small.
pub struct Db {
    conn: Mutex<Connection>,
    /// Second, read-only connection (file databases only) so searches and status
    /// queries do not wait for a long write transaction; WAL readers never block
    /// the writer.
    reader: Option<Mutex<Connection>>,
    path: PathBuf,
    journal_mode: JournalMode,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db").field("path", &self.path).field("journal_mode", &self.journal_mode).finish()
    }
}

/// Registers sqlite-vec (`vec0` virtual tables, `vec_*` functions) as an
/// auto-extension so every connection opened afterwards has it. Process-wide,
/// idempotent; the extension is statically linked into the bundled SQLite.
pub fn register_extensions() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        type InitFn = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        // SAFETY: sqlite3_vec_init has the sqlite3 extension entry-point ABI; the
        // crate declares it without arguments, so the signature is restored here.
        unsafe {
            let init: InitFn = std::mem::transmute::<*const (), InitFn>(sqlite_vec::sqlite3_vec_init as *const ());
            rusqlite::ffi::sqlite3_auto_extension(Some(init));
        }
    });
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Db(e.to_string())
    }
}

impl Db {
    /// Opens (creating if needed) and migrates the database at `path`.
    pub fn open(path: &Path, journal_mode: JournalMode) -> Result<Db> {
        register_extensions();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        let mut db = Self::init(conn, path.to_path_buf(), journal_mode)?;
        let reader =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        reader.busy_timeout(Duration::from_secs(5))?;
        db.reader = Some(Mutex::new(reader));
        Ok(db)
    }

    /// In-memory database (tests, dry runs).
    pub fn open_in_memory() -> Result<Db> {
        register_extensions();
        let conn = Connection::open_in_memory()?;
        Self::init(conn, PathBuf::from(":memory:"), JournalMode::Delete)
    }

    fn init(conn: Connection, path: PathBuf, journal_mode: JournalMode) -> Result<Db> {
        conn.busy_timeout(Duration::from_secs(5))?;
        if path != Path::new(":memory:") {
            conn.pragma_update(None, "journal_mode", journal_mode.pragma())?;
        }
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        let db = Db { conn: Mutex::new(conn), reader: None, path, journal_mode };
        db.migrate()?;
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn journal_mode(&self) -> JournalMode {
        self.journal_mode
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        // A panic while holding the guard poisons it, but the connection itself is still
        // usable (an open transaction rolled back when it was dropped).
        Ok(self.conn.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Runs a read-only query on the reader connection when there is one (file
    /// databases), otherwise on the main connection.
    pub fn with_read<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        match &self.reader {
            Some(r) => {
                let guard = r.lock().unwrap_or_else(|e| e.into_inner());
                Ok(f(&guard)?)
            }
            None => self.with(f),
        }
    }

    /// Runs `f` with the connection.
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let conn = self.lock()?;
        Ok(f(&conn)?)
    }

    /// Runs `f` inside a transaction that commits when `f` returns `Ok`.
    pub fn with_tx<T>(&self, f: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>) -> Result<T> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    pub fn user_version(&self) -> Result<u32> {
        self.with(|c| c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).map(|v| v as u32))
    }

    fn migrate(&self) -> Result<()> {
        let mut conn = self.lock()?;
        let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let current = current.max(0) as u32;
        if current > SCHEMA_VERSION {
            return Err(CoreError::Db(format!(
                "database schema {current} is newer than this build supports ({SCHEMA_VERSION})"
            )));
        }
        for (version, sql) in MIGRATIONS {
            if *version <= current {
                continue;
            }
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", *version as i64)?;
            tx.commit()?;
            log::info!("workspace db: migrated to schema {version}");
        }
        Ok(())
    }

    /// Writes WAL content back into the main file (call before backups / on close).
    pub fn checkpoint(&self) -> Result<()> {
        if self.journal_mode == JournalMode::Wal {
            self.with(|c| c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)"))?;
        }
        Ok(())
    }

    // -- key/value store --------------------------------------------------

    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached("SELECT value FROM kv WHERE key = ?1")?;
            let mut rows = stmt.query([key])?;
            match rows.next()? {
                Some(row) => Ok(Some(row.get(0)?)),
                None => Ok(None),
            }
        })
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with(|c| {
            c.execute(
                "INSERT INTO kv (key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                rusqlite::params![key, value, now],
            )
            .map(|_| ())
        })
    }

    pub fn kv_delete(&self, key: &str) -> Result<()> {
        self.with(|c| c.execute("DELETE FROM kv WHERE key = ?1", [key]).map(|_| ()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_fresh_file_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws").join("workspace.db");
        let db = Db::open(&path, JournalMode::Wal).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(db.journal_mode(), JournalMode::Wal);
        let mode: String = db.with(|c| c.query_row("PRAGMA journal_mode", [], |r| r.get(0))).unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        db.kv_set("inbox_watch", "true").unwrap();
        db.kv_set("inbox_watch", "false").unwrap();
        assert_eq!(db.kv_get("inbox_watch").unwrap().as_deref(), Some("false"));
        db.kv_delete("inbox_watch").unwrap();
        assert_eq!(db.kv_get("inbox_watch").unwrap(), None);
        db.checkpoint().unwrap();
        drop(db);
        // Re-open: no migration re-runs, data persists.
        let db = Db::open(&path, JournalMode::Delete).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        let tables: Vec<String> = db
            .with(|c| {
                let mut s = c.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
                let rows = s.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect()
            })
            .unwrap();
        for t in ["documents", "pages", "tags", "document_tags", "import_batches", "kv"] {
            assert!(tables.iter().any(|n| n == t), "missing table {t}: {tables:?}");
        }
    }

    #[test]
    fn refuses_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.db");
        {
            let c = Connection::open(&path).unwrap();
            c.pragma_update(None, "user_version", 99).unwrap();
        }
        let err = Db::open(&path, JournalMode::Delete).err().unwrap();
        assert_eq!(err.code(), "db");
    }

    #[test]
    fn journal_mode_heuristics() {
        assert_eq!(journal_mode_for(Path::new("\\\\nas\\share\\docs")), JournalMode::Delete);
        assert_eq!(journal_mode_for(Path::new("C:\\Users\\me\\OneDrive\\Docs")), JournalMode::Delete);
        assert_eq!(journal_mode_for(Path::new("/home/me/Dropbox/docs")), JournalMode::Delete);
        // A local folder: the Linux form everywhere, plus the system drive on Windows
        // (where a drive-less path cannot be classified and must not count as remote).
        assert_eq!(journal_mode_for(Path::new("/home/me/docs")), JournalMode::Wal);
        #[cfg(windows)]
        {
            let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
            assert_eq!(journal_mode_for(Path::new(&format!("{system}\\Work\\Docs"))), JournalMode::Wal);
        }
    }

    #[test]
    fn in_memory_transactions_roll_back_on_error() {
        let db = Db::open_in_memory().unwrap();
        let r = db.with_tx(|tx| {
            tx.execute("INSERT INTO kv (key, value, updated_at) VALUES ('a', '1', '')", [])?;
            Err::<(), _>(rusqlite::Error::InvalidQuery)
        });
        assert!(r.is_err());
        assert_eq!(db.kv_get("a").unwrap(), None);
    }
}
