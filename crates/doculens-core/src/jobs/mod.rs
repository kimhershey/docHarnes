//! Persistent job runner primitives (design §3.9 scheduling): the queue rows,
//! replayable specs, priorities and lanes, the per-job control (cancel /
//! pause / resume) and the engine gate that lets interactive requests
//! (planner, Q&A) jump in at the next page boundary of a batch.
//!
//! The scheduler thread, event emission and the OS-specific bits live in the
//! shell; everything here is plain Rust so it can be unit-tested.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::generate::DraftRequest;
use crate::organize::OrganizePlan;
use crate::pipeline::ProcessRequest;
use crate::planner::{Plan, PlanContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JobPriority {
    Low,
    #[default]
    Normal,
    High,
    /// Command bar / Q&A: served at the next page boundary.
    Interactive,
}

impl JobPriority {
    pub fn as_i64(self) -> i64 {
        match self {
            JobPriority::Low => 0,
            JobPriority::Normal => 1,
            JobPriority::High => 2,
            JobPriority::Interactive => 3,
        }
    }
    pub fn from_i64(v: i64) -> JobPriority {
        match v {
            i64::MIN..=0 => JobPriority::Low,
            1 => JobPriority::Normal,
            2 => JobPriority::High,
            _ => JobPriority::Interactive,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
    Cancelled,
    /// Was running when the app stopped; `retry` re-queues it.
    Interrupted,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Paused => "paused",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
            JobState::Interrupted => "interrupted",
        }
    }
    pub fn parse(s: &str) -> Option<JobState> {
        [
            JobState::Queued,
            JobState::Running,
            JobState::Paused,
            JobState::Done,
            JobState::Failed,
            JobState::Cancelled,
            JobState::Interrupted,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
    pub fn is_finished(self) -> bool {
        matches!(self, JobState::Done | JobState::Failed | JobState::Cancelled | JobState::Interrupted)
    }
    pub fn is_active(self) -> bool {
        matches!(self, JobState::Queued | JobState::Running | JobState::Paused)
    }
}

/// Replayable description of a job (persisted as JSON in `jobs.spec`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobSpec {
    Pipeline {
        req: ProcessRequest,
    },
    Plan {
        plan: Box<Plan>,
        text: String,
        context: Box<PlanContext>,
    },
    Export {
        req: Value,
    },
    Organize {
        plan: Box<OrganizePlan>,
        label: String,
        source: String,
        batch_kind: String,
    },
    Undo {
        batch_id: Option<String>,
    },
    Redo,
    /// §3.13 document draft (planner `draft_document`, the 문서 생성 screen, §3.15 보완요청서).
    Draft {
        req: Box<DraftRequest>,
        /// Project the draft belongs to (보완요청서), for the result card link.
        project_id: Option<String>,
        /// `generate` | `planner` | `project`.
        source: String,
    },
    /// §3.8 daily retention purge (journal, jobs, conversations, cache LRU, trash, logs, temp).
    Maintenance,
}

impl JobSpec {
    pub fn kind(&self) -> &'static str {
        match self {
            JobSpec::Pipeline { .. } => "pipeline",
            JobSpec::Plan { .. } => "plan",
            JobSpec::Export { .. } => "export",
            JobSpec::Organize { .. } => "organize",
            JobSpec::Undo { .. } => "undo",
            JobSpec::Redo => "redo",
            JobSpec::Draft { .. } => "draft",
            JobSpec::Maintenance => "maintenance",
        }
    }

    /// The lane a job occupies: `Engine` jobs run one at a time (single slot,
    /// §3.9); `Io` jobs may overlap with them.
    pub fn lane(&self) -> Lane {
        match self {
            JobSpec::Pipeline { .. } => Lane::Engine,
            JobSpec::Plan { plan, .. } if plan.needs_model() => Lane::Engine,
            // Drafts call the model (the rule-based 보완요청서 path finishes quickly anyway).
            JobSpec::Draft { .. } => Lane::Engine,
            _ => Lane::Io,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    Engine,
    Io,
}

/// One row of `jobs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub kind: String,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<JobSpec>,
    pub priority: JobPriority,
    pub state: JobState,
    pub progress: Option<f32>,
    pub detail: Option<String>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    pub attempts: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl JobRecord {
    pub fn new(spec: JobSpec, subject: &str, priority: JobPriority) -> JobRecord {
        let now = Utc::now();
        JobRecord {
            id: crate::documents::new_id(),
            kind: spec.kind().into(),
            subject: subject.into(),
            spec: Some(spec),
            priority,
            state: JobState::Queued,
            progress: None,
            detail: None,
            error: None,
            result: None,
            attempts: 0,
            created_at: now,
            updated_at: now,
            started_at: None,
            finished_at: None,
        }
    }
    pub fn lane(&self) -> Lane {
        self.spec.as_ref().map(JobSpec::lane).unwrap_or(Lane::Io)
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

fn ts(t: &DateTime<Utc>) -> String {
    t.to_rfc3339()
}

fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now())
}

const COLUMNS: &str = "id, kind, subject, spec, priority, state, progress, detail, error, result, attempts, created_at, updated_at, started_at, finished_at";

fn row_to_job(r: &Row<'_>) -> rusqlite::Result<JobRecord> {
    let spec: Option<String> = r.get(3)?;
    let result: Option<String> = r.get(9)?;
    Ok(JobRecord {
        id: r.get(0)?,
        kind: r.get(1)?,
        subject: r.get(2)?,
        spec: spec.and_then(|s| serde_json::from_str(&s).ok()),
        priority: JobPriority::from_i64(r.get::<_, i64>(4)?),
        state: JobState::parse(&r.get::<_, String>(5)?).unwrap_or(JobState::Failed),
        progress: r.get::<_, Option<f64>>(6)?.map(|p| p as f32),
        detail: r.get(7)?,
        error: r.get(8)?,
        result: result.and_then(|s| serde_json::from_str(&s).ok()),
        attempts: r.get::<_, i64>(10)?.max(0) as u32,
        created_at: parse_ts(&r.get::<_, String>(11)?),
        updated_at: parse_ts(&r.get::<_, String>(12)?),
        started_at: r.get::<_, Option<String>>(13)?.as_deref().map(parse_ts),
        finished_at: r.get::<_, Option<String>>(14)?.as_deref().map(parse_ts),
    })
}

pub fn insert(conn: &Connection, job: &JobRecord) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO jobs ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
        ),
        params![
            job.id,
            job.kind,
            job.subject,
            job.spec.as_ref().and_then(|s| serde_json::to_string(s).ok()),
            job.priority.as_i64(),
            job.state.as_str(),
            job.progress.map(f64::from),
            job.detail,
            job.error,
            job.result.as_ref().map(Value::to_string),
            job.attempts as i64,
            ts(&job.created_at),
            ts(&job.updated_at),
            job.started_at.as_ref().map(ts),
            job.finished_at.as_ref().map(ts),
        ],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<JobRecord>> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM jobs WHERE id = ?1"), [id], row_to_job).optional()
}

/// Active jobs (queued / running / paused) by priority then age, followed by
/// the most recent `finished_limit` finished ones.
pub fn list(conn: &Connection, finished_limit: u32) -> rusqlite::Result<Vec<JobRecord>> {
    let mut out = Vec::new();
    let mut st = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM jobs WHERE state IN ('queued','running','paused') ORDER BY priority DESC, created_at ASC"
    ))?;
    out.extend(st.query_map([], row_to_job)?.collect::<rusqlite::Result<Vec<_>>>()?);
    let mut st = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM jobs WHERE state NOT IN ('queued','running','paused') ORDER BY finished_at DESC, created_at DESC LIMIT ?1"
    ))?;
    out.extend(st.query_map([finished_limit as i64], row_to_job)?.collect::<rusqlite::Result<Vec<_>>>()?);
    Ok(out)
}

