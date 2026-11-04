//! Glue between the document core (`doculens_core::documents`) and the shell:
//! PDFium discovery, inbox watching, import / rescan / re-render jobs and the
//! `documents://changed` event.
//!
//! Lifecycle rules: document jobs hold their own `DocsContext` (DB handle,
//! paths, workspace generation). Before a workspace is closed or switched,
//! `before_workspace_change` cancels them and waits briefly; a job whose
//! generation went stale stops publishing events.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
/// How long a workspace switch waits for cancelled document jobs to stop.
const CANCEL_WAIT: Duration = Duration::from_secs(8);
const DOC_JOBS: [JobKind; 3] = [JobKind::DocumentImport, JobKind::DocumentRescan, JobKind::DocumentRender];
/// Everything that touches the workspace DB on a background thread.
const WORKSPACE_JOBS: [JobKind; 8] = [
    JobKind::DocumentImport,
    JobKind::DocumentRescan,
    JobKind::DocumentRender,
    JobKind::KnowledgeIndex,
    JobKind::KnowledgeEmbed,
    JobKind::Pipeline,
    JobKind::DocumentSplit,
    JobKind::DocumentMerge,
];

/// Work chained after a job has been marked finished.
pub(crate) type AfterJob = Box<dyn FnOnce(&AppHandle) + Send>;

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

/// Binds PDFium once; a failed attempt is remembered for diagnostics but
/// retried on the next call (the user may have installed the library since).
pub fn render_service(app: &AppHandle) -> Option<&'static RenderService> {
    if let Some(s) = RenderService::get() {
        return Some(s);
    }
    let state = app.state::<AppState>();
    let candidates = pdfium_candidates(app);
    let outcome = match render::locate_pdfium(&candidates) {
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
            log::warn!("pdfium library not found (searched {} locations); PDF pages will not render", candidates.len());
            Err("pdfium_not_found".into())
        }
    };
    let ok = outcome.is_ok();
    if let Ok(mut g) = state.pdfium.lock() {
        *g = Some(outcome);
    }
    if ok {
        RenderService::get()
    } else {
        None
    }
}

