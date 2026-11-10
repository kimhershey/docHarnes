//! End-to-end pipeline tests against a scripted vision backend: a scanned
//! receipt (classify → OCR → extraction → validation → review flags →
//! mirror → knowledge re-index), a digital PDF (text-only extraction, no
//! OCR), rotation handling, manual types, review actions and promotion,
//! schema CRUD, form derivation and natural-language schemas.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde_json::json;

use crate::db::Db;
use crate::documents::convert::StubPdfPrinter;
use crate::documents::import::ImportDeps;
use crate::documents::render::{write_jpeg, RenderOptions};
use crate::documents::{
    repo as docs, DocumentCache, DocumentRole, DocumentStatus, NewDocument, NewPage, ReviewStatus, SourceKind,
};
use crate::knowledge::index::{index_pending, IndexDeps};
use crate::knowledge::tokenize::Analyzer;
use crate::paths::WorkspacePaths;
use crate::settings::{ProcessingSettings, Profile};

use super::repo::{self, ReviewFilter};
use super::review::{self, ReviewAction};
use super::run::{self, PipelineDeps};
use super::schema::{self, FieldDef, FieldKind, SchemaDef, SchemaDraft, SchemaSource};
use super::vlm::testing::StageChat;
use super::vlm::Vlm;
use super::{forms, FieldStatus, PipelineLevel, ProcessRequest, ReviewReason, SchemaChoice};

struct World {
    db: Db,
    paths: WorkspacePaths,
    _dir: tempfile::TempDir,
    chat: StageChat,
    settings: ProcessingSettings,
}

impl World {
    fn new() -> World {
        let dir = tempfile::tempdir().unwrap();
        let paths = WorkspacePaths::new(dir.path());
        for d in paths.all_dirs() {
            std::fs::create_dir_all(d).unwrap();
        }
        let db = Db::open_in_memory().unwrap();
        db.with(repo::ensure_builtins).unwrap();
        // The 0.1 business forms the fixtures exercise are hidden by default; the tests still cover them.
        db.with(repo::enable_legacy_builtins).unwrap();
        // No random audit sampling in tests (deterministic review queues).
        let settings = ProcessingSettings { audit_sample_percent: 0, ..ProcessingSettings::default() };
        World { db, paths, _dir: dir, chat: StageChat::new(), settings }
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

    fn deps<'a>(&'a self, cancel: Option<&'a AtomicBool>) -> PipelineDeps<'a> {
        PipelineDeps {
            import: self.import_deps(),
            chat: &self.chat,
            model: "scripted".into(),
            settings: self.settings.clone(),
            profile: Profile::Balanced,
            pii_masking: true,
            cancel,
            ctx: None,
        }
    }

    fn vlm(&self) -> Vlm<'_> {
        Vlm::new(&self.chat, "scripted".into(), Profile::Balanced, None)
    }

