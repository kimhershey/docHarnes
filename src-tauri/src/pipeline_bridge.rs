//! Pipeline wiring (P4): vision-model resolution, processing / split / merge
//! jobs, the auto-processing hook after imports, form registration and
//! natural-language schema calls, and the `pipeline://changed` event.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use doculens_core::db::Db;
use doculens_core::documents::import::ImportSource;
use doculens_core::jobs::{JobPriority, JobSpec};
use doculens_core::pipeline::schema::SchemaDraft;
use doculens_core::pipeline::split as splitter;
use doculens_core::pipeline::image::VisionGeometry;
use doculens_core::pipeline::vlm::Vlm;
use doculens_core::pipeline::{forms, repo, PipelineLevel, ProcessRequest};
use doculens_core::settings::{AutoProcessingLevel, ProcessingSettings, Profile};

use crate::docs_bridge::{self, spawn_job_thread, AfterJob, DocsContext};
use crate::engine_bridge;
use crate::jobs::JobKind;
use crate::knowledge_bridge;
use crate::state::{AppState, UiError, UiResult};

pub const EV_PIPELINE_CHANGED: &str = "pipeline://changed";
const MODEL_LOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Serialize)]
pub struct PipelineChanged {
    /// `processed` | `fields` | `schemas` | `review` | `status` | `engine_off` | `no_model`
    pub kind: &'static str,
    pub ids: Vec<String>,
}

pub fn emit_changed(app: &AppHandle, kind: &'static str, ids: Vec<String>) {
    let _ = app.emit(EV_PIPELINE_CHANGED, PipelineChanged { kind, ids });
}

pub struct PipelineContext {
    pub docs: DocsContext,
    pub settings: ProcessingSettings,
    pub profile: Profile,
    pub pii_masking: bool,
}

impl PipelineContext {
    pub fn db(&self) -> &Arc<Db> {
        &self.docs.db
    }
    pub fn emit(&self, app: &AppHandle, kind: &'static str, ids: Vec<String>) {
        if self.docs.is_current(app) {
            emit_changed(app, kind, ids);
        }
    }
}

pub fn context(app: &AppHandle) -> UiResult<PipelineContext> {
    let docs = docs_bridge::context(app)?;
    let settings = app.state::<AppState>().settings_snapshot()?;
    Ok(PipelineContext {
        docs,
        settings: settings.processing.clone(),
        profile: settings.models.profile,
        pii_masking: settings.privacy.pii_masking,
    })
}

// ---------------------------------------------------------------------------
// Vision model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Default)]
pub struct VisionModelInfo {
    pub model: Option<String>,
    pub engine_running: bool,
    pub loaded: bool,
}

/// The chat/vision model the pipeline would use and whether it is ready.
pub fn vision_model_info(app: &AppHandle) -> VisionModelInfo {
    let state = app.state::<AppState>();
    let status = state.engine.status();
    let model = knowledge_bridge::chat_model(app).ok();
    let loaded = model.as_deref().is_some_and(|m| status.model(m).is_some_and(|i| i.is_usable()));
    VisionModelInfo { model, engine_running: status.is_running(), loaded }
}

/// Context size the router reports for a loaded model (prompt budgeting).
fn model_context(state: &AppState, model: &str) -> Option<u32> {
    state.engine.status().model(model).and_then(|m| m.ctx)
}

/// Vision-encoder geometry of a model (page render budgets): from its registry /
/// drop-in architecture, the shipped family when unknown.
pub fn vision_geometry(state: &AppState, model: &str) -> VisionGeometry {
    state.merged_registry().get(model).map(|e| VisionGeometry::for_arch(&e.arch)).unwrap_or_default()
}

