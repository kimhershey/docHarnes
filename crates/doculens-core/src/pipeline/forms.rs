//! Form registration (§3.4.0) and natural-language schemas (§3.5): derive the
//! field list of a user's form from its structure (HWPX/DOCX tables and
//! "label: ____" lines, no model needed) or with the VLM for PDF / image
//! forms, and create / modify schemas from a description.

use std::sync::OnceLock;

use regex::Regex;
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
        ("견적서", "quotation"),
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
        ("회의록", "minutes"),
        ("이력서", "resume"),
        ("계약서", "contract"),
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
            let mut ck = schema::key_from_label(&label, &format!("col_{}", i + 1));
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
pub fn derive(
    deps: &ImportDeps<'_>,
    vlm: Option<&Vlm<'_>>,
    doc: &DocumentRecord,
    name: Option<&str>,
) -> Result<SchemaDraft> {
    let abs = doc.absolute_path(deps.paths);
    let fallback_name =
        name.map(str::to_string).filter(|n| !n.trim().is_empty()).or_else(|| doc.title.clone()).unwrap_or_else(|| {
            abs.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "양식".into())
        });
    let pages = deps.db.with(|c| docs::list_pages(c, &doc.id, true))?;
    let text: String = pages.iter().filter_map(|p| p.text.as_deref()).collect::<Vec<_>>().join("\n");
    let mut draft = if doc.source_kind.needs_conversion() && abs.is_file() {
        let model = convert::read_document(&abs, doc.source_kind)?;
        let d = derive_from_model(&model, Some(&fallback_name));
        if d.definition.fields.is_empty() {
            match vlm {
                Some(v) => derive_with_vlm(v, None, Some(&text), Some(&fallback_name))?,
                None => d,
            }
        } else {
            d
        }
    } else {
        let cache = DocumentCache::for_document(deps.paths, &doc.id);
        let first = pages.iter().find(|p| p.width_px > 0);
        let img = match first {
            Some(p) => {
                let path = cache.page_file(p.page_no);
                if path.is_file() {
                    Some(image::prepare(&path, 2_048)?)
                } else {
                    None
                }
            }
            None => None,
        };
        let from_text = |note: Option<String>| -> Result<SchemaDraft> {
            if text.trim().is_empty() {
                return Err(CoreError::Invalid("no_model".into()));
            }
            let tables: Vec<ExtractedTable> = pages
                .iter()
                .flat_map(|p| super::ocr::parse_tables(p.text.as_deref().unwrap_or(""), p.page_no))
                .collect();
            let mut d = derive_from_text(&text, &tables, Some(&fallback_name));
            d.note = note;
            Ok(d)
        };
        match vlm {
            Some(v) => match derive_with_vlm(
                v,
                img.as_ref().map(|i| i.url.as_str()),
                Some(text.as_str()).filter(|t| !t.trim().is_empty()),
                Some(&fallback_name),
            ) {
                Ok(d) => d,
                // A model failure (cut-off reply, output-parser 500, engine hiccup) must not leave the
                // user without a draft (user report 2026-11-09 "양식 등록이 되지 않음"): the text / table
                // heuristics stand in and the note says why, so the editor can still be confirmed.
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
    if draft.definition.fields.is_empty() {
        draft.definition.fields.push(FieldDef::new("title", "제목", "Title", FieldKind::String));
    }
    Ok(draft)
}

/// Saves a confirmed form draft and marks the sample document as the form
/// (`role = form`, `doc_type = <slug>` with source `form`).
pub fn register(db: &Db, draft: &SchemaDraft) -> Result<SchemaRecord> {
    let mut draft = draft.clone();
    draft.source = SchemaSource::Form;
    if draft.doc_type.trim().is_empty() || !schema::is_key(&draft.doc_type) {
        draft.doc_type = slug_for_name(&draft.name);
    }
    let form_doc = draft.form_document_id.clone();
    db.with_tx(move |tx| {
        let saved = repo::save_schema(tx, &draft)?;
        if let Some(id) = &form_doc {
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
        assert_eq!(d.doc_type, "quotation");
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
}