    /// Registers a document with `pages` (text per page; `None` = image-only page with a render).
    fn add_doc(&self, id: &str, name: &str, kind: SourceKind, pages: &[Option<&str>]) {
        let rel = format!("documents/{name}");
        let abs = self.paths.root.join(&rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        if kind == SourceKind::Image {
            let img = image::RgbImage::from_fn(600, 400, |x, y| {
                if (x / 50 + y / 50) % 2 == 0 {
                    image::Rgb([255, 255, 255])
                } else {
                    image::Rgb([30, 30, 30])
                }
            });
            img.save(&abs).unwrap();
        } else {
            std::fs::write(&abs, b"%PDF-1.4 stub").unwrap();
        }
        self.db
            .with(|c| {
                docs::insert_document(
                    c,
                    &NewDocument {
                        id: id.into(),
                        rel_path: rel.clone(),
                        original_name: name.into(),
                        source_kind: kind,
                        content_hash: format!("hash-{id}"),
                        size_bytes: 10,
                        file_modified_at: None,
                        role: DocumentRole::Work,
                        import_batch_id: None,
                    },
                )
            })
            .unwrap();
        let cache = DocumentCache::for_document(&self.paths, id);
        let mut new_pages = Vec::new();
        for (i, text) in pages.iter().enumerate() {
            let page_no = i as u32 + 1;
            let has_image = text.is_none() || kind == SourceKind::Image;
            let (w, h) = if has_image {
                // Image documents render from the source (600×400 landscape); PDF pages are portrait.
                let (iw, ih) = if kind == SourceKind::Image { (600, 400) } else { (600, 800) };
                let img = image::RgbImage::from_fn(iw, ih, |x, y| {
                    if (x / 40 + y / 40) % 2 == 0 {
                        image::Rgb([255, 255, 255])
                    } else {
                        image::Rgb([20, 20, 20])
                    }
                });
                write_jpeg(&img, &cache.page_file(page_no), 85).unwrap()
            } else {
                (0, 0)
            };
            new_pages.push(NewPage {
                page_no,
                width_px: w,
                height_px: h,
                text: text.map(str::to_string),
                dpi: Some(200),
                is_blank: has_image.then_some(false),
                ..Default::default()
            });
        }
        self.db.with_tx(|tx| docs::replace_pages(tx, id, &new_pages, Some(200), false, None)).unwrap();
        self.db.with(|c| docs::set_status(c, id, DocumentStatus::Rendered, None)).unwrap();
    }

    fn doc(&self, id: &str) -> crate::documents::DocumentRecord {
        self.db.with(|c| docs::get_document(c, id)).unwrap().unwrap()
    }

    fn pages(&self, id: &str) -> Vec<crate::documents::PageRecord> {
        self.db.with(|c| docs::list_pages(c, id, true)).unwrap()
    }

    fn fields(&self, id: &str) -> Vec<super::FieldRecord> {
        self.db.with(|c| repo::latest_fields(c, id)).unwrap()
    }
}

fn sv(v: &str, e: &str, c: u8) -> serde_json::Value {
    json!({ "value": v, "evidence": e, "confidence": c })
}

const RECEIPT_OCR: &str = "# 스타벅스 역삼점\n사업자등록번호 220-81-62517\n2026-10-14 12:31\n<table><tr><th>품명</th><th>수량</th><th>금액</th></tr><tr><td>아메리카노</td><td>2</td><td>9,000</td></tr><tr><td>쿠키</td><td>1</td><td>3,500</td></tr></table>\n공급가액 11,364\n부가세 1,136\n합계 12,500\n카드 승인 12345678\n고객 주민번호 900101-1234567";

fn receipt_kie() -> serde_json::Value {
    json!({
        "store_name": sv("스타벅스 역삼점", "스타벅스 역삼점", 95),
        "brn": sv("220-81-62517", "사업자등록번호 220-81-62517", 92),
        "merchant_no": { "value": null, "evidence": null, "confidence": 0 },
        "address": { "value": null, "evidence": null, "confidence": 0 },
        "phone": { "value": null, "evidence": null, "confidence": 0 },
        "datetime": sv("2026-10-14 12:31", "2026-10-14 12:31", 90),
        "items": [
            { "name": "아메리카노", "quantity": "2", "unit_price": "4,500", "amount": "9,000" },
            { "name": "쿠키", "quantity": "1", "unit_price": "3,500", "amount": "3,500" }
        ],
        "supply_amount": sv("11,364", "공급가액 11,364", 85),
        "vat": sv("1,136", "부가세 1,136", 85),
        "service_charge": { "value": null, "evidence": null, "confidence": 0 },
        "exempt_amount": { "value": null, "evidence": null, "confidence": 0 },
        "total": sv("12,500", "합계 12,500", 93),
        "payment_method": sv("카드", "카드 승인", 70),
        "approval_type": { "value": null, "evidence": null, "confidence": 0 },
        "installment": { "value": null, "evidence": null, "confidence": 0 },
        "card_last4": { "value": null, "evidence": null, "confidence": 0 },
        "approval_no": sv("12345678", "카드 승인 12345678", 60),
        "cash_receipt_no": { "value": null, "evidence": null, "confidence": 0 },
        "cash_receipt_id": { "value": null, "evidence": null, "confidence": 0 }
    })
}

fn classify_reply(doc_type: &str, rotation: u16) -> serde_json::Value {
    json!({ "doc_type": doc_type, "rotation": rotation, "is_blank": false, "language": "ko" })
}

#[test]
fn scanned_receipt_runs_end_to_end() {
    let w = World::new();
    w.add_doc("r1", "receipt.pdf", SourceKind::Pdf, &[None]);
    w.chat.push("classify", classify_reply("receipt", 0));
    w.chat.push_ocr(RECEIPT_OCR);
    w.chat.push("kie_receipt", receipt_kie());
    let deps = w.deps(None);
    let req = ProcessRequest { document_ids: vec!["r1".into()], ..Default::default() };
    let mut stages: Vec<String> = Vec::new();
    let report = run::process(&deps, &req, &mut |p| stages.push(p.stage.clone())).unwrap();
    assert_eq!(report.processed, vec!["r1".to_string()], "{report:?}");
    assert_eq!((report.classified, report.ocr_pages, report.extracted, report.review_pending), (1, 1, 1, 1));
    assert!(
        stages.contains(&"classify".to_string())
            && stages.contains(&"ocr".to_string())
            && stages.contains(&"extract".to_string())
    );
    assert_eq!(w.chat.calls("classify"), 1);
    assert_eq!(w.chat.calls("kie_receipt"), 1);
    assert_eq!(w.chat.image_calls(), 3, "classify + ocr + kie each saw the page image");

    let doc = w.doc("r1");
    assert_eq!(doc.doc_type.as_deref(), Some("receipt"));
    assert_eq!(doc.doc_type_source.as_deref(), Some("auto"));
    assert_eq!(doc.language.as_deref(), Some("ko"));
    assert!(doc.classified_at.is_some() && doc.ocr_at.is_some() && doc.extracted_at.is_some());
    assert_eq!(doc.status, DocumentStatus::Extracted);
    assert_eq!(doc.review_status, ReviewStatus::Pending);
    assert_eq!(doc.vendor.as_deref(), Some("스타벅스 역삼점"));
    assert_eq!(doc.doc_date.as_deref(), Some("2026-10-14"));
    assert_eq!(doc.total_amount, Some(12_500.0));
    assert!(doc.updated_at > doc.imported_at);

    let page = &w.pages("r1")[0];
    assert_eq!(page.text_source.as_deref(), Some("ocr"));
    assert_eq!(page.page_doc_type.as_deref(), Some("receipt"));
    let text = page.text.as_deref().unwrap();
    assert!(text.contains("900101-1******") && !text.contains("900101-1234567"), "PII masked in OCR text: {text}");
    assert!(text.contains("<table>"));

    let fields = w.fields("r1");
    let f = |p: &str| fields.iter().find(|f| f.path == p).unwrap();
    assert_eq!(f("brn").normalized.as_deref(), Some("220-81-62517"));
    assert!(f("brn").validation.is_empty() && f("brn").review_reason.is_none());
    assert_eq!(f("total").normalized.as_deref(), Some("12500"));
    assert_eq!(f("items[0].amount").normalized.as_deref(), Some("9000"));
    assert_eq!(f("approval_no").review_reason, Some(ReviewReason::LowConfidence));
    assert_eq!(f("approval_no").status, FieldStatus::Auto);
    assert!(f("store_name").confidence >= 0.95);
    assert_eq!(f("merchant_no").value, None);

    let runs = w.db.with(|c| repo::list_runs(c, "r1")).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "done");
    assert_eq!(runs[0].schema_id.as_deref(), Some("builtin-receipt"));
    assert!(runs[0].prompt_tokens > 0 && runs[0].field_count > 20 && runs[0].review_count >= 1);
    let queue = w.db.with(|c| repo::review_queue(c, &ReviewFilter::default())).unwrap();
    assert!(queue.iter().any(|f| f.path == "approval_no"));

