//! Tracked physical copies of documents that belong to several projects (0.3.0 plan §6,
//! user decision 6 "양쪽 폴더에 사본"). The `documents` row stays the single record — the knowledge
//! index, the review queue and the aggregates count a document once — while every project after
//! the first gets a file copy in its own folder, tracked in `document_copies` (migration 0008):
//!
//! 1. a copy is written when a document joins a second (third …) project whose settings keep
//!    copies on (`ProjectSettings::copies`, default true);
//! 2. the import planner and the rescan know the copy paths, so a copy is never registered as a
//!    new document ([`is_copy_path`]);
//! 3. when the original moves / is renamed (organise) or its content changes, [`sync_document`]
//!    renames / rewrites the copies whose files are still what DocuLens wrote;
//! 4. a copy edited outside DocuLens hashes differently from what was written → state `modified`
//!    and it is left alone ("원본과 다름 — 다시 동기화" resets it from the original);
//! 5. leaving the project removes that copy, deleting the document removes them all — except a
//!    copy the user edited: its tracking row goes, the file stays where it is (it is their work);
//! 6. a copy is only ever written over its own earlier file: a registered document, another copy
//!    (paths compared without letter case — one file on Windows) or any other file found at the
//!    target makes the copy take the next free name (`_2`, `_3` …);
//! 7. the folder of a copy uses the Korean type labels whatever the UI language, so a rescan under
//!    another language never moves it.

use std::collections::HashMap;
use std::path::Path;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::documents::import::blake3_file;
use crate::documents::{from_rel_path, repo as docs, DocumentRecord};
use crate::error::{CoreError, Result};
use crate::organize::template::{render, sanitize_component, sanitize_folder};
use crate::organize::{document_variable, ProjectVars, TypeLabels};
use crate::paths::WorkspacePaths;
use crate::pipeline::repo as pipeline_repo;
use crate::settings::Language;

use super::{repo, CopyState, DocumentCopy, Project};

const COPY_COLUMNS: &str = "document_id, project_id, rel_path, content_hash, size_bytes, state, created_at, synced_at";

fn row_to_copy(r: &Row<'_>) -> rusqlite::Result<DocumentCopy> {
    let state: String = r.get(5)?;
    let created: String = r.get(6)?;
    let synced: String = r.get(7)?;
    let parse =
        |s: &str| chrono::DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now());
    Ok(DocumentCopy {
        document_id: r.get(0)?,
        project_id: r.get(1)?,
        rel_path: r.get(2)?,
        content_hash: r.get(3)?,
        size_bytes: r.get::<_, i64>(4)?.max(0) as u64,
        state: CopyState::parse(&state).unwrap_or_default(),
        created_at: parse(&created),
        synced_at: parse(&synced),
    })
}

pub fn list_for_project(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<DocumentCopy>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {COPY_COLUMNS} FROM document_copies WHERE project_id = ?1 ORDER BY created_at, rel_path"
    ))?;
    let rows: Vec<DocumentCopy> = st.query_map([project_id], row_to_copy)?.collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn list_for_document(conn: &Connection, doc_id: &str) -> rusqlite::Result<Vec<DocumentCopy>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {COPY_COLUMNS} FROM document_copies WHERE document_id = ?1 ORDER BY created_at, rel_path"
    ))?;
    let rows: Vec<DocumentCopy> = st.query_map([doc_id], row_to_copy)?.collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn all(conn: &Connection) -> rusqlite::Result<Vec<DocumentCopy>> {
    let mut st =
        conn.prepare_cached(&format!("SELECT {COPY_COLUMNS} FROM document_copies ORDER BY created_at, rel_path"))?;
    let rows: Vec<DocumentCopy> = st.query_map([], row_to_copy)?.collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// The copy registered at a workspace-relative path.
pub fn find_by_rel_path(conn: &Connection, rel_path: &str) -> rusqlite::Result<Option<DocumentCopy>> {
    let mut st = conn.prepare_cached(&format!("SELECT {COPY_COLUMNS} FROM document_copies WHERE rel_path = ?1"))?;
    st.query_row([rel_path], row_to_copy).optional()
}

/// Whether a path is a tracked copy (the import planner and the inbox watcher skip those).
pub fn is_copy_path(conn: &Connection, rel_path: &str) -> rusqlite::Result<bool> {
    Ok(find_by_rel_path(conn, rel_path)?.is_some())
}

fn insert(conn: &Connection, copy: &DocumentCopy) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO document_copies (document_id, project_id, rel_path, content_hash, size_bytes, state, \
         created_at, synced_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            copy.document_id,
            copy.project_id,
            copy.rel_path,
            copy.content_hash,
            copy.size_bytes as i64,
            copy.state.as_str(),
            copy.created_at.to_rfc3339(),
            copy.synced_at.to_rfc3339()
        ],
    )?;
    Ok(())
}

