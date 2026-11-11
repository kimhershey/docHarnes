//! Job book + persistent runner (design §3.9 scheduling).
//!
//! Two kinds of jobs share one list and one `jobs://update` event:
//! * lightweight shell jobs (engine start, model load / import / download /
//!   hash, document import / rescan / render, knowledge index / embed, split /
//!   merge) — in-memory only, started directly by their bridge;
//! * persistent jobs with a replayable spec (`doculens_core::jobs::JobSpec`:
//!   pipeline processing, plan execution, export, organise, undo / redo) —
//!   stored in the workspace database, scheduled by priority through lanes
//!   (one engine-bound job at a time), pausable, retryable, and restored when
//!   the workspace opens again.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use doculens_core::db::Db;
use doculens_core::jobs::JobState as CoreState;
use doculens_core::jobs::{self as core_jobs, JobControl, JobPriority, JobRecord, JobSpec, Lane};

use crate::state::{AppState, UiError, UiResult};

pub const JOBS_EVENT: &str = "jobs://update";
const MAX_FINISHED: usize = 50;
/// Overlapping I/O-lane persistent jobs (exports, organise …).
const IO_LIMIT: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    EngineStart,
    ModelLoad,
    ModelImport,
    ModelDownload,
    ModelHash,
    DocumentImport,
    DocumentRescan,
    DocumentRender,
    KnowledgeIndex,
    KnowledgeEmbed,
    /// Classification / OCR / extraction over a set of documents (P4).
    Pipeline,
    DocumentSplit,
    DocumentMerge,
    /// Natural-language plan execution (P5).
    Plan,
    Export,
    Organize,
    Undo,
    Redo,
    /// Document draft generation (P6 §3.13).
    Draft,
    /// Daily retention purge (P7 §3.8).
    Maintenance,
    /// Form field discovery over sample documents (0.3.0 §2).
    SchemaDiscovery,
}

