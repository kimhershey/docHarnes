//! Knowledge-base wiring (P3): the morphological analyzer's lifetime, index /
//! embedding jobs, retrieval + answer streaming through the engine, and the
//! `knowledge://changed` / `knowledge://answer` events.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use doculens_core::db::Db;
use doculens_core::documents::{repo as docs_repo, DocumentFilter, DocumentRecord, DocumentSort};
use doculens_core::engine::RouterClient;
use doculens_core::knowledge::answer::{self, AnswerDeps};
use doculens_core::knowledge::chunker::ChunkOptions;
use doculens_core::knowledge::embed::{self, EmbedPrefixes, Embedder, RouterEmbedder};
use doculens_core::knowledge::index::{self, IndexDeps};
use doculens_core::knowledge::search::SearchDeps;
use doculens_core::knowledge::tokenize::{Analyzer, KODIC_DIR_NAME};
use doculens_core::knowledge::{Answer, AnswerKind, AnswerRequest, SessionState};
use doculens_core::projects::repo as projects_repo;
use doculens_core::registry::ModelRole;
use doculens_core::settings::KnowledgeSettings;

use crate::docs_bridge::{self, spawn_job_thread, AfterJob};
use crate::engine_bridge;
use crate::jobs::JobKind;
use crate::planner_bridge::UiPlanContext;
use crate::state::{AppState, UiError, UiResult};

pub const EV_KNOWLEDGE_CHANGED: &str = "knowledge://changed";
pub const EV_ANSWER: &str = "knowledge://answer";
const MODEL_LOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const EMBED_BATCH: usize = 16;

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeChanged {
    /// `indexed` | `embedded` | `collections` | `glossary` | `versions` | `answers` | `status`
    pub kind: &'static str,
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnswerDelta {
    pub request_id: String,
    pub delta: String,
    pub done: bool,
}

pub fn emit_changed(app: &AppHandle, kind: &'static str, ids: Vec<String>) {
    let _ = app.emit(EV_KNOWLEDGE_CHANGED, KnowledgeChanged { kind, ids });
}

// ---------------------------------------------------------------------------
// Analyzer
// ---------------------------------------------------------------------------

static ANALYZER: OnceLock<Arc<Analyzer>> = OnceLock::new();

fn kodic_candidates(app: &AppHandle) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        v.push(dir.join(KODIC_DIR_NAME));
    }
    let state = app.state::<AppState>();
    v.push(state.app_paths.tokenizers_dir().join(KODIC_DIR_NAME));
    v.push(state.app_paths.root.join(KODIC_DIR_NAME));
    v
}

/// Loads ko-dic once per process (falls back to the built-in analyzer).
/// Where the app expects the dictionary when it is not installed (shown to the user).
pub fn expected_kodic_dir(app: &AppHandle) -> PathBuf {
    app.state::<AppState>().app_paths.tokenizers_dir().join(KODIC_DIR_NAME)
}

pub fn analyzer(app: &AppHandle) -> Arc<Analyzer> {
    ANALYZER
        .get_or_init(|| {
            let candidates = Analyzer::candidates(&kodic_candidates(app));
            let a = Analyzer::load_first(&candidates);
            if a.kind() == doculens_core::knowledge::tokenize::AnalyzerKind::Simple {
                log::warn!(
                    "ko-dic dictionary not found (searched {} locations); using the simple tokenizer",
                    candidates.len()
                );
            }
            Arc::new(a)
        })
        .clone()
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

pub struct KnowledgeContext {
    pub db: Arc<Db>,
    pub analyzer: Arc<Analyzer>,
    pub settings: KnowledgeSettings,
    pub generation: u64,
}

impl KnowledgeContext {
    pub fn index_deps(&self) -> IndexDeps<'_> {
        IndexDeps {
            db: &self.db,
            analyzer: &self.analyzer,
            options: ChunkOptions::with_target(self.settings.chunk_target_tokens),
        }
    }
    pub fn search_deps<'a>(&'a self, embedder: Option<&'a dyn Embedder>) -> SearchDeps<'a> {
        SearchDeps { db: &self.db, analyzer: &self.analyzer, embedder }
    }
    pub fn is_current(&self, app: &AppHandle) -> bool {
        app.state::<AppState>().workspace_generation.load(Ordering::SeqCst) == self.generation
    }
    pub fn emit(&self, app: &AppHandle, kind: &'static str, ids: Vec<String>) {
        if self.is_current(app) {
            emit_changed(app, kind, ids);
        }
    }
}

