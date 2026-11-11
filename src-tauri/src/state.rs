//! Process-wide state shared by all commands.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::AppHandle;

use doculens_core::documents::convert::PdfPrinter;
use doculens_core::documents::import::ImportPlan;
use doculens_core::engine::EngineManager;
use doculens_core::fswatch::DirWatcher;
use doculens_core::hw::HardwareSummary;
use doculens_core::jobs::EngineGate;
use doculens_core::modelstore::{self, DiscoveredModel, ModelWatcher};
use doculens_core::paths::AppPaths;
use doculens_core::registry::Registry;
use doculens_core::settings::{AppSettings, LoadStatus};
use doculens_core::workspace::{OpenOptions, Workspace};
use doculens_core::{CoreError, Result};

use crate::jobs::JobBook;

/// Error payload sent to the webview (`invoke` rejects with this object).
#[derive(Debug, Clone, Serialize)]
pub struct UiError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

impl From<CoreError> for UiError {
    fn from(e: CoreError) -> Self {
        let detail = match &e {
            // A lock of this PC that still blocks belongs to a running DocuLens and is never taken
            // over; only a lock from another host can be force-released.
            CoreError::WorkspaceLocked { pid, host } => Some(serde_json::json!({
                "pid": pid,
                "host": host,
                "forcible": *host != doculens_core::workspace::hostname(),
            })),
            _ => None,
        };
        UiError { code: e.code().to_string(), message: e.to_string(), detail }
    }
}

impl UiError {
    pub fn poisoned() -> Self {
        UiError { code: "internal".into(), message: "state lock poisoned".into(), detail: None }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        UiError { code: "invalid".into(), message: message.into(), detail: None }
    }
}

pub type UiResult<T> = std::result::Result<T, UiError>;

