//! Resolves a plan scope to the documents it covers (shared by the executor,
//! the aggregate, export and organise modules and the previews).

use rusqlite::Connection;

use crate::documents::repo::list_documents;
use crate::documents::{DocumentFilter, DocumentRecord, DocumentRole, DocumentSort, DocumentStatus};

use super::{compact, Scope, TypeMatch};

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

/// The names a document type goes by, as comparison keys ([`compact`]): the names of the forms
/// registered for it, the labels of the built-in sample form and the aliases commands use
/// (`pipeline::schema::doc_type_aliases`). Only names of three or more characters: "계약" or "cr"
/// inside a file name says nothing about its type.
pub fn type_names(conn: &Connection, slug: &str) -> rusqlite::Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    let mut st = conn.prepare_cached("SELECT name, name_en FROM schemas WHERE doc_type = ?1")?;
    let rows = st.query_map([slug], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
    for row in rows {
        let (name, name_en) = row?;
        names.push(name);
        names.extend(name_en);
    }
    for b in crate::pipeline::schema::builtins().into_iter().filter(|b| b.doc_type == slug) {
        names.push(b.name_ko.to_string());
        names.push(b.name_en.to_string());
    }
    names.extend(crate::pipeline::schema::doc_type_aliases(slug).iter().map(|a| a.to_string()));
    let mut keys: Vec<String> = names.iter().map(|n| compact(n)).filter(|k| k.chars().count() >= 3).collect();
    keys.sort();
    keys.dedup();
    Ok(keys)
}

/// Whether the document carries one of the type's names (`type_names`) in its title, its original
/// file name or the file name it has now.
pub fn named_as(doc: &DocumentRecord, names: &[String]) -> bool {
    let file = doc.rel_path.rsplit(['/', '\\']).next().unwrap_or_default();
    let own = [doc.title.as_deref().unwrap_or_default(), doc.original_name.as_str(), file].map(compact);
    names.iter().any(|n| own.iter().any(|o| o.contains(n.as_str())))
}

/// Documents in scope, sorted by path. Deleted / importing / missing / error
/// documents are never in scope. `doc_ids` and the filter intersect.
///
/// 1.0.5 (user decision B): a `doc_type` condition takes the documents classified as the type and
/// the ones that carry the type's name in their title or file name — a document the classifier has
/// not seen, or got wrong, is still "the contract" to its owner. `filter.type_match` narrows it to
/// one of the two. A condition that matches nothing matches nothing: it is never dropped.
pub fn resolve(conn: &Connection, scope: &Scope) -> rusqlite::Result<Vec<DocumentRecord>> {
    let f = &scope.filter;
    let filter = DocumentFilter {
        folder: scope.folder.clone(),
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
    if let Some(slug) = f.doc_type.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let classified = |d: &DocumentRecord| d.doc_type.as_deref() == Some(slug);
        match f.type_match {
            Some(TypeMatch::Classified) => docs.retain(classified),
            Some(TypeMatch::Named) => {
                let names = type_names(conn, slug)?;
                docs.retain(|d| named_as(d, &names));
            }
            Some(TypeMatch::Both) | None => {
                let names = type_names(conn, slug)?;
                docs.retain(|d| classified(d) || named_as(d, &names));
            }
        }
    }
    // Project scope (plan §6): the project's members only (an unknown project → nothing).
    // "참고 자료에서 …" / "업무 문서만 …" (0.8.1): the kind of document, when the sentence names one.
    let role = f.role.map(super::RoleParam::as_role);
    if let Some(pid) = f.project_id.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        // A project holds its work documents (`project_documents`) and its 참고 자료 (its reference
        // group). Which of the two a document is here is decided by where the project holds it, not
        // by the document's own mark: a regulation that is reference material of another project is
        // still this project's work document (1.0.1 — the mark used to drop it from "업무 문서만").
        let work = || -> rusqlite::Result<std::collections::HashSet<String>> {
            let mut st = conn.prepare_cached("SELECT document_id FROM project_documents WHERE project_id = ?1")?;
            let rows = st.query_map([pid], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        };
        let members = match role {
            Some(DocumentRole::Knowledge) => crate::knowledge::repo::project_reference_doc_ids(conn, pid)?,
            Some(_) => work()?,
            // 1.0.1: a plan that only reads covers both (`ScopeFilter::with_reference`).
            None if f.with_reference => {
                let mut both = work()?;
                both.extend(crate::knowledge::repo::project_reference_doc_ids(conn, pid)?);
                both
            }
            None => work()?,
        };
        docs.retain(|d| members.contains(&d.id));
        if let Some(role) = role.filter(|r| !matches!(r, DocumentRole::Work | DocumentRole::Knowledge)) {
            docs.retain(|d| d.role == role);
        }
    } else if let Some(role) = role {
        // Without a project the mark decides — and, for reference material, a place in a reference
        // group (the mark follows the groups, but a document may have been marked by hand since).
        let grouped = if role == DocumentRole::Knowledge {
            crate::knowledge::repo::reference_doc_ids(conn)?
        } else {
            std::collections::HashSet::new()
        };
        docs.retain(|d| d.role == role || grouped.contains(&d.id));
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