pub fn context(app: &AppHandle) -> UiResult<KnowledgeContext> {
    let docs = docs_bridge::context(app)?;
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot()?.knowledge;
    Ok(KnowledgeContext { db: docs.db, analyzer: analyzer(app), settings, generation: docs.generation })
}

// ---------------------------------------------------------------------------
// Embedding model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Default)]
pub struct EmbeddingModelInfo {
    pub model: Option<String>,
    pub installed: bool,
    pub engine_running: bool,
    pub loaded: bool,
}

pub fn embedding_model(app: &AppHandle) -> EmbeddingModelInfo {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot().ok();
    let discovered = state.model_index().discovered;
    // A configured id that neither the registry nor the drop-in scan knows (e.g. the
    // 0.3.0 default `embed-bge-m3-q8` after the 0.4.0 registry change) is ignored and
    // the registry default is used instead.
    let known = |id: &str| state.registry.get(id).is_some() || discovered.iter().any(|d| d.id == id);
    let model = settings
        .as_ref()
        .and_then(|s| s.knowledge.embedding_model.clone())
        .filter(|id| known(id))
        .or_else(|| state.registry.default_embedding.clone());
    let Some(model) = model else { return EmbeddingModelInfo::default() };
    let installed = discovered.iter().any(|d| d.id == model && d.health.is_ready());
    let status = state.engine.status();
    let loaded = status.model(&model).is_some_and(|m| m.is_usable());
    EmbeddingModelInfo { model: Some(model), installed, engine_running: status.is_running(), loaded }
}

/// A usable embedder, or `None` when the model is not installed / not loaded.
/// With `load`, the engine is started and the model loaded (blocking; call
/// from a job thread).
pub fn embedder(app: &AppHandle, load: bool) -> UiResult<Option<RouterEmbedder>> {
    let info = embedding_model(app);
    let Some(model) = info.model.clone() else { return Ok(None) };
    if !info.installed {
        return Ok(None);
    }
    let state = app.state::<AppState>();
    if !info.engine_running {
        if !load {
            return Ok(None);
        }
        engine_bridge::start_engine(app)?;
    }
    if !info.loaded {
        if !load {
            return Ok(None);
        }
        state.engine.load_model(&model, MODEL_LOAD_TIMEOUT, false)?;
    }
    let client: RouterClient = state.engine.client()?;
    // Task prefixes the model expects (registry `defaults`; drop-ins may carry them in model.json).
    let prefixes =
        state.merged_registry().get(&model).map(|e| EmbedPrefixes::from_defaults(&e.defaults)).unwrap_or_default();
    Ok(Some(RouterEmbedder::new(client, model, prefixes)))
}

