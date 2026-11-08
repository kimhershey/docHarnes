//! Natural-language planner (design §3.5, appendix D): a command becomes one
//! typed JSON plan — a closed op set with typed parameters — that is validated
//! programmatically, shown to the user for confirmation when it has side
//! effects and then replayed by a deterministic executor.
//!
//! Three stages, cheapest first:
//! 1. the pre-router ([`crate::prerouter`]) for slash / settings commands,
//! 2. deterministic phrase rules ([`rules`]) for the frequent single-op
//!    commands (totals, exports, inbox processing, search, undo, job control),
//! 3. the model ([`llm`]): thinking off, temperature 0, grammar-constrained to
//!    [`schema::plan_schema`], one re-query with the validator's feedback, then
//!    a clarification question.
//!
//! The op set here is the v1.0 subset of appendix D (phase 5 scope decision);
//! document generation / project ops stay for v1.1.

pub mod exec;
pub mod llm;
pub mod prompts;
pub mod rules;
pub mod schema;
pub mod scope;
pub mod validate;

#[cfg(test)]
mod tests;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::documents::ReviewStatus;
use crate::knowledge::SearchMode;
use crate::prerouter::DateRange;
use crate::settings::{Language, Profile, Theme};

/// Prompt template version recorded with plans (bump when the planner prompt changes).
pub const PLANNER_PROMPT_VERSION: &str = "p5.1";
/// Hard cap on operations per plan (§3.5 validator).
pub const MAX_OPS: usize = 12;
/// Plans below this confidence ask a clarification question instead of running (§3.5).
pub const MIN_CONFIDENCE: f32 = 0.6;

// ---------------------------------------------------------------------------
// Op kinds
// ---------------------------------------------------------------------------

/// Every op the planner may emit (`intent` of a plan and the `op` tag of each step).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    Import,
    SetInboxWatch,
    Classify,
    SetDocType,
    Ocr,
    DefineSchema,
    Extract,
    Validate,
    Review,
    Reindex,
    Search,
    Answer,
    Summarize,
    Aggregate,
    Organize,
    Rename,
    Move,
    Tag,
    Delete,
    Export,
    SetModel,
    SetProfile,
    SetTheme,
    SetLanguage,
    SetSetting,
    Open,
    Job,
    Engine,
    Undo,
    Redo,
    Help,
    Clarify,
    Noop,
}

