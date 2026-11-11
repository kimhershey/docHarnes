//! Natural-language planner wiring (P5): plan creation (pre-router → rules →
//! model behind the engine gate), side-effect previews, inline execution of
//! quick plans, the persistent-job worker (pipeline / plan / export / organise
//! / undo / redo) and the organise, journal, aggregate and export helpers the
//! commands call.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use doculens_core::aggregate::{self, AggregateRequest, AggregateResult};
use doculens_core::documents::import::{self as doc_import, ImportOptions, ImportSource};
use doculens_core::documents::repo as docs;
use doculens_core::export::{self, ExportPreview, ExportRequest};
use doculens_core::jobs::EngineGate;
use doculens_core::jobs::{JobControl, JobPriority, JobSpec};
use doculens_core::knowledge::embed::Embedder;
use doculens_core::knowledge::ConversationTurn;
use doculens_core::organize::journal::{self, JournalBatch, JournalEntry, UndoReport};
use doculens_core::organize::{self, OrganizePlan, OrganizeRequest};
use doculens_core::pipeline::repo as pipeline_repo;
use doculens_core::pipeline::run::{self as pipeline_run, PipelineDeps};
use doculens_core::planner::exec::{self, EngineLease, ExecDeps, PlanExecution, PlanPreview};
use doculens_core::planner::llm::{self, PlannerDeps};
use doculens_core::planner::{
    self, DocTypeSummary, FieldSummary, Op, OpKind, Plan, PlanContext, PlanMode, PlanResult, ProjectSummary,
    SchemaSummary,
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
    /// Project the assistant's "현재 프로젝트" scope names (0.5.0): default scope of commands and
    /// evidence scope of questions.
    pub project_id: Option<String>,
    /// Earlier turns of the assistant conversation (oldest first).
    pub history: Vec<ConversationTurn>,
    /// 0.7.0: the draft open in the 문서 작성 screen (default target of `revise_draft`).
    pub draft_id: Option<String>,
    /// Interpretation mode (default: the model reads every sentence; `rules_first` is the legacy path).
    pub routing: Option<PlanMode>,
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
    /// Set for a `preview_only` call: nothing ran — the plan as the executor would take it and
    /// the preview of its side effects (the card of an edited plan shows these).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<PlanCheck>,
}

/// A (possibly edited) plan checked the way `execute` checks it, without running it.
#[derive(Debug, Clone, Serialize)]
pub struct PlanCheck {
    /// The plan after validation (normalised parameters, default scope, confirmation flag).
    pub plan: Plan,
    pub preview: PlanPreview,
    /// Non-blocking findings (blocking ones make the call fail with `plan_invalid`).
    pub issues: Vec<planner::PlanIssue>,
}

/// What `planner_execute` is told besides the plan.
#[derive(Debug, Clone, Default)]
pub struct ExecuteOptions<'a> {
    /// The exact text the plan was made from (`PlanResponse.result.text`). After an ask-back it is
    /// the command, the assistant's question and the user's reply — the reply is where "전부" was
    /// said, so the scope check reads this text and not the command shown on the card (audit PLN-7).
    /// `None` = the command itself.
    pub planned_text: Option<&'a str>,
    /// Validate and preview only (the card of an edited plan); nothing is executed.
    pub preview_only: bool,
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
        .map(|p| ProjectSummary { id: p.id.clone(), name: p.name.clone(), status: p.phase.as_str().to_string() })
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
        scope_project: ui.project_id.clone(),
        draft_id: ui.draft_id.clone().filter(|d| !d.trim().is_empty()),
        history: ui.history.clone(),
    })
}

/// Plans a command through the model (loading it on demand) behind the engine gate, so a running
/// batch yields at its next page boundary. `None` when no model can be had.
fn plan_with_model(
    app: &AppHandle,
    text: &str,
    ctx: &PlanContext,
    mode: PlanMode,
) -> UiResult<Option<planner::PlanResult>> {
    let state = app.state::<AppState>();
    let model = match pipeline_bridge::vision_model(app, true, None) {
        Ok(model) => model,
        Err(e) => {
            log::info!("planner: model unavailable ({})", e.message);
            return Ok(None);
        }
    };
    let settings = state.settings_snapshot()?;
    let ctx_size = state.engine.status().model(&model).and_then(|m| m.ctx);
    let deps = PlannerDeps { chat: &state.engine, model, profile: settings.models.profile, ctx_size, cancel: None };
    Ok(Some(state.gate.interactive(|| llm::plan(Some(&deps), text, ctx, mode))?))
}

