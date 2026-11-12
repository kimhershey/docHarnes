//! Project-level multi-document analysis (design §3.15, DocuOps-inspired):
//! documents are grouped into **projects** (심사 건·거래처 건·과제), a project
//! **template** lists the required document types and a set of **consistency
//! rules**; rules are written in natural language, converted to rule JSON
//! (phrase rules first, grammar-constrained model fallback), confirmed by the
//! user and then executed **deterministically** against the latest extracted
//! fields of the project's documents. Results are pass / mismatch /
//! undetermined / missing with evidence (document, field, page). Findings feed
//! a 보완요청서 draft (`generate` module) and a project summary report
//! (§3.14.3). Documents of a project are linked by shared entities (BRN,
//! contract number, amounts, dates, names) for the relation graph tab.
//!
//! Contract (types and function signatures) is fixed here; the submodules hold
//! the implementation. Storage: migration `0005_projects.sql`.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::documents::DocumentRecord;
use crate::error::Result;
use crate::generate::{DraftRequest, OutputFormat};
use crate::pipeline::vlm::Vlm;
use crate::planner::{ExportFormat, ExportLayout, GroupBy};
use crate::settings::Language;

pub mod assign;
pub mod check;
pub mod classify;
pub mod copies;
pub mod relations;
pub mod repo;
pub mod report;
pub mod rules;
#[cfg(test)]
mod tests;

/// Phase of a project, computed from its dates (0.3.0 plan §5; never stored): before the start
/// → 진행예정, from the end date on → 완료, otherwise 진행중. A missing start counts from the creation day,
/// a missing end keeps the project 진행중 (user decision 2026-11-10). The end date itself is 완료:
/// "완료 처리" stores today as the end date and must show at once, not on the next day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPhase {
    Planned,
    #[default]
    Active,
    Done,
}

impl ProjectPhase {
    pub const ALL: [ProjectPhase; 3] = [ProjectPhase::Planned, ProjectPhase::Active, ProjectPhase::Done];

    pub fn as_str(self) -> &'static str {
        match self {
            ProjectPhase::Planned => "planned",
            ProjectPhase::Active => "active",
            ProjectPhase::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<ProjectPhase> {
        ProjectPhase::ALL.into_iter().find(|k| k.as_str() == s.trim())
    }

    pub fn label(self, ko: bool) -> &'static str {
        match (self, ko) {
            (ProjectPhase::Planned, true) => "진행예정",
            (ProjectPhase::Active, true) => "진행중",
            (ProjectPhase::Done, true) => "완료",
            (ProjectPhase::Planned, false) => "Planned",
            (ProjectPhase::Active, false) => "Active",
            (ProjectPhase::Done, false) => "Done",
        }
    }
}

/// Parses an ISO date (`YYYY-MM-DD`, longer timestamps are cut to the date).
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    let head = s.get(..10).unwrap_or(s);
    NaiveDate::parse_from_str(head, "%Y-%m-%d").ok()
}

/// The phase for `today` (see [`ProjectPhase`]).
pub fn phase_for(
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
    created: NaiveDate,
    today: NaiveDate,
) -> ProjectPhase {
    let start = start.unwrap_or(created);
    if today < start {
        ProjectPhase::Planned
    } else if end.is_some_and(|e| e <= today) {
        ProjectPhase::Done
    } else {
        ProjectPhase::Active
    }
}

/// Per-project organise / export settings (plan §6), JSON in `projects.settings`. Every field is
/// optional: the organise and export screens fall back to the global defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSettings {
    /// Folder template used by "이 프로젝트 정리" and for the copies of shared documents
    /// (default [`DEFAULT_PROJECT_FOLDER`]).
    pub folder_template: Option<String>,
    /// File-name template (default: the global `organize::NAME_TEMPLATE`).
    pub name_template: Option<String>,
    pub group_by: Option<GroupBy>,
    pub export_format: Option<ExportFormat>,
    pub export_layout: Option<ExportLayout>,
    /// Workspace-relative export path template (default `exports/<date>_<project>.<ext>`).
    pub export_path: Option<String>,
    /// Keep a physical copy of a document that also belongs to another project (decision 6).
    pub copies: bool,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            folder_template: None,
            name_template: None,
            group_by: None,
            export_format: None,
            export_layout: None,
            export_path: None,
            copies: true,
        }
    }
}

/// Folder of a project's documents when the project has no template of its own.
pub const DEFAULT_PROJECT_FOLDER: &str = "documents/{{project}}/{{doc_type}}";

impl ProjectSettings {
    pub fn folder_template(&self) -> &str {
        self.folder_template.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(DEFAULT_PROJECT_FOLDER)
    }
}

