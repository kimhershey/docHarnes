//! Import pipeline (design §3.4 "가져오기"): plan (supported / duplicate /
//! unsupported with reasons) → run (copy into the workspace, register,
//! convert, render) → rescan (missing / changed files, interrupted imports)
//! and delete (OS trash with a workspace fallback).

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::{CoreError, Result};
use crate::paths::WorkspacePaths;
use crate::version::WORKSPACE_DIR_NAME;

use super::convert::{self, PdfPrinter};
use super::render::{self, RenderOptions, RenderService};
use super::{
    from_rel_path, new_id, nfc, repo, to_rel_path, DocumentCache, DocumentRecord, DocumentRole, DocumentStatus,
    NewDocument, NewPage, SourceKind,
};

/// Files modified more recently than this are treated as still being copied
/// (inbox watch only).
pub const INBOX_SETTLE_SECONDS: u64 = 3;
/// Largest source file accepted.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Upper bound on files collected from dropped folders in one plan.
pub const MAX_PLAN_FILES: usize = 5_000;
const WALK_MAX_DEPTH: u32 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImportSource {
    #[default]
    Drop,
    Folder,
    Inbox,
    Onboarding,
    Rescan,
}

impl ImportSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ImportSource::Drop => "drop",
            ImportSource::Folder => "folder",
            ImportSource::Inbox => "inbox",
            ImportSource::Onboarding => "onboarding",
            ImportSource::Rescan => "rescan",
        }
    }
}

