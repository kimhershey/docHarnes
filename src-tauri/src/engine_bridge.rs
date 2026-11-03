//! Glue between the core engine/model-store modules and the Tauri shell:
//! model folder scans + watcher, preset generation, engine start/auto-start,
//! event forwarding to the webview and background jobs (load / import /
//! download / hash) with progress.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::Duration;

use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use doculens_core::engine::{
    locate, presets, EngineBinary, EngineConfig, EngineEvent, EngineStatus, PresetBuild, PresetContext,
};
use doculens_core::hw::{self, TierAssessment};
use doculens_core::modelstore::{self, DiscoveredModel, ImportPlan, InstallState, ModelWatcher, ScanOptions};
use doculens_core::registry::ModelEntry;
use doculens_core::settings::AppSettings;
use doculens_core::{CoreError, Result};

use crate::jobs::JobKind;
use crate::state::{AppState, ModelIndex, UiError, UiResult};

pub const EV_ENGINE_STATUS: &str = "engine://status";
pub const EV_ENGINE_LOG: &str = "engine://log";
pub const EV_ENGINE_MODEL: &str = "engine://model";
pub const EV_MODELS_CHANGED: &str = "models://changed";

const LOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const WATCH_SETTLE: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// Model folders
// ---------------------------------------------------------------------------

/// Scans every model root and publishes `models://changed`.
pub fn rescan(app: &AppHandle) -> ModelIndex {
    rescan_inner(app).0
}

/// Scan + publish; the flag says whether anything relevant to the engine
/// changed since the previous scan (so the router is not reloaded needlessly).
fn rescan_inner(app: &AppHandle) -> (ModelIndex, bool) {
    let state = app.state::<AppState>();
    let roots = state.models_roots();
    let discovered = modelstore::scan_all(&roots, &state.registry, ScanOptions::default());
    let signature = modelstore::scan_signature(&discovered);
    let index = ModelIndex { discovered, roots, scanned_at: Some(Utc::now()), signature: signature.clone() };
    let changed = match state.models.lock() {
        Ok(mut guard) => {
            let changed = guard.signature != signature;
            *guard = index.clone();
            changed
        }
        Err(_) => true,
    };
    let _ = app.emit(EV_MODELS_CHANGED, &index);
    (index, changed)
}

fn on_models_dir_changed(app: &AppHandle) {
    let (index, changed) = rescan_inner(app);
    if !changed {
        return;
    }
    let state = app.state::<AppState>();
    log::info!("model folder changed: {} model(s)", index.discovered.len());
    if state.engine.is_running() {
        match write_presets(&state) {
            Ok(_) => {
                if let Err(e) = state.engine.refresh_models(true) {
                    log::warn!("router reload after folder change failed: {e}");
                }
            }
            Err(e) => log::warn!("cannot regenerate presets: {e}"),
        }
    }
}

/// Called after a workspace was opened/created/closed: rescan, (re)start the
/// folder watcher and auto-start the engine when configured.
pub fn on_workspace_changed(app: &AppHandle) {
    let state = app.state::<AppState>();
    rescan(app);
    let models_dir = state.workspace_models_dir();
    if let Ok(mut guard) = state.watcher.lock() {
        *guard = None;
        if let Some(dir) = &models_dir {
            let app2 = app.clone();
            match ModelWatcher::start(dir, WATCH_SETTLE, move || on_models_dir_changed(&app2)) {
                Ok(w) => *guard = Some(w),
                Err(e) => log::warn!("model folder watcher not started: {e}"),
            }
        }
    }
    let auto = state.settings_snapshot().map(|s| s.models.auto_start_engine).unwrap_or(false);
    if models_dir.is_some() {
        if auto {
            let app2 = app.clone();
            std::thread::Builder::new().name("doculens-engine-autostart".into()).spawn(move || auto_start(&app2)).ok();
        }
    } else if state.engine.is_running() {
        let _ = state.engine.stop();
    }
}

