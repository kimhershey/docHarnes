//! Document-generation tests: templates and the grammar schema, drafts from a
//! scripted model (citations, regeneration, retry), the rule-based 보완요청서,
//! reverse validation, HTML / Markdown rendering, DOCX / HWPX round trips,
//! saving (PDF fallback, overwrite, registration) and in-place form filling.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
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
    db.with(prepo::enable_legacy_builtins).unwrap();
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
            progress: None,
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
    /// The user message of a call (without the system message).
    fn user_prompt(&self, call: usize) -> String {
        let reqs = self.requests.lock().unwrap();
        reqs[call]
            .messages
            .iter()
            .filter(|m| m["role"] == "user")
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
            {"key": "items", "rows": [["노트북 임대", "", "2", "1,200,000", "2,000,000", ""]]}
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
    // 12 software templates + 보완요청서 are offered; the 6 generic ones of 0.1 stay hidden but defined.
    let visible = templates::list();
    assert_eq!(visible.len(), 13);
    assert!(visible.iter().all(|t| !t.kind.hidden()));
    assert_eq!(TemplateKind::visible().count(), 13);
    let all = templates::list_all();
    assert_eq!(all.len(), 19);
    // Generation slugs equal the pipeline's document types for the software set.
    for k in TemplateKind::visible().filter(|k| *k != TemplateKind::SupplementRequest) {
        assert!(crate::pipeline::schema::builtin(k.as_str()).is_some(), "{k:?} has no pipeline schema");
    }
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
    // Pipeline-compatible keys on the money template (the estimate's staffing columns).
    let q = templates::get(TemplateKind::Estimate);
    for k in ["issue_date", "supplier.name", "supplier.brn", "supply_total", "tax_total", "grand_total", "items"] {
        assert!(q.field(k).is_some(), "estimate lacks {k}");
    }
    let cols: Vec<&str> = q.field("items").unwrap().columns.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(cols, ["role", "grade", "months", "unit_price", "supply_amount", "note"]);
    assert_eq!(templates::match_keywords("보완요청서를 보고서 형식으로"), Some(TemplateKind::SupplementRequest));
    assert_eq!(templates::match_keywords("㈜한빛에 노트북 10대 견적서 만들어줘"), Some(TemplateKind::Estimate));
    assert_eq!(templates::match_keywords("Draft the meeting minutes"), Some(TemplateKind::MeetingMinutes));
    assert_eq!(templates::match_keywords("개발 계약서 초안"), Some(TemplateKind::DevContract));
    assert_eq!(templates::match_keywords("10월 진척 보고서"), Some(TemplateKind::ProgressReport));
    assert_eq!(templates::match_keywords("아무 내용"), None);
    // Hidden generic templates: not matched from free text, still reachable by an explicit reference.
    assert_eq!(templates::match_keywords("A4 공문 형식"), None);
    assert_eq!(templates::match_keywords_any("A4 공문 형식"), Some(TemplateKind::OfficialLetter));
    assert!(templates::get(TemplateKind::Quotation).kind.hidden());
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
                    // Named apart from the built-in 견적서 sample: the schema name guard (0.3.0 §2) refuses duplicates.
                    name: "한빛 견적서".into(),
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
        super::resolve_template(&w.db, &req(Some("한빛 견적서"), "")).unwrap(),
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
        TemplateRef::Builtin { template: TemplateKind::MeetingMinutes }
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
        TemplateRef::Builtin { template: TemplateKind::DevContract }
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
    assert_eq!(d.template, TemplateRef::Builtin { template: TemplateKind::Estimate });
    assert_eq!(d.doc_type.as_deref(), Some("estimate"));
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
    assert_eq!(d.fields.len(), TemplateSpec::builtin(TemplateKind::Estimate).scalars().count());
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

/// CON-3 (core side): a draft that carries `project_id` looks for evidence inside that project — its work
/// documents, its reference group and the 공통 참고 자료 — in every lookup: the explicit query, the content
/// query, the per-section queries of a split draft and a revision. Another project's documents stay out.
#[test]
fn project_drafts_search_for_evidence_inside_the_project() {
    use crate::knowledge::{repo as krepo, CollectionInput};
    use crate::projects::{repo as projects, ProjectInput};
    let w = world();
    // Two projects whose 과업지시서 both speak of the 검수 기준, and a common regulation.
    w.add_doc(
        "a-sow",
        "한빛_과업지시서.pdf",
        DocumentRole::Work,
        &["한빛 포털 과업지시서\n\n검수 기준: 통합 테스트 통과율 95% 이상이면 검수를 완료한다. 추진 일정은 12월까지다."],
    );
    w.add_doc(
        "b-sow",
        "새빛_과업지시서.pdf",
        DocumentRole::Work,
        &["새빛 쇼핑몰 과업지시서\n\n검수 기준: 통합 테스트 통과율 80% 이상이면 검수를 완료한다. 추진 일정은 3월까지다."],
    );
    w.add_doc(
        "rule",
        "검수규정.pdf",
        DocumentRole::Knowledge,
        &["검수 규정\n\n제5조(검수 기준) 검수는 과업지시서의 검수 기준에 따른다."],
    );
    let analyzer = Analyzer::simple();
    for id in ["a-sow", "b-sow", "rule"] {
        let doc = w.db.with(|c| docs::get_document(c, id)).unwrap().unwrap();
        index_document(&IndexDeps { db: &w.db, analyzer: &analyzer, options: Default::default() }, &doc, true).unwrap();
    }
    let project = |name: &str, doc: &str| {
        let p = w.db.with(|c| projects::save_project(c, &ProjectInput { name: name.into(), ..Default::default() }));
        let p = p.unwrap();
        w.db.with(|c| projects::add_documents(c, &p.id, &[doc.to_string()])).unwrap();
        p.id
    };
    let a = project("한빛 포털", "a-sow");
    let _b = project("새빛 쇼핑몰", "b-sow");
    let common =
        w.db.with(|c| {
            krepo::save_collection(c, &CollectionInput { name: "사규".into(), active: true, ..Default::default() })
        })
        .unwrap();
    w.db.with(|c| krepo::add_docs_to_collection(c, &common.id, &["rule".into()])).unwrap();

    let reply = json!({"title": "검수 보고", "fields": [], "tables": [], "body": [], "notes": []}).to_string();
    let section = json!({ "blocks": [{"kind": "paragraph", "text": "본문", "items": [], "source_index": null}] });
    // Runs a draft and returns every prompt of its calls plus the documents offered as references.
    let run = |req: &DraftRequest| {
        let chat = SeqChat::new(vec![reply.clone(), section.to_string()]);
        let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None);
        let mut deps = w.deps(Some(&vlm), None);
        deps.knowledge = Some(SearchDeps { db: &w.db, analyzer: &analyzer, embedder: None });
        let out = draft::draft(&deps, req).unwrap();
        let calls = chat.requests.lock().unwrap().len();
        let prompts: Vec<String> = (0..calls).map(|c| chat.user_prompt(c)).collect();
        let mut refs: Vec<String> = out.draft.references.iter().filter_map(|r| r.doc_id.clone()).collect();
        refs.sort();
        refs.dedup();
        (prompts.join("\n=====\n"), refs, out)
    };
    let base = DraftRequest {
        form_ref: Some("progress_report".into()),
        content: "검수 기준에 따른 통합 테스트 진척 보고".into(),
        sources: vec![SourceRef::Knowledge { query: "검수 기준 통과율".into() }],
        use_knowledge: true,
        ..Default::default()
    };
    // Without a project the whole library is searched: both projects' documents come back.
    let (prompts, everything, _) = run(&base);
    assert_eq!(everything, ["a-sow", "b-sow", "rule"], "{prompts}");
    // Inside project A: its own 과업지시서 and the common regulation, never project B's.
    let in_a = DraftRequest { project_id: Some(a.clone()), ..base.clone() };
    let (prompts, refs, out) = run(&in_a);
    assert_eq!(refs, ["a-sow", "rule"], "{prompts}");
    assert!(prompts.contains("통과율 95%") && !prompts.contains("새빛") && !prompts.contains("80%"), "{prompts}");
    // The stored request keeps the project, so a regeneration stays inside it.
    assert_eq!(out.draft.request.as_ref().unwrap().project_id.as_deref(), Some(a.as_str()));
    // Section by section (five calls, each section looks up its own heading): still no document of B.
    let (prompts, refs, out) = run(&DraftRequest { max_tokens: Some(8_000), ..in_a.clone() });
    assert_eq!(out.sections, 5);
    assert!(!refs.contains(&"b-sow".to_string()) && !prompts.contains("새빛"), "{refs:?}\n{prompts}");
    // A section revision of that draft gathers its sources the same way.
    let revise = DraftRequest {
        previous: Some(Box::new(filled_progress_report())),
        feedback: Some("검수 기준을 인용해 주세요".into()),
        revise: Some(super::ReviseScope::Section { section: 2 }),
        ..in_a.clone()
    };
    let (prompts, refs, _) = run(&revise);
    assert_eq!(refs, ["a-sow", "rule"], "{prompts}");
    assert!(!prompts.contains("새빛"), "{prompts}");

    // A project that no longer exists allows nothing, like every project-scoped search — the draft says why
    // it has no evidence (only when it would have searched).
    let gone = DraftRequest { project_id: Some("gone".into()), ..base.clone() };
    let (_, refs, out) = run(&gone);
    assert!(refs.is_empty(), "{refs:?}");
    assert!(out.draft.notes.iter().any(|n| n.contains("프로젝트를 찾을 수 없어")), "{:?}", out.draft.notes);
    let (_, _, out) = run(&DraftRequest { sources: Vec::new(), use_knowledge: false, ..gone });
    assert!(!out.draft.notes.iter().any(|n| n.contains("프로젝트를 찾을 수 없어")), "{:?}", out.draft.notes);

    // The job bridge makes the job's project and the request's project one thing.
    let mut from_project_page = DraftRequest::default();
    assert_eq!(from_project_page.bind_project(Some(a.clone())), Some(a.clone()));
    assert_eq!(from_project_page.project_id.as_deref(), Some(a.as_str()), "the 보완요청서 searches inside its project");
    let mut from_screen = DraftRequest { project_id: Some(format!(" {a} ")), ..Default::default() };
    assert_eq!(from_screen.bind_project(None), Some(a.clone()), "the result links to the request's project");
    assert_eq!(from_screen.project_id.as_deref(), Some(a.as_str()));
    let mut none = DraftRequest { project_id: Some("  ".into()), ..Default::default() };
    assert_eq!((none.bind_project(Some(String::new())), none.project_id.clone()), (None, None));
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

/// GEN-1: the 초안 어시스턴트 on a rule-composed 보완요청서. A request that carries `revise`, `feedback` or
/// `previous` is not rule-only (the job bridge loads the model for it); without a model it fails instead of
/// being recomposed by rules, and with one the instruction reaches the model and the user's edits survive.
#[test]
fn supplement_request_revisions_take_the_model_path() {
    use super::ReviseScope;
    let w = world();
    let base = DraftRequest {
        form_ref: Some("supplement_request".into()),
        content: "수신: ㈜한빛\n기한: 2026-11-30".into(),
        sources: vec![SourceRef::Text {
            title: Some("정합성 검사 결과".into()),
            text: "계약서 | 서명 누락 | 계약서 3쪽 | 서명본 제출".into(),
        }],
        use_knowledge: false,
        ..Default::default()
    };
    assert!(draft::rule_only(&base));
    // The rule-composed draft, then edited in the 문서 작성 screen: a table cell and a body paragraph.
    let mut prev = draft::draft(&w.deps(None, None), &base).unwrap().draft;
    prev.tables[0].rows[0][3] = "서명본을 11월 20일까지 제출".into();
    let edited = prev.body.iter().position(|b| matches!(b, DraftBlock::Paragraph { .. })).unwrap();
    prev.body[edited] = DraftBlock::Paragraph { text: "사용자가 고친 근거 문단".into(), source: None };
    let with_prev = |revise: Option<ReviseScope>, feedback: Option<&str>| DraftRequest {
        previous: Some(Box::new(prev.clone())),
        feedback: feedback.map(str::to_string),
        revise,
        ..base.clone()
    };
    let whole = with_prev(Some(ReviseScope::Whole), Some("더 정중하게"));
    let section = with_prev(Some(ReviseScope::Section { section: 2 }), Some("2절을 더 정중하게"));
    let insert =
        with_prev(Some(ReviseScope::InsertAfter { section: None, heading: Some("문의".into()) }), Some("문의 절"));
    let regenerate = with_prev(None, None);
    let feedback_only = DraftRequest { feedback: Some("더 짧게".into()), ..base.clone() };
    for (name, req) in [
        ("whole", &whole),
        ("section", &section),
        ("insert", &insert),
        ("previous", &regenerate),
        ("feedback", &feedback_only),
    ] {
        assert!(!draft::rule_only(req), "{name}: the bridge must load the model");
        // Before the fix `whole`, `previous` and `feedback` were recomposed by rules: the instruction was
        // ignored and the edited cell / paragraph were lost.
        let r = draft::draft(&w.deps(None, None), req);
        assert!(matches!(r, Err(CoreError::EngineNotRunning)), "{name}: {r:?}");
    }
    // A blank instruction is no instruction; other templates and non-text sources were never rule-only.
    assert!(draft::rule_only(&DraftRequest { feedback: Some("  ".into()), ..base.clone() }));
    assert!(!draft::rule_only(&DraftRequest { sources: Vec::new(), ..base.clone() }));
    assert!(!draft::rule_only(&DraftRequest { form_ref: Some("estimate".into()), ..base.clone() }));
    let mut mixed = base.clone();
    mixed.sources.push(SourceRef::Document { doc_id: "d".into() });
    assert!(!draft::rule_only(&mixed));

    // With the model, a section revision rewrites that section only: the item table stays in it, the
    // edited cell and the other sections are untouched.
    let para = |t: &str| json!({"kind": "paragraph", "text": t, "items": [], "source_index": null});
    let chat = SeqChat::new(vec![json!({ "blocks": [para("아래 항목의 보완을 정중히 요청드립니다.")] }).to_string()]);
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None);
    let out = draft::draft(&w.deps(Some(&vlm), None), &section).unwrap();
    assert_eq!(out.source, "model");
    let d = &out.draft;
    assert_eq!(d.id, prev.id);
    assert_eq!(d.tables[0].rows[0][3], "서명본을 11월 20일까지 제출");
    assert!(d
        .body
        .iter()
        .any(|b| matches!(b, DraftBlock::Paragraph { text, .. } if text == "사용자가 고친 근거 문단")));
    let s2 = super::sections_of(&d.body)[1].clone();
    assert_eq!(s2.heading.as_deref(), Some("보완 요청 사항"));
    assert!(
        matches!(&d.body[s2.start + 1], DraftBlock::Paragraph { text, .. } if text == "아래 항목의 보완을 정중히 요청드립니다.")
    );
    assert!(d.body[s2.start..s2.end].iter().any(|b| matches!(b, DraftBlock::TableRef { key } if key == "items")));
    assert!(chat.prompt(0).contains("[수정 요청]\n2절을 더 정중하게"), "{}", chat.prompt(0));

    // "전체 다시 쓰기": the model sees the edited draft and the instruction, and its reply is the result.
    let reply = json!({
        "title": "보완 요청",
        "fields": [{"key": "recipient", "value": "㈜한빛", "source_index": null}],
        "tables": [{"key": "items", "rows": [["계약서", "서명 누락", "계약서 3쪽", "서명본을 11월 20일까지 제출해 주시기 바랍니다"]]}],
        "body": [
            {"kind": "heading", "text": "근거", "items": [], "source_index": null},
            para("정중히 다시 쓴 근거 문단")
        ],
        "notes": []
    });
    let chat2 = SeqChat::json(reply);
    let vlm2 = Vlm::new(&chat2, "scripted".into(), Profile::Balanced, None);
    let out2 = draft::draft(&w.deps(Some(&vlm2), None), &whole).unwrap();
    assert_eq!(out2.source, "model");
    let prompt = chat2.prompt(0);
    assert!(prompt.contains("[이전 초안]") && prompt.contains("사용자가 고친 근거 문단"), "{prompt}");
    assert!(prompt.contains("서명본을 11월 20일까지 제출") && prompt.contains("[수정 요청]\n더 정중하게"), "{prompt}");
    assert_eq!(out2.draft.id, prev.id);
    assert_eq!(out2.draft.tables[0].rows[0][3], "서명본을 11월 20일까지 제출해 주시기 바랍니다");
    assert!(out2
        .draft
        .body
        .iter()
        .any(|b| matches!(b, DraftBlock::Paragraph { text, .. } if text == "정중히 다시 쓴 근거 문단")));
    assert!(!out2.draft.notes.iter().any(|n| n.contains("규칙으로 작성")), "{:?}", out2.draft.notes);
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

/// A built-in draft with every scalar field of its template (empty) and the given values.
fn template_draft(kind: TemplateKind, values: &[(&str, &str)]) -> Draft {
    let spec = TemplateSpec::builtin(kind);
    let mut d = Draft { template: TemplateRef::Builtin { template: kind }, ..Default::default() };
    d.doc_type = Some(kind.as_str().into());
    d.fields = spec
        .scalars()
        .map(|f| {
            let value = values.iter().find(|(k, _)| *k == f.key).map(|(_, v)| *v).unwrap_or("");
            field(&f.key, &f.label_ko, FieldKind::parse(&f.kind).unwrap(), f.required, value)
        })
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
    d
}

/// GEN-15: the required `title` field of the templates that are not letters (계약명, 회의명, 변경 제목) is
/// printed in the preview and in every saved format; it was dropped everywhere before.
#[test]
fn title_fields_of_non_letter_templates_are_rendered() {
    for (kind, label, value) in [
        (TemplateKind::DevContract, "계약명", "한빛 포털 구축 용역 계약"),
        (TemplateKind::MeetingMinutes, "회의명", "11월 2주차 주간 회의"),
        (TemplateKind::ChangeRequest, "변경 제목", "로그인 화면에 SSO 추가"),
    ] {
        let mut d = template_draft(kind, &[("title", value)]);
        d.title = "문서 머리 제목".into();
        let html = render::to_html(&d);
        assert!(html.contains("<h1 class=\"title\">문서 머리 제목</h1>"), "{kind:?}");
        assert!(html.contains(&format!("<th>{label}</th><td>{value}</td>")), "{kind:?}: {html}");
        let md = render::to_markdown(&d);
        assert!(md.contains(&format!("| {label} | {value} |")), "{kind:?}: {md}");
        // The DOCX / HWPX writers take the same plan.
        let grid = table_texts(&render::doc_model(&d));
        assert!(
            grid.iter().flatten().any(|row| row.contains(&label.to_string()) && row.contains(&value.to_string())),
            "{kind:?}: {grid:?}"
        );
        // A title field that only repeats the document title is not printed twice …
        d.title = format!(" {value} ");
        let html = render::to_html(&d);
        assert!(html.contains(&format!("<h1 class=\"title\">{value}</h1>")), "{kind:?}");
        assert!(!html.contains(&format!("<th>{label}</th>")), "{kind:?}: {html}");
        // … and an empty required one shows its blank like any other required field.
        let empty = template_draft(kind, &[]);
        assert!(render::to_html(&empty).contains(&format!("<th>{label}</th><td><span class=\"blank\">")), "{kind:?}");
    }
    // Letters keep their layout: the subject line of the head, no grid row.
    let mut letter = template_draft(TemplateKind::SupplementRequest, &[("title", "제출 서류 보완 요청")]);
    letter.title = "제출 서류 보완 요청".into();
    let html = render::to_html(&letter);
    assert!(html.contains("<b>제목</b> 제출 서류 보완 요청") && !html.contains("<th>제목</th>"), "{html}");
}

/// GEN-2: the `amount` kind is every number with a possible fraction, not only money. Each amount field and
/// column of the built-in templates is classified on purpose (a new one must be added here), and labels
/// decide for registered forms.
#[test]
fn amount_fields_are_classified_as_money_or_plain_numbers() {
    // (template, field key | "<table>.<column>")
    let money = [
        ("rfp", "budget"),
        ("proposal", "price"),
        ("sow", "amount"),
        ("sow", "milestones.amount"),
        ("dev_contract", "amount"),
        ("dev_contract", "payments.amount"),
        ("change_request", "impact_cost"),
        ("acceptance", "amount"),
        ("estimate", "supply_total"),
        ("estimate", "tax_total"),
        ("estimate", "grand_total"),
        ("estimate", "items.unit_price"),
        ("estimate", "items.supply_amount"),
        ("quotation", "supply_total"),
        ("quotation", "tax_total"),
        ("quotation", "grand_total"),
        ("quotation", "items.unit_price"),
        ("quotation", "items.supply_amount"),
        ("quotation", "items.tax_amount"),
        ("transaction_statement", "supply_total"),
        ("transaction_statement", "tax_total"),
        ("transaction_statement", "grand_total"),
        ("transaction_statement", "items.unit_price"),
        ("transaction_statement", "items.supply_amount"),
        ("transaction_statement", "items.tax_amount"),
        ("contract", "amount"),
    ];
    let numbers = [
        ("rfp", "evaluation.points"),
        ("proposal", "team.months"),
        ("sow", "milestones.ratio"),
        ("dev_contract", "payments.ratio"),
        ("wbs", "tasks.progress"),
        ("progress_report", "progress_percent"),
        ("progress_report", "plan_percent"),
        ("progress_report", "tasks.progress"),
        ("test_report", "pass_rate"),
        ("estimate", "items.months"),
    ];
    let mut seen = 0usize;
    for t in templates::list_all() {
        let mut check = |path: String, key: &str, labels: &[&str], kind: &str| {
            if kind != "amount" {
                return;
            }
            seen += 1;
            let id = (t.kind.as_str(), path.as_str());
            let expected = money.contains(&id);
            assert!(expected || numbers.contains(&id), "unclassified amount field {id:?}");
            for label in labels {
                assert_eq!(render::is_money(key, label), expected, "{id:?} ({label})");
            }
        };
        for f in &t.fields {
            check(f.key.clone(), &f.key, &[f.label_ko.as_str(), f.label_en.as_str()], &f.kind);
            for c in &f.columns {
                check(format!("{}.{}", f.key, c.key), &c.key, &[c.label.as_str()], &c.kind);
            }
        }
    }
    assert_eq!(seen, money.len() + numbers.len(), "every listed field exists");
    // Registered forms: the key is derived from the label and says nothing, so the label decides.
    for (label, expected) in [
        ("계약금액", true),
        ("합계", true),
        ("단가", true),
        ("공급가액", true),
        ("세액", true),
        ("비용 영향", true),
        ("환율 적용 금액", true),
        ("Unit price", true),
        ("진척률(%)", false),
        ("통과율", false),
        ("세율", false),
        ("투입 MM", false),
        ("공수(M/M)", false),
        ("수량", false),
        ("투입 인원 (명)", false),
        ("배점", false),
        ("Qty", false),
        ("Pass rate (%)", false),
        // Money in another unit: the figure is printed as entered, without `원`.
        ("금액(천원)", false),
        ("계약금액 (USD)", false),
    ] {
        assert_eq!(render::is_money("field_3", label), expected, "{label}");
    }
    // The prompt announces plain numbers as numbers (they used to be "금액" like money).
    let req = DraftRequest::default();
    let allow = super::prompts::user_allowance(&req, 0, None, 1_800, 0, true);
    let spec = TemplateSpec::builtin(TemplateKind::ProgressReport);
    let prompt = super::prompts::user(&spec, &req, &[], &allow, true);
    assert!(prompt.contains("- progress_percent — 전체 진척률(%) — 숫자"), "{prompt}");
    assert!(prompt.contains("progress=진척률(%)(숫자)"), "{prompt}");
    let spec = TemplateSpec::builtin(TemplateKind::Estimate);
    let prompt = super::prompts::user(&spec, &req, &[], &allow, true);
    assert!(prompt.contains("months=투입 MM(숫자)") && prompt.contains("unit_price=단가(금액)"), "{prompt}");
    assert!(prompt.contains("- grand_total — 합계금액 — 금액; 필수"), "{prompt}");
}

/// GEN-2: percentages, person-months and meeting times are printed as entered — "62.5" was "63원", "95.2" was
/// "95원", 0.5 MM was "1" and "2026-11-10 14:00" lost its time — while money keeps its format.
#[test]
fn rendered_values_keep_decimals_and_times_and_format_only_money() {
    use FieldKind::*;
    // 진척 보고서: 전체 / 계획 진척률 and the per-task progress column.
    let mut report = template_draft(
        TemplateKind::ProgressReport,
        &[
            ("project_name", "한빛 포털 구축"),
            ("report_date", "2026-11-10"),
            ("progress_percent", "62.5"),
            ("plan_percent", "70"),
        ],
    );
    report.title = "11월 2주차 진척 보고서".into();
    report.tables.iter_mut().find(|t| t.key == "tasks").unwrap().rows =
        vec![vec!["화면 설계".into(), "김설계".into(), "완료".into(), "완료".into(), "87.5".into()]];
    validate::validate(&mut report);
    assert!(!report.has_blocking_issues(), "{:?}", report.issues);
    let html = render::to_html(&report);
    assert!(html.contains("<th>전체 진척률(%)</th><td>62.5</td>"), "{html}");
    assert!(html.contains("<th>계획 진척률(%)</th><td>70</td>"), "{html}");
    assert!(html.contains("<td class=\"num\">87.5</td>"), "{html}");
    assert!(!html.contains("63원") && !html.contains("70원") && !html.contains(">88<"), "{html}");
    let md = render::to_markdown(&report);
    assert!(md.contains("| 전체 진척률(%) | 62.5 |") && md.contains("| 87.5 |"), "{md}");
    let model = table_texts(&render::doc_model(&report));
    assert!(model
        .iter()
        .flatten()
        .any(|r| r.contains(&"전체 진척률(%)".to_string()) && r.contains(&"62.5".to_string())));

    // 테스트 결과서: 통과율.
    let mut test = template_draft(
        TemplateKind::TestReport,
        &[
            ("project_name", "한빛 포털 구축"),
            ("test_date", "2026-11-10"),
            ("pass_rate", "95.2"),
            ("total_cases", "125"),
        ],
    );
    validate::validate(&mut test);
    let html = render::to_html(&test);
    assert!(html.contains("<th>통과율(%)</th><td>95.2</td>") && !html.contains("95원"), "{html}");

    // 견적서: 0.5 MM stays 0.5 while the money columns and totals are formatted.
    let mut estimate = template_draft(
        TemplateKind::Estimate,
        &[("issue_date", "2026-11-10"), ("recipient", "㈜한빛"), ("supplier.name", "㈜도큐렌즈")],
    );
    estimate.tables[0].rows = vec![
        vec!["PM".into(), "특급".into(), "0.5".into(), "12000000".into(), "".into(), "".into()],
        vec!["개발자".into(), "고급".into(), "2.5".into(), "9,000,000".into(), "22500000".into(), "".into()],
    ];
    validate::validate(&mut estimate);
    assert!(!estimate.has_blocking_issues(), "{:?}", estimate.issues);
    let html = render::to_html(&estimate);
    assert!(html.contains("<td class=\"num\">0.5</td>") && html.contains("<td class=\"num\">2.5</td>"), "{html}");
    assert!(html.contains("<td class=\"num\">12,000,000</td>") && html.contains("<td class=\"num\">6,000,000</td>"));
    assert!(html.contains("<td class=\"num\">22,500,000</td>"), "{html}");
    assert!(html.contains("<th>합계금액</th><td>31,350,000원</td>"), "0.5 × 12,000,000 + 22,500,000 + 10 %: {html}");
    let md = render::to_markdown(&estimate);
    assert!(md.contains("| PM | 특급 | 0.5 | 12,000,000 | 6,000,000 |"), "{md}");

    // 회의록: the meeting time survives; a bare date is still normalised.
    let mut minutes = template_draft(
        TemplateKind::MeetingMinutes,
        &[("title", "주간 회의"), ("meeting_date", "2026-11-10 14:00"), ("next_meeting", "2026.11.17")],
    );
    minutes.title = "주간 회의".into();
    validate::validate(&mut minutes);
    assert_eq!(minutes.field("meeting_date").unwrap().normalized.as_deref(), Some("2026-11-10"));
    let html = render::to_html(&minutes);
    assert!(html.contains("<th>일시</th><td>2026-11-10 14:00</td>"), "{html}");
    assert!(html.contains("<th>차기 회의</th><td>2026-11-17</td>"), "{html}");

    // Field by field: whole money is formatted, anything else is the writer's wording.
    let money = |value: &str, ko: bool| render::display_value(&field("amount", "계약금액", Amount, true, value), ko);
    assert_eq!(money("120000000", true), "120,000,000원");
    assert_eq!(money("120000000", false), "KRW 120,000,000");
    assert_eq!(money("1억 2,000만원", true), "120,000,000원");
    assert_eq!(money("1,234.56", true), "1,234.56", "a fraction is never rounded");
    assert_eq!(money("USD 1,200", true), "USD 1,200", "another currency is not won");
    assert_eq!(money("1,200,000원 (부가세 별도)", true), "1,200,000원 (부가세 별도)");
    let date = |value: &str| {
        let mut f = field("period", "기간", Date, false, value);
        f.normalized = crate::pipeline::validate::normalize_date(value);
        render::display_value(&f, true)
    };
    assert_eq!(date("2026년 11월 8일"), "2026-11-08");
    assert_eq!(date("2026-11-10 ~ 2026-11-14"), "2026-11-10 ~ 2026-11-14");
    assert_eq!(date("2026-11-10 - 2026-11-14"), "2026-11-10 - 2026-11-14");
    assert_eq!(date("2026-11-10 (화) 오후 2시"), "2026-11-10 (화) 오후 2시");
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

/// GEN-3: a draft job used to write its result over the stored draft whatever had happened meanwhile — the
/// partial result of a cancelled "전체 다시 쓰기" replaced the complete draft, a retried job put its old
/// snapshot over newer edits, a deleted draft came back. `save_job_result` decides where a result may go.
#[test]
fn job_results_do_not_replace_complete_or_newer_drafts() {
    use super::store::{save_job_result, CopyReason, JobInput, JobSave, SkipReason};
    use chrono::{Duration, Utc};
    let w = world();
    let p = |t: &str| DraftBlock::Paragraph { text: t.into(), source: None };
    // The stored draft the job starts from, exported once.
    let mut original = quotation_draft();
    original.id = "dr-orig".into();
    original.saved_path = Some("documents/generated/견적.docx".into());
    let reset = || {
        for s in store::list(&w.paths).unwrap() {
            store::delete(&w.paths, &s.id).unwrap();
        }
        store::save(&w.paths, &original).unwrap();
    };
    let stored = |id: &str| store::load(&w.paths, id).unwrap();
    let ids = || {
        let mut v: Vec<String> = store::list(&w.paths).unwrap().into_iter().map(|s| s.id).collect();
        v.sort();
        v
    };
    // What a job makes of it: the same id (a regeneration / revision keeps the id of `previous`).
    let result = |text: &str| {
        let mut d = original.clone();
        d.body = vec![p(text)];
        d
    };
    // The job took its input after the draft was last stored …
    let after_save = Utc::now() + Duration::seconds(30);
    // … or long before (a failed job retried from the job list keeps its creation time and its `previous`).
    let hour_ago = Utc::now() - Duration::hours(1);
    let input = |captured_at| JobInput { previous: Some(&original), captured_at, existed: true };

    // 1. Nothing happened meanwhile: the result replaces the draft, and the outcome carries the stored time.
    reset();
    let mut d = result("고쳐 쓴 본문");
    assert_eq!(save_job_result(&w.paths, &mut d, &input(after_save), false, true).unwrap(), JobSave::Saved);
    assert_eq!((d.id.as_str(), ids()), ("dr-orig", vec!["dr-orig".to_string()]));
    assert_eq!(stored("dr-orig").body, vec![p("고쳐 쓴 본문")]);
    assert_eq!(stored("dr-orig").updated_at, d.updated_at);

    // 2. Cancelled: the complete draft stays, what the job wrote so far becomes a draft of its own.
    reset();
    let mut partial = result("1절만 다시 쓴 부분 결과");
    partial.notes.push("취소되어 2/8절까지만 생성했습니다 (여기까지 생성됨).".into());
    let saved = save_job_result(&w.paths, &mut partial, &input(after_save), true, true).unwrap();
    assert_eq!(saved, JobSave::Copy { original: "dr-orig".into(), reason: CopyReason::Cancelled });
    assert_ne!(partial.id, "dr-orig");
    assert_eq!(stored("dr-orig").body, original.body, "the complete draft is untouched");
    let copy = stored(&partial.id);
    assert_eq!(copy.body, vec![p("1절만 다시 쓴 부분 결과")]);
    assert!(
        copy.notes[0].starts_with("취소된 작업이 남긴 결과입니다") && copy.notes[1].contains("2/8절"),
        "{:?}",
        copy.notes
    );
    assert_eq!(copy.saved_path, None, "the export belongs to the original");
    assert_eq!(stored("dr-orig").saved_path, original.saved_path);
    assert_eq!(ids().len(), 2);
    // A cancelled new draft keeps what was written under its own id, as before.
    let mut fresh = quotation_draft();
    let fresh_id = fresh.id.clone();
    let no_previous = JobInput { previous: None, captured_at: after_save, existed: false };
    assert_eq!(save_job_result(&w.paths, &mut fresh, &no_previous, true, true).unwrap(), JobSave::Saved);
    assert!(fresh.id == fresh_id && store::exists(&w.paths, &fresh_id));

    // 3. Retried after the user edited and saved the draft: the edits stay, the result goes next to them.
    reset();
    let mut edited = original.clone();
    edited.body.push(p("그 사이에 사용자가 덧붙인 문단"));
    store::save(&w.paths, &edited).unwrap();
    let mut late = result("옛 스냅샷으로 만든 결과");
    let saved = save_job_result(&w.paths, &mut late, &input(hour_ago), false, false).unwrap();
    assert_eq!(saved, JobSave::Copy { original: "dr-orig".into(), reason: CopyReason::Newer });
    assert_eq!(stored("dr-orig").body, edited.body, "the newer edits survive");
    assert_eq!(stored(&late.id).body, vec![p("옛 스냅샷으로 만든 결과")]);
    assert!(stored(&late.id).notes[0].starts_with("The original draft was edited after this job started"));

    // 4. Newer on disk but saying the same as the job's input (the editor's save raced the job): replaced.
    reset();
    let mut d = result("결과");
    assert_eq!(save_job_result(&w.paths, &mut d, &input(hour_ago), false, true).unwrap(), JobSave::Saved);
    assert_eq!(stored("dr-orig").body, vec![p("결과")]);

    // 5. Stored before the job took its input, and different from it (the request carried edits the
    //    editor had not saved yet): replaced — the job's input was the newer one.
    reset();
    let mut unsaved = original.clone();
    unsaved.title = "아직 저장되지 않은 편집".into();
    let from_editor = JobInput { previous: Some(&unsaved), captured_at: after_save, existed: true };
    let mut d = unsaved.clone();
    d.body = vec![p("결과")];
    assert_eq!(save_job_result(&w.paths, &mut d, &from_editor, false, true).unwrap(), JobSave::Saved);
    assert_eq!(stored("dr-orig").title, "아직 저장되지 않은 편집");

    // 6. Deleted while the job ran: not brought back. (A draft that was not stored when the job started is.)
    reset();
    store::delete(&w.paths, "dr-orig").unwrap();
    let mut d = result("결과");
    let saved = save_job_result(&w.paths, &mut d, &input(after_save), false, true).unwrap();
    assert_eq!(saved, JobSave::NotSaved { reason: SkipReason::Deleted });
    assert!(ids().is_empty());
    let not_stored = JobInput { previous: Some(&original), captured_at: after_save, existed: false };
    assert_eq!(save_job_result(&w.paths, &mut d, &not_stored, false, true).unwrap(), JobSave::Saved);
    assert_eq!(ids(), ["dr-orig"]);

    // The shapes the job result carries (`draft_store`).
    let json = |s: JobSave| serde_json::to_value(s).unwrap();
    assert_eq!(json(JobSave::Saved), json!({"status": "saved"}));
    assert_eq!(
        json(JobSave::Copy { original: "dr-orig".into(), reason: CopyReason::Cancelled }),
        json!({"status": "copy", "original": "dr-orig", "reason": "cancelled"})
    );
    assert_eq!(
        json(JobSave::Copy { original: "dr-orig".into(), reason: CopyReason::Newer }),
        json!({"status": "copy", "original": "dr-orig", "reason": "newer"})
    );
    assert_eq!(
        json(JobSave::NotSaved { reason: SkipReason::Deleted }),
        json!({"status": "not_saved", "reason": "deleted"})
    );
}

/// GEN-3, the audit's scenario end to end: "전체 다시 쓰기" of a filled draft cancelled after two of five
/// sections. The job result is partial; stored through `save_job_result` it leaves the complete draft alone.
#[test]
fn a_cancelled_regeneration_keeps_the_complete_draft() {
    use super::store::{save_job_result, CopyReason, JobInput, JobSave};
    let w = world();
    let complete = filled_progress_report();
    store::save(&w.paths, &complete).unwrap();
    let para = |t: &str| json!({"kind": "paragraph", "text": t, "items": [], "source_index": null});
    let head = json!({
        "title": "11월 2주차 진척 보고서",
        "fields": [],
        "tables": [],
        "body": [{"kind": "heading", "text": "보고 개요", "items": [], "source_index": null}, para("새 개요")],
        "notes": []
    });
    let chat = SeqChat::new(vec![head.to_string(), json!({ "blocks": [para("새 본문")] }).to_string()]);
    let cancel = AtomicBool::new(false);
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, Some(&cancel));
    let on_progress = |_f: f32, detail: &str| {
        if detail == "section 3/5" {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let mut deps = w.deps(Some(&vlm), None);
    deps.progress = Some(&on_progress);
    let req = DraftRequest {
        form_ref: Some("progress_report".into()),
        content: "한빛 포털 구축 진척 보고서".into(),
        max_tokens: Some(8_000),
        previous: Some(Box::new(complete.clone())),
        feedback: Some("더 간결하게".into()),
        revise: Some(super::ReviseScope::Whole),
        use_knowledge: false,
        ..Default::default()
    };
    let captured_at = chrono::Utc::now();
    let mut out = draft::draft(&deps, &req).unwrap();
    assert_eq!(out.sections, 2, "cancelled between sections");
    assert_eq!(out.draft.id, complete.id, "a regeneration keeps the id — it used to be stored over the draft");
    let input = JobInput { previous: req.previous.as_deref(), captured_at, existed: true };
    let saved = save_job_result(&w.paths, &mut out.draft, &input, true, true).unwrap();
    assert_eq!(saved, JobSave::Copy { original: complete.id.clone(), reason: CopyReason::Cancelled });
    let sections = |d: &Draft| super::sections_of(&d.body).len();
    assert_eq!(sections(&store::load(&w.paths, &complete.id).unwrap()), 5, "the complete draft is still stored");
    let partial = store::load(&w.paths, &out.draft.id).unwrap();
    assert_eq!(sections(&partial), 2);
    assert!(partial.notes[0].starts_with("취소된 작업이 남긴 결과입니다"), "{:?}", partial.notes);
    assert!(partial.notes.iter().any(|n| n.contains("2/5절까지만")), "{:?}", partial.notes);
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
    // Markdown is an input format too (text family, 2026-11-09): the file is registered as a generated
    // document and converted through the text reader, so its content is searchable.
    let md =
        render::save(&deps, &SaveRequest { draft: d.clone(), output: OutputFormat::Md, ..Default::default() }).unwrap();
    let md_id = md.doc_id.clone().expect("markdown registered");
    assert!(!md.warnings.iter().any(|w| w.contains("Markdown")), "{:?}", md.warnings);
    let md_doc = w.db.with(|c| docs::get_document(c, &md_id)).unwrap().unwrap();
    assert_eq!(md_doc.source_kind, crate::documents::SourceKind::Text);
    assert_eq!(md_doc.role, crate::documents::DocumentRole::Generated);
    let md_pages = w.db.with(|c| docs::list_pages(c, &md_id, true)).unwrap();
    assert!(md_pages.first().and_then(|p| p.text.as_deref()).unwrap_or("").contains("노트북"), "{md_pages:?}");
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
    write_docx_package(path, &body);
}

/// A minimal DOCX package around the given `<w:body>` content.
fn write_docx_package(path: &Path, body: &str) {
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
    let form_before = docx::read(&form_abs).unwrap().text();
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
    assert_eq!(docx::read(&form_abs).unwrap().text(), form_before, "the form file is unchanged");
    // PDF / Markdown of a form draft use the generic renderer.
    let md =
        render::save(&deps, &SaveRequest { draft: d, output: OutputFormat::Md, register: false, ..Default::default() })
            .unwrap();
    assert!(!md.filled_form);
    assert!(std::fs::read_to_string(&md.absolute).unwrap().contains("| 수신 | ㈜한빛 |"));
}

#[test]
fn save_never_overwrites_forms_or_library_documents() {
    let w = world();
    let form_rel = "documents/forms/견적서양식.docx";
    let form_abs = w.paths.root.join(form_rel);
    std::fs::create_dir_all(form_abs.parent().unwrap()).unwrap();
    write_docx_form(&form_abs);
    let work_rel = "documents/generated/보고서.docx";
    w.db.with(|c| {
        for (id, rel, name, role) in [
            ("form-doc", form_rel, "견적서양식.docx", DocumentRole::Form),
            ("work-doc", work_rel, "보고서.docx", DocumentRole::Work),
        ] {
            docs::insert_document(
                c,
                &NewDocument {
                    id: id.into(),
                    rel_path: rel.into(),
                    original_name: name.into(),
                    source_kind: SourceKind::Docx,
                    content_hash: format!("hash-{id}"),
                    size_bytes: 10,
                    file_modified_at: None,
                    role,
                    import_batch_id: None,
                },
            )?;
        }
        Ok(())
    })
    .unwrap();
    let imp = w.import_deps();
    let deps = w.deps(None, Some(&imp));
    let form_before = docx::read(&form_abs).unwrap().text();
    // Filling the form onto its own path is refused even with `overwrite`.
    let err = render::save(
        &deps,
        &SaveRequest {
            draft: form_draft(Some("form-doc")),
            output: OutputFormat::Docx,
            file_name: Some("견적서양식".into()),
            folder: Some("documents/forms".into()),
            overwrite: true,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(ref c) if c == "export_over_form"), "{err}");
    assert_eq!(docx::read(&form_abs).unwrap().text(), form_before);
    assert!(!form_abs.with_extension("docx.part").exists(), "no leftovers");
    // A library document (role work) at the target path is refused before the overwrite
    // question, whether or not its file is on disk.
    for overwrite in [false, true] {
        let err = render::save(
            &deps,
            &SaveRequest {
                draft: quotation_draft(),
                output: OutputFormat::Docx,
                file_name: Some("보고서".into()),
                overwrite,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::Invalid(ref c) if c == "export_over_document"), "{err}");
    }
    assert!(!w.paths.root.join(work_rel).exists());
    let doc = w.db.with(|c| docs::get_document(c, "work-doc")).unwrap().unwrap();
    assert_eq!(doc.role, DocumentRole::Work, "the row is untouched");
}

/// Registers `write_docx_form` as the form document `form-doc` of the workspace.
fn register_docx_form(w: &World) {
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
}

/// GEN-11: a registered HWPX / DOCX form takes the fields and the tables only. Saving a draft that has a
/// body into it used to drop the body without a word (the preview shows it); now the save says so.
#[test]
fn saving_into_a_registered_form_warns_about_the_body_it_leaves_out() {
    let w = world();
    register_docx_form(&w);
    let deps = w.deps(None, None);
    let mut d = form_draft(Some("form-doc"));
    d.body = vec![
        DraftBlock::Heading { level: 2, text: "납품 조건".into() },
        DraftBlock::Paragraph { text: "납품은 발주 후 2주 이내에 완료합니다.".into(), source: None },
        DraftBlock::TableRef { key: "items".into() },
        DraftBlock::List { items: vec!["무상 보증 1년".into()], ordered: false },
        DraftBlock::Paragraph { text: "  ".into(), source: None },
    ];
    let save = |draft: &Draft, output: OutputFormat, name: &str| {
        render::save(
            &deps,
            &SaveRequest {
                draft: draft.clone(),
                output,
                file_name: Some(name.into()),
                register: false,
                ..Default::default()
            },
        )
        .unwrap()
    };
    let saved = save(&d, OutputFormat::Docx, "본문포함");
    assert!(saved.filled_form);
    let body_warnings: Vec<&String> = saved.warnings.iter().filter(|w| w.contains("본문")).collect();
    assert_eq!(body_warnings.len(), 1, "{:?}", saved.warnings);
    assert!(body_warnings[0].contains("3개"), "three blocks carry text: {}", body_warnings[0]);
    assert_eq!(saved.omitted_body_blocks, 3, "the heading, the paragraph and the list");
    assert_eq!(serde_json::to_value(&saved).unwrap()["omitted_body_blocks"], json!(3));
    // The fill itself is as before: fields and item rows are placed, the body is not in the file.
    let back = docx::read(&saved.absolute).unwrap();
    assert_eq!(table_texts(&back)[0][0], vec!["수신", "㈜한빛", "견적일자", "2026-11-08"]);
    assert!(!back.text().contains("납품은 발주 후 2주"), "{}", back.text());
    // Nothing to leave out, nothing to warn about: a draft without a body, and the generic renderer
    // (Markdown / PDF of a form draft), which does print the body.
    let plain = save(&form_draft(Some("form-doc")), OutputFormat::Docx, "본문없음");
    assert!(plain.filled_form && !plain.warnings.iter().any(|w| w.contains("본문")), "{:?}", plain.warnings);
    assert_eq!(plain.omitted_body_blocks, 0);
    let md = save(&d, OutputFormat::Md, "본문포함");
    assert!(!md.filled_form && !md.warnings.iter().any(|w| w.contains("본문")), "{:?}", md.warnings);
    assert_eq!(md.omitted_body_blocks, 0);
    assert!(std::fs::read_to_string(&md.absolute).unwrap().contains("납품은 발주 후 2주 이내에 완료합니다."));
}

/// GEN-2 in a filled form: the slots get the same values as the rendered document — a percentage, a
/// person-month figure and a meeting time as entered, money with separators.
#[test]
fn filled_forms_keep_numbers_and_times_as_entered() {
    use FieldKind::*;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("form.docx");
    let mut body = "<w:tbl><w:tblPr/>".to_string();
    for label in ["전체 진척률(%)", "통과율(%)", "일시", "계약금액"] {
        body.push_str(&format!("<w:tr>{}{}</w:tr>", docx_text_cell(label), docx_cell("<w:p/>")));
    }
    body.push_str("</w:tbl><w:p/><w:tbl><w:tblPr/>");
    body.push_str(&format!(
        "<w:tr>{}{}{}{}</w:tr>",
        docx_text_cell("역할"),
        docx_text_cell("투입 MM"),
        docx_text_cell("단가"),
        docx_text_cell("공급가액")
    ));
    body.push_str(&format!("<w:tr>{}</w:tr></w:tbl>", docx_cell("<w:p/>").repeat(4)));
    write_docx_package(&src, &body);
    let mut d =
        Draft { template: TemplateRef::Form { schema_id: "form-schema".into(), doc_id: None }, ..Default::default() };
    d.fields = vec![
        field("progress_percent", "전체 진척률(%)", Amount, false, "62.5"),
        field("pass_rate", "통과율(%)", Amount, false, "95.2"),
        field("meeting_date", "일시", Date, false, "2026-11-10 14:00"),
        field("amount", "계약금액", Amount, false, "120000000"),
    ];
    d.tables = vec![DraftTable {
        key: "items".into(),
        label: "견적 내역".into(),
        columns: vec![
            col("role", "역할", String),
            col("months", "투입 MM", Amount),
            col("unit_price", "단가", Amount),
            col("supply_amount", "공급가액", Amount),
        ],
        rows: vec![vec!["PM".into(), "0.5".into(), "12000000".into(), "6000000".into()]],
    }];
    validate::validate(&mut d);
    let out = dir.path().join("filled.docx");
    let warnings = fill::fill_form(&src, SourceKind::Docx, &d, &out).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let tables = table_texts(&docx::read(&out).unwrap());
    assert_eq!(
        tables[0],
        vec![
            vec!["전체 진척률(%)", "62.5"],
            vec!["통과율(%)", "95.2"],
            vec!["일시", "2026-11-10 14:00"],
            vec!["계약금액", "120,000,000"],
        ]
    );
    assert_eq!(tables[1][1], vec!["PM", "0.5", "12,000,000", "6,000,000"]);
}

// ---------------------------------------------------------------------------
// Token budgets and the outline → sections flow (0.3.0 item 4)
// ---------------------------------------------------------------------------

#[test]
fn token_budget_helpers_and_split_decision() {
    let req = DraftRequest { length: super::Length::Medium, ..Default::default() };
    assert_eq!(super::prompts::reply_budget(&req), 1_800);
    let req = DraftRequest { max_tokens: Some(24_000), ..Default::default() };
    assert_eq!(super::prompts::reply_budget(&req), 24_000);
    assert!(!super::prompts::split_needed(2_600, Some(8_192)));
    assert!(super::prompts::split_needed(3_001, None));
    assert!(super::prompts::split_needed(3_000, Some(8_192)), "more than 35 % of an 8K context");
    assert_eq!(super::prompts::section_budget(24_000, 1, 8), 3_000);
    assert_eq!(super::prompts::section_budget(6_000, 1, 8), 750);
    assert_eq!(super::prompts::section_budget(2_000, 1, 8), 600);
    assert!(schema::is_grammar_safe(&schema::outline_schema()));
    assert!(schema::is_grammar_safe(&schema::section_schema()));
    assert_eq!(super::prompts::fallback_outline(true).len(), 5);

    // GEN-13: the instruction, the content, the earlier text of a revision and the sources share what the
    // context leaves next to the fixed prompt (1,400), the reply and a margin (256).
    use super::prompts::{allot, Allowance, Blocks};
    let want = |content, feedback, previous| Blocks { content, feedback, previous, fixed: 0, sources: 3 };
    // Unknown context: the static caps, as before.
    assert_eq!(
        allot(None, 1_800, want(100, 50, 3_000)),
        Allowance { content: 100, feedback: 50, previous: 3_000, sources: 4_500 }
    );
    // 8K with a medium reply leaves 4,736 characters: a plain draft keeps the full source cap …
    assert_eq!(allot(Some(8_192), 1_800, want(100, 0, 0)).sources, 4_500);
    // … a regeneration shows its previous draft and gives the sources the rest …
    assert_eq!(
        allot(Some(8_192), 1_800, want(100, 36, 3_000)),
        Allowance { content: 100, feedback: 36, previous: 3_000, sources: 1_600 }
    );
    // … and when it gets tight the previous draft is cut before the sources fall under their floor.
    assert_eq!(
        allot(Some(8_192), 2_600, want(100, 36, 6_000)),
        Allowance { content: 100, feedback: 36, previous: 3_200, sources: 600 }
    );
    // Blocks shown as they are (facts, the split note) come off the top.
    assert_eq!(allot(Some(8_192), 2_600, Blocks { fixed: 1_000, ..want(100, 36, 6_000) }).previous, 2_200);
    // Every source keeps a slice (200 characters each): twelve of them need 2,400.
    assert_eq!(
        allot(Some(8_192), 1_800, Blocks { sources: 12, ..want(100, 36, 3_000) }),
        Allowance { content: 100, feedback: 36, previous: 2_200, sources: 2_400 }
    );
    // Without sources the previous draft may use the room.
    assert_eq!(allot(Some(8_192), 2_600, Blocks { sources: 0, ..want(100, 36, 6_000) }).previous, 3_800);
    // A context too small for everything: each block keeps its floor.
    assert_eq!(
        allot(Some(4_096), 2_600, want(2_000, 100, 3_000)),
        Allowance { content: 600, feedback: 100, previous: 600, sources: 600 }
    );
}

/// Characters of the variable blocks of a draft prompt: everything from `[내용]` up to the output rules
/// (content, sources, previous draft / revision context, instruction).
fn variable_chars(user_prompt: &str) -> usize {
    let from = user_prompt.find("[내용]").expect("content block");
    let to = user_prompt.rfind("[출력 규칙]").expect("output rules");
    user_prompt[from..to].chars().count()
}

/// A 진척 보고서 whose five sections are filled (700 characters each) — the "내용이 찬 초안" of GEN-13.
fn filled_progress_report() -> Draft {
    let text = |seed: &str, n: usize| seed.chars().cycle().take(n).collect::<String>();
    let mut d = template_draft(
        TemplateKind::ProgressReport,
        &[("project_name", "한빛 포털 구축"), ("report_date", "2026-11-10"), ("progress_percent", "62.5")],
    );
    d.id = "dr-filled".into();
    d.title = "11월 2주차 진척 보고서".into();
    d.fields.iter_mut().find(|f| f.key == "project_name").unwrap().user_edited = true;
    for (i, heading) in TemplateSpec::builtin(TemplateKind::ProgressReport).sections(true).iter().enumerate() {
        d.body.push(DraftBlock::Heading { level: 2, text: heading.clone() });
        d.body
            .push(DraftBlock::Paragraph { text: text(&format!("{}절 본문입니다. ", i + 1), 700), source: None });
    }
    d
}

/// GEN-13: a regeneration / revision prompt carries the previous draft (or the section and its surroundings)
/// next to the sources. The source budget ignored those blocks, so on an 8K context (CPU, speed profile) a
/// filled draft plus full sources overflowed the context and the reply was cut. Every variable block now
/// shares what the context leaves next to the fixed prompt and the reply.
#[test]
fn regeneration_and_revision_prompts_fit_a_small_context() {
    use super::ReviseScope;
    const CTX: u32 = 8_192;
    // What `prompts` leaves for the variable blocks: context − fixed prompt (1,400) − reply − margin (256).
    let free = |reply: u32| (CTX - 1_400 - reply - 256) as usize;
    // Block titles and the four `<source …>` tags are not counted by the budget.
    const MARKUP: usize = 400;
    let w = world();
    let prev = filled_progress_report();
    let sources: Vec<SourceRef> = (1..=4)
        .map(|i| SourceRef::Text {
            title: Some(format!("회의 메모 {i}")),
            text: format!("메모 {i}의 내용입니다. ").chars().cycle().take(2_000).collect(),
        })
        .collect();
    let base = DraftRequest {
        form_ref: Some("progress_report".into()),
        content: "한빛 포털 구축 11월 2주차 진척 보고서".into(),
        sources,
        use_knowledge: false,
        previous: Some(Box::new(prev.clone())),
        feedback: Some("표현을 더 간결하게 다듬어 주세요".into()),
        ..Default::default()
    };
    let para = |t: &str| json!({"kind": "paragraph", "text": t, "items": [], "source_index": null});
    let whole_reply = json!({
        "title": "11월 2주차 진척 보고서",
        "fields": [{"key": "project_name", "value": "한빛 포털 구축", "source_index": null}],
        "tables": [],
        "body": [{"kind": "heading", "text": "보고 개요", "items": [], "source_index": null}, para("개요입니다.")],
        "notes": []
    })
    .to_string();
    let section_reply = json!({ "blocks": [para("다듬은 본문입니다.")] }).to_string();

    // 1. "전체 다시 쓰기" in one call (medium length = 1,800 tokens).
    let chat = SeqChat::new(vec![whole_reply.clone()]);
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None).with_context(Some(CTX));
    draft::draft(&w.deps(Some(&vlm), None), &base).unwrap();
    let prompt = chat.user_prompt(0);
    let used = variable_chars(&prompt);
    assert!(used <= free(1_800) + MARKUP, "one-shot regeneration: {used} > {}", free(1_800));
    // The previous draft is still what the prompt is about, and every source keeps a slice.
    assert!(prompt.contains("[이전 초안]") && prompt.contains("project_name: 한빛 포털 구축 (사용자 수정"), "{prompt}");
    assert!(prompt.contains("1절 본문입니다.") && prompt.contains("[수정 요청]\n표현을 더 간결하게"), "{prompt}");
    assert_eq!(prompt.matches("<source index=").count(), 4, "{prompt}");
    assert!(prompt.contains("메모 4의 내용입니다."), "{prompt}");

    // 2. A section rewrite: the section's text and its surroundings count as well.
    let chat2 = SeqChat::new(vec![section_reply.clone()]);
    let vlm2 = Vlm::new(&chat2, "scripted".into(), Profile::Balanced, None).with_context(Some(CTX));
    let revise = DraftRequest { revise: Some(ReviseScope::Section { section: 2 }), ..base.clone() };
    draft::draft(&w.deps(Some(&vlm2), None), &revise).unwrap();
    let prompt = chat2.user_prompt(0);
    let used = variable_chars(&prompt);
    assert!(used <= free(1_800) + MARKUP, "section revision: {used} > {}", free(1_800));
    assert!(prompt.contains("[현재 절 본문]\n2절 본문입니다."), "{prompt}");
    assert_eq!(prompt.matches("<source index=").count(), 4, "{prompt}");

    // 3. "전체 다시 쓰기" section by section (8,000 tokens over five sections: 1,600 each, the head 2,200).
    let chat3 = SeqChat::new(vec![
        whole_reply,
        section_reply.clone(),
        section_reply.clone(),
        section_reply.clone(),
        section_reply,
    ]);
    let vlm3 = Vlm::new(&chat3, "scripted".into(), Profile::Balanced, None).with_context(Some(CTX));
    let split = DraftRequest { max_tokens: Some(8_000), revise: Some(ReviseScope::Whole), ..base.clone() };
    let out = draft::draft(&w.deps(Some(&vlm3), None), &split).unwrap();
    assert_eq!(out.sections, 5);
    let head = variable_chars(&chat3.user_prompt(0));
    assert!(head <= free(2_200) + MARKUP, "head call: {head} > {}", free(2_200));
    for call in 1..5 {
        let prompt = chat3.user_prompt(call);
        let used = variable_chars(&prompt);
        assert!(used <= free(1_600) + MARKUP, "section call {call}: {used} > {}", free(1_600));
        assert!(prompt.contains("[수정 요청]\n표현을 더 간결하게"), "{prompt}");
    }
    // The second section call still shows that section's earlier text.
    assert!(chat3.user_prompt(1).contains("[이 절의 이전 본문]\n2절 본문입니다."), "{}", chat3.user_prompt(1));
}

/// GEN-14: the content was cut at 2,000 characters and the instruction at 600 on the way into the prompt
/// while the job bridge (and the planner) accept 6,000 / 2,000 — the rest was dropped without a word.
/// The prompt now takes what the callers accept, and when something is cut the draft says so.
#[test]
fn long_content_and_instructions_reach_the_model_or_the_draft_says_what_was_cut() {
    let w = world();
    let reply = json!({"title": "보고서", "fields": [], "tables": [], "body": [], "notes": []});
    let run = |req: &DraftRequest, ctx: u32| {
        let chat = SeqChat::json(reply.clone());
        let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None).with_context(Some(ctx));
        let out = draft::draft(&w.deps(Some(&vlm), None), req).unwrap();
        (out, chat.user_prompt(0))
    };
    // 3,000 characters of content and 900 of instruction, each ending in a marker.
    let content = format!("{}끝머리표식A", "가".repeat(2_990));
    let instruction = format!("{}끝머리표식B", "나".repeat(890));
    let req = DraftRequest {
        form_ref: Some("report".into()),
        content: content.clone(),
        feedback: Some(instruction.clone()),
        use_knowledge: false,
        ..Default::default()
    };
    // A GPU-sized context takes both in full: nothing to report.
    let (out, prompt) = run(&req, 32_768);
    assert!(prompt.contains("끝머리표식A"), "the content was cut at 2,000 characters");
    assert!(prompt.contains("끝머리표식B"), "the instruction was cut at 600 characters");
    assert!(!out.draft.notes.iter().any(|n| n.contains("전달했습니다")), "{:?}", out.draft.notes);
    assert!(out.input_cuts.is_empty());
    assert!(serde_json::to_value(&out).unwrap().get("input_cuts").is_none(), "absent when nothing was cut");

    // Over the limits themselves: the first 6,000 / 2,000 characters are passed and both cuts are reported,
    // in the notes (shown under the preview) and as `input_cuts`. ('뷁' / '쀍' occur nowhere else in a prompt.)
    let cut = |field: &str, chars: u32, passed: u32| super::InputCut { field: field.into(), chars, passed };
    let over = DraftRequest { content: "뷁".repeat(7_000), feedback: Some("쀍".repeat(2_500)), ..req.clone() };
    let (out, prompt) = run(&over, 32_768);
    assert_eq!((prompt.matches('뷁').count(), prompt.matches('쀍').count()), (6_000, 2_000));
    assert_eq!(out.input_cuts, vec![cut("content", 7_000, 6_000), cut("feedback", 2_500, 2_000)]);
    assert!(
        out.draft.notes.iter().any(|n| n.starts_with("내용 7,000자 중 앞 6,000자만 모델에 전달했습니다")),
        "{:?}",
        out.draft.notes
    );
    assert!(
        out.draft.notes.iter().any(|n| n == "수정 지시 2,500자 중 앞 2,000자만 모델에 전달했습니다."),
        "{:?}",
        out.draft.notes
    );
    assert_eq!(
        serde_json::to_value(&out).unwrap()["input_cuts"],
        json!([
            {"field": "content", "chars": 7000, "passed": 6000},
            {"field": "feedback", "chars": 2500, "passed": 2000}
        ])
    );
    // The stored request keeps the whole text (a later regeneration on a bigger context can use it).
    assert_eq!(out.draft.request.as_ref().unwrap().content.chars().count(), 7_000);

    // An 8K context cannot hold 6,000 characters of content next to the reply (8,192 − 1,400 − 1,800 − 256
    // leaves 4,736): what fits is passed and the draft says how much.
    let long = DraftRequest { content: "뷁".repeat(6_000), feedback: None, ..req.clone() };
    let (out, prompt) = run(&long, 8_192);
    assert_eq!(prompt.matches('뷁').count(), 4_736);
    assert_eq!(out.input_cuts, vec![cut("content", 6_000, 4_736)]);
    assert!(out.draft.notes.iter().any(|n| n.starts_with("내용 6,000자 중 앞 4,736자만")), "{:?}", out.draft.notes);

    // Section by section: every call has its own allowance; the smallest one is what is reported, once.
    let section = json!({ "blocks": [{"kind": "paragraph", "text": "본문", "items": [], "source_index": null}] });
    let chat = SeqChat::new(vec![reply.to_string(), section.to_string()]);
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None).with_context(Some(8_192));
    let split = DraftRequest { max_tokens: Some(8_000), ..long.clone() };
    let out = draft::draft(&w.deps(Some(&vlm), None), &split).unwrap();
    assert!(out.sections > 1);
    let calls = chat.requests.lock().unwrap().len();
    let least = (0..calls).map(|c| chat.user_prompt(c).matches('뷁').count()).min().unwrap();
    assert_eq!(out.input_cuts, vec![cut("content", 6_000, least as u32)]);
    assert_eq!(out.draft.notes.iter().filter(|n| n.starts_with("내용 6,000자 중")).count(), 1, "{:?}", out.draft.notes);

    // A section revision reports the cut instruction before its own note (the screen logs the last note).
    let chat = SeqChat::new(vec![section.to_string()]);
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None).with_context(Some(32_768));
    let revise = DraftRequest {
        form_ref: Some("progress_report".into()),
        previous: Some(Box::new(filled_progress_report())),
        feedback: Some("쀍".repeat(2_500)),
        revise: Some(super::ReviseScope::Section { section: 2 }),
        use_knowledge: false,
        ..Default::default()
    };
    let out = draft::draft(&w.deps(Some(&vlm), None), &revise).unwrap();
    assert_eq!(chat.user_prompt(0).matches('쀍').count(), 2_000);
    assert_eq!(out.input_cuts, vec![cut("feedback", 2_500, 2_000)]);
    let notes = &out.draft.notes;
    assert!(notes[notes.len() - 1].starts_with("AI 수정(2절"), "{notes:?}");
    assert_eq!(notes[notes.len() - 2], "수정 지시 2,500자 중 앞 2,000자만 모델에 전달했습니다.");

    // The job bridge only bounds what is stored with the job (20,000 characters per text). It used to cut
    // the content to 6,000 and the instruction to 2,000 itself — a cut the core could not see or report.
    let mut stored = DraftRequest {
        content: format!("  {}  ", "뷁".repeat(7_000)),
        feedback: Some("쀍".repeat(2_500)),
        sources: vec![SourceRef::Text { title: None, text: "가".repeat(25_000) }],
        ..Default::default()
    };
    stored.bound_stored_text();
    assert_eq!(stored.content.chars().count(), 7_000);
    assert_eq!(stored.feedback.as_ref().unwrap().chars().count(), 2_500);
    assert!(matches!(&stored.sources[0], SourceRef::Text { text, .. } if text.chars().count() == 20_000));
    stored.content = "뷁".repeat(30_000);
    stored.bound_stored_text();
    assert_eq!(stored.content.chars().count(), super::MAX_STORED_TEXT_CHARS);
}