pub fn queued(conn: &Connection) -> rusqlite::Result<Vec<JobRecord>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM jobs WHERE state = 'queued' ORDER BY priority DESC, created_at ASC"
    ))?;
    let rows = st.query_map([], row_to_job)?;
    rows.collect()
}

pub fn set_state(conn: &Connection, id: &str, state: JobState, error: Option<&str>) -> rusqlite::Result<()> {
    let now = ts(&Utc::now());
    match state {
        JobState::Running => conn.execute(
            "UPDATE jobs SET state = ?2, error = NULL, updated_at = ?3, started_at = COALESCE(started_at, ?3), attempts = attempts + 1 WHERE id = ?1",
            params![id, state.as_str(), now],
        )?,
        s if s.is_finished() => conn.execute(
            "UPDATE jobs SET state = ?2, error = ?3, updated_at = ?4, finished_at = ?4 WHERE id = ?1",
            params![id, state.as_str(), error, now],
        )?,
        _ => conn.execute("UPDATE jobs SET state = ?2, updated_at = ?3 WHERE id = ?1", params![id, state.as_str(), now])?,
    };
    Ok(())
}

pub fn set_progress(conn: &Connection, id: &str, progress: Option<f32>, detail: Option<&str>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE jobs SET progress = COALESCE(?2, progress), detail = COALESCE(?3, detail), updated_at = ?4 WHERE id = ?1",
        params![id, progress.map(f64::from), detail, ts(&Utc::now())],
    )?;
    Ok(())
}