// ---------------------------------------------------------------------------
// Assessment (tier + active model) on the merged registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Assessment {
    pub tier: TierAssessment,
    pub recommended_model: Option<ModelEntry>,
    pub active_model: Option<ModelEntry>,
    pub active_model_installed: Option<InstallState>,
}

pub fn assess(state: &AppState) -> UiResult<Assessment> {
    let hw = state.hardware()?;
    let settings = state.settings_snapshot()?;
    let tier = hw::assess(&hw, &settings.models);
    let registry = state.merged_registry();
    let recommended = registry.default_model_for(tier.tier, tier.free_vram_mib).cloned();
    let active = if settings.models.active_model == "auto" {
        recommended.clone()
    } else {
        registry.get(&settings.models.active_model).cloned()
    };
    let index = state.model_index();
    let installed = match (&active, state.workspace_models_dir()) {
        (Some(m), Some(dir)) => Some(modelstore::install_state(&dir, m, &index.discovered)),
        _ => None,
    };
    Ok(Assessment { tier, recommended_model: recommended, active_model: active, active_model_installed: installed })
}

/// Concrete id of the active model (`auto` resolved through the tier rules).
pub fn active_model_id(state: &AppState) -> UiResult<Option<String>> {
    Ok(assess(state)?.active_model.map(|m| m.id))
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct EngineBinaryInfo {
    pub resolved: Option<EngineBinary>,
    pub error: Option<String>,
    pub installed_packs: Vec<locate::InstalledPack>,
    pub engines_dir: PathBuf,
    pub bundled_roots: Vec<PathBuf>,
    pub min_build: u32,
}

fn bundled_roots(app: &AppHandle) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        roots.push(dir.join("engines"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.join("engines"));
        }
    }
    roots
}

pub fn resolve_binary(app: &AppHandle, settings: &AppSettings) -> Result<EngineBinary> {
    let state = app.state::<AppState>();
    let hw = state.hardware().map_err(|e| CoreError::engine(e.message))?;
    let gpu = hw.primary_gpu();
    let roots = bundled_roots(app);
    let inputs = locate::LocateInputs {
        engines_dir: &state.app_paths.engines_dir(),
        bundled_roots: &roots,
        override_path: settings.models.engine_binary.as_deref(),
        gpu,
        preference: settings.models.backend,
    };
    locate::locate(&inputs)
}

pub fn binary_info(app: &AppHandle) -> UiResult<EngineBinaryInfo> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot()?;
    let engines_dir = state.app_paths.engines_dir();
    let (resolved, error) = match resolve_binary(app, &settings) {
        Ok(b) => (Some(b.with_probed_version()), None),
        Err(e) => (None, Some(e.to_string())),
    };
    Ok(EngineBinaryInfo {
        resolved,
        error,
        installed_packs: locate::list_installed(&engines_dir),
        engines_dir,
        bundled_roots: bundled_roots(app),
        min_build: locate::MIN_ENGINE_BUILD,
    })
}

/// Generates `presets.ini` from the merged registry + scan for the current
/// backend/tier/profile and writes it to the app-data directory.
pub fn write_presets(state: &AppState) -> Result<PresetBuild> {
    let settings = state.settings_snapshot().map_err(|e| CoreError::engine(e.message))?;
    let hw = state.hardware().map_err(|e| CoreError::engine(e.message))?;
    let tier = hw::assess(&hw, &settings.models);
    let app = state.app().ok_or_else(|| CoreError::engine("app handle not attached"))?;
    let backend = resolve_binary(app, &settings)?.backend;
    let registry = state.merged_registry();
    let index = state.model_index();
    let active = active_model_id(state).ok().flatten();
    let ctx = PresetContext {
        backend,
        tier: tier.tier,
        profile: settings.models.profile,
        settings: &settings.models,
        active_model: active.as_deref(),
        debug: cfg!(debug_assertions),
    };
    let build = presets::build_presets(&registry, &index.discovered, &ctx);
    build.file.write(&state.app_paths.presets_file())?;
    Ok(build)
}

