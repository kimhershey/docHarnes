//! Form registration (§3.4.0) and natural-language schemas (§3.5): derive the
//! field list of a user's form from its structure (HWPX/DOCX tables and
//! "label: ____" lines, no model needed) or with the VLM for PDF / image
//! forms, and create / modify schemas from a description.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::Db;
use crate::documents::convert::{
    self,
    model::{Block, DocModel},
};
use crate::documents::import::ImportDeps;
use crate::documents::{repo as docs, DocumentCache, DocumentRecord, DocumentRole, SourceKind};
use crate::engine::client::{user_message, user_message_with_image};
use crate::error::{CoreError, Result};

use super::prompts::{self, Stage};
use super::repo;
use super::schema::{self, FieldDef, FieldKind, SchemaDef, SchemaDraft, SchemaRecord, SchemaSource};
use super::vlm::Vlm;
use super::{image, ExtractedTable};

const MAX_LABEL_CHARS: usize = 24;

/// Korean form names → type slugs (the fallback is a hash-based slug).
pub fn slug_for_name(name: &str) -> String {
    // Drop bracketed qualifiers ("거래명세서(공급자용)") and spaces before matching.
    let mut n = String::new();
    let mut depth = 0;
    for c in name.trim().chars() {
        match c {
            '(' | '（' | '[' | '［' => depth += 1,
            ')' | '）' | ']' | '］' => depth = (depth - 1).max(0),
            ' ' | '\u{a0}' => {}
            _ if depth == 0 => n.push(c),
            _ => {}
        }
    }
    const TABLE: &[(&str, &str)] = &[
        ("제안요청서", "rfp"),
        ("제안서", "proposal"),
        ("과업지시서", "sow"),
        ("과업내용서", "sow"),
        ("요구사항정의서", "srs"),
        ("요구사항명세서", "srs"),
        ("요구사항정의", "srs"),
        ("개발계약서", "dev_contract"),
        ("용역계약서", "dev_contract"),
        ("WBS", "wbs"),
        ("일정표", "wbs"),
        ("진척보고서", "progress_report"),
        ("주간보고서", "progress_report"),
        ("월간보고서", "progress_report"),
        ("진도보고서", "progress_report"),
        ("테스트결과서", "test_report"),
        ("시험결과서", "test_report"),
        ("테스트보고서", "test_report"),
        ("변경요청서", "change_request"),
        ("검수확인서", "acceptance"),
        ("검수조서", "acceptance"),
        ("완료보고서", "acceptance"),
        ("견적서", "estimate"),
        ("발주서", "purchase_order"),
        ("주문서", "order_form"),
        ("거래명세서", "transaction_statement"),
        ("거래명세표", "transaction_statement"),
        ("명세서", "statement"),
        ("청구서", "bill"),
        ("입금표", "deposit_slip"),
        ("지출결의서", "expense_approval"),
        ("품의서", "approval_request"),
        ("휴가신청서", "leave_request"),
        ("출장신청서", "trip_request"),
        ("출장보고서", "trip_report"),
        ("공문", "official_letter"),
        ("보고서", "report"),
        ("회의록", "meeting_minutes"),
        ("이력서", "resume"),
        ("계약서", "dev_contract"),
        ("신청서", "application_form"),
        ("영수증", "receipt"),
        ("세금계산서", "tax_invoice"),
        ("계산서", "invoice_exempt"),
        ("사업자등록증", "business_registration"),
        ("납품확인서", "delivery_confirmation"),
        ("검수조서", "inspection_report"),
        ("위임장", "power_of_attorney"),
        ("동의서", "consent_form"),
        ("확인서", "confirmation"),
    ];
    let near = |k: &str| {
        let extra = n.chars().count().saturating_sub(k.chars().count());
        n == k || ((n.ends_with(k) || n.starts_with(k)) && extra <= 4)
    };
    if let Some((_, s)) = TABLE.iter().find(|(k, _)| near(k)) {
        return s.to_string();
    }
    let ascii = schema::key_from_label(name, "");
    if schema::is_key(&ascii) && ascii.len() >= 3 {
        return ascii;
    }
    let h = blake3::hash(n.as_bytes());
    format!("form_{}", &h.to_hex()[..6])
}

/// The type slug a new form gets when its name gave `base`.
///
/// The slug comes from the end of the name, so "결과보고서" and "검토보고서" both give `report`.
/// A second form of a type is not its default and is never picked automatically: the form
/// registered later was silently unused. When a form of the user already owns the type, the new
/// one gets a type of its own (`report_2`, …) — the classifier then tells the two apart by their
/// names. A type that only has the built-in sample is taken over as before (the user's form
/// becomes its default).
pub fn free_doc_type(conn: &rusqlite::Connection, base: &str) -> rusqlite::Result<String> {
    let count = |slug: &str, user_only: bool| -> rusqlite::Result<i64> {
        conn.query_row(
            "SELECT COUNT(*) FROM schemas WHERE doc_type = ?1 AND (?2 = 0 OR source <> 'builtin')",
            rusqlite::params![slug, user_only],
            |r| r.get(0),
        )
    };
    if !schema::is_key(base) || count(base, true)? == 0 {
        return Ok(base.to_string());
    }
    // Room for the suffix inside the 40 characters of a key.
    let stem: String = base.chars().take(36).collect();
    for k in 2..1000 {
        let candidate = format!("{stem}_{k}");
        if count(&candidate, false)? == 0 {
            return Ok(candidate);
        }
    }
    Ok(base.to_string())
}

/// [`free_doc_type`] for a draft that is not saved yet (a saved schema keeps its type).
fn settle_doc_type(db: &Db, draft: &mut SchemaDraft) {
    if draft.id.is_some() {
        return;
    }
    let base = draft.doc_type.clone();
    match db.with(|c| free_doc_type(c, &base)) {
        Ok(slug) => draft.doc_type = slug,
        Err(e) => log::warn!("form type for '{}' not checked: {e}", draft.name),
    }
}

