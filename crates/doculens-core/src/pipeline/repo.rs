//! SQL for `schemas`, `extraction_runs`, `fields`, `field_history` and the
//! pipeline columns of `documents` / `pages`. Functions take a connection or
//! transaction so callers compose them atomically; expected failures surface
//! as `rusqlite::Error::InvalidParameterName(code)` (typed UI errors).

use chrono::{DateTime, Utc};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde_json::Value;

use crate::documents::ReviewStatus;

use super::schema::{self, SchemaDef, SchemaDraft, SchemaRecord, SchemaSource};
use super::{
    DocTypeInfo, ExtractionRun, FieldChange, FieldKind, FieldRecord, FieldStatus, PageClass, PipelineStatus,
    ReviewReason, ValidationIssue,
};

fn ts(d: &DateTime<Utc>) -> String {
    d.to_rfc3339()
}

fn parse_ts(s: Option<String>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc)))
}

pub(crate) fn typed(code: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(code.to_string())
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

const SCHEMA_COLUMNS: &str =
    "id, name, name_en, doc_type, source, version, description_ko, description_en, definition, \
form_document_id, builtin_key, enabled, is_default, use_count, created_at, updated_at";

fn row_to_schema(r: &Row<'_>) -> rusqlite::Result<SchemaRecord> {
    let source: String = r.get(4)?;
    let def: String = r.get(8)?;
    Ok(SchemaRecord {
        id: r.get(0)?,
        name: r.get(1)?,
        name_en: r.get(2)?,
        doc_type: r.get(3)?,
        source: SchemaSource::parse(&source).unwrap_or_default(),
        version: r.get::<_, i64>(5)?.max(1) as u32,
        description_ko: r.get(6)?,
        description_en: r.get(7)?,
        definition: serde_json::from_str(&def).unwrap_or_default(),
        form_document_id: r.get(9)?,
        builtin_key: r.get(10)?,
        enabled: r.get::<_, i64>(11)? != 0,
        is_default: r.get::<_, i64>(12)? != 0,
        use_count: r.get::<_, i64>(13)?.max(0) as u64,
        created_at: parse_ts(r.get(14)?).unwrap_or_else(Utc::now),
        updated_at: parse_ts(r.get(15)?).unwrap_or_else(Utc::now),
    })
}

/// Inserts the bundled sample forms that are missing (and refreshes unmodified
/// ones when `BUILTIN_VERSION` moved). Called when a workspace opens.
pub fn ensure_builtins(conn: &Connection) -> rusqlite::Result<usize> {
    let now = ts(&Utc::now());
    let mut added = 0;
    for b in schema::builtins() {
        let existing: Option<(String, i64)> = conn
            .query_row("SELECT id, version FROM schemas WHERE builtin_key = ?1", [b.key], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let def = serde_json::to_string(&b.def).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        match existing {
            None => {
                // A user schema may already own the doc type (registered before the sample existed).
                let taken: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM schemas WHERE doc_type = ?1 AND is_default = 1",
                    [b.doc_type],
                    |r| r.get(0),
                )?;
                conn.execute(
                    "INSERT INTO schemas (id, name, name_en, doc_type, source, version, description_ko, description_en, \
                     definition, form_document_id, builtin_key, enabled, is_default, use_count, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, 'builtin', ?5, ?6, ?7, ?8, NULL, ?9, 1, ?10, 0, ?11, ?11)",
                    params![
                        format!("builtin-{}", b.key),
                        b.name_ko,
                        b.name_en,
                        b.doc_type,
                        schema::BUILTIN_VERSION as i64,
                        b.description_ko,
                        b.description_en,
                        def,
                        b.key,
                        (taken == 0) as i64,
                        now,
                    ],
                )?;
                added += 1;
            }
            Some((id, version)) if (version as u32) < schema::BUILTIN_VERSION => {
                // Only refresh samples the user never edited (edits bump the version past the bundled one).
                let edited: i64 =
                    conn.query_row("SELECT COUNT(*) FROM schema_versions WHERE schema_id = ?1", [&id], |r| r.get(0))?;
                if edited == 0 {
                    conn.execute(
                        "UPDATE schemas SET definition = ?2, version = ?3, updated_at = ?4 WHERE id = ?1",
                        params![id, def, schema::BUILTIN_VERSION as i64, now],
                    )?;
                }
            }
            Some(_) => {}
        }
    }
    Ok(added)
}

pub fn list_schemas(conn: &Connection) -> rusqlite::Result<Vec<SchemaRecord>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {SCHEMA_COLUMNS} FROM schemas ORDER BY use_count DESC, (source = 'builtin'), created_at, name"
    ))?;
    let rows = st.query_map([], row_to_schema)?;
    rows.collect()
}

