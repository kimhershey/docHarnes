//! Natural language → rule JSON (§3.15 "정합성 규칙: 자연어로 정의 → 규칙 JSON").
//!
//! 1. Deterministic phrase rules: an operator trigger (Korean / English phrases or a
//!    comparison symbol) splits the sentence into two operands (Korean particles
//!    `…는 …와 같아야`, `…와 …의 차이`, English `A must equal B`); each operand is resolved
//!    against the document types the rule may refer to (project template + project
//!    documents, or every registered type) and their schema fields — labels in either
//!    language, keys, and kind-based synonyms (사업자번호 → any BRN field, 합계 → the total).
//! 2. Grammar-constrained model fallback (`Stage::NlSchema`): an enum of the allowed
//!    document types / field paths, the operator enum, tolerance and required flag.
//! 3. Without a model an unresolved rule comes back with `confidence < 0.5`, the
//!    unresolved names and `issues` explaining what the grid has to fix.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::engine::client::user_message;
use crate::error::{CoreError, Result};
use crate::pipeline::prompts::{self, Stage};
use crate::pipeline::repo as pipeline_repo;
use crate::pipeline::schema::{FieldKind, SchemaDef};
use crate::pipeline::validate::{normalize_amount, normalize_date};
use crate::pipeline::vlm::Vlm;
use crate::planner::compact;

use super::repo;
use super::{ProjectDeps, Rule, RuleDraft, RuleOp, RuleOperand};

// ---------------------------------------------------------------------------
// Resolution context: document types the rule may refer to and their fields
// ---------------------------------------------------------------------------

/// A field (scalar, group member `supplier.brn` or table column `parties.brn`) a rule can name.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInfo {
    pub path: String,
    pub label_ko: String,
    pub label_en: String,
    pub kind: FieldKind,
    /// Column of a repeated-row table (compared per row).
    pub column: bool,
}

