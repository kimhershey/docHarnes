//! SQL for the knowledge tables (migration 0002). Functions take a
//! `rusqlite::Connection` (or transaction) so callers decide the transaction
//! boundaries; `Db::with` / `Db::with_tx` wrap them.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};

use super::{
    Answer, AnswerKind, AnswerSentence, Chunk, ChunkDraft, ChunkKind, Citation, Collection, CollectionInput,
    CollectionKind, GlossaryEntry, GlossaryInput, IndexStatus, SavedAnswer, SearchScope, VersionGroup, VersionInfo,
    VersionInput, INDEX_VERSION,
};
use crate::documents::{new_id, DocumentRecord, DocumentRole};

fn ts(d: &DateTime<Utc>) -> String {
    d.to_rfc3339()
}

fn parse_ts(s: Option<String>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc)))
}

fn now() -> String {
    ts(&Utc::now())
}

fn placeholders(n: usize) -> String {
    std::iter::repeat_n("?", n).collect::<Vec<_>>().join(",")
}

/// Explicitly numbered placeholders `?from, ?from+1, …` (needed when they follow
/// numbered ones in the statement text; bare `?` would restart the numbering).
fn numbered_placeholders(from: usize, n: usize) -> String {
    (from..from + n).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",")
}

/// Above this many documents a scope is applied after the candidate fetch
/// instead of inside the SQL (`IN (...)` list size).
pub const SCOPE_SQL_MAX_DOCS: usize = 900;

pub fn kv_get(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0)).optional()
}

pub fn kv_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO kv (key, value, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![key, value, now()],
    )?;
    Ok(())
}

/// Typed failure carried through `rusqlite::Error` (the command layer maps the code).
fn typed(code: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(code.to_string())
}

pub fn collection_exists(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT COUNT(*) FROM collections WHERE id = ?1", [id], |r| r.get::<_, i64>(0)).map(|n| n > 0)
}

pub fn document_exists(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT COUNT(*) FROM documents WHERE id = ?1 AND deleted_at IS NULL", [id], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)
}

// ---------------------------------------------------------------------------
// Collections
// ---------------------------------------------------------------------------

const COLLECTION_COLS: &str = "c.id, c.name, c.description, c.color, c.active, c.kind, c.project_id, c.created_at, c.updated_at, \
    (SELECT COUNT(*) FROM collection_docs cd WHERE cd.collection_id = c.id), \
    (SELECT COALESCE(SUM(d.chunk_count), 0) FROM collection_docs cd JOIN documents d ON d.id = cd.document_id WHERE cd.collection_id = c.id)";

fn collection_from_row(r: &Row<'_>) -> rusqlite::Result<Collection> {
    Ok(Collection {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        color: r.get(3)?,
        active: r.get::<_, i64>(4)? != 0,
        kind: CollectionKind::parse(&r.get::<_, String>(5)?),
        project_id: r.get(6)?,
        created_at: parse_ts(r.get(7)?).unwrap_or_else(Utc::now),
        updated_at: parse_ts(r.get(8)?).unwrap_or_else(Utc::now),
        doc_count: r.get::<_, i64>(9)?.max(0) as u64,
        chunk_count: r.get::<_, i64>(10)?.max(0) as u64,
    })
}