pub fn get_schema(conn: &Connection, id: &str) -> rusqlite::Result<Option<SchemaRecord>> {
    conn.query_row(&format!("SELECT {SCHEMA_COLUMNS} FROM schemas WHERE id = ?1"), [id], row_to_schema).optional()
}

/// Default schema of a document type (form/user/nl before builtin, then most used).
pub fn schema_for_doc_type(conn: &Connection, doc_type: &str) -> rusqlite::Result<Option<SchemaRecord>> {
    conn.query_row(
        &format!(
            "SELECT {SCHEMA_COLUMNS} FROM schemas WHERE doc_type = ?1 AND enabled = 1 \
             ORDER BY is_default DESC, (source = 'builtin'), use_count DESC, updated_at DESC LIMIT 1"
        ),
        [doc_type],
        row_to_schema,
    )
    .optional()
}

/// Document types the classifier may answer with: one entry per enabled schema type.
pub fn doc_types(conn: &Connection) -> rusqlite::Result<Vec<DocTypeInfo>> {
    let all = list_schemas(conn)?;
    let mut out: Vec<DocTypeInfo> = Vec::new();
    for s in all.into_iter().filter(|s| s.enabled) {
        if let Some(e) = out.iter_mut().find(|d| d.slug == s.doc_type) {
            e.use_count += s.use_count;
            if !e.builtin && s.is_default {
                e.schema_id = Some(s.id.clone());
            }
            continue;
        }
        out.push(DocTypeInfo {
            slug: s.doc_type.clone(),
            label_ko: s.name.clone(),
            label_en: s.name_en.clone().unwrap_or_else(|| s.name.clone()),
            schema_id: Some(s.id.clone()),
            builtin: s.source == SchemaSource::Builtin,
            use_count: s.use_count,
        });
    }
    // Registration order / usage first (design §3.4.0 "등록 순서·사용 빈도"); samples after user forms.
    out.sort_by(|a, b| a.builtin.cmp(&b.builtin).then(b.use_count.cmp(&a.use_count)));
    Ok(out)
}

/// Saves a draft: inserts a new schema or stores a new version of an existing one.
pub fn save_schema(conn: &Connection, draft: &SchemaDraft) -> rusqlite::Result<SchemaRecord> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err(typed("schema_name_empty"));
    }
    let doc_type = draft.doc_type.trim();
    if !schema::is_key(doc_type) {
        return Err(typed("schema_doc_type_invalid"));
    }
    let issues = schema::lint(&draft.definition);
    if schema::has_errors(&issues) {
        return Err(typed("schema_invalid"));
    }
    let def =
        serde_json::to_string(&draft.definition).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let now = ts(&Utc::now());
    let id = match &draft.id {
        Some(id) => {
            let Some(current) = get_schema(conn, id)? else { return Err(typed("schema_not_found")) };
            // Keep the previous definition as a version row before overwriting.
            conn.execute(
                "INSERT OR IGNORE INTO schema_versions (schema_id, version, definition, note, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    current.version as i64,
                    serde_json::to_string(&current.definition).unwrap_or_default(),
                    draft.note,
                    now
                ],
            )?;
            // Moving a schema to another type releases the old default slot.
            if current.doc_type != doc_type {
                conn.execute(
                    "UPDATE schemas SET is_default = 1 WHERE doc_type = ?1 AND id = \
                     (SELECT id FROM schemas WHERE doc_type = ?1 AND id <> ?2 ORDER BY (source = 'builtin'), updated_at DESC LIMIT 1)",
                    params![current.doc_type, id],
                )?;
            }
            conn.execute(
                "UPDATE schemas SET name = ?2, name_en = ?3, doc_type = ?4, description_ko = ?5, description_en = ?6, \
                 definition = ?7, version = version + 1, form_document_id = COALESCE(?8, form_document_id), updated_at = ?9 \
                 WHERE id = ?1",
                params![
                    id,
                    name,
                    draft.name_en.as_deref().map(str::trim).filter(|s| !s.is_empty()),
                    doc_type,
                    draft.description_ko,
                    draft.description_en,
                    def,
                    draft.form_document_id,
                    now
                ],
            )?;
            id.clone()
        }
        None => {
            let id = crate::documents::new_id();
            // The first non-builtin schema of a type becomes its default (user forms win over samples).
            let has_user_default: i64 = conn.query_row(
                "SELECT COUNT(*) FROM schemas WHERE doc_type = ?1 AND is_default = 1 AND source <> 'builtin'",
                [doc_type],
                |r| r.get(0),
            )?;
            let is_default = has_user_default == 0;
            if is_default {
                conn.execute("UPDATE schemas SET is_default = 0 WHERE doc_type = ?1", [doc_type])?;
            }
            conn.execute(
                "INSERT INTO schemas (id, name, name_en, doc_type, source, version, description_ko, description_en, \
                 definition, form_document_id, builtin_key, enabled, is_default, use_count, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8, ?9, NULL, 1, ?10, 0, ?11, ?11)",
                params![
                    id,
                    name,
                    draft.name_en.as_deref().map(str::trim).filter(|s| !s.is_empty()),
                    doc_type,
                    draft.source.as_str(),
                    draft.description_ko,
                    draft.description_en,
                    def,
                    draft.form_document_id,
                    is_default as i64,
                    now
                ],
            )?;
            id
        }
    };
    get_schema(conn, &id)?.ok_or_else(|| typed("schema_not_found"))
}