impl OpKind {
    pub const ALL: [OpKind; 33] = [
        OpKind::Import,
        OpKind::SetInboxWatch,
        OpKind::Classify,
        OpKind::SetDocType,
        OpKind::Ocr,
        OpKind::DefineSchema,
        OpKind::Extract,
        OpKind::Validate,
        OpKind::Review,
        OpKind::Reindex,
        OpKind::Search,
        OpKind::Answer,
        OpKind::Summarize,
        OpKind::Aggregate,
        OpKind::Organize,
        OpKind::Rename,
        OpKind::Move,
        OpKind::Tag,
        OpKind::Delete,
        OpKind::Export,
        OpKind::SetModel,
        OpKind::SetProfile,
        OpKind::SetTheme,
        OpKind::SetLanguage,
        OpKind::SetSetting,
        OpKind::Open,
        OpKind::Job,
        OpKind::Engine,
        OpKind::Undo,
        OpKind::Redo,
        OpKind::Help,
        OpKind::Clarify,
        OpKind::Noop,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            OpKind::Import => "import",
            OpKind::SetInboxWatch => "set_inbox_watch",
            OpKind::Classify => "classify",
            OpKind::SetDocType => "set_doc_type",
            OpKind::Ocr => "ocr",
            OpKind::DefineSchema => "define_schema",
            OpKind::Extract => "extract",
            OpKind::Validate => "validate",
            OpKind::Review => "review",
            OpKind::Reindex => "reindex",
            OpKind::Search => "search",
            OpKind::Answer => "answer",
            OpKind::Summarize => "summarize",
            OpKind::Aggregate => "aggregate",
            OpKind::Organize => "organize",
            OpKind::Rename => "rename",
            OpKind::Move => "move",
            OpKind::Tag => "tag",
            OpKind::Delete => "delete",
            OpKind::Export => "export",
            OpKind::SetModel => "set_model",
            OpKind::SetProfile => "set_profile",
            OpKind::SetTheme => "set_theme",
            OpKind::SetLanguage => "set_language",
            OpKind::SetSetting => "set_setting",
            OpKind::Open => "open",
            OpKind::Job => "job",
            OpKind::Engine => "engine",
            OpKind::Undo => "undo",
            OpKind::Redo => "redo",
            OpKind::Help => "help",
            OpKind::Clarify => "clarify",
            OpKind::Noop => "noop",
        }
    }

    pub fn parse(s: &str) -> Option<OpKind> {
        OpKind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Where the op runs: in the core executor (`Core`), or in the UI / shell
    /// (`Ui`: settings, navigation, job and engine control, help).
    pub fn runs_in_ui(self) -> bool {
        matches!(
            self,
            OpKind::DefineSchema
                | OpKind::SetModel
                | OpKind::SetProfile
                | OpKind::SetTheme
                | OpKind::SetLanguage
                | OpKind::SetSetting
                | OpKind::Open
                | OpKind::Job
                | OpKind::Engine
                | OpKind::Help
                | OpKind::Clarify
                | OpKind::Noop
                | OpKind::Reindex
                | OpKind::SetInboxWatch
        )
    }

    /// Ops that need the vision / chat model (the executor loads it first).
    pub fn needs_model(self) -> bool {
        matches!(
            self,
            OpKind::Classify | OpKind::Ocr | OpKind::Extract | OpKind::Answer | OpKind::Summarize | OpKind::Import
        )
    }
}

