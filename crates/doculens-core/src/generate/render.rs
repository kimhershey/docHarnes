//! Rendering and saving. One layout plan per template feeds three writers:
//! the self-contained HTML preview (A4 print CSS, Korean fonts), Markdown and
//! the `DocModel` the from-scratch DOCX / HWPX writers (`office`) consume.
//! `save` resolves the target path inside the workspace, writes `<path>.part`
//! then renames, fills a registered HWPX / DOCX form in place when the draft
//! was made for one (`fill`), prints PDF through the host's `PdfPrinter`
//! (falling back to Markdown with a warning), registers the file in the
//! library with `DocumentRole::Generated` and records the saved path on the
//! stored draft.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, Utc};

use crate::documents::convert::html::escape;
use crate::documents::convert::model::{Align, Block, Cell, DocModel, ListKind, Paragraph, Run, Table};
use crate::documents::import::{self, ImportDeps};
use crate::documents::{self, repo as docs, DocumentRole, DocumentStatus, NewDocument, SourceKind};
use crate::error::{CoreError, Result};
use crate::organize::template::{sanitize_component, sanitize_folder};
use crate::pipeline::schema::FieldKind;
use crate::pipeline::validate::normalize_amount;
use crate::settings::Language;
use crate::version::WORKSPACE_DIR_NAME;

use super::templates::{compact, TemplateSpec};
use super::validate::format_won;
use super::{
    fill, office, store, Draft, DraftBlock, DraftCitation, DraftField, DraftTable, GenerateDeps, OutputFormat,
    SaveRequest, SavedDocument, TemplateKind, TemplateRef,
};

/// Default workspace-relative folder of generated files.
pub const DEFAULT_FOLDER: &str = "documents/generated";

// ---------------------------------------------------------------------------
// Layout plan
// ---------------------------------------------------------------------------

enum Item<'a> {
    Title(String),
    /// `label: value` line (letter heads, 붙임, 기한).
    KeyValue(String, String),
    /// Label / value grid; the second element is the number of pairs per row (1 or 2).
    Grid(Vec<(String, String)>, usize),
    Heading(u8, String),
    /// Paragraph with an optional reference number (1-based, into the references item).
    Paragraph(String, Option<usize>),
    List(Vec<String>, bool),
    /// Item table with its totals rows (label, value).
    Table(&'a DraftTable, Vec<(String, String)>),
    /// Right-aligned closing lines (발신 기관, 갑/을 서명란).
    Signature(Vec<String>),
    /// `[n] title p.N` lines.
    References(Vec<String>),
}

fn tr(ko: bool, k: &str, e: &str) -> String {
    if ko {
        k.to_string()
    } else {
        e.to_string()
    }
}

fn kind_of(f: &DraftField) -> FieldKind {
    FieldKind::parse(&f.kind).unwrap_or_default()
}

/// Keys (last path segment) of amount-kind fields and columns that are plain numbers.
const NUMBER_KEYS: [&str; 14] = [
    "months",
    "ratio",
    "progress",
    "points",
    "quantity",
    "qty",
    "count",
    "rate",
    "percent",
    "percentage",
    "score",
    "hours",
    "days",
    "weight",
];
const NUMBER_KEY_ENDINGS: [&str; 11] =
    ["_percent", "_rate", "_ratio", "_count", "_months", "_points", "_qty", "_quantity", "_hours", "_days", "_score"];
/// English unit words of a label that name a plain number.
const NUMBER_LABEL_WORDS: [&str; 13] = [
    "mm",
    "qty",
    "quantity",
    "rate",
    "ratio",
    "percent",
    "percentage",
    "points",
    "score",
    "hours",
    "days",
    "months",
    "count",
];
/// Unit nouns a Korean label ends with when the field is a plain number (진척률, 투입 인원 …).
const NUMBER_LABEL_ENDINGS: [&str; 17] = [
    "율",
    "률",
    "수량",
    "개월",
    "개월수",
    "인원",
    "인원수",
    "건수",
    "횟수",
    "시간",
    "일수",
    "개수",
    "배점",
    "점수",
    "공수",
    "중량",
    "면적",
];

/// Units in a label that are not plain won: the value is in that unit, so no `원` is put after it.
const OTHER_UNITS: [&str; 13] =
    ["천원", "만원", "백만", "억원", "USD", "usd", "달러", "$", "EUR", "유로", "JPY", "엔화", "¥"];

/// Whether an amount-kind field or column holds money in won. The pipeline's `amount` kind is
/// every number that may carry a fraction — percentages, person-months, points and quantities as
/// well as money — so the currency format is decided per field: by its key (the built-in
/// templates name theirs `progress_percent`, `pass_rate`, `months`, `ratio` …) and by the unit
/// its label names (registered forms: `진척률(%)`, `투입 MM`, `수량`, `금액(천원)`).
pub fn is_money(key: &str, label: &str) -> bool {
    let key = key.rsplit('.').next().unwrap_or(key).trim().to_ascii_lowercase();
    if NUMBER_KEYS.contains(&key.as_str()) || NUMBER_KEY_ENDINGS.iter().any(|e| key.ends_with(e)) {
        return false;
    }
    let label = label.trim();
    if label.contains(['%', '％'])
        || label.to_ascii_lowercase().contains("m/m")
        || OTHER_UNITS.iter().any(|u| label.contains(u))
    {
        return false;
    }
    let mut words = label.split(|c: char| !c.is_ascii_alphabetic()).filter(|w| !w.is_empty());
    if words.any(|w| NUMBER_LABEL_WORDS.contains(&w.to_ascii_lowercase().as_str())) {
        return false;
    }
    // A parenthesised unit aside: "투입 인원 (명)".
    let base = label.find(['(', '（']).map_or(label, |i| &label[..i]).trim_end();
    !NUMBER_LABEL_ENDINGS.iter().any(|e| base.ends_with(e))
}

