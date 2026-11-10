//! Knowledge base (design §3.14): structure-aware chunking of page text,
//! morpheme (lindera ko-dic) + trigram FTS5 indexes with a LIKE fallback,
//! embeddings stored in sqlite-vec, hybrid search (RRF) and grounded answers
//! with per-sentence citations.
//!
//! Everything here is UI-agnostic: the Tauri shell wires jobs, the engine
//! router (embeddings / chat) and events around these functions.

pub mod answer;
pub mod chunker;
pub mod embed;
pub mod index;
pub mod repo;
pub mod search;
pub mod tokenize;

#[cfg(test)]
mod tests;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::documents::DocumentRole;

/// Bumped whenever the chunker or tokenizer output changes; documents indexed
/// with an older version are re-chunked by the next index run.
pub const INDEX_VERSION: u32 = 1;

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Collections / versions / glossary
// ---------------------------------------------------------------------------

/// What a collection is (0.5.0 plan A): a group of the workspace-wide 공통 참고 자료
/// (`Reference`, no project) or the single reference group of one project (`Project`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CollectionKind {
    #[default]
    Reference,
    Project,
}

impl CollectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CollectionKind::Reference => "reference",
            CollectionKind::Project => "project",
        }
    }
    pub fn parse(s: &str) -> CollectionKind {
        if s == "project" {
            CollectionKind::Project
        } else {
            CollectionKind::Reference
        }
    }
}

/// A reference-material group ("참고 자료 묶음"; the pre-0.5.0 "collection"). Groups without a
/// project make up the 공통 참고 자료; a project's own group carries its `project_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub active: bool,
    #[serde(default)]
    pub kind: CollectionKind,
    /// The project this group belongs to (`kind = Project`); `None` = 공통 참고 자료.
    #[serde(default)]
    pub project_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub doc_count: u64,
    pub chunk_count: u64,
}

impl Collection {
    /// A group of the 공통 참고 자료 (not tied to a project).
    pub fn is_common(&self) -> bool {
        self.project_id.is_none()
    }
}

/// Create / update payload for a collection (`id` = None creates). `project_id` only matters on
/// creation (a group never moves between a project and the 공통 참고 자료); prefer
/// `repo::project_reference_group` for a project's group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CollectionInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub document_id: String,
    pub title: String,
    pub group_key: String,
    pub version_label: Option<String>,
    pub effective_date: Option<String>,
    pub is_latest: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionGroup {
    pub group_key: String,
    pub documents: Vec<VersionInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VersionInput {
    pub document_id: String,
    pub group_key: String,
    #[serde(default)]
    pub version_label: Option<String>,
    #[serde(default)]
    pub effective_date: Option<String>,
    #[serde(default = "default_true")]
    pub is_latest: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlossaryEntry {
    pub id: i64,
    pub term: String,
    pub definition: Option<String>,
    pub synonyms: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GlossaryInput {
    #[serde(default)]
    pub id: Option<i64>,
    pub term: String,
    #[serde(default)]
    pub definition: Option<String>,
    #[serde(default)]
    pub synonyms: Vec<String>,
}

// ---------------------------------------------------------------------------
// Chunks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChunkKind {
    #[default]
    Text,
    Table,
    Heading,
    List,
}

impl ChunkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChunkKind::Text => "text",
            ChunkKind::Table => "table",
            ChunkKind::Heading => "heading",
            ChunkKind::List => "list",
        }
    }
    pub fn parse(s: &str) -> ChunkKind {
        match s {
            "table" => ChunkKind::Table,
            "heading" => ChunkKind::Heading,
            "list" => ChunkKind::List,
            _ => ChunkKind::Text,
        }
    }
}

/// A chunk produced by the chunker (not yet stored).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChunkDraft {
    pub page_start: u32,
    pub page_end: u32,
    pub kind: ChunkKind,
    pub heading_path: Option<String>,
    pub clause: Option<String>,
    pub text: String,
    pub token_estimate: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub id: i64,
    pub document_id: String,
    pub chunk_index: u32,
    pub page_start: u32,
    pub page_end: u32,
    pub kind: ChunkKind,
    pub heading_path: Option<String>,
    pub clause: Option<String>,
    pub text: String,
    pub token_estimate: u32,
    pub embedded: bool,
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Hybrid,
    Keyword,
    Semantic,
}

/// Which documents a search / answer may draw from. Empty vectors mean "no
/// restriction"; `latest_only` hides superseded editions (`doc_versions`).
///
/// `project_id` (0.5.0 plan A) allows the project's work documents ∪ its reference group ∪ the
/// active groups of the 공통 참고 자료; with `documents` set as well, those documents stand in for
/// the work documents (a selection asked about inside the project still sees its reference
/// material). `collections`, `roles` and `folder` intersect as usual.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchScope {
    #[serde(default)]
    pub collections: Vec<String>,
    #[serde(default)]
    pub documents: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default)]
    pub roles: Vec<DocumentRole>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default = "default_true")]
    pub latest_only: bool,
    /// Skip documents in inactive collections (only meaningful without explicit `collections`).
    #[serde(default = "default_true")]
    pub active_collections_only: bool,
}

