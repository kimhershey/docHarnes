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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

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