impl FieldInfo {
    pub fn label(&self, ko: bool) -> &str {
        if ko {
            &self.label_ko
        } else {
            &self.label_en
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeInfo {
    pub slug: String,
    pub label_ko: String,
    pub label_en: String,
    pub fields: Vec<FieldInfo>,
    /// Part of the project / template the rule is written for (others are known types).
    pub in_scope: bool,
    /// An extraction schema with this slug exists (otherwise the default vocabulary is used).
    pub registered: bool,
}

impl TypeInfo {
    pub fn label(&self, ko: bool) -> &str {
        if ko {
            &self.label_ko
        } else {
            &self.label_en
        }
    }
    pub fn field(&self, path: &str) -> Option<&FieldInfo> {
        self.fields.iter().find(|f| f.path == path)
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuleContext {
    /// In-scope types first, then the other registered types.
    pub types: Vec<TypeInfo>,
}

impl RuleContext {
    pub fn type_by_slug(&self, slug: &str) -> Option<&TypeInfo> {
        self.types.iter().find(|t| t.slug == slug)
    }

    pub fn field(&self, slug: &str, path: &str) -> Option<&FieldInfo> {
        self.type_by_slug(slug).and_then(|t| t.field(path))
    }

    /// Label of a field path (the path itself when unknown).
    pub fn field_label(&self, slug: &str, path: &str, ko: bool) -> String {
        self.field(slug, path).map(|f| f.label(ko).to_string()).unwrap_or_else(|| path.to_string())
    }

    /// Label of a document type (slug when unknown).
    pub fn type_label(&self, slug: &str, ko: bool) -> String {
        match self.type_by_slug(slug) {
            Some(t) => t.label(ko).to_string(),
            None => repo::type_label(&[], slug, ko),
        }
    }

    fn scope(&self) -> Vec<&TypeInfo> {
        let v: Vec<&TypeInfo> = self.types.iter().filter(|t| t.in_scope).collect();
        if v.is_empty() {
            self.types.iter().collect()
        } else {
            v
        }
    }
}

/// Fields of a schema as rule operands: scalars, group members (`supplier.brn`) and table
/// columns (`parties.brn`, labelled "당사자 사업자번호").
pub fn fields_of_def(def: &SchemaDef) -> Vec<FieldInfo> {
    let mut out = Vec::new();
    for f in &def.fields {
        match f.kind {
            FieldKind::Group => {
                for c in &f.fields {
                    out.push(FieldInfo {
                        path: format!("{}.{}", f.key, c.key),
                        label_ko: c.label_ko.clone(),
                        label_en: c.label_en.clone(),
                        kind: c.kind,
                        column: false,
                    });
                }
            }
            FieldKind::Table => {
                for c in &f.fields {
                    out.push(FieldInfo {
                        path: format!("{}.{}", f.key, c.key),
                        label_ko: format!("{} {}", f.label_ko, c.label_ko),
                        label_en: format!("{} {}", f.label_en, c.label_en),
                        kind: c.kind,
                        column: true,
                    });
                }
            }
            _ => out.push(FieldInfo {
                path: f.key.clone(),
                label_ko: f.label_ko.clone(),
                label_en: f.label_en.clone(),
                kind: f.kind,
                column: false,
            }),
        }
    }
    out
}

fn fi(path: &str, ko: &str, en: &str, kind: FieldKind) -> FieldInfo {
    FieldInfo { path: path.into(), label_ko: ko.into(), label_en: en.into(), kind, column: false }
}

/// Default vocabulary of a type that has no registered schema yet (견적서 / 거래명세서 in the
/// built-in templates): the tax-invoice party / total keys, so template rules resolve and
/// later match a registered schema through the key aliases of the checker.
pub fn fallback_fields(slug: &str) -> Vec<FieldInfo> {
    use FieldKind::*;
    match slug {
        "quotation" | "transaction_statement" | "purchase_order" | "tax_invoice" | "invoice_exempt" | "estimate" => {
            vec![
                fi("issue_date", "작성일자", "Issue date", Date),
                fi("supplier.brn", "공급자 사업자등록번호", "Supplier business reg. no.", Brn),
                fi("supplier.name", "공급자 상호", "Supplier name", String),
                fi("buyer.brn", "공급받는자 사업자등록번호", "Buyer business reg. no.", Brn),
                fi("buyer.name", "공급받는자 상호", "Buyer name", String),
                fi("supply_total", "공급가액 합계", "Supply total", Amount),
                fi("tax_total", "세액 합계", "Tax total", Amount),
                fi("grand_total", "합계금액", "Grand total", Amount),
            ]
        }
        "financial_statement" => vec![
            fi("period_end", "기준일", "Period end", Date),
            fi("total_assets", "자산총계", "Total assets", Amount),
            fi("total_liabilities", "부채총계", "Total liabilities", Amount),
            fi("total_equity", "자본총계", "Total equity", Amount),
            fi("revenue", "매출액", "Revenue", Amount),
            fi("net_income", "당기순이익", "Net income", Amount),
        ],
        _ => vec![
            fi("title", "제목", "Title", String),
            fi("date", "날짜", "Date", Date),
            fi("amount", "금액", "Amount", Amount),
        ],
    }
}

/// Builds the resolution context: the project's template types + the types of its documents
/// (or the template's types) are in scope; every registered type is known.
pub fn context(
    conn: &Connection,
    project_id: Option<&str>,
    template_id: Option<&str>,
) -> rusqlite::Result<RuleContext> {
    let mut scope: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        if !s.is_empty() && !scope.iter().any(|x| x == s) {
            scope.push(s.to_string());
        }
    };
    let mut template = template_id.map(str::to_string);
    if let Some(pid) = project_id {
        if let Some(p) = repo::get_project(conn, pid)? {
            if template.is_none() {
                template = p.template_id.clone();
            }
            for d in repo::documents_of(conn, pid)? {
                if let Some(t) = d.document.doc_type.as_deref() {
                    push(t);
                }
            }
        }
    }
    if let Some(t) = template.as_deref() {
        if let Some(t) = repo::get_template(conn, t)? {
            for s in &t.required_doc_types {
                push(s);
            }
        }
    }
    let registered = pipeline_repo::doc_types(conn)?;
    let mut types: Vec<TypeInfo> = Vec::new();
    let add = |slug: &str, in_scope: bool, types: &mut Vec<TypeInfo>| -> rusqlite::Result<()> {
        if types.iter().any(|t| t.slug == slug) {
            return Ok(());
        }
        let schema = pipeline_repo::schema_for_doc_type(conn, slug)?;
        let info = registered.iter().find(|d| d.slug == slug);
        let (label_ko, label_en) = match info {
            Some(i) => (i.label_ko.clone(), i.label_en.clone()),
            None => (repo::type_label(&[], slug, true), repo::type_label(&[], slug, false)),
        };
        let (fields, is_registered) = match &schema {
            Some(s) => (fields_of_def(&s.definition), true),
            None => (fallback_fields(slug), false),
        };
        types.push(TypeInfo {
            slug: slug.to_string(),
            label_ko,
            label_en,
            fields,
            in_scope,
            registered: is_registered,
        });
        Ok(())
    };
    for s in &scope {
        add(s, true, &mut types)?;
    }
    for d in &registered {
        add(&d.slug, scope.is_empty(), &mut types)?;
    }
    Ok(RuleContext { types })
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

fn has_hangul(s: &str) -> bool {
    s.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c))
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static regex"))
}

/// Lower-cased text with characters failing `keep` removed, mapped back to the original
/// character positions, so a trigger / label found in the squashed form locates a span of
/// the original text.
struct Squash {
    orig: Vec<char>,
    keys: Vec<char>,
    map: Vec<usize>,
}

impl Squash {
    fn new(text: &str, keep: fn(char) -> bool) -> Squash {
        let orig: Vec<char> = text.chars().collect();
        let mut keys = Vec::new();
        let mut map = Vec::new();
        for (i, c) in orig.iter().enumerate() {
            if keep(*c) {
                for l in c.to_lowercase() {
                    keys.push(l);
                    map.push(i);
                }
            }
        }
        Squash { orig, keys, map }
    }

    fn spaceless(text: &str) -> Squash {
        Squash::new(text, |c| !c.is_whitespace())
    }

    fn alnum(text: &str) -> Squash {
        Squash::new(text, |c| c.is_alphanumeric())
    }

    fn key(&self, needle: &str) -> Vec<char> {
        needle.chars().filter(|c| !c.is_whitespace()).flat_map(|c| c.to_lowercase()).collect()
    }

    /// First occurrence of `needle` (squashed the same way) as a span of original characters.
    fn find(&self, needle: &str) -> Option<(usize, usize)> {
        let n = self.key(needle);
        if n.is_empty() || n.len() > self.keys.len() {
            return None;
        }
        (0..=self.keys.len() - n.len())
            .find(|&i| self.keys[i..i + n.len()] == n[..])
            .map(|i| (self.map[i], self.map[i + n.len() - 1] + 1))
    }

    fn slice(&self, start: usize, end: usize) -> String {
        self.orig[start.min(self.orig.len())..end.min(self.orig.len())].iter().collect()
    }

    fn len(&self) -> usize {
        self.orig.len()
    }
}

fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Operator detection
// ---------------------------------------------------------------------------

const NEQ: &[&str] = &[
    "달라야",
    "다르게",
    "달라",
    "서로다른",
    "같으면안",
    "같지않",
    "일치하면안",
    "일치하지않",
    "동일하면안",
    "동일하지않",
    "불일치",
    "must differ",
    "must not equal",
    "must not match",
    "must not be equal",
    "not equal",
    "different from",
    "differ from",
    "must not be the same",
    "cannot be the same",
];
const LTE: &[&str] = &[
    "작거나같",
    "이하",
    "크면안",
    "넘으면안",
    "넘어서는안",
    "초과하면안",
    "초과하지않",
    "넘지않",
    "최대",
    "at most",
    "no more than",
    "not more than",
    "less than or equal",
    "must not exceed",
    "cannot exceed",
    "may not exceed",
    "should not exceed",
    "or less",
    "maximum",
];
const GTE: &[&str] = &[
    "크거나같",
    "이상",
    "작으면안",
    "작아서는안",
    "미만이면안",
    "적어도",
    "최소",
    "커야",
    "많아야",
    "at least",
    "no less than",
    "not less than",
    "greater than or equal",
    "must not be less",
    "or more",
    "minimum",
    "greater than",
    "more than",
];
const MATCHES: &[&str] = &[
    "정규식",
    "패턴",
    "형식이어야",
    "형식에맞아야",
    "형식과같아야",
    "형식을따라야",
    "자리",
    "regex",
    "pattern",
    "digits",
    "format",
];
const EQ: &[&str] = &[
    "같아야",
    "일치해야",
    "일치할것",
    "일치",
    "동일해야",
    "동일할것",
    "동일",
    "같을것",
    "같게",
    "맞아야",
    "같아",
    "must equal",
    "must be equal",
    "should equal",
    "should be equal",
    "equals",
    "equal to",
    "must match",
    "should match",
    "must be the same",
    "same as",
    "identical",
    "match",
    "equal",
];
const PRESENT: &[&str] = &[
    "필수",
    "있어야",
    "비어있으면안",
    "비어있지않아야",
    "누락되면안",
    "누락불가",
    "존재해야",
    "기재되어야",
    "기재돼야",
    "입력되어야",
    "입력돼야",
    "빠지면안",
    "채워져야",
    "서명되어",
    "서명돼",
    "서명해야",
    "날인되어",
    "날인돼",
    "날인해야",
    "required",
    "must be present",
    "must exist",
    "must not be empty",
    "is required",
    "mandatory",
    "must be filled",
    "non-empty",
    "must be provided",
    "must have",
    "must include",
    "be signed",
    "must be stamped",
];

const OPERATORS: &[(RuleOp, &[&str])] = &[
    (RuleOp::Neq, NEQ),
    (RuleOp::Lte, LTE),
    (RuleOp::Gte, GTE),
    (RuleOp::Matches, MATCHES),
    (RuleOp::Eq, EQ),
    (RuleOp::Present, PRESENT),
];

/// Comparison symbols (longest first at equal positions).
const SYMBOLS: &[(&str, RuleOp)] = &[
    ("≠", RuleOp::Neq),
    ("!=", RuleOp::Neq),
    ("<>", RuleOp::Neq),
    (">=", RuleOp::Gte),
    ("≥", RuleOp::Gte),
    ("=>", RuleOp::Gte),
    ("<=", RuleOp::Lte),
    ("≤", RuleOp::Lte),
    ("=<", RuleOp::Lte),
    ("==", RuleOp::Eq),
    ("=", RuleOp::Eq),
    (">", RuleOp::Gte),
    ("<", RuleOp::Lte),
];

struct Trigger {
    op: RuleOp,
    /// Original-character span of the trigger.
    start: usize,
    end: usize,
    symbol: bool,
    text: String,
}

fn detect_symbol(text: &str) -> Option<Trigger> {
    let chars: Vec<char> = text.chars().collect();
    let mut best: Option<(usize, usize, RuleOp, &str)> = None;
    for (sym, op) in SYMBOLS {
        let s: Vec<char> = sym.chars().collect();
        if s.len() > chars.len() {
            continue;
        }
        if let Some(i) = (0..=chars.len() - s.len()).find(|&i| chars[i..i + s.len()] == s[..]) {
            // Skip symbols inside a quoted literal / regex.
            if best.is_none_or(|(bi, bl, _, _)| i < bi || (i == bi && s.len() > bl)) {
                best = Some((i, s.len(), *op, sym));
            }
        }
    }
    best.map(|(i, l, op, sym)| Trigger { op, start: i, end: i + l, symbol: true, text: sym.to_string() })
}

fn detect_phrase(sq: &Squash) -> Option<Trigger> {
    for (op, words) in OPERATORS {
        let mut hit: Option<(usize, usize, &str)> = None;
        for w in words.iter() {
            if let Some((s, e)) = sq.find(w) {
                if hit.is_none_or(|(hs, _, _)| s < hs) {
                    hit = Some((s, e, w));
                }
            }
        }
        if let Some((s, e, w)) = hit {
            return Some(Trigger { op: *op, start: s, end: e, symbol: false, text: w.to_string() });
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tolerance / literals
// ---------------------------------------------------------------------------

/// `(percent tolerance, span)` — "5% 이내", "within 5 %", "오차 2.5퍼센트".
fn percent(text: &str) -> Option<(f64, (usize, usize))> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let r = re(&RE, r"(\d+(?:\.\d+)?)\s*(%|％|퍼센트|percent|pct)");
    let m = r.captures(text)?;
    let whole = m.get(0)?;
    let v: f64 = m[1].parse().ok()?;
    Some((v, (text[..whole.start()].chars().count(), text[..whole.end()].chars().count())))
}

/// Absolute tolerance with a keyword: "오차 1,000원 이내", "within 1000 won", "차이 500 이하".
fn keyword_tolerance(text: &str) -> Option<(f64, (usize, usize))> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let r = re(
        &RE,
        r"(?i)(오차|차이|편차|허용\s*오차|tolerance|margin|deviation|within)\s*(?:of\s*|는\s*|은\s*|가\s*|이\s*)?([\d,]+(?:\.\d+)?)\s*(원|won|krw)?\s*(이내|이하|까지|or less)?",
    );
    let m = r.captures(text)?;
    let whole = m.get(0)?;
    let v = normalize_amount(&m[2])?;
    Some((v, (text[..whole.start()].chars().count(), text[..whole.end()].chars().count())))
}

/// "1,000원 이내로 같아야": a currency amount followed by 이내 / 이하 is a tolerance only next to
/// an equality phrase — otherwise ("합계는 10,000원 이하여야") it is the compared literal.
fn currency_tolerance(text: &str) -> Option<(f64, (usize, usize))> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let sq = Squash::spaceless(text);
    if !EQ.iter().any(|w| sq.find(w).is_some()) {
        return None;
    }
    let r = re(&RE, r"(?i)([\d,]+(?:\.\d+)?)\s*(원|won|krw)\s*(이내|이하|까지|or less)");
    let m = r.captures(text)?;
    let whole = m.get(0)?;
    let v = normalize_amount(&m[1])?;
    Some((v, (text[..whole.start()].chars().count(), text[..whole.end()].chars().count())))
}

fn absolute_tolerance(text: &str) -> Option<(f64, (usize, usize))> {
    keyword_tolerance(text).or_else(|| currency_tolerance(text))
}

fn quoted(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let r = re(&RE, "[\"'“”‘’`]([^\"'“”‘’`]+)[\"'“”‘’`]");
    r.captures(text).map(|c| c[1].trim().to_string()).filter(|s| !s.is_empty())
}

/// Regex literal for `matches`: quoted / slashed pattern, "숫자 10자리", "10 digits".
fn regex_literal(text: &str) -> Option<String> {
    static DIGITS: OnceLock<Regex> = OnceLock::new();
    static SLASH: OnceLock<Regex> = OnceLock::new();
    if let Some(q) = quoted(text) {
        return Some(q);
    }
    if let Some(c) = re(&SLASH, r"/([^/]+)/").captures(text) {
        return Some(c[1].to_string());
    }
    let d = re(&DIGITS, r"(?i)(\d+)\s*(자리|digits?|characters?|chars?)");
    if let Some(c) = d.captures(text) {
        let n = &c[1];
        let numeric = text.contains("숫자") || text.to_lowercase().contains("digit") || text.contains("번호");
        return Some(if numeric { format!("^[0-9]{{{n}}}$") } else { format!("^.{{{n}}}$") });
    }
    let t = text.trim();
    if t.starts_with('^') || t.ends_with('$') || t.contains("\\d") || t.contains("[0-9]") {
        return Some(t.to_string());
    }
    None
}

/// A value rather than a field reference: quoted text, number, date, yes / no.
fn value_literal(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(q) = quoted(t) {
        return Some(q);
    }
    let key = compact(t);
    if ["true", "yes", "예", "참", "o", "체크"].contains(&key.as_str()) {
        return Some("true".into());
    }
    if ["false", "no", "아니오", "아니요", "거짓", "x"].contains(&key.as_str()) {
        return Some("false".into());
    }
    if t.chars().any(|c| c.is_ascii_digit()) {
        if let Some(d) = normalize_date(t) {
            if key.len() >= 6 {
                return Some(d);
            }
        }
        let numeric = t
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ',' | '.' | '-' | ' ' | '원' | '₩' | '만' | '억' | '천' | '%'));
        if numeric {
            if let Some(v) = normalize_amount(t) {
                return Some(fmt_number(v));
            }
        }
    }
    None
}

fn fmt_number(v: f64) -> String {
    if v.fract().abs() < 1e-9 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

// ---------------------------------------------------------------------------
// Operand splitting
// ---------------------------------------------------------------------------

const SUBJECT: &[&str] = &["은", "는", "이", "가"];
const COMPARE: &[&str] = &["이랑", "하고", "보다", "와는", "과는", "랑", "와", "과"];
const TRAIL: &[&str] = &[
    "으로", "에서", "이랑", "하고", "보다", "와는", "과는", "은", "는", "이", "가", "을", "를", "의", "와", "과", "랑",
    "에", "로", "도", "만",
];
const KO_NOISE: &[&str] = &[
    "값",
    "항목",
    "필드",
    "내용",
    "정보",
    "서로",
    "각각",
    "두",
    "모두",
    "그리고",
    "및",
    "이내",
    "이내로",
    "내로",
    "내에서",
    "범위",
    "범위에서",
    "오차",
    "허용",
    "허용오차",
    "반드시",
    "항상",
    "꼭",
];
const EN_NOISE: &[&str] = &[
    "the", "a", "an", "of", "on", "in", "to", "that", "with", "its", "value", "field", "must", "should", "be", "is",
    "are", "and", "then", "please", "always", "same", "as", "one", "from", "this", "each", "between", "both", "for",
    "entry", "number", "no.",
];

/// The word without its trailing particle (`사업자번호는` → `사업자번호`).
fn strip_particle<'a>(word: &'a str, suffixes: &[&str]) -> &'a str {
    for s in suffixes {
        if let Some(rest) = word.strip_suffix(s) {
            if !rest.is_empty() {
                return rest;
            }
        }
    }
    word
}

