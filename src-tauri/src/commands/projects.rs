//! Project / consistency commands (P6 §3.15) and the retention purge controls (P7 §3.8).

use tauri::AppHandle;

use doculens_core::generate::OutputFormat;
use doculens_core::projects::copies::SyncReport;
use doculens_core::projects::{
    CheckRun, Project, ProjectDetail, ProjectInput, ProjectTemplate, Rule, RuleDraft, TemplateInput,
};

use crate::commands::generate::JobStarted;
use crate::maintenance::{self, MaintenanceStatus};
use crate::projects_bridge::{self, ReportResponse};
use crate::state::UiResult;

#[tauri::command(async)]
pub fn projects_list(app: AppHandle) -> UiResult<Vec<Project>> {
    projects_bridge::list(&app)
}

#[tauri::command(async)]
pub fn project_create(app: AppHandle, input: ProjectInput) -> UiResult<Project> {
    projects_bridge::create(&app, input)
}

#[tauri::command(async)]
pub fn project_update(app: AppHandle, input: ProjectInput) -> UiResult<Project> {
    projects_bridge::update(&app, input)
}

#[tauri::command(async)]
pub fn project_delete(app: AppHandle, id: String) -> UiResult<bool> {
    projects_bridge::delete(&app, &id)
}

#[tauri::command(async)]
pub fn project_get(app: AppHandle, id: String) -> UiResult<ProjectDetail> {
    projects_bridge::get(&app, &id)
}

#[tauri::command(async)]
pub fn project_add_documents(app: AppHandle, id: String, doc_ids: Vec<String>) -> UiResult<ProjectDetail> {
    projects_bridge::add_documents(&app, &id, &doc_ids)
}

#[tauri::command(async)]
pub fn project_remove_document(app: AppHandle, id: String, doc_id: String) -> UiResult<ProjectDetail> {
    projects_bridge::remove_document(&app, &id, &doc_id)
}

#[tauri::command(async)]
pub fn project_sync_copies(app: AppHandle, id: String) -> UiResult<SyncReport> {
    projects_bridge::sync_copies(&app, &id)
}

#[tauri::command(async)]
pub fn project_templates(app: AppHandle) -> UiResult<Vec<ProjectTemplate>> {
    projects_bridge::templates(&app)
}

#[tauri::command(async)]
pub fn project_template_save(app: AppHandle, input: TemplateInput) -> UiResult<ProjectTemplate> {
    projects_bridge::template_save(&app, input)
}

#[tauri::command(async)]
pub fn project_template_delete(app: AppHandle, id: String) -> UiResult<bool> {
    projects_bridge::template_delete(&app, &id)
}

#[tauri::command(async)]
pub fn project_define_rule(app: AppHandle, id: String, description: String) -> UiResult<RuleDraft> {
    projects_bridge::define_rule(&app, &id, &description)
}

#[tauri::command(async)]
pub fn project_save_rule(app: AppHandle, id: String, rule: Rule) -> UiResult<Rule> {
    projects_bridge::save_rule(&app, &id, rule)
}

#[tauri::command(async)]
pub fn project_delete_rule(app: AppHandle, id: String, rule_id: String) -> UiResult<bool> {
    projects_bridge::delete_rule(&app, &id, &rule_id)
}

#[tauri::command(async)]
pub fn project_check(app: AppHandle, id: String, rule_ids: Option<Vec<String>>) -> UiResult<CheckRun> {
    projects_bridge::check(&app, &id, &rule_ids.unwrap_or_default())
}

#[tauri::command(async)]
pub fn project_runs(app: AppHandle, id: String) -> UiResult<Vec<CheckRun>> {
    projects_bridge::runs(&app, &id)
}

#[tauri::command(async)]
pub fn project_report(app: AppHandle, id: String) -> UiResult<ReportResponse> {
    projects_bridge::report(&app, &id)
}

#[tauri::command(async)]
pub fn project_request_supplement(
    app: AppHandle,
    id: String,
    finding_ids: Option<Vec<String>>,
    output: Option<OutputFormat>,
) -> UiResult<JobStarted> {
    projects_bridge::request_supplement(&app, &id, &finding_ids.unwrap_or_default(), output)
        .map(|job_id| JobStarted { job_id })
}

// -- retention purge (P7) ------------------------------------------------------

/// 자동 분류 preview: suggested project per document (nothing is written).
#[tauri::command(async)]
pub fn project_classify_preview(
    app: AppHandle,
    req: doculens_core::projects::classify::ClassifyRequest,
) -> UiResult<doculens_core::projects::classify::ClassifyResult> {
    projects_bridge::classify_preview(&app, req)
}

/// 자동 분류 apply: the accepted document → project rows.
#[tauri::command(async)]
pub fn project_classify_apply(
    app: AppHandle,
    assignments: Vec<doculens_core::projects::classify::Assignment>,
) -> UiResult<doculens_core::projects::classify::ApplyResult> {
    projects_bridge::classify_apply(&app, assignments)
}

#[tauri::command(async)]
pub fn maintenance_status(app: AppHandle) -> UiResult<MaintenanceStatus> {
    maintenance::status(&app)
}

#[tauri::command(async)]
pub fn maintenance_run_now(app: AppHandle) -> UiResult<JobStarted> {
    maintenance::run_now(&app).map(|job_id| JobStarted { job_id })
}