/// Makes `id` the default schema of its type.
pub fn set_default_schema(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    let Some(s) = get_schema(conn, id)? else { return Err(typed("schema_not_found")) };
    conn.execute("UPDATE schemas SET is_default = 0 WHERE doc_type = ?1", [&s.doc_type])?;
    conn.execute("UPDATE schemas SET is_default = 1, updated_at = ?2 WHERE id = ?1", params![id, ts(&Utc::now())])?;
    Ok(())
}

pub fn set_schema_enabled(conn: &Connection, id: &str, enabled: bool) -> rusqlite::Result<()> {
    let n = conn.execute(
        "UPDATE schemas SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, enabled as i64, ts(&Utc::now())],
    )?;
    if n == 0 {
        return Err(typed("schema_not_found"));
    }
    Ok(())
}

/// Deletes a non-builtin schema (runs keep their `schema_id` as a loose reference).
pub fn delete_schema(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let Some(s) = get_schema(conn, id)? else { return Ok(false) };
    if s.source == SchemaSource::Builtin {
        return Err(typed("schema_builtin"));
    }
    conn.execute("DELETE FROM schemas WHERE id = ?1", [id])?;
    if s.is_default {
        conn.execute(
            "UPDATE schemas SET is_default = 1 WHERE doc_type = ?1 AND id = \
             (SELECT id FROM schemas WHERE doc_type = ?1 ORDER BY (source = 'builtin'), updated_at DESC LIMIT 1)",
            [&s.doc_type],
        )?;
    }
    conn.execute("UPDATE documents SET schema_id = NULL WHERE schema_id = ?1", [id])?;
    Ok(true)
}

pub fn bump_schema_use(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE schemas SET use_count = use_count + 1 WHERE id = ?1", [id])?;
    Ok(())
}

/// One stored older definition of a schema.
pub type SchemaVersion = (u32, SchemaDef, Option<String>, DateTime<Utc>);

/// Older definitions of a schema (newest first).
pub fn schema_versions(conn: &Connection, id: &str) -> rusqlite::Result<Vec<SchemaVersion>> {
    let mut st = conn.prepare_cached(
        "SELECT version, definition, note, created_at FROM schema_versions WHERE schema_id = ?1 ORDER BY version DESC",
    )?;
    let rows = st.query_map([id], |r| {
        let def: String = r.get(1)?;
        Ok((
            r.get::<_, i64>(0)?.max(0) as u32,
            serde_json::from_str::<SchemaDef>(&def).unwrap_or_default(),
            r.get(2)?,
            parse_ts(r.get(3)?).unwrap_or_else(Utc::now),
        ))
    })?;
    rows.collect()
}

// ---------------------------------------------------------------------------
// Documents / pages
// ---------------------------------------------------------------------------

/// Records a page classification (and the classifier's rotation).
pub fn set_page_class(conn: &Connection, doc_id: &str, page_no: u32, class: &PageClass) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE pages SET classified_at = ?3, page_doc_type = ?4, page_language = ?5, page_rotation = ?6, \
         is_blank = CASE WHEN ?7 THEN 1 ELSE is_blank END WHERE document_id = ?1 AND page_no = ?2",
        params![
            doc_id,
            page_no as i64,
            ts(&Utc::now()),
            class.doc_type,
            class.language,
            class.rotation as i64,
            class.is_blank
        ],
    )?;
    Ok(())
}

