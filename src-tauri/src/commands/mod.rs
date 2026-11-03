//! Tauri commands. Thin wrappers: validation and business logic live in
//! `doculens-core`; this layer only adapts state and error types.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use doculens_core::hw::{self, HardwareSummary, TierAssessment};
use doculens_core::paths::normalize_user_path;
use doculens_core::prerouter::{self, DateRange, PreRouteResult};
use doculens_core::registry::{self, InstallState, ModelEntry};
use doculens_core::retention::RetentionPolicy;
use doculens_core::settings::{AppSettings, Language, Theme, TierMode};
use doculens_core::workspace::{self, OpenOptions, WorkspaceInspection, WorkspaceManifest};
use doculens_core::{hw::Tier, version};

use crate::native_theme;
use crate::state::{AppState, StartupStatus, UiError, UiResult};

// ---------------------------------------------------------------------------
// App / startup
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub settings_path: PathBuf,
    pub app_data_dir: PathBuf,
    pub os: String,
    pub arch: &'static str,
    pub debug: bool,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        name: version::APP_NAME,
        version: version::APP_VERSION,
        settings_path: state.settings_path(),
        app_data_dir: state.app_paths.root.clone(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH,
        debug: cfg!(debug_assertions),
    }
}

#[tauri::command]
pub fn startup_status(state: State<'_, AppState>) -> UiResult<StartupStatus> {
    state.startup_status()
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> UiResult<AppSettings> {
    state.settings_snapshot()
}

/// Replaces the whole settings object (the UI sends a full, validated copy).
#[tauri::command]
pub fn settings_update(state: State<'_, AppState>, settings: AppSettings) -> UiResult<AppSettings> {
    let app_paths_workspace = state.settings_snapshot()?.active_workspace;
    state.update_settings(move |s| {
        *s = settings;
        // The active workspace is owned by the workspace commands, never by a bulk update.
        s.active_workspace = app_paths_workspace;
    })
}

fn apply_native_theme(app: &AppHandle, theme: Theme) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_theme(native_theme(theme));
    }
}

#[tauri::command]
pub fn settings_set_theme(app: AppHandle, state: State<'_, AppState>, theme: Theme) -> UiResult<AppSettings> {
    let s = state.update_settings(|s| s.ui.theme = theme)?;
    apply_native_theme(&app, theme);
    Ok(s)
}

#[tauri::command]
pub fn settings_set_language(state: State<'_, AppState>, language: Language) -> UiResult<AppSettings> {
    state.update_settings(|s| s.ui.language = language)
}

#[tauri::command]
pub fn settings_set_ui_scale(state: State<'_, AppState>, scale_percent: u16) -> UiResult<AppSettings> {
    state.update_settings(|s| s.ui.scale_percent = scale_percent)
}

#[tauri::command]
pub fn settings_set_active_model(state: State<'_, AppState>, model_id: String) -> UiResult<AppSettings> {
    if model_id != "auto" && state.registry.get(&model_id).is_none() {
        return Err(doculens_core::CoreError::UnknownModel(model_id).into());
    }
    state.update_settings(|s| s.models.active_model = model_id)
}

