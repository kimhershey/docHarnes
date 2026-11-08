//! OS notification when a long job finishes (design §3.9 "완료 시 OS 알림",
//! setting `notifications.job_complete`). Short shell jobs (model load, hash,
//! render …) stay silent; batch-like kinds notify on success and failure.

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use doculens_core::settings::Language;

use crate::jobs::{Job, JobKind, JobState};
use crate::state::AppState;

fn notifies(kind: JobKind) -> bool {
    matches!(
        kind,
        JobKind::Pipeline
            | JobKind::Plan
            | JobKind::Export
            | JobKind::Organize
            | JobKind::Draft
            | JobKind::DocumentImport
            | JobKind::KnowledgeIndex
            | JobKind::KnowledgeEmbed
            | JobKind::ModelDownload
    )
}

fn kind_label(kind: JobKind, ko: bool) -> &'static str {
    match (kind, ko) {
        (JobKind::Pipeline, true) => "문서 처리",
        (JobKind::Plan, true) => "명령 실행",
        (JobKind::Export, true) => "내보내기",
        (JobKind::Organize, true) => "정리",
        (JobKind::Draft, true) => "문서 초안",
        (JobKind::DocumentImport, true) => "가져오기",
        (JobKind::KnowledgeIndex, true) => "색인",
        (JobKind::KnowledgeEmbed, true) => "임베딩",
        (JobKind::ModelDownload, true) => "모델 다운로드",
        (_, true) => "작업",
        (JobKind::Pipeline, false) => "Processing",
        (JobKind::Plan, false) => "Command",
        (JobKind::Export, false) => "Export",
        (JobKind::Organize, false) => "Organise",
        (JobKind::Draft, false) => "Draft",
        (JobKind::DocumentImport, false) => "Import",
        (JobKind::KnowledgeIndex, false) => "Indexing",
        (JobKind::KnowledgeEmbed, false) => "Embedding",
        (JobKind::ModelDownload, false) => "Model download",
        (_, false) => "Job",
    }
}

/// Shows a toast for a finished job when the setting is on. Never fails the caller.
pub fn job_finished(app: &AppHandle, job: &Job) {
    if !notifies(job.kind) || !matches!(job.state, JobState::Done | JobState::Failed) {
        return;
    }
    let state = app.state::<AppState>();
    let (enabled, ko) = state
        .settings
        .lock()
        .map(|s| (s.notifications.job_complete, s.ui.language == Language::Ko))
        .unwrap_or((false, true));
    if !enabled {
        return;
    }
    let label = kind_label(job.kind, ko);
    let title = match (job.state, ko) {
        (JobState::Done, true) => format!("{label} 완료"),
        (JobState::Done, false) => format!("{label} finished"),
        (_, true) => format!("{label} 실패"),
        (_, false) => format!("{label} failed"),
    };
    let body = job
        .error
        .clone()
        .or_else(|| job.detail.clone())
        .map(|d| truncate(&d, 160))
        .unwrap_or_else(|| truncate(&job.subject, 160));
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        log::debug!("notification not shown: {e}");
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