/// The chat (VLM) model answers are generated with: the engine's active model
/// unless that is the embedding model (loading the embedding model makes it
/// "active"), then the configured / assessed model, then any loaded
/// non-embedding model.
pub fn chat_model(app: &AppHandle) -> UiResult<String> {
    let state = app.state::<AppState>();
    let is_embedding = |id: &str| {
        state.registry.get(id).map(|e| e.role == ModelRole::Embedding).unwrap_or(false)
            || state
                .models
                .lock()
                .map(|m| m.discovered.iter().any(|d| d.id == id && d.role == ModelRole::Embedding))
                .unwrap_or(false)
    };
    let status = state.engine.status();
    if let Some(active) = status.active_model.as_deref() {
        if !is_embedding(active) && status.model(active).is_some_and(|m| m.is_usable()) {
            return Ok(active.to_string());
        }
    }
    if let Some(configured) = engine_bridge::active_model_id(&state)? {
        if !is_embedding(&configured) {
            return Ok(configured);
        }
    }
    if let Some(loaded) = status.loaded_models().map(|m| m.id.clone()).find(|id| !is_embedding(id)) {
        return Ok(loaded);
    }
    Err(UiError::invalid("no_active_model"))
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

/// Chunks documents whose text changed. Returns the job id, or `None` when a
/// run is already active (it will re-run afterwards) or no workspace is open.
pub fn index_job(app: &AppHandle, ids: Option<Vec<String>>, force: bool) -> UiResult<Option<String>> {
    let state = app.state::<AppState>();
    if state.switching.load(Ordering::SeqCst) {
        return Ok(None);
    }
    let ctx = context(app)?;
    let subject = match &ids {
        Some(ids) => format!("{} documents", ids.len()),
        None => "library".to_string(),
    };
    let Some((job, cancel)) = state.jobs.create_if_idle(JobKind::KnowledgeIndex, &subject) else {
        state.knowledge_dirty.store(true, Ordering::SeqCst);
        return Ok(None);
    };
    let job_id = job.clone();
    let auto_embed = ctx.settings.auto_embed;
    let generation = ctx.generation;
    spawn_job_thread(
        "doculens-kb-index",
        app.clone(),
        job.clone(),
        move |app| {
            let state = app.state::<AppState>();
            let deps = ctx.index_deps();
            let report = index::index_pending(&deps, ids.as_deref(), force, Some(&cancel), &mut |p| {
                let frac = if p.total == 0 { None } else { Some(p.done as f32 / p.total as f32) };
                state.jobs.progress(
                    &job_id,
                    frac,
                    Some(p.done as u64),
                    Some(p.total as u64),
                    Some(p.current.to_string()),
                );
            })
            .map_err(|e| e.to_string())?;
            if !report.indexed.is_empty() || !report.no_text.is_empty() {
                let mut ids = report.indexed.clone();
                ids.extend(report.no_text.iter().cloned());
                ctx.emit(app, "indexed", ids);
            } else {
                ctx.emit(app, "status", Vec::new());
            }
            if report.cancelled {
                return Err("cancelled".into());
            }
            let summary = format!(
                "indexed {} ({} chunks) / unchanged {} / no text {} / failed {}",
                report.indexed.len(),
                report.chunks,
                report.unchanged,
                report.no_text.len(),
                report.failed.len()
            );
            if !report.failed.is_empty() && report.indexed.is_empty() {
                return Err(report.failed.iter().map(|(id, e)| format!("{id}: {e}")).collect::<Vec<_>>().join("; "));
            }
            Ok(Some(summary))
        },
        Some(Box::new(move |app: &AppHandle| {
            let state = app.state::<AppState>();
            // Never chain into a workspace that is being closed / was switched meanwhile.
            if state.switching.load(Ordering::SeqCst) || state.workspace_generation.load(Ordering::SeqCst) != generation
            {
                return;
            }
            if state.knowledge_dirty.swap(false, Ordering::SeqCst) {
                let _ = index_job(app, None, false);
            } else if auto_embed {
                let _ = embed_job(app, false);
            }
        }) as AfterJob),
    );
    Ok(Some(job))
}

/// An embedding run was asked for while one was active. The active run embeds whatever is indexed while it
/// walks the chunks, but a request that arrives in its last moments would be lost: the run looks at this
/// flag when it ends and starts once more.
static EMBED_AGAIN: AtomicBool = AtomicBool::new(false);

/// Embeds chunks without vectors. `load_model` starts the engine / loads the
/// embedding model when needed (user action); automatic runs only use an
/// already loaded model.
pub fn embed_job(app: &AppHandle, load_model: bool) -> UiResult<Option<String>> {
    let state = app.state::<AppState>();
    if state.switching.load(Ordering::SeqCst) {
        return Ok(None);
    }
    if state.jobs.is_active_kind(&[JobKind::KnowledgeEmbed]) {
        EMBED_AGAIN.store(true, Ordering::SeqCst);
        return Ok(None);
    }
    let ctx = context(app)?;
    let info = embedding_model(app);
    let Some(model) = info.model.clone() else { return Ok(None) };
    if !info.installed {
        if load_model {
            return Err(UiError::invalid("embedding_model_not_installed"));
        }
        return Ok(None);
    }
    let pending = ctx.db.with(embed::pending_count)?;
    let stale = ctx.db.with(|c| embed::embedding_info(c).map(|i| i.model.is_some_and(|m| m != model)))?;
    if pending == 0 && !stale {
        return Ok(None);
    }
    // Automatic runs never start the engine, but they do load the (CPU) embedding
    // model into a running engine so background embedding actually happens.
    if !load_model && !info.engine_running {
        return Ok(None);
    }
    let load_model = load_model || !info.loaded;
    let Some((job, cancel)) = state.jobs.create_if_idle(JobKind::KnowledgeEmbed, &model) else {
        EMBED_AGAIN.store(true, Ordering::SeqCst);
        return Ok(None);
    };
    // This run covers everything pending now, earlier requests included.
    EMBED_AGAIN.store(false, Ordering::SeqCst);
    let job_id = job.clone();
    let generation = ctx.generation;
    spawn_job_thread(
        "doculens-kb-embed",
        app.clone(),
        job.clone(),
        move |app| {
            let state = app.state::<AppState>();
            state.jobs.update(&job_id, |j| j.detail = Some("loading model".into()));
            let embedder = embedder(app, load_model).map_err(|e| e.message)?.ok_or("embedding model unavailable")?;
            let report = embed::embed_pending(&ctx.db, &embedder, EMBED_BATCH, Some(&cancel), &mut |done, total| {
                let frac = if total == 0 { None } else { Some(done as f32 / total as f32) };
                state.jobs.progress(&job_id, frac, Some(done), Some(total), Some(format!("{done}/{total}")));
            })
            .map_err(|e| e.to_string())?;
            ctx.emit(app, "embedded", Vec::new());
            if report.cancelled {
                return Err("cancelled".into());
            }
            Ok(Some(format!(
                "embedded {} / failed {} / remaining {}{}",
                report.embedded,
                report.failed,
                report.remaining,
                if report.reset { " (rebuilt for a new model)" } else { "" }
            )))
        },
        Some(Box::new(move |app: &AppHandle| {
            // A request skipped while this run was active (documents indexed as it finished): run once more.
            // Never chain into a workspace that is being closed / was switched meanwhile.
            let state = app.state::<AppState>();
            let again = EMBED_AGAIN.swap(false, Ordering::SeqCst);
            if again
                && !state.switching.load(Ordering::SeqCst)
                && state.workspace_generation.load(Ordering::SeqCst) == generation
            {
                let _ = embed_job(app, false);
            }
        }) as AfterJob),
    );
    Ok(Some(job))
}

/// Called after document jobs: chunk whatever changed (when enabled).
pub fn auto_index(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.switching.load(Ordering::SeqCst) {
        return;
    }
    let enabled = state.settings_snapshot().map(|s| s.knowledge.auto_index).unwrap_or(true);
    if enabled {
        match index_job(app, None, false) {
            Ok(_) => {}
            Err(e) if e.code == "no_workspace" => {}
            Err(e) => log::warn!("auto index not started: {}", e.message),
        }
    }
}

pub fn on_workspace_changed(app: &AppHandle) {
    let app2 = app.clone();
    // Loading the dictionary (mmap) and the first index pass run off the UI thread.
    std::thread::Builder::new()
        .name("doculens-kb-init".into())
        .spawn(move || {
            let _ = analyzer(&app2);
            auto_index(&app2);
        })
        .ok();
}

// ---------------------------------------------------------------------------
// Answers
// ---------------------------------------------------------------------------

/// Runs an answer / explanation / summary / comparison, streaming deltas as
/// `knowledge://answer` events tagged with `request_id`.
/// What the UI is showing, with names instead of ids for the model: the open document, the
/// library selection, the folder, the open project and the latest imports.
fn session_state(ctx: &KnowledgeContext, ui: &UiPlanContext) -> UiResult<SessionState> {
    fn title(d: &DocumentRecord) -> String {
        d.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| d.original_name.clone())
    }
    let state = ctx.db.with_read(|c| {
        let mut s = SessionState { folder: ui.folder.clone(), current_page: ui.current_page, ..Default::default() };
        if let Some(id) = &ui.current_document {
            s.current_document = docs_repo::get_document(c, id)?.map(|d| title(&d));
        }
        for id in ui.selected_doc_ids.iter().take(5) {
            if let Some(d) = docs_repo::get_document(c, id)? {
                s.selected_documents.push(title(&d));
            }
        }
        if let Some(pid) = &ui.current_project {
            s.project = projects_repo::get_project(c, pid)?.map(|p| p.name);
        }
        let recent = docs_repo::list_documents(
            c,
            &DocumentFilter { sort: DocumentSort::ImportedDesc, limit: Some(3), ..Default::default() },
        )?;
        s.recent_documents = recent.iter().map(title).collect();
        Ok(s)
    })?;
    Ok(state)
}

