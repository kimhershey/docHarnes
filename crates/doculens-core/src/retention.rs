//! Retention / purge policy (design §3.8, decision 16). Stored per workspace
//! in `workspace.json`; edited from Settings › Privacy. `None` means "keep
//! forever" except where documented.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionItem {
    /// Original documents (default: forever).
    Documents,
    /// Extraction results and fields (default: same as documents).
    Extractions,
    /// Rendered page cache — size-capped (LRU), not time based.
    PageCache,
    JobLogs,
    AppLogs,
    /// File-operation journal (undo history).
    Journal,
    /// Assistant conversations.
    Chats,
    /// Temporary pre-masking data (default 0 days = purge immediately).
    Temp,
    /// Golden-set exports (manual only).
    GoldenExports,
}

impl RetentionItem {
    pub const ALL: [RetentionItem; 9] = [
        RetentionItem::Documents,
        RetentionItem::Extractions,
        RetentionItem::PageCache,
        RetentionItem::JobLogs,
        RetentionItem::AppLogs,
        RetentionItem::Journal,
        RetentionItem::Chats,
        RetentionItem::Temp,
        RetentionItem::GoldenExports,
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionPolicy {
    pub documents_days: Option<u32>,
    pub extractions_days: Option<u32>,
    pub page_cache_max_gb: f32,
    pub job_logs_days: Option<u32>,
    pub app_logs_days: Option<u32>,
    pub journal_days: Option<u32>,
    pub chats_days: Option<u32>,
    pub temp_days: Option<u32>,
    pub golden_exports_days: Option<u32>,
    /// Local hour (0-23) at which the daily purge job runs.
    pub purge_hour_local: u8,
    /// Whether the daily purge job is enabled at all.
    pub purge_enabled: bool,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            documents_days: None,
            extractions_days: None,
            page_cache_max_gb: 5.0,
            job_logs_days: Some(90),
            app_logs_days: Some(90),
            journal_days: Some(90),
            chats_days: Some(180),
            temp_days: Some(0),
            golden_exports_days: None,
            purge_hour_local: 3,
            purge_enabled: true,
        }
    }
}

impl RetentionPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.purge_hour_local > 23 {
            return Err(CoreError::Invalid("purge_hour_local must be 0-23".into()));
        }
        if !(0.5..=1000.0).contains(&self.page_cache_max_gb) {
            return Err(CoreError::Invalid("page_cache_max_gb must be between 0.5 and 1000".into()));
        }
        for (name, v) in [
            ("documents_days", self.documents_days),
            ("extractions_days", self.extractions_days),
            ("job_logs_days", self.job_logs_days),
            ("app_logs_days", self.app_logs_days),
            ("journal_days", self.journal_days),
            ("chats_days", self.chats_days),
            ("temp_days", self.temp_days),
            ("golden_exports_days", self.golden_exports_days),
        ] {
            if let Some(d) = v {
                if d > 36500 {
                    return Err(CoreError::Invalid(format!("{name} must be <= 36500 days")));
                }
            }
        }
        Ok(())
    }

    /// Effective retention in days for an item (`None` = keep forever).
    pub fn effective_days(&self, item: RetentionItem) -> Option<u32> {
        match item {
            RetentionItem::Documents => self.documents_days,
            // Extractions follow the document unless set explicitly.
            RetentionItem::Extractions => self.extractions_days.or(self.documents_days),
            RetentionItem::PageCache => None,
            RetentionItem::JobLogs => self.job_logs_days,
            RetentionItem::AppLogs => self.app_logs_days,
            RetentionItem::Journal => self.journal_days,
            RetentionItem::Chats => self.chats_days,
            RetentionItem::Temp => self.temp_days,
            RetentionItem::GoldenExports => self.golden_exports_days,
        }
    }

    /// Whether an item created at `created` has expired at `now`.
    pub fn is_expired(&self, item: RetentionItem, created: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        match self.effective_days(item) {
            None => false,
            Some(days) => now - created >= chrono::Duration::days(i64::from(days)),
        }
    }
}

// ---------------------------------------------------------------------------
// File-system helpers for the daily purge job (§3.8: no OS recycle bin)
// ---------------------------------------------------------------------------

/// Outcome of a file-system purge step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FsPurge {
    pub files: u64,
    pub bytes: u64,
}