/// Resolves the vision model. With `load`, starts the engine / loads the
/// model when needed (blocking — job threads and explicit user actions only;
/// `cancel` aborts a pending load).
pub fn vision_model(app: &AppHandle, load: bool, cancel: Option<&AtomicBool>) -> UiResult<String> {
    let state = app.state::<AppState>();
    let model = knowledge_bridge::chat_model(app)?;
    if !state.engine.is_running() {
        if !load {
            return Err(UiError::invalid("engine_not_running"));
        }
        engine_bridge::start_engine(app)?;
    }
    let loaded = state.engine.status().model(&model).is_some_and(|m| m.is_usable());
    if !loaded {
        if !load {
            return Err(UiError::invalid("model_not_loaded"));
        }
        // The router's presets may predate the drop-in folder scan (startup race):
        // rescan the folder, regenerate the presets and reload the model list first.
        engine_bridge::rescan_and_sync(app);
        engine_bridge::start_engine(app)?;
        state.engine.load_model_cancellable(&model, MODEL_LOAD_TIMEOUT, false, cancel)?;
    }
    Ok(model)
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

/// Queues a processing request as a persistent job (engine lane: one at a time,
/// priority order, pausable, restored after a restart). Returns the job id.
pub fn process_job(app: &AppHandle, req: ProcessRequest) -> UiResult<Option<String>> {
    let state = app.state::<AppState>();
    if state.switching.load(Ordering::SeqCst) {
        return Ok(None);
    }
    if req.document_ids.is_empty() {
        return Err(UiError::invalid("no documents"));
    }
    let subject = format!("{} documents", req.document_ids.len());
    let id = state.jobs.submit(JobSpec::Pipeline { req }, &subject, JobPriority::Normal)?;
    Ok(Some(id))
}

/// Runs the configured auto pipeline on freshly imported documents (§3.4
/// `auto_pipeline`): drop/folder → `processing.auto_level`, inbox →
/// `processing.inbox_auto_level`, or the level chosen in the import dialog.
pub fn auto_process(app: &AppHandle, ids: Vec<String>, level: Option<AutoProcessingLevel>, source: ImportSource) {
    if ids.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    if state.switching.load(Ordering::SeqCst) {
        return;
    }
    let Ok(settings) = state.settings_snapshot() else { return };
    let level = level.unwrap_or(match source {
        ImportSource::Inbox => settings.processing.inbox_auto_level,
        _ => settings.processing.auto_level,
    });
    let Some(level): Option<PipelineLevel> = level.into() else { return };
    if knowledge_bridge::chat_model(app).is_err() {
        log::info!("auto pipeline skipped: no vision model configured");
        emit_changed(app, "no_model", ids);
        return;
    }
    if !state.engine.is_running() && !settings.models.auto_start_engine {
        log::info!("auto pipeline skipped: engine not running and auto start is off");
        emit_changed(app, "engine_off", ids);
        return;
    }
    match process_job(app, ProcessRequest { document_ids: ids, level, ..Default::default() }) {
        Ok(_) => {}
        Err(e) if e.code == "no_workspace" => {}
        Err(e) => log::warn!("auto pipeline not started: {}", e.message),
    }
}

/// Splits a document at page boundaries (job; the parts are rendered and auto-processed).
pub fn split_job(app: &AppHandle, doc_id: String, boundaries: Vec<u32>) -> UiResult<String> {
    let state = app.state::<AppState>();
    let ctx = context(app)?;
    let doc = docs_bridge::document(app, &doc_id)?;
    if boundaries.iter().all(|b| *b <= 1 || *b > doc.page_count) {
        return Err(UiError::invalid("no split boundary inside the document"));
    }
    let (job, cancel) = state.jobs.create(JobKind::DocumentSplit, &doc.original_name);
    let generation = ctx.docs.generation;
    let new_ids: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let new_ids_after = new_ids.clone();
    spawn_job_thread(
        "doculens-doc-split",
        app.clone(),
        job.clone(),
        move |app| {
            let deps = ctx.docs.deps();
            let ids = splitter::split_document(&deps, &doc, &boundaries, Some(&cancel)).map_err(|e| e.to_string())?;
            if let Ok(mut n) = new_ids.lock() {
                *n = ids.clone();
            }
            docs_bridge::emit_changed(app, "imported", ids.clone());
            docs_bridge::emit_changed(app, "updated", vec![doc.id.clone()]);
            Ok(Some(format!("split into {}", ids.len())))
        },
        Some(Box::new(move |app: &AppHandle| {
            let state = app.state::<AppState>();
            if state.switching.load(Ordering::SeqCst) || state.workspace_generation.load(Ordering::SeqCst) != generation
            {
                return;
            }
            knowledge_bridge::auto_index(app);
            let ids = new_ids_after.lock().map(|n| n.clone()).unwrap_or_default();
            auto_process(app, ids, None, ImportSource::Drop);
        }) as AfterJob),
    );
    Ok(job)
}

/// Merges documents into one PDF (job).
pub fn merge_job(app: &AppHandle, ids: Vec<String>, name: Option<String>) -> UiResult<String> {
    let state = app.state::<AppState>();
    let ctx = context(app)?;
    let mut documents = Vec::new();
    for id in &ids {
        documents.push(docs_bridge::document(app, id)?);
    }
    if documents.len() < 2 {
        return Err(UiError::invalid("merge needs at least two documents"));
    }
    if let Some(n) = name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        splitter::safe_stem(n).map_err(|_| UiError::invalid("merge_name_invalid"))?;
    }
    let (job, cancel) = state.jobs.create(JobKind::DocumentMerge, &format!("{} documents", documents.len()));
    let generation = ctx.docs.generation;
    let merged_id: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let merged_after = merged_id.clone();
    spawn_job_thread(
        "doculens-doc-merge",
        app.clone(),
        job.clone(),
        move |app| {
            let deps = ctx.docs.deps();
            let id = splitter::merge_documents(&deps, &documents, name.as_deref(), Some(&cancel))
                .map_err(|e| e.to_string())?;
            if let Ok(mut m) = merged_id.lock() {
                *m = Some(id.clone());
            }
            docs_bridge::emit_changed(app, "imported", vec![id.clone()]);
            Ok(Some(format!("merged {}", documents.len())))
        },
        Some(Box::new(move |app: &AppHandle| {
            let state = app.state::<AppState>();
            if state.switching.load(Ordering::SeqCst) || state.workspace_generation.load(Ordering::SeqCst) != generation
            {
                return;
            }
            knowledge_bridge::auto_index(app);
            if let Some(id) = merged_after.lock().ok().and_then(|m| m.clone()) {
                auto_process(app, vec![id], None, ImportSource::Drop);
            }
        }) as AfterJob),
    );
    Ok(job)
}