/// Records the document-level classification. The classifier never overrides a
/// manual choice (`doc_type_source = 'manual'`) unless `force`.
pub fn set_classified(
    conn: &Connection,
    doc_id: &str,
    doc_type: &str,
    language: Option<&str>,
    force: bool,
) -> rusqlite::Result<bool> {
    let now = ts(&Utc::now());
    let n = if force {
        conn.execute(
            "UPDATE documents SET doc_type = ?2, doc_type_source = 'auto', language = COALESCE(?3, language), \
             classified_at = ?4, pipeline_error = NULL, updated_at = ?4 WHERE id = ?1",
            params![doc_id, doc_type, language, now],
        )?
    } else {
        conn.execute(
            "UPDATE documents SET doc_type = CASE WHEN doc_type_source = 'manual' THEN doc_type ELSE ?2 END, \
             doc_type_source = CASE WHEN doc_type_source = 'manual' THEN 'manual' ELSE 'auto' END, \
             language = COALESCE(?3, language), classified_at = ?4, pipeline_error = NULL, updated_at = ?4 WHERE id = ?1",
            params![doc_id, doc_type, language, now],
        )?
    };
    Ok(n > 0)
}

/// Stores OCR output for a page (`text_source = 'ocr'`) and bumps the document
/// so the knowledge indexer re-chunks it.
pub fn set_page_ocr(
    conn: &Connection,
    doc_id: &str,
    page_no: u32,
    text: Option<&str>,
    profile: &str,
    model: &str,
) -> rusqlite::Result<()> {
    let now = ts(&Utc::now());
    let text = text.map(str::trim).filter(|t| !t.is_empty());
    conn.execute(
        "UPDATE pages SET text = ?3, has_text = ?4, text_source = 'ocr', ocr_at = ?5, ocr_profile = ?6, ocr_model = ?7 \
         WHERE document_id = ?1 AND page_no = ?2",
        params![doc_id, page_no as i64, text, text.is_some() as i64, now, profile, model],
    )?;
    conn.execute("UPDATE documents SET ocr_at = ?2, updated_at = ?2 WHERE id = ?1", params![doc_id, now])?;
    Ok(())
}

pub fn set_pipeline_error(conn: &Connection, doc_id: &str, error: Option<&str>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE documents SET pipeline_error = ?2, updated_at = ?3 WHERE id = ?1",
        params![doc_id, error, ts(&Utc::now())],
    )?;
    Ok(())
}

pub fn set_schema_override(conn: &Connection, doc_id: &str, schema_id: Option<&str>) -> rusqlite::Result<()> {
    if let Some(id) = schema_id {
        if get_schema(conn, id)?.is_none() {
            return Err(typed("schema_not_found"));
        }
    }
    let n = conn.execute(
        "UPDATE documents SET schema_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![doc_id, schema_id, ts(&Utc::now())],
    )?;
    if n == 0 {
        return Err(typed("document_not_found"));
    }
    Ok(())
}

/// Marks the latest run on the document and derives `review_status` / `status`.
#[allow(clippy::too_many_arguments)]
pub fn set_extracted(
    conn: &Connection,
    doc_id: &str,
    run_id: &str,
    confidence: Option<f64>,
    review_pending: bool,
    vendor: Option<&str>,
    doc_date: Option<&str>,
    total_amount: Option<f64>,
) -> rusqlite::Result<()> {
    let now = ts(&Utc::now());
    conn.execute(
        "UPDATE documents SET last_run_id = ?2, extracted_at = ?3, confidence = ?4, review_status = ?5, \
         status = CASE WHEN status IN ('rendered', 'new', 'extracted', 'reviewed') THEN ?6 ELSE status END, \
         vendor = COALESCE(?7, vendor), doc_date = COALESCE(?8, doc_date), total_amount = COALESCE(?9, total_amount), \
         pipeline_error = NULL, updated_at = ?3 WHERE id = ?1",
        params![
            doc_id,
            run_id,
            now,
            confidence,
            if review_pending { "pending" } else { "none" },
            if review_pending { "extracted" } else { "reviewed" },
            vendor,
            doc_date,
            total_amount
        ],
    )?;
    Ok(())
}

pub fn set_review_status(conn: &Connection, doc_id: &str, status: ReviewStatus, promote: bool) -> rusqlite::Result<()> {
    let now = ts(&Utc::now());
    conn.execute(
        "UPDATE documents SET review_status = ?2, status = CASE WHEN ?3 AND status IN ('extracted', 'rendered', 'new') \
         THEN 'reviewed' ELSE status END, updated_at = ?4 WHERE id = ?1",
        params![doc_id, status.as_str(), promote, now],
    )?;
    Ok(())
}

