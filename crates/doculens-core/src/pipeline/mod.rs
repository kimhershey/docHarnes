//! Document pipeline (design §3.4): page classification, OCR to Markdown /
//! HTML tables, key-information extraction against a schema, deterministic
//! validation, confidence scoring, the human review queue and a human-readable
//! mirror of every extraction under `.doculens/extractions/`.
//!
//! The VLM is reached through [`crate::knowledge::answer::ChatBackend`] so the
//! same code runs against llama-server, the fake router and scripted tests.
//! OCR text is written into `pages.text`, which the P3 indexer re-chunks
//! through `documents.updated_at`.

pub mod classify;
pub mod extract;
pub mod forms;
pub mod image;
pub mod ocr;
pub mod prompts;
pub mod repo;
pub mod review;
pub mod run;
pub mod schema;
pub mod split;
pub mod validate;
pub mod vlm;

#[cfg(test)]
mod tests;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use schema::{FieldDef, FieldKind, SchemaDef, SchemaDraft, SchemaRecord, SchemaSource};

/// Prompt template version recorded with every run (bump when prompts change).
pub const PROMPT_VERSION: &str = "p4.1";

/// What a processing request runs (each level includes the previous ones).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PipelineLevel {
    Classify,
    Ocr,
    #[default]
    Extract,
}

impl PipelineLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            PipelineLevel::Classify => "classify",
            PipelineLevel::Ocr => "ocr",
            PipelineLevel::Extract => "extract",
        }
    }
}

impl From<crate::settings::AutoProcessingLevel> for Option<PipelineLevel> {
    fn from(l: crate::settings::AutoProcessingLevel) -> Self {
        use crate::settings::AutoProcessingLevel as A;
        match l {
            A::None => None,
            A::ClassifyOnly => Some(PipelineLevel::Classify),
            A::ClassifyAndOcr => Some(PipelineLevel::Ocr),
            A::Full => Some(PipelineLevel::Extract),
        }
    }
}

/// Which schema the extractor uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchemaChoice {
    /// Type → schema mapping (document override first).
    #[default]
    Auto,
    Id {
        id: String,
    },
}

/// One processing request (library bulk action, viewer "다시 처리", auto pipeline).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessRequest {
    pub document_ids: Vec<String>,
    pub level: PipelineLevel,
    pub schema: SchemaChoice,
    /// Re-run stages whose results already exist.
    pub force: bool,
    /// Inference profile override (`None` = settings).
    pub profile: Option<crate::settings::Profile>,
    /// Pages to restrict OCR / extraction to (empty = all).
    pub pages: Vec<u32>,
    /// Precision mode override.
    pub precise: Option<bool>,
    /// Leave the extraction of documents that already have one alone (the automatic pipeline
    /// after an import: a document that merely came back under a new name must not lose its
    /// reviewed fields). Explicit requests keep this off — they replace the latest run.
    pub skip_extracted: bool,
}

impl Default for ProcessRequest {
    fn default() -> Self {
        Self {
            document_ids: Vec::new(),
            level: PipelineLevel::Extract,
            schema: SchemaChoice::Auto,
            force: false,
            profile: None,
            pages: Vec::new(),
            precise: None,
            skip_extracted: false,
        }
    }
}

/// Progress callback payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ProcessProgress {
    pub done: u32,
    pub total: u32,
    pub current: String,
    /// `classify` | `ocr` | `extract` | `validate`
    pub stage: String,
    pub page: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ProcessReport {
    pub processed: Vec<String>,
    pub skipped: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub classified: u32,
    pub ocr_pages: u32,
    pub extracted: u32,
    pub review_pending: u32,
    pub cancelled: bool,
}

/// Classifier output for one page (§3.4 "VLM 1차").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageClass {
    pub doc_type: String,
    pub rotation: u16,
    pub is_blank: bool,
    pub language: String,
}

/// Review state of one extracted field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FieldStatus {
    #[default]
    Auto,
    Approved,
    Corrected,
    Rejected,
    Held,
}