// ---------------------------------------------------------------------------
// Forms / natural-language schemas (blocking commands)
// ---------------------------------------------------------------------------

/// Derives the field list of a form document. Uses the vision model when it is
/// already loaded; otherwise office formats are read structurally and PDFs /
/// images fall back to text heuristics.
pub fn form_derive(app: &AppHandle, doc_id: &str, name: Option<&str>) -> UiResult<SchemaDraft> {
    let ctx = context(app)?;
    let doc = docs_bridge::document(app, doc_id)?;
    let state = app.state::<AppState>();
    let model = vision_model(app, false, None).ok();
    let vlm = model.map(|m| {
        let c = model_context(&state, &m);
        let g = vision_geometry(&state, &m);
        Vlm::new(&state.engine, m, ctx.profile, None).with_context(c).with_geometry(g)
    });
    state.gate.interactive(|| Ok(forms::derive(&ctx.docs.deps(), vlm.as_ref(), &doc, name)?))
}

/// Queues a 0.3.0 §2 field discovery over the sample documents (one unsaved draft for the
/// editor); returns the job id. Office forms need no model; scanned / PDF samples load it.
pub fn schema_discovery_job(app: &AppHandle, doc_ids: Vec<String>, name_hint: Option<String>) -> UiResult<String> {
    let state = app.state::<AppState>();
    let mut ids: Vec<String> = Vec::new();
    for id in doc_ids {
        let id = id.trim().to_string();
        if !id.is_empty() && !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Err(UiError::invalid("no documents"));
    }
    let _ctx = context(app)?; // a workspace must be open
    let hint = name_hint.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let subject = hint.clone().unwrap_or_else(|| format!("{} documents", ids.len()));
    state.jobs.submit(JobSpec::SchemaDiscovery { doc_ids: ids, name_hint: hint }, &subject, JobPriority::High)
}

