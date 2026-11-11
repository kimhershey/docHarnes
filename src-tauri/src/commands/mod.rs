//! Tauri commands. Thin wrappers: validation and business logic live in
//! `doculens-core`; this layer only adapts state and error types.

pub mod documents;
pub mod generate;
pub mod knowledge;
pub mod pipeline;
pub mod planner;
pub mod projects;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use doculens_core::engine::{ChatRequest, EngineDevice, EngineStatus, LogLine, ModelInstanceStatus};
use doculens_core::hw::{GpuSource, HardwareSummary};
use doculens_core::modelstore::{self, ImportMode, ImportPlan, InstallState};
use doculens_core::paths::normalize_user_path;
use doculens_core::prerouter::{self, DateRange, PreRouteResult};
use doculens_core::registry::{self, ModelEntry};
use doculens_core::retention::RetentionPolicy;
use doculens_core::settings::{AppSettings, Language, Theme, TierMode};
use doculens_core::workspace::{self, OpenOptions, WorkspaceInspection, WorkspaceManifest};
use doculens_core::{hw::Tier, version, CoreError};

use crate::engine_bridge::{self, Assessment, EngineBinaryInfo};
use crate::jobs::Job;
use crate::native_theme;
use crate::state::{AppState, ModelIndex, StartupStatus, UiError, UiResult};

