//! Daily retention purge (design §3.8, decision 16): once a day at the
//! workspace's `purge_hour_local` (and on demand from Settings) a low-priority
//! persistent job deletes expired data **without** the OS recycle bin —
//! file-operation journal + workspace trash, finished jobs, saved
//! conversations, app / workspace logs, temporary files, old extraction runs
//! (never a document's latest run), optionally documents past their retention,
//! and trims the page / thumbnail cache to its size cap (LRU, oldest-used
//! first, never anything used in the last week). Every run appends one line to
//! `.doculens/logs/purge-YYYYMM.jsonl` (the audit trail) and records the last
//! completed summary in the workspace database for the settings screen (plus the
//! time of the last attempt, so a failing run is retried hourly, not every poll).

use std::time::{Duration, SystemTime};

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager};

use doculens_core::documents::{self as docs, import, repo as docs_repo, DocumentRole};
use doculens_core::jobs::{self as core_jobs, JobControl, JobPriority, JobSpec};
use doculens_core::knowledge::repo as kb_repo;
use doculens_core::organize::journal;
use doculens_core::pipeline::repo as pipeline_repo;
use doculens_core::retention::{self, FsPurge, RetentionItem, RetentionPolicy};
use doculens_core::settings::CacheSettings;

use crate::jobs::{JobKind, JobState, WorkerResult};
use crate::pipeline_bridge;
use crate::state::{AppState, UiResult};

const KV_LAST_RUN: &str = "maintenance.last_run";
/// Every attempt (early error, cancelled run, success) as RFC 3339; `tick` schedules no other
/// run within `RETRY_AFTER` of it, so a run that fails early is not re-queued at every poll.
const KV_LAST_ATTEMPT: &str = "maintenance.last_attempt";
const RETRY_AFTER: chrono::Duration = chrono::Duration::minutes(60);
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

fn last_attempt(app: &AppHandle) -> Option<DateTime<Utc>> {
    let pctx = pipeline_bridge::context(app).ok()?;
    let raw = pctx.db().kv_get(KV_LAST_ATTEMPT).ok().flatten()?;
    DateTime::parse_from_rfc3339(&raw).ok().map(|d| d.with_timezone(&Utc))
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
    if last_attempt(app).is_some_and(|t| Utc::now() - t < RETRY_AFTER) {
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
    let db = pctx.db().clone();
    // Recorded before anything can fail, so an early error is not retried at the next poll.
    let _ = db.kv_set(KV_LAST_ATTEMPT, &Utc::now().to_rfc3339());
    let policy = policy(app).map_err(|e| (e.message, None))?;
    let mut summary = MaintenanceSummary::default();
    let paths = pctx.docs.paths.clone();
    // `purge_enabled` only switches the daily schedule (`tick`). A job that reached this point was
    // asked for — "지금 파기 실행" must purge with the daily run switched off as well.
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
            // The page text edit history (viewer Ctrl+Z) is undo history too.
            summary.purged.journal +=
                db.with(|c| docs_repo::purge_page_text_history_before(c, &cutoff_utc(days))).unwrap_or(0) as u64;
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
            // 0.7.0: chat sessions not touched within the period (their messages cascade).
            summary.purged.answers += db
                .with(|c| doculens_core::knowledge::chat::purge_sessions_before(c, &cutoff_utc(days)))
                .unwrap_or(0) as u64;
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
            let mut kept_noted = false;
            for d in expired {
                if check(&mut summary) {
                    break;
                }
                // Registered forms and knowledge documents are reference material, not records
                // with a retention period — and so is any document in a reference group, whatever
                // its role says (one project may use a shared regulation as a work document).
                if matches!(d.role, DocumentRole::Form | DocumentRole::Knowledge)
                    || db.with(|c| kb_repo::in_reference_group(c, &d.id)).unwrap_or(true)
                {
                    if !kept_noted {
                        summary.skipped.push("documents:forms_knowledge_kept".into());
                        kept_noted = true;
                    }
                    continue;
                }
                // The file goes first (no recycle bin); the row, cache and extraction mirror follow
                // only once it is gone, so a file that cannot be deleted keeps its row.
                let abs = docs::from_rel_path(&paths.root, &d.rel_path);
                match std::fs::remove_file(&abs) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        summary.skipped.push(format!("document {}: {e}", d.id));
                        continue;
                    }
                }
                match import::delete_document(&deps, &d.id, true) {
                    Ok(_) => removed.push(d.id.clone()),
                    Err(e) => summary.skipped.push(format!("document {}: {e}", d.id)),
                }
            }
            summary.purged.documents += removed.len() as u64;
            if !removed.is_empty() {
                pctx.emit(app, "documents", removed);
            }
        }
        None => summary.skipped.push("documents".into()),
    }
    // 8. Cache size cap: LRU over the per-document folders, by last use (`.last_used` marker, else
    //    render time). The cap is the one in the settings (처리 › 페이지 캐시 상한). `cache/tmp` is
    //    reserved for pre-masking temporaries (nothing writes it yet) and is purged by age in
    //    step 5, not trimmed here.
    let cache = app.state::<AppState>().settings_snapshot().map(|s| s.cache).unwrap_or_default();
    let caps = CacheCaps::from_settings(&cache);
    let pages = retention::trim_lru_dirs(&paths.page_cache_dir(), caps.pages, CACHE_MIN_AGE);
    let thumbs = match caps.thumbs {
        Some(cap) => retention::trim_lru_dirs(&paths.thumb_cache_dir(), cap, CACHE_MIN_AGE),
        None => FsPurge::default(),
    };
    let converted = retention::trim_lru_dirs(&paths.cache_dir().join("converted"), caps.converted, CACHE_MIN_AGE);
    summary.purged.cache_bytes += pages.bytes + thumbs.bytes + converted.bytes;
    finish(app, &db, &paths, summary, started)
}

