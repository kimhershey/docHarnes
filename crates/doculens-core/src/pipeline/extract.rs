//! Key-information extraction (§3.4 "VLM 3차"): schema-constrained JSON per
//! page, page merge, and the turn of raw model output into validated,
//! masked, confidence-scored field rows with review reasons.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::engine::client::{user_message, user_message_with_image};
use crate::error::{CoreError, Result};

use super::prompts::{self, Stage};
use super::repo::NewField;
use super::schema::{self, FieldDef, FieldKind, SchemaDef, SchemaRecord};
use super::validate::{self, ConfidenceInput, Values};
use super::vlm::{Reply, Vlm};
use super::{ReviewReason, ValidationIssue};

/// One page's input to the extractor.
pub struct PageInput<'a> {
    pub page_no: u32,
    pub page_count: u32,
    pub image_url: Option<&'a str>,
    pub text: Option<&'a str>,
}

/// llama-server refuses a prompt longer than the slot's context ("request (N tokens)
/// exceeds the available context size (M tokens)"); small presets (CPU, 4K) hit this
/// with a dense page text next to the image and the schema summary.
pub fn context_overflow(e: &CoreError) -> bool {
    matches!(e, CoreError::Engine(m) if m.contains("exceeds the available context size"))
}

/// Extracts one page. `temperature` overrides the preset (precision-mode second sample).
/// When the prompt does not fit the context, the page-text anchor is shortened
/// (half, then a quarter, then dropped) before giving up.
pub fn extract_page(
    vlm: &Vlm<'_>,
    schema: &SchemaRecord,
    page: &PageInput<'_>,
    temperature: Option<f32>,
) -> Result<(Value, Reply)> {
    let js = schema::json_schema(&schema.definition);
    let name = format!("kie_{}", schema.doc_type);
    // Precision mode (§3.4): the same request sampled at T=t with another seed —
    // greedy decoding twice would return the same answer and prove nothing.
    let sampled = temperature
        .map(|t| Vlm::new(vlm.chat, vlm.model.clone(), vlm.profile, vlm.cancel).with_sampling(Some(t), Some(11)));
    let caller = sampled.as_ref().unwrap_or(vlm);
    let mut text_chars = page.text.map(|t| t.chars().count().min(prompts::KIE_TEXT_CHARS)).unwrap_or(0);
    loop {
        let shortened;
        let text = match (page.text, text_chars) {
            (Some(t), n) if n > 0 => {
                shortened = prompts::truncate(t, n);
                Some(shortened.as_str())
            }
            _ => None,
        };
        let prompt = prompts::kie(&schema.definition, &schema.name, page.page_no, page.page_count, text);
        let system = json!({ "role": "system", "content": prompts::SYSTEM_JSON });
        let messages = match page.image_url {
            Some(url) => vec![system, user_message_with_image(&prompt, url)],
            None => vec![system, user_message(&prompt)],
        };
        match caller.call_json(Stage::Kie, &messages, &name, &js, None) {
            Err(e) if context_overflow(&e) && text_chars > 0 => {
                text_chars = if text_chars > 1_000 { text_chars / 2 } else { 0 };
                log::warn!(
                    "kie page {}: prompt exceeds the context, retrying with {text_chars} text chars",
                    page.page_no
                );
            }
            r => return r,
        }
    }
}

