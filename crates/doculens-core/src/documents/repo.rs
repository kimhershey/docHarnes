//! SQL for the `documents` / `pages` / `tags` tables. All functions take a
//! connection (or transaction) so callers can compose them atomically.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};

use super::{
    DocumentFilter, DocumentRecord, DocumentRole, DocumentSort, DocumentStatus, FolderCount, LibraryStats, NewDocument,
    NewPage, PageRecord, ReviewStatus, SourceKind,
};

const DOC_COLUMNS: &str = "id, rel_path, original_name, source_kind, content_hash, size_bytes, file_modified_at, imported_at, \
updated_at, status, role, title, doc_type, vendor, doc_date, total_amount, language, page_count, has_text_layer, converted, \
conversion_note, render_dpi, error, import_batch_id, deleted_at, doc_type_source, classified_at, ocr_at, extracted_at, \
last_run_id, schema_id, review_status, confidence, pipeline_error";

fn parse_ts(s: Option<String>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc)))
}

fn ts(d: &DateTime<Utc>) -> String {
    d.to_rfc3339()
}

fn row_to_document(row: &Row<'_>) -> rusqlite::Result<DocumentRecord> {
    let kind: String = row.get(3)?;
    let status: String = row.get(9)?;
    let role: String = row.get(10)?;
    Ok(DocumentRecord {
        id: row.get(0)?,
        rel_path: row.get(1)?,
        original_name: row.get(2)?,
        source_kind: SourceKind::parse(&kind).unwrap_or(SourceKind::Pdf),
        content_hash: row.get(4)?,
        size_bytes: row.get::<_, i64>(5)?.max(0) as u64,
        file_modified_at: parse_ts(row.get(6)?),
        imported_at: parse_ts(row.get(7)?).unwrap_or_else(Utc::now),
        updated_at: parse_ts(row.get(8)?).unwrap_or_else(Utc::now),
        status: DocumentStatus::parse(&status).unwrap_or(DocumentStatus::Error),
        role: DocumentRole::parse(&role).unwrap_or_default(),
        title: row.get(11)?,
        doc_type: row.get(12)?,
        vendor: row.get(13)?,
        doc_date: row.get(14)?,
        total_amount: row.get(15)?,
        language: row.get(16)?,
        page_count: row.get::<_, i64>(17)?.max(0) as u32,
        has_text_layer: row.get::<_, i64>(18)? != 0,
        converted: row.get::<_, i64>(19)? != 0,
        conversion_note: row.get(20)?,
        render_dpi: row.get::<_, Option<i64>>(21)?.map(|v| v.clamp(0, u16::MAX as i64) as u16),
        error: row.get(22)?,
        import_batch_id: row.get(23)?,
        deleted_at: parse_ts(row.get(24)?),
        tags: Vec::new(),
        doc_type_source: row.get(25)?,
        classified_at: parse_ts(row.get(26)?),
        ocr_at: parse_ts(row.get(27)?),
        extracted_at: parse_ts(row.get(28)?),
        last_run_id: row.get(29)?,
        schema_id: row.get(30)?,
        review_status: ReviewStatus::parse(&row.get::<_, String>(31)?).unwrap_or_default(),
        confidence: row.get(32)?,
        pipeline_error: row.get(33)?,
    })
}

fn attach_tags(conn: &Connection, docs: &mut [DocumentRecord]) -> rusqlite::Result<()> {
    if docs.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT dt.document_id, t.name FROM document_tags dt JOIN tags t ON t.id = dt.tag_id ORDER BY t.name",
    )?;
    let mut by_doc: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (id, name) = row?;
        by_doc.entry(id).or_default().push(name);
    }
    for d in docs.iter_mut() {
        if let Some(t) = by_doc.remove(&d.id) {
            d.tags = t;
        }
    }
    Ok(())
}

pub fn insert_document(conn: &Connection, doc: &NewDocument) -> rusqlite::Result<()> {
    let now = ts(&Utc::now());
    conn.execute(
        "INSERT INTO documents (id, rel_path, original_name, source_kind, content_hash, size_bytes, file_modified_at, \
         imported_at, updated_at, status, role, import_batch_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, 'importing', ?9, ?10)",
        params![
            doc.id,
            doc.rel_path,
            doc.original_name,
            doc.source_kind.as_str(),
            doc.content_hash,
            doc.size_bytes as i64,
            doc.file_modified_at.as_ref().map(ts),
            now,
            doc.role.as_str(),
            doc.import_batch_id,
        ],
    )?;
    Ok(())
}

pub fn get_document(conn: &Connection, id: &str) -> rusqlite::Result<Option<DocumentRecord>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {DOC_COLUMNS} FROM documents WHERE id = ?1"))?;
    let doc = stmt.query_row([id], row_to_document).optional()?;
    match doc {
        Some(d) => {
            let mut v = vec![d];
            attach_tags(conn, &mut v)?;
            Ok(v.pop())
        }
        None => Ok(None),
    }
}