/// Byte caps of the cache trim, from `settings.cache` — the one page-cache cap of the app (the
/// workspace retention policy carried a second one up to 0.7.x). Rendered pages get 8/10 of it,
/// thumbnails and converted office files 1/10 each; with "keep thumbnails" on (the default,
/// design §3.6) the thumbnails are not trimmed at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CacheCaps {
    pages: u64,
    thumbs: Option<u64>,
    converted: u64,
}

impl CacheCaps {
    fn from_settings(cache: &CacheSettings) -> CacheCaps {
        let gb = if cache.page_cache_max_gb.is_finite() { cache.page_cache_max_gb.clamp(0.5, 1000.0) } else { 5.0 };
        let cap = (f64::from(gb) * 1024.0 * 1024.0 * 1024.0) as u64;
        CacheCaps { pages: cap * 8 / 10, thumbs: (!cache.keep_thumbnails).then_some(cap / 10), converted: cap / 10 }
    }
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
        // The settings screen shows the last completed run; attempts live under `KV_LAST_ATTEMPT`.
        if !summary.cancelled {
            let _ = db.kv_set(KV_LAST_RUN, &raw);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// PLT-10: the purge follows the page-cache cap of the settings (처리 › 페이지 캐시 상한, the
    /// assistant's "캐시 상한 50GB로") and the "keep thumbnails" switch.
    #[test]
    fn cache_caps_follow_the_settings() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let default = CacheCaps::from_settings(&CacheSettings::default());
        assert_eq!(default, CacheCaps { pages: 4 * GIB, thumbs: None, converted: GIB / 2 });
        let big = CacheCaps::from_settings(&CacheSettings { page_cache_max_gb: 50.0, keep_thumbnails: false });
        assert_eq!(big, CacheCaps { pages: 40 * GIB, thumbs: Some(5 * GIB), converted: 5 * GIB });
        // Values a hand-edited file could carry (the loader clamps them as well).
        let tiny = CacheCaps::from_settings(&CacheSettings { page_cache_max_gb: 0.0, keep_thumbnails: true });
        assert_eq!(tiny.pages, GIB / 2 * 8 / 10);
        let nan = CacheCaps::from_settings(&CacheSettings { page_cache_max_gb: f32::NAN, keep_thumbnails: true });
        assert_eq!(nan, default);
    }

    /// PLT-15, the half that can be checked without an app handle: the daily switch still stops
    /// the schedule (no next run). That `run` itself no longer asks for the switch — so "지금 파기
    /// 실행" purges with the daily run off — is not covered by a unit test.
    #[test]
    fn the_daily_switch_stops_the_schedule() {
        let now = Local.with_ymd_and_hms(2026, 11, 11, 10, 0, 0).single().unwrap();
        let on = RetentionPolicy { purge_enabled: true, purge_hour_local: 3, ..Default::default() };
        let off = RetentionPolicy { purge_enabled: false, ..on.clone() };
        assert_eq!(next_run(&on, now).map(|t| t.with_timezone(&Local).hour()), Some(3));
        assert_eq!(next_run(&off, now), None);
    }
}