pub fn set_result(conn: &Connection, id: &str, result: &Value) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE jobs SET result = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, result.to_string(), ts(&Utc::now())],
    )?;
    Ok(())
}

pub fn set_priority(conn: &Connection, id: &str, priority: JobPriority) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE jobs SET priority = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, priority.as_i64(), ts(&Utc::now())],
    )?;
    Ok(())
}

/// Re-queues a finished job with the same spec (retry). Returns false when the
/// job is unknown, still active or has no spec to replay.
pub fn requeue(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let Some(job) = get(conn, id)? else { return Ok(false) };
    if job.state.is_active() || job.spec.is_none() {
        return Ok(false);
    }
    conn.execute(
        "UPDATE jobs SET state = 'queued', error = NULL, progress = NULL, detail = NULL, result = NULL, finished_at = NULL, started_at = NULL, updated_at = ?2 WHERE id = ?1",
        params![id, ts(&Utc::now())],
    )?;
    Ok(true)
}

/// Jobs that were running when the app stopped become `interrupted`
/// (retryable); paused ones stay paused. Returns how many were marked.
pub fn mark_interrupted(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE jobs SET state = 'interrupted', error = 'interrupted', updated_at = ?1, finished_at = ?1 WHERE state = 'running'",
        [ts(&Utc::now())],
    )
}

/// Deletes finished jobs beyond the newest `keep`.
pub fn prune(conn: &Connection, keep: u32) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM jobs WHERE state NOT IN ('queued','running','paused') AND id NOT IN (
            SELECT id FROM jobs WHERE state NOT IN ('queued','running','paused') ORDER BY finished_at DESC, created_at DESC LIMIT ?1)",
        [keep as i64],
    )
}

/// Deletes finished jobs whose `finished_at` is older than `cutoff` (retention purge, §3.8 job logs).
pub fn purge_finished_before(conn: &Connection, cutoff: &chrono::DateTime<Utc>) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM jobs WHERE state NOT IN ('queued','running','paused') AND finished_at IS NOT NULL AND finished_at < ?1",
        [ts(cutoff)],
    )
}

pub fn clear_finished(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM jobs WHERE state NOT IN ('queued','running','paused')", [])
}

/// Picks the next queued job that can start: highest priority first, oldest
/// first among equals, skipping engine-lane jobs while one is running.
pub fn pick_next(queued: &[JobRecord], engine_busy: bool, io_running: usize, io_limit: usize) -> Option<&JobRecord> {
    let mut candidates: Vec<&JobRecord> = queued.iter().filter(|j| j.state == JobState::Queued).collect();
    candidates.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.created_at.cmp(&b.created_at)));
    candidates.into_iter().find(|j| match j.lane() {
        Lane::Engine => !engine_busy,
        Lane::Io => io_running < io_limit,
    })
}

// ---------------------------------------------------------------------------
// Control: cancel / pause / resume
// ---------------------------------------------------------------------------

/// Shared between the scheduler and the worker running one job.
pub struct JobControl {
    pub cancel: Arc<AtomicBool>,
    paused: Mutex<bool>,
    cv: Condvar,
}

