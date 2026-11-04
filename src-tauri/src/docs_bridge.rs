//! Glue between the document core (`doculens_core::documents`) and the shell:
//! PDFium discovery, inbox watching, import / rescan / re-render jobs and the
//! `documents://changed` event.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use doculens_core::db::Db;
use doculens_core::documents::convert::PdfPrinter;
use doculens_core::documents::import::{self, ImportDeps, ImportOptions, ImportPlan, ImportSource};
use doculens_core::documents::render::{self, RenderOptions, RenderService};
use doculens_core::documents::{repo, DocumentRecord, SourceKind};
use doculens_core::fswatch::DirWatcher;
use doculens_core::paths::WorkspacePaths;
use doculens_core::CoreError;

use crate::jobs::JobKind;
use crate::state::{AppState, UiError, UiResult};

pub const EV_DOCUMENTS_CHANGED: &str = "documents://changed";
const INBOX_SETTLE: Duration = Duration::from_secs(3);
/// How often the inbox is re-checked while files are still being copied.
const INBOX_RETRY: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Serialize)]
pub struct DocumentsChanged {
    /// `imported` | `updated` | `removed` | `rescanned` | `batch`
    pub kind: &'static str,
    pub ids: Vec<String>,
}

pub fn emit_changed(app: &AppHandle, kind: &'static str, ids: Vec<String>) {
    let _ = app.emit(EV_DOCUMENTS_CHANGED, DocumentsChanged { kind, ids });
}

// ---------------------------------------------------------------------------
// PDFium
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PdfiumInfo {
    pub library: Option<PathBuf>,
    pub error: Option<String>,
    pub searched: Vec<PathBuf>,
}

fn pdfium_candidates(app: &AppHandle) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        v.push(dir.join("pdfium"));
        v.push(dir);
    }
    v.extend(render::default_pdfium_candidates());
    let state = app.state::<AppState>();
    v.push(state.app_paths.root.join("pdfium"));
    v
}

/// Binds PDFium once; later calls return the cached outcome.
pub fn render_service(app: &AppHandle) -> Option<&'static RenderService> {
    let state = app.state::<AppState>();
    let outcome = state.pdfium.get_or_init(|| {
        let candidates = pdfium_candidates(app);
        match render::locate_pdfium(&candidates) {
            Some(lib) => match RenderService::global(&lib) {
                Ok(_) => {
                    log::info!("pdfium loaded from {}", lib.display());
                    Ok(lib)
                }
                Err(e) => {
                    log::error!("pdfium load failed: {e}");
                    Err(e.to_string())
                }
            },
            None => {
                log::warn!("pdfium library not found (searched {:?}); PDF pages will not render", candidates);
                Err("pdfium_not_found".into())
            }
        }
    });
    match outcome {
        Ok(_) => RenderService::get(),
        Err(_) => None,
    }
}

pub fn pdfium_info(app: &AppHandle) -> PdfiumInfo {
    let searched = pdfium_candidates(app);
    let _ = render_service(app);
    let state = app.state::<AppState>();
    match state.pdfium.get() {
        Some(Ok(lib)) => PdfiumInfo { library: Some(lib.clone()), error: None, searched },
        Some(Err(e)) => PdfiumInfo { library: None, error: Some(e.clone()), searched },
        None => PdfiumInfo { library: None, error: None, searched },
    }
}

// ---------------------------------------------------------------------------
// Dependencies for the core pipeline
// ---------------------------------------------------------------------------

/// Owned handles so a job thread can build `ImportDeps` without holding the
/// workspace lock.
pub struct DocsContext {
    pub db: Arc<Db>,
    pub paths: WorkspacePaths,
    pub render: Option<&'static RenderService>,
    pub printer: Arc<dyn PdfPrinter>,
    pub render_options: RenderOptions,
}

impl DocsContext {
    pub fn deps(&self) -> ImportDeps<'_> {
        ImportDeps {
            db: &self.db,
            paths: &self.paths,
            render: self.render,
            printer: self.printer.as_ref(),
            render_options: self.render_options.clone(),
        }
    }
}

pub fn context(app: &AppHandle) -> UiResult<DocsContext> {
    let state = app.state::<AppState>();
    let (db, paths) = state.with_workspace(|ws| (ws.db.clone(), ws.paths.clone()))?;
    let settings = state.settings_snapshot()?;
    let render_options =
        RenderOptions { dpi: settings.processing.render_dpi.clamp(72, 600), thumbnails: true, ..Default::default() };
    let printer: Arc<dyn PdfPrinter> =
        state.printer.get().cloned().unwrap_or_else(|| Arc::new(doculens_core::documents::convert::StubPdfPrinter));
    Ok(DocsContext { db, paths, render: render_service(app), printer, render_options })
}

pub fn document(app: &AppHandle, id: &str) -> UiResult<DocumentRecord> {
    let ctx = context(app)?;
    let doc = ctx.db.with(|c| repo::get_document(c, id))?;
    doc.ok_or_else(|| CoreError::NotFound(id.to_string()).into())
}

