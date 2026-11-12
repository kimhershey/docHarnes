//! Natural-language document generation (design §3.13, §3.14.4): a natural-
//! language request plus optional sources (extracted fields of documents, an
//! aggregate result, knowledge-base evidence, free text) becomes a structured
//! **draft** (fields, item tables, body blocks with citations) produced by the
//! VLM under a grammar-safe JSON schema, validated in reverse with the
//! pipeline validators (sums, BRN checksums, dates), rendered to HTML for the
//! preview and saved as HWPX / DOCX / PDF / Markdown. Registered HWPX / DOCX
//! forms are filled in place (label → adjacent cell / underscore run, repeated
//! item rows); PDF / image forms and free-form descriptions use the built-in
//! generic templates. Generated files are registered in the library with
//! `DocumentRole::Generated` and the draft is kept under `.doculens/drafts/`.
//!
//! Contract (types and function signatures) is fixed here; the submodules hold
//! the implementation.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::Db;
use crate::documents::convert::PdfPrinter;
use crate::documents::import::ImportDeps;
use crate::error::Result;
use crate::knowledge::search::SearchDeps;
use crate::paths::WorkspacePaths;
use crate::pipeline::vlm::Vlm;
use crate::settings::Language;

pub mod draft;
pub mod fill;
pub mod office;
pub mod prompts;
pub mod render;
pub mod schema;
pub mod store;
pub mod templates;
#[cfg(test)]
mod tests;
pub mod validate;

/// Output file format of a saved draft (appendix D `draft_document.output`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    #[default]
    Hwpx,
    Docx,
    Pdf,
    Md,
}

impl OutputFormat {
    pub const ALL: [OutputFormat; 4] = [OutputFormat::Hwpx, OutputFormat::Docx, OutputFormat::Pdf, OutputFormat::Md];

    pub fn as_str(self) -> &'static str {
        match self {
            OutputFormat::Hwpx => "hwpx",
            OutputFormat::Docx => "docx",
            OutputFormat::Pdf => "pdf",
            OutputFormat::Md => "md",
        }
    }

    pub fn parse(s: &str) -> Option<OutputFormat> {
        OutputFormat::ALL.into_iter().find(|f| f.as_str() == s.trim().to_ascii_lowercase())
    }

    pub fn extension(self) -> &'static str {
        self.as_str()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// 격식체 (공문·계약서 기본).
    #[default]
    Formal,
    /// 평이한 설명체.
    Plain,
    /// 개조식·간결.
    Concise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Length {
    Short,
    #[default]
    Medium,
    Long,
}

/// Built-in generic templates (§3.13: 공문·견적서·거래명세서·계약서·보고서·회의록)
/// plus the 보완요청서 used by §3.15 `request_supplement`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateKind {
    // The software-development set (0.3.0): the slugs equal the pipeline's document types.
    Rfp,
    Proposal,
    Sow,
    Srs,
    DevContract,
    Wbs,
    ProgressReport,
    MeetingMinutes,
    TestReport,
    ChangeRequest,
    Acceptance,
    Estimate,
    SupplementRequest,
    // The generic set of 0.1, hidden since 0.3.0 (user decision 2026-11-10): not listed in the
    // picker and not matched from free text, but an explicit reference and saved drafts keep working.
    OfficialLetter,
    Quotation,
    TransactionStatement,
    Contract,
    Report,
    Minutes,
}