// ---------------------------------------------------------------------------
// Parameter enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImportFrom {
    #[default]
    Inbox,
    Folder,
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoPipeline {
    None,
    Classify,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrProfile {
    Standard,
    FinePrint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaMode {
    Create,
    Modify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewOp {
    Show,
    Approve,
    Reject,
    Correct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewOnly {
    LowConfidence,
    Failed,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReindexScope {
    Fts,
    Vec,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummarizeTarget {
    EachDoc,
    Set,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryLength {
    Short,
    Long,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateFn {
    Sum,
    Count,
    Avg,
    Min,
    Max,
}

impl AggregateFn {
    pub fn as_str(self) -> &'static str {
        match self {
            AggregateFn::Sum => "sum",
            AggregateFn::Count => "count",
            AggregateFn::Avg => "avg",
            AggregateFn::Min => "min",
            AggregateFn::Max => "max",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    Month,
    Vendor,
    DocType,
    #[default]
    None,
}

impl GroupBy {
    pub fn as_str(self) -> &'static str {
        match self {
            GroupBy::Month => "month",
            GroupBy::Vendor => "vendor",
            GroupBy::DocType => "doc_type",
            GroupBy::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Xlsx,
    Csv,
    Jsonl,
    Md,
    PdfSearchable,
}

impl ExportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ExportFormat::Xlsx => "xlsx",
            ExportFormat::Csv => "csv",
            ExportFormat::Jsonl => "jsonl",
            ExportFormat::Md => "md",
            ExportFormat::PdfSearchable => "pdf_searchable",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Xlsx => "xlsx",
            ExportFormat::Csv => "csv",
            ExportFormat::Jsonl => "jsonl",
            ExportFormat::Md => "md",
            ExportFormat::PdfSearchable => "pdf",
        }
    }
    pub fn parse(s: &str) -> Option<ExportFormat> {
        [ExportFormat::Xlsx, ExportFormat::Csv, ExportFormat::Jsonl, ExportFormat::Md, ExportFormat::PdfSearchable]
            .into_iter()
            .find(|f| f.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExportLayout {
    #[default]
    SheetPerType,
    Single,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportInclude {
    Evidence,
    Confidence,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExportTarget {
    #[default]
    Fields,
    Table,
    Text,
}

/// Settings the planner may change (`set_setting.key`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKey {
    RenderDpi,
    PiiMasking,
    AutoPipeline,
    InboxWatch,
    ReviewThreshold,
    CacheLimitGb,
    JournalDays,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenTarget {
    Document,
    Page,
    Folder,
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOp {
    Show,
    Cancel,
    Retry,
    Pause,
    Resume,
    Prioritize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineOp {
    Restart,
    Sleep,
    Wake,
}

// ---------------------------------------------------------------------------
// Ops
// ---------------------------------------------------------------------------

/// One plan step. Appendix D is the single source of truth for the parameters:
/// anything else is rejected (`deny_unknown_fields`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    Import {
        #[serde(default)]
        source: ImportFrom,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recursive: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auto_pipeline: Option<AutoPipeline>,
    },
    SetInboxWatch {
        enabled: bool,
    },
    Classify {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        doc_types: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        force: Option<bool>,
    },
    SetDocType {
        doc_type: String,
    },
    Ocr {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pages: Vec<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<OcrProfile>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        force: Option<bool>,
    },
    DefineSchema {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<SchemaMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
    },
    Extract {
        /// `auto` (type → schema mapping) or a schema name / id / doc-type slug.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fields: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        force: Option<bool>,
    },
    Validate {},
    Review {
        action: ReviewOp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only: Option<ReviewOnly>,
    },
    Reindex {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<ReindexScope>,
    },
    Search {
        query: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<SearchMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        top_k: Option<u32>,
    },
    Answer {
        question: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cite: Option<bool>,
    },
    Summarize {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<SummarizeTarget>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        length: Option<SummaryLength>,
    },
    Aggregate {
        #[serde(rename = "fn")]
        func: AggregateFn,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_by: Option<GroupBy>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include_unreviewed: Option<bool>,
    },
    Organize {
        /// File-name template (`{{year}}-{{month}}-{{day}}_{{vendor}}_{{total}}` …).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>,
        /// Folder template (`documents/{{year}}/{{month}}/{{doc_type}}`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder_template: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_by: Option<GroupBy>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only_reviewed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dry_run: Option<bool>,
    },
    Rename {
        template: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only_reviewed: Option<bool>,
    },
    Move {
        /// Destination folder template (workspace-relative).
        folder: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_by: Option<GroupBy>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only_reviewed: Option<bool>,
    },
    Tag {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        add: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remove: Vec<String>,
    },
    Delete {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_trash: Option<bool>,
    },
    Export {
        format: ExportFormat,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        columns: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<ExportLayout>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        include: Vec<ExportInclude>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<ExportTarget>,
    },
    SetModel {
        id: String,
    },
    SetProfile {
        profile: Profile,
    },
    SetTheme {
        theme: Theme,
    },
    SetLanguage {
        lang: Language,
    },
    SetSetting {
        key: SettingKey,
        value: Value,
    },
    Open {
        target: OpenTarget,
        #[serde(rename = "ref")]
        reference: String,
    },
    Job {
        action: JobOp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Engine {
        action: EngineOp,
    },
    Undo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    Redo {},
    Help {},
    Clarify {
        question: String,
    },
    Noop {},
}

impl Op {
    pub fn kind(&self) -> OpKind {
        match self {
            Op::Import { .. } => OpKind::Import,
            Op::SetInboxWatch { .. } => OpKind::SetInboxWatch,
            Op::Classify { .. } => OpKind::Classify,
            Op::SetDocType { .. } => OpKind::SetDocType,
            Op::Ocr { .. } => OpKind::Ocr,
            Op::DefineSchema { .. } => OpKind::DefineSchema,
            Op::Extract { .. } => OpKind::Extract,
            Op::Validate {} => OpKind::Validate,
            Op::Review { .. } => OpKind::Review,
            Op::Reindex { .. } => OpKind::Reindex,
            Op::Search { .. } => OpKind::Search,
            Op::Answer { .. } => OpKind::Answer,
            Op::Summarize { .. } => OpKind::Summarize,
            Op::Aggregate { .. } => OpKind::Aggregate,
            Op::Organize { .. } => OpKind::Organize,
            Op::Rename { .. } => OpKind::Rename,
            Op::Move { .. } => OpKind::Move,
            Op::Tag { .. } => OpKind::Tag,
            Op::Delete { .. } => OpKind::Delete,
            Op::Export { .. } => OpKind::Export,
            Op::SetModel { .. } => OpKind::SetModel,
            Op::SetProfile { .. } => OpKind::SetProfile,
            Op::SetTheme { .. } => OpKind::SetTheme,
            Op::SetLanguage { .. } => OpKind::SetLanguage,
            Op::SetSetting { .. } => OpKind::SetSetting,
            Op::Open { .. } => OpKind::Open,
            Op::Job { .. } => OpKind::Job,
            Op::Engine { .. } => OpKind::Engine,
            Op::Undo { .. } => OpKind::Undo,
            Op::Redo {} => OpKind::Redo,
            Op::Help {} => OpKind::Help,
            Op::Clarify { .. } => OpKind::Clarify,
            Op::Noop {} => OpKind::Noop,
        }
    }

    /// Side effect class (appendix D "부작용"): what a confirmation card has to say.
    pub fn side_effect(&self) -> SideEffect {
        match self {
            Op::Organize { dry_run: Some(true), .. } => SideEffect::None,
            Op::Organize { .. } | Op::Rename { .. } | Op::Move { .. } => SideEffect::Files,
            Op::Delete { .. } => SideEffect::Trash,
            Op::Export { .. } => SideEffect::FileCreate,
            Op::Undo { .. } | Op::Redo {} => SideEffect::Files,
            Op::Import { .. } => SideEffect::Database,
            Op::Classify { .. } | Op::SetDocType { .. } | Op::Ocr { .. } | Op::Extract { .. } | Op::Validate {} => {
                SideEffect::Database
            }
            Op::Review { action: ReviewOp::Show, .. } => SideEffect::None,
            Op::Review { .. } => SideEffect::Database,
            Op::Summarize { .. } | Op::Tag { .. } | Op::Reindex { .. } => SideEffect::Database,
            Op::DefineSchema { .. } => SideEffect::Database,
            Op::SetInboxWatch { .. }
            | Op::SetModel { .. }
            | Op::SetProfile { .. }
            | Op::SetTheme { .. }
            | Op::SetLanguage { .. }
            | Op::SetSetting { .. } => SideEffect::Settings,
            Op::Job { .. } | Op::Engine { .. } => SideEffect::Control,
            Op::Search { .. }
            | Op::Answer { .. }
            | Op::Aggregate { .. }
            | Op::Open { .. }
            | Op::Help {}
            | Op::Clarify { .. }
            | Op::Noop {} => SideEffect::None,
        }
    }

    /// Whether the op needs the confirmation card (appendix D "확인 카드").
    /// `export` overwriting an existing file and multi-document review actions
    /// are decided by the preview with the workspace at hand.
    pub fn needs_confirmation(&self) -> bool {
        match self {
            Op::Organize { dry_run: Some(true), .. } => false,
            Op::Organize { .. } | Op::Rename { .. } | Op::Move { .. } | Op::Delete { .. } => true,
            // Copying an arbitrary folder into the workspace is shown before it runs.
            Op::Import { source: ImportFrom::Folder, .. } => true,
            Op::Undo { .. } | Op::Redo {} => true,
            Op::Job { action: JobOp::Cancel, .. } => true,
            Op::Engine { action: EngineOp::Restart } => true,
            Op::DefineSchema { .. } => true,
            Op::Review { action: ReviewOp::Approve | ReviewOp::Correct | ReviewOp::Reject, .. } => true,
            _ => false,
        }
    }
}

/// Side-effect class of an op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffect {
    None,
    Database,
    Files,
    FileCreate,
    Trash,
    Settings,
    Control,
}

// ---------------------------------------------------------------------------
// Scope and plan
// ---------------------------------------------------------------------------

/// Which documents a plan works on (§3.5 `scope`). Empty = the whole library
/// (or whatever the UI selected: the assistant passes the current folder /
/// selection as the default scope).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Scope {
    /// Workspace-relative folder (`inbox`, `documents/2026/10`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub doc_ids: Vec<String>,
    pub filter: ScopeFilter,
}

impl Scope {
    pub fn is_empty(&self) -> bool {
        self.folder.is_none() && self.doc_ids.is_empty() && self.filter.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ScopeFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_type: Option<String>,
    /// ISO-8601 dates on the document date (`doc_date`, else the import date).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Document status (`new`, `extracted`, `reviewed` …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_status: Option<ReviewStatus>,
}

impl ScopeFilter {
    pub fn is_empty(&self) -> bool {
        self.doc_type.is_none()
            && self.date_from.is_none()
            && self.date_to.is_none()
            && self.keyword.is_none()
            && self.vendor.is_none()
            && self.status.is_none()
            && self.tags.is_empty()
            && self.min_confidence.is_none()
            && self.review_status.is_none()
    }

    pub fn date_range(&self) -> Option<(NaiveDate, NaiveDate)> {
        let from = self.date_from.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
        let to = self.date_to.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
        match (from, to) {
            (Some(f), Some(t)) => Some((f, t)),
            (Some(f), None) => Some((f, NaiveDate::from_ymd_opt(9999, 12, 31)?)),
            (None, Some(t)) => Some((NaiveDate::from_ymd_opt(1900, 1, 1)?, t)),
            (None, None) => None,
        }
    }
}

/// The typed plan (§3.5 PlanSchema).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub intent: OpKind,
    #[serde(default)]
    pub scope: Scope,
    pub operations: Vec<Op>,
    #[serde(default)]
    pub requires_confirmation: bool,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clarification_question: Option<String>,
    /// One-line interpretation shown on the plan card ("해석: …").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl Plan {
    pub fn single(op: Op, confidence: f32, summary: impl Into<String>) -> Plan {
        let intent = op.kind();
        let requires_confirmation = op.needs_confirmation();
        Plan {
            intent,
            scope: Scope::default(),
            operations: vec![op],
            requires_confirmation,
            confidence,
            clarification_question: None,
            summary: Some(summary.into()),
        }
    }

    pub fn clarify(question: impl Into<String>, confidence: f32) -> Plan {
        let q = question.into();
        Plan {
            intent: OpKind::Clarify,
            scope: Scope::default(),
            operations: vec![Op::Clarify { question: q.clone() }],
            requires_confirmation: false,
            confidence,
            clarification_question: Some(q),
            summary: None,
        }
    }

    pub fn is_clarification(&self) -> bool {
        self.intent == OpKind::Clarify
            || self.operations.iter().all(|o| matches!(o, Op::Clarify { .. }))
            || (self.operations.is_empty() && self.clarification_question.is_some())
    }

    pub fn has_side_effects(&self) -> bool {
        self.operations.iter().any(|o| o.side_effect() != SideEffect::None)
    }

    pub fn needs_model(&self) -> bool {
        self.operations.iter().any(|o| o.kind().needs_model())
    }

    /// The dominant intent: the first op that is not a pure filter / no-op.
    pub fn recompute_intent(&mut self) {
        if let Some(op) = self.operations.iter().find(|o| !matches!(o, Op::Noop {})) {
            self.intent = op.kind();
        }
    }
}

// ---------------------------------------------------------------------------
// Context handed to the planner (what the model may refer to)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocTypeSummary {
    pub slug: String,
    pub label_ko: String,
    pub label_en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSummary {
    pub key: String,
    pub label_ko: String,
    pub label_en: String,
    /// `amount`, `date`, `text`, `table` … (`FieldKind::as_str`).
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaSummary {
    pub id: String,
    pub name: String,
    pub name_en: String,
    pub doc_type: String,
    pub fields: Vec<FieldSummary>,
}

/// Workspace facts the planner grounds its plans in (built by the shell).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanContext {
    pub today: NaiveDate,
    pub language: Language,
    /// Folders that contain documents (`inbox`, `documents/2026/10` …).
    pub folders: Vec<String>,
    pub doc_types: Vec<DocTypeSummary>,
    pub schemas: Vec<SchemaSummary>,
    pub tags: Vec<String>,
    /// Folder the UI is showing (library) — default scope for "여기 있는 문서".
    pub current_folder: Option<String>,
    /// Documents selected in the library / open in the viewer.
    pub selected_doc_ids: Vec<String>,
    pub current_document: Option<String>,
    pub current_page: Option<u32>,
    pub document_count: u64,
    /// Ids of models the user may switch to (`set_model`).
    pub model_ids: Vec<String>,
}

impl Default for PlanContext {
    fn default() -> Self {
        Self {
            today: chrono::Local::now().date_naive(),
            language: Language::Ko,
            folders: Vec::new(),
            doc_types: Vec::new(),
            schemas: Vec::new(),
            tags: Vec::new(),
            current_folder: None,
            selected_doc_ids: Vec::new(),
            current_document: None,
            current_page: None,
            document_count: 0,
            model_ids: Vec::new(),
        }
    }
}

impl PlanContext {
    /// Resolves a document type mentioned by slug or label (either language).
    pub fn doc_type(&self, text: &str) -> Option<&DocTypeSummary> {
        let key = compact(text);
        if key.is_empty() {
            return None;
        }
        self.doc_types
            .iter()
            .find(|d| compact(&d.slug) == key || compact(&d.label_ko) == key || compact(&d.label_en) == key)
    }

    pub fn schema_for_type(&self, slug: &str) -> Option<&SchemaSummary> {
        self.schemas.iter().find(|s| s.doc_type == slug)
    }

    /// Resolves a schema by id, name (either language) or doc-type slug.
    pub fn schema(&self, reference: &str) -> Option<&SchemaSummary> {
        let key = compact(reference);
        self.schemas.iter().find(|s| {
            s.id == reference || compact(&s.name) == key || compact(&s.name_en) == key || compact(&s.doc_type) == key
        })
    }

    /// Resolves a field of a schema by key or label.
    pub fn field<'a>(&self, schema: &'a SchemaSummary, text: &str) -> Option<&'a FieldSummary> {
        let key = compact(text);
        schema
            .fields
            .iter()
            .find(|f| compact(&f.key) == key || compact(&f.label_ko) == key || compact(&f.label_en) == key)
    }
}

/// Lower-case, whitespace / punctuation stripped comparison key.
pub fn compact(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

// ---------------------------------------------------------------------------
// Planner results
// ---------------------------------------------------------------------------

/// A validation finding (`path` = JSON pointer-ish location, `code` = stable id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanIssue {
    pub path: String,
    pub code: String,
    pub message: String,
    /// Errors block the plan (re-query / clarification); warnings are shown on the card.
    pub blocking: bool,
}

impl PlanIssue {
    pub fn error(path: impl Into<String>, code: impl Into<String>, message: impl Into<String>) -> PlanIssue {
        PlanIssue { path: path.into(), code: code.into(), message: message.into(), blocking: true }
    }
    pub fn warn(path: impl Into<String>, code: impl Into<String>, message: impl Into<String>) -> PlanIssue {
        PlanIssue { path: path.into(), code: code.into(), message: message.into(), blocking: false }
    }
}

/// True when any issue blocks execution.
pub fn has_errors(issues: &[PlanIssue]) -> bool {
    issues.iter().any(|i| i.blocking)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanSource {
    Prerouter,
    Rules,
    Model,
    /// No plan: the model was unavailable or every attempt was rejected.
    None,
}

/// Outcome of planning one command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanResult {
    pub text: String,
    pub source: PlanSource,
    /// The validated plan (a clarification plan when the command could not be interpreted).
    pub plan: Plan,
    /// Validator findings of the last attempt (empty when the plan is clean).
    pub issues: Vec<PlanIssue>,
    /// Relative-date phrase the pre-router resolved ("지난달" → 2026-10-01 … 10-31).
    pub date: Option<DateRange>,
    pub attempts: u32,
    pub model: Option<String>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Raw model output of the last attempt (diagnostics).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}
