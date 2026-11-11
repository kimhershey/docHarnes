//! Organise / rename / move with a dry-run diff, the file-operation journal
//! and undo / redo (design §3.5 "되돌리기", §3.7 "정리/내보내기").
//!
//! `plan` renders the templates for every document in scope and produces the
//! diff table the user confirms; `apply` replays exactly that plan (rename
//! with copy fallback, DB path update, journal entry per file), stopping at
//! the first cancel so the journal always describes what really happened.

pub mod journal;
pub mod template;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::documents::{from_rel_path, repo as docs, DocumentRecord, ReviewStatus};
use crate::error::{CoreError, Result};
use crate::paths::WorkspacePaths;
use crate::pipeline::repo as pipeline_repo;
use crate::planner::scope;
use crate::planner::{GroupBy, Scope};
use crate::settings::Language;

use journal::{EntryKind, NewEntry};
use template::{date_parts, format_total, sanitize_component, sanitize_folder};

/// Default file-name template (`2026-10-14_영수증_스타벅스_12,500.jpg`).
pub const NAME_TEMPLATE: &str = "{{year}}-{{month}}-{{day}}_{{doc_type}}_{{vendor}}_{{total}}";

/// Default folder template per grouping (design §3.7 정리 화면; `Project` = plan §6).
pub fn folder_template(group: GroupBy) -> &'static str {
    match group {
        GroupBy::Month => "documents/{{year}}/{{month}}/{{doc_type}}",
        GroupBy::Vendor => "documents/{{vendor}}/{{doc_type}}",
        GroupBy::Project => crate::projects::DEFAULT_PROJECT_FOLDER,
        GroupBy::DocType | GroupBy::None => "documents/{{doc_type}}",
    }
}

/// Project values behind the `{{project*}}` template variables.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectVars {
    pub name: String,
    /// 접수번호 (`{{project_code}}`).
    pub code: Option<String>,
    pub phase: &'static str,
    pub start: Option<String>,
    pub end: Option<String>,
    pub owner: Option<String>,
}

impl From<&crate::projects::Project> for ProjectVars {
    fn from(p: &crate::projects::Project) -> Self {
        ProjectVars {
            name: p.name.clone(),
            code: p.intake_no.clone().filter(|c| !c.trim().is_empty()),
            phase: p.phase.as_str(),
            start: p.start_date.clone(),
            end: p.end_date.clone(),
            owner: p.owner.clone().filter(|o| !o.trim().is_empty()),
        }
    }
}