fn is_placeholder(s: &str) -> bool {
    let t = s.trim();
    t.is_empty()
        || t.chars().all(|c| {
            matches!(
                c,
                '_' | ' ' | '.' | '·' | '(' | ')' | '（' | '）' | '□' | '☐' | '-' | '－' | '~' | '∼' | '/' | '\u{a0}'
            )
        })
        || t == "년 월 일"
        || t == "년월일"
}

fn is_label(s: &str) -> bool {
    let t = s.trim().trim_end_matches([':', '：', '*', '※']).trim();
    !t.is_empty() && t.chars().count() <= MAX_LABEL_CHARS && t.chars().any(|c| c.is_alphabetic()) && !t.contains('\n')
}

struct Collector {
    fields: Vec<FieldDef>,
    keys: Vec<String>,
}

impl Collector {
    fn new() -> Collector {
        Collector { fields: Vec::new(), keys: Vec::new() }
    }
    fn unique(&mut self, key: String) -> String {
        let mut k = key;
        let base = k.clone();
        let mut n = 2;
        while self.keys.contains(&k) {
            k = format!("{base}_{n}");
            n += 1;
        }
        self.keys.push(k.clone());
        k
    }
    fn push_scalar(&mut self, label: &str, example: Option<String>) {
        let label = label.trim().trim_end_matches([':', '：', '*', '※']).trim().to_string();
        if self.fields.len() >= schema::MAX_FIELDS || self.fields.iter().any(|f| f.label_ko == label) {
            return;
        }
        let key = self.unique(schema::key_from_label(&label, &format!("field_{}", self.fields.len() + 1)));
        let kind = schema::kind_from_label(&label);
        let mut f = FieldDef::new(&key, &label, &english_label(&label, &key), kind);
        f.example = example.filter(|e| !e.trim().is_empty());
        if kind == FieldKind::Rrn || label.contains("생년월일") {
            f.mask = true;
        }
        self.fields.push(f);
    }
    fn push_table(&mut self, name: &str, columns: Vec<String>) {
        if self.fields.len() >= schema::MAX_FIELDS || columns.is_empty() {
            return;
        }
        let name = if name.trim().is_empty() { "items".to_string() } else { name.trim().to_string() };
        let key = self.unique(schema::key_from_label(&name, "items"));
        let mut cols = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for (i, c) in columns.iter().enumerate() {
            let label = c.trim().trim_end_matches([':', '：']).to_string();
            let mut ck = schema::column_key(schema::key_from_label(&label, &format!("col_{}", i + 1)));
            let base = ck.clone();
            let mut n = 2;
            while seen.contains(&ck) {
                ck = format!("{base}_{n}");
                n += 1;
            }
            seen.push(ck.clone());
            cols.push(FieldDef::new(&ck, &label, &english_label(&label, &ck), schema::kind_from_label(&label)));
        }
        self.fields.push(FieldDef::new(&key, &name, &english_label(&name, &key), FieldKind::Table).with(cols));
    }
}

/// English label guess: the key words title-cased (good enough until the user edits the grid).
fn english_label(label: &str, key: &str) -> String {
    if label.is_ascii() {
        return label.to_string();
    }
    key.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Fields from the converted document model (HWPX / DOCX / HWP forms):
/// label cells followed by an empty cell, header-row tables with empty body
/// rows (repeat regions), and "label: ____" paragraphs.
pub fn derive_from_model(model: &DocModel, name: Option<&str>) -> SchemaDraft {
    let mut col = Collector::new();
    let mut last_paragraph = String::new();
    for block in &model.blocks {
        match block {
            Block::Paragraph(p) => {
                let text = p.text();
                paragraph_fields(&text, &mut col);
                if !text.trim().is_empty() {
                    last_paragraph = text.trim().to_string();
                }
            }
            Block::Table(t) => {
                let rows: Vec<Vec<String>> =
                    t.rows.iter().map(|r| r.iter().map(|c| c.text().trim().to_string()).collect()).collect();
                if rows.is_empty() {
                    continue;
                }
                let header_like = rows[0].len() >= 2 && rows[0].iter().all(|c| is_label(c));
                let body_empty = rows.len() >= 2 && rows[1..].iter().any(|r| r.iter().all(|c| is_placeholder(c)));
                if header_like && body_empty {
                    col.push_table(&last_paragraph, rows[0].clone());
                    continue;
                }
                for row in &rows {
                    let mut j = 0;
                    while j < row.len() {
                        let cell = &row[j];
                        if is_label(cell) {
                            if let Some(next) = row.get(j + 1) {
                                if is_placeholder(next) || (next.chars().count() <= 40 && !is_label(next)) {
                                    col.push_scalar(cell, (!is_placeholder(next)).then(|| next.clone()));
                                    j += 2;
                                    continue;
                                }
                            }
                            // "라벨: 값" inside one cell.
                            paragraph_fields(cell, &mut col);
                        }
                        j += 1;
                    }
                }
            }
            _ => {}
        }
    }
    let name = name
        .map(str::to_string)
        .filter(|n| !n.trim().is_empty())
        .or_else(|| model.title.clone())
        .or_else(|| {
            model.blocks.iter().find_map(|b| match b {
                Block::Paragraph(p) if p.heading.is_some() && !p.is_empty() => Some(p.text().trim().to_string()),
                _ => None,
            })
        })
        .unwrap_or_else(|| "양식".to_string());
    draft(name, col.fields)
}

fn paragraph_fields(text: &str, col: &mut Collector) {
    static LINE: OnceLock<Regex> = OnceLock::new();
    let re = LINE.get_or_init(|| {
        Regex::new(r"(?m)^\s*([^\n:：_]{1,24}?)\s*[:：]\s*(_{2,}|\(\s*\)|（\s*）|□|☐|$)").expect("static regex")
    });
    for cap in re.captures_iter(text) {
        let label = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        if is_label(label) {
            col.push_scalar(label, None);
        }
    }
    static BLANK: OnceLock<Regex> = OnceLock::new();
    let re2 = BLANK.get_or_init(|| Regex::new(r"(?m)^\s*([^\n:：_]{1,24}?)\s+_{3,}\s*$").expect("static regex"));
    for cap in re2.captures_iter(text) {
        let label = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        if is_label(label) {
            col.push_scalar(label, None);
        }
    }
}

/// Fields from plain page text (OCR / text layer of a PDF form) without a model.
pub fn derive_from_text(text: &str, tables: &[ExtractedTable], name: Option<&str>) -> SchemaDraft {
    let mut col = Collector::new();
    paragraph_fields(text, &mut col);
    for line in text.lines() {
        // "라벨<tab>값" / "라벨 : 값" pairs separated by wide gaps.
        for part in line.split(['\t', '|']) {
            if let Some((l, v)) = part.split_once([':', '：']) {
                if is_label(l) {
                    col.push_scalar(l, (!is_placeholder(v)).then(|| v.trim().to_string()));
                }
            }
        }
    }
    for t in tables {
        let header_like = t.header.len() >= 2 && t.header.iter().all(|c| is_label(c));
        let body_empty = t.rows.iter().any(|r| r.iter().all(|c| is_placeholder(c)));
        if header_like && (body_empty || t.rows.len() >= 2) {
            col.push_table("items", t.header.clone());
        } else {
            for row in &t.rows {
                let mut j = 0;
                while j < row.len() {
                    if is_label(&row[j]) {
                        if let Some(next) = row.get(j + 1) {
                            if is_placeholder(next) {
                                col.push_scalar(&row[j], None);
                                j += 2;
                                continue;
                            }
                        }
                    }
                    j += 1;
                }
            }
        }
    }
    let name = name.map(str::to_string).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
        text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("양식").trim_start_matches('#').trim().to_string()
    });
    draft(name, col.fields)
}