/// How long the exit handler waits for the last database checkpoint.
const EXIT_CHECKPOINT_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// What happened while opening the startup workspace (shown once by the UI).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StartupWorkspaceStatus {
    /// No active workspace configured — show onboarding.
    None,
    Opened {
        path: PathBuf,
    },
    /// The configured path does not exist / is not a workspace — show onboarding.
    Missing {
        path: PathBuf,
    },
    Locked {
        path: PathBuf,
        pid: u32,
        host: String,
    },
    Failed {
        path: PathBuf,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct StartupStatus {
    pub settings: LoadStatus,
    pub settings_path: PathBuf,
    pub workspace: StartupWorkspaceStatus,
}

/// Result of the latest drop-in folder scan (`models://changed` payload).
#[derive(Debug, Clone, Serialize, Default)]
pub struct ModelIndex {
    pub discovered: Vec<DiscoveredModel>,
    pub roots: Vec<PathBuf>,
    pub scanned_at: Option<DateTime<Utc>>,
    /// `modelstore::scan_signature` of `discovered` (change detection).
    #[serde(skip)]
    pub signature: String,
}

/// A workspace that is open (locked, database migrated) but not the current one yet — see
/// [`AppState::prepare_workspace`].
#[derive(Debug)]
pub enum PreparedWorkspace {
    /// The requested folder is the workspace that is already open.
    Current,
    Opened(Box<Workspace>),
}

pub struct AppState {
    pub app_paths: AppPaths,
    pub settings: Mutex<AppSettings>,
    pub settings_status: LoadStatus,
    pub workspace: Mutex<Option<Workspace>>,
    /// Serialises open / create / close: the "before" and "after" hooks of two switches must
    /// not interleave.
    pub workspace_switch: Mutex<()>,
    pub startup_workspace: Mutex<StartupWorkspaceStatus>,
    /// Built-in registry; drop-in models are merged on demand (`merged_registry`).
    pub registry: Registry,
    pub hardware: Mutex<Option<HardwareSummary>>,
    pub engine: EngineManager,
    pub models: Mutex<ModelIndex>,
    pub watcher: Mutex<Option<ModelWatcher>>,
    pub jobs: JobBook,
    pub app: OnceLock<AppHandle>,
    /// Serialises `presets.ini` regeneration + router reload.
    pub presets_lock: Mutex<()>,
    /// PDFium library path once resolved, or why the last attempt failed
    /// (failures are retried on the next use).
    pub pdfium: Mutex<Option<std::result::Result<PathBuf, String>>>,
    /// Bumped on every workspace open/close; document jobs carry the value they
    /// started with and stop publishing once it is stale.
    pub workspace_generation: AtomicU64,
    /// An inbox change arrived while an import was running → re-check at job end.
    pub inbox_dirty: AtomicBool,
    /// Documents changed while a knowledge index job ran: re-run it afterwards.
    pub knowledge_dirty: AtomicBool,
    /// Answer request ids the user cancelled while streaming.
    pub answer_cancels: Mutex<HashSet<String>>,
    /// Answer request ids currently streaming (cancelled on a workspace switch).
    pub answer_running: Mutex<HashSet<String>>,
    /// True between `before_workspace_change` and the next `on_workspace_changed`:
    /// background work must not start against the workspace being closed.
    pub switching: AtomicBool,
    /// HTML → PDF printer backed by the webview (set in `setup`).
    pub printer: OnceLock<Arc<dyn PdfPrinter>>,
    pub inbox_watcher: Mutex<Option<DirWatcher>>,
    /// Import previews waiting for confirmation, keyed by token.
    pub import_plans: Mutex<HashMap<String, ImportPlan>>,
    /// Print bundle token → directory served by the `doculens://print/` route.
    pub print_bundles: Mutex<HashMap<String, PathBuf>>,
    /// Single-slot engine arbitration: batch jobs yield to interactive requests at page boundaries (§3.9).
    pub gate: EngineGate,
}

impl AppState {
    /// Loads settings (never fails: a corrupt file is moved aside).
    pub fn bootstrap() -> AppState {
        Self::bootstrap_at(AppPaths::default_location())
    }

    /// [`Self::bootstrap`] on a given app-data folder (tests use a temporary one).
    pub fn bootstrap_at(app_paths: AppPaths) -> AppState {
        let settings_path = app_paths.settings_file();
        let (settings, status) = AppSettings::load(&settings_path).unwrap_or_else(|e| {
            log::error!("cannot read {}: {e}", settings_path.display());
            (AppSettings::default(), LoadStatus::Created)
        });
        if status == LoadStatus::Created {
            if let Err(e) = settings.save(&settings_path) {
                log::error!("cannot write {}: {e}", settings_path.display());
            }
        }
        log::info!("settings loaded from {} ({:?})", settings_path.display(), status);
        AppState {
            app_paths,
            settings: Mutex::new(settings),
            settings_status: status,
            workspace: Mutex::new(None),
            workspace_switch: Mutex::new(()),
            startup_workspace: Mutex::new(StartupWorkspaceStatus::None),
            registry: Registry::builtin(),
            hardware: Mutex::new(None),
            engine: EngineManager::new(),
            models: Mutex::new(ModelIndex::default()),
            watcher: Mutex::new(None),
            jobs: JobBook::default(),
            app: OnceLock::new(),
            presets_lock: Mutex::new(()),
            pdfium: Mutex::new(None),
            workspace_generation: AtomicU64::new(0),
            inbox_dirty: AtomicBool::new(false),
            knowledge_dirty: AtomicBool::new(false),
            answer_cancels: Mutex::new(HashSet::new()),
            answer_running: Mutex::new(HashSet::new()),
            switching: AtomicBool::new(false),
            printer: OnceLock::new(),
            inbox_watcher: Mutex::new(None),
            import_plans: Mutex::new(HashMap::new()),
            print_bundles: Mutex::new(HashMap::new()),
            gate: EngineGate::new(),
        }
    }

    pub fn settings_path(&self) -> PathBuf {
        self.app_paths.settings_file()
    }

    pub fn app(&self) -> Option<&AppHandle> {
        self.app.get()
    }

    /// Runs `f` on the settings and persists the result.
    pub fn update_settings(&self, f: impl FnOnce(&mut AppSettings)) -> UiResult<AppSettings> {
        let mut guard = self.settings.lock().map_err(|_| UiError::poisoned())?;
        f(&mut guard);
        guard.normalize();
        guard.save(&self.settings_path())?;
        Ok(guard.clone())
    }

    pub fn settings_snapshot(&self) -> UiResult<AppSettings> {
        Ok(self.settings.lock().map_err(|_| UiError::poisoned())?.clone())
    }

    /// Cached probe (runs it on first use).
    pub fn hardware(&self) -> UiResult<HardwareSummary> {
        let mut guard = self.hardware.lock().map_err(|_| UiError::poisoned())?;
        if guard.is_none() {
            *guard = Some(crate::engine_bridge::probe_hardware(self));
        }
        Ok(guard.clone().expect("probed"))
    }

    /// Workspace models dir plus the global store when it exists.
    pub fn models_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        if let Ok(guard) = self.workspace.lock() {
            if let Some(ws) = guard.as_ref() {
                roots.push(ws.paths.models_dir());
            }
        }
        let global = self.app_paths.global_models_dir();
        if global.is_dir() {
            roots.push(global);
        }
        roots
    }

    pub fn workspace_models_dir(&self) -> Option<PathBuf> {
        self.workspace.lock().ok().and_then(|g| g.as_ref().map(|w| w.paths.models_dir()))
    }

    pub fn model_index(&self) -> ModelIndex {
        self.models.lock().map(|m| m.clone()).unwrap_or_default()
    }

    /// Built-in registry plus drop-in models of the last scan.
    pub fn merged_registry(&self) -> Registry {
        let index = self.model_index();
        modelstore::merged_registry(&self.registry, &index.discovered)
    }

    /// Opens the workspace recorded in `settings.json` at startup.
    pub fn open_startup_workspace(&self) {
        let active = self.settings.lock().ok().and_then(|s| s.active_workspace.clone());
        let status = match active {
            None => StartupWorkspaceStatus::None,
            Some(path) => match self.open_workspace_inner(&path, OpenOptions::default()) {
                Ok(()) => StartupWorkspaceStatus::Opened { path },
                Err(CoreError::InvalidWorkspace(_)) => StartupWorkspaceStatus::Missing { path },
                Err(CoreError::WorkspaceLocked { pid, host }) => StartupWorkspaceStatus::Locked { path, pid, host },
                Err(e) => StartupWorkspaceStatus::Failed { path, message: e.to_string() },
            },
        };
        log::info!("startup workspace: {status:?}");
        if let Ok(mut g) = self.startup_workspace.lock() {
            *g = status;
        }
    }

    /// Opens (or creates) a workspace, closing the current one first, and
    /// records it as active in settings.
    pub fn open_workspace_inner(&self, path: &Path, opts: OpenOptions) -> Result<()> {
        let prepared = self.prepare_workspace(path, opts)?;
        self.install_workspace(path, prepared)
    }

    /// First half of a switch: opens the target (folder, manifest, lock, database) without
    /// touching the workspace that is open. Every way a switch normally fails — the folder is
    /// gone, the drive unplugged, the workspace locked — fails here, before anything of the
    /// current workspace was stopped.
    pub fn prepare_workspace(&self, path: &Path, opts: OpenOptions) -> Result<PreparedWorkspace> {
        // Re-selecting the folder that is already open must not touch the lock.
        {
            let guard = self.workspace.lock().map_err(|_| CoreError::Invalid("lock poisoned".into()))?;
            if guard.as_ref().is_some_and(|current| current.root() == path) {
                return Ok(PreparedWorkspace::Current);
            }
        }
        Ok(PreparedWorkspace::Opened(Box::new(Workspace::open(path, opts)?)))
    }

    /// Second half of a switch: the prepared workspace replaces the current one (which is
    /// closed) and becomes the active workspace in the settings.
    pub fn install_workspace(&self, path: &Path, prepared: PreparedWorkspace) -> Result<()> {
        let name = {
            let mut guard = self.workspace.lock().map_err(|_| CoreError::Invalid("lock poisoned".into()))?;
            match prepared {
                PreparedWorkspace::Current => match guard.as_ref() {
                    Some(current) if current.root() == path => current.manifest.name.clone(),
                    _ => return Err(CoreError::NoWorkspace), // closed since `prepare_workspace`
                },
                PreparedWorkspace::Opened(ws) => {
                    let name = ws.manifest.name.clone();
                    if let Some(old) = guard.take() {
                        old.close();
                    }
                    *guard = Some(*ws);
                    self.workspace_generation.fetch_add(1, Ordering::SeqCst);
                    name
                }
            }
        };
        let mut s = self.settings.lock().map_err(|_| CoreError::Invalid("lock poisoned".into()))?;
        s.remember_workspace(path, &name);
        s.save(&self.settings_path())?;
        Ok(())
    }

    pub fn close_workspace(&self) -> UiResult<()> {
        let mut guard = self.workspace.lock().map_err(|_| UiError::poisoned())?;
        if let Some(ws) = guard.take() {
            ws.close();
        }
        self.workspace_generation.fetch_add(1, Ordering::SeqCst);
        drop(guard);
        self.update_settings(|s| s.active_workspace = None)?;
        Ok(())
    }

    /// App exit: checkpoint the database and release the workspace lock. The process ends without
    /// running destructors, so the lock file would stay behind and the workspace would read as
    /// "in use" on the next PC that opens it (USB / shared / synced folder). The active workspace
    /// stays recorded in the settings for the next start.
    pub fn release_workspace_for_exit(&self) {
        // Nothing may start against the workspace any more.
        self.switching.store(true, Ordering::SeqCst);
        let Some(ws) = self.workspace.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        // The checkpoint waits for the database connection. A job thread that is still inside a
        // long database call must not keep the app from exiting, so it gets a bounded wait (the
        // WAL is replayed at the next open otherwise). The lock file goes either way.
        let db = ws.db.clone();
        let (done, checkpointed) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("doculens-exit-checkpoint".into()).spawn(move || {
            let _ = db.checkpoint();
            let _ = done.send(());
        });
        if spawned.is_ok() {
            let _ = checkpointed.recv_timeout(EXIT_CHECKPOINT_WAIT);
        }
        drop(ws); // releases the lock file (only if it is still this handle's)
    }

    pub fn with_workspace<T>(&self, f: impl FnOnce(&Workspace) -> T) -> UiResult<T> {
        let guard = self.workspace.lock().map_err(|_| UiError::poisoned())?;
        match guard.as_ref() {
            Some(ws) => Ok(f(ws)),
            None => Err(CoreError::NoWorkspace.into()),
        }
    }

    pub fn with_workspace_mut<T>(&self, f: impl FnOnce(&mut Workspace) -> Result<T>) -> UiResult<T> {
        let mut guard = self.workspace.lock().map_err(|_| UiError::poisoned())?;
        match guard.as_mut() {
            Some(ws) => Ok(f(ws)?),
            None => Err(CoreError::NoWorkspace.into()),
        }
    }

    pub fn startup_status(&self) -> UiResult<StartupStatus> {
        Ok(StartupStatus {
            settings: self.settings_status.clone(),
            settings_path: self.settings_path(),
            workspace: self.startup_workspace.lock().map_err(|_| UiError::poisoned())?.clone(),
        })
    }
}

