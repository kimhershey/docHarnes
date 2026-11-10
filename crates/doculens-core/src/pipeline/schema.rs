//! Extraction schemas (design §3.4.0 / §3.4.1 / §3.5): field definitions that
//! drive the KIE prompt, the llama.cpp JSON-schema grammar, validation and the
//! review grid. Seven sample forms are built in; users derive new ones from a
//! registered form (`forms`) or describe them in natural language.
//!
//! Grammar rules (llama.cpp JSON Schema → GBNF, §3.4.1 lint): nullable types
//! as `["string","null"]`, no `$ref`/`$defs` (groups and line items are
//! inlined), regex hints anchored `^…$`, `confidence` as an integer 0–100,
//! `additionalProperties: false` everywhere.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Bumped when a built-in sample form changes; stored rows are refreshed on open.
pub const BUILTIN_VERSION: u32 = 2;
/// Hard cap on fields per schema (prompt size / grammar size).
pub const MAX_FIELDS: usize = 80;
pub const MAX_COLUMNS: usize = 20;
pub const MAX_ROWS: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    #[default]
    String,
    /// Money / quantity printed with separators ("1,250,000원"); normalised to a plain number.
    Amount,
    Integer,
    /// Any printed date form; normalised to ISO-8601.
    Date,
    Boolean,
    /// One of `options`.
    Enum,
    /// 사업자등록번호 (checksum validated).
    Brn,
    /// 주민등록번호 / 외국인등록번호 — masked on extraction, never stored in full (§3.8).
    Rrn,
    /// Bank account / cash-receipt id — masked to the last four digits.
    Account,
    Phone,
    Email,
    /// Repeated rows (line items) with `fields` as columns.
    Table,
    /// Nested group of scalar `fields` (inline object).
    Group,
}

impl FieldKind {
    pub const SCALAR: [FieldKind; 11] = [
        FieldKind::String,
        FieldKind::Amount,
        FieldKind::Integer,
        FieldKind::Date,
        FieldKind::Boolean,
        FieldKind::Enum,
        FieldKind::Brn,
        FieldKind::Rrn,
        FieldKind::Account,
        FieldKind::Phone,
        FieldKind::Email,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FieldKind::String => "string",
            FieldKind::Amount => "amount",
            FieldKind::Integer => "integer",
            FieldKind::Date => "date",
            FieldKind::Boolean => "boolean",
            FieldKind::Enum => "enum",
            FieldKind::Brn => "brn",
            FieldKind::Rrn => "rrn",
            FieldKind::Account => "account",
            FieldKind::Phone => "phone",
            FieldKind::Email => "email",
            FieldKind::Table => "table",
            FieldKind::Group => "group",
        }
    }

    pub fn parse(s: &str) -> Option<FieldKind> {
        [FieldKind::Table, FieldKind::Group].into_iter().chain(FieldKind::SCALAR).find(|k| k.as_str() == s)
    }

    pub fn is_scalar(self) -> bool {
        !matches!(self, FieldKind::Table | FieldKind::Group)
    }

    /// Values of this kind are masked before they are stored (§3.8).
    pub fn is_pii(self) -> bool {
        matches!(self, FieldKind::Rrn | FieldKind::Account)
    }

    /// Human label used in prompts / the grid.
    pub fn label(self, ko: bool) -> &'static str {
        match (self, ko) {
            (FieldKind::String, true) => "문자열",
            (FieldKind::String, false) => "text",
            (FieldKind::Amount, true) => "금액",
            (FieldKind::Amount, false) => "amount",
            (FieldKind::Integer, true) => "정수",
            (FieldKind::Integer, false) => "integer",
            (FieldKind::Date, true) => "날짜",
            (FieldKind::Date, false) => "date",
            (FieldKind::Boolean, true) => "예/아니오",
            (FieldKind::Boolean, false) => "yes/no",
            (FieldKind::Enum, true) => "선택",
            (FieldKind::Enum, false) => "choice",
            (FieldKind::Brn, true) => "사업자등록번호",
            (FieldKind::Brn, false) => "business registration no.",
            (FieldKind::Rrn, true) => "주민등록번호(마스킹)",
            (FieldKind::Rrn, false) => "resident registration no. (masked)",
            (FieldKind::Account, true) => "계좌/식별번호(마스킹)",
            (FieldKind::Account, false) => "account no. (masked)",
            (FieldKind::Phone, true) => "전화번호",
            (FieldKind::Phone, false) => "phone",
            (FieldKind::Email, true) => "이메일",
            (FieldKind::Email, false) => "email",
            (FieldKind::Table, true) => "표(반복 행)",
            (FieldKind::Table, false) => "table (rows)",
            (FieldKind::Group, true) => "그룹",
            (FieldKind::Group, false) => "group",
        }
    }
}

/// One field of a schema. Keys are ASCII snake_case (`supplier_brn`); the
/// Korean / English labels travel with the field so prompts, the grid and
/// exports never need a lookup table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FieldDef {
    pub key: String,
    pub label_ko: String,
    pub label_en: String,
    pub kind: FieldKind,
    pub required: bool,
    /// Anchored regex (`^…$`) the value should match (prompt hint + validator).
    pub regex_hint: Option<String>,
    pub example: Option<String>,
    /// Extra instruction for the model (where the value is printed, how to read it).
    pub description: Option<String>,
    /// Allowed values of an `Enum` field.
    pub options: Vec<String>,
    /// Columns of a `Table`, or members of a `Group` (scalar kinds only).
    pub fields: Vec<FieldDef>,
    /// Mask the stored value (dates → year only, other text → last four characters).
    pub mask: bool,
}

impl FieldDef {
    pub fn new(key: &str, label_ko: &str, label_en: &str, kind: FieldKind) -> FieldDef {
        FieldDef { key: key.into(), label_ko: label_ko.into(), label_en: label_en.into(), kind, ..Default::default() }
    }
    pub fn required(mut self) -> FieldDef {
        self.required = true;
        self
    }
    pub fn example(mut self, e: &str) -> FieldDef {
        self.example = Some(e.into());
        self
    }
    pub fn regex(mut self, r: &str) -> FieldDef {
        self.regex_hint = Some(r.into());
        self
    }
    pub fn describe(mut self, d: &str) -> FieldDef {
        self.description = Some(d.into());
        self
    }
    pub fn options(mut self, o: &[&str]) -> FieldDef {
        self.kind = FieldKind::Enum;
        self.options = o.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn masked(mut self) -> FieldDef {
        self.mask = true;
        self
    }
    pub fn with(mut self, fields: Vec<FieldDef>) -> FieldDef {
        self.fields = fields;
        self
    }
    pub fn label(&self, ko: bool) -> &str {
        if ko || self.label_en.is_empty() {
            &self.label_ko
        } else {
            &self.label_en
        }
    }
}

/// Cross-field rule ids evaluated by `validate::document_rules`.
pub const RULE_SUPPLY_TAX_TOTAL: &str = "supply_plus_tax_equals_total";
pub const RULE_ITEMS_SUM_SUPPLY: &str = "items_sum_equals_supply";
pub const RULE_TAX_RATE: &str = "tax_is_ten_percent";
pub const RULE_ZERO_RATE: &str = "zero_rate_tax_is_zero";
pub const RULE_RECEIPT_TOTAL: &str = "receipt_parts_equal_total";
pub const RULE_ITEMS_SUM_TOTAL: &str = "items_sum_equals_total";
pub const RULE_PERIOD_ORDER: &str = "period_start_before_end";
pub const RULE_EXEMPT_TOTAL: &str = "exempt_total_equals_supply";
/// Percentages (`*_percent`, `*_rate`, table column `progress`) lie within 0–100.
pub const RULE_PERCENT_RANGE: &str = "percent_range";
/// Test result counts add up: total_cases = passed + failed + blocked.
pub const RULE_TEST_COUNTS: &str = "test_counts_sum";

/// The editable part of a schema (`schemas.definition` JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SchemaDef {
    pub fields: Vec<FieldDef>,
    /// Cross-field rule ids (see `RULE_*`).
    pub rules: Vec<String>,
}

impl SchemaDef {
    pub fn new(fields: Vec<FieldDef>, rules: &[&str]) -> SchemaDef {
        SchemaDef { fields, rules: rules.iter().map(|r| r.to_string()).collect() }
    }

    /// Flat list of `(path, field)` for scalar fields, including group members
    /// (`supplier.brn`). Tables are listed once under their own key.
    pub fn paths(&self) -> Vec<(String, &FieldDef)> {
        let mut out = Vec::new();
        for f in &self.fields {
            match f.kind {
                FieldKind::Group => {
                    for c in &f.fields {
                        out.push((format!("{}.{}", f.key, c.key), c));
                    }
                }
                _ => out.push((f.key.clone(), f)),
            }
        }
        out
    }

    pub fn field(&self, path: &str) -> Option<&FieldDef> {
        self.paths().into_iter().find(|(p, _)| p == path).map(|(_, f)| f)
    }

