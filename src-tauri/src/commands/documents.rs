//! Document library commands (P2). Heavy work (hashing, conversion,
//! rendering) runs on job threads; these commands only plan, query and start.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use doculens_core::documents::import::{self, ImportOptions, ImportPlan, ImportSource};
use doculens_core::documents::{repo, DocumentFilter, DocumentRecord, LibraryStats, PageRecord};
use doculens_core::paths::normalize_user_path;
use doculens_core::CoreError;

use crate::docs_bridge::{self, PdfiumInfo};
use crate::protocol;
use crate::state::{AppState, UiError, UiResult};

/// Document plus the URLs / paths the UI needs.
#[derive(Debug, Clone, Serialize)]
pub struct DocumentView {
    #[serde(flatten)]
    pub record: DocumentRecord,
    pub absolute_path: PathBuf,
    pub thumb_url: Option<String>,
}

pub(crate) fn view(app: &AppHandle, d: DocumentRecord) -> UiResult<DocumentView> {
    let state = app.state::<AppState>();
    let paths = state.with_workspace(|ws| ws.paths.clone())?;
    let version = d.updated_at.timestamp_millis().to_string();
    let thumb_url = (d.page_count > 0).then(|| protocol::page_url(&d.id, 1, true, Some(&version)));
    Ok(DocumentView { absolute_path: d.absolute_path(&paths), thumb_url, record: d })
}

#[derive(Debug, Clone, Serialize)]
pub struct PageView {
    #[serde(flatten)]
    pub page: PageRecord,
    pub image_url: Option<String>,
    pub thumb_url: Option<String>,
}

fn page_view(p: PageRecord) -> PageView {
    let version = p.rendered_at.map(|t| t.timestamp_millis().to_string());
    let has_image = p.width_px > 0 && p.height_px > 0;
    PageView {
        image_url: has_image.then(|| protocol::page_url(&p.document_id, p.page_no, false, version.as_deref())),
        thumb_url: has_image.then(|| protocol::page_url(&p.document_id, p.page_no, true, version.as_deref())),
        page: p,
    }
}

#[tauri::command(async)]
pub fn documents_list(app: AppHandle, filter: Option<DocumentFilter>) -> UiResult<Vec<DocumentView>> {
    let ctx = docs_bridge::context(&app)?;
    let filter = filter.unwrap_or_default();
    let docs = ctx.db.with(|c| repo::list_documents(c, &filter))?;
    docs.into_iter().map(|d| view(&app, d)).collect()
}

#[tauri::command(async)]
pub fn documents_stats(app: AppHandle) -> UiResult<LibraryStats> {
    let ctx = docs_bridge::context(&app)?;
    Ok(ctx.db.with(repo::stats)?)
}

#[tauri::command(async)]
pub fn documents_get(app: AppHandle, id: String) -> UiResult<DocumentView> {
    let d = docs_bridge::document(&app, &id)?;
    view(&app, d)
}

#[tauri::command(async)]
pub fn documents_pages(app: AppHandle, id: String, with_text: Option<bool>) -> UiResult<Vec<PageView>> {
    let ctx = docs_bridge::context(&app)?;
    let pages = ctx.db.with(|c| repo::list_pages(c, &id, with_text.unwrap_or(false)))?;
    Ok(pages.into_iter().map(page_view).collect())
}

