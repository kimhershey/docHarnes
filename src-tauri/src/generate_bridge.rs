//! Document generation bridge (P6 §3.13): drafts run as persistent `draft`
//! jobs (the model call may take a minute; the 문서 생성 screen follows the job
//! and reads the `DraftOutcome` from the result), previews / validation /
//! saving run inline. Saved drafts live under `.doculens/drafts/`.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use doculens_core::generate::{
    self, store, Draft, DraftOutcome, DraftRequest, DraftSummary, GenerateDeps, SaveRequest, SavedDocument,
    TemplateInfo,
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
    /// Characters of the content (`DraftRequest.content`) and of a revision instruction
    /// (`DraftRequest.feedback`) a prompt takes at most. A small context takes less; the draft says
    /// what was cut (`DraftOutcome.input_cuts` and its notes).
    pub content_max_chars: u32,
    pub instruction_max_chars: u32,
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
        content_max_chars: generate::prompts::MAX_CONTENT_CHARS as u32,
        instruction_max_chars: generate::prompts::MAX_FEEDBACK_CHARS as u32,
    })
}

/// Queues a draft job; returns the job id (the result is a `DraftOutcome`).
pub fn draft(app: &AppHandle, mut req: DraftRequest, project_id: Option<String>, source: &str) -> UiResult<String> {
    if req.content.trim().is_empty() && req.previous.is_none() {
        return Err(UiError::invalid("content_empty"));
    }
    // 0.7.0: a revision needs the draft it edits and an instruction.
    if req.revise.is_some() {
        if req.previous.is_none() {
            return Err(UiError::invalid("revise_previous_missing"));
        }
        if req.feedback.as_deref().map(str::trim).unwrap_or("").is_empty() {
            return Err(UiError::invalid("revise_instruction_empty"));
        }
    }
    // Only the size stored with the job is bounded here. What a prompt takes of the content and the
    // instruction is the core's decision (`prompts::MAX_CONTENT_CHARS` / `MAX_FEEDBACK_CHARS`, less on a
    // small context), and the core reports a cut in the draft — cutting to those limits here as well
    // would hide it.
    req.bound_stored_text();
    // A draft that belongs to a project searches for evidence inside it, and its result links to it.
    let project_id = req.bind_project(project_id);
    let state = app.state::<AppState>();
    // The reply token budget is capped by the active backend (GPU 32K / CPU 8K by default, settings).
    if let Some(m) = req.max_tokens {
        let info = budget_info(app)?;
        req.max_tokens = Some(m.clamp(200, info.cap));
    }
    let pctx = pipeline_bridge::context(app)?; // a workspace must be open
    let mut label = draft_label(&req, pctx.db());
    if req.revise.is_some() {
        // 0.7.0: a revision job is listed under the draft's title ("… · AI 수정").
        let title = req.previous.as_deref().map(|d| d.title.trim().to_string()).filter(|t| !t.is_empty());
        let ko = req.language == doculens_core::settings::Language::Ko;
        label = format!("{} · {}", title.unwrap_or(label), if ko { "AI 수정" } else { "AI revision" });
    }
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

/// Whether a request can be filled without the model (a fresh 보완요청서 from findings). A request
/// with `revise`, `feedback` or `previous` — the 초안 어시스턴트 on such a draft — is not: it takes
/// the model path like any other draft.
fn rule_only(req: &DraftRequest) -> bool {
    generate::draft::rule_only(req)
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
    let ko = req.language == doculens_core::settings::Language::Ko;
    // Whether the draft this job revises is stored right now: a draft deleted while the job runs is
    // not brought back by its result.
    let existed = req.previous.as_deref().is_some_and(|p| store::exists(&pctx.docs.paths, &p.id));
    let mut model: Option<String> = None;
    let mut ctx_size = None;
    if !rule_only(req) {
        state.jobs.progress(&job.id, None, None, None, Some("loading model".into()));
        let m = pipeline_bridge::vision_model(app, true, Some(&ctl.cancel)).map_err(|e| (e.message, None))?;
        ctx_size = state.engine.status().model(&m).and_then(|x| x.ctx);
        model = Some(m);
    }
    let vlm = model.as_ref().map(|m| {
        Vlm::new(&state.engine, m.clone(), pctx.profile, Some(&ctl.cancel))
            .with_context(ctx_size)
            .with_geometry(pipeline_bridge::vision_geometry(&state, m))
    });
    let analyzer = knowledge_bridge::analyzer(app);
    let embedder = knowledge_bridge::embedder(app, false).ok().flatten();
    let import = pctx.docs.deps();
    // Section-level progress of a split draft ("section 3/8", "outline", "finishing") or of a revision.
    // Every report comes between two model calls, so it is also where the job lets go of the engine:
    // assistant requests that are waiting run now instead of after the whole draft, and a paused job
    // parks here until it is resumed (a cancel surfaces through the flag the core checks next).
    let progress_cb = |frac: f32, detail: &str| {
        let _ = state.gate.checkpoint(ctl);
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
    // A model call holds the engine like any batch step; interactive requests cut in at the
    // checkpoints between the calls (the progress callback above). The first checkpoint comes before
    // the first call: a pause asked for while the model was loading holds the job here, and a request
    // that is being answered right now finishes first. A rule-composed draft makes no call and does
    // not wait for anyone.
    let _gate = state.gate.batch_begin();
    if vlm.is_some() && state.gate.checkpoint(ctl).is_err() {
        return Err(("cancelled".into(), None));
    }
    state.jobs.progress(&job.id, Some(0.2), None, None, Some("drafting".into()));
    let mut outcome: DraftOutcome = generate::draft(&deps, req).map_err(|e| (e.to_string(), None))?;
    // The result replaces the stored draft it started from only when that loses nothing. After a
    // cancel (a split draft keeps the sections written so far) or when the stored draft was changed
    // after this job was created — edits made before a failed job is retried — it is stored as a new
    // draft; a new draft is stored under its own id as before.
    let cancelled = ctl.cancelled();
    let input = store::JobInput { previous: req.previous.as_deref(), captured_at: job.created_at, existed };
    let stored =
        store::save_job_result(&pctx.docs.paths, &mut outcome.draft, &input, cancelled, ko).unwrap_or_else(|e| {
            log::warn!("draft job {}: the result could not be stored: {e}", job.id);
            store::JobSave::NotSaved { reason: store::SkipReason::Failed }
        });
    if cancelled {
        return Err(("cancelled".into(), None));
    }
    let mut detail = if ko {
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
    detail.push_str(match (&stored, ko) {
        (store::JobSave::Saved, _) => "",
        (store::JobSave::Copy { .. }, true) => " · 원래 초안이 그 사이 수정되어 새 초안으로 저장",
        (store::JobSave::Copy { .. }, false) => " · stored as a new draft (the original was edited meanwhile)",
        (store::JobSave::NotSaved { reason: store::SkipReason::Deleted }, true) => {
            " · 초안이 삭제되어 목록에 저장하지 않음"
        }
        (store::JobSave::NotSaved { reason: store::SkipReason::Deleted }, false) => {
            " · not stored (the draft was deleted)"
        }
        (store::JobSave::NotSaved { reason: store::SkipReason::Failed }, true) => " · 초안 목록에 저장하지 못함",
        (store::JobSave::NotSaved { reason: store::SkipReason::Failed }, false) => " · could not be stored",
    });
    let mut payload = serde_json::to_value(&outcome).map_err(|e| (e.to_string(), None))?;
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("project_id".into(), serde_json::json!(project_id));
        // `source` is the draft's own field (rules | model); the job origin lives under its own key.
        obj.insert("job_source".into(), serde_json::json!(source));
        // Where the result went: `{status: saved}` | `{status: copy, original, reason}` | `{status: not_saved, reason}`.
        obj.insert("draft_store".into(), serde_json::to_value(&stored).unwrap_or(serde_json::Value::Null));
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