    pub fn table(&self, key: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.key == key && f.kind == FieldKind::Table)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SchemaSource {
    Builtin,
    Nl,
    #[default]
    User,
    Form,
}

impl SchemaSource {
    pub const ALL: [SchemaSource; 4] =
        [SchemaSource::Builtin, SchemaSource::Nl, SchemaSource::User, SchemaSource::Form];
    pub fn as_str(self) -> &'static str {
        match self {
            SchemaSource::Builtin => "builtin",
            SchemaSource::Nl => "nl",
            SchemaSource::User => "user",
            SchemaSource::Form => "form",
        }
    }
    pub fn parse(s: &str) -> Option<SchemaSource> {
        SchemaSource::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// One row of `schemas`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaRecord {
    pub id: String,
    pub name: String,
    pub name_en: Option<String>,
    pub doc_type: String,
    pub source: SchemaSource,
    pub version: u32,
    pub description_ko: Option<String>,
    pub description_en: Option<String>,
    pub definition: SchemaDef,
    pub form_document_id: Option<String>,
    pub builtin_key: Option<String>,
    pub enabled: bool,
    pub is_default: bool,
    pub use_count: u64,
    /// Change note of the current version (older notes live in `schema_versions`).
    #[serde(default)]
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SchemaRecord {
    pub fn display_name(&self, ko: bool) -> &str {
        match (&self.name_en, ko) {
            (Some(en), false) if !en.is_empty() => en,
            _ => &self.name,
        }
    }
}

/// Unsaved schema (form registration result, NL generation result, editor state).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SchemaDraft {
    /// Existing schema to update (new version), or `None` for a new schema.
    pub id: Option<String>,
    pub name: String,
    pub name_en: Option<String>,
    pub doc_type: String,
    pub source: SchemaSource,
    pub description_ko: Option<String>,
    pub description_en: Option<String>,
    pub definition: SchemaDef,
    pub form_document_id: Option<String>,
    pub note: Option<String>,
    /// Further sample documents of a multi-document discovery (0.3.0 §2); `register` marks
    /// them as forms of the type like `form_document_id`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_document_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// Slugs / keys
// ---------------------------------------------------------------------------

/// `true` for an ASCII snake_case identifier (`[a-z][a-z0-9_]*`, ≤ 40 chars).
pub fn is_key(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 40
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// Best-effort key from a label: ASCII letters/digits lower-cased, runs of
/// anything else collapsed to `_`; non-ASCII labels get a dictionary hit or
/// the `fallback`.
pub fn key_from_label(label: &str, fallback: &str) -> String {
    if let Some(k) = known_key(label) {
        return k.to_string();
    }
    if let Some(k) = suffix_key(label) {
        return k.to_string();
    }
    let mut out = String::new();
    let mut last_sep = true;
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_sep = false;
        } else if !last_sep {
            out.push('_');
            last_sep = true;
        }
    }
    let out = out.trim_matches('_').to_string();
    let out = if out.chars().next().is_some_and(|c| c.is_ascii_digit()) { format!("f_{out}") } else { out };
    if is_key(&out) {
        out
    } else {
        fallback.to_string()
    }
}

/// Common Korean form labels → canonical keys (form derivation without a model).
pub fn known_key(label: &str) -> Option<&'static str> {
    let l = label.trim().trim_end_matches([':', '：']).replace([' ', '\u{a0}'], "");
    const TABLE: &[(&str, &str)] = &[
        ("상호", "company_name"),
        ("상호(법인명)", "company_name"),
        ("법인명", "company_name"),
        ("회사명", "company_name"),
        ("업체명", "company_name"),
        ("사업자등록번호", "brn"),
        ("사업자번호", "brn"),
        ("등록번호", "registration_no"),
        ("법인등록번호", "corporate_no"),
        ("대표자", "representative"),
        ("대표자명", "representative"),
        ("대표이사", "representative"),
        ("성명", "name"),
        ("이름", "name"),
        ("담당자", "contact_person"),
        ("주소", "address"),
        ("사업장소재지", "business_address"),
        ("사업장주소", "business_address"),
        ("본점소재지", "head_office_address"),
        ("업태", "business_type"),
        ("종목", "business_item"),
        ("생년월일", "birth_date"),
        ("주민등록번호", "rrn"),
        ("연락처", "phone"),
        ("전화번호", "phone"),
        ("전화", "phone"),
        ("휴대전화", "mobile"),
        ("핸드폰", "mobile"),
        ("팩스", "fax"),
        ("이메일", "email"),
        ("e-mail", "email"),
        ("날짜", "date"),
        ("일자", "date"),
        ("작성일", "issue_date"),
        ("작성일자", "issue_date"),
        ("발행일", "issue_date"),
        ("발급일", "issue_date"),
        ("신청일", "application_date"),
        ("계약일", "contract_date"),
        ("계약기간", "contract_period"),
        ("유효기간", "valid_until"),
        ("납기", "due_date"),
        ("납품일", "delivery_date"),
        ("금액", "amount"),
        ("합계", "total"),
        ("합계금액", "total"),
        ("총액", "total"),
        ("공급가액", "supply_amount"),
        ("세액", "tax_amount"),
        ("부가세", "vat"),
        ("부가가치세", "vat"),
        ("수량", "quantity"),
        ("단가", "unit_price"),
        ("품명", "item_name"),
        ("품목", "item_name"),
        ("규격", "spec"),
        ("비고", "note"),
        ("계약명", "contract_title"),
        ("계약금액", "contract_amount"),
        ("제목", "title"),
        ("건명", "title"),
        ("서식명", "form_title"),
        ("수신", "recipient"),
        ("참조", "cc"),
        ("발신", "sender"),
        ("문서번호", "document_no"),
        ("번호", "number"),
        ("부서", "department"),
        ("직위", "position"),
        ("직급", "position"),
        ("승인번호", "approval_no"),
        ("결제수단", "payment_method"),
        ("계좌번호", "account_no"),
        ("은행", "bank"),
        ("예금주", "account_holder"),
        ("서명", "signature"),
        ("날인", "seal"),
    ];
    TABLE.iter().find(|(k, _)| *k == l).map(|(_, v)| *v)
}

/// Keys for labels that end in a well-known noun (`견적일자` → `date`, `계약번호` → `number`).
fn suffix_key(label: &str) -> Option<&'static str> {
    let l = label.trim().trim_end_matches([':', '：']).replace([' ', '\u{a0}'], "");
    const SUFFIXES: &[(&str, &str)] = &[
        ("일자", "date"),
        ("날짜", "date"),
        ("연월일", "date"),
        ("년월일", "date"),
        ("금액", "amount"),
        ("가액", "amount"),
        ("단가", "unit_price"),
        ("수량", "quantity"),
        ("번호", "number"),
        ("기간", "period"),
        ("주소", "address"),
        ("소재지", "address"),
        ("연락처", "phone"),
        ("전화", "phone"),
        ("이메일", "email"),
        ("성명", "name"),
        ("이름", "name"),
        ("명", "name"),
        ("여부", "flag"),
        ("사유", "reason"),
        ("비고", "note"),
    ];
    if l.is_ascii() {
        return None;
    }
    SUFFIXES.iter().find(|(suf, _)| l.ends_with(suf)).map(|(_, k)| *k)
}

/// Field kind guessed from a label (form derivation without a model).
pub fn kind_from_label(label: &str) -> FieldKind {
    let l = label.replace(' ', "");
    if l.contains("주민") || l.contains("외국인등록") {
        FieldKind::Rrn
    } else if l.contains("사업자등록번호") || l.contains("사업자번호") {
        FieldKind::Brn
    } else if l.contains("계좌") {
        FieldKind::Account
    } else if l.contains("이메일") || l.to_ascii_lowercase().contains("mail") {
        FieldKind::Email
    } else if l.contains("전화") || l.contains("연락처") || l.contains("휴대") || l.contains("팩스") {
        FieldKind::Phone
    } else if l.contains("수량") || l.contains("인원") || l.contains("개월") {
        FieldKind::Integer
    } else if l.contains("금액")
        || l.contains("가액")
        || l.contains("합계")
        || l.contains("단가")
        || l.contains("세액")
        || l.contains("총액")
        || l.contains("부가세")
        || l.contains("비용")
    {
        FieldKind::Amount
    } else if l.contains("일자")
        || l.contains("날짜")
        || (l.ends_with('일') && l.chars().count() <= 6)
        || l.contains("년월일")
        || l.contains("연월일")
        || l.contains("기간")
        || l.contains("일시")
    {
        FieldKind::Date
    } else if l.contains("여부") {
        FieldKind::Boolean
    } else {
        FieldKind::String
    }
}

// ---------------------------------------------------------------------------
// JSON Schema (llama.cpp grammar)
// ---------------------------------------------------------------------------

fn value_type(f: &FieldDef) -> Value {
    match f.kind {
        FieldKind::Boolean => json!({ "type": ["boolean", "null"] }),
        FieldKind::Enum if !f.options.is_empty() => {
            let mut opts: Vec<Value> = f.options.iter().map(|o| Value::String(o.clone())).collect();
            opts.push(Value::Null);
            json!({ "enum": opts })
        }
        // Numbers are copied as printed (commas included) and normalised later (§3.4 "2단계 추출 원칙").
        _ => json!({ "type": ["string", "null"] }),
    }
}

/// `{value, evidence, confidence}` object of one scalar field.
fn scalar_schema(f: &FieldDef) -> Value {
    json!({
        "type": "object",
        "properties": {
            "value": value_type(f),
            "evidence": { "type": ["string", "null"] },
            "confidence": { "type": "integer", "minimum": 0, "maximum": 100 }
        },
        "required": ["value", "evidence", "confidence"],
        "additionalProperties": false
    })
}

fn table_schema(f: &FieldDef) -> Value {
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();
    for c in f.fields.iter().filter(|c| c.kind.is_scalar()) {
        props.insert(c.key.clone(), value_type(c));
        required.push(Value::String(c.key.clone()));
    }
    json!({
        "type": "array",
        "maxItems": MAX_ROWS,
        "items": { "type": "object", "properties": props, "required": required, "additionalProperties": false }
    })
}