impl JobControl {
    pub fn new(cancel: Arc<AtomicBool>) -> JobControl {
        JobControl { cancel, paused: Mutex::new(false), cv: Condvar::new() }
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cv.notify_all();
    }
    pub fn pause(&self) {
        *self.paused.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }
    pub fn resume(&self) {
        *self.paused.lock().unwrap_or_else(|e| e.into_inner()) = false;
        self.cv.notify_all();
    }
    pub fn is_paused(&self) -> bool {
        *self.paused.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Blocks while paused; `Err(Cancelled)` when cancelled meanwhile.
    pub fn wait_while_paused(&self) -> Result<()> {
        let mut g = self.paused.lock().unwrap_or_else(|e| e.into_inner());
        while *g {
            if self.cancelled() {
                return Err(CoreError::Cancelled);
            }
            g = self.cv.wait_timeout(g, Duration::from_millis(200)).unwrap_or_else(|e| e.into_inner()).0;
        }
        drop(g);
        if self.cancelled() {
            return Err(CoreError::Cancelled);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Engine gate: interactive requests cut in at page boundaries
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct GateState {
    batch_busy: bool,
    waiting: u32,
    interactive_running: u32,
    busy_since: Option<Instant>,
    last_page_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GateStatus {
    /// A batch job is inside a page right now.
    pub batch_busy: bool,
    /// Interactive requests waiting for the next page boundary.
    pub waiting: u32,
    /// Rough wait until the next boundary (last page duration minus elapsed).
    pub estimated_wait_ms: u64,
}

/// Single-slot engine arbitration (§3.9): batch jobs call `checkpoint`
/// between pages; `interactive` runs a request as soon as the batch reaches a
/// boundary (or immediately when no batch is inside a page).
#[derive(Default)]
pub struct EngineGate {
    state: Mutex<GateState>,
    cv: Condvar,
}

impl EngineGate {
    pub fn new() -> EngineGate {
        EngineGate::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A batch job starts using the engine; the returned guard ends the batch when dropped
    /// (also when the worker panics or returns early).
    pub fn batch_begin(&self) -> BatchGuard<'_> {
        self.batch_begin_raw();
        BatchGuard(self)
    }

    /// `batch_begin` without the guard (callers that pair it with `batch_end` themselves).
    pub fn batch_begin_raw(&self) {
        let mut g = self.lock();
        g.batch_busy = true;
        g.busy_since = Some(Instant::now());
    }

    /// A batch job is done with the engine.
    pub fn batch_end(&self) {
        let mut g = self.lock();
        g.batch_busy = false;
        g.busy_since = None;
        self.cv.notify_all();
    }

    /// Page boundary of a batch job: lets waiting interactive requests run,
    /// honours pause, returns `Err(Cancelled)` on cancel.
    pub fn checkpoint(&self, ctl: &JobControl) -> Result<()> {
        {
            let mut g = self.lock();
            if let Some(since) = g.busy_since {
                g.last_page_ms = since.elapsed().as_millis() as u64;
            }
            g.batch_busy = false;
            g.busy_since = None;
            self.cv.notify_all();
            loop {
                if ctl.cancelled() {
                    return Err(CoreError::Cancelled);
                }
                if !ctl.is_paused() && g.waiting == 0 && g.interactive_running == 0 {
                    break;
                }
                g = self.cv.wait_timeout(g, Duration::from_millis(50)).unwrap_or_else(|e| e.into_inner()).0;
            }
            g.batch_busy = true;
            g.busy_since = Some(Instant::now());
        }
        Ok(())
    }

    /// Runs an interactive request at the next page boundary.
    pub fn interactive<T>(&self, f: impl FnOnce() -> T) -> T {
        match self.interactive_cancellable(None, f) {
            Ok(v) => v,
            Err(_) => unreachable!("no cancel flag"),
        }
    }

    /// `interactive` that gives up with `Cancelled` while still waiting when `cancel` is set.
    /// The counters are restored by guards, so a panic inside `f` cannot wedge the gate.
    pub fn interactive_cancellable<T>(&self, cancel: Option<&AtomicBool>, f: impl FnOnce() -> T) -> Result<T> {
        {
            let mut g = self.lock();
            g.waiting += 1;
            self.cv.notify_all();
            while g.batch_busy {
                if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                    g.waiting -= 1;
                    self.cv.notify_all();
                    return Err(CoreError::Cancelled);
                }
                g = self.cv.wait_timeout(g, Duration::from_millis(100)).unwrap_or_else(|e| e.into_inner()).0;
            }
            g.waiting -= 1;
            g.interactive_running += 1;
        }
        let _guard = InteractiveGuard(self);
        Ok(f())
    }

    pub fn status(&self) -> GateStatus {
        let g = self.lock();
        let elapsed = g.busy_since.map(|s| s.elapsed().as_millis() as u64).unwrap_or(0);
        GateStatus {
            batch_busy: g.batch_busy,
            waiting: g.waiting,
            estimated_wait_ms: if g.batch_busy { g.last_page_ms.saturating_sub(elapsed) } else { 0 },
        }
    }
}

/// Ends a batch section when dropped.
pub struct BatchGuard<'a>(&'a EngineGate);

impl Drop for BatchGuard<'_> {
    fn drop(&mut self) {
        self.0.batch_end();
    }
}

struct InteractiveGuard<'a>(&'a EngineGate);

impl Drop for InteractiveGuard<'_> {
    fn drop(&mut self) {
        let mut g = self.0.lock();
        g.interactive_running = g.interactive_running.saturating_sub(1);
        self.0.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::planner::{Op, OpKind, Scope};
    use std::sync::atomic::AtomicU32;
    use std::thread;

    fn spec_plan(needs_model: bool) -> JobSpec {
        let op = if needs_model {
            Op::Classify { doc_types: Vec::new(), force: None }
        } else {
            Op::Tag { add: vec!["x".into()], remove: Vec::new() }
        };
        JobSpec::Plan {
            plan: Box::new(Plan {
                intent: OpKind::Tag,
                scope: Scope::default(),
                operations: vec![op],
                requires_confirmation: false,
                confidence: 0.9,
                clarification_question: None,
                summary: None,
            }),
            text: "t".into(),
            context: Box::new(PlanContext::default()),
        }
    }

    #[test]
    fn store_round_trip_priority_order_and_retry() {
        let db = Db::open_in_memory().unwrap();
        let a = JobRecord::new(
            JobSpec::Pipeline { req: ProcessRequest { document_ids: vec!["d".into()], ..Default::default() } },
            "1 documents",
            JobPriority::Normal,
        );
        let mut b = JobRecord::new(spec_plan(false), "plan", JobPriority::High);
        b.created_at = a.created_at + chrono::Duration::seconds(1);
        let c = JobRecord::new(spec_plan(true), "plan2", JobPriority::Normal);
        db.with(|conn| {
            insert(conn, &a)?;
            insert(conn, &b)?;
            insert(conn, &c)
        })
        .unwrap();
        let q = db.with(queued).unwrap();
        assert_eq!(
            q.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(),
            vec![b.id.as_str(), a.id.as_str(), c.id.as_str()]
        );
        // Engine busy → the io-lane plan job goes first; then the engine jobs in order.
        assert_eq!(pick_next(&q, true, 0, 2).map(|j| j.id.as_str()), Some(b.id.as_str()));
        assert_eq!(pick_next(&q[1..], true, 0, 2), None);
        assert_eq!(pick_next(&q, false, 0, 2).map(|j| j.id.as_str()), Some(b.id.as_str()));
        assert_eq!(pick_next(&q, false, 2, 2).map(|j| j.id.as_str()), Some(a.id.as_str()));
        // running → interrupted on restart → retry re-queues with a fresh attempt
        db.with(|conn| set_state(conn, &a.id, JobState::Running, None)).unwrap();
        assert_eq!(db.with(|conn| get(conn, &a.id)).unwrap().unwrap().attempts, 1);
        assert_eq!(db.with(mark_interrupted).unwrap(), 1);
        let j = db.with(|conn| get(conn, &a.id)).unwrap().unwrap();
        assert_eq!(j.state, JobState::Interrupted);
        assert!(j.finished_at.is_some());
        assert!(db.with(|conn| requeue(conn, &a.id)).unwrap());
        assert!(!db.with(|conn| requeue(conn, &a.id)).unwrap(), "active jobs are not re-queued");
        db.with(|conn| set_state(conn, &a.id, JobState::Running, None)).unwrap();
        db.with(|conn| set_progress(conn, &a.id, Some(0.5), Some("ocr (1/2)"))).unwrap();
        db.with(|conn| set_result(conn, &a.id, &serde_json::json!({ "ok": true }))).unwrap();
        db.with(|conn| set_state(conn, &a.id, JobState::Done, None)).unwrap();
        let j = db.with(|conn| get(conn, &a.id)).unwrap().unwrap();
        assert_eq!((j.attempts, j.progress, j.state), (2, Some(0.5), JobState::Done));
        assert_eq!(j.result.unwrap()["ok"], true);
        assert!(j.spec.is_some());
        let all = db.with(|conn| list(conn, 10)).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].id, b.id);
        db.with(|conn| set_priority(conn, &c.id, JobPriority::Interactive)).unwrap();
        assert_eq!(db.with(queued).unwrap()[0].id, c.id);
        assert_eq!(db.with(|conn| prune(conn, 0)).unwrap(), 1);
        assert_eq!(db.with(clear_finished).unwrap(), 0);
    }

    #[test]
    fn gate_lets_interactive_requests_in_at_page_boundaries_and_honours_pause_and_cancel() {
        let gate = Arc::new(EngineGate::new());
        let ctl = Arc::new(JobControl::new(Arc::new(AtomicBool::new(false))));
        let pages_done = Arc::new(AtomicU32::new(0));
        let g2 = gate.clone();
        let c2 = ctl.clone();
        let p2 = pages_done.clone();
        let batch = thread::spawn(move || {
            let _lease = g2.batch_begin();
            for _ in 0..50 {
                thread::sleep(Duration::from_millis(5)); // "a page"
                p2.fetch_add(1, Ordering::SeqCst);
                if g2.checkpoint(&c2).is_err() {
                    return "cancelled";
                }
            }
            "done"
        });
        // An interactive request gets served while the batch keeps going.
        let wait_until = |pred: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !pred() {
                assert!(Instant::now() < deadline, "timed out waiting for the batch");
                thread::sleep(Duration::from_millis(2));
            }
        };
        wait_until(&|| pages_done.load(Ordering::SeqCst) >= 2);
        let served_at = gate.interactive(|| pages_done.load(Ordering::SeqCst));
        assert!((1..50).contains(&served_at), "served at page {served_at}");
        assert!(gate.status().waiting == 0);
        // Pause stops the batch at the next boundary (it parks inside `checkpoint`); resume continues it.
        ctl.pause();
        wait_until(&|| !gate.status().batch_busy);
        let at_pause = pages_done.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(60));
        assert_eq!(pages_done.load(Ordering::SeqCst), at_pause, "no pages while paused");
        ctl.resume();
        wait_until(&|| pages_done.load(Ordering::SeqCst) > at_pause);
        // Cancel while paused ends the job.
        ctl.pause();
        thread::sleep(Duration::from_millis(20));
        ctl.request_cancel();
        assert_eq!(batch.join().unwrap(), "cancelled");
        assert!(!gate.status().batch_busy);
        // Without a batch, interactive runs immediately.
        assert_eq!(gate.interactive(|| 7), 7);
        assert!(ctl.wait_while_paused().is_err());
    }

    #[test]
    fn gate_survives_panics_and_cancels_waiting_requests() {
        let gate = EngineGate::new();
        // A panic inside an interactive closure must not leave `interactive_running` behind.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gate.interactive(|| panic!("boom"))));
        assert!(r.is_err());
        let ctl = JobControl::new(Arc::new(AtomicBool::new(false)));
        let guard = gate.batch_begin();
        assert!(gate.status().batch_busy);
        // A checkpoint with no waiters returns at once (the counter was restored by the guard).
        assert!(gate.checkpoint(&ctl).is_ok());
        // A dropped batch guard (panic / early return) frees the gate.
        drop(guard);
        assert!(!gate.status().batch_busy);
        // Waiting interactive requests can be cancelled.
        let _busy = gate.batch_begin();
        let cancel = AtomicBool::new(true);
        assert!(matches!(gate.interactive_cancellable(Some(&cancel), || 1), Err(CoreError::Cancelled)));
        assert_eq!(gate.status().waiting, 0);
    }
}