/// Where files from outside the workspace are copied to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImportDestination {
    #[default]
    Documents,
    Inbox,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportOptions {
    pub destination: ImportDestination,
    /// Move external files instead of copying them.
    pub move_files: bool,
    /// Render pages after registering (off = register + convert text only).
    pub render: bool,
    pub role: DocumentRole,
    /// Override of the settings render DPI.
    pub dpi: Option<u16>,
    /// Knowledge-base collection that receives every document of this batch
    /// (new, relinked, already registered and duplicates alike).
    #[serde(default)]
    pub collection_id: Option<String>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            destination: ImportDestination::Documents,
            move_files: false,
            render: true,
            role: DocumentRole::Work,
            dpi: None,
            collection_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportItemStatus {
    New,
    /// Same content as a registered document.
    Duplicate {
        of: String,
        of_rel_path: String,
    },
    /// Same content as an earlier file in this plan.
    DuplicateInBatch {
        of: String,
    },
    /// This exact path is already registered.
    AlreadyRegistered {
        id: String,
    },
    /// Content matches a document whose file went missing → reconnect.
    Relink {
        id: String,
        old_rel_path: String,
    },
    Unsupported {
        reason: String,
    },
    /// Still being written (inbox).
    Copying,
}

impl ImportItemStatus {
    pub fn is_importable(&self) -> bool {
        matches!(self, ImportItemStatus::New | ImportItemStatus::Relink { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportItem {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub kind: Option<SourceKind>,
    pub inside_workspace: bool,
    /// Target workspace-relative path (existing for in-place files).
    pub rel_path: Option<String>,
    pub content_hash: Option<String>,
    pub status: ImportItemStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ImportPlan {
    pub source: ImportSource,
    pub options: ImportOptions,
    pub items: Vec<ImportItem>,
    pub new_count: u32,
    pub relink_count: u32,
    pub duplicate_count: u32,
    pub registered_count: u32,
    pub unsupported_count: u32,
    pub copying_count: u32,
    pub new_bytes: u64,
    /// Folder walk stopped at `MAX_PLAN_FILES`.
    pub truncated: bool,
}

impl ImportPlan {
    pub fn importable(&self) -> impl Iterator<Item = &ImportItem> {
        self.items.iter().filter(|i| i.status.is_importable())
    }
}

/// Walks `sources` (files or folders) collecting candidate files; hidden
/// entries, `.doculens/` and symlinks are skipped. Returns `(files, truncated)`.
pub fn collect_paths(sources: &[PathBuf], max_files: usize) -> (Vec<PathBuf>, bool) {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut truncated = false;
    for src in sources {
        let is_link = std::fs::symlink_metadata(src).map(|m| m.file_type().is_symlink()).unwrap_or(false);
        if is_link {
            continue; // same policy as nested entries: links are never followed
        }
        if src.is_file() {
            if seen.insert(src.clone()) {
                out.push(src.clone());
            }
            continue;
        }
        if !src.is_dir() {
            out.push(src.clone()); // reported as unreadable by the plan
            continue;
        }
        let mut stack = vec![(src.clone(), 0u32)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            let mut entries: Vec<_> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || name == WORKSPACE_DIR_NAME {
                    continue;
                }
                let Ok(ft) = entry.file_type() else { continue };
                if ft.is_symlink() {
                    continue;
                }
                let path = entry.path();
                if ft.is_dir() {
                    if depth < WALK_MAX_DEPTH {
                        stack.push((path, depth + 1));
                    }
                } else if ft.is_file() {
                    if out.len() >= max_files {
                        truncated = true;
                        return (out, truncated);
                    }
                    if seen.insert(path.clone()) {
                        out.push(path);
                    }
                }
            }
        }
    }
    (out, truncated)
}

/// BLAKE3 of a whole file.
pub fn blake3_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|e| CoreError::io(path, e))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(|e| CoreError::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn modified_at(meta: &std::fs::Metadata) -> Option<DateTime<Utc>> {
    meta.modified().ok().map(DateTime::<Utc>::from)
}

fn recently_modified(meta: &std::fs::Metadata, within: Duration) -> bool {
    meta.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok()).is_some_and(|age| age < within)
}

/// Reason a file cannot be imported (`None` = supported).
pub fn unsupported_reason(path: &Path, meta: &std::fs::Metadata) -> Option<String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
    if SourceKind::from_path(path).is_none() {
        return Some(match ext.as_str() {
            "heic" | "heif" => "heic".to_string(),
            "doc" | "xls" | "ppt" | "pptx" | "xlsx" | "hwpml" | "hml" => "office_legacy".to_string(),
            "txt" | "md" | "csv" | "json" | "xml" | "html" | "htm" => "text".to_string(),
            "" => "no_extension".to_string(),
            _ => "unsupported_extension".to_string(),
        });
    }
    if meta.len() == 0 {
        return Some("empty".to_string());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Some("too_large".to_string());
    }
    None
}

/// Builds the import preview. Hashes every supported file (BLAKE3, ~1 GB/s)
/// so duplicates against the workspace and within the batch are exact.
pub fn plan(
    db: &Db,
    paths: &WorkspacePaths,
    sources: &[PathBuf],
    options: &ImportOptions,
    source: ImportSource,
) -> Result<ImportPlan> {
    let (files, truncated) = collect_paths(sources, MAX_PLAN_FILES);
    let root = std::fs::canonicalize(&paths.root).unwrap_or_else(|_| paths.root.clone());
    let mut plan = ImportPlan { source, options: options.clone(), truncated, ..Default::default() };
    let mut seen_hashes: HashMap<String, String> = HashMap::new();
    for path in files {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                plan.items.push(ImportItem {
                    path,
                    name,
                    size: 0,
                    kind: None,
                    inside_workspace: false,
                    rel_path: None,
                    content_hash: None,
                    status: ImportItemStatus::Unsupported { reason: "unreadable".into() },
                });
                continue;
            }
        };
        let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let rel_existing = to_rel_path(&root, &canon);
        let inside = rel_existing.is_some();
        let kind = SourceKind::from_path(&path);
        let mut item = ImportItem {
            path: path.clone(),
            name: name.clone(),
            size: meta.len(),
            kind,
            inside_workspace: inside,
            rel_path: rel_existing.clone(),
            content_hash: None,
            status: ImportItemStatus::New,
        };
        if let Some(reason) = unsupported_reason(&path, &meta) {
            item.status = ImportItemStatus::Unsupported { reason };
            plan.items.push(item);
            continue;
        }
        if source == ImportSource::Inbox && recently_modified(&meta, Duration::from_secs(INBOX_SETTLE_SECONDS)) {
            item.status = ImportItemStatus::Copying;
            plan.items.push(item);
            continue;
        }
        if let Some(rel) = &rel_existing {
            if let Some(existing) = db.with(|c| repo::find_by_rel_path(c, rel))? {
                if existing.status != DocumentStatus::Missing {
                    item.status = ImportItemStatus::AlreadyRegistered { id: existing.id };
                    plan.items.push(item);
                    continue;
                }
            }
        }
        let hash = match blake3_file(&path) {
            Ok(h) => h,
            Err(e) => {
                item.status = ImportItemStatus::Unsupported { reason: format!("unreadable: {e}") };
                plan.items.push(item);
                continue;
            }
        };
        item.content_hash = Some(hash.clone());
        if let Some(first) = seen_hashes.get(&hash) {
            item.status = ImportItemStatus::DuplicateInBatch { of: first.clone() };
            plan.items.push(item);
            continue;
        }
        let same = db.with(|c| repo::find_by_hash(c, &hash))?;
        let live = same.iter().find(|d| d.status != DocumentStatus::Missing && d.deleted_at.is_none());
        let missing = same.iter().find(|d| d.status == DocumentStatus::Missing);
        if let Some(d) = live {
            // The registered copy may itself be this very file under another path spelling.
            item.status = ImportItemStatus::Duplicate { of: d.id.clone(), of_rel_path: d.rel_path.clone() };
        } else if let Some(d) = missing {
            item.status = ImportItemStatus::Relink { id: d.id.clone(), old_rel_path: d.rel_path.clone() };
        }
        seen_hashes.insert(hash, name);
        plan.items.push(item);
    }
    for item in &plan.items {
        match &item.status {
            ImportItemStatus::New => {
                plan.new_count += 1;
                plan.new_bytes += item.size;
            }
            ImportItemStatus::Relink { .. } => plan.relink_count += 1,
            ImportItemStatus::Duplicate { .. } | ImportItemStatus::DuplicateInBatch { .. } => plan.duplicate_count += 1,
            ImportItemStatus::AlreadyRegistered { .. } => plan.registered_count += 1,
            ImportItemStatus::Unsupported { .. } => plan.unsupported_count += 1,
            ImportItemStatus::Copying => plan.copying_count += 1,
        }
    }
    Ok(plan)
}

/// Everything `run` needs besides the plan.
pub struct ImportDeps<'a> {
    pub db: &'a Db,
    pub paths: &'a WorkspacePaths,
    /// `None` = PDFium unavailable (documents register without page images).
    pub render: Option<&'a RenderService>,
    pub printer: &'a dyn PdfPrinter,
    pub render_options: RenderOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ImportProgress {
    pub done: u32,
    pub total: u32,
    pub current: String,
    /// Page progress of the current document when known.
    pub page: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ImportReport {
    pub batch_id: String,
    pub imported: Vec<String>,
    pub relinked: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub skipped: u32,
    pub cancelled: bool,
}

/// Picks a free file name inside `dir` for `name` (`name_2.ext`, …). `taken`
/// lets callers also exclude names that are registered in the database.
pub fn unique_target(dir: &Path, name: &str, taken: &dyn Fn(&Path) -> bool) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() && !taken(&candidate) {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for i in 2..10_000 {
        let c = dir.join(format!("{stem}_{i}{ext}"));
        if !c.exists() && !taken(&c) {
            return c;
        }
    }
    dir.join(format!("{stem}_{}{ext}", new_id()))
}

fn copy_or_move(src: &Path, dst: &Path, move_files: bool) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    if move_files && std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    let tmp = dst.with_extension(format!("{}.part", dst.extension().and_then(|e| e.to_str()).unwrap_or("bin")));
    std::fs::copy(src, &tmp).map_err(|e| CoreError::io(src, e))?;
    std::fs::rename(&tmp, dst).map_err(|e| CoreError::io(dst, e))?;
    if move_files {
        let _ = std::fs::remove_file(src);
    }
    Ok(())
}

/// Executes a plan. `progress` is called before each document and between
/// render chunks; `on_document(id, ok)` after each registered document (the
/// shell emits `documents://changed`).
pub fn run(
    deps: &ImportDeps<'_>,
    plan: &ImportPlan,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(&ImportProgress),
    on_document: &mut dyn FnMut(&str, bool),
) -> Result<ImportReport> {
    let total = plan.importable().count() as u32;
    let batch_id = new_id();
    deps.db.with(|c| repo::insert_batch(c, &batch_id, plan.source.as_str(), total))?;
    let mut report = ImportReport { batch_id: batch_id.clone(), ..Default::default() };
    let mut prog = ImportProgress { done: 0, total, ..Default::default() };
    let dest_dir = match plan.options.destination {
        ImportDestination::Documents => deps.paths.documents_dir(),
        ImportDestination::Inbox => deps.paths.inbox_dir(),
    };
    // The target collection must exist (a deleted one would fail every registration on the FK).
    if let Some(cid) = plan.options.collection_id.as_deref() {
        if !deps.db.with(|c| crate::knowledge::repo::collection_exists(c, cid))? {
            return Err(CoreError::Invalid("collection_not_found".into()));
        }
    }
    // Documents this batch merely re-encountered still join the collection.
    if plan.options.collection_id.is_some() {
        for item in &plan.items {
            let existing = match &item.status {
                ImportItemStatus::AlreadyRegistered { id } => Some(id.clone()),
                ImportItemStatus::Duplicate { of, .. } => Some(of.clone()),
                _ => None,
            };
            if let Some(id) = existing {
                deps.db.with(|c| attach_to_collection(c, &plan.options, &id))?;
            }
        }
    }
    for item in plan.importable() {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            report.cancelled = true;
            break;
        }
        prog.current = item.name.clone();
        prog.page = None;
        progress(&prog);
        match import_one(deps, plan, item, &dest_dir, &batch_id, cancel, &mut |p| {
            prog.page = Some(p);
            progress(&prog);
        }) {
            Ok((id, relinked)) => {
                if relinked {
                    report.relinked.push(id.clone());
                } else {
                    report.imported.push(id.clone());
                }
                on_document(&id, true);
            }
            Err(CoreError::Cancelled) => {
                report.cancelled = true;
                break;
            }
            Err(e) => {
                // Names/paths stay out of the log (design §3.8); the UI payload carries them.
                log::debug!("import item failed: {e}");
                report.failed.push((item.name.clone(), e.to_string()));
            }
        }
        prog.done += 1;
        progress(&prog);
    }
    report.skipped = plan.duplicate_count + plan.registered_count + plan.unsupported_count + plan.copying_count;
    if !report.failed.is_empty() {
        log::warn!("import batch {}: {} of {} items failed", batch_id, report.failed.len(), total);
    }
    deps.db.with(|c| {
        repo::finish_batch(
            c,
            &batch_id,
            (report.imported.len() + report.relinked.len()) as u32,
            plan.duplicate_count,
            plan.unsupported_count,
            report.failed.len() as u32,
        )
    })?;
    Ok(report)
}