pub fn pdfium_info(app: &AppHandle) -> PdfiumInfo {
    let searched = pdfium_candidates(app);
    let _ = render_service(app);
    let state = app.state::<AppState>();
    let cached = state.pdfium.lock().ok().and_then(|g| g.clone());
    match cached {
        Some(Ok(lib)) => PdfiumInfo { library: Some(lib), error: None, searched },
        Some(Err(e)) => PdfiumInfo { library: None, error: Some(e), searched },
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
    /// `AppState::workspace_generation` when the context was created.
    pub generation: u64,
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

    /// False once the workspace was closed or switched after this context was made.
    pub fn is_current(&self, app: &AppHandle) -> bool {
        app.state::<AppState>().workspace_generation.load(Ordering::SeqCst) == self.generation
    }

    fn emit(&self, app: &AppHandle, kind: &'static str, ids: Vec<String>) {
        if self.is_current(app) {
            emit_changed(app, kind, ids);
        }
    }
}

pub fn context(app: &AppHandle) -> UiResult<DocsContext> {
    let state = app.state::<AppState>();
    let (db, paths) = state.with_workspace(|ws| (ws.db.clone(), ws.paths.clone()))?;
    let generation = state.workspace_generation.load(Ordering::SeqCst);
    let settings = state.settings_snapshot()?;
    let render_options =
        RenderOptions { dpi: settings.processing.render_dpi.clamp(72, 600), thumbnails: true, ..Default::default() };
    let printer: Arc<dyn PdfPrinter> =
        state.printer.get().cloned().unwrap_or_else(|| Arc::new(doculens_core::documents::convert::StubPdfPrinter));
    Ok(DocsContext { db, paths, render: render_service(app), printer, render_options, generation })
}

pub fn document(app: &AppHandle, id: &str) -> UiResult<DocumentRecord> {
    let ctx = context(app)?;
    let doc = ctx.db.with(|c| repo::get_document(c, id))?;
    doc.ok_or_else(|| CoreError::NotFound(id.to_string()).into())
}

// ---------------------------------------------------------------------------
// Workspace lifecycle: cancel jobs, inbox watcher, startup reconciliation
// ---------------------------------------------------------------------------

/// Stops the inbox watcher and cancels document jobs of the workspace that is
/// about to be closed, waiting (bounded) for them to release the database.
pub fn before_workspace_change(app: &AppHandle) {
    let state = app.state::<AppState>();
    // Chained `after` hooks and auto-index must not start new work on the closing workspace.
    state.switching.store(true, Ordering::SeqCst);
    if let Ok(mut g) = state.inbox_watcher.lock() {
        *g = None;
    }
    if let Ok(mut m) = state.import_plans.lock() {
        m.clear();
    }
    if let Ok(mut q) = state.pipeline_queue.lock() {
        q.clear();
    }
    // Streaming answers hold the old database too: stop them at their next delta.
    if let (Ok(running), Ok(mut cancels)) = (state.answer_running.lock(), state.answer_cancels.lock()) {
        for id in running.iter() {
            cancels.insert(id.clone());
        }
    }
    let answers_running = || state.answer_running.lock().map(|r| !r.is_empty()).unwrap_or(false);
    let cancelled = state.jobs.cancel_kinds(&WORKSPACE_JOBS);
    if cancelled.is_empty() && !answers_running() {
        return;
    }
    let deadline = Instant::now() + CANCEL_WAIT;
    while (state.jobs.is_active_kind(&WORKSPACE_JOBS) || answers_running()) && Instant::now() < deadline {
        // A job spawned by an `after` hook in the meantime gets the cancel too.
        state.jobs.cancel_kinds(&WORKSPACE_JOBS);
        std::thread::sleep(Duration::from_millis(50));
    }
    if state.jobs.is_active_kind(&WORKSPACE_JOBS) {
        log::warn!("workspace jobs did not stop within {CANCEL_WAIT:?}; they will stop at the next cancel point");
    }
}

pub fn on_workspace_changed(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.switching.store(false, Ordering::SeqCst);
    if let Ok(mut g) = state.inbox_watcher.lock() {
        *g = None;
    }
    state.inbox_dirty.store(false, Ordering::SeqCst);
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
    // The callback runs on the watcher thread (which `Drop` joins): only hand off.
    let on_change = move || spawn_inbox_scan(&app2);
    match DirWatcher::start(&inbox, INBOX_SETTLE, "doculens-inbox-watch", relevant, on_change) {
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
        spawn_inbox_scan(app);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

/// Runs a job body on a named thread; a panic inside the body fails the job
/// instead of leaving it "running" forever. `after` runs once the job has
/// been marked finished (chained work that must not see this job as active).
pub(crate) fn spawn_job_thread(
    name: &str,
    app: AppHandle,
    job: String,
    body: impl FnOnce(&AppHandle) -> std::result::Result<Option<String>, String> + Send + 'static,
    after: Option<AfterJob>,
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
            if let Some(f) = after {
                f(&app);
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
    let source = plan.source;
    let auto_level = plan.options.auto_level;
    // Ids of the documents this import touched, handed to the auto pipeline afterwards.
    let processed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let processed_after = processed.clone();
    spawn_job_thread(
        "doculens-doc-import",
        app.clone(),
        job.clone(),
        move |app| {
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
                        let page_part =
                            p.page.map(|(d, t)| if t == 0 { 0.0 } else { d as f32 / t as f32 }).unwrap_or(0.0);
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
                    ctx.emit(app, "imported", vec![id.to_string()]);
                },
            )
            .map_err(|e| e.to_string())?;
            if !batch_ids.is_empty() {
                ctx.emit(app, "batch", batch_ids.clone());
            }
            if let Ok(mut p) = processed.lock() {
                p.extend(report.imported.iter().cloned());
                p.extend(report.relinked.iter().cloned());
                p.retain(|id| batch_ids.contains(id));
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
        },
        // Files that arrived while this import ran (or an import into inbox/
        // itself) are picked up once the job is no longer active.
        Some(Box::new(move |app| {
            let state = app.state::<AppState>();
            if state.switching.load(Ordering::SeqCst) {
                return;
            }
            if source == ImportSource::Inbox || state.inbox_dirty.swap(false, Ordering::SeqCst) {
                spawn_inbox_scan(app);
            }
            crate::knowledge_bridge::auto_index(app);
            let ids = processed_after.lock().map(|p| p.clone()).unwrap_or_default();
            crate::pipeline_bridge::auto_process(app, ids, auto_level, source);
        })),
    );
    Ok(job)
}

/// Plans an inbox import off the calling thread (hashing can take a while).
pub fn spawn_inbox_scan(app: &AppHandle) {
    let app2 = app.clone();
    std::thread::Builder::new().name("doculens-inbox-scan".into()).spawn(move || inbox_job(&app2)).ok();
}

/// Imports settled, unregistered inbox files (watcher / toggle / startup).
/// Runs on a background thread; see `spawn_inbox_scan`.
pub fn inbox_job(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.jobs.is_active_kind(&[JobKind::DocumentImport]) {
        // A drop import may be copying into inbox/ right now, or an inbox run is
        // in progress: re-check when the running job finishes.
        state.inbox_dirty.store(true, Ordering::SeqCst);
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
    if !ctx.is_current(app) {
        return;
    }
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
        log::warn!("inbox import not started: {}", e.code);
    }
}

/// Reconciles the library with the file system; `then_inbox` chains an inbox
/// scan. Returns `None` while another document job runs (a rescan would
/// reprocess documents that job is writing).
pub fn rescan_job(app: &AppHandle, then_inbox: bool) -> Option<String> {
    let state = app.state::<AppState>();
    if state.jobs.is_active_kind(&DOC_JOBS) {
        return None;
    }
    let ctx = context(app).ok()?;
    let (job, cancel) = state.jobs.create(JobKind::DocumentRescan, "library");
    spawn_job_thread(
        "doculens-doc-rescan",
        app.clone(),
        job.clone(),
        move |app| {
            let deps = ctx.deps();
            let mut touched = Vec::new();
            let report = import::rescan(&deps, Some(&cancel), &mut |id| {
                touched.push(id.to_string());
                ctx.emit(app, "updated", vec![id.to_string()]);
            })
            .map_err(|e| e.to_string())?;
            if !touched.is_empty() {
                ctx.emit(app, "rescanned", touched);
            }
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            Ok(Some(format!(
                "checked {} / missing {} / restored {} / changed {} / failed {}",
                report.checked,
                report.missing.len(),
                report.restored.len(),
                report.changed.len(),
                report.failed.len()
            )))
        },
        Some(Box::new(move |app: &AppHandle| {
            if app.state::<AppState>().switching.load(Ordering::SeqCst) {
                return;
            }
            if then_inbox {
                spawn_inbox_scan(app);
            }
            crate::knowledge_bridge::auto_index(app);
        }) as AfterJob),
    );
    Some(job)
}

/// Re-renders documents (new DPI or after a renderer became available).
pub fn rerender_job(app: &AppHandle, ids: Vec<String>, dpi: Option<u16>) -> UiResult<String> {
    let state = app.state::<AppState>();
    let ctx = context(app)?;
    let subject = format!("{} documents", ids.len());
    let (job, cancel) = state.jobs.create(JobKind::DocumentRender, &subject);
    let job_id = job.clone();
    spawn_job_thread(
        "doculens-doc-render",
        app.clone(),
        job.clone(),
        move |app| {
            let state = app.state::<AppState>();
            let deps = ctx.deps();
            let total = ids.len() as u32;
            let mut failed = Vec::new();
            for (i, id) in ids.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let Some(doc) = ctx.db.with(|c| repo::get_document(c, id)).map_err(|e| e.to_string())? else {
                    continue;
                };
                state.jobs.progress(
                    &job_id,
                    Some(i as f32 / total as f32),
                    None,
                    None,
                    Some(doc.original_name.clone()),
                );
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
                ctx.emit(app, "updated", vec![id.clone()]);
            }
            if !failed.is_empty() {
                return Err(failed.join("; "));
            }
            Ok(Some(format!("rendered {total}")))
        },
        Some(Box::new(|app: &AppHandle| crate::knowledge_bridge::auto_index(app)) as AfterJob),
    );
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