/// Masks the model's raw output before it is stored or mirrored (§3.8): PII-kind
/// and `mask` fields through `mask_value`, every string through the resident-number
/// pattern when masking is on.
pub fn mask_output(def: &SchemaDef, raw: &Value, pii_masking: bool) -> Value {
    fn mask_scalar(f: &FieldDef, obj: &mut Value, pii: bool) {
        let Some(m) = obj.as_object_mut() else { return };
        for key in ["value", "evidence"] {
            if let Some(Value::String(s)) = m.get(key) {
                let mut out = if f.kind.is_pii() || f.mask { validate::mask_value(f, s) } else { s.clone() };
                if pii {
                    out = validate::mask_text(&out);
                }
                m.insert(key.into(), Value::String(out));
            }
        }
    }
    fn mask_all_strings(v: &mut Value) {
        match v {
            Value::String(s) => *s = validate::mask_text(s),
            Value::Array(a) => a.iter_mut().for_each(mask_all_strings),
            Value::Object(o) => o.values_mut().for_each(mask_all_strings),
            _ => {}
        }
    }
    let mut out = raw.clone();
    for f in &def.fields {
        match f.kind {
            FieldKind::Table => {
                if let Some(rows) = out.get_mut(&f.key).and_then(Value::as_array_mut) {
                    for row in rows.iter_mut() {
                        if let Some(m) = row.as_object_mut() {
                            for c in f.fields.iter().filter(|c| c.kind.is_pii() || c.mask) {
                                if let Some(Value::String(s)) = m.get(&c.key) {
                                    m.insert(c.key.clone(), Value::String(validate::mask_value(c, s)));
                                }
                            }
                        }
                    }
                }
            }
            FieldKind::Group => {
                if let Some(g) = out.get_mut(&f.key) {
                    for c in &f.fields {
                        if let Some(obj) = g.get_mut(&c.key) {
                            mask_scalar(c, obj, pii_masking);
                        }
                    }
                }
            }
            _ => {
                if let Some(obj) = out.get_mut(&f.key) {
                    mask_scalar(f, obj, pii_masking);
                }
            }
        }
    }
    if pii_masking {
        mask_all_strings(&mut out);
    }
    out
}

fn conf_of(obj: &Value) -> Option<u8> {
    obj.get("confidence").and_then(Value::as_i64).map(|c| c.clamp(0, 100) as u8)
}

/// Merges per-page outputs: scalars keep the most confident non-null value
/// (earliest page on ties) and remember its page; tables concatenate in page
/// order. Output keeps the model's shape plus a `page` member per scalar.
pub fn merge_pages(def: &SchemaDef, pages: &[(u32, Value)]) -> Value {
    let mut out = serde_json::Map::new();
    let pick = |path: &[&str]| -> Value {
        let mut best: Option<(u8, u32, Value)> = None;
        for (page_no, v) in pages {
            let mut cur = v;
            for p in path {
                match cur.get(*p) {
                    Some(c) => cur = c,
                    None => {
                        cur = &Value::Null;
                        break;
                    }
                }
            }
            if !cur.is_object() {
                continue;
            }
            let has_value = validate::value_text(cur.get("value").unwrap_or(&Value::Null)).is_some();
            if !has_value {
                continue;
            }
            let c = conf_of(cur).unwrap_or(70);
            if best.as_ref().is_none_or(|(bc, _, _)| c > *bc) {
                let mut obj = cur.clone();
                if let Some(m) = obj.as_object_mut() {
                    m.insert("page".into(), json!(page_no));
                }
                best = Some((c, *page_no, obj));
            }
        }
        best.map(|(_, _, v)| v).unwrap_or(json!({ "value": null, "evidence": null, "confidence": 0 }))
    };
    for f in &def.fields {
        match f.kind {
            FieldKind::Table => {
                let mut rows = Vec::new();
                for (page_no, v) in pages {
                    if let Some(arr) = v.get(&f.key).and_then(Value::as_array) {
                        for r in arr {
                            if r.as_object().is_some_and(|o| o.values().any(|x| validate::value_text(x).is_some())) {
                                let mut row = r.clone();
                                if let Some(m) = row.as_object_mut() {
                                    m.insert("page".into(), json!(page_no));
                                }
                                rows.push(row);
                            }
                        }
                    }
                }
                out.insert(f.key.clone(), Value::Array(rows));
            }
            FieldKind::Group => {
                let mut g = serde_json::Map::new();
                for c in &f.fields {
                    g.insert(c.key.clone(), pick(&[&f.key, &c.key]));
                }
                out.insert(f.key.clone(), Value::Object(g));
            }
            _ => {
                out.insert(f.key.clone(), pick(&[&f.key]));
            }
        }
    }
    Value::Object(out)
}

/// Settings that shape fields.
#[derive(Debug, Clone, Copy)]
pub struct FieldOptions {
    pub review_threshold: f64,
    pub audit_percent: u8,
    pub pii_masking: bool,
}

/// Result of `build_fields`.
#[derive(Debug, Default)]
pub struct Built {
    pub fields: Vec<NewField>,
    pub values: Values,
    /// Lowest confidence among fields that carry a value (None = nothing extracted).
    pub min_confidence: Option<f64>,
    pub review_count: u32,
    pub vendor: Option<String>,
    pub doc_date: Option<String>,
    pub total_amount: Option<f64>,
}

fn scalar_obj<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.is_object().then_some(cur)
}

