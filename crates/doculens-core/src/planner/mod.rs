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
pub mod guard;
pub mod llm;
pub mod prompts;
pub mod rules;
pub mod schema;
pub mod scope;
pub mod settings_catalog;
pub mod validate;

#[cfg(test)]
mod tests;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::documents::ReviewStatus;
use crate::generate::{Length, OutputFormat, Tone};
use crate::knowledge::{ConversationTurn, SearchMode};
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
    DraftDocument,
    /// 0.7.0: revise the open draft (a section, a new section or the whole).
    ReviseDraft,
    RegisterForm,
    Explain,
    Compare,
    CreateProject,
    AddToProject,
    DefineRule,
    CheckConsistency,
    RequestSupplement,
    SetRetention,
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
    // -- 0.6.0: every GUI function as an op ------------------------------------------
    Rotate,
    Rerender,
    Split,
    Merge,
    SetTitle,
    EditText,
    SetRole,
    ManageReference,
    DefineTerm,
    SetVersion,
    UpdateProject,
    DeleteProject,
    RemoveFromProject,
    ManageTemplate,
    ClassifyProjects,
    ManageSchema,
    ManageModel,
    Workspace,
    Maintenance,
}

impl OpKind {
    pub const ALL: [OpKind; 63] = [
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
        OpKind::DraftDocument,
        OpKind::ReviseDraft,
        OpKind::RegisterForm,
        OpKind::Explain,
        OpKind::Compare,
        OpKind::CreateProject,
        OpKind::AddToProject,
        OpKind::DefineRule,
        OpKind::CheckConsistency,
        OpKind::RequestSupplement,
        OpKind::SetRetention,
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
        OpKind::Rotate,
        OpKind::Rerender,
        OpKind::Split,
        OpKind::Merge,
        OpKind::SetTitle,
        OpKind::EditText,
        OpKind::SetRole,
        OpKind::ManageReference,
        OpKind::DefineTerm,
        OpKind::SetVersion,
        OpKind::UpdateProject,
        OpKind::DeleteProject,
        OpKind::RemoveFromProject,
        OpKind::ManageTemplate,
        OpKind::ClassifyProjects,
        OpKind::ManageSchema,
        OpKind::ManageModel,
        OpKind::Workspace,
        OpKind::Maintenance,
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
            OpKind::DraftDocument => "draft_document",
            OpKind::ReviseDraft => "revise_draft",
            OpKind::RegisterForm => "register_form",
            OpKind::Explain => "explain",
            OpKind::Compare => "compare",
            OpKind::CreateProject => "create_project",
            OpKind::AddToProject => "add_to_project",
            OpKind::DefineRule => "define_rule",
            OpKind::CheckConsistency => "check_consistency",
            OpKind::RequestSupplement => "request_supplement",
            OpKind::SetRetention => "set_retention",
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
            OpKind::Rotate => "rotate",
            OpKind::Rerender => "rerender",
            OpKind::Split => "split",
            OpKind::Merge => "merge",
            OpKind::SetTitle => "set_title",
            OpKind::EditText => "edit_text",
            OpKind::SetRole => "set_role",
            OpKind::ManageReference => "manage_reference",
            OpKind::DefineTerm => "define_term",
            OpKind::SetVersion => "set_version",
            OpKind::UpdateProject => "update_project",
            OpKind::DeleteProject => "delete_project",
            OpKind::RemoveFromProject => "remove_from_project",
            OpKind::ManageTemplate => "manage_template",
            OpKind::ClassifyProjects => "classify_projects",
            OpKind::ManageSchema => "manage_schema",
            OpKind::ManageModel => "manage_model",
            OpKind::Workspace => "workspace",
            OpKind::Maintenance => "maintenance",
        }
    }

    /// The op family of the two-stage planner (0.6.0 plan §2); `Open`, `Help`, `Clarify` and
    /// `Noop` are shared by every family (`OpFamily::ops` adds them).
    pub fn family(self) -> OpFamily {
        match self {
            OpKind::Search | OpKind::Answer | OpKind::Summarize | OpKind::Explain | OpKind::Compare => OpFamily::Ask,
            OpKind::Import
            | OpKind::Classify
            | OpKind::SetDocType
            | OpKind::Ocr
            | OpKind::Extract
            | OpKind::Validate
            | OpKind::Review
            | OpKind::Tag
            | OpKind::Rename
            | OpKind::Move
            | OpKind::Organize
            | OpKind::Delete
            | OpKind::Rotate
            | OpKind::Rerender
            | OpKind::Split
            | OpKind::Merge
            | OpKind::SetTitle
            | OpKind::EditText => OpFamily::Documents,
            OpKind::ManageReference | OpKind::DefineTerm | OpKind::SetVersion | OpKind::Reindex | OpKind::SetRole => {
                OpFamily::Reference
            }
            OpKind::CreateProject
            | OpKind::UpdateProject
            | OpKind::DeleteProject
            | OpKind::AddToProject
            | OpKind::RemoveFromProject
            | OpKind::ManageTemplate
            | OpKind::ClassifyProjects
            | OpKind::DefineRule
            | OpKind::CheckConsistency
            | OpKind::RequestSupplement => OpFamily::Projects,
            OpKind::DefineSchema | OpKind::RegisterForm | OpKind::ManageSchema => OpFamily::Forms,
            OpKind::DraftDocument => OpFamily::Compose,
            OpKind::ReviseDraft => OpFamily::Compose,
            OpKind::Aggregate | OpKind::Export => OpFamily::Output,
            OpKind::SetSetting
            | OpKind::SetTheme
            | OpKind::SetLanguage
            | OpKind::SetModel
            | OpKind::SetProfile
            | OpKind::SetRetention
            | OpKind::SetInboxWatch
            | OpKind::ManageModel
            | OpKind::Engine
            | OpKind::Workspace
            | OpKind::Maintenance
            | OpKind::Job
            | OpKind::Undo
            | OpKind::Redo
            | OpKind::Open
            | OpKind::Help
            | OpKind::Clarify
            | OpKind::Noop => OpFamily::System,
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
                | OpKind::RegisterForm
                | OpKind::SetRetention
                | OpKind::ManageModel
                | OpKind::Workspace
                | OpKind::Maintenance
        )
    }

    /// Ops that need the vision / chat model (the executor loads it first).
    pub fn needs_model(self) -> bool {
        matches!(
            self,
            OpKind::Classify
                | OpKind::Ocr
                | OpKind::Extract
                | OpKind::Answer
                | OpKind::Summarize
                | OpKind::Import
                | OpKind::DraftDocument
                | OpKind::ReviseDraft
                | OpKind::Explain
                | OpKind::Compare
                | OpKind::DefineRule
                | OpKind::RequestSupplement
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

/// What the documents of a `summarize` are grouped by (0.8.0 plan §1.4): one combined summary per
/// group ("프로젝트별로 요약해줘"). The executor does the grouping, not the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryGroup {
    None,
    Project,
    DocType,
    Folder,
}

/// How a turn that is not a command is answered (0.8.0 plan §1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerMode {
    /// Greeting / thanks / small talk: a short reply, nothing is looked up.
    Chat,
    /// A knowledge question that is not about the documents: evidence is used when the library
    /// has some, general knowledge (marked as such) otherwise.
    General,
}