pub fn find_by_rel_path(conn: &Connection, rel_path: &str) -> rusqlite::Result<Option<DocumentRecord>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {DOC_COLUMNS} FROM documents WHERE rel_path = ?1"))?;
    stmt.query_row([rel_path], row_to_document).optional()
}

pub fn find_by_hash(conn: &Connection, hash: &str) -> rusqlite::Result<Vec<DocumentRecord>> {
    let mut stmt = conn
        .prepare_cached(&format!("SELECT {DOC_COLUMNS} FROM documents WHERE content_hash = ?1 ORDER BY imported_at"))?;
    let rows = stmt.query_map([hash], row_to_document)?;
    rows.collect()
}

/// Every non-deleted document (for rescans).
pub fn all_documents(conn: &Connection) -> rusqlite::Result<Vec<DocumentRecord>> {
    let mut stmt = conn
        .prepare_cached(&format!("SELECT {DOC_COLUMNS} FROM documents WHERE deleted_at IS NULL ORDER BY rel_path"))?;
    let rows = stmt.query_map([], row_to_document)?;
    rows.collect()
}

pub fn list_documents(conn: &Connection, filter: &DocumentFilter) -> rusqlite::Result<Vec<DocumentRecord>> {
    let mut sql = format!("SELECT {DOC_COLUMNS} FROM documents WHERE 1=1");
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if !filter.include_deleted {
        sql.push_str(" AND deleted_at IS NULL");
    }
    if !filter.statuses.is_empty() {
        let marks = vec!["?"; filter.statuses.len()].join(",");
        sql.push_str(&format!(" AND status IN ({marks})"));
        for s in &filter.statuses {
            args.push(Box::new(s.as_str().to_string()));
        }
    }
    if !filter.kinds.is_empty() {
        let marks = vec!["?"; filter.kinds.len()].join(",");
        sql.push_str(&format!(" AND source_kind IN ({marks})"));
        for k in &filter.kinds {
            args.push(Box::new(k.as_str().to_string()));
        }
    }
    if let Some(folder) = filter.folder.as_deref() {
        if folder.is_empty() {
            sql.push_str(" AND instr(rel_path, '/') = 0");
        } else {
            sql.push_str(" AND (rel_path LIKE ? ESCAPE '\\')");
            args.push(Box::new(format!("{}/%", like_escape(folder))));
        }
    }
    if let Some(q) = filter.query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        sql.push_str(
            " AND (original_name LIKE ? ESCAPE '\\' OR title LIKE ? ESCAPE '\\' OR rel_path LIKE ? ESCAPE '\\')",
        );
        let pat = format!("%{}%", like_escape(q));
        args.push(Box::new(pat.clone()));
        args.push(Box::new(pat.clone()));
        args.push(Box::new(pat));
    }
    if let Some(dt) = filter.doc_type.as_deref().filter(|s| !s.is_empty()) {
        sql.push_str(" AND doc_type = ?");
        args.push(Box::new(dt.to_string()));
    }
    if let Some(tag) = filter.tag.as_deref().filter(|s| !s.is_empty()) {
        sql.push_str(
            " AND id IN (SELECT dt.document_id FROM document_tags dt JOIN tags t ON t.id = dt.tag_id WHERE t.name = ? COLLATE NOCASE)",
        );
        args.push(Box::new(tag.to_string()));
    }
    if let Some(rs) = filter.review_status {
        sql.push_str(" AND review_status = ?");
        args.push(Box::new(rs.as_str().to_string()));
    }
    if let Some(max) = filter.max_confidence {
        sql.push_str(" AND confidence IS NOT NULL AND confidence < ?");
        args.push(Box::new(max));
    }
    if let Some(role) = filter.role {
        sql.push_str(" AND role = ?");
        args.push(Box::new(role.as_str().to_string()));
    }
    sql.push_str(match filter.sort {
        DocumentSort::ImportedDesc => " ORDER BY imported_at DESC, rel_path",
        DocumentSort::ImportedAsc => " ORDER BY imported_at ASC, rel_path",
        DocumentSort::NameAsc => " ORDER BY original_name COLLATE NOCASE ASC, rel_path",
        DocumentSort::NameDesc => " ORDER BY original_name COLLATE NOCASE DESC, rel_path",
        DocumentSort::SizeDesc => " ORDER BY size_bytes DESC, rel_path",
    });
    if let Some(limit) = filter.limit {
        sql.push_str(&format!(" LIMIT {} OFFSET {}", limit.max(1), filter.offset));
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter().map(|a| a.as_ref())), row_to_document)?;
    let mut docs: Vec<DocumentRecord> = rows.collect::<rusqlite::Result<_>>()?;
    attach_tags(conn, &mut docs)?;
    Ok(docs)
}

fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub fn stats(conn: &Connection) -> rusqlite::Result<LibraryStats> {
    let mut out = LibraryStats::default();
    let mut stmt =
        conn.prepare_cached("SELECT status, COUNT(*) FROM documents WHERE deleted_at IS NULL GROUP BY status")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (k, n) = row?;
        out.total += n.max(0) as u64;
        out.by_status.insert(k, n.max(0) as u64);
    }
    let mut stmt = conn
        .prepare_cached("SELECT source_kind, COUNT(*) FROM documents WHERE deleted_at IS NULL GROUP BY source_kind")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (k, n) = row?;
        out.by_kind.insert(k, n.max(0) as u64);
    }
    out.total_pages = conn
        .query_row("SELECT COALESCE(SUM(page_count), 0) FROM documents WHERE deleted_at IS NULL", [], |r| {
            r.get::<_, i64>(0)
        })?
        .max(0) as u64;
    let mut stmt = conn.prepare_cached("SELECT rel_path FROM documents WHERE deleted_at IS NULL")?;
    let mut folders: BTreeMap<String, u64> = BTreeMap::new();
    for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
        let rel = row?;
        let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("").to_string();
        *folders.entry(dir).or_default() += 1;
    }
    out.folders = folders.into_iter().map(|(rel_dir, count)| FolderCount { rel_dir, count }).collect();
    Ok(out)
}

pub fn set_status(conn: &Connection, id: &str, status: DocumentStatus, error: Option<&str>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE documents SET status = ?2, error = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, status.as_str(), error, ts(&Utc::now())],
    )?;
    Ok(())
}

/// Replaces the page rows of a document and updates the page-level summary
/// columns (`page_count`, `has_text_layer`, `render_dpi`).
pub fn replace_pages(
    conn: &Connection,
    id: &str,
    pages: &[NewPage],
    render_dpi: Option<u16>,
    converted: bool,
    conversion_note: Option<&str>,
) -> rusqlite::Result<()> {
    // OCR results, manual corrections and classifications survive a re-render (the pixels did not
    // change); a page that now carries its own text layer drops the OCR copy but keeps a manual one.
    let mut kept: BTreeMap<u32, OcrColumns> = BTreeMap::new();
    {
        let mut old = conn.prepare_cached(
            "SELECT page_no, text, ocr_at, ocr_profile, ocr_model, classified_at, page_doc_type, page_language, page_rotation, \
             text_source, layout FROM pages WHERE document_id = ?1 AND (text_source IN ('ocr', 'manual') OR classified_at IS NOT NULL)",
        )?;
        let rows = old.query_map([id], |r| {
            Ok((
                r.get::<_, i64>(0)?.max(0) as u32,
                OcrColumns {
                    text: r.get(1)?,
                    ocr_at: r.get(2)?,
                    ocr_profile: r.get(3)?,
                    ocr_model: r.get(4)?,
                    classified_at: r.get(5)?,
                    page_doc_type: r.get(6)?,
                    page_language: r.get(7)?,
                    page_rotation: r.get(8)?,
                    text_source: r.get(9)?,
                    layout: r.get(10)?,
                },
            ))
        })?;
        for row in rows {
            let (n, c) = row?;
            kept.insert(n, c);
        }
    }
    conn.execute("DELETE FROM pages WHERE document_id = ?1", [id])?;
    let now = ts(&Utc::now());
    let mut stmt = conn.prepare_cached(
        "INSERT INTO pages (document_id, page_no, width_px, height_px, width_pt, height_pt, dpi, rotation, has_text, text, \
         is_blank, rendered_at, text_source, ocr_at, ocr_profile, ocr_model, classified_at, page_doc_type, page_language, \
         page_rotation, layout) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
    )?;
    let mut has_text = false;
    for p in pages {
        let own = p.text.as_deref().map(str::trim).filter(|t| !t.is_empty());
        let old = kept.get(&p.page_no);
        let manual = old
            .filter(|c| c.text_source.as_deref() == Some("manual"))
            .and_then(|c| c.text.as_deref())
            .filter(|t| !t.trim().is_empty());
        let ocr = old
            .filter(|c| c.text_source.as_deref() == Some("ocr"))
            .filter(|_| own.is_none())
            .and_then(|c| c.text.as_deref())
            .filter(|t| !t.trim().is_empty());
        let text = manual.or(own).or(ocr);
        let text_source = if manual.is_some() {
            Some("manual")
        } else if own.is_some() {
            Some(if converted { "converted" } else { "layer" })
        } else if ocr.is_some() {
            Some("ocr")
        } else {
            None
        };
        has_text |= own.is_some() || manual.is_some();
        // The word layout belongs to the kept text (fractions of the page, so a DPI change keeps it);
        // a page rendered afresh with its own text layer is laid out again on demand.
        let layout = if manual.is_some() || ocr.is_some() { old.and_then(|c| c.layout.clone()) } else { None };
        stmt.execute(params![
            id,
            p.page_no as i64,
            p.width_px as i64,
            p.height_px as i64,
            p.width_pt,
            p.height_pt,
            p.dpi.map(|d| d as i64),
            p.rotation as i64,
            text.is_some() as i64,
            text,
            p.is_blank.map(|b| b as i64),
            now,
            text_source,
            ocr.and(old.and_then(|c| c.ocr_at.clone())),
            ocr.and(old.and_then(|c| c.ocr_profile.clone())),
            ocr.and(old.and_then(|c| c.ocr_model.clone())),
            old.and_then(|c| c.classified_at.clone()),
            old.and_then(|c| c.page_doc_type.clone()),
            old.and_then(|c| c.page_language.clone()),
            old.and_then(|c| c.page_rotation),
            layout,
        ])?;
    }
    conn.execute(
        "UPDATE documents SET page_count = ?2, has_text_layer = ?3, render_dpi = ?4, converted = ?5, conversion_note = ?6, \
         updated_at = ?7 WHERE id = ?1",
        params![
            id,
            pages.len() as i64,
            has_text as i64,
            render_dpi.map(|d| d as i64),
            converted as i64,
            conversion_note,
            now
        ],
    )?;
    Ok(())
}