/// Plans a command. Model-first (the default since 2026-11-09): slash commands are routed
/// deterministically, every other sentence goes to the model, and without a model the result is a
/// clarification that asks for one. Rules-first (legacy): the pre-router and the phrase rules
/// answer first and the model is loaded only when they cannot interpret the text.
pub fn plan(app: &AppHandle, text: &str, ui: &UiPlanContext) -> UiResult<PlanResponse> {
    let ctx = build_context(app, ui)?;
    let mode = ui.routing.unwrap_or_default();
    let mut result = match mode {
        PlanMode::ModelFirst if !text.trim_start().starts_with('/') => match plan_with_model(app, text, &ctx, mode)? {
            Some(r) => r,
            None => llm::plan(None, text, &ctx, mode)?, // → `no_model` clarification
        },
        _ => llm::plan(None, text, &ctx, mode)?,
    };
    if mode == PlanMode::RulesFirst
        && result.source == planner::PlanSource::None
        && result.issues.iter().any(|i| i.code == "no_model")
    {
        if let Some(r) = plan_with_model(app, text, &ctx, mode)? {
            result = r;
        }
    }
    // A plan that changes documents without naming any got here only because the sentence said
    // "전부" (the planner's scope guard): inside the 현재 프로젝트 scope that "all" is the project.
    // (The validator leaves such a plan's scope empty so that the guard can ask — audit PLN-6.)
    if !result.plan.is_clarification() {
        planner::validate::apply_default_scope(&mut result.plan, &ctx);
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
            // Review decisions are quick database writes; `show` only hands the review screen to the UI.
            || matches!(op, Op::Review { .. })
    })
}

/// The checks a plan from a card passes before it runs or is previewed; returns the non-blocking
/// findings. The plan is normalised in place and gets the 현재 프로젝트 default scope where it applies.
fn check_for_execution(
    plan: &mut Plan,
    ctx: &PlanContext,
    text: &str,
    options: &ExecuteOptions<'_>,
) -> UiResult<Vec<planner::PlanIssue>> {
    // The plan comes from a card: its date range is the one the planning step resolved — or what
    // the user made of it in the editor. The date phrase of the sentence is not applied again, or
    // a range the user removed would come back (audit PLN-14).
    let issues = planner::validate::validate(plan, ctx, None);
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
    // Documents must be named unless the user said "전부" — in the text the plan was made from,
    // which after an ask-back includes the reply (audit PLN-7).
    let said = options.planned_text.filter(|t| !t.trim().is_empty()).unwrap_or(text);
    if let Some(issue) = planner::validate::settle_scope(plan, ctx, said) {
        return Err(UiError {
            code: "plan_invalid".into(),
            message: issue.message.clone(),
            detail: serde_json::to_value(vec![issue]).ok(),
        });
    }
    Ok(issues)
}

/// Executes a (confirmed, possibly edited) plan: quick plans inline, the rest
/// as a persistent job. With `options.preview_only` the plan is only checked and previewed.
pub fn execute(
    app: &AppHandle,
    mut plan: Plan,
    text: &str,
    ui: &UiPlanContext,
    options: &ExecuteOptions<'_>,
) -> UiResult<ExecuteResponse> {
    let ctx = build_context(app, ui)?;
    let issues = check_for_execution(&mut plan, &ctx, text, options)?;
    if options.preview_only {
        let pctx = pipeline_bridge::context(app)?;
        let preview = exec::preview(pctx.db(), &pctx.docs.paths, &plan, &pctx.settings, ctx.language)?;
        return Ok(ExecuteResponse {
            job_id: None,
            execution: None,
            preview: Some(PlanCheck { plan, preview, issues }),
        });
    }
    if inline_only(&plan) {
        let execution = run_inline(app, &plan, &ctx)?;
        return Ok(ExecuteResponse { job_id: None, execution: Some(execution), preview: None });
    }
    let state = app.state::<AppState>();
    let subject = plan.summary.clone().unwrap_or_else(|| text.to_string());
    let id = state.jobs.submit(
        JobSpec::Plan { plan: Box::new(plan), text: text.to_string(), context: Box::new(ctx) },
        &subject,
        JobPriority::Normal,
    )?;
    Ok(ExecuteResponse { job_id: Some(id), execution: None, preview: None })
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
    emit_step_changes(app, pctx, execution);
    reimport_restored(app, pctx, &execution.restored_paths);
}