    // Mirror on disk.
    let dir = w.paths.extractions_dir().join("r1").join(&runs[0].id);
    assert!(dir.join("result.json").is_file() && dir.join("p0001.md").is_file());
    assert_eq!(std::fs::read_to_string(w.paths.extractions_dir().join("r1").join("LATEST")).unwrap(), runs[0].id);
    let result: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("result.json")).unwrap()).unwrap();
    assert_eq!(result["schema"]["doc_type"], "receipt");

    // The knowledge indexer picks the OCR text up through updated_at.
    let analyzer = Analyzer::simple();
    let idx = IndexDeps { db: &w.db, analyzer: &analyzer, options: Default::default() };
    let report = index_pending(&idx, None, false, None, &mut |_| {}).unwrap();
    assert_eq!(report.indexed, vec!["r1".to_string()]);
    assert!(report.chunks >= 1);
    let status = w.db.with(repo::pipeline_status).unwrap();
    assert_eq!((status.documents, status.extracted, status.review_pending), (1, 1, 1));
    assert!(status.review_fields >= 1);

    // A second run without `force` skips classification and OCR but extracts again.
    w.chat.push("kie_receipt", receipt_kie());
    let report = run::process(&deps, &req, &mut |_| {}).unwrap();
    assert_eq!((report.classified, report.ocr_pages, report.extracted), (0, 0, 1));
    assert_eq!(w.chat.calls("classify"), 1);
    assert_eq!(w.db.with(|c| repo::list_runs(c, "r1")).unwrap().len(), 2);
    let used = w.db.with(|c| repo::get_schema(c, "builtin-receipt")).unwrap().unwrap().use_count;
    assert_eq!(used, 2);
}

#[test]
fn manual_text_is_kept_unless_ocr_is_forced() {
    let w = World::new();
    w.add_doc("m1", "memo.pdf", SourceKind::Pdf, &[None]);
    w.db.with(|c| docs::set_page_text(c, "m1", 1, "손으로 고친 영수증 텍스트")).unwrap();
    w.chat.push("classify", classify_reply("receipt", 0));
    let deps = w.deps(None);
    let req = ProcessRequest { document_ids: vec!["m1".into()], level: PipelineLevel::Ocr, ..Default::default() };
    let report = run::process(&deps, &req, &mut |_| {}).unwrap();
    assert_eq!(report.ocr_pages, 0, "manual text is not overwritten: {report:?}");
    let page = &w.pages("m1")[0];
    assert_eq!(page.text.as_deref(), Some("손으로 고친 영수증 텍스트"));
    assert_eq!(page.text_source.as_deref(), Some("manual"));
    // A forced re-run replaces it with OCR output.
    w.chat.push_ocr(RECEIPT_OCR);
    let forced = ProcessRequest {
        document_ids: vec!["m1".into()],
        level: PipelineLevel::Ocr,
        force: true,
        ..Default::default()
    };
    let report = run::process(&deps, &forced, &mut |_| {}).unwrap();
    assert_eq!(report.ocr_pages, 1, "{report:?}");
    assert_eq!(w.pages("m1")[0].text_source.as_deref(), Some("ocr"));
}

#[test]
fn digital_pdf_skips_ocr_and_extracts_from_text() {
    let w = World::new();
    let text = "세금계산서 (과세) 작성일자 2026.03.02\n공급자 상호 한빛상사\n사업자등록번호 220-81-62517\n공급받는자 DocuLens\n공급가액 1,000,000 세액 100,000 합계 1,100,000";
    w.add_doc("t1", "invoice.pdf", SourceKind::Pdf, &[Some(text), Some("2쪽 비고")]);
    w.chat.push("classify", classify_reply("tax_invoice", 0));
    w.chat.push(
        "kie_tax_invoice",
        json!({
            "issue_date": sv("2026.03.02", "2026.03.02", 90),
            "tax_type": sv("과세", "과세", 90),
            "supplier": { "brn": sv("220-81-62517", "사업자등록번호 220-81-62517", 95), "name": sv("한빛상사", "상호 한빛상사", 95) },
            "buyer": { "brn": { "value": null, "evidence": null, "confidence": 0 }, "name": sv("DocuLens", "DocuLens", 80) },
            "supply_total": sv("1,000,000", "공급가액 1,000,000", 90),
            "tax_total": sv("100,000", "세액 100,000", 90),
            "grand_total": sv("1,100,000", "합계 1,100,000", 90),
            "items": []
        }),
    );
    let deps = w.deps(None);
    let req = ProcessRequest { document_ids: vec!["t1".into()], ..Default::default() };
    let report = run::process(&deps, &req, &mut |_| {}).unwrap();
    assert_eq!(report.failed, Vec::<(String, String)>::new());
    assert_eq!(report.ocr_pages, 0, "text-layer pages are not OCRed by default");
    assert_eq!(w.chat.image_calls(), 0, "no renders → text-only calls");
    assert_eq!(w.chat.calls("kie_tax_invoice"), 2, "both pages are extracted");
    let doc = w.doc("t1");
    assert_eq!(doc.doc_type.as_deref(), Some("tax_invoice"));
    assert!(w.pages("t1")[0].text_source.as_deref() == Some("layer"));
    let fields = w.fields("t1");
    let f = |p: &str| fields.iter().find(|f| f.path == p).unwrap();
    assert_eq!(f("supplier.brn").normalized.as_deref(), Some("220-81-62517"));
    assert_eq!(f("grand_total").normalized.as_deref(), Some("1100000"));
    assert!(f("grand_total").validation.is_empty(), "{:?}", f("grand_total").validation);
    // The buyer's registration number is required and missing → queued, not promoted.
    assert_eq!(f("buyer.brn").review_reason, Some(ReviewReason::RequiredMissing));
    assert_eq!(doc.status, DocumentStatus::Extracted);
    assert_eq!(doc.review_status, ReviewStatus::Pending);
    let queue = w
        .db
        .with(|c| repo::review_queue(c, &ReviewFilter { doc_type: Some("tax_invoice".into()), ..Default::default() }))
        .unwrap();
    assert_eq!(queue.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["buyer.brn"]);
    // Rejecting it (there is no buyer number on this invoice) resolves the queue; the doc still
    // lacks a required value, so it stays `extracted`.
    review::apply(&w.db, &[queue[0].id], &ReviewAction::Reject, None).unwrap();
    assert_eq!(w.doc("t1").review_status, ReviewStatus::Done);
    assert_eq!(w.doc("t1").status, DocumentStatus::Extracted);
    // Typing the number promotes the document.
    review::apply(&w.db, &[queue[0].id], &ReviewAction::Correct { value: "123-45-67890".into() }, None).unwrap();
    assert_eq!(w.doc("t1").status, DocumentStatus::Reviewed, "every required field resolved with a value");
}