#[tauri::command]
pub fn settings_set_tier_mode(
    state: State<'_, AppState>,
    mode: TierMode,
    manual_tier: Option<Tier>,
) -> UiResult<AppSettings> {
    if mode == TierMode::Manual && manual_tier.is_none() {
        return Err(UiError { code: "invalid".into(), message: "manual tier requires a tier".into(), detail: None });
    }
    state.update_settings(|s| {
        s.models.tier_mode = mode;
        s.models.manual_tier = manual_tier;
    })
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceView {
    pub path: PathBuf,
    pub manifest: WorkspaceManifest,
    pub models_dir: PathBuf,
    pub inbox_dir: PathBuf,
    pub documents_dir: PathBuf,
    pub exports_dir: PathBuf,
}

fn view(ws: &doculens_core::workspace::Workspace) -> WorkspaceView {
    WorkspaceView {
        path: ws.paths.root.clone(),
        manifest: ws.manifest.clone(),
        models_dir: ws.paths.models_dir(),
        inbox_dir: ws.paths.inbox_dir(),
        documents_dir: ws.paths.documents_dir(),
        exports_dir: ws.paths.exports_dir(),
    }
}

/// Async: walks the folder (bounded) off the main thread so typing a path never freezes the UI.
#[tauri::command(async)]
pub fn workspace_inspect(path: String) -> WorkspaceInspection {
    workspace::inspect(&normalize_user_path(&path))
}

#[tauri::command]
pub fn workspace_current(state: State<'_, AppState>) -> UiResult<Option<WorkspaceView>> {
    let guard = state.workspace.lock().map_err(|_| UiError::poisoned())?;
    Ok(guard.as_ref().map(view))
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenArgs {
    pub path: String,
    #[serde(default)]
    pub force_unlock: bool,
}

#[tauri::command]
pub fn workspace_open(state: State<'_, AppState>, args: OpenArgs) -> UiResult<WorkspaceView> {
    let path = normalize_user_path(&args.path);
    state.open_workspace_inner(&path, OpenOptions { force_unlock: args.force_unlock, ..Default::default() })?;
    state.with_workspace(view)
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateArgs {
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub locale: Option<Language>,
    #[serde(default)]
    pub force_unlock: bool,
}

#[tauri::command]
pub fn workspace_create(state: State<'_, AppState>, args: CreateArgs) -> UiResult<WorkspaceView> {
    let path = normalize_user_path(&args.path);
    let locale = args.locale.or_else(|| state.settings_snapshot().ok().map(|s| s.ui.language));
    state.open_workspace_inner(
        &path,
        OpenOptions { create: true, name: args.name, locale, force_unlock: args.force_unlock },
    )?;
    state.with_workspace(view)
}

#[tauri::command]
pub fn workspace_close(state: State<'_, AppState>) -> UiResult<()> {
    state.close_workspace()
}

#[tauri::command]
pub fn workspace_forget(state: State<'_, AppState>, path: String) -> UiResult<AppSettings> {
    let path = normalize_user_path(&path);
    state.update_settings(|s| s.forget_workspace(&path))
}

#[tauri::command]
pub fn workspace_set_retention(state: State<'_, AppState>, retention: RetentionPolicy) -> UiResult<WorkspaceView> {
    state.with_workspace_mut(|ws| {
        ws.update_manifest(|m| m.retention = retention)?;
        Ok(view(ws))
    })
}

#[tauri::command]
pub fn workspace_set_preferred_model(state: State<'_, AppState>, model_id: Option<String>) -> UiResult<WorkspaceView> {
    state.with_workspace_mut(|ws| {
        ws.update_manifest(|m| m.preferred_model = model_id)?;
        Ok(view(ws))
    })
}

// ---------------------------------------------------------------------------
// Hardware / tier
// ---------------------------------------------------------------------------

/// Async: NVML initialisation can stall for a noticeable time.
#[tauri::command(async)]
pub fn hw_probe(state: State<'_, AppState>, refresh: Option<bool>) -> UiResult<HardwareSummary> {
    let mut guard = state.hardware.lock().map_err(|_| UiError::poisoned())?;
    if refresh.unwrap_or(false) || guard.is_none() {
        *guard = Some(hw::probe());
    }
    Ok(guard.clone().expect("probed"))
}

#[derive(Debug, Clone, Serialize)]
pub struct Assessment {
    pub tier: TierAssessment,
    pub recommended_model: Option<ModelEntry>,
    pub active_model: Option<ModelEntry>,
    pub active_model_installed: Option<InstallState>,
}

#[tauri::command(async)]
pub fn hw_assess(state: State<'_, AppState>) -> UiResult<Assessment> {
    let hw = {
        let mut guard = state.hardware.lock().map_err(|_| UiError::poisoned())?;
        if guard.is_none() {
            *guard = Some(hw::probe());
        }
        guard.clone().expect("probed")
    };
    let settings = state.settings_snapshot()?;
    let tier = hw::assess(&hw, &settings.models);
    let recommended = state.registry.default_model_for(tier.tier, tier.free_vram_mib).cloned();
    let active = if settings.models.active_model == "auto" {
        recommended.clone()
    } else {
        state.registry.get(&settings.models.active_model).cloned()
    };
    let models_dir = state.workspace.lock().map_err(|_| UiError::poisoned())?.as_ref().map(|w| w.paths.models_dir());
    let installed = match (&active, &models_dir) {
        (Some(m), Some(dir)) => Some(registry::install_state(dir, m)),
        _ => None,
    };
    Ok(Assessment { tier, recommended_model: recommended, active_model: active, active_model_installed: installed })
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct RegistryModelView {
    #[serde(flatten)]
    pub entry: ModelEntry,
    pub install: Option<InstallState>,
    pub total_size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryView {
    pub models: Vec<RegistryModelView>,
    pub tier_defaults: std::collections::BTreeMap<Tier, String>,
    pub default_embedding: Option<String>,
    pub models_dir: Option<PathBuf>,
}

#[tauri::command]
pub fn registry_list(state: State<'_, AppState>) -> UiResult<RegistryView> {
    let models_dir = state.workspace.lock().map_err(|_| UiError::poisoned())?.as_ref().map(|w| w.paths.models_dir());
    let models = state
        .registry
        .models
        .iter()
        .map(|m| RegistryModelView {
            install: models_dir.as_ref().map(|d| registry::install_state(d, m)),
            total_size: m.total_size(),
            entry: m.clone(),
        })
        .collect();
    Ok(RegistryView {
        models,
        tier_defaults: state.registry.tier_defaults.clone(),
        default_embedding: state.registry.default_embedding.clone(),
        models_dir,
    })
}

#[tauri::command]
pub fn registry_required_files_text(state: State<'_, AppState>, model_id: String) -> UiResult<String> {
    let model = state.registry.require(&model_id)?;
    let target = state
        .workspace
        .lock()
        .map_err(|_| UiError::poisoned())?
        .as_ref()
        .map(|w| w.paths.models_dir().join(&model.id))
        .unwrap_or_else(|| PathBuf::from("<workspace>/.doculens/models").join(&model.id));
    Ok(registry::required_files_text(model, &target))
}

// ---------------------------------------------------------------------------
// Natural-language pre-router
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn prerouter_route(text: String) -> PreRouteResult {
    prerouter::route(&text)
}

#[tauri::command]
pub fn prerouter_resolve_date(text: String) -> Option<DateRange> {
    let today = chrono::Local::now().date_naive();
    prerouter::resolve_relative_date(&text, today)
}