fn evidence_lookup(texts: &BTreeMap<u32, String>, page: Option<u32>, needle: &str) -> (Option<bool>, Option<u32>) {
    if texts.is_empty() {
        return (None, page);
    }
    let order: Vec<u32> = page.into_iter().chain(texts.keys().copied().filter(|p| Some(*p) != page)).collect();
    for p in order {
        if let Some(t) = texts.get(&p) {
            if validate::find_evidence(t, needle).is_some() {
                return (Some(true), Some(p));
            }
        }
    }
    (Some(false), page)
}

fn value_lookup(def: &FieldDef, texts: &BTreeMap<u32, String>, page: Option<u32>, value: &str) -> Option<bool> {
    if texts.is_empty() {
        return None;
    }
    let order: Vec<u32> = page.into_iter().chain(texts.keys().copied().filter(|p| Some(*p) != page)).collect();
    Some(order.iter().any(|p| texts.get(p).is_some_and(|t| validate::value_in_text(def, value, t))))
}

/// Deterministic audit sample (§3.7.2 "고신뢰 필드 중 무작위 p% 감사"), drawn per
/// document so that `audit_sample_percent` is the share of documents that get one
/// extra look — not the share of fields, which would flag almost every document.
fn audit_pick(doc_id: &str, percent: u8, candidates: usize) -> Option<usize> {
    if percent == 0 || candidates == 0 {
        return None;
    }
    let h = blake3::hash(doc_id.as_bytes());
    let bytes = h.as_bytes();
    (bytes[0] % 100 < percent).then(|| (u16::from_le_bytes([bytes[1], bytes[2]]) as usize) % candidates)
}

fn build_scalar(
    def: &FieldDef,
    path: &str,
    obj: Option<&Value>,
    second: Option<&Value>,
    texts: &BTreeMap<u32, String>,
    opts: FieldOptions,
    doc_id: &str,
) -> NewField {
    let raw = obj.and_then(|o| validate::value_text(o.get("value").unwrap_or(&Value::Null)));
    let page = obj.and_then(|o| o.get("page")).and_then(Value::as_u64).map(|p| p as u32);
    let model_conf = obj.and_then(conf_of);
    let evidence_raw =
        obj.and_then(|o| o.get("evidence")).and_then(Value::as_str).map(str::trim).filter(|e| !e.is_empty());
    let mut f = NewField {
        path: path.to_string(),
        key: def.key.clone(),
        row: None,
        label_ko: def.label_ko.clone(),
        label_en: def.label_en.clone(),
        kind: def.kind,
        required: def.required,
        page,
        model_confidence: model_conf,
        ..Default::default()
    };
    let Some(raw) = raw else {
        f.confidence = 0.0;
        f.review_reason = def.required.then_some(ReviewReason::RequiredMissing);
        f.evidence = evidence_raw.map(|e| if opts.pii_masking { validate::mask_text(e) } else { e.to_string() });
        return f;
    };
    let (normalized, mut issues) = validate::validate_value(def, &raw);
    let (evidence_found, page_found) = match evidence_raw {
        Some(e) => evidence_lookup(texts, page, e),
        None => (None, page),
    };
    // PII values are looked up in their masked form (the page text is masked the same way).
    let stored = if opts.pii_masking || def.kind.is_pii() { validate::mask_value(def, &raw) } else { raw.clone() };
    let stored = if opts.pii_masking { validate::mask_text(&stored) } else { stored };
    let value_found = value_lookup(def, texts, page, &stored);
    if evidence_found.is_some() || value_found.is_some() {
        f.page = page_found.or(page);
    }
    let agreement = second.and_then(|s| {
        let other = validate::value_text(s.get("value").unwrap_or(&Value::Null))?;
        let (n2, _) = validate::validate_value(def, &other);
        Some(match (&normalized, n2) {
            (Some(a), Some(b)) => a == &b,
            _ => raw.trim() == other.trim(),
        })
    });
    let has_fail = issues.iter().any(|i| i.level == "fail");
    let has_warn = issues.iter().any(|i| i.level == "warn");
    f.confidence = validate::confidence(ConfidenceInput {
        model: model_conf,
        evidence_found,
        value_found,
        agreement,
        has_fail,
        has_warn,
    });
    f.value = Some(stored);
    f.normalized = normalized.map(|n| if opts.pii_masking { validate::mask_text(&n) } else { n });
    f.evidence = evidence_raw.map(|e| {
        let e = if def.kind.is_pii() || def.mask { validate::mask_value(def, e) } else { e.to_string() };
        if opts.pii_masking {
            validate::mask_text(&e)
        } else {
            e
        }
    });
    if has_fail {
        f.review_reason = Some(ReviewReason::ValidationFailed);
    } else if f.confidence < opts.review_threshold {
        f.review_reason = Some(ReviewReason::LowConfidence);
    }
    issues.retain(|i| i.level != "ok");
    f.validation = issues;
    let _ = doc_id;
    f
}

