//! Built-in generic templates (§3.13: 공문·견적서·거래명세서·계약서·보고서·회의록
//! plus the 보완요청서 of §3.15). Field keys follow the built-in pipeline
//! schemas (`supply_total`, `tax_total`, `grand_total`, `issue_date`,
//! `supplier.name`, …) so extracted fields of source documents map onto
//! template fields by key, and the item-table columns match the pipeline's
//! `items[]` columns (품명·규격·수량·단가·공급가액·세액·비고).

use crate::pipeline::schema::{FieldKind, SchemaRecord};

use super::{DraftColumn, TemplateField, TemplateInfo, TemplateKind, TemplateRef};

fn field(key: &str, ko: &str, en: &str, kind: FieldKind, required: bool) -> TemplateField {
    TemplateField {
        key: key.into(),
        label_ko: ko.into(),
        label_en: en.into(),
        kind: kind.as_str().into(),
        required,
        columns: Vec::new(),
    }
}

/// `en` is looked up through [`column_label_en`] by the UI; `DraftColumn` carries one label.
fn column(key: &str, ko: &str, _en: &str, kind: FieldKind) -> DraftColumn {
    DraftColumn { key: key.into(), label: ko.into(), kind: kind.as_str().into() }
}

/// English label of a column (the UI picks by language; `DraftColumn` carries one label).
pub fn column_label_en(key: &str) -> &'static str {
    match key {
        "item_name" => "Item",
        "spec" => "Spec",
        "quantity" => "Qty",
        "unit_price" => "Unit price",
        "supply_amount" => "Supply amount",
        "tax_amount" => "Tax",
        "note" => "Note",
        "item" => "Item",
        "owner" => "Owner",
        "due_date" => "Due",
        "document" => "Document",
        "issue" => "Finding",
        "evidence" => "Evidence",
        "action" => "Requested action",
        _ => "",
    }
}

fn table(key: &str, ko: &str, en: &str, columns: Vec<DraftColumn>, required: bool) -> TemplateField {
    TemplateField {
        key: key.into(),
        label_ko: ko.into(),
        label_en: en.into(),
        kind: FieldKind::Table.as_str().into(),
        required,
        columns,
    }
}

/// 품명·규격·수량·단가·공급가액·세액·비고 — the same keys as the pipeline's `items[]`.
pub fn item_columns() -> Vec<DraftColumn> {
    use FieldKind::*;
    vec![
        column("item_name", "품명", "Item", String),
        column("spec", "규격", "Spec", String),
        column("quantity", "수량", "Qty", Integer),
        column("unit_price", "단가", "Unit price", Amount),
        column("supply_amount", "공급가액", "Supply amount", Amount),
        column("tax_amount", "세액", "Tax", Amount),
        column("note", "비고", "Note", String),
    ]
}

