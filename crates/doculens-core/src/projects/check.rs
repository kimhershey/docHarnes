//! Deterministic rule execution (§3.15 "판정은 코드가 수행"): the project's
//! documents and their latest extracted fields are loaded once, every enabled
//! rule resolves its operands (field of each document of the type, table
//! columns per row, literals), values are normalised by field kind (BRN digits,
//! amounts, ISO dates, booleans, names without ㈜ / spacing) and compared with
//! the rule operator. Verdicts: `pass` / `mismatch` / `undetermined` (absent,
//! unparsable, review-pending or rejected value, incomparable kinds) /
//! `missing` (no document of the referenced type). With several documents of
//! one type every pairing is evaluated and the worst verdict is reported with
//! the offending pair as evidence; rows of a table column take the best row.
//! The model is consulted only for spelling variants of names, as a yes / no
//! question, and never decides a mismatch on its own.

use chrono::Utc;
use regex::Regex;
use serde_json::json;

use crate::documents::{self, DocumentRecord};
use crate::engine::client::user_message;
use crate::error::{CoreError, Result};
use crate::pipeline::prompts::{self, Stage};
use crate::pipeline::repo as pipeline_repo;
use crate::pipeline::schema::FieldKind;
use crate::pipeline::validate::{brn_digits, format_brn, format_number, normalize_amount, normalize_date};
use crate::pipeline::vlm::Vlm;
use crate::pipeline::{FieldRecord, FieldStatus};

use super::repo;
use super::rules::RuleContext;
use super::{CheckRun, Evidence, ProjectDeps, Rule, RuleOp, RuleOperand, RuleResult, Verdict};

/// A project document with the fields of its latest extraction run.
#[derive(Debug, Clone)]
pub struct DocFields {
    pub doc: DocumentRecord,
    pub fields: Vec<FieldRecord>,
}

impl DocFields {
    pub fn name(&self) -> String {
        self.doc.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| self.doc.original_name.clone())
    }
}

/// Normalised comparison value.
#[derive(Debug, Clone, PartialEq)]
pub enum Norm {
    Number(f64),
    /// ISO `YYYY-MM-DD`.
    Date(String),
    /// Digits only (BRN, phone, account).
    Digits(String),
    Bool(bool),
    /// Lower-case, whitespace / corporate suffixes removed.
    Text(String),
}

impl Norm {
    pub fn display(&self) -> String {
        match self {
            Norm::Number(v) => format_number(*v),
            Norm::Date(d) => d.clone(),
            Norm::Digits(d) if d.len() == 10 => format_brn(d),
            Norm::Digits(d) => d.clone(),
            Norm::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            Norm::Text(t) => t.clone(),
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            Norm::Number(v) => Some(*v),
            Norm::Digits(d) => d.parse().ok(),
            Norm::Text(t) => {
                let numeric = t.chars().any(|c| c.is_ascii_digit())
                    && t.chars().all(|c| c.is_ascii_digit() || matches!(c, ',' | '.' | '-' | '원' | '₩' | '만' | '억'));
                numeric.then(|| normalize_amount(t)).flatten()
            }
            _ => None,
        }
    }

    fn date(&self) -> Option<String> {
        match self {
            Norm::Date(d) => Some(d.clone()),
            Norm::Text(t) => normalize_date(t),
            Norm::Digits(d) if d.len() == 8 => normalize_date(d),
            _ => None,
        }
    }

    fn digits(&self) -> Option<String> {
        match self {
            Norm::Digits(d) => Some(d.clone()),
            Norm::Text(t) => {
                let d: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
                (!d.is_empty()).then_some(d)
            }
            Norm::Number(v) if v.fract().abs() < 1e-9 => Some(format!("{}", *v as i64)),
            _ => None,
        }
    }

    fn boolean(&self) -> Option<bool> {
        match self {
            Norm::Bool(b) => Some(*b),
            Norm::Text(t) => parse_bool(t),
            Norm::Number(v) => Some(*v != 0.0),
            _ => None,
        }
    }
}

/// Why a value could not take part in a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gap {
    /// No field with that path in the document's latest run.
    Absent,
    /// The field exists but has no value.
    Empty,
    Unparsable,
    /// Waiting in the review queue.
    Pending,
    Rejected,
}

/// One resolved operand value (one per document, or per table row).
#[derive(Debug, Clone)]
struct Resolved {
    evidence: Evidence,
    value: Option<Norm>,
    gap: Option<Gap>,
    /// A company / person name (spelling variants may be judged by the model).
    name_like: bool,
    kind: Option<FieldKind>,
}