fn draft(name: String, fields: Vec<FieldDef>) -> SchemaDraft {
    SchemaDraft {
        id: None,
        doc_type: slug_for_name(&name),
        name_en: None,
        source: SchemaSource::Form,
        description_ko: None,
        description_en: None,
        definition: SchemaDef { fields, rules: Vec::new() },
        form_document_id: None,
        note: None,
        name,
        sample_document_ids: Vec::new(),
    }
}

/// Field discovery with the VLM (PDF / image forms, §3.4.0 step 2).
pub fn derive_with_vlm(
    vlm: &Vlm<'_>,
    image_url: Option<&str>,
    text: Option<&str>,
    name: Option<&str>,
) -> Result<SchemaDraft> {
    let prompt = prompts::form_fields(name, text);
    let system = json!({ "role": "system", "content": prompts::SYSTEM_JSON });
    let messages = match image_url {
        Some(url) => vec![system, user_message_with_image(&prompt, url)],
        None => vec![system, user_message(&prompt)],
    };
    let (v, _) = vlm.call_json(Stage::FormFields, &messages, "form_fields", &schema::metaschema(), None)?;
    let mut d = schema::draft_from_metaschema(&v, SchemaSource::Form);
    if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
        d.name = n.trim().to_string();
        d.doc_type = slug_for_name(&d.name);
    }
    Ok(d)
}

/// Derives the draft for a registered form document: office formats from
/// their structure, PDFs / images with the VLM (text heuristics without one).
/// One document of [`discover`]; kept for the single-document command.
pub fn derive(
    deps: &ImportDeps<'_>,
    vlm: Option<&Vlm<'_>>,
    doc: &DocumentRecord,
    name: Option<&str>,
) -> Result<SchemaDraft> {
    let (mut draft, _) = discover_document(deps, vlm, doc, name, None, &mut |_, _| {})?;
    settle_doc_type(deps.db, &mut draft);
    Ok(draft)
}

/// Saves a confirmed form draft and marks the sample document as the form
/// (`role = form`, `doc_type = <slug>` with source `form`).
pub fn register(db: &Db, draft: &SchemaDraft) -> Result<SchemaRecord> {
    let mut draft = draft.clone();
    draft.source = SchemaSource::Form;
    if draft.doc_type.trim().is_empty() || !schema::is_key(&draft.doc_type) {
        // No type given: the name decides, without landing on a type another form of the user owns.
        // A type the draft carries is the user's choice (the editor shows it) and is kept.
        draft.doc_type = slug_for_name(&draft.name);
        settle_doc_type(db, &mut draft);
    }
    // Every sample document becomes a form of the type (one draft, 1..N samples since 0.3.0).
    let mut sample_docs: Vec<String> = draft.form_document_id.iter().cloned().collect();
    for id in &draft.sample_document_ids {
        if !sample_docs.contains(id) {
            sample_docs.push(id.clone());
        }
    }
    db.with_tx(move |tx| {
        let saved = repo::save_schema(tx, &draft)?;
        for id in &sample_docs {
            if docs::get_document(tx, id)?.is_some() {
                docs::set_role(tx, id, DocumentRole::Form)?;
                docs::set_doc_type(tx, id, Some(&saved.doc_type), Some("form"))?;
            }
        }
        Ok(saved)
    })
}

/// Creates (or modifies, when `base` is given) a schema from a description.
pub fn nl_schema(vlm: &Vlm<'_>, description: &str, base: Option<&SchemaRecord>) -> Result<SchemaDraft> {
    let d = description.trim();
    if d.is_empty() {
        return Err(CoreError::Invalid("description_empty".into()));
    }
    let prompt = prompts::nl_schema(d, base.map(|b| (b.name.as_str(), &b.definition)));
    let messages = vec![json!({ "role": "system", "content": prompts::SYSTEM_JSON }), user_message(&prompt)];
    let (v, _) = vlm.call_json(Stage::NlSchema, &messages, "nl_schema", &schema::metaschema(), None)?;
    let mut draft = schema::draft_from_metaschema(&v, SchemaSource::Nl);
    if let Some(b) = base {
        // Modification keeps the identity and the rules of the schema being edited.
        draft.id = Some(b.id.clone());
        draft.doc_type = b.doc_type.clone();
        if draft.name.trim().is_empty() || draft.name == "새 스키마" {
            draft.name = b.name.clone();
        }
        draft.name_en = draft.name_en.or_else(|| b.name_en.clone());
        draft.definition.rules = b.definition.rules.clone();
        draft.source = b.source;
        draft.form_document_id = b.form_document_id.clone();
        draft.note = Some(format!("NL: {}", prompts::truncate(d, 80)));
    }
    Ok(draft)
}