fn is_marker_word(word: &str, markers: &[&str]) -> bool {
    let w = word.trim_end_matches([',', '.', ':', ';']);
    w.chars().count() >= 2 && markers.iter().any(|m| w.ends_with(m) && w.chars().count() > m.chars().count())
}

/// Removes trailing particles, punctuation and filler words from a Korean operand.
fn clean_ko(text: &str) -> String {
    let particle_word = |w: &str| w.chars().count() == 1 && TRAIL.contains(&w);
    let mut words: Vec<String> = text
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| c.is_ascii_punctuation() || matches!(c, '：' | '·' | '，')).to_string())
        .filter(|w| !w.is_empty() && !KO_NOISE.contains(&w.as_str()) && !particle_word(w))
        .collect();
    if let Some(last) = words.last_mut() {
        *last = strip_particle(last, TRAIL).to_string();
    }
    words.retain(|w| !w.is_empty() && !KO_NOISE.contains(&w.as_str()));
    tidy(&words.join(" "))
}

/// Removes articles / prepositions / verbs from an English operand.
fn clean_en(text: &str) -> String {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| c.is_ascii_punctuation() && c != '.' && c != '_' && c != '-').to_string())
        .map(|w| w.strip_suffix("'s").map(str::to_string).unwrap_or(w))
        .filter(|w| !w.is_empty() && !EN_NOISE.contains(&w.to_lowercase().as_str()))
        .collect();
    tidy(&words.join(" "))
}

fn clean(text: &str, ko: bool) -> String {
    if ko {
        clean_ko(text)
    } else {
        clean_en(text)
    }
}

