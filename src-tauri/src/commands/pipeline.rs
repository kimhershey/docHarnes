//! Pipeline commands (P4): processing, schemas / forms, extracted fields,
//! the review queue, history, tables and split / merge.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use doculens_core::pipeline::repo::{self, ReviewFilter, SchemaVersion};
use doculens_core::pipeline::review::{self, ReviewAction};
use doculens_core::pipeline::schema::{self, LintIssue, SchemaDef, SchemaDraft, SchemaRecord};
use doculens_core::pipeline::{
    forms, ocr, run, DocTypeInfo, ExtractedTable, ExtractionRun, FieldChange, FieldRecord, PipelineStatus,
    ProcessRequest, ReviewReason,
};

use crate::docs_bridge;
use crate::pipeline_bridge::{self, VisionModelInfo};
use crate::protocol;
use crate::state::{AppState, UiError, UiResult};

/// Repository / core errors with a stable code → typed UI errors.
fn typed_error(e: doculens_core::CoreError) -> UiError {
    const CODES: [&str; 12] = [
        "schema_name_empty",
        "schema_doc_type_invalid",
        "schema_invalid",
        "schema_not_found",
        "schema_builtin",
        "document_not_found",
        "field_not_found",
        "field_value_empty",
        "change_not_found",
        "description_empty",
        "no_model",
        "no_text",
    ];
    let m = e.to_string();
    if let Some(code) = CODES.iter().find(|c| m.contains(*c)) {
        return UiError::invalid(*code);
    }
    e.into()
}

#[derive(Debug, Clone, Serialize)]
pub struct PipelineStatusView {
    #[serde(flatten)]
    pub status: PipelineStatus,
    pub running: bool,
    pub queued: usize,
    pub model: VisionModelInfo,
    pub review_threshold: f32,
}

#[tauri::command(async)]
pub fn pipeline_status(app: AppHandle) -> UiResult<PipelineStatusView> {
    let ctx = pipeline_bridge::context(&app)?;
    let status = ctx.db().with_read(repo::pipeline_status)?;
    let state = app.state::<AppState>();
    Ok(PipelineStatusView {
        status,
        running: state.jobs.is_active_kind(&[crate::jobs::JobKind::Pipeline]),
        queued: state.pipeline_queue.lock().map(|q| q.len()).unwrap_or(0),
        model: pipeline_bridge::vision_model_info(&app),
        review_threshold: ctx.settings.review_threshold,
    })
}

#[tauri::command(async)]
pub fn pipeline_process(app: AppHandle, req: ProcessRequest) -> UiResult<Option<String>> {
    pipeline_bridge::process_job(&app, req)
}

// -- schemas ----------------------------------------------------------------

#[tauri::command(async)]
pub fn pipeline_doc_types(app: AppHandle) -> UiResult<Vec<DocTypeInfo>> {
    let ctx = pipeline_bridge::context(&app)?;
    Ok(ctx.db().with_read(repo::doc_types)?)
}

#[tauri::command(async)]
pub fn pipeline_schemas(app: AppHandle) -> UiResult<Vec<SchemaRecord>> {
    let ctx = pipeline_bridge::context(&app)?;
    Ok(ctx.db().with_read(repo::list_schemas)?)
}

#[tauri::command(async)]
pub fn pipeline_schema_get(app: AppHandle, id: String) -> UiResult<SchemaRecord> {
    let ctx = pipeline_bridge::context(&app)?;
    ctx.db().with_read(|c| repo::get_schema(c, &id))?.ok_or_else(|| UiError::invalid("schema_not_found"))
}