fn set_state(conn: &Connection, document_id: &str, project_id: &str, state: CopyState) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE document_copies SET state = ?3 WHERE document_id = ?1 AND project_id = ?2",
        params![document_id, project_id, state.as_str()],
    )?;
    Ok(())
}

fn delete_row(conn: &Connection, document_id: &str, project_id: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "DELETE FROM document_copies WHERE document_id = ?1 AND project_id = ?2",
        params![document_id, project_id],
    )? > 0)
}

fn labels(conn: &Connection) -> rusqlite::Result<TypeLabels> {
    let types = pipeline_repo::doc_types(conn)?;
    Ok(TypeLabels {
        ko: types.iter().map(|t| (t.slug.clone(), t.label_ko.clone())).collect(),
        en: types.iter().map(|t| (t.slug.clone(), t.label_en.clone())).collect(),
    })
}

/// Language of the type / phase labels in copy folders. Fixed: a copy made under the English UI used to be
/// filed under "…/Contract/" and moved to "…/계약서/" by the next rescan (which knows no UI language).
const COPY_LABEL_LANGUAGE: Language = Language::Ko;

/// Whether the copy of `doc` in `project` may not be written at `candidate`: the path belongs to the original,
/// to a registered document or to another tracked copy (compared without letter case: `견적서.PDF` and
/// `견적서.pdf` are one file on Windows), or some other file is there — one the user put in the folder.
/// Only this copy's own earlier file may be written over.
fn path_taken(
    conn: &Connection,
    paths: &WorkspacePaths,
    project: &Project,
    doc: &DocumentRecord,
    candidate: &str,
) -> rusqlite::Result<bool> {
    if candidate.to_lowercase() == doc.rel_path.to_lowercase() {
        return Ok(true);
    }
    let registered: i64 =
        conn.query_row("SELECT COUNT(*) FROM documents WHERE rel_path = ?1 COLLATE NOCASE", [candidate], |r| r.get(0))?;
    if registered > 0 {
        return Ok(true);
    }
    let mut st =
        conn.prepare_cached("SELECT document_id, project_id FROM document_copies WHERE rel_path = ?1 COLLATE NOCASE")?;
    let owners: Vec<(String, String)> =
        st.query_map([candidate], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let own = |(d, p): &(String, String)| *d == doc.id && *p == project.id;
    if owners.iter().any(|o| !own(o)) {
        return Ok(true);
    }
    Ok(!owners.iter().any(own) && from_rel_path(&paths.root, candidate).exists())
}

/// Target path of a document's copy inside a project: the project's folder template
/// (`{{project}}` = this project) plus the document's current file name; a name that is taken
/// ([`path_taken`]) gets `_2`, `_3` ….
fn target_path(
    conn: &Connection,
    paths: &WorkspacePaths,
    project: &Project,
    doc: &DocumentRecord,
) -> rusqlite::Result<String> {
    let labels = labels(conn)?;
    let vars = ProjectVars::from(project);
    let file = doc.rel_path.rsplit('/').next().unwrap_or(&doc.rel_path).to_string();
    let (stem, ext) = match file.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() && e.len() <= 8 => (s.to_string(), e.to_string()),
        _ => (file.clone(), String::new()),
    };
    let (rendered, _) = render(project.settings.folder_template(), &|v| {
        document_variable(doc, &labels, COPY_LABEL_LANGUAGE, v, &stem, &ext, None, Some(&vars))
    });
    let folder = sanitize_folder(&rendered);
    let join = |name: &str| {
        let f = if ext.is_empty() { name.to_string() } else { format!("{name}.{ext}") };
        if folder.is_empty() {
            f
        } else {
            format!("{folder}/{f}")
        }
    };
    let base = sanitize_component(&stem);
    let mut candidate = join(&base);
    let mut n = 2;
    while path_taken(conn, paths, project, doc, &candidate)? {
        candidate = join(&format!("{base}_{n}"));
        n += 1;
        if n > 10_000 {
            break;
        }
    }
    Ok(candidate)
}