/// Splits the Korean operand text in front of the trigger into `(left, right)`:
/// "A는 B와 같아야" (subject then comparison particle) or "A와 B는 같아야" / "A와 B의 차이"
/// (comparison particle then subject). `None` right when only one operand is named.
fn split_korean(operand_text: &str) -> (String, Option<String>) {
    let words: Vec<&str> = operand_text.split_whitespace().collect();
    if words.is_empty() {
        return (String::new(), None);
    }
    let s_idx = words.iter().position(|w| is_marker_word(w, SUBJECT));
    let p_idx = words.iter().position(|w| is_marker_word(w, COMPARE));
    let join = |ws: &[&str]| ws.join(" ");
    match (s_idx, p_idx) {
        (_, Some(p)) if s_idx.is_none_or(|s| p < s) && p + 1 < words.len() => {
            // "A와 B(는) …" — the right operand ends at the subject marker; "의 차이" tails go.
            let left = join(&words[..=p]);
            let end = s_idx.map(|s| s + 1).unwrap_or(words.len());
            let mut right: Vec<&str> = words[p + 1..end].to_vec();
            while right.last().is_some_and(|w| {
                let w = w.trim_end_matches([',', '.']);
                matches!(
                    w,
                    "차이"
                        | "차이는"
                        | "차이가"
                        | "차이의"
                        | "차액"
                        | "차액은"
                        | "차액이"
                        | "비율"
                        | "비율은"
                        | "비율이"
                )
            }) {
                right.pop();
            }
            if right.last().is_some_and(|w| *w == "의") {
                right.pop();
            }
            let right = join(&right);
            (left, (!right.is_empty()).then_some(right))
        }
        (Some(s), _) => {
            let left = join(&words[..=s]);
            let right = join(&words[s + 1..]);
            (left, (!right.is_empty()).then_some(right))
        }
        _ => (operand_text.to_string(), None),
    }
}

/// English: the operator phrase sits between the operands; "A and B must …" when nothing
/// follows the trigger.
fn split_english(before: &str, after: &str) -> (String, Option<String>) {
    let after = after.trim().trim_start_matches(|c: char| c == ':' || c == ',' || c.is_whitespace());
    let after_clean = clean_en(after);
    if !after_clean.is_empty() {
        return (before.to_string(), Some(after.to_string()));
    }
    let lower = before.to_lowercase();
    if let Some(pos) = lower.find(" and ") {
        let left = before[..pos].to_string();
        let right = before[pos + 5..].to_string();
        return (left, Some(right));
    }
    (before.to_string(), None)
}

// ---------------------------------------------------------------------------
// Entity resolution
// ---------------------------------------------------------------------------

/// Extra names a document type is called by (besides its registered labels).
fn type_aliases(slug: &str) -> &'static [&'static str] {
    match slug {
        "rfp" => &["제안요청서", "제안 요청서", "rfp", "request for proposal"],
        "proposal" => &["제안서", "proposal"],
        "sow" => &["과업지시서", "과업 지시서", "과업내용서", "sow", "statement of work"],
        "srs" => &["요구사항정의서", "요구사항 정의서", "요구사항명세서", "요구사항", "srs", "requirements"],
        "dev_contract" => &["개발계약서", "개발 계약서", "용역계약서", "계약서", "계약", "contract", "agreement"],
        "wbs" => &["wbs", "일정표", "작업분류체계", "schedule"],
        "progress_report" => &[
            "진척보고서",
            "진척 보고서",
            "주간보고서",
            "주간 보고",
            "월간보고서",
            "월간 보고",
            "진도보고",
            "progress report",
            "weekly report",
        ],
        "meeting_minutes" => &["회의록", "회의 록", "minutes", "meeting minutes"],
        "test_report" => {
            &["테스트결과서", "테스트 결과서", "시험결과서", "테스트 보고서", "test report", "test results"]
        }
        "change_request" => &["변경요청서", "변경 요청서", "변경요청", "change request", "cr"],
        "acceptance" => {
            &["검수확인서", "검수 확인서", "검수조서", "완료보고서", "검수", "acceptance", "acceptance certificate"]
        }
        "estimate" => &["견적서", "견적", "estimate", "quotation", "quote"],
        "tax_invoice" => &["세금계산서", "세금 계산서", "tax invoice", "invoice"],
        "invoice_exempt" => &["계산서", "면세계산서", "exempt invoice"],
        "business_registration" => {
            &["사업자등록증", "사업자 등록증", "등록증", "business registration", "registration certificate"]
        }
        "contract" => &["계약서", "계약", "contract", "agreement"],
        "application_form" => &["신청서", "신청", "application", "application form"],
        "quotation" => &["견적서", "견적", "quotation", "quote", "estimate"],
        "transaction_statement" => &["거래명세서", "거래명세표", "명세서", "transaction statement", "statement"],
        "receipt" => &["영수증", "receipt"],
        "financial_statement" => &["재무제표", "financial statement", "financials"],
        "purchase_order" => &["발주서", "purchase order"],
        _ => &[],
    }
}

/// Longest type name mentioned in `text` (compact containment), with its position.
fn find_type(ctx: &RuleContext, text: &str) -> Option<(usize, usize, usize)> {
    let sq = Squash::alnum(text);
    let mut best: Option<(usize, usize, usize, usize)> = None; // (type_idx, start, end, key_len)
    for (i, t) in ctx.types.iter().enumerate() {
        let mut names: Vec<&str> = vec![t.label_ko.as_str(), t.label_en.as_str(), t.slug.as_str()];
        names.extend(type_aliases(&t.slug));
        for n in names {
            let key = compact(n);
            if key.chars().count() < 2 {
                continue;
            }
            if let Some((s, e)) = sq.find(&key) {
                let len = key.chars().count();
                // Longer names win; at equal length an in-scope type wins.
                let better =
                    best.is_none_or(|(bi, _, _, bl)| len > bl || (len == bl && t.in_scope && !ctx.types[bi].in_scope));
                if better {
                    best = Some((i, s, e, len));
                }
            }
        }
    }
    best.map(|(i, s, e, _)| (i, s, e))
}

/// `text` without the (longest) document-type mention, for kind-based field synonyms.
fn without_type(ctx: &RuleContext, text: &str) -> String {
    match find_type(ctx, text) {
        Some((_, s, e)) => tidy(&cut(text, (s, e))),
        None => text.to_string(),
    }
}

/// Field of `t` whose label / key is mentioned in `text`: the longest full label (either
/// language, the path, a label without its parenthetical) wins; only when nothing matches, a
/// label stem without a generic tail (`공급가액 합계` → `공급가액`) or a table column's own label
/// is tried — never a stem that is itself a document-type word (`계약`, `등록`).
fn find_field_by_label(t: &TypeInfo, text: &str) -> Option<(usize, usize)> {
    const STEM_STOP: &[&str] =
        &["계약", "신청", "견적", "등록", "거래", "세금", "사업", "문서", "서식", "품목", "당사자"];
    let sq = Squash::alnum(text);
    let pick = |candidates: &[(usize, String)]| -> Option<(usize, usize)> {
        let mut best: Option<(usize, usize, usize)> = None; // (field_idx, key_len, start)
        for (i, label) in candidates {
            let key = compact(label);
            if key.chars().count() < 2 {
                continue;
            }
            if let Some((s, _)) = sq.find(&key) {
                let len = key.chars().count();
                let f = &t.fields[*i];
                let better = best.is_none_or(|(bi, bl, bs)| {
                    let b = &t.fields[bi];
                    len > bl || (len == bl && b.column && !f.column) || (len == bl && s < bs && b.column == f.column)
                });
                if better {
                    best = Some((*i, len, s));
                }
            }
        }
        best.map(|(i, _, s)| (i, s))
    };
    let mut full: Vec<(usize, String)> = Vec::new();
    let mut stems: Vec<(usize, String)> = Vec::new();
    for (i, f) in t.fields.iter().enumerate() {
        full.push((i, f.label_ko.clone()));
        full.push((i, f.label_en.clone()));
        full.push((i, f.path.clone()));
        for label in [&f.label_ko, &f.label_en] {
            if let Some(pos) = label.find(['(', '（']) {
                full.push((i, label[..pos].trim().to_string()));
            }
        }
        if f.column {
            // A column's own label ("공급가액" of 품목) only when no scalar label matches.
            if let Some(col) = f.label_ko.rsplit(' ').next() {
                stems.push((i, col.to_string()));
            }
        }
        let stem = ["합계", "금액", "번호", "일자", "총액"]
            .iter()
            .find_map(|g| f.label_ko.trim_end().strip_suffix(g))
            .map(str::trim)
            .filter(|s| s.chars().count() >= 2 && !STEM_STOP.contains(s))
            .map(str::to_string);
        if let Some(stem) = stem {
            stems.push((i, stem));
        }
    }
    pick(&full).or_else(|| pick(&stems))
}

