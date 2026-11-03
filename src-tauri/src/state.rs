//! Process-wide state shared by all commands.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

use doculens_core::hw::HardwareSummary;
use doculens_core::paths::AppPaths;
use doculens_core::registry::Registry;
use doculens_core::settings::{AppSettings, LoadStatus};
use doculens_core::workspace::{OpenOptions, Workspace};
use doculens_core::{CoreError, Result};

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

pub struct AppState {
    pub app_paths: AppPaths,
    pub settings: Mutex<AppSettings>,
    pub settings_status: LoadStatus,
    pub workspace: Mutex<Option<Workspace>>,
    pub startup_workspace: Mutex<StartupWorkspaceStatus>,
    pub registry: Registry,
    pub hardware: Mutex<Option<HardwareSummary>>,
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
        }
    }

    pub fn settings_path(&self) -> PathBuf {
        self.app_paths.settings_file()
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
        let ws = Workspace::open(path, opts)?;
        let name = ws.manifest.name.clone();
        {
            let mut guard = self.workspace.lock().map_err(|_| CoreError::Invalid("lock poisoned".into()))?;
            if let Some(old) = guard.take() {
                old.close();
            }
            *guard = Some(ws);
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