impl TemplateKind {
    pub const ALL: [TemplateKind; 19] = [
        TemplateKind::Rfp,
        TemplateKind::Proposal,
        TemplateKind::Sow,
        TemplateKind::Srs,
        TemplateKind::DevContract,
        TemplateKind::Wbs,
        TemplateKind::ProgressReport,
        TemplateKind::MeetingMinutes,
        TemplateKind::TestReport,
        TemplateKind::ChangeRequest,
        TemplateKind::Acceptance,
        TemplateKind::Estimate,
        TemplateKind::SupplementRequest,
        TemplateKind::OfficialLetter,
        TemplateKind::Quotation,
        TemplateKind::TransactionStatement,
        TemplateKind::Contract,
        TemplateKind::Report,
        TemplateKind::Minutes,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TemplateKind::Rfp => "rfp",
            TemplateKind::Proposal => "proposal",
            TemplateKind::Sow => "sow",
            TemplateKind::Srs => "srs",
            TemplateKind::DevContract => "dev_contract",
            TemplateKind::Wbs => "wbs",
            TemplateKind::ProgressReport => "progress_report",
            TemplateKind::MeetingMinutes => "meeting_minutes",
            TemplateKind::TestReport => "test_report",
            TemplateKind::ChangeRequest => "change_request",
            TemplateKind::Acceptance => "acceptance",
            TemplateKind::Estimate => "estimate",
            TemplateKind::SupplementRequest => "supplement_request",
            TemplateKind::OfficialLetter => "official_letter",
            TemplateKind::Quotation => "quotation",
            TemplateKind::TransactionStatement => "transaction_statement",
            TemplateKind::Contract => "contract",
            TemplateKind::Report => "report",
            TemplateKind::Minutes => "minutes",
        }
    }

    pub fn parse(s: &str) -> Option<TemplateKind> {
        TemplateKind::ALL.into_iter().find(|k| k.as_str() == s.trim())
    }

    /// The generic templates of 0.1: kept for explicit references and saved drafts, out of the picker.
    pub fn hidden(self) -> bool {
        matches!(
            self,
            TemplateKind::OfficialLetter
                | TemplateKind::Quotation
                | TemplateKind::TransactionStatement
                | TemplateKind::Contract
                | TemplateKind::Report
                | TemplateKind::Minutes
        )
    }

    /// The templates the UI offers and free text is matched against, in UI order.
    pub fn visible() -> impl Iterator<Item = TemplateKind> {
        TemplateKind::ALL.into_iter().filter(|k| !k.hidden())
    }
}

/// Where the draft's layout comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TemplateRef {
    /// One of the built-in generic templates.
    Builtin { template: TemplateKind },
    /// A registered form (pipeline schema with `SchemaSource::Form`); `doc_id` is the
    /// registered HWPX / DOCX / PDF document the form was derived from, when known.
    Form {
        schema_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        doc_id: Option<String>,
    },
}

/// Material the draft may quote (§3.13 "내용 = … 기존 문서의 추출 결과/검색 결과 인용").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceRef {
    /// Latest extracted fields (and page text when short) of a library document.
    Document { doc_id: String },
    /// An aggregate card result (`aggregate::AggregateResult` as JSON).
    Aggregate { result: Value },
    /// Knowledge-base search for this query (regulations, past documents) — §3.14.4.
    Knowledge { query: String },
    /// Free text the user pasted.
    Text {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        text: String,
    },
}