impl JobKind {
    fn from_str(s: &str) -> JobKind {
        match s {
            "plan" => JobKind::Plan,
            "export" => JobKind::Export,
            "organize" => JobKind::Organize,
            "undo" => JobKind::Undo,
            "redo" => JobKind::Redo,
            "draft" => JobKind::Draft,
            "maintenance" => JobKind::Maintenance,
            "schema_discovery" => JobKind::SchemaDiscovery,
            _ => JobKind::Pipeline,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
    Cancelled,
    /// Was running when the app stopped (retryable).
    Interrupted,
}

impl JobState {
    pub fn is_finished(self) -> bool {
        matches!(self, JobState::Done | JobState::Failed | JobState::Cancelled | JobState::Interrupted)
    }
    fn from_core(s: CoreState) -> JobState {
        match s {
            CoreState::Queued => JobState::Queued,
            CoreState::Running => JobState::Running,
            CoreState::Paused => JobState::Paused,
            CoreState::Done => JobState::Done,
            CoreState::Failed => JobState::Failed,
            CoreState::Cancelled => JobState::Cancelled,
            CoreState::Interrupted => JobState::Interrupted,
        }
    }
    fn to_core(self) -> CoreState {
        match self {
            JobState::Queued => CoreState::Queued,
            JobState::Running => CoreState::Running,
            JobState::Paused => CoreState::Paused,
            JobState::Done => CoreState::Done,
            JobState::Failed => CoreState::Failed,
            JobState::Cancelled => CoreState::Cancelled,
            JobState::Interrupted => CoreState::Interrupted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub kind: JobKind,
    /// Model id / folder the job works on (for UI grouping).
    pub subject: String,
    pub state: JobState,
    /// 0..1 when known.
    pub progress: Option<f32>,
    pub detail: Option<String>,
    pub bytes_done: Option<u64>,
    pub bytes_total: Option<u64>,
    pub error: Option<String>,
    pub cancel_requested: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    // -- persistent runner (P5) --
    pub priority: JobPriority,
    /// Stored in the workspace database; survives restarts, can be paused / retried.
    pub persistent: bool,
    pub attempts: u32,
    /// Structured outcome (plan execution, export path …) once finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Human-readable label of the request (plan text, export target …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Outcome of a persistent job's worker: `Ok((detail, result))` or
/// `Err((error, partial result))`.
pub type WorkerResult = std::result::Result<(Option<String>, Option<Value>), (String, Option<Value>)>;

/// What a persistent job's worker does; installed by `planner_bridge`.
pub type Worker = dyn Fn(&AppHandle, &Job, &JobSpec, &JobControl) -> WorkerResult + Send + Sync;

#[derive(Default)]
pub struct JobBook {
    jobs: Mutex<Vec<Job>>,
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Specs + controls of persistent jobs known to this process.
    specs: Mutex<HashMap<String, JobSpec>>,
    controls: Mutex<HashMap<String, Arc<JobControl>>>,
    app: OnceLock<AppHandle>,
    worker: OnceLock<Arc<Worker>>,
    /// Serialises `create_if_idle` (check + create) and scheduling decisions.
    create_gate: Mutex<()>,
}

impl JobBook {
    pub fn attach(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    /// Installs the persistent-job worker (once, at setup).
    pub fn set_worker(&self, worker: Arc<Worker>) {
        let _ = self.worker.set(worker);
    }

    /// Broadcasts the list without the structured results (those can be large and are served by
    /// `jobs_list` / `jobs_get`).
    fn emit(&self) {
        if let (Some(app), Ok(jobs)) = (self.app.get(), self.jobs.lock()) {
            let stripped: Vec<Job> = jobs
                .iter()
                .cloned()
                .map(|mut j| {
                    j.result = None;
                    j
                })
                .collect();
            let _ = app.emit(JOBS_EVENT, stripped);
        }
    }

    pub fn list(&self) -> Vec<Job> {
        self.jobs.lock().map(|j| j.clone()).unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.jobs.lock().ok()?.iter().find(|j| j.id == id).cloned()
    }

    /// True when any job of one of these kinds is still queued/running.
    pub fn is_active_kind(&self, kinds: &[JobKind]) -> bool {
        self.jobs.lock().map(|j| j.iter().any(|j| kinds.contains(&j.kind) && !j.state.is_finished())).unwrap_or(false)
    }

    /// Requests cancellation of every active job of these kinds; returns their ids.
    pub fn cancel_kinds(&self, kinds: &[JobKind]) -> Vec<String> {
        let ids: Vec<String> = self
            .jobs
            .lock()
            .map(|j| {
                j.iter().filter(|j| kinds.contains(&j.kind) && !j.state.is_finished()).map(|j| j.id.clone()).collect()
            })
            .unwrap_or_default();
        for id in &ids {
            self.cancel(id);
        }
        ids
    }

    /// True when a job of this kind is still queued/running for `subject`.
    pub fn is_active(&self, kind: JobKind, subject: &str) -> bool {
        self.jobs
            .lock()
            .map(|j| j.iter().any(|j| j.kind == kind && j.subject == subject && !j.state.is_finished()))
            .unwrap_or(false)
    }

    /// Creates a job unless one of `kind` is still active — the check and the
    /// creation happen under one gate so two threads cannot both pass.
    pub fn create_if_idle(&self, kind: JobKind, subject: &str) -> Option<(String, Arc<AtomicBool>)> {
        let _gate = self.create_gate.lock().unwrap_or_else(|e| e.into_inner());
        if self.is_active_kind(&[kind]) {
            return None;
        }
        Some(self.create(kind, subject))
    }

    fn push(&self, job: Job) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        let id = job.id.clone();
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.retain(|j| j.id != id);
            jobs.push(job);
            // Trim the oldest finished jobs beyond the cap.
            let finished: Vec<usize> =
                jobs.iter().enumerate().filter(|(_, j)| j.state.is_finished()).map(|(i, _)| i).collect();
            if finished.len() > MAX_FINISHED {
                let overflow = finished.len() - MAX_FINISHED;
                for i in finished.into_iter().take(overflow).rev() {
                    jobs.remove(i);
                }
            }
        }
        if let Ok(mut c) = self.cancels.lock() {
            c.insert(id, flag.clone());
        }
        flag
    }

    pub fn create(&self, kind: JobKind, subject: &str) -> (String, Arc<AtomicBool>) {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let now = Utc::now();
        let flag = self.push(Job {
            id: id.clone(),
            kind,
            subject: subject.to_string(),
            state: JobState::Queued,
            progress: None,
            detail: None,
            bytes_done: None,
            bytes_total: None,
            error: None,
            cancel_requested: false,
            created_at: now,
            updated_at: now,
            finished_at: None,
            priority: JobPriority::Normal,
            persistent: false,
            attempts: 1,
            result: None,
            label: None,
        });
        self.emit();
        (id, flag)
    }

    pub fn update(&self, id: &str, f: impl FnOnce(&mut Job)) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(j) = jobs.iter_mut().find(|j| j.id == id) {
                f(j);
                j.updated_at = Utc::now();
                if j.state == JobState::Queued && !j.persistent {
                    j.state = JobState::Running;
                }
            }
        }
        self.emit();
    }

    pub fn progress(
        &self,
        id: &str,
        progress: Option<f32>,
        bytes_done: Option<u64>,
        bytes_total: Option<u64>,
        detail: Option<String>,
    ) {
        let persistent = self.get(id).is_some_and(|j| j.persistent);
        self.update(id, |j| {
            j.progress = progress;
            if bytes_done.is_some() {
                j.bytes_done = bytes_done;
            }
            if bytes_total.is_some() {
                j.bytes_total = bytes_total;
            }
            if detail.is_some() {
                j.detail = detail.clone();
            }
        });
        if persistent {
            if let Some(db) = self.db() {
                let _ = db.with(|c| core_jobs::set_progress(c, id, progress, detail.as_deref()));
            }
        }
    }

    /// Attaches a structured outcome to a (lightweight) job before it finishes; `jobs_get` /
    /// `jobs_list` deliver it, the `jobs://update` broadcast omits it.
    pub fn set_result(&self, id: &str, result: Option<Value>) {
        self.update(id, |j| j.result = result);
    }

    pub fn finish(&self, id: &str, result: Result<Option<String>, String>) {
        let cancelled = self.cancel_flag(id).map(|f| f.load(Ordering::Relaxed)).unwrap_or(false);
        self.update(id, |j| {
            j.finished_at = Some(Utc::now());
            match result {
                Ok(detail) => {
                    j.state = JobState::Done;
                    j.progress = Some(1.0);
                    if detail.is_some() {
                        j.detail = detail;
                    }
                }
                Err(e) if cancelled || e.contains("cancelled") => {
                    j.state = JobState::Cancelled;
                    j.error = None;
                }
                Err(e) => {
                    j.state = JobState::Failed;
                    j.error = Some(e);
                }
            }
        });
        if let Ok(mut c) = self.cancels.lock() {
            c.remove(id);
        }
        if let (Some(app), Some(job)) = (self.app.get(), self.get(id)) {
            crate::notify::job_finished(app, &job);
        }
    }

    pub fn cancel_flag(&self, id: &str) -> Option<Arc<AtomicBool>> {
        self.cancels.lock().ok()?.get(id).cloned()
    }

    pub fn cancel(&self, id: &str) -> bool {
        let Some(job) = self.get(id) else { return false };
        if job.state.is_finished() {
            return false;
        }
        if job.persistent && matches!(job.state, JobState::Queued | JobState::Paused) && self.control(id).is_none() {
            // Not started: finish it right away.
            self.set_persistent_state(id, JobState::Cancelled, None);
            if let Ok(mut c) = self.cancels.lock() {
                c.remove(id);
            }
            self.schedule();
            return true;
        }
        let Some(flag) = self.cancel_flag(id) else { return false };
        flag.store(true, Ordering::Relaxed);
        if let Some(ctl) = self.control(id) {
            ctl.request_cancel();
        }
        self.update(id, |j| j.cancel_requested = true);
        true
    }

    pub fn clear_finished(&self) {
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.retain(|j| !j.state.is_finished());
        }
        if let Some(db) = self.db() {
            let _ = db.with(core_jobs::clear_finished);
        }
        self.emit();
    }

    // -----------------------------------------------------------------------
    // Persistent runner
    // -----------------------------------------------------------------------

    fn db(&self) -> Option<Arc<Db>> {
        let app = self.app.get()?;
        app.state::<AppState>().workspace.lock().ok()?.as_ref().map(|ws| ws.db.clone())
    }

    fn control(&self, id: &str) -> Option<Arc<JobControl>> {
        self.controls.lock().ok()?.get(id).cloned()
    }

    pub fn spec(&self, id: &str) -> Option<JobSpec> {
        self.specs.lock().ok()?.get(id).cloned()
    }

    fn set_persistent_state(&self, id: &str, state: JobState, error: Option<&str>) {
        self.update(id, |j| {
            j.state = state;
            if state.is_finished() {
                j.finished_at = Some(Utc::now());
                j.error = error.map(str::to_string);
            }
        });
        if let Some(db) = self.db() {
            let _ = db.with(|c| core_jobs::set_state(c, id, state.to_core(), error));
        }
    }

    fn view(record: &JobRecord) -> Job {
        Job {
            id: record.id.clone(),
            kind: JobKind::from_str(&record.kind),
            subject: record.subject.clone(),
            state: JobState::from_core(record.state),
            progress: record.progress,
            detail: record.detail.clone(),
            bytes_done: None,
            bytes_total: None,
            error: record.error.clone(),
            cancel_requested: false,
            created_at: record.created_at,
            updated_at: record.updated_at,
            finished_at: record.finished_at,
            priority: record.priority,
            persistent: true,
            attempts: record.attempts,
            result: record.result.clone(),
            label: match &record.spec {
                Some(JobSpec::Plan { text, .. }) => Some(text.clone()),
                Some(JobSpec::Organize { label, .. }) => Some(label.clone()),
                _ => None,
            },
        }
    }

    /// Queues a persistent job (stored in the workspace database) and schedules it.
    pub fn submit(&self, spec: JobSpec, subject: &str, priority: JobPriority) -> UiResult<String> {
        let db = self.db().ok_or(UiError::from(doculens_core::CoreError::NoWorkspace))?;
        let record = JobRecord::new(spec.clone(), subject, priority);
        db.with(|c| core_jobs::insert(c, &record))?;
        let id = record.id.clone();
        if let Ok(mut s) = self.specs.lock() {
            s.insert(id.clone(), spec);
        }
        self.push(Self::view(&record));
        self.emit();
        self.schedule();
        Ok(id)
    }

    /// Starts the next runnable queued jobs (priority first, lanes respected).
    pub fn schedule(&self) {
        let Some(app) = self.app.get() else { return };
        if app.state::<AppState>().switching.load(Ordering::SeqCst) {
            return;
        }
        let _gate = self.create_gate.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let next = {
                let Ok(jobs) = self.jobs.lock() else { return };
                let Ok(specs) = self.specs.lock() else { return };
                let Ok(controls) = self.controls.lock() else { return };
                next_to_start(&jobs, &|j| specs.get(&j.id).map(JobSpec::lane).unwrap_or(Lane::Io), &|id| {
                    controls.contains_key(id)
                })
            };
            let Some(id) = next else { return };
            self.start(app, &id);
        }
    }

