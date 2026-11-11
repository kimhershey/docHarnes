//! Saved drafts: one JSON file per draft under `<workspace>/.doculens/drafts/<id>.json`
//! (created lazily). Listing is newest first; corrupt files are skipped with a log line.
//!
//! [`save_job_result`] is how a draft job stores its result: a job that revised or regenerated a
//! stored draft may replace it only when that loses nothing — not after a cancel, and not when the
//! stored draft was changed after the job took its input.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::documents::is_safe_id;
use crate::error::{CoreError, Result};
use crate::paths::WorkspacePaths;

use super::{Draft, DraftSummary};

fn file_for(paths: &WorkspacePaths, id: &str) -> Result<PathBuf> {
    if !is_safe_id(id) {
        return Err(CoreError::Invalid("draft_id".into()));
    }
    Ok(paths.drafts_dir().join(format!("{id}.json")))
}

fn summary(d: &Draft) -> DraftSummary {
    DraftSummary {
        id: d.id.clone(),
        title: d.title.clone(),
        template: d.template.clone(),
        updated_at: d.updated_at,
        issues: d.issues.len() as u32,
        saved_path: d.saved_path.clone(),
    }
}

/// Writes the draft (bumping `updated_at`) and returns its listing entry.
pub fn save(paths: &WorkspacePaths, draft: &Draft) -> Result<DraftSummary> {
    let path = file_for(paths, &draft.id)?;
    let dir = paths.drafts_dir();
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    let mut d = draft.clone();
    d.updated_at = Utc::now();
    let bytes = serde_json::to_vec_pretty(&d).map_err(|e| CoreError::json(&path, e))?;
    let part = path.with_extension("json.part");
    std::fs::write(&part, bytes).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        CoreError::io(&part, e)
    })?;
    std::fs::rename(&part, &path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        CoreError::io(&path, e)
    })?;
    Ok(summary(&d))
}

fn read(path: &Path) -> Result<Draft> {
    let bytes = std::fs::read(path).map_err(|e| CoreError::io(path, e))?;
    serde_json::from_slice(&bytes).map_err(|e| CoreError::json(path, e))
}

/// Every saved draft, newest first.
pub fn list(paths: &WorkspacePaths) -> Result<Vec<DraftSummary>> {
    let dir = paths.drafts_dir();
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let rd = std::fs::read_dir(&dir).map_err(|e| CoreError::io(&dir, e))?;
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match read(&p) {
            Ok(d) => out.push(summary(&d)),
            Err(e) => log::warn!("skipping unreadable draft {}: {e}", p.display()),
        }
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

pub fn load(paths: &WorkspacePaths, id: &str) -> Result<Draft> {
    let path = file_for(paths, id)?;
    if !path.is_file() {
        return Err(CoreError::NotFound(format!("draft {id}")));
    }
    read(&path)
}

/// Whether a draft with this id is stored.
pub fn exists(paths: &WorkspacePaths, id: &str) -> bool {
    file_for(paths, id).is_ok_and(|p| p.is_file())
}

/// Removes a saved draft; `false` when there was none.
pub fn delete(paths: &WorkspacePaths, id: &str) -> Result<bool> {
    let path = file_for(paths, id)?;
    if !path.is_file() {
        return Ok(false);
    }
    std::fs::remove_file(&path).map_err(|e| CoreError::io(&path, e))?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Results of draft jobs
// ---------------------------------------------------------------------------

/// What a draft job started from.
#[derive(Debug, Clone, Copy)]
pub struct JobInput<'a> {
    /// The draft the job regenerated or revised (`DraftRequest.previous`); `None` for a new draft.
    pub previous: Option<&'a Draft>,
    /// When the job took its input: the creation time of the job record. A retried job keeps it —
    /// and the `previous` it was created with.
    pub captured_at: DateTime<Utc>,
    /// Whether `previous` was stored when the job started running ([`exists`]).
    pub existed: bool,
}

/// Why a job's result was stored next to the draft it started from instead of over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyReason {
    /// The job was cancelled: what it wrote so far must not replace a complete draft.
    Cancelled,
    /// The stored draft was changed after the job took its input (edits made while a job waited, or
    /// before a failed job was retried).
    Newer,
}

