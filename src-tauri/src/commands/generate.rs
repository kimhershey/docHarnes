//! Document generation commands (P6 §3.13).

use tauri::AppHandle;

use doculens_core::generate::{Draft, DraftRequest, DraftSummary, SaveRequest, SavedDocument, TemplateInfo};

use crate::generate_bridge::{self, PreviewResponse};
use crate::state::UiResult;

#[derive(Debug, Clone, serde::Serialize)]
pub struct JobStarted {
    pub job_id: String,
}

#[tauri::command]
pub fn generate_templates() -> Vec<TemplateInfo> {
    generate_bridge::templates()
}

#[tauri::command]
pub fn generate_budget_info(app: AppHandle) -> UiResult<generate_bridge::BudgetInfo> {
    generate_bridge::budget_info(&app)
}

#[tauri::command(async)]
pub fn generate_draft(app: AppHandle, req: DraftRequest) -> UiResult<JobStarted> {
    generate_bridge::draft(&app, req, None, "generate").map(|job_id| JobStarted { job_id })
}

#[tauri::command(async)]
pub fn generate_preview(draft: Draft) -> PreviewResponse {
    generate_bridge::preview(&draft)
}

#[tauri::command(async)]
pub fn generate_validate(draft: Draft) -> Draft {
    generate_bridge::validate(draft)
}

#[tauri::command(async)]
pub fn generate_save(app: AppHandle, req: SaveRequest) -> UiResult<SavedDocument> {
    generate_bridge::save(&app, &req)
}

#[tauri::command(async)]
pub fn generate_drafts(app: AppHandle) -> UiResult<Vec<DraftSummary>> {
    generate_bridge::drafts(&app)
}

#[tauri::command(async)]
pub fn generate_draft_get(app: AppHandle, id: String) -> UiResult<Draft> {
    generate_bridge::draft_get(&app, &id)
}

#[tauri::command(async)]
pub fn generate_draft_put(app: AppHandle, draft: Draft) -> UiResult<DraftSummary> {
    generate_bridge::draft_put(&app, &draft)
}

#[tauri::command(async)]
pub fn generate_draft_delete(app: AppHandle, id: String) -> UiResult<bool> {
    generate_bridge::draft_delete(&app, &id)
}