    fn start(&self, app: &AppHandle, id: &str) {
        if app.state::<AppState>().switching.load(Ordering::SeqCst) {
            return; // the workspace is closing: the job stays queued in its database
        }
        let Some(spec) = self.spec(id) else {
            self.set_persistent_state(id, JobState::Failed, Some("spec missing"));
            return;
        };
        let Some(worker) = self.worker.get().cloned() else {
            self.set_persistent_state(id, JobState::Failed, Some("no worker"));
            return;
        };
        let cancel = self.cancel_flag(id).unwrap_or_else(|| {
            let f = Arc::new(AtomicBool::new(false));
            if let Ok(mut c) = self.cancels.lock() {
                c.insert(id.to_string(), f.clone());
            }
            f
        });
        let ctl = Arc::new(JobControl::new(cancel));
        if let Ok(mut c) = self.controls.lock() {
            c.insert(id.to_string(), ctl.clone());
        }
        self.update(id, |j| {
            j.state = JobState::Running;
            j.attempts += 1;
            j.error = None;
        });
        if let Some(db) = self.db() {
            let _ = db.with(|c| core_jobs::set_state(c, id, CoreState::Running, None));
        }
        let Some(job) = self.get(id) else { return };
        let app = app.clone();
        let id = id.to_string();
        std::thread::Builder::new()
            .name(format!("doculens-job-{}", job.kind_name()))
            .spawn(move || {
                let state = app.state::<AppState>();
                let outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker(&app, &job, &spec, &ctl)));
                let result: WorkerResult = match outcome {
                    Ok(r) => r,
                    Err(p) => {
                        let msg = p
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                            .unwrap_or_else(|| "panic".into());
                        log::error!("job {id} panicked: {msg}");
                        Err((format!("internal error: {msg}"), None))
                    }
                };
                let cancelled = ctl.cancelled();
                let (state_, error, detail, payload) = match result {
                    Ok((detail, payload)) => (JobState::Done, None, detail, payload),
                    Err((e, payload)) if cancelled || e.contains("cancelled") => {
                        (JobState::Cancelled, None, None, payload)
                    }
                    Err((e, payload)) => (JobState::Failed, Some(e), None, payload),
                };
                if let Some(db) = state.jobs.db() {
                    if let Some(p) = payload.as_ref() {
                        let _ = db.with(|c| core_jobs::set_result(c, &id, p));
                    }
                    if let Some(d) = detail.as_deref() {
                        let _ = db.with(|c| core_jobs::set_progress(c, &id, Some(1.0), Some(d)));
                    }
                    let _ = db.with(|c| core_jobs::set_state(c, &id, state_.to_core(), error.as_deref()));
                }
                state.jobs.update(&id, |j| {
                    j.state = state_;
                    j.finished_at = Some(Utc::now());
                    j.error = error.clone();
                    if state_ == JobState::Done {
                        j.progress = Some(1.0);
                    }
                    if detail.is_some() {
                        j.detail = detail.clone();
                    }
                    j.result = payload.clone();
                });
                if let Ok(mut c) = state.jobs.controls.lock() {
                    c.remove(&id);
                }
                if let Ok(mut c) = state.jobs.cancels.lock() {
                    c.remove(&id);
                }
                if let Some(job) = state.jobs.get(&id) {
                    crate::notify::job_finished(&app, &job);
                }
                state.jobs.schedule();
            })
            .ok();
    }

    pub fn pause(&self, id: &str) -> bool {
        let Some(job) = self.get(id) else { return false };
        if !job.persistent {
            return false;
        }
        match job.state {
            JobState::Running => {
                if let Some(ctl) = self.control(id) {
                    ctl.pause();
                }
                self.set_persistent_state(id, JobState::Paused, None);
                true
            }
            JobState::Queued => {
                self.set_persistent_state(id, JobState::Paused, None);
                true
            }
            _ => false,
        }
    }

    pub fn resume(&self, id: &str) -> bool {
        let Some(job) = self.get(id) else { return false };
        if !job.persistent || job.state != JobState::Paused {
            return false;
        }
        match self.control(id) {
            Some(ctl) => {
                ctl.resume();
                self.set_persistent_state(id, JobState::Running, None);
            }
            None => {
                self.set_persistent_state(id, JobState::Queued, None);
                self.schedule();
            }
        }
        true
    }

    /// Re-queues a finished persistent job with the same spec.
    pub fn retry(&self, id: &str) -> UiResult<bool> {
        let Some(job) = self.get(id) else { return Ok(false) };
        if !job.persistent || !job.state.is_finished() {
            return Ok(false);
        }
        let db = self.db().ok_or(UiError::from(doculens_core::CoreError::NoWorkspace))?;
        if !db.with(|c| core_jobs::requeue(c, id))? {
            return Ok(false);
        }
        if self.spec(id).is_none() {
            if let Some(rec) = db.with(|c| core_jobs::get(c, id))? {
                if let (Some(spec), Ok(mut s)) = (rec.spec, self.specs.lock()) {
                    s.insert(id.to_string(), spec);
                }
            }
        }
        self.update(id, |j| {
            j.state = JobState::Queued;
            j.error = None;
            j.progress = None;
            j.detail = None;
            j.result = None;
            j.finished_at = None;
            j.cancel_requested = false;
        });
        self.schedule();
        Ok(true)
    }

    /// Moves a queued job to the front of its lane.
    pub fn prioritize(&self, id: &str) -> bool {
        let Some(job) = self.get(id) else { return false };
        if !job.persistent || job.state != JobState::Queued {
            return false;
        }
        self.update(id, |j| j.priority = JobPriority::High);
        if let Some(db) = self.db() {
            let _ = db.with(|c| core_jobs::set_priority(c, id, JobPriority::High));
        }
        self.schedule();
        true
    }

    /// Workspace opened: queued / paused jobs of the database come back, jobs
    /// that were running when the app last stopped become `interrupted`.
    pub fn restore_from_workspace(&self) {
        let Some(db) = self.db() else { return };
        let restored = db.with(|c| {
            let n = core_jobs::mark_interrupted(c)?;
            let _ = core_jobs::prune(c, MAX_FINISHED as u32)?;
            let list = core_jobs::list(c, MAX_FINISHED as u32)?;
            Ok((n, list))
        });
        let Ok((interrupted, list)) = restored else { return };
        if interrupted > 0 {
            log::info!("jobs: {interrupted} job(s) interrupted by the previous shutdown");
        }
        for rec in restore_order(&list) {
            if let (Some(spec), Ok(mut s)) = (rec.spec.clone(), self.specs.lock()) {
                s.insert(rec.id.clone(), spec);
            }
            self.push(Self::view(rec));
        }
        self.emit();
        self.schedule();
    }

    /// Workspace closing: running persistent jobs are cancelled and marked
    /// `interrupted` (retryable), queued ones stay queued in the database; all
    /// of them leave the in-memory list.
    pub fn detach_from_workspace(&self) {
        let ids: Vec<(String, JobState)> = self
            .jobs
            .lock()
            .map(|j| j.iter().filter(|j| j.persistent).map(|j| (j.id.clone(), j.state)).collect())
            .unwrap_or_default();
        for (id, state) in &ids {
            if matches!(state, JobState::Running | JobState::Paused) {
                if let Some(ctl) = self.control(id) {
                    ctl.request_cancel();
                }
                if let Some(f) = self.cancel_flag(id) {
                    f.store(true, Ordering::Relaxed);
                }
                if let Some(db) = self.db() {
                    let _ = db.with(|c| core_jobs::set_state(c, id, CoreState::Interrupted, Some("workspace closed")));
                }
            }
        }
        // Let running workers notice the cancel and leave the old database before it closes.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.controls.lock().map(|c| !c.is_empty()).unwrap_or(false) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if self.controls.lock().map(|c| !c.is_empty()).unwrap_or(false) {
            log::warn!("persistent jobs did not stop within 5 s; they stop at their next checkpoint");
        }
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.retain(|j| !j.persistent);
        }
        if let Ok(mut s) = self.specs.lock() {
            s.clear();
        }
        self.emit();
    }

    pub fn persistent_queued(&self) -> usize {
        self.jobs.lock().map(|j| j.iter().filter(|j| j.persistent && j.state == JobState::Queued).count()).unwrap_or(0)
    }
}

