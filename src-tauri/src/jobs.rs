//! Lightweight job book for phase-1 background work (model load / import /
//! download / hash / engine start). Phase 5 replaces this with the
//! persistent Job runner (priority queue, pause/resume, retries); the event
//! shape (`jobs://update` with the full list) stays.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

pub const JOBS_EVENT: &str = "jobs://update";
const MAX_FINISHED: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    EngineStart,
    ModelLoad,
    ModelImport,
    ModelDownload,
    ModelHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn is_finished(self) -> bool {
        matches!(self, JobState::Done | JobState::Failed | JobState::Cancelled)
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
}

#[derive(Default)]
pub struct JobBook {
    jobs: Mutex<Vec<Job>>,
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    app: OnceLock<AppHandle>,
}

impl JobBook {
    pub fn attach(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    fn emit(&self) {
        if let (Some(app), Ok(jobs)) = (self.app.get(), self.jobs.lock()) {
            let _ = app.emit(JOBS_EVENT, jobs.clone());
        }
    }

    pub fn list(&self) -> Vec<Job> {
        self.jobs.lock().map(|j| j.clone()).unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.jobs.lock().ok()?.iter().find(|j| j.id == id).cloned()
    }

    /// True when a job of this kind is still queued/running for `subject`.
    pub fn is_active(&self, kind: JobKind, subject: &str) -> bool {
        self.jobs
            .lock()
            .map(|j| j.iter().any(|j| j.kind == kind && j.subject == subject && !j.state.is_finished()))
            .unwrap_or(false)
    }

    pub fn create(&self, kind: JobKind, subject: &str) -> (String, Arc<AtomicBool>) {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let flag = Arc::new(AtomicBool::new(false));
        let now = Utc::now();
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.push(Job {
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
            });
            // Trim old finished jobs.
            let finished: Vec<usize> =
                jobs.iter().enumerate().filter(|(_, j)| j.state.is_finished()).map(|(i, _)| i).collect();
            if finished.len() > MAX_FINISHED {
                for i in finished.into_iter().take(finished_overflow(jobs.len())).rev() {
                    jobs.remove(i);
                }
            }
        }
        if let Ok(mut c) = self.cancels.lock() {
            c.insert(id.clone(), flag.clone());
        }
        self.emit();
        (id, flag)
    }

    pub fn update(&self, id: &str, f: impl FnOnce(&mut Job)) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(j) = jobs.iter_mut().find(|j| j.id == id) {
                f(j);
                j.updated_at = Utc::now();
                if j.state == JobState::Queued {
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
        self.update(id, |j| {
            j.progress = progress;
            if bytes_done.is_some() {
                j.bytes_done = bytes_done;
            }
            if bytes_total.is_some() {
                j.bytes_total = bytes_total;
            }
            if detail.is_some() {
                j.detail = detail;
            }
        });
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
    }

    pub fn cancel_flag(&self, id: &str) -> Option<Arc<AtomicBool>> {
        self.cancels.lock().ok()?.get(id).cloned()
    }

    pub fn cancel(&self, id: &str) -> bool {
        let Some(flag) = self.cancel_flag(id) else { return false };
        flag.store(true, Ordering::Relaxed);
        self.update(id, |j| j.cancel_requested = true);
        true
    }

    pub fn clear_finished(&self) {
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.retain(|j| !j.state.is_finished());
        }
        self.emit();
    }
}

fn finished_overflow(total: usize) -> usize {
    total.saturating_sub(MAX_FINISHED)
}