pub fn list_collections(conn: &Connection) -> rusqlite::Result<Vec<Collection>> {
    let mut st =
        conn.prepare(&format!("SELECT {COLLECTION_COLS} FROM collections c ORDER BY c.name COLLATE NOCASE"))?;
    let rows = st.query_map([], collection_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_collection(conn: &Connection, id: &str) -> rusqlite::Result<Option<Collection>> {
    conn.query_row(&format!("SELECT {COLLECTION_COLS} FROM collections c WHERE c.id = ?1"), [id], collection_from_row)
        .optional()
}

/// Inserts (no id) or updates a collection. Names are unique (case-insensitively) among the
/// groups of the 공통 참고 자료; a project's group carries the project's name and may share it
/// with a common group. A new group with `project_id` becomes that project's reference group
/// (kind `project`; at most one per project — `project_group_exists` otherwise); on update the
/// kind and project are kept as stored.
pub fn save_collection(conn: &Connection, input: &CollectionInput) -> rusqlite::Result<Collection> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(typed("collection_name_empty"));
    }
    let stored = match &input.id {
        Some(id) => Some(get_collection(conn, id)?.ok_or_else(|| typed("collection_not_found"))?),
        None => None,
    };
    let new_project = input.project_id.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let common = match &stored {
        Some(c) => c.is_common(),
        None => new_project.is_none(),
    };
    if common {
        let taken: i64 = conn.query_row(
            "SELECT COUNT(*) FROM collections WHERE project_id IS NULL AND name = ?1 COLLATE NOCASE AND id <> COALESCE(?2, '')",
            params![name, input.id],
            |r| r.get(0),
        )?;
        if taken > 0 {
            return Err(typed("collection_name_taken"));
        }
    }
    let now = now();
    let id = match &input.id {
        Some(id) => {
            conn.execute(
                "UPDATE collections SET name = ?2, description = ?3, color = ?4, active = ?5, updated_at = ?6 WHERE id = ?1",
                params![id, name, input.description, input.color, input.active as i64, now],
            )?;
            id.clone()
        }
        None => {
            let id = new_id();
            let project_id = new_project;
            let kind = if project_id.is_some() { CollectionKind::Project } else { CollectionKind::Reference };
            if let Some(p) = project_id {
                let exists: i64 = conn.query_row("SELECT COUNT(*) FROM projects WHERE id = ?1", [p], |r| r.get(0))?;
                if exists == 0 {
                    return Err(typed("project_not_found"));
                }
                if reference_group_of_project(conn, p)?.is_some() {
                    return Err(typed("project_group_exists"));
                }
            }
            conn.execute(
                "INSERT INTO collections (id, name, description, color, active, kind, project_id, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![id, name, input.description, input.color, input.active as i64, kind.as_str(), project_id, now],
            )?;
            id
        }
    };
    get_collection(conn, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn delete_collection(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM collections WHERE id = ?1", [id])? > 0)
}

/// The reference group of a project, if it has one.
pub fn reference_group_of_project(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<Collection>> {
    conn.query_row(
        &format!("SELECT {COLLECTION_COLS} FROM collections c WHERE c.project_id = ?1 ORDER BY c.created_at LIMIT 1"),
        [project_id],
        collection_from_row,
    )
    .optional()
}

/// The name a project's reference group carries: the project's name ("참고 자료" when blank).
fn group_name(project_name: &str) -> &str {
    let name = project_name.trim();
    if name.is_empty() {
        "참고 자료"
    } else {
        name
    }
}

/// The reference group of a project (0.5.0 plan A), created on first use and named after the
/// project (`project_name`; a later rename follows through `sync_reference_group_name`).
pub fn project_reference_group(
    conn: &Connection,
    project_id: &str,
    project_name: &str,
) -> rusqlite::Result<Collection> {
    if let Some(group) = reference_group_of_project(conn, project_id)? {
        return Ok(group);
    }
    let exists: i64 = conn.query_row("SELECT COUNT(*) FROM projects WHERE id = ?1", [project_id], |r| r.get(0))?;
    if exists == 0 {
        return Err(typed("project_not_found"));
    }
    let name = group_name(project_name);
    let id = new_id();
    let now = now();
    conn.execute(
        "INSERT INTO collections (id, name, description, color, active, kind, project_id, created_at, updated_at) \
         VALUES (?1, ?2, NULL, NULL, 1, 'project', ?3, ?4, ?4)",
        params![id, name, project_id, now],
    )?;
    get_collection(conn, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// Renames a project's reference group after the project (no-op without a group).
pub fn sync_reference_group_name(conn: &Connection, project_id: &str, project_name: &str) -> rusqlite::Result<()> {
    let Some(group) = reference_group_of_project(conn, project_id)? else {
        return Ok(());
    };
    let name = group_name(project_name);
    if name != group.name {
        conn.execute(
            "UPDATE collections SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![group.id, name, now()],
        )?;
    }
    Ok(())
}

/// Removes the reference group(s) of a project (the documents stay in the library).
pub fn delete_reference_groups_of_project(conn: &Connection, project_id: &str) -> rusqlite::Result<u32> {
    Ok(conn.execute("DELETE FROM collections WHERE project_id = ?1", [project_id])? as u32)
}

/// Documents of the active groups of the 공통 참고 자료 (collections without a project).
pub fn common_reference_doc_ids(conn: &Connection) -> rusqlite::Result<HashSet<String>> {
    let mut st = conn.prepare(
        "SELECT DISTINCT cd.document_id FROM collection_docs cd JOIN collections c ON c.id = cd.collection_id \
         WHERE c.project_id IS NULL AND c.active = 1",
    )?;
    let out = st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(out)
}

/// Work documents of a project (`project_documents`, existing documents only).
pub fn project_work_doc_ids(conn: &Connection, project_id: &str) -> rusqlite::Result<HashSet<String>> {
    let mut st = conn.prepare(
        "SELECT pd.document_id FROM project_documents pd JOIN documents d ON d.id = pd.document_id \
         WHERE pd.project_id = ?1 AND d.deleted_at IS NULL",
    )?;
    let out = st.query_map([project_id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(out)
}

/// Documents of a project's reference group (empty without a group).
pub fn project_reference_doc_ids(conn: &Connection, project_id: &str) -> rusqlite::Result<HashSet<String>> {
    let mut st = conn.prepare(
        "SELECT cd.document_id FROM collection_docs cd JOIN collections c ON c.id = cd.collection_id WHERE c.project_id = ?1",
    )?;
    let out = st.query_map([project_id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(out)
}

pub fn collection_doc_ids(conn: &Connection, id: &str) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare("SELECT document_id FROM collection_docs WHERE collection_id = ?1 ORDER BY added_at")?;
    let rows = st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn add_docs_to_collection(conn: &Connection, id: &str, doc_ids: &[String]) -> rusqlite::Result<u32> {
    if !collection_exists(conn, id)? {
        return Err(typed("collection_not_found"));
    }
    let now = now();
    let mut st = conn
        .prepare("INSERT OR IGNORE INTO collection_docs (collection_id, document_id, added_at) VALUES (?1, ?2, ?3)")?;
    let mut n = 0;
    for d in doc_ids {
        n += st.execute(params![id, d, now])? as u32;
    }
    conn.execute("UPDATE collections SET updated_at = ?2 WHERE id = ?1", params![id, now])?;
    Ok(n)
}

pub fn remove_docs_from_collection(conn: &Connection, id: &str, doc_ids: &[String]) -> rusqlite::Result<u32> {
    let mut st = conn.prepare("DELETE FROM collection_docs WHERE collection_id = ?1 AND document_id = ?2")?;
    let mut n = 0;
    for d in doc_ids {
        n += st.execute(params![id, d])? as u32;
    }
    Ok(n)
}

/// Documents of a collection in insertion order (deleted documents skipped).
pub fn collection_documents(conn: &Connection, id: &str) -> rusqlite::Result<Vec<DocumentRecord>> {
    let ids = collection_doc_ids(conn, id)?;
    let mut out = Vec::with_capacity(ids.len());
    for d in &ids {
        if let Some(doc) = crate::documents::repo::get_document(conn, d)? {
            if doc.deleted_at.is_none() {
                out.push(doc);
            }
        }
    }
    Ok(out)
}

/// `(collection id, name)` pairs of a document.
pub fn collections_of_document(conn: &Connection, doc_id: &str) -> rusqlite::Result<Vec<(String, String)>> {
    let mut st = conn.prepare(
        "SELECT c.id, c.name FROM collection_docs cd JOIN collections c ON c.id = cd.collection_id WHERE cd.document_id = ?1 ORDER BY c.name",
    )?;
    let rows = st.query_map([doc_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------

fn version_from_row(r: &Row<'_>) -> rusqlite::Result<VersionInfo> {
    Ok(VersionInfo {
        document_id: r.get(0)?,
        title: r.get::<_, Option<String>>(1)?.unwrap_or_else(|| r.get::<_, String>(2).unwrap_or_default()),
        group_key: r.get(3)?,
        version_label: r.get(4)?,
        effective_date: r.get(5)?,
        is_latest: r.get::<_, i64>(6)? != 0,
        note: r.get(7)?,
    })
}

const VERSION_SQL: &str = "SELECT v.document_id, d.title, d.original_name, v.group_key, v.version_label, v.effective_date, v.is_latest, v.note \
    FROM doc_versions v JOIN documents d ON d.id = v.document_id";

pub fn list_version_groups(conn: &Connection) -> rusqlite::Result<Vec<VersionGroup>> {
    let mut st = conn.prepare(&format!(
        "{VERSION_SQL} ORDER BY v.group_key COLLATE NOCASE, v.is_latest DESC, v.effective_date DESC, d.imported_at DESC"
    ))?;
    let rows: Vec<VersionInfo> = st.query_map([], version_from_row)?.collect::<rusqlite::Result<_>>()?;
    let mut groups: Vec<VersionGroup> = Vec::new();
    for v in rows {
        match groups.last_mut() {
            Some(g) if g.group_key == v.group_key => g.documents.push(v),
            _ => groups.push(VersionGroup { group_key: v.group_key.clone(), documents: vec![v] }),
        }
    }
    Ok(groups)
}

pub fn get_version(conn: &Connection, doc_id: &str) -> rusqlite::Result<Option<VersionInfo>> {
    conn.query_row(&format!("{VERSION_SQL} WHERE v.document_id = ?1"), [doc_id], version_from_row).optional()
}

/// Upserts a version row; marking a document latest demotes the rest of its group.
pub fn set_version(conn: &Connection, input: &VersionInput) -> rusqlite::Result<VersionInfo> {
    let group = input.group_key.trim();
    if group.is_empty() {
        return Err(typed("version_group_empty"));
    }
    if !document_exists(conn, &input.document_id)? {
        return Err(typed("document_not_found"));
    }
    let now = now();
    conn.execute(
        "INSERT INTO doc_versions (document_id, group_key, version_label, effective_date, is_latest, note, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(document_id) DO UPDATE SET group_key = excluded.group_key, version_label = excluded.version_label, effective_date = excluded.effective_date, is_latest = excluded.is_latest, note = excluded.note, updated_at = excluded.updated_at",
        params![input.document_id, group, input.version_label, input.effective_date, input.is_latest as i64, input.note, now],
    )?;
    if input.is_latest {
        conn.execute(
            "UPDATE doc_versions SET is_latest = 0, updated_at = ?3 WHERE group_key = ?1 AND document_id <> ?2",
            params![group, input.document_id, now],
        )?;
    }
    get_version(conn, &input.document_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// Removes a document's version row ("버전 해제"). When it was its group's latest edition, the newest
/// remaining edition takes over, so the older ones stay superseded behind a current edition instead of
/// being hidden for good.
pub fn clear_version(conn: &Connection, doc_id: &str) -> rusqlite::Result<bool> {
    let row: Option<(String, bool)> = conn
        .query_row("SELECT group_key, is_latest FROM doc_versions WHERE document_id = ?1", [doc_id], |r| {
            Ok((r.get(0)?, r.get::<_, i64>(1)? != 0))
        })
        .optional()?;
    let Some((group, was_latest)) = row else { return Ok(false) };
    conn.execute("DELETE FROM doc_versions WHERE document_id = ?1", [doc_id])?;
    if was_latest {
        promote_newest_edition(conn, &group)?;
    }
    Ok(true)
}

/// Marks the newest edition of a group (effective date, then import time) as its latest when the group
/// has none. Returns the promoted document.
fn promote_newest_edition(conn: &Connection, group_key: &str) -> rusqlite::Result<Option<String>> {
    const LIVE: &str = "FROM doc_versions v JOIN documents d ON d.id = v.document_id \
                        WHERE v.group_key = ?1 AND d.deleted_at IS NULL";
    let latest: i64 =
        conn.query_row(&format!("SELECT COUNT(*) {LIVE} AND v.is_latest = 1"), [group_key], |r| r.get(0))?;
    if latest > 0 {
        return Ok(None);
    }
    let newest: Option<String> = conn
        .query_row(
            &format!("SELECT v.document_id {LIVE} ORDER BY v.effective_date DESC, d.imported_at DESC LIMIT 1"),
            [group_key],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = &newest {
        conn.execute("UPDATE doc_versions SET is_latest = 1, updated_at = ?2 WHERE document_id = ?1", params![id, now()])?;
    }
    Ok(newest)
}

// ---------------------------------------------------------------------------
// Glossary
// ---------------------------------------------------------------------------

fn glossary_from_row(r: &Row<'_>) -> rusqlite::Result<GlossaryEntry> {
    let syn: String = r.get(3)?;
    Ok(GlossaryEntry {
        id: r.get(0)?,
        term: r.get(1)?,
        definition: r.get(2)?,
        synonyms: serde_json::from_str(&syn).unwrap_or_default(),
        created_at: parse_ts(r.get(4)?).unwrap_or_else(Utc::now),
        updated_at: parse_ts(r.get(5)?).unwrap_or_else(Utc::now),
    })
}

const GLOSSARY_COLS: &str = "id, term, definition, synonyms, created_at, updated_at";

pub fn list_glossary(conn: &Connection) -> rusqlite::Result<Vec<GlossaryEntry>> {
    let mut st = conn.prepare(&format!("SELECT {GLOSSARY_COLS} FROM glossary ORDER BY term COLLATE NOCASE"))?;
    let rows = st.query_map([], glossary_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn save_glossary(conn: &Connection, input: &GlossaryInput) -> rusqlite::Result<GlossaryEntry> {
    let term = input.term.trim();
    if term.is_empty() {
        return Err(typed("glossary_term_empty"));
    }
    if let Some(id) = input.id {
        let clash: i64 = conn.query_row(
            "SELECT COUNT(*) FROM glossary WHERE term = ?1 COLLATE NOCASE AND id <> ?2",
            params![term, id],
            |r| r.get(0),
        )?;
        if clash > 0 {
            return Err(typed("glossary_term_taken"));
        }
    }
    let synonyms: Vec<String> = input
        .synonyms
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != term)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let syn = serde_json::to_string(&synonyms).unwrap_or_else(|_| "[]".into());
    let now = now();
    let id: i64 = match input.id {
        Some(id) => {
            conn.execute(
                "UPDATE glossary SET term = ?2, definition = ?3, synonyms = ?4, updated_at = ?5 WHERE id = ?1",
                params![id, term, input.definition, syn, now],
            )?;
            id
        }
        None => {
            conn.execute(
                "INSERT INTO glossary (term, definition, synonyms, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4) \
                 ON CONFLICT(term) DO UPDATE SET definition = excluded.definition, synonyms = excluded.synonyms, updated_at = excluded.updated_at",
                params![term, input.definition, syn, now],
            )?;
            conn.query_row("SELECT id FROM glossary WHERE term = ?1", [term], |r| r.get(0))?
        }
    };
    conn.query_row(&format!("SELECT {GLOSSARY_COLS} FROM glossary WHERE id = ?1"), [id], glossary_from_row)
}

pub fn delete_glossary(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM glossary WHERE id = ?1", [id])? > 0)
}

/// For each query term that is a glossary term or synonym (case-insensitive),
/// the other forms of that entry. Small table: scanned in memory.
pub fn glossary_expansions(conn: &Connection, terms: &[String]) -> rusqlite::Result<Vec<(String, Vec<String>)>> {
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let entries = list_glossary(conn)?;
    let mut out = Vec::new();
    for t in terms {
        let tl = t.to_lowercase();
        for e in &entries {
            let mut forms: Vec<String> = std::iter::once(e.term.clone()).chain(e.synonyms.iter().cloned()).collect();
            if forms.iter().any(|f| f.to_lowercase() == tl) {
                forms.retain(|f| f.to_lowercase() != tl);
                if !forms.is_empty() {
                    out.push((t.clone(), forms));
                }
                break;
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Chunks
// ---------------------------------------------------------------------------

const CHUNK_COLS: &str =
    "id, document_id, chunk_index, page_start, page_end, kind, heading_path, clause, text, token_estimate, embedded_at";

fn chunk_from_row(r: &Row<'_>) -> rusqlite::Result<Chunk> {
    Ok(Chunk {
        id: r.get(0)?,
        document_id: r.get(1)?,
        chunk_index: r.get::<_, i64>(2)? as u32,
        page_start: r.get::<_, i64>(3)? as u32,
        page_end: r.get::<_, i64>(4)? as u32,
        kind: ChunkKind::parse(&r.get::<_, String>(5)?),
        heading_path: r.get(6)?,
        clause: r.get(7)?,
        text: r.get(8)?,
        token_estimate: r.get::<_, i64>(9)? as u32,
        embedded: r.get::<_, Option<String>>(10)?.is_some(),
    })
}

pub fn vec_table_exists(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'chunks_vec'", [], |r| {
        r.get::<_, i64>(0)
    })
    .map(|n| n > 0)
}

/// Removes vectors of the given chunks (no-op without a vector table).
pub fn delete_vectors(conn: &Connection, ids: &[i64]) -> rusqlite::Result<()> {
    if ids.is_empty() || !vec_table_exists(conn)? {
        return Ok(());
    }
    for group in ids.chunks(500) {
        conn.execute(
            &format!("DELETE FROM chunks_vec WHERE rowid IN ({})", placeholders(group.len())),
            params_from_iter(group.iter()),
        )?;
    }
    Ok(())
}

fn chunk_ids_of(conn: &Connection, doc_id: &str) -> rusqlite::Result<Vec<i64>> {
    let mut st = conn.prepare("SELECT id FROM chunks WHERE document_id = ?1")?;
    let rows = st.query_map([doc_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Replaces a document's chunks (FTS rows follow via triggers, vectors are
/// dropped) and records the index state on the document. Run inside a
/// transaction.
pub fn replace_chunks(
    conn: &Connection,
    doc_id: &str,
    drafts: &[ChunkDraft],
    tokens: &[String],
    text_hash: &str,
) -> rusqlite::Result<u32> {
    debug_assert_eq!(drafts.len(), tokens.len());
    let old = chunk_ids_of(conn, doc_id)?;
    delete_vectors(conn, &old)?;
    conn.execute("DELETE FROM chunks WHERE document_id = ?1", [doc_id])?;
    let mut st = conn.prepare(
        "INSERT INTO chunks (document_id, chunk_index, page_start, page_end, kind, heading_path, clause, text, tokens, token_estimate, text_hash) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for (i, (d, tok)) in drafts.iter().zip(tokens).enumerate() {
        let hash = blake3::hash(d.text.as_bytes()).to_hex().to_string();
        st.execute(params![
            doc_id,
            i as i64,
            d.page_start as i64,
            d.page_end as i64,
            d.kind.as_str(),
            d.heading_path,
            d.clause,
            d.text,
            tok,
            d.token_estimate as i64,
            hash
        ])?;
    }
    conn.execute(
        "UPDATE documents SET indexed_at = ?2, index_version = ?3, index_error = NULL, text_hash = ?4, chunk_count = ?5 WHERE id = ?1",
        params![doc_id, now(), INDEX_VERSION as i64, text_hash, drafts.len() as i64],
    )?;
    Ok(drafts.len() as u32)
}

/// Drops a document's chunks and index state (used before deleting / when text vanished).
pub fn clear_chunks(conn: &Connection, doc_id: &str) -> rusqlite::Result<()> {
    let old = chunk_ids_of(conn, doc_id)?;
    delete_vectors(conn, &old)?;
    conn.execute("DELETE FROM chunks WHERE document_id = ?1", [doc_id])?;
    conn.execute(
        "UPDATE documents SET indexed_at = NULL, index_version = 0, text_hash = NULL, chunk_count = 0 WHERE id = ?1",
        [doc_id],
    )?;
    Ok(())
}

/// Marks an unchanged document as checked now (stops metadata-only updates from re-queuing it).
pub fn touch_indexed(conn: &Connection, doc_id: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE documents SET indexed_at = ?2, index_error = NULL WHERE id = ?1", params![doc_id, now()])?;
    Ok(())
}

pub fn set_index_error(conn: &Connection, doc_id: &str, error: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE documents SET index_error = ?2 WHERE id = ?1", params![doc_id, error])?;
    Ok(())
}

/// Index bookkeeping of one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexState {
    pub indexed_at: Option<String>,
    pub index_version: u32,
    pub text_hash: Option<String>,
}

pub fn index_state(conn: &Connection, doc_id: &str) -> rusqlite::Result<Option<IndexState>> {
    conn.query_row("SELECT indexed_at, index_version, text_hash FROM documents WHERE id = ?1", [doc_id], |r| {
        Ok(IndexState { indexed_at: r.get(0)?, index_version: r.get::<_, i64>(1)? as u32, text_hash: r.get(2)? })
    })
    .optional()
}

pub fn list_chunks(conn: &Connection, doc_id: &str) -> rusqlite::Result<Vec<Chunk>> {
    let mut st =
        conn.prepare(&format!("SELECT {CHUNK_COLS} FROM chunks WHERE document_id = ?1 ORDER BY chunk_index"))?;
    let rows = st.query_map([doc_id], chunk_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_chunks(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<Chunk>> {
    let mut out = Vec::new();
    for group in ids.chunks(500) {
        let mut st =
            conn.prepare(&format!("SELECT {CHUNK_COLS} FROM chunks WHERE id IN ({})", placeholders(group.len())))?;
        out.extend(
            st.query_map(params_from_iter(group.iter()), chunk_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Ok(out)
}

/// `(total, embedded)` chunk counts.
pub fn chunk_counts(conn: &Connection) -> rusqlite::Result<(u64, u64)> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN embedded_at IS NOT NULL THEN 1 ELSE 0 END), 0) FROM chunks",
        [],
        |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
    )
}

// ---------------------------------------------------------------------------
// Search primitives
// ---------------------------------------------------------------------------

/// A chunk joined with its document, as returned by candidate lookups.
#[derive(Debug, Clone)]
pub struct ChunkRow {
    pub chunk: Chunk,
    pub title: String,
    pub rel_path: String,
    pub role: DocumentRole,
    pub doc_type: Option<String>,
}

pub fn chunk_rows(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<ChunkRow>> {
    let mut out = Vec::new();
    for group in ids.chunks(500) {
        let mut st = conn.prepare(&format!(
            "SELECT c.id, c.document_id, c.chunk_index, c.page_start, c.page_end, c.kind, c.heading_path, c.clause, c.text, c.token_estimate, c.embedded_at, \
                    d.title, d.original_name, d.rel_path, d.role, d.doc_type \
             FROM chunks c JOIN documents d ON d.id = c.document_id WHERE c.id IN ({}) AND d.deleted_at IS NULL",
            placeholders(group.len())
        ))?;
        let rows = st.query_map(params_from_iter(group.iter()), |r| {
            let chunk = chunk_from_row(r)?;
            let title: Option<String> = r.get(11)?;
            let original: String = r.get(12)?;
            Ok(ChunkRow {
                chunk,
                title: title.filter(|t| !t.trim().is_empty()).unwrap_or(original),
                rel_path: r.get(13)?,
                role: DocumentRole::parse(&r.get::<_, String>(14)?).unwrap_or_default(),
                doc_type: r.get(15)?,
            })
        })?;
        out.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    Ok(out)
}

/// `(chunk id, document id)` for the given chunks — cheap enough for thousands of candidates.
pub fn chunk_docs(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut out = Vec::with_capacity(ids.len());
    for group in ids.chunks(500) {
        let mut st =
            conn.prepare(&format!("SELECT id, document_id FROM chunks WHERE id IN ({})", placeholders(group.len())))?;
        out.extend(
            st.query_map(params_from_iter(group.iter()), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Ok(out)
}

/// `id → tokens` for the given chunks (query-term coverage checks).
pub fn chunk_tokens(conn: &Connection, ids: &[i64]) -> rusqlite::Result<std::collections::HashMap<i64, String>> {
    let mut out = std::collections::HashMap::new();
    for group in ids.chunks(500) {
        let mut st =
            conn.prepare(&format!("SELECT id, tokens FROM chunks WHERE id IN ({})", placeholders(group.len())))?;
        let rows =
            st.query_map(params_from_iter(group.iter()), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (id, tokens) = row?;
            out.insert(id, tokens);
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtsTable {
    Morph,
    Trigram,
}

/// Sorted document ids when the scope is small enough to go into the SQL.
pub fn sql_scope(allowed: Option<&HashSet<String>>) -> Option<Vec<String>> {
    allowed.filter(|a| a.len() <= SCOPE_SQL_MAX_DOCS).map(|a| {
        let mut v: Vec<String> = a.iter().cloned().collect();
        v.sort();
        v
    })
}

/// `(chunk id, bm25)` for an FTS5 MATCH expression; lower bm25 = better. A
/// small document scope is applied inside the query so the top-N is taken
/// within the scope, not globally.
pub fn fts_match(
    conn: &Connection,
    table: FtsTable,
    expr: &str,
    limit: u32,
    allowed: Option<&HashSet<String>>,
) -> rusqlite::Result<Vec<(i64, f64)>> {
    let name = match table {
        FtsTable::Morph => "chunks_fts",
        FtsTable::Trigram => "chunks_fts_tri",
    };
    let scope = sql_scope(allowed);
    let limit = limit as i64;
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&expr, &limit];
    let sql = match &scope {
        Some(docs) => {
            for d in docs {
                args.push(d);
            }
            format!(
                "SELECT rowid, bm25({name}) FROM {name} WHERE {name} MATCH ?1 AND rowid IN (SELECT id FROM chunks WHERE document_id IN ({})) ORDER BY bm25({name}) LIMIT ?2",
                numbered_placeholders(3, docs.len())
            )
        }
        None => format!("SELECT rowid, bm25({name}) FROM {name} WHERE {name} MATCH ?1 ORDER BY bm25({name}) LIMIT ?2"),
    };
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(args.as_slice(), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?)))?;
    let out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(out)
}

/// Substring fallback for queries the FTS indexes cannot serve (≤ 2 chars).
pub fn like_search(
    conn: &Connection,
    needle: &str,
    limit: u32,
    allowed: Option<&HashSet<String>>,
) -> rusqlite::Result<Vec<i64>> {
    let escaped = needle.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let scope = sql_scope(allowed);
    let limit = limit as i64;
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&pattern, &limit];
    let sql = match &scope {
        Some(docs) => {
            for d in docs {
                args.push(d);
            }
            format!(
                "SELECT id FROM chunks WHERE text LIKE ?1 ESCAPE '\\' AND document_id IN ({}) ORDER BY id LIMIT ?2",
                numbered_placeholders(3, docs.len())
            )
        }
        None => "SELECT id FROM chunks WHERE text LIKE ?1 ESCAPE '\\' ORDER BY id LIMIT ?2".to_string(),
    };
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(args.as_slice(), |r| r.get::<_, i64>(0))?;
    let out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(out)
}

/// Resolves a scope to the set of allowed document ids (`None` = no restriction).
///
/// A project scope (0.5.0 plan A) allows the project's work documents ∪ its reference group ∪ the
/// active groups of the 공통 참고 자료; explicit `documents` stand in for the work documents. The
/// project's own documents are wanted even when superseded or in an inactive group.
pub fn scope_doc_ids(conn: &Connection, scope: &SearchScope) -> rusqlite::Result<Option<HashSet<String>>> {
    let mut restricted = false;
    let mut allowed: Option<HashSet<String>> = None;
    let intersect = |allowed: &mut Option<HashSet<String>>, set: HashSet<String>| match allowed {
        Some(a) => a.retain(|x| set.contains(x)),
        None => *allowed = Some(set),
    };
    // Documents the project itself holds (work + its reference group): exempt from the exclusions.
    let mut project_own: HashSet<String> = HashSet::new();
    if let Some(pid) = scope.project_id.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        restricted = true;
        let known: i64 = conn.query_row("SELECT COUNT(*) FROM projects WHERE id = ?1", [pid], |r| r.get(0))?;
        if known == 0 {
            return Ok(Some(HashSet::new())); // an unknown project allows nothing (as the planner scope)
        }
        let reference = project_reference_doc_ids(conn, pid)?;
        let work: HashSet<String> = if scope.documents.is_empty() {
            project_work_doc_ids(conn, pid)?
        } else {
            scope.documents.iter().cloned().collect()
        };
        project_own.extend(work.iter().cloned());
        project_own.extend(reference.iter().cloned());
        let mut set = project_own.clone();
        set.extend(common_reference_doc_ids(conn)?);
        intersect(&mut allowed, set);
    } else if !scope.documents.is_empty() {
        restricted = true;
        intersect(&mut allowed, scope.documents.iter().cloned().collect());
    }
    if !scope.collections.is_empty() {
        restricted = true;
        let mut set = HashSet::new();
        for c in &scope.collections {
            set.extend(collection_doc_ids(conn, c)?);
        }
        intersect(&mut allowed, set);
    }
    if !scope.roles.is_empty() {
        restricted = true;
        let roles: Vec<&str> = scope.roles.iter().map(|r| r.as_str()).collect();
        let mut st = conn.prepare(&format!(
            "SELECT id FROM documents WHERE deleted_at IS NULL AND role IN ({})",
            placeholders(roles.len())
        ))?;
        let set: HashSet<String> =
            st.query_map(params_from_iter(roles.iter()), |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        intersect(&mut allowed, set);
    }
    if let Some(folder) = scope.folder.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        restricted = true;
        let prefix = folder.trim_end_matches('/').to_string() + "/";
        let escaped = prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let mut st =
            conn.prepare("SELECT id FROM documents WHERE deleted_at IS NULL AND rel_path LIKE ?1 ESCAPE '\\'")?;
        let set: HashSet<String> =
            st.query_map([format!("{escaped}%")], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        intersect(&mut allowed, set);
    }
    // Exclusions apply even to an unrestricted scope: superseded editions and
    // documents whose every collection is inactive.
    let mut excluded: HashSet<String> = HashSet::new();
    if scope.latest_only {
        // An edition is superseded only while its group still has a latest edition. Once that one is
        // cleared, demoted, moved to another group or deleted with its document, the editions that are
        // left are all there is — hiding them would leave the title unanswerable.
        let mut st = conn.prepare(
            "SELECT v.document_id FROM doc_versions v WHERE v.is_latest = 0 AND EXISTS (\
               SELECT 1 FROM doc_versions l JOIN documents d ON d.id = l.document_id \
               WHERE l.group_key = v.group_key AND l.is_latest = 1 AND d.deleted_at IS NULL)",
        )?;
        excluded.extend(st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    if scope.collections.is_empty() && scope.active_collections_only {
        let mut st = conn.prepare(
            "SELECT cd.document_id FROM collection_docs cd GROUP BY cd.document_id \
             HAVING SUM(CASE WHEN (SELECT active FROM collections c WHERE c.id = cd.collection_id) = 1 THEN 1 ELSE 0 END) = 0",
        )?;
        excluded.extend(st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    // Documents named explicitly (and a project's own) are wanted even when superseded or in an
    // inactive collection.
    for d in &scope.documents {
        excluded.remove(d);
    }
    for d in &project_own {
        excluded.remove(d);
    }
    if excluded.is_empty() {
        return Ok(if restricted { allowed } else { None });
    }
    if restricted {
        if let Some(a) = allowed.as_mut() {
            a.retain(|d| !excluded.contains(d));
        }
        return Ok(allowed);
    }
    // Unrestricted minus exclusions: materialise the complement.
    let mut st = conn.prepare("SELECT id FROM documents WHERE deleted_at IS NULL")?;
    let all: HashSet<String> = st
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|d| !excluded.contains(d))
        .collect();
    Ok(Some(all))
}

// ---------------------------------------------------------------------------
// Answers
// ---------------------------------------------------------------------------

pub fn insert_answer(conn: &Connection, a: &Answer, scope: &SearchScope) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO answers (kind, question, answer, scope, citations, sentences, model, no_evidence, used_general, document_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            a.kind.as_str(),
            a.question,
            a.text,
            serde_json::to_string(scope).unwrap_or_else(|_| "{}".into()),
            serde_json::to_string(&a.sources).unwrap_or_else(|_| "[]".into()),
            serde_json::to_string(&a.sentences).unwrap_or_else(|_| "[]".into()),
            a.model,
            a.no_evidence as i64,
            a.used_general_knowledge as i64,
            a.document_id,
            now()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

const ANSWER_COLS: &str =
    "id, kind, question, answer, citations, sentences, model, no_evidence, used_general, document_id, feedback, feedback_note, created_at";

fn answer_from_row(r: &Row<'_>) -> rusqlite::Result<SavedAnswer> {
    let citations: String = r.get(4)?;
    let sentences: String = r.get(5)?;
    Ok(SavedAnswer {
        id: r.get(0)?,
        kind: AnswerKind::parse(&r.get::<_, String>(1)?),
        question: r.get(2)?,
        text: r.get(3)?,
        sources: serde_json::from_str::<Vec<Citation>>(&citations).unwrap_or_default(),
        sentences: serde_json::from_str::<Vec<AnswerSentence>>(&sentences).unwrap_or_default(),
        model: r.get(6)?,
        no_evidence: r.get::<_, i64>(7)? != 0,
        used_general_knowledge: r.get::<_, i64>(8)? != 0,
        document_id: r.get(9)?,
        feedback: r.get::<_, Option<i64>>(10)?.map(|v| v as i8),
        feedback_note: r.get(11)?,
        created_at: parse_ts(r.get(12)?).unwrap_or_else(Utc::now),
    })
}

pub fn list_answers(
    conn: &Connection,
    kind: Option<AnswerKind>,
    document_id: Option<&str>,
    limit: u32,
) -> rusqlite::Result<Vec<SavedAnswer>> {
    let mut sql = format!("SELECT {ANSWER_COLS} FROM answers WHERE 1 = 1");
    let mut args: Vec<String> = Vec::new();
    if let Some(k) = kind {
        args.push(k.as_str().to_string());
        sql.push_str(&format!(" AND kind = ?{}", args.len()));
    }
    if let Some(d) = document_id {
        args.push(d.to_string());
        sql.push_str(&format!(" AND document_id = ?{}", args.len()));
    }
    args.push(limit.max(1).to_string());
    sql.push_str(&format!(" ORDER BY created_at DESC, id DESC LIMIT ?{}", args.len()));
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(params_from_iter(args.iter()), answer_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_answer(conn: &Connection, id: i64) -> rusqlite::Result<Option<SavedAnswer>> {
    conn.query_row(&format!("SELECT {ANSWER_COLS} FROM answers WHERE id = ?1"), [id], answer_from_row).optional()
}

pub fn set_answer_feedback(
    conn: &Connection,
    id: i64,
    feedback: Option<i8>,
    note: Option<&str>,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE answers SET feedback = ?2, feedback_note = ?3 WHERE id = ?1",
        params![id, feedback.map(|f| f as i64), note],
    )?;
    Ok(n > 0)
}

pub fn delete_answer(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM answers WHERE id = ?1", [id])? > 0)
}

/// Deletes saved conversations older than `cutoff` (retention purge, §3.8 "대화 기록").
pub fn purge_answers_before(conn: &Connection, cutoff: &chrono::DateTime<chrono::Utc>) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM answers WHERE created_at < ?1", [cutoff.to_rfc3339()])
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Counts for the dashboard (tokenizer / embedding fields are filled by the caller).
pub fn index_status(conn: &Connection) -> rusqlite::Result<IndexStatus> {
    let mut s = IndexStatus::default();
    conn.query_row(
        "SELECT COUNT(*), \
                SUM(CASE WHEN indexed_at IS NOT NULL AND index_version = ?1 AND index_error IS NULL THEN 1 ELSE 0 END), \
                SUM(CASE WHEN has_text_layer = 1 AND (indexed_at IS NULL OR index_version <> ?1 OR updated_at > indexed_at) AND index_error IS NULL THEN 1 ELSE 0 END), \
                SUM(CASE WHEN has_text_layer = 0 THEN 1 ELSE 0 END), \
                SUM(CASE WHEN index_error IS NOT NULL THEN 1 ELSE 0 END), \
                SUM(CASE WHEN role = 'knowledge' THEN 1 ELSE 0 END) \
         FROM documents WHERE deleted_at IS NULL",
        [INDEX_VERSION as i64],
        |r| {
            s.documents_total = r.get::<_, i64>(0)?.max(0) as u64;
            s.documents_indexed = r.get::<_, Option<i64>>(1)?.unwrap_or(0).max(0) as u64;
            s.documents_pending = r.get::<_, Option<i64>>(2)?.unwrap_or(0).max(0) as u64;
            s.documents_no_text = r.get::<_, Option<i64>>(3)?.unwrap_or(0).max(0) as u64;
            s.documents_failed = r.get::<_, Option<i64>>(4)?.unwrap_or(0).max(0) as u64;
            s.knowledge_documents = r.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0) as u64;
            Ok(())
        },
    )?;
    let (total, embedded) = chunk_counts(conn)?;
    s.chunks_total = total;
    s.chunks_embedded = embedded;
    s.collections = conn.query_row("SELECT COUNT(*) FROM collections", [], |r| r.get::<_, i64>(0))? as u64;
    s.glossary_terms = conn.query_row("SELECT COUNT(*) FROM glossary", [], |r| r.get::<_, i64>(0))? as u64;
    conn.query_row(
        "SELECT COUNT(*), SUM(CASE WHEN feedback = 1 THEN 1 ELSE 0 END), SUM(CASE WHEN feedback = -1 THEN 1 ELSE 0 END) FROM answers",
        [],
        |r| {
            s.answers = r.get::<_, i64>(0)? as u64;
            s.answers_helpful = r.get::<_, Option<i64>>(1)?.unwrap_or(0) as u64;
            s.answers_wrong = r.get::<_, Option<i64>>(2)?.unwrap_or(0) as u64;
            Ok(())
        },
    )?;
    Ok(s)
}

/// Documents whose text should be (re)chunked: not deleted, not mid-import,
/// and (unless `force`) never indexed, indexed by an older chunker, updated
/// since, or flagged by an error. Content changes are confirmed by hash at
/// index time, so a document listed here may turn out unchanged.
pub fn documents_to_index(
    conn: &Connection,
    ids: Option<&[String]>,
    force: bool,
) -> rusqlite::Result<Vec<DocumentRecord>> {
    let mut found: Vec<String> = Vec::new();
    match ids {
        Some(ids) => {
            for group in ids.chunks(500) {
                let mut st = conn.prepare(&format!(
                    "SELECT id FROM documents WHERE deleted_at IS NULL AND status NOT IN ('importing', 'missing', 'error') AND id IN ({}) ORDER BY imported_at",
                    placeholders(group.len())
                ))?;
                found.extend(
                    st.query_map(params_from_iter(group.iter()), |r| r.get::<_, String>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?,
                );
            }
        }
        None => {
            let mut sql = String::from(
                "SELECT id FROM documents WHERE deleted_at IS NULL AND status NOT IN ('importing', 'missing', 'error')",
            );
            if !force {
                sql.push_str(
                    " AND (indexed_at IS NULL OR index_version <> ?1 OR index_error IS NOT NULL OR updated_at > indexed_at)",
                );
            }
            sql.push_str(" ORDER BY imported_at");
            let mut st = conn.prepare(&sql)?;
            found = if force {
                st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
            } else {
                st.query_map([INDEX_VERSION as i64], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
            };
        }
    }
    let mut out = Vec::with_capacity(found.len());
    for id in found {
        if let Some(d) = crate::documents::repo::get_document(conn, &id)? {
            out.push(d);
        }
    }
    Ok(out)
}