/// Candidate documents for a pipeline run (rendered or text-only, not deleted).
pub fn candidate_documents(
    conn: &Connection,
    ids: &[String],
) -> rusqlite::Result<Vec<crate::documents::DocumentRecord>> {
    let mut out = Vec::new();
    for group in ids.chunks(400) {
        let mut st = conn.prepare(&format!(
            "SELECT id FROM documents WHERE deleted_at IS NULL AND status NOT IN ('importing', 'missing', 'error') \
             AND id IN ({})",
            placeholders(group.len())
        ))?;
        let found: Vec<String> =
            st.query_map(params_from_iter(group.iter()), |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for id in group.iter().filter(|id| found.contains(id)) {
            if let Some(d) = crate::documents::repo::get_document(conn, id)? {
                out.push(d);
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

const RUN_COLUMNS: &str = "id, document_id, schema_id, schema_version, doc_type, model, prompt_version, profile, precise, \
status, error, started_at, finished_at, duration_ms, prompt_tokens, completion_tokens, pages_done, confidence, field_count, \
review_count";

fn row_to_run(r: &Row<'_>) -> rusqlite::Result<ExtractionRun> {
    Ok(ExtractionRun {
        id: r.get(0)?,
        document_id: r.get(1)?,
        schema_id: r.get(2)?,
        schema_version: r.get::<_, Option<i64>>(3)?.map(|v| v.max(0) as u32),
        doc_type: r.get(4)?,
        model: r.get(5)?,
        prompt_version: r.get(6)?,
        profile: r.get(7)?,
        precise: r.get::<_, i64>(8)? != 0,
        status: r.get(9)?,
        error: r.get(10)?,
        started_at: parse_ts(r.get(11)?).unwrap_or_else(Utc::now),
        finished_at: parse_ts(r.get(12)?),
        duration_ms: r.get::<_, Option<i64>>(13)?.map(|v| v.max(0) as u64),
        prompt_tokens: r.get::<_, i64>(14)?.max(0) as u64,
        completion_tokens: r.get::<_, i64>(15)?.max(0) as u64,
        pages_done: r.get::<_, i64>(16)?.max(0) as u32,
        confidence: r.get(17)?,
        field_count: r.get::<_, i64>(18)?.max(0) as u32,
        review_count: r.get::<_, i64>(19)?.max(0) as u32,
    })
}

pub struct NewRun<'a> {
    pub id: &'a str,
    pub document_id: &'a str,
    pub schema: Option<&'a SchemaRecord>,
    pub doc_type: Option<&'a str>,
    pub model: &'a str,
    pub profile: &'a str,
    pub precise: bool,
}

pub fn insert_run(conn: &Connection, run: &NewRun<'_>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO extraction_runs (id, document_id, schema_id, schema_version, doc_type, model, prompt_version, profile, \
         precise, status, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'running', ?10)",
        params![
            run.id,
            run.document_id,
            run.schema.map(|s| s.id.as_str()),
            run.schema.map(|s| s.version as i64),
            run.doc_type,
            run.model,
            super::PROMPT_VERSION,
            run.profile,
            run.precise as i64,
            ts(&Utc::now())
        ],
    )?;
    Ok(())
}

pub struct RunOutcome<'a> {
    pub status: &'a str,
    pub error: Option<&'a str>,
    pub duration_ms: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub pages_done: u32,
    pub result_json: Option<&'a Value>,
    pub confidence: Option<f64>,
    pub field_count: u32,
    pub review_count: u32,
}

pub fn finish_run(conn: &Connection, id: &str, out: &RunOutcome<'_>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE extraction_runs SET status = ?2, error = ?3, finished_at = ?4, duration_ms = ?5, prompt_tokens = ?6, \
         completion_tokens = ?7, pages_done = ?8, result_json = ?9, confidence = ?10, field_count = ?11, review_count = ?12 \
         WHERE id = ?1",
        params![
            id,
            out.status,
            out.error,
            ts(&Utc::now()),
            out.duration_ms as i64,
            out.prompt_tokens as i64,
            out.completion_tokens as i64,
            out.pages_done as i64,
            out.result_json.map(|v| v.to_string()),
            out.confidence,
            out.field_count as i64,
            out.review_count as i64
        ],
    )?;
    Ok(())
}

pub fn list_runs(conn: &Connection, doc_id: &str) -> rusqlite::Result<Vec<ExtractionRun>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {RUN_COLUMNS} FROM extraction_runs WHERE document_id = ?1 ORDER BY started_at DESC"
    ))?;
    let rows = st.query_map([doc_id], row_to_run)?;
    rows.collect()
}

