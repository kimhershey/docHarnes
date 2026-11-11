//! Planner / organise / journal / aggregate / export / job-control commands (P5).

use serde::Deserialize;
use tauri::{AppHandle, Manager};

use doculens_core::aggregate::{AggregateRequest, AggregateResult};
use doculens_core::export::{ExportPreview, ExportRequest};
use doculens_core::organize::journal::{JournalBatch, JournalEntry};
use doculens_core::organize::{OrganizePlan, OrganizeRequest};
use doculens_core::planner::Plan;

use crate::jobs::Job;
use crate::planner_bridge::{self, ExecuteOptions, ExecuteResponse, PlanResponse, UiPlanContext, UndoResponse};
use crate::state::{AppState, UiError, UiResult};

#[tauri::command(async)]
pub fn planner_plan(app: AppHandle, text: String, context: Option<UiPlanContext>) -> UiResult<PlanResponse> {
    if text.trim().is_empty() {
        return Err(UiError::invalid("text_empty"));
    }
    planner_bridge::plan(&app, &text, &context.unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExecuteArgs {
    pub plan: Plan,
    /// The command as the card shows it (job subject).
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub context: Option<UiPlanContext>,
    /// The exact text the plan was made from — `PlanResponse.result.text` of the planning call
    /// that produced the card. After an ask-back this is the command, the assistant's question
    /// and the user's reply; the reply is where "전부" was said, so the documents-required check
    /// reads this text (audit PLN-7). Absent = `text`.
    #[serde(default)]
    pub planned_text: Option<String>,
    /// `true`: validate and preview the (edited) plan without running it — the response carries
    /// `preview: { plan, preview, issues }` instead of `job_id` / `execution` (audit PLN-14).
    #[serde(default)]
    pub preview_only: bool,
}

#[tauri::command(async)]
pub fn planner_execute(app: AppHandle, args: ExecuteArgs) -> UiResult<ExecuteResponse> {
    let options = ExecuteOptions { planned_text: args.planned_text.as_deref(), preview_only: args.preview_only };
    planner_bridge::execute(&app, args.plan, &args.text, &args.context.unwrap_or_default(), &options)
}

// -- organise / journal ------------------------------------------------------

#[tauri::command(async)]
pub fn organize_plan(app: AppHandle, req: OrganizeRequest) -> UiResult<OrganizePlan> {
    planner_bridge::organize_plan(&app, &req)
}

#[tauri::command(async)]
pub fn organize_apply(app: AppHandle, plan: OrganizePlan, label: Option<String>) -> UiResult<String> {
    planner_bridge::organize_apply(&app, plan, label, "organize")
}

#[tauri::command(async)]
pub fn journal_batches(app: AppHandle, limit: Option<u32>) -> UiResult<Vec<JournalBatch>> {
    planner_bridge::journal_batches(&app, limit.unwrap_or(50).clamp(1, 500))
}

#[tauri::command(async)]
pub fn journal_entries(app: AppHandle, batch_id: String) -> UiResult<Vec<JournalEntry>> {
    planner_bridge::journal_entries(&app, &batch_id)
}

#[tauri::command(async)]
pub fn journal_undo(app: AppHandle, batch_id: Option<String>) -> UiResult<UndoResponse> {
    planner_bridge::undo(&app, batch_id.as_deref(), false)
}

#[tauri::command(async)]
pub fn journal_redo(app: AppHandle) -> UiResult<UndoResponse> {
    planner_bridge::undo(&app, None, true)
}

// -- aggregate / export ------------------------------------------------------

#[tauri::command(async)]
pub fn aggregate_run(app: AppHandle, req: AggregateRequest) -> UiResult<AggregateResult> {
    planner_bridge::aggregate_run(&app, &req)
}

#[tauri::command(async)]
pub fn export_preview(app: AppHandle, req: ExportRequest) -> UiResult<ExportPreview> {
    planner_bridge::export_preview(&app, &req)
}

#[tauri::command(async)]
pub fn export_run(app: AppHandle, req: ExportRequest) -> UiResult<String> {
    planner_bridge::export_run(&app, &req)
}

// -- job control --------------------------------------------------------------

#[tauri::command]
pub fn jobs_get(app: AppHandle, job_id: String) -> Option<Job> {
    app.state::<AppState>().jobs.get(&job_id)
}

#[tauri::command(async)]
pub fn jobs_pause(app: AppHandle, job_id: String) -> bool {
    app.state::<AppState>().jobs.pause(&job_id)
}

#[tauri::command(async)]
pub fn jobs_resume(app: AppHandle, job_id: String) -> bool {
    app.state::<AppState>().jobs.resume(&job_id)
}

#[tauri::command(async)]
pub fn jobs_retry(app: AppHandle, job_id: String) -> UiResult<bool> {
    app.state::<AppState>().jobs.retry(&job_id)
}

#[tauri::command(async)]
pub fn jobs_prioritize(app: AppHandle, job_id: String) -> bool {
    app.state::<AppState>().jobs.prioritize(&job_id)
}