/// State of a tracked copy (`document_copies.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CopyState {
    #[default]
    Synced,
    /// The copy was edited outside DocuLens (hash differs from the one written).
    Modified,
    /// The copy file is gone.
    Missing,
}

impl CopyState {
    pub fn as_str(self) -> &'static str {
        match self {
            CopyState::Synced => "synced",
            CopyState::Modified => "modified",
            CopyState::Missing => "missing",
        }
    }
    pub fn parse(s: &str) -> Option<CopyState> {
        [CopyState::Synced, CopyState::Modified, CopyState::Missing].into_iter().find(|k| k.as_str() == s.trim())
    }
}

/// A physical copy of a document in the folder of one of its further projects (decision 6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentCopy {
    pub document_id: String,
    pub project_id: String,
    /// Workspace-relative path of the copy.
    pub rel_path: String,
    pub content_hash: String,
    pub size_bytes: u64,
    pub state: CopyState,
    pub created_at: DateTime<Utc>,
    pub synced_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub template_id: Option<String>,
    /// 접수번호.
    pub intake_no: Option<String>,
    /// 담당.
    pub owner: Option<String>,
    /// 시작일 (ISO date; `None` = the creation day).
    pub start_date: Option<String>,
    /// 종료일 (ISO date; `None` = open-ended).
    pub end_date: Option<String>,
    /// Computed from the dates for today (plan §5).
    pub phase: ProjectPhase,
    /// 기한 경과: the end date has passed (it is before today) but the project is incomplete (required
    /// documents missing, mismatches in the last check, or never checked) — a badge, not a phase.
    pub overdue: bool,
    #[serde(default)]
    pub settings: ProjectSettings,
    pub notes: Option<String>,
    /// `inbox/<folder>` auto-mapping source when the project was created from an inbox subfolder.
    pub inbox_folder: Option<String>,
    /// Classification keywords (자동 분류): matched against document titles / names / text.
    #[serde(default)]
    pub keywords: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub document_count: u32,
    /// Required document types still missing (from the template checklist).
    pub missing_count: u32,
    pub last_check_at: Option<DateTime<Utc>>,
    /// Verdict counts of the last check run.
    pub last_pass: u32,
    pub last_mismatch: u32,
    pub last_undetermined: u32,
}

/// Create / update payload (`id = None` → create). On update an optional field left `None`
/// keeps the stored value and `Some("")` (blank) clears it; `name` is always required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProjectInput {
    pub id: Option<String>,
    pub name: String,
    pub template_id: Option<String>,
    pub intake_no: Option<String>,
    pub owner: Option<String>,
    /// ISO dates; on update `None` keeps, `Some("")` clears.
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub notes: Option<String>,
    pub inbox_folder: Option<String>,
    /// `None` keeps the stored settings.
    pub settings: Option<ProjectSettings>,
    /// `None` keeps the stored keywords, `Some(vec![])` clears them.
    pub keywords: Option<Vec<String>>,
}

/// A project template: required document types + rule set (built-ins + user-defined).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectTemplate {
    pub id: String,
    pub name_ko: String,
    pub name_en: String,
    pub description_ko: String,
    pub description_en: String,
    /// Document type slugs (`business_registration`, `tax_invoice` …).
    pub required_doc_types: Vec<String>,
    pub rules: Vec<Rule>,
    pub builtin: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TemplateInput {
    pub id: Option<String>,
    pub name_ko: String,
    pub name_en: Option<String>,
    pub description_ko: Option<String>,
    pub description_en: Option<String>,
    pub required_doc_types: Vec<String>,
    pub rules: Vec<Rule>,
}

/// Comparison operators (appendix D `define_rule.op`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOp {
    Eq,
    Neq,
    Gte,
    Lte,
    /// |left − right| ≤ tolerance % of right.
    WithinPct,
    /// Left field is present and non-empty (right unused).
    Present,
    /// Left value matches the regular expression in `right.value`.
    Matches,
}

impl RuleOp {
    pub const ALL: [RuleOp; 7] =
        [RuleOp::Eq, RuleOp::Neq, RuleOp::Gte, RuleOp::Lte, RuleOp::WithinPct, RuleOp::Present, RuleOp::Matches];

    pub fn as_str(self) -> &'static str {
        match self {
            RuleOp::Eq => "eq",
            RuleOp::Neq => "neq",
            RuleOp::Gte => "gte",
            RuleOp::Lte => "lte",
            RuleOp::WithinPct => "within_pct",
            RuleOp::Present => "present",
            RuleOp::Matches => "matches",
        }
    }

    pub fn parse(s: &str) -> Option<RuleOp> {
        RuleOp::ALL.into_iter().find(|k| k.as_str() == s.trim())
    }
}