/// OCR / manual-text / classification columns carried over a re-render.
struct OcrColumns {
    text: Option<String>,
    ocr_at: Option<String>,
    ocr_profile: Option<String>,
    ocr_model: Option<String>,
    classified_at: Option<String>,
    page_doc_type: Option<String>,
    page_language: Option<String>,
    page_rotation: Option<i64>,
    text_source: Option<String>,
    layout: Option<String>,
}

pub fn list_pages(conn: &Connection, id: &str, with_text: bool) -> rusqlite::Result<Vec<PageRecord>> {
    let mut stmt = conn.prepare_cached(
        "SELECT document_id, page_no, width_px, height_px, width_pt, height_pt, dpi, rotation, has_text, text, is_blank, \
         rendered_at, text_source, ocr_at, ocr_profile, ocr_model, classified_at, page_doc_type, page_language, page_rotation \
         FROM pages WHERE document_id = ?1 ORDER BY page_no",
    )?;
    let rows = stmt.query_map([id], |r| {
        Ok(PageRecord {
            document_id: r.get(0)?,
            page_no: r.get::<_, i64>(1)?.max(0) as u32,
            width_px: r.get::<_, i64>(2)?.max(0) as u32,
            height_px: r.get::<_, i64>(3)?.max(0) as u32,
            width_pt: r.get(4)?,
            height_pt: r.get(5)?,
            dpi: r.get::<_, Option<i64>>(6)?.map(|v| v.clamp(0, u16::MAX as i64) as u16),
            rotation: r.get::<_, i64>(7)?.rem_euclid(360) as u16,
            has_text: r.get::<_, i64>(8)? != 0,
            text: if with_text { r.get(9)? } else { None },
            is_blank: r.get::<_, Option<i64>>(10)?.map(|v| v != 0),
            rendered_at: parse_ts(r.get(11)?),
            text_source: r.get(12)?,
            ocr_at: parse_ts(r.get(13)?),
            ocr_profile: r.get(14)?,
            ocr_model: r.get(15)?,
            classified_at: parse_ts(r.get(16)?),
            page_doc_type: r.get(17)?,
            page_language: r.get(18)?,
            page_rotation: r.get::<_, Option<i64>>(19)?.map(|v| v.rem_euclid(360) as u16),
        })
    })?;
    rows.collect()
}

pub fn get_page(conn: &Connection, id: &str, page_no: u32) -> rusqlite::Result<Option<PageRecord>> {
    Ok(list_pages(conn, id, true)?.into_iter().find(|p| p.page_no == page_no))
}

/// Stores a manual correction of a page's text (`text_source = 'manual'`, design §3.7 "직접 수정"):
/// it survives re-renders and the OCR stage (only a forced re-OCR replaces it) and bumps the
/// document so the knowledge indexer re-chunks it. An empty text clears the page's text.
/// Returns false when the page does not exist.
pub fn set_page_text(conn: &Connection, id: &str, page_no: u32, text: &str) -> rusqlite::Result<bool> {
    let now = ts(&Utc::now());
    let value = (!text.trim().is_empty()).then_some(text.trim_end());
    let n = conn.execute(
        "UPDATE pages SET text = ?3, has_text = ?4, text_source = 'manual', layout = NULL WHERE document_id = ?1 AND page_no = ?2",
        params![id, page_no as i64, value, value.is_some() as i64],
    )?;
    if n == 0 {
        return Ok(false);
    }
    conn.execute(
        "UPDATE documents SET updated_at = ?2, \
         has_text_layer = CASE WHEN ?3 THEN 1 ELSE (SELECT COUNT(*) > 0 FROM pages WHERE document_id = ?1 AND has_text = 1) END \
         WHERE id = ?1",
        params![id, now, value.is_some()],
    )?;
    Ok(true)
}

/// The stored word layout (JSON) of a page, if one was computed or edited.
pub fn page_layout_json(conn: &Connection, id: &str, page_no: u32) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT layout FROM pages WHERE document_id = ?1 AND page_no = ?2",
        params![id, page_no as i64],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    .map(Option::flatten)
}

/// Caches a computed word layout for a page (the text and its source are untouched).
pub fn set_page_layout(conn: &Connection, id: &str, page_no: u32, layout_json: Option<&str>) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE pages SET layout = ?3 WHERE document_id = ?1 AND page_no = ?2",
        params![id, page_no as i64, layout_json],
    )?;
    Ok(n > 0)
}

