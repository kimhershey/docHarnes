//! Projects / consistency bridge (P6 §3.15): CRUD on projects, templates and
//! rules, deterministic consistency checks, relation graph, summary report and
//! the 보완요청서 draft job. Natural-language rule definition uses the loaded
//! model only as a fallback (phrase rules first) through the interactive gate —
//! and not at all while a batch job is inside a page, so dialogs never wait.

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use doculens_core::generate::OutputFormat;
use doculens_core::pipeline::vlm::Vlm;
use doculens_core::projects::classify::{ApplyResult, Assignment, ClassifyRequest, ClassifyResult};
use doculens_core::projects::copies::SyncReport;
use doculens_core::projects::{
    self, repo, CheckRun, Project, ProjectDeps, ProjectDetail, ProjectInput, ProjectTemplate, Rule, RuleDraft,
    TemplateInput,
};

use crate::state::{AppState, UiError, UiResult};
use crate::{generate_bridge, pipeline_bridge};

pub const PROJECTS_EVENT: &str = "projects://changed";

#[derive(Debug, Clone, Serialize)]
pub struct ReportResponse {
    pub markdown: String,
}

fn emit(app: &AppHandle, ids: Vec<String>) {
    let _ = app.emit(PROJECTS_EVENT, json!({ "project_ids": ids }));
}

fn language(app: &AppHandle) -> doculens_core::settings::Language {
    app.state::<AppState>().settings.lock().map(|s| s.ui.language).unwrap_or_default()
}

pub fn list(app: &AppHandle) -> UiResult<Vec<Project>> {
    let pctx = pipeline_bridge::context(app)?;
    pctx.db().with(repo::list_projects).map_err(UiError::from)
}

pub fn create(app: &AppHandle, mut input: ProjectInput) -> UiResult<Project> {
    input.id = None;
    if input.name.trim().is_empty() {
        return Err(UiError::invalid("name_empty"));
    }
    let pctx = pipeline_bridge::context(app)?;
    let project = pctx.db().with(|c| repo::save_project(c, &input))?;
    emit(app, vec![project.id.clone()]);
    Ok(project)
}

pub fn update(app: &AppHandle, input: ProjectInput) -> UiResult<Project> {
    if input.id.is_none() {
        return Err(UiError::invalid("id_required"));
    }
    let pctx = pipeline_bridge::context(app)?;
    let project = pctx.db().with(|c| repo::save_project(c, &input))?;
    emit(app, vec![project.id.clone()]);
    Ok(project)
}

pub fn delete(app: &AppHandle, id: &str) -> UiResult<bool> {
    let pctx = pipeline_bridge::context(app)?;
    // The project's tracked copies (files in its folder) go with it; the originals stay.
    projects::copies::remove_project_copies(pctx.db(), &pctx.docs.paths, id)?;
    let ok = pctx.db().with(|c| repo::delete_project(c, id))?;
    if ok {
        emit(app, vec![id.to_string()]);
    }
    Ok(ok)
}

pub fn get(app: &AppHandle, id: &str) -> UiResult<ProjectDetail> {
    let pctx = pipeline_bridge::context(app)?;
    projects::detail(pctx.db(), id, language(app)).map_err(UiError::from)
}

pub fn add_documents(app: &AppHandle, id: &str, doc_ids: &[String]) -> UiResult<ProjectDetail> {
    let pctx = pipeline_bridge::context(app)?;
    // A document that already belongs to another project gets a tracked copy in this one (plan §6).
    projects::copies::add_documents(pctx.db(), &pctx.docs.paths, id, doc_ids, language(app))?;
    emit(app, vec![id.to_string()]);
    get(app, id)
}

pub fn remove_document(app: &AppHandle, id: &str, doc_id: &str) -> UiResult<ProjectDetail> {
    let pctx = pipeline_bridge::context(app)?;
    projects::copies::remove_document(pctx.db(), &pctx.docs.paths, id, doc_id)?;
    emit(app, vec![id.to_string()]);
    get(app, id)
}

/// "다시 동기화": rewrites the project's copies from their originals (externally edited copies too).
pub fn sync_copies(app: &AppHandle, id: &str) -> UiResult<SyncReport> {
    let pctx = pipeline_bridge::context(app)?;
    let report = projects::copies::sync_project(pctx.db(), &pctx.docs.paths, id)?;
    emit(app, vec![id.to_string()]);
    Ok(report)
}

pub fn templates(app: &AppHandle) -> UiResult<Vec<ProjectTemplate>> {
    let pctx = pipeline_bridge::context(app)?;
    pctx.db().with(repo::list_templates).map_err(UiError::from)
}

pub fn template_save(app: &AppHandle, input: TemplateInput) -> UiResult<ProjectTemplate> {
    if input.name_ko.trim().is_empty() {
        return Err(UiError::invalid("name_empty"));
    }
    let pctx = pipeline_bridge::context(app)?;
    let tp = pctx.db().with(|c| repo::save_template(c, &input))?;
    emit(app, Vec::new());
    Ok(tp)
}

pub fn template_delete(app: &AppHandle, id: &str) -> UiResult<bool> {
    let pctx = pipeline_bridge::context(app)?;
    let ok = pctx.db().with(|c| repo::delete_template(c, id))?;
    emit(app, Vec::new());
    Ok(ok)
}

