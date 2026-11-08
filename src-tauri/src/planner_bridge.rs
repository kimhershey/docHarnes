//! Natural-language planner wiring (P5): plan creation (pre-router → rules →
//! model behind the engine gate), side-effect previews, inline execution of
//! quick plans, the persistent-job worker (pipeline / plan / export / organise
//! / undo / redo) and the organise, journal, aggregate and export helpers the
//! commands call.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};

use doculens_core::aggregate::{self, AggregateRequest, AggregateResult};
use doculens_core::documents::import::{self as doc_import, ImportOptions, ImportSource};
use doculens_core::documents::repo as docs;
use doculens_core::export::{self, ExportPreview, ExportRequest};
use doculens_core::jobs::EngineGate;
use doculens_core::jobs::{JobControl, JobPriority, JobSpec};
use doculens_core::knowledge::embed::Embedder;
use doculens_core::organize::journal::{self, JournalBatch, JournalEntry, UndoReport};
use doculens_core::organize::{self, OrganizePlan, OrganizeRequest};
use doculens_core::pipeline::repo as pipeline_repo;
use doculens_core::pipeline::run::{self as pipeline_run, PipelineDeps};
use doculens_core::planner::exec::{self, EngineLease, ExecDeps, PlanExecution, PlanPreview};
use doculens_core::planner::llm::{self, PlannerDeps};
use doculens_core::planner::{
    self, DocTypeSummary, FieldSummary, Op, OpKind, Plan, PlanContext, PlanResult, ProjectSummary, SchemaSummary,
};
use doculens_core::projects::repo as projects_repo;
use doculens_core::CoreError;

use crate::docs_bridge;
use crate::jobs::{Job, JobKind, WorkerResult};
use crate::knowledge_bridge;
use crate::pipeline_bridge;
use crate::state::{AppState, UiError, UiResult};

/// What the UI knows about its current view (default scope of a command).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct UiPlanContext {
    pub folder: Option<String>,
    pub selected_doc_ids: Vec<String>,
    pub current_document: Option<String>,
    pub current_page: Option<u32>,
    /// Project open on the projects screen (default target of project ops, §3.15).
    pub current_project: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanResponse {
    pub result: PlanResult,
    pub preview: Option<PlanPreview>,
    pub context: PlanContext,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecuteResponse {
    /// Set when the plan runs as a background job.
    pub job_id: Option<String>,
    /// Set when the plan ran inline.
    pub execution: Option<PlanExecution>,
}

/// The engine gate as an executor lease: model-bound steps occupy the single slot, imports and
/// exports in the same plan do not.
struct GateLease<'a>(&'a EngineGate);

impl EngineLease for GateLease<'_> {
    fn begin(&self) {
        self.0.batch_begin_raw();
    }
    fn end(&self) {
        self.0.batch_end();
    }
}