fn has_any(key: &str, words: &[&str]) -> bool {
    words.iter().any(|w| key.contains(&compact(w)))
}

/// Which party a BRN / name refers to ("공급받는자", "buyer" …).
fn party_prefix(key: &str) -> Option<&'static str> {
    if has_any(key, &["공급받는자", "구매자", "매입자", "buyer", "customer", "purchaser", "client"]) {
        Some("buyer")
    } else if has_any(key, &["공급자", "판매자", "공급업체", "supplier", "seller", "vendor"]) {
        Some("supplier")
    } else {
        None
    }
}

fn pick_by_keys(t: &TypeInfo, keys: &[&str], kind: Option<FieldKind>) -> Option<usize> {
    for k in keys {
        if let Some(i) = t.fields.iter().position(|f| &f.path == k || f.path.rsplit('.').next() == Some(k)) {
            return Some(i);
        }
    }
    kind.and_then(|k| t.fields.iter().position(|f| f.kind == k))
}

fn pick_party(t: &TypeInfo, key: &str, kind: FieldKind, fallback: &[&str]) -> Option<usize> {
    let party = party_prefix(key);
    let candidates: Vec<usize> = t.fields.iter().enumerate().filter(|(_, f)| f.kind == kind).map(|(i, _)| i).collect();
    if let Some(p) = party {
        if let Some(i) = candidates.iter().copied().find(|i| t.fields[*i].path.starts_with(&format!("{p}."))) {
            return Some(i);
        }
    }
    if let Some(i) = pick_by_keys(t, fallback, None) {
        return Some(i);
    }
    candidates.iter().copied().find(|i| !t.fields[*i].column).or_else(|| candidates.first().copied())
}

/// Kind-based synonyms when no label matched: 사업자번호 → BRN field, 합계 → total, 서명 → signed …
fn fallback_field(t: &TypeInfo, text: &str) -> Option<usize> {
    let key = compact(text);
    if has_any(&key, &["전화", "휴대", "연락처", "phone", "tel", "mobile", "fax", "팩스"]) {
        return pick_by_keys(t, &["phone", "mobile", "fax", "applicant.phone"], Some(FieldKind::Phone));
    }
    if has_any(&key, &["계좌", "account"]) {
        return pick_by_keys(t, &["account_no"], Some(FieldKind::Account));
    }
    if has_any(&key, &["이메일", "email", "메일"]) {
        return pick_by_keys(t, &["email", "e_invoice_email", "email1"], Some(FieldKind::Email));
    }
    if has_any(&key, &["승인번호", "approval"]) {
        return pick_by_keys(t, &["nts_approval_no", "approval_no", "cash_receipt_no"], None);
    }
    if has_any(&key, &["일련번호", "serial"]) {
        return pick_by_keys(t, &["serial_no"], None);
    }
    if has_any(&key, &["계약번호", "contractno", "contractnumber"]) {
        return pick_by_keys(t, &["contract_no", "contract_number"], None);
    }
    if has_any(&key, &["문서번호", "documentno", "documentnumber"]) {
        return pick_by_keys(t, &["document_no", "doc_no"], None);
    }
    if has_any(
        &key,
        &[
            "사업자등록번호",
            "사업자번호",
            "등록번호",
            "brn",
            "businessnumber",
            "businessreg",
            "registrationno",
            "registrationnumber",
            "regno",
        ],
    ) {
        return pick_party(t, &key, FieldKind::Brn, &["registration_no", "brn", "supplier.brn"]);
    }
    if has_any(&key, &["서명", "날인", "사인", "signed", "signature", "seal", "stamp"]) {
        return t.fields.iter().position(|f| f.path == "signed" || f.path.contains("sign") || f.path.contains("seal"));
    }
    if has_any(&key, &["공급가액", "supply"]) {
        return pick_by_keys(t, &["supply_total", "supply_amount"], None);
    }
    if has_any(&key, &["세액", "부가세", "vat", "tax"]) {
        return pick_by_keys(t, &["tax_total", "vat", "tax_amount"], None);
    }
    if has_any(&key, &["합계", "총액", "총금액", "total", "grandtotal", "sum"]) {
        return pick_by_keys(
            t,
            &["grand_total", "total", "total_amount", "amount", "contract_amount"],
            Some(FieldKind::Amount),
        );
    }
    if has_any(&key, &["대표자", "대표", "representative", "ceo"]) {
        return pick_by_keys(t, &["representative"], None);
    }
    if has_any(&key, &["상호", "회사명", "법인명", "업체명", "명칭", "이름", "성명", "name", "company", "vendor"])
    {
        return pick_party(
            t,
            &key,
            FieldKind::String,
            &["company_name", "store_name", "supplier.name", "name", "applicant.name", "title"],
        );
    }
    if has_any(&key, &["주소", "소재지", "address"]) {
        return pick_by_keys(t, &["business_address", "address", "supplier.address"], None);
    }
    if has_any(&key, &["날짜", "일자", "작성일", "발행일", "발급일", "계약일", "신청일", "일시", "date"])
    {
        return pick_by_keys(
            t,
            &["issue_date", "contract_date", "application_date", "datetime", "date", "doc_date"],
            Some(FieldKind::Date),
        );
    }
    if has_any(&key, &["금액", "amount", "price", "가격"]) {
        return pick_by_keys(t, &["amount", "grand_total", "total", "contract_amount"], Some(FieldKind::Amount));
    }
    if has_any(&key, &["번호", "number"]) {
        return pick_party(
            t,
            &key,
            FieldKind::Brn,
            &["registration_no", "brn", "supplier.brn", "contract_no", "serial_no"],
        );
    }
    None
}

/// Resolution of one side of a rule.
#[derive(Debug, Default)]
struct Side {
    operand: Option<RuleOperand>,
    type_idx: Option<usize>,
    /// The type was named in the text (not inferred from a field / the other side).
    explicit_type: bool,
    /// The field came from kind-based synonyms rather than its label.
    generic: bool,
    literal: bool,
}