/// Lower-case text without whitespace, punctuation and Korean corporate markers
/// (`㈜한빛 상사` = `(주)한빛상사` = `한빛상사`).
pub fn normalize_name(s: &str) -> String {
    let mut t = documents::nfc(s).to_lowercase();
    for m in [
        "주식회사",
        "유한회사",
        "유한책임회사",
        "합자회사",
        "합명회사",
        "(주)",
        "（주）",
        "㈜",
        "(유)",
        "㈜",
        "co., ltd.",
        "co.,ltd.",
        "co., ltd",
        "co.,ltd",
        "inc.",
        "ltd.",
        "corp.",
        "llc",
        "inc",
        "ltd",
        "corp",
        "(사)",
        "(재)",
    ] {
        t = t.replace(m, "");
    }
    t.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn parse_bool(s: &str) -> Option<bool> {
    let key: String = s.trim().to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    match key.as_str() {
        "true" | "yes" | "y" | "o" | "예" | "네" | "참" | "있음" | "서명" | "날인" | "서명함" | "체크" | "✓" | "✔"
        | "v" | "1" | "signed" | "checked" | "present" => Some(true),
        "false" | "no" | "n" | "x" | "아니오" | "아니요" | "거짓" | "없음" | "미서명" | "미체크" | "0" | "unsigned"
        | "unchecked" | "absent" | "-" => Some(false),
        _ => None,
    }
}

/// Normalises a printed value by field kind.
pub fn normalize(kind: FieldKind, value: &str, normalized: Option<&str>) -> Option<Norm> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    match kind {
        FieldKind::Brn => brn_digits(normalized.unwrap_or(v))
            .or_else(|| brn_digits(v))
            .or_else(|| {
                let d: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
                (!d.is_empty()).then_some(d)
            })
            .map(Norm::Digits),
        FieldKind::Amount | FieldKind::Integer => {
            normalized.and_then(|n| n.trim().parse::<f64>().ok()).or_else(|| normalize_amount(v)).map(Norm::Number)
        }
        FieldKind::Date => normalized
            .filter(|n| n.len() == 10 && n.as_bytes()[4] == b'-')
            .map(str::to_string)
            .or_else(|| normalize_date(v))
            .map(Norm::Date),
        FieldKind::Boolean => Some(parse_bool(v).map(Norm::Bool).unwrap_or_else(|| Norm::Text(normalize_name(v)))),
        FieldKind::Phone | FieldKind::Account => {
            let d: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
            Some(if d.is_empty() { Norm::Text(normalize_name(v)) } else { Norm::Digits(d) })
        }
        _ => {
            let t = normalize_name(v);
            (!t.is_empty()).then_some(Norm::Text(t))
        }
    }
}

/// A literal operand coerced to the kind of the field it is compared with.
fn literal_norm(value: &str, kind: Option<FieldKind>) -> Option<Norm> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    match kind {
        Some(FieldKind::Brn) => normalize(FieldKind::Brn, v, None),
        Some(FieldKind::Amount | FieldKind::Integer) => normalize_amount(v).map(Norm::Number),
        Some(FieldKind::Date) => normalize_date(v).map(Norm::Date),
        Some(FieldKind::Boolean) => parse_bool(v).map(Norm::Bool).or_else(|| Some(Norm::Text(normalize_name(v)))),
        Some(FieldKind::Phone | FieldKind::Account) => normalize(FieldKind::Phone, v, None),
        _ => {
            let numeric = v.chars().any(|c| c.is_ascii_digit())
                && v.chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, ',' | '.' | '-' | ' ' | '원' | '₩' | '만' | '억'));
            if numeric {
                if let Some(n) = normalize_amount(v) {
                    return Some(Norm::Number(n));
                }
            }
            if let Some(b) = parse_bool(v) {
                return Some(Norm::Bool(b));
            }
            Some(Norm::Text(normalize_name(v)))
        }
    }
}

fn name_like(key: &str) -> bool {
    ["name", "company", "vendor", "store", "representative", "issuer", "party", "applicant", "title"]
        .iter()
        .any(|w| key.contains(w))
}

/// Keys that mean the same thing across schemas (a template written against the built-in
/// tax-invoice vocabulary still matches a user schema).
fn aliases(key: &str) -> &'static [&'static str] {
    const GROUPS: &[&[&str]] = &[
        &["grand_total", "total", "total_amount", "amount", "contract_amount", "sum_total", "price", "budget"],
        &["supply_total", "supply_amount"],
        &["tax_total", "tax_amount", "vat"],
        &["registration_no", "brn", "business_number", "business_no"],
        &["company_name", "name", "store_name", "vendor", "company"],
        &[
            "issue_date",
            "date",
            "doc_date",
            "datetime",
            "contract_date",
            "application_date",
            "meeting_date",
            "report_date",
            "test_date",
            "acceptance_date",
            "request_date",
        ],
        &["signed", "signature", "seal"],
    ];
    GROUPS.iter().find(|g| g.contains(&key)).copied().unwrap_or(&[])
}