/// Builds the planner context from the workspace (types, schemas, folders, tags, models).
pub fn build_context(app: &AppHandle, ui: &UiPlanContext) -> UiResult<PlanContext> {
    let state = app.state::<AppState>();
    let ctx = pipeline_bridge::context(app)?;
    let settings = state.settings_snapshot()?;
    let (types, schemas, stats, tags, projects) = ctx.db().with_read(|c| {
        let types = pipeline_repo::doc_types(c)?;
        let schemas = pipeline_repo::list_schemas(c)?;
        let stats = docs::stats(c)?;
        let tags = docs::list_tags(c)?;
        let projects = projects_repo::list_projects(c)?;
        Ok((types, schemas, stats, tags, projects))
    })?;
    let projects = projects
        .iter()
        .map(|p| ProjectSummary { id: p.id.clone(), name: p.name.clone(), status: p.status.as_str().to_string() })
        .collect();
    let doc_types = types
        .iter()
        .map(|t| DocTypeSummary { slug: t.slug.clone(), label_ko: t.label_ko.clone(), label_en: t.label_en.clone() })
        .collect();
    let schemas = schemas
        .iter()
        .filter(|s| s.enabled)
        .map(|s| SchemaSummary {
            id: s.id.clone(),
            name: s.name.clone(),
            name_en: s.name_en.clone().unwrap_or_default(),
            doc_type: s.doc_type.clone(),
            fields: s
                .definition
                .fields
                .iter()
                .map(|f| FieldSummary {
                    key: f.key.clone(),
                    label_ko: f.label_ko.clone(),
                    label_en: f.label_en.clone(),
                    kind: f.kind.as_str().to_string(),
                })
                .collect(),
        })
        .collect();
    let registry = state.merged_registry();
    let model_ids: Vec<String> = registry
        .models
        .iter()
        .filter(|m| m.role != doculens_core::registry::ModelRole::Embedding)
        .map(|m| m.id.clone())
        .collect();
    Ok(PlanContext {
        today: chrono::Local::now().date_naive(),
        language: settings.ui.language,
        folders: stats.folders.iter().map(|f| f.rel_dir.clone()).filter(|f| !f.is_empty()).collect(),
        doc_types,
        schemas,
        tags: tags.into_iter().map(|(name, _)| name).collect(),
        current_folder: ui.folder.clone(),
        selected_doc_ids: ui.selected_doc_ids.clone(),
        current_document: ui.current_document.clone(),
        current_page: ui.current_page,
        document_count: stats.total,
        model_ids,
        projects,
        current_project: ui.current_project.clone(),
    })
}

/// Plans a command. The model is only loaded when the pre-router and the
/// rules could not interpret the text; model calls go through the engine
/// gate so a running batch yields at its next page boundary.
pub fn plan(app: &AppHandle, text: &str, ui: &UiPlanContext) -> UiResult<PlanResponse> {
    let state = app.state::<AppState>();
    let ctx = build_context(app, ui)?;
    let mut result = llm::plan(None, text, &ctx)?;
    if result.source == planner::PlanSource::None && result.issues.iter().any(|i| i.code == "no_model") {
        match pipeline_bridge::vision_model(app, true, None) {
            Ok(model) => {
                let settings = state.settings_snapshot()?;
                let ctx_size = state.engine.status().model(&model).and_then(|m| m.ctx);
                let deps = PlannerDeps {
                    chat: &state.engine,
                    model,
                    profile: settings.models.profile,
                    ctx_size,
                    cancel: None,
                };
                result = state.gate.interactive(|| llm::plan(Some(&deps), text, &ctx))?;
            }
            Err(e) => {
                log::info!("planner: model unavailable ({}); rules-only result kept", e.message);
            }
        }
    }
    let preview = if result.plan.is_clarification() {
        None
    } else {
        let pctx = pipeline_bridge::context(app)?;
        let preview = exec::preview(pctx.db(), &pctx.docs.paths, &result.plan, &pctx.settings, ctx.language)?;
        // Pin "undo the last batch" to the batch the card shows, so a batch finishing in the
        // meantime is not the one reverted.
        for step in &preview.steps {
            if step.op == OpKind::Undo {
                if let (Some(Op::Undo { target }), Some(id)) =
                    (result.plan.operations.get_mut(step.index), step.payload.get("id").and_then(Value::as_str))
                {
                    if target.is_none() {
                        *target = Some(id.to_string());
                    }
                }
            }
        }
        Some(preview)
    };
    Ok(PlanResponse { result, preview, context: ctx })
}

/// Ops that run inline (quick, no model, no files).
fn inline_only(plan: &Plan) -> bool {
    plan.operations.iter().all(|op| {
        op.kind().runs_in_ui()
            || matches!(
                op.kind(),
                OpKind::Search | OpKind::Aggregate | OpKind::Tag | OpKind::SetDocType | OpKind::Validate
            )
            || matches!(op, Op::Review { action: planner::ReviewOp::Approve | planner::ReviewOp::Reject, .. })
    })
}