/// The queued persistent job to start next: highest priority first, oldest first among equals,
/// one engine-lane job at a time and at most `IO_LIMIT` I/O-lane jobs. A lane is held by running
/// jobs and by paused jobs that have a worker (`has_worker`: paused while running, they wait
/// inside the job). A job paused before it ever started — or restored paused after a restart —
/// has no worker and holds nothing; counting it kept every later job queued for ever.
fn next_to_start(jobs: &[Job], lane_of: &dyn Fn(&Job) -> Lane, has_worker: &dyn Fn(&str) -> bool) -> Option<String> {
    let holds_lane = |j: &Job| j.state == JobState::Running || (j.state == JobState::Paused && has_worker(&j.id));
    let engine_busy = jobs.iter().any(|j| {
        holds_lane(j)
            && ((j.persistent && lane_of(j) == Lane::Engine) || (!j.persistent && j.kind == JobKind::Pipeline))
    });
    let io_running = jobs.iter().filter(|j| j.persistent && holds_lane(j) && lane_of(j) == Lane::Io).count();
    let mut queued: Vec<&Job> = jobs.iter().filter(|j| j.persistent && j.state == JobState::Queued).collect();
    queued.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.created_at.cmp(&b.created_at)));
    queued
        .into_iter()
        .find(|j| match lane_of(j) {
            Lane::Engine => !engine_busy,
            Lane::Io => io_running < IO_LIMIT,
        })
        .map(|j| j.id.clone())
}