/// The loaded chat / vision model, when the engine already has it (never loads: the user
/// is waiting on a dialog). `None` → phrase rules only.
fn loaded_model(app: &AppHandle) -> Option<(String, Option<u32>)> {
    let state = app.state::<AppState>();
    let m = pipeline_bridge::vision_model(app, false, None).ok()?;
    let ctx = state.engine.status().model(&m).and_then(|x| x.ctx);
    Some((m, ctx))
}

/// The loaded model for a dialog request — unless a batch job is inside a page right now: the
/// dialog then runs without the model (phrase rules, plain comparisons) instead of waiting for
/// the page boundary behind the batch.
fn dialog_model<'a>(
    app: &AppHandle,
    state: &'a AppState,
    pctx: &pipeline_bridge::PipelineContext,
    what: &str,
) -> Option<Vlm<'a>> {
    let (model, ctx) = loaded_model(app)?;
    if state.gate.status().batch_busy {
        log::info!("projects: a batch job holds the engine; {what} runs without the model");
        return None;
    }
    Some(Vlm::new(&state.engine, model, pctx.profile, None).with_context(ctx))
}

pub fn define_rule(app: &AppHandle, id: &str, description: &str) -> UiResult<RuleDraft> {
    if description.trim().is_empty() {
        return Err(UiError::invalid("description_empty"));
    }
    let pctx = pipeline_bridge::context(app)?;
    let state = app.state::<AppState>();
    let vlm = dialog_model(app, &state, &pctx, "define_rule");
    let deps = ProjectDeps { db: pctx.db(), vlm: vlm.as_ref(), language: language(app) };
    let run = || projects::define_rule(&deps, Some(id), None, description);
    let draft = if vlm.is_some() { state.gate.interactive(run) } else { run() };
    draft.map_err(UiError::from)
}

/// Preview of the project classification (자동 분류): rule scores inline; the model pass, when
/// asked for and a model is loaded, runs through the interactive gate like a dialog request.
pub fn classify_preview(app: &AppHandle, req: ClassifyRequest) -> UiResult<ClassifyResult> {
    let pctx = pipeline_bridge::context(app)?;
    let state = app.state::<AppState>();
    let vlm = if req.use_model { dialog_model(app, &state, &pctx, "classify") } else { None };
    let deps = ProjectDeps { db: pctx.db(), vlm: vlm.as_ref(), language: language(app) };
    let run = || projects::classify(&deps, &req);
    let result = if vlm.is_some() { state.gate.interactive(run) } else { run() };
    result.map_err(UiError::from)
}

/// Writes the accepted assignments and refreshes the projects.
pub fn classify_apply(app: &AppHandle, assignments: Vec<Assignment>) -> UiResult<ApplyResult> {
    let pctx = pipeline_bridge::context(app)?;
    let result = projects::apply_classification(pctx.db(), &assignments)?;
    emit(app, result.project_ids.clone());
    Ok(result)
}

pub fn save_rule(app: &AppHandle, id: &str, rule: Rule) -> UiResult<Rule> {
    if rule.description.trim().is_empty() {
        return Err(UiError::invalid("description_empty"));
    }
    let pctx = pipeline_bridge::context(app)?;
    let saved = pctx.db().with(|c| repo::save_rule(c, id, &rule))?;
    emit(app, vec![id.to_string()]);
    Ok(saved)
}

pub fn delete_rule(app: &AppHandle, id: &str, rule_id: &str) -> UiResult<bool> {
    let pctx = pipeline_bridge::context(app)?;
    let ok = pctx.db().with(|c| repo::delete_rule(c, id, rule_id))?;
    emit(app, vec![id.to_string()]);
    Ok(ok)
}

/// Deterministic check (inline: a handful of rules over a handful of documents). The model
/// is only consulted for fuzzy name comparisons when it is already loaded.
pub fn check(app: &AppHandle, id: &str, rule_ids: &[String]) -> UiResult<CheckRun> {
    let pctx = pipeline_bridge::context(app)?;
    let state = app.state::<AppState>();
    let vlm = dialog_model(app, &state, &pctx, "check");
    let deps = ProjectDeps { db: pctx.db(), vlm: vlm.as_ref(), language: language(app) };
    let run = || projects::check(&deps, id, rule_ids);
    let result = if vlm.is_some() { state.gate.interactive(run) } else { run() };
    let run = result.map_err(UiError::from)?;
    emit(app, vec![id.to_string()]);
    Ok(run)
}

pub fn runs(app: &AppHandle, id: &str) -> UiResult<Vec<CheckRun>> {
    let pctx = pipeline_bridge::context(app)?;
    pctx.db().with(|c| repo::list_runs(c, id, 20)).map_err(UiError::from)
}

pub fn report(app: &AppHandle, id: &str) -> UiResult<ReportResponse> {
    let pctx = pipeline_bridge::context(app)?;
    let markdown = projects::report(pctx.db(), id, language(app)).map_err(UiError::from)?;
    Ok(ReportResponse { markdown })
}

/// Builds the 보완요청서 request from the last check run and queues the draft job.
pub fn request_supplement(
    app: &AppHandle,
    id: &str,
    finding_ids: &[String],
    output: Option<OutputFormat>,
) -> UiResult<String> {
    let pctx = pipeline_bridge::context(app)?;
    let run = pctx.db().with(|c| repo::last_run(c, id))?.ok_or_else(|| UiError::invalid("no_check_run"))?;
    let req = projects::supplement_request(pctx.db(), id, &run, finding_ids, output.unwrap_or_default(), language(app))
        .map_err(UiError::from)?;
    generate_bridge::draft(app, req, Some(id.to_string()), "project")
}