pub fn answer(app: &AppHandle, req: AnswerRequest, request_id: String, ui: Option<UiPlanContext>) -> UiResult<Answer> {
    let ctx = context(app)?;
    let state = app.state::<AppState>();
    let model = chat_model(app)?;
    let embedder = embedder(app, false)?;
    let mut req = req;
    if req.min_similarity.is_none() {
        req.min_similarity = Some(ctx.settings.min_vector_similarity);
    }
    if req.session.is_none() {
        if let Some(ui) = ui.as_ref() {
            match session_state(&ctx, ui) {
                Ok(s) => req.session = Some(s),
                Err(e) => log::warn!("session context for the answer: {}", e.message),
            }
        }
    }
    if let Ok(mut r) = state.answer_running.lock() {
        r.insert(request_id.clone());
    }
    // The loaded model's context caps the evidence budget so the prompt always fits the slot.
    let context_size = state.engine.status().model(&model).and_then(|m| m.ctx);
    // "중지" (and a workspace switch) must take effect at once — also while the request queues behind a batch
    // job and while the engine evaluates the prompt, where the model call cannot be interrupted. The request
    // runs on a thread of its own: this one returns `cancelled` as soon as the cancel is there, and the
    // abandoned request ends at its next cancel point (the first streamed token at the latest).
    let stop = Arc::new(AtomicBool::new(false));
    let run = AnswerRun {
        slot: AnswerSlot { app: app.clone(), request_id: request_id.clone() },
        ctx,
        embedder,
        model,
        req,
        context_size,
        stop: stop.clone(),
    };
    let cancelled = || state.answer_cancels.lock().map(|c| c.contains(&request_id)).unwrap_or(false);
    let result = answer::wait_or_abandon("doculens-answer", &stop, &cancelled, move || run.run());
    let _ = app.emit(EV_ANSWER, AnswerDelta { request_id: request_id.clone(), delta: String::new(), done: true });
    Ok(result?)
}