/// Registers (or relinks) one item and processes it. Returns `(id, relinked)`.
fn import_one(
    deps: &ImportDeps<'_>,
    plan: &ImportPlan,
    item: &ImportItem,
    dest_dir: &Path,
    batch_id: &str,
    cancel: Option<&AtomicBool>,
    page_progress: &mut dyn FnMut((u32, u32)),
) -> Result<(String, bool)> {
    let kind = item.kind.ok_or_else(|| CoreError::Unsupported("unsupported_extension".into()))?;
    let display_name = nfc(&item.name);
    // 1. Place the file inside the workspace. Copies get an NFC name (the app's
    //    own spelling); in-place files keep whatever the file system has.
    let mut copied: Option<PathBuf> = None;
    let (abs, rel) = if item.inside_workspace {
        let rel = item.rel_path.clone().ok_or_else(|| CoreError::Invalid("missing rel_path".into()))?;
        (from_rel_path(&deps.paths.root, &rel), rel)
    } else {
        let root = deps.paths.root.clone();
        let db = deps.db;
        let taken = |p: &Path| -> bool {
            to_rel_path(&root, p)
                .is_some_and(|rel| db.with(|c| repo::find_by_rel_path(c, &rel)).ok().flatten().is_some())
        };
        let target = unique_target(dest_dir, &display_name, &taken);
        copy_or_move(&item.path, &target, plan.options.move_files)?;
        copied = Some(target.clone());
        let rel = to_rel_path(&deps.paths.root, &target)
            .ok_or_else(|| CoreError::Invalid(format!("{} is outside the workspace", target.display())))?;
        (target, rel)
    };
    let (id, relinked) = match register(deps, plan, item, kind, &display_name, &abs, &rel, batch_id) {
        Ok(v) => v,
        Err(e) => {
            // Never leave an unregistered copy behind (with `move_files` the
            // source is already gone, so that file stays where it landed).
            if let Some(p) = copied.filter(|_| !plan.options.move_files) {
                let _ = std::fs::remove_file(p);
            }
            return Err(e);
        }
    };
    // 2. Convert + render (a relinked document with cached pages is kept as is).
    let doc = deps.db.with(|c| repo::get_document(c, &id))?.ok_or_else(|| CoreError::NotFound(id.clone()))?;
    let keep_pages =
        relinked && doc.page_count > 0 && DocumentCache::for_document(deps.paths, &id).page_file(1).is_file();
    if keep_pages {
        deps.db.with(|c| repo::set_status(c, &id, DocumentStatus::Rendered, None))?;
    } else {
        process_document(deps, &doc, plan.options.render, plan.options.dpi, cancel, page_progress)?;
    }
    Ok((id, relinked))
}

