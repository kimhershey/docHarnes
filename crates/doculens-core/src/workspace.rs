//! Workspace folders (design §3.6): a user-chosen directory that holds all
//! inference/document data under `.doculens/`, plus `inbox/`, `documents/`
//! and `exports/`. One app instance opens one workspace at a time; a lock
//! file prevents two instances from opening the same folder.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CoreError, Result};
use crate::paths::WorkspacePaths;
use crate::retention::RetentionPolicy;
use crate::settings::Language;
use crate::version::{APP_VERSION, WORKSPACE_DIR_NAME, WORKSPACE_SCHEMA_VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelsLocation {
    /// `<workspace>/.doculens/models/` (decision 4, portable).
    #[default]
    Workspace,
    /// `<AppData>/DocuLens/models/` shared between workspaces.
    Global,
}

/// `<workspace>/.doculens/workspace.json`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceManifest {
    pub workspace_id: Uuid,
    pub schema_version: u32,
    /// App version that created the workspace.
    pub app_version: String,
    /// App version that last opened it (for "results were produced by model X" notices).
    pub last_opened_app_version: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_opened_at: Option<DateTime<Utc>>,
    pub locale: Language,
    /// Recommendation only; the active model is global (design §3.3.3).
    pub preferred_model: Option<String>,
    pub models_location: ModelsLocation,
    pub retention: RetentionPolicy,
}

impl Default for WorkspaceManifest {
    fn default() -> Self {
        Self {
            workspace_id: Uuid::new_v4(),
            schema_version: WORKSPACE_SCHEMA_VERSION,
            app_version: APP_VERSION.to_string(),
            last_opened_app_version: APP_VERSION.to_string(),
            name: String::new(),
            created_at: Utc::now(),
            last_opened_at: None,
            locale: Language::Ko,
            preferred_model: None,
            models_location: ModelsLocation::Workspace,
            retention: RetentionPolicy::default(),
        }
    }
}

impl WorkspaceManifest {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| CoreError::io(path, e))?;
        serde_json::from_slice(&bytes).map_err(|e| CoreError::json(path, e))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(self).map_err(|e| CoreError::json(path, e))?;
        std::fs::write(&tmp, json).map_err(|e| CoreError::io(&tmp, e))?;
        std::fs::rename(&tmp, path).map_err(|e| CoreError::io(path, e))?;
        Ok(())
    }
}

/// `<workspace>/.doculens/lock`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockInfo {
    pub pid: u32,
    pub host: String,
    pub app_version: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockState {
    /// No lock file.
    Free,
    /// Lock from a process that no longer exists on this host.
    Stale,
    /// Lock from a live process on this host.
    HeldHere,
    /// Lock written by another host (network share) — liveness unknown.
    HeldElsewhere,
}

fn hostname() -> String {
    sysinfo::System::host_name().unwrap_or_else(|| "localhost".to_string())
}

fn process_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(Pid::from_u32(pid)).is_some()
}

pub fn read_lock(paths: &WorkspacePaths) -> (LockState, Option<LockInfo>) {
    let Ok(bytes) = std::fs::read(paths.lock_file()) else { return (LockState::Free, None) };
    let Ok(info) = serde_json::from_slice::<LockInfo>(&bytes) else { return (LockState::Stale, None) };
    let state = if info.host != hostname() {
        LockState::HeldElsewhere
    } else if info.pid != std::process::id() && !process_alive(info.pid) {
        LockState::Stale
    } else {
        LockState::HeldHere
    };
    (state, Some(info))
}

/// Result of `inspect`, used by onboarding and the workspace picker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceInspection {
    pub path: PathBuf,
    pub exists: bool,
    pub is_dir: bool,
    /// `.doculens/workspace.json` present.
    pub initialized: bool,
    pub manifest: Option<WorkspaceManifest>,
    pub lock_state: LockState,
    pub lock: Option<LockInfo>,
    /// Supported document files found outside `.doculens/` (capped at `DOCUMENT_COUNT_CAP`).
    pub document_files: u32,
    /// Path looks like a drive root / home directory and should not be used.
    pub unsafe_location: bool,
}

pub const DOCUMENT_COUNT_CAP: u32 = 5_000;
pub const DOCUMENT_EXTENSIONS: &[&str] =
    &["pdf", "png", "jpg", "jpeg", "tif", "tiff", "webp", "bmp", "heic", "heif", "hwp", "hwpx", "docx", "txt", "md"];