impl AnswerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AnswerMode::Chat => "chat",
            AnswerMode::General => "general",
        }
    }
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
    /// By the document's project (its first project; the scope's project when one is set).
    Project,
    #[default]
    None,
}

impl GroupBy {
    pub fn as_str(self) -> &'static str {
        match self {
            GroupBy::Month => "month",
            GroupBy::Vendor => "vendor",
            GroupBy::DocType => "doc_type",
            GroupBy::Project => "project",
            GroupBy::None => "none",
        }
    }
}

/// `explain.scope` (appendix D): where the evidence comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExplainScope {
    #[default]
    Kb,
    /// The 공통 참고 자료 (pre-0.5.0 `collection`; the old name still deserialises).
    #[serde(alias = "collection")]
    Reference,
    Project,
    Document,
}

impl ExplainScope {
    pub fn as_str(self) -> &'static str {
        match self {
            ExplainScope::Kb => "kb",
            ExplainScope::Reference => "reference",
            ExplainScope::Project => "project",
            ExplainScope::Document => "document",
        }
    }
}

/// `set_retention.item` (appendix D; the page cache is size-capped and is set through
/// `set_setting cache_limit_gb` instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionTarget {
    Documents,
    Extractions,
    /// Job logs and app logs together.
    Logs,
    Journal,
    Chats,
    Temp,
}