/// Inserts the row, relinks a `missing` document, or — when the path is
/// already registered (e.g. a `missing` row at the same path whose content
/// changed) — refreshes that row instead of hitting the UNIQUE constraint.
#[allow(clippy::too_many_arguments)]
fn register(
    deps: &ImportDeps<'_>,
    plan: &ImportPlan,
    item: &ImportItem,
    kind: SourceKind,
    display_name: &str,
    abs: &Path,
    rel: &str,
    batch_id: &str,
) -> Result<(String, bool)> {
    let meta = std::fs::metadata(abs).map_err(|e| CoreError::io(abs, e))?;
    let hash = match &item.content_hash {
        Some(h) => h.clone(),
        None => blake3_file(abs)?,
    };
    let modified = modified_at(&meta);
    let size = meta.len();
    let existing_id = match &item.status {
        ImportItemStatus::Relink { id, .. } => Some(id.clone()),
        _ => deps.db.with(|c| repo::find_by_rel_path(c, rel))?.map(|d| d.id),
    };
    if let Some(id) = existing_id {
        deps.db.with_tx(|tx| {
            repo::set_rel_path(tx, &id, rel, display_name)?;
            repo::set_file_info(tx, &id, &hash, size, modified)?;
            repo::set_status(tx, &id, DocumentStatus::Importing, None)?;
            attach_to_collection(tx, &plan.options, &id)
        })?;
        return Ok((id, true));
    }
    let id = new_id();
    deps.db.with(|c| {
        repo::insert_document(
            c,
            &NewDocument {
                id: id.clone(),
                rel_path: rel.to_string(),
                original_name: display_name.to_string(),
                source_kind: kind,
                content_hash: hash.clone(),
                size_bytes: size,
                file_modified_at: modified,
                role: plan.options.role,
                import_batch_id: Some(batch_id.to_string()),
            },
        )?;
        attach_to_collection(c, &plan.options, &id)
    })?;
    Ok((id, false))
}

/// Adds a document to the batch's knowledge collection (and gives it the
/// batch role when that is not the default) — no-op without `collection_id`.
fn attach_to_collection(conn: &rusqlite::Connection, options: &ImportOptions, id: &str) -> rusqlite::Result<()> {
    let Some(cid) = options.collection_id.as_deref() else { return Ok(()) };
    crate::knowledge::repo::add_docs_to_collection(conn, cid, std::slice::from_ref(&id.to_string()))?;
    if options.role != DocumentRole::Work {
        repo::set_role(conn, id, options.role)?;
    }
    Ok(())
}

/// Converts (office formats) and renders a registered document, replacing its
/// pages and setting the final status. Errors are recorded on the document
/// (status `error`) and returned.
pub fn process_document(
    deps: &ImportDeps<'_>,
    doc: &DocumentRecord,
    render_pages: bool,
    dpi: Option<u16>,
    cancel: Option<&AtomicBool>,
    page_progress: &mut dyn FnMut((u32, u32)),
) -> Result<()> {
    // Crash safety: if the process dies inside a parser/renderer (abort, OOM), the row
    // must not stay `importing` (which the startup rescan would retry forever).
    deps.db.with(|c| repo::set_status(c, &doc.id, DocumentStatus::Error, Some("interrupted while processing")))?;
    let result = process_inner(deps, doc, render_pages, dpi, cancel, page_progress);
    match &result {
        Ok(status) => {
            deps.db.with(|c| repo::set_status(c, &doc.id, *status, None))?;
        }
        Err(CoreError::Cancelled) => {
            deps.db.with(|c| repo::set_status(c, &doc.id, DocumentStatus::New, None))?;
        }
        Err(e) => {
            let msg = e.to_string();
            deps.db.with(|c| repo::set_status(c, &doc.id, DocumentStatus::Error, Some(&msg)))?;
        }
    }
    result.map(|_| ())
}

fn process_inner(
    deps: &ImportDeps<'_>,
    doc: &DocumentRecord,
    render_pages: bool,
    dpi: Option<u16>,
    cancel: Option<&AtomicBool>,
    page_progress: &mut dyn FnMut((u32, u32)),
) -> Result<DocumentStatus> {
    let abs = doc.absolute_path(deps.paths);
    if !abs.is_file() {
        return Err(CoreError::NotFound(format!("{} (file missing)", doc.rel_path)));
    }
    let cache = DocumentCache::for_document(deps.paths, &doc.id);
    let mut opts = deps.render_options.clone();
    if let Some(d) = dpi {
        opts.dpi = d.clamp(72, 600);
    }
    let rotations: HashMap<u32, u16> = deps
        .db
        .with(|c| repo::list_pages(c, &doc.id, false))?
        .into_iter()
        .filter(|p| p.rotation != 0)
        .map(|p| (p.page_no, p.rotation))
        .collect();
    let mut converted = false;
    let mut note: Option<String> = None;
    let mut fallback_text: Option<String> = None;
    let render_src: Option<PathBuf> = match doc.source_kind {
        SourceKind::Pdf => Some(abs.clone()),
        SourceKind::Image => None,
        SourceKind::Hwpx | SourceKind::Docx | SourceKind::Hwp => {
            let conv = convert::convert(&abs, doc.source_kind, &cache.converted_dir, deps.printer)?;
            converted = true;
            let mut notes = conv.model.notes.clone();
            if let Some(e) = &conv.print_error {
                notes.push(format!("PDF 변환본 생성 실패({}): {e}", conv.printer));
            }
            if !notes.is_empty() {
                note = Some(notes.join("\n"));
            }
            let text = conv.model.text();
            if !text.trim().is_empty() {
                fallback_text = Some(text);
            }
            conv.pdf.clone()
        }
    };
    if !render_pages {
        // Register only: keep whatever pages exist, store converted text for search.
        if let Some(text) = fallback_text {
            let page = NewPage { page_no: 1, text: Some(text), ..Default::default() };
            deps.db.with_tx(|tx| repo::replace_pages(tx, &doc.id, &[page], None, converted, note.as_deref()))?;
        }
        return Ok(DocumentStatus::New);
    }
    let pages: Vec<NewPage> = match (doc.source_kind, render_src, deps.render) {
        (SourceKind::Image, _, _) => {
            let rotation = rotations.get(&1).copied().unwrap_or(0);
            render::render_image(&abs, &cache, &opts, rotation)?
        }
        (_, Some(pdf), Some(svc)) => {
            let mut pages =
                render::render_pdf(svc, &pdf, &cache, &opts, &rotations, cancel, &mut |d, t| page_progress((d, t)))?;
            // A printed PDF carries its own text layer; when it has none (stub
            // printer, image-only export) keep the extracted text on page 1.
            if pages.iter().all(|p| p.text.is_none()) {
                if let (Some(text), Some(first)) = (fallback_text.take(), pages.first_mut()) {
                    first.text = Some(text);
                }
            }
            pages
        }
        (_, Some(_), None) => {
            note = Some(match note {
                Some(n) => format!("{n}\nPDF 렌더러(PDFium)를 찾을 수 없습니다"),
                None => "PDF 렌더러(PDFium)를 찾을 수 없습니다".to_string(),
            });
            text_only_pages(fallback_text)
        }
        (_, None, _) => text_only_pages(fallback_text),
    };
    let rendered = pages.iter().any(|p| p.width_px > 0);
    render::prune_cache(&cache, pages.len() as u32);
    deps.db.with_tx(|tx| repo::replace_pages(tx, &doc.id, &pages, Some(opts.dpi), converted, note.as_deref()))?;
    Ok(if rendered { DocumentStatus::Rendered } else { DocumentStatus::New })
}

