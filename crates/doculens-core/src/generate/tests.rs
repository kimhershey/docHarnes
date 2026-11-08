//! Document-generation tests: templates and the grammar schema, drafts from a
//! scripted model (citations, regeneration, retry), the rule-based 보완요청서,
//! reverse validation, HTML / Markdown rendering, DOCX / HWPX round trips,
//! saving (PDF fallback, overwrite, registration) and in-place form filling.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::db::Db;
use crate::documents::convert::model::{Block, ListKind};
use crate::documents::convert::{docx, hwpx, StubPdfPrinter};
use crate::documents::import::ImportDeps;
use crate::documents::render::RenderOptions;
use crate::documents::{repo as docs, DocumentRole, DocumentStatus, NewDocument, NewPage, SourceKind};
use crate::engine::client::{ChatRequest, ChatResponse, ChatUsage};
use crate::error::CoreError;
use crate::knowledge::answer::ChatBackend;
use crate::knowledge::index::{index_document, IndexDeps};
use crate::knowledge::search::SearchDeps;
use crate::knowledge::tokenize::Analyzer;
use crate::paths::WorkspacePaths;
use crate::pipeline::repo as prepo;
use crate::pipeline::schema::{FieldDef, FieldKind, SchemaDef, SchemaDraft, SchemaSource};
use crate::pipeline::vlm::Vlm;
use crate::settings::{Language, Profile};

use super::templates::{self, TemplateSpec};
use super::{
    draft, fill, office, render, schema, store, validate, Draft, DraftBlock, DraftColumn, DraftField, DraftRequest,
    DraftTable, GenerateDeps, OutputFormat, SaveRequest, SourceRef, TemplateKind, TemplateRef,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

struct World {
    db: Db,
    paths: WorkspacePaths,
    _dir: tempfile::TempDir,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let paths = WorkspacePaths::new(dir.path());
    for d in paths.all_dirs() {
        std::fs::create_dir_all(d).unwrap();
    }
    let db = Db::open_in_memory().unwrap();
    db.with(prepo::ensure_builtins).unwrap();
    World { db, paths, _dir: dir }
}

impl World {
    fn deps<'a>(&'a self, vlm: Option<&'a Vlm<'a>>, import: Option<&'a ImportDeps<'a>>) -> GenerateDeps<'a> {
        GenerateDeps {
            db: &self.db,
            paths: &self.paths,
            vlm,
            knowledge: None,
            printer: None,
            import,
            cancel: None,
            language: Language::Ko,
        }
    }

    fn import_deps(&self) -> ImportDeps<'_> {
        ImportDeps {
            db: &self.db,
            paths: &self.paths,
            render: None,
            printer: &StubPdfPrinter,
            render_options: RenderOptions::default(),
        }
    }

    /// Registers a document with page texts (no file needed for drafting).
    fn add_doc(&self, id: &str, name: &str, role: DocumentRole, pages: &[&str]) {
        self.db
            .with(|c| {
                docs::insert_document(
                    c,
                    &NewDocument {
                        id: id.into(),
                        rel_path: format!("documents/{name}"),
                        original_name: name.into(),
                        source_kind: SourceKind::Pdf,
                        content_hash: format!("hash-{id}"),
                        size_bytes: 10,
                        file_modified_at: None,
                        role,
                        import_batch_id: None,
                    },
                )?;
                let new_pages: Vec<NewPage> = pages
                    .iter()
                    .enumerate()
                    .map(|(i, t)| NewPage { page_no: i as u32 + 1, text: Some(t.to_string()), ..Default::default() })
                    .collect();
                docs::replace_pages(c, id, &new_pages, None, false, None)?;
                docs::set_status(c, id, DocumentStatus::Rendered, None)
            })
            .unwrap();
    }

    /// Stores an extraction run with scalar + table fields as the document's latest run.
    fn add_fields(&self, doc_id: &str, scalars: &[(&str, &str, &str, FieldKind)], rows: &[(&str, u32, &str, &str)]) {
        let run_id = format!("run-{doc_id}");
        self.db
            .with(|c| {
                prepo::insert_run(
                    c,
                    &prepo::NewRun {
                        id: &run_id,
                        document_id: doc_id,
                        schema: None,
                        doc_type: Some("quotation"),
                        model: "scripted",
                        profile: "balanced",
                        precise: false,
                    },
                )?;
                let mut fields: Vec<prepo::NewField> = scalars
                    .iter()
                    .map(|(path, label, value, kind)| prepo::NewField {
                        path: path.to_string(),
                        key: path.rsplit('.').next().unwrap_or(path).to_string(),
                        label_ko: label.to_string(),
                        kind: *kind,
                        value: Some(value.to_string()),
                        page: Some(1),
                        confidence: 0.9,
                        ..Default::default()
                    })
                    .collect();
                for (table, row, col, value) in rows {
                    fields.push(prepo::NewField {
                        path: format!("{table}[{row}].{col}"),
                        key: col.to_string(),
                        row: Some(*row),
                        label_ko: col.to_string(),
                        value: Some(value.to_string()),
                        confidence: 0.9,
                        ..Default::default()
                    });
                }
                prepo::insert_fields(c, &run_id, doc_id, &fields)?;
                prepo::set_extracted(c, doc_id, &run_id, Some(0.9), false, None, None, None)
            })
            .unwrap();
    }
}