/// A rule operand: a field of a document type, or a literal value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuleOperand {
    /// Document type slug (`tax_invoice`); `None` with `value` = literal.
    pub doc_type: Option<String>,
    /// Field key / path (`supplier.brn`, `grand_total`).
    pub field: Option<String>,
    /// Literal (number, text, regex for `matches`).
    pub value: Option<String>,
}

impl RuleOperand {
    pub fn field(doc_type: &str, field: &str) -> RuleOperand {
        RuleOperand { doc_type: Some(doc_type.into()), field: Some(field.into()), value: None }
    }
    pub fn literal(value: &str) -> RuleOperand {
        RuleOperand { doc_type: None, field: None, value: Some(value.into()) }
    }
    pub fn is_literal(&self) -> bool {
        self.doc_type.is_none() && self.field.is_none()
    }
}

/// Rule JSON (appendix D `define_rule` → 규칙 JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub id: String,
    /// The natural-language description the rule was made from (shown in the grid).
    pub description: String,
    pub left: RuleOperand,
    pub op: RuleOp,
    pub right: Option<RuleOperand>,
    /// Percent for `within_pct`, absolute tolerance for numeric `eq` (default 0).
    pub tolerance: Option<f64>,
    /// A mismatch of a required rule blocks "완료"; optional rules only warn.
    pub required: bool,
    pub enabled: bool,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            id: crate::documents::new_id(),
            description: String::new(),
            left: RuleOperand::default(),
            op: RuleOp::Present,
            right: None,
            tolerance: None,
            required: true,
            enabled: true,
        }
    }
}

/// Output of `define_rule`: the rule plus how sure the converter is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleDraft {
    pub rule: Rule,
    pub confidence: f32,
    /// `rules` (deterministic phrase rules) or `model`.
    pub source: String,
    /// Problems the user has to fix in the grid (unknown field, missing right side …).
    pub issues: Vec<String>,
    /// Field / type names that could not be resolved against the known schemas.
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Mismatch,
    /// A value could not be compared (missing / unparsable / review pending, or the document has no
    /// extraction run yet — evidence status `not_extracted`).
    Undetermined,
    /// A document of the referenced type is not in the project.
    Missing,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Mismatch => "mismatch",
            Verdict::Undetermined => "undetermined",
            Verdict::Missing => "missing",
        }
    }
    pub fn parse(s: &str) -> Option<Verdict> {
        [Verdict::Pass, Verdict::Mismatch, Verdict::Undetermined, Verdict::Missing]
            .into_iter()
            .find(|v| v.as_str() == s)
    }
}