#[test]
fn rotation_is_applied_and_manual_type_wins() {
    let w = World::new();
    w.add_doc("p1", "photo.jpg", SourceKind::Image, &[None]);
    let before = w.pages("p1")[0].clone();
    assert!(before.width_px > before.height_px, "landscape render");
    w.chat.push("classify", classify_reply("contract", 90));
    w.chat.push("classify", classify_reply("contract", 0));
    let deps = w.deps(None);
    let req = ProcessRequest { document_ids: vec!["p1".into()], level: PipelineLevel::Classify, ..Default::default() };
    run::process(&deps, &req, &mut |_| {}).unwrap();
    let after = w.pages("p1")[0].clone();
    assert_eq!(after.rotation, 90);
    assert!(after.width_px < after.height_px, "re-rendered rotated: {}x{}", after.width_px, after.height_px);
    assert_eq!(after.page_rotation, Some(90));
    assert_eq!(w.chat.calls("classify"), 2, "classified again after the rotation");
    assert_eq!(w.doc("p1").doc_type.as_deref(), Some("contract"));

    // Manual choice survives a forced re-classification.
    w.db.with(|c| docs::set_doc_type(c, "p1", Some("receipt"), Some("manual"))).unwrap();
    w.chat.push("classify", classify_reply("contract", 0));
    let req = ProcessRequest {
        document_ids: vec!["p1".into()],
        level: PipelineLevel::Classify,
        force: true,
        ..Default::default()
    };
    run::process(&deps, &req, &mut |_| {}).unwrap();
    let doc = w.doc("p1");
    assert_eq!(doc.doc_type.as_deref(), Some("receipt"));
    assert_eq!(doc.doc_type_source.as_deref(), Some("manual"));
}

#[test]
fn blank_verdict_needs_the_pixel_heuristic() {
    // A garbage model answering is_blank=true must not blank out a page the renderer saw ink on.
    let w = World::new();
    w.add_doc("b1", "receipt.pdf", SourceKind::Pdf, &[None]);
    w.chat.push("classify", json!({ "doc_type": "receipt", "rotation": 0, "is_blank": true, "language": "ko" }));
    w.chat.push_ocr(RECEIPT_OCR);
    w.chat.push("kie_receipt", receipt_kie());
    let deps = w.deps(None);
    let report =
        run::process(&deps, &ProcessRequest { document_ids: vec!["b1".into()], ..Default::default() }, &mut |_| {})
            .unwrap();
    assert_eq!(report.failed, Vec::<(String, String)>::new());
    assert_eq!((report.ocr_pages, report.extracted), (1, 1));
    assert_eq!(w.pages("b1")[0].is_blank, Some(false));
    // A page the renderer could not judge takes the model's word and is skipped gracefully.
    w.add_doc("b2", "scan.pdf", SourceKind::Pdf, &[None]);
    w.db.with(|c| c.execute("UPDATE pages SET is_blank = NULL WHERE document_id = 'b2'", [])).unwrap();
    w.chat.push("classify", json!({ "doc_type": "receipt", "rotation": 0, "is_blank": true, "language": "ko" }));
    let report =
        run::process(&deps, &ProcessRequest { document_ids: vec!["b2".into()], ..Default::default() }, &mut |_| {})
            .unwrap();
    assert_eq!(report.failed, Vec::<(String, String)>::new());
    assert_eq!((report.ocr_pages, report.extracted), (0, 0));
    assert!(w.doc("b2").pipeline_error.is_none());
}

#[test]
fn cancellation_and_failures_are_isolated() {
    let w = World::new();
    w.add_doc("a", "a.pdf", SourceKind::Pdf, &[None]);
    w.add_doc("b", "b.pdf", SourceKind::Pdf, &[None]);
    // No scripted classify reply → the backend errors → recorded per document.
    let deps = w.deps(None);
    let req = ProcessRequest {
        document_ids: vec!["a".into(), "b".into(), "missing".into()],
        level: PipelineLevel::Classify,
        ..Default::default()
    };
    let report = run::process(&deps, &req, &mut |_| {}).unwrap();
    assert_eq!(report.failed.len(), 2);
    assert_eq!(report.skipped, vec!["missing".to_string()]);
    assert!(w.doc("a").pipeline_error.as_deref().unwrap().contains("no scripted reply"));
    let cancel = AtomicBool::new(true);
    let deps = w.deps(Some(&cancel));
    let report = run::process(&deps, &req, &mut |_| {}).unwrap();
    assert!(report.cancelled && report.processed.is_empty());
}