/// What the finished steps of an execution changed besides documents, as the events the screens
/// listen to: the glossary, editions and reference groups (`knowledge://changed`), projects and
/// their templates / rules / check results (`projects://changed`), extraction schemas and review
/// decisions (`pipeline://changed`).
#[derive(Debug, Default, PartialEq)]
struct StepChanges {
    /// `knowledge://changed` kinds (`glossary`, `versions`, `collections`).
    knowledge: Vec<&'static str>,
    /// `pipeline://changed` kinds (`schemas`, `review`).
    pipeline: Vec<&'static str>,
    /// Whether `projects://changed` is due, and the projects the payloads name.
    projects: bool,
    project_ids: Vec<String>,
}

fn step_changes(execution: &PlanExecution) -> StepChanges {
    fn once(kinds: &mut Vec<&'static str>, kind: &'static str) {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    let mut out = StepChanges::default();
    for step in execution.steps.iter().filter(|s| s.status == exec::StepStatus::Done) {
        let payload = &step.payload;
        let count = |key: &str| payload.get(key).and_then(Value::as_u64).unwrap_or(0);
        let project_id = ["project_id", "id"]
            .iter()
            .find_map(|k| payload.get(k).and_then(Value::as_str))
            .or_else(|| payload.get("project").and_then(|p| p.get("id")).and_then(Value::as_str));
        let project = |out: &mut StepChanges| {
            out.projects = true;
            if let Some(id) = project_id {
                if !out.project_ids.iter().any(|x| x == id) {
                    out.project_ids.push(id.to_string());
                }
            }
        };
        match step.op {
            OpKind::DefineTerm => once(&mut out.knowledge, "glossary"),
            OpKind::SetVersion => once(&mut out.knowledge, "versions"),
            OpKind::ManageReference => once(&mut out.knowledge, "collections"),
            OpKind::CreateProject | OpKind::RemoveFromProject | OpKind::CheckConsistency | OpKind::DefineRule => {
                project(&mut out)
            }
            // A rename renames the project's reference group, a deletion takes the group along,
            // and `as_reference` fills it.
            OpKind::UpdateProject | OpKind::DeleteProject | OpKind::AddToProject => {
                project(&mut out);
                once(&mut out.knowledge, "collections");
            }
            // Templates are listed on the projects screen; a rule saved to a template too.
            OpKind::ManageTemplate => out.projects = true,
            OpKind::ClassifyProjects if count("applied") > 0 => out.projects = true,
            OpKind::ManageSchema => once(&mut out.pipeline, "schemas"),
            OpKind::Review if count("changed") > 0 => once(&mut out.pipeline, "review"),
            _ => {}
        }
    }
    out
}

/// The assistant changed the glossary, an edition, a project, a template or a schema: the screen
/// that shows it must hear about it now, not at its next periodic refresh (audit CON-6).
fn emit_step_changes(app: &AppHandle, pctx: &pipeline_bridge::PipelineContext, execution: &PlanExecution) {
    if !pctx.docs.is_current(app) {
        return; // the workspace was switched while the plan ran
    }
    let changes = step_changes(execution);
    for kind in changes.knowledge {
        knowledge_bridge::emit_changed(app, kind, Vec::new());
    }
    for kind in changes.pipeline {
        pctx.emit(app, kind, execution.documents.clone());
    }
    if changes.projects {
        let _ =
            app.emit(crate::projects_bridge::PROJECTS_EVENT, serde_json::json!({ "project_ids": changes.project_ids }));
    }
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
        JobSpec::SchemaDiscovery { doc_ids, name_hint } => {
            pipeline_bridge::run_schema_discovery(app, job, doc_ids, name_hint.as_deref(), ctl, &pctx)
        }
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
    let report = match report {
        Ok(report) => report,
        Err(e) => {
            // The batch stopped part-way (the engine went away, audit PIP-6): the documents finished
            // before that changed all the same, and the screens showing them must hear of it.
            docs_bridge::emit_changed(app, "updated", req.document_ids.clone());
            pctx.emit(app, "processed", req.document_ids.clone());
            return Err((e.to_string(), None));
        }
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use doculens_core::planner::exec::{StepResult, StepStatus};
    use serde_json::json;

    fn step(op: OpKind, status: StepStatus, payload: Value) -> StepResult {
        StepResult { index: 0, op, status, summary: String::new(), payload, error: None, elapsed_ms: 0 }
    }

    fn changes(steps: Vec<StepResult>) -> StepChanges {
        step_changes(&PlanExecution { steps, ..PlanExecution::default() })
    }

    /// Audit CON-6: after the assistant changed the glossary, an edition, a project, a template or
    /// a schema, the screen that shows it kept the old content until its periodic refresh.
    #[test]
    fn finished_steps_name_the_change_events_of_what_they_touched() {
        let done = |op, payload| step(op, StepStatus::Done, payload);
        let c = changes(vec![done(OpKind::DefineTerm, json!({ "action": "save" }))]);
        assert_eq!((c.knowledge.as_slice(), c.projects, c.pipeline.is_empty()), (&["glossary"][..], false, true));
        assert_eq!(changes(vec![done(OpKind::SetVersion, json!({}))]).knowledge, ["versions"]);
        assert_eq!(changes(vec![done(OpKind::ManageReference, json!({}))]).knowledge, ["collections"]);
        assert_eq!(changes(vec![done(OpKind::ManageSchema, json!({}))]).pipeline, ["schemas"]);
        // Projects: the payloads name the project in three shapes.
        let c = changes(vec![
            done(OpKind::CreateProject, json!({ "id": "p1", "name": "한빛" })),
            done(OpKind::UpdateProject, json!({ "project": { "id": "p2" } })),
            done(OpKind::RemoveFromProject, json!({ "project_id": "p3", "removed": 1 })),
            done(OpKind::CheckConsistency, json!({ "id": "run-1", "project_id": "p1" })),
        ]);
        assert!(c.projects);
        assert_eq!(c.project_ids, ["p1", "p2", "p3"]);
        assert_eq!(c.knowledge, ["collections"], "a project rename renames its reference group");
        // Templates have no project: every project view refreshes (an empty id list).
        let c = changes(vec![done(OpKind::ManageTemplate, json!({ "action": "save" }))]);
        assert!(c.projects && c.project_ids.is_empty());
        // Suggestions that assigned nothing, review steps that changed nothing and steps that did
        // not finish announce nothing.
        assert_eq!(changes(vec![done(OpKind::ClassifyProjects, json!({ "applied": 0 }))]), StepChanges::default());
        assert!(changes(vec![done(OpKind::ClassifyProjects, json!({ "applied": 2 }))]).projects);
        assert_eq!(changes(vec![done(OpKind::Review, json!({ "changed": 0 }))]), StepChanges::default());
        assert_eq!(changes(vec![done(OpKind::Review, json!({ "changed": 3 }))]).pipeline, ["review"]);
        assert_eq!(changes(vec![step(OpKind::DefineTerm, StepStatus::Failed, json!({}))]), StepChanges::default());
        assert_eq!(changes(vec![done(OpKind::Tag, json!({ "documents": ["d1"] }))]), StepChanges::default());
    }

    fn context(scope_project: Option<&str>) -> PlanContext {
        PlanContext {
            today: chrono::NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(),
            projects: vec![ProjectSummary {
                id: "p-hanbit".into(), name: "한빛 납품".into(), status: "active".into()
            }],
            scope_project: scope_project.map(str::to_string),
            ..PlanContext::default()
        }
    }

    /// "태그 '긴급' 달아줘" as a plan from a card: a tag step and whatever scope the card carries.
    fn tag_plan(scope: planner::Scope) -> Plan {
        let mut plan = Plan::single(Op::Tag { add: vec!["긴급".into()], remove: Vec::new() }, 0.9, "태그 추가");
        plan.scope = scope;
        plan
    }

    fn checked(plan: &mut Plan, ctx: &PlanContext, text: &str, planned_text: Option<&str>) -> Result<(), String> {
        let options = ExecuteOptions { planned_text, preview_only: false };
        check_for_execution(plan, ctx, text, &options).map(|_| ()).map_err(|e| {
            let codes: Vec<String> = e
                .detail
                .as_ref()
                .and_then(Value::as_array)
                .map(|issues| issues.iter().filter_map(|i| i["code"].as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            format!("{}: {}", e.code, codes.join(","))
        })
    }

    /// Audit PLN-7: a plan made after an ask-back ("… 전체면 '전부'라고 해 주세요" → "전부") could
    /// never run — the card keeps the command only, and execution looked for "전부" in it.
    #[test]
    fn a_plan_runs_against_the_text_it_was_made_from() {
        const COMMAND: &str = "태그 '긴급' 달아줘";
        let asked = |reply: &str| {
            format!("{COMMAND}\n\n[어시스턴트 질문] 어떤 문서에 적용할까요? 전체면 '전부'라고 해 주세요.\n[사용자 답변] {reply}")
        };
        let library = context(None);
        // The card's command alone names no documents.
        let mut plan = tag_plan(planner::Scope::default());
        assert_eq!(checked(&mut plan, &library, COMMAND, None), Err("plan_invalid: scope_required".into()));
        assert_eq!(checked(&mut plan, &library, COMMAND, Some("  ")), Err("plan_invalid: scope_required".into()));
        // With the text of the planning call the reply counts — the user's reply, not the question.
        assert_eq!(checked(&mut plan, &library, COMMAND, Some(&asked("전부"))), Ok(()));
        assert!(plan.scope.is_empty(), "outside a project \"all\" is the library: {:?}", plan.scope);
        assert_eq!(
            checked(&mut plan, &library, COMMAND, Some(&asked("영수증만"))),
            Err("plan_invalid: scope_required".into())
        );
        // Inside the 현재 프로젝트 scope "all" is the project (audit PLN-6).
        let in_project = context(Some("p-hanbit"));
        let mut plan = tag_plan(planner::Scope::default());
        assert_eq!(checked(&mut plan, &in_project, COMMAND, None), Err("plan_invalid: scope_required".into()));
        assert_eq!(checked(&mut plan, &in_project, COMMAND, Some(&asked("전부"))), Ok(()));
        assert_eq!(plan.scope.filter.project_id.as_deref(), Some("p-hanbit"));
        // A sentence that says "all" itself needs no second text.
        let mut plan = tag_plan(planner::Scope::default());
        assert_eq!(checked(&mut plan, &library, "모든 문서에 태그 '긴급' 달아줘", None), Ok(()));
    }

    /// Audit PLN-14: execution resolved the date phrase of the command again and wrote it into the
    /// plan, so a range the user had removed in the plan editor came back.
    #[test]
    fn a_plan_from_a_card_keeps_the_date_range_it_carries() {
        const COMMAND: &str = "지난달 영수증에 태그 '긴급' 달아줘";
        let ctx = context(None);
        let receipts = |from: Option<&str>, to: Option<&str>| planner::Scope {
            filter: planner::ScopeFilter {
                doc_type: Some("receipt".into()),
                date_from: from.map(str::to_string),
                date_to: to.map(str::to_string),
                ..Default::default()
            },
            ..Default::default()
        };
        let range = |plan: &Plan| (plan.scope.filter.date_from.clone(), plan.scope.filter.date_to.clone());
        // The planning step does read the phrase: this is what used to run again at execution.
        let date = doculens_core::prerouter::resolve_relative_date(COMMAND, ctx.today);
        let mut planned = tag_plan(receipts(None, None));
        planner::validate::validate(&mut planned, &ctx, date.as_ref());
        assert_eq!(range(&planned), (Some("2026-10-01".into()), Some("2026-10-31".into())));
        // The user removed the range on the card: it stays removed.
        let mut plan = tag_plan(receipts(None, None));
        assert_eq!(checked(&mut plan, &ctx, COMMAND, None), Ok(()));
        assert_eq!(range(&plan), (None, None));
        // … or narrowed it: the plan runs with the edited range.
        let mut plan = tag_plan(receipts(Some("2026-10-01"), Some("2026-10-15")));
        assert_eq!(checked(&mut plan, &ctx, COMMAND, None), Ok(()));
        assert_eq!(range(&plan), (Some("2026-10-01".into()), Some("2026-10-15".into())));
    }
}