/// Resolves an operand text: literal (right side only), then document type + field.
fn resolve_side(ctx: &RuleContext, raw: &str, ko: bool, op: RuleOp, is_right: bool, hint: Option<usize>) -> Side {
    let text = clean(raw, ko);
    let mut side = Side::default();
    if text.is_empty() {
        return side;
    }
    if is_right {
        if op == RuleOp::Matches {
            if let Some(r) = regex_literal(raw) {
                side.operand = Some(RuleOperand::literal(&r));
                side.literal = true;
                return side;
            }
        }
        if let Some(v) = value_literal(&text).or_else(|| value_literal(raw)) {
            side.operand = Some(RuleOperand::literal(&v));
            side.literal = true;
            return side;
        }
    }
    let found = find_type(ctx, &text);
    let (type_idx, explicit) = match found {
        Some((i, _, _)) => (Some(i), true),
        None => (None, false),
    };
    let rest = without_type(ctx, &text);
    let resolve_in = |i: usize| -> Option<(usize, bool)> {
        let t = &ctx.types[i];
        if let Some((f, _)) = find_field_by_label(t, &text) {
            return Some((f, false));
        }
        fallback_field(t, &rest).map(|f| (f, true))
    };
    if let Some(i) = type_idx {
        side.type_idx = Some(i);
        side.explicit_type = explicit;
        if let Some((f, generic)) = resolve_in(i) {
            side.operand = Some(RuleOperand::field(&ctx.types[i].slug, &ctx.types[i].fields[f].path));
            side.generic = generic;
        }
        return side;
    }
    // No type named: the other side's type first (single-document rules), then a unique
    // label match across the known types (scope first).
    if let Some(h) = hint {
        if let Some((f, _)) = find_field_by_label(&ctx.types[h], &text) {
            side.type_idx = Some(h);
            side.operand = Some(RuleOperand::field(&ctx.types[h].slug, &ctx.types[h].fields[f].path));
            return side;
        }
    }
    let scope = ctx.scope();
    let index = |t: &TypeInfo| ctx.types.iter().position(|x| x.slug == t.slug).unwrap_or(0);
    let hits: Vec<(usize, usize)> =
        scope.iter().filter_map(|t| find_field_by_label(t, &text).map(|(f, _)| (index(t), f))).collect();
    match hits.len() {
        1 => {
            let (i, f) = hits[0];
            side.type_idx = Some(i);
            side.operand = Some(RuleOperand::field(&ctx.types[i].slug, &ctx.types[i].fields[f].path));
            return side;
        }
        // Ambiguous across types: the type stays undecided (the grid / model picks).
        n if n > 1 => return side,
        _ => {}
    }
    if let Some(h) = hint {
        if let Some(f) = fallback_field(&ctx.types[h], &text) {
            side.type_idx = Some(h);
            side.operand = Some(RuleOperand::field(&ctx.types[h].slug, &ctx.types[h].fields[f].path));
            side.generic = true;
            return side;
        }
    }
    let generic: Vec<(usize, usize)> =
        scope.iter().filter_map(|t| fallback_field(t, &rest).map(|f| (index(t), f))).collect();
    if generic.len() == 1 {
        let (i, f) = generic[0];
        side.type_idx = Some(i);
        side.operand = Some(RuleOperand::field(&ctx.types[i].slug, &ctx.types[i].fields[f].path));
        side.generic = true;
    }
    side
}

// ---------------------------------------------------------------------------
// Phrase parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Parsed {
    op: Option<RuleOp>,
    tolerance: Option<f64>,
    required: bool,
    left: Option<RuleOperand>,
    right: Option<RuleOperand>,
    left_text: String,
    right_text: Option<String>,
    issues: Vec<String>,
    unresolved: Vec<String>,
    penalty: f32,
}

impl Parsed {
    fn complete(&self) -> bool {
        let Some(op) = self.op else { return false };
        self.left.is_some() && (op == RuleOp::Present || self.right.is_some()) && self.unresolved.is_empty()
    }

    fn draft(self, description: &str, source: &str) -> RuleDraft {
        let complete = self.complete();
        let op = self.op.unwrap_or(if self.right.is_some() { RuleOp::Eq } else { RuleOp::Present });
        let right = if op == RuleOp::Present { None } else { self.right };
        let rule = Rule {
            description: description.to_string(),
            left: self.left.unwrap_or_default(),
            op,
            right,
            tolerance: self.tolerance,
            required: self.required,
            ..Rule::default()
        };
        let confidence = if complete { (0.92 - self.penalty).clamp(0.5, 0.95) } else { 0.3 };
        RuleDraft { rule, confidence, source: source.into(), issues: self.issues, unresolved: self.unresolved }
    }
}

fn t<'a>(ko: bool, k: &'a str, e: &'a str) -> &'a str {
    if ko {
        k
    } else {
        e
    }
}

/// `text` without the character span (tolerance phrase).
fn cut(text: &str, span: (usize, usize)) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: String = chars[..span.0.min(chars.len())].iter().collect();
    out.push(' ');
    out.extend(chars[span.1.min(chars.len())..].iter());
    tidy(&out)
}

