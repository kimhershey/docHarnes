//! Glue between the core engine/model-store modules and the Tauri shell:
//! model folder scans + watcher, preset generation, engine start/auto-start,
//! event forwarding to the webview and background jobs (load / import /
//! download / hash) with progress and cancellation.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::Duration;

use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use doculens_core::engine::{
    devices, locate, presets, EngineBinary, EngineConfig, EngineEvent, EngineStatus, PresetBuild, PresetContext,
};
use doculens_core::hw::{self, Backend, HardwareSummary, TierAssessment};
use doculens_core::modelstore::{
    self, DiscoveredModel, ImportPlan, InstallState, ModelHealth, ModelWatcher, ScanOptions,
};
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
/// `llama-server --list-devices` of the Vulkan pack during the hardware probe (normally < 2 s).
const VULKAN_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

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

/// Rescan, and when the result changed regenerate the presets and reload the
/// router (if running). Files still being copied re-arm one more scan after
/// the settle window so freshly written models become loadable without a
/// manual rescan.
pub fn rescan_and_sync(app: &AppHandle) -> ModelIndex {
    let (index, changed) = rescan_inner(app);
    if index.discovered.iter().any(|d| matches!(d.health, ModelHealth::Copying)) {
        schedule_rescan(app, Duration::from_secs(modelstore::COPY_SETTLE_SECONDS + 1));
    }
    if changed {
        let state = app.state::<AppState>();
        log::info!("model folder changed: {} model(s)", index.discovered.len());
        if state.engine.is_running() {
            if let Err(e) = regenerate_and_reload(&state) {
                log::warn!("router reload after folder change failed: {e}");
            }
        }
    }
    index
}

fn schedule_rescan(app: &AppHandle, delay: Duration) {
    let app2 = app.clone();
    std::thread::Builder::new()
        .name("doculens-model-rescan".into())
        .spawn(move || {
            std::thread::sleep(delay);
            rescan_and_sync(&app2);
        })
        .ok();
}

fn on_models_dir_changed(app: &AppHandle) {
    rescan_and_sync(app);
}