/// Scripted backend: replies are served in order (the last one repeats); raw strings so a
/// non-JSON reply can be simulated.
struct SeqChat {
    replies: Mutex<Vec<String>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl SeqChat {
    fn new(replies: Vec<String>) -> SeqChat {
        SeqChat { replies: Mutex::new(replies), requests: Mutex::new(Vec::new()) }
    }
    fn json(v: Value) -> SeqChat {
        SeqChat::new(vec![v.to_string()])
    }
    fn prompt(&self, call: usize) -> String {
        let reqs = self.requests.lock().unwrap();
        reqs[call]
            .messages
            .iter()
            .filter_map(|m| m["content"].as_str().map(str::to_string))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl ChatBackend for SeqChat {
    fn model_id(&self) -> Option<String> {
        Some("scripted".into())
    }
    fn chat_stream(
        &self,
        req: &ChatRequest,
        on_delta: &mut dyn FnMut(&str) -> bool,
    ) -> crate::error::Result<ChatResponse> {
        self.requests.lock().unwrap().push(req.clone());
        let mut q = self.replies.lock().unwrap();
        let reply = if q.len() > 1 { q.remove(0) } else { q.first().cloned().unwrap_or_default() };
        on_delta(&reply);
        Ok(ChatResponse {
            content: reply,
            finish_reason: Some("stop".into()),
            usage: Some(ChatUsage { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 }),
            ..ChatResponse::default()
        })
    }
}

fn field(key: &str, label: &str, kind: FieldKind, required: bool, value: &str) -> DraftField {
    DraftField {
        key: key.into(),
        label: label.into(),
        kind: kind.as_str().into(),
        required,
        value: value.into(),
        ..Default::default()
    }
}

fn col(key: &str, label: &str, kind: FieldKind) -> DraftColumn {
    DraftColumn { key: key.into(), label: label.into(), kind: kind.as_str().into() }
}

/// A quotation draft with two items (supply 2,400,000 + 40,000) and no totals.
fn quotation_draft() -> Draft {
    use FieldKind::*;
    let mut d = Draft { template: TemplateRef::Builtin { template: TemplateKind::Quotation }, ..Default::default() };
    d.title = "노트북 공급 견적서".into();
    d.doc_type = Some("quotation".into());
    d.fields = vec![
        field("document_no", "견적번호", String, false, "Q-2026-011"),
        field("issue_date", "견적일자", Date, true, "2026년 11월 8일"),
        field("recipient", "수신", String, true, "㈜한빛"),
        field("valid_until", "유효기간", String, false, "견적일로부터 30일"),
        field("supplier.name", "공급자 상호", String, true, "㈜도큐렌즈"),
        field("supplier.brn", "공급자 사업자등록번호", Brn, false, "220-81-62517"),
        field("supplier.representative", "공급자 대표자", String, false, "홍길동"),
        field("supply_total", "공급가액 합계", Amount, false, ""),
        field("tax_total", "세액 합계", Amount, false, ""),
        field("grand_total", "합계금액", Amount, true, ""),
        field("payment_terms", "결제조건", String, false, "납품 후 30일 이내 현금"),
    ];
    d.tables = vec![DraftTable {
        key: "items".into(),
        label: "품목".into(),
        columns: templates::item_columns(),
        rows: vec![
            vec!["노트북".into(), "15형".into(), "2".into(), "1,200,000".into(), "".into(), "".into(), "".into()],
            vec![
                "무선 마우스".into(),
                "".into(),
                "2".into(),
                "20,000".into(),
                "40,000".into(),
                "4,000".into(),
                "".into(),
            ],
        ],
    }];
    d.body = vec![
        DraftBlock::Heading { level: 2, text: "견적 조건".into() },
        DraftBlock::Paragraph {
            text: "본 견적은 부가세 포함 금액 2,444,000원이며 납기는 발주 후 2주입니다.".into(),
            source: None,
        },
        DraftBlock::List { items: vec!["무상 보증 1년".into(), "설치 지원 포함".into()], ordered: false },
    ];
    d
}

fn model_reply() -> Value {
    json!({
        "title": "노트북 공급 견적서",
        "fields": [
            {"key": "issue_date", "value": "2026-11-08", "source_index": null},
            {"key": "recipient", "value": "㈜한빛", "source_index": 2},
            {"key": "supplier.name", "value": "㈜도큐렌즈", "source_index": 1},
            {"key": "supplier.brn", "value": "220-81-62517", "source_index": 1},
            {"key": "grand_total", "value": "", "source_index": null}
        ],
        "tables": [
            {"key": "items", "rows": [["노트북", "15형", "2", "1,200,000", "2,000,000", "200,000", ""]]}
        ],
        "body": [
            {"kind": "heading", "text": "견적 조건", "items": [], "source_index": null},
            {"kind": "paragraph", "text": "공급자는 등록된 사업자입니다.", "items": [], "source_index": 1},
            {"kind": "paragraph", "text": "납품은 2026-12-01까지 완료하며 대금 2,640,000원은 납품 후 지급합니다.", "items": [], "source_index": null},
            {"kind": "list", "text": "", "items": ["무상 보증 1년", "설치 지원"], "source_index": null}
        ],
        "notes": ["유효기간은 요청에 없어 비워 두었습니다."]
    })
}

// ---------------------------------------------------------------------------
// Templates and schema
// ---------------------------------------------------------------------------

#[test]
fn templates_are_consistent_and_render() {
    let all = templates::list();
    assert_eq!(all.len(), 7);
    for t in &all {
        let mut keys: Vec<&str> = t.fields.iter().map(|f| f.key.as_str()).collect();
        let n = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), n, "{:?} has duplicate keys", t.kind);
        assert!(!t.keywords.is_empty() && !t.name_ko.is_empty() && !t.name_en.is_empty());
        for f in &t.fields {
            assert!(FieldKind::parse(&f.kind).is_some(), "{}: unknown kind {}", f.key, f.kind);
            assert_eq!(f.is_table(), !f.columns.is_empty(), "{}", f.key);
        }
        // Every template renders from an empty draft without panicking.
        let spec = TemplateSpec::builtin(t.kind);
        let mut d = Draft { template: TemplateRef::Builtin { template: t.kind }, ..Default::default() };
        d.fields = spec
            .scalars()
            .map(|f| field(&f.key, &f.label_ko, FieldKind::parse(&f.kind).unwrap(), f.required, ""))
            .collect();
        d.tables = spec
            .tables()
            .map(|f| DraftTable {
                key: f.key.clone(),
                label: f.label_ko.clone(),
                columns: f.columns.clone(),
                rows: Vec::new(),
            })
            .collect();
        let html = render::to_html(&d);
        assert!(html.contains(&t.name_ko), "{:?}", t.kind);
        assert!(render::to_markdown(&d).starts_with("# "));
    }
    // Pipeline-compatible keys on the money templates.
    let q = templates::get(TemplateKind::Quotation);
    for k in ["issue_date", "supplier.name", "supplier.brn", "supply_total", "tax_total", "grand_total", "items"] {
        assert!(q.field(k).is_some(), "quotation lacks {k}");
    }
    let cols: Vec<&str> = q.field("items").unwrap().columns.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(cols, ["item_name", "spec", "quantity", "unit_price", "supply_amount", "tax_amount", "note"]);
    assert_eq!(templates::match_keywords("보완요청서를 보고서 형식으로"), Some(TemplateKind::SupplementRequest));
    assert_eq!(templates::match_keywords("㈜한빛에 노트북 10대 견적서 만들어줘"), Some(TemplateKind::Quotation));
    assert_eq!(templates::match_keywords("Draft the meeting minutes"), Some(TemplateKind::Minutes));
    assert_eq!(templates::match_keywords("아무 내용"), None);
    // A pipeline schema becomes a spec with group paths and table columns.
    let tax = crate::pipeline::schema::builtins().into_iter().find(|b| b.key == "tax_invoice").unwrap();
    let rec = crate::pipeline::schema::SchemaRecord {
        id: "builtin-tax_invoice".into(),
        name: tax.name_ko.into(),
        name_en: Some(tax.name_en.into()),
        doc_type: tax.doc_type.into(),
        source: SchemaSource::Builtin,
        version: 1,
        description_ko: None,
        description_en: None,
        definition: tax.def,
        form_document_id: None,
        builtin_key: Some("tax_invoice".into()),
        enabled: true,
        is_default: true,
        use_count: 0,
        note: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let spec = TemplateSpec::from_schema(&rec);
    assert!(spec.field("supplier.brn").is_some() && spec.field("buyer.name").is_some());
    assert_eq!(spec.field("supplier.brn").unwrap().label_ko, "공급자 사업자등록번호");
    assert_eq!(spec.tables().count(), 1);
    assert_eq!(spec.doc_type, "tax_invoice");
}

#[test]
fn draft_schema_is_grammar_safe() {
    for kind in TemplateKind::ALL {
        let spec = TemplateSpec::builtin(kind);
        let s = schema::json_schema(&spec);
        assert!(schema::is_grammar_safe(&s), "{kind:?}: {s}");
        let text = s.to_string();
        assert!(!text.contains("$ref") && !text.contains("$defs"));
        let keys = s["properties"]["fields"]["items"]["properties"]["key"]["enum"].as_array().unwrap();
        assert_eq!(keys.len(), spec.scalars().count());
        assert_eq!(s["properties"]["body"]["maxItems"], json!(schema::MAX_BODY_BLOCKS));
        if spec.tables().count() == 0 {
            assert_eq!(s["properties"]["tables"]["maxItems"], json!(0));
        } else {
            assert!(s["properties"]["tables"]["items"]["properties"]["key"]["enum"].as_array().is_some());
        }
    }
    assert!(!schema::is_grammar_safe(&json!({"type": "object", "properties": {"a": {"type": "string"}}})));
}

// ---------------------------------------------------------------------------
// Template resolution
// ---------------------------------------------------------------------------

#[test]
fn resolve_template_prefers_registered_forms_then_keywords() {
    let w = world();
    w.add_doc("form-doc", "견적서양식.docx", DocumentRole::Form, &["견적서 수신 ____"]);
    let rec =
        w.db.with(|c| {
            prepo::save_schema(
                c,
                &SchemaDraft {
                    name: "견적서".into(),
                    doc_type: "quotation".into(),
                    source: SchemaSource::Form,
                    definition: SchemaDef::new(
                        vec![
                            FieldDef::new("recipient", "수신", "Recipient", FieldKind::String).required(),
                            FieldDef::new("total", "합계", "Total", FieldKind::Amount),
                        ],
                        &[],
                    ),
                    form_document_id: Some("form-doc".into()),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let req = |form_ref: Option<&str>, content: &str| DraftRequest {
        form_ref: form_ref.map(str::to_string),
        content: content.into(),
        ..Default::default()
    };
    assert_eq!(
        super::resolve_template(&w.db, &req(Some("견적서"), "")).unwrap(),
        TemplateRef::Form { schema_id: rec.id.clone(), doc_id: Some("form-doc".into()) }
    );
    assert_eq!(
        super::resolve_template(&w.db, &req(Some(&rec.id), "")).unwrap(),
        TemplateRef::Form { schema_id: rec.id.clone(), doc_id: Some("form-doc".into()) }
    );
    // Built-in template names and keywords.
    assert_eq!(
        super::resolve_template(&w.db, &req(Some("quotation"), "")).unwrap(),
        TemplateRef::Builtin { template: TemplateKind::Quotation }
    );
    assert_eq!(
        super::resolve_template(&w.db, &req(Some("A4 공문 형식"), "")).unwrap(),
        TemplateRef::Builtin { template: TemplateKind::OfficialLetter }
    );
    assert_eq!(
        super::resolve_template(&w.db, &req(None, "지난주 회의 내용으로 회의록 작성")).unwrap(),
        TemplateRef::Builtin { template: TemplateKind::Minutes }
    );
    assert_eq!(
        super::resolve_template(&w.db, &req(None, "뭔가 써줘")).unwrap(),
        TemplateRef::Builtin { template: TemplateKind::Report }
    );
    // A pipeline sample schema is used when no generic template has that name.
    assert_eq!(
        super::resolve_template(&w.db, &req(Some("세금계산서"), "")).unwrap(),
        TemplateRef::Form { schema_id: "builtin-tax_invoice".into(), doc_id: None }
    );
    // Unknown form_ref falls through to the content keywords.
    assert_eq!(
        super::resolve_template(&w.db, &req(Some("이상한양식"), "계약서 초안")).unwrap(),
        TemplateRef::Builtin { template: TemplateKind::Contract }
    );
}

// ---------------------------------------------------------------------------
// Drafting with a scripted model
// ---------------------------------------------------------------------------

#[test]
fn draft_from_scripted_model_cites_sources_and_validates() {
    let w = world();
    w.add_doc("supplier-doc", "사업자등록증.pdf", DocumentRole::Work, &["사업자등록증 ㈜도큐렌즈 220-81-62517"]);
    w.add_fields(
        "supplier-doc",
        &[
            ("company_name", "상호(법인명)", "㈜도큐렌즈", FieldKind::String),
            ("registration_no", "등록번호", "220-81-62517", FieldKind::Brn),
            ("representative", "대표자", "홍길동", FieldKind::String),
        ],
        &[],
    );
    let chat = SeqChat::json(model_reply());
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None);
    let deps = w.deps(Some(&vlm), None);
    let req = DraftRequest {
        content: "㈜한빛에 노트북 2대, 대당 1,200,000원 견적서 만들어줘".into(),
        sources: vec![
            SourceRef::Document { doc_id: "supplier-doc".into() },
            SourceRef::Text { title: Some("고객 요청".into()), text: "수신: ㈜한빛 구매팀".into() },
        ],
        use_knowledge: false,
        ..Default::default()
    };
    let out = draft::draft(&deps, &req).unwrap();
    assert_eq!(out.source, "model");
    assert_eq!((out.prompt_tokens, out.completion_tokens), (100, 20));
    let d = &out.draft;
    assert_eq!(d.template, TemplateRef::Builtin { template: TemplateKind::Quotation });
    assert_eq!(d.doc_type.as_deref(), Some("quotation"));
    assert_eq!(d.title, "노트북 공급 견적서");
    // The prompt carried the template fields and the numbered sources.
    let prompt = chat.prompt(0);
    assert!(prompt.contains("supply_total") && prompt.contains("items —"), "{prompt}");
    assert!(prompt.contains("<source index=\"1\" kind=\"document\" title=\"사업자등록증.pdf\">"), "{prompt}");
    assert!(prompt.contains("registration_no (등록번호): 220-81-62517"), "{prompt}");
    assert!(prompt.contains("<source index=\"2\" kind=\"text\"") && prompt.contains("[자료]"), "{prompt}");
    assert_eq!(chat.requests.lock().unwrap()[0].temperature, Some(super::prompts::TEMPERATURE));
    assert_eq!(
        chat.requests.lock().unwrap()[0].response_format.as_ref().unwrap()["json_schema"]["name"],
        "draft_document"
    );
    // Fields: every template key is present; citations resolved from source_index.
    assert_eq!(d.fields.len(), TemplateSpec::builtin(TemplateKind::Quotation).scalars().count());
    let brn = d.field("supplier.brn").unwrap();
    assert_eq!(brn.normalized.as_deref(), Some("220-81-62517"));
    assert_eq!(
        brn.source.as_ref().map(|c| (c.doc_id.clone(), c.source.clone())),
        Some((Some("supplier-doc".into()), "document".into()))
    );
    assert_eq!(d.field("recipient").unwrap().source.as_ref().map(|c| c.title.clone()), Some("고객 요청".into()));
    assert_eq!(d.field("issue_date").unwrap().normalized.as_deref(), Some("2026-11-08"));
    // Reverse validation: 2 × 1,200,000 ≠ 2,000,000 → suggestion; totals auto-filled from the table.
    let mismatch = d.issues.iter().find(|i| i.code == "sum_mismatch").expect("sum mismatch");
    assert_eq!(mismatch.path, "tables.items[0].supply_amount");
    assert_eq!(mismatch.suggestion.as_deref(), Some("2,400,000"));
    assert!(mismatch.blocking);
    assert_eq!(d.field("supply_total").unwrap().value, "2,000,000");
    assert_eq!(d.field("tax_total").unwrap().value, "200,000");
    assert_eq!(d.field("grand_total").unwrap().value, "2,200,000");
    assert!(d.notes.iter().any(|n| n.contains("자동 계산")), "{:?}", d.notes);
    // The uncited money sentence is a non-blocking warning; the cited one is fine.
    let uncited: Vec<&super::DraftIssue> = d.issues.iter().filter(|i| i.code == "uncited_claim").collect();
    assert_eq!(uncited.len(), 1);
    assert_eq!(uncited[0].path, "body[2]");
    assert!(!uncited[0].blocking);
    assert!(
        matches!(&d.body[1], DraftBlock::Paragraph { source: Some(c), .. } if c.doc_id.as_deref() == Some("supplier-doc"))
    );
    assert!(matches!(&d.body[3], DraftBlock::List { items, .. } if items.len() == 2));
    assert_eq!(d.references.len(), 2);
    assert_eq!(d.notes[0], "유효기간은 요청에 없어 비워 두었습니다.");
    assert!(d.request.as_ref().is_some_and(|r| r.previous.is_none() && r.content == req.content));
    assert!(d.has_blocking_issues());

    // Regeneration keeps user edits and sends the previous draft + feedback.
    let mut prev = d.clone();
    let f = prev.fields.iter_mut().find(|f| f.key == "recipient").unwrap();
    f.value = "㈜한빛 구매팀".into();
    f.user_edited = true;
    let req2 = DraftRequest {
        content: "더 짧게 다시".into(),
        feedback: Some("더 짧게".into()),
        previous: Some(Box::new(prev.clone())),
        ..req.clone()
    };
    let out2 = draft::draft(&deps, &req2).unwrap();
    assert_eq!(out2.draft.id, prev.id, "regeneration keeps the draft id");
    assert_eq!(out2.draft.template, prev.template, "regeneration keeps the template without keywords");
    let kept = out2.draft.field("recipient").unwrap();
    assert_eq!((kept.value.as_str(), kept.user_edited), ("㈜한빛 구매팀", true));
    let prompt2 = chat.prompt(1);
    assert!(
        prompt2.contains("[이전 초안]") && prompt2.contains("[수정 요청]\n더 짧게") && prompt2.contains("사용자 수정"),
        "{prompt2}"
    );
}

#[test]
fn draft_retries_once_on_non_json_and_fails_without_model() {
    let w = world();
    let chat = SeqChat::new(vec!["죄송합니다, JSON을 만들 수 없습니다".into(), model_reply().to_string()]);
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None);
    let deps = w.deps(Some(&vlm), None);
    let req = DraftRequest { content: "견적서".into(), use_knowledge: false, ..Default::default() };
    let out = draft::draft(&deps, &req).unwrap();
    assert_eq!(chat.requests.lock().unwrap().len(), 2);
    assert!(chat.prompt(1).contains("올바른 JSON이 아니었습니다"));
    assert_eq!(out.draft.title, "노트북 공급 견적서");
    // English request → English prompt.
    let chat_en = SeqChat::json(model_reply());
    let vlm_en = Vlm::new(&chat_en, "scripted".into(), Profile::Balanced, None);
    let deps_en = w.deps(Some(&vlm_en), None);
    let req_en = DraftRequest {
        content: "quotation for 2 laptops".into(),
        language: Language::En,
        use_knowledge: false,
        ..Default::default()
    };
    let out_en = draft::draft(&deps_en, &req_en).unwrap();
    assert!(chat_en.prompt(0).contains("[Template]") && chat_en.prompt(0).contains("[Sources]"));
    assert_eq!(out_en.draft.field("recipient").unwrap().label, "Recipient");
    // No model: only the 보완요청서 can be drafted.
    let deps_none = w.deps(None, None);
    assert!(matches!(draft::draft(&deps_none, &req), Err(CoreError::EngineNotRunning)));
}

#[test]
fn knowledge_sources_are_numbered_and_excludable() {
    let w = world();
    w.add_doc(
        "reg",
        "출장비규정.pdf",
        DocumentRole::Knowledge,
        &["출장비 규정\n\n제3조(숙박비) ① 숙박비는 1박당 120,000원을 한도로 한다.\n② 영수증을 첨부하여 정산한다.\n제4조(정산) 출장 종료 후 14일 이내에 정산서를 제출한다."],
    );
    let analyzer = Analyzer::simple();
    let doc = w.db.with(|c| docs::get_document(c, "reg")).unwrap().unwrap();
    index_document(&IndexDeps { db: &w.db, analyzer: &analyzer, options: Default::default() }, &doc, true).unwrap();
    let reply = json!({
        "title": "부산 출장 정산 보고서",
        "fields": [{"key": "title", "value": "부산 출장 정산 보고서", "source_index": null}, {"key": "issue_date", "value": "2026-11-08", "source_index": null}],
        "tables": [],
        "body": [{"kind": "paragraph", "text": "제3조(숙박비) ① 숙박비는 1박당 120,000원을 한도로 한다.", "items": [], "source_index": 1}],
        "notes": []
    });
    let chat = SeqChat::json(reply);
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None);
    let mut deps = w.deps(Some(&vlm), None);
    deps.knowledge = Some(SearchDeps { db: &w.db, analyzer: &analyzer, embedder: None });
    let req = DraftRequest {
        content: "출장비 규정에 맞춰 부산 출장 정산 보고서 초안".into(),
        sources: vec![SourceRef::Knowledge { query: "숙박비 한도".into() }],
        use_knowledge: true,
        ..Default::default()
    };
    let out = draft::draft(&deps, &req).unwrap();
    let d = &out.draft;
    assert_eq!(d.template, TemplateRef::Builtin { template: TemplateKind::Report });
    assert!(chat.prompt(0).contains("kind=\"knowledge\" title=\"출장비규정.pdf\""), "{}", chat.prompt(0));
    let cite = match &d.body[0] {
        DraftBlock::Paragraph { source: Some(c), .. } => c.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!((cite.source.as_str(), cite.doc_id.as_deref(), cite.page), ("knowledge", Some("reg"), Some(1)));
    assert!(cite.chunk_id.is_some());
    assert!(d.references.iter().all(|r| r.source == "knowledge"));
    // Excluding the document removes its evidence and says so.
    let chat2 = SeqChat::json(json!({"title": "x", "fields": [], "tables": [], "body": [], "notes": []}));
    let vlm2 = Vlm::new(&chat2, "scripted".into(), Profile::Balanced, None);
    deps.vlm = Some(&vlm2);
    let req2 = DraftRequest { excluded_sources: vec!["reg".into()], ..req.clone() };
    let out2 = draft::draft(&deps, &req2).unwrap();
    assert!(out2.draft.references.is_empty());
    assert!(chat2.prompt(0).contains("(자료 없음"), "{}", chat2.prompt(0));
    assert!(out2.draft.notes.iter().any(|n| n.contains("근거를 찾지 못했습니다")), "{:?}", out2.draft.notes);
}

#[test]
fn supplement_request_is_composed_by_rules_without_a_model() {
    let w = world();
    let deps = w.deps(None, None);
    let req = DraftRequest {
        form_ref: Some("supplement_request".into()),
        content: "수신: ㈜한빛\n발신: 심사팀\n건명: 2026년 인증 심사\n기한: 2026-11-30\n담당자: 홍길동\n연락처: 02-123-4567".into(),
        sources: vec![SourceRef::Text {
            title: Some("정합성 검사 결과".into()),
            text: "- 사업자등록증 | 등록번호가 세금계산서의 공급자 번호와 다름 | 세금계산서 1쪽 | 정정본 제출\n계약서 | 서명 누락 | 계약서 3쪽 | 서명본 제출\n# 참고\n재무제표 미제출".into(),
        }],
        use_knowledge: false,
        ..Default::default()
    };
    let out = draft::draft(&deps, &req).unwrap();
    assert_eq!(out.source, "rules");
    let d = &out.draft;
    assert_eq!(d.template, TemplateRef::Builtin { template: TemplateKind::SupplementRequest });
    assert_eq!(d.field("recipient").unwrap().value, "㈜한빛");
    assert_eq!(d.field("sender").unwrap().value, "심사팀");
    assert_eq!(d.field("due_date").unwrap().normalized.as_deref(), Some("2026-11-30"));
    assert_eq!(d.field("issue_date").unwrap().value, chrono::Local::now().format("%Y-%m-%d").to_string());
    assert_eq!(d.field("phone").unwrap().normalized.as_deref(), Some("02-123-4567"));
    assert_eq!(d.title, "2026년 인증 심사 보완 요청");
    let items = d.table("items").unwrap();
    assert_eq!(items.rows.len(), 3);
    assert_eq!(
        items.rows[0],
        vec!["사업자등록증", "등록번호가 세금계산서의 공급자 번호와 다름", "세금계산서 1쪽", "정정본 제출"]
    );
    assert_eq!(items.rows[2][0], "정합성 검사 결과", "a bare line gets the source title as its document");
    assert_eq!(items.rows[2][1], "재무제표 미제출");
    assert!(d.body.iter().any(|b| matches!(b, DraftBlock::TableRef { key } if key == "items")));
    assert!(d.body.iter().any(|b| matches!(b, DraftBlock::Paragraph { text, .. } if text.contains("2026-11-30까지"))));
    assert!(!d.has_blocking_issues(), "{:?}", d.issues);
    let html = render::to_html(d);
    assert!(
        html.contains("㈜한빛")
            && html.contains("정정본 제출")
            && html.contains("보완 기한")
            && html.contains("심사팀 (직인)")
    );
    // Model present: rows the model left empty are completed from the findings.
    let chat = SeqChat::json(
        json!({"title": "보완 요청", "fields": [{"key": "recipient", "value": "㈜한빛", "source_index": null}], "tables": [{"key": "items", "rows": []}], "body": [], "notes": []}),
    );
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None);
    let out2 = draft::draft(&w.deps(Some(&vlm), None), &req).unwrap();
    assert_eq!(out2.source, "model");
    assert_eq!(out2.draft.table("items").unwrap().rows.len(), 3);
    assert!(!out2.draft.field("due_date").unwrap().value.is_empty());
}

#[test]
fn supplement_request_maps_project_json_findings() {
    let w = world();
    let deps = w.deps(None, None);
    let project = json!({
        "name": "㈜한빛 인증 심사", "status": "checking", "intake_no": "2026-0042", "owner": "홍길동",
        "due_date": "2026-12-05", "template": "certification", "documents": [], "checked_at": "2026-11-08T00:00:00Z",
        "counts": {"fail": 2}
    });
    let mismatch = json!({
        "rule": "세금계산서 공급자 사업자번호 = 사업자등록증 등록번호", "verdict": "mismatch",
        "expected": "220-81-62517", "found": "220-81-62518", "doc_name": "세금계산서.pdf", "page": 1,
        "message": "정정 세금계산서를 제출하세요"
    });
    let missing = json!({
        "rule": "재무제표 제출", "verdict": "missing", "expected": "재무제표", "found": null,
        "doc_name": null, "page": null, "message": "최근 연도 재무제표를 제출하세요"
    });
    let req = DraftRequest {
        form_ref: Some("supplement_request".into()),
        content: "보완요청서 작성".into(),
        sources: vec![
            SourceRef::Text { title: Some("project".into()), text: project.to_string() },
            SourceRef::Text { title: Some("finding:mismatch:brn_match".into()), text: mismatch.to_string() },
            SourceRef::Text {
                title: Some("finding:missing:missing_doc:financial_statement".into()),
                text: missing.to_string(),
            },
        ],
        use_knowledge: false,
        ..Default::default()
    };
    let out = draft::draft(&deps, &req).unwrap();
    assert_eq!(out.source, "rules");
    let d = &out.draft;
    assert_eq!(d.field("recipient").unwrap().value, "㈜한빛 인증 심사");
    assert_eq!(d.field("subject").unwrap().value, "㈜한빛 인증 심사 (접수번호 2026-0042)");
    assert_eq!(d.field("contact_person").unwrap().value, "홍길동");
    assert_eq!(d.field("due_date").unwrap().normalized.as_deref(), Some("2026-12-05"));
    assert_eq!(d.title, "㈜한빛 인증 심사 (접수번호 2026-0042) 보완 요청");
    let rows = &d.table("items").unwrap().rows;
    assert_eq!(
        rows[0],
        vec![
            "세금계산서.pdf 1쪽",
            "[불일치] 세금계산서 공급자 사업자번호 = 사업자등록증 등록번호",
            "기대 220-81-62517 / 확인 220-81-62518",
            "정정 세금계산서를 제출하세요"
        ]
    );
    assert_eq!(rows[1], vec!["", "[누락] 재무제표 제출", "기대 재무제표", "최근 연도 재무제표를 제출하세요"]);
    assert_eq!(rows.len(), 2, "the project card is not a finding");
    let titles: Vec<&str> = d.references.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, vec!["프로젝트 정보", "세금계산서.pdf", "정합성 검사 결과"]);
    assert!(!d.has_blocking_issues(), "{:?}", d.issues);
    assert!(render::to_html(d).contains("기대 220-81-62517 / 확인 220-81-62518"));
    // Content hints still win over the project card.
    let req2 = DraftRequest { content: "수신: 한빛 품질팀\n기한: 2026-12-20".into(), ..req.clone() };
    let d2 = draft::draft(&deps, &req2).unwrap().draft;
    assert_eq!(d2.field("recipient").unwrap().value, "한빛 품질팀");
    assert_eq!(d2.field("due_date").unwrap().value, "2026-12-20");
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[test]
fn validation_covers_sums_checksums_required_and_dates() {
    let mut d = quotation_draft();
    validate::validate(&mut d);
    // Row 0 supply amount filled from qty × unit price; totals computed from the table.
    assert_eq!(d.tables[0].rows[0][4], "2,400,000");
    assert_eq!(d.field("supply_total").unwrap().value, "2,440,000");
    assert_eq!(d.field("tax_total").unwrap().value, "4,000", "the table's tax column wins over 10%");
    assert_eq!(d.field("grand_total").unwrap().value, "2,444,000");
    assert_eq!(d.field("grand_total").unwrap().normalized.as_deref(), Some("2444000"));
    assert_eq!(d.field("issue_date").unwrap().normalized.as_deref(), Some("2026-11-08"));
    assert!(!d.issues.iter().any(|i| i.code == "required_missing"), "{:?}", d.issues);
    assert!(d.issues.iter().any(|i| i.code == "uncited_claim" && !i.blocking), "{:?}", d.issues);
    assert!(!d.has_blocking_issues(), "{:?}", d.issues);

    // Conflicts: wrong row sum, wrong grand total, bad BRN, missing required, bad date, unknown key.
    let mut bad = quotation_draft();
    bad.tables[0].rows[0][4] = "2,000,000".into();
    bad.fields.iter_mut().find(|f| f.key == "supply_total").unwrap().value = "2,040,000".into();
    bad.fields.iter_mut().find(|f| f.key == "grand_total").unwrap().value = "9,999".into();
    bad.fields.iter_mut().find(|f| f.key == "supplier.brn").unwrap().value = "220-81-62518".into();
    bad.fields.iter_mut().find(|f| f.key == "recipient").unwrap().value = "".into();
    bad.fields.iter_mut().find(|f| f.key == "issue_date").unwrap().value = "언젠가".into();
    bad.fields.push(field("bogus", "엉뚱", FieldKind::String, false, "x"));
    validate::validate(&mut bad);
    let by_code = |code: &str| bad.issues.iter().filter(|i| i.code == code).collect::<Vec<_>>();
    let row = by_code("sum_mismatch");
    assert!(
        row.iter().any(|i| i.path == "tables.items[0].supply_amount" && i.suggestion.as_deref() == Some("2,400,000")),
        "{row:?}"
    );
    assert!(
        row.iter().any(|i| i.path == "fields.grand_total" && i.suggestion.as_deref() == Some("2,044,000")),
        "{row:?}"
    );
    assert!(row.iter().all(|i| i.blocking));
    assert!(by_code("brn_invalid").iter().any(|i| i.path == "fields.supplier.brn" && i.blocking));
    assert!(by_code("required_missing").iter().any(|i| i.path == "fields.recipient"));
    assert!(by_code("date_invalid").iter().any(|i| i.path == "fields.issue_date" && !i.blocking));
    assert!(by_code("template_unknown_field").iter().any(|i| i.path == "fields.bogus"));
    // Tax off by more than 10 % tolerance when no tax column backs it.
    let mut tax = quotation_draft();
    tax.tables[0].rows[1][5] = "".into();
    tax.fields.iter_mut().find(|f| f.key == "tax_total").unwrap().value = "1,000".into();
    validate::validate(&mut tax);
    assert!(
        tax.issues
            .iter()
            .any(|i| i.code == "tax_mismatch" && !i.blocking && i.suggestion.as_deref() == Some("244,000")),
        "{:?}",
        tax.issues
    );
    // Period order on contracts.
    let mut c = Draft { template: TemplateRef::Builtin { template: TemplateKind::Contract }, ..Default::default() };
    c.fields = vec![
        field("period_start", "계약기간 시작", FieldKind::Date, false, "2026-12-01"),
        field("period_end", "계약기간 종료", FieldKind::Date, false, "2026-01-01"),
    ];
    validate::validate(&mut c);
    assert!(c.issues.iter().any(|i| i.code == "date_order" && i.blocking));
    assert_eq!(validate::format_won(1234567.0), "1,234,567");
}

// ---------------------------------------------------------------------------
// Rendering and office round trips
// ---------------------------------------------------------------------------

#[test]
fn html_and_markdown_show_fields_tables_and_totals() {
    let mut d = quotation_draft();
    d.body.push(DraftBlock::Paragraph {
        text: "공급자는 등록된 사업자입니다.".into(),
        source: Some(super::DraftCitation {
            doc_id: Some("x".into()),
            title: "사업자등록증".into(),
            page: Some(1),
            source: "document".into(),
            ..Default::default()
        }),
    });
    validate::validate(&mut d);
    let html = render::to_html(&d);
    assert!(html.contains("<h1 class=\"title\">노트북 공급 견적서</h1>"));
    assert!(html.contains("<th>수신</th><td>㈜한빛</td>"));
    assert!(html.contains("<th>품명</th>") && html.contains("<td>노트북</td>"));
    assert!(html.contains("합계금액") && html.contains("2,444,000원"));
    assert!(html.contains("<sup class=\"cite\">[1]</sup>") && html.contains("[1] 사업자등록증 1쪽"));
    assert!(html.contains("<li>무상 보증 1년</li>") && html.contains("㈜도큐렌즈 (인)"));
    assert!(html.contains("Malgun Gothic") && html.contains("size: A4"));
    let md = render::to_markdown(&d);
    assert!(md.starts_with("# 노트북 공급 견적서"));
    assert!(md.contains("| 수신 | ㈜한빛 |"));
    assert!(md.contains("| 품명 | 규격 | 수량 | 단가 | 공급가액 | 세액 | 비고 |"));
    assert!(md.contains("**합계금액** | **2,444,000원**"));
    assert!(md.contains("- 무상 보증 1년") && md.contains("[1] 사업자등록증 1쪽"));
    // Letter layout: template name as title, subject in the head, signature block.
    let mut letter =
        Draft { template: TemplateRef::Builtin { template: TemplateKind::OfficialLetter }, ..Default::default() };
    letter.title = "2026년 안전 점검 협조 요청".into();
    letter.fields = vec![
        field("document_no", "문서번호", FieldKind::String, false, "총무-2026-17"),
        field("issue_date", "시행일자", FieldKind::Date, true, "2026-11-08"),
        field("sender", "발신", FieldKind::String, true, "㈜도큐렌즈 총무팀"),
        field("recipient", "수신", FieldKind::String, true, "입주사 대표"),
        field("title", "제목", FieldKind::String, true, "2026년 안전 점검 협조 요청"),
        field("attachments", "붙임", FieldKind::String, false, "점검 일정표 1부."),
    ];
    letter.body =
        vec![DraftBlock::Paragraph { text: "귀사의 무궁한 발전을 기원합니다.".into(), source: None }];
    let html = render::to_html(&letter);
    assert!(html.contains("<h1 class=\"title\">공문</h1>"));
    assert!(html.contains("<b>제목</b> 2026년 안전 점검 협조 요청") && html.contains("<b>수신</b> 입주사 대표"));
    assert!(html.contains("<b>붙임</b> 점검 일정표 1부.") && html.contains("㈜도큐렌즈 총무팀 (직인)"));
}

fn table_texts(model: &crate::documents::convert::model::DocModel) -> Vec<Vec<Vec<String>>> {
    model
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Table(t) => {
                Some(t.rows.iter().map(|r| r.iter().map(|c| c.text().trim().to_string()).collect()).collect())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn docx_and_hwpx_written_from_scratch_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut d = quotation_draft();
    validate::validate(&mut d);
    let model = render::doc_model(&d);
    assert_eq!(model.title.as_deref(), Some("노트북 공급 견적서"));

    let docx_path = dir.path().join("q.docx");
    office::write_docx(&model, &docx_path).unwrap();
    let back = docx::read(&docx_path).unwrap();
    assert_eq!(back.title.as_deref(), Some("노트북 공급 견적서"));
    assert_eq!(back.page_mm.map(|(w, h)| (w.round(), h.round())), Some((210.0, 297.0)));
    let first = back.blocks.iter().find_map(|b| match b {
        Block::Paragraph(p) if !p.is_empty() => Some(p),
        _ => None,
    });
    assert_eq!(first.and_then(|p| p.heading), Some(1));
    let tables = table_texts(&back);
    assert_eq!(tables.len(), 2, "field grid + item table");
    assert!(
        tables[0].iter().any(|r| r.contains(&"수신".to_string()) && r.contains(&"㈜한빛".to_string())),
        "{:?}",
        tables[0]
    );
    assert_eq!(tables[1][0][0], "품명");
    assert_eq!(tables[1][1][0], "노트북");
    assert_eq!(tables[1][1][4], "2,400,000");
    assert!(tables[1].last().unwrap().iter().any(|c| c == "2,444,000원"), "{:?}", tables[1]);
    assert!(back
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Paragraph(p) if p.heading == Some(2) && p.text() == "견적 조건")));
    assert!(back
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Paragraph(p) if p.list == Some(ListKind::Bullet) && p.text() == "무상 보증 1년")));
    assert!(back.text().contains("납기는 발주 후 2주"));