fn group_schema(f: &FieldDef) -> Value {
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();
    for c in f.fields.iter().filter(|c| c.kind.is_scalar()) {
        props.insert(c.key.clone(), scalar_schema(c));
        required.push(Value::String(c.key.clone()));
    }
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

/// JSON Schema for the KIE `response_format` (every key required, nullable values).
pub fn json_schema(def: &SchemaDef) -> Value {
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();
    for f in &def.fields {
        let v = match f.kind {
            FieldKind::Table => table_schema(f),
            FieldKind::Group => group_schema(f),
            _ => scalar_schema(f),
        };
        props.insert(f.key.clone(), v);
        required.push(Value::String(f.key.clone()));
    }
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

/// Schema of the classifier reply (`doc_types` = enabled schema slugs + `other`).
pub fn classify_schema(doc_types: &[String]) -> Value {
    let mut types: Vec<Value> = doc_types.iter().map(|s| Value::String(s.clone())).collect();
    if !doc_types.iter().any(|t| t == "other") {
        types.push(Value::String("other".into()));
    }
    json!({
        "type": "object",
        "properties": {
            "doc_type": { "enum": types },
            "rotation": { "enum": [0, 90, 180, 270] },
            "is_blank": { "type": "boolean" },
            "language": { "enum": ["ko", "en", "mixed"] }
        },
        "required": ["doc_type", "rotation", "is_blank", "language"],
        "additionalProperties": false
    })
}

/// Schema of the NL / form field-discovery reply (metaschema, §3.5).
pub fn metaschema() -> Value {
    let kinds: Vec<Value> = FieldKind::SCALAR.iter().map(|k| Value::String(k.as_str().into())).collect();
    let mut field_kinds = kinds.clone();
    field_kinds.push(Value::String("table".into()));
    let column = json!({
        "type": "object",
        "properties": {
            "key": { "type": "string", "pattern": "^[a-z][a-z0-9_]{0,39}$" },
            "label_ko": { "type": "string" },
            "label_en": { "type": "string" },
            "type": { "enum": kinds }
        },
        "required": ["key", "label_ko", "label_en", "type"],
        "additionalProperties": false
    });
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "name_en": { "type": "string" },
            "doc_type": { "type": "string", "pattern": "^[a-z][a-z0-9_]{1,30}$" },
            "description_ko": { "type": "string" },
            "description_en": { "type": "string" },
            "fields": {
                "type": "array",
                "maxItems": MAX_FIELDS,
                "items": {
                    "type": "object",
                    "properties": {
                        "key": { "type": "string", "pattern": "^[a-z][a-z0-9_]{0,39}$" },
                        "label_ko": { "type": "string" },
                        "label_en": { "type": "string" },
                        "type": { "enum": field_kinds },
                        "required": { "type": "boolean" },
                        "regex_hint": { "type": ["string", "null"] },
                        "example": { "type": ["string", "null"] },
                        "options": { "type": "array", "maxItems": 12, "items": { "type": "string" } },
                        "columns": { "type": "array", "maxItems": MAX_COLUMNS, "items": column }
                    },
                    "required": ["key", "label_ko", "label_en", "type", "required", "regex_hint", "example", "options", "columns"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["name", "name_en", "doc_type", "description_ko", "description_en", "fields"],
        "additionalProperties": false
    })
}

/// Parses a metaschema reply into a draft (keys sanitised, duplicates suffixed).
pub fn draft_from_metaschema(v: &Value, source: SchemaSource) -> SchemaDraft {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let mut fields = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let unique = |key: String, seen: &mut Vec<String>| {
        let mut k = key;
        let mut n = 2;
        let base = k.clone();
        while seen.contains(&k) {
            k = format!("{base}_{n}");
            n += 1;
        }
        seen.push(k.clone());
        k
    };
    for (i, f) in v.get("fields").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let label_ko = f.get("label_ko").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let label_en = f.get("label_en").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let raw_key = f.get("key").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let key = if is_key(&raw_key) {
            raw_key
        } else {
            key_from_label(if label_en.is_empty() { &label_ko } else { &label_en }, &format!("field_{}", i + 1))
        };
        let key = unique(key, &mut seen);
        let kind = f.get("type").and_then(Value::as_str).and_then(FieldKind::parse).unwrap_or_default();
        let mut columns = Vec::new();
        if kind == FieldKind::Table {
            let mut cseen: Vec<String> = Vec::new();
            for (j, c) in f.get("columns").and_then(Value::as_array).into_iter().flatten().enumerate() {
                let cl_ko = c.get("label_ko").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let cl_en = c.get("label_en").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let ck = c.get("key").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let mut ck = if is_key(&ck) {
                    ck
                } else {
                    key_from_label(if cl_en.is_empty() { &cl_ko } else { &cl_en }, &format!("col_{}", j + 1))
                };
                let mut n = 2;
                let base = ck.clone();
                while cseen.contains(&ck) {
                    ck = format!("{base}_{n}");
                    n += 1;
                }
                cseen.push(ck.clone());
                let ckind = c.get("type").and_then(Value::as_str).and_then(FieldKind::parse).unwrap_or_default();
                let ckind = if ckind.is_scalar() { ckind } else { FieldKind::String };
                columns.push(FieldDef {
                    key: ck,
                    label_ko: if cl_ko.is_empty() { cl_en.clone() } else { cl_ko },
                    label_en: cl_en,
                    kind: ckind,
                    ..Default::default()
                });
            }
            if columns.is_empty() {
                columns.push(FieldDef::new("value", "값", "Value", FieldKind::String));
            }
        }
        let options: Vec<String> = f
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let kind = if kind == FieldKind::Enum && options.is_empty() { FieldKind::String } else { kind };
        fields.push(FieldDef {
            key,
            label_ko: if label_ko.is_empty() { label_en.clone() } else { label_ko },
            label_en,
            kind,
            required: f.get("required").and_then(Value::as_bool).unwrap_or(false),
            regex_hint: f
                .get("regex_hint")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            example: f.get("example").and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            description: None,
            options,
            fields: columns,
            mask: false,
        });
        if fields.len() >= MAX_FIELDS {
            break;
        }
    }
    let name = s("name").unwrap_or_else(|| "새 스키마".to_string());
    let doc_type = s("doc_type").filter(|d| is_key(d)).unwrap_or_else(|| key_from_label(&name, "custom"));
    SchemaDraft {
        id: None,
        name,
        name_en: s("name_en"),
        doc_type,
        source,
        description_ko: s("description_ko"),
        description_en: s("description_en"),
        definition: SchemaDef { fields, rules: Vec::new() },
        form_document_id: None,
        note: None,
        sample_document_ids: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Lint (§3.4.1 grammar restrictions)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LintIssue {
    /// `error` | `warn`
    pub level: String,
    pub path: String,
    pub code: String,
    pub message: String,
}

fn issue(level: &str, path: &str, code: &str, message: impl Into<String>) -> LintIssue {
    LintIssue { level: level.into(), path: path.into(), code: code.into(), message: message.into() }
}

fn lint_field(f: &FieldDef, path: &str, nested: bool, out: &mut Vec<LintIssue>) {
    if !is_key(&f.key) {
        out.push(issue("error", path, "key_format", format!("'{}' must be ASCII snake_case (a-z, 0-9, _)", f.key)));
    }
    if f.label_ko.trim().is_empty() && f.label_en.trim().is_empty() {
        out.push(issue("error", path, "label_missing", "a Korean or English label is required"));
    }
    if let Some(r) = &f.regex_hint {
        if !(r.starts_with('^') && r.ends_with('$')) {
            out.push(issue(
                "warn",
                path,
                "regex_anchor",
                "regex hints must be anchored ^…$ (validator; not part of the grammar)",
            ));
        }
        if r.contains("(?") {
            out.push(issue("warn", path, "regex_feature", "look-around / flags are not supported by the grammar"));
        }
    }
    match f.kind {
        FieldKind::Enum => {
            if f.options.is_empty() {
                out.push(issue("error", path, "enum_empty", "a choice field needs at least one option"));
            }
            if f.options.len() > 12 {
                out.push(issue("warn", path, "enum_large", "more than 12 options bloat the grammar"));
            }
        }
        FieldKind::Table | FieldKind::Group => {
            if nested {
                out.push(issue(
                    "error",
                    path,
                    "nesting",
                    "tables and groups cannot be nested (no $ref in the grammar)",
                ));
            }
            if f.fields.is_empty() {
                out.push(issue("error", path, "children_empty", "a table / group needs at least one column"));
            }
            if f.fields.len() > MAX_COLUMNS {
                out.push(issue("warn", path, "columns_many", format!("more than {MAX_COLUMNS} columns")));
            }
            let mut seen = std::collections::HashSet::new();
            for c in &f.fields {
                let cp = format!("{path}.{}", c.key);
                if !seen.insert(c.key.clone()) {
                    out.push(issue("error", &cp, "key_duplicate", "duplicate column key"));
                }
                lint_field(c, &cp, true, out);
            }
        }
        _ => {}
    }
    if f.kind.is_scalar() && !f.fields.is_empty() {
        out.push(issue("warn", path, "children_ignored", "a scalar field cannot have columns"));
    }
    if f.kind != FieldKind::Enum && !f.options.is_empty() {
        out.push(issue("warn", path, "options_ignored", "options only apply to choice fields"));
    }
}

/// Checks a definition against the grammar rules. Errors block saving; warnings are shown.
pub fn lint(def: &SchemaDef) -> Vec<LintIssue> {
    let mut out = Vec::new();
    if def.fields.is_empty() {
        out.push(issue("error", "", "fields_empty", "a schema needs at least one field"));
    }
    if def.fields.len() > MAX_FIELDS {
        out.push(issue("error", "", "fields_many", format!("more than {MAX_FIELDS} fields")));
    }
    let mut seen = std::collections::HashSet::new();
    for f in &def.fields {
        if !seen.insert(f.key.clone()) {
            out.push(issue("error", &f.key, "key_duplicate", "duplicate field key"));
        }
        if matches!(f.key.as_str(), "value" | "evidence" | "confidence") {
            out.push(issue("error", &f.key, "key_reserved", "value / evidence / confidence are reserved"));
        }
        lint_field(f, &f.key, false, &mut out);
    }
    for r in &def.rules {
        if !KNOWN_RULES.contains(&r.as_str()) {
            out.push(issue("warn", "", "rule_unknown", format!("unknown rule '{r}' is ignored")));
        }
    }
    out
}

pub const KNOWN_RULES: [&str; 10] = [
    RULE_PERCENT_RANGE,
    RULE_TEST_COUNTS,
    RULE_SUPPLY_TAX_TOTAL,
    RULE_ITEMS_SUM_SUPPLY,
    RULE_TAX_RATE,
    RULE_ZERO_RATE,
    RULE_RECEIPT_TOTAL,
    RULE_ITEMS_SUM_TOTAL,
    RULE_PERIOD_ORDER,
    RULE_EXEMPT_TOTAL,
];

pub fn has_errors(issues: &[LintIssue]) -> bool {
    issues.iter().any(|i| i.level == "error")
}

// ---------------------------------------------------------------------------
// Prompt summary
// ---------------------------------------------------------------------------

/// Field list injected into the KIE prompt (llama.cpp does not add the schema itself).
pub fn prompt_summary(def: &SchemaDef, ko: bool) -> String {
    let mut out = String::new();
    for f in &def.fields {
        match f.kind {
            FieldKind::Table => {
                out.push_str(&format!("- {}[] ({}; {}): ", f.key, f.label(ko), f.kind.label(ko)));
                let cols: Vec<String> =
                    f.fields.iter().map(|c| format!("{}={}({})", c.key, c.label(ko), c.kind.label(ko))).collect();
                out.push_str(&cols.join(", "));
                out.push('\n');
            }
            FieldKind::Group => {
                out.push_str(&format!("- {} ({}):\n", f.key, f.label(ko)));
                for c in &f.fields {
                    out.push_str("  ");
                    out.push_str(&scalar_line(c, ko));
                }
            }
            _ => out.push_str(&scalar_line(f, ko)),
        }
    }
    out
}

fn scalar_line(f: &FieldDef, ko: bool) -> String {
    let mut s = format!("- {} ({}; {}", f.key, f.label(ko), f.kind.label(ko));
    if f.required {
        s.push_str(if ko { "; 필수" } else { "; required" });
    }
    if f.kind == FieldKind::Enum && !f.options.is_empty() {
        s.push_str(&format!("; {}", f.options.join("/")));
    }
    s.push(')');
    if let Some(e) = &f.example {
        s.push_str(&format!(" {} {e}", if ko { "예:" } else { "e.g." }));
    }
    if let Some(d) = &f.description {
        s.push_str(&format!(" — {d}"));
    }
    s.push('\n');
    s
}

// ---------------------------------------------------------------------------
// Built-in sample forms (§3.4.1)
// ---------------------------------------------------------------------------

/// A bundled sample form.
pub struct Builtin {
    pub key: &'static str,
    pub doc_type: &'static str,
    pub name_ko: &'static str,
    pub name_en: &'static str,
    pub description_ko: &'static str,
    pub description_en: &'static str,
    pub def: SchemaDef,
    /// Kept for workspaces that classified documents with it, but not offered: inserted (or
    /// switched) to `enabled = 0`, left out of the classifier enum and of `builtin_doc_types`.
    /// The 0.3.0 default set is software-development paperwork; the 0.1 business forms are hidden.
    pub hidden: bool,
}

fn party(prefix_ko: &str, prefix_en: &str, with_email: bool) -> Vec<FieldDef> {
    use FieldKind::*;
    let mut v = vec![
        FieldDef::new("brn", &format!("{prefix_ko} 사업자등록번호"), &format!("{prefix_en} business reg. no."), Brn)
            .required()
            .example("123-45-67890"),
        FieldDef::new("sub_office_no", "종사업장번호", "Branch no.", String),
        FieldDef::new("name", &format!("{prefix_ko} 상호"), &format!("{prefix_en} name"), String).required(),
        FieldDef::new("representative", "대표자", "Representative", String),
        FieldDef::new("address", "주소", "Address", String),
        FieldDef::new("business_type", "업태", "Business type", String),
        FieldDef::new("business_item", "종목", "Business item", String),
    ];
    if with_email {
        v.push(FieldDef::new("email1", "이메일1", "Email 1", Email));
        v.push(FieldDef::new("email2", "이메일2", "Email 2", Email));
    }
    v
}

fn invoice_items(with_tax: bool) -> Vec<FieldDef> {
    use FieldKind::*;
    let mut v = vec![
        FieldDef::new("month_day", "월일", "Date (MM/DD)", String),
        FieldDef::new("item", "품목", "Item", String),
        FieldDef::new("spec", "규격", "Spec", String),
        FieldDef::new("quantity", "수량", "Qty", Integer),
        FieldDef::new("unit_price", "단가", "Unit price", Amount),
        FieldDef::new("supply_amount", "공급가액", "Supply amount", Amount),
    ];
    if with_tax {
        v.push(FieldDef::new("tax_amount", "세액", "Tax", Amount));
    }
    v.push(FieldDef::new("note", "비고", "Note", String));
    v
}

fn payment_group() -> FieldDef {
    use FieldKind::*;
    FieldDef::new("payment", "결제", "Payment", Group).with(vec![
        FieldDef::new("cash", "현금", "Cash", Amount),
        FieldDef::new("check", "수표", "Check", Amount),
        FieldDef::new("note", "어음", "Promissory note", Amount),
        FieldDef::new("credit", "외상", "Credit", Amount),
    ])
}

/// A party of a software project (발주자 · 수행사 · 제안사 · 발주기관).
fn sw_party(prefix_ko: &str, prefix_en: &str) -> Vec<FieldDef> {
    use FieldKind::*;
    vec![
        FieldDef::new("name", &format!("{prefix_ko} 명칭"), &format!("{prefix_en} name"), String).required(),
        FieldDef::new("brn", &format!("{prefix_ko} 사업자등록번호"), &format!("{prefix_en} business reg. no."), Brn),
        FieldDef::new("representative", "대표자", "Representative", String),
        FieldDef::new("contact_person", "담당자", "Contact person", String),
        FieldDef::new("phone", "연락처", "Phone", Phone),
        FieldDef::new("email", "이메일", "Email", Email),
    ]
}

fn requirements_table() -> FieldDef {
    use FieldKind::*;
    FieldDef::new("requirements", "요구사항", "Requirements", Table).with(vec![
        FieldDef::new("id", "ID", "ID", String).example("SFR-001"),
        FieldDef::new("name", "요구사항명", "Name", String),
        FieldDef::new("description", "상세 설명", "Description", String),
        FieldDef::new("category", "구분", "Category", Enum).options(&[
            "기능",
            "비기능",
            "보안",
            "성능",
            "인터페이스",
            "데이터",
        ]),
        FieldDef::new("priority", "우선순위", "Priority", Enum).options(&["상", "중", "하"]),
        FieldDef::new("source", "출처", "Source", String),
        FieldDef::new("acceptance", "수용 기준", "Acceptance criteria", String),
    ])
}

fn deliverables_table(with_result: bool) -> FieldDef {
    use FieldKind::*;
    let mut cols = vec![
        FieldDef::new("name", "산출물", "Deliverable", String),
        FieldDef::new("version", "버전", "Version", String),
        FieldDef::new("due_date", "납기", "Due date", Date),
    ];
    if with_result {
        cols.push(FieldDef::new("result", "검수 결과", "Result", Enum).options(&["적합", "부적합", "조건부"]));
    }
    cols.push(FieldDef::new("note", "비고", "Note", String));
    FieldDef::new("deliverables", "산출물", "Deliverables", Table).with(cols)
}

fn schedule_table() -> FieldDef {
    use FieldKind::*;
    FieldDef::new("schedule", "일정", "Schedule", Table).with(vec![
        FieldDef::new("phase", "단계", "Phase", String),
        FieldDef::new("start_date", "시작", "Start", Date),
        FieldDef::new("end_date", "종료", "End", Date),
        FieldDef::new("note", "비고", "Note", String),
    ])
}

fn staffing_items() -> Vec<FieldDef> {
    use FieldKind::*;
    vec![
        FieldDef::new("role", "역할·품목", "Role / item", String).example("백엔드 개발"),
        FieldDef::new("grade", "등급", "Grade", Enum).options(&["특급", "고급", "중급", "초급", "기타"]),
        FieldDef::new("months", "투입 MM", "Person-months", Amount).example("3.0"),
        FieldDef::new("unit_price", "단가", "Unit price", Amount),
        FieldDef::new("supply_amount", "공급가액", "Supply amount", Amount),
        FieldDef::new("note", "비고", "Note", String),
    ]
}

/// The software-development document set (0.3.0 defaults) followed by the hidden 0.1 business forms.
pub fn builtins() -> Vec<Builtin> {
    let mut out = software_builtins();
    out.extend(legacy_builtins());
    out
}

fn software_builtins() -> Vec<Builtin> {
    use FieldKind::*;
    vec![
        Builtin {
            key: "rfp",
            hidden: false,
            doc_type: "rfp",
            name_ko: "제안요청서(RFP)",
            name_en: "Request for proposal (RFP)",
            description_ko: "발주기관이 공고하는 소프트웨어 개발 제안요청서 — 사업 개요·예산·기간·요구사항·산출물·일정·제출 마감.",
            description_en: "A software-development RFP: overview, budget, period, requirements, deliverables, schedule and the submission deadline.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("project_name", "사업명", "Project name", String).required(),
                    FieldDef::new("issuer", "발주기관", "Issuer", Group).with(sw_party("발주기관", "Issuer")),
                    FieldDef::new("issue_date", "공고일", "Issue date", Date).required(),
                    FieldDef::new("budget", "사업 예산", "Budget", Amount).example("250,000,000"),
                    FieldDef::new("period_start", "사업 기간 시작", "Period start", Date),
                    FieldDef::new("period_end", "사업 기간 종료", "Period end", Date),
                    FieldDef::new("submission_deadline", "제출 마감", "Submission deadline", Date),
                    FieldDef::new("summary", "사업 개요", "Overview", String).describe("추진 배경·목적·범위 요약"),
                    requirements_table(),
                    deliverables_table(false),
                    schedule_table(),
                    FieldDef::new("evaluation", "평가 기준", "Evaluation criteria", Table).with(vec![
                        FieldDef::new("item", "항목", "Item", String),
                        FieldDef::new("points", "배점", "Points", Amount),
                    ]),
                ],
                &[RULE_PERIOD_ORDER],
            ),
        },
        Builtin {
            key: "proposal",
            hidden: false,
            doc_type: "proposal",
            name_ko: "제안서",
            name_en: "Proposal",
            description_ko: "RFP에 대한 제안서 — 제안사·제안 금액·기간·투입 인력·일정·산출물.",
            description_en: "A proposal answering an RFP: proposer, price, period, staffing, schedule and deliverables.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("project_name", "사업명", "Project name", String).required(),
                    FieldDef::new("proposer", "제안사", "Proposer", Group).with(sw_party("제안사", "Proposer")),
                    FieldDef::new("client_name", "발주기관", "Client", String),
                    FieldDef::new("issue_date", "제출일", "Submission date", Date),
                    FieldDef::new("price", "제안 금액", "Proposed price", Amount).required().example("240,000,000"),
                    FieldDef::new("period_start", "수행 기간 시작", "Period start", Date),
                    FieldDef::new("period_end", "수행 기간 종료", "Period end", Date),
                    FieldDef::new("duration_months", "수행 기간(개월)", "Duration (months)", Integer),
                    FieldDef::new("approach", "수행 방법론 요약", "Approach", String),
                    FieldDef::new("team", "투입 인력", "Team", Table).with(vec![
                        FieldDef::new("role", "역할", "Role", String),
                        FieldDef::new("name", "성명", "Name", String),
                        FieldDef::new("grade", "등급", "Grade", Enum).options(&["특급", "고급", "중급", "초급", "기타"]),
                        FieldDef::new("months", "투입 MM", "Person-months", Amount),
                    ]),
                    schedule_table(),
                    deliverables_table(false),
                ],
                &[RULE_PERIOD_ORDER],
            ),
        },
        Builtin {
            key: "sow",
            hidden: false,
            doc_type: "sow",
            name_ko: "과업지시서",
            name_en: "Statement of work",
            description_ko: "과업지시서(과업내용서) — 발주자·수행자, 과업 범위, 산출물, 검수 기준, 단계별 납기·대금.",
            description_en: "Statement of work: client and contractor, scope, deliverables, acceptance criteria and milestone payments.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("project_name", "과업명", "Project name", String).required(),
                    FieldDef::new("client", "발주자", "Client", Group).with(sw_party("발주자", "Client")),
                    FieldDef::new("contractor", "수행자", "Contractor", Group).with(sw_party("수행자", "Contractor")),
                    FieldDef::new("issue_date", "작성일", "Issue date", Date),
                    FieldDef::new("period_start", "과업 기간 시작", "Period start", Date),
                    FieldDef::new("period_end", "과업 기간 종료", "Period end", Date),
                    FieldDef::new("amount", "과업 금액", "Amount", Amount),
                    FieldDef::new("scope", "과업 범위", "Scope", String),
                    FieldDef::new("acceptance_criteria", "검수 기준", "Acceptance criteria", String),
                    FieldDef::new("tasks", "세부 과업", "Tasks", Table).with(vec![
                        FieldDef::new("id", "번호", "No.", String),
                        FieldDef::new("task", "과업", "Task", String),
                        FieldDef::new("description", "내용", "Description", String),
                        FieldDef::new("deliverable", "산출물", "Deliverable", String),
                    ]),
                    deliverables_table(false),
                    FieldDef::new("milestones", "단계별 납기·대금", "Milestones", Table).with(vec![
                        FieldDef::new("phase", "단계", "Phase", String),
                        FieldDef::new("due_date", "납기", "Due date", Date),
                        FieldDef::new("ratio", "대금 비율(%)", "Payment ratio (%)", Amount),
                        FieldDef::new("amount", "금액", "Amount", Amount),
                    ]),
                ],
                &[RULE_PERIOD_ORDER],
            ),
        },
        Builtin {
            key: "srs",
            hidden: false,
            doc_type: "srs",
            name_ko: "요구사항 정의서",
            name_en: "Software requirements specification",
            description_ko: "요구사항 정의서(SRS) — 버전·작성자·승인자와 요구사항 표(ID·구분·우선순위·수용 기준).",
            description_en: "Requirements specification: version, author, approver and the requirements table.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("project_name", "사업명", "Project name", String).required(),
                    FieldDef::new("version", "버전", "Version", String).example("1.2"),
                    FieldDef::new("issue_date", "작성일", "Issue date", Date),
                    FieldDef::new("author", "작성자", "Author", String),
                    FieldDef::new("approver", "승인자", "Approver", String),
                    FieldDef::new("summary", "시스템 개요", "System overview", String),
                    requirements_table(),
                ],
                &[],
            ),
        },
        Builtin {
            key: "dev_contract",
            hidden: false,
            doc_type: "dev_contract",
            name_ko: "소프트웨어 개발 계약서",
            name_en: "Software development contract",
            description_ko: "소프트웨어 개발(외주·용역) 계약서 — 갑·을, 계약일·기간·금액, 대금 지급 단계, 지식재산권·하자보수·비밀유지, 서명.",
            description_en: "Software development / outsourcing contract: parties, date, period, amount, payment milestones, IP, warranty, confidentiality, signatures.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("title", "계약명", "Title", String).required(),
                    FieldDef::new("client", "발주자(갑)", "Client (party A)", Group).with(sw_party("발주자", "Client")),
                    FieldDef::new("contractor", "수행사(을)", "Contractor (party B)", Group).with(sw_party("수행사", "Contractor")),
                    FieldDef::new("contract_date", "계약일", "Contract date", Date),
                    FieldDef::new("period_start", "계약기간 시작", "Period start", Date),
                    FieldDef::new("period_end", "계약기간 종료", "Period end", Date),
                    FieldDef::new("amount", "계약금액", "Contract amount", Amount).required().example("240,000,000"),
                    FieldDef::new("vat_included", "부가세 포함 여부", "VAT included", Boolean),
                    FieldDef::new("payment_terms", "지급 조건", "Payment terms", String),
                    FieldDef::new("payments", "대금 지급 단계", "Payment milestones", Table).with(vec![
                        FieldDef::new("phase", "단계", "Phase", String).example("착수금"),
                        FieldDef::new("ratio", "비율(%)", "Ratio (%)", Amount),
                        FieldDef::new("amount", "금액", "Amount", Amount),
                        FieldDef::new("due_date", "지급 시기", "Due date", Date),
                    ]),
                    FieldDef::new("warranty_months", "하자보수 기간(개월)", "Warranty (months)", Integer),
                    FieldDef::new("ip_owner", "지식재산권 귀속", "IP owner", Enum).options(&["발주자", "수행사", "공동"]),
                    FieldDef::new("confidentiality", "비밀유지 조항", "Confidentiality clause", Boolean),
                    FieldDef::new("key_clauses", "주요 조항 요약", "Key clauses", Table).with(vec![
                        FieldDef::new("clause", "조항", "Clause", String),
                        FieldDef::new("summary", "요약", "Summary", String),
                    ]),
                    FieldDef::new("signed", "서명/날인 여부", "Signed / sealed", Boolean),
                ],
                &[RULE_PERIOD_ORDER],
            ),
        },
        Builtin {
            key: "wbs",
            hidden: false,
            doc_type: "wbs",
            name_ko: "WBS·일정표",
            name_en: "WBS / schedule",
            description_ko: "작업 분류 체계와 일정표 — 작업별 담당·시작·종료·진척률·산출물, 마일스톤.",
            description_en: "Work breakdown structure and schedule: tasks with owner, dates, progress and deliverable; milestones.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("project_name", "사업명", "Project name", String).required(),
                    FieldDef::new("version", "버전", "Version", String),
                    FieldDef::new("issue_date", "작성일", "Issue date", Date),
                    FieldDef::new("period_start", "전체 시작", "Period start", Date),
                    FieldDef::new("period_end", "전체 종료", "Period end", Date),
                    FieldDef::new("tasks", "작업", "Tasks", Table).with(vec![
                        FieldDef::new("id", "WBS 번호", "WBS id", String).example("1.2.3"),
                        FieldDef::new("task", "작업", "Task", String),
                        FieldDef::new("owner", "담당", "Owner", String),
                        FieldDef::new("start_date", "시작", "Start", Date),
                        FieldDef::new("end_date", "종료", "End", Date),
                        FieldDef::new("progress", "진척률(%)", "Progress (%)", Amount),
                        FieldDef::new("deliverable", "산출물", "Deliverable", String),
                    ]),
                    FieldDef::new("milestones", "마일스톤", "Milestones", Table).with(vec![
                        FieldDef::new("name", "마일스톤", "Milestone", String),
                        FieldDef::new("date", "일자", "Date", Date),
                    ]),
                ],
                &[RULE_PERIOD_ORDER, RULE_PERCENT_RANGE],
            ),
        },
        Builtin {
            key: "progress_report",
            hidden: false,
            doc_type: "progress_report",
            name_ko: "진척 보고서",
            name_en: "Progress report",
            description_ko: "주간·월간·단계 진척 보고서 — 보고 기간, 전체 진척률(계획 대비), 작업별 실적, 이슈·리스크, 차기 계획.",
            description_en: "Weekly / monthly / phase progress report: period, overall vs planned progress, task results, issues and the next plan.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("project_name", "사업명", "Project name", String).required(),
                    FieldDef::new("report_type", "보고 구분", "Report type", Enum).options(&["주간", "월간", "단계", "기타"]),
                    FieldDef::new("period_start", "보고 기간 시작", "Period start", Date),
                    FieldDef::new("period_end", "보고 기간 종료", "Period end", Date),
                    FieldDef::new("report_date", "보고일", "Report date", Date).required(),
                    FieldDef::new("author", "작성자", "Author", String),
                    FieldDef::new("progress_percent", "전체 진척률(%)", "Overall progress (%)", Amount).example("62"),
                    FieldDef::new("plan_percent", "계획 진척률(%)", "Planned progress (%)", Amount),
                    FieldDef::new("summary", "주요 실적", "Summary", String),
                    FieldDef::new("tasks", "작업별 실적", "Task results", Table).with(vec![
                        FieldDef::new("task", "작업", "Task", String),
                        FieldDef::new("owner", "담당", "Owner", String),
                        FieldDef::new("planned", "계획", "Planned", String),
                        FieldDef::new("actual", "실적", "Actual", String),
                        FieldDef::new("progress", "진척률(%)", "Progress (%)", Amount),
                    ]),
                    FieldDef::new("issues", "이슈·리스크", "Issues / risks", Table).with(vec![
                        FieldDef::new("issue", "이슈", "Issue", String),
                        FieldDef::new("impact", "영향도", "Impact", Enum).options(&["상", "중", "하"]),
                        FieldDef::new("action", "조치", "Action", String),
                        FieldDef::new("owner", "담당", "Owner", String),
                        FieldDef::new("due_date", "기한", "Due date", Date),
                    ]),
                    FieldDef::new("next_plan", "차기 계획", "Next plan", String),
                ],
                &[RULE_PERIOD_ORDER, RULE_PERCENT_RANGE],
            ),
        },
        Builtin {
            key: "meeting_minutes",
            hidden: false,
            doc_type: "meeting_minutes",
            name_ko: "회의록",
            name_en: "Meeting minutes",
            description_ko: "회의록 — 일시·장소·참석자, 안건, 결정 사항, 실행 항목(담당·기한), 차기 회의.",
            description_en: "Meeting minutes: date, place, attendees, agenda, decisions, action items with owners and due dates, next meeting.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("title", "회의명", "Title", String).required(),
                    FieldDef::new("project_name", "사업명", "Project name", String),
                    FieldDef::new("meeting_date", "일시", "Date", Date).required(),
                    FieldDef::new("location", "장소", "Location", String),
                    FieldDef::new("author", "작성자", "Author", String),
                    FieldDef::new("attendees", "참석자", "Attendees", Table).with(vec![
                        FieldDef::new("name", "성명", "Name", String),
                        FieldDef::new("organization", "소속", "Organization", String),
                        FieldDef::new("role", "역할", "Role", String),
                    ]),
                    FieldDef::new("agenda", "안건", "Agenda", String),
                    FieldDef::new("decisions", "결정 사항", "Decisions", Table).with(vec![
                        FieldDef::new("item", "항목", "Item", String),
                        FieldDef::new("detail", "내용", "Detail", String),
                    ]),
                    FieldDef::new("action_items", "실행 항목", "Action items", Table).with(vec![
                        FieldDef::new("item", "항목", "Item", String),
                        FieldDef::new("owner", "담당", "Owner", String),
                        FieldDef::new("due_date", "기한", "Due date", Date),
                    ]),
                    FieldDef::new("next_meeting", "차기 회의", "Next meeting", Date),
                ],
                &[],
            ),
        },
        Builtin {
            key: "test_report",
            hidden: false,
            doc_type: "test_report",
            name_ko: "테스트 결과서",
            name_en: "Test report",
            description_ko: "테스트 결과서 — 대상 버전·일자·구분·환경, 케이스 집계(총·통과·실패·보류·통과율), 케이스별 결과, 결함 목록.",
            description_en: "Test report: version, date, type, environment, case counts (total / passed / failed / blocked / pass rate), per-case results and defects.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("project_name", "사업명", "Project name", String).required(),
                    FieldDef::new("version", "대상 버전", "Version under test", String),
                    FieldDef::new("test_date", "테스트 일자", "Test date", Date).required(),
                    FieldDef::new("tester", "테스터", "Tester", String),
                    FieldDef::new("test_type", "테스트 구분", "Test type", Enum).options(&["단위", "통합", "시스템", "인수", "성능", "보안"]),
                    FieldDef::new("environment", "테스트 환경", "Environment", String),
                    FieldDef::new("total_cases", "총 케이스", "Total cases", Integer),
                    FieldDef::new("passed", "통과", "Passed", Integer),
                    FieldDef::new("failed", "실패", "Failed", Integer),
                    FieldDef::new("blocked", "보류", "Blocked", Integer),
                    FieldDef::new("pass_rate", "통과율(%)", "Pass rate (%)", Amount),
                    FieldDef::new("cases", "케이스", "Cases", Table).with(vec![
                        FieldDef::new("id", "ID", "ID", String).example("TC-014"),
                        FieldDef::new("scenario", "시나리오", "Scenario", String),
                        FieldDef::new("expected", "기대 결과", "Expected", String),
                        FieldDef::new("result", "결과", "Result", Enum).options(&["통과", "실패", "보류"]),
                        FieldDef::new("defect_id", "결함 ID", "Defect id", String),
                    ]),
                    FieldDef::new("defects", "결함", "Defects", Table).with(vec![
                        FieldDef::new("id", "결함 ID", "Defect id", String),
                        FieldDef::new("severity", "심각도", "Severity", Enum).options(&["치명", "높음", "보통", "낮음"]),
                        FieldDef::new("description", "내용", "Description", String),
                        FieldDef::new("status", "상태", "Status", Enum).options(&["열림", "수정중", "해결", "종료"]),
                    ]),
                ],
                &[RULE_TEST_COUNTS, RULE_PERCENT_RANGE],
            ),
        },
        Builtin {
            key: "change_request",
            hidden: false,
            doc_type: "change_request",
            name_ko: "변경요청서",
            name_en: "Change request",
            description_ko: "변경요청서(CR) — 번호·요청자·일자, 변경 내용·사유, 일정·비용 영향, 우선순위, 승인 상태·승인자.",
            description_en: "Change request: number, requester, date, change and reason, schedule / cost impact, priority, approval status and approver.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("cr_no", "CR 번호", "CR no.", String).required().example("CR-2026-014"),
                    FieldDef::new("project_name", "사업명", "Project name", String),
                    FieldDef::new("request_date", "요청일", "Request date", Date).required(),
                    FieldDef::new("requester", "요청자", "Requester", String),
                    FieldDef::new("title", "변경 제목", "Title", String),
                    FieldDef::new("description", "변경 내용", "Description", String),
                    FieldDef::new("reason", "변경 사유", "Reason", String),
                    FieldDef::new("impact_schedule", "일정 영향", "Schedule impact", String),
                    FieldDef::new("impact_cost", "비용 영향", "Cost impact", Amount),
                    FieldDef::new("priority", "우선순위", "Priority", Enum).options(&["긴급", "높음", "보통", "낮음"]),
                    FieldDef::new("status", "승인 상태", "Status", Enum).options(&["요청", "검토", "승인", "반려", "완료"]),
                    FieldDef::new("approver", "승인자", "Approver", String),
                    FieldDef::new("approval_date", "승인일", "Approval date", Date),
                ],
                &[],
            ),
        },
        Builtin {
            key: "acceptance",
            hidden: false,
            doc_type: "acceptance",
            name_ko: "검수확인서",
            name_en: "Acceptance certificate",
            description_ko: "검수확인서·완료보고서 — 발주자·수행사, 검수일, 산출물별 검수 결과, 검수 금액, 최종 판정, 서명.",
            description_en: "Acceptance certificate / completion report: client and contractor, acceptance date, per-deliverable results, amount, verdict, signatures.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("project_name", "사업명", "Project name", String).required(),
                    FieldDef::new("client", "발주자", "Client", Group).with(sw_party("발주자", "Client")),
                    FieldDef::new("contractor", "수행사", "Contractor", Group).with(sw_party("수행사", "Contractor")),
                    FieldDef::new("acceptance_date", "검수일", "Acceptance date", Date).required(),
                    FieldDef::new("period_start", "사업 기간 시작", "Period start", Date),
                    FieldDef::new("period_end", "사업 기간 종료", "Period end", Date),
                    deliverables_table(true),
                    FieldDef::new("amount", "검수 금액", "Amount", Amount),
                    FieldDef::new("result", "최종 판정", "Verdict", Enum).options(&["합격", "조건부 합격", "불합격"]),
                    FieldDef::new("conditions", "조건·특이사항", "Conditions / remarks", String),
                    FieldDef::new("signed", "서명/날인 여부", "Signed / sealed", Boolean),
                ],
                &[RULE_PERIOD_ORDER],
            ),
        },
        Builtin {
            key: "estimate",
            hidden: false,
            doc_type: "estimate",
            name_ko: "견적서",
            name_en: "Estimate",
            description_ko: "소프트웨어 개발 견적서(인력·MM 기준) — 공급자, 수신, 유효기간, 역할별 등급·MM·단가·공급가액, 합계·세액.",
            description_en: "Software development estimate (staffing / person-month based): supplier, recipient, validity, per-role grade, MM, unit price and amounts, totals.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("project_name", "사업명", "Project name", String),
                    FieldDef::new("issue_date", "견적일", "Issue date", Date).required(),
                    FieldDef::new("supplier", "공급자", "Supplier", Group).with(party("공급자", "Supplier", true)),
                    FieldDef::new("recipient", "수신", "Recipient", String),
                    FieldDef::new("valid_until", "유효기간", "Valid until", Date),
                    FieldDef::new("items", "견적 내역", "Items", Table).with(staffing_items()),
                    FieldDef::new("supply_total", "공급가액 합계", "Supply total", Amount).required(),
                    FieldDef::new("tax_total", "세액 합계", "Tax total", Amount),
                    FieldDef::new("grand_total", "합계금액", "Grand total", Amount).required().example("132,000,000"),
                    FieldDef::new("note", "비고", "Note", String),
                ],
                &[RULE_SUPPLY_TAX_TOTAL, RULE_ITEMS_SUM_SUPPLY, RULE_TAX_RATE],
            ),
        },
        Builtin {
            key: "general",
            hidden: false,
            doc_type: "general",
            name_ko: "일반(표/차트)",
            name_en: "General (table / chart)",
            description_ko: "유형이 없는 문서 — 제목과 핵심 수치, 표는 텍스트 탭의 HTML 표로 보관.",
            description_en: "Untyped documents — title and key figures; tables stay in the OCR text.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("title", "제목", "Title", String),
                    FieldDef::new("date", "날짜", "Date", Date),
                    FieldDef::new("key_figures", "핵심 수치", "Key figures", Table).with(vec![
                        FieldDef::new("label", "항목", "Label", String),
                        FieldDef::new("value", "값", "Value", String),
                        FieldDef::new("unit", "단위", "Unit", String),
                    ]),
                    FieldDef::new("chart_series", "차트 데이터", "Chart series", Table).with(vec![
                        FieldDef::new("series", "시리즈", "Series", String),
                        FieldDef::new("value", "값", "Value", String),
                    ]),
                ],
                &[],
            ),
        },
    ]
}