/// Fields of a document matching a rule path: exact scalar path, every row of a table
/// column (`parties.brn` → `parties[i].brn`), then the key alone (`supplier.brn` → `brn`,
/// preferring the same group) and finally the key's aliases.
fn find_fields<'a>(fields: &'a [FieldRecord], path: &str) -> Vec<&'a FieldRecord> {
    let exact: Vec<&FieldRecord> = fields.iter().filter(|f| f.path == path && f.row.is_none()).collect();
    if !exact.is_empty() {
        return exact;
    }
    if let Some((table, col)) = path.rsplit_once('.') {
        let prefix = format!("{table}[");
        let rows: Vec<&FieldRecord> =
            fields.iter().filter(|f| f.row.is_some() && f.key == col && f.path.starts_with(&prefix)).collect();
        if !rows.is_empty() {
            return rows;
        }
    }
    let key = path.rsplit('.').next().unwrap_or(path);
    let group = path.rsplit_once('.').map(|(g, _)| format!("{g}."));
    let by_key: Vec<&FieldRecord> = fields.iter().filter(|f| f.row.is_none() && f.key == key).collect();
    if !by_key.is_empty() {
        if let Some(g) = &group {
            if let Some(f) = by_key.iter().find(|f| f.path.starts_with(g.as_str())) {
                return vec![f];
            }
        }
        return vec![by_key[0]];
    }
    for alias in aliases(key) {
        if let Some(f) = fields.iter().find(|f| f.row.is_none() && f.key == *alias) {
            return vec![f];
        }
    }
    Vec::new()
}

fn field_status(f: &FieldRecord) -> &'static str {
    if f.status == FieldStatus::Rejected {
        "rejected"
    } else if f.needs_review() {
        "pending"
    } else {
        f.status.as_str()
    }
}

struct Env<'a> {
    docs: &'a [DocFields],
    ctx: &'a RuleContext,
    labels: &'a [(String, String, String)],
    ko: bool,
    vlm: Option<&'a Vlm<'a>>,
}

impl Env<'_> {
    fn type_label(&self, slug: &str) -> String {
        repo::type_label(self.labels, slug, self.ko)
    }

    fn field_label(&self, slug: &str, path: &str) -> String {
        self.ctx.field_label(slug, path, self.ko)
    }

    fn docs_of(&self, slug: &str) -> Vec<&DocFields> {
        self.docs.iter().filter(|d| d.doc.doc_type.as_deref() == Some(slug)).collect()
    }

    /// Resolves a field operand in one document: one entry per matching field / row, or a
    /// single `Absent` entry (document-level vendor / date / total stand in for missing fields).
    fn resolve_field(&self, d: &DocFields, slug: &str, path: &str) -> Vec<Resolved> {
        let base = Evidence {
            doc_id: Some(d.doc.id.clone()),
            doc_name: Some(d.name()),
            doc_type: d.doc.doc_type.clone(),
            field: Some(path.to_string()),
            label: Some(self.field_label(slug, path)),
            ..Evidence::default()
        };
        let matches = find_fields(&d.fields, path);
        if matches.is_empty() {
            if let Some(r) = self.document_level(d, path, &base) {
                return vec![r];
            }
            return vec![Resolved {
                evidence: Evidence { status: Some("absent".into()), ..base },
                value: None,
                gap: Some(Gap::Absent),
                name_like: name_like(path),
                kind: self.ctx.field(slug, path).map(|f| f.kind),
            }];
        }
        matches
            .into_iter()
            .map(|f| {
                let label = if self.ko { &f.label_ko } else { &f.label_en };
                let mut ev = Evidence {
                    field: Some(f.path.clone()),
                    label: Some(if label.is_empty() { base.label.clone().unwrap_or_default() } else { label.clone() }),
                    value: f.effective_value().map(str::to_string),
                    page: f.page,
                    status: Some(field_status(f).into()),
                    ..base.clone()
                };
                let (value, gap) = if f.status == FieldStatus::Rejected {
                    (None, Some(Gap::Rejected))
                } else {
                    match f.effective_value().map(str::trim).filter(|v| !v.is_empty()) {
                        None => (None, Some(Gap::Empty)),
                        Some(v) => match normalize(f.kind, v, f.normalized.as_deref()) {
                            Some(n) if f.needs_review() => (Some(n), Some(Gap::Pending)),
                            Some(n) => (Some(n), None),
                            None => (None, Some(Gap::Unparsable)),
                        },
                    }
                };
                ev.normalized = value.as_ref().map(Norm::display);
                Resolved {
                    evidence: ev,
                    value,
                    gap,
                    name_like: f.kind == FieldKind::String && name_like(&f.key),
                    kind: Some(f.kind),
                }
            })
            .collect()
    }

    /// `documents.vendor / doc_date / total_amount` for total / date / name paths.
    fn document_level(&self, d: &DocFields, path: &str, base: &Evidence) -> Option<Resolved> {
        let key = path.rsplit('.').next().unwrap_or(path);
        let mk =
            |field: &str, label_ko: &str, label_en: &str, value: String, norm: Norm, name: bool, kind: FieldKind| {
                Resolved {
                    evidence: Evidence {
                        field: Some(field.into()),
                        label: Some(if self.ko { label_ko } else { label_en }.into()),
                        value: Some(value),
                        normalized: Some(norm.display()),
                        status: Some("document".into()),
                        ..base.clone()
                    },
                    value: Some(norm),
                    gap: None,
                    name_like: name,
                    kind: Some(kind),
                }
            };
        if aliases("grand_total").contains(&key) {
            let v = d.doc.total_amount?;
            return Some(mk(
                "total_amount",
                "합계(문서)",
                "Total (document)",
                format_number(v),
                Norm::Number(v),
                false,
                FieldKind::Amount,
            ));
        }
        if aliases("issue_date").contains(&key) {
            let v = d.doc.doc_date.clone()?;
            let iso = normalize_date(&v)?;
            return Some(mk("doc_date", "문서 날짜", "Document date", v, Norm::Date(iso), false, FieldKind::Date));
        }
        if aliases("company_name").contains(&key) {
            let v = d.doc.vendor.clone()?;
            let n = normalize_name(&v);
            return Some(mk("vendor", "거래처(문서)", "Vendor (document)", v, Norm::Text(n), true, FieldKind::String));
        }
        None
    }
}