/// Stores an edit made on the page image: the rewritten page text (`text_source = 'manual'`) together
/// with the word layout that carries the edited boxes. Returns false when the page does not exist.
pub fn set_page_text_and_layout(
    conn: &Connection,
    id: &str,
    page_no: u32,
    text: &str,
    layout_json: &str,
) -> rusqlite::Result<bool> {
    let now = ts(&Utc::now());
    let value = (!text.trim().is_empty()).then_some(text.trim_end());
    let n = conn.execute(
        "UPDATE pages SET text = ?3, has_text = ?4, text_source = 'manual', layout = ?5 WHERE document_id = ?1 AND page_no = ?2",
        params![id, page_no as i64, value, value.is_some() as i64, layout_json],
    )?;
    if n == 0 {
        return Ok(false);
    }
    conn.execute(
        "UPDATE documents SET updated_at = ?2, \
         has_text_layer = CASE WHEN ?3 THEN 1 ELSE (SELECT COUNT(*) > 0 FROM pages WHERE document_id = ?1 AND has_text = 1) END \
         WHERE id = ?1",
        params![id, now, value.is_some()],
    )?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Page text edit history (0.3.0 item 7): undo / redo of on-image and manual corrections
// ---------------------------------------------------------------------------

/// Text, word layout (JSON) and text source of a page as stored — the unit the edit history records.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageTextState {
    pub text: Option<String>,
    pub layout: Option<String>,
    pub source: Option<String>,
}

/// One row of `page_text_history`: a correction of a page (`kind = "edit"`, `source` = layer | add |
/// manual) or the bookkeeping of an undo / redo (`kind = "revert" | "redo"`, `target_id` = the edit it
/// undid or re-applied). `previous` / `new` are the page state before and after the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageTextHistoryRow {
    pub id: i64,
    pub document_id: String,
    pub page_no: u32,
    pub kind: String,
    pub source: String,
    pub target_id: Option<i64>,
    pub previous: PageTextState,
    pub new: PageTextState,
    pub edited_at: DateTime<Utc>,
}

pub fn page_text_state(conn: &Connection, id: &str, page_no: u32) -> rusqlite::Result<Option<PageTextState>> {
    conn.query_row(
        "SELECT text, layout, text_source FROM pages WHERE document_id = ?1 AND page_no = ?2",
        params![id, page_no as i64],
        |r| Ok(PageTextState { text: r.get(0)?, layout: r.get(1)?, source: r.get(2)? }),
    )
    .optional()
}

/// Puts a page back into a recorded state (text, layout and source exactly as they were) and bumps
/// the document so the knowledge indexer re-chunks it. Returns false when the page does not exist.
pub fn restore_page_text_state(
    conn: &Connection,
    id: &str,
    page_no: u32,
    state: &PageTextState,
) -> rusqlite::Result<bool> {
    let now = ts(&Utc::now());
    let n = conn.execute(
        "UPDATE pages SET text = ?3, has_text = ?4, text_source = ?5, layout = ?6 WHERE document_id = ?1 AND page_no = ?2",
        params![id, page_no as i64, state.text, state.text.is_some() as i64, state.source, state.layout],
    )?;
    if n == 0 {
        return Ok(false);
    }
    conn.execute(
        "UPDATE documents SET updated_at = ?2, \
         has_text_layer = (SELECT COUNT(*) > 0 FROM pages WHERE document_id = ?1 AND has_text = 1) WHERE id = ?1",
        params![id, now],
    )?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub fn insert_page_text_history(
    conn: &Connection,
    id: &str,
    page_no: u32,
    kind: &str,
    source: &str,
    target_id: Option<i64>,
    previous: &PageTextState,
    new: &PageTextState,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO page_text_history (document_id, page_no, kind, source, target_id, previous_text, previous_layout, \
         previous_source, new_text, new_layout, new_source, edited_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            id,
            page_no as i64,
            kind,
            source,
            target_id,
            previous.text,
            previous.layout,
            previous.source,
            new.text,
            new.layout,
            new.source,
            ts(&Utc::now()),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// History rows of one page (or of the whole document) in insertion order.
pub fn page_text_history(
    conn: &Connection,
    id: &str,
    page_no: Option<u32>,
) -> rusqlite::Result<Vec<PageTextHistoryRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, document_id, page_no, kind, source, target_id, previous_text, previous_layout, previous_source, \
         new_text, new_layout, new_source, edited_at FROM page_text_history \
         WHERE document_id = ?1 AND (?2 IS NULL OR page_no = ?2) ORDER BY id",
    )?;
    let rows = stmt.query_map(params![id, page_no.map(|p| p as i64)], |r| {
        Ok(PageTextHistoryRow {
            id: r.get(0)?,
            document_id: r.get(1)?,
            page_no: r.get::<_, i64>(2)?.max(0) as u32,
            kind: r.get(3)?,
            source: r.get(4)?,
            target_id: r.get(5)?,
            previous: PageTextState { text: r.get(6)?, layout: r.get(7)?, source: r.get(8)? },
            new: PageTextState { text: r.get(9)?, layout: r.get(10)?, source: r.get(11)? },
            edited_at: parse_ts(r.get(12)?).unwrap_or(DateTime::<Utc>::MIN_UTC),
        })
    })?;
    rows.collect()
}

pub fn delete_page_text_history(conn: &Connection, ids: &[i64]) -> rusqlite::Result<usize> {
    let mut n = 0;
    for id in ids {
        n += conn.execute("DELETE FROM page_text_history WHERE id = ?1", [id])?;
    }
    Ok(n)
}

/// Daily maintenance: history rows older than the cutoff (the undo history follows the journal retention).
pub fn purge_page_text_history_before(conn: &Connection, cutoff: &DateTime<Utc>) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM page_text_history WHERE edited_at < ?1", [ts(cutoff)])
}

