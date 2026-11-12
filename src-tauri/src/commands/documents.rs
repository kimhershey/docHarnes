//! Document library commands (P2). Heavy work (hashing, conversion,
//! rendering) runs on job threads; these commands only plan, query and start.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use doculens_core::documents::import::{
    self, ImportOptions, ImportPlan, ImportSource, PageTextHistory, PageTextRevert,
};
use doculens_core::documents::layout::PageLayout;
use doculens_core::documents::{repo, DocumentFilter, DocumentRecord, LibraryStats, PageRecord};
use doculens_core::paths::normalize_user_path;
use doculens_core::projects::repo::DocumentProject;
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
    /// 1.0.0: the projects the document belongs to (work document or reference material) — the
    /// library card's project badges. Filled by `documents_list` / `documents_get`; empty elsewhere.
    pub projects: Vec<DocumentProject>,
}

pub(crate) fn view(app: &AppHandle, d: DocumentRecord) -> UiResult<DocumentView> {
    let state = app.state::<AppState>();
    let paths = state.with_workspace(|ws| ws.paths.clone())?;
    let version = d.updated_at.timestamp_millis().to_string();
    let thumb_url = (d.page_count > 0).then(|| protocol::page_url(&d.id, 1, true, Some(&version)));
    Ok(DocumentView { absolute_path: d.absolute_path(&paths), thumb_url, record: d, projects: Vec::new() })
}

/// Attaches the project memberships to document views (one read of the membership tables).
fn with_projects(db: &doculens_core::db::Db, mut views: Vec<DocumentView>) -> UiResult<Vec<DocumentView>> {
    if views.is_empty() {
        return Ok(views);
    }
    let mut map = db.with(doculens_core::projects::repo::document_projects)?;
    for v in &mut views {
        if let Some(list) = map.remove(&v.record.id) {
            v.projects = list;
        }
    }
    Ok(views)
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
    let views = docs.into_iter().map(|d| view(&app, d)).collect::<UiResult<Vec<_>>>()?;
    with_projects(&ctx.db, views)
}

#[tauri::command(async)]
pub fn documents_stats(app: AppHandle) -> UiResult<LibraryStats> {
    let ctx = docs_bridge::context(&app)?;
    Ok(ctx.db.with(repo::stats)?)
}