#[test]
fn review_actions_history_and_promotion() {
    let w = World::new();
    w.add_doc("r1", "receipt.pdf", SourceKind::Pdf, &[None]);
    w.chat.push("classify", classify_reply("receipt", 0));
    w.chat.push_ocr(RECEIPT_OCR);
    w.chat.push("kie_receipt", receipt_kie());
    let deps = w.deps(None);
    run::process(&deps, &ProcessRequest { document_ids: vec!["r1".into()], ..Default::default() }, &mut |_| {})
        .unwrap();
    let fields = w.fields("r1");
    let approval = fields.iter().find(|f| f.path == "approval_no").unwrap();
    let total = fields.iter().find(|f| f.path == "total").unwrap();
    let pending: Vec<i64> = fields.iter().filter(|f| f.needs_review()).map(|f| f.id).collect();
    assert!(pending.contains(&approval.id));

    // Correct → history row, confidence 1.0, status corrected.
    let out = review::apply(&w.db, &[approval.id], &ReviewAction::Correct { value: "87654321".into() }, Some("typo"))
        .unwrap();
    assert_eq!(out[0].status, FieldStatus::Corrected);
    assert_eq!(out[0].value.as_deref(), Some("87654321"));
    assert_eq!(out[0].confidence, 1.0);
    let history = w.db.with(|c| repo::field_history(c, "r1")).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        (history[0].old_value.as_deref(), history[0].new_value.as_deref()),
        (Some("12345678"), Some("87654321"))
    );
    assert_eq!(history[0].note.as_deref(), Some("typo"));

    // Hold keeps the document pending; approve the rest → promoted to reviewed.
    review::apply(&w.db, &[total.id], &ReviewAction::Hold, None).unwrap();
    assert_eq!(w.doc("r1").review_status, ReviewStatus::Pending);
    let n = review::approve_document(&w.db, "r1", None).unwrap();
    assert!(n >= 1);
    let doc = w.doc("r1");
    assert_eq!(doc.review_status, ReviewStatus::Done);
    assert_eq!(doc.status, DocumentStatus::Reviewed);
    assert!(w.db.with(|c| repo::review_queue(c, &ReviewFilter::default())).unwrap().is_empty());

    // Restore the original value from history → corrected back, still resolved.
    let before = w.db.with(|c| repo::field_history(c, "r1")).unwrap().len();
    assert!(before > 2, "bulk approvals are journaled too");
    let restored = review::restore(&w.db, history[0].id).unwrap();
    assert_eq!(restored.value.as_deref(), Some("12345678"));
    assert_eq!(restored.status, FieldStatus::Corrected);
    let after = w.db.with(|c| repo::field_history(c, "r1")).unwrap();
    assert_eq!(after.len(), before + 1);
    assert_eq!((after[0].new_value.as_deref(), after[0].note.as_deref()), (Some("12345678"), Some("restored")));

    // Reset re-opens the queue and demotes the document.
    review::apply(&w.db, &[restored.id], &ReviewAction::Reset, None).unwrap();
    let doc = w.doc("r1");
    assert_eq!(doc.review_status, ReviewStatus::Pending);
    assert_eq!(doc.status, DocumentStatus::Extracted);

    // Reject a required field → no promotion even after approving everything else.
    let store = w.fields("r1").into_iter().find(|f| f.path == "store_name").unwrap();
    review::apply(&w.db, &[store.id], &ReviewAction::Reject, None).unwrap();
    review::approve_document(&w.db, "r1", None).unwrap();
    assert_eq!(w.doc("r1").status, DocumentStatus::Extracted, "required field rejected → not reviewed");

    // Revalidate after a correction that breaks the sum rule.
    let supply = w.fields("r1").into_iter().find(|f| f.path == "supply_amount").unwrap();
    review::apply(&w.db, &[supply.id], &ReviewAction::Correct { value: "5,000".into() }, None).unwrap();
    let fields = review::revalidate(&w.db, "r1", 0.8, 0).unwrap();
    let total = fields.iter().find(|f| f.path == "total").unwrap();
    assert!(
        total.validation.iter().any(|i| i.rule == schema::RULE_RECEIPT_TOTAL && i.level == "fail"),
        "{:?}",
        total.validation
    );
    assert_eq!(total.review_reason, Some(ReviewReason::ValidationFailed));
    assert!(review::apply(&w.db, &[supply.id], &ReviewAction::Correct { value: "  ".into() }, None).is_err());
}

#[test]
fn schema_crud_defaults_versions_and_doc_types() {
    let w = World::new();
    let all = w.db.with(repo::list_schemas).unwrap();
    assert_eq!(all.len(), 19, "13 software forms + 6 hidden legacy forms");
    assert!(all.iter().all(|s| s.source == SchemaSource::Builtin && s.is_default && s.enabled));
    assert_eq!(w.db.with(repo::ensure_builtins).unwrap(), 0, "idempotent");
    let types = w.db.with(repo::doc_types).unwrap();
    assert_eq!(types.len(), 19, "legacy forms re-enabled by the fixture");
    assert!(types.iter().all(|t| t.builtin));

    // A user schema for a new type becomes its default; a user schema for an existing type beats the sample.
    let draft = SchemaDraft {
        name: "견적서".into(),
        doc_type: "quotation".into(),
        source: SchemaSource::User,
        definition: SchemaDef::new(vec![FieldDef::new("total", "합계", "Total", FieldKind::Amount).required()], &[]),
        ..Default::default()
    };
    let q = w.db.with(|c| repo::save_schema(c, &draft)).unwrap();
    assert!(q.is_default && q.version == 1);
    let types = w.db.with(repo::doc_types).unwrap();
    assert_eq!(types[0].slug, "quotation", "user types come first: {types:?}");
    let my_receipt = SchemaDraft {
        name: "우리 영수증".into(),
        doc_type: "receipt".into(),
        source: SchemaSource::User,
        definition: SchemaDef::new(vec![FieldDef::new("total", "합계", "Total", FieldKind::Amount)], &[]),
        ..Default::default()
    };
    let mine = w.db.with(|c| repo::save_schema(c, &my_receipt)).unwrap();
    assert!(mine.is_default);
    assert!(!w.db.with(|c| repo::get_schema(c, "builtin-receipt")).unwrap().unwrap().is_default);
    assert_eq!(w.db.with(|c| repo::schema_for_doc_type(c, "receipt")).unwrap().unwrap().id, mine.id);
    w.db.with(|c| repo::set_default_schema(c, "builtin-receipt")).unwrap();
    assert_eq!(w.db.with(|c| repo::schema_for_doc_type(c, "receipt")).unwrap().unwrap().id, "builtin-receipt");

    // Editing stores a version row; invalid drafts and builtin deletion are refused.
    let mut edit = SchemaDraft {
        id: Some(q.id.clone()),
        name: "견적서".into(),
        doc_type: "quotation".into(),
        definition: SchemaDef::new(
            vec![
                FieldDef::new("total", "합계", "Total", FieldKind::Amount),
                FieldDef::new("vendor", "거래처", "Vendor", FieldKind::String),
            ],
            &[],
        ),
        note: Some("거래처 추가".into()),
        ..Default::default()
    };
    let q2 = w.db.with(|c| repo::save_schema(c, &edit)).unwrap();
    assert_eq!(q2.version, 2);
    let versions = w.db.with(|c| repo::schema_versions(c, &q.id)).unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].0, 1);
    assert_eq!(versions[0].2, None, "version 1 was saved without a note");
    assert_eq!(q2.note.as_deref(), Some("거래처 추가"), "the note describes the version it created");
    let q3 =
        w.db.with(|c| repo::save_schema(c, &SchemaDraft { note: Some("세 번째".into()), ..edit.clone() })).unwrap();
    assert_eq!(q3.version, 3);
    let versions = w.db.with(|c| repo::schema_versions(c, &q.id)).unwrap();
    assert_eq!((versions[0].0, versions[0].2.as_deref()), (2, Some("거래처 추가")));
    edit.definition.fields[1].key = "Bad Key".into();
    let err = w.db.with(|c| repo::save_schema(c, &edit)).unwrap_err();
    assert!(err.to_string().contains("schema_invalid"));
    assert!(w
        .db
        .with(|c| repo::delete_schema(c, "builtin-receipt"))
        .unwrap_err()
        .to_string()
        .contains("schema_builtin"));
    assert!(w.db.with(|c| repo::delete_schema(c, &mine.id)).unwrap());
    assert!(
        w.db.with(|c| repo::get_schema(c, "builtin-receipt")).unwrap().unwrap().is_default,
        "default falls back to the sample"
    );
    w.db.with(|c| repo::set_schema_enabled(c, &q.id, false)).unwrap();
    assert!(!w.db.with(repo::doc_types).unwrap().iter().any(|t| t.slug == "quotation"));

    // Document schema override + explicit choice are honoured by the resolver.
    w.add_doc("d", "d.pdf", SourceKind::Pdf, &[Some("x")]);
    let doc = w.doc("d");
    assert_eq!(run::resolve_schema(&w.db, &doc, &SchemaChoice::Auto).unwrap().doc_type, "general");
    w.db.with(|c| repo::set_schema_override(c, "d", Some(&q.id))).unwrap();
    assert_eq!(run::resolve_schema(&w.db, &w.doc("d"), &SchemaChoice::Auto).unwrap().id, q.id);
    assert_eq!(
        run::resolve_schema(&w.db, &doc, &SchemaChoice::Id { id: "builtin-contract".into() }).unwrap().doc_type,
        "contract"
    );
    assert!(w.db.with(|c| repo::set_schema_override(c, "d", Some("nope"))).is_err());
}