// ---------------------------------------------------------------------------
// App / startup
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub settings_path: PathBuf,
    pub app_data_dir: PathBuf,
    pub engines_dir: PathBuf,
    pub presets_path: PathBuf,
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
        engines_dir: state.app_paths.engines_dir(),
        presets_path: state.app_paths.presets_file(),
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
    if model_id != "auto" && state.merged_registry().get(&model_id).is_none() {
        return Err(CoreError::UnknownModel(model_id).into());
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
        return Err(UiError::invalid("manual tier requires a tier"));
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

/// The hooks that follow `docs_bridge::before_workspace_change`: they clear the switching flag,
/// restart the watchers and bring back the job queue of whichever workspace is open now. Every
/// "before" must be followed by them, whatever happened in between — otherwise the queue, the
/// inbox watcher, auto-index, auto-processing and the purge stay off until the app restarts.
fn after_workspace_change(app: &AppHandle) {
    engine_bridge::on_workspace_changed(app);
    crate::docs_bridge::on_workspace_changed(app);
    crate::knowledge_bridge::on_workspace_changed(app);
    crate::pipeline_bridge::on_workspace_changed(app);
}

/// Opens (or creates) `path` and makes it the current workspace.
fn switch_workspace(app: &AppHandle, state: &AppState, path: &Path, opts: OpenOptions) -> UiResult<WorkspaceView> {
    let _one_at_a_time = state.workspace_switch.lock().unwrap_or_else(|e| e.into_inner());
    // The target is opened first. When that fails (folder gone, drive unplugged, locked by
    // another instance) the workspace that is open has not been touched: its jobs keep running
    // and its inbox is still watched.
    let prepared = state.prepare_workspace(path, opts)?;
    crate::docs_bridge::before_workspace_change(app);
    let installed = state.install_workspace(path, prepared);
    after_workspace_change(app);
    installed?;
    state.with_workspace(view)
}

#[tauri::command(async)]
pub fn workspace_open(app: AppHandle, state: State<'_, AppState>, args: OpenArgs) -> UiResult<WorkspaceView> {
    let path = normalize_user_path(&args.path);
    switch_workspace(&app, &state, &path, OpenOptions { force_unlock: args.force_unlock, ..Default::default() })
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

#[tauri::command(async)]
pub fn workspace_create(app: AppHandle, state: State<'_, AppState>, args: CreateArgs) -> UiResult<WorkspaceView> {
    let path = normalize_user_path(&args.path);
    let locale = args.locale.or_else(|| state.settings_snapshot().ok().map(|s| s.ui.language));
    let opts = OpenOptions { create: true, name: args.name, locale, force_unlock: args.force_unlock };
    switch_workspace(&app, &state, &path, opts)
}

#[tauri::command(async)]
pub fn workspace_close(app: AppHandle, state: State<'_, AppState>) -> UiResult<()> {
    let _one_at_a_time = state.workspace_switch.lock().unwrap_or_else(|e| e.into_inner());
    crate::docs_bridge::before_workspace_change(&app);
    let closed = state.close_workspace();
    after_workspace_change(&app);
    closed
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
    if refresh.unwrap_or(false) {
        let mut guard = state.hardware.lock().map_err(|_| UiError::poisoned())?;
        *guard = Some(engine_bridge::probe_hardware(&state));
    }
    state.hardware()
}

#[tauri::command(async)]
pub fn hw_assess(state: State<'_, AppState>) -> UiResult<Assessment> {
    engine_bridge::assess(&state)
}

// ---------------------------------------------------------------------------
// Registry / models
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
    let models_dir = state.workspace_models_dir();
    let registry = state.merged_registry();
    let index = state.model_index();
    let models = registry
        .models
        .iter()
        .map(|m| RegistryModelView {
            install: models_dir.as_ref().map(|d| modelstore::install_state(d, m, &index.discovered)),
            total_size: m.total_size(),
            entry: m.clone(),
        })
        .collect();
    Ok(RegistryView {
        models,
        tier_defaults: registry.tier_defaults.clone(),
        default_embedding: registry.default_embedding.clone(),
        models_dir,
    })
}

#[tauri::command]
pub fn registry_required_files_text(state: State<'_, AppState>, model_id: String) -> UiResult<String> {
    let registry = state.merged_registry();
    let model = registry.require(&model_id)?;
    let target = state
        .workspace_models_dir()
        .map(|d| d.join(&model.id))
        .unwrap_or_else(|| PathBuf::from("<workspace>/.doculens/models").join(&model.id));
    Ok(registry::required_files_text(model, &target))
}

#[tauri::command(async)]
pub fn models_scan(app: AppHandle) -> ModelIndex {
    engine_bridge::rescan_and_sync(&app)
}

#[tauri::command]
pub fn models_list(state: State<'_, AppState>) -> ModelIndex {
    state.model_index()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportArgs {
    pub paths: Vec<String>,
    #[serde(default = "default_import_mode")]
    pub mode: ImportMode,
    #[serde(default)]
    pub folder_name: Option<String>,
}

fn default_import_mode() -> ImportMode {
    ImportMode::Copy
}

#[tauri::command(async)]
pub fn models_import_plan(state: State<'_, AppState>, args: ImportArgs) -> UiResult<ImportPlan> {
    let models_dir = state.workspace_models_dir().ok_or(CoreError::NoWorkspace)?;
    let sources: Vec<PathBuf> = args.paths.iter().map(|p| normalize_user_path(p)).collect();
    Ok(modelstore::plan_import(&models_dir, &sources, args.mode, args.folder_name.as_deref())?)
}

#[tauri::command(async)]
/// The plan is recomputed here from the arguments (the webview-supplied plan
/// is display-only), so destination names and folders are always derived by
/// the core from validated GGUF headers.
pub fn models_import_run(app: AppHandle, args: ImportArgs) -> UiResult<String> {
    let state = app.state::<AppState>();
    let models_dir = state.workspace_models_dir().ok_or(CoreError::NoWorkspace)?;
    let sources: Vec<PathBuf> = args.paths.iter().map(|p| normalize_user_path(p)).collect();
    let plan: ImportPlan = modelstore::plan_import(&models_dir, &sources, args.mode, args.folder_name.as_deref())?;
    if plan.already_present {
        return Err(UiError::invalid("the same files are already in the target folder"));
    }
    Ok(engine_bridge::import_job(&app, plan))
}

#[tauri::command(async)]
pub fn models_download(app: AppHandle, model_id: String) -> UiResult<String> {
    engine_bridge::download_job(&app, &model_id)
}

#[tauri::command(async)]
pub fn models_hash(app: AppHandle, folder: String) -> UiResult<String> {
    engine_bridge::hash_job(&app, &folder)
}

#[tauri::command(async)]
pub fn models_delete(app: AppHandle, state: State<'_, AppState>, folder: String) -> UiResult<()> {
    let models_dir = state.workspace_models_dir().ok_or(CoreError::NoWorkspace)?;
    let index = state.model_index();
    let folder_path = normalize_user_path(&folder);
    if let Some(d) = index.discovered.iter().find(|d| d.folder == folder_path || d.folder_name == folder) {
        if state.engine.is_running() && state.engine.status().model(&d.id).is_some_and(|m| m.is_running()) {
            state.engine.unload_model(&d.id)?;
        }
        modelstore::delete_model_folder(&models_dir, &d.folder)?;
    } else {
        modelstore::delete_model_folder(&models_dir, &folder_path)?;
    }
    engine_bridge::rescan_and_sync(&app);
    Ok(())
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn engine_status(state: State<'_, AppState>) -> EngineStatus {
    state.engine.status()
}

#[tauri::command(async)]
pub fn engine_start(app: AppHandle) -> UiResult<EngineStatus> {
    Ok(engine_bridge::start_engine(&app)?)
}

#[tauri::command(async)]
pub fn engine_stop(state: State<'_, AppState>) -> UiResult<()> {
    Ok(state.engine.stop()?)
}

#[tauri::command(async)]
pub fn engine_restart(app: AppHandle) -> UiResult<EngineStatus> {
    Ok(engine_bridge::restart_engine(&app)?)
}

/// Loads a model as a background job (starts the engine first when stopped).
#[tauri::command(async)]
pub fn engine_load_model(
    app: AppHandle,
    state: State<'_, AppState>,
    model_id: String,
    warmup: Option<bool>,
) -> UiResult<String> {
    if !state.engine.is_running() {
        engine_bridge::start_engine(&app)?;
    } else if state.engine.status().model(&model_id).is_none() {
        // A model that appeared after the last preset generation.
        engine_bridge::regenerate_and_reload(&state)?;
    }
    Ok(engine_bridge::load_model_job(&app, &model_id, warmup))
}

#[tauri::command(async)]
pub fn engine_unload_model(state: State<'_, AppState>, model_id: String) -> UiResult<()> {
    Ok(state.engine.unload_model(&model_id)?)
}

#[tauri::command(async)]
pub fn engine_refresh_models(state: State<'_, AppState>, reload: Option<bool>) -> UiResult<Vec<ModelInstanceStatus>> {
    if reload.unwrap_or(false) {
        engine_bridge::regenerate_and_reload(&state)?;
        return Ok(state.engine.status().models);
    }
    Ok(state.engine.refresh_models(false)?)
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatTestResult {
    pub model: String,
    pub prompt: String,
    pub content: String,
    pub finish_reason: Option<String>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub elapsed_ms: u64,
    pub tokens_per_second: Option<f64>,
}

/// Smoke test / benchmark request (model manager "test" button).
#[tauri::command(async)]
pub fn engine_chat_test(
    state: State<'_, AppState>,
    model_id: Option<String>,
    prompt: Option<String>,
) -> UiResult<ChatTestResult> {
    let model = match model_id {
        Some(m) => m,
        None => state
            .engine
            .status()
            .active_model
            .or_else(|| engine_bridge::active_model_id(&state).ok().flatten())
            .ok_or_else(|| UiError::invalid("no active model"))?,
    };
    let prompt = prompt.unwrap_or_else(|| "안녕하세요. 한 문장으로 자기소개를 해 주세요.".to_string());
    let start = Instant::now();
    let req = ChatRequest::text(&model, &prompt).max_tokens(64);
    let r = state.engine.chat(&req)?;
    let elapsed = start.elapsed();
    let tps = r.timings.as_ref().and_then(|t| t.get("predicted_per_second")).and_then(|v| v.as_f64());
    Ok(ChatTestResult {
        model,
        prompt,
        content: r.content,
        finish_reason: r.finish_reason,
        prompt_tokens: r.usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0),
        completion_tokens: r.usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0),
        elapsed_ms: elapsed.as_millis() as u64,
        tokens_per_second: tps,
    })
}

/// `llama-server --list-devices`; engine-reported GPUs replace the NVML rows
/// (the tier is defined on the engine's free VRAM, design §3.3.3).
#[tauri::command(async)]
pub fn engine_list_devices(app: AppHandle, state: State<'_, AppState>) -> UiResult<Vec<EngineDevice>> {
    let settings = state.settings_snapshot()?;
    let binary = engine_bridge::resolve_binary(&app, &settings)?;
    let devices = doculens_core::engine::devices::list_devices(&binary.path, Duration::from_secs(60))?;
    if !devices.is_empty() {
        let gpus = doculens_core::engine::devices::to_gpu_infos(&devices);
        // The free figures were read just now, with whatever the engine has loaded.
        let engine_used_mib = engine_bridge::engine_vram_in_use_mib(&state);
        let mut guard = state.hardware.lock().map_err(|_| UiError::poisoned())?;
        if let Some(hw) = guard.as_mut() {
            hw.engine_used_mib = engine_used_mib;
            // Keep NVML driver/CC details when the names match.
            let mut merged = gpus;
            for g in &mut merged {
                if let Some(n) = hw.gpus.iter().find(|n| n.source == GpuSource::Nvml && n.name == g.name) {
                    g.driver_version = n.driver_version.clone();
                    g.cuda_driver_version = n.cuda_driver_version;
                    g.compute_capability = n.compute_capability;
                }
            }
            hw.gpus = merged;
        }
    }
    Ok(devices)
}

#[tauri::command(async)]
pub fn engine_binary_info(app: AppHandle) -> UiResult<EngineBinaryInfo> {
    engine_bridge::binary_info(&app)
}

#[tauri::command]
pub fn engine_logs(state: State<'_, AppState>, lines: Option<usize>) -> Vec<LogLine> {
    state.engine.recent_logs(lines.unwrap_or(200))
}

#[tauri::command(async)]
pub fn presets_preview(state: State<'_, AppState>) -> UiResult<String> {
    Ok(engine_bridge::presets_preview(&state)?)
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn jobs_list(state: State<'_, AppState>) -> Vec<Job> {
    state.jobs.list()
}

#[tauri::command]
pub fn jobs_cancel(state: State<'_, AppState>, job_id: String) -> bool {
    state.jobs.cancel(&job_id)
}

#[tauri::command]
pub fn jobs_clear_finished(state: State<'_, AppState>) {
    state.jobs.clear_finished();
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

// ---------------------------------------------------------------------------
// Files shown outside the app
// ---------------------------------------------------------------------------

/// What `reveal_path` showed: the path itself, or (`exact = false`) the nearest folder above it
/// that still exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Revealed {
    pub path: PathBuf,
    pub exact: bool,
}

/// The thing to show for `path`: itself when it exists, else the nearest existing folder above it.
fn reveal_target(path: &Path) -> Option<Revealed> {
    if path.exists() {
        return Some(Revealed { path: path.to_path_buf(), exact: true });
    }
    let folder = path.ancestors().skip(1).find(|p| !p.as_os_str().is_empty() && p.is_dir())?;
    Some(Revealed { path: folder.to_path_buf(), exact: false })
}

/// "폴더 열기": selects `path` in the file manager. A file that was moved or deleted since (an
/// organised document, a cleaned-up export) used to make the button do nothing; now the nearest
/// folder that still exists opens.
#[tauri::command(async)]
pub fn reveal_path(path: String) -> UiResult<Revealed> {
    let requested = normalize_user_path(&path);
    let shown = reveal_target(&requested).ok_or_else(|| UiError::from(CoreError::NotFound(path.clone())))?;
    let result = if shown.exact {
        tauri_plugin_opener::reveal_item_in_dir(&shown.path)
    } else {
        tauri_plugin_opener::open_path(&shown.path, None::<&str>)
    };
    result.map_err(|e| UiError::invalid(format!("cannot show {}: {e}", shown.path.display())))?;
    Ok(shown)
}

/// Documents that ship with the app, opened from 설정 › 정보 (never a web address: the app is
/// made for offline PCs, and the installed version is the one that counts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppDocument {
    /// `THIRD-PARTY-NOTICES.md` next to the application.
    Notices,
    /// `licenses/THIRD-PARTY-LICENSES.txt` next to the application.
    Licenses,
    /// The design document this build was made from (carried inside the binary).
    DesignDoc,
}

const DESIGN_DOC_NAME: &str = "doculens-design-v1.1.md";
const DESIGN_DOC: &str = include_str!("../../../docs/design/doculens-design-v1.1.md");

impl AppDocument {
    /// Path below the application's resource folder (`None`: not a bundled file).
    fn resource_path(self) -> Option<&'static str> {
        match self {
            AppDocument::Notices => Some("THIRD-PARTY-NOTICES.md"),
            AppDocument::Licenses => Some("licenses/THIRD-PARTY-LICENSES.txt"),
            AppDocument::DesignDoc => None,
        }
    }
}

/// First of `roots` that holds the bundled file `relative`.
fn bundled_file(roots: &[PathBuf], relative: &str) -> Option<PathBuf> {
    roots.iter().map(|root| root.join(relative)).find(|p| p.is_file())
}

/// Writes the embedded design document to `<app data>/docs/` (again only when it differs, e.g.
/// after an update) and returns its path.
fn materialize_design_doc(app_data: &Path) -> Result<PathBuf, CoreError> {
    let dir = app_data.join("docs");
    let path = dir.join(DESIGN_DOC_NAME);
    if std::fs::read_to_string(&path).is_ok_and(|current| current == DESIGN_DOC) {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    std::fs::write(&path, DESIGN_DOC).map_err(|e| CoreError::io(&path, e))?;
    Ok(path)
}

/// Opens one of the app's own documents in a text viewer and returns the file's path.
#[tauri::command(async)]
pub fn open_app_document(app: AppHandle, state: State<'_, AppState>, document: AppDocument) -> UiResult<PathBuf> {
    let path = match document.resource_path() {
        Some(relative) => {
            let mut roots = Vec::new();
            if let Ok(dir) = app.path().resource_dir() {
                roots.push(dir);
            }
            if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
                roots.push(dir);
            }
            bundled_file(&roots, relative).ok_or_else(|| UiError::from(CoreError::NotFound(relative.to_string())))?
        }
        None => materialize_design_doc(&state.app_paths.root)?,
    };
    // `.md` has no default application on a plain Windows install; Notepad is always there.
    let viewer = if cfg!(windows) { Some("notepad") } else { None };
    tauri_plugin_opener::open_path(&path, viewer)
        .or_else(|_| tauri_plugin_opener::reveal_item_in_dir(&path))
        .map_err(|e| UiError::invalid(format!("cannot open {}: {e}", path.display())))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::TempDir;

    /// PKG-15: "폴더 열기" for a file that is gone shows the nearest folder that still exists.
    #[test]
    fn reveal_falls_back_to_the_nearest_existing_folder() {
        let tmp = TempDir::new();
        let exports = tmp.path().join("ws").join("exports");
        std::fs::create_dir_all(&exports).unwrap();
        let file = exports.join("집계.xlsx");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(reveal_target(&file), Some(Revealed { path: file.clone(), exact: true }));
        assert_eq!(reveal_target(&exports), Some(Revealed { path: exports.clone(), exact: true }));
        // The file was deleted; then its folder as well.
        std::fs::remove_file(&file).unwrap();
        assert_eq!(reveal_target(&file), Some(Revealed { path: exports.clone(), exact: false }));
        let deep = exports.join("2026").join("11").join("집계.xlsx");
        assert_eq!(reveal_target(&deep), Some(Revealed { path: exports.clone(), exact: false }));
        std::fs::remove_dir_all(tmp.path().join("ws")).unwrap();
        assert_eq!(reveal_target(&file), Some(Revealed { path: tmp.path().to_path_buf(), exact: false }));
        // Nothing to show for an empty or relative path that leads nowhere.
        assert_eq!(reveal_target(Path::new("")), None);
        assert_eq!(reveal_target(Path::new("no-such-folder-anywhere/x.pdf")), None);
    }

    /// PKG-7: the notices and license texts come from the installed files, the design document
    /// from the binary — no web address involved.
    #[test]
    fn app_documents_are_local_files() {
        let tmp = TempDir::new();
        let resources = tmp.path().join("install");
        std::fs::create_dir_all(resources.join("licenses")).unwrap();
        std::fs::write(resources.join("THIRD-PARTY-NOTICES.md"), b"# notices").unwrap();
        std::fs::write(resources.join("licenses").join("THIRD-PARTY-LICENSES.txt"), b"MIT").unwrap();
        let roots = vec![tmp.path().join("elsewhere"), resources.clone()];
        for (doc, file) in [
            (AppDocument::Notices, resources.join("THIRD-PARTY-NOTICES.md")),
            (AppDocument::Licenses, resources.join("licenses").join("THIRD-PARTY-LICENSES.txt")),
        ] {
            assert_eq!(bundled_file(&roots, doc.resource_path().unwrap()), Some(file));
        }
        assert_eq!(bundled_file(&roots[..1], "THIRD-PARTY-NOTICES.md"), None);
        assert_eq!(AppDocument::DesignDoc.resource_path(), None);
        assert_eq!(serde_json::from_str::<AppDocument>("\"design_doc\"").unwrap(), AppDocument::DesignDoc);

        // The design document is written once and refreshed when the file differs.
        let app_data = tmp.path().join("appdata");
        let path = materialize_design_doc(&app_data).unwrap();
        assert_eq!(path, app_data.join("docs").join(DESIGN_DOC_NAME));
        assert!(DESIGN_DOC.contains("DocuLens") && std::fs::read_to_string(&path).unwrap() == DESIGN_DOC);
        std::fs::write(&path, "an older copy").unwrap();
        assert_eq!(materialize_design_doc(&app_data).unwrap(), path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DESIGN_DOC);
    }
}