// ---------------------------------------------------------------------------
// Workspace lifecycle: inbox watcher + startup reconciliation
// ---------------------------------------------------------------------------

pub fn on_workspace_changed(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(mut g) = state.inbox_watcher.lock() {
        *g = None;
    }
    if let Ok(mut m) = state.import_plans.lock() {
        m.clear();
    }
    let Ok(ctx) = context(app) else { return };
    let watch = state.settings_snapshot().map(|s| s.processing.inbox_watch).unwrap_or(true);
    if watch {
        start_inbox_watcher(app, &ctx.paths);
    }
    // Reconcile registered documents (missing / changed / interrupted) and
    // pick up inbox files on a background job.
    rescan_job(app, true);
}

pub fn start_inbox_watcher(app: &AppHandle, paths: &WorkspacePaths) {
    let state = app.state::<AppState>();
    let inbox = paths.inbox_dir();
    let app2 = app.clone();
    let relevant = |p: &std::path::Path| p.is_dir() || SourceKind::from_path(p).is_some();
    match DirWatcher::start(&inbox, INBOX_SETTLE, "doculens-inbox-watch", relevant, move || inbox_job(&app2)) {
        Ok(w) => {
            if let Ok(mut g) = state.inbox_watcher.lock() {
                *g = Some(w);
            }
        }
        Err(e) => log::warn!("inbox watcher not started: {e}"),
    }
}

pub fn set_inbox_watch(app: &AppHandle, enabled: bool) -> UiResult<()> {
    let state = app.state::<AppState>();
    if let Ok(mut g) = state.inbox_watcher.lock() {
        *g = None;
    }
    if enabled {
        let paths = state.with_workspace(|ws| ws.paths.clone())?;
        start_inbox_watcher(app, &paths);
        inbox_job(app);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

fn spawn_job_thread(
    name: &str,
    app: AppHandle,
    job: String,
    body: impl FnOnce(&AppHandle) -> std::result::Result<Option<String>, String> + Send + 'static,
) {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&app)));
            let state = app.state::<AppState>();
            match outcome {
                Ok(result) => state.jobs.finish(&job, result),
                Err(p) => {
                    let msg = p
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "panic".into());
                    log::error!("job {job} panicked: {msg}");
                    state.jobs.finish(&job, Err(format!("internal error: {msg}")));
                }
            }
        })
        .ok();
}

/// Stores a plan for `documents_import_run`; returns its token.
pub fn remember_plan(app: &AppHandle, plan: ImportPlan) -> String {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let state = app.state::<AppState>();
    if let Ok(mut m) = state.import_plans.lock() {
        if m.len() > 16 {
            m.clear();
        }
        m.insert(token.clone(), plan);
    }
    token
}

pub fn take_plan(app: &AppHandle, token: &str) -> Option<ImportPlan> {
    let state = app.state::<AppState>();
    let plan = state.import_plans.lock().ok().and_then(|mut m| m.remove(token));
    plan
}

/// Runs an import plan as a job; returns the job id.
pub fn import_job(app: &AppHandle, plan: ImportPlan) -> UiResult<String> {
    let state = app.state::<AppState>();
    let ctx = context(app)?;
    let subject = match plan.source {
        ImportSource::Inbox => "inbox".to_string(),
        _ => format!("{} files", plan.new_count + plan.relink_count),
    };
    let (job, cancel) = state.jobs.create(JobKind::DocumentImport, &subject);
    let job_id = job.clone();
    spawn_job_thread("doculens-doc-import", app.clone(), job.clone(), move |app| {
        let state = app.state::<AppState>();
        let deps = ctx.deps();
        let mut batch_ids: Vec<String> = Vec::new();
        let report = import::run(
            &deps,
            &plan,
            Some(&cancel),
            &mut |p| {
                let frac = if p.total == 0 {
                    None
                } else {
                    let page_part = p.page.map(|(d, t)| if t == 0 { 0.0 } else { d as f32 / t as f32 }).unwrap_or(0.0);
                    Some(((p.done as f32 + page_part) / p.total as f32).min(1.0))
                };
                let detail = match p.page {
                    Some((d, t)) if t > 0 => format!("{} ({d}/{t})", p.current),
                    _ => p.current.clone(),
                };
                state.jobs.progress(&job_id, frac, None, None, Some(detail));
            },
            &mut |id, _ok| {
                batch_ids.push(id.to_string());
                emit_changed(app, "imported", vec![id.to_string()]);
            },
        )
        .map_err(|e| e.to_string())?;
        if !batch_ids.is_empty() {
            emit_changed(app, "batch", batch_ids);
        }
        if report.cancelled || cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let summary = format!(
            "imported {} / relinked {} / failed {} / skipped {}",
            report.imported.len(),
            report.relinked.len(),
            report.failed.len(),
            report.skipped
        );
        if !report.failed.is_empty() && report.imported.is_empty() && report.relinked.is_empty() {
            return Err(report.failed.iter().map(|(n, e)| format!("{n}: {e}")).collect::<Vec<_>>().join("; "));
        }
        Ok(Some(summary))
    });
    Ok(job)
}