pub fn get_run(conn: &Connection, id: &str) -> rusqlite::Result<Option<ExtractionRun>> {
    conn.query_row(&format!("SELECT {RUN_COLUMNS} FROM extraction_runs WHERE id = ?1"), [id], row_to_run).optional()
}

/// Raw merged model output of a run (mirror / re-validation).
pub fn run_result(conn: &Connection, id: &str) -> rusqlite::Result<Option<Value>> {
    let s: Option<Option<String>> =
        conn.query_row("SELECT result_json FROM extraction_runs WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    Ok(s.flatten().and_then(|s| serde_json::from_str(&s).ok()))
}

/// Runs left `running` by a crash → `failed` (startup housekeeping).
pub fn fail_stale_runs(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE extraction_runs SET status = 'failed', error = 'interrupted', finished_at = ?1 WHERE status = 'running'",
        [ts(&Utc::now())],
    )
}

// ---------------------------------------------------------------------------
// Fields
// ---------------------------------------------------------------------------

const FIELD_COLUMNS: &str =
    "id, run_id, document_id, path, key, row, label_ko, label_en, kind, required, value, normalized, \
original_value, page, bbox, evidence, confidence, model_confidence, status, review_reason, validation, reviewed_at";

fn row_to_field(r: &Row<'_>) -> rusqlite::Result<FieldRecord> {
    let kind: String = r.get(8)?;
    let bbox: Option<String> = r.get(14)?;
    let status: String = r.get(18)?;
    let reason: Option<String> = r.get(19)?;
    let validation: Option<String> = r.get(20)?;
    Ok(FieldRecord {
        id: r.get(0)?,
        run_id: r.get(1)?,
        document_id: r.get(2)?,
        path: r.get(3)?,
        key: r.get(4)?,
        row: r.get::<_, Option<i64>>(5)?.map(|v| v.max(0) as u32),
        label_ko: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
        label_en: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
        kind: FieldKind::parse(&kind).unwrap_or_default(),
        required: r.get::<_, i64>(9)? != 0,
        value: r.get(10)?,
        normalized: r.get(11)?,
        original_value: r.get(12)?,
        page: r.get::<_, Option<i64>>(13)?.map(|v| v.max(0) as u32),
        bbox: bbox.and_then(|b| serde_json::from_str::<[u16; 4]>(&b).ok()),
        evidence: r.get(15)?,
        confidence: r.get(16)?,
        model_confidence: r.get::<_, Option<i64>>(17)?.map(|v| v.clamp(0, 100) as u8),
        status: FieldStatus::parse(&status).unwrap_or_default(),
        review_reason: reason.as_deref().and_then(ReviewReason::parse),
        validation: validation.and_then(|v| serde_json::from_str::<Vec<ValidationIssue>>(&v).ok()).unwrap_or_default(),
        reviewed_at: parse_ts(r.get(21)?),
    })
}

/// Field to insert (id assigned by SQLite).
#[derive(Debug, Clone, Default)]
pub struct NewField {
    pub path: String,
    pub key: String,
    pub row: Option<u32>,
    pub label_ko: String,
    pub label_en: String,
    pub kind: FieldKind,
    pub required: bool,
    pub value: Option<String>,
    pub normalized: Option<String>,
    pub page: Option<u32>,
    pub bbox: Option<[u16; 4]>,
    pub evidence: Option<String>,
    pub confidence: f64,
    pub model_confidence: Option<u8>,
    pub review_reason: Option<ReviewReason>,
    pub validation: Vec<ValidationIssue>,
}

pub fn insert_fields(conn: &Connection, run_id: &str, doc_id: &str, fields: &[NewField]) -> rusqlite::Result<usize> {
    let mut st = conn.prepare_cached(
        "INSERT INTO fields (run_id, document_id, path, key, row, label_ko, label_en, kind, required, value, normalized, \
         original_value, page, bbox, evidence, confidence, model_confidence, status, review_reason, validation) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?10, ?12, ?13, ?14, ?15, ?16, 'auto', ?17, ?18)",
    )?;
    for f in fields {
        st.execute(params![
            run_id,
            doc_id,
            f.path,
            f.key,
            f.row.map(|r| r as i64),
            f.label_ko,
            f.label_en,
            f.kind.as_str(),
            f.required as i64,
            f.value,
            f.normalized,
            f.page.map(|p| p as i64),
            f.bbox.map(|b| serde_json::to_string(&b).unwrap_or_default()),
            f.evidence,
            f.confidence,
            f.model_confidence.map(|c| c as i64),
            f.review_reason.map(|r| r.as_str()),
            if f.validation.is_empty() { None } else { Some(serde_json::to_string(&f.validation).unwrap_or_default()) },
        ])?;
    }
    Ok(fields.len())
}