#[test]
fn a_large_budget_drafts_section_by_section_and_keeps_sections_on_cancel() {
    let w = world();
    // 진척 보고서 has five template sections: one head call (fields + section 1) and four section calls.
    let head = json!({
        "title": "10월 진척 보고서",
        "fields": [
            {"key": "project_name", "value": "한빛 포털 구축", "source_index": null},
            {"key": "report_date", "value": "2026-11-08", "source_index": null}
        ],
        "tables": [],
        "body": [
            {"kind": "heading", "text": "보고 개요", "items": [], "source_index": null},
            {"kind": "paragraph", "text": "10월 진척 현황을 보고합니다.", "items": [], "source_index": null}
        ],
        "notes": []
    });
    let section = |text: &str| {
        json!({ "blocks": [
            {"kind": "paragraph", "text": text, "items": [], "source_index": null},
            {"kind": "heading", "text": "세부", "items": [], "source_index": null},
            {"kind": "list", "text": "", "items": ["항목 1", "항목 2"], "source_index": null}
        ] })
        .to_string()
    };
    let replies = || {
        vec![
            head.to_string(),
            section("금주 실적입니다."),
            section("차주 계획입니다."),
            section("이슈입니다."),
            section("지원 요청입니다."),
        ]
    };
    let chat = SeqChat::new(replies());
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None);
    let progress: Mutex<Vec<(f32, String)>> = Mutex::new(Vec::new());
    let cb = |f: f32, d: &str| progress.lock().unwrap().push((f, d.to_string()));
    let mut deps = w.deps(Some(&vlm), None);
    deps.progress = Some(&cb);
    let req = DraftRequest {
        form_ref: Some("progress_report".into()),
        content: "한빛 포털 구축 10월 진척 보고서".into(),
        max_tokens: Some(8_000),
        use_knowledge: false,
        ..Default::default()
    };
    let out = draft::draft(&deps, &req).unwrap();
    assert_eq!(out.sections, 5);
    let d = &out.draft;
    let headings: Vec<&str> = d
        .body
        .iter()
        .filter_map(|b| match b {
            DraftBlock::Heading { level: 2, text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(headings, ["보고 개요", "금주 실적", "차주 계획", "이슈 및 리스크", "지원 요청 사항"]);
    // The model's own headings inside a section become sub-headings.
    assert!(d.body.iter().any(|b| matches!(b, DraftBlock::Heading { level: 3, text } if text == "세부")));
    assert_eq!(d.field("project_name").unwrap().value, "한빛 포털 구축");
    assert_eq!((out.prompt_tokens, out.completion_tokens), (500, 100), "usage summed over five calls");
    assert!(d.notes.iter().any(|n| n.contains("5개 절")), "{:?}", d.notes);
    let details: Vec<String> = progress.lock().unwrap().iter().map(|(_, d)| d.clone()).collect();
    assert!(details.iter().any(|x| x == "section 1/5") && details.iter().any(|x| x == "section 5/5"), "{details:?}");
    assert_eq!(details.last().map(String::as_str), Some("finishing"));
    // Section prompts carry the settled facts and the tail of the previous text; budgets split 8,000 over 5.
    let p2 = chat.prompt(1);
    assert!(
        p2.contains("[확정 정보]")
            && p2.contains("한빛 포털 구축")
            && p2.contains("[앞 절 요약]")
            && p2.contains("2/5번째 절"),
        "{p2}"
    );
    assert!(
        chat.requests.lock().unwrap()[1].response_format.as_ref().unwrap()["json_schema"]["name"] == "draft_section"
    );
    let budgets: Vec<Option<u32>> = chat.requests.lock().unwrap().iter().map(|r| r.max_tokens).collect();
    assert_eq!(budgets[0], Some(1_600 + super::prompts::HEAD_EXTRA_TOKENS));
    assert!(budgets[1..].iter().all(|b| *b == Some(1_600)), "{budgets:?}");

    // Cancellation while the third section is requested keeps the two sections written so far.
    let chat2 = SeqChat::new(replies());
    let cancel = AtomicBool::new(false);
    let vlm2 = Vlm::new(&chat2, "scripted".into(), Profile::Balanced, Some(&cancel));
    let cb2 = |_f: f32, d: &str| {
        if d == "section 3/5" {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let mut deps2 = w.deps(Some(&vlm2), None);
    deps2.progress = Some(&cb2);
    let out2 = draft::draft(&deps2, &req).unwrap();
    assert_eq!(out2.sections, 2);
    assert!(out2.draft.notes.iter().any(|n| n.contains("2/5절까지만")), "{:?}", out2.draft.notes);
    assert_eq!(out2.draft.body.iter().filter(|b| matches!(b, DraftBlock::Heading { level: 2, .. })).count(), 2);

    // A small budget keeps the one-shot path.
    let chat3 = SeqChat::json(model_reply());
    let vlm3 = Vlm::new(&chat3, "scripted".into(), Profile::Balanced, None);
    let deps3 = w.deps(Some(&vlm3), None);
    let out3 = draft::draft(
        &deps3,
        &DraftRequest {
            content: "노트북 견적서".into(),
            max_tokens: Some(2_000),
            use_knowledge: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(out3.sections, 1);
    assert_eq!(chat3.requests.lock().unwrap()[0].max_tokens, Some(2_000));
}

/// 0.7.0 plan §1.5: `DraftRequest.revise` rewrites one section or inserts one with a single
/// `draft_revise` call that carries the current text, the surrounding text and the instruction;
/// a whole regeneration of a split draft passes the instruction (and the section's earlier text)
/// to every section call.
#[test]
fn revise_edits_one_section_in_place_and_split_regeneration_carries_the_instruction() {
    use super::{Draft, DraftRequest, ReviseScope, TemplateKind, TemplateRef};
    let w = world();
    let h = |t: &str| DraftBlock::Heading { level: 2, text: t.into() };
    let p = |t: &str| DraftBlock::Paragraph { text: t.into(), source: None };
    let previous = Draft {
        id: "dr-1".into(),
        title: "10월 진척 보고서".into(),
        template: TemplateRef::Builtin { template: TemplateKind::ProgressReport },
        doc_type: Some("progress_report".into()),
        language: Language::Ko,
        body: vec![
            h("보고 개요"),
            p("10월 진척 현황"),
            h("금주 실적"),
            p("실적 A"),
            DraftBlock::TableRef { key: "items".into() },
            h("차주 계획"),
            p("계획 B"),
        ],
        ..Default::default()
    };
    let blocks = |items: Vec<Value>| json!({ "blocks": items }).to_string();
    let para = |t: &str| json!({"kind": "paragraph", "text": t, "items": [], "source_index": null});

    // 1. Rewrite section 2: heading kept, body replaced, the other sections untouched.
    let chat = SeqChat::new(vec![blocks(vec![para("실적 A를 더 자세히: 1,200 MM")])]);
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None);
    let deps = w.deps(Some(&vlm), None);
    let req = DraftRequest {
        form_ref: Some("progress_report".into()),
        previous: Some(Box::new(previous.clone())),
        feedback: Some("2절을 더 자세히".into()),
        revise: Some(ReviseScope::Section { section: 2 }),
        use_knowledge: false,
        ..Default::default()
    };
    let out = draft::draft(&deps, &req).unwrap();
    assert_eq!(out.sections, 1);
    let d = &out.draft;
    assert_eq!(d.id, "dr-1");
    let headings = |d: &Draft| -> Vec<String> {
        d.body
            .iter()
            .filter_map(|b| match b {
                DraftBlock::Heading { level: 2, text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(headings(d), ["보고 개요", "금주 실적", "차주 계획"]);
    assert!(matches!(&d.body[1], DraftBlock::Paragraph { text, .. } if text == "10월 진척 현황"));
    assert!(matches!(&d.body[3], DraftBlock::Paragraph { text, .. } if text == "실적 A를 더 자세히: 1,200 MM"));
    // The table the section placed stays in it (the model cannot emit table references).
    assert!(matches!(&d.body[4], DraftBlock::TableRef { key } if key == "items"), "{:?}", d.body);
    assert!(matches!(&d.body[6], DraftBlock::Paragraph { text, .. } if text == "계획 B"));
    assert!(d.notes.iter().any(|n| n.starts_with("AI 수정(2절 '금주 실적')")), "{:?}", d.notes);
    assert!(d.request.as_ref().is_some_and(|r| r.previous.is_none() && r.revise.is_none() && r.feedback.is_none()));
    let prompt = chat.prompt(0);
    for needle in [
        "2/3번째 절",
        "[현재 절 본문]",
        "실적 A",
        "[앞 내용 요약]",
        "10월 진척 현황",
        "[뒤 내용 요약]",
        "계획 B",
        "[수정 요청]",
        "2절을 더 자세히",
    ] {
        assert!(prompt.contains(needle), "missing {needle}: {prompt}");
    }
    assert_eq!(
        chat.requests.lock().unwrap()[0].response_format.as_ref().unwrap()["json_schema"]["name"],
        "draft_revise"
    );

    // 2. Insert a titled section after section 1.
    let chat2 = SeqChat::new(vec![blocks(vec![para("리스크 없음")])]);
    let vlm2 = Vlm::new(&chat2, "scripted".into(), Profile::Balanced, None);
    let deps2 = w.deps(Some(&vlm2), None);
    let out2 = draft::draft(
        &deps2,
        &DraftRequest {
            previous: Some(Box::new(previous.clone())),
            feedback: Some("리스크 절 추가".into()),
            revise: Some(ReviseScope::InsertAfter { section: Some(1), heading: Some("리스크".into()) }),
            ..req.clone()
        },
    )
    .unwrap();
    assert_eq!(headings(&out2.draft), ["보고 개요", "리스크", "금주 실적", "차주 계획"]);
    assert!(matches!(&out2.draft.body[3], DraftBlock::Paragraph { text, .. } if text == "리스크 없음"));
    assert!(out2.draft.body.iter().any(|b| matches!(b, DraftBlock::TableRef { key } if key == "items")));
    assert!(chat2.prompt(0).contains("절 제목은 \"리스크\""), "{}", chat2.prompt(0));

    // 3. Insert at the end without a title: the reply's first heading becomes the section title.
    let chat3 = SeqChat::new(vec![blocks(vec![
        json!({"kind": "heading", "text": "결론", "items": [], "source_index": null}),
        para("마무리"),
    ])]);
    let vlm3 = Vlm::new(&chat3, "scripted".into(), Profile::Balanced, None);
    let deps3 = w.deps(Some(&vlm3), None);
    let out3 = draft::draft(
        &deps3,
        &DraftRequest {
            previous: Some(Box::new(previous.clone())),
            feedback: Some("결론 절 추가".into()),
            revise: Some(ReviseScope::InsertAfter { section: None, heading: None }),
            ..req.clone()
        },
    )
    .unwrap();
    assert_eq!(headings(&out3.draft), ["보고 개요", "금주 실적", "차주 계획", "결론"]);
    assert!(matches!(out3.draft.body.last(), Some(DraftBlock::Paragraph { text, .. }) if text == "마무리"));

    // 4. Errors: a missing section, a missing instruction.
    let chat4 = SeqChat::new(vec![]);
    let vlm4 = Vlm::new(&chat4, "scripted".into(), Profile::Balanced, None);
    let deps4 = w.deps(Some(&vlm4), None);
    let bad = draft::draft(&deps4, &DraftRequest { revise: Some(ReviseScope::Section { section: 9 }), ..req.clone() });
    assert!(matches!(bad, Err(CoreError::Invalid(_))), "{bad:?}");
    let bad = draft::draft(&deps4, &DraftRequest { feedback: None, ..req.clone() });
    assert!(matches!(bad, Err(CoreError::Invalid(_))), "{bad:?}");

    // 5. Whole regeneration through the split flow: every section call sees the instruction and,
    //    when the heading matches, the section's earlier text.
    let head = json!({
        "title": "10월 진척 보고서",
        "fields": [{"key": "project_name", "value": "한빛 포털 구축", "source_index": null}],
        "tables": [],
        "body": [{"kind": "heading", "text": "보고 개요", "items": [], "source_index": null}, para("개요입니다.")],
        "notes": []
    })
    .to_string();
    let section = |t: &str| blocks(vec![para(t)]);
    let chat5 = SeqChat::new(vec![
        head,
        section("실적입니다."),
        section("계획입니다."),
        section("이슈입니다."),
        section("지원입니다."),
    ]);
    let vlm5 = Vlm::new(&chat5, "scripted".into(), Profile::Balanced, None);
    let deps5 = w.deps(Some(&vlm5), None);
    let out5 = draft::draft(
        &deps5,
        &DraftRequest {
            content: "한빛 포털 구축 10월 진척 보고서".into(),
            max_tokens: Some(8_000),
            previous: Some(Box::new(out.draft.clone())),
            feedback: Some("더 간결하게".into()),
            revise: Some(ReviseScope::Whole),
            ..req.clone()
        },
    )
    .unwrap();
    assert_eq!(out5.sections, 5);
    let p2 = chat5.prompt(1);
    assert!(p2.contains("[수정 요청]") && p2.contains("더 간결하게"), "{p2}");
    assert!(p2.contains("[이 절의 이전 본문]") && p2.contains("1,200 MM"), "{p2}");
    let p3 = chat5.prompt(3);
    assert!(p3.contains("[수정 요청]") && !p3.contains("[이 절의 이전 본문]"), "{p3}");
}