/// Source kinds whose structure can be read without a model.
pub fn structural_kind(kind: SourceKind) -> bool {
    kind.needs_conversion()
}

// ---------------------------------------------------------------------------
// Multi-document discovery (0.3.0 §2): one job over 1..N sample documents
// ---------------------------------------------------------------------------

/// Pages of a scanned / PDF form the model reads: the first rendered page, then pages with a table.
const MAX_DISCOVERY_PAGES: usize = 3;
/// Enum options kept after merging (the grammar lint warns above this).
const MAX_MERGED_OPTIONS: usize = 12;
/// Table columns kept after merging.
const MAX_MERGED_COLUMNS: usize = 20;

/// What one sample document contributed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocDiscovery {
    pub doc_id: String,
    pub title: String,
    /// `structure` (office tables), `model` (vision model), `text` (text heuristics) or `failed`.
    pub source: String,
    pub pages_used: Vec<u32>,
    pub field_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Result of a discovery job: the unsaved draft the editor opens plus the per-document summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryOutcome {
    /// Discriminator of the job result (`schema_discovery`).
    pub kind: String,
    pub draft: SchemaDraft,
    pub documents: Vec<DocDiscovery>,
    /// Fields adopted (seen in at least ⌈n/2⌉ documents) and dropped by that threshold.
    pub merged_fields: usize,
    pub dropped_fields: usize,
}

/// Progress of [`discover`]: the document being read and, for scanned pages, the page.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryProgress<'a> {
    pub index: usize,
    pub total: usize,
    pub title: &'a str,
    pub page: Option<(u32, u32)>,
}

/// Counts of [`merge_drafts`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MergeStats {
    pub adopted: usize,
    pub dropped: usize,
}

fn vote(votes: &mut Vec<(String, usize)>, v: &str) {
    let v = v.trim();
    if v.is_empty() {
        return;
    }
    match votes.iter_mut().find(|(k, _)| k == v) {
        Some((_, n)) => *n += 1,
        None => votes.push((v.to_string(), 1)),
    }
}

fn top(votes: &[(String, usize)]) -> Option<&str> {
    votes.iter().max_by_key(|(_, n)| *n).map(|(k, _)| k.as_str())
}

/// A field's identity across documents: its canonical key (known label → key, else the key itself).
fn field_norm(f: &FieldDef) -> String {
    let base = if f.label_ko.trim().is_empty() { f.key.clone() } else { schema::key_from_label(&f.label_ko, &f.key) };
    if f.kind == FieldKind::Table {
        format!("{base}/table")
    } else {
        base
    }
}

/// `String` is the least specific kind; a typed vote wins a tie against it.
fn kind_rank(k: FieldKind) -> u8 {
    match k {
        FieldKind::String => 0,
        FieldKind::Table | FieldKind::Group => 2,
        _ => 1,
    }
}

struct ColumnAcc {
    norm: String,
    first: usize,
    votes: usize,
    def: FieldDef,
}

struct FieldAcc {
    norm: String,
    first: (usize, usize),
    /// Drafts the field appears in (once per draft).
    docs: usize,
    keys: Vec<(String, usize)>,
    labels_ko: Vec<(String, usize)>,
    label_en: String,
    kinds: Vec<(FieldKind, usize)>,
    required_votes: usize,
    options: Vec<String>,
    columns: Vec<ColumnAcc>,
    example: Option<String>,
    description: Option<String>,
    regex_hint: Option<String>,
    mask: bool,
}

impl FieldAcc {
    fn new(norm: String, first: (usize, usize)) -> FieldAcc {
        FieldAcc {
            norm,
            first,
            docs: 0,
            keys: Vec::new(),
            labels_ko: Vec::new(),
            label_en: String::new(),
            kinds: Vec::new(),
            required_votes: 0,
            options: Vec::new(),
            columns: Vec::new(),
            example: None,
            description: None,
            regex_hint: None,
            mask: false,
        }
    }

    fn add(&mut self, f: &FieldDef) {
        self.docs += 1;
        vote(&mut self.keys, &f.key);
        vote(&mut self.labels_ko, &f.label_ko);
        if self.label_en.is_empty() && !f.label_en.trim().is_empty() {
            self.label_en = f.label_en.trim().to_string();
        }
        match self.kinds.iter_mut().find(|(k, _)| *k == f.kind) {
            Some((_, n)) => *n += 1,
            None => self.kinds.push((f.kind, 1)),
        }
        if f.required {
            self.required_votes += 1;
        }
        for o in &f.options {
            if !self.options.contains(o) && self.options.len() < MAX_MERGED_OPTIONS {
                self.options.push(o.clone());
            }
        }
        for (ci, c) in f.fields.iter().enumerate() {
            let norm = field_norm(c);
            match self.columns.iter_mut().find(|x| x.norm == norm) {
                Some(x) => x.votes += 1,
                None => self.columns.push(ColumnAcc { norm, first: ci, votes: 1, def: c.clone() }),
            }
        }
        if self.example.is_none() {
            self.example = f.example.clone().filter(|e| !e.trim().is_empty());
        }
        if self.description.is_none() {
            self.description = f.description.clone().filter(|d| !d.trim().is_empty());
        }
        if self.regex_hint.is_none() {
            self.regex_hint = f.regex_hint.clone().filter(|r| !r.trim().is_empty());
        }
        self.mask |= f.mask;
    }

    fn kind(&self) -> FieldKind {
        self.kinds
            .iter()
            .max_by(|a, b| a.1.cmp(&b.1).then(kind_rank(a.0).cmp(&kind_rank(b.0))))
            .map(|(k, _)| *k)
            .unwrap_or_default()
    }