pub fn fields_of_run(conn: &Connection, run_id: &str) -> rusqlite::Result<Vec<FieldRecord>> {
    let mut st = conn.prepare_cached(&format!("SELECT {FIELD_COLUMNS} FROM fields WHERE run_id = ?1 ORDER BY id"))?;
    let rows = st.query_map([run_id], row_to_field)?;
    rows.collect()
}

/// Fields of the document's latest run (`documents.last_run_id`).
pub fn latest_fields(conn: &Connection, doc_id: &str) -> rusqlite::Result<Vec<FieldRecord>> {
    let run: Option<Option<String>> =
        conn.query_row("SELECT last_run_id FROM documents WHERE id = ?1", [doc_id], |r| r.get(0)).optional()?;
    match run.flatten() {
        Some(run) => fields_of_run(conn, &run),
        None => Ok(Vec::new()),
    }
}

pub fn get_field(conn: &Connection, id: i64) -> rusqlite::Result<Option<FieldRecord>> {
    conn.query_row(&format!("SELECT {FIELD_COLUMNS} FROM fields WHERE id = ?1"), [id], row_to_field).optional()
}

/// Review-queue filter.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ReviewFilter {
    pub document_id: Option<String>,
    pub doc_type: Option<String>,
    pub reason: Option<ReviewReason>,
    /// Include fields whose review is already resolved.
    pub include_resolved: bool,
    pub limit: Option<u32>,
}

/// Fields waiting for review (latest run of each document only).
pub fn review_queue(conn: &Connection, filter: &ReviewFilter) -> rusqlite::Result<Vec<FieldRecord>> {
    let mut sql = format!(
        "SELECT {} FROM fields f JOIN documents d ON d.id = f.document_id AND d.last_run_id = f.run_id \
         WHERE d.deleted_at IS NULL AND f.review_reason IS NOT NULL",
        FIELD_COLUMNS.split(", ").map(|c| format!("f.{c}")).collect::<Vec<_>>().join(", ")
    );
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if !filter.include_resolved {
        sql.push_str(" AND f.status IN ('auto', 'held')");
    }
    if let Some(id) = &filter.document_id {
        sql.push_str(" AND f.document_id = ?");
        args.push(Box::new(id.clone()));
    }
    if let Some(dt) = &filter.doc_type {
        sql.push_str(" AND d.doc_type = ?");
        args.push(Box::new(dt.clone()));
    }
    if let Some(r) = filter.reason {
        sql.push_str(" AND f.review_reason = ?");
        args.push(Box::new(r.as_str().to_string()));
    }
    sql.push_str(" ORDER BY d.extracted_at, f.document_id, f.id");
    if let Some(l) = filter.limit {
        sql.push_str(&format!(" LIMIT {}", l.max(1)));
    }
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(params_from_iter(args.iter().map(|a| a.as_ref())), row_to_field)?;
    rows.collect()
}

/// Applies a review action, recording the change in `field_history`.
pub fn update_field_status(
    conn: &Connection,
    id: i64,
    status: FieldStatus,
    value: Option<&str>,
    normalized: Option<&str>,
    note: Option<&str>,
) -> rusqlite::Result<FieldRecord> {
    let Some(current) = get_field(conn, id)? else { return Err(typed("field_not_found")) };
    let now = ts(&Utc::now());
    let new_value = match status {
        FieldStatus::Corrected => value.map(str::to_string),
        _ => current.value.clone(),
    };
    conn.execute(
        "INSERT INTO field_history (field_id, document_id, path, old_value, new_value, old_status, new_status, changed_at, note) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![id, current.document_id, current.path, current.value, new_value, current.status.as_str(), status.as_str(), now, note],
    )?;
    match status {
        FieldStatus::Corrected => {
            conn.execute(
                "UPDATE fields SET status = 'corrected', value = ?2, normalized = ?3, confidence = 1.0, reviewed_at = ?4 WHERE id = ?1",
                params![id, new_value, normalized, now],
            )?;
        }
        FieldStatus::Approved => {
            conn.execute(
                "UPDATE fields SET status = 'approved', confidence = 1.0, reviewed_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
        }
        FieldStatus::Rejected => {
            conn.execute("UPDATE fields SET status = 'rejected', reviewed_at = ?2 WHERE id = ?1", params![id, now])?;
        }
        FieldStatus::Held | FieldStatus::Auto => {
            conn.execute(
                "UPDATE fields SET status = ?2, reviewed_at = NULL WHERE id = ?1",
                params![id, status.as_str()],
            )?;
        }
    }
    get_field(conn, id)?.ok_or_else(|| typed("field_not_found"))
}

/// Replaces validation / confidence / review reason of a field in place (re-validation).
pub fn update_field_validation(
    conn: &Connection,
    id: i64,
    confidence: f64,
    review_reason: Option<ReviewReason>,
    validation: &[ValidationIssue],
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE fields SET confidence = CASE WHEN status IN ('approved', 'corrected') THEN confidence ELSE ?2 END, \
         review_reason = ?3, validation = ?4 WHERE id = ?1",
        params![
            id,
            confidence,
            review_reason.map(|r| r.as_str()),
            if validation.is_empty() { None } else { Some(serde_json::to_string(validation).unwrap_or_default()) }
        ],
    )?;
    Ok(())
}

