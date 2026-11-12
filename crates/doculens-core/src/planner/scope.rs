//! Resolves a plan scope to the documents it covers (shared by the executor,
//! the aggregate, export and organise modules and the previews).

use rusqlite::Connection;

use crate::documents::repo::list_documents;
use crate::documents::{DocumentFilter, DocumentRecord, DocumentRole, DocumentSort, DocumentStatus};

use super::{compact, Scope};

/// Date a document is filed under: its document date, else the import date
/// (design §3.5 "날짜 기준 필드는 doc_date 기본"). Returns `(YYYY-MM-DD, basis)`.
///
/// `doc_date` is the normalised value of the schema's date field, but a schema may type that
/// field as plain text ("2026년10월5일", "10월 중순"): only a leading ISO date counts. The first
/// ten bytes used to be sliced off whatever the string was, which panics inside a Hangul
/// character (audit PLN-19).
pub fn basis_date(doc: &DocumentRecord) -> (Option<String>, &'static str) {
    let iso = doc
        .doc_date
        .as_deref()
        .map(str::trim)
        .and_then(|d| d.get(..10))
        .filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok());
    match iso {
        Some(d) => (Some(d.to_string()), "doc_date"),
        None => (Some(doc.imported_at.format("%Y-%m-%d").to_string()), "imported_at"),
    }
}

/// Of `ids`, the documents whose latest extraction carries review work: a field the user
/// approved, corrected, rejected or put on hold. Re-extracting such a document replaces that
/// work (the planner's `extract` confirms first — audit PLN-3).
pub fn reviewed_documents(conn: &Connection, ids: &[String]) -> rusqlite::Result<std::collections::HashSet<String>> {
    let mut out = std::collections::HashSet::new();
    for group in ids.chunks(400) {
        let marks = vec!["?"; group.len()].join(", ");
        let mut st = conn.prepare(&format!(
            "SELECT DISTINCT f.document_id FROM fields f JOIN documents d ON d.id = f.document_id AND d.last_run_id = f.run_id \
             WHERE f.status != 'auto' AND f.document_id IN ({marks})"
        ))?;
        for id in st.query_map(rusqlite::params_from_iter(group.iter()), |r| r.get::<_, String>(0))? {
            out.insert(id?);
        }
    }
    Ok(out)
}

/// Documents in scope, sorted by path. Deleted / importing / missing / error
/// documents are never in scope. `doc_ids` and the filter intersect.
pub fn resolve(conn: &Connection, scope: &Scope) -> rusqlite::Result<Vec<DocumentRecord>> {
    let f = &scope.filter;
    let filter = DocumentFilter {
        folder: scope.folder.clone(),
        doc_type: f.doc_type.clone(),
        tag: f.tags.first().cloned(),
        review_status: f.review_status,
        query: f.keyword.clone(),
        statuses: f.status.as_deref().and_then(DocumentStatus::parse).into_iter().collect(),
        sort: DocumentSort::NameAsc,
        ..DocumentFilter::default()
    };
    let mut docs = list_documents(conn, &filter)?;
    docs.retain(|d| !matches!(d.status, DocumentStatus::Importing | DocumentStatus::Missing | DocumentStatus::Error));
    if !scope.doc_ids.is_empty() {
        docs.retain(|d| scope.doc_ids.contains(&d.id));
    }
    // Project scope (plan §6): the project's members only (an unknown project → nothing).
    // "참고 자료에서 …" / "업무 문서만 …" (0.8.1): the kind of document, when the sentence names one.
    let role = f.role.map(super::RoleParam::as_role);
    if let Some(pid) = f.project_id.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        // A project holds its work documents (`project_documents`) and its 참고 자료 (its reference
        // group): the latter only when the scope asks for reference material.
        let members: std::collections::HashSet<String> = if role == Some(DocumentRole::Knowledge) {
            crate::knowledge::repo::project_reference_doc_ids(conn, pid)?
        } else {
            let mut st = conn.prepare_cached("SELECT document_id FROM project_documents WHERE project_id = ?1")?;
            let rows = st.query_map([pid], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
            rows
        };
        docs.retain(|d| members.contains(&d.id));
    }
    if let Some(role) = role {
        docs.retain(|d| d.role == role);
    }
    if let Some((from, to)) = f.date_range() {
        docs.retain(|d| {
            basis_date(d)
                .0
                .and_then(|s| chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
                .is_some_and(|d| d >= from && d <= to)
        });
    }
    if let Some(v) = f.vendor.as_deref().map(compact).filter(|v| !v.is_empty()) {
        docs.retain(|d| {
            d.vendor.as_deref().is_some_and(|x| compact(x).contains(&v))
                || d.title.as_deref().is_some_and(|x| compact(x).contains(&v))
                || compact(&d.original_name).contains(&v)
        });
    }
    if f.tags.len() > 1 {
        docs.retain(|d| f.tags.iter().all(|t| d.tags.iter().any(|x| x.eq_ignore_ascii_case(t))));
    }
    if let Some(min) = f.min_confidence {
        docs.retain(|d| d.confidence.is_some_and(|c| c >= min));
    }
    docs.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(docs)
}