    fn into_field(mut self, key: String, n_drafts: usize) -> FieldDef {
        let label_ko = top(&self.labels_ko).unwrap_or(&key).to_string();
        let label_en = if self.label_en.is_empty() { english_label(&label_ko, &key) } else { self.label_en.clone() };
        let kind = self.kind();
        let mut f = FieldDef::new(&key, &label_ko, &label_en, kind);
        // Required = printed on every sample (a single sample keeps what was derived from it).
        f.required = if n_drafts >= 2 { self.docs == n_drafts } else { self.required_votes > 0 };
        f.example = self.example.take();
        f.description = self.description.take();
        f.regex_hint = self.regex_hint.take();
        f.mask = self.mask;
        if kind == FieldKind::Enum {
            f.options = std::mem::take(&mut self.options);
        }
        if kind == FieldKind::Table {
            self.columns.sort_by(|a, b| b.votes.cmp(&a.votes).then(a.first.cmp(&b.first)));
            let mut keys: Vec<String> = Vec::new();
            for c in self.columns.into_iter().take(MAX_MERGED_COLUMNS) {
                let mut col = c.def;
                let base = col.key.clone();
                let mut n = 2;
                while keys.contains(&col.key) {
                    col.key = format!("{base}_{n}");
                    n += 1;
                }
                keys.push(col.key.clone());
                col.fields.clear();
                f.fields.push(col);
            }
        }
        f
    }
}

/// Deterministic cross-field rules a merged field set supports (`KNOWN_RULES` only — the model
/// never writes rules).
pub fn infer_rules(def: &SchemaDef) -> Vec<String> {
    let has = |k: &str| def.field(k).is_some();
    let table_amount = def
        .fields
        .iter()
        .filter(|f| f.kind == FieldKind::Table)
        .any(|t| t.fields.iter().any(|c| matches!(c.key.as_str(), "supply_amount" | "amount")));
    let percent = def.fields.iter().any(|f| {
        f.key.ends_with("_percent") || f.key.ends_with("_rate") || f.fields.iter().any(|c| c.key == "progress")
    });
    let mut rules = Vec::new();
    if has("supply_total") && has("tax_total") && (has("grand_total") || has("total")) {
        rules.push(schema::RULE_SUPPLY_TAX_TOTAL.to_string());
        rules.push(schema::RULE_TAX_RATE.to_string());
    }
    if table_amount && has("supply_total") {
        rules.push(schema::RULE_ITEMS_SUM_SUPPLY.to_string());
    } else if table_amount && (has("grand_total") || has("total")) {
        rules.push(schema::RULE_ITEMS_SUM_TOTAL.to_string());
    }
    if has("period_start") && has("period_end") {
        rules.push(schema::RULE_PERIOD_ORDER.to_string());
    }
    if percent {
        rules.push(schema::RULE_PERCENT_RANGE.to_string());
    }
    if has("total_cases") && has("passed") && has("failed") {
        rules.push(schema::RULE_TEST_COUNTS.to_string());
    }
    rules
}

/// Merges the drafts derived from several samples into one: fields are matched by their canonical
/// key, kept when at least `min_docs` drafts carry them, ordered by frequency then first
/// appearance; labels and kinds by majority (a typed kind beats `string` on a tie), enum options
/// and table columns by union; `required` means "on every sample". The user's `hint` names the
/// form, else the most common derived name.
pub fn merge_drafts(drafts: &[SchemaDraft], hint: Option<&str>, min_docs: usize) -> (SchemaDraft, MergeStats) {
    let n = drafts.len();
    let min_docs = min_docs.clamp(1, n.max(1));
    let mut accs: Vec<FieldAcc> = Vec::new();
    for (di, d) in drafts.iter().enumerate() {
        let mut seen: Vec<String> = Vec::new();
        for (fi, f) in d.definition.fields.iter().enumerate() {
            // Two fields of one sample never merge into each other.
            let mut norm = field_norm(f);
            let base = norm.clone();
            let mut k = 2;
            while seen.contains(&norm) {
                norm = format!("{base}#{k}");
                k += 1;
            }
            seen.push(norm.clone());
            let acc = match accs.iter_mut().position(|a| a.norm == norm) {
                Some(i) => &mut accs[i],
                None => {
                    accs.push(FieldAcc::new(norm, (di, fi)));
                    accs.last_mut().expect("just pushed")
                }
            };
            acc.add(f);
        }
    }
    let total = accs.len();
    let mut adopted: Vec<FieldAcc> = accs.into_iter().filter(|a| a.docs >= min_docs).collect();
    adopted.sort_by(|a, b| b.docs.cmp(&a.docs).then(a.first.cmp(&b.first)));
    adopted.truncate(schema::MAX_FIELDS);
    let stats = MergeStats { adopted: adopted.len(), dropped: total.saturating_sub(adopted.len()) };
    let mut keys: Vec<String> = Vec::new();
    let mut fields = Vec::new();
    for acc in adopted {
        let base = top(&acc.keys).map(str::to_string).unwrap_or_else(|| acc.norm.replace("/table", ""));
        let base = if schema::is_key(&base) { base } else { schema::key_from_label(&base, "field") };
        let mut key = base.clone();
        let mut k = 2;
        while keys.contains(&key) || matches!(key.as_str(), "value" | "evidence" | "confidence") {
            key = format!("{base}_{k}");
            k += 1;
        }
        keys.push(key.clone());
        fields.push(acc.into_field(key, n));
    }
    // Name: the user's hint, else the most common derived name; the type slug follows the name
    // unless that gives a hash slug while the samples agreed on a real one.
    let mut names: Vec<(String, usize)> = Vec::new();
    let mut types: Vec<(String, usize)> = Vec::new();
    for d in drafts {
        vote(&mut names, &d.name);
        vote(&mut types, &d.doc_type);
    }
    let name = hint
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .or_else(|| top(&names).map(str::to_string))
        .unwrap_or_else(|| "양식".to_string());
    let mut doc_type = slug_for_name(&name);
    if doc_type.starts_with("form_") {
        if let Some(t) = top(&types).filter(|t| schema::is_key(t) && !t.starts_with("form_")) {
            doc_type = t.to_string();
        }
    }
    let mut sample_document_ids: Vec<String> = Vec::new();
    for d in drafts {
        for id in d.form_document_id.iter().chain(d.sample_document_ids.iter()) {
            if !sample_document_ids.contains(id) {
                sample_document_ids.push(id.clone());
            }
        }
    }
    let form_document_id = sample_document_ids.first().cloned();
    let mut definition = SchemaDef { fields, rules: Vec::new() };
    definition.rules = infer_rules(&definition);
    let draft = SchemaDraft {
        id: None,
        name_en: drafts.iter().find_map(|d| d.name_en.clone().filter(|n| !n.trim().is_empty())),
        doc_type,
        source: SchemaSource::Form,
        description_ko: drafts.iter().find_map(|d| d.description_ko.clone().filter(|x| !x.trim().is_empty())),
        description_en: drafts.iter().find_map(|d| d.description_en.clone().filter(|x| !x.trim().is_empty())),
        definition,
        form_document_id,
        note: None,
        name,
        sample_document_ids: sample_document_ids.into_iter().skip(1).collect(),
    };
    (draft, stats)
}