pub fn field_history(conn: &Connection, doc_id: &str) -> rusqlite::Result<Vec<FieldChange>> {
    let mut st = conn.prepare_cached(
        "SELECT id, field_id, document_id, path, old_value, new_value, old_status, new_status, changed_at, note \
         FROM field_history WHERE document_id = ?1 ORDER BY changed_at DESC, id DESC",
    )?;
    let rows = st.query_map([doc_id], |r| {
        let old: String = r.get(6)?;
        let new: String = r.get(7)?;
        Ok(FieldChange {
            id: r.get(0)?,
            field_id: r.get(1)?,
            document_id: r.get(2)?,
            path: r.get(3)?,
            old_value: r.get(4)?,
            new_value: r.get(5)?,
            old_status: FieldStatus::parse(&old).unwrap_or_default(),
            new_status: FieldStatus::parse(&new).unwrap_or_default(),
            changed_at: parse_ts(r.get(8)?).unwrap_or_else(Utc::now),
            note: r.get(9)?,
        })
    })?;
    rows.collect()
}

pub fn get_change(conn: &Connection, id: i64) -> rusqlite::Result<Option<FieldChange>> {
    field_history_by_id(conn, id)
}

fn field_history_by_id(conn: &Connection, id: i64) -> rusqlite::Result<Option<FieldChange>> {
    conn.query_row(
        "SELECT id, field_id, document_id, path, old_value, new_value, old_status, new_status, changed_at, note \
         FROM field_history WHERE id = ?1",
        [id],
        |r| {
            let old: String = r.get(6)?;
            let new: String = r.get(7)?;
            Ok(FieldChange {
                id: r.get(0)?,
                field_id: r.get(1)?,
                document_id: r.get(2)?,
                path: r.get(3)?,
                old_value: r.get(4)?,
                new_value: r.get(5)?,
                old_status: FieldStatus::parse(&old).unwrap_or_default(),
                new_status: FieldStatus::parse(&new).unwrap_or_default(),
                changed_at: parse_ts(r.get(8)?).unwrap_or_else(Utc::now),
                note: r.get(9)?,
            })
        },
    )
    .optional()
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

pub fn pipeline_status(conn: &Connection) -> rusqlite::Result<PipelineStatus> {
    let (documents, classified, ocr_done, extracted, review_pending, reviewed, failed): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = conn.query_row(
        "SELECT COUNT(*), SUM(classified_at IS NOT NULL), SUM(ocr_at IS NOT NULL), SUM(extracted_at IS NOT NULL), \
             SUM(review_status = 'pending'), SUM(status = 'reviewed'), SUM(pipeline_error IS NOT NULL) \
             FROM documents WHERE deleted_at IS NULL",
        [],
        |r| {
            Ok((
                r.get(0)?,
                r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                r.get::<_, Option<i64>>(6)?.unwrap_or(0),
            ))
        },
    )?;
    let review_fields: i64 = conn.query_row(
        "SELECT COUNT(*) FROM fields f JOIN documents d ON d.id = f.document_id AND d.last_run_id = f.run_id \
         WHERE d.deleted_at IS NULL AND f.review_reason IS NOT NULL AND f.status IN ('auto', 'held')",
        [],
        |r| r.get(0),
    )?;
    Ok(PipelineStatus {
        documents: documents.max(0) as u64,
        classified: classified.max(0) as u64,
        ocr_done: ocr_done.max(0) as u64,
        extracted: extracted.max(0) as u64,
        review_pending: review_pending.max(0) as u64,
        reviewed: reviewed.max(0) as u64,
        review_fields: review_fields.max(0) as u64,
        failed: failed.max(0) as u64,
    })
}