fn severity(v: Verdict) -> u8 {
    match v {
        Verdict::Pass => 0,
        Verdict::Undetermined => 1,
        Verdict::Mismatch => 2,
        Verdict::Missing => 3,
    }
}

/// Outcome of one comparison.
struct Outcome {
    verdict: Verdict,
    left: Option<Evidence>,
    right: Option<Evidence>,
    message: String,
}

fn tr<'a>(ko: bool, k: &'a str, e: &'a str) -> &'a str {
    if ko {
        k
    } else {
        e
    }
}

fn gap_message(ko: bool, r: &Resolved, gap: Gap) -> String {
    let who = format!(
        "{} {}",
        r.evidence.doc_name.clone().unwrap_or_default(),
        r.evidence.label.clone().or(r.evidence.field.clone()).unwrap_or_default()
    );
    let raw = r.evidence.value.clone().unwrap_or_default();
    match gap {
        Gap::Absent | Gap::Empty => {
            tr(ko, &format!("{who}: 값이 없어 판단할 수 없습니다."), &format!("{who}: no value, cannot judge."))
                .to_string()
        }
        Gap::Unparsable => tr(
            ko,
            &format!("{who}: 값 '{raw}'을(를) 해석할 수 없습니다."),
            &format!("{who}: value '{raw}' could not be parsed."),
        )
        .to_string(),
        Gap::Pending => {
            tr(ko, &format!("{who}: 검토 대기 중인 값입니다."), &format!("{who}: value is waiting for review."))
                .to_string()
        }
        Gap::Rejected => {
            tr(ko, &format!("{who}: 검토에서 거부된 값입니다."), &format!("{who}: value was rejected in review."))
                .to_string()
        }
    }
}

fn label_value(r: &Resolved) -> String {
    let doc = r.evidence.doc_name.clone().unwrap_or_default();
    let label = r.evidence.label.clone().or(r.evidence.field.clone()).unwrap_or_default();
    let value = r.evidence.normalized.clone().or(r.evidence.value.clone()).unwrap_or_default();
    if doc.is_empty() {
        value
    } else if label.is_empty() {
        format!("{doc} {value}")
    } else {
        format!("{doc} {label} {value}")
    }
}

/// Deterministic comparison; `None` when the kinds cannot be compared.
fn compare(op: RuleOp, l: &Norm, r: &Norm, tolerance: Option<f64>) -> Option<bool> {
    let tol = tolerance.unwrap_or(0.0).abs();
    let numbers = match (l, r) {
        (Norm::Number(a), Norm::Number(b)) => Some((*a, *b)),
        (Norm::Number(a), other) => other.number().map(|b| (*a, b)),
        (other, Norm::Number(b)) => other.number().map(|a| (a, *b)),
        _ => None,
    };
    match op {
        RuleOp::Gte | RuleOp::Lte => {
            let (a, b) = match numbers {
                Some(n) => n,
                None => match (l.date(), r.date()) {
                    (Some(a), Some(b)) => {
                        let ord = a.cmp(&b);
                        return Some(if op == RuleOp::Gte {
                            ord != std::cmp::Ordering::Less
                        } else {
                            ord != std::cmp::Ordering::Greater
                        });
                    }
                    _ => return None,
                },
            };
            Some(if op == RuleOp::Gte { a >= b - 1e-9 } else { a <= b + 1e-9 })
        }
        RuleOp::WithinPct => {
            let (a, b) = numbers?;
            Some(if b.abs() < 1e-9 { a.abs() < 1e-9 } else { (a - b).abs() <= b.abs() * tol / 100.0 + 1e-9 })
        }
        RuleOp::Eq | RuleOp::Neq => {
            let equal = if let Some((a, b)) = numbers {
                (a - b).abs() <= tol + 1e-9
            } else {
                match (l, r) {
                    (Norm::Date(a), _) => r.date().map(|b| *a == b)?,
                    (_, Norm::Date(b)) => l.date().map(|a| a == *b)?,
                    (Norm::Digits(a), _) => r.digits().map(|b| *a == b)?,
                    (_, Norm::Digits(b)) => l.digits().map(|a| a == *b)?,
                    (Norm::Bool(a), _) => r.boolean().map(|b| *a == b)?,
                    (_, Norm::Bool(b)) => l.boolean().map(|a| a == *b)?,
                    (Norm::Text(a), Norm::Text(b)) => a == b,
                    _ => return None,
                }
            };
            Some(if op == RuleOp::Eq { equal } else { !equal })
        }
        RuleOp::Present | RuleOp::Matches => None,
    }
}