/// Fields of one sample document: office formats from their structure, scanned / PDF forms
/// through the model page by page (first page + pages with tables, at most
/// [`MAX_DISCOVERY_PAGES`]), text heuristics when the model is missing or fails.
pub fn discover_document(
    deps: &ImportDeps<'_>,
    vlm: Option<&Vlm<'_>>,
    doc: &DocumentRecord,
    name: Option<&str>,
    cancel: Option<&AtomicBool>,
    on_page: &mut dyn FnMut(u32, u32),
) -> Result<(SchemaDraft, DocDiscovery)> {
    let title = doc.title.clone().unwrap_or_else(|| doc.original_name.clone());
    let abs = doc.absolute_path(deps.paths);
    let fallback_name =
        name.map(str::to_string).filter(|n| !n.trim().is_empty()).or_else(|| doc.title.clone()).unwrap_or_else(|| {
            abs.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "양식".into())
        });
    let pages = deps.db.with(|c| docs::list_pages(c, &doc.id, true))?;
    let text: String = pages.iter().filter_map(|p| p.text.as_deref()).collect::<Vec<_>>().join("\n");
    let mut source = "text";
    let mut pages_used: Vec<u32> = Vec::new();
    let mut note: Option<String> = None;
    let from_text = |note: Option<String>| -> Result<SchemaDraft> {
        if text.trim().is_empty() {
            return Err(CoreError::Invalid("no_model".into()));
        }
        let tables: Vec<ExtractedTable> =
            pages.iter().flat_map(|p| super::ocr::parse_tables(p.text.as_deref().unwrap_or(""), p.page_no)).collect();
        let mut d = derive_from_text(&text, &tables, Some(&fallback_name));
        d.note = note;
        Ok(d)
    };
    let mut draft = if doc.source_kind.needs_conversion() && abs.is_file() {
        let model = convert::read_document(&abs, doc.source_kind)?;
        let d = derive_from_model(&model, Some(&fallback_name));
        if d.definition.fields.is_empty() {
            match vlm {
                Some(v) => {
                    source = "model";
                    derive_with_vlm(
                        v,
                        None,
                        Some(text.as_str()).filter(|t| !t.trim().is_empty()),
                        Some(&fallback_name),
                    )?
                }
                None => d,
            }
        } else {
            source = "structure";
            d
        }
    } else {
        let cache = DocumentCache::for_document(deps.paths, &doc.id);
        let rendered = |p: &&crate::documents::PageRecord| p.width_px > 0 && cache.page_file(p.page_no).is_file();
        // The first rendered page, then pages carrying a table.
        let mut picks: Vec<&crate::documents::PageRecord> = pages.iter().filter(rendered).take(1).collect();
        for p in pages.iter().filter(rendered) {
            if picks.len() >= MAX_DISCOVERY_PAGES {
                break;
            }
            if picks.iter().any(|q| q.page_no == p.page_no) {
                continue;
            }
            if !super::ocr::parse_tables(p.text.as_deref().unwrap_or(""), p.page_no).is_empty() {
                picks.push(p);
            }
        }
        match vlm {
            Some(v) if !picks.is_empty() => {
                let mut per_page: Vec<SchemaDraft> = Vec::new();
                let mut failures: Vec<String> = Vec::new();
                let total = picks.len() as u32;
                for (i, p) in picks.iter().enumerate() {
                    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                        return Err(CoreError::Cancelled);
                    }
                    on_page(i as u32 + 1, total);
                    let img = image::prepare(&cache.page_file(p.page_no), v.geometry, 2_048)?;
                    match derive_with_vlm(
                        v,
                        Some(&img.url),
                        p.text.as_deref().filter(|t| !t.trim().is_empty()),
                        Some(&fallback_name),
                    ) {
                        Ok(d) => {
                            pages_used.push(p.page_no);
                            per_page.push(d);
                        }
                        Err(e) => {
                            log::warn!("form field discovery on page {} of {} failed: {e}", p.page_no, doc.id);
                            failures.push(format!("{}쪽: {e}", p.page_no));
                        }
                    }
                }
                if per_page.is_empty() {
                    // A model failure must not leave the user without a draft (user report 2026-11-09):
                    // the text / table heuristics stand in and the note says why.
                    if text.trim().is_empty() {
                        return Err(CoreError::Invalid(format!("no_fields: {}", failures.join("; "))));
                    }
                    from_text(Some(format!("모델 응답 실패로 텍스트 규칙으로 도출했습니다: {}", failures.join("; "))))?
                } else {
                    source = "model";
                    if !failures.is_empty() {
                        note = Some(format!("일부 쪽은 모델 응답 실패로 건너뛰었습니다: {}", failures.join("; ")));
                    }
                    let (merged, _) = merge_drafts(&per_page, Some(&fallback_name), 1);
                    merged
                }
            }
            Some(v) => match derive_with_vlm(
                v,
                None,
                Some(text.as_str()).filter(|t| !t.trim().is_empty()),
                Some(&fallback_name),
            ) {
                Ok(d) => {
                    source = "model";
                    d
                }
                Err(e) if !text.trim().is_empty() => {
                    log::warn!("form field discovery with the model failed, using the text heuristics: {e}");
                    from_text(Some(format!("모델 응답 실패로 텍스트 규칙으로 도출했습니다: {e}")))?
                }
                Err(e) => return Err(e),
            },
            None => from_text(None)?,
        }
    };
    draft.source = SchemaSource::Form;
    draft.form_document_id = Some(doc.id.clone());
    draft.sample_document_ids.clear();
    if draft.definition.fields.is_empty() {
        draft.definition.fields.push(FieldDef::new("title", "제목", "Title", FieldKind::String));
    }
    if note.is_some() {
        draft.note = note.clone();
    }
    let summary = DocDiscovery {
        doc_id: doc.id.clone(),
        title,
        source: source.into(),
        pages_used,
        field_count: draft.definition.fields.len(),
        note: draft.note.clone(),
    };
    Ok((draft, summary))
}

