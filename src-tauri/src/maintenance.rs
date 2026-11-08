//! Daily retention purge (design §3.8, decision 16): once a day at the
//! workspace's `purge_hour_local` (and on demand from Settings) a low-priority
//! persistent job deletes expired data **without** the OS recycle bin —
//! file-operation journal + workspace trash, finished jobs, saved
//! conversations, app / workspace logs, temporary files, old extraction runs
//! (never a document's latest run), optionally documents past their retention,
//! and trims the page / thumbnail cache to its size cap (LRU, oldest-used
//! first, never anything used in the last week). Every run appends one line to
//! `.doculens/logs/purge-YYYYMM.jsonl` (the audit trail) and records the last
//! summary in the workspace database for the settings screen.

use std::time::{Duration, SystemTime};

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager};

use doculens_core::documents::{self as docs, import, repo as docs_repo};
use doculens_core::jobs::{self as core_jobs, JobControl, JobPriority, JobSpec};
use doculens_core::knowledge::repo as kb_repo;
use doculens_core::organize::journal;
use doculens_core::pipeline::repo as pipeline_repo;
use doculens_core::retention::{self, FsPurge, RetentionItem, RetentionPolicy};

use crate::jobs::{JobKind, JobState, WorkerResult};
use crate::pipeline_bridge;
use crate::state::{AppState, UiResult};