/// Asks the model whether two names denote the same party (yes / no JSON).
fn same_name(vlm: &Vlm<'_>, a: &str, b: &str, ko: bool) -> Result<bool> {
    let prompt = if ko {
        format!("두 명칭이 같은 회사 또는 같은 사람을 가리키는지 판단하세요. 띄어쓰기, ㈜/(주)/주식회사 표기, 영문·한글 표기 차이는 같은 것으로 봅니다. 다른 회사·다른 사람이면 false입니다. JSON {{\"same\": true|false}} 형식으로만 답하세요.\nA: {a}\nB: {b}")
    } else {
        format!("Decide whether the two names denote the same company or person. Spacing, corporate markers (Co., Ltd., ㈜) and transliteration differences count as the same; a different company or person is false. Answer only as JSON {{\"same\": true|false}}.\nA: {a}\nB: {b}")
    };
    let schema = json!({
        "type": "object",
        "properties": { "same": { "type": "boolean" } },
        "required": ["same"],
        "additionalProperties": false
    });
    let messages = vec![json!({ "role": "system", "content": prompts::SYSTEM_JSON }), user_message(&prompt)];
    let (v, _) = vlm.call_json(Stage::NlSchema, &messages, "name_match", &schema, Some(32))?;
    v.get("same")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| CoreError::engine("name_match reply has no 'same'"))
}