pub fn set_page_rotation(conn: &Connection, id: &str, page_no: u32, rotation: u16) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE pages SET rotation = ?3, rendered_at = ?4 WHERE document_id = ?1 AND page_no = ?2",
        params![id, page_no as i64, (rotation % 360) as i64, ts(&Utc::now())],
    )?;
    conn.execute("UPDATE documents SET updated_at = ?2 WHERE id = ?1", params![id, ts(&Utc::now())])?;
    Ok(())
}

pub fn update_page_render(
    conn: &Connection,
    id: &str,
    page_no: u32,
    width_px: u32,
    height_px: u32,
    dpi: Option<u16>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE pages SET width_px = ?3, height_px = ?4, dpi = ?5, rendered_at = ?6 WHERE document_id = ?1 AND page_no = ?2",
        params![id, page_no as i64, width_px as i64, height_px as i64, dpi.map(|d| d as i64), ts(&Utc::now())],
    )?;
    Ok(())
}

pub fn set_rel_path(conn: &Connection, id: &str, rel_path: &str, original_name: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE documents SET rel_path = ?2, original_name = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, rel_path, original_name, ts(&Utc::now())],
    )?;
    Ok(())
}

/// Records a file's hash / size / mtime. A changed hash means different content: the
/// OCR text, page classes and extraction bookkeeping of the old content are dropped
/// so the next pipeline run starts over (a manual document type is kept).
pub fn set_file_info(
    conn: &Connection,
    id: &str,
    content_hash: &str,
    size_bytes: u64,
    file_modified_at: Option<DateTime<Utc>>,
) -> rusqlite::Result<()> {
    let old: Option<String> =
        conn.query_row("SELECT content_hash FROM documents WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    conn.execute(
        "UPDATE documents SET content_hash = ?2, size_bytes = ?3, file_modified_at = ?4, updated_at = ?5 WHERE id = ?1",
        params![id, content_hash, size_bytes as i64, file_modified_at.as_ref().map(ts), ts(&Utc::now())],
    )?;
    if old.as_deref() != Some(content_hash) {
        reset_pipeline_state(conn, id)?;
    }
    Ok(())
}

/// Forgets everything the pipeline derived from a document's previous content.
pub fn reset_pipeline_state(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE pages SET text = CASE WHEN text_source = 'ocr' THEN NULL ELSE text END, \
         has_text = CASE WHEN text_source = 'ocr' THEN 0 ELSE has_text END, \
         layout = CASE WHEN text_source = 'ocr' THEN NULL ELSE layout END, \
         text_source = CASE WHEN text_source = 'ocr' THEN NULL ELSE text_source END, ocr_at = NULL, ocr_profile = NULL, \
         ocr_model = NULL, classified_at = NULL, page_doc_type = NULL, page_language = NULL, page_rotation = NULL \
         WHERE document_id = ?1",
        [id],
    )?;
    conn.execute(
        "UPDATE documents SET has_text_layer = EXISTS(SELECT 1 FROM pages WHERE document_id = ?1 AND has_text = 1) WHERE id = ?1",
        [id],
    )?;
    conn.execute(
        "UPDATE documents SET doc_type = CASE WHEN doc_type_source = 'manual' THEN doc_type ELSE NULL END, \
         doc_type_source = CASE WHEN doc_type_source = 'manual' THEN 'manual' ELSE NULL END, classified_at = NULL, \
         ocr_at = NULL, extracted_at = NULL, last_run_id = NULL, review_status = 'none', confidence = NULL, \
         pipeline_error = NULL, updated_at = ?2 WHERE id = ?1",
        params![id, ts(&Utc::now())],
    )?;
    Ok(())
}

/// Sets the document type. `source` records who decided (`manual` wins over the
/// classifier on later runs; `None` keeps the stored source).
pub fn set_doc_type(conn: &Connection, id: &str, doc_type: Option<&str>, source: Option<&str>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE documents SET doc_type = ?2, doc_type_source = COALESCE(?3, doc_type_source), updated_at = ?4 WHERE id = ?1",
        params![id, doc_type, source, ts(&Utc::now())],
    )?;
    Ok(())
}

pub fn set_title(conn: &Connection, id: &str, title: Option<&str>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE documents SET title = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, title, ts(&Utc::now())],
    )?;
    Ok(())
}