/// Where a compared value came from (evidence link for the results table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Evidence {
    pub doc_id: Option<String>,
    pub doc_name: Option<String>,
    pub doc_type: Option<String>,
    pub field: Option<String>,
    pub label: Option<String>,
    pub value: Option<String>,
    pub normalized: Option<String>,
    pub page: Option<u32>,
    /// Field review state at check time (`auto`, `pending`, `approved`, `rejected`, …), or why there is
    /// no field: `absent` (the extraction has no such field), `not_extracted` (the document has no
    /// extraction run yet), `missing_document` (no document of the type), `document` (a document-level
    /// value stands in).
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleResult {
    pub id: String,
    pub run_id: String,
    pub rule_id: String,
    pub description: String,
    pub verdict: Verdict,
    pub required: bool,
    pub left: Option<Evidence>,
    pub right: Option<Evidence>,
    /// Human-readable explanation ("공급자 사업자번호 123-45-67890 ≠ 사업자등록증 123-45-67891").
    pub message: String,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckRun {
    pub id: String,
    pub project_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub pass: u32,
    pub mismatch: u32,
    pub undetermined: u32,
    pub missing: u32,
    pub results: Vec<RuleResult>,
    /// Required document types absent at check time.
    pub missing_doc_types: Vec<String>,
    /// Only some of the enabled rules were run (`rule_ids`). Such a run answers that question alone: it is
    /// returned to the caller but not stored, so it never replaces the project's last full check.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}

/// One row of the document checklist (required types from the template + extra types present).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub doc_type: String,
    pub label_ko: String,
    pub label_en: String,
    pub required: bool,
    /// Project documents of this type.
    pub doc_ids: Vec<String>,
    pub missing: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationNode {
    pub doc_id: String,
    pub label: String,
    pub doc_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationEdge {
    pub from: String,
    pub to: String,
    /// `brn` | `contract_no` | `amount` | `date` | `name` | `account`.
    pub kind: String,
    pub value: String,
    pub from_field: Option<String>,
    pub to_field: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RelationGraph {
    pub nodes: Vec<RelationNode>,
    pub edges: Vec<RelationEdge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDocument {
    pub document: DocumentRecord,
    pub added_at: DateTime<Utc>,
    /// The tracked copy in this project's folder when the document also belongs to an earlier
    /// project (decision 6); `None` = the original file is the project's copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<DocumentCopy>,
}

/// Everything the project screen shows for one project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDetail {
    pub project: Project,
    pub template: Option<ProjectTemplate>,
    pub documents: Vec<ProjectDocument>,
    pub checklist: Vec<ChecklistItem>,
    pub rules: Vec<Rule>,
    pub last_run: Option<CheckRun>,
    pub relations: RelationGraph,
}

/// Host dependencies for rule definition and checking.
pub struct ProjectDeps<'a> {
    pub db: &'a Db,
    /// Model for NL → rule JSON when the phrase rules fail and for fuzzy name comparison
    /// (`None` → phrase rules only; fuzzy comparisons become `undetermined`).
    pub vlm: Option<&'a Vlm<'a>>,
    pub language: Language,
}

impl ProjectDeps<'_> {
    pub fn ko(&self) -> bool {
        self.language == Language::Ko
    }
}

// ---------------------------------------------------------------------------
// Entry points (implemented in the submodules)
// ---------------------------------------------------------------------------

/// Built-in project templates (§3.15 examples: 인증 심사, 거래처 등록, 구매 정산).
pub fn builtin_templates() -> Vec<ProjectTemplate> {
    repo::builtin_templates()
}

/// Converts a natural-language rule into rule JSON (phrase rules → model fallback).
/// `project_id` / `template_id` give the document types the rule may refer to.
pub fn define_rule(
    deps: &ProjectDeps<'_>,
    project_id: Option<&str>,
    template_id: Option<&str>,
    description: &str,
) -> Result<RuleDraft> {
    rules::define(deps, project_id, template_id, description)
}

/// Runs the project's (enabled) rules deterministically; `rule_ids` empty = all.
/// A full run is persisted and updates the project's counters; a run over some of the rules
/// (`CheckRun::partial`) is only returned.
pub fn check(deps: &ProjectDeps<'_>, project_id: &str, rule_ids: &[String]) -> Result<CheckRun> {
    check::run(deps, project_id, rule_ids)
}

/// Shared entities between the project's documents.
pub fn relations(db: &Db, project_id: &str) -> Result<RelationGraph> {
    relations::graph(db, project_id)
}

/// Everything the project screen needs.
pub fn detail(db: &Db, project_id: &str, language: Language) -> Result<ProjectDetail> {
    repo::detail(db, project_id, language)
}

/// Markdown summary report (§3.14.3): documents, checklist, rule results, relations.
pub fn report(db: &Db, project_id: &str, language: Language) -> Result<String> {
    report::markdown(db, project_id, language)
}

/// Builds the 보완요청서 draft request from the findings of a check run; the `generate` module renders it.
/// `finding_ids` empty = every mismatch / missing; otherwise rule results by `RuleResult::id` and missing
/// required documents by `missing_doc:<doc type>` (errors and details: [`report::supplement_request`]).
pub fn supplement_request(
    db: &Db,
    project_id: &str,
    run: &CheckRun,
    finding_ids: &[String],
    output: OutputFormat,
    language: Language,
) -> Result<DraftRequest> {
    report::supplement_request(db, project_id, run, finding_ids, output, language)
}

/// Suggests projects for library / knowledge-base documents by title and content (§3.15 자동 분류);
/// nothing is written until [`apply_classification`].
pub fn classify(deps: &ProjectDeps<'_>, req: &classify::ClassifyRequest) -> Result<classify::ClassifyResult> {
    classify::suggest(deps, req)
}

/// Writes the accepted document → project assignments.
pub fn apply_classification(db: &Db, assignments: &[classify::Assignment]) -> Result<classify::ApplyResult> {
    classify::apply(db, assignments)
}

/// Puts documents in projects and takes them out, many pairs at once (1.0.0 §12; see [`assign`]).
pub fn assign_documents(
    db: &Db,
    paths: &crate::paths::WorkspacePaths,
    req: &assign::AssignRequest,
) -> Result<assign::AssignOutcome> {
    assign::assign(db, paths, req)
}

/// `inbox/<folder>/file` → the project mapped to that inbox subfolder (created on demand
/// when `create` is set). Returns the project id when the document was assigned.
pub fn assign_inbox_document(db: &Db, doc_id: &str, rel_path: &str, create: bool) -> Result<Option<String>> {
    repo::assign_inbox_document(db, doc_id, rel_path, create)
}