impl RetentionTarget {
    pub const ALL: [RetentionTarget; 6] = [
        RetentionTarget::Documents,
        RetentionTarget::Extractions,
        RetentionTarget::Logs,
        RetentionTarget::Journal,
        RetentionTarget::Chats,
        RetentionTarget::Temp,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RetentionTarget::Documents => "documents",
            RetentionTarget::Extractions => "extractions",
            RetentionTarget::Logs => "logs",
            RetentionTarget::Journal => "journal",
            RetentionTarget::Chats => "chats",
            RetentionTarget::Temp => "temp",
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
    /// One sheet (XLSX) / file (CSV) per project (plan §6); documents outside projects share one.
    SheetPerProject,
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
    Start,
    Stop,
}

// -- 0.6.0 parameter enums -------------------------------------------------------------

/// `set_role.role` (the `generated` role is set by the app, never by a command).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleParam {
    Work,
    Knowledge,
    Form,
}

impl RoleParam {
    pub fn as_role(self) -> crate::documents::DocumentRole {
        match self {
            RoleParam::Work => crate::documents::DocumentRole::Work,
            RoleParam::Knowledge => crate::documents::DocumentRole::Knowledge,
            RoleParam::Form => crate::documents::DocumentRole::Form,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceAction {
    Create,
    Rename,
    Delete,
    Enable,
    Disable,
    Add,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TermAction {
    #[default]
    Save,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TemplateAction {
    #[default]
    Save,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaAction {
    Enable,
    Disable,
    SetDefault,
    Delete,
    Rename,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelAction {
    SetTier,
    SetBackend,
    Import,
    Download,
    Delete,
    Verify,
    Benchmark,
    Load,
    Unload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceAction {
    Open,
    Create,
    Close,
    Forget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceAction {
    PurgeNow,
    ClearFinishedJobs,
    Rescan,
    InboxScan,
}

/// Op families of the two-stage planner (0.6.0 plan §2): the first model call picks one or two
/// families, the second plans with that family's op table only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpFamily {
    Ask,
    Documents,
    Reference,
    Projects,
    Forms,
    Compose,
    Output,
    System,
}

impl OpFamily {
    pub const ALL: [OpFamily; 8] = [
        OpFamily::Ask,
        OpFamily::Documents,
        OpFamily::Reference,
        OpFamily::Projects,
        OpFamily::Forms,
        OpFamily::Compose,
        OpFamily::Output,
        OpFamily::System,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            OpFamily::Ask => "ask",
            OpFamily::Documents => "documents",
            OpFamily::Reference => "reference",
            OpFamily::Projects => "projects",
            OpFamily::Forms => "forms",
            OpFamily::Compose => "compose",
            OpFamily::Output => "output",
            OpFamily::System => "system",
        }
    }

    pub fn parse(s: &str) -> Option<OpFamily> {
        OpFamily::ALL.into_iter().find(|f| f.as_str() == s)
    }

    /// One-line description for the family-selection prompt.
    pub fn describe(self, ko: bool) -> &'static str {
        match (self, ko) {
            (OpFamily::Ask, true) => "질문·검색·요약·설명·비교(문서나 참고 자료의 내용을 묻는 말)",
            (OpFamily::Ask, false) => "questions, search, summaries, explanations, comparisons (asking about content)",
            (OpFamily::Documents, true) => "문서 가져오기·분류·OCR·추출·검증·검토·태그·이름/폴더 정리·삭제·회전·다시 렌더·분할·병합·제목·글자 수정",
            (OpFamily::Documents, false) => "import, classify, OCR, extract, validate, review, tag, organise/rename/move, delete, rotate, re-render, split, merge, title, text edits",
            (OpFamily::Reference, true) => "참고 자료 묶음·용어집·문서 버전·역할(업무/참고/양식)·색인",
            (OpFamily::Reference, false) => "reference groups, glossary, document versions, roles, indexing",
            (OpFamily::Projects, true) => "프로젝트 만들기·수정·삭제, 문서 넣기/빼기, 템플릿, 자동 분류, 정합성 규칙·검사, 보완요청서",
            (OpFamily::Projects, false) => "create / update / delete projects, membership, templates, auto-classification, rules and checks, supplement requests",
            (OpFamily::Forms, true) => "양식 등록·추출 항목(스키마) 정의·수정·기본값·끄기·삭제",
            (OpFamily::Forms, false) => "form registration, extraction schemas (define, modify, default, enable, delete)",
            (OpFamily::Compose, true) => "문서 초안 작성(견적서·보고서·계약서 등 생성)",
            (OpFamily::Compose, false) => "drafting documents (quotes, reports, contracts …)",
            (OpFamily::Output, true) => "집계(합계·건수·평균)와 내보내기(엑셀·CSV·JSONL·MD·PDF)",
            (OpFamily::Output, false) => "aggregates (sum / count / average) and exports (xlsx / csv / jsonl / md / pdf)",
            (OpFamily::System, true) => "설정·테마·언어·모델·엔진·워크스페이스·보존·작업 제어·되돌리기·화면 열기·도움말",
            (OpFamily::System, false) => "settings, theme, language, models, engine, workspace, retention, job control, undo, navigation, help",
        }
    }

    /// Ops of the family plus the shared ones (`open`, `help`, `clarify`, `noop`), in `ALL` order.
    pub fn ops(self) -> Vec<OpKind> {
        OpKind::ALL
            .into_iter()
            .filter(|k| k.family() == self || matches!(k, OpKind::Open | OpKind::Help | OpKind::Clarify | OpKind::Noop))
            .collect()
    }

    /// Union of several families' ops (deduplicated, `ALL` order).
    pub fn ops_of(families: &[OpFamily]) -> Vec<OpKind> {
        OpKind::ALL
            .into_iter()
            .filter(|k| {
                families.iter().any(|f| k.family() == *f)
                    || matches!(k, OpKind::Open | OpKind::Help | OpKind::Clarify | OpKind::Noop)
            })
            .collect()
    }
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
        /// Files to import when `source = files` (absolute paths the user typed or dropped; 0.6.0).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        paths: Vec<String>,
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
        /// `false`: the user supplied the text to work on (다듬기 · 교정 · 번역 · 정리) — answer from that
        /// text alone, without looking documents up.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        knowledge: Option<bool>,
        /// `chat` / `general`: the turn is not a document question (0.8.0 plan §1.2). Absent = a
        /// grounded answer (or, with `knowledge = false`, work on the user's own text).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<AnswerMode>,
        /// A ready reply (fixed greeting of the fast path): shown as is, no model call.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply: Option<String>,
    },
    Summarize {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<SummarizeTarget>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        length: Option<SummaryLength>,
        /// One combined summary per project / document type / folder.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_by: Option<SummaryGroup>,
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
    /// §3.13 document generation: the draft is produced by the executor and opened in the
    /// 문서 생성 screen (saving / registering happens there).
    DraftDocument {
        /// Registered form / schema (id, name, doc-type slug) or a built-in template name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        form_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format_description: Option<String>,
        content: String,
        /// Document ids to quote, or `selection` (selected documents) / `aggregate` (last aggregate card).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sources: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<OutputFormat>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tone: Option<Tone>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        length: Option<Length>,
    },
    /// 0.7.0 plan §1.5: revise the draft open in the 문서 작성 screen (or `draft_id`) — one
    /// section rewritten, a new section inserted or the whole draft regenerated per `instruction`.
    ReviseDraft {
        /// Draft id; `None` = the draft open in the document generator (`PlanContext::draft_id`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        draft_id: Option<String>,
        /// `section` | `insert_after` | `whole`.
        scope: String,
        /// 1-based section ordinal (`section`: the one rewritten; `insert_after`: insert after it, none = at the end).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<u32>,
        /// Title of the inserted section (optional).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        heading: Option<String>,
        instruction: String,
    },
    /// §3.4.0 form registration — runs in the UI (field grid confirmation).
    RegisterForm {
        /// Document id or `current`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        doc_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        doc_type_slug: Option<String>,
        /// Several sample documents (0.3.0 §2 multi-document discovery); `doc_ref` names one.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        doc_ids: Vec<String>,
    },
    /// §3.14.2 grounded explanation of a selection / question.
    Explain {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selection: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        question: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<ExplainScope>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_general_knowledge: Option<bool>,
    },
    /// §3.14.3 comparison summary of 2–5 documents.
    Compare {
        doc_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        aspects: Vec<String>,
    },
    /// §3.15 projects.
    CreateProject {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        doc_types_required: Vec<String>,
        /// ISO dates (plan §5: 시작·종료일; both optional).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_date: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_date: Option<String>,
    },
    AddToProject {
        /// Project id or name; `None` = the project open in the UI.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        /// Defaults to the plan scope.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        doc_ids: Vec<String>,
        /// Put the documents in the project's reference group (참고 자료, role `knowledge`)
        /// instead of its work documents (0.5.0 plan A).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        as_reference: bool,
    },
    DefineRule {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>,
        description: String,
    },
    CheckConsistency {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        /// Rule ids; empty = all enabled rules.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rules: Vec<String>,
    },
    RequestSupplement {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        /// Finding (rule result) ids; empty = every mismatch / missing finding of the last run.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        findings: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<OutputFormat>,
    },
    /// §3.8 retention — runs in the UI (`workspace_set_retention`); `days < 0` = keep forever.
    SetRetention {
        item: RetentionTarget,
        days: i64,
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
    /// `key` is a settings-catalog path (`processing.render_dpi`; `planner::settings_catalog`),
    /// the 0.1–0.5 short keys (`render_dpi` …) still deserialise and are mapped by the validator.
    SetSetting {
        key: String,
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
    // -- 0.6.0: every GUI function as an op ------------------------------------------
    /// Rotate the page images of the documents in scope (the originals never change).
    Rotate {
        degrees: u16,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pages: Vec<u32>,
    },
    Rerender {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dpi: Option<u16>,
    },
    /// Split the one document in scope: `boundaries` = pages that start a new document, or `every`.
    Split {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        boundaries: Vec<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        every: Option<u32>,
    },
    Merge {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    SetTitle {
        title: String,
    },
    /// Find / replace in the page text of the documents in scope (recorded in the text history).
    EditText {
        find: String,
        replace: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pages: Vec<u32>,
    },
    SetRole {
        role: RoleParam,
    },
    /// Reference groups (참고 자료 묶음): the common ones by `group` (name / id), a project's by `project`.
    ManageReference {
        action: ReferenceAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
    },
    DefineTerm {
        #[serde(default)]
        action: TermAction,
        term: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        definition: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        synonyms: Vec<String>,
    },
    /// Mark the documents in scope as editions of one title (`clear` removes the mark).
    SetVersion {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effective_date: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        latest: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clear: Option<bool>,
    },
    UpdateProject {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_date: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_date: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intake_no: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keywords: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inbox_folder: Option<String>,
    },
    DeleteProject {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
    },
    RemoveFromProject {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        doc_ids: Vec<String>,
    },
    ManageTemplate {
        #[serde(default)]
        action: TemplateAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        doc_types_required: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// Suggest projects for the documents in scope (empty scope = every unassigned document);
    /// `apply` assigns the confident suggestions.
    ClassifyProjects {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        apply: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        use_model: Option<bool>,
    },
    ManageSchema {
        action: SchemaAction,
        schema: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    ManageModel {
        action: ModelAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tier: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backend: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    Workspace {
        action: WorkspaceAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    Maintenance {
        action: MaintenanceAction,
    },
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
            Op::DraftDocument { .. } => OpKind::DraftDocument,
            Op::ReviseDraft { .. } => OpKind::ReviseDraft,
            Op::RegisterForm { .. } => OpKind::RegisterForm,
            Op::Explain { .. } => OpKind::Explain,
            Op::Compare { .. } => OpKind::Compare,
            Op::CreateProject { .. } => OpKind::CreateProject,
            Op::AddToProject { .. } => OpKind::AddToProject,
            Op::DefineRule { .. } => OpKind::DefineRule,
            Op::CheckConsistency { .. } => OpKind::CheckConsistency,
            Op::RequestSupplement { .. } => OpKind::RequestSupplement,
            Op::SetRetention { .. } => OpKind::SetRetention,
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
            Op::Rotate { .. } => OpKind::Rotate,
            Op::Rerender { .. } => OpKind::Rerender,
            Op::Split { .. } => OpKind::Split,
            Op::Merge { .. } => OpKind::Merge,
            Op::SetTitle { .. } => OpKind::SetTitle,
            Op::EditText { .. } => OpKind::EditText,
            Op::SetRole { .. } => OpKind::SetRole,
            Op::ManageReference { .. } => OpKind::ManageReference,
            Op::DefineTerm { .. } => OpKind::DefineTerm,
            Op::SetVersion { .. } => OpKind::SetVersion,
            Op::UpdateProject { .. } => OpKind::UpdateProject,
            Op::DeleteProject { .. } => OpKind::DeleteProject,
            Op::RemoveFromProject { .. } => OpKind::RemoveFromProject,
            Op::ManageTemplate { .. } => OpKind::ManageTemplate,
            Op::ClassifyProjects { .. } => OpKind::ClassifyProjects,
            Op::ManageSchema { .. } => OpKind::ManageSchema,
            Op::ManageModel { .. } => OpKind::ManageModel,
            Op::Workspace { .. } => OpKind::Workspace,
            Op::Maintenance { .. } => OpKind::Maintenance,
        }
    }

    /// Side effect class (appendix D "부작용"): what a confirmation card has to say.
    pub fn side_effect(&self) -> SideEffect {
        match self {
            Op::Organize { dry_run: Some(true), .. } => SideEffect::None,
            Op::Organize { .. } | Op::Rename { .. } | Op::Move { .. } => SideEffect::Files,
            Op::Delete { .. } => SideEffect::Trash,
            Op::Export { .. } => SideEffect::FileCreate,
            Op::DraftDocument { .. } | Op::RequestSupplement { .. } | Op::ReviseDraft { .. } => SideEffect::FileCreate,
            Op::RegisterForm { .. }
            | Op::CreateProject { .. }
            | Op::AddToProject { .. }
            | Op::DefineRule { .. }
            | Op::CheckConsistency { .. } => SideEffect::Database,
            Op::SetRetention { .. } => SideEffect::Settings,
            Op::Explain { .. } | Op::Compare { .. } => SideEffect::None,
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
            // 0.6.0
            Op::Rotate { .. }
            | Op::Rerender { .. }
            | Op::SetTitle { .. }
            | Op::EditText { .. }
            | Op::SetRole { .. } => SideEffect::Database,
            Op::Split { .. } | Op::Merge { .. } => SideEffect::FileCreate,
            Op::ManageReference { .. } | Op::DefineTerm { .. } | Op::SetVersion { .. } => SideEffect::Database,
            Op::UpdateProject { .. } | Op::ManageTemplate { .. } | Op::ManageSchema { .. } => SideEffect::Database,
            Op::DeleteProject { .. } | Op::RemoveFromProject { .. } => SideEffect::Files,
            Op::ClassifyProjects { apply: Some(true), .. } => SideEffect::Database,
            Op::ClassifyProjects { .. } => SideEffect::None,
            Op::ManageModel { action: ModelAction::Delete | ModelAction::Import | ModelAction::Download, .. } => {
                SideEffect::Files
            }
            Op::ManageModel { .. } => SideEffect::Settings,
            Op::Workspace { .. } => SideEffect::Control,
            Op::Maintenance { action: MaintenanceAction::PurgeNow, .. } => SideEffect::Trash,
            Op::Maintenance { .. } => SideEffect::Control,
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
            // Model-generated documents, rule grids, form field grids and shorter retention are shown first.
            Op::DraftDocument { .. }
            | Op::RequestSupplement { .. }
            | Op::DefineRule { .. }
            | Op::RegisterForm { .. }
            | Op::SetRetention { .. }
            // Creates rows (and possibly an ad-hoc template) — shown before it runs.
            | Op::CreateProject { .. } => true,
            Op::Review { action: ReviewOp::Approve | ReviewOp::Correct | ReviewOp::Reject, .. } => true,
            // 0.6.0: files are written, rows deleted, the network used or the engine stopped.
            Op::Import { source: ImportFrom::Files, .. } => true,
            Op::Split { .. } | Op::Merge { .. } | Op::EditText { .. } => true,
            Op::ManageReference { action: ReferenceAction::Delete, .. } => true,
            Op::DeleteProject { .. } => true,
            Op::ManageTemplate { action: TemplateAction::Delete, .. } => true,
            Op::ClassifyProjects { apply: Some(true), .. } => true,
            Op::ManageSchema { action: SchemaAction::Delete, .. } => true,
            Op::SetSetting { key, .. } => settings_catalog::needs_confirmation(key),
            Op::ManageModel { action: ModelAction::Download | ModelAction::Delete, .. } => true,
            Op::Engine { action: EngineOp::Stop } => true,
            Op::Workspace { action: WorkspaceAction::Close | WorkspaceAction::Forget, .. } => true,
            Op::Maintenance { action: MaintenanceAction::PurgeNow } => true,
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
    /// Documents of one project (id; the validator resolves names) — plan §6 project scope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

impl ScopeFilter {
    pub fn is_empty(&self) -> bool {
        self.doc_type.is_none()
            && self.date_from.is_none()
            && self.date_to.is_none()
            && self.keyword.is_none()
            && self.vendor.is_none()
            && self.status.is_none()
            && self.project_id.is_none()
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
    /// The part of the request the operations do not cover (0.8.0 plan §1.3) — shown on the card;
    /// a plan that leaves something out is confirmed instead of run at once. Empty / absent = none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmet: Option<String>,
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
            unmet: None,
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
            unmet: None,
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

/// How the assistant interprets free text (user decision 2026-11-09): the model reads every
/// sentence — only slash commands and the cancel / ask-back controls stay deterministic. The
/// phrase rules remain for the evaluation harness and as an explicit legacy mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanMode {
    /// Slash commands → pre-router; everything else → the model (no model → clarification).
    #[default]
    ModelFirst,
    /// Pre-router and phrase rules first, the model only when they cannot interpret the text.
    RulesFirst,
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
    /// Projects of the workspace (§3.15) — `add_to_project` / `check_consistency` targets.
    pub projects: Vec<ProjectSummary>,
    /// Project open in the UI (default target of project ops).
    pub current_project: Option<String>,
    /// Project the assistant's "현재 프로젝트" scope names (0.5.0): the default scope of a command
    /// that names no documents / folder / project of its own, and the evidence scope of questions
    /// (work documents ∪ reference material ∪ 공통 참고 자료).
    #[serde(default)]
    pub scope_project: Option<String>,
    /// 0.7.0: the draft open in the 문서 작성 screen (default target of `revise_draft`).
    pub draft_id: Option<String>,
    /// Earlier turns of the assistant conversation (oldest first), for commands that refer back
    /// ("그것도 엑셀로", "같은 폴더로").
    #[serde(default)]
    pub history: Vec<ConversationTurn>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    /// The computed phase: `planned` | `active` | `done` (0.3.0; the field keeps its old name so
    /// stored plans and the evaluation context still load).
    pub status: String,
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
            projects: Vec::new(),
            current_project: None,
            scope_project: None,
            draft_id: None,
            history: Vec::new(),
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

    /// Resolves a project by id or name (either exact or compacted); `None` input → the current project.
    pub fn project(&self, reference: Option<&str>) -> Option<&ProjectSummary> {
        match reference {
            None => self.current_project.as_deref().and_then(|id| self.projects.iter().find(|p| p.id == id)),
            Some(r) => {
                let key = compact(r);
                self.projects.iter().find(|p| p.id == r || compact(&p.name) == key)
            }
        }
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
    /// The whole-sentence fast path (0.8.0 plan §1.6): a settings sentence matched against the
    /// settings catalog, or a fixed greeting — no model call.
    Fast,
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
    /// Op families the first model call chose (0.6.0 two-stage planning; empty = single stage).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub families: Vec<OpFamily>,
    /// `chat` / `general` when the first model call read the turn as conversation or a general
    /// question and no plan call was made (0.8.0 plan §1.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
}