pub fn set_role(conn: &Connection, id: &str, role: DocumentRole) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE documents SET role = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, role.as_str(), ts(&Utc::now())],
    )?;
    Ok(())
}

pub fn delete_document(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    // Chunks and FTS rows cascade (triggers); vectors live in a virtual table
    // without foreign keys and are removed explicitly.
    crate::knowledge::repo::clear_chunks(conn, id)?;
    let n = conn.execute("DELETE FROM documents WHERE id = ?1", [id])?;
    Ok(n > 0)
}

/// Replaces the tag set of a document (creating unknown tags).
pub fn set_tags(conn: &Connection, id: &str, tags: &[String]) -> rusqlite::Result<Vec<String>> {
    conn.execute("DELETE FROM document_tags WHERE document_id = ?1", [id])?;
    let now = ts(&Utc::now());
    let mut applied = Vec::new();
    for raw in tags {
        let name = raw.trim();
        if name.is_empty() || name.chars().count() > 64 {
            continue;
        }
        conn.execute("INSERT OR IGNORE INTO tags (name, created_at) VALUES (?1, ?2)", params![name, now])?;
        let tag_id: i64 = conn.query_row("SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE", [name], |r| r.get(0))?;
        let stored: String = conn.query_row("SELECT name FROM tags WHERE id = ?1", [tag_id], |r| r.get(0))?;
        conn.execute("INSERT OR IGNORE INTO document_tags (document_id, tag_id) VALUES (?1, ?2)", params![id, tag_id])?;
        if !applied.contains(&stored) {
            applied.push(stored);
        }
    }
    conn.execute("UPDATE documents SET updated_at = ?2 WHERE id = ?1", params![id, now])?;
    Ok(applied)
}

pub fn list_tags(conn: &Connection) -> rusqlite::Result<Vec<(String, u64)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.name, COUNT(dt.document_id) FROM tags t LEFT JOIN document_tags dt ON dt.tag_id = t.id \
         GROUP BY t.id ORDER BY t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?.max(0) as u64)))?;
    rows.collect()
}

pub fn insert_batch(conn: &Connection, id: &str, source: &str, total: u32) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO import_batches (id, source, started_at, total) VALUES (?1, ?2, ?3, ?4)",
        params![id, source, ts(&Utc::now()), total as i64],
    )?;
    Ok(())
}