/// Order in which the jobs of a workspace database enter the in-memory list. `core_jobs::list`
/// returns the active jobs followed by the finished ones newest first; the list in memory is
/// oldest first (new jobs are appended, the finished tabs show it reversed, and `push` trims
/// the oldest finished entries from the front). So: finished jobs by finish time, then the
/// active ones in their scheduling order.
fn restore_order(list: &[JobRecord]) -> Vec<&JobRecord> {
    let (mut finished, active): (Vec<&JobRecord>, Vec<&JobRecord>) = list.iter().partition(|r| r.state.is_finished());
    finished.sort_by(|a, b| a.finished_at.cmp(&b.finished_at).then(a.created_at.cmp(&b.created_at)));
    finished.into_iter().chain(active).collect()
}

impl Job {
    fn kind_name(&self) -> &'static str {
        match self.kind {
            JobKind::Pipeline => "pipeline",
            JobKind::Plan => "plan",
            JobKind::Export => "export",
            JobKind::Organize => "organize",
            JobKind::Undo => "undo",
            JobKind::Redo => "redo",
            JobKind::Draft => "draft",
            JobKind::Maintenance => "maintenance",
            _ => "job",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, kind: JobKind, state: JobState, persistent: bool, minute: i64) -> Job {
        let at = DateTime::parse_from_rfc3339("2026-11-11T09:00:00Z").unwrap().with_timezone(&Utc)
            + chrono::Duration::minutes(minute);
        Job {
            id: id.into(),
            kind,
            subject: String::new(),
            state,
            progress: None,
            detail: None,
            bytes_done: None,
            bytes_total: None,
            error: None,
            cancel_requested: false,
            created_at: at,
            updated_at: at,
            finished_at: None,
            priority: JobPriority::Normal,
            persistent,
            attempts: 0,
            result: None,
            label: None,
        }
    }