#[tauri::command(async)]
pub fn documents_get(app: AppHandle, id: String) -> UiResult<DocumentView> {
    let d = docs_bridge::document(&app, &id)?;
    let ctx = docs_bridge::context(&app)?;
    let mut views = with_projects(&ctx.db, vec![view(&app, d)?])?;
    views.pop().ok_or_else(|| UiError::invalid("not_found"))
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

/// Saves a manual correction of a page's text (viewer text tab). The knowledge indexer picks the
/// document up again (its text hash changed) and the library refreshes through `documents://changed`.
#[tauri::command(async)]
pub fn documents_set_page_text(app: AppHandle, id: String, page_no: u32, text: String) -> UiResult<bool> {
    let ctx = docs_bridge::context(&app)?;
    let ok = import::set_page_text(&ctx.db, &id, page_no, &text)?;
    if ok {
        docs_bridge::emit_changed(&app, "updated", vec![id]);
        crate::knowledge_bridge::auto_index(&app);
    }
    Ok(ok)
}

/// The corrections of a page (`page_no`) or of the whole document (`None`), newest first, with whether
/// the page can undo / redo (0.3.0 item 7).
#[tauri::command(async)]
pub fn documents_page_text_history(app: AppHandle, id: String, page_no: Option<u32>) -> UiResult<PageTextHistory> {
    let ctx = docs_bridge::context(&app)?;
    Ok(import::page_text_history(&ctx.db, &id, page_no)?)
}

fn after_text_revert(app: &AppHandle, id: &str, r: Option<PageTextRevert>) -> Option<PageTextRevert> {
    if r.is_some() {
        docs_bridge::emit_changed(app, "updated", vec![id.to_string()]);
        crate::knowledge_bridge::auto_index(app);
    }
    r
}

/// Ctrl+Z in the viewer: takes back the newest correction of the page (`None` when there is nothing to undo).
#[tauri::command(async)]
pub fn documents_undo_page_text(app: AppHandle, id: String, page_no: u32) -> UiResult<Option<PageTextRevert>> {
    let ctx = docs_bridge::context(&app)?;
    let doc = docs_bridge::document(&app, &id)?;
    let r = import::undo_page_text(&ctx.deps(), &doc, page_no)?;
    Ok(after_text_revert(&app, &id, r))
}

/// Ctrl+Shift+Z / Ctrl+Y in the viewer: re-applies the oldest undone correction of the page.
#[tauri::command(async)]
pub fn documents_redo_page_text(app: AppHandle, id: String, page_no: u32) -> UiResult<Option<PageTextRevert>> {
    let ctx = docs_bridge::context(&app)?;
    let doc = docs_bridge::document(&app, &id)?;
    let r = import::redo_page_text(&ctx.deps(), &doc, page_no)?;
    Ok(after_text_revert(&app, &id, r))
}

/// 이력 tab "이 수정 되돌리기": undoes the correction and every later one of its page.
#[tauri::command(async)]
pub fn documents_revert_page_text(app: AppHandle, id: String, history_id: i64) -> UiResult<PageTextRevert> {
    let ctx = docs_bridge::context(&app)?;
    let doc = docs_bridge::document(&app, &id)?;
    let r = import::revert_page_text_to(&ctx.deps(), &doc, history_id)?;
    Ok(after_text_revert(&app, &id, Some(r)).expect("revert result"))
}

/// The word layout of a page for the on-image text selection (computed and cached on first use;
/// `None` when the page has no text or no image yet).
#[tauri::command(async)]
pub fn documents_page_layout(app: AppHandle, id: String, page_no: u32) -> UiResult<Option<PageLayout>> {
    let ctx = docs_bridge::context(&app)?;
    let doc = docs_bridge::document(&app, &id)?;
    Ok(import::ensure_layout(&ctx.deps(), &doc, page_no)?)
}

/// Replaces the words `start..end` of a page's layout with `text` (drag-select on the page image →
/// 수정). Empty text deletes the words. The page text is rewritten in place (`text_source = manual`),
/// the knowledge indexer picks the document up again and the library refreshes.
#[tauri::command(async)]
pub fn documents_edit_page_words(
    app: AppHandle,
    id: String,
    page_no: u32,
    start: usize,
    end: usize,
    text: String,
) -> UiResult<PageLayout> {
    let ctx = docs_bridge::context(&app)?;
    let doc = docs_bridge::document(&app, &id)?;
    let layout = import::edit_words(&ctx.deps(), &doc, page_no, start, end, &text)?;
    docs_bridge::emit_changed(&app, "updated", vec![id]);
    crate::knowledge_bridge::auto_index(&app);
    Ok(layout)
}

/// Adds text the recogniser missed over a box drawn on the page image (fractions `[x, y, w, h]`).
#[tauri::command(async)]
pub fn documents_add_page_words(
    app: AppHandle,
    id: String,
    page_no: u32,
    bbox: [f32; 4],
    text: String,
) -> UiResult<PageLayout> {
    let ctx = docs_bridge::context(&app)?;
    let doc = docs_bridge::document(&app, &id)?;
    let layout = import::add_words(&ctx.deps(), &doc, page_no, (bbox[0], bbox[1], bbox[2], bbox[3]), &text)?;
    docs_bridge::emit_changed(&app, "updated", vec![id]);
    crate::knowledge_bridge::auto_index(&app);
    Ok(layout)
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

/// Starts the import job; returns the job id. A plan with nothing new to import is refused
/// (`nothing to import`) — unless it carries a collection / project and met documents that are
/// already registered: then the job only puts those documents into the target and lists them in
/// its result (`ImportReport.existing`).
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
    if plan.new_count + plan.relink_count == 0 && !plan.attaches_existing() {
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