impl Env<'_> {
    /// Compares one left value with one right value.
    fn pair(&self, rule: &Rule, l: &Resolved, r: &Resolved) -> Result<Outcome> {
        let ko = self.ko;
        let (le, re_) = (Some(l.evidence.clone()), Some(r.evidence.clone()));
        if let Some(g) = l.gap {
            return Ok(Outcome {
                verdict: Verdict::Undetermined,
                left: le,
                right: re_,
                message: gap_message(ko, l, g),
            });
        }
        if let Some(g) = r.gap {
            return Ok(Outcome {
                verdict: Verdict::Undetermined,
                left: le,
                right: re_,
                message: gap_message(ko, r, g),
            });
        }
        let (Some(lv), Some(rv)) = (&l.value, &r.value) else {
            return Ok(Outcome {
                verdict: Verdict::Undetermined,
                left: le,
                right: re_,
                message: gap_message(ko, l, Gap::Empty),
            });
        };
        let (ls, rs) = (label_value(l), label_value(r));
        let result = compare(rule.op, lv, rv, rule.tolerance);
        let Some(mut ok) = result else {
            return Ok(Outcome {
                verdict: Verdict::Undetermined,
                left: le,
                right: re_,
                message: tr(
                    ko,
                    &format!("{ls} 와(과) {rs}: 비교할 수 없는 형식입니다."),
                    &format!("{ls} and {rs}: incomparable kinds."),
                )
                .to_string(),
            });
        };
        let mut note = String::new();
        // Names: a deterministic mismatch of two company / person names may be a spelling
        // variant — the model answers yes / no; without one the verdict stays open.
        if !ok
            && matches!(rule.op, RuleOp::Eq | RuleOp::Neq)
            && l.name_like
            && r.name_like
            && matches!((lv, rv), (Norm::Text(_), Norm::Text(_)))
        {
            let (a, b) = (l.evidence.value.clone().unwrap_or_default(), r.evidence.value.clone().unwrap_or_default());
            match self.vlm {
                Some(vlm) => match same_name(vlm, &a, &b, ko) {
                    Ok(same) => {
                        ok = if rule.op == RuleOp::Eq { same } else { !same };
                        note = tr(ko, " (모델 판정: 표기 차이)", " (model: spelling variant)").to_string();
                        if !same {
                            note = tr(ko, " (모델 판정: 다른 명칭)", " (model: different names)").to_string();
                        }
                    }
                    Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
                    Err(e) => {
                        return Ok(Outcome {
                            verdict: Verdict::Undetermined,
                            left: le,
                            right: re_,
                            message: tr(
                                ko,
                                &format!("{ls} ≠ {rs}: 표기 차이 여부를 모델이 판정하지 못했습니다({e})."),
                                &format!("{ls} ≠ {rs}: the model could not judge the spelling variant ({e})."),
                            )
                            .to_string(),
                        })
                    }
                },
                None => {
                    return Ok(Outcome {
                        verdict: Verdict::Undetermined,
                        left: le,
                        right: re_,
                        message: tr(
                            ko,
                            &format!("{ls} ≠ {rs}: 표기 차이일 수 있어 확인이 필요합니다(모델 없음)."),
                            &format!("{ls} ≠ {rs}: may be a spelling variant; needs confirmation (no model)."),
                        )
                        .to_string(),
                    })
                }
            }
        }
        let message = match (rule.op, ok) {
            (RuleOp::Eq, true) => format!("{ls} = {rs}{note}"),
            (RuleOp::Eq, false) => format!("{ls} ≠ {rs}{note}"),
            (RuleOp::Neq, true) => format!("{ls} ≠ {rs}{note}"),
            (RuleOp::Neq, false) => tr(
                ko,
                &format!("{ls} = {rs}: 두 값이 같습니다{note}"),
                &format!("{ls} = {rs}: the values are equal{note}"),
            )
            .to_string(),
            (RuleOp::Gte, true) => format!("{ls} ≥ {rs}"),
            (RuleOp::Gte, false) => format!("{ls} < {rs}"),
            (RuleOp::Lte, true) => format!("{ls} ≤ {rs}"),
            (RuleOp::Lte, false) => format!("{ls} > {rs}"),
            (RuleOp::WithinPct, _) => {
                let tol = rule.tolerance.unwrap_or(0.0);
                let pct = match (lv.number(), rv.number()) {
                    (Some(a), Some(b)) if b.abs() > 1e-9 => (a - b).abs() / b.abs() * 100.0,
                    _ => 0.0,
                };
                if ok {
                    tr(
                        ko,
                        &format!("{ls} ≈ {rs}: 차이 {pct:.1}% ≤ 허용 {tol}%"),
                        &format!("{ls} ≈ {rs}: difference {pct:.1}% ≤ {tol}% allowed"),
                    )
                    .to_string()
                } else {
                    tr(
                        ko,
                        &format!("{ls} vs {rs}: 차이 {pct:.1}% > 허용 {tol}%"),
                        &format!("{ls} vs {rs}: difference {pct:.1}% > {tol}% allowed"),
                    )
                    .to_string()
                }
            }
            _ => String::new(),
        };
        Ok(Outcome { verdict: if ok { Verdict::Pass } else { Verdict::Mismatch }, left: le, right: re_, message })
    }

    /// `present`: the value exists (booleans must be true).
    fn present(&self, l: &Resolved) -> Outcome {
        let ko = self.ko;
        let who = format!(
            "{} {}",
            l.evidence.doc_name.clone().unwrap_or_default(),
            l.evidence.label.clone().or(l.evidence.field.clone()).unwrap_or_default()
        );
        let le = Some(l.evidence.clone());
        match (l.gap, &l.value) {
            (Some(Gap::Pending), _) => Outcome {
                verdict: Verdict::Undetermined,
                left: le,
                right: None,
                message: gap_message(ko, l, Gap::Pending),
            },
            (Some(Gap::Unparsable), _) => Outcome {
                verdict: Verdict::Pass,
                left: le,
                right: None,
                message: tr(
                    ko,
                    &format!("{who}: 값이 있습니다('{}').", l.evidence.value.clone().unwrap_or_default()),
                    &format!("{who}: value present ('{}').", l.evidence.value.clone().unwrap_or_default()),
                )
                .to_string(),
            },
            (Some(_), _) | (None, None) => Outcome {
                verdict: Verdict::Mismatch,
                left: le,
                right: None,
                message: tr(ko, &format!("{who}: 값이 없습니다."), &format!("{who}: missing.")).to_string(),
            },
            (None, Some(Norm::Bool(false))) => Outcome {
                verdict: Verdict::Mismatch,
                left: le,
                right: None,
                message: tr(ko, &format!("{who}: '아니오'로 기재되어 있습니다."), &format!("{who}: recorded as 'no'."))
                    .to_string(),
            },
            (None, Some(v)) => Outcome {
                verdict: Verdict::Pass,
                left: le,
                right: None,
                message: tr(
                    ko,
                    &format!("{who}: 값이 있습니다({}).", v.display()),
                    &format!("{who}: value present ({}).", v.display()),
                )
                .to_string(),
            },
        }
    }

    /// `matches`: the raw or normalised value matches the pattern.
    fn matches(&self, l: &Resolved, pattern: &str, regex: &std::result::Result<Regex, regex::Error>) -> Outcome {
        let ko = self.ko;
        let le = Some(l.evidence.clone());
        let re_ = Some(Evidence { value: Some(pattern.to_string()), ..Evidence::default() });
        let regex = match regex {
            Ok(r) => r,
            Err(e) => {
                return Outcome {
                    verdict: Verdict::Undetermined,
                    left: le,
                    right: re_,
                    message: tr(
                        ko,
                        &format!("정규식 '{pattern}'이(가) 잘못되었습니다: {e}"),
                        &format!("invalid regular expression '{pattern}': {e}"),
                    )
                    .to_string(),
                }
            }
        };
        if let Some(g) = l.gap.filter(|g| *g != Gap::Unparsable) {
            return Outcome { verdict: Verdict::Undetermined, left: le, right: re_, message: gap_message(ko, l, g) };
        }
        let raw = l.evidence.value.clone().unwrap_or_default();
        let norm = l.value.as_ref().map(Norm::display).unwrap_or_default();
        let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
        let ok = regex.is_match(raw.trim()) || regex.is_match(&norm) || (!digits.is_empty() && regex.is_match(&digits));
        let who = label_value(l);
        Outcome {
            verdict: if ok { Verdict::Pass } else { Verdict::Mismatch },
            left: le,
            right: re_,
            message: if ok {
                tr(ko, &format!("{who}: 형식 {pattern}에 맞습니다."), &format!("{who}: matches {pattern}.")).to_string()
            } else {
                tr(ko, &format!("{who}: 형식 {pattern}에 맞지 않습니다."), &format!("{who}: does not match {pattern}."))
                    .to_string()
            },
        }
    }

    /// Best outcome over the rows of a table column (a matching row satisfies the rule),
    /// worst over document pairings.
    fn evaluate(&self, rule: &Rule) -> Result<Outcome> {
        let ko = self.ko;
        let missing = |slug: &str, field: Option<&str>| Outcome {
            verdict: Verdict::Missing,
            left: Some(Evidence {
                doc_type: Some(slug.to_string()),
                field: field.map(str::to_string),
                label: field.map(|f| self.field_label(slug, f)),
                status: Some("missing_document".into()),
                ..Evidence::default()
            }),
            right: None,
            message: tr(
                ko,
                &format!("프로젝트에 {} 문서가 없습니다.", self.type_label(slug)),
                &format!("No {} document in the project.", self.type_label(slug)),
            )
            .to_string(),
        };
        let (Some(l_type), Some(l_field)) = (rule.left.doc_type.as_deref(), rule.left.field.as_deref()) else {
            return Ok(Outcome {
                verdict: Verdict::Undetermined,
                left: None,
                right: None,
                message: tr(
                    ko,
                    "규칙의 좌변에 문서 유형과 필드가 없습니다.",
                    "The rule's left operand has no document type / field.",
                )
                .to_string(),
            });
        };
        let l_docs = self.docs_of(l_type);
        if l_docs.is_empty() {
            return Ok(missing(l_type, Some(l_field)));
        }
        // Left values grouped by document (rows of a table column stay together).
        let left: Vec<Vec<Resolved>> = l_docs.iter().map(|d| self.resolve_field(d, l_type, l_field)).collect();

        match rule.op {
            RuleOp::Present => {
                let mut worst: Option<Outcome> = None;
                for rows in &left {
                    let best =
                        rows.iter().map(|r| self.present(r)).min_by_key(|o| severity(o.verdict)).expect("one row");
                    if worst.as_ref().is_none_or(|w| severity(best.verdict) > severity(w.verdict)) {
                        worst = Some(best);
                    }
                }
                Ok(worst.expect("one document"))
            }
            RuleOp::Matches => {
                let pattern = rule.right.as_ref().and_then(|r| r.value.clone()).unwrap_or_default();
                if pattern.trim().is_empty() {
                    return Ok(Outcome {
                        verdict: Verdict::Undetermined,
                        left: None,
                        right: None,
                        message: tr(
                            ko,
                            "matches 규칙에 정규식이 없습니다.",
                            "The matches rule has no regular expression.",
                        )
                        .to_string(),
                    });
                }
                let regex = Regex::new(&pattern);
                let mut worst: Option<Outcome> = None;
                for rows in &left {
                    let best = rows
                        .iter()
                        .map(|r| self.matches(r, &pattern, &regex))
                        .min_by_key(|o| severity(o.verdict))
                        .expect("one row");
                    if worst.as_ref().is_none_or(|w| severity(best.verdict) > severity(w.verdict)) {
                        worst = Some(best);
                    }
                }
                Ok(worst.expect("one document"))
            }
            _ => {
                let Some(right_op) = &rule.right else {
                    return Ok(Outcome {
                        verdict: Verdict::Undetermined,
                        left: None,
                        right: None,
                        message: tr(ko, "규칙의 우변(비교 대상)이 없습니다.", "The rule has no right-hand operand.")
                            .to_string(),
                    });
                };
                // Right values grouped by document (a literal is one group of one).
                let right: Vec<Vec<Resolved>> = if right_op.is_literal() {
                    let kind = left.iter().flatten().find_map(|r| r.kind);
                    let raw = right_op.value.clone().unwrap_or_default();
                    let value = literal_norm(&raw, kind);
                    vec![vec![Resolved {
                        evidence: Evidence {
                            value: Some(raw.clone()),
                            normalized: value.as_ref().map(Norm::display),
                            ..Evidence::default()
                        },
                        gap: if value.is_none() { Some(Gap::Unparsable) } else { None },
                        value,
                        name_like: false,
                        kind: None,
                    }]]
                } else {
                    let (Some(r_type), Some(r_field)) = (right_op.doc_type.as_deref(), right_op.field.as_deref())
                    else {
                        return Ok(Outcome {
                            verdict: Verdict::Undetermined,
                            left: None,
                            right: None,
                            message: tr(
                                ko,
                                "규칙의 우변에 문서 유형과 필드가 없습니다.",
                                "The rule's right operand has no document type / field.",
                            )
                            .to_string(),
                        });
                    };
                    let r_docs = self.docs_of(r_type);
                    if r_docs.is_empty() {
                        return Ok(missing(r_type, Some(r_field)));
                    }
                    r_docs.iter().map(|d| self.resolve_field(d, r_type, r_field)).collect()
                };
                // Both operands of one document type (supplier ≠ buyer on a tax invoice): each
                // document is compared with itself — `left` and `right` come from the same
                // `docs_of` order, so equal indices are one document. Different types pair every
                // document of one with every document of the other.
                let same_type = !right_op.is_literal() && right_op.doc_type.as_deref() == Some(l_type);
                let mut worst: Option<Outcome> = None;
                for (li, l_rows) in left.iter().enumerate() {
                    for (ri, r_rows) in right.iter().enumerate() {
                        if same_type && li != ri {
                            continue;
                        }
                        // Best over row combinations of this document pair.
                        let mut best: Option<Outcome> = None;
                        for l in l_rows {
                            for r in r_rows {
                                let o = self.pair(rule, l, r)?;
                                if best.as_ref().is_none_or(|b| severity(o.verdict) < severity(b.verdict)) {
                                    best = Some(o);
                                }
                            }
                        }
                        if let Some(b) = best {
                            if worst.as_ref().is_none_or(|w| severity(b.verdict) > severity(w.verdict)) {
                                worst = Some(b);
                            }
                        }
                    }
                }
                Ok(worst.unwrap_or_else(|| Outcome {
                    verdict: Verdict::Undetermined,
                    left: None,
                    right: None,
                    message: tr(ko, "비교할 값이 없습니다.", "Nothing to compare.").to_string(),
                }))
            }
        }
    }
}