fn modified(meta: &std::fs::Metadata) -> Option<std::time::SystemTime> {
    meta.modified().ok()
}

/// Total size of a directory tree (0 when it does not exist).
pub fn dir_size(dir: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

/// Deletes every file under `dir` whose modification time is older than `cutoff`
/// (recursively), then removes directories left empty. Missing `dir` → no-op.
pub fn purge_old_files(dir: &std::path::Path, cutoff: std::time::SystemTime) -> FsPurge {
    let mut out = FsPurge::default();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            let inner = purge_old_files(&path, cutoff);
            out.files += inner.files;
            out.bytes += inner.bytes;
            let _ = std::fs::remove_dir(&path); // only succeeds when empty
        } else if modified(&meta).is_some_and(|m| m < cutoff) && std::fs::remove_file(&path).is_ok() {
            out.files += 1;
            out.bytes += meta.len();
        }
    }
    out
}

/// Size-capped LRU trim over the per-document sub-directories of a cache folder:
/// while the tree is larger than `max_bytes`, the least recently used sub-directory (see
/// `last_used`) that is older than `min_age` is removed. Returns the removed directories and bytes.
pub fn trim_lru_dirs(dir: &std::path::Path, max_bytes: u64, min_age: std::time::Duration) -> FsPurge {
    let mut out = FsPurge::default();
    let mut total = dir_size(dir);
    if total <= max_bytes {
        return out;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    let now = std::time::SystemTime::now();
    let mut subdirs: Vec<(std::time::SystemTime, std::path::PathBuf, u64)> = rd
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_dir() {
                return None;
            }
            let used = last_used(&e.path()).or_else(|| modified(&meta))?;
            Some((used, e.path(), dir_size(&e.path())))
        })
        .collect();
    subdirs.sort_by_key(|(m, _, _)| *m);
    for (mtime, path, size) in subdirs {
        if total <= max_bytes {
            break;
        }
        if now.duration_since(mtime).map(|age| age < min_age).unwrap_or(true) {
            continue;
        }
        if std::fs::remove_dir_all(&path).is_ok() {
            out.files += 1;
            out.bytes += size;
            total = total.saturating_sub(size);
        }
    }
    out
}

/// Marker file inside a per-document cache folder whose modification time records when one of
/// its files was last served to the viewer (the shell's `doculens://` handler touches it).
pub const LAST_USED_MARKER: &str = ".last_used";