/// Copies the original to `to_rel`. Only `replace` — the copy's own earlier file at that path — may be
/// written over; any other file found there is not this module's to touch.
fn write_copy(paths: &WorkspacePaths, from_rel: &str, to_rel: &str, replace: bool) -> Result<(String, u64)> {
    let from = from_rel_path(&paths.root, from_rel);
    let to = from_rel_path(&paths.root, to_rel);
    if !replace && to.exists() {
        return Err(CoreError::Invalid(format!("copy target exists: {to_rel}")));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    std::fs::copy(&from, &to).map_err(|e| CoreError::io(&from, e))?;
    let hash = blake3_file(&to)?;
    let size = std::fs::metadata(&to).map(|m| m.len()).unwrap_or(0);
    Ok((hash, size))
}

fn remove_file(path: &Path) {
    if path.is_file() {
        let _ = std::fs::remove_file(path);
    }
}

/// Deletes the file of a copy whose tracking ends (the document leaves the project, the project or the
/// document is deleted) — unless the user edited it: a file that no longer hashes as DocuLens wrote it is
/// their work and stays where it is, untracked. Returns whether the file was deleted.
fn remove_copy_file(paths: &WorkspacePaths, copy: &DocumentCopy) -> bool {
    let abs = from_rel_path(&paths.root, &copy.rel_path);
    if !abs.is_file() {
        return false;
    }
    match blake3_file(&abs) {
        Ok(hash) if hash == copy.content_hash => std::fs::remove_file(&abs).is_ok(),
        Ok(_) => {
            log::info!("the edited copy {} is kept (no longer tracked)", copy.rel_path);
            false
        }
        Err(e) => {
            log::warn!("copy {} left in place: {e}", copy.rel_path);
            false
        }
    }
}

/// Writes the copy of `doc` in `project` when it has none on disk; returns the stored row.
fn ensure_copy(db: &Db, paths: &WorkspacePaths, project: &Project, doc: &DocumentRecord) -> Result<DocumentCopy> {
    let existing = db.with(|c| {
        let mut st = c.prepare_cached(&format!(
            "SELECT {COPY_COLUMNS} FROM document_copies WHERE document_id = ?1 AND project_id = ?2"
        ))?;
        st.query_row(params![doc.id, project.id], row_to_copy).optional()
    })?;
    if let Some(copy) = existing {
        if from_rel_path(&paths.root, &copy.rel_path).is_file() {
            return Ok(copy);
        }
    }
    let rel = db.with(|c| target_path(c, paths, project, doc))?;
    // A fresh path (or this copy's own, whose file is gone): nothing is there to replace.
    let (hash, size) = write_copy(paths, &doc.rel_path, &rel, false)?;
    let created = Utc::now();
    let copy = DocumentCopy {
        document_id: doc.id.clone(),
        project_id: project.id.clone(),
        rel_path: rel,
        content_hash: hash,
        size_bytes: size,
        state: CopyState::Synced,
        created_at: created,
        synced_at: created,
    };
    db.with(|c| insert(c, &copy))?;
    Ok(copy)
}

/// Adds documents to a project (`repo::add_documents`) and writes the copies the membership
/// calls for: a document that already belongs to another project gets a copy in this project's
/// folder when the project keeps copies. Copy failures do not undo the membership (they are
/// retried by [`rescan`]). Returns the number of documents actually added.
///
/// `_lang` (the UI language) no longer plays a part: copy folders always use the Korean type labels
/// ([`COPY_LABEL_LANGUAGE`]). The parameter stays for the callers.
pub fn add_documents(
    db: &Db,
    paths: &WorkspacePaths,
    project_id: &str,
    doc_ids: &[String],
    _lang: Language,
) -> Result<u32> {
    let before: HashMap<String, usize> = db.with(|c| {
        let mut out = HashMap::new();
        for id in doc_ids {
            let n: i64 = c.query_row(
                "SELECT COUNT(*) FROM project_documents WHERE document_id = ?1 AND project_id <> ?2",
                params![id, project_id],
                |r| r.get(0),
            )?;
            out.insert(id.clone(), n.max(0) as usize);
        }
        Ok(out)
    })?;
    let added = db.with(|c| repo::add_documents(c, project_id, doc_ids))?;
    let project = db.with(|c| repo::get_project(c, project_id))?;
    let Some(project) = project else { return Ok(added) };
    if !project.settings.copies {
        return Ok(added);
    }
    for id in doc_ids {
        if before.get(id).copied().unwrap_or(0) == 0 {
            continue; // first project: the original file is the project's copy
        }
        let Some(doc) = db.with(|c| docs::get_document(c, id))? else { continue };
        if !from_rel_path(&paths.root, &doc.rel_path).is_file() {
            continue;
        }
        if let Err(e) = ensure_copy(db, paths, &project, &doc) {
            log::warn!("copy of {} for project {} failed: {e}", doc.id, project.id);
        }
    }
    Ok(added)
}

/// Removes a document from a project together with its copy in that project (a copy the user edited
/// keeps its file, see [`remove_copy_file`]).
pub fn remove_document(db: &Db, paths: &WorkspacePaths, project_id: &str, doc_id: &str) -> Result<bool> {
    let copies = db.with(|c| list_for_document(c, doc_id))?;
    for copy in copies.iter().filter(|c| c.project_id == project_id) {
        remove_copy_file(paths, copy);
        db.with(|c| delete_row(c, doc_id, project_id))?;
    }
    db.with(|c| repo::remove_document(c, project_id, doc_id))
}

/// Ends the tracking of every copy of a project (before the project row goes) and deletes the copy files
/// DocuLens wrote; copies the user edited stay in the folder. Returns the number of copies untracked.
pub fn remove_project_copies(db: &Db, paths: &WorkspacePaths, project_id: &str) -> Result<u32> {
    let copies = db.with(|c| list_for_project(c, project_id))?;
    for copy in &copies {
        remove_copy_file(paths, copy);
        db.with(|c| delete_row(c, &copy.document_id, &copy.project_id))?;
    }
    Ok(copies.len() as u32)
}

/// Ends the tracking of every copy of a document (before the document is deleted) and deletes the copy
/// files DocuLens wrote; copies the user edited stay. Returns the number of copies untracked.
pub fn remove_all_for_document(db: &Db, paths: &WorkspacePaths, doc_id: &str) -> Result<u32> {
    let copies = db.with(|c| list_for_document(c, doc_id))?;
    for copy in &copies {
        remove_copy_file(paths, copy);
        db.with(|c| delete_row(c, &copy.document_id, &copy.project_id))?;
    }
    Ok(copies.len() as u32)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SyncReport {
    pub renamed: u32,
    pub rewritten: u32,
    pub created: u32,
    pub modified: u32,
    pub missing: u32,
    pub removed: u32,
}

/// Brings the copies of one document in line with its original: a copy whose file still hashes
/// as written follows a rename / move and is rewritten when the original's content changed; an
/// externally edited copy is marked `modified` and left alone (`force` rewrites it anyway).
pub fn sync_document(db: &Db, paths: &WorkspacePaths, doc_id: &str, force: bool) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let Some(doc) = db.with(|c| docs::get_document(c, doc_id))? else { return Ok(report) };
    let original_abs = from_rel_path(&paths.root, &doc.rel_path);
    if !original_abs.is_file() {
        return Ok(report);
    }
    let original_hash = blake3_file(&original_abs)?;
    let copies = db.with(|c| list_for_document(c, doc_id))?;
    for copy in copies {
        let Some(project) = db.with(|c| repo::get_project(c, &copy.project_id))? else {
            db.with(|c| delete_row(c, &copy.document_id, &copy.project_id))?;
            report.removed += 1;
            continue;
        };
        let abs = from_rel_path(&paths.root, &copy.rel_path);
        let on_disk = if abs.is_file() { Some(blake3_file(&abs)?) } else { None };
        let unmodified = on_disk.as_deref() == Some(copy.content_hash.as_str());
        if on_disk.is_some() && !unmodified && !force {
            db.with(|c| set_state(c, &copy.document_id, &copy.project_id, CopyState::Modified))?;
            report.modified += 1;
            continue;
        }
        // Where the copy should live now (the original may have been renamed / moved).
        let want = db.with(|c| target_path(c, paths, &project, &doc))?;
        let same_place = want == copy.rel_path;
        if same_place && unmodified && original_hash == copy.content_hash {
            continue;
        }
        if !same_place && on_disk.is_some() {
            remove_file(&abs);
        }
        // In place this copy's own file is rewritten; a new place is free (`target_path`).
        let (hash, size) = write_copy(paths, &doc.rel_path, &want, same_place)?;
        let fresh = DocumentCopy {
            rel_path: want,
            content_hash: hash,
            size_bytes: size,
            state: CopyState::Synced,
            synced_at: Utc::now(),
            ..copy.clone()
        };
        db.with(|c| {
            delete_row(c, &copy.document_id, &copy.project_id)?;
            insert(c, &fresh)
        })?;
        if on_disk.is_none() {
            report.missing += 1;
        } else if same_place {
            report.rewritten += 1;
        } else {
            report.renamed += 1;
        }
    }
    Ok(report)
}

/// Re-syncs every copy of a project's documents from their originals (the "다시 동기화" action).
pub fn sync_project(db: &Db, paths: &WorkspacePaths, project_id: &str) -> Result<SyncReport> {
    let ids = db.with(|c| repo::member_ids(c, project_id))?;
    let mut total = SyncReport::default();
    for id in ids {
        let r = sync_document(db, paths, &id, true)?;
        total.renamed += r.renamed;
        total.rewritten += r.rewritten;
        total.created += r.created;
        total.modified += r.modified;
        total.missing += r.missing;
        total.removed += r.removed;
    }
    // Memberships that never got their copy (classification, imports into a project …).
    total.created += ensure_missing_copies(db, paths, Some(project_id))?;
    Ok(total)
}

/// Writes the copies that memberships call for but that do not exist yet (second and later
/// projects of a document, copies on). Returns how many were written.
fn ensure_missing_copies(db: &Db, paths: &WorkspacePaths, only_project: Option<&str>) -> Result<u32> {
    let memberships = db.with(repo::all_memberships)?;
    let mut first_project: HashMap<String, String> = HashMap::new();
    let mut created = 0u32;
    for (doc_id, project_id) in memberships {
        let home = first_project.entry(doc_id.clone()).or_insert_with(|| project_id.clone());
        if *home == project_id {
            continue;
        }
        if only_project.is_some_and(|p| p != project_id) {
            continue;
        }
        let exists = db.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM document_copies WHERE document_id = ?1 AND project_id = ?2",
                params![doc_id, project_id],
                |r| r.get::<_, i64>(0),
            )
        })? > 0;
        if exists {
            continue;
        }
        let Some(project) = db.with(|c| repo::get_project(c, &project_id))? else { continue };
        if !project.settings.copies {
            continue;
        }
        let Some(doc) = db.with(|c| docs::get_document(c, &doc_id))? else { continue };
        if doc.deleted_at.is_some() || !from_rel_path(&paths.root, &doc.rel_path).is_file() {
            continue;
        }
        match ensure_copy(db, paths, &project, &doc) {
            Ok(_) => created += 1,
            Err(e) => log::warn!("copy of {} for project {} failed: {e}", doc.id, project.id),
        }
    }
    Ok(created)
}

/// Rescan pass over every copy: missing files are flagged, externally edited copies become
/// `modified`, copies whose original changed are refreshed, and copies that memberships still
/// owe are written. Called at the end of `documents::import::rescan`.
pub fn rescan(db: &Db, paths: &WorkspacePaths) -> Result<SyncReport> {
    let copies = db.with(all)?;
    let mut seen = std::collections::HashSet::new();
    let mut total = SyncReport::default();
    for copy in copies {
        if !seen.insert(copy.document_id.clone()) {
            continue;
        }
        let r = sync_document(db, paths, &copy.document_id, false)?;
        total.renamed += r.renamed;
        total.rewritten += r.rewritten;
        total.modified += r.modified;
        total.missing += r.missing;
        total.removed += r.removed;
    }
    total.created += ensure_missing_copies(db, paths, None)?;
    Ok(total)
}