pub fn presets_preview(state: &AppState) -> Result<String> {
    write_presets(state).map(|b| b.file.to_ini())
}

static FORWARDER: OnceLock<()> = OnceLock::new();

/// Forwards engine events to the webview (started once per process).
fn ensure_forwarder(app: &AppHandle) {
    FORWARDER.get_or_init(|| {
        let state = app.state::<AppState>();
        let rx = state.engine.subscribe();
        let app2 = app.clone();
        std::thread::Builder::new()
            .name("doculens-engine-events".into())
            .spawn(move || {
                for ev in rx {
                    let _ = match ev {
                        EngineEvent::Status { status } => app2.emit(EV_ENGINE_STATUS, &status),
                        EngineEvent::Log { line } => app2.emit(EV_ENGINE_LOG, &line),
                        EngineEvent::Model { model } => app2.emit(EV_ENGINE_MODEL, &model),
                    };
                }
            })
            .ok();
    });
}

/// Starts (or restarts) the sidecar with freshly generated presets.
pub fn start_engine(app: &AppHandle) -> Result<EngineStatus> {
    ensure_forwarder(app);
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot().map_err(|e| CoreError::engine(e.message))?;
    let binary = resolve_binary(app, &settings)?.with_probed_version();
    if binary.is_too_old() {
        return Err(CoreError::engine(format!(
            "engine_too_old:{}:{}",
            binary.version.clone().unwrap_or_default(),
            locate::MIN_ENGINE_BUILD
        )));
    }
    let build = write_presets(&state)?;
    let mut cfg = EngineConfig::new(binary, state.app_paths.presets_file(), state.app_paths.root.clone());
    cfg.sleep_idle_seconds = settings.models.sleep_idle_seconds;
    cfg.models_max = u32::from(settings.models.resident_models_max);
    cfg.expose_slots = cfg!(debug_assertions);
    cfg.mmproj_sizes = build.mmproj_sizes;
    std::fs::create_dir_all(&cfg.cwd).map_err(|e| CoreError::io(&cfg.cwd, e))?;
    state.engine.start(cfg)
}

/// Engine start + load of the active model, run on a background thread.
fn auto_start(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.engine.is_running() {
        // Workspace switch: presets change, the router keeps unchanged instances.
        if let Err(e) = write_presets(&state).and_then(|_| state.engine.refresh_models(true)) {
            log::warn!("preset refresh after workspace change failed: {e}");
        }
    } else {
        let (job, _) = state.jobs.create(JobKind::EngineStart, "engine");
        state.jobs.update(&job, |j| j.detail = Some("starting".into()));
        match start_engine(app) {
            Ok(st) => state.jobs.finish(&job, Ok(Some(format!("running on port {}", st.port.unwrap_or(0))))),
            Err(e) => {
                log::warn!("engine auto-start failed: {e}");
                state.jobs.finish(&job, Err(e.to_string()));
                return;
            }
        }
    }
    match assess(&state) {
        Ok(a) => {
            if let (Some(m), Some(inst)) = (a.active_model, a.active_model_installed) {
                if inst.installed {
                    load_model_job(app, &m.id, None);
                }
            }
        }
        Err(e) => log::warn!("assessment failed: {}", e.message),
    }
}