#[test]
fn forms_register_from_structure_and_from_the_model() {
    let w = World::new();
    // HWPX form derived from its tables (no model involved).
    let hwpx = w.paths.root.join("documents/quote-form.hwpx");
    std::fs::create_dir_all(hwpx.parent().unwrap()).unwrap();
    crate::documents::convert::hwpx::write_sample_hwpx(&hwpx, 1, crate::documents::fixtures::TINY_PNG).unwrap();
    w.db.with(|c| {
        docs::insert_document(
            c,
            &NewDocument {
                id: "f1".into(),
                rel_path: "documents/quote-form.hwpx".into(),
                original_name: "quote-form.hwpx".into(),
                source_kind: SourceKind::Hwpx,
                content_hash: "h".into(),
                size_bytes: 1,
                file_modified_at: None,
                role: DocumentRole::Work,
                import_batch_id: None,
            },
        )
    })
    .unwrap();
    let deps = w.import_deps();
    let draft = forms::derive(&deps, None, &w.doc("f1"), Some("견적서")).unwrap();
    assert_eq!(draft.source, SchemaSource::Form);
    assert_eq!(draft.form_document_id.as_deref(), Some("f1"));
    assert_eq!(draft.doc_type, "estimate");
    assert!(!draft.definition.fields.is_empty());
    let saved = forms::register(&w.db, &draft).unwrap();
    assert_eq!(saved.source, SchemaSource::Form);
    let doc = w.doc("f1");
    assert_eq!(doc.role, DocumentRole::Form);
    assert_eq!((doc.doc_type.as_deref(), doc.doc_type_source.as_deref()), (Some("estimate"), Some("form")));
    assert!(w.db.with(repo::doc_types).unwrap().iter().any(|t| t.slug == "estimate" && !t.builtin));

    // Image form through the scripted VLM.
    w.add_doc("f2", "application.jpg", SourceKind::Image, &[None]);
    w.chat.push(
        "form_fields",
        json!({
            "name": "휴가 신청서", "name_en": "Leave request", "doc_type": "leave_request", "description_ko": "휴가", "description_en": "leave",
            "fields": [
                {"key": "name", "label_ko": "성명", "label_en": "Name", "type": "string", "required": true, "regex_hint": null, "example": null, "options": [], "columns": []},
                {"key": "period", "label_ko": "휴가 기간", "label_en": "Period", "type": "date", "required": true, "regex_hint": null, "example": null, "options": [], "columns": []},
                {"key": "kind", "label_ko": "종류", "label_en": "Kind", "type": "enum", "required": false, "regex_hint": null, "example": null, "options": ["연차", "병가"], "columns": []}
            ]
        }),
    );
    let vlm = w.vlm();
    let draft = forms::derive(&deps, Some(&vlm), &w.doc("f2"), None).unwrap();
    assert_eq!(draft.name, "application", "file stem is the fallback name");
    assert_eq!(draft.definition.fields.len(), 3);
    assert_eq!(draft.definition.fields[2].options, vec!["연차", "병가"]);
    assert_eq!(w.chat.image_calls(), 1);

    // Natural-language creation and modification.
    w.chat.push(
        "nl_schema",
        json!({
            "name": "발주서", "name_en": "Purchase order", "doc_type": "purchase_order", "description_ko": "발주", "description_en": "PO",
            "fields": [
                {"key": "po_no", "label_ko": "발주번호", "label_en": "PO no.", "type": "string", "required": true, "regex_hint": "^PO-[0-9]+$", "example": "PO-1001", "options": [], "columns": []},
                {"key": "items", "label_ko": "품목", "label_en": "Items", "type": "table", "required": false, "regex_hint": null, "example": null, "options": [],
                 "columns": [{"key": "name", "label_ko": "품명", "label_en": "Item", "type": "string"}, {"key": "qty", "label_ko": "수량", "label_en": "Qty", "type": "integer"}]}
            ]
        }),
    );
    let d = forms::nl_schema(&vlm, "발주번호와 품목(품명·수량)이 있는 발주서", None).unwrap();
    assert_eq!((d.doc_type.as_str(), d.source), ("purchase_order", SchemaSource::Nl));
    let po = w.db.with(|c| repo::save_schema(c, &d)).unwrap();
    w.chat.push(
        "nl_schema",
        json!({
            "name": "발주서", "name_en": "Purchase order", "doc_type": "ignored_slug", "description_ko": "발주", "description_en": "PO",
            "fields": [
                {"key": "po_no", "label_ko": "발주번호", "label_en": "PO no.", "type": "string", "required": true, "regex_hint": null, "example": null, "options": [], "columns": []},
                {"key": "due_date", "label_ko": "납기", "label_en": "Due", "type": "date", "required": false, "regex_hint": null, "example": null, "options": [], "columns": []}
            ]
        }),
    );
    let d2 = forms::nl_schema(&vlm, "납기 필드를 추가해", Some(&po)).unwrap();
    assert_eq!(d2.id.as_deref(), Some(po.id.as_str()));
    assert_eq!(d2.doc_type, "purchase_order", "modification keeps the type");
    let po2 = w.db.with(|c| repo::save_schema(c, &d2)).unwrap();
    assert_eq!(po2.version, 2);
    assert!(po2.definition.field("due_date").is_some());
    assert!(forms::nl_schema(&vlm, "   ", None).is_err());
}