impl Default for SearchScope {
    fn default() -> Self {
        SearchScope {
            collections: Vec::new(),
            documents: Vec::new(),
            project_id: None,
            roles: Vec::new(),
            folder: None,
            latest_only: true,
            active_collections_only: true,
        }
    }
}

impl SearchScope {
    pub fn is_unrestricted(&self) -> bool {
        self.collections.is_empty()
            && self.documents.is_empty()
            && self.project_id.is_none()
            && self.roles.is_empty()
            && self.folder.is_none()
    }
    pub fn documents(ids: &[String]) -> SearchScope {
        SearchScope { documents: ids.to_vec(), ..SearchScope::default() }
    }
    /// A project's work documents + its reference group + the 공통 참고 자료.
    pub fn project(id: &str) -> SearchScope {
        SearchScope { project_id: Some(id.to_string()), ..SearchScope::default() }
    }
    pub fn knowledge() -> SearchScope {
        SearchScope { roles: vec![DocumentRole::Knowledge], ..SearchScope::default() }
    }
}

fn default_limit() -> u32 {
    20
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub mode: SearchMode,
    #[serde(default)]
    pub scope: SearchScope,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default = "default_true")]
    pub expand_glossary: bool,
    /// Max hits per document (diversity); 0 = unlimited.
    #[serde(default = "default_per_document")]
    pub per_document: u32,
}

fn default_per_document() -> u32 {
    3
}

impl SearchRequest {
    pub fn new(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.to_string(),
            mode: SearchMode::Hybrid,
            scope: SearchScope::default(),
            limit: 20,
            expand_glossary: true,
            per_document: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchChannel {
    Morph,
    Trigram,
    Like,
    Vector,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub chunk_id: i64,
    pub document_id: String,
    pub title: String,
    pub rel_path: String,
    pub role: DocumentRole,
    pub doc_type: Option<String>,
    pub page_start: u32,
    pub page_end: u32,
    pub kind: ChunkKind,
    pub heading_path: Option<String>,
    pub clause: Option<String>,
    pub text: String,
    /// Window of `text` around the first highlight.
    pub snippet: String,
    /// `[start, end)` char ranges inside `snippet`.
    pub highlights: Vec<(usize, usize)>,
    pub score: f32,
    pub channels: Vec<SearchChannel>,
    /// Cosine similarity from the vector channel (None when not a vector hit).
    #[serde(default)]
    pub vector_similarity: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub query_terms: Vec<String>,
    pub expanded_terms: Vec<String>,
    pub used_vector: bool,
    pub used_like_fallback: bool,
    pub elapsed_ms: u64,
    pub total_candidates: u32,
}

// ---------------------------------------------------------------------------
// Index status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TokenizerInfo {
    /// `ko-dic` (lindera + mecab-ko-dic) or `simple` (built-in fallback).
    pub kind: String,
    pub path: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct IndexStatus {
    pub documents_total: u64,
    pub documents_indexed: u64,
    /// Have text but are not (or no longer) indexed.
    pub documents_pending: u64,
    pub documents_no_text: u64,
    pub documents_failed: u64,
    pub knowledge_documents: u64,
    pub chunks_total: u64,
    pub chunks_embedded: u64,
    pub embedding_model: Option<String>,
    pub embedding_dim: Option<u32>,
    pub tokenizer: TokenizerInfo,
    pub collections: u64,
    pub glossary_terms: u64,
    pub answers: u64,
    pub answers_helpful: u64,
    pub answers_wrong: u64,
}

// ---------------------------------------------------------------------------
// Answers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnswerKind {
    #[default]
    Answer,
    Explain,
    Summary,
    Compare,
}

impl AnswerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AnswerKind::Answer => "answer",
            AnswerKind::Explain => "explain",
            AnswerKind::Summary => "summary",
            AnswerKind::Compare => "compare",
        }
    }
    pub fn parse(s: &str) -> AnswerKind {
        match s {
            "explain" => AnswerKind::Explain,
            "summary" => AnswerKind::Summary,
            "compare" => AnswerKind::Compare,
            _ => AnswerKind::Answer,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SummaryMode {
    OneLine,
    #[default]
    KeyPoints,
    OnePage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnswerLanguage {
    #[default]
    Auto,
    Ko,
    En,
}

fn default_top_k() -> u32 {
    8
}
fn default_context_tokens() -> u32 {
    6_000
}

/// Who said an earlier turn of the assistant conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnRole {
    User,
    Assistant,
}

/// One earlier turn of the assistant conversation. Sent with follow-up questions and commands so
/// the model can resolve references ("그 문서", "아까 금액") — context only, never evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub role: TurnRole,
    pub text: String,
}

impl ConversationTurn {
    pub fn user(text: &str) -> ConversationTurn {
        ConversationTurn { role: TurnRole::User, text: text.to_string() }
    }

    pub fn assistant(text: &str) -> ConversationTurn {
        ConversationTurn { role: TurnRole::Assistant, text: text.to_string() }
    }
}

/// What the user is looking at while asking (titles resolved by the shell): the open document,
/// the library selection / folder, the open project and the latest imports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SessionState {
    pub current_document: Option<String>,
    pub current_page: Option<u32>,
    pub selected_documents: Vec<String>,
    pub folder: Option<String>,
    pub project: Option<String>,
    pub recent_documents: Vec<String>,
}