/// The 0.1 business forms (세금계산서 · 계산서 · 영수증 · 사업자등록증 · 계약서 · 신청서): still defined so
/// that documents already classified with them keep their fields and labels, but hidden by default.
pub fn legacy_builtins() -> Vec<Builtin> {
    use FieldKind::*;
    vec![
        Builtin {
            key: "tax_invoice",
            hidden: true,
            doc_type: "tax_invoice",
            name_ko: "세금계산서",
            name_en: "Tax invoice",
            description_ko: "과세·영세율 세금계산서(전자·종이). 면세 거래는 '계산서'를 사용.",
            description_en: "Taxable / zero-rated tax invoice (electronic or paper).",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("issue_date", "작성일자", "Issue date", Date).required().example("2026-10-14"),
                    FieldDef::new("tax_type", "과세유형", "Tax type", Enum).options(&["과세", "영세율"]).required(),
                    FieldDef::new("receipt_or_claim", "영수/청구", "Receipt / claim", Enum).options(&["영수", "청구"]),
                    FieldDef::new("book_no", "책번호(권-호)", "Book no.", String).describe("종이 양식의 책번호 권·호"),
                    FieldDef::new("serial_no", "일련번호", "Serial no.", String),
                    FieldDef::new("supplier", "공급자", "Supplier", Group).with(party("공급자", "Supplier", false)),
                    FieldDef::new("buyer", "공급받는자", "Buyer", Group).with(party("공급받는자", "Buyer", true)),
                    FieldDef::new("supply_total", "공급가액 합계", "Supply total", Amount)
                        .required()
                        .example("1,000,000"),
                    FieldDef::new("tax_total", "세액 합계", "Tax total", Amount).required().example("100,000"),
                    FieldDef::new("grand_total", "합계금액", "Grand total", Amount).required().example("1,100,000"),
                    payment_group(),
                    FieldDef::new("nts_approval_no", "국세청 승인번호", "NTS approval no.", String),
                    FieldDef::new("correction_reason_code", "수정사유 코드", "Correction reason", Enum)
                        .options(&["1", "2", "3", "4", "5", "6"])
                        .describe("수정세금계산서일 때만"),
                    FieldDef::new("original_approval_no", "당초 승인번호", "Original approval no.", String),
                    FieldDef::new("remarks", "비고", "Remarks", String),
                    FieldDef::new("items", "품목", "Line items", Table).with(invoice_items(true)),
                ],
                &[RULE_SUPPLY_TAX_TOTAL, RULE_ITEMS_SUM_SUPPLY, RULE_TAX_RATE, RULE_ZERO_RATE],
            ),
        },
        Builtin {
            key: "invoice_exempt",
            hidden: true,
            doc_type: "invoice_exempt",
            name_ko: "계산서(면세)",
            name_en: "Invoice (tax-exempt)",
            description_ko: "면세 거래의 계산서 — 세액 항목이 없음.",
            description_en: "Invoice for tax-exempt transactions (no VAT fields).",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("issue_date", "작성일자", "Issue date", Date).required(),
                    FieldDef::new("receipt_or_claim", "영수/청구", "Receipt / claim", Enum).options(&["영수", "청구"]),
                    FieldDef::new("book_no", "책번호(권-호)", "Book no.", String),
                    FieldDef::new("serial_no", "일련번호", "Serial no.", String),
                    FieldDef::new("supplier", "공급자", "Supplier", Group).with(party("공급자", "Supplier", false)),
                    FieldDef::new("buyer", "공급받는자", "Buyer", Group).with(party("공급받는자", "Buyer", true)),
                    FieldDef::new("supply_total", "공급가액 합계", "Supply total", Amount).required(),
                    FieldDef::new("grand_total", "합계금액", "Grand total", Amount).required(),
                    payment_group(),
                    FieldDef::new("nts_approval_no", "국세청 승인번호", "NTS approval no.", String),
                    FieldDef::new("remarks", "비고", "Remarks", String),
                    FieldDef::new("items", "품목", "Line items", Table).with(invoice_items(false)),
                ],
                &[RULE_ITEMS_SUM_SUPPLY, RULE_EXEMPT_TOTAL],
            ),
        },
        Builtin {
            key: "receipt",
            hidden: true,
            doc_type: "receipt",
            name_ko: "영수증",
            name_en: "Receipt",
            description_ko: "카드·현금 영수증, 간이영수증.",
            description_en: "Card / cash receipts and simplified receipts.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("store_name", "상호", "Store", String).required(),
                    FieldDef::new("brn", "사업자등록번호", "Business reg. no.", Brn),
                    FieldDef::new("merchant_no", "가맹점번호", "Merchant no.", String),
                    FieldDef::new("address", "주소", "Address", String),
                    FieldDef::new("phone", "전화", "Phone", Phone),
                    FieldDef::new("datetime", "일시", "Date / time", Date).required().example("2026-10-14 12:31"),
                    FieldDef::new("items", "품목", "Items", Table).with(vec![
                        FieldDef::new("name", "품명", "Item", String),
                        FieldDef::new("quantity", "수량", "Qty", Integer),
                        FieldDef::new("unit_price", "단가", "Unit price", Amount),
                        FieldDef::new("amount", "금액", "Amount", Amount),
                    ]),
                    FieldDef::new("supply_amount", "공급가액", "Supply amount", Amount),
                    FieldDef::new("vat", "부가세", "VAT", Amount),
                    FieldDef::new("service_charge", "봉사료", "Service charge", Amount),
                    FieldDef::new("exempt_amount", "면세물품가액", "Exempt amount", Amount),
                    FieldDef::new("total", "합계", "Total", Amount).required().example("12,500"),
                    FieldDef::new("payment_method", "결제수단", "Payment method", Enum).options(&[
                        "카드",
                        "현금",
                        "현금영수증",
                        "간편결제",
                        "기타",
                    ]),
                    FieldDef::new("approval_type", "승인/취소", "Approval / cancel", Enum).options(&["승인", "취소"]),
                    FieldDef::new("installment", "할부", "Installment", String).example("일시불"),
                    FieldDef::new("card_last4", "카드 끝 4자리", "Card last 4", String).regex("^[0-9]{4}$"),
                    FieldDef::new("approval_no", "승인번호", "Approval no.", String),
                    FieldDef::new("cash_receipt_no", "현금영수증 승인번호", "Cash receipt approval no.", String),
                    FieldDef::new("cash_receipt_id", "현금영수증 식별번호", "Cash receipt id", Account),
                ],
                &[RULE_RECEIPT_TOTAL, RULE_ITEMS_SUM_TOTAL],
            ),
        },
        Builtin {
            key: "business_registration",
            hidden: true,
            doc_type: "business_registration",
            name_ko: "사업자등록증",
            name_en: "Business registration",
            description_ko: "일반·간이·면세·법인 사업자등록증.",
            description_en: "Business registration certificate.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("tax_header", "과세유형", "Taxpayer type", Enum)
                        .options(&["일반과세자", "간이과세자", "면세사업자", "법인사업자"])
                        .describe("증 상단의 유형 — 간이과세자는 세금계산서 발행 가능 여부 검증에 사용"),
                    FieldDef::new("registration_no", "등록번호", "Registration no.", Brn).required(),
                    FieldDef::new("company_name", "상호(법인명)", "Company name", String).required(),
                    FieldDef::new("representative", "대표자", "Representative", String).required(),
                    FieldDef::new("birth_date", "생년월일", "Date of birth", Date).masked(),
                    FieldDef::new("opening_date", "개업연월일", "Opening date", Date),
                    FieldDef::new("corporate_no", "법인등록번호", "Corporate reg. no.", String),
                    FieldDef::new("business_address", "사업장 소재지", "Business address", String).required(),
                    FieldDef::new("head_office_address", "본점 소재지", "Head office address", String),
                    FieldDef::new("business_type", "업태", "Business type", String),
                    FieldDef::new("business_item", "종목", "Business item", String),
                    FieldDef::new("co_owners", "공동사업자", "Co-owners", String),
                    FieldDef::new("issue_reason", "발급사유", "Issue reason", String),
                    FieldDef::new("unit_taxation", "사업자단위과세 여부", "Unit taxation", Boolean),
                    FieldDef::new("e_invoice_email", "전자세금계산서 전용 이메일", "E-invoice email", Email),
                    FieldDef::new("issue_date", "발급일", "Issue date", Date),
                    FieldDef::new("issuer", "발급기관", "Issuer", String).example("강남세무서장"),
                ],
                &[],
            ),
        },
        Builtin {
            key: "contract",
            hidden: true,
            doc_type: "contract",
            name_ko: "계약서",
            name_en: "Contract",
            description_ko: "공급·용역·임대 등 계약서.",
            description_en: "Supply / service / lease contracts.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("title", "계약명", "Title", String).required(),
                    FieldDef::new("parties", "당사자", "Parties", Table).with(vec![
                        FieldDef::new("role", "구분", "Role", String).example("갑"),
                        FieldDef::new("name", "명칭", "Name", String),
                        FieldDef::new("representative", "대표자", "Representative", String),
                        FieldDef::new("brn", "사업자번호", "Business reg. no.", Brn),
                        FieldDef::new("address", "주소", "Address", String),
                    ]),
                    FieldDef::new("contract_date", "계약일", "Contract date", Date),
                    FieldDef::new("period_start", "계약기간 시작", "Period start", Date),
                    FieldDef::new("period_end", "계약기간 종료", "Period end", Date),
                    FieldDef::new("amount", "계약금액", "Amount", Amount),
                    FieldDef::new("payment_terms", "지급조건", "Payment terms", String),
                    FieldDef::new("key_clauses", "주요 조항 요약", "Key clauses", Table).with(vec![
                        FieldDef::new("clause", "조항", "Clause", String),
                        FieldDef::new("summary", "요약", "Summary", String),
                    ]),
                    FieldDef::new("signed", "서명/날인 여부", "Signed / sealed", Boolean),
                ],
                &[RULE_PERIOD_ORDER],
            ),
        },
        Builtin {
            key: "application_form",
            hidden: true,
            doc_type: "application_form",
            name_ko: "신청서",
            name_en: "Application form",
            description_ko: "각종 신청서·접수 서식.",
            description_en: "Application / request forms.",
            def: SchemaDef::new(
                vec![
                    FieldDef::new("form_title", "서식명", "Form title", String).required(),
                    FieldDef::new("applicant", "신청인", "Applicant", Group).with(vec![
                        FieldDef::new("name", "성명", "Name", String).required(),
                        FieldDef::new("birth_date", "생년월일", "Date of birth", Date).masked(),
                        FieldDef::new("phone", "연락처", "Phone", Phone),
                        FieldDef::new("address", "주소", "Address", String),
                    ]),
                    FieldDef::new("application_date", "신청일", "Application date", Date),
                    FieldDef::new("items", "신청 항목", "Items", Table).with(vec![
                        FieldDef::new("item", "항목", "Item", String),
                        FieldDef::new("value", "값", "Value", String),
                    ]),
                    FieldDef::new("checkboxes", "체크박스", "Checkboxes", Table).with(vec![
                        FieldDef::new("label", "항목", "Label", String),
                        FieldDef::new("checked", "체크", "Checked", Boolean),
                    ]),
                    FieldDef::new("signed", "서명/날인 여부", "Signed / sealed", Boolean),
                ],
                &[],
            ),
        },
    ]
}

