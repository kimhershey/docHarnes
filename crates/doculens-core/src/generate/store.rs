//! Saved drafts: one JSON file per draft under `<workspace>/.doculens/drafts/<id>.json`
//! (created lazily). Listing is newest first; corrupt files are skipped with a log line.

use std::path::{Path, PathBuf};

use chrono::Utc;

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

/// Removes a saved draft; `false` when there was none.
pub fn delete(paths: &WorkspacePaths, id: &str) -> Result<bool> {
    let path = file_for(paths, id)?;
    if !path.is_file() {
        return Ok(false);
    }
    std::fs::remove_file(&path).map_err(|e| CoreError::io(&path, e))?;
    Ok(true)
}