    /// Lane by job kind, as the specs would give it.
    fn lane_of(j: &Job) -> Lane {
        match j.kind {
            JobKind::Pipeline | JobKind::Plan | JobKind::Draft => Lane::Engine,
            _ => Lane::Io,
        }
    }

    /// PLT-7: a job paused while it waited in the queue (or restored paused after a restart) has
    /// no worker. It must not hold the engine lane — the jobs behind it stayed queued for ever.
    #[test]
    fn a_job_paused_before_it_started_does_not_block_its_lane() {
        let no_worker = |_: &str| false;
        let jobs = vec![
            job("paused", JobKind::Pipeline, JobState::Paused, true, 0),
            job("next", JobKind::Pipeline, JobState::Queued, true, 1),
            job("later", JobKind::Draft, JobState::Queued, true, 2),
        ];
        assert_eq!(next_to_start(&jobs, &lane_of, &no_worker).as_deref(), Some("next"));
        // Paused while running: the worker waits inside the job and still owns the engine.
        let has_worker = |id: &str| id == "paused";
        assert_eq!(next_to_start(&jobs, &lane_of, &has_worker), None);
        // … but the I/O lane is free in both cases.
        let mut with_io = jobs.clone();
        with_io.push(job("export", JobKind::Export, JobState::Queued, true, 3));
        assert_eq!(next_to_start(&with_io, &lane_of, &has_worker).as_deref(), Some("export"));

        // The same on the I/O lane: paused-in-queue exports do not use up the two slots.
        let io = vec![
            job("p1", JobKind::Export, JobState::Paused, true, 0),
            job("p2", JobKind::Organize, JobState::Paused, true, 1),
            job("queued", JobKind::Export, JobState::Queued, true, 2),
        ];
        assert_eq!(next_to_start(&io, &lane_of, &no_worker).as_deref(), Some("queued"));
        assert_eq!(next_to_start(&io, &lane_of, &|_| true), None);
    }