/// Called after a workspace was opened/created/closed: rescan, (re)start the
/// folder watcher, refresh a running router's presets, and auto-start the
/// engine when configured.
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
    match (models_dir.is_some(), state.engine.is_running()) {
        (false, true) => {
            let _ = state.engine.stop();
        }
        (true, true) => {
            // Workspace switch: presets change, the router keeps unchanged instances (§3.3.2).
            if let Err(e) = regenerate_and_reload(&state) {
                log::warn!("preset refresh after workspace change failed: {e}");
            }
            if auto {
                let app2 = app.clone();
                std::thread::Builder::new()
                    .name("doculens-engine-autoload".into())
                    .spawn(move || load_active_model(&app2))
                    .ok();
            }
        }
        (true, false) if auto => {
            let app2 = app.clone();
            std::thread::Builder::new().name("doculens-engine-autostart".into()).spawn(move || auto_start(&app2)).ok();
        }
        _ => {}
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

/// GPU memory (MiB) the engine's loaded instances hold right now.
pub fn engine_vram_in_use_mib(state: &AppState) -> u64 {
    let index = state.model_index();
    let file_bytes = |id: &str| index.discovered.iter().find(|d| d.id == id).map(|d| d.total_bytes);
    state.engine.status().gpu_bytes_in_use(&file_bytes) / doculens_core::memory::MIB
}

/// The shell's hardware probe: the core probe plus the engine's own share of the VRAM at this
/// moment, so a probe taken while a model is loaded does not lower the tier (PLT-3). NVML only
/// knows NVIDIA cards: when it finds no GPU, the Vulkan pack (AppData or bundled) is asked for
/// its devices, so an AMD / Intel card selects the Vulkan backend and a GPU tier (PLT-4).
pub fn probe_hardware(state: &AppState) -> HardwareSummary {
    let mut hw = hw::probe();
    hw.engine_used_mib = engine_vram_in_use_mib(state);
    if hw.gpus.is_empty() {
        let roots = state.app().map(bundled_roots).unwrap_or_default();
        if let Some(pack) = locate::find_pack(&state.app_paths.engines_dir(), &roots, Backend::Vulkan) {
            match devices::probe_vulkan_gpus(&pack.path, VULKAN_PROBE_TIMEOUT) {
                Ok(gpus) => {
                    log::info!("hardware: the Vulkan pack lists {} GPU(s)", gpus.len());
                    hw.gpus = gpus;
                }
                Err(e) => log::warn!("hardware: asking the Vulkan pack for its devices failed: {e}"),
            }
        }
    }
    hw
}

pub fn assess(state: &AppState) -> UiResult<Assessment> {
    let hw = state.hardware()?;
    let settings = state.settings_snapshot()?;
    let tier = hw::assess(&hw, &settings.models);
    let registry = state.merged_registry();
    let index = state.model_index();
    let recommended = registry.default_model_for(tier.tier, tier.free_vram_mib).cloned();
    // "자동" runs the recommendation when it is installed, otherwise the closest installed model:
    // the free VRAM differs a little at every start and must not select a model that is not on
    // disk (every request would fail with "unknown model id").
    let auto = || {
        registry.auto_model(tier.tier, tier.free_vram_mib, &|m| modelstore::is_installed(m, &index.discovered)).cloned()
    };
    let active = if settings.models.active_model == "auto" {
        auto()
    } else {
        // An id that is neither built in nor a drop-in any more (0.3.0 Qwen entries after the 0.4.0
        // registry swap, a removed folder) falls back to what "자동" would run; the setting itself
        // is left alone so the explicit choice returns with its folder.
        registry.get(&settings.models.active_model).cloned().or_else(auto)
    };
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

/// Builds the preset file for the current backend/tier/profile from the
/// merged registry + scan (no side effects).
pub fn build_presets(state: &AppState) -> Result<PresetBuild> {
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
    Ok(presets::build_presets(&registry, &index.discovered, &ctx))
}

/// Generates `presets.ini` and writes it to the app-data directory. Writers
/// are serialised through `AppState::presets_lock`.
pub fn write_presets(state: &AppState) -> Result<PresetBuild> {
    let _guard = state.presets_lock.lock().unwrap_or_else(|e| e.into_inner());
    let build = build_presets(state)?;
    build.file.write(&state.app_paths.presets_file())?;
    Ok(build)
}

/// Regenerates the presets and asks a running router to re-read them
/// (`GET /models?reload=1`). One serialised "sync" used by every caller.
pub fn regenerate_and_reload(state: &AppState) -> Result<()> {
    let _guard = state.presets_lock.lock().unwrap_or_else(|e| e.into_inner());
    let build = build_presets(state)?;
    build.file.write(&state.app_paths.presets_file())?;
    if state.engine.is_running() {
        state.engine.refresh_models(true)?;
    }
    Ok(())
}

pub fn presets_preview(state: &AppState) -> Result<String> {
    build_presets(state).map(|b| b.file.to_ini())
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

fn engine_config(app: &AppHandle) -> Result<(EngineConfig, AppSettings)> {
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
    Ok((cfg, settings))
}

/// Starts the sidecar with freshly generated presets (no-op when running).
pub fn start_engine(app: &AppHandle) -> Result<EngineStatus> {
    ensure_forwarder(app);
    let state = app.state::<AppState>();
    if state.engine.is_running() {
        regenerate_and_reload(&state)?;
        return Ok(state.engine.status());
    }
    let (cfg, _) = engine_config(app)?;
    state.engine.start(cfg)
}

/// Stop + start with regenerated presets (restart counter kept).
pub fn restart_engine(app: &AppHandle) -> Result<EngineStatus> {
    ensure_forwarder(app);
    let state = app.state::<AppState>();
    let (cfg, _) = engine_config(app)?;
    state.engine.restart_with(cfg)
}

/// Engine start as a job + load of the active model, on a background thread.
fn auto_start(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.jobs.is_active(JobKind::EngineStart, "engine") {
        return;
    }
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
    load_active_model(app);
}

/// Loads the assessed active model when it is installed.
fn load_active_model(app: &AppHandle) {
    let state = app.state::<AppState>();
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

/// Runs a job body on a named thread; a panic inside the body fails the job
/// instead of leaving it "running" forever.
fn spawn_job_thread(
    name: &str,
    app: AppHandle,
    job: String,
    body: impl FnOnce(&AppHandle) -> std::result::Result<Option<String>, String> + Send + 'static,
) {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&app)));
            let state = app.state::<AppState>();
            match outcome {
                Ok(result) => state.jobs.finish(&job, result),
                Err(p) => {
                    let msg = p
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "panic".into());
                    log::error!("job {job} panicked: {msg}");
                    state.jobs.finish(&job, Err(format!("internal error: {msg}")));
                }
            }
        })
        .ok();
}