/// A request in `answer_running`: leaves it (and the cancel set) when the run ends — also when the run never
/// starts or panics — so a workspace switch does not wait for a request that is gone.
struct AnswerSlot {
    app: AppHandle,
    request_id: String,
}

impl Drop for AnswerSlot {
    fn drop(&mut self) {
        let state = self.app.state::<AppState>();
        if let Ok(mut c) = state.answer_cancels.lock() {
            c.remove(&self.request_id);
        }
        if let Ok(mut r) = state.answer_running.lock() {
            r.remove(&self.request_id);
        }; // the guard's temporary must go before `state`
    }
}

/// One answer request with everything it needs, so it can run on its own thread.
struct AnswerRun {
    slot: AnswerSlot,
    ctx: KnowledgeContext,
    embedder: Option<RouterEmbedder>,
    model: String,
    req: AnswerRequest,
    context_size: Option<u32>,
    /// Raised by the waiting command when it gives up on this run.
    stop: Arc<AtomicBool>,
}

impl AnswerRun {
    fn run(self) -> doculens_core::Result<Answer> {
        let app = &self.slot.app;
        let request_id = &self.slot.request_id;
        let state = app.state::<AppState>();
        let cancelled = || {
            self.stop.load(Ordering::SeqCst)
                || state.answer_cancels.lock().map(|c| c.contains(request_id)).unwrap_or(false)
        };
        let deps = AnswerDeps {
            search: self.ctx.search_deps(self.embedder.as_ref().map(|e| e as &dyn Embedder)),
            chat: &state.engine,
            model: Some(self.model.clone()),
            cancelled: Some(&cancelled),
            context_size: self.context_size,
        };
        let mut on_delta = |d: &str| {
            if cancelled() {
                return false;
            }
            // An empty fragment only asks whether to go on (a streamed event without visible text).
            if !d.is_empty() {
                let _ = app.emit(
                    EV_ANSWER,
                    AnswerDelta { request_id: request_id.clone(), delta: d.to_string(), done: false },
                );
            }
            true
        };
        // Interactive request: a running batch yields at its next page boundary (§3.9); a request that was
        // cancelled meanwhile stops waiting for it.
        let out = state.gate.interactive_cancellable(Some(self.stop.as_ref()), || match self.req.kind {
            AnswerKind::Answer | AnswerKind::Explain => answer::answer(&deps, &self.req, &mut on_delta),
            AnswerKind::Summary => answer::summarize(&deps, &self.req, &mut on_delta),
            AnswerKind::Compare => answer::compare(&deps, &self.req, &mut on_delta),
        })??;
        if out.id.is_some() {
            self.ctx.emit(app, "answers", Vec::new());
        }
        Ok(out)
    }
}

/// Stops the answer tagged `request_id`: its command returns `cancelled` at once (the request itself ends at
/// its next cancel point).
pub fn cancel_answer(app: &AppHandle, request_id: &str) -> bool {
    let state = app.state::<AppState>();
    let running = state.answer_running.lock().map(|r| r.contains(request_id)).unwrap_or(false);
    if !running {
        return false; // finished (or unknown) requests leave nothing behind
    }
    state.answer_cancels.lock().map(|mut c| c.insert(request_id.to_string())).unwrap_or(false)
}