/// Runs the project's enabled rules (`rule_ids` empty = all), persists the run and returns it.
pub fn run(deps: &ProjectDeps<'_>, project_id: &str, rule_ids: &[String]) -> Result<CheckRun> {
    let started_at = Utc::now();
    let loaded = deps.db.with(|c| {
        let Some(project) = repo::get_project(c, project_id)? else { return Ok(None) };
        let required = match project.template_id.as_deref() {
            Some(t) => repo::get_template(c, t)?.map(|t| t.required_doc_types).unwrap_or_default(),
            None => Vec::new(),
        };
        let rules = repo::rules_of(c, project_id)?;
        let mut docs = Vec::new();
        for d in repo::documents_of(c, project_id)? {
            let fields = pipeline_repo::latest_fields(c, &d.document.id)?;
            docs.push(DocFields { doc: d.document, fields });
        }
        let ctx = super::rules::context(c, Some(project_id), None)?;
        let labels = repo::doc_type_labels(c)?;
        Ok(Some((required, rules, docs, ctx, labels)))
    })?;
    let Some((required, rules, docs, ctx, labels)) = loaded else {
        return Err(CoreError::NotFound(format!("project {project_id}")));
    };
    let env = Env { docs: &docs, ctx: &ctx, labels: &labels, ko: deps.ko(), vlm: deps.vlm };
    let run_id = documents::new_id();
    let mut run = CheckRun {
        id: run_id.clone(),
        project_id: project_id.to_string(),
        started_at,
        finished_at: started_at,
        pass: 0,
        mismatch: 0,
        undetermined: 0,
        missing: 0,
        results: Vec::new(),
        missing_doc_types: required.iter().filter(|t| env.docs_of(t).is_empty()).cloned().collect(),
    };
    for rule in rules.iter().filter(|r| r.enabled && (rule_ids.is_empty() || rule_ids.contains(&r.id))) {
        if deps.vlm.is_some_and(|v| v.cancelled()) {
            return Err(CoreError::Cancelled);
        }
        let o = env.evaluate(rule)?;
        match o.verdict {
            Verdict::Pass => run.pass += 1,
            Verdict::Mismatch => run.mismatch += 1,
            Verdict::Undetermined => run.undetermined += 1,
            Verdict::Missing => run.missing += 1,
        }
        run.results.push(RuleResult {
            id: documents::new_id(),
            run_id: run_id.clone(),
            rule_id: rule.id.clone(),
            description: rule.description.clone(),
            verdict: o.verdict,
            required: rule.required,
            left: o.left,
            right: o.right,
            message: o.message,
            checked_at: Utc::now(),
        });
    }
    run.finished_at = Utc::now();
    deps.db.with(|c| repo::insert_run(c, &run))?;
    Ok(run)
}

/// Resolves the operand field label for display (grid tooltips, reports).
pub fn operand_label(ctx: &RuleContext, op: &RuleOperand, ko: bool) -> String {
    match (&op.doc_type, &op.field, &op.value) {
        (Some(t), Some(f), _) => format!("{} · {}", ctx.type_label(t, ko), ctx.field_label(t, f, ko)),
        (_, _, Some(v)) => v.clone(),
        _ => String::new(),
    }
}