/// Field discovery over 1..N sample documents (0.3.0 §2): each sample is read on its own, the
/// drafts are merged (a field needs ⌈n/2⌉ samples), deterministic rules are attached and the
/// result goes to the editor unsaved. A sample that fails is reported, not fatal, while another
/// sample succeeded.
pub fn discover(
    deps: &ImportDeps<'_>,
    vlm: Option<&Vlm<'_>>,
    documents: &[DocumentRecord],
    name: Option<&str>,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(DiscoveryProgress<'_>),
) -> Result<DiscoveryOutcome> {
    if documents.is_empty() {
        return Err(CoreError::Invalid("no documents".into()));
    }
    let n = documents.len();
    let mut drafts: Vec<SchemaDraft> = Vec::new();
    let mut summaries: Vec<DocDiscovery> = Vec::new();
    for (i, doc) in documents.iter().enumerate() {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(CoreError::Cancelled);
        }
        let title = doc.title.clone().unwrap_or_else(|| doc.original_name.clone());
        progress(DiscoveryProgress { index: i, total: n, title: &title, page: None });
        let outcome = discover_document(deps, vlm, doc, name, cancel, &mut |p, t| {
            progress(DiscoveryProgress { index: i, total: n, title: &title, page: Some((p, t)) })
        });
        match outcome {
            Ok((d, s)) => {
                drafts.push(d);
                summaries.push(s);
            }
            Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
            Err(e) if n > 1 => {
                log::warn!("form field discovery skipped {}: {e}", doc.id);
                summaries.push(DocDiscovery {
                    doc_id: doc.id.clone(),
                    title,
                    source: "failed".into(),
                    pages_used: Vec::new(),
                    field_count: 0,
                    note: Some(e.to_string()),
                });
            }
            Err(e) => return Err(e),
        }
    }
    if drafts.is_empty() {
        let why: Vec<String> = summaries.iter().filter_map(|s| s.note.clone()).collect();
        return Err(CoreError::Invalid(format!("no_fields: {}", why.join("; "))));
    }
    let min_docs = drafts.len().div_ceil(2);
    let (mut draft, stats) = merge_drafts(&drafts, name, min_docs);
    settle_doc_type(deps.db, &mut draft);
    let notes: Vec<String> =
        summaries.iter().filter_map(|s| s.note.as_ref().map(|n| format!("{}: {n}", s.title))).collect();
    draft.note = (!notes.is_empty()).then(|| notes.join(" / "));
    if draft.definition.fields.is_empty() {
        draft.definition.fields.push(FieldDef::new("title", "제목", "Title", FieldKind::String));
    }
    Ok(DiscoveryOutcome {
        kind: "schema_discovery".into(),
        draft,
        documents: summaries,
        merged_fields: stats.adopted,
        dropped_fields: stats.dropped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::convert::model::{Cell, Paragraph, Table};

    fn cell(s: &str) -> Cell {
        Cell { blocks: vec![Block::Paragraph(Paragraph::plain(s))], row_span: 1, col_span: 1, header: false }
    }

    #[test]
    fn derives_fields_from_tables_and_label_lines() {
        let model = DocModel {
            title: Some("견 적 서".into()),
            blocks: vec![
                Block::Paragraph(Paragraph { heading: Some(1), ..Paragraph::plain("견적서") }),
                Block::Table(Table {
                    rows: vec![
                        vec![cell("수신"), cell(""), cell("견적일자"), cell("____")],
                        vec![cell("사업자등록번호"), cell("( )"), cell("담당자"), cell("")],
                        vec![cell("합계금액"), cell("")],
                    ],
                    col_widths_mm: vec![],
                }),
                Block::Paragraph(Paragraph::plain("견적 내역")),
                Block::Table(Table {
                    rows: vec![
                        vec![cell("품명"), cell("규격"), cell("수량"), cell("단가"), cell("금액")],
                        vec![cell(""), cell(""), cell(""), cell(""), cell("")],
                    ],
                    col_widths_mm: vec![],
                }),
                Block::Paragraph(Paragraph::plain("유효기간: ______\n비고 ____")),
            ],
            images: vec![],
            notes: vec![],
            page_mm: None,
            slides: vec![],
        };
        let d = derive_from_model(&model, None);
        assert_eq!(d.name, "견 적 서");
        assert_eq!(d.doc_type, "estimate", "견적서 is the software estimate form since 0.3.0");
        let keys: Vec<&str> = d.definition.fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["recipient", "date", "brn", "contact_person", "total", "items", "valid_until", "note"],
            "{keys:?}"
        );
        let items = d.definition.table("items").unwrap();
        assert_eq!(items.label_ko, "견적 내역");
        assert_eq!(
            items.fields.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
            vec!["item_name", "spec", "quantity", "unit_price", "amount"]
        );
        assert_eq!(items.fields[2].kind, FieldKind::Integer);
        assert_eq!(d.definition.field("brn").unwrap().kind, FieldKind::Brn);
        assert!(!schema::has_errors(&schema::lint(&d.definition)));
    }

    #[test]
    fn derives_from_plain_text_and_slugs_names() {
        let text = "휴가 신청서\n성명: ______\n부서: \n주민등록번호: ( )\n휴가기간 _____\n사유: 개인 사정";
        let d = derive_from_text(text, &[], None);
        assert_eq!(d.name, "휴가 신청서");
        assert_eq!(d.doc_type, "leave_request");
        let keys: Vec<&str> = d.definition.fields.iter().map(|f| f.key.as_str()).collect();
        assert!(
            keys.contains(&"name") && keys.contains(&"department") && keys.contains(&"rrn") && keys.contains(&"reason"),
            "{keys:?}"
        );
        assert!(d.definition.field("rrn").unwrap().mask);
        assert_eq!(slug_for_name("거래명세서(공급자용)"), "transaction_statement");
        assert_eq!(slug_for_name("Purchase Order"), "purchase_order");
        assert!(slug_for_name("특이한양식").starts_with("form_"));
    }

    fn fd(key: &str, label: &str, kind: FieldKind) -> FieldDef {
        FieldDef::new(key, label, key, kind)
    }

    fn sample(name: &str, doc: &str, fields: Vec<FieldDef>) -> SchemaDraft {
        let mut d = draft(name.to_string(), fields);
        d.form_document_id = Some(doc.into());
        d
    }

    #[test]
    fn merges_samples_by_majority_with_required_on_all_and_union_of_columns() {
        use FieldKind::*;
        let blank = sample(
            "견적서 양식",
            "d1",
            vec![
                fd("recipient", "수신", String),
                fd("date", "견적일자", Date),
                FieldDef::new("total", "합계금액", "Total", Amount).required(),
                fd("note", "비고", String),
                FieldDef::new("items", "견적 내역", "Items", Table).with(vec![
                    fd("item_name", "품명", String),
                    fd("quantity", "수량", Integer),
                    fd("amount", "금액", Amount),
                ]),
            ],
        );
        let filled = sample(
            "견적서",
            "d2",
            vec![
                fd("project_name", "사업명", String),
                fd("recipient", "수신", String),
                fd("date", "견적일자", Date),
                fd("total", "합계금액", Amount),
                FieldDef::new("items", "견적 내역", "Items", Table)
                    .with(vec![fd("item_name", "품명", String), fd("unit_price", "단가", Amount)]),
                fd("valid_until", "유효기간", String),
            ],
        );
        let (m, stats) = merge_drafts(&[blank.clone(), filled.clone()], Some("견적서"), 1);
        assert_eq!((m.name.as_str(), m.doc_type.as_str()), ("견적서", "estimate"));
        let keys: Vec<&str> = m.definition.fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["recipient", "date", "total", "items", "note", "project_name", "valid_until"], "{keys:?}");
        let required: Vec<&str> = m.definition.fields.iter().filter(|f| f.required).map(|f| f.key.as_str()).collect();
        assert_eq!(required, ["recipient", "date", "total", "items"], "on every sample");
        let items = m.definition.table("items").unwrap();
        assert_eq!(
            items.fields.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
            ["item_name", "quantity", "unit_price", "amount"],
            "columns by union: the shared one first, then by first position"
        );
        assert_eq!((stats.adopted, stats.dropped), (7, 0));
        assert_eq!(m.form_document_id.as_deref(), Some("d1"));
        assert_eq!(m.sample_document_ids, vec!["d2".to_string()]);
        assert_eq!(m.source, SchemaSource::Form);
        assert_eq!(m.definition.rules, vec![schema::RULE_ITEMS_SUM_TOTAL.to_string()]);

        // Three samples: a field on one sample falls below ⌈3/2⌉; a typed kind beats `string` on a tie.
        let third = sample("견적서", "d3", vec![fd("valid_until", "유효기간", Date), fd("recipient", "수신", String)]);
        let (m3, s3) = merge_drafts(&[blank, filled, third], None, 2);
        let keys: Vec<&str> = m3.definition.fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["recipient", "date", "total", "items", "valid_until"], "{keys:?}");
        assert_eq!(s3.dropped, 2, "note and project_name");
        assert_eq!(m3.definition.field("valid_until").unwrap().kind, Date);
        let required: Vec<&str> = m3.definition.fields.iter().filter(|f| f.required).map(|f| f.key.as_str()).collect();
        assert_eq!(required, ["recipient"]);
        assert_eq!(m3.name, "견적서", "the most common derived name without a hint");
        assert_eq!(m3.sample_document_ids, vec!["d2".to_string(), "d3".to_string()]);
    }

    #[test]
    fn infers_only_known_rules_from_the_merged_keys() {
        use FieldKind::*;
        let def = SchemaDef {
            fields: vec![
                fd("supply_total", "공급가액 합계", Amount),
                fd("tax_total", "세액 합계", Amount),
                fd("grand_total", "합계금액", Amount),
                FieldDef::new("items", "견적 내역", "Items", Table).with(vec![fd("supply_amount", "공급가액", Amount)]),
                fd("period_start", "시작", Date),
                fd("period_end", "종료", Date),
                fd("progress_percent", "진척률", Amount),
            ],
            rules: Vec::new(),
        };
        let rules = infer_rules(&def);
        assert_eq!(
            rules,
            [
                schema::RULE_SUPPLY_TAX_TOTAL,
                schema::RULE_TAX_RATE,
                schema::RULE_ITEMS_SUM_SUPPLY,
                schema::RULE_PERIOD_ORDER,
                schema::RULE_PERCENT_RANGE
            ]
        );
        assert!(rules.iter().all(|r| schema::KNOWN_RULES.contains(&r.as_str())));
        assert!(infer_rules(&SchemaDef { fields: vec![fd("name", "성명", String)], rules: Vec::new() }).is_empty());
    }
}