/// Whether the entry is nothing but an amount: digits with separators, a sign, the won mark
/// (원, ₩, KRW) or the Korean magnitude words. Anything more — another currency, "약", a remark
/// in brackets — is the writer's wording.
fn bare_amount(v: &str) -> bool {
    let t = v.trim();
    let t = ["KRW", "krw"].iter().find_map(|m| t.strip_suffix(m).or_else(|| t.strip_prefix(m))).unwrap_or(t);
    t.chars().any(|c| c.is_ascii_digit())
        && t.chars().all(|c| {
            c.is_ascii_digit()
                || c.is_whitespace()
                || matches!(c, ',' | '.' | '-' | '(' | ')' | '₩' | '\\' | '원' | '정' | '만' | '억' | '△' | '▲')
        })
}

/// A money amount as printed (`1,250,000`), or `None` when the value is printed as entered: the
/// field is not money, the entry says more than an amount, or it is not a whole number — a
/// fraction is never rounded away (0.5 MM, 62.5 %, 1,234.56).
pub(super) fn money_text(key: &str, label: &str, v: &str) -> Option<String> {
    if !is_money(key, label) || !bare_amount(v) {
        return None;
    }
    let n = normalize_amount(v)?;
    (n.fract().abs() < 1e-9).then(|| format_won(n))
}

/// A date field as printed: the normalised date when the entry is nothing but one date; an entry
/// that says more (a time, a weekday, a range) is printed as entered.
pub(super) fn date_text(v: &str, normalized: Option<&str>) -> String {
    let bare =
        v.chars().all(|c| c.is_ascii_digit() || c.is_whitespace() || matches!(c, '.' | '-' | '/' | '년' | '월' | '일'))
            && v.chars().filter(char::is_ascii_digit).count() <= 8;
    match normalized {
        Some(n) if bare => n.to_string(),
        _ => v.to_string(),
    }
}

/// Value as printed in the document: money with separators and a currency mark, other numbers as
/// entered, dates / BRNs / phone numbers in their normalised form.
pub fn display_value(f: &DraftField, ko: bool) -> String {
    let v = f.value.trim();
    if v.is_empty() {
        return String::new();
    }
    match kind_of(f) {
        FieldKind::Amount => match money_text(&f.key, &f.label, v) {
            Some(n) if ko => format!("{n}원"),
            Some(n) => format!("KRW {n}"),
            None => v.to_string(),
        },
        FieldKind::Date => date_text(v, f.normalized.as_deref()),
        FieldKind::Brn | FieldKind::Phone => f.normalized.clone().unwrap_or_else(|| v.to_string()),
        _ => v.to_string(),
    }
}

fn display_cell(t: &DraftTable, col: usize, v: &str) -> String {
    let v = v.trim();
    match t.columns.get(col) {
        Some(c) if FieldKind::parse(&c.kind) == Some(FieldKind::Amount) => {
            money_text(&c.key, &c.label, v).unwrap_or_else(|| v.to_string())
        }
        _ => v.to_string(),
    }
}

fn col(t: &DraftTable, keys: &[&str]) -> Option<usize> {
    keys.iter().find_map(|k| t.columns.iter().position(|c| c.key == *k))
}

fn column_sum(t: &DraftTable, idx: Option<usize>) -> Option<f64> {
    let idx = idx?;
    let nums: Vec<f64> = t
        .rows
        .iter()
        .filter_map(|r| r.get(idx))
        .filter(|s| !s.trim().is_empty())
        .filter_map(|s| normalize_amount(s))
        .collect();
    (!nums.is_empty()).then(|| nums.iter().sum())
}

fn field_amount(draft: &Draft, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| draft.field(k)).and_then(|f| normalize_amount(f.value.trim()))
}

fn table_totals(draft: &Draft, t: &DraftTable, ko: bool) -> Vec<(String, String)> {
    let supply_i = col(t, &["supply_amount", "amount"]);
    if supply_i.is_none() {
        return Vec::new();
    }
    let tax_i = col(t, &["tax_amount", "tax", "vat"]);
    let supply = field_amount(draft, &["supply_total", "supply_amount"]).or_else(|| column_sum(t, supply_i));
    let Some(supply) = supply else { return Vec::new() };
    let tax = field_amount(draft, &["tax_total", "tax_amount", "vat"]).or_else(|| column_sum(t, tax_i));
    let grand = field_amount(draft, &["grand_total", "total", "total_amount"]).unwrap_or(supply + tax.unwrap_or(0.0));
    let money = |n: f64| if ko { format!("{}원", format_won(n)) } else { format!("KRW {}", format_won(n)) };
    let mut rows = vec![(tr(ko, "공급가액 합계", "Supply total"), money(supply))];
    if let Some(t) = tax {
        rows.push((tr(ko, "세액 합계", "Tax total"), money(t)));
    }
    rows.push((tr(ko, "합계금액", "Grand total"), money(grand)));
    rows
}

fn cite_key(c: &DraftCitation) -> String {
    format!("{}|{}|{}|{}", c.doc_id.clone().unwrap_or_default(), c.chunk_id.unwrap_or(-1), c.page.unwrap_or(0), c.title)
}

fn cite_label(c: &DraftCitation, ko: bool) -> String {
    let mut s = c.title.clone();
    if let Some(p) = c.page {
        let _ = write!(s, " {}", if ko { format!("{p}쪽") } else { format!("p.{p}") });
    }
    if c.source == "aggregate" || c.source == "text" {
        let _ = write!(s, " ({})", c.source);
    }
    s
}

struct Plan<'a> {
    items: Vec<Item<'a>>,
    ko: bool,
}

fn builtin_kind(draft: &Draft) -> Option<TemplateKind> {
    match &draft.template {
        TemplateRef::Builtin { template } => Some(*template),
        TemplateRef::Form { .. } => None,
    }
}

/// The two parties of a contract-like template (갑 / 을), as field-key prefixes.
fn party_prefixes(kind: Option<TemplateKind>) -> Option<(&'static str, &'static str)> {
    match kind {
        Some(TemplateKind::Contract) => Some(("party_a", "party_b")),
        Some(TemplateKind::DevContract | TemplateKind::Sow | TemplateKind::Acceptance) => {
            Some(("client", "contractor"))
        }
        _ => None,
    }
}