#[derive(Debug, Clone, Serialize)]
pub struct SchemaVersionView {
    pub version: u32,
    pub definition: SchemaDef,
    pub note: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[tauri::command(async)]
pub fn pipeline_schema_versions(app: AppHandle, id: String) -> UiResult<Vec<SchemaVersionView>> {
    let ctx = pipeline_bridge::context(&app)?;
    let rows: Vec<SchemaVersion> = ctx.db().with_read(|c| repo::schema_versions(c, &id))?;
    Ok(rows
        .into_iter()
        .map(|(version, definition, note, created_at)| SchemaVersionView { version, definition, note, created_at })
        .collect())
}

#[tauri::command(async)]
pub fn pipeline_schema_save(app: AppHandle, draft: SchemaDraft) -> UiResult<SchemaRecord> {
    let ctx = pipeline_bridge::context(&app)?;
    let saved = ctx.db().with_tx(|tx| repo::save_schema(tx, &draft)).map_err(typed_error)?;
    ctx.emit(&app, "schemas", vec![saved.id.clone()]);
    Ok(saved)
}

#[tauri::command(async)]
pub fn pipeline_schema_delete(app: AppHandle, id: String) -> UiResult<bool> {
    let ctx = pipeline_bridge::context(&app)?;
    let removed = ctx.db().with_tx(|tx| repo::delete_schema(tx, &id)).map_err(typed_error)?;
    ctx.emit(&app, "schemas", vec![id]);
    Ok(removed)
}

#[tauri::command(async)]
pub fn pipeline_schema_set_default(app: AppHandle, id: String) -> UiResult<()> {
    let ctx = pipeline_bridge::context(&app)?;
    ctx.db().with_tx(|tx| repo::set_default_schema(tx, &id)).map_err(typed_error)?;
    ctx.emit(&app, "schemas", vec![id]);
    Ok(())
}

#[tauri::command(async)]
pub fn pipeline_schema_set_enabled(app: AppHandle, id: String, enabled: bool) -> UiResult<()> {
    let ctx = pipeline_bridge::context(&app)?;
    ctx.db().with(|c| repo::set_schema_enabled(c, &id, enabled)).map_err(typed_error)?;
    ctx.emit(&app, "schemas", vec![id]);
    Ok(())
}

/// Pure grammar lint of an editor draft.
#[tauri::command]
pub fn pipeline_schema_lint(definition: SchemaDef) -> Vec<LintIssue> {
    schema::lint(&definition)
}

#[tauri::command(async)]
pub fn pipeline_schema_from_nl(app: AppHandle, description: String, base_id: Option<String>) -> UiResult<SchemaDraft> {
    pipeline_bridge::nl_schema(&app, &description, base_id.as_deref())
}

#[tauri::command(async)]
pub fn pipeline_form_derive(app: AppHandle, document_id: String, name: Option<String>) -> UiResult<SchemaDraft> {
    pipeline_bridge::form_derive(&app, &document_id, name.as_deref()).map_err(|e| {
        if e.code == "invalid" && e.message.contains("no_model") {
            UiError::invalid("no_model")
        } else {
            e
        }
    })
}

#[tauri::command(async)]
pub fn pipeline_form_register(app: AppHandle, draft: SchemaDraft) -> UiResult<SchemaRecord> {
    let ctx = pipeline_bridge::context(&app)?;
    let saved = forms::register(ctx.db(), &draft).map_err(typed_error)?;
    ctx.emit(&app, "schemas", vec![saved.id.clone()]);
    if let Some(doc) = &draft.form_document_id {
        docs_bridge::emit_changed(&app, "updated", vec![doc.clone()]);
    }
    Ok(saved)
}

/// Per-document schema override (viewer "스키마 변경"; `None` = automatic mapping).
#[tauri::command(async)]
pub fn pipeline_document_schema(app: AppHandle, document_id: String, schema_id: Option<String>) -> UiResult<()> {
    let ctx = pipeline_bridge::context(&app)?;
    ctx.db().with(|c| repo::set_schema_override(c, &document_id, schema_id.as_deref())).map_err(typed_error)?;
    docs_bridge::emit_changed(&app, "updated", vec![document_id]);
    Ok(())
}

// -- runs / fields ------------------------------------------------------------

#[tauri::command(async)]
pub fn pipeline_runs(app: AppHandle, document_id: String) -> UiResult<Vec<ExtractionRun>> {
    let ctx = pipeline_bridge::context(&app)?;
    Ok(ctx.db().with_read(|c| repo::list_runs(c, &document_id))?)
}

#[tauri::command(async)]
pub fn pipeline_fields(app: AppHandle, document_id: String, run_id: Option<String>) -> UiResult<Vec<FieldRecord>> {
    let ctx = pipeline_bridge::context(&app)?;
    Ok(ctx.db().with_read(|c| match &run_id {
        Some(r) => repo::fields_of_run(c, r),
        None => repo::latest_fields(c, &document_id),
    })?)
}

#[tauri::command(async)]
pub fn pipeline_field_history(app: AppHandle, document_id: String) -> UiResult<Vec<FieldChange>> {
    let ctx = pipeline_bridge::context(&app)?;
    Ok(ctx.db().with_read(|c| repo::field_history(c, &document_id))?)
}

#[tauri::command(async)]
pub fn pipeline_restore(app: AppHandle, change_id: i64) -> UiResult<FieldRecord> {
    let ctx = pipeline_bridge::context(&app)?;
    let f = review::restore(ctx.db(), change_id).map_err(typed_error)?;
    ctx.emit(&app, "fields", vec![f.document_id.clone()]);
    docs_bridge::emit_changed(&app, "updated", vec![f.document_id.clone()]);
    Ok(f)
}

#[tauri::command(async)]
pub fn pipeline_revalidate(app: AppHandle, document_id: String) -> UiResult<Vec<FieldRecord>> {
    let ctx = pipeline_bridge::context(&app)?;
    let fields = review::revalidate(
        ctx.db(),
        &document_id,
        ctx.settings.review_threshold as f64,
        ctx.settings.audit_sample_percent,
    )
    .map_err(typed_error)?;
    ctx.emit(&app, "fields", vec![document_id.clone()]);
    docs_bridge::emit_changed(&app, "updated", vec![document_id]);
    Ok(fields)
}

// -- review -------------------------------------------------------------------

/// A queue entry with what the review screen shows next to the field.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewItemView {
    #[serde(flatten)]
    pub field: FieldRecord,
    pub document_name: String,
    pub doc_type: Option<String>,
    pub page_image_url: Option<String>,
    pub page_count: u32,
}