/// Executes a (confirmed, possibly edited) plan: quick plans inline, the rest
/// as a persistent job.
pub fn execute(app: &AppHandle, mut plan: Plan, text: &str, ui: &UiPlanContext) -> UiResult<ExecuteResponse> {
    let ctx = build_context(app, ui)?;
    let date = doculens_core::prerouter::resolve_relative_date(text, ctx.today);
    let issues = planner::validate::validate(&mut plan, &ctx, date.as_ref());
    if planner::has_errors(&issues) {
        return Err(UiError {
            code: "plan_invalid".into(),
            message: issues.iter().find(|i| i.blocking).map(|i| i.message.clone()).unwrap_or_default(),
            detail: serde_json::to_value(&issues).ok(),
        });
    }
    if plan.is_clarification() {
        return Err(UiError::invalid("plan_clarify"));
    }
    if let Some(issue) = planner::validate::scope_required(&plan) {
        if !planner::rules::mentions_all(text) {
            return Err(UiError {
                code: "plan_invalid".into(),
                message: issue.message.clone(),
                detail: serde_json::to_value(vec![issue]).ok(),
            });
        }
    }
    if inline_only(&plan) {
        let execution = run_inline(app, &plan, &ctx)?;
        return Ok(ExecuteResponse { job_id: None, execution: Some(execution) });
    }
    let state = app.state::<AppState>();
    let subject = plan.summary.clone().unwrap_or_else(|| text.to_string());
    let id = state.jobs.submit(
        JobSpec::Plan { plan: Box::new(plan), text: text.to_string(), context: Box::new(ctx) },
        &subject,
        JobPriority::Normal,
    )?;
    Ok(ExecuteResponse { job_id: Some(id), execution: None })
}

fn run_inline(app: &AppHandle, plan: &Plan, ctx: &PlanContext) -> UiResult<PlanExecution> {
    let state = app.state::<AppState>();
    let pctx = pipeline_bridge::context(app)?;
    let settings = state.settings_snapshot()?;
    let analyzer = knowledge_bridge::analyzer(app);
    let embedder = knowledge_bridge::embedder(app, false).ok().flatten();
    let deps = ExecDeps {
        import: pctx.docs.deps(),
        chat: None,
        model: None,
        ctx_size: None,
        settings: pctx.settings.clone(),
        knowledge: settings.knowledge.clone(),
        profile: pctx.profile,
        pii_masking: pctx.pii_masking,
        analyzer: &analyzer,
        embedder: embedder.as_ref().map(|e| e as &dyn Embedder),
        language: ctx.language,
        today: ctx.today,
        cancel: None,
        checkpoint: None,
        lease: None,
    };
    let execution = exec::run(&deps, plan, &mut |_| {})?;
    after_execution(app, &pctx, &execution);
    Ok(execution)
}

/// Emits change events for what an execution touched and re-imports files an
/// undo brought back from the workspace trash.
fn after_execution(app: &AppHandle, pctx: &pipeline_bridge::PipelineContext, execution: &PlanExecution) {
    if !execution.documents.is_empty() {
        docs_bridge::emit_changed(app, "updated", execution.documents.clone());
        pctx.emit(app, "processed", execution.documents.clone());
    }
    if !execution.batches.is_empty() {
        pctx.emit(app, "journal", Vec::new());
    }
    reimport_restored(app, pctx, &execution.restored_paths);
}