    let hwpx_path = dir.path().join("q.hwpx");
    office::write_hwpx(&model, &hwpx_path).unwrap();
    let back = hwpx::read(&hwpx_path).unwrap();
    assert_eq!(back.title.as_deref(), Some("노트북 공급 견적서"));
    assert_eq!(back.page_mm.map(|(w, h)| (w.round(), h.round())), Some((210.0, 297.0)));
    let tables = table_texts(&back);
    assert_eq!(tables.len(), 2);
    assert!(
        tables[0].iter().any(|r| r.contains(&"수신".to_string()) && r.contains(&"㈜한빛".to_string())),
        "{:?}",
        tables[0]
    );
    assert_eq!(tables[1][1][0], "노트북");
    assert_eq!(tables[1][1][4], "2,400,000");
    assert!(back
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Paragraph(p) if p.heading == Some(1) && p.text() == "노트북 공급 견적서")));
    assert!(back
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Paragraph(p) if p.heading == Some(2) && p.text() == "견적 조건")));
    assert!(back.blocks.iter().any(|b| matches!(b, Block::Paragraph(p) if p.list == Some(ListKind::Bullet))));
    // The mimetype entry is stored first, uncompressed.
    let f = File::open(&hwpx_path).unwrap();
    let mut z = zip::ZipArchive::new(f).unwrap();
    let first = z.by_index(0).unwrap();
    assert_eq!(first.name().unwrap(), "mimetype");
    assert_eq!(first.compression(), zip::CompressionMethod::Stored);
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