/// Field order for the grid: template order for built-ins (갑/을 interleaved for
/// contracts, 발주자/수행사 for the software templates), draft order for forms.
fn ordered_fields(draft: &Draft, kind: Option<TemplateKind>) -> Vec<&DraftField> {
    let mut fields: Vec<&DraftField> = draft.fields.iter().collect();
    if let Some((pa, pb)) = party_prefixes(kind) {
        let (pa, pb) = (format!("{pa}."), format!("{pb}."));
        let a: Vec<&DraftField> = fields.iter().copied().filter(|f| f.key.starts_with(&pa)).collect();
        let b: Vec<&DraftField> = fields.iter().copied().filter(|f| f.key.starts_with(&pb)).collect();
        let mut out: Vec<&DraftField> =
            fields.iter().copied().filter(|f| !f.key.starts_with(&pa) && !f.key.starts_with(&pb)).collect();
        let mut pairs = Vec::new();
        for (i, fa) in a.iter().enumerate() {
            pairs.push(*fa);
            if let Some(fb) = b.get(i) {
                pairs.push(*fb);
            }
        }
        // Parties first, then the rest.
        pairs.append(&mut out);
        fields = pairs;
    }
    fields
}

fn plan(draft: &Draft) -> Plan<'_> {
    let ko = draft.language == Language::Ko;
    let kind = builtin_kind(draft);
    let spec = kind.map(TemplateSpec::builtin);
    let template_name = spec
        .as_ref()
        .map(|s| s.name(ko).to_string())
        .or_else(|| draft.doc_type.clone())
        .unwrap_or_else(|| tr(ko, "문서", "Document"));
    let title = if draft.title.trim().is_empty() { template_name.clone() } else { draft.title.trim().to_string() };
    let letter = matches!(kind, Some(TemplateKind::OfficialLetter | TemplateKind::SupplementRequest));
    let mut items: Vec<Item<'_>> = Vec::new();
    let mut used: HashSet<String> = HashSet::new();
    let label_of = |key: &str, fallback: &str| -> String {
        draft.field(key).map(|f| f.label.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| fallback.to_string())
    };
    let value_of =
        |key: &str| -> Option<String> { draft.field(key).map(|f| display_value(f, ko)).filter(|v| !v.is_empty()) };

    // 1. Title (letters show the template name and carry the subject in the head).
    items.push(Item::Title(if letter { template_name.clone() } else { title.clone() }));
    // The `title` field is the subject line of a letter head (below). Elsewhere it is a field of its
    // own (계약명, 회의명, 변경 제목) and goes into the grid — unless it only repeats the document title.
    if letter || draft.field("title").is_some_and(|f| compact(&f.value) == compact(&title)) {
        used.insert("title".into());
    }

    // 2. Letter head.
    if letter {
        for key in ["document_no", "issue_date", "recipient", "cc"] {
            if let Some(v) = value_of(key) {
                items.push(Item::KeyValue(label_of(key, key), v));
            }
            used.insert(key.into());
        }
        items.push(Item::KeyValue(label_of("title", &tr(ko, "제목", "Subject")), title.clone()));
    }

    // 3. Grid of the remaining scalar fields.
    let signature_keys: &[&str] = match kind {
        Some(TemplateKind::OfficialLetter) => &["sender", "contact_person", "phone", "attachments"],
        Some(TemplateKind::SupplementRequest) => &["sender", "contact_person", "phone", "due_date", "subject"],
        Some(TemplateKind::Contract) => &[],
        _ => &[],
    };
    let long_keys = ["summary", "attachments"];
    let mut grid: Vec<(String, String)> = Vec::new();
    for f in ordered_fields(draft, kind) {
        if used.contains(&f.key) || signature_keys.contains(&f.key.as_str()) || long_keys.contains(&f.key.as_str()) {
            continue;
        }
        let v = display_value(f, ko);
        if v.is_empty() && !f.required {
            continue;
        }
        grid.push((f.label.clone(), v));
    }
    // 보완요청서: the case name sits under the head.
    if kind == Some(TemplateKind::SupplementRequest) {
        if let Some(v) = value_of("subject") {
            items.push(Item::KeyValue(label_of("subject", &tr(ko, "대상 건명", "Case")), v));
        }
    }
    if !grid.is_empty() {
        let per_row = if grid.len() >= 6 { 2 } else { 1 };
        items.push(Item::Grid(grid, per_row));
    }
    if let Some(v) = value_of("summary") {
        items.push(Item::KeyValue(label_of("summary", &tr(ko, "요약", "Summary")), v));
    }

    // 4. Tables of item-centric templates / forms come before the body.
    let tables_first = kind.is_none()
        || matches!(kind, Some(TemplateKind::Quotation | TemplateKind::TransactionStatement | TemplateKind::Estimate));
    let referenced: HashSet<&str> = draft
        .body
        .iter()
        .filter_map(|b| match b {
            DraftBlock::TableRef { key } => Some(key.as_str()),
            _ => None,
        })
        .collect();
    let mut rendered_tables: HashSet<&str> = HashSet::new();
    if tables_first {
        for t in &draft.tables {
            if !referenced.contains(t.key.as_str()) && (!t.rows.is_empty() || kind.is_none()) {
                items.push(Item::Table(t, table_totals(draft, t, ko)));
                rendered_tables.insert(&t.key);
            }
        }
    }

    // 5. Body with citations.
    let mut refs: Vec<DraftCitation> = Vec::new();
    let mut ref_index = |c: &DraftCitation| -> usize {
        let k = cite_key(c);
        if let Some(i) = refs.iter().position(|r| cite_key(r) == k) {
            i + 1
        } else {
            refs.push(c.clone());
            refs.len()
        }
    };
    let supplement_table_slot = kind == Some(TemplateKind::SupplementRequest)
        && !referenced.contains("items")
        && draft.table("items").is_some_and(|t| !t.rows.is_empty());
    let mut heading_count = 0usize;
    for b in &draft.body {
        match b {
            DraftBlock::Heading { level, text } => {
                heading_count += 1;
                // 보완요청서 without an explicit TableRef: the item table follows the second section heading.
                if supplement_table_slot && heading_count == 3 && !rendered_tables.contains("items") {
                    if let Some(t) = draft.table("items") {
                        items.push(Item::Table(t, Vec::new()));
                        rendered_tables.insert("items");
                    }
                }
                items.push(Item::Heading((*level).clamp(1, 4), text.clone()));
            }
            DraftBlock::Paragraph { text, source } => {
                let n = source.as_ref().map(&mut ref_index);
                items.push(Item::Paragraph(text.clone(), n));
            }
            DraftBlock::List { items: li, ordered } => items.push(Item::List(li.clone(), *ordered)),
            DraftBlock::TableRef { key } => {
                if let Some(t) = draft.table(key) {
                    items.push(Item::Table(t, table_totals(draft, t, ko)));
                    rendered_tables.insert(&t.key);
                }
            }
        }
    }
    // 6. Remaining tables.
    for t in &draft.tables {
        if !rendered_tables.contains(t.key.as_str()) && !t.rows.is_empty() {
            items.push(Item::Table(t, table_totals(draft, t, ko)));
        }
    }
    // 7. Closing lines.
    if kind == Some(TemplateKind::SupplementRequest) {
        if let Some(v) = value_of("due_date") {
            items.push(Item::KeyValue(label_of("due_date", &tr(ko, "보완 기한", "Deadline")), v));
        }
    }
    if let Some(v) = value_of("attachments") {
        items.push(Item::KeyValue(label_of("attachments", &tr(ko, "붙임", "Attachments")), v));
    }
    let mut signature: Vec<String> = Vec::new();
    match kind {
        Some(TemplateKind::OfficialLetter | TemplateKind::SupplementRequest) => {
            if let Some(v) = value_of("sender") {
                signature.push(if ko { format!("{v} (직인)") } else { format!("{v} (seal)") });
            }
            let contact: Vec<String> = [
                ("contact_person", if ko { "담당자" } else { "Contact" }),
                ("phone", if ko { "연락처" } else { "Phone" }),
            ]
            .iter()
            .filter_map(|(k, l)| value_of(k).map(|v| format!("{l} {v}")))
            .collect();
            if !contact.is_empty() {
                signature.push(contact.join(" · "));
            }
        }
        Some(TemplateKind::Contract | TemplateKind::DevContract | TemplateKind::Acceptance) => {
            let (pa, pb) = party_prefixes(kind).unwrap_or(("party_a", "party_b"));
            for (p, l) in [(pa, if ko { "갑" } else { "Party A" }), (pb, if ko { "을" } else { "Party B" })] {
                let name = value_of(&format!("{p}.name"));
                let rep = value_of(&format!("{p}.representative"));
                if name.is_some() || rep.is_some() {
                    let mut line = format!("{l}: {}", name.unwrap_or_default());
                    if let Some(r) = rep {
                        let _ = write!(line, " {} {r}", if ko { "대표" } else { "Rep." });
                    }
                    line.push_str(if ko { " (인)" } else { " (seal)" });
                    signature.push(line);
                }
            }
        }
        Some(TemplateKind::Quotation | TemplateKind::Estimate) => {
            if let Some(v) = value_of("supplier.name") {
                signature.push(if ko { format!("{v} (인)") } else { format!("{v} (seal)") });
            }
        }
        _ => {}
    }
    if !signature.is_empty() {
        items.push(Item::Signature(signature));
    }
    if !refs.is_empty() {
        items.push(Item::References(
            refs.iter().enumerate().map(|(i, c)| format!("[{}] {}", i + 1, cite_label(c, ko))).collect(),
        ));
    }
    Plan { items, ko }
}

// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------

const CSS: &str = r#"
@page { size: A4; margin: 0; }
html, body { margin: 0; padding: 0; background: #fff; color: #111; }
body { font-family: 'Malgun Gothic', 'Pretendard', 'Noto Sans KR', 'Apple SD Gothic Neo', sans-serif; font-size: 11pt; line-height: 1.6; word-break: keep-all; overflow-wrap: anywhere; }
article.sheet { width: 210mm; min-height: 297mm; box-sizing: border-box; padding: 20mm 18mm 22mm; margin: 0 auto; }
h1.title { text-align: center; font-size: 20pt; letter-spacing: 0.1em; margin: 0 0 10mm; }
h2 { font-size: 13pt; margin: 7mm 0 2mm; page-break-after: avoid; }
h3, h4 { font-size: 11.5pt; margin: 5mm 0 1.5mm; page-break-after: avoid; }
p { margin: 0 0 2.2mm; }
.kv { margin: 0 0 1.2mm; } .kv b { display: inline-block; min-width: 18mm; margin-right: 3mm; }
table { border-collapse: collapse; width: 100%; margin: 3mm 0 5mm; table-layout: fixed; }
td, th { border: 0.5pt solid #333; padding: 1.4mm 2mm; vertical-align: top; font-size: 10pt; }
th { background: #f2f2f2; font-weight: 600; text-align: center; }
table.grid th { width: 24%; text-align: left; }
table.items td.num { text-align: right; font-variant-numeric: tabular-nums; }
table.items tr.totals td { font-weight: 600; background: #fafafa; }
table.items tr.totals td.label { text-align: right; }
span.blank { display: inline-block; min-width: 30mm; border-bottom: 0.5pt solid #999; }
sup.cite { font-size: 7.5pt; color: #555; margin-left: 0.3em; }
ul, ol { margin: 0 0 2.5mm; padding-left: 6mm; }
.signature { text-align: right; margin-top: 10mm; } .signature p { margin: 0 0 1.5mm; }
.refs { margin-top: 8mm; border-top: 0.5pt solid #999; padding-top: 2mm; font-size: 9pt; color: #444; }
.refs p { margin: 0 0 0.8mm; }
"#;

fn html_value(v: &str) -> String {
    if v.is_empty() {
        "<span class=\"blank\">&nbsp;</span>".to_string()
    } else {
        escape(v).replace('\n', "<br>")
    }
}

fn table_html(t: &DraftTable, totals: &[(String, String)], out: &mut String) {
    out.push_str("<table class=\"items\"><thead><tr>");
    for c in &t.columns {
        let _ = write!(out, "<th>{}</th>", escape(&c.label));
    }
    out.push_str("</tr></thead><tbody>");
    let numeric: Vec<bool> = t
        .columns
        .iter()
        .map(|c| matches!(FieldKind::parse(&c.kind), Some(FieldKind::Amount | FieldKind::Integer)))
        .collect();
    for row in &t.rows {
        out.push_str("<tr>");
        for (i, _) in t.columns.iter().enumerate() {
            let v = row.get(i).map(|s| display_cell(t, i, s)).unwrap_or_default();
            let _ = write!(
                out,
                "<td{}>{}</td>",
                if numeric[i] { " class=\"num\"" } else { "" },
                html_value(&v).replace("<span class=\"blank\">&nbsp;</span>", "&nbsp;")
            );
        }
        out.push_str("</tr>");
    }
    let cols = t.columns.len().max(1);
    for (label, value) in totals {
        let _ = write!(
            out,
            "<tr class=\"totals\"><td class=\"label\" colspan=\"{}\">{}</td><td class=\"num\">{}</td></tr>",
            cols.saturating_sub(1).max(1),
            escape(label),
            escape(value)
        );
    }
    out.push_str("</tbody></table>\n");
}

/// Self-contained HTML page of the draft (preview and PDF source).
pub fn to_html(draft: &Draft) -> String {
    let p = plan(draft);
    let ko = p.ko;
    let mut out = String::with_capacity(8_192);
    let title = draft.title.trim();
    let _ = write!(
        out,
        "<!doctype html>\n<html lang=\"{}\"><head><meta charset=\"utf-8\">\n<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:\">\n<title>{}</title>\n<meta name=\"viewport\" content=\"width=device-width\">\n<style>{CSS}</style></head>\n<body><article class=\"sheet\">\n",
        if ko { "ko" } else { "en" },
        escape(if title.is_empty() { "document" } else { title })
    );
    for item in &p.items {
        match item {
            Item::Title(t) => {
                let _ = writeln!(out, "<h1 class=\"title\">{}</h1>", escape(t));
            }
            Item::KeyValue(l, v) => {
                let _ = writeln!(out, "<p class=\"kv\"><b>{}</b> {}</p>", escape(l), html_value(v));
            }
            Item::Grid(rows, per_row) => {
                out.push_str("<table class=\"grid\"><tbody>");
                for chunk in rows.chunks(*per_row) {
                    out.push_str("<tr>");
                    for (l, v) in chunk {
                        let _ = write!(out, "<th>{}</th><td>{}</td>", escape(l), html_value(v));
                    }
                    if chunk.len() < *per_row {
                        out.push_str("<th></th><td></td>");
                    }
                    out.push_str("</tr>");
                }
                out.push_str("</tbody></table>\n");
            }
            Item::Heading(level, text) => {
                let l = (*level).clamp(2, 4);
                let _ = writeln!(out, "<h{l}>{}</h{l}>", escape(text));
            }
            Item::Paragraph(text, n) => {
                let _ = write!(out, "<p>{}", escape(text).replace('\n', "<br>"));
                if let Some(n) = n {
                    let _ = write!(out, "<sup class=\"cite\">[{n}]</sup>");
                }
                out.push_str("</p>\n");
            }
            Item::List(items, ordered) => {
                let tag = if *ordered { "ol" } else { "ul" };
                let _ = write!(out, "<{tag}>");
                for i in items {
                    let _ = write!(out, "<li>{}</li>", escape(i));
                }
                let _ = writeln!(out, "</{tag}>");
            }
            Item::Table(t, totals) => table_html(t, totals, &mut out),
            Item::Signature(lines) => {
                out.push_str("<div class=\"signature\">");
                for l in lines {
                    let _ = write!(out, "<p>{}</p>", escape(l));
                }
                out.push_str("</div>\n");
            }
            Item::References(lines) => {
                out.push_str("<div class=\"refs\">");
                let _ = write!(out, "<p><b>{}</b></p>", tr(ko, "참고 자료", "References"));
                for l in lines {
                    let _ = write!(out, "<p>{}</p>", escape(l));
                }
                out.push_str("</div>\n");
            }
        }
    }
    out.push_str("</article></body></html>\n");
    out
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

pub fn to_markdown(draft: &Draft) -> String {
    let p = plan(draft);
    let ko = p.ko;
    let mut out = String::with_capacity(4_096);
    for item in &p.items {
        match item {
            Item::Title(t) => {
                let _ = writeln!(out, "# {t}\n");
            }
            Item::KeyValue(l, v) => {
                let _ = writeln!(out, "**{l}**: {}  ", if v.is_empty() { "____" } else { v.as_str() });
            }
            Item::Grid(rows, _) => {
                out.push('\n');
                let _ = writeln!(out, "| {} | {} |", tr(ko, "항목", "Field"), tr(ko, "내용", "Value"));
                out.push_str("| --- | --- |\n");
                for (l, v) in rows {
                    let _ = writeln!(
                        out,
                        "| {} | {} |",
                        md_cell(l),
                        if v.is_empty() { "____".to_string() } else { md_cell(v) }
                    );
                }
                out.push('\n');
            }
            Item::Heading(level, text) => {
                let _ = writeln!(out, "\n{} {text}\n", "#".repeat((*level).clamp(2, 4) as usize));
            }
            Item::Paragraph(text, n) => {
                let _ = writeln!(out, "{text}{}\n", n.map(|n| format!(" [{n}]")).unwrap_or_default());
            }
            Item::List(items, ordered) => {
                for (i, it) in items.iter().enumerate() {
                    if *ordered {
                        let _ = writeln!(out, "{}. {it}", i + 1);
                    } else {
                        let _ = writeln!(out, "- {it}");
                    }
                }
                out.push('\n');
            }
            Item::Table(t, totals) => {
                out.push('\n');
                let _ = writeln!(
                    out,
                    "| {} |",
                    t.columns.iter().map(|c| md_cell(&c.label)).collect::<Vec<_>>().join(" | ")
                );
                let _ = writeln!(out, "|{}", " --- |".repeat(t.columns.len().max(1)));
                for row in &t.rows {
                    let cells: Vec<String> = (0..t.columns.len())
                        .map(|i| row.get(i).map(|s| md_cell(&display_cell(t, i, s))).unwrap_or_default())
                        .collect();
                    let _ = writeln!(out, "| {} |", cells.join(" | "));
                }
                for (l, v) in totals {
                    let mut cells = vec![String::new(); t.columns.len().saturating_sub(2)];
                    cells.push(format!("**{}**", md_cell(l)));
                    cells.push(format!("**{}**", md_cell(v)));
                    let _ = writeln!(out, "| {} |", cells.join(" | "));
                }
                out.push('\n');
            }
            Item::Signature(lines) => {
                out.push('\n');
                for l in lines {
                    let _ = writeln!(out, "{l}  ");
                }
            }
            Item::References(lines) => {
                let _ = writeln!(out, "\n---\n**{}**\n", tr(ko, "참고 자료", "References"));
                for l in lines {
                    let _ = writeln!(out, "{l}  ");
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// DocModel (from-scratch DOCX / HWPX)
// ---------------------------------------------------------------------------

fn run(text: &str, bold: bool) -> Run {
    Run { text: text.to_string(), bold, ..Default::default() }
}

fn para(runs: Vec<Run>, heading: Option<u8>, align: Align, list: Option<ListKind>) -> Block {
    Block::Paragraph(Paragraph { runs, heading, align, list, level: 0 })
}

fn cell(text: &str, header: bool, col_span: u16) -> Cell {
    let blocks: Vec<Block> =
        text.split('\n').map(|line| para(vec![run(line, header)], None, Align::Left, None)).collect();
    Cell { blocks, row_span: 1, col_span, header }
}

/// The draft as a format-independent document model (title, field grid, body, item tables).
pub fn doc_model(draft: &Draft) -> DocModel {
    let p = plan(draft);
    let ko = p.ko;
    let mut model = DocModel { page_mm: Some((210.0, 297.0)), ..Default::default() };
    for item in &p.items {
        match item {
            Item::Title(t) => {
                model.title = Some(t.clone());
                model.blocks.push(para(vec![run(t, true)], Some(1), Align::Center, None));
            }
            Item::KeyValue(l, v) => {
                let value = if v.is_empty() { "________".to_string() } else { v.clone() };
                model.blocks.push(para(
                    vec![run(&format!("{l}: "), true), run(&value, false)],
                    None,
                    Align::Left,
                    None,
                ));
            }
            Item::Grid(rows, per_row) => {
                let mut table = Table::default();
                for chunk in rows.chunks(*per_row) {
                    let mut r = Vec::new();
                    for (l, v) in chunk {
                        r.push(cell(l, true, 1));
                        r.push(cell(if v.is_empty() { "" } else { v }, false, 1));
                    }
                    while r.len() < per_row * 2 {
                        r.push(cell("", false, 1));
                    }
                    table.rows.push(r);
                }
                let text_mm = 174.0_f32;
                let n = per_row * 2;
                table.col_widths_mm = (0..n)
                    .map(|i| {
                        if i % 2 == 0 {
                            text_mm * 0.22 / *per_row as f32 * 2.0 / 2.0
                        } else {
                            text_mm * 0.28 / *per_row as f32 * 2.0 / 2.0
                        }
                    })
                    .collect();
                // Normalise so the widths sum to the text width.
                let sum: f32 = table.col_widths_mm.iter().sum();
                if sum > 0.0 {
                    for w in &mut table.col_widths_mm {
                        *w = *w / sum * text_mm;
                    }
                }
                model.blocks.push(Block::Table(table));
            }
            Item::Heading(level, text) => {
                model.blocks.push(para(vec![run(text, true)], Some((*level).clamp(2, 4)), Align::Left, None));
            }
            Item::Paragraph(text, n) => {
                let mut runs: Vec<Run> = Vec::new();
                for (i, line) in text.split('\n').enumerate() {
                    if i > 0 {
                        if let Some(last) = runs.last_mut() {
                            last.line_break = true;
                        }
                    }
                    runs.push(run(line, false));
                }
                if let Some(n) = n {
                    runs.push(Run { text: format!(" [{n}]"), size_pt: Some(8.0), ..Default::default() });
                }
                model.blocks.push(para(runs, None, Align::Justify, None));
            }
            Item::List(items, ordered) => {
                let kind = if *ordered { ListKind::Number } else { ListKind::Bullet };
                for it in items {
                    model.blocks.push(para(vec![run(it, false)], None, Align::Left, Some(kind)));
                }
            }
            Item::Table(t, totals) => {
                let mut table = Table::default();
                table.rows.push(t.columns.iter().map(|c| cell(&c.label, true, 1)).collect());
                for row in &t.rows {
                    table.rows.push(
                        (0..t.columns.len())
                            .map(|i| cell(&row.get(i).map(|s| display_cell(t, i, s)).unwrap_or_default(), false, 1))
                            .collect(),
                    );
                }
                let cols = t.columns.len().max(1) as u16;
                for (l, v) in totals {
                    table.rows.push(vec![cell(l, true, cols.saturating_sub(1).max(1)), cell(v, false, 1)]);
                }
                model.blocks.push(Block::Table(table));
            }
            Item::Signature(lines) => {
                model.blocks.push(para(vec![run("", false)], None, Align::Left, None));
                for l in lines {
                    model.blocks.push(para(vec![run(l, false)], None, Align::Right, None));
                }
            }
            Item::References(lines) => {
                model.blocks.push(para(vec![run(&tr(ko, "참고 자료", "References"), true)], None, Align::Left, None));
                for l in lines {
                    model.blocks.push(para(
                        vec![Run { text: l.clone(), size_pt: Some(9.0), ..Default::default() }],
                        None,
                        Align::Left,
                        None,
                    ));
                }
            }
        }
    }
    model
}

// ---------------------------------------------------------------------------
// Save
// ---------------------------------------------------------------------------

fn part_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

/// Whether two paths name the same file: canonical paths when both exist (symlinks, case),
/// else the lexically normalised paths (a target that does not exist yet).
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a.components().collect::<PathBuf>() == b.components().collect::<PathBuf>(),
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let part = part_path(path);
    std::fs::write(&part, bytes).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        CoreError::io(&part, e)
    })?;
    std::fs::rename(&part, path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        CoreError::io(path, e)
    })
}

/// Registered HWPX / DOCX form behind a `TemplateRef::Form` draft, when it exists on disk.
fn form_source(deps: &GenerateDeps<'_>, draft: &Draft) -> Result<Option<(PathBuf, SourceKind)>> {
    let TemplateRef::Form { doc_id: Some(id), .. } = &draft.template else { return Ok(None) };
    let Some(doc) = deps.db.with(|c| docs::get_document(c, id))? else { return Ok(None) };
    if !matches!(doc.source_kind, SourceKind::Docx | SourceKind::Hwpx) {
        return Ok(None);
    }
    let abs = doc.absolute_path(deps.paths);
    Ok(abs.is_file().then_some((abs, doc.source_kind)))
}

fn strip_known_extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && OutputFormat::parse(ext).is_some() => stem,
        _ => name,
    }
}

/// Target folder (workspace-relative, sanitised) or an error for paths outside the workspace.
fn resolve_folder(req: &SaveRequest) -> Result<String> {
    let raw = req.folder.as_deref().map(str::trim).filter(|f| !f.is_empty()).unwrap_or(DEFAULT_FOLDER);
    let escapes = raw.replace('\\', "/").split('/').any(|c| c.trim() == "..");
    if Path::new(raw).is_absolute() || raw.contains(':') || raw.starts_with('\\') || raw.starts_with('/') || escapes {
        return Err(CoreError::Invalid("folder_outside_workspace".into()));
    }
    // The app folder is never a target, even before sanitising turns ".doculens" into "_doculens".
    if raw.replace('\\', "/").split('/').any(|c| c.trim().eq_ignore_ascii_case(WORKSPACE_DIR_NAME)) {
        return Err(CoreError::Invalid("folder_invalid".into()));
    }
    let folder = sanitize_folder(raw);
    if folder.is_empty() {
        return Err(CoreError::Invalid("folder_invalid".into()));
    }
    Ok(folder)
}

/// Inserts (or refreshes) the library row of the written file and processes it
/// (conversion / rendering through the import deps). Returns the document id.
fn register(
    deps: &GenerateDeps<'_>,
    imp: &ImportDeps<'_>,
    abs: &Path,
    rel: &str,
    draft: &Draft,
    warnings: &mut Vec<String>,
) -> Result<Option<String>> {
    let ko = deps.ko();
    let Some(kind) = SourceKind::from_path(abs) else {
        // Every output format is an input format now (Markdown through the text reader); this only
        // guards an unexpected extension.
        let ext = abs.extension().and_then(|e| e.to_str()).unwrap_or("");
        warnings.push(tr(
            ko,
            &format!("{ext} 파일은 라이브러리에 등록되지 않습니다(입력 형식이 아님)."),
            &format!("{ext} files are not registered in the library (not an input format)."),
        ));
        return Ok(None);
    };
    let meta = std::fs::metadata(abs).map_err(|e| CoreError::io(abs, e))?;
    let hash = import::blake3_file(abs)?;
    let modified = meta.modified().ok().map(DateTime::<Utc>::from);
    let name = documents::nfc(abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().as_str());
    let existing = deps.db.with(|c| docs::find_by_rel_path(c, rel))?;
    let id = match existing {
        // `save` refuses such targets before writing; this only catches a registration that
        // slipped in between (never turn a user's document into a generated one).
        Some(doc) if doc.role != DocumentRole::Generated => {
            return Err(CoreError::Invalid("export_over_document".into()));
        }
        Some(doc) => {
            deps.db.with(|c| {
                docs::set_file_info(c, &doc.id, &hash, meta.len(), modified)?;
                docs::set_rel_path(c, &doc.id, rel, &name)?;
                docs::set_status(c, &doc.id, DocumentStatus::Importing, None)
            })?;
            doc.id
        }
        None => {
            let id = documents::new_id();
            deps.db.with(|c| {
                docs::insert_document(
                    c,
                    &NewDocument {
                        id: id.clone(),
                        rel_path: rel.to_string(),
                        original_name: name.clone(),
                        source_kind: kind,
                        content_hash: hash.clone(),
                        size_bytes: meta.len(),
                        file_modified_at: modified,
                        role: DocumentRole::Generated,
                        import_batch_id: None,
                    },
                )
            })?;
            id
        }
    };
    let doc_type = draft.doc_type.clone().filter(|d| !d.is_empty());
    let title = draft.title.trim();
    deps.db.with(|c| {
        docs::set_role(c, &id, DocumentRole::Generated)?;
        docs::set_doc_type(c, &id, doc_type.as_deref(), Some("generated"))?;
        docs::set_title(c, &id, (!title.is_empty()).then_some(title))
    })?;
    let doc = deps.db.with(|c| docs::get_document(c, &id))?.ok_or_else(|| CoreError::NotFound(id.clone()))?;
    if let Err(e) = import::process_document(imp, &doc, true, None, deps.cancel, &mut |_| {}) {
        warnings.push(if ko {
            format!("생성 문서를 라이브러리에 등록했지만 처리(변환/렌더링)에 실패했습니다: {e}")
        } else {
            format!("The file was registered but processing (conversion / rendering) failed: {e}")
        });
    }
    Ok(Some(id))
}

/// Writes the draft in the requested format and registers the file.
pub fn save(deps: &GenerateDeps<'_>, req: &SaveRequest) -> Result<SavedDocument> {
    let ko = deps.ko();
    let mut draft = req.draft.clone();
    super::validate::validate(&mut draft);
    let mut warnings: Vec<String> = Vec::new();
    if draft.has_blocking_issues() {
        warnings.push(tr(
            ko,
            "검증에서 확인이 필요한 항목이 있는 상태로 저장했습니다 (검토 전).",
            "Saved with unresolved validation issues (not yet reviewed).",
        ));
    }
    let folder = resolve_folder(req)?;
    let doc_ko = draft.language == Language::Ko;
    let template_name = match &draft.template {
        TemplateRef::Builtin { template } => TemplateSpec::builtin(*template).name(doc_ko).to_string(),
        TemplateRef::Form { .. } => draft.doc_type.clone().unwrap_or_else(|| tr(doc_ko, "문서", "document")),
    };
    let title = if draft.title.trim().is_empty() { template_name.clone() } else { draft.title.trim().to_string() };
    let stem = match req.file_name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => sanitize_component(strip_known_extension(n)),
        None => format!("{}_{}", Local::now().format("%Y-%m-%d"), sanitize_component(&title)),
    };

    // Which format is really written: a registered HWPX / DOCX form keeps its own format;
    // PDF needs a printer (else Markdown).
    let mut format = req.output;
    let mut fill_from: Option<(PathBuf, SourceKind)> = None;
    if matches!(format, OutputFormat::Docx | OutputFormat::Hwpx) {
        if let Some((src, kind)) = form_source(deps, &draft)? {
            let form_format = if kind == SourceKind::Docx { OutputFormat::Docx } else { OutputFormat::Hwpx };
            if form_format != format {
                warnings.push(tr(
                    ko,
                    &format!(
                        "등록 양식이 {} 파일이라 {}로 저장했습니다.",
                        form_format.as_str().to_uppercase(),
                        form_format.as_str().to_uppercase()
                    ),
                    &format!(
                        "The registered form is a {} file, so the draft was saved as {}.",
                        form_format.as_str().to_uppercase(),
                        form_format.as_str().to_uppercase()
                    ),
                ));
                format = form_format;
            }
            fill_from = Some((src, kind));
        }
    }
    let mut pdf_bytes: Option<Vec<u8>> = None;
    if format == OutputFormat::Pdf {
        match deps.printer {
            None => {
                warnings.push(tr(
                    ko,
                    "PDF 인쇄 모듈을 사용할 수 없어 Markdown(.md)으로 저장했습니다.",
                    "No PDF printer is available; the draft was saved as Markdown (.md) instead.",
                ));
                format = OutputFormat::Md;
            }
            Some(printer) => {
                let dir = deps.paths.cache_dir().join("generated").join(&draft.id);
                std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
                let html = dir.join("index.html");
                std::fs::write(&html, to_html(&draft)).map_err(|e| CoreError::io(&html, e))?;
                let pdf = dir.join("document.pdf");
                let printed = match printer.print_to_pdf(&html, &pdf, crate::documents::convert::PageSize::A4) {
                    Ok(()) => std::fs::read(&pdf).ok().filter(|b| !b.is_empty()),
                    Err(e) => {
                        warnings.push(if ko {
                            format!("PDF 인쇄에 실패해 Markdown(.md)으로 저장했습니다: {e}")
                        } else {
                            format!("PDF printing failed; saved as Markdown (.md) instead: {e}")
                        });
                        None
                    }
                };
                let _ = std::fs::remove_dir_all(&dir);
                match printed {
                    Some(b) => pdf_bytes = Some(b),
                    None => {
                        if warnings.iter().all(|w| !w.contains("PDF")) {
                            warnings.push(tr(
                                ko,
                                "PDF 인쇄 결과가 비어 있어 Markdown(.md)으로 저장했습니다.",
                                "The PDF printer produced no output; saved as Markdown (.md) instead.",
                            ));
                        }
                        format = OutputFormat::Md;
                    }
                }
            }
        }
    }

    let rel = format!("{folder}/{stem}.{}", format.extension());
    let abs = documents::from_rel_path(&deps.paths.root, &rel);
    // Never on top of the registered form being filled or of a library document that is not
    // an earlier generated file — whatever `overwrite` says: it only replaces unregistered
    // files and generated documents.
    if fill_from.as_ref().is_some_and(|(src, _)| same_file(src, &abs)) {
        return Err(CoreError::Invalid("export_over_form".into()));
    }
    if deps.db.with(|c| docs::find_by_rel_path(c, &rel))?.is_some_and(|d| d.role != DocumentRole::Generated) {
        return Err(CoreError::Invalid("export_over_document".into()));
    }
    if abs.exists() && !req.overwrite {
        return Err(CoreError::Invalid("export_exists".into()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    if deps.cancelled() {
        return Err(CoreError::Cancelled);
    }
    let mut filled_form = false;
    match format {
        OutputFormat::Md => write_atomic(&abs, to_markdown(&draft).as_bytes())?,
        OutputFormat::Pdf => write_atomic(&abs, &pdf_bytes.unwrap_or_default())?,
        OutputFormat::Docx | OutputFormat::Hwpx => {
            let part = part_path(&abs);
            let result = match &fill_from {
                Some((src, kind)) => fill::fill_form(src, *kind, &draft, &part).map(|w| {
                    filled_form = true;
                    warnings.extend(w);
                }),
                None => {
                    let model = doc_model(&draft);
                    if format == OutputFormat::Docx {
                        office::write_docx(&model, &part)
                    } else {
                        office::write_hwpx(&model, &part)
                    }
                }
            };
            if let Err(e) = result {
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
            std::fs::rename(&part, &abs).map_err(|e| {
                let _ = std::fs::remove_file(&part);
                CoreError::io(&abs, e)
            })?;
        }
    }
    // A registered form has slots for fields and item rows only: the body the preview shows is not
    // in the filled file. The fill stays as it is — the user is told what was left out.
    let omitted_body_blocks = if filled_form { fill::body_blocks(&draft) as u32 } else { 0 };
    if omitted_body_blocks > 0 {
        warnings.push(tr(
            ko,
            &format!(
                "등록 양식에는 필드와 표만 채워집니다. 본문 {omitted_body_blocks}개 블록은 저장한 파일에 들어가지 않았습니다 — 본문까지 담으려면 PDF나 Markdown으로 저장하세요."
            ),
            &format!(
                "A registered form takes fields and tables only: {omitted_body_blocks} body block(s) were left out of the saved file. Save as PDF or Markdown to keep the body."
            ),
        ));
    }

    let mut doc_id = None;
    if req.register {
        match deps.import {
            Some(imp) => doc_id = register(deps, imp, &abs, &rel, &draft, &mut warnings)?,
            None => warnings.push(tr(
                ko,
                "라이브러리 등록을 사용할 수 없어 파일만 저장했습니다.",
                "Library registration is unavailable; only the file was written.",
            )),
        }
    }
    draft.saved_path = Some(rel.clone());
    draft.saved_doc_id = doc_id.clone();
    if let Err(e) = store::save(deps.paths, &draft) {
        warnings.push(if ko {
            format!("초안 저장 실패: {e}")
        } else {
            format!("Could not store the draft: {e}")
        });
    }
    Ok(SavedDocument { rel_path: rel, absolute: abs, format, doc_id, filled_form, warnings, omitted_body_blocks })
}