/// Test helpers of the shell crate (it has no `tempfile` dependency).
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    use doculens_core::paths::AppPaths;

    use super::AppState;

    /// A scratch folder under the OS temp directory, removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> TempDir {
            let dir = std::env::temp_dir().join(format!("doculens-shell-test-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&dir).expect("temp dir");
            TempDir(dir)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
        /// An app state whose app-data folder (settings, presets, logs) lives in this folder.
        pub fn state(&self) -> AppState {
            AppState::bootstrap_at(AppPaths::new(self.0.join("appdata")))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use doculens_core::paths::WorkspacePaths;

    use super::test_support::TempDir;
    use super::*;

    /// PLT-6: the process exits without dropping the state, so the exit handler closes the
    /// workspace by hand. The lock file must be gone and the workspace still be the active one.
    #[test]
    fn exit_releases_the_workspace_lock_and_keeps_the_workspace_active() {
        let tmp = TempDir::new();
        let state = tmp.state();
        let root = tmp.path().join("ws");
        state.open_workspace_inner(&root, OpenOptions { create: true, ..Default::default() }).unwrap();
        let lock = WorkspacePaths::new(&root).lock_file();
        assert!(lock.is_file());

        state.release_workspace_for_exit();
        assert!(!lock.exists(), "another PC must not find the workspace \"in use\"");
        assert!(state.workspace.lock().unwrap().is_none());
        assert!(state.switching.load(Ordering::SeqCst), "no background work may start during exit");
        let (saved, _) = AppSettings::load(&state.settings_path()).unwrap();
        assert_eq!(saved.active_workspace.as_deref(), Some(root.as_path()), "the next start reopens it");
        // Idempotent: a second call (or an exit without a workspace) is harmless.
        state.release_workspace_for_exit();
    }

    /// PLT-2: `workspace_open` / `workspace_create` open the target before they stop anything of
    /// the current workspace. A target that cannot be opened must leave everything as it was.
    #[test]
    fn a_switch_that_cannot_open_its_target_leaves_the_open_workspace_alone() {
        let tmp = TempDir::new();
        let state = tmp.state();
        let a = tmp.path().join("a");
        state.open_workspace_inner(&a, OpenOptions { create: true, ..Default::default() }).unwrap();
        let generation = state.workspace_generation.load(Ordering::SeqCst);
        let lock_a = WorkspacePaths::new(&a).lock_file();
        let current = |state: &AppState| state.with_workspace(|ws| ws.root().to_path_buf()).unwrap();

        // The folder is gone (unplugged drive), is no workspace, or is locked from another PC.
        let gone = tmp.path().join("gone");
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let b = tmp.path().join("b");
        drop(Workspace::open(&b, OpenOptions { create: true, ..Default::default() }).unwrap());
        let lock_b = WorkspacePaths::new(&b).lock_file();
        let foreign = doculens_core::workspace::LockInfo {
            pid: 1,
            host: "another-pc".into(),
            app_version: "0.0.0".into(),
            created_at: Utc::now(),
            token: None,
        };
        std::fs::write(&lock_b, serde_json::to_vec(&foreign).unwrap()).unwrap();
        assert!(matches!(state.prepare_workspace(&gone, OpenOptions::default()), Err(CoreError::InvalidWorkspace(_))));
        assert!(matches!(state.prepare_workspace(&plain, OpenOptions::default()), Err(CoreError::InvalidWorkspace(_))));
        assert!(matches!(state.prepare_workspace(&b, OpenOptions::default()), Err(CoreError::WorkspaceLocked { .. })));
        assert!(state.open_workspace_inner(&gone, OpenOptions::default()).is_err());

        assert_eq!(current(&state), a);
        assert_eq!(state.workspace_generation.load(Ordering::SeqCst), generation, "jobs of A stay valid");
        assert!(!state.switching.load(Ordering::SeqCst), "queue / watcher / auto-index of A keep running");
        assert!(lock_a.is_file());
        assert_eq!(state.settings_snapshot().unwrap().active_workspace.as_deref(), Some(a.as_path()));

        // A target that opens: still A until it is installed, then B with A closed and unlocked.
        let prepared = state.prepare_workspace(&b, OpenOptions { force_unlock: true, ..Default::default() }).unwrap();
        assert!(matches!(prepared, PreparedWorkspace::Opened(_)));
        assert_eq!(current(&state), a);
        state.install_workspace(&b, prepared).unwrap();
        assert_eq!(current(&state), b);
        assert_eq!(state.workspace_generation.load(Ordering::SeqCst), generation + 1);
        assert!(!lock_a.exists() && lock_b.is_file());
        assert_eq!(state.settings_snapshot().unwrap().active_workspace.as_deref(), Some(b.as_path()));

        // Re-selecting the open folder opens nothing new.
        let again = state.prepare_workspace(&b, OpenOptions::default()).unwrap();
        assert!(matches!(again, PreparedWorkspace::Current));
        state.install_workspace(&b, again).unwrap();
        assert_eq!(state.workspace_generation.load(Ordering::SeqCst), generation + 1);
    }

    /// PLT-18: the error tells the webview whether "force unlock" can help at all.
    #[test]
    fn locked_error_says_whether_force_unlock_applies() {
        let here = UiError::from(CoreError::WorkspaceLocked { pid: 42, host: doculens_core::workspace::hostname() });
        assert_eq!(here.code, "workspace_locked");
        let detail = here.detail.unwrap();
        assert_eq!((detail["pid"].as_u64(), detail["forcible"].as_bool()), (Some(42), Some(false)));
        let elsewhere = UiError::from(CoreError::WorkspaceLocked { pid: 7, host: "another-pc".into() });
        assert_eq!(elsewhere.detail.unwrap()["forcible"].as_bool(), Some(true));
    }
}