fn party(prefix: &str, ko: &str, en: &str, name_required: bool) -> Vec<TemplateField> {
    use FieldKind::*;
    vec![
        field(&format!("{prefix}.name"), &format!("{ko} 상호"), &format!("{en} name"), String, name_required),
        field(
            &format!("{prefix}.brn"),
            &format!("{ko} 사업자등록번호"),
            &format!("{en} business reg. no."),
            Brn,
            false,
        ),
        field(
            &format!("{prefix}.representative"),
            &format!("{ko} 대표자"),
            &format!("{en} representative"),
            String,
            false,
        ),
        field(&format!("{prefix}.address"), &format!("{ko} 주소"), &format!("{en} address"), String, false),
        field(&format!("{prefix}.phone"), &format!("{ko} 전화"), &format!("{en} phone"), Phone, false),
    ]
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// The seven built-in templates in UI order.
pub fn list() -> Vec<TemplateInfo> {
    use FieldKind::*;
    vec![
        TemplateInfo {
            kind: TemplateKind::OfficialLetter,
            name_ko: "공문".into(),
            name_en: "Official letter".into(),
            description_ko: "수신·참조·제목·본문·붙임·발신 기관으로 구성된 공문(시행문).".into(),
            description_en: "Formal letter with recipient, cc, subject, body, attachments and sender.".into(),
            fields: vec![
                field("document_no", "문서번호", "Document no.", String, false),
                field("issue_date", "시행일자", "Date", Date, true),
                field("sender", "발신", "Sender", String, true),
                field("recipient", "수신", "Recipient", String, true),
                field("cc", "참조", "Cc", String, false),
                field("title", "제목", "Subject", String, true),
                field("contact_person", "담당자", "Contact person", String, false),
                field("phone", "연락처", "Phone", Phone, false),
                field("attachments", "붙임", "Attachments", String, false),
            ],
            sections_ko: strings(&["본문"]),
            sections_en: strings(&["Body"]),
            keywords: strings(&["공문", "공문서", "시행문", "협조문", "official letter", "formal letter", "letter"]),
        },
        TemplateInfo {
            kind: TemplateKind::Quotation,
            name_ko: "견적서".into(),
            name_en: "Quotation".into(),
            description_ko: "공급자·수신처·품목 표(수량×단가=공급가액)·합계·견적 조건.".into(),
            description_en: "Supplier, recipient, item table (qty × unit price), totals and terms.".into(),
            fields: {
                let mut f = vec![
                    field("document_no", "견적번호", "Quotation no.", String, false),
                    field("issue_date", "견적일자", "Quotation date", Date, true),
                    field("recipient", "수신", "Recipient", String, true),
                    field("valid_until", "유효기간", "Valid until", String, false),
                ];
                f.extend(party("supplier", "공급자", "Supplier", true));
                f.extend(vec![
                    field("contact_person", "담당자", "Contact person", String, false),
                    field("supply_total", "공급가액 합계", "Supply total", Amount, false),
                    field("tax_total", "세액 합계", "Tax total", Amount, false),
                    field("grand_total", "합계금액", "Grand total", Amount, true),
                    field("payment_terms", "결제조건", "Payment terms", String, false),
                    field("delivery_date", "납기", "Delivery", String, false),
                    field("remarks", "비고", "Remarks", String, false),
                    table("items", "품목", "Items", item_columns(), true),
                ]);
                f
            },
            sections_ko: strings(&["견적 조건", "비고"]),
            sections_en: strings(&["Terms", "Notes"]),
            keywords: strings(&["견적", "견적서", "quotation", "quote", "estimate"]),
        },
        TemplateInfo {
            kind: TemplateKind::TransactionStatement,
            name_ko: "거래명세서".into(),
            name_en: "Transaction statement".into(),
            description_ko: "공급자·공급받는자·품목 표·공급가액/세액/합계.".into(),
            description_en: "Supplier, buyer, item table and supply / tax / grand totals.".into(),
            fields: {
                let mut f = vec![
                    field("document_no", "명세서 번호", "Statement no.", String, false),
                    field("issue_date", "거래일자", "Transaction date", Date, true),
                ];
                f.extend(party("supplier", "공급자", "Supplier", true));
                f.extend(party("buyer", "공급받는자", "Buyer", true));
                f.extend(vec![
                    field("supply_total", "공급가액 합계", "Supply total", Amount, false),
                    field("tax_total", "세액 합계", "Tax total", Amount, false),
                    field("grand_total", "합계금액", "Grand total", Amount, true),
                    field("remarks", "비고", "Remarks", String, false),
                    table("items", "품목", "Items", item_columns(), true),
                ]);
                f
            },
            sections_ko: strings(&["비고"]),
            sections_en: strings(&["Notes"]),
            keywords: strings(&[
                "거래명세",
                "거래명세서",
                "거래명세표",
                "명세서",
                "transaction statement",
                "statement",
            ]),
        },
        TemplateInfo {
            kind: TemplateKind::Contract,
            name_ko: "계약서".into(),
            name_en: "Contract".into(),
            description_ko: "갑·을 당사자, 계약일·기간·금액·지급조건과 조항(제1조…).".into(),
            description_en: "Parties, date, period, amount, payment terms and numbered articles.".into(),
            fields: {
                let mut f = vec![
                    field("title", "계약명", "Title", String, true),
                    field("contract_date", "계약일", "Contract date", Date, true),
                ];
                f.extend(party("party_a", "갑", "Party A", true));
                f.extend(party("party_b", "을", "Party B", true));
                f.extend(vec![
                    field("period_start", "계약기간 시작", "Period start", Date, false),
                    field("period_end", "계약기간 종료", "Period end", Date, false),
                    field("amount", "계약금액", "Contract amount", Amount, false),
                    field("payment_terms", "지급조건", "Payment terms", String, false),
                ]);
                f
            },
            sections_ko: strings(&[
                "제1조 (목적)",
                "제2조 (계약금액)",
                "제3조 (계약기간)",
                "제4조 (대금 지급)",
                "제5조 (당사자의 의무)",
                "제6조 (계약의 해지)",
                "제7조 (분쟁 해결)",
            ]),
            sections_en: strings(&[
                "Article 1 (Purpose)",
                "Article 2 (Contract amount)",
                "Article 3 (Term)",
                "Article 4 (Payment)",
                "Article 5 (Obligations)",
                "Article 6 (Termination)",
                "Article 7 (Disputes)",
            ]),
            keywords: strings(&["계약", "계약서", "contract", "agreement"]),
        },
        TemplateInfo {
            kind: TemplateKind::Report,
            name_ko: "보고서".into(),
            name_en: "Report".into(),
            description_ko: "작성일·부서·작성자와 개요·현황·주요 내용·결론 구성의 업무 보고서.".into(),
            description_en: "Business report: overview, status, details and conclusion.".into(),
            fields: vec![
                field("title", "제목", "Title", String, true),
                field("issue_date", "작성일", "Date", Date, true),
                field("department", "부서", "Department", String, false),
                field("author", "작성자", "Author", String, false),
                field("recipient", "보고 대상", "Addressed to", String, false),
                field("period", "대상 기간", "Period", String, false),
                field("summary", "요약", "Summary", String, false),
            ],
            sections_ko: strings(&["개요", "현황", "주요 내용", "결론 및 건의"]),
            sections_en: strings(&["Overview", "Status", "Details", "Conclusion"]),
            keywords: strings(&["보고서", "보고", "정산서", "결과 보고", "report", "summary report"]),
        },
        TemplateInfo {
            kind: TemplateKind::Minutes,
            name_ko: "회의록".into(),
            name_en: "Meeting minutes".into(),
            description_ko: "일시·장소·참석자와 안건·논의 내용·결정 사항·조치 사항 표.".into(),
            description_en: "Date, place, attendees, agenda, discussion, decisions and action items.".into(),
            fields: vec![
                field("title", "회의명", "Meeting", String, true),
                field("meeting_date", "일시", "Date", Date, true),
                field("location", "장소", "Place", String, false),
                field("attendees", "참석자", "Attendees", String, true),
                field("recorder", "작성자", "Recorded by", String, false),
                table(
                    "action_items",
                    "조치 사항",
                    "Action items",
                    vec![
                        column("item", "항목", "Item", String),
                        column("owner", "담당자", "Owner", String),
                        column("due_date", "기한", "Due", Date),
                    ],
                    false,
                ),
            ],
            sections_ko: strings(&["안건", "논의 내용", "결정 사항", "향후 일정"]),
            sections_en: strings(&["Agenda", "Discussion", "Decisions", "Next steps"]),
            keywords: strings(&["회의록", "회의", "minutes", "meeting minutes", "meeting"]),
        },
        TemplateInfo {
            kind: TemplateKind::SupplementRequest,
            name_ko: "보완요청서".into(),
            name_en: "Supplement request".into(),
            description_ko: "수신·제목·근거·보완 항목 표·기한·서명으로 구성된 보완 요청 공문(§3.15).".into(),
            description_en:
                "Request for supplementary documents: recipient, subject, basis, item table, deadline, signature."
                    .into(),
            fields: vec![
                field("document_no", "문서번호", "Document no.", String, false),
                field("issue_date", "발송일", "Date", Date, true),
                field("recipient", "수신", "Recipient", String, true),
                field("sender", "발신", "Sender", String, false),
                field("title", "제목", "Subject", String, true),
                field("subject", "대상 건명", "Case", String, false),
                field("due_date", "보완 기한", "Deadline", Date, true),
                field("contact_person", "담당자", "Contact person", String, false),
                field("phone", "연락처", "Phone", Phone, false),
                table(
                    "items",
                    "보완 항목",
                    "Items",
                    vec![
                        column("document", "대상 문서", "Document", String),
                        column("issue", "지적 사항", "Finding", String),
                        column("evidence", "근거", "Evidence", String),
                        column("action", "조치 요청", "Requested action", String),
                    ],
                    true,
                ),
            ],
            sections_ko: strings(&["근거", "보완 요청 사항", "안내"]),
            sections_en: strings(&["Basis", "Items to supplement", "Notice"]),
            keywords: strings(&[
                "보완요청",
                "보완 요청",
                "보완요청서",
                "보완",
                "supplement request",
                "supplement",
                "deficiency",
            ]),
        },
    ]
}

/// One built-in template.
pub fn get(kind: TemplateKind) -> TemplateInfo {
    list().into_iter().find(|t| t.kind == kind).expect("every TemplateKind has a template")
}

/// Lower-cased text without whitespace / punctuation, for keyword and label matching.
pub fn compact(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_whitespace() && !matches!(c, '·' | ',' | '.' | ':' | '：' | '(' | ')' | '/' | '-' | '_' | '*' | '※')
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Built-in template whose keyword occurs in `text` (longest keyword wins, so
/// "보완요청서" beats "보고서" inside "보완요청서를 보고서 형식으로").
pub fn match_keywords(text: &str) -> Option<TemplateKind> {
    let hay = compact(text);
    if hay.is_empty() {
        return None;
    }
    let mut best: Option<(usize, TemplateKind)> = None;
    for t in list() {
        for k in &t.keywords {
            let kc = compact(k);
            if kc.is_empty() || !hay.contains(&kc) {
                continue;
            }
            if best.is_none_or(|(len, _)| kc.chars().count() > len) {
                best = Some((kc.chars().count(), t.kind));
            }
        }
    }
    best.map(|(_, k)| k)
}

impl TemplateInfo {
    pub fn name(&self, ko: bool) -> &str {
        if ko {
            &self.name_ko
        } else {
            &self.name_en
        }
    }
    pub fn sections(&self, ko: bool) -> &[String] {
        if ko {
            &self.sections_ko
        } else {
            &self.sections_en
        }
    }
    pub fn field(&self, key: &str) -> Option<&TemplateField> {
        self.fields.iter().find(|f| f.key == key)
    }
    pub fn scalars(&self) -> impl Iterator<Item = &TemplateField> {
        self.fields.iter().filter(|f| f.kind != "table")
    }
    pub fn tables(&self) -> impl Iterator<Item = &TemplateField> {
        self.fields.iter().filter(|f| f.kind == "table")
    }
}

impl TemplateField {
    pub fn label(&self, ko: bool) -> &str {
        if ko || self.label_en.is_empty() {
            &self.label_ko
        } else {
            &self.label_en
        }
    }
    pub fn is_table(&self) -> bool {
        self.kind == "table"
    }
}

/// Unified view of the layout source: a built-in template or a registered
/// form / pipeline schema (`SchemaDef` groups become `group.key` paths, tables
/// keep their columns).
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateSpec {
    pub template: TemplateRef,
    pub name_ko: String,
    pub name_en: String,
    /// Document type slug the generated file is registered with.
    pub doc_type: String,
    pub fields: Vec<TemplateField>,
    pub sections_ko: Vec<String>,
    pub sections_en: Vec<String>,
}

impl TemplateSpec {
    pub fn builtin(kind: TemplateKind) -> TemplateSpec {
        let t = get(kind);
        TemplateSpec {
            template: TemplateRef::Builtin { template: kind },
            name_ko: t.name_ko,
            name_en: t.name_en,
            doc_type: kind.as_str().to_string(),
            fields: t.fields,
            sections_ko: t.sections_ko,
            sections_en: t.sections_en,
        }
    }

    /// A registered form (or any pipeline schema) as a template.
    pub fn from_schema(rec: &SchemaRecord) -> TemplateSpec {
        let mut fields = Vec::new();
        for f in &rec.definition.fields {
            match f.kind {
                FieldKind::Table => {
                    let columns = f
                        .fields
                        .iter()
                        .filter(|c| c.kind.is_scalar())
                        .map(|c| DraftColumn {
                            key: c.key.clone(),
                            label: c.label_ko.clone(),
                            kind: c.kind.as_str().into(),
                        })
                        .collect();
                    fields.push(table(&f.key, &f.label_ko, &f.label_en, columns, f.required));
                }
                FieldKind::Group => {
                    for c in f.fields.iter().filter(|c| c.kind.is_scalar()) {
                        let ko = if c.label_ko.contains(&f.label_ko) {
                            c.label_ko.clone()
                        } else {
                            format!("{} {}", f.label_ko, c.label_ko)
                        };
                        let en = if c.label_en.is_empty() {
                            String::new()
                        } else {
                            format!("{} {}", f.label_en, c.label_en)
                        };
                        fields.push(field(&format!("{}.{}", f.key, c.key), &ko, &en, c.kind, c.required));
                    }
                }
                _ => fields.push(field(&f.key, &f.label_ko, &f.label_en, f.kind, f.required)),
            }
        }
        TemplateSpec {
            template: TemplateRef::Form { schema_id: rec.id.clone(), doc_id: rec.form_document_id.clone() },
            name_ko: rec.name.clone(),
            name_en: rec.name_en.clone().unwrap_or_else(|| rec.name.clone()),
            doc_type: rec.doc_type.clone(),
            fields,
            sections_ko: Vec::new(),
            sections_en: Vec::new(),
        }
    }

    pub fn builtin_kind(&self) -> Option<TemplateKind> {
        match &self.template {
            TemplateRef::Builtin { template } => Some(*template),
            TemplateRef::Form { .. } => None,
        }
    }
    pub fn name(&self, ko: bool) -> &str {
        if ko || self.name_en.is_empty() {
            &self.name_ko
        } else {
            &self.name_en
        }
    }
    pub fn sections(&self, ko: bool) -> &[String] {
        if ko {
            &self.sections_ko
        } else {
            &self.sections_en
        }
    }
    pub fn field(&self, key: &str) -> Option<&TemplateField> {
        self.fields.iter().find(|f| f.key == key)
    }
    pub fn scalars(&self) -> impl Iterator<Item = &TemplateField> {
        self.fields.iter().filter(|f| !f.is_table())
    }
    pub fn tables(&self) -> impl Iterator<Item = &TemplateField> {
        self.fields.iter().filter(|f| f.is_table())
    }
}