#[tauri::command(async)]
pub fn pipeline_review_queue(app: AppHandle, filter: Option<ReviewFilter>) -> UiResult<Vec<ReviewItemView>> {
    let ctx = pipeline_bridge::context(&app)?;
    let mut filter = filter.unwrap_or_default();
    if filter.limit.is_none() {
        filter.limit = Some(500);
    }
    let fields = ctx.db().with_read(|c| repo::review_queue(c, &filter))?;
    let mut docs: std::collections::HashMap<String, doculens_core::documents::DocumentRecord> = Default::default();
    let mut out = Vec::with_capacity(fields.len());
    for f in fields {
        if !docs.contains_key(&f.document_id) {
            if let Some(d) = ctx.db().with_read(|c| doculens_core::documents::repo::get_document(c, &f.document_id))? {
                docs.insert(f.document_id.clone(), d);
            }
        }
        let Some(d) = docs.get(&f.document_id) else { continue };
        let version = d.updated_at.timestamp_millis().to_string();
        let page = f.page.unwrap_or(1).max(1);
        out.push(ReviewItemView {
            document_name: d.title.clone().unwrap_or_else(|| d.original_name.clone()),
            doc_type: d.doc_type.clone(),
            page_image_url: (d.page_count > 0)
                .then(|| protocol::page_url(&d.id, page.min(d.page_count.max(1)), false, Some(&version))),
            page_count: d.page_count,
            field: f,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewApplyArgs {
    pub field_ids: Vec<i64>,
    pub action: ReviewAction,
    #[serde(default)]
    pub note: Option<String>,
}

#[tauri::command(async)]
pub fn pipeline_review_apply(app: AppHandle, args: ReviewApplyArgs) -> UiResult<Vec<FieldRecord>> {
    let ctx = pipeline_bridge::context(&app)?;
    if args.field_ids.is_empty() {
        return Err(UiError::invalid("no fields"));
    }
    let fields = review::apply(ctx.db(), &args.field_ids, &args.action, args.note.as_deref()).map_err(typed_error)?;
    let mut ids: Vec<String> = fields.iter().map(|f| f.document_id.clone()).collect();
    ids.sort();
    ids.dedup();
    ctx.emit(&app, "review", ids.clone());
    docs_bridge::emit_changed(&app, "updated", ids);
    Ok(fields)
}

#[tauri::command(async)]
pub fn pipeline_review_approve_document(
    app: AppHandle,
    document_id: String,
    reason: Option<ReviewReason>,
) -> UiResult<usize> {
    let ctx = pipeline_bridge::context(&app)?;
    let n = review::approve_document(ctx.db(), &document_id, reason).map_err(typed_error)?;
    ctx.emit(&app, "review", vec![document_id.clone()]);
    docs_bridge::emit_changed(&app, "updated", vec![document_id]);
    Ok(n)
}

// -- tables / split / merge ---------------------------------------------------

#[tauri::command(async)]
pub fn pipeline_tables(app: AppHandle, document_id: String) -> UiResult<Vec<ExtractedTable>> {
    let ctx = pipeline_bridge::context(&app)?;
    Ok(run::document_tables(ctx.db(), &document_id)?)
}

#[tauri::command(async)]
pub fn pipeline_table_csv(app: AppHandle, document_id: String, page: u32, index: u32) -> UiResult<String> {
    let ctx = pipeline_bridge::context(&app)?;
    let tables = run::document_tables(ctx.db(), &document_id)?;
    let t = tables
        .iter()
        .find(|t| t.page == page && t.index == index)
        .ok_or_else(|| UiError::invalid("table_not_found"))?;
    Ok(ocr::table_csv(t))
}

#[tauri::command(async)]
pub fn pipeline_split(app: AppHandle, document_id: String, boundaries: Vec<u32>) -> UiResult<String> {
    pipeline_bridge::split_job(&app, document_id, boundaries)
}

#[tauri::command(async)]
pub fn pipeline_merge(app: AppHandle, ids: Vec<String>, name: Option<String>) -> UiResult<String> {
    pipeline_bridge::merge_job(&app, ids, name)
}