/// A draft request (planner `draft_document`, the 문서 생성 screen, §3.15 보완요청서).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DraftRequest {
    /// Registered form / schema reference (id, name, doc-type slug) or a built-in
    /// template name (`TemplateKind::as_str`). `None` → derived from
    /// `format_description` / `content` keywords, falling back to `report`.
    pub form_ref: Option<String>,
    /// Natural-language description of the layout ("A4 공문 형식, 수신·참조·제목·본문·붙임").
    pub format_description: Option<String>,
    /// What the document should say.
    pub content: String,
    pub sources: Vec<SourceRef>,
    pub output: OutputFormat,
    pub tone: Tone,
    pub length: Length,
    pub language: Language,
    /// 1.0.0: also look for passages in the reference material (the project's reference group and
    /// the 공통 참고 자료) — never in work documents the user did not cite. Off by default; passages
    /// inside the cited documents are looked up whatever this says.
    pub use_knowledge: bool,
    /// Regeneration instructions ("더 짧게", "금액을 1,200,000원으로") — applied on top of `previous`.
    pub feedback: Option<String>,
    /// The draft being revised; user edits in it are preserved unless `feedback` asks otherwise.
    pub previous: Option<Box<Draft>>,
    /// Knowledge documents the user excluded in the "참고 문서" panel (§3.14.4).
    pub excluded_sources: Vec<String>,
    /// Reply token budget (0.3.0 item 4). `None` → the `length` preset. Budgets above
    /// `prompts::SPLIT_THRESHOLD_TOKENS` (or 35 % of the context) are produced section by section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Knowledge searches (`use_knowledge`, `SourceRef::Knowledge`) stay inside this project's
    /// work documents + reference material + the 공통 참고 자료 (0.5.0 plan A); `None` = everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// 0.7.0 (plan §1.5): revise `previous` instead of regenerating it — one section rewritten
    /// or a new section inserted per `feedback` (the instruction); `Whole` is the full
    /// regeneration with the instruction passed to every section call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revise: Option<ReviseScope>,
}

/// What a revision touches (`DraftRequest.revise`). Sections are the runs of `Draft::body`
/// split at headings of level ≤ 2 (`sections_of`); ordinals are 1-based.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum ReviseScope {
    /// Rewrite section `section` (fields, tables and the other sections stay).
    Section { section: usize },
    /// Insert a new section after `section` (`None` = at the end), titled `heading` when given.
    InsertAfter {
        #[serde(default)]
        section: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        heading: Option<String>,
    },
    /// Regenerate the whole draft with the instruction (`previous` + `feedback`).
    Whole,
}

/// A section of a draft body: blocks `start..end` (the heading block included when `titled`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub start: usize,
    pub end: usize,
    /// The heading text (`None` for a leading run without a level ≤ 2 heading).
    pub heading: Option<String>,
}

/// Splits a body into sections at headings of level ≤ 2. A leading run without such a heading
/// is section 1 (untitled); a body without headings is one section; an empty body has none.
pub fn sections_of(body: &[DraftBlock]) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for (i, b) in body.iter().enumerate() {
        match b {
            DraftBlock::Heading { level, text } if *level <= 2 => {
                if let Some(last) = out.last_mut() {
                    last.end = i;
                }
                out.push(Section { start: i, end: body.len(), heading: Some(text.clone()) });
            }
            _ if out.is_empty() => out.push(Section { start: i, end: body.len(), heading: None }),
            _ => {}
        }
    }
    out
}

/// Characters of free text a request keeps per item when it is stored with its job (the content,
/// the instruction, each pasted text source).
pub const MAX_STORED_TEXT_CHARS: usize = 20_000;

impl DraftRequest {
    /// Makes the project of a draft job and the project of its request one thing. A job started for
    /// a project (`job_project`: a 보완요청서 requested from the project page) gives its project to a
    /// request that names none, so the request's knowledge searches stay inside it; a request that
    /// names a project (the planner's project scope, the 문서 작성 screen) gives it to a job that has
    /// none, so the result links to it. Returns the job's project.
    pub fn bind_project(&mut self, job_project: Option<String>) -> Option<String> {
        let named = |p: Option<String>| p.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
        self.project_id = named(self.project_id.take());
        let job_project = named(job_project).or_else(|| self.project_id.clone());
        if self.project_id.is_none() {
            self.project_id = job_project.clone();
        }
        job_project
    }