fn reimport_restored(app: &AppHandle, pctx: &pipeline_bridge::PipelineContext, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    let files: Vec<std::path::PathBuf> =
        paths.iter().map(|p| doculens_core::documents::from_rel_path(&pctx.docs.paths.root, p)).collect();
    let options = ImportOptions { destination: doc_import::ImportDestination::Documents, ..ImportOptions::default() };
    match doc_import::plan(pctx.db(), &pctx.docs.paths, &files, &options, ImportSource::Rescan) {
        Ok(plan) => {
            if let Err(e) = docs_bridge::import_job(app, plan) {
                log::warn!("re-import of restored files failed: {}", e.message);
            }
        }
        Err(e) => log::warn!("re-import of restored files failed: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Persistent-job worker
// ---------------------------------------------------------------------------

/// Installs the worker on the job book (setup).
pub fn install_worker(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.jobs.set_worker(Arc::new(|app: &AppHandle, job: &Job, spec: &JobSpec, ctl: &JobControl| {
        run_spec(app, job, spec, ctl)
    }));
}

fn err_result<T>(r: Result<T, UiError>) -> Result<T, (String, Option<Value>)> {
    r.map_err(|e| (e.message, None))
}

fn run_spec(app: &AppHandle, job: &Job, spec: &JobSpec, ctl: &JobControl) -> WorkerResult {
    let state = app.state::<AppState>();
    let pctx = err_result(pipeline_bridge::context(app))?;
    let generation = pctx.docs.generation;
    let outcome = match spec {
        JobSpec::Pipeline { req } => run_pipeline(app, job, req, ctl, &pctx),
        JobSpec::Plan { plan, context, .. } => run_plan(app, job, plan, context, ctl, &pctx),
        JobSpec::Export { req } => run_export(req, ctl, &pctx),
        JobSpec::Organize { plan, label, source, batch_kind } => {
            run_organize(app, job, plan, label, source, batch_kind, ctl, &pctx)
        }
        JobSpec::Undo { batch_id } => run_undo(app, batch_id.as_deref(), false, &pctx),
        JobSpec::Redo => run_undo(app, None, true, &pctx),
        JobSpec::Draft { req, project_id, source } => {
            crate::generate_bridge::run_draft(app, job, req, project_id.as_deref(), source, ctl, &pctx)
        }
        JobSpec::Maintenance => crate::maintenance::run(app, ctl),
    };
    if state.workspace_generation.load(Ordering::SeqCst) == generation && !state.switching.load(Ordering::SeqCst) {
        knowledge_bridge::auto_index(app);
    }
    outcome
}

fn progress_detail(job: &Job, kind: JobKind, p: &doculens_core::pipeline::ProcessProgress) -> String {
    let _ = (job, kind);
    match p.page {
        Some((d, t)) if t > 0 => format!("{} · {} ({d}/{t})", p.current, p.stage),
        _ if p.current.is_empty() => p.stage.clone(),
        _ => format!("{} · {}", p.current, p.stage),
    }
}

fn run_pipeline(
    app: &AppHandle,
    job: &Job,
    req: &doculens_core::pipeline::ProcessRequest,
    ctl: &JobControl,
    pctx: &pipeline_bridge::PipelineContext,
) -> WorkerResult {
    let state = app.state::<AppState>();
    state.jobs.progress(&job.id, None, None, None, Some("loading model".into()));
    let model = pipeline_bridge::vision_model(app, true, Some(&ctl.cancel)).map_err(|e| (e.message, None))?;
    let model_ctx = state.engine.status().model(&model).and_then(|m| m.ctx);
    log::info!(
        "pipeline: vision model {model} (context {})",
        model_ctx.map_or("unknown".to_string(), |c| c.to_string())
    );
    let deps = PipelineDeps {
        import: pctx.docs.deps(),
        chat: &state.engine,
        model,
        settings: pctx.settings.clone(),
        profile: pctx.profile,
        pii_masking: pctx.pii_masking,
        cancel: Some(&ctl.cancel),
        ctx: model_ctx,
    };
    let total = req.document_ids.len() as u32;
    let _lease = state.gate.batch_begin();
    let report = pipeline_run::process(&deps, req, &mut |p| {
        // Page boundary: pause / let interactive requests in (a cancel surfaces through the flag).
        let _ = state.gate.checkpoint(ctl);
        let page_part = p.page.map(|(d, t)| if t == 0 { 0.0 } else { d as f32 / t as f32 }).unwrap_or(0.0);
        let frac =
            if p.total == 0 { None } else { Some(((p.done as f32 + page_part * 0.9) / p.total as f32).min(1.0)) };
        state.jobs.progress(
            &job.id,
            frac,
            Some(p.done as u64),
            Some(total as u64),
            Some(progress_detail(job, JobKind::Pipeline, p)),
        );
    });
    drop(_lease);
    let report = report.map_err(|e| (e.to_string(), None))?;
    let mut ids = report.processed.clone();
    ids.extend(report.failed.iter().map(|(id, _)| id.clone()));
    if !ids.is_empty() {
        docs_bridge::emit_changed(app, "updated", ids.clone());
        pctx.emit(app, "processed", ids);
    }
    let payload = serde_json::to_value(&report).ok();
    if report.cancelled {
        return Err(("cancelled".into(), payload));
    }
    let summary = format!(
        "processed {} / classified {} / ocr pages {} / extracted {} / review {} / failed {}",
        report.processed.len(),
        report.classified,
        report.ocr_pages,
        report.extracted,
        report.review_pending,
        report.failed.len()
    );
    if !report.failed.is_empty() && report.processed.is_empty() {
        return Err((report.failed.iter().map(|(id, e)| format!("{id}: {e}")).collect::<Vec<_>>().join("; "), payload));
    }
    Ok((Some(summary), payload))
}

fn run_plan(
    app: &AppHandle,
    job: &Job,
    plan: &Plan,
    context: &PlanContext,
    ctl: &JobControl,
    pctx: &pipeline_bridge::PipelineContext,
) -> WorkerResult {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot().map_err(|e| (e.message, None))?;
    let needs_model = plan.needs_model();
    let mut model = None;
    let mut ctx_size = None;
    if needs_model {
        state.jobs.progress(&job.id, None, None, None, Some("loading model".into()));
        let m = pipeline_bridge::vision_model(app, true, Some(&ctl.cancel)).map_err(|e| (e.message, None))?;
        ctx_size = state.engine.status().model(&m).and_then(|x| x.ctx);
        model = Some(m);
    }
    let analyzer = knowledge_bridge::analyzer(app);
    let embedder = knowledge_bridge::embedder(app, false).ok().flatten();
    let engine_lane = needs_model;
    let gate = &state.gate;
    let lease = GateLease(gate);
    let checkpoint = move || -> doculens_core::Result<()> {
        if engine_lane {
            gate.checkpoint(ctl)
        } else {
            ctl.wait_while_paused()
        }
    };
    let deps = ExecDeps {
        import: pctx.docs.deps(),
        chat: if needs_model { Some(&state.engine) } else { None },
        model,
        ctx_size,
        settings: pctx.settings.clone(),
        knowledge: settings.knowledge.clone(),
        profile: pctx.profile,
        pii_masking: pctx.pii_masking,
        analyzer: &analyzer,
        embedder: embedder.as_ref().map(|e| e as &dyn Embedder),
        language: context.language,
        today: context.today,
        cancel: Some(&ctl.cancel),
        checkpoint: Some(&checkpoint),
        lease: Some(&lease),
    };
    let steps = plan.operations.len() as u64;
    let result = exec::run(&deps, plan, &mut |p| {
        let frac = if p.steps == 0 {
            None
        } else {
            let inner = if p.total == 0 { 0.0 } else { p.done as f32 / p.total as f32 };
            Some(((p.step as f32 + inner * 0.9) / p.steps as f32).min(1.0))
        };
        let detail = if p.total > 0 {
            format!("{}/{} · {} · {} ({}/{})", p.step + 1, p.steps, p.stage, p.current, p.done, p.total)
        } else {
            format!("{}/{} · {}", p.step + 1, p.steps, p.stage)
        };
        state.jobs.progress(&job.id, frac, Some(p.step as u64), Some(steps), Some(detail));
    });
    let execution = result.map_err(|e| (e.to_string(), None))?;
    after_execution(app, pctx, &execution);
    let payload = serde_json::to_value(&execution).ok();
    if execution.cancelled {
        return Err(("cancelled".into(), payload));
    }
    if let Some(failed) = execution.steps.iter().find(|s| s.status == exec::StepStatus::Failed) {
        return Err((format!("{}: {}", failed.op.as_str(), failed.error.clone().unwrap_or_default()), payload));
    }
    let done = execution.steps.iter().filter(|s| s.status == exec::StepStatus::Done).count();
    let last = execution
        .steps
        .iter()
        .rev()
        .find(|s| s.status == exec::StepStatus::Done)
        .map(|s| s.summary.clone())
        .unwrap_or_default();
    Ok((Some(format!("plan {done}/{} · {last}", execution.steps.len())), payload))
}

fn run_export(req: &Value, ctl: &JobControl, pctx: &pipeline_bridge::PipelineContext) -> WorkerResult {
    let req: ExportRequest = serde_json::from_value(req.clone()).map_err(|e| (e.to_string(), None))?;
    let res =
        export::export(pctx.db(), &pctx.docs.paths, &req, Some(&ctl.cancel)).map_err(|e| (e.to_string(), None))?;
    let payload = serde_json::to_value(&res).ok();
    Ok((Some(format!("exported {} documents → {}", res.documents, res.path)), payload))
}

#[allow(clippy::too_many_arguments)]
fn run_organize(
    app: &AppHandle,
    job: &Job,
    plan: &OrganizePlan,
    label: &str,
    source: &str,
    batch_kind: &str,
    ctl: &JobControl,
    pctx: &pipeline_bridge::PipelineContext,
) -> WorkerResult {
    let state = app.state::<AppState>();
    let report = organize::apply(
        pctx.db(),
        &pctx.docs.paths,
        plan,
        batch_kind,
        label,
        source,
        Some(&ctl.cancel),
        &mut |done, total, name| {
            let _ = ctl.wait_while_paused();
            state.jobs.progress(
                &job.id,
                if total == 0 { None } else { Some(done as f32 / total as f32) },
                Some(done),
                Some(total),
                Some(name.to_string()),
            );
        },
    )
    .map_err(|e| (e.to_string(), None))?;
    if !report.documents.is_empty() {
        docs_bridge::emit_changed(app, "updated", report.documents.clone());
    }
    pctx.emit(app, "journal", Vec::new());
    let payload = serde_json::to_value(&report).ok();
    if report.cancelled {
        return Err(("cancelled".into(), payload));
    }
    Ok((Some(format!("moved {} / failed {} / skipped {}", report.moved, report.failed.len(), report.skipped)), payload))
}

fn run_undo(
    app: &AppHandle,
    batch_id: Option<&str>,
    redo: bool,
    pctx: &pipeline_bridge::PipelineContext,
) -> WorkerResult {
    let report = if redo {
        journal::redo_last(pctx.db(), &pctx.docs.paths)
    } else {
        match batch_id {
            Some(id) => journal::undo_batch(pctx.db(), &pctx.docs.paths, id).map(Some),
            None => journal::undo_last(pctx.db(), &pctx.docs.paths),
        }
    }
    .map_err(|e| (e.to_string(), None))?;
    let Some(report) = report else {
        return Ok((Some(if redo { "nothing to redo".into() } else { "nothing to undo".into() }), None));
    };
    if !report.documents.is_empty() {
        docs_bridge::emit_changed(app, "updated", report.documents.clone());
    }
    pctx.emit(app, "journal", Vec::new());
    reimport_restored(app, pctx, &report.restored_paths);
    let payload = serde_json::to_value(&report).ok();
    Ok((Some(format!("restored {} / skipped {}", report.restored, report.skipped.len())), payload))
}

// ---------------------------------------------------------------------------
// Organise / journal / aggregate / export helpers (commands)
// ---------------------------------------------------------------------------

pub fn organize_plan(app: &AppHandle, req: &OrganizeRequest) -> UiResult<OrganizePlan> {
    let pctx = pipeline_bridge::context(app)?;
    organize::plan(pctx.db(), &pctx.docs.paths, req).map_err(typed)
}

/// Applies a previewed plan as a job; returns the job id.
pub fn organize_apply(app: &AppHandle, plan: OrganizePlan, label: Option<String>, source: &str) -> UiResult<String> {
    let state = app.state::<AppState>();
    if plan.moves == 0 {
        return Err(UiError::invalid("organize_nothing"));
    }
    let label = label.unwrap_or_else(|| format!("정리 {}건", plan.moves));
    let batch_kind = match (&plan.name_template, &plan.folder_template) {
        (Some(_), None) => "rename",
        (None, Some(_)) => "move",
        _ => "organize",
    };
    let subject = format!("{} files", plan.moves);
    state.jobs.submit(
        JobSpec::Organize { plan: Box::new(plan), label, source: source.to_string(), batch_kind: batch_kind.into() },
        &subject,
        JobPriority::High,
    )
}

pub fn journal_batches(app: &AppHandle, limit: u32) -> UiResult<Vec<JournalBatch>> {
    let pctx = pipeline_bridge::context(app)?;
    Ok(pctx.db().with_read(|c| journal::list_batches(c, limit))?)
}

pub fn journal_entries(app: &AppHandle, batch_id: &str) -> UiResult<Vec<JournalEntry>> {
    let pctx = pipeline_bridge::context(app)?;
    Ok(pctx.db().with_read(|c| journal::entries(c, batch_id))?)
}

#[derive(Debug, Clone, Serialize)]
pub struct UndoResponse {
    pub job_id: Option<String>,
    pub report: Option<UndoReport>,
}

/// Small batches are reverted inline (Ctrl+Z feel); large ones run as a job.
pub fn undo(app: &AppHandle, batch_id: Option<&str>, redo: bool) -> UiResult<UndoResponse> {
    let state = app.state::<AppState>();
    let pctx = pipeline_bridge::context(app)?;
    let batch = pctx.db().with_read(|c| match (redo, batch_id) {
        (true, _) => journal::last_redoable(c),
        (false, Some(id)) => journal::get_batch(c, id),
        (false, None) => journal::last_undoable(c),
    })?;
    let Some(batch) = batch else {
        return Err(UiError::invalid(if redo { "nothing_to_redo" } else { "nothing_to_undo" }));
    };
    if batch.entries <= 50 {
        let (_, payload) = run_undo(app, Some(&batch.id), false, &pctx).map_err(|(m, _)| UiError::invalid(m))?;
        let report = payload.and_then(|v| serde_json::from_value(v).ok());
        return Ok(UndoResponse { job_id: None, report });
    }
    let spec = if redo { JobSpec::Redo } else { JobSpec::Undo { batch_id: Some(batch.id.clone()) } };
    let id = state.jobs.submit(spec, &format!("{} entries", batch.entries), JobPriority::High)?;
    Ok(UndoResponse { job_id: Some(id), report: None })
}

pub fn aggregate_run(app: &AppHandle, req: &AggregateRequest) -> UiResult<AggregateResult> {
    let pctx = pipeline_bridge::context(app)?;
    aggregate::aggregate(pctx.db(), req).map_err(typed)
}

pub fn export_preview(app: &AppHandle, req: &ExportRequest) -> UiResult<ExportPreview> {
    let pctx = pipeline_bridge::context(app)?;
    export::preview(pctx.db(), &pctx.docs.paths, req).map_err(typed)
}

/// Runs an export as a job; returns the job id.
pub fn export_run(app: &AppHandle, req: &ExportRequest) -> UiResult<String> {
    let state = app.state::<AppState>();
    let pctx = pipeline_bridge::context(app)?;
    let preview = export::preview(pctx.db(), &pctx.docs.paths, req).map_err(typed)?;
    if preview.exists && !req.overwrite {
        return Err(UiError::invalid("export_exists"));
    }
    let value = serde_json::to_value(req).map_err(|e| UiError::invalid(e.to_string()))?;
    state.jobs.submit(JobSpec::Export { req: value }, &preview.path, JobPriority::High)
}

/// Core errors whose message is a stable code (`export_exists`, `path_outside` …).
fn typed(e: CoreError) -> UiError {
    const CODES: [&str; 8] = [
        "export_exists",
        "path_outside",
        "template_variable",
        "no_documents",
        "batch_already_undone",
        "nothing_to_undo",
        "organize_nothing",
        "no_model",
    ];
    let m = e.to_string();
    if let Some(code) = CODES.iter().find(|c| m.contains(*c)) {
        return UiError { code: (*code).into(), message: m, detail: None };
    }
    e.into()
}