/// Imports settled, unregistered inbox files (watcher / toggle / startup).
pub fn inbox_job(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.jobs.is_active(JobKind::DocumentImport, "inbox") {
        // A run is in progress; re-check when it finishes (the watcher fires again on new files).
        return;
    }
    let Ok(ctx) = context(app) else { return };
    let plan = match import::inbox_plan(&ctx.db, &ctx.paths, &ImportOptions::default()) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("inbox scan failed: {e}");
            return;
        }
    };
    if plan.copying_count > 0 {
        // Files still being written: look again shortly.
        let app2 = app.clone();
        std::thread::Builder::new()
            .name("doculens-inbox-retry".into())
            .spawn(move || {
                std::thread::sleep(INBOX_RETRY);
                inbox_job(&app2);
            })
            .ok();
    }
    if plan.new_count + plan.relink_count == 0 {
        return;
    }
    if let Err(e) = import_job(app, plan) {
        log::warn!("inbox import not started: {}", e.message);
    }
}

/// Reconciles the library with the file system; `then_inbox` chains an inbox scan.
pub fn rescan_job(app: &AppHandle, then_inbox: bool) -> Option<String> {
    let state = app.state::<AppState>();
    if state.jobs.is_active(JobKind::DocumentRescan, "library") {
        return None;
    }
    let ctx = context(app).ok()?;
    let (job, cancel) = state.jobs.create(JobKind::DocumentRescan, "library");
    let job_id = job.clone();
    spawn_job_thread("doculens-doc-rescan", app.clone(), job_id, move |app| {
        let deps = ctx.deps();
        let mut touched = Vec::new();
        let report = import::rescan(&deps, Some(&cancel), &mut |id| {
            touched.push(id.to_string());
            emit_changed(app, "updated", vec![id.to_string()]);
        })
        .map_err(|e| e.to_string())?;
        if !touched.is_empty() {
            emit_changed(app, "rescanned", touched);
        }
        if then_inbox {
            inbox_job(app);
        }
        Ok(Some(format!(
            "checked {} / missing {} / restored {} / changed {} / failed {}",
            report.checked,
            report.missing.len(),
            report.restored.len(),
            report.changed.len(),
            report.failed.len()
        )))
    });
    Some(job)
}

/// Re-renders documents (new DPI or after a renderer became available).
pub fn rerender_job(app: &AppHandle, ids: Vec<String>, dpi: Option<u16>) -> UiResult<String> {
    let state = app.state::<AppState>();
    let ctx = context(app)?;
    let subject = format!("{} documents", ids.len());
    let (job, cancel) = state.jobs.create(JobKind::DocumentRender, &subject);
    let job_id = job.clone();
    spawn_job_thread("doculens-doc-render", app.clone(), job.clone(), move |app| {
        let state = app.state::<AppState>();
        let deps = ctx.deps();
        let total = ids.len() as u32;
        let mut failed = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let Some(doc) = ctx.db.with(|c| repo::get_document(c, id)).map_err(|e| e.to_string())? else { continue };
            state.jobs.progress(&job_id, Some(i as f32 / total as f32), None, None, Some(doc.original_name.clone()));
            let name = doc.original_name.clone();
            if let Err(e) = import::process_document(&deps, &doc, true, dpi, Some(&cancel), &mut |(d, t)| {
                let page_part = if t == 0 { 0.0 } else { d as f32 / t as f32 };
                state.jobs.progress(
                    &job_id,
                    Some((i as f32 + page_part) / total as f32),
                    None,
                    None,
                    Some(format!("{name} ({d}/{t})")),
                );
            }) {
                if matches!(e, CoreError::Cancelled) {
                    return Err("cancelled".into());
                }
                failed.push(format!("{name}: {e}"));
            }
            emit_changed(app, "updated", vec![id.clone()]);
        }
        if !failed.is_empty() {
            return Err(failed.join("; "));
        }
        Ok(Some(format!("rendered {total}")))
    });
    Ok(job)
}

/// Deletes documents synchronously (fast: DB rows, cache files, OS trash).
pub fn delete_documents(app: &AppHandle, ids: &[String], keep_files: bool) -> UiResult<Vec<String>> {
    let ctx = context(app)?;
    let deps = ctx.deps();
    let mut removed = Vec::new();
    let mut errors = Vec::new();
    for id in ids {
        match import::delete_document(&deps, id, keep_files) {
            Ok(_) => removed.push(id.clone()),
            Err(e) => errors.push(format!("{id}: {e}")),
        }
    }
    if !removed.is_empty() {
        emit_changed(app, "removed", removed.clone());
    }
    if !errors.is_empty() && removed.is_empty() {
        return Err(UiError::invalid(errors.join("; ")));
    }
    Ok(removed)
}
