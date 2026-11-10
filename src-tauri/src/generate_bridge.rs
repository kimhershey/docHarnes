//! Document generation bridge (P6 §3.13): drafts run as persistent `draft`
//! jobs (the model call may take a minute; the 문서 생성 screen follows the job
//! and reads the `DraftOutcome` from the result), previews / validation /
//! saving run inline. Saved drafts live under `.doculens/drafts/`.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use doculens_core::generate::{
    self, store, Draft, DraftOutcome, DraftRequest, DraftSummary, GenerateDeps, SaveRequest, SavedDocument,
    TemplateInfo, TemplateKind,
};
use doculens_core::jobs::{JobControl, JobPriority, JobSpec};
use doculens_core::knowledge::embed::Embedder;
use doculens_core::knowledge::search::SearchDeps;
use doculens_core::pipeline::vlm::Vlm;

use crate::jobs::{Job, WorkerResult};
use crate::state::{AppState, UiError, UiResult};
use crate::{docs_bridge, knowledge_bridge, pipeline_bridge};

#[derive(Debug, Clone, Serialize)]
pub struct PreviewResponse {
    pub html: String,
    pub markdown: String,
}

pub fn templates() -> Vec<TemplateInfo> {
    generate::templates()
}

/// What the 문서 생성 screen needs to size a token budget (0.3.0 item 4).
#[derive(Debug, Clone, serde::Serialize)]
pub struct BudgetInfo {
    /// Context size of the loaded vision model, when known.
    pub ctx: Option<u32>,
    /// llama-server runs on CUDA / Vulkan (false = CPU only).
    pub gpu: bool,
    /// Reply token cap of the active backend (settings).
    pub cap: u32,
    /// Budgets above this are drafted section by section.
    pub split_threshold: u32,
}

pub fn budget_info(app: &AppHandle) -> UiResult<BudgetInfo> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot()?;
    let status = state.engine.status();
    let gpu = status.binary.as_ref().is_none_or(|b| b.backend != doculens_core::hw::Backend::Cpu);
    let ctx = status.active_model.as_deref().and_then(|m| status.model(m)).and_then(|m| m.ctx);
    Ok(BudgetInfo {
        ctx,
        gpu,
        cap: settings.generation.cap(gpu),
        split_threshold: generate::prompts::SPLIT_THRESHOLD_TOKENS,
    })
}

/// Queues a draft job; returns the job id (the result is a `DraftOutcome`).
pub fn draft(app: &AppHandle, mut req: DraftRequest, project_id: Option<String>, source: &str) -> UiResult<String> {
    if req.content.trim().is_empty() && req.previous.is_none() {
        return Err(UiError::invalid("content_empty"));
    }
    // Caps: the body as the validator allows it, sources and feedback as the prompt takes them.
    req.content = req.content.trim().chars().take(6_000).collect();
    for s in &mut req.sources {
        if let generate::SourceRef::Text { text, .. } = s {
            if text.chars().count() > 20_000 {
                *text = text.chars().take(20_000).collect();
            }
        }
    }
    if let Some(f) = req.feedback.as_mut() {
        if f.chars().count() > 2_000 {
            *f = f.chars().take(2_000).collect();
        }
    }
    let state = app.state::<AppState>();
    // The reply token budget is capped by the active backend (GPU 32K / CPU 8K by default, settings).
    if let Some(m) = req.max_tokens {
        let info = budget_info(app)?;
        req.max_tokens = Some(m.clamp(200, info.cap));
    }
    let pctx = pipeline_bridge::context(app)?; // a workspace must be open
    let label = draft_label(&req, pctx.db());
    state.jobs.submit(
        JobSpec::Draft { req: Box::new(req), project_id, source: source.to_string() },
        &label,
        JobPriority::High,
    )
}

fn draft_label(req: &DraftRequest, db: &doculens_core::db::Db) -> String {
    match generate::resolve_template(db, req) {
        Ok(generate::TemplateRef::Builtin { template }) => {
            let ko = req.language == doculens_core::settings::Language::Ko;
            generate::templates()
                .into_iter()
                .find(|t| t.kind == template)
                .map(|t| if ko { t.name_ko } else { t.name_en })
                .unwrap_or_else(|| template.as_str().to_string())
        }
        Ok(generate::TemplateRef::Form { schema_id, .. }) => schema_id,
        Err(_) => req.form_ref.clone().unwrap_or_else(|| "draft".into()),
    }
}

/// Whether a request can be filled without the model (보완요청서 from findings).
fn rule_only(req: &DraftRequest) -> bool {
    req.form_ref.as_deref().and_then(TemplateKind::parse) == Some(TemplateKind::SupplementRequest)
        && req.sources.iter().all(|s| matches!(s, generate::SourceRef::Text { .. }))
        && !req.sources.is_empty()
}