impl SessionState {
    pub fn is_empty(&self) -> bool {
        self.current_document.is_none()
            && self.selected_documents.is_empty()
            && self.folder.is_none()
            && self.project.is_none()
            && self.recent_documents.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnswerRequest {
    pub question: String,
    #[serde(default)]
    pub kind: AnswerKind,
    #[serde(default)]
    pub scope: SearchScope,
    /// Fall back to the model's general knowledge when the knowledge base has no evidence.
    #[serde(default)]
    pub allow_general: bool,
    #[serde(default = "default_top_k")]
    pub top_k: u32,
    #[serde(default = "default_context_tokens")]
    pub context_tokens: u32,
    #[serde(default)]
    pub language: AnswerLanguage,
    /// Explain: the selected text; Summary/Compare: unused.
    #[serde(default)]
    pub selection: Option<String>,
    /// Document the question was asked from (viewer); also added to the scope for `explain`.
    #[serde(default)]
    pub document_id: Option<String>,
    /// Compare: the second document.
    #[serde(default)]
    pub compare_document_id: Option<String>,
    #[serde(default)]
    pub summary_mode: SummaryMode,
    /// Chunk ids from a previous answer to keep as evidence (follow-up questions).
    #[serde(default)]
    pub keep_chunks: Vec<i64>,
    /// Persist the answer in `answers` (default true).
    #[serde(default = "default_true")]
    pub save: bool,
    /// Look evidence up in the knowledge base (default). `false` for a request that brings its own text
    /// (다듬기 · 교정 · 번역 · 정리): the model works on that text alone and cites nothing.
    #[serde(default = "default_true")]
    pub retrieve: bool,
    /// Cosine-similarity floor for vector-only evidence (None = built-in default).
    #[serde(default)]
    pub min_similarity: Option<f32>,
    /// Earlier turns of the conversation, oldest first (the question itself is not included).
    #[serde(default)]
    pub history: Vec<ConversationTurn>,
    /// What the UI is showing (resolved by the shell from the ids the UI sends).
    #[serde(default)]
    pub session: Option<SessionState>,
}

impl AnswerRequest {
    pub fn question(q: &str) -> AnswerRequest {
        AnswerRequest {
            question: q.to_string(),
            kind: AnswerKind::Answer,
            scope: SearchScope::default(),
            allow_general: false,
            top_k: 8,
            context_tokens: 6_000,
            language: AnswerLanguage::Auto,
            selection: None,
            document_id: None,
            compare_document_id: None,
            summary_mode: SummaryMode::KeyPoints,
            keep_chunks: Vec::new(),
            save: true,
            retrieve: true,
            min_similarity: None,
            history: Vec::new(),
            session: None,
        }
    }
}

/// One numbered source shown to the model (`[n]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    pub index: u32,
    pub chunk_id: i64,
    pub document_id: String,
    pub title: String,
    pub page_start: u32,
    pub page_end: u32,
    pub heading_path: Option<String>,
    pub clause: Option<String>,
    pub score: f32,
    /// Short excerpt for the UI.
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnswerSentence {
    pub text: String,
    /// Source indexes (1-based) cited by this sentence.
    pub citations: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Answer {
    pub id: Option<i64>,
    pub kind: AnswerKind,
    pub question: String,
    /// Full answer text with `[n]` markers.
    pub text: String,
    pub sentences: Vec<AnswerSentence>,
    pub sources: Vec<Citation>,
    pub no_evidence: bool,
    pub used_general_knowledge: bool,
    pub conflicts: Option<String>,
    pub model: Option<String>,
    pub elapsed_ms: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub document_id: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub feedback: Option<i8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedAnswer {
    pub id: i64,
    pub kind: AnswerKind,
    pub question: String,
    pub text: String,
    pub sentences: Vec<AnswerSentence>,
    pub sources: Vec<Citation>,
    pub no_evidence: bool,
    pub used_general_knowledge: bool,
    pub model: Option<String>,
    pub document_id: Option<String>,
    pub feedback: Option<i8>,
    pub feedback_note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EmbeddingInfo {
    pub model: Option<String>,
    pub dim: Option<u32>,
    pub chunks_total: u64,
    pub chunks_embedded: u64,
}