    /// Bounds the request's free text for storage: the content (trimmed), the instruction and every
    /// pasted text source keep [`MAX_STORED_TEXT_CHARS`] characters. This is not the prompt limit:
    /// what the model is given of the content and the instruction is decided — and reported when it
    /// is less than the whole — by the prompt (`prompts::MAX_CONTENT_CHARS`, `MAX_FEEDBACK_CHARS`).
    pub fn bound_stored_text(&mut self) {
        let bound = |text: &mut String| {
            if text.chars().count() > MAX_STORED_TEXT_CHARS {
                *text = text.chars().take(MAX_STORED_TEXT_CHARS).collect();
            }
        };
        self.content = self.content.trim().to_string();
        bound(&mut self.content);
        if let Some(feedback) = self.feedback.as_mut() {
            bound(feedback);
        }
        for source in &mut self.sources {
            if let SourceRef::Text { text, .. } = source {
                bound(text);
            }
        }
    }
}

impl Default for DraftRequest {
    fn default() -> Self {
        Self {
            form_ref: None,
            format_description: None,
            content: String::new(),
            sources: Vec::new(),
            output: OutputFormat::default(),
            tone: Tone::default(),
            length: Length::default(),
            language: Language::Ko,
            // 1.0.0: off by default — a draft is written from what the user cites.
            use_knowledge: false,
            feedback: None,
            previous: None,
            excluded_sources: Vec::new(),
            max_tokens: None,
            project_id: None,
            revise: None,
        }
    }
}

/// Provenance of a value or sentence (a document page / knowledge chunk / aggregate).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DraftCitation {
    pub doc_id: Option<String>,
    pub title: String,
    pub page: Option<u32>,
    pub chunk_id: Option<i64>,
    /// `document` | `knowledge` | `aggregate` | `text`.
    pub source: String,
    pub excerpt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DraftField {
    pub key: String,
    pub label: String,
    /// `FieldKind::as_str` (`string`, `amount`, `date`, `brn`, …).
    pub kind: String,
    pub required: bool,
    pub value: String,
    /// Normalised value (ISO date, digits-only amount …) when the kind has one.
    pub normalized: Option<String>,
    pub source: Option<DraftCitation>,
    /// Set when the user edited the value in the 문서 생성 screen (kept across regenerations).
    pub user_edited: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DraftColumn {
    pub key: String,
    pub label: String,
    pub kind: String,
}

/// An item table (견적 품목, 거래 명세 행 …).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DraftTable {
    pub key: String,
    pub label: String,
    pub columns: Vec<DraftColumn>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DraftBlock {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<DraftCitation>,
    },
    List {
        items: Vec<String>,
        #[serde(default)]
        ordered: bool,
    },
    /// References a `Draft::tables` entry by key (rendered where it appears in the body).
    TableRef {
        key: String,
    },
}

/// A reverse-validation finding (§3.13 "검증": sums, checksums, dates, required fields).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftIssue {
    /// `fields.total`, `tables.items[2].amount`, `body[3]` …
    pub path: String,
    /// Stable code: `required_missing`, `sum_mismatch`, `brn_invalid`, `date_invalid`,
    /// `amount_invalid`, `uncited_claim`, `template_unknown_field` …
    pub code: String,
    pub message: String,
    /// Blocking issues keep the draft in "검토 전" with a warning on save.
    pub blocking: bool,
    /// Proposed correction (e.g. the computed sum), applied by the UI on request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// The structured draft (model output after validation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Draft {
    pub id: String,
    pub title: String,
    pub template: TemplateRef,
    /// Document type slug the generated file is registered with (form slug or template name).
    pub doc_type: Option<String>,
    pub fields: Vec<DraftField>,
    pub tables: Vec<DraftTable>,
    pub body: Vec<DraftBlock>,
    /// Knowledge / document sources used (the "참고 문서" panel).
    pub references: Vec<DraftCitation>,
    pub issues: Vec<DraftIssue>,
    pub language: Language,
    pub tone: Tone,
    pub length: Length,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: Option<String>,
    /// The request that produced it (regeneration keeps it).
    pub request: Option<Box<DraftRequest>>,
    /// Reader / generator notes shown under the preview.
    pub notes: Vec<String>,
    /// Set once the draft was saved (workspace-relative path and registered document).
    pub saved_path: Option<String>,
    pub saved_doc_id: Option<String>,
}