impl FieldStatus {
    pub const ALL: [FieldStatus; 5] =
        [FieldStatus::Auto, FieldStatus::Approved, FieldStatus::Corrected, FieldStatus::Rejected, FieldStatus::Held];
    pub fn as_str(self) -> &'static str {
        match self {
            FieldStatus::Auto => "auto",
            FieldStatus::Approved => "approved",
            FieldStatus::Corrected => "corrected",
            FieldStatus::Rejected => "rejected",
            FieldStatus::Held => "held",
        }
    }
    pub fn parse(s: &str) -> Option<FieldStatus> {
        FieldStatus::ALL.into_iter().find(|k| k.as_str() == s)
    }
    /// A field whose review is finished (approved, corrected or rejected).
    pub fn is_resolved(self) -> bool {
        matches!(self, FieldStatus::Approved | FieldStatus::Corrected | FieldStatus::Rejected)
    }
}

/// Why a field sits in the review queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    LowConfidence,
    ValidationFailed,
    RequiredMissing,
    Audit,
}

impl ReviewReason {
    pub const ALL: [ReviewReason; 4] = [
        ReviewReason::LowConfidence,
        ReviewReason::ValidationFailed,
        ReviewReason::RequiredMissing,
        ReviewReason::Audit,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewReason::LowConfidence => "low_confidence",
            ReviewReason::ValidationFailed => "validation_failed",
            ReviewReason::RequiredMissing => "required_missing",
            ReviewReason::Audit => "audit",
        }
    }
    pub fn parse(s: &str) -> Option<ReviewReason> {
        ReviewReason::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// Validation outcome attached to a field (`fields.validation` JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationIssue {
    /// `brn_checksum`, `sum_total`, `date_parse`, …
    pub rule: String,
    /// `ok` | `warn` | `fail`
    pub level: String,
    pub message: String,
}

/// One row of `fields`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldRecord {
    pub id: i64,
    pub run_id: String,
    pub document_id: String,
    /// `supplier.brn`, `items[2].amount`, …
    pub path: String,
    pub key: String,
    pub row: Option<u32>,
    pub label_ko: String,
    pub label_en: String,
    pub kind: FieldKind,
    pub required: bool,
    pub value: Option<String>,
    pub normalized: Option<String>,
    pub original_value: Option<String>,
    pub page: Option<u32>,
    /// `[x0, y0, x1, y1]` in 0..1000 of the page, `None` = page-level evidence.
    pub bbox: Option<[u16; 4]>,
    pub evidence: Option<String>,
    pub confidence: f64,
    pub model_confidence: Option<u8>,
    pub status: FieldStatus,
    pub review_reason: Option<ReviewReason>,
    pub validation: Vec<ValidationIssue>,
    pub reviewed_at: Option<DateTime<Utc>>,
}

impl FieldRecord {
    /// The value that counts (rejected = none).
    pub fn effective_value(&self) -> Option<&str> {
        if self.status == FieldStatus::Rejected {
            None
        } else {
            self.value.as_deref()
        }
    }
    pub fn needs_review(&self) -> bool {
        self.review_reason.is_some() && !self.status.is_resolved()
    }
}

/// One row of `extraction_runs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractionRun {
    pub id: String,
    pub document_id: String,
    pub schema_id: Option<String>,
    pub schema_version: Option<u32>,
    pub doc_type: Option<String>,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    pub profile: Option<String>,
    pub precise: bool,
    /// `running` | `done` | `failed` | `cancelled`
    pub status: String,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub pages_done: u32,
    pub confidence: Option<f64>,
    pub field_count: u32,
    pub review_count: u32,
}

/// One row of `field_history` (snapshot of a review change).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldChange {
    pub id: i64,
    pub field_id: i64,
    pub document_id: String,
    pub path: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub old_status: FieldStatus,
    pub new_status: FieldStatus,
    pub changed_at: DateTime<Utc>,
    pub note: Option<String>,
}

/// Document type known to the classifier (one per enabled schema `doc_type`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocTypeInfo {
    pub slug: String,
    pub label_ko: String,
    pub label_en: String,
    pub schema_id: Option<String>,
    pub builtin: bool,
    pub use_count: u64,
}

/// Per-document pipeline summary (viewer header / library badges).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PipelineStatus {
    pub documents: u64,
    pub classified: u64,
    pub ocr_done: u64,
    pub extracted: u64,
    pub review_pending: u64,
    pub reviewed: u64,
    pub review_fields: u64,
    pub failed: u64,
}

/// A table parsed out of OCR / text-layer HTML (viewer "표" tab, exports).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ExtractedTable {
    pub page: u32,
    pub index: u32,
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}