fn is_unsafe_location(root: &Path) -> bool {
    if root.parent().is_none() {
        return true; // "/" or "C:\"
    }
    if let Some(home) = dirs::home_dir() {
        if root == home {
            return true;
        }
    }
    false
}

fn count_document_files(root: &Path, cap: u32) -> u32 {
    let mut count = 0u32;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == WORKSPACE_DIR_NAME {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if DOCUMENT_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
                    count += 1;
                    if count >= cap {
                        return cap;
                    }
                }
            }
        }
    }
    count
}

/// Inspects a folder without modifying it.
pub fn inspect(root: &Path) -> WorkspaceInspection {
    let paths = WorkspacePaths::new(root);
    let exists = root.exists();
    let is_dir = root.is_dir();
    let manifest = if is_dir { WorkspaceManifest::load(&paths.manifest_file()).ok() } else { None };
    let (lock_state, lock) = if is_dir { read_lock(&paths) } else { (LockState::Free, None) };
    WorkspaceInspection {
        path: root.to_path_buf(),
        exists,
        is_dir,
        initialized: manifest.is_some(),
        manifest,
        lock_state,
        lock,
        document_files: if is_dir { count_document_files(root, DOCUMENT_COUNT_CAP) } else { 0 },
        unsafe_location: is_unsafe_location(root),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpenOptions {
    /// Initialise `.doculens/` when missing (create a new workspace).
    pub create: bool,
    /// Display name for a newly created workspace (defaults to the folder name).
    pub name: Option<String>,
    pub locale: Option<Language>,
    /// Take over a lock held by another host or a live process (user confirmed).
    pub force_unlock: bool,
}

/// An open workspace. Dropping it releases the lock.
#[derive(Debug)]
pub struct Workspace {
    pub paths: WorkspacePaths,
    pub manifest: WorkspaceManifest,
    lock_held: bool,
}

impl Workspace {
    pub fn open(root: &Path, opts: OpenOptions) -> Result<Workspace> {
        if root.as_os_str().is_empty() {
            return Err(CoreError::InvalidWorkspace("empty path".into()));
        }
        if is_unsafe_location(root) {
            return Err(CoreError::InvalidWorkspace(format!("{} is a drive root or home directory", root.display())));
        }
        let paths = WorkspacePaths::new(root);
        let manifest_path = paths.manifest_file();
        let existing = if root.is_dir() { manifest_path.is_file() } else { false };
        if !existing && !opts.create {
            return Err(CoreError::InvalidWorkspace(format!("{} is not a DocuLens workspace", root.display())));
        }
        if root.exists() && !root.is_dir() {
            return Err(CoreError::InvalidWorkspace(format!("{} is not a directory", root.display())));
        }
        for dir in paths.all_dirs() {
            std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
        }

        // Lock
        let (state, info) = read_lock(&paths);
        match state {
            LockState::Free | LockState::Stale => {}
            LockState::HeldHere | LockState::HeldElsewhere => {
                let info = info.expect("lock info present when held");
                let ours = info.pid == std::process::id() && info.host == hostname();
                if !ours && !opts.force_unlock {
                    return Err(CoreError::WorkspaceLocked { pid: info.pid, host: info.host });
                }
            }
        }
        let lock = LockInfo {
            pid: std::process::id(),
            host: hostname(),
            app_version: APP_VERSION.to_string(),
            created_at: Utc::now(),
        };
        let lock_path = paths.lock_file();
        std::fs::write(&lock_path, serde_json::to_vec(&lock).expect("lock serialises"))
            .map_err(|e| CoreError::io(&lock_path, e))?;

        // Manifest
        let mut manifest = if existing {
            WorkspaceManifest::load(&manifest_path)?
        } else {
            WorkspaceManifest {
                name: opts.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| folder_name(root)),
                locale: opts.locale.unwrap_or_default(),
                ..Default::default()
            }
        };
        if manifest.name.trim().is_empty() {
            manifest.name = folder_name(root);
        }
        manifest.last_opened_at = Some(Utc::now());
        manifest.last_opened_app_version = APP_VERSION.to_string();
        manifest.save(&manifest_path)?;

        Ok(Workspace { paths, manifest, lock_held: true })
    }

    pub fn root(&self) -> &Path {
        &self.paths.root
    }

    pub fn update_manifest(&mut self, f: impl FnOnce(&mut WorkspaceManifest)) -> Result<()> {
        f(&mut self.manifest);
        self.manifest.retention.validate()?;
        self.manifest.save(&self.paths.manifest_file())
    }

    /// Releases the lock explicitly (also done on drop).
    pub fn close(mut self) {
        self.release_lock();
    }

    fn release_lock(&mut self) {
        if !self.lock_held {
            return;
        }
        self.lock_held = false;
        let (_, info) = read_lock(&self.paths);
        if let Some(info) = info {
            if info.pid == std::process::id() && info.host == hostname() {
                let _ = std::fs::remove_file(self.paths.lock_file());
            }
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        self.release_lock();
    }
}

pub fn folder_name(root: &Path) -> String {
    root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| root.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_open_lock_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("My Docs");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.pdf"), b"x").unwrap();
        std::fs::write(root.join("b.HWPX"), b"x").unwrap();
        std::fs::write(root.join("notes.docx"), b"x").unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("c.jpg"), b"x").unwrap();
        std::fs::write(root.join("ignore.exe"), b"x").unwrap();

        let insp = inspect(&root);
        assert!(insp.exists && insp.is_dir && !insp.initialized);
        assert_eq!(insp.document_files, 4);
        assert_eq!(insp.lock_state, LockState::Free);
        assert!(!insp.unsafe_location);

        // Not initialised and create=false -> error
        assert!(matches!(Workspace::open(&root, OpenOptions::default()), Err(CoreError::InvalidWorkspace(_))));

        let ws = Workspace::open(
            &root,
            OpenOptions { create: true, name: None, locale: Some(Language::En), force_unlock: false },
        )
        .unwrap();
        assert_eq!(ws.manifest.name, "My Docs");
        assert_eq!(ws.manifest.locale, Language::En);
        assert!(ws.paths.models_dir().is_dir());
        assert!(ws.paths.inbox_dir().is_dir());
        assert!(ws.paths.lock_file().is_file());

        // Same process re-open is allowed (lock is ours)
        let insp = inspect(&root);
        assert!(insp.initialized);
        assert_eq!(insp.lock_state, LockState::HeldHere);
        let again = Workspace::open(&root, OpenOptions::default()).unwrap();
        assert_eq!(again.manifest.workspace_id, ws.manifest.workspace_id);
        drop(again);
        drop(ws);
        assert!(!WorkspacePaths::new(&root).lock_file().exists());

        // Stale lock from a dead pid is recovered
        let paths = WorkspacePaths::new(&root);
        let stale =
            LockInfo { pid: 4_000_000_000, host: hostname(), app_version: "0.0.0".into(), created_at: Utc::now() };
        std::fs::write(paths.lock_file(), serde_json::to_vec(&stale).unwrap()).unwrap();
        assert_eq!(read_lock(&paths).0, LockState::Stale);
        let ws = Workspace::open(&root, OpenOptions::default()).unwrap();
        assert_eq!(read_lock(&paths).0, LockState::HeldHere);
        drop(ws);

        // Lock from another host requires force_unlock
        let other = LockInfo { pid: 1, host: "other-host".into(), app_version: "0.0.0".into(), created_at: Utc::now() };
        std::fs::write(paths.lock_file(), serde_json::to_vec(&other).unwrap()).unwrap();
        assert!(matches!(Workspace::open(&root, OpenOptions::default()), Err(CoreError::WorkspaceLocked { .. })));
        let ws = Workspace::open(&root, OpenOptions { force_unlock: true, ..Default::default() }).unwrap();
        assert_eq!(read_lock(&paths).1.unwrap().host, hostname());
        drop(ws);
    }

    #[test]
    fn manifest_update_validates_retention() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let mut ws = Workspace::open(&root, OpenOptions { create: true, ..Default::default() }).unwrap();
        ws.update_manifest(|m| m.retention.chats_days = Some(30)).unwrap();
        let loaded = WorkspaceManifest::load(&ws.paths.manifest_file()).unwrap();
        assert_eq!(loaded.retention.chats_days, Some(30));
        assert!(ws.update_manifest(|m| m.retention.purge_hour_local = 99).is_err());
    }

    #[test]
    fn refuses_root_and_home() {
        assert!(is_unsafe_location(Path::new("/")));
        if let Some(home) = dirs::home_dir() {
            assert!(is_unsafe_location(&home));
            assert!(matches!(
                Workspace::open(&home, OpenOptions { create: true, ..Default::default() }),
                Err(CoreError::InvalidWorkspace(_))
            ));
        }
    }
}