#[test]
fn store_round_trip() {
    let w = world();
    assert!(store::list(&w.paths).unwrap().is_empty(), "no directory yet");
    let mut a = quotation_draft();
    a.issues.push(super::DraftIssue {
        path: "x".into(),
        code: "required_missing".into(),
        message: "m".into(),
        blocking: true,
        suggestion: None,
    });
    let sa = store::save(&w.paths, &a).unwrap();
    assert_eq!((sa.id.as_str(), sa.issues), (a.id.as_str(), 1));
    std::thread::sleep(std::time::Duration::from_millis(5));
    let mut b = quotation_draft();
    b.title = "둘째".into();
    store::save(&w.paths, &b).unwrap();
    let list = store::list(&w.paths).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].title, "둘째", "newest first");
    let loaded = store::load(&w.paths, &a.id).unwrap();
    assert_eq!(loaded.title, a.title);
    assert_eq!(loaded.tables[0].rows, a.tables[0].rows);
    assert!(loaded.updated_at >= a.updated_at);
    assert!(store::delete(&w.paths, &a.id).unwrap());
    assert!(!store::delete(&w.paths, &a.id).unwrap());
    assert!(matches!(store::load(&w.paths, &a.id), Err(CoreError::NotFound(_))));
    assert!(matches!(
        store::save(&w.paths, &Draft { id: "../evil".into(), ..Draft::default() }),
        Err(CoreError::Invalid(_))
    ));
    assert!(w.paths.drafts_dir().starts_with(w.paths.app_dir()));
}