impl Default for Draft {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            id: crate::documents::new_id(),
            title: String::new(),
            template: TemplateRef::Builtin { template: TemplateKind::Report },
            doc_type: None,
            fields: Vec::new(),
            tables: Vec::new(),
            body: Vec::new(),
            references: Vec::new(),
            issues: Vec::new(),
            language: Language::Ko,
            tone: Tone::default(),
            length: Length::default(),
            created_at: now,
            updated_at: now,
            model: None,
            request: None,
            notes: Vec::new(),
            saved_path: None,
            saved_doc_id: None,
        }
    }
}

impl Draft {
    pub fn has_blocking_issues(&self) -> bool {
        self.issues.iter().any(|i| i.blocking)
    }

    pub fn field(&self, key: &str) -> Option<&DraftField> {
        self.fields.iter().find(|f| f.key == key)
    }

    pub fn table(&self, key: &str) -> Option<&DraftTable> {
        self.tables.iter().find(|t| t.key == key)
    }
}

/// Result of one generation call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftOutcome {
    pub draft: Draft,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub elapsed_ms: u64,
    /// `rules` when no model call was needed (e.g. a filled 보완요청서 from findings), else `model`.
    pub source: String,
    /// Model calls that produced body sections (1 = one-shot draft; >1 = outline → sections flow).
    #[serde(default)]
    pub sections: u32,
    /// The user's own text the model saw only in part: the content is limited to
    /// `prompts::MAX_CONTENT_CHARS`, the instruction to `prompts::MAX_FEEDBACK_CHARS`, and a small
    /// context can take less. The draft's notes carry the same in words. Empty = nothing was cut.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_cuts: Vec<InputCut>,
}

/// Part of the request's text that did not reach the model (`DraftOutcome.input_cuts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputCut {
    /// `content` (`DraftRequest.content`) | `feedback` (`DraftRequest.feedback`, the instruction).
    pub field: String,
    /// Characters the request carried.
    pub chars: u32,
    /// Characters passed to the model (the first ones).
    pub passed: u32,
}

/// Save request (문서 생성 screen "저장", planner `draft_document` after confirmation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveRequest {
    pub draft: Draft,
    pub output: OutputFormat,
    /// File name without extension; default `<YYYY-MM-DD>_<title>`.
    pub file_name: Option<String>,
    /// Workspace-relative folder; default `documents/generated`.
    pub folder: Option<String>,
    /// Register the file in the library (`DocumentRole::Generated`, type = `draft.doc_type`).
    pub register: bool,
    pub overwrite: bool,
}

