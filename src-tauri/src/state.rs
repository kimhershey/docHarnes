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
use doculens_core::modelstore::{self, DiscoveredModel, ModelWatcher};
use doculens_core::paths::AppPaths;
use doculens_core::pipeline::PipelineLevel;
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
            CoreError::WorkspaceLocked { pid, host } => Some(serde_json::json!({ "pid": pid, "host": host })),
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

pub struct AppState {
    pub app_paths: AppPaths,
    pub settings: Mutex<AppSettings>,
    pub settings_status: LoadStatus,
    pub workspace: Mutex<Option<Workspace>>,
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
    /// Processing requests that arrived while a pipeline job was running (run afterwards, in order).
    pub pipeline_queue: Mutex<Vec<PendingProcess>>,
}

/// A deferred pipeline request (document ids + level).
#[derive(Debug, Clone)]
pub struct PendingProcess {
    pub ids: Vec<String>,
    pub level: PipelineLevel,
}

impl AppState {
    /// Loads settings (never fails: a corrupt file is moved aside).
    pub fn bootstrap() -> AppState {
        let app_paths = AppPaths::default_location();
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
            pipeline_queue: Mutex::new(Vec::new()),
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
            *guard = Some(doculens_core::hw::probe());
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
        // Re-selecting the folder that is already open must not touch the lock.
        {
            let guard = self.workspace.lock().map_err(|_| CoreError::Invalid("lock poisoned".into()))?;
            if let Some(current) = guard.as_ref() {
                if current.root() == path {
                    let name = current.manifest.name.clone();
                    drop(guard);
                    let mut s = self.settings.lock().map_err(|_| CoreError::Invalid("lock poisoned".into()))?;
                    s.remember_workspace(path, &name);
                    s.save(&self.settings_path())?;
                    return Ok(());
                }
            }
        }
        let ws = Workspace::open(path, opts)?;
        let name = ws.manifest.name.clone();
        {
            let mut guard = self.workspace.lock().map_err(|_| CoreError::Invalid("lock poisoned".into()))?;
            if let Some(old) = guard.take() {
                old.close();
            }
            *guard = Some(ws);
            self.workspace_generation.fetch_add(1, Ordering::SeqCst);
        }
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