    #[test]
    fn scheduling_keeps_priority_order_and_the_single_engine_slot() {
        let no_worker = |_: &str| false;
        let mut high = job("high", JobKind::Plan, JobState::Queued, true, 5);
        high.priority = JobPriority::High;
        let mut jobs = vec![
            job("old", JobKind::Pipeline, JobState::Queued, true, 0),
            job("new", JobKind::Pipeline, JobState::Queued, true, 1),
            high,
            job("finished", JobKind::Pipeline, JobState::Done, true, -5),
        ];
        assert_eq!(next_to_start(&jobs, &lane_of, &no_worker).as_deref(), Some("high"));
        jobs.retain(|j| j.id != "high");
        assert_eq!(next_to_start(&jobs, &lane_of, &no_worker).as_deref(), Some("old"));
        // A running engine job — persistent or a lightweight pipeline job — holds the lane.
        jobs.push(job("running", JobKind::Draft, JobState::Running, true, -1));
        assert_eq!(next_to_start(&jobs, &lane_of, &no_worker), None);
        jobs.retain(|j| j.id != "running");
        jobs.push(job("light", JobKind::Pipeline, JobState::Running, false, -1));
        assert_eq!(next_to_start(&jobs, &lane_of, &no_worker), None);
        // Lightweight jobs are never started by the scheduler.
        let light_only = vec![job("light", JobKind::DocumentImport, JobState::Queued, false, 0)];
        assert_eq!(next_to_start(&light_only, &lane_of, &no_worker), None);
    }