#[test]
fn split_registers_parts_and_archives_the_original() {
    let w = World::new();
    let path = w.paths.root.join("documents/batch.pdf");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    crate::documents::convert::write_blank_pdf(&path, 4, &["a".into(), "b".into(), "c".into(), "d".into()]).unwrap();
    w.db.with(|c| {
        docs::insert_document(
            c,
            &NewDocument {
                id: "s1".into(),
                rel_path: "documents/batch.pdf".into(),
                original_name: "batch.pdf".into(),
                source_kind: SourceKind::Pdf,
                content_hash: "h".into(),
                size_bytes: 1,
                file_modified_at: None,
                role: DocumentRole::Work,
                import_batch_id: None,
            },
        )
    })
    .unwrap();
    w.db.with(|c| docs::set_doc_type(c, "s1", Some("receipt"), Some("manual"))).unwrap();
    let deps = w.import_deps();
    let ids = super::split::split_document(&deps, &w.doc("s1"), &[3], None).unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(w.doc("s1").status, DocumentStatus::Archived);
    for id in &ids {
        let d = w.doc(id);
        assert!(d.rel_path.starts_with("documents/batch_part"));
        assert_eq!(d.doc_type.as_deref(), Some("receipt"));
        assert!(Path::new(&w.paths.root).join(&d.rel_path).is_file());
    }
    let merged =
        super::split::merge_documents(&deps, &[w.doc(&ids[0]), w.doc(&ids[1])], Some("다시 합침"), None).unwrap();
    let m = w.doc(&merged);
    assert_eq!(m.rel_path, "documents/다시 합침.pdf");
    assert_eq!(lopdf::Document::load(w.paths.root.join(&m.rel_path)).unwrap().get_pages().len(), 4);
}