/// Deterministic parse of one rule sentence.
fn parse(ctx: &RuleContext, text: &str, ko: bool) -> Parsed {
    let text = tidy(&crate::documents::nfc(text));
    let mut p = Parsed { required: true, ..Parsed::default() };
    let lower = text.to_lowercase();
    if ["선택", "경고만", "권장", "optional", "warn only", "warning only", "recommended"]
        .iter()
        .any(|w| lower.contains(w))
    {
        p.required = false;
    }

    // Tolerance phrases are removed before the operands are split.
    let mut work = text.clone();
    if let Some((pct, span)) = percent(&work) {
        p.tolerance = Some(pct);
        p.op = Some(RuleOp::WithinPct);
        work = cut(&work, span);
    } else if let Some((abs, span)) = absolute_tolerance(&work) {
        p.tolerance = Some(abs);
        work = cut(&work, span);
    }

    // Operator: symbol first, then phrases.
    let sq = Squash::spaceless(&work);
    let trigger = detect_symbol(&work).or_else(|| detect_phrase(&sq));
    let (op, left_raw, right_raw) = match &trigger {
        Some(tr) => {
            let op = match (p.op, tr.op) {
                // "within 5%" already decided the comparison; "이내 / 같아야" around it are noise,
                // a strict ordering is kept as an extra note.
                (Some(RuleOp::WithinPct), other) => {
                    if matches!(other, RuleOp::Gte | RuleOp::Lte) {
                        p.issues.push(
                            t(
                                ko,
                                "허용 오차(%)만 규칙에 반영했습니다. 대소 비교는 별도 규칙으로 추가하세요.",
                                "Only the percentage tolerance was kept; add the ordering as a separate rule.",
                            )
                            .to_string(),
                        );
                    }
                    RuleOp::WithinPct
                }
                (_, other) => other,
            };
            let before = sq.slice(0, tr.start);
            let after = sq.slice(tr.end, sq.len());
            let (l, r) = if tr.symbol {
                (before, Some(after))
            } else if matches!(op, RuleOp::Present | RuleOp::Matches) {
                // One operand: the pattern of `matches` is read from the whole sentence.
                let (l, _) = if ko { split_korean(&before) } else { (before, None) };
                (l, None)
            } else if ko {
                let (l, r) = split_korean(&before);
                // "A는 B와 같아야 한다, 즉 …": words after the trigger only matter when the
                // operands were not both found before it.
                if r.is_none() && !clean_ko(&after).is_empty() {
                    (l, Some(after))
                } else {
                    (l, r)
                }
            } else {
                split_english(&before, &after)
            };
            (Some(op), l, r)
        }
        None if p.op == Some(RuleOp::WithinPct) => {
            let (l, r) = if ko { split_korean(&work) } else { split_english(&work, "") };
            (Some(RuleOp::WithinPct), l, r)
        }
        None => (None, work.clone(), None),
    };
    p.op = op;
    let Some(op) = op else {
        p.issues.push(
            t(
                ko,
                "비교 연산을 인식하지 못했습니다(같아야 / 이상 / 필수 / 5% 이내 …).",
                "No comparison was recognised (must equal / at least / required / within 5% …).",
            )
            .to_string(),
        );
        p.left_text = clean(&left_raw, ko);
        return p;
    };

    // Entity-order fallback: both operands in one phrase without particles ("계약금액 견적 합계 일치").
    let (left_raw, right_raw) = match right_raw {
        Some(r) => (left_raw, Some(r)),
        None if matches!(op, RuleOp::Eq | RuleOp::Neq | RuleOp::Gte | RuleOp::Lte | RuleOp::WithinPct) => {
            match split_by_entities(ctx, &left_raw) {
                Some((l, r)) => (l, Some(r)),
                None => (left_raw, None),
            }
        }
        None => (left_raw, None),
    };

    let left = resolve_side(ctx, &left_raw, ko, op, false, None);
    p.left_text = clean(&left_raw, ko);
    let right = right_raw.as_deref().map(|r| resolve_side(ctx, r, ko, op, true, left.type_idx));
    p.right_text = right_raw.as_deref().map(|r| clean(r, ko));

    // A side that names only a type ("세금계산서 공급자가 … 상호와 같아야") borrows the field word
    // of the other side (상호 → supplier.name).
    let (mut left, right) = mirror(ctx, left, right, &left_raw, right_raw.as_deref().unwrap_or(""), ko);

    // `present` on "…must be signed" names the field in the trigger itself.
    if op == RuleOp::Present && left.operand.is_none() {
        if let Some(i) = left.type_idx {
            let signed =
                trigger.as_ref().is_some_and(|tr| has_any(&compact(&tr.text), &["서명", "날인", "sign", "stamp"]));
            if signed {
                if let Some(f) = fallback_field(&ctx.types[i], "서명") {
                    left.operand = Some(RuleOperand::field(&ctx.types[i].slug, &ctx.types[i].fields[f].path));
                }
            }
        }
    }

    match &left.operand {
        Some(o) => {
            p.left = Some(o.clone());
            if !left.explicit_type {
                p.penalty += 0.15;
            }
            if left.generic {
                p.penalty += 0.05;
            }
        }
        None => {
            let name = if p.left_text.is_empty() { left_raw.trim().to_string() } else { p.left_text.clone() };
            if !name.is_empty() {
                p.unresolved.push(name.clone());
            }
            p.issues.push(match left.type_idx {
                Some(i) => t(
                    ko,
                    &format!("'{name}': {} 문서의 어떤 필드인지 찾지 못했습니다.", ctx.types[i].label_ko),
                    &format!("'{name}': no field of {} matched.", ctx.types[i].label_en),
                )
                .to_string(),
                None => t(
                    ko,
                    &format!("'{name}': 문서 유형과 필드를 찾지 못했습니다."),
                    &format!("'{name}': document type and field not recognised."),
                )
                .to_string(),
            });
        }
    }

    match op {
        RuleOp::Present => {}
        RuleOp::Matches => match regex_literal(&text) {
            Some(r) => p.right = Some(RuleOperand::literal(&r)),
            None => p.issues.push(
                t(
                    ko,
                    "matches 규칙에는 정규식(또는 '숫자 10자리' 같은 형식)이 필요합니다.",
                    "A matches rule needs a regular expression (or a form such as '10 digits').",
                )
                .to_string(),
            ),
        },
        _ => match right {
            Some(r) => match r.operand {
                Some(o) => {
                    p.right = Some(o);
                    if !r.literal && !r.explicit_type {
                        p.penalty += 0.1;
                    }
                    if r.generic {
                        p.penalty += 0.05;
                    }
                }
                None => {
                    let name = p.right_text.clone().unwrap_or_default();
                    if !name.is_empty() {
                        p.unresolved.push(name.clone());
                    }
                    p.issues.push(match r.type_idx {
                        Some(i) => t(
                            ko,
                            &format!("'{name}': {} 문서의 어떤 필드인지 찾지 못했습니다.", ctx.types[i].label_ko),
                            &format!("'{name}': no field of {} matched.", ctx.types[i].label_en),
                        )
                        .to_string(),
                        None => t(
                            ko,
                            &format!("'{name}': 비교 대상 문서 유형과 필드를 찾지 못했습니다."),
                            &format!("'{name}': the compared document type and field were not recognised."),
                        )
                        .to_string(),
                    });
                }
            },
            None => p.issues.push(
                t(
                    ko,
                    "비교 대상(우변)이 없습니다. '…와 같아야' 형태로 적어 주세요.",
                    "The right-hand operand is missing (… must equal …).",
                )
                .to_string(),
            ),
        },
    }
    if op == RuleOp::WithinPct && p.tolerance.is_none() {
        p.tolerance = Some(5.0);
        p.issues.push(t(ko, "허용 오차가 없어 5%로 두었습니다.", "No tolerance given; 5% assumed.").to_string());
    }
    if let Some(l) = &p.left {
        if let Some(slug) = &l.doc_type {
            if ctx.type_by_slug(slug).is_some_and(|x| !x.in_scope) && ctx.types.iter().any(|x| x.in_scope) {
                p.issues.push(
                    t(
                        ko,
                        &format!("'{}'은(는) 프로젝트 템플릿에 없는 문서 유형입니다.", ctx.type_label(slug, true)),
                        &format!(
                            "'{}' is not a document type of this project's template.",
                            ctx.type_label(slug, false)
                        ),
                    )
                    .to_string(),
                );
            }
        }
    }
    p
}

/// When one side resolved to a type only, its field is picked from the other side's field
/// label (kind words such as 상호 / 사업자번호) combined with the side's own party words.
fn mirror(
    ctx: &RuleContext,
    mut left: Side,
    right: Option<Side>,
    left_raw: &str,
    right_raw: &str,
    ko: bool,
) -> (Side, Option<Side>) {
    let other_label = |s: &Side| -> Option<String> {
        let o = s.operand.as_ref()?;
        let f = ctx.field(o.doc_type.as_deref()?, o.field.as_deref()?)?;
        Some(format!("{} {}", f.label_ko, f.label_en))
    };
    let mut right = right;
    if left.operand.is_none() {
        if let (Some(i), Some(label)) = (left.type_idx, right.as_ref().and_then(other_label)) {
            if let Some(f) =
                fallback_field(&ctx.types[i], &format!("{} {label}", without_type(ctx, &clean(left_raw, ko))))
            {
                left.operand = Some(RuleOperand::field(&ctx.types[i].slug, &ctx.types[i].fields[f].path));
                left.generic = true;
            }
        }
    }
    if let Some(r) = right.as_mut() {
        if r.operand.is_none() && !r.literal {
            if let (Some(i), Some(label)) = (r.type_idx, other_label(&left)) {
                if let Some(f) =
                    fallback_field(&ctx.types[i], &format!("{} {label}", without_type(ctx, &clean(right_raw, ko))))
                {
                    r.operand = Some(RuleOperand::field(&ctx.types[i].slug, &ctx.types[i].fields[f].path));
                    r.generic = true;
                }
            }
        }
    }
    (left, right)
}