    fn record(id: &str, state: CoreState, finished_minute: Option<i64>) -> JobRecord {
        let base = DateTime::parse_from_rfc3339("2026-11-11T09:00:00Z").unwrap().with_timezone(&Utc);
        let mut r = JobRecord::new(JobSpec::Maintenance, id, JobPriority::Normal);
        r.id = id.into();
        r.state = state;
        r.created_at = base;
        r.finished_at = finished_minute.map(|m| base + chrono::Duration::minutes(m));
        r
    }

    /// PLT-17: finished jobs come back oldest first, like the list they were in before the
    /// restart — the finished tabs show it reversed and the cap trims from the front.
    #[test]
    fn restored_finished_jobs_keep_their_order() {
        // What `core_jobs::list` returns: active jobs, then finished ones newest first.
        let mut list = vec![record("queued", CoreState::Queued, None), record("paused", CoreState::Paused, None)];
        for minute in (1..=MAX_FINISHED as i64).rev() {
            let state = if minute % 2 == 0 { CoreState::Done } else { CoreState::Failed };
            list.push(record(&format!("f{minute:02}"), state, Some(minute)));
        }
        let ids: Vec<&str> = restore_order(&list).into_iter().map(|r| r.id.as_str()).collect();
        assert_eq!(&ids[..3], ["f01", "f02", "f03"]);
        assert_eq!(&ids[MAX_FINISHED - 1..], ["f50", "queued", "paused"]);

        let book = JobBook::default();
        for rec in restore_order(&list) {
            book.push(JobBook::view(rec));
        }
        let in_memory = |book: &JobBook| book.list().into_iter().map(|j| j.id).collect::<Vec<_>>();
        assert_eq!(in_memory(&book)[..2], ["f01", "f02"]);
        // One more job finishes: the cap drops the oldest finished job, not the most recent ones.
        let mut newest = JobBook::view(&record("f51", CoreState::Done, Some(51)));
        newest.state = JobState::Done;
        book.push(newest);
        let now = in_memory(&book);
        assert_eq!(now.len(), MAX_FINISHED + 2);
        assert!(!now.contains(&"f01".to_string()) && now.contains(&"f50".to_string()));
        assert_eq!(now.last().map(String::as_str), Some("f51"));
        assert_eq!(now[0], "f02");
    }
}