/// Extra names a document type goes by in commands and rules ("계약서" → dev_contract). Shared by the
/// planner's type matcher and the project rule parser; the registered labels still win.
pub fn doc_type_aliases(slug: &str) -> &'static [&'static str] {
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

/// One bundled sample form by key (software set or hidden legacy).
pub fn builtin(key: &str) -> Option<Builtin> {
    builtins().into_iter().find(|b| b.key == key)
}

/// Slugs of the offered sample forms (classifier enum when nothing else is registered); hidden legacy forms excluded.
pub fn builtin_doc_types() -> Vec<&'static str> {
    builtins().into_iter().filter(|b| !b.hidden).map(|b| b.doc_type).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_lint_clean_and_produce_grammar_friendly_schemas() {
        for b in builtins() {
            let issues = lint(&b.def);
            assert!(issues.iter().all(|i| i.level != "error"), "{}: {issues:?}", b.key);
            let js = json_schema(&b.def);
            let text = js.to_string();
            assert!(!text.contains("$ref") && !text.contains("$defs") && !text.contains("oneOf"), "{}", b.key);
            assert_eq!(js["additionalProperties"], json!(false));
            // Every scalar carries value / evidence / confidence(0..100 integer).
            let first = b
                .def
                .fields
                .iter()
                .find(|f| matches!(f.kind, FieldKind::String | FieldKind::Date | FieldKind::Amount))
                .unwrap();
            let v = &js["properties"][&first.key];
            assert_eq!(v["properties"]["confidence"]["maximum"], json!(100));
            assert_eq!(v["properties"]["value"]["type"], json!(["string", "null"]));
            assert!(!prompt_summary(&b.def, true).is_empty());
        }
        assert_eq!(builtins().iter().filter(|b| !b.hidden).count(), 13);
        assert_eq!(builtins().iter().filter(|b| b.hidden).count(), 6, "the 0.1 business forms stay defined, hidden");
        let dts = builtin_doc_types();
        assert!(dts.contains(&"rfp") && dts.contains(&"estimate") && dts.contains(&"general"), "{dts:?}");
        assert!(!dts.contains(&"tax_invoice") && !dts.contains(&"receipt"), "hidden forms are not offered: {dts:?}");
        let keys: Vec<&str> = builtins().iter().map(|b| b.key).collect();
        let mut unique = keys.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), keys.len(), "keys are unique");
        assert!(builtin("tax_invoice").is_some_and(|b| b.hidden) && builtin("wbs").is_some_and(|b| !b.hidden));
    }

    #[test]
    fn lint_catches_grammar_problems() {
        let def = SchemaDef {
            fields: vec![
                FieldDef::new("Bad Key", "라벨", "", FieldKind::String).regex("[0-9]+"),
                FieldDef::new("choice", "선택", "", FieldKind::Enum),
                FieldDef::new("rows", "표", "", FieldKind::Table).with(vec![FieldDef::new(
                    "inner",
                    "내부",
                    "",
                    FieldKind::Table,
                )]),
                FieldDef::new("rows", "중복", "", FieldKind::String),
                FieldDef::new("value", "예약", "", FieldKind::String),
            ],
            rules: vec!["nope".into()],
        };
        let issues = lint(&def);
        let codes: Vec<&str> = issues.iter().map(|i| i.code.as_str()).collect();
        for c in
            ["key_format", "regex_anchor", "enum_empty", "nesting", "key_duplicate", "key_reserved", "rule_unknown"]
        {
            assert!(codes.contains(&c), "missing {c}: {codes:?}");
        }
        assert!(has_errors(&issues));
        assert!(!has_errors(&lint(&builtins()[0].def)));
    }

    #[test]
    fn metaschema_reply_becomes_a_draft_with_sanitised_keys() {
        let reply = json!({
            "name": "견적서", "name_en": "Quotation", "doc_type": "quotation",
            "description_ko": "견적", "description_en": "quote",
            "fields": [
                {"key": "Total Amount", "label_ko": "합계", "label_en": "Total", "type": "amount", "required": true, "regex_hint": null, "example": "1,000", "options": [], "columns": []},
                {"key": "", "label_ko": "거래처", "label_en": "", "type": "string", "required": false, "regex_hint": null, "example": null, "options": [], "columns": []},
                {"key": "total", "label_ko": "중복", "label_en": "Dup", "type": "string", "required": false, "regex_hint": null, "example": null, "options": [], "columns": []},
                {"key": "items", "label_ko": "품목", "label_en": "Items", "type": "table", "required": false, "regex_hint": null, "example": null, "options": [],
                 "columns": [{"key": "name", "label_ko": "품명", "label_en": "Name", "type": "string"}, {"key": "name", "label_ko": "수량", "label_en": "Qty", "type": "integer"}]},
                {"key": "kind", "label_ko": "종류", "label_en": "Kind", "type": "enum", "required": false, "regex_hint": null, "example": null, "options": [], "columns": []}
            ]
        });
        let d = draft_from_metaschema(&reply, SchemaSource::Nl);
        assert_eq!(d.doc_type, "quotation");
        let keys: Vec<&str> = d.definition.fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, vec!["total", "field_2", "total_2", "items", "kind"]);
        assert_eq!(d.definition.fields[3].fields[1].key, "name_2");
        assert_eq!(d.definition.fields[4].kind, FieldKind::String, "enum without options degrades to text");
        assert!(d.definition.fields[0].required);
        assert!(!has_errors(&lint(&d.definition)));
        assert_eq!(key_from_label("사업자등록번호", "x"), "brn");
        assert_eq!(key_from_label("Unit Price (KRW)", "x"), "unit_price_krw");
        assert_eq!(key_from_label("123", "x"), "f_123");
        assert_eq!(key_from_label("한글만", "fallback"), "fallback");
        assert_eq!(kind_from_label("계약금액"), FieldKind::Amount);
        assert_eq!(kind_from_label("주민등록번호"), FieldKind::Rrn);
        assert_eq!(kind_from_label("개업연월일"), FieldKind::Date);
    }

    #[test]
    fn classify_schema_lists_types_plus_other() {
        let s = classify_schema(&["receipt".into(), "contract".into()]);
        assert_eq!(s["properties"]["doc_type"]["enum"], json!(["receipt", "contract", "other"]));
        assert_eq!(s["properties"]["rotation"]["enum"], json!([0, 90, 180, 270]));
        let s2 = classify_schema(&["other".into()]);
        assert_eq!(s2["properties"]["doc_type"]["enum"], json!(["other"]));
    }
}