const KV_LAST_RUN: &str = "maintenance.last_run";
const SCHEDULER_POLL: Duration = Duration::from_secs(600);
const STARTUP_DELAY: Duration = Duration::from_secs(120);
/// Cache entries used within this window are never evicted by the LRU trim.
const CACHE_MIN_AGE: Duration = Duration::from_secs(7 * 86_400);
/// A missed daily run (PC off at the scheduled hour) is caught up after this long.
const CATCH_UP_AFTER: chrono::Duration = chrono::Duration::hours(36);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Purged {
    pub journal: u64,
    pub jobs: u64,
    pub answers: u64,
    pub cache_bytes: u64,
    pub trash: u64,
    pub logs: u64,
    pub temp: u64,
    pub documents: u64,
    pub extractions: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaintenanceSummary {
    pub started_at: DateTime<Utc>,
    pub elapsed_ms: u64,
    pub purged: Purged,
    /// Steps that were skipped (policy "keep forever", cancelled, errors).
    pub skipped: Vec<String>,
    pub cancelled: bool,
}

impl Default for MaintenanceSummary {
    fn default() -> Self {
        Self { started_at: Utc::now(), elapsed_ms: 0, purged: Purged::default(), skipped: Vec::new(), cancelled: false }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MaintenanceStatus {
    pub enabled: bool,
    pub purge_hour_local: u8,
    pub last_run_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub running: bool,
    pub last_summary: Option<MaintenanceSummary>,
}

fn policy(app: &AppHandle) -> UiResult<RetentionPolicy> {
    app.state::<AppState>().with_workspace(|ws| ws.manifest.retention.clone())
}

fn last_summary(app: &AppHandle) -> Option<MaintenanceSummary> {
    let pctx = pipeline_bridge::context(app).ok()?;
    let raw = pctx.db().kv_get(KV_LAST_RUN).ok().flatten()?;
    serde_json::from_str(&raw).ok()
}

/// Next scheduled run: today at `purge_hour_local` (local time) if still ahead, else tomorrow.
fn next_run(policy: &RetentionPolicy, now_local: DateTime<Local>) -> Option<DateTime<Utc>> {
    if !policy.purge_enabled {
        return None;
    }
    let hour = u32::from(policy.purge_hour_local.min(23));
    let today = Local.with_ymd_and_hms(now_local.year(), now_local.month(), now_local.day(), hour, 0, 0).single()?;
    let next = if today > now_local { today } else { today + chrono::Duration::days(1) };
    Some(next.with_timezone(&Utc))
}

pub fn status(app: &AppHandle) -> UiResult<MaintenanceStatus> {
    let policy = policy(app)?;
    let state = app.state::<AppState>();
    let running = state
        .jobs
        .list()
        .iter()
        .any(|j| j.kind == JobKind::Maintenance && matches!(j.state, JobState::Running | JobState::Queued));
    let last = last_summary(app);
    Ok(MaintenanceStatus {
        enabled: policy.purge_enabled,
        purge_hour_local: policy.purge_hour_local,
        last_run_at: last.as_ref().map(|s| s.started_at),
        next_run_at: next_run(&policy, Local::now()),
        running,
        last_summary: last,
    })
}

/// Queues the purge job now (or returns the id of the one already queued / running).
pub fn run_now(app: &AppHandle) -> UiResult<String> {
    let state = app.state::<AppState>();
    if let Some(j) = state
        .jobs
        .list()
        .into_iter()
        .find(|j| j.kind == JobKind::Maintenance && matches!(j.state, JobState::Running | JobState::Queued))
    {
        return Ok(j.id);
    }
    state.jobs.submit(JobSpec::Maintenance, "retention purge", JobPriority::Low)
}

/// Background scheduler: every ten minutes, when a workspace is open and the policy is
/// enabled, run at the configured local hour once per day; catch up a missed day.
pub fn install_scheduler(app: &AppHandle) {
    let app = app.clone();
    std::thread::Builder::new()
        .name("doculens-maintenance".into())
        .spawn(move || {
            std::thread::sleep(STARTUP_DELAY);
            loop {
                tick(&app);
                std::thread::sleep(SCHEDULER_POLL);
            }
        })
        .ok();
}

fn tick(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.switching.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let Ok(policy) = policy(app) else { return };
    if !policy.purge_enabled {
        return;
    }
    let now_local = Local::now();
    let last = last_summary(app).map(|s| s.started_at);
    let ran_today = last.is_some_and(|t| t.with_timezone(&Local).date_naive() == now_local.date_naive());
    let due_hour = now_local.hour() == u32::from(policy.purge_hour_local.min(23));
    let overdue = last.is_none_or(|t| Utc::now() - t > CATCH_UP_AFTER);
    if (due_hour && !ran_today) || (overdue && !ran_today) {
        match run_now(app) {
            Ok(id) => log::info!("maintenance: purge job {id} scheduled"),
            Err(e) => log::warn!("maintenance: could not schedule the purge job: {}", e.message),
        }
    }
}

fn cutoff(days: u32) -> SystemTime {
    SystemTime::now() - Duration::from_secs(u64::from(days) * 86_400)
}

fn cutoff_utc(days: u32) -> DateTime<Utc> {
    Utc::now() - chrono::Duration::days(i64::from(days))
}

fn add(fs: FsPurge, files: &mut u64, bytes: Option<&mut u64>) {
    *files += fs.files;
    if let Some(b) = bytes {
        *b += fs.bytes;
    }
}

/// The purge itself (runs on the persistent job worker thread).
pub fn run(app: &AppHandle, ctl: &JobControl) -> WorkerResult {
    let started = std::time::Instant::now();
    let pctx = pipeline_bridge::context(app).map_err(|e| (e.message, None))?;
    let policy = policy(app).map_err(|e| (e.message, None))?;
    let mut summary = MaintenanceSummary::default();
    let paths = pctx.docs.paths.clone();
    let db = pctx.db().clone();
    if !policy.purge_enabled {
        summary.skipped.push("disabled".into());
        return finish(app, &db, &paths, summary, started);
    }
    let check = |summary: &mut MaintenanceSummary| -> bool {
        if ctl.cancelled() {
            summary.cancelled = true;
            true
        } else {
            false
        }
    };

    // 1. Journal + workspace trash (the trash only backs undo of journal batches).
    match policy.effective_days(RetentionItem::Journal) {
        Some(days) => {
            summary.purged.journal += db.with(|c| journal::purge(c, days)).unwrap_or(0) as u64;
            add(retention::purge_old_files(&paths.trash_dir(), cutoff(days)), &mut summary.purged.trash, None);
        }
        None => summary.skipped.push("journal".into()),
    }
    if check(&mut summary) {
        return finish(app, &db, &paths, summary, started);
    }
    // 2. Finished jobs + job files.
    match policy.effective_days(RetentionItem::JobLogs) {
        Some(days) => {
            summary.purged.jobs +=
                db.with(|c| core_jobs::purge_finished_before(c, &cutoff_utc(days))).unwrap_or(0) as u64;
            add(retention::purge_old_files(&paths.jobs_dir(), cutoff(days)), &mut summary.purged.jobs, None);
        }
        None => summary.skipped.push("jobs".into()),
    }
    // 3. Saved conversations.
    match policy.effective_days(RetentionItem::Chats) {
        Some(days) => {
            summary.purged.answers +=
                db.with(|c| kb_repo::purge_answers_before(c, &cutoff_utc(days))).unwrap_or(0) as u64;
        }
        None => summary.skipped.push("chats".into()),
    }
    // 4. Logs: workspace logs and app logs.
    match policy.effective_days(RetentionItem::AppLogs) {
        Some(days) => {
            let app_logs = app.state::<AppState>().app_paths.logs_dir();
            add(retention::purge_old_files(&paths.logs_dir(), cutoff(days)), &mut summary.purged.logs, None);
            add(retention::purge_old_files(&app_logs, cutoff(days)), &mut summary.purged.logs, None);
        }
        None => summary.skipped.push("logs".into()),
    }
    // 5. Temporary pre-masking data (default 0 days = whatever is there goes).
    match policy.effective_days(RetentionItem::Temp) {
        Some(days) => {
            add(
                retention::purge_old_files(&paths.cache_dir().join("tmp"), cutoff(days)),
                &mut summary.purged.temp,
                None,
            );
        }
        None => summary.skipped.push("temp".into()),
    }
    if check(&mut summary) {
        return finish(app, &db, &paths, summary, started);
    }
    // 6. Old extraction runs (never a document's latest run) + their mirrors.
    match policy.effective_days(RetentionItem::Extractions) {
        Some(days) => match db.with(|c| pipeline_repo::purge_runs_before(c, &cutoff_utc(days))) {
            Ok(runs) => {
                for (run_id, doc_id) in &runs {
                    let _ = std::fs::remove_dir_all(paths.extractions_dir().join(doc_id).join(run_id));
                }
                summary.purged.extractions += runs.len() as u64;
            }
            Err(e) => summary.skipped.push(format!("extractions: {e}")),
        },
        None => summary.skipped.push("extractions".into()),
    }
    // 7. Documents past their retention (opt-in; default keep forever). No recycle bin.
    match policy.effective_days(RetentionItem::Documents) {
        Some(days) => {
            let cut = cutoff_utc(days);
            let expired: Vec<_> = db
                .with(docs_repo::all_documents)
                .unwrap_or_default()
                .into_iter()
                .filter(|d| d.imported_at < cut && d.deleted_at.is_none())
                .collect();
            let deps = pctx.docs.deps();
            let mut removed = Vec::new();
            for d in expired {
                if check(&mut summary) {
                    break;
                }
                let abs = docs::from_rel_path(&paths.root, &d.rel_path);
                // `keep_file` skips the recycle bin; the cache and the extraction mirror go with the row.
                if import::delete_document(&deps, &d.id, true).is_ok() {
                    let _ = std::fs::remove_file(&abs);
                    removed.push(d.id.clone());
                }
            }
            summary.purged.documents += removed.len() as u64;
            if !removed.is_empty() {
                pctx.emit(app, "documents", removed);
            }
        }
        None => summary.skipped.push("documents".into()),
    }
    // 8. Page / thumbnail cache size cap (LRU over per-document folders).
    let cap = (policy.page_cache_max_gb.max(0.5) * 1024.0 * 1024.0 * 1024.0) as u64;
    let pages = retention::trim_lru_dirs(&paths.page_cache_dir(), cap * 9 / 10, CACHE_MIN_AGE);
    let thumbs = retention::trim_lru_dirs(&paths.thumb_cache_dir(), cap / 10, CACHE_MIN_AGE);
    summary.purged.cache_bytes += pages.bytes + thumbs.bytes;
    finish(app, &db, &paths, summary, started)
}

fn finish(
    app: &AppHandle,
    db: &doculens_core::db::Db,
    paths: &doculens_core::paths::WorkspacePaths,
    mut summary: MaintenanceSummary,
    started: std::time::Instant,
) -> WorkerResult {
    summary.elapsed_ms = started.elapsed().as_millis() as u64;
    let value = serde_json::to_value(&summary).unwrap_or(json!({}));
    if let Ok(raw) = serde_json::to_string(&summary) {
        let _ = db.kv_set(KV_LAST_RUN, &raw);
        // Audit trail (§3.8: 파기 내역을 감사 로그에 남긴다).
        let logs = paths.logs_dir();
        let _ = std::fs::create_dir_all(&logs);
        let file = logs.join(format!("purge-{}.jsonl", Local::now().format("%Y%m")));
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(&file) {
            use std::io::Write;
            let _ = writeln!(f, "{raw}");
        }
    }
    let ko = app
        .state::<AppState>()
        .settings
        .lock()
        .map(|s| s.ui.language == doculens_core::settings::Language::Ko)
        .unwrap_or(true);
    let p = &summary.purged;
    let detail = if ko {
        format!(
            "저널 {} · 작업 {} · 대화 {} · 추출 {} · 문서 {} · 캐시 {} MB",
            p.journal,
            p.jobs,
            p.answers,
            p.extractions,
            p.documents,
            p.cache_bytes / (1024 * 1024)
        )
    } else {
        format!(
            "journal {} · jobs {} · chats {} · runs {} · documents {} · cache {} MB",
            p.journal,
            p.jobs,
            p.answers,
            p.extractions,
            p.documents,
            p.cache_bytes / (1024 * 1024)
        )
    };
    if summary.cancelled {
        return Err(("cancelled".into(), Some(value)));
    }
    Ok((Some(detail), Some(value)))
}