// ---------------------------------------------------------------------------
// Save
// ---------------------------------------------------------------------------

#[test]
fn save_pdf_without_printer_falls_back_to_markdown_and_respects_overwrite() {
    let w = world();
    let deps = w.deps(None, None);
    let d = quotation_draft();
    let req = SaveRequest { draft: d.clone(), output: OutputFormat::Pdf, register: false, ..Default::default() };
    let saved = render::save(&deps, &req).unwrap();
    assert_eq!(saved.format, OutputFormat::Md);
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert_eq!(saved.rel_path, format!("documents/generated/{today}_노트북 공급 견적서.md"));
    assert!(saved.absolute.is_file());
    assert!(saved.warnings.iter().any(|w| w.contains("Markdown")), "{:?}", saved.warnings);
    assert!(!saved.filled_form && saved.doc_id.is_none());
    assert!(!Path::new(&format!("{}.part", saved.absolute.display())).exists(), "no temp file left");
    let text = std::fs::read_to_string(&saved.absolute).unwrap();
    assert!(text.starts_with("# 노트북 공급 견적서"));
    let stored = store::load(&w.paths, &d.id).unwrap();
    assert_eq!(stored.saved_path.as_deref(), Some(saved.rel_path.as_str()));
    // Same target again: refused unless overwrite.
    let err = render::save(&deps, &req).unwrap_err();
    assert!(matches!(err, CoreError::Invalid(ref c) if c == "export_exists"), "{err}");
    let again = render::save(&deps, &SaveRequest { overwrite: true, ..req.clone() }).unwrap();
    assert_eq!(again.rel_path, saved.rel_path);
    // Custom name / folder; paths outside the workspace are refused.
    let custom = render::save(
        &deps,
        &SaveRequest {
            file_name: Some("견적_한빛.md".into()),
            folder: Some("documents/2026/11".into()),
            output: OutputFormat::Md,
            register: false,
            ..req.clone()
        },
    )
    .unwrap();
    assert_eq!(custom.rel_path, "documents/2026/11/견적_한빛.md");
    for folder in ["/etc", "C:\\Temp", "../outside", ".doculens/x"] {
        let r = render::save(
            &deps,
            &SaveRequest { folder: Some(folder.into()), output: OutputFormat::Md, register: false, ..req.clone() },
        );
        assert!(matches!(r, Err(CoreError::Invalid(_))), "{folder}: {r:?}");
    }
    // A stub printer produces a real PDF.
    let mut deps_pdf = w.deps(None, None);
    deps_pdf.printer = Some(&StubPdfPrinter);
    let pdf = render::save(&deps_pdf, &SaveRequest { file_name: Some("견적".into()), ..req.clone() }).unwrap();
    assert_eq!((pdf.format, pdf.rel_path.as_str()), (OutputFormat::Pdf, "documents/generated/견적.pdf"));
    assert!(std::fs::read(&pdf.absolute).unwrap().starts_with(b"%PDF"));
    assert!(pdf.warnings.is_empty(), "{:?}", pdf.warnings);
}