fn text_only_pages(text: Option<String>) -> Vec<NewPage> {
    match text {
        Some(t) => vec![NewPage { page_no: 1, text: Some(t), ..Default::default() }],
        None => Vec::new(),
    }
}

/// Re-renders one page with a new rotation (viewer "회전 저장").
pub fn rotate_page(deps: &ImportDeps<'_>, doc: &DocumentRecord, page_no: u32, rotation: u16) -> Result<NewPage> {
    let rotation = rotation % 360;
    if rotation % 90 != 0 {
        return Err(CoreError::Invalid("rotation must be a multiple of 90".into()));
    }
    let abs = doc.absolute_path(deps.paths);
    let cache = DocumentCache::for_document(deps.paths, &doc.id);
    let opts = RenderOptions { dpi: doc.render_dpi.unwrap_or(deps.render_options.dpi), ..deps.render_options.clone() };
    let page = match doc.source_kind {
        SourceKind::Image => render::render_image(&abs, &cache, &opts, rotation)?
            .pop()
            .ok_or_else(|| CoreError::Render("no page".into()))?,
        _ => {
            let pdf = if doc.source_kind == SourceKind::Pdf { abs } else { cache.converted_pdf() };
            let svc = deps.render.ok_or_else(|| CoreError::Render("PDFium is not available".into()))?;
            render::render_pdf_page(svc, &pdf, &cache, &opts, page_no, rotation)?
        }
    };
    deps.db.with(|c| {
        repo::set_page_rotation(c, &doc.id, page_no, rotation)?;
        repo::update_page_render(c, &doc.id, page_no, page.width_px, page.height_px, page.dpi)
    })?;
    Ok(page)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RescanReport {
    pub checked: u32,
    pub missing: Vec<String>,
    pub restored: Vec<String>,
    pub changed: Vec<String>,
    pub reprocessed: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Reconciles registered documents with the file system: missing files,
/// files that came back, changed content (re-render) and imports that were
/// interrupted (`importing`).
pub fn rescan(
    deps: &ImportDeps<'_>,
    cancel: Option<&AtomicBool>,
    on_document: &mut dyn FnMut(&str),
) -> Result<RescanReport> {
    let docs = deps.db.with(repo::all_documents)?;
    let mut report = RescanReport::default();
    for doc in docs {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            break;
        }
        report.checked += 1;
        // One unreadable / locked file must not abort the whole pass.
        if let Err(e) = rescan_one(deps, &doc, cancel, &mut report, on_document) {
            log::debug!("rescan: document {} failed: {e}", doc.id);
            report.failed.push((doc.id.clone(), e.to_string()));
        }
    }
    Ok(report)
}

fn rescan_one(
    deps: &ImportDeps<'_>,
    doc: &DocumentRecord,
    cancel: Option<&AtomicBool>,
    report: &mut RescanReport,
    on_document: &mut dyn FnMut(&str),
) -> Result<()> {
    {
        let abs = doc.absolute_path(deps.paths);
        let meta = match std::fs::metadata(&abs) {
            Ok(m) if m.is_file() => m,
            _ => {
                if doc.status != DocumentStatus::Missing {
                    deps.db.with(|c| repo::set_status(c, &doc.id, DocumentStatus::Missing, None))?;
                    report.missing.push(doc.id.clone());
                    on_document(&doc.id);
                }
                return Ok(());
            }
        };
        let size_changed = meta.len() != doc.size_bytes;
        let mtime_changed =
            modified_at(&meta).zip(doc.file_modified_at).is_some_and(|(a, b)| (a - b).num_seconds().abs() > 1);
        let mut needs_process = doc.status == DocumentStatus::Importing;
        if doc.status == DocumentStatus::Missing || size_changed || mtime_changed {
            let hash = blake3_file(&abs)?;
            if hash != doc.content_hash {
                deps.db.with(|c| repo::set_file_info(c, &doc.id, &hash, meta.len(), modified_at(&meta)))?;
                report.changed.push(doc.id.clone());
                needs_process = true;
            } else if doc.status == DocumentStatus::Missing {
                let cache = DocumentCache::for_document(deps.paths, &doc.id);
                let has_pages = doc.page_count > 0 && cache.page_file(1).is_file();
                deps.db.with(|c| {
                    repo::set_file_info(c, &doc.id, &hash, meta.len(), modified_at(&meta))?;
                    repo::set_status(
                        c,
                        &doc.id,
                        if has_pages { DocumentStatus::Rendered } else { DocumentStatus::New },
                        None,
                    )
                })?;
                report.restored.push(doc.id.clone());
                needs_process = !has_pages;
                on_document(&doc.id);
            } else {
                deps.db.with(|c| repo::set_file_info(c, &doc.id, &hash, meta.len(), modified_at(&meta)))?;
            }
        }
        if needs_process {
            let fresh = deps.db.with(|c| repo::get_document(c, &doc.id))?.unwrap_or_else(|| doc.clone());
            match process_document(deps, &fresh, true, doc.render_dpi, cancel, &mut |_| {}) {
                Ok(()) => report.reprocessed.push(doc.id.clone()),
                Err(e) => report.failed.push((doc.id.clone(), e.to_string())),
            }
            on_document(&doc.id);
        }
    }
    Ok(())
}

/// Files in `inbox/` that are not registered yet (settled only).
pub fn inbox_plan(db: &Db, paths: &WorkspacePaths, options: &ImportOptions) -> Result<ImportPlan> {
    let inbox = paths.inbox_dir();
    if !inbox.is_dir() {
        return Ok(ImportPlan { source: ImportSource::Inbox, options: options.clone(), ..Default::default() });
    }
    let (files, truncated) = collect_paths(std::slice::from_ref(&inbox), MAX_PLAN_FILES);
    // Skip paths that are registered already before hashing anything.
    let root = std::fs::canonicalize(&paths.root).unwrap_or_else(|_| paths.root.clone());
    let mut candidates = Vec::new();
    for f in files {
        let canon = std::fs::canonicalize(&f).unwrap_or_else(|_| f.clone());
        let registered = match to_rel_path(&root, &canon) {
            Some(rel) => {
                db.with(|c| repo::find_by_rel_path(c, &rel))?.is_some_and(|d| d.status != DocumentStatus::Missing)
            }
            None => false,
        };
        if !registered && SourceKind::from_path(&f).is_some() {
            candidates.push(f);
        }
    }
    let mut p = plan(db, paths, &candidates, options, ImportSource::Inbox)?;
    p.truncated |= truncated;
    Ok(p)
}

/// Deletes a document: DB row, cache, and (unless `keep_file`) the file —
/// OS trash first, `.doculens/trash/` as fallback (USB sticks, network shares).
pub fn delete_document(deps: &ImportDeps<'_>, id: &str, keep_file: bool) -> Result<Option<PathBuf>> {
    let doc = deps.db.with(|c| repo::get_document(c, id))?.ok_or_else(|| CoreError::NotFound(id.to_string()))?;
    let abs = doc.absolute_path(deps.paths);
    let mut moved_to = None;
    if !keep_file && abs.is_file() {
        match trash::delete(&abs) {
            Ok(()) => {}
            Err(e) => {
                log::info!("os trash unavailable ({e}); using the workspace trash folder");
                let trash_dir = deps.paths.trash_dir();
                std::fs::create_dir_all(&trash_dir).map_err(|e| CoreError::io(&trash_dir, e))?;
                let target = unique_target(
                    &trash_dir,
                    &format!("{}_{}", &doc.id[..8.min(doc.id.len())], doc.original_name),
                    &|_| false,
                );
                std::fs::rename(&abs, &target).or_else(|_| copy_or_move(&abs, &target, true))?;
                moved_to = Some(target);
            }
        }
    }
    DocumentCache::for_document(deps.paths, id).remove_all();
    deps.db.with(|c| repo::delete_document(c, id))?;
    Ok(moved_to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::JournalMode;
    use crate::documents::convert::hwpx::tests::TINY_PNG;
    use crate::documents::convert::{hwpx, write_blank_pdf, StubPdfPrinter};
    use crate::documents::DocumentFilter;
    use image::{Rgb, RgbImage};

    struct Env {
        _dir: tempfile::TempDir,
        db: Db,
        paths: WorkspacePaths,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let paths = WorkspacePaths::new(&root);
        for d in paths.all_dirs() {
            std::fs::create_dir_all(d).unwrap();
        }
        let db = Db::open(&paths.db_file(), JournalMode::Delete).unwrap();
        Env { _dir: dir, db, paths }
    }

    fn deps<'a>(e: &'a Env, render: Option<&'a RenderService>) -> ImportDeps<'a> {
        ImportDeps {
            db: &e.db,
            paths: &e.paths,
            render,
            printer: &StubPdfPrinter,
            render_options: RenderOptions { dpi: 72, ..Default::default() },
        }
    }

    fn pdfium() -> Option<&'static RenderService> {
        let lib = render::locate_pdfium(&render::default_pdfium_candidates())?;
        RenderService::global(&lib).ok()
    }

    fn external_dir(e: &Env) -> PathBuf {
        let d = e._dir.path().join("usb");
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn plan_classifies_files_and_duplicates() {
        let e = env();
        let ext = external_dir(&e);
        write_blank_pdf(&ext.join("a.pdf"), 2, &[]).unwrap();
        std::fs::copy(ext.join("a.pdf"), ext.join("a_copy.pdf")).unwrap();
        std::fs::write(ext.join("photo.heic"), b"x").unwrap();
        std::fs::write(ext.join("empty.pdf"), b"").unwrap();
        std::fs::write(ext.join("notes.txt"), b"x").unwrap();
        std::fs::create_dir_all(ext.join("sub").join(".hidden")).unwrap();
        RgbImage::from_pixel(8, 8, Rgb([0, 0, 0])).save(ext.join("sub").join("scan.png")).unwrap();
        std::fs::write(ext.join("sub").join(".hidden").join("x.pdf"), b"%PDF").unwrap();
        // One file already inside the workspace.
        write_blank_pdf(&e.paths.inbox_dir().join("in.pdf"), 1, &[]).unwrap();

        let plan = plan(
            &e.db,
            &e.paths,
            &[ext.clone(), e.paths.inbox_dir().join("in.pdf")],
            &ImportOptions::default(),
            ImportSource::Folder,
        )
        .unwrap();
        let by_name = |n: &str| plan.items.iter().find(|i| i.name == n).unwrap();
        assert_eq!(by_name("a.pdf").status, ImportItemStatus::New);
        assert!(matches!(by_name("a_copy.pdf").status, ImportItemStatus::DuplicateInBatch { .. }));
        assert_eq!(by_name("photo.heic").status, ImportItemStatus::Unsupported { reason: "heic".into() });
        assert_eq!(by_name("empty.pdf").status, ImportItemStatus::Unsupported { reason: "empty".into() });
        assert_eq!(by_name("notes.txt").status, ImportItemStatus::Unsupported { reason: "text".into() });
        assert_eq!(by_name("scan.png").status, ImportItemStatus::New);
        assert!(plan.items.iter().all(|i| i.name != "x.pdf"), "hidden folders are skipped");
        let inside = by_name("in.pdf");
        assert!(inside.inside_workspace);
        assert_eq!(inside.rel_path.as_deref(), Some("inbox/in.pdf"));
        assert_eq!(plan.new_count, 3);
        assert_eq!(plan.duplicate_count, 1);
        assert_eq!(plan.unsupported_count, 3);
    }

    #[test]
    fn run_registers_converts_and_renders_without_pdfium() {
        let e = env();
        let ext = external_dir(&e);
        write_blank_pdf(&ext.join("a.pdf"), 2, &[]).unwrap();
        hwpx::write_sample_hwpx(&ext.join("견적서.hwpx"), 2, TINY_PNG).unwrap();
        RgbImage::from_pixel(300, 200, Rgb([10, 10, 10])).save(ext.join("scan.jpg")).unwrap();
        let d = deps(&e, None);
        let plan =
            plan(&e.db, &e.paths, std::slice::from_ref(&ext), &ImportOptions::default(), ImportSource::Drop).unwrap();
        assert_eq!(plan.new_count, 3);
        let mut events = Vec::new();
        let report = run(&d, &plan, None, &mut |_| {}, &mut |id, ok| events.push((id.to_string(), ok))).unwrap();
        assert_eq!(report.imported.len(), 3, "{:?}", report.failed);
        assert_eq!(events.len(), 3);
        let docs = e.db.with(|c| repo::list_documents(c, &DocumentFilter::default())).unwrap();
        let by_name = |n: &str| docs.iter().find(|d| d.original_name == n).unwrap().clone();
        // Image: rendered through `image`, no PDFium needed.
        let scan = by_name("scan.jpg");
        assert_eq!(scan.status, DocumentStatus::Rendered);
        assert_eq!(scan.rel_path, "documents/scan.jpg");
        assert!(e.paths.documents_dir().join("scan.jpg").is_file(), "copied into documents/");
        assert!(DocumentCache::for_document(&e.paths, &scan.id).thumb_file(1).is_file());
        // HWPX: converted (stub printer PDF) but no renderer → text only, status new.
        let hwpx_doc = by_name("견적서.hwpx");
        assert!(hwpx_doc.converted);
        assert_eq!(hwpx_doc.status, DocumentStatus::New);
        assert!(hwpx_doc.conversion_note.as_deref().unwrap_or("").contains("PDFium"));
        let pages = e.db.with(|c| repo::list_pages(c, &hwpx_doc.id, true)).unwrap();
        assert_eq!(pages.len(), 1);
        assert!(pages[0].text.as_deref().unwrap().contains("견적서 1쪽"));
        assert!(DocumentCache::for_document(&e.paths, &hwpx_doc.id).converted_pdf().is_file());
        // PDF without PDFium: registered, no pages.
        let pdf = by_name("a.pdf");
        assert_eq!(pdf.status, DocumentStatus::New);
        assert_eq!(pdf.page_count, 0);
        // Second run: everything is a duplicate / already registered.
        let plan2 =
            super::plan(&e.db, &e.paths, std::slice::from_ref(&ext), &ImportOptions::default(), ImportSource::Drop)
                .unwrap();
        assert_eq!(plan2.new_count, 0);
        assert_eq!(plan2.duplicate_count, 3);
        // Delete with workspace-trash fallback path exercised when OS trash is unavailable.
        let moved = delete_document(&d, &scan.id, false).unwrap();
        assert!(e.db.with(|c| repo::get_document(c, &scan.id)).unwrap().is_none());
        assert!(!e.paths.documents_dir().join("scan.jpg").exists());
        if let Some(p) = moved {
            assert!(p.starts_with(e.paths.trash_dir()));
        }
    }

    #[test]
    fn nfd_named_files_keep_their_spelling_and_copies_become_nfc() {
        let e = env();
        let d = deps(&e, None);
        let nfd = "\u{1112}\u{1161}\u{11ab}.png"; // 한.png in decomposed form
                                                  // In place (inbox): the stored rel_path must match the file on disk byte for byte.
        let inside = e.paths.inbox_dir().join(nfd);
        RgbImage::from_pixel(16, 16, Rgb([0, 0, 0])).save(&inside).unwrap();
        crate::modelstore::mark_settled(&inside);
        // External copy: the copy gets an NFC name.
        let ext = external_dir(&e);
        let outside = ext.join(nfd);
        RgbImage::from_pixel(24, 16, Rgb([0, 0, 0])).save(&outside).unwrap();
        let plan =
            plan(&e.db, &e.paths, &[inside.clone(), outside.clone()], &ImportOptions::default(), ImportSource::Folder)
                .unwrap();
        assert_eq!(plan.new_count, 2, "{:?}", plan.items);
        let report = run(&d, &plan, None, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert_eq!(report.imported.len(), 2, "{:?}", report.failed);
        let docs = e.db.with(|c| repo::list_documents(c, &DocumentFilter::default())).unwrap();
        let in_place = docs.iter().find(|x| x.rel_path.starts_with("inbox/")).unwrap();
        assert_eq!(in_place.rel_path, format!("inbox/{nfd}"), "on-disk spelling kept");
        assert_eq!(in_place.original_name, "\u{d55c}.png", "display name is NFC");
        assert_eq!(in_place.status, DocumentStatus::Rendered);
        let copied = docs.iter().find(|x| x.rel_path.starts_with("documents/")).unwrap();
        assert_eq!(copied.rel_path, "documents/\u{d55c}.png", "copies are written with an NFC name");
        assert!(e.paths.documents_dir().join("\u{d55c}.png").is_file());
        assert_eq!(copied.status, DocumentStatus::Rendered);
    }

    #[test]
    fn rescan_tracks_missing_changed_and_relinks() {
        let e = env();
        let d = deps(&e, None);
        let src = e.paths.inbox_dir().join("scan.png");
        RgbImage::from_pixel(64, 64, Rgb([0, 0, 0])).save(&src).unwrap();
        let plan =
            plan(&e.db, &e.paths, std::slice::from_ref(&src), &ImportOptions::default(), ImportSource::Inbox).unwrap();
        // Inbox source: a file written just now is still "copying".
        assert_eq!(plan.copying_count, 1);
        crate::modelstore::mark_settled(&src);
        let plan =
            super::plan(&e.db, &e.paths, std::slice::from_ref(&src), &ImportOptions::default(), ImportSource::Inbox)
                .unwrap();
        assert_eq!(plan.new_count, 1);
        let report = run(&d, &plan, None, &mut |_| {}, &mut |_, _| {}).unwrap();
        let id = report.imported[0].clone();
        // Move the file (explorer rename) → missing, then relink by hash.
        let moved = e.paths.documents_dir().join("renamed.png");
        std::fs::rename(&src, &moved).unwrap();
        let mut changed = Vec::new();
        let r = rescan(&d, None, &mut |id| changed.push(id.to_string())).unwrap();
        assert_eq!(r.missing, vec![id.clone()]);
        assert_eq!(e.db.with(|c| repo::get_document(c, &id)).unwrap().unwrap().status, DocumentStatus::Missing);
        let plan =
            super::plan(&e.db, &e.paths, std::slice::from_ref(&moved), &ImportOptions::default(), ImportSource::Folder)
                .unwrap();
        assert!(matches!(plan.items[0].status, ImportItemStatus::Relink { .. }));
        let report = run(&d, &plan, None, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert_eq!(report.relinked, vec![id.clone()]);
        let doc = e.db.with(|c| repo::get_document(c, &id)).unwrap().unwrap();
        assert_eq!(doc.rel_path, "documents/renamed.png");
        assert_eq!(doc.status, DocumentStatus::Rendered, "cached pages were kept");
        // Content change → re-rendered.
        std::thread::sleep(Duration::from_millis(1100));
        RgbImage::from_pixel(32, 48, Rgb([0, 0, 0])).save(&moved).unwrap();
        let r = rescan(&d, None, &mut |_| {}).unwrap();
        assert_eq!(r.changed, vec![id.clone()]);
        assert_eq!(r.reprocessed, vec![id.clone()]);
        let pages = e.db.with(|c| repo::list_pages(c, &id, false)).unwrap();
        assert_eq!((pages[0].width_px, pages[0].height_px), (32, 48));
        // inbox_plan ignores registered files.
        let p = inbox_plan(&e.db, &e.paths, &ImportOptions::default()).unwrap();
        assert!(p.items.is_empty());
    }

    #[test]
    fn full_pipeline_with_pdfium_renders_hwpx_and_pdf() {
        let Some(svc) = pdfium() else {
            eprintln!("import tests: skipped (set DOCULENS_PDFIUM)");
            return;
        };
        let e = env();
        let d = deps(&e, Some(svc));
        let ext = external_dir(&e);
        write_blank_pdf(&ext.join("big.pdf"), 100, &["Page one".into()]).unwrap();
        hwpx::write_sample_hwpx(&ext.join("견적서.hwpx"), 3, TINY_PNG).unwrap();
        let plan =
            plan(&e.db, &e.paths, std::slice::from_ref(&ext), &ImportOptions::default(), ImportSource::Drop).unwrap();
        let mut last = ImportProgress::default();
        let report = run(&d, &plan, None, &mut |p| last = p.clone(), &mut |_, _| {}).unwrap();
        assert_eq!(report.imported.len(), 2, "{:?}", report.failed);
        assert_eq!(last.done, 2);
        let docs = e.db.with(|c| repo::list_documents(c, &DocumentFilter::default())).unwrap();
        let big = docs.iter().find(|d| d.original_name == "big.pdf").unwrap();
        assert_eq!(big.status, DocumentStatus::Rendered);
        assert_eq!(big.page_count, 100);
        assert!(big.has_text_layer);
        assert!(DocumentCache::for_document(&e.paths, &big.id).page_file(100).is_file());
        let h = docs.iter().find(|d| d.original_name == "견적서.hwpx").unwrap();
        assert_eq!(h.status, DocumentStatus::Rendered);
        assert_eq!(h.page_count, 3, "stub printer: one page per section break");
        assert!(h.converted);
        let pages = e.db.with(|c| repo::list_pages(c, &h.id, true)).unwrap();
        assert!(
            pages[0].text.as_deref().unwrap().contains("견적서 1쪽"),
            "model text kept when the PDF has no text layer"
        );
        // Rotate a page of the PDF.
        let p = rotate_page(&d, big, 2, 90).unwrap();
        assert_eq!((p.width_px, p.height_px), (842, 595));
        assert_eq!(e.db.with(|c| repo::get_page(c, &big.id, 2)).unwrap().unwrap().rotation, 90);
        let stats = e.db.with(repo::stats).unwrap();
        assert_eq!(stats.total_pages, 103);
    }
}