impl Default for SaveRequest {
    fn default() -> Self {
        Self {
            draft: Draft::default(),
            output: OutputFormat::default(),
            file_name: None,
            folder: None,
            register: true,
            overwrite: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedDocument {
    /// Workspace-relative path with `/` separators.
    pub rel_path: String,
    pub absolute: PathBuf,
    pub format: OutputFormat,
    /// Library document id when registered.
    pub doc_id: Option<String>,
    /// `true` when the registered form file was filled in place (HWPX / DOCX template fill),
    /// `false` when a built-in template was rendered.
    pub filled_form: bool,
    /// Printing / filling problems that did not stop the save (e.g. PDF printer unavailable → MD kept).
    pub warnings: Vec<String>,
    /// Body blocks with text (headings, paragraphs, lists) that a filled form left out: a registered
    /// HWPX / DOCX form has slots for fields and item rows only. `warnings` carries the sentence for
    /// the user; 0 when nothing was left out or no form was filled.
    #[serde(default)]
    pub omitted_body_blocks: u32,
}

/// Field definition of a built-in template (for the UI picker and the prompt).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateField {
    pub key: String,
    pub label_ko: String,
    pub label_en: String,
    pub kind: String,
    pub required: bool,
    /// Item-table columns when `kind == "table"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<DraftColumn>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateInfo {
    pub kind: TemplateKind,
    pub name_ko: String,
    pub name_en: String,
    pub description_ko: String,
    pub description_en: String,
    pub fields: Vec<TemplateField>,
    /// Body section headings in order (prompt outline + rendering).
    pub sections_ko: Vec<String>,
    pub sections_en: Vec<String>,
    /// Keywords that select this template from a free-form request.
    pub keywords: Vec<String>,
}

/// Saved-draft listing entry (`.doculens/drafts/<id>.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftSummary {
    pub id: String,
    pub title: String,
    pub template: TemplateRef,
    pub updated_at: DateTime<Utc>,
    pub issues: u32,
    pub saved_path: Option<String>,
}

/// Everything generation needs from the host (shell / executor / tests).
pub struct GenerateDeps<'a> {
    pub db: &'a Db,
    pub paths: &'a WorkspacePaths,
    /// `None` → only rule-based drafts (e.g. 보완요청서 from findings) are possible.
    pub vlm: Option<&'a Vlm<'a>>,
    /// Knowledge-base retrieval for grounding (`None` → no KB sources).
    pub knowledge: Option<SearchDeps<'a>>,
    /// PDF output (`None` → PDF requests fall back to Markdown with a warning).
    pub printer: Option<&'a dyn PdfPrinter>,
    /// Registering the saved file in the library (`None` → file only).
    pub import: Option<&'a ImportDeps<'a>>,
    pub cancel: Option<&'a AtomicBool>,
    pub language: Language,
    /// Section-level progress of a split draft / of a revision: `(fraction 0..1, detail)` — the job
    /// bridge forwards it to the job book ("section 3/8"). It is called between model calls only,
    /// never while one is in flight, and may block: the bridge uses it as the engine checkpoint
    /// where waiting interactive requests run and a paused job parks.
    pub progress: Option<&'a ProgressFn<'a>>,
}

/// Progress callback of a split draft (`fraction 0..1`, stage detail such as `section 3/8`).
pub type ProgressFn<'a> = dyn Fn(f32, &str) + Sync + 'a;

impl GenerateDeps<'_> {
    pub fn ko(&self) -> bool {
        self.language == Language::Ko
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// Entry points (implemented in the submodules)
// ---------------------------------------------------------------------------

/// The built-in templates with their fields and sections.
pub fn templates() -> Vec<TemplateInfo> {
    templates::list()
}

/// Every built-in template, the hidden generic ones included (explicit references, old drafts).
pub fn templates_all() -> Vec<TemplateInfo> {
    templates::list_all()
}

/// Resolves `form_ref` / `format_description` / `content` to a template reference
/// (registered form schema first, then built-in template keywords, then `report`).
pub fn resolve_template(db: &Db, req: &DraftRequest) -> Result<TemplateRef> {
    draft::resolve_template(db, req)
}

/// Produces a validated draft (model call unless the request can be filled by rules).
pub fn draft(deps: &GenerateDeps<'_>, req: &DraftRequest) -> Result<DraftOutcome> {
    draft::draft(deps, req)
}

/// Re-runs the reverse validation on an edited draft (sums, checksums, required fields);
/// fills `normalized` values and `issues`, applies computed sums to empty total fields.
pub fn validate(draft: &mut Draft) {
    validate::validate(draft)
}

/// HTML preview of a draft (same renderer the HWPX / DOCX / PDF writers use).
pub fn preview_html(draft: &Draft) -> String {
    render::to_html(draft)
}

pub fn to_markdown(draft: &Draft) -> String {
    render::to_markdown(draft)
}

/// Writes the draft in the requested format (filling the registered form when the
/// template is an HWPX / DOCX form), registers it in the library and records the
/// saved path on the stored draft.
pub fn save(deps: &GenerateDeps<'_>, req: &SaveRequest) -> Result<SavedDocument> {
    render::save(deps, req)
}