/// Loads a model as a job (progress from router load stages). Returns the job id.
pub fn load_model_job(app: &AppHandle, model_id: &str, warmup: Option<bool>) -> String {
    let state = app.state::<AppState>();
    if let Some(existing) = state
        .jobs
        .list()
        .into_iter()
        .find(|j| j.kind == JobKind::ModelLoad && j.subject == model_id && !j.state.is_finished())
    {
        return existing.id;
    }
    let (job, cancel) = state.jobs.create(JobKind::ModelLoad, model_id);
    let warmup =
        warmup.unwrap_or_else(|| state.settings_snapshot().map(|s| s.models.warmup_after_load).unwrap_or(true));
    let app2 = app.clone();
    let id = model_id.to_string();
    let job2 = job.clone();
    std::thread::Builder::new()
        .name("doculens-model-load".into())
        .spawn(move || {
            let state = app2.state::<AppState>();
            state.jobs.update(&job2, |j| j.detail = Some("loading".into()));
            // Progress reporter: mirrors the router's load stages while the blocking load runs.
            let app3 = app2.clone();
            let job3 = job2.clone();
            let id2 = id.clone();
            let cancel2 = cancel.clone();
            let reporter = std::thread::spawn(move || {
                let state = app3.state::<AppState>();
                loop {
                    std::thread::sleep(Duration::from_millis(300));
                    let Some(j) = state.jobs.get(&job3) else { break };
                    if j.state.is_finished() {
                        break;
                    }
                    if cancel2.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Some(m) = state.engine.status().model(&id2) {
                        state.jobs.progress(
                            &job3,
                            m.progress,
                            None,
                            None,
                            m.stage.clone().or_else(|| Some(m.status.clone())),
                        );
                    }
                }
            });
            let result = state
                .engine
                .load_model(&id, LOAD_TIMEOUT, warmup)
                .map(|m| Some(format!("{} ({} bytes)", m.status, m.memory_effective_bytes.unwrap_or(0))))
                .map_err(|e| e.to_string());
            state.jobs.finish(&job2, result);
            let _ = reporter.join();
        })
        .ok();
    job
}

/// Executes an import plan as a job. Returns the job id.
pub fn import_job(app: &AppHandle, plan: ImportPlan) -> String {
    let state = app.state::<AppState>();
    let (job, cancel) = state.jobs.create(JobKind::ModelImport, &plan.folder_name);
    let app2 = app.clone();
    let job2 = job.clone();
    std::thread::Builder::new()
        .name("doculens-model-import".into())
        .spawn(move || {
            let state = app2.state::<AppState>();
            state.jobs.update(&job2, |j| {
                j.bytes_total = Some(plan.total_bytes);
                j.detail = Some(format!("{:?}", plan.mode).to_lowercase());
            });
            let result = modelstore::run_import(&plan, &mut |p| {
                let frac = if p.total > 0 { p.total_done as f32 / p.total as f32 } else { 0.0 };
                state.jobs.progress(&job2, Some(frac), Some(p.total_done), Some(p.total), Some(p.file.clone()));
                !cancel.load(Ordering::Relaxed)
            });
            match result {
                Ok(folder) => {
                    rescan(&app2);
                    if state.engine.is_running() {
                        let _ = write_presets(&state).and_then(|_| state.engine.refresh_models(true));
                    }
                    state.jobs.finish(&job2, Ok(Some(folder.display().to_string())));
                }
                Err(e) => state.jobs.finish(&job2, Err(e.to_string())),
            }
        })
        .ok();
    job
}