/// Worker for `JobSpec::SchemaDiscovery`: loads the vision model when a scanned / PDF sample needs
/// it (a failed load falls back to the text heuristics), reads each sample page by page and merges
/// the drafts (`forms::discover`). The payload is the `DiscoveryOutcome` the editor opens.
pub fn run_schema_discovery(
    app: &AppHandle,
    job: &crate::jobs::Job,
    doc_ids: &[String],
    name_hint: Option<&str>,
    ctl: &doculens_core::jobs::JobControl,
    pctx: &PipelineContext,
) -> crate::jobs::WorkerResult {
    let state = app.state::<AppState>();
    let mut documents = Vec::new();
    for id in doc_ids {
        match pctx.db().with(|c| doculens_core::documents::repo::get_document(c, id)) {
            Ok(Some(d)) => documents.push(d),
            Ok(None) => log::warn!("schema discovery: document {id} not found"),
            Err(e) => return Err((e.to_string(), None)),
        }
    }
    if documents.is_empty() {
        return Err(("document_not_found".into(), None));
    }
    let needs_model = documents.iter().any(|d| !forms::structural_kind(d.source_kind));
    let mut model: Option<String> = None;
    let mut ctx_size = None;
    if needs_model {
        state.jobs.progress(&job.id, None, None, None, Some("loading model".into()));
        match vision_model(app, true, Some(&ctl.cancel)) {
            Ok(m) => {
                ctx_size = model_context(&state, &m);
                model = Some(m);
            }
            Err(_) if ctl.cancelled() => return Err(("cancelled".into(), None)),
            Err(e) => log::warn!("schema discovery without the vision model ({}): text heuristics only", e.message),
        }
    }
    let vlm = model
        .as_ref()
        .map(|m| {
            Vlm::new(&state.engine, m.clone(), pctx.profile, Some(&ctl.cancel))
                .with_context(ctx_size)
                .with_geometry(vision_geometry(&state, m))
        });
    let deps = pctx.docs.deps();
    let total = documents.len() as u64;
    let lease = state.gate.batch_begin();
    let outcome = forms::discover(&deps, vlm.as_ref(), &documents, name_hint, Some(&ctl.cancel), &mut |p| {
        // Document / page boundary: pause, let interactive requests in (a cancel surfaces through the flag).
        let _ = state.gate.checkpoint(ctl);
        let page_part = p.page.map(|(d, t)| if t == 0 { 0.0 } else { d as f32 / t as f32 }).unwrap_or(0.0);
        let frac = ((p.index as f32 + page_part * 0.9) / p.total.max(1) as f32).min(1.0);
        let detail = match p.page {
            Some((d, t)) => format!("{} · discover ({d}/{t})", p.title),
            None => format!("{} · discover", p.title),
        };
        state.jobs.progress(&job.id, Some(frac), Some(p.index as u64), Some(total), Some(detail));
    });
    drop(lease);
    let outcome = match outcome {
        Ok(o) => o,
        Err(doculens_core::CoreError::Cancelled) => return Err(("cancelled".into(), None)),
        Err(e) => return Err((e.to_string(), None)),
    };
    let read = outcome.documents.iter().filter(|d| d.source != "failed").count();
    let detail = format!("merged {} fields from {} documents", outcome.merged_fields, read);
    let payload = serde_json::to_value(&outcome).map_err(|e| (e.to_string(), None))?;
    Ok((Some(detail), Some(payload)))
}

/// Creates or modifies a schema from a description (loads the model if needed).
pub fn nl_schema(app: &AppHandle, description: &str, base_id: Option<&str>) -> UiResult<SchemaDraft> {
    let ctx = context(app)?;
    let base = match base_id {
        Some(id) => {
            Some(ctx.db().with(|c| repo::get_schema(c, id))?.ok_or_else(|| UiError::invalid("schema_not_found"))?)
        }
        None => None,
    };
    let state = app.state::<AppState>();
    let model = vision_model(app, true, None)?;
    let c = model_context(&state, &model);
    let g = vision_geometry(&state, &model);
    let vlm = Vlm::new(&state.engine, model, ctx.profile, None).with_context(c).with_geometry(g);
    state.gate.interactive(|| Ok(forms::nl_schema(&vlm, description, base.as_ref())?))
}

/// Workspace opened: seed the sample forms, fail runs interrupted by a crash,
/// bring the persistent job queue back.
pub fn on_workspace_changed(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(ctx) = context(app) else { return };
    state.jobs.restore_from_workspace();
    let app2 = app.clone();
    std::thread::Builder::new()
        .name("doculens-pipeline-init".into())
        .spawn(move || {
            match ctx.db().with(|c| {
                let added = repo::ensure_builtins(c)?;
                let stale = repo::fail_stale_runs(c)?;
                Ok((added, stale))
            }) {
                Ok((added, stale)) => {
                    if added > 0 || stale > 0 {
                        log::info!("pipeline init: {added} sample forms added, {stale} interrupted runs closed");
                        ctx.emit(&app2, "schemas", Vec::new());
                    }
                }
                Err(e) => log::warn!("pipeline init failed: {e}"),
            }
        })
        .ok();
}