/// Why a job's result was not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// The draft the job started from was deleted while the job ran: it is not brought back.
    Deleted,
    /// Writing the draft failed (the job's result still carries it).
    Failed,
}

/// Where the result of a draft job went ([`save_job_result`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum JobSave {
    /// Under the draft's own id: a new draft, or the draft the job started from.
    Saved,
    /// As a new draft; `original` — the draft the job started from — is as it was.
    Copy {
        original: String,
        reason: CopyReason,
    },
    NotSaved {
        reason: SkipReason,
    },
}

/// Whether two versions of a draft say the same: title, field values, table rows and body.
/// Timestamps, notes and validation results do not count — replacing a stored version that only
/// differs in those loses nothing.
fn same_content(a: &Draft, b: &Draft) -> bool {
    a.title == b.title
        && a.body == b.body
        && a.fields.len() == b.fields.len()
        && a.fields.iter().zip(&b.fields).all(|(x, y)| x.key == y.key && x.value == y.value)
        && a.tables.len() == b.tables.len()
        && a.tables.iter().zip(&b.tables).all(|(x, y)| x.key == y.key && x.rows == y.rows)
}

/// Stores the result of a draft job without losing what is stored already.
///
/// A new draft is stored under its id. A job that started from a stored draft (`input.previous`)
/// replaces it — except:
/// * after a cancel, the (possibly partial) result becomes a new draft and the original stays;
/// * when the stored draft was changed after the job took its input, the result becomes a new
///   draft too (the editor's saved edits win over a job that never saw them);
/// * when the stored draft was deleted while the job ran, nothing is stored.
///
/// `draft` is updated to what was stored: a copy gets a new id, a note saying what it is and no
/// export path; `updated_at` is the stored one. `ko` picks the language of the note.
pub fn save_job_result(
    paths: &WorkspacePaths,
    draft: &mut Draft,
    input: &JobInput<'_>,
    cancelled: bool,
    ko: bool,
) -> Result<JobSave> {
    let reason = match input.previous.filter(|p| p.id == draft.id) {
        // A new draft: nothing stored to protect.
        None => None,
        Some(previous) => {
            if input.existed && !exists(paths, &previous.id) {
                return Ok(JobSave::NotSaved { reason: SkipReason::Deleted });
            }
            // A stored file that cannot be read holds nothing to protect.
            let changed = load(paths, &previous.id)
                .is_ok_and(|stored| stored.updated_at > input.captured_at && !same_content(&stored, previous));
            if cancelled {
                Some(CopyReason::Cancelled)
            } else if changed {
                Some(CopyReason::Newer)
            } else {
                None
            }
        }
    };
    let outcome = match reason {
        None => JobSave::Saved,
        Some(reason) => {
            let original = std::mem::replace(&mut draft.id, crate::documents::new_id());
            draft.created_at = Utc::now();
            // The copy was never exported: the path belongs to the original.
            draft.saved_path = None;
            draft.saved_doc_id = None;
            let note = match (reason, ko) {
                (CopyReason::Cancelled, true) => {
                    "취소된 작업이 남긴 결과입니다 — 원래 초안은 그대로 두고 새 초안으로 저장했습니다."
                }
                (CopyReason::Cancelled, false) => {
                    "Result of a cancelled job: stored as a new draft, the original draft is unchanged."
                }
                (CopyReason::Newer, true) => {
                    "작업이 시작된 뒤 원래 초안이 수정되어, 덮어쓰지 않고 새 초안으로 저장했습니다."
                }
                (CopyReason::Newer, false) => {
                    "The original draft was edited after this job started, so the result was stored as a new draft."
                }
            };
            draft.notes.insert(0, note.to_string());
            JobSave::Copy { original, reason }
        }
    };
    draft.updated_at = save(paths, draft)?.updated_at;
    Ok(outcome)
}