/// The project each document is filed under for the template variables: the scope's project
/// when the request names one, else the document's first (oldest membership) project.
pub(crate) fn project_vars(
    conn: &rusqlite::Connection,
    scope: &Scope,
    doc_ids: &[String],
) -> rusqlite::Result<HashMap<String, ProjectVars>> {
    let mut out = HashMap::new();
    if let Some(pid) = scope.filter.project_id.as_deref() {
        if let Some(p) = crate::projects::repo::get_project(conn, pid)? {
            let vars = ProjectVars::from(&p);
            for id in doc_ids {
                out.insert(id.clone(), vars.clone());
            }
        }
        return Ok(out);
    }
    let memberships = crate::projects::repo::all_memberships(conn)?;
    let wanted: HashSet<&str> = doc_ids.iter().map(String::as_str).collect();
    let mut projects: HashMap<String, ProjectVars> = HashMap::new();
    for (doc_id, project_id) in memberships {
        if !wanted.contains(doc_id.as_str()) || out.contains_key(&doc_id) {
            continue;
        }
        if !projects.contains_key(&project_id) {
            match crate::projects::repo::get_project(conn, &project_id)? {
                Some(p) => projects.insert(project_id.clone(), ProjectVars::from(&p)),
                None => continue,
            };
        }
        if let Some(v) = projects.get(&project_id) {
            out.insert(doc_id, v.clone());
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OrganizeRequest {
    pub scope: Scope,
    /// File-name template (`None` = keep the current name).
    pub name_template: Option<String>,
    /// Folder template, workspace-relative (`None` = keep the current folder).
    pub folder_template: Option<String>,
    pub only_reviewed: bool,
    pub language: Language,
}

impl Default for OrganizeRequest {
    fn default() -> Self {
        Self {
            scope: Scope::default(),
            name_template: None,
            folder_template: None,
            only_reviewed: true,
            language: Language::Ko,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// Will be moved / renamed.
    Move,
    /// Template renders to the current path.
    Unchanged,
    /// Skipped: review pending (`only_reviewed`).
    Held,
    /// Skipped: the source file is missing.
    Missing,
    /// Apply outcome.
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrganizeItem {
    pub doc_id: String,
    pub name: String,
    pub from: String,
    pub to: Option<String>,
    pub status: ItemStatus,
    /// `review_pending` | `missing_file` | `project_copy` (unchanged: in the scope's project the document is
    /// a tracked copy of another project's original) | an apply error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Template variables without a value (rendered as `미상` → "검토 필요").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct OrganizePlan {
    pub items: Vec<OrganizeItem>,
    pub moves: u64,
    pub unchanged: u64,
    pub held: u64,
    pub missing_files: u64,
    /// Items that render with `미상` somewhere.
    pub needs_review: u64,
    pub name_template: Option<String>,
    pub folder_template: Option<String>,
    pub only_reviewed: bool,
}

pub(crate) struct TypeLabels {
    pub(crate) ko: Vec<(String, String)>,
    pub(crate) en: Vec<(String, String)>,
}

impl TypeLabels {
    pub(crate) fn label(&self, slug: &str, lang: Language) -> Option<String> {
        let list = if lang == Language::Ko { &self.ko } else { &self.en };
        list.iter().find(|(s, _)| s == slug).map(|(_, l)| l.clone())
    }
}

fn split_name(name: &str) -> (String, String) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 8 => {
            (stem.to_string(), ext.to_string())
        }
        _ => (name.to_string(), String::new()),
    }
}

/// Renders the templates for the documents in scope (the diff table).
pub fn plan(db: &Db, paths: &WorkspacePaths, req: &OrganizeRequest) -> Result<OrganizePlan> {
    if let Some(t) = req.name_template.as_deref() {
        let unknown = template::unknown_variables(t);
        if !unknown.is_empty() {
            return Err(CoreError::Invalid(format!("template_variable: {}", unknown.join(", "))));
        }
    }
    if let Some(t) = req.folder_template.as_deref() {
        let unknown = template::unknown_variables(t);
        if !unknown.is_empty() {
            return Err(CoreError::Invalid(format!("template_variable: {}", unknown.join(", "))));
        }
    }
    let (documents, labels, projects, filed_as_copies) = db.with(|c| {
        let docs = scope::resolve(c, &req.scope)?;
        let types = pipeline_repo::doc_types(c)?;
        let labels = TypeLabels {
            ko: types.iter().map(|t| (t.slug.clone(), t.label_ko.clone())).collect(),
            en: types.iter().map(|t| (t.slug.clone(), t.label_en.clone())).collect(),
        };
        let ids: Vec<String> = docs.iter().map(|d| d.id.clone()).collect();
        let projects = project_vars(c, &req.scope, &ids)?;
        // Documents that are in the scope's project through a tracked copy: the copy is that project's
        // file, the original belongs to the folder of an earlier project.
        let filed_as_copies: HashSet<String> = match req.scope.filter.project_id.as_deref() {
            Some(pid) => {
                crate::projects::copies::list_for_project(c, pid)?.into_iter().map(|copy| copy.document_id).collect()
            }
            None => HashSet::new(),
        };
        Ok((docs, labels, projects, filed_as_copies))
    })?;
    let mut out = OrganizePlan {
        name_template: req.name_template.clone(),
        folder_template: req.folder_template.clone(),
        only_reviewed: req.only_reviewed,
        ..OrganizePlan::default()
    };
    let mut taken: HashSet<String> = HashSet::new();
    let mut seq_by_folder: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for doc in &documents {
        let display = doc.title.clone().unwrap_or_else(|| doc.original_name.clone());
        let from = doc.rel_path.clone();
        let abs = from_rel_path(&paths.root, &from);
        if !abs.is_file() {
            out.missing_files += 1;
            out.items.push(OrganizeItem {
                doc_id: doc.id.clone(),
                name: display,
                from,
                to: None,
                status: ItemStatus::Missing,
                reason: Some("missing_file".into()),
                missing: Vec::new(),
            });
            continue;
        }
        if req.only_reviewed && doc.review_status == ReviewStatus::Pending {
            out.held += 1;
            out.items.push(OrganizeItem {
                doc_id: doc.id.clone(),
                name: display,
                from,
                to: None,
                status: ItemStatus::Held,
                reason: Some("review_pending".into()),
                missing: Vec::new(),
            });
            continue;
        }
        if filed_as_copies.contains(&doc.id) {
            // Organising this project must not pull the original out of its own project's folder and drop
            // it next to its copy (`…_2`); the copy stays put and keeps following the original's name.
            out.unchanged += 1;
            out.items.push(OrganizeItem {
                doc_id: doc.id.clone(),
                name: display,
                from,
                to: None,
                status: ItemStatus::Unchanged,
                reason: Some("project_copy".into()),
                missing: Vec::new(),
            });
            continue;
        }
        let (stem, ext) = split_name(doc.original_name.rsplit('/').next().unwrap_or(&doc.original_name));
        let file_name_on_disk = from.rsplit('/').next().unwrap_or(&from).to_string();
        let (disk_stem, disk_ext) = split_name(&file_name_on_disk);
        let ext = if disk_ext.is_empty() { ext } else { disk_ext };
        let stem = if disk_stem.is_empty() { stem } else { disk_stem };
        let mut missing = Vec::new();
        let project = projects.get(&doc.id);
        // Folder first (it must not use {{seq}}), then the running number inside that folder.
        let folder = match req.folder_template.as_deref() {
            Some(t) => {
                let (rendered, miss) = template::render(t, &|v| {
                    document_variable(doc, &labels, req.language, v, &stem, &ext, None, project)
                });
                missing.extend(miss);
                sanitize_folder(&rendered)
            }
            None => doc.rel_dir().to_string(),
        };
        let seq = {
            let n = seq_by_folder.entry(folder.clone()).or_insert(0);
            *n += 1;
            *n
        };
        let name = match req.name_template.as_deref() {
            Some(t) => {
                let (rendered, miss) = template::render(t, &|v| {
                    document_variable(doc, &labels, req.language, v, &stem, &ext, Some(seq), project)
                });
                missing.extend(miss);
                sanitize_component(&rendered)
            }
            None => stem.clone(),
        };
        missing.sort();
        missing.dedup();
        let mut candidate = join(&folder, &name, &ext);
        // Keep the whole relative path within the Windows-safe budget.
        let budget = template::MAX_PATH_CHARS;
        if candidate.chars().count() > budget {
            let overflow = candidate.chars().count() - budget;
            let short: String = name.chars().take(name.chars().count().saturating_sub(overflow).max(1)).collect();
            candidate = join(&folder, &short, &ext);
        }
        if candidate == from {
            out.unchanged += 1;
            taken.insert(candidate.to_lowercase());
            out.items.push(OrganizeItem {
                doc_id: doc.id.clone(),
                name: display,
                from,
                to: None,
                status: ItemStatus::Unchanged,
                reason: None,
                missing: Vec::new(),
            });
            continue;
        }
        // Collisions inside the batch or on disk → `_2`, `_3` …
        let base_name = name.clone();
        let mut n = 2;
        // Windows paths are case-insensitive: compare lower-cased.
        while taken.contains(&candidate.to_lowercase()) || from_rel_path(&paths.root, &candidate).exists() {
            candidate = join(&folder, &format!("{base_name}_{n}"), &ext);
            n += 1;
            if n > 10_000 {
                break;
            }
        }
        taken.insert(candidate.to_lowercase());
        if !missing.is_empty() {
            out.needs_review += 1;
        }
        out.moves += 1;
        out.items.push(OrganizeItem {
            doc_id: doc.id.clone(),
            name: display,
            from,
            to: Some(candidate),
            status: ItemStatus::Move,
            reason: None,
            missing,
        });
    }
    Ok(out)
}

fn join(folder: &str, name: &str, ext: &str) -> String {
    let file = if ext.is_empty() { name.to_string() } else { format!("{name}.{ext}") };
    if folder.is_empty() {
        file
    } else {
        format!("{folder}/{file}")
    }
}

/// Value of one template variable for a document (`None` = unknown → `미상`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn document_variable(
    doc: &DocumentRecord,
    labels: &TypeLabels,
    lang: Language,
    var: &str,
    stem: &str,
    ext: &str,
    seq: Option<u32>,
    project: Option<&ProjectVars>,
) -> Option<String> {
    let (date, _) = scope::basis_date(doc);
    let parts = date.as_deref().and_then(date_parts);
    match var {
        "project" => project.map(|p| p.name.clone()),
        "project_code" => project.and_then(|p| p.code.clone()),
        "project_phase" => project.map(|p| {
            crate::projects::ProjectPhase::parse(p.phase).unwrap_or_default().label(lang == Language::Ko).to_string()
        }),
        "project_start" => project.and_then(|p| p.start.clone()),
        "project_end" => project.and_then(|p| p.end.clone()),
        "project_owner" => project.and_then(|p| p.owner.clone()),
        "year" => parts.as_ref().map(|p| p.0.clone()),
        "month" => parts.as_ref().map(|p| p.1.clone()),
        "day" => parts.as_ref().map(|p| p.2.clone()).filter(|d| !d.is_empty()),
        "doc_type" => doc.doc_type.as_deref().map(|slug| labels.label(slug, lang).unwrap_or_else(|| slug.to_string())),
        "vendor" => doc.vendor.clone().filter(|v| !v.trim().is_empty()),
        "total" => doc.total_amount.map(format_total),
        "title" => doc.title.clone().filter(|t| !t.trim().is_empty()).or_else(|| Some(stem.to_string())),
        "original_name" | "name" => Some(stem.to_string()),
        "ext" => Some(ext.to_string()),
        "tags" => {
            if doc.tags.is_empty() {
                None
            } else {
                Some(doc.tags.join("-"))
            }
        }
        "seq" => seq.map(|n| format!("{n:02}")),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ApplyReport {
    pub batch_id: Option<String>,
    pub moved: u64,
    pub skipped: u64,
    pub failed: Vec<(String, String)>,
    pub cancelled: bool,
    pub items: Vec<OrganizeItem>,
    /// Documents whose path changed.
    pub documents: Vec<String>,
}

fn move_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    if to.exists() {
        return Err(CoreError::Invalid(format!("target exists: {}", to.display())));
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|e| CoreError::io(from, e))?;
    std::fs::remove_file(from).map_err(|e| CoreError::io(from, e))?;
    Ok(())
}

/// Replays a plan: every `Move` item is renamed on disk, its path updated in
/// the database and journaled. `progress(done, total, name)`.
#[allow(clippy::too_many_arguments)]
pub fn apply(
    db: &Db,
    paths: &WorkspacePaths,
    plan: &OrganizePlan,
    kind: &str,
    label: &str,
    source: &str,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<ApplyReport> {
    let total = plan.items.iter().filter(|i| i.status == ItemStatus::Move).count() as u64;
    let mut report = ApplyReport { items: plan.items.clone(), ..ApplyReport::default() };
    if total == 0 {
        return Ok(report);
    }
    let batch_id = crate::documents::new_id();
    let plan_json = serde_json::to_string(&serde_json::json!({
        "name_template": plan.name_template, "folder_template": plan.folder_template, "only_reviewed": plan.only_reviewed, "moves": plan.moves
    }))
    .ok();
    db.with(|c| journal::begin_batch(c, &batch_id, kind, Some(label), Some(source), plan_json.as_deref()))?;
    report.batch_id = Some(batch_id.clone());
    let mut done = 0u64;
    let mut seq = 0u32;
    for item in report.items.iter_mut() {
        if item.status != ItemStatus::Move {
            continue;
        }
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            report.cancelled = true;
            break;
        }
        let Some(to) = item.to.clone() else { continue };
        progress(done, total, &item.name);
        let from_abs = from_rel_path(&paths.root, &item.from);
        let to_abs = from_rel_path(&paths.root, &to);
        // The document's current path must still be what the plan saw (a rescan may have moved it).
        let doc = db.with(|c| docs::get_document(c, &item.doc_id))?;
        let Some(doc) = doc else {
            item.status = ItemStatus::Failed;
            item.reason = Some("document_missing".into());
            report.failed.push((item.doc_id.clone(), "document_missing".into()));
            continue;
        };
        if doc.rel_path != item.from || !from_abs.is_file() {
            item.status = ItemStatus::Failed;
            item.reason = Some("source_changed".into());
            report.failed.push((item.doc_id.clone(), "source_changed".into()));
            continue;
        }
        match move_file(&from_abs, &to_abs) {
            Ok(()) => {
                let new_name = to.rsplit('/').next().unwrap_or(&to).to_string();
                let entry = NewEntry {
                    kind: if doc.rel_dir() == to.rsplit_once('/').map(|(d, _)| d).unwrap_or("") {
                        EntryKind::Rename
                    } else {
                        EntryKind::Move
                    },
                    document_id: Some(doc.id.clone()),
                    from_path: item.from.clone(),
                    to_path: Some(to.clone()),
                    trash_path: None,
                    content_hash: Some(doc.content_hash.clone()),
                    size_bytes: Some(doc.size_bytes),
                };
                // The file moved; if the bookkeeping fails, move it back so disk and DB agree.
                match db.with(|c| {
                    docs::set_rel_path(c, &doc.id, &to, &new_name)?;
                    journal::append(c, paths, &batch_id, seq, &entry)
                }) {
                    Ok(_) => {
                        seq += 1;
                        item.status = ItemStatus::Done;
                        report.moved += 1;
                        report.documents.push(doc.id.clone());
                        // The copies in other project folders follow the renamed / moved original.
                        if let Err(e) = crate::projects::copies::sync_document(db, paths, &doc.id, false) {
                            log::warn!("copy sync after organise failed for {}: {e}", doc.id);
                        }
                    }
                    Err(e) => {
                        let _ = move_file(&to_abs, &from_abs);
                        item.status = ItemStatus::Failed;
                        item.reason = Some(e.to_string());
                        report.failed.push((item.doc_id.clone(), e.to_string()));
                    }
                }
            }
            Err(e) => {
                item.status = ItemStatus::Failed;
                item.reason = Some(e.to_string());
                report.failed.push((item.doc_id.clone(), e.to_string()));
            }
        }
        done += 1;
    }
    report.skipped = plan.items.len() as u64 - total;
    if report.moved == 0 {
        // Nothing happened: drop the empty batch so undo does not offer it.
        let _ = db.with(|c| c.execute("DELETE FROM journal_batches WHERE id = ?1 AND entries = 0", [&batch_id]));
        if report.batch_id.as_deref() == Some(&batch_id) {
            let still = db.with(|c| journal::get_batch(c, &batch_id))?;
            if still.is_none() {
                report.batch_id = None;
            }
        }
    }
    Ok(report)
}

/// Convenience: plan + apply in one call (library / viewer actions).
#[allow(clippy::too_many_arguments)]
pub fn organize(
    db: &Db,
    paths: &WorkspacePaths,
    req: &OrganizeRequest,
    kind: &str,
    label: &str,
    source: &str,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<(OrganizePlan, ApplyReport)> {
    let p = plan(db, paths, req)?;
    let r = apply(db, paths, &p, kind, label, source, cancel, progress)?;
    Ok((p, r))
}

/// Deletes documents to the recycle bin (workspace trash fallback) and journals
/// each deletion so the workspace-trash case can be restored.
pub fn delete_documents(
    deps: &crate::documents::import::ImportDeps<'_>,
    ids: &[String],
    label: &str,
    source: &str,
    cancel: Option<&AtomicBool>,
) -> Result<ApplyReport> {
    let mut report = ApplyReport::default();
    if ids.is_empty() {
        return Ok(report);
    }
    let batch_id = crate::documents::new_id();
    deps.db.with(|c| journal::begin_batch(c, &batch_id, "delete", Some(label), Some(source), None))?;
    report.batch_id = Some(batch_id.clone());
    let mut seq = 0u32;
    for id in ids {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            report.cancelled = true;
            break;
        }
        let Some(doc) = deps.db.with(|c| docs::get_document(c, id))? else {
            report.failed.push((id.clone(), "document_missing".into()));
            continue;
        };
        match crate::documents::import::delete_document(deps, id, false) {
            Ok(moved_to) => {
                let trash_rel = moved_to.as_deref().and_then(|p| crate::documents::to_rel_path(&deps.paths.root, p));
                let entry = NewEntry {
                    kind: EntryKind::Delete,
                    document_id: Some(doc.id.clone()),
                    from_path: doc.rel_path.clone(),
                    to_path: None,
                    trash_path: trash_rel,
                    content_hash: Some(doc.content_hash.clone()),
                    size_bytes: Some(doc.size_bytes),
                };
                deps.db.with(|c| journal::append(c, deps.paths, &batch_id, seq, &entry))?;
                seq += 1;
                report.moved += 1;
                report.documents.push(doc.id.clone());
                report.items.push(OrganizeItem {
                    doc_id: doc.id.clone(),
                    name: doc.original_name.clone(),
                    from: doc.rel_path.clone(),
                    to: None,
                    status: ItemStatus::Done,
                    reason: None,
                    missing: Vec::new(),
                });
            }
            Err(e) => report.failed.push((id.clone(), e.to_string())),
        }
    }
    if report.moved == 0 {
        let _ = deps.db.with(|c| c.execute("DELETE FROM journal_batches WHERE id = ?1 AND entries = 0", [&batch_id]));
        report.batch_id = None;
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