/// Turns the merged model output into field rows, running validation and the
/// cross-field rules; `second` is the precision-mode sample for agreement.
pub fn build_fields(
    schema: &SchemaRecord,
    merged: &Value,
    second: Option<&Value>,
    texts: &BTreeMap<u32, String>,
    opts: FieldOptions,
    doc_id: &str,
) -> Built {
    let def = &schema.definition;
    let mut fields: Vec<NewField> = Vec::new();
    let mut scalars: Vec<(String, Option<String>)> = Vec::new();
    let mut rows: Vec<(String, u32, String, Option<String>)> = Vec::new();
    for f in &def.fields {
        match f.kind {
            FieldKind::Table => {
                let arr = merged.get(&f.key).and_then(Value::as_array).cloned().unwrap_or_default();
                for (ri, row) in arr.iter().take(schema::MAX_ROWS).enumerate() {
                    let page = row.get("page").and_then(Value::as_u64).map(|p| p as u32);
                    // Row evidence is built from the *stored* (masked) cell values.
                    let row_text: Vec<String> = f
                        .fields
                        .iter()
                        .filter_map(|c| validate::value_text(row.get(&c.key).unwrap_or(&Value::Null)).map(|v| (c, v)))
                        .map(|(c, v)| {
                            let v = if opts.pii_masking || c.kind.is_pii() { validate::mask_value(c, &v) } else { v };
                            if opts.pii_masking {
                                validate::mask_text(&v)
                            } else {
                                v
                            }
                        })
                        .collect();
                    for c in &f.fields {
                        let path = format!("{}[{}].{}", f.key, ri, c.key);
                        let raw = validate::value_text(row.get(&c.key).unwrap_or(&Value::Null));
                        let mut nf = NewField {
                            path: path.clone(),
                            key: c.key.clone(),
                            row: Some(ri as u32),
                            label_ko: c.label_ko.clone(),
                            label_en: c.label_en.clone(),
                            kind: c.kind,
                            required: false,
                            page,
                            ..Default::default()
                        };
                        if let Some(raw) = raw {
                            let (normalized, mut issues) = validate::validate_value(c, &raw);
                            let value_found = value_lookup(c, texts, page, &raw);
                            let has_fail = issues.iter().any(|i| i.level == "fail");
                            nf.confidence = validate::confidence(ConfidenceInput {
                                model: Some(85),
                                evidence_found: None,
                                value_found,
                                agreement: None,
                                has_fail,
                                has_warn: issues.iter().any(|i| i.level == "warn"),
                            });
                            let stored = if opts.pii_masking || c.kind.is_pii() {
                                validate::mask_value(c, &raw)
                            } else {
                                raw.clone()
                            };
                            nf.value = Some(if opts.pii_masking { validate::mask_text(&stored) } else { stored });
                            let normalized =
                                normalized.map(|n| if opts.pii_masking { validate::mask_text(&n) } else { n });
                            nf.normalized = normalized.clone();
                            nf.evidence = Some(row_text.join(" | "));
                            if has_fail {
                                nf.review_reason = Some(ReviewReason::ValidationFailed);
                            } else if nf.confidence < opts.review_threshold {
                                nf.review_reason = Some(ReviewReason::LowConfidence);
                            }
                            issues.retain(|i| i.level != "ok");
                            nf.validation = issues;
                            rows.push((f.key.clone(), ri as u32, c.key.clone(), normalized));
                        } else {
                            rows.push((f.key.clone(), ri as u32, c.key.clone(), None));
                        }
                        fields.push(nf);
                    }
                }
            }
            FieldKind::Group => {
                for c in &f.fields {
                    let path = format!("{}.{}", f.key, c.key);
                    let obj = scalar_obj(merged, &[&f.key, &c.key]);
                    let sec = second.and_then(|s| scalar_obj(s, &[&f.key, &c.key]));
                    let nf = build_scalar(c, &path, obj, sec, texts, opts, doc_id);
                    scalars.push((path, nf.normalized.clone()));
                    fields.push(nf);
                }
            }
            _ => {
                let obj = scalar_obj(merged, &[&f.key]);
                let sec = second.and_then(|s| scalar_obj(s, &[&f.key]));
                let nf = build_scalar(f, &f.key, obj, sec, texts, opts, doc_id);
                scalars.push((f.key.clone(), nf.normalized.clone()));
                fields.push(nf);
            }
        }
    }
    let values = validate::values_from(&scalars, &rows);
    // Cross-field rules: the first path of a failing rule is the anchor that goes to review.
    for (path, issue) in validate::document_rules(def, &values) {
        if let Some(f) = fields.iter_mut().find(|f| f.path == path) {
            if issue.level == "ok" {
                continue;
            }
            let fail = issue.level == "fail";
            f.validation.push(ValidationIssue {
                rule: issue.rule.clone(),
                level: issue.level.clone(),
                message: issue.message.clone(),
            });
            if fail {
                f.confidence = f.confidence.min(0.4);
                f.review_reason = Some(ReviewReason::ValidationFailed);
            } else {
                f.confidence = (f.confidence * 0.9 * 1000.0).round() / 1000.0;
                if f.review_reason.is_none() && f.confidence < opts.review_threshold {
                    f.review_reason = Some(ReviewReason::LowConfidence);
                }
            }
        }
    }
    // Audit sampling: one clean field of a sampled document gets a human look.
    let candidates: Vec<usize> = fields
        .iter()
        .enumerate()
        .filter(|(_, f)| f.row.is_none() && f.value.is_some() && f.review_reason.is_none())
        .map(|(i, _)| i)
        .collect();
    if let Some(pick) = audit_pick(doc_id, opts.audit_percent, candidates.len()) {
        fields[candidates[pick]].review_reason = Some(ReviewReason::Audit);
    }
    let with_value: Vec<&NewField> = fields.iter().filter(|f| f.value.is_some()).collect();
    let min_confidence =
        with_value.iter().map(|f| f.confidence).fold(None, |m: Option<f64>, c| Some(m.map_or(c, |x| x.min(c))));
    let review_count = fields.iter().filter(|f| f.review_reason.is_some()).count() as u32;
    let text_of = |paths: &[&str]| paths.iter().find_map(|p| values.text(p).map(str::to_string));
    let vendor = text_of(&["supplier.name", "store_name", "company_name", "name"])
        .or_else(|| values.tables.get("parties").and_then(|r| r.first()).and_then(|r| r.get("name")).cloned());
    let doc_date = text_of(&["issue_date", "datetime", "contract_date", "application_date", "date", "opening_date"]);
    let total_amount = ["grand_total", "total", "amount", "supply_total"].iter().find_map(|p| values.amount(p));
    Built { fields, values, min_confidence, review_count, vendor, doc_date, total_amount }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn receipt_schema() -> SchemaRecord {
        let b = schema::builtins().into_iter().find(|b| b.key == "receipt").unwrap();
        SchemaRecord {
            id: "builtin-receipt".into(),
            name: b.name_ko.into(),
            name_en: Some(b.name_en.into()),
            doc_type: "receipt".into(),
            source: schema::SchemaSource::Builtin,
            version: 1,
            description_ko: None,
            description_en: None,
            definition: b.def,
            form_document_id: None,
            builtin_key: Some("receipt".into()),
            enabled: true,
            is_default: true,
            use_count: 0,
            note: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn sv(v: &str, e: &str, c: u8) -> Value {
        json!({ "value": v, "evidence": e, "confidence": c })
    }

    #[test]
    fn merge_prefers_confident_values_and_concatenates_rows() {
        let s = receipt_schema();
        let p1 = json!({ "store_name": sv("스타벅스", "스타벅스 역삼점", 60), "total": sv("12,500", "합계 12,500", 95), "items": [{"name": "아메리카노", "quantity": "1", "unit_price": "4,500", "amount": "4,500"}] });
        let p2 = json!({ "store_name": sv("스타벅스 역삼점", "스타벅스 역삼점", 90), "total": { "value": null, "evidence": null, "confidence": 0 }, "items": [{"name": "라떼", "quantity": "2", "unit_price": "4,000", "amount": "8,000"}, {"name": null, "quantity": null, "unit_price": null, "amount": null}] });
        let m = merge_pages(&s.definition, &[(1, p1), (2, p2)]);
        assert_eq!(m["store_name"]["value"], "스타벅스 역삼점");
        assert_eq!(m["store_name"]["page"], 2);
        assert_eq!(m["total"]["value"], "12,500");
        assert_eq!(m["total"]["page"], 1);
        assert_eq!(m["items"].as_array().unwrap().len(), 2, "empty rows are dropped");
        assert_eq!(m["items"][1]["page"], 2);
    }

    #[test]
    fn build_fields_validates_masks_scores_and_flags_review() {
        let s = receipt_schema();
        let merged = json!({
            "store_name": sv("한빛상사", "한빛상사", 92),
            "brn": sv("220-81-62518", "사업자 220-81-62518", 90),
            "datetime": sv("2026.10.14 12:31", "2026.10.14 12:31", 90),
            "total": sv("12,500원", "합계 12,500원", 88),
            "supply_amount": sv("10,000", "공급가액 10,000", 80),
            "vat": sv("1,000", "부가세 1,000", 55),
            "cash_receipt_id": sv("010-1234-5678", "식별 010-1234-5678", 90),
            "approval_no": sv("12345678", "승인 12345678", 90),
            "items": [{"name": "아메리카노", "quantity": "2", "unit_price": "4,500", "amount": "9,000", "page": 1}, {"name": "쿠키", "quantity": "1", "unit_price": "3,500", "amount": "3,500", "page": 1}]
        });
        let mut texts = BTreeMap::new();
        texts.insert(1u32, "한빛상사\n사업자 220-81-62518\n2026.10.14 12:31\n아메리카노 2 4,500 9,000\n쿠키 1 3,500 3,500\n공급가액 10,000\n부가세 1,000\n합계 12,500원\n주민 900101-1234567".to_string());
        let built = build_fields(
            &s,
            &merged,
            None,
            &texts,
            FieldOptions { review_threshold: 0.8, audit_percent: 0, pii_masking: true },
            "doc1",
        );
        let f = |p: &str| built.fields.iter().find(|f| f.path == p).unwrap();
        assert_eq!(f("brn").normalized.as_deref(), Some("220-81-62518"));
        assert_eq!(f("brn").review_reason, Some(ReviewReason::ValidationFailed), "checksum fails");
        assert_eq!(f("datetime").normalized.as_deref(), Some("2026-10-14"));
        assert_eq!(f("total").normalized.as_deref(), Some("12500"));
        assert_eq!(f("cash_receipt_id").value.as_deref(), Some("***-****-5678"));
        assert_eq!(f("vat").review_reason, Some(ReviewReason::LowConfidence));
        assert_eq!(f("store_name").review_reason, None);
        assert!(f("store_name").confidence >= 0.9);
        // receipt_parts_equal_total: 10,000 + 1,000 ≠ 12,500 → total fails.
        assert!(f("total").validation.iter().any(|i| i.rule == schema::RULE_RECEIPT_TOTAL && i.level == "fail"));
        assert_eq!(f("total").review_reason, Some(ReviewReason::ValidationFailed));
        assert_eq!(f("items[0].amount").normalized.as_deref(), Some("9000"));
        assert_eq!(f("items[1].name").row, Some(1));
        assert_eq!(f("merchant_no").value, None);
        assert_eq!(f("merchant_no").review_reason, None, "optional missing fields are not queued");
        assert_eq!(built.vendor.as_deref(), Some("한빛상사"));
        assert_eq!(built.doc_date.as_deref(), Some("2026-10-14"));
        assert_eq!(built.total_amount, Some(12_500.0));
        assert!(built.review_count >= 3);
        assert!(built.min_confidence.unwrap() <= 0.4);
    }

    #[test]
    fn precision_mode_agreement_and_required_missing() {
        let s = receipt_schema();
        let merged = json!({ "store_name": sv("한빛", "한빛", 80), "total": sv("1,000", "1,000", 80), "datetime": { "value": null, "evidence": null, "confidence": 0 } });
        let second = json!({ "store_name": sv("한빛", "한빛", 80), "total": sv("1,200", "1,200", 80) });
        let built = build_fields(
            &s,
            &merged,
            Some(&second),
            &BTreeMap::new(),
            FieldOptions { review_threshold: 0.8, audit_percent: 100, pii_masking: true },
            "d",
        );
        let f = |p: &str| built.fields.iter().find(|f| f.path == p).unwrap();
        assert!(f("store_name").confidence > f("total").confidence);
        assert!(f("total").confidence <= 0.5);
        assert_eq!(f("datetime").review_reason, Some(ReviewReason::RequiredMissing));
        // Audit sampling is per document: exactly one clean field of a sampled document.
        let audits = built.fields.iter().filter(|f| f.review_reason == Some(ReviewReason::Audit)).count();
        assert_eq!(audits, 1, "{:?}", built.fields.iter().map(|f| (&f.path, f.review_reason)).collect::<Vec<_>>());
        let none = build_fields(
            &s,
            &merged,
            Some(&second),
            &BTreeMap::new(),
            FieldOptions { review_threshold: 0.8, audit_percent: 0, pii_masking: true },
            "d",
        );
        assert_eq!(none.fields.iter().filter(|f| f.review_reason == Some(ReviewReason::Audit)).count(), 0);
    }

    #[test]
    fn context_overflow_shortens_the_text_anchor() {
        use super::super::vlm::testing::StageChat;
        let s = receipt_schema();
        let chat = StageChat::new();
        let overflow =
            "/v1/chat/completions: HTTP 400: request (6272 tokens) exceeds the available context size (4096 tokens)";
        chat.fail_queue.lock().unwrap().extend([overflow.to_string(), overflow.to_string()]);
        chat.push("kie_receipt", json!({ "total": sv("1,000", "합계 1,000", 90) }));
        let vlm = Vlm::new(&chat, "m".into(), crate::settings::Profile::Balanced, None);
        let text = "가".repeat(4_000);
        let page = PageInput { page_no: 1, page_count: 1, image_url: None, text: Some(&text) };
        let (v, _) = extract_page(&vlm, &s, &page, None).unwrap();
        assert_eq!(v["total"]["value"], "1,000");
        let prompts: Vec<usize> = chat
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.messages[1]["content"].as_str().unwrap().chars().count())
            .collect();
        assert_eq!(prompts.len(), 3);
        assert!(prompts[0] > prompts[1] && prompts[1] > prompts[2], "{prompts:?}");
        assert!(prompts[2] < prompts[0] - 2_000, "the third try carries at most 1,000 text chars: {prompts:?}");

        // With the text gone the overflow is final.
        let chat = StageChat::new();
        *chat.fail_with.lock().unwrap() = Some(overflow.into());
        let vlm = Vlm::new(&chat, "m".into(), crate::settings::Profile::Balanced, None);
        let err = extract_page(&vlm, &s, &page, None).unwrap_err();
        assert!(context_overflow(&err));
        assert_eq!(chat.requests.lock().unwrap().len(), 4, "4000 → 2000 → 1000 → none");
    }

    #[test]
    fn pii_never_survives_in_raw_output_or_row_evidence() {
        let s = receipt_schema();
        let merged = json!({
            "store_name": sv("한빛", "담당 900101-1234567 한빛", 90),
            "cash_receipt_id": sv("010-1234-5678", "식별 010-1234-5678", 90),
            "items": [{ "name": "홍길동 850315-2345678", "quantity": "1", "unit_price": "1,000", "amount": "1,000" }]
        });
        let masked = mask_output(&s.definition, &merged, true);
        let text = masked.to_string();
        assert!(!text.contains("1234567") && !text.contains("2345678") && !text.contains("010-1234"), "{text}");
        assert_eq!(masked["cash_receipt_id"]["value"], "***-****-5678");
        assert_eq!(masked["store_name"]["evidence"], "담당 900101-1****** 한빛");
        let built = build_fields(
            &s,
            &merged,
            None,
            &BTreeMap::new(),
            FieldOptions { review_threshold: 0.5, audit_percent: 0, pii_masking: true },
            "d",
        );
        for f in &built.fields {
            for v in [&f.value, &f.evidence, &f.normalized].into_iter().flatten() {
                assert!(!v.contains("1234567") && !v.contains("2345678"), "{}: {v}", f.path);
            }
        }
        assert_eq!(
            built.fields.iter().find(|f| f.path == "items[0].quantity").unwrap().evidence.as_deref(),
            Some("홍길동 850315-2****** | 1 | 1,000 | 1,000")
        );
    }
}