/// Touches the `.last_used` marker of a cache folder (created when missing). Errors are
/// ignored: without the marker the LRU falls back to the newest file time.
pub fn touch_last_used(dir: &std::path::Path) {
    if !dir.is_dir() {
        return;
    }
    let marker = dir.join(LAST_USED_MARKER);
    match std::fs::File::options().write(true).open(&marker) {
        Ok(f) => {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        Err(_) => {
            let _ = std::fs::write(&marker, b"");
        }
    }
}

/// When a cache folder was last used: the `.last_used` marker when present (viewing), else the
/// newest file inside (its render time). The marker wins over newer files so a bulk re-render
/// does not turn the LRU into FIFO.
fn last_used(dir: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(dir.join(LAST_USED_MARKER)).ok().and_then(|m| modified(&m)).or_else(|| newest_mtime(dir))
}

/// Most recent modification time inside a directory tree (the render time of a page cache
/// folder that was never viewed since the marker was introduced).
fn newest_mtime(dir: &std::path::Path) -> Option<std::time::SystemTime> {
    let mut newest: Option<std::time::SystemTime> = None;
    let rd = std::fs::read_dir(dir).ok()?;
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let m = if meta.is_dir() { newest_mtime(&entry.path()) } else { modified(&meta) };
        if let Some(m) = m {
            if newest.is_none_or(|n| m > n) {
                newest = Some(m);
            }
        }
    }
    newest
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn fs_purge_removes_old_files_and_trims_lru_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("docA");
        let new = dir.path().join("docB");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("p1.jpg"), vec![0u8; 1000]).unwrap();
        std::fs::write(new.join("p1.jpg"), vec![0u8; 1000]).unwrap();
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 86_400);
        let f = std::fs::File::options().write(true).open(old.join("p1.jpg")).unwrap();
        f.set_modified(past).unwrap();
        drop(f);
        assert_eq!(dir_size(dir.path()), 2000);
        // LRU: the cap of 1,500 bytes evicts the old directory only.
        let trimmed = trim_lru_dirs(dir.path(), 1_500, std::time::Duration::from_secs(86_400));
        assert_eq!((trimmed.files, trimmed.bytes), (1, 1000));
        assert!(!old.exists() && new.exists());
        // Age-based purge: nothing older than the cutoff is left.
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("x.log"), b"abc").unwrap();
        let f = std::fs::File::options().write(true).open(old.join("x.log")).unwrap();
        f.set_modified(past).unwrap();
        drop(f);
        let purged = purge_old_files(dir.path(), std::time::SystemTime::now() - std::time::Duration::from_secs(86_400));
        assert_eq!((purged.files, purged.bytes), (1, 3));
        assert!(!old.exists(), "empty directory removed");
        assert!(new.join("p1.jpg").exists());
    }

    fn set_mtime(path: &std::path::Path, t: std::time::SystemTime) {
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(t).unwrap();
    }

    #[test]
    fn lru_prefers_the_last_used_marker() {
        let dir = tempfile::tempdir().unwrap();
        let viewed = dir.path().join("viewed"); // rendered long ago, viewed just now
        let rendered = dir.path().join("rendered"); // rendered just now, last viewed long ago
        std::fs::create_dir_all(&viewed).unwrap();
        std::fs::create_dir_all(&rendered).unwrap();
        std::fs::write(viewed.join("p1.jpg"), vec![0u8; 1000]).unwrap();
        std::fs::write(rendered.join("p1.jpg"), vec![0u8; 1000]).unwrap();
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 86_400);
        set_mtime(&viewed.join("p1.jpg"), past);
        touch_last_used(&viewed);
        assert!(viewed.join(LAST_USED_MARKER).is_file());
        touch_last_used(&rendered);
        set_mtime(&rendered.join(LAST_USED_MARKER), past);
        touch_last_used(&dir.path().join("missing")); // no folder → no marker, no error
        assert!(!dir.path().join("missing").exists());
        let trimmed = trim_lru_dirs(dir.path(), 1_500, std::time::Duration::from_secs(86_400));
        assert_eq!(trimmed.files, 1);
        assert!(viewed.exists() && !rendered.exists(), "evicted by last use, not by render time");
        // Touching again moves the marker forward.
        set_mtime(&viewed.join(LAST_USED_MARKER), past);
        touch_last_used(&viewed);
        let age = std::fs::metadata(viewed.join(LAST_USED_MARKER)).unwrap().modified().unwrap().elapsed().unwrap();
        assert!(age < std::time::Duration::from_secs(60), "{age:?}");
    }

    #[test]
    fn defaults_match_design() {
        let p = RetentionPolicy::default();
        assert_eq!(p.effective_days(RetentionItem::Documents), None);
        assert_eq!(p.effective_days(RetentionItem::Extractions), None);
        assert_eq!(p.effective_days(RetentionItem::JobLogs), Some(90));
        assert_eq!(p.effective_days(RetentionItem::Chats), Some(180));
        assert_eq!(p.effective_days(RetentionItem::Temp), Some(0));
        assert!(p.validate().is_ok());
    }

    #[test]
    fn extractions_follow_documents() {
        let p = RetentionPolicy { documents_days: Some(30), ..Default::default() };
        assert_eq!(p.effective_days(RetentionItem::Extractions), Some(30));
        let p2 = RetentionPolicy { documents_days: Some(30), extractions_days: Some(7), ..Default::default() };
        assert_eq!(p2.effective_days(RetentionItem::Extractions), Some(7));
    }

    #[test]
    fn expiry_boundaries() {
        let p = RetentionPolicy::default();
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert!(p.is_expired(RetentionItem::Temp, t0, t0)); // 0 days => immediately
        assert!(!p.is_expired(RetentionItem::JobLogs, t0, t0 + chrono::Duration::days(89)));
        assert!(p.is_expired(RetentionItem::JobLogs, t0, t0 + chrono::Duration::days(90)));
        assert!(!p.is_expired(RetentionItem::Documents, t0, t0 + chrono::Duration::days(100_000)));
    }

    #[test]
    fn validation_rejects_bad_values() {
        let p = RetentionPolicy { purge_hour_local: 24, ..Default::default() };
        assert!(p.validate().is_err());
        let p = RetentionPolicy { page_cache_max_gb: 0.0, ..Default::default() };
        assert!(p.validate().is_err());
    }
}