pub fn finish_batch(
    conn: &Connection,
    id: &str,
    imported: u32,
    duplicates: u32,
    unsupported: u32,
    failed: u32,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE import_batches SET finished_at = ?2, imported = ?3, duplicates = ?4, unsupported = ?5, failed = ?6 WHERE id = ?1",
        params![id, ts(&Utc::now()), imported as i64, duplicates as i64, unsupported as i64, failed as i64],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn new_doc(id: &str, rel: &str, hash: &str) -> NewDocument {
        NewDocument {
            id: id.into(),
            rel_path: rel.into(),
            original_name: rel.rsplit('/').next().unwrap().into(),
            source_kind: SourceKind::from_path(std::path::Path::new(rel)).unwrap(),
            content_hash: hash.into(),
            size_bytes: 10,
            file_modified_at: None,
            role: DocumentRole::Work,
            import_batch_id: None,
        }
    }

    #[test]
    fn insert_list_filter_pages_tags_delete() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            insert_document(c, &new_doc("a", "inbox/a.pdf", "h1"))?;
            insert_document(c, &new_doc("b", "documents/2026/b.jpg", "h2"))?;
            insert_document(c, &new_doc("c", "c.hwpx", "h1"))?;
            set_status(c, "a", DocumentStatus::Rendered, None)?;
            set_status(c, "c", DocumentStatus::Error, Some("boom"))?;
            replace_pages(
                c,
                "a",
                &[
                    NewPage {
                        page_no: 1,
                        width_px: 100,
                        height_px: 200,
                        text: Some("hello".into()),
                        ..Default::default()
                    },
                    NewPage {
                        page_no: 2,
                        width_px: 100,
                        height_px: 200,
                        text: Some("   ".into()),
                        ..Default::default()
                    },
                ],
                Some(200),
                false,
                None,
            )?;
            set_tags(c, "a", &["출장".into(), " 2026 ".into(), "".into(), "출장".into()])?;
            Ok(())
        })
        .unwrap();
        let a = db.with(|c| get_document(c, "a")).unwrap().unwrap();
        assert_eq!(a.status, DocumentStatus::Rendered);
        assert_eq!(a.page_count, 2);
        assert!(a.has_text_layer);
        assert_eq!(a.render_dpi, Some(200));
        assert_eq!(a.tags, vec!["2026".to_string(), "출장".to_string()]);
        assert_eq!(a.rel_dir(), "inbox");
        let pages = db.with(|c| list_pages(c, "a", true)).unwrap();
        assert_eq!(pages.len(), 2);
        assert!(pages[0].has_text && !pages[1].has_text);
        assert_eq!(pages[0].text.as_deref(), Some("hello"));
        assert!(db.with(|c| list_pages(c, "a", false)).unwrap()[0].text.is_none());

        let dup = db.with(|c| find_by_hash(c, "h1")).unwrap();
        assert_eq!(dup.len(), 2);
        assert!(db.with(|c| find_by_rel_path(c, "c.hwpx")).unwrap().is_some());

        let all = db.with(|c| list_documents(c, &DocumentFilter::default())).unwrap();
        assert_eq!(all.len(), 3);
        let errors = db
            .with(|c| {
                list_documents(c, &DocumentFilter { statuses: vec![DocumentStatus::Error], ..Default::default() })
            })
            .unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].error.as_deref(), Some("boom"));
        let inbox = db
            .with(|c| list_documents(c, &DocumentFilter { folder: Some("inbox".into()), ..Default::default() }))
            .unwrap();
        assert_eq!(inbox.len(), 1);
        let root = db
            .with(|c| list_documents(c, &DocumentFilter { folder: Some(String::new()), ..Default::default() }))
            .unwrap();
        assert_eq!(root.len(), 1);
        let q = db
            .with(|c| list_documents(c, &DocumentFilter { query: Some("B.JPG".into()), ..Default::default() }))
            .unwrap();
        assert_eq!(q.len(), 1);
        let tagged =
            db.with(|c| list_documents(c, &DocumentFilter { tag: Some("출장".into()), ..Default::default() })).unwrap();
        assert_eq!(tagged.len(), 1);
        let like =
            db.with(|c| list_documents(c, &DocumentFilter { query: Some("%".into()), ..Default::default() })).unwrap();
        assert!(like.is_empty(), "LIKE wildcards must be escaped");

        let s = db.with(stats).unwrap();
        assert_eq!(s.total, 3);
        assert_eq!(s.by_status.get("rendered"), Some(&1));
        assert_eq!(s.total_pages, 2);
        assert_eq!(s.folders.len(), 3);

        db.with(|c| set_page_rotation(c, "a", 1, 90)).unwrap();
        assert_eq!(db.with(|c| get_page(c, "a", 1)).unwrap().unwrap().rotation, 90);
        assert!(db.with(|c| delete_document(c, "a")).unwrap());
        assert!(db.with(|c| list_pages(c, "a", false)).unwrap().is_empty(), "pages cascade");
        assert!(!db.with(|c| delete_document(c, "a")).unwrap());
        let tags = db.with(list_tags).unwrap();
        assert_eq!(tags.iter().find(|(n, _)| n == "출장").map(|(_, n)| *n), Some(0));
    }

    #[test]
    fn manual_page_text_survives_rerender_and_clears() {
        let db = Db::open_in_memory().unwrap();
        let page = |no: u32, text: Option<&str>| NewPage {
            page_no: no,
            width_px: 100,
            height_px: 200,
            text: text.map(str::to_string),
            ..Default::default()
        };
        db.with(|c| {
            insert_document(c, &new_doc("a", "documents/a.pdf", "h1"))?;
            replace_pages(c, "a", &[page(1, None), page(2, Some("layer text"))], Some(200), false, None)
        })
        .unwrap();
        assert!(!db.with(|c| get_document(c, "a")).unwrap().unwrap().has_text_layer || true);
        // Correct page 1 (image-only) by hand.
        assert!(db.with(|c| set_page_text(c, "a", 1, "수동 입력 텍스트  \n")).unwrap());
        assert!(!db.with(|c| set_page_text(c, "a", 9, "x")).unwrap(), "unknown page");
        let pages = db.with(|c| list_pages(c, "a", true)).unwrap();
        assert_eq!(pages[0].text.as_deref(), Some("수동 입력 텍스트"));
        assert_eq!(pages[0].text_source.as_deref(), Some("manual"));
        assert!(pages[0].has_text);
        assert!(db.with(|c| get_document(c, "a")).unwrap().unwrap().has_text_layer);
        // A re-render keeps the manual text (even for a page that now has a layer) and the layer elsewhere.
        db.with(|c| {
            replace_pages(c, "a", &[page(1, Some("fresh layer")), page(2, Some("layer text"))], Some(300), false, None)
        })
        .unwrap();
        let pages = db.with(|c| list_pages(c, "a", true)).unwrap();
        assert_eq!(pages[0].text.as_deref(), Some("수동 입력 텍스트"));
        assert_eq!(pages[0].text_source.as_deref(), Some("manual"));
        assert_eq!(pages[1].text_source.as_deref(), Some("layer"));
        // Clearing the text leaves the page without text; the document flag follows the other pages.
        assert!(db.with(|c| set_page_text(c, "a", 1, "   ")).unwrap());
        let pages = db.with(|c| list_pages(c, "a", true)).unwrap();
        assert!(pages[0].text.is_none() && !pages[0].has_text);
        assert!(db.with(|c| get_document(c, "a")).unwrap().unwrap().has_text_layer, "page 2 still has text");
    }

    #[test]
    fn rel_path_is_unique() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| insert_document(c, &new_doc("a", "x.pdf", "h"))).unwrap();
        let err = db.with(|c| insert_document(c, &new_doc("b", "x.pdf", "h2"))).err().unwrap();
        assert_eq!(err.code(), "db");
    }
}