#[tauri::command(async)]
pub fn documents_page_text(app: AppHandle, id: String, page_no: u32) -> UiResult<Option<String>> {
    let ctx = docs_bridge::context(&app)?;
    let page = ctx.db.with(|c| repo::get_page(c, &id, page_no))?;
    Ok(page.and_then(|p| p.text))
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportPlanArgs {
    pub paths: Vec<String>,
    #[serde(default)]
    pub options: ImportOptions,
    #[serde(default)]
    pub source: ImportSource,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportPlanView {
    pub token: String,
    #[serde(flatten)]
    pub plan: ImportPlan,
}

/// Builds the import preview (hashes files — async command).
#[tauri::command(async)]
pub fn documents_import_plan(app: AppHandle, args: ImportPlanArgs) -> UiResult<ImportPlanView> {
    let ctx = docs_bridge::context(&app)?;
    let sources: Vec<PathBuf> = args.paths.iter().map(|p| normalize_user_path(p)).collect();
    if sources.is_empty() {
        return Err(UiError::invalid("no files"));
    }
    let plan = import::plan(&ctx.db, &ctx.paths, &sources, &args.options, args.source)?;
    let token = docs_bridge::remember_plan(&app, plan.clone());
    Ok(ImportPlanView { token, plan })
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportRunArgs {
    /// Token from `documents_import_plan`; when missing the plan is rebuilt from `paths`.
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub options: Option<ImportOptions>,
    #[serde(default)]
    pub source: ImportSource,
}

/// Starts the import job; returns the job id.
#[tauri::command(async)]
pub fn documents_import_run(app: AppHandle, args: ImportRunArgs) -> UiResult<String> {
    let mut plan = args.token.as_deref().and_then(|t| docs_bridge::take_plan(&app, t));
    if let (Some(p), Some(opts)) = (plan.as_mut(), args.options.clone()) {
        // Options chosen in the dialog after the preview (destination, move, role, render).
        p.options = opts;
    }
    let plan = match plan {
        Some(p) => p,
        None => {
            let ctx = docs_bridge::context(&app)?;
            let sources: Vec<PathBuf> = args.paths.iter().map(|p| normalize_user_path(p)).collect();
            if sources.is_empty() {
                return Err(UiError::invalid("no files"));
            }
            import::plan(&ctx.db, &ctx.paths, &sources, &args.options.unwrap_or_default(), args.source)?
        }
    };
    if plan.new_count + plan.relink_count == 0 {
        return Err(UiError::invalid("nothing to import"));
    }
    docs_bridge::import_job(&app, plan)
}

/// Returns the job id, or `None` when an import / render / rescan is already
/// running (a rescan would reprocess documents the running job is writing).
#[tauri::command(async)]
pub fn documents_rescan(app: AppHandle) -> UiResult<Option<String>> {
    docs_bridge::context(&app)?;
    Ok(docs_bridge::rescan_job(&app, true))
}

#[tauri::command(async)]
pub fn documents_inbox_scan(app: AppHandle) -> UiResult<()> {
    docs_bridge::context(&app)?;
    docs_bridge::inbox_job(&app);
    Ok(())
}

#[tauri::command(async)]
pub fn documents_rerender(app: AppHandle, ids: Vec<String>, dpi: Option<u16>) -> UiResult<String> {
    if ids.is_empty() {
        return Err(UiError::invalid("no documents"));
    }
    docs_bridge::rerender_job(&app, ids, dpi)
}

#[tauri::command(async)]
pub fn documents_rotate_page(app: AppHandle, id: String, page_no: u32, rotation: u16) -> UiResult<PageView> {
    let ctx = docs_bridge::context(&app)?;
    let doc = docs_bridge::document(&app, &id)?;
    let deps = ctx.deps();
    import::rotate_page(&deps, &doc, page_no, rotation)?;
    let page = ctx
        .db
        .with(|c| repo::get_page(c, &id, page_no))?
        .ok_or_else(|| CoreError::NotFound(format!("page {page_no}")))?;
    docs_bridge::emit_changed(&app, "updated", vec![id]);
    Ok(page_view(page))
}

#[tauri::command(async)]
pub fn documents_delete(app: AppHandle, ids: Vec<String>, keep_files: Option<bool>) -> UiResult<Vec<String>> {
    docs_bridge::delete_documents(&app, &ids, keep_files.unwrap_or(false))
}

#[tauri::command(async)]
pub fn documents_set_tags(app: AppHandle, id: String, tags: Vec<String>) -> UiResult<DocumentView> {
    let ctx = docs_bridge::context(&app)?;
    ctx.db.with_tx(|tx| repo::set_tags(tx, &id, &tags))?;
    docs_bridge::emit_changed(&app, "updated", vec![id.clone()]);
    view(&app, docs_bridge::document(&app, &id)?)
}

#[tauri::command(async)]
pub fn documents_set_doc_type(app: AppHandle, id: String, doc_type: Option<String>) -> UiResult<DocumentView> {
    let ctx = docs_bridge::context(&app)?;
    let dt = doc_type.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    ctx.db.with(|c| repo::set_doc_type(c, &id, dt.as_deref(), Some("manual")))?;
    docs_bridge::emit_changed(&app, "updated", vec![id.clone()]);
    view(&app, docs_bridge::document(&app, &id)?)
}

#[tauri::command(async)]
pub fn documents_set_title(app: AppHandle, id: String, title: Option<String>) -> UiResult<DocumentView> {
    let ctx = docs_bridge::context(&app)?;
    let t = title.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    ctx.db.with(|c| repo::set_title(c, &id, t.as_deref()))?;
    docs_bridge::emit_changed(&app, "updated", vec![id.clone()]);
    view(&app, docs_bridge::document(&app, &id)?)
}

#[tauri::command(async)]
pub fn documents_tags(app: AppHandle) -> UiResult<Vec<(String, u64)>> {
    let ctx = docs_bridge::context(&app)?;
    Ok(ctx.db.with(repo::list_tags)?)
}

#[tauri::command(async)]
pub fn documents_set_inbox_watch(app: AppHandle, state: State<'_, AppState>, enabled: bool) -> UiResult<bool> {
    state.update_settings(|s| s.processing.inbox_watch = enabled)?;
    docs_bridge::set_inbox_watch(&app, enabled)?;
    Ok(enabled)
}

#[tauri::command(async)]
pub fn documents_pdfium_info(app: AppHandle) -> PdfiumInfo {
    docs_bridge::pdfium_info(&app)
}