/// Downloads every file of a registry model into the workspace models dir.
pub fn download_job(app: &AppHandle, model_id: &str) -> UiResult<String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot()?;
    if !settings.models.allow_online_download {
        return Err(UiError::invalid("online downloads are disabled in settings"));
    }
    let registry = state.merged_registry();
    let model = registry.require(model_id)?.clone();
    let models_dir = state.workspace_models_dir().ok_or(CoreError::NoWorkspace)?;
    if model.files.iter().any(|f| f.download_url().is_none()) {
        return Err(UiError::invalid("this model has no public download URL"));
    }
    if state.jobs.is_active(JobKind::ModelDownload, model_id) {
        return Err(UiError::invalid("download already running"));
    }
    let (job, cancel) = state.jobs.create(JobKind::ModelDownload, model_id);
    let app2 = app.clone();
    let job2 = job.clone();
    std::thread::Builder::new()
        .name("doculens-model-download".into())
        .spawn(move || {
            let state = app2.state::<AppState>();
            let target = models_dir.join(&model.id);
            let total: u64 = model.files.iter().map(|f| f.size).sum();
            let agent = modelstore::download_agent();
            let mut done_before = 0u64;
            let mut outcome: std::result::Result<Option<String>, String> = Ok(None);
            for f in &model.files {
                let req = modelstore::DownloadRequest {
                    url: f.download_url().expect("checked"),
                    dest: target.join(&f.name),
                    expected_size: (f.size > 0).then_some(f.size),
                    expected_sha256: f.sha256.clone(),
                };
                if req.dest.is_file() {
                    done_before += f.size;
                    continue;
                }
                let name = f.name.clone();
                let r = modelstore::download_file(&agent, &req, &mut |done, file_total| {
                    let t = file_total.unwrap_or(f.size);
                    let overall = done_before + done;
                    let frac = if total > 0 { overall as f32 / total as f32 } else { 0.0 };
                    state.jobs.progress(
                        &job2,
                        Some(frac.min(1.0)),
                        Some(overall),
                        Some(total.max(done_before + t)),
                        Some(name.clone()),
                    );
                    !cancel.load(Ordering::Relaxed)
                });
                match r {
                    Ok(o) => done_before += o.bytes,
                    Err(e) => {
                        outcome = Err(e.to_string());
                        break;
                    }
                }
            }
            if outcome.is_ok() {
                let _ = std::fs::write(
                    target.join("SOURCE.txt"),
                    format!(
                        "{}\n{}\n",
                        model.source_repo.clone().unwrap_or_default(),
                        model.license_url.clone().unwrap_or_default()
                    ),
                );
                rescan(&app2);
                if state.engine.is_running() {
                    let _ = write_presets(&state).and_then(|_| state.engine.refresh_models(true));
                }
                outcome = Ok(Some(target.display().to_string()));
            }
            state.jobs.finish(&job2, outcome);
        })
        .ok();
    Ok(job)
}

/// Computes full hashes of a discovered model as a job.
pub fn hash_job(app: &AppHandle, folder: &str) -> UiResult<String> {
    let state = app.state::<AppState>();
    let index = state.model_index();
    let model: DiscoveredModel = index
        .discovered
        .iter()
        .find(|d| d.folder == std::path::Path::new(folder) || d.folder_name == folder)
        .cloned()
        .ok_or_else(|| UiError::invalid(format!("unknown model folder {folder}")))?;
    let models_dir =
        model.folder.parent().map(|p| p.to_path_buf()).ok_or_else(|| UiError::invalid("folder has no parent"))?;
    if state.jobs.is_active(JobKind::ModelHash, &model.folder_name) {
        return Err(UiError::invalid("hashing already running"));
    }
    let (job, cancel) = state.jobs.create(JobKind::ModelHash, &model.folder_name);
    let app2 = app.clone();
    let job2 = job.clone();
    std::thread::Builder::new()
        .name("doculens-model-hash".into())
        .spawn(move || {
            let state = app2.state::<AppState>();
            let total = model.total_bytes.max(1);
            let r = modelstore::hash_model(&models_dir, &model, &mut |file, done, file_total| {
                let frac = done as f32 / file_total.max(1) as f32;
                state.jobs.progress(&job2, Some(frac), Some(done), Some(total), Some(file.to_string()));
                !cancel.load(Ordering::Relaxed)
            });
            match r {
                Ok(hashes) => {
                    rescan(&app2);
                    let summary =
                        hashes.iter().map(|(n, h)| format!("{n}: {}", &h.sha256[..16])).collect::<Vec<_>>().join(", ");
                    state.jobs.finish(&job2, Ok(Some(summary)));
                }
                Err(e) => state.jobs.finish(&job2, Err(e.to_string())),
            }
        })
        .ok();
    Ok(job)
}