/// Worker for `JobSpec::Draft`.
pub fn run_draft(
    app: &AppHandle,
    job: &Job,
    req: &DraftRequest,
    project_id: Option<&str>,
    source: &str,
    ctl: &JobControl,
    pctx: &pipeline_bridge::PipelineContext,
) -> WorkerResult {
    let state = app.state::<AppState>();
    let mut model: Option<String> = None;
    let mut ctx_size = None;
    if !rule_only(req) {
        state.jobs.progress(&job.id, None, None, None, Some("loading model".into()));
        let m = pipeline_bridge::vision_model(app, true, Some(&ctl.cancel)).map_err(|e| (e.message, None))?;
        ctx_size = state.engine.status().model(&m).and_then(|x| x.ctx);
        model = Some(m);
    }
    let vlm = model
        .as_ref()
        .map(|m| Vlm::new(&state.engine, m.clone(), pctx.profile, Some(&ctl.cancel)).with_context(ctx_size));
    let analyzer = knowledge_bridge::analyzer(app);
    let embedder = knowledge_bridge::embedder(app, false).ok().flatten();
    let import = pctx.docs.deps();
    // Section-level progress of a split draft ("section 3/8", "outline", "finishing").
    let progress_cb = |frac: f32, detail: &str| {
        state.jobs.progress(&job.id, Some(frac), None, None, Some(detail.to_string()));
    };
    let deps = GenerateDeps {
        db: pctx.db(),
        paths: &pctx.docs.paths,
        vlm: vlm.as_ref(),
        knowledge: Some(SearchDeps {
            db: pctx.db(),
            analyzer: &analyzer,
            embedder: embedder.as_ref().map(|e| e as &dyn Embedder),
        }),
        printer: Some(pctx.docs.printer.as_ref()),
        import: Some(&import),
        cancel: Some(&ctl.cancel),
        language: req.language,
        progress: Some(&progress_cb),
    };
    // The model call holds the engine like any batch step (interactive requests cut in between).
    let _gate = state.gate.batch_begin();
    state.jobs.progress(&job.id, Some(0.2), None, None, Some("drafting".into()));
    let outcome: DraftOutcome = generate::draft(&deps, req).map_err(|e| (e.to_string(), None))?;
    // A split draft cancelled between sections keeps what was written: save it before reporting the cancel.
    let _ = store::save(&pctx.docs.paths, &outcome.draft);
    if ctl.cancelled() {
        return Err(("cancelled".into(), None));
    }
    let detail = if req.language == doculens_core::settings::Language::Ko {
        format!(
            "{} · 필드 {} · 검토 항목 {}",
            outcome.draft.title,
            outcome.draft.fields.len(),
            outcome.draft.issues.len()
        )
    } else {
        format!(
            "{} · {} fields · {} issues",
            outcome.draft.title,
            outcome.draft.fields.len(),
            outcome.draft.issues.len()
        )
    };
    let mut payload = serde_json::to_value(&outcome).map_err(|e| (e.to_string(), None))?;
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("project_id".into(), serde_json::json!(project_id));
        // `source` is the draft's own field (rules | model); the job origin lives under its own key.
        obj.insert("job_source".into(), serde_json::json!(source));
    }
    Ok((Some(detail), Some(payload)))
}

pub fn preview(draft: &Draft) -> PreviewResponse {
    PreviewResponse { html: generate::preview_html(draft), markdown: generate::to_markdown(draft) }
}

pub fn validate(mut draft: Draft) -> Draft {
    generate::validate(&mut draft);
    draft
}

/// Writes the file (+ library registration) inline: fast, and the user is waiting.
pub fn save(app: &AppHandle, req: &SaveRequest) -> UiResult<SavedDocument> {
    let pctx = pipeline_bridge::context(app)?;
    let import = pctx.docs.deps();
    let deps = GenerateDeps {
        db: pctx.db(),
        paths: &pctx.docs.paths,
        vlm: None,
        knowledge: None,
        printer: Some(pctx.docs.printer.as_ref()),
        import: Some(&import),
        cancel: None,
        language: req.draft.language,
        progress: None,
    };
    let saved = generate::save(&deps, req).map_err(UiError::from)?;
    if let Some(id) = saved.doc_id.clone() {
        docs_bridge::emit_changed(app, "imported", vec![id]);
    }
    Ok(saved)
}

pub fn drafts(app: &AppHandle) -> UiResult<Vec<DraftSummary>> {
    let pctx = pipeline_bridge::context(app)?;
    store::list(&pctx.docs.paths).map_err(UiError::from)
}

pub fn draft_get(app: &AppHandle, id: &str) -> UiResult<Draft> {
    let pctx = pipeline_bridge::context(app)?;
    store::load(&pctx.docs.paths, id).map_err(UiError::from)
}

pub fn draft_put(app: &AppHandle, draft: &Draft) -> UiResult<DraftSummary> {
    let pctx = pipeline_bridge::context(app)?;
    store::save(&pctx.docs.paths, draft).map_err(UiError::from)
}

pub fn draft_delete(app: &AppHandle, id: &str) -> UiResult<bool> {
    let pctx = pipeline_bridge::context(app)?;
    store::delete(&pctx.docs.paths, id).map_err(UiError::from)
}

/// Keeps the `Arc` import used on every platform (the printer handle).
#[allow(dead_code)]
fn _printer(pctx: &pipeline_bridge::PipelineContext) -> Arc<dyn doculens_core::documents::convert::PdfPrinter> {
    pctx.docs.printer.clone()
}