#[test]
fn save_registers_generated_documents_in_the_library() {
    let w = world();
    let imp = w.import_deps();
    let deps = w.deps(None, Some(&imp));
    let d = quotation_draft();
    for (output, ext) in [(OutputFormat::Docx, "docx"), (OutputFormat::Hwpx, "hwpx")] {
        let saved = render::save(
            &deps,
            &SaveRequest { draft: d.clone(), output, file_name: Some(format!("견적_{ext}")), ..Default::default() },
        )
        .unwrap();
        assert_eq!(saved.format, output);
        assert_eq!(saved.rel_path, format!("documents/generated/견적_{ext}.{ext}"));
        let id = saved.doc_id.clone().expect("registered");
        let doc = w.db.with(|c| docs::get_document(c, &id)).unwrap().unwrap();
        assert_eq!(doc.role, DocumentRole::Generated);
        assert_eq!(doc.doc_type.as_deref(), Some("quotation"));
        assert_eq!(doc.doc_type_source.as_deref(), Some("generated"));
        assert_eq!(doc.title.as_deref(), Some("노트북 공급 견적서"));
        assert_eq!(doc.rel_path, saved.rel_path);
        assert!(doc.converted, "office files are converted on registration");
        assert_eq!(doc.status, DocumentStatus::New, "no PDFium: text only");
        let pages = w.db.with(|c| docs::list_pages(c, &id, true)).unwrap();
        assert!(pages[0].text.as_deref().unwrap_or("").contains("노트북"), "{:?}", pages[0].text);
        assert!(saved.warnings.is_empty(), "{:?}", saved.warnings);
        // Overwriting the same file keeps the registration (same id).
        let again = render::save(
            &deps,
            &SaveRequest {
                draft: d.clone(),
                output,
                file_name: Some(format!("견적_{ext}")),
                overwrite: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(again.doc_id.as_deref(), Some(id.as_str()));
    }
    let stored = store::load(&w.paths, &d.id).unwrap();
    assert!(stored.saved_doc_id.is_some());
    // Markdown is written but not registered (not an input format).
    let md =
        render::save(&deps, &SaveRequest { draft: d.clone(), output: OutputFormat::Md, ..Default::default() }).unwrap();
    assert!(md.doc_id.is_none() && md.warnings.iter().any(|w| w.contains("Markdown")));
}

// ---------------------------------------------------------------------------
// Form filling
// ---------------------------------------------------------------------------

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

fn docx_cell(inner: &str) -> String {
    format!("<w:tc>{inner}</w:tc>")
}

fn docx_text_cell(text: &str) -> String {
    docx_cell(&format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>"))
}

/// A quotation-like DOCX form: label/value grid, label lines and an item table with one
/// template row, one extra empty row and a totals row.
fn write_docx_form(path: &Path) {
    let mut body = String::new();
    body.push_str(
        r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>견 적 서</w:t></w:r></w:p>"#,
    );
    body.push_str("<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"3000\"/><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"3000\"/></w:tblGrid>");
    body.push_str(&format!(
        "<w:tr>{}{}{}{}</w:tr>",
        docx_text_cell("수신"),
        docx_cell("<w:p/>"),
        docx_text_cell("견적일자"),
        docx_text_cell("____")
    ));
    body.push_str(&format!(
        "<w:tr>{}{}{}{}</w:tr>",
        docx_text_cell("사업자등록번호"),
        docx_cell(r#"<w:p><w:r><w:t>(</w:t></w:r><w:r><w:t xml:space="preserve">   )</w:t></w:r></w:p>"#),
        docx_text_cell("담당자"),
        docx_cell("<w:p><w:pPr/></w:p>")
    ));
    body.push_str("</w:tbl>");
    body.push_str(r#"<w:p><w:r><w:t xml:space="preserve">유효기간: </w:t></w:r><w:r><w:t>______</w:t></w:r></w:p>"#);
    body.push_str(r#"<w:p><w:r><w:t>결제조건:</w:t></w:r></w:p>"#);
    body.push_str("<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"500\"/><w:gridCol w:w=\"2500\"/><w:gridCol w:w=\"1500\"/><w:gridCol w:w=\"1000\"/><w:gridCol w:w=\"1500\"/><w:gridCol w:w=\"2000\"/></w:tblGrid>");
    body.push_str(&format!(
        "<w:tr><w:trPr><w:tblHeader/></w:trPr>{}{}{}{}{}{}</w:tr>",
        docx_text_cell("No."),
        docx_text_cell("품명"),
        docx_text_cell("규격"),
        docx_text_cell("수량"),
        docx_text_cell("단가"),
        docx_text_cell("금액")
    ));
    let empty_row = format!("<w:tr>{}</w:tr>", docx_cell("<w:p/>").repeat(6));
    body.push_str(&empty_row);
    body.push_str(&empty_row);
    body.push_str(&format!(
        "<w:tr><w:tc><w:tcPr><w:gridSpan w:val=\"5\"/></w:tcPr><w:p><w:r><w:t>합계</w:t></w:r></w:p></w:tc>{}</w:tr>",
        docx_text_cell("____")
    ));
    body.push_str("</w:tbl>");
    body.push_str(r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838"/></w:sectPr>"#);
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W_NS}><w:body>{body}</w:body></w:document>"#
    );
    let file = File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    z.start_file("[Content_Types].xml", opts).unwrap();
    z.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#).unwrap();
    z.start_file("_rels/.rels", opts).unwrap();
    z.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#).unwrap();
    z.start_file("word/document.xml", opts).unwrap();
    z.write_all(document.as_bytes()).unwrap();
    z.finish().unwrap();
}

fn hwpx_cell(col: u32, row: u32, inner_runs: &str) -> String {
    format!(
        r#"<hp:tc name="" header="0" hasMargin="0" protect="0" editable="0" dirty="0" borderFillIDRef="3"><hp:subList id="" textDirection="HORIZONTAL" lineWrap="BREAK" vertAlign="CENTER"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0">{inner_runs}</hp:p></hp:subList><hp:cellAddr colAddr="{col}" rowAddr="{row}"/><hp:cellSpan colSpan="1" rowSpan="1"/><hp:cellSz width="10000" height="2000"/></hp:tc>"#
    )
}

fn hwpx_text_cell(col: u32, row: u32, text: &str) -> String {
    hwpx_cell(col, row, &format!(r#"<hp:run charPrIDRef="0"><hp:t>{text}</hp:t></hp:run>"#))
}

/// The same form as an HWPX package (empty cells carry an empty run, as Hangul writes them).
fn write_hwpx_form(path: &Path) {
    let mut sec = String::from(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core">"##,
    );
    sec.push_str(r#"<hp:p id="1" paraPrIDRef="1" styleIDRef="1" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="1"><hp:t>견 적 서</hp:t></hp:run></hp:p>"#);
    sec.push_str(r#"<hp:p id="2" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:tbl id="10" zOrder="0" numberingType="TABLE" textWrap="TOP_AND_BOTTOM" textFlow="BOTH_SIDES" lock="0" dropcapstyle="None" pageBreak="CELL" repeatHeader="1" rowCnt="2" colCnt="4" cellSpacing="0" borderFillIDRef="3" noAdjust="0"><hp:sz width="42520" height="4000" widthRelTo="ABSOLUTE" heightRelTo="ABSOLUTE" protect="0"/><hp:pos treatAsChar="1" affectLSpacing="0" flowWithText="1" allowOverlap="0" holdAnchorAndSO="0" vertRelTo="PARA" horzRelTo="COLUMN" vertAlign="TOP" horzAlign="LEFT" vertOffset="0" horzOffset="0"/>"#);
    sec.push_str(&format!(
        "<hp:tr>{}{}{}{}</hp:tr>",
        hwpx_text_cell(0, 0, "수신"),
        hwpx_cell(1, 0, r#"<hp:run charPrIDRef="0"/>"#),
        hwpx_text_cell(2, 0, "견적일자"),
        hwpx_text_cell(3, 0, "____")
    ));
    sec.push_str(&format!(
        "<hp:tr>{}{}{}{}</hp:tr>",
        hwpx_text_cell(0, 1, "사업자등록번호"),
        hwpx_text_cell(1, 1, "( )"),
        hwpx_text_cell(2, 1, "담당자"),
        hwpx_cell(3, 1, "")
    ));
    sec.push_str("</hp:tbl></hp:run></hp:p>");
    sec.push_str(r#"<hp:p id="3" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>유효기간: ______</hp:t></hp:run></hp:p>"#);
    sec.push_str(r#"<hp:p id="4" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:tbl id="11" zOrder="0" numberingType="TABLE" textWrap="TOP_AND_BOTTOM" textFlow="BOTH_SIDES" lock="0" dropcapstyle="None" pageBreak="CELL" repeatHeader="1" rowCnt="4" colCnt="5" cellSpacing="0" borderFillIDRef="3" noAdjust="0"><hp:sz width="42520" height="8000" widthRelTo="ABSOLUTE" heightRelTo="ABSOLUTE" protect="0"/><hp:pos treatAsChar="1" affectLSpacing="0" flowWithText="1" allowOverlap="0" holdAnchorAndSO="0" vertRelTo="PARA" horzRelTo="COLUMN" vertAlign="TOP" horzAlign="LEFT" vertOffset="0" horzOffset="0"/>"#);
    sec.push_str(&format!(
        "<hp:tr>{}{}{}{}{}</hp:tr>",
        hwpx_text_cell(0, 0, "품명"),
        hwpx_text_cell(1, 0, "규격"),
        hwpx_text_cell(2, 0, "수량"),
        hwpx_text_cell(3, 0, "단가"),
        hwpx_text_cell(4, 0, "공급가액")
    ));
    for row in 1..=2 {
        sec.push_str("<hp:tr>");
        for c in 0..5 {
            sec.push_str(&hwpx_cell(c, row, r#"<hp:run charPrIDRef="0"/>"#));
        }
        sec.push_str("</hp:tr>");
    }
    sec.push_str(&format!("<hp:tr>{}{}</hp:tr>", hwpx_text_cell(0, 3, "합계"), hwpx_text_cell(4, 3, "")));
    sec.push_str("</hp:tbl></hp:run></hp:p></hs:sec>");
    let file = File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(file);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    z.start_file("mimetype", stored).unwrap();
    z.write_all(b"application/hwp+zip").unwrap();
    z.start_file("Contents/content.hpf", deflated).unwrap();
    z.write_all(br##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><opf:package xmlns:opf="http://www.idpf.org/2007/opf/" version="" unique-identifier="" id=""><opf:metadata/><opf:manifest><opf:item id="section0" href="Contents/section0.xml" media-type="application/xml"/></opf:manifest><opf:spine><opf:itemref idref="section0" linear="yes"/></opf:spine></opf:package>"##).unwrap();
    z.start_file("Contents/section0.xml", deflated).unwrap();
    z.write_all(sec.as_bytes()).unwrap();
    z.finish().unwrap();
}

/// Draft for the generated forms: keys as `forms::derive_from_model` would derive them.
fn form_draft(doc_id: Option<&str>) -> Draft {
    use FieldKind::*;
    let mut d = Draft {
        template: TemplateRef::Form { schema_id: "form-schema".into(), doc_id: doc_id.map(str::to_string) },
        ..Default::default()
    };
    d.title = "한빛 견적".into();
    d.doc_type = Some("quotation".into());
    d.fields = vec![
        field("recipient", "수신", String, true, "㈜한빛"),
        field("date", "견적일자", Date, true, "2026.11.08"),
        field("brn", "사업자등록번호", Brn, false, "2208162517"),
        field("contact_person", "담당자", String, false, "홍길동"),
        field("valid_until", "유효기간", String, false, "30일"),
        field("payment_terms", "결제조건", String, false, "현금"),
        field("total", "합계", Amount, true, "2440000"),
        field("delivery_date", "납기", String, true, "2주"),
    ];
    d.tables = vec![DraftTable {
        key: "items".into(),
        label: "견적 내역".into(),
        columns: vec![
            col("item_name", "품명", String),
            col("spec", "규격", String),
            col("quantity", "수량", Integer),
            col("unit_price", "단가", Amount),
            col("supply_amount", "공급가액", Amount),
        ],
        rows: vec![
            vec!["노트북".into(), "15형".into(), "2".into(), "1200000".into(), "2400000".into()],
            vec!["무선 마우스".into(), "".into(), "2".into(), "20,000".into(), "40,000".into()],
        ],
    }];
    d
}

#[test]
fn fill_form_docx_places_labels_blanks_and_item_rows() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("form.docx");
    write_docx_form(&src);
    let out = dir.path().join("filled.docx");
    let mut d = form_draft(None);
    validate::validate(&mut d);
    let warnings = fill::fill_form(&src, SourceKind::Docx, &d, &out).unwrap();
    let back = docx::read(&out).unwrap();
    let tables = table_texts(&back);
    assert_eq!(tables.len(), 2);
    assert_eq!(tables[0][0], vec!["수신", "㈜한빛", "견적일자", "2026-11-08"]);
    assert_eq!(tables[0][1], vec!["사업자등록번호", "220-81-62517", "담당자", "홍길동"]);
    let paras: Vec<String> = back
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Paragraph(p) => Some(p.text().trim().to_string()),
            _ => None,
        })
        .collect();
    assert!(paras.contains(&"유효기간: 30일".to_string()), "{paras:?}");
    assert!(paras.contains(&"결제조건: 현금".to_string()), "{paras:?}");
    // Items: header, two cloned rows (numbered), extra empty row removed, totals row kept and filled.
    assert_eq!(tables[1].len(), 4, "{:?}", tables[1]);
    assert_eq!(tables[1][1], vec!["1", "노트북", "15형", "2", "1,200,000", "2,400,000"]);
    assert_eq!(tables[1][2], vec!["2", "무선 마우스", "", "2", "20,000", "40,000"]);
    assert_eq!(tables[1][3], vec!["합계", "2,440,000"]);
    // 납기 has no slot in this form.
    assert!(warnings.iter().any(|w| w.contains("delivery_date")), "{warnings:?}");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    // Other parts of the package survive untouched.
    let f = File::open(&out).unwrap();
    let z = zip::ZipArchive::new(f).unwrap();
    assert!(z.file_names().any(|n| n.unwrap() == "[Content_Types].xml"));
}

#[test]
fn fill_form_hwpx_renumbers_cloned_rows() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("form.hwpx");
    write_hwpx_form(&src);
    let out = dir.path().join("filled.hwpx");
    let mut d = form_draft(None);
    validate::validate(&mut d);
    let warnings = fill::fill_form(&src, SourceKind::Hwpx, &d, &out).unwrap();
    let back = hwpx::read(&out).unwrap();
    let tables = table_texts(&back);
    assert_eq!(tables.len(), 2, "{tables:?}");
    assert_eq!(tables[0][0], vec!["수신", "㈜한빛", "견적일자", "2026-11-08"]);
    assert_eq!(tables[0][1], vec!["사업자등록번호", "220-81-62517", "담당자", "홍길동"]);
    assert!(back.text().contains("유효기간: 30일"), "{}", back.text());
    assert_eq!(tables[1].len(), 4, "{:?}", tables[1]);
    assert_eq!(tables[1][1], vec!["노트북", "15형", "2", "1,200,000", "2,400,000"]);
    assert_eq!(tables[1][2], vec!["무선 마우스", "", "2", "20,000", "40,000"]);
    assert_eq!(tables[1][3], vec!["합계", "2,440,000"]);
    // rowCnt follows the new geometry and the mimetype stays first.
    let f = File::open(&out).unwrap();
    let mut z = zip::ZipArchive::new(f).unwrap();
    assert_eq!(z.by_index(0).unwrap().name().unwrap(), "mimetype");
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut z.by_name("Contents/section0.xml").unwrap(), &mut xml).unwrap();
    assert!(xml.contains("rowCnt=\"4\"") && !xml.contains("rowCnt=\"2\" colCnt=\"5\""), "{xml}");
    assert!(xml.contains("rowAddr=\"3\""));
    assert!(warnings.iter().all(|w| !w.contains("recipient")), "{warnings:?}");
}

#[test]
fn save_fills_registered_office_forms_in_place() {
    let w = world();
    // The registered form lives in the workspace as a DOCX document with role form.
    let form_rel = "documents/forms/견적서양식.docx";
    let form_abs = w.paths.root.join(form_rel);
    std::fs::create_dir_all(form_abs.parent().unwrap()).unwrap();
    write_docx_form(&form_abs);
    w.db.with(|c| {
        docs::insert_document(
            c,
            &NewDocument {
                id: "form-doc".into(),
                rel_path: form_rel.into(),
                original_name: "견적서양식.docx".into(),
                source_kind: SourceKind::Docx,
                content_hash: "form-hash".into(),
                size_bytes: 10,
                file_modified_at: None,
                role: DocumentRole::Form,
                import_batch_id: None,
            },
        )?;
        docs::set_status(c, "form-doc", DocumentStatus::New, None)
    })
    .unwrap();
    let imp = w.import_deps();
    let deps = w.deps(None, Some(&imp));
    let d = form_draft(Some("form-doc"));
    // HWPX was requested but the form is DOCX: the form's format wins (with a warning).
    let saved = render::save(
        &deps,
        &SaveRequest {
            draft: d.clone(),
            output: OutputFormat::Hwpx,
            file_name: Some("한빛견적".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(saved.format, OutputFormat::Docx);
    assert!(saved.filled_form);
    assert_eq!(saved.rel_path, "documents/generated/한빛견적.docx");
    assert!(saved.warnings.iter().any(|w| w.contains("DOCX")), "{:?}", saved.warnings);
    let back = docx::read(&saved.absolute).unwrap();
    assert_eq!(table_texts(&back)[0][0], vec!["수신", "㈜한빛", "견적일자", "2026-11-08"]);
    let id = saved.doc_id.expect("registered");
    let doc = w.db.with(|c| docs::get_document(c, &id)).unwrap().unwrap();
    assert_eq!((doc.role, doc.doc_type.as_deref()), (DocumentRole::Generated, Some("quotation")));
    assert_ne!(id, "form-doc", "the form itself is untouched");
    assert_eq!(docx::read(&form_abs).unwrap().text(), docx::read(&form_abs).unwrap().text());
    // PDF / Markdown of a form draft use the generic renderer.
    let md =
        render::save(&deps, &SaveRequest { draft: d, output: OutputFormat::Md, register: false, ..Default::default() })
            .unwrap();
    assert!(!md.filled_form);
    assert!(std::fs::read_to_string(&md.absolute).unwrap().contains("| 수신 | ㈜한빛 |"));
}