/// Loads a model as a job (progress from the router load stages; cancel
/// unloads the half-loaded instance). Returns the job id.
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
    let id = model_id.to_string();
    let job2 = job.clone();
    spawn_job_thread("doculens-model-load", app.clone(), job.clone(), move |app| {
        let state = app.state::<AppState>();
        state.jobs.update(&job2, |j| j.detail = Some("loading".into()));
        // Progress reporter: mirrors the router's load stages while the blocking load runs.
        let app3 = app.clone();
        let job3 = job2.clone();
        let id2 = id.clone();
        let reporter = std::thread::spawn(move || {
            let state = app3.state::<AppState>();
            loop {
                std::thread::sleep(Duration::from_millis(300));
                let Some(j) = state.jobs.get(&job3) else { break };
                if j.state.is_finished() {
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
            .load_model_cancellable(&id, LOAD_TIMEOUT, warmup, Some(&cancel))
            .map(|m| Some(format!("{} ({} bytes)", m.status, m.memory_effective_bytes.unwrap_or(0))))
            .map_err(|e| e.to_string());
        state.jobs.finish(&job2, result.clone());
        let _ = reporter.join();
        result
    });
    job
}

/// Executes an import plan as a job. Returns the job id.
pub fn import_job(app: &AppHandle, plan: ImportPlan) -> String {
    let state = app.state::<AppState>();
    let (job, cancel) = state.jobs.create(JobKind::ModelImport, &plan.folder_name);
    let job2 = job.clone();
    spawn_job_thread("doculens-model-import", app.clone(), job.clone(), move |app| {
        let state = app.state::<AppState>();
        state.jobs.update(&job2, |j| {
            j.bytes_total = Some(plan.total_bytes);
            j.detail = Some(format!("{:?}", plan.mode).to_lowercase());
        });
        let folder = modelstore::run_import(&plan, &mut |p| {
            let frac = if p.total > 0 { p.total_done as f32 / p.total as f32 } else { 0.0 };
            state.jobs.progress(&job2, Some(frac), Some(p.total_done), Some(p.total), Some(p.file.clone()));
            !cancel.load(Ordering::Relaxed)
        })
        .map_err(|e| e.to_string())?;
        rescan_and_sync(app);
        Ok(Some(folder.display().to_string()))
    });
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
    let job2 = job.clone();
    spawn_job_thread("doculens-model-download", app.clone(), job.clone(), move |app| {
        let state = app.state::<AppState>();
        let target = models_dir.join(&model.id);
        let total: u64 = model.files.iter().map(|f| f.size).sum();
        let agent = modelstore::download_agent();
        let mut done_before = 0u64;
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
            let outcome = modelstore::download_file(&agent, &req, &mut |done, file_total| {
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
            })
            .map_err(|e| e.to_string())?;
            done_before += outcome.bytes;
        }
        let _ = std::fs::write(
            target.join("SOURCE.txt"),
            format!(
                "{}\n{}\n",
                model.source_repo.clone().unwrap_or_default(),
                model.license_url.clone().unwrap_or_default()
            ),
        );
        rescan_and_sync(app);
        Ok(Some(target.display().to_string()))
    });
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
    let job2 = job.clone();
    spawn_job_thread("doculens-model-hash", app.clone(), job.clone(), move |app| {
        let state = app.state::<AppState>();
        let total = model.total_bytes.max(1);
        // Bytes of the files hashed before the current one (progress is cumulative).
        let sizes: Vec<(String, u64)> = model.all_files().map(|f| (f.name.clone(), f.size)).collect();
        let hashes = modelstore::hash_model(&models_dir, &model, &mut |file, done, _file_total| {
            let before: u64 = sizes.iter().take_while(|(n, _)| n != file).map(|(_, s)| *s).sum();
            let overall = before + done;
            state.jobs.progress(
                &job2,
                Some(overall as f32 / total as f32),
                Some(overall),
                Some(total),
                Some(file.to_string()),
            );
            !cancel.load(Ordering::Relaxed)
        })
        .map_err(|e| e.to_string())?;
        rescan_and_sync(app);
        let summary = hashes.iter().map(|(n, h)| format!("{n}: {}", &h.sha256[..16])).collect::<Vec<_>>().join(", ");
        Ok(Some(summary))
    });
    Ok(job)
}