/// Splits a phrase that names two entities without particles at the second entity.
fn split_by_entities(ctx: &RuleContext, text: &str) -> Option<(String, String)> {
    let sq = Squash::alnum(text);
    let mut mentions: Vec<(usize, usize)> = Vec::new(); // (start, end) of type mentions
    for t in &ctx.types {
        let mut names: Vec<&str> = vec![t.label_ko.as_str(), t.label_en.as_str(), t.slug.as_str()];
        names.extend(type_aliases(&t.slug));
        for n in names {
            let key = compact(n);
            if key.chars().count() >= 2 {
                if let Some(span) = sq.find(&key) {
                    if !mentions.iter().any(|(s, e)| span.0 >= *s && span.1 <= *e) {
                        mentions.retain(|(s, e)| !(*s >= span.0 && *e <= span.1));
                        mentions.push(span);
                    }
                }
            }
        }
    }
    mentions.sort();
    if mentions.len() >= 2 {
        let cut = mentions[1].0;
        let l = sq.slice(0, cut);
        let r = sq.slice(cut, sq.len());
        if !l.trim().is_empty() && !r.trim().is_empty() {
            return Some((l, r));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Model fallback
// ---------------------------------------------------------------------------

/// Grammar-safe reply schema: enum of the allowed types and field paths, operator enum,
/// tolerance number|null, required flag. Empty strings in `right` mean "literal value".
pub fn rule_schema(ctx: &RuleContext) -> Value {
    let scope = ctx.scope();
    let types: Vec<Value> = scope.iter().map(|t| Value::String(t.slug.clone())).collect();
    let fields: BTreeSet<String> = scope.iter().flat_map(|t| t.fields.iter().map(|f| f.path.clone())).collect();
    let fields: Vec<Value> = fields.into_iter().map(Value::String).collect();
    let mut right_types = types.clone();
    right_types.push(Value::String(String::new()));
    let mut right_fields = fields.clone();
    right_fields.push(Value::String(String::new()));
    let ops: Vec<Value> = RuleOp::ALL.iter().map(|o| Value::String(o.as_str().into())).collect();
    json!({
        "type": "object",
        "properties": {
            "left": {
                "type": "object",
                "properties": { "doc_type": { "enum": types }, "field": { "enum": fields } },
                "required": ["doc_type", "field"],
                "additionalProperties": false
            },
            "op": { "enum": ops },
            "right": {
                "type": "object",
                "properties": {
                    "doc_type": { "enum": right_types },
                    "field": { "enum": right_fields },
                    "value": { "type": ["string", "null"] }
                },
                "required": ["doc_type", "field", "value"],
                "additionalProperties": false
            },
            "tolerance": { "type": ["number", "null"] },
            "required": { "type": "boolean" }
        },
        "required": ["left", "op", "right", "tolerance", "required"],
        "additionalProperties": false
    })
}

fn prompt(ctx: &RuleContext, description: &str, ko: bool) -> String {
    let mut s = String::from(if ko {
        "다음 자연어 규칙을 문서 정합성 규칙 JSON으로 바꾸세요. JSON으로만 답하세요.\n\
         - left/right: 문서 유형(doc_type)과 필드(field)는 아래 목록의 값만 사용합니다.\n\
         - op: eq(같다) neq(다르다) gte(크거나 같다) lte(작거나 같다) within_pct(허용 오차 % 이내) present(값이 있어야 함) matches(정규식).\n\
         - 상수와 비교할 때는 right.doc_type과 right.field를 빈 문자열로 두고 right.value에 값(또는 정규식)을 적습니다. present는 right를 모두 비워 둡니다.\n\
         - tolerance: within_pct의 퍼센트, eq의 절대 허용 오차, 그 밖에는 null.\n\
         - required: 불일치가 완료를 막아야 하면 true, 경고만이면 false.\n\n사용 가능한 문서 유형과 필드:\n"
    } else {
        "Convert the following natural-language rule into a document consistency rule as JSON. Answer with JSON only.\n\
         - left/right: doc_type and field must be values from the list below.\n\
         - op: eq, neq, gte, lte, within_pct (percent tolerance), present (value must exist), matches (regular expression).\n\
         - For a constant, leave right.doc_type and right.field empty and put the value (or regex) in right.value. For present leave right empty.\n\
         - tolerance: the percent for within_pct, an absolute tolerance for eq, otherwise null.\n\
         - required: true when a mismatch must block completion, false for a warning.\n\nAvailable document types and fields:\n"
    });
    for t in ctx.scope() {
        s.push_str(&format!("- {} ({} / {}):", t.slug, t.label_ko, t.label_en));
        let fields: Vec<String> = t
            .fields
            .iter()
            .map(|f| format!(" {}[{} / {}, {}]", f.path, f.label_ko, f.label_en, f.kind.as_str()))
            .collect();
        s.push_str(&fields.join(","));
        s.push('\n');
    }
    s.push_str(if ko { "\n[규칙]\n" } else { "\n[Rule]\n" });
    s.push_str(&prompts::truncate(description, 1_000));
    s
}

fn operand_from(ctx: &RuleContext, v: &Value, unresolved: &mut Vec<String>, is_right: bool) -> Option<RuleOperand> {
    let doc_type = v.get("doc_type").and_then(Value::as_str).unwrap_or("").trim();
    let field = v.get("field").and_then(Value::as_str).unwrap_or("").trim();
    let value = v.get("value").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    if doc_type.is_empty() && field.is_empty() {
        return if is_right { value.map(RuleOperand::literal) } else { None };
    }
    match ctx.type_by_slug(doc_type) {
        Some(t) if t.field(field).is_some() => Some(RuleOperand::field(doc_type, field)),
        Some(t) => {
            unresolved.push(format!("{}.{}", t.slug, field));
            None
        }
        None => {
            unresolved.push(if field.is_empty() { doc_type.to_string() } else { format!("{doc_type}.{field}") });
            None
        }
    }
}

/// Asks the model for the rule JSON under the grammar and validates the reply.
fn via_model(vlm: &Vlm<'_>, ctx: &RuleContext, description: &str, ko: bool) -> Result<RuleDraft> {
    let messages =
        vec![json!({ "role": "system", "content": prompts::SYSTEM_JSON }), user_message(&prompt(ctx, description, ko))];
    let (v, _) = vlm.call_json(Stage::NlSchema, &messages, "project_rule", &rule_schema(ctx), Some(512))?;
    let mut unresolved = Vec::new();
    let mut issues = Vec::new();
    let op = v.get("op").and_then(Value::as_str).and_then(RuleOp::parse);
    let left = v.get("left").and_then(|l| operand_from(ctx, l, &mut unresolved, false));
    let right = v.get("right").and_then(|r| operand_from(ctx, r, &mut unresolved, true));
    let tolerance = v.get("tolerance").and_then(Value::as_f64);
    let required = v.get("required").and_then(Value::as_bool).unwrap_or(true);
    let Some(op) = op else {
        return Err(CoreError::Invalid("rule_op_invalid".into()));
    };
    let mut complete = left.is_some() && unresolved.is_empty();
    match op {
        RuleOp::Present => {}
        RuleOp::Matches => {
            if !right.as_ref().is_some_and(|r| r.is_literal()) {
                issues.push(
                    t(
                        ko,
                        "matches 규칙에는 정규식 값이 필요합니다.",
                        "A matches rule needs a regular expression value.",
                    )
                    .to_string(),
                );
                complete = false;
            }
        }
        _ => {
            if right.is_none() {
                issues.push(t(ko, "비교 대상(우변)이 없습니다.", "The right-hand operand is missing.").to_string());
                complete = false;
            }
        }
    }
    let tolerance = match (op, tolerance) {
        (RuleOp::WithinPct, None) => {
            issues.push(t(ko, "허용 오차가 없어 5%로 두었습니다.", "No tolerance given; 5% assumed.").to_string());
            Some(5.0)
        }
        (RuleOp::WithinPct | RuleOp::Eq, Some(v)) => Some(v),
        _ => None,
    };
    for u in &unresolved {
        issues.push(
            t(
                ko,
                &format!("'{u}': 알 수 없는 문서 유형 또는 필드입니다."),
                &format!("'{u}': unknown document type or field."),
            )
            .to_string(),
        );
    }
    let rule = Rule {
        description: description.to_string(),
        left: left.unwrap_or_default(),
        op,
        right: if op == RuleOp::Present { None } else { right },
        tolerance,
        required,
        ..Rule::default()
    };
    Ok(RuleDraft { rule, confidence: if complete { 0.75 } else { 0.4 }, source: "model".into(), issues, unresolved })
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn define(
    deps: &ProjectDeps<'_>,
    project_id: Option<&str>,
    template_id: Option<&str>,
    description: &str,
) -> Result<RuleDraft> {
    let desc = description.trim();
    if desc.is_empty() {
        return Err(CoreError::Invalid("description_empty".into()));
    }
    let ctx = deps.db.with(|c| context(c, project_id, template_id))?;
    let ko = deps.ko() || has_hangul(desc);
    let mut parsed = parse(&ctx, desc, ko);
    if parsed.complete() {
        return Ok(parsed.draft(desc, "rules"));
    }
    if let Some(vlm) = deps.vlm {
        match via_model(vlm, &ctx, desc, ko) {
            Ok(mut draft) => {
                for u in parsed.unresolved {
                    if !draft.unresolved.contains(&u)
                        && !draft.issues.iter().any(|i| i.contains(&u))
                        && draft.confidence < 0.5
                    {
                        draft.unresolved.push(u);
                    }
                }
                return Ok(draft);
            }
            Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
            Err(e) => parsed.issues.push(
                t(ko, &format!("모델 변환에 실패했습니다: {e}"), &format!("The model conversion failed: {e}"))
                    .to_string(),
            ),
        }
    } else {
        parsed.issues.push(
            t(ko, "모델이 없어 구문 규칙으로만 해석했습니다. 규칙 그리드에서 문서 유형·필드를 지정해 주세요.", "No model is loaded, so only the phrase rules were used. Pick the document type and field in the rule grid.")
                .to_string(),
        );
    }
    Ok(parsed.draft(desc, "rules"))
}