/// Phase-end review gaps: nothing unmasked in the run result / mirror, the
/// precision pass samples with its own temperature, a changed file resets the
/// pipeline state (manual type kept), revalidation is idempotent, corrections
/// on enum / masked fields, merge names are validated, one default per type.
#[test]
fn review_gaps_pii_precision_reset_and_defaults() {
    let w = World::new();
    w.add_doc("r1", "receipt.pdf", SourceKind::Pdf, &[None]);
    w.chat.push("classify", classify_reply("receipt", 0));
    w.chat.push_ocr(RECEIPT_OCR);
    let mut kie = receipt_kie();
    kie["cash_receipt_id"] = sv("010-1234-5678", "식별번호 010-1234-5678", 88);
    kie["items"][0]["name"] = json!("홍길동 850315-2345678 아메리카노");
    kie["store_name"] = sv("스타벅스 역삼점", "담당 900101-1234567 스타벅스 역삼점", 95);
    w.chat.push("kie_receipt", kie.clone());
    w.chat.push("kie_receipt", kie);
    let deps = w.deps(None);
    let req = ProcessRequest { document_ids: vec!["r1".into()], precise: Some(true), ..Default::default() };
    let report = run::process(&deps, &req, &mut |_| {}).unwrap();
    assert_eq!(report.extracted, 1, "{report:?}");

    // Precision mode: two KIE samples, the second with its own temperature / seed.
    let kie_reqs: Vec<_> = w
        .chat
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| StageChat::stage_of(r).as_deref() == Some("kie_receipt"))
        .cloned()
        .collect();
    assert_eq!(kie_reqs.len(), 2);
    assert_eq!((kie_reqs[0].temperature, kie_reqs[0].seed), (Some(0.0), Some(7)));
    assert_eq!((kie_reqs[1].temperature, kie_reqs[1].seed), (Some(0.3), Some(11)));
    assert_eq!(kie_reqs[0].messages, kie_reqs[1].messages, "same prompt, different sampling");

    // No full resident / phone number anywhere: fields, run result, mirror files.
    let runs = w.db.with(|c| repo::list_runs(c, "r1")).unwrap();
    assert!(runs[0].precise);
    let leaks = ["850315-2345678", "900101-1234567", "010-1234-5678"];
    let has_leak = |s: &str| leaks.iter().any(|l| s.contains(l));
    for f in w.fields("r1") {
        for v in [&f.value, &f.evidence, &f.normalized].into_iter().flatten() {
            assert!(!has_leak(v), "{}: {v}", f.path);
        }
    }
    let raw = w.db.with(|c| repo::run_result(c, &runs[0].id)).unwrap().unwrap().to_string();
    assert!(!has_leak(&raw), "{raw}");
    assert!(raw.contains("850315-2******") && raw.contains("***-****-5678"), "{raw}");
    let dir = w.paths.extractions_dir().join("r1").join(&runs[0].id);
    for name in ["result.json", "p0001.md"] {
        let s = std::fs::read_to_string(dir.join(name)).unwrap();
        assert!(!has_leak(&s), "{name}: {s}");
    }
    let f = |p: &str| w.fields("r1").into_iter().find(|f| f.path == p).unwrap();
    assert_eq!(f("cash_receipt_id").value.as_deref(), Some("***-****-5678"));
    assert_eq!(f("items[0].name").value.as_deref(), Some("홍길동 850315-2****** 아메리카노"));

    // Revalidation is idempotent and keeps the stored scores.
    let first = review::revalidate(&w.db, "r1", 0.8, 0).unwrap();
    let second = review::revalidate(&w.db, "r1", 0.8, 0).unwrap();
    let snap = |v: &[super::FieldRecord]| -> Vec<(String, f64, Option<ReviewReason>)> {
        v.iter().map(|f| (f.path.clone(), f.confidence, f.review_reason)).collect()
    };
    assert_eq!(snap(&first), snap(&second));
    let before: Vec<(String, f64)> = w.fields("r1").iter().map(|f| (f.path.clone(), f.confidence)).collect();
    assert_eq!(snap(&first).iter().map(|(p, c, _)| (p.clone(), *c)).collect::<Vec<_>>(), before);

    // Corrections honour the schema: an enum value outside its options is flagged,
    // a masked (account) field never stores the full number.
    let pm = f("payment_method");
    let out = review::apply(&w.db, &[pm.id], &ReviewAction::Correct { value: "상품권".into() }, None).unwrap();
    assert!(out[0].validation.iter().any(|i| i.rule == "enum_option"), "{:?}", out[0].validation);
    let id = f("cash_receipt_id");
    let out = review::apply(&w.db, &[id.id], &ReviewAction::Correct { value: "010-9876-5432".into() }, None).unwrap();
    assert_eq!(out[0].value.as_deref(), Some("***-****-5432"));
    let history = w.db.with(|c| repo::field_history(c, "r1")).unwrap();
    assert!(history
        .iter()
        .all(|h| !has_leak(h.new_value.as_deref().unwrap_or(""))
            && !h.new_value.as_deref().unwrap_or("").contains("9876")));

    // Changed content resets what the pipeline derived; a manual type survives.
    w.db.with(|c| docs::set_doc_type(c, "r1", Some("invoice_exempt"), Some("manual"))).unwrap();
    w.db.with(|c| docs::set_file_info(c, "r1", "hash-r1", 10, None)).unwrap();
    assert!(w.doc("r1").classified_at.is_some(), "same hash: nothing changes");
    w.db.with(|c| docs::set_file_info(c, "r1", "hash-r1-v2", 11, None)).unwrap();
    let doc = w.doc("r1");
    assert_eq!((doc.doc_type.as_deref(), doc.doc_type_source.as_deref()), (Some("invoice_exempt"), Some("manual")));
    assert!(doc.classified_at.is_none() && doc.ocr_at.is_none() && doc.extracted_at.is_none());
    assert_eq!((doc.review_status, doc.last_run_id, doc.confidence), (ReviewStatus::None, None, None));
    let page = &w.pages("r1")[0];
    assert!(page.text.is_none() && page.text_source.is_none() && page.ocr_at.is_none() && page.page_doc_type.is_none());
    w.add_doc("r2", "auto.pdf", SourceKind::Pdf, &[None]);
    w.db.with(|c| docs::set_doc_type(c, "r2", Some("receipt"), Some("auto"))).unwrap();
    w.db.with(|c| docs::set_file_info(c, "r2", "other", 11, None)).unwrap();
    assert_eq!(w.doc("r2").doc_type, None, "an automatic type is forgotten with the content");

    // A merge name that is not a plain file stem is refused before anything is written.
    let deps = w.import_deps();
    let docs_dir = w.paths.documents_dir();
    let before = std::fs::read_dir(&docs_dir).unwrap().count();
    for bad in ["../escape", "a/b", "con:fig"] {
        let err = super::split::merge_documents(&deps, &[w.doc("r1"), w.doc("r2")], Some(bad), None).unwrap_err();
        assert!(err.to_string().contains("invalid name"), "{bad}: {err}");
    }
    assert_eq!(std::fs::read_dir(&docs_dir).unwrap().count(), before);

    // Exactly one default per document type when a schema moves between types.
    let q =
        w.db.with(|c| {
            repo::save_schema(
                c,
                &SchemaDraft {
                    name: "견적서".into(),
                    doc_type: "quotation".into(),
                    source: SchemaSource::User,
                    definition: SchemaDef::new(vec![FieldDef::new("total", "합계", "Total", FieldKind::Amount)], &[]),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    assert!(q.is_default);
    let moved =
        w.db.with(|c| {
            repo::save_schema(
                c,
                &SchemaDraft {
                    id: Some(q.id.clone()),
                    name: "견적서".into(),
                    doc_type: "receipt".into(),
                    source: SchemaSource::User,
                    definition: q.definition.clone(),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    assert!(!moved.is_default, "the sample receipt schema keeps the slot");
    let defaults =
        |t: &str| w.db.with(repo::list_schemas).unwrap().iter().filter(|s| s.doc_type == t && s.is_default).count();
    assert_eq!((defaults("receipt"), defaults("quotation")), (1, 0));
    let back =
        w.db.with(|c| {
            repo::save_schema(
                c,
                &SchemaDraft {
                    id: Some(q.id.clone()),
                    name: "견적서".into(),
                    doc_type: "quotation".into(),
                    source: SchemaSource::User,
                    definition: q.definition.clone(),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    assert!(back.is_default, "a type without a default adopts the arriving schema");
    assert_eq!((defaults("receipt"), defaults("quotation")), (1, 1));
    // A draft may not claim the builtin source.
    let fake =
        w.db.with(|c| {
            repo::save_schema(
                c,
                &SchemaDraft {
                    name: "가짜 내장".into(),
                    doc_type: "quotation".into(),
                    source: SchemaSource::Builtin,
                    definition: q.definition.clone(),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    assert_eq!(fake.source, SchemaSource::User);
    assert!(w.db.with(|c| repo::delete_schema(c, &fake.id)).unwrap());
}

#[test]
fn extraction_page_budget_keeps_first_then_last() {
    let w = World::new();
    w.add_doc("m", "multi.pdf", SourceKind::Pdf, &[Some("1"), Some("2"), Some("3"), Some("4"), Some("5")]);
    let pages = w.pages("m");
    let nos = |max: usize, only: &[u32]| -> Vec<u32> {
        run::extraction_pages(&pages, max, only).iter().map(|p| p.page_no).collect()
    };
    assert_eq!(nos(1, &[]), vec![1], "a one-page budget is the first page");
    assert_eq!(nos(2, &[]), vec![1, 5]);
    assert_eq!(nos(4, &[]), vec![1, 2, 3, 5]);
    assert_eq!(nos(9, &[]), vec![1, 2, 3, 4, 5]);
    assert_eq!(nos(2, &[2, 3, 4]), vec![2, 4], "an explicit page list is honoured");
}
