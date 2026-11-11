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

/// Audit PIP-2: a forced re-run (the review screen's "재추출", the form editor's "테스트") read
/// every page again and replaced the page text the user had corrected by hand.
#[test]
fn manual_text_is_only_replaced_when_the_request_asks_for_it() {
    let w = World::new();
    w.add_doc("m1", "memo.pdf", SourceKind::Pdf, &[None, None]);
    w.db.with(|c| docs::set_page_text(c, "m1", 1, "손으로 고친 영수증 텍스트")).unwrap();
    w.chat.push("classify", classify_reply("receipt", 0));
    w.chat.push_ocr(RECEIPT_OCR);
    let deps = w.deps(None);
    let source = |w: &World, page: usize| w.pages("m1")[page - 1].text_source.clone();
    let ocr = |force: bool, reocr_manual: bool| ProcessRequest {
        document_ids: vec!["m1".into()],
        level: PipelineLevel::Ocr,
        force,
        reocr_manual,
        ..Default::default()
    };
    // A normal run reads the page without text and leaves the corrected one alone.
    let report = run::process(&deps, &ocr(false, false), &mut |_| {}).unwrap();
    assert_eq!(report.ocr_pages, 1, "manual text is not overwritten: {report:?}");
    assert_eq!(w.pages("m1")[0].text.as_deref(), Some("손으로 고친 영수증 텍스트"));
    assert_eq!((source(&w, 1).as_deref(), source(&w, 2).as_deref()), (Some("manual"), Some("ocr")));
    // Force reads the OCR page again — and still keeps the manual correction.
    let report = run::process(&deps, &ocr(true, false), &mut |_| {}).unwrap();
    assert_eq!(report.ocr_pages, 1, "{report:?}");
    assert_eq!(w.pages("m1")[0].text.as_deref(), Some("손으로 고친 영수증 텍스트"));
    assert_eq!(source(&w, 1).as_deref(), Some("manual"));
    // `reocr_manual` without force changes nothing.
    assert_eq!(run::process(&deps, &ocr(false, true), &mut |_| {}).unwrap().ocr_pages, 0);
    assert_eq!(source(&w, 1).as_deref(), Some("manual"));
    // Only the explicit flag replaces the correction with OCR output.
    let report = run::process(&deps, &ocr(true, true), &mut |_| {}).unwrap();
    assert_eq!(report.ocr_pages, 2, "{report:?}");
    assert_eq!(source(&w, 1).as_deref(), Some("ocr"));
    assert!(w.pages("m1")[0].text.as_deref().unwrap().contains("스타벅스"));
    // A page the user emptied by hand holds no correction: a forced run reads it again.
    w.db.with(|c| docs::set_page_text(c, "m1", 1, "  ")).unwrap();
    assert_eq!(source(&w, 1).as_deref(), Some("manual"));
    assert_eq!(run::process(&deps, &ocr(false, false), &mut |_| {}).unwrap().ocr_pages, 0);
    assert_eq!(run::process(&deps, &ocr(true, false), &mut |_| {}).unwrap().ocr_pages, 2);
    assert_eq!(source(&w, 1).as_deref(), Some("ocr"));
    // The request reads its flag from JSON as sent by the UI; without it the default is off.
    let parsed: ProcessRequest =
        serde_json::from_value(json!({ "document_ids": ["m1"], "level": "extract", "force": true })).unwrap();
    assert!(parsed.force && !parsed.reocr_manual && !parsed.skip_extracted);
    let parsed: ProcessRequest =
        serde_json::from_value(json!({ "document_ids": ["m1"], "force": true, "reocr_manual": true })).unwrap();
    assert!(parsed.reocr_manual);
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
        name: "우리 견적서".into(),
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
        name: "우리 견적서".into(),
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
    // The built-in sample is already called 견적서: a user form needs its own name (name guard).
    let draft = forms::derive(&deps, None, &w.doc("f1"), Some("한빛 견적서")).unwrap();
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
fn schema_discovery_merges_samples_and_the_name_guard_refuses_duplicates() {
    let w = World::new();
    // A blank HWPX form (read from its tables) and a scanned sample read by the scripted model.
    let hwpx = w.paths.root.join("documents/estimate-form.hwpx");
    std::fs::create_dir_all(hwpx.parent().unwrap()).unwrap();
    crate::documents::convert::hwpx::write_sample_hwpx(&hwpx, 1, crate::documents::fixtures::TINY_PNG).unwrap();
    w.db.with(|c| {
        docs::insert_document(
            c,
            &NewDocument {
                id: "s1".into(),
                rel_path: "documents/estimate-form.hwpx".into(),
                original_name: "estimate-form.hwpx".into(),
                source_kind: SourceKind::Hwpx,
                content_hash: "h-s1".into(),
                size_bytes: 1,
                file_modified_at: None,
                role: DocumentRole::Work,
                import_batch_id: None,
            },
        )
    })
    .unwrap();
    w.add_doc("s2", "estimate-scan.jpg", SourceKind::Image, &[None]);
    let field = |key: &str, label: &str, kind: &str, required: bool| json!({"key": key, "label_ko": label, "label_en": key, "type": kind, "required": required, "regex_hint": null, "example": null, "options": [], "columns": []});
    w.chat.push(
        "form_fields",
        json!({
            "name": "견적서", "name_en": "Estimate", "doc_type": "estimate", "description_ko": "견적", "description_en": "estimate",
            "fields": [field("project_name", "사업명", "string", true), field("recipient", "수신", "string", true), field("grand_total", "합계금액", "amount", false)]
        }),
    );
    let deps = w.import_deps();
    let vlm = w.vlm();
    let mut seen: Vec<(usize, Option<(u32, u32)>)> = Vec::new();
    let out =
        forms::discover(&deps, Some(&vlm), &[w.doc("s1"), w.doc("s2")], Some("한빛 견적서"), None, &mut |p| {
            seen.push((p.index, p.page))
        })
        .unwrap();
    assert_eq!(out.kind, "schema_discovery");
    assert_eq!(out.documents.len(), 2);
    assert_eq!((out.documents[0].source.as_str(), out.documents[1].source.as_str()), ("structure", "model"));
    assert!(out.documents[0].field_count >= 1 && out.documents[1].field_count == 3);
    assert_eq!(out.documents[1].pages_used, vec![1]);
    assert_eq!(w.chat.image_calls(), 1, "one page per scanned sample");
    assert!(seen.iter().any(|(i, p)| *i == 1 && *p == Some((1, 1))), "{seen:?}");
    let d = &out.draft;
    assert_eq!((d.name.as_str(), d.doc_type.as_str()), ("한빛 견적서", "estimate"));
    assert_eq!(d.source, SchemaSource::Form);
    assert_eq!(d.form_document_id.as_deref(), Some("s1"));
    assert_eq!(d.sample_document_ids, vec!["s2".to_string()]);
    // Two samples: every field of either is kept (⌈2/2⌉ = 1); one seen on a single sample is not required.
    assert_eq!(out.merged_fields, d.definition.fields.len());
    assert_eq!(out.dropped_fields, 0);
    let project = d.definition.field("project_name").expect("the model's field survives the merge");
    assert!(!project.required, "required means on every sample");
    assert!(d.definition.field("recipient").is_some());

    // Registering marks both samples as forms of the type.
    let saved = forms::register(&w.db, d).unwrap();
    for id in ["s1", "s2"] {
        let doc = w.doc(id);
        assert_eq!(
            (doc.role, doc.doc_type.as_deref(), doc.doc_type_source.as_deref()),
            (DocumentRole::Form, Some("estimate"), Some("form")),
            "{id}"
        );
    }

    // The name guard: the built-in sample is called 견적서 (case / space insensitive), the editor gets "(2)".
    assert!(w.db.with(|c| repo::name_exists(c, "견적서", None)).unwrap());
    assert!(w.db.with(|c| repo::name_exists(c, " 견 적 서 ", None)).unwrap());
    assert!(
        !w.db.with(|c| repo::name_exists(c, "한빛 견적서", Some(saved.id.as_str()))).unwrap(),
        "editing keeps its own name"
    );
    assert!(w.db.with(|c| repo::name_exists(c, "한빛 견적서", None)).unwrap());
    assert_eq!(w.db.with(|c| repo::free_name(c, "견적서", None)).unwrap(), "견적서 (2)");
    let dup = SchemaDraft {
        name: "견적서".into(),
        doc_type: "estimate".into(),
        source: SchemaSource::User,
        definition: SchemaDef::new(vec![FieldDef::new("total", "합계", "Total", FieldKind::Amount)], &[]),
        ..Default::default()
    };
    let err = w.db.with(|c| repo::save_schema(c, &dup)).unwrap_err();
    assert!(err.to_string().contains("schema_name_duplicate"), "{err}");
    assert!(w.db.with(|c| repo::save_schema(c, &SchemaDraft { name: "견적서 (2)".into(), ..dup.clone() })).is_ok());

    // A sample the model cannot read is reported, not fatal, while another sample succeeded.
    w.add_doc("s3", "blank-scan.jpg", SourceKind::Image, &[None]);
    *w.chat.fail_with.lock().unwrap() = Some("boom".into());
    let out = forms::discover(&deps, Some(&vlm), &[w.doc("s1"), w.doc("s3")], None, None, &mut |_| {}).unwrap();
    assert_eq!(out.documents[1].source, "failed");
    assert!(out.documents[1].note.as_deref().is_some_and(|n| n.contains("no_fields")), "{:?}", out.documents[1].note);
    assert_eq!(out.documents[0].source, "structure");
    assert!(out.draft.note.as_deref().is_some_and(|n| n.contains("blank-scan.jpg")), "{:?}", out.draft.note);
    // Alone, that sample is an error.
    assert!(forms::discover(&deps, Some(&vlm), &[w.doc("s3")], None, None, &mut |_| {}).is_err());
    *w.chat.fail_with.lock().unwrap() = None;
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

/// Audit DOC-3: while a split registered its parts one by one, the inbox watcher imported the
/// parts still waiting; the split then hit the duplicate path and its rollback deleted the files
/// of the documents the watcher had just registered.
#[test]
fn a_failed_split_rolls_back_only_its_own_parts() {
    let w = World::new();
    let path = w.paths.inbox_dir().join("batch.pdf");
    crate::documents::convert::write_blank_pdf(&path, 4, &["a".into(), "b".into(), "c".into(), "d".into()]).unwrap();
    let row = |id: &str, rel: &str| NewDocument {
        id: id.into(),
        rel_path: rel.into(),
        original_name: rel.rsplit('/').next().unwrap().into(),
        source_kind: SourceKind::Pdf,
        content_hash: format!("h-{id}"),
        size_bytes: 1,
        file_modified_at: None,
        role: DocumentRole::Work,
        import_batch_id: None,
    };
    w.db.with(|c| docs::insert_document(c, &row("s1", "inbox/batch.pdf"))).unwrap();
    w.db.with(|c| docs::set_status(c, "s1", DocumentStatus::Rendered, None)).unwrap();
    let deps = w.import_deps();
    let (part1, part2) = (w.paths.inbox_dir().join("batch_part1.pdf"), w.paths.inbox_dir().join("batch_part2.pdf"));
    // Another importer registers part 2 between the write and the split's own registration.
    let err = super::split::split_document_observed(&deps, &w.doc("s1"), &[3], None, &mut |parts| {
        assert_eq!(
            parts.iter().map(|(p, n)| (p.clone(), *n)).collect::<Vec<_>>(),
            [(part1.clone(), 2), (part2.clone(), 2)]
        );
        w.db.with(|c| docs::insert_document(c, &row("watcher", "inbox/batch_part2.pdf"))).unwrap();
    })
    .unwrap_err();
    assert_eq!(err.code(), "db", "{err}");
    assert!(!part1.exists(), "the split's own unregistered part is removed");
    assert!(part2.is_file(), "the other importer's document keeps its file");
    assert_eq!(w.doc("watcher").rel_path, "inbox/batch_part2.pdf");
    assert_eq!(w.doc("s1").status, DocumentStatus::Rendered, "the original is not archived by a failed split");
    let all = w.db.with(|c| docs::list_documents(c, &Default::default())).unwrap();
    assert_eq!(all.len(), 2, "no half-registered part rows: {:?}", all.iter().map(|d| &d.rel_path).collect::<Vec<_>>());

    // A name a document still owns is not reused, even when its file is gone (a `missing` part of
    // an earlier split used to end the next split with a UNIQUE(rel_path) error).
    std::fs::remove_file(&part2).unwrap();
    let ids = super::split::split_document(&deps, &w.doc("s1"), &[3], None).unwrap();
    let rels: Vec<String> = ids.iter().map(|id| w.doc(id).rel_path).collect();
    assert_eq!(rels, ["inbox/batch_part1.pdf", "inbox/batch_part2_2.pdf"]);
    assert_eq!(w.doc("s1").status, DocumentStatus::Archived);
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
                    name: "이동 견적서".into(),
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
                    name: "이동 견적서".into(),
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
                    name: "이동 견적서".into(),
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

/// Audit PIP-1: the automatic pipeline after an import re-extracted documents that only came
/// back under a new name (inbox default = full processing) — reviewed fields were replaced by a
/// fresh run and the review started over.
#[test]
fn automatic_runs_leave_extracted_documents_alone() {
    let w = World::new();
    w.add_doc("r1", "receipt.pdf", SourceKind::Pdf, &[None]);
    w.chat.push("classify", classify_reply("receipt", 0));
    w.chat.push_ocr(RECEIPT_OCR);
    w.chat.push("kie_receipt", receipt_kie());
    let deps = w.deps(None);
    run::process(&deps, &ProcessRequest { document_ids: vec!["r1".into()], ..Default::default() }, &mut |_| {})
        .unwrap();
    // The user corrects one field and approves the rest: the document is reviewed.
    let approval = w.fields("r1").into_iter().find(|f| f.path == "approval_no").unwrap();
    review::apply(&w.db, &[approval.id], &ReviewAction::Correct { value: "87654321".into() }, None).unwrap();
    review::approve_document(&w.db, "r1", None).unwrap();
    assert_eq!(w.doc("r1").status, DocumentStatus::Reviewed);
    let snapshot = |w: &World| -> Vec<(String, Option<String>, FieldStatus)> {
        w.fields("r1").into_iter().map(|f| (f.path, f.value, f.status)).collect()
    };
    let before = snapshot(&w);

    // The automatic pipeline comes by again (the file was renamed and relinked through the inbox).
    let auto = ProcessRequest { document_ids: vec!["r1".into()], skip_extracted: true, ..Default::default() };
    let report = run::process(&deps, &auto, &mut |_| {}).unwrap();
    assert_eq!((report.processed.len(), report.extracted, report.failed.len()), (1, 0, 0), "{report:?}");
    assert_eq!(w.chat.calls("kie_receipt"), 1, "no second extraction call");
    assert_eq!(w.db.with(|c| repo::list_runs(c, "r1")).unwrap().len(), 1);
    assert_eq!(snapshot(&w), before, "the reviewed fields stand");
    assert_eq!((w.doc("r1").status, w.doc("r1").review_status), (DocumentStatus::Reviewed, ReviewStatus::Done));

    // The same automatic request still processes a document that has no extraction yet …
    w.add_doc("r2", "receipt-2.pdf", SourceKind::Pdf, &[None]);
    w.chat.push("classify", classify_reply("receipt", 0));
    w.chat.push("kie_receipt", receipt_kie());
    let auto = ProcessRequest { document_ids: vec!["r2".into()], skip_extracted: true, ..Default::default() };
    assert_eq!(run::process(&deps, &auto, &mut |_| {}).unwrap().extracted, 1);
    // … and an explicit request replaces the latest run as before (the UI confirms first).
    w.chat.push("kie_receipt", receipt_kie());
    let explicit = ProcessRequest { document_ids: vec!["r1".into()], ..Default::default() };
    assert_eq!(run::process(&deps, &explicit, &mut |_| {}).unwrap().extracted, 1);
    assert_eq!(w.db.with(|c| repo::list_runs(c, "r1")).unwrap().len(), 2);
}

/// Audit DOC-10: after the retention purge trimmed `cache/converted/`, splitting or merging an
/// office document failed with "no converted PDF" although its pages were still shown.
#[test]
fn split_and_merge_write_a_purged_converted_pdf_again() {
    let w = World::new();
    let hwpx = w.paths.root.join("documents/견적서.hwpx");
    std::fs::create_dir_all(hwpx.parent().unwrap()).unwrap();
    crate::documents::convert::hwpx::write_sample_hwpx(&hwpx, 3, crate::documents::fixtures::TINY_PNG).unwrap();
    let deps = w.import_deps();
    let plan = crate::documents::import::plan(
        &w.db,
        &w.paths,
        std::slice::from_ref(&hwpx),
        &Default::default(),
        crate::documents::import::ImportSource::Folder,
    )
    .unwrap();
    let report = crate::documents::import::run(&deps, &plan, None, &mut |_| {}, &mut |_, _| {}).unwrap();
    let id = report.imported[0].clone();
    let cache = DocumentCache::for_document(&w.paths, &id);
    assert!(cache.converted_pdf().is_file(), "stub printer: one page per section");
    // The purge takes the document's folder under cache/converted/.
    std::fs::remove_dir_all(&cache.converted_dir).unwrap();
    let parts = super::split::split_document(&deps, &w.doc(&id), &[2], None).unwrap();
    assert_eq!(parts.len(), 2);
    assert!(cache.converted_pdf().is_file(), "written again from the source file");
    assert_eq!(w.doc(&id).status, DocumentStatus::Archived);
    // The same for a merge that takes the office document as a source.
    std::fs::remove_dir_all(&cache.converted_dir).unwrap();
    let merged = super::split::merge_documents(&deps, &[w.doc(&id), w.doc(&parts[0])], Some("합본"), None).unwrap();
    assert_eq!(lopdf::Document::load(w.paths.root.join(&w.doc(&merged).rel_path)).unwrap().get_pages().len(), 3 + 1);
}

/// Audit PIP-4: "이 값으로 복원" on a history entry of an earlier extraction run changed the field
/// row of that old run — the toast said restored, the value on screen (latest run) stayed.
#[test]
fn restore_applies_to_the_field_of_the_latest_run() {
    let w = World::new();
    w.add_doc("r1", "receipt.pdf", SourceKind::Pdf, &[None]);
    w.chat.push("classify", classify_reply("receipt", 0));
    w.chat.push_ocr(RECEIPT_OCR);
    w.chat.push("kie_receipt", receipt_kie());
    let deps = w.deps(None);
    let req = ProcessRequest { document_ids: vec!["r1".into()], ..Default::default() };
    run::process(&deps, &req, &mut |_| {}).unwrap();
    let field = |w: &World, path: &str| w.fields("r1").into_iter().find(|f| f.path == path).unwrap();
    let first = field(&w, "approval_no");
    review::apply(&w.db, &[first.id], &ReviewAction::Correct { value: "87654321".into() }, None).unwrap();
    let change = w.db.with(|c| repo::field_history(c, "r1")).unwrap().remove(0);
    assert_eq!((change.field_id, change.old_value.as_deref()), (first.id, Some("12345678")));

    // The document is extracted again: a new run with its own field rows and another value.
    let mut again = receipt_kie();
    again["approval_no"] = sv("99999999", "카드 승인 99999999", 60);
    w.chat.push("kie_receipt", again);
    run::process(&deps, &req, &mut |_| {}).unwrap();
    let latest = field(&w, "approval_no");
    assert_ne!(latest.id, first.id);
    assert_eq!(latest.value.as_deref(), Some("99999999"));

    // Restoring the old entry now changes what the user looks at: the latest run's field.
    let restored = review::restore(&w.db, change.id).unwrap();
    assert_eq!((restored.id, restored.run_id.as_str()), (latest.id, latest.run_id.as_str()));
    assert_eq!(field(&w, "approval_no").value.as_deref(), Some("12345678"));
    assert_eq!(field(&w, "approval_no").status, FieldStatus::Corrected);
    let old_row = w.db.with(|c| repo::get_field(c, first.id)).unwrap().unwrap();
    assert_eq!(old_row.value.as_deref(), Some("87654321"), "the old run's row is history, not a target");
    let newest = w.db.with(|c| repo::field_history(c, "r1")).unwrap().remove(0);
    assert_eq!((newest.field_id, newest.note.as_deref()), (latest.id, Some("restored")));

    // A path the latest run no longer has (another schema) cannot be restored.
    w.db.with(|c| c.execute("UPDATE fields SET path = 'gone', key = 'gone' WHERE id = ?1", [latest.id])).unwrap();
    let err = review::restore(&w.db, change.id).unwrap_err();
    assert!(err.to_string().contains("field_not_found"), "{err}");
}

/// Audit PIP-8: one page whose extraction reply was cut off by the length limit (a table of 30+
/// rows on an 8K preset) failed the whole document and threw the other pages' results away.
#[test]
fn one_bad_page_does_not_fail_the_whole_extraction() {
    let w = World::new();
    let p1 = "스타벅스 역삼점\n사업자등록번호 220-81-62517\n2026-10-14 12:31\n합계 12,500";
    w.add_doc("t1", "receipt-3p.pdf", SourceKind::Pdf, &[Some(p1), Some("품목 계속\n라떼 1 5,000"), Some("비고")]);
    w.chat.push("classify", classify_reply("receipt", 0));
    // Page 1 answers in full; page 2 is cut inside its second row (three attempts with a growing
    // reply budget); page 3 answers with something that is not JSON.
    w.chat.push("kie_receipt", receipt_kie());
    let cut = r#"{"store_name": {"value": null, "evidence": null, "confidence": 0}, "items": [{"name": "라떼", "quantity": "1", "unit_price": "5,000", "amount": "5,000"}, {"name": "쿠"#;
    for _ in 0..3 {
        w.chat.push_raw("kie_receipt", cut, Some("length"));
    }
    w.chat.push_raw("kie_receipt", "죄송합니다, 이 쪽은 읽을 수 없습니다.", None);
    let deps = w.deps(None);
    let req = ProcessRequest { document_ids: vec!["t1".into()], ..Default::default() };
    let report = run::process(&deps, &req, &mut |_| {}).unwrap();
    assert_eq!((report.failed.len(), report.extracted, report.review_pending), (0, 1, 1), "{report:?}");
    let runs = w.db.with(|c| repo::list_runs(c, "t1")).unwrap();
    assert_eq!((runs[0].status.as_str(), runs[0].pages_done), ("done", 2));
    let note = runs[0].error.as_deref().unwrap_or("");
    assert!(note.contains("2쪽") && note.contains("3쪽") && note.contains("not JSON"), "{note}");

    // Page 1's values and page 2's complete row are there; the half-written row is not.
    let fields = w.fields("t1");
    let f = |p: &str| fields.iter().find(|f| f.path == p).unwrap();
    assert_eq!(f("store_name").value.as_deref(), Some("스타벅스 역삼점"));
    assert_eq!((f("items[2].name").value.as_deref(), f("items[2].name").page), (Some("라떼"), Some(2)));
    assert!(fields.iter().all(|f| f.row != Some(3)), "no row from the cut part");
    // Each incomplete page leaves one marked field in the review queue.
    let marked: Vec<&str> = fields
        .iter()
        .filter(|f| f.validation.iter().any(|i| i.rule == super::extract::RULE_PARTIAL_PAGE))
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(marked, ["store_name", "items[2].amount"], "page 3 → first field, page 2 → its last value");
    assert!(fields.iter().filter(|f| marked.contains(&f.path.as_str())).all(|f| f.needs_review()));
    assert_eq!((w.doc("t1").status, w.doc("t1").review_status), (DocumentStatus::Extracted, ReviewStatus::Pending));

    // When no page answers, the document fails as before.
    w.add_doc("t2", "bad.pdf", SourceKind::Pdf, &[Some("본문")]);
    w.db.with(|c| docs::set_doc_type(c, "t2", Some("receipt"), Some("manual"))).unwrap();
    w.chat.push_raw("kie_receipt", "not json", None);
    let report =
        run::process(&deps, &ProcessRequest { document_ids: vec!["t2".into()], ..Default::default() }, &mut |_| {})
            .unwrap();
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(report.failed[0].1.contains("not JSON"), "{:?}", report.failed);
    assert_eq!(w.db.with(|c| repo::list_runs(c, "t2")).unwrap()[0].status, "failed");
}

/// Audit PIP-6: when the engine died or was stopped during a batch, every remaining document
/// "failed" within seconds (300 documents, engine gone at the 40th → 260 marked 처리 오류).
#[test]
fn a_missing_engine_stops_the_batch_and_leaves_the_rest_untouched() {
    let w = World::new();
    for id in ["a", "b", "c"] {
        w.add_doc(id, &format!("{id}.pdf"), SourceKind::Pdf, &[None]);
    }
    w.chat.push("classify", classify_reply("receipt", 0));
    w.chat.push_ocr(RECEIPT_OCR);
    w.chat.push("kie_receipt", receipt_kie());
    let deps = w.deps(None);
    let req = ProcessRequest { document_ids: vec!["a".into(), "b".into(), "c".into()], ..Default::default() };
    // The engine goes away when the second document starts: its port refuses connections.
    let refused = "POST /v1/chat/completions: io: Connection refused (os error 111)";
    let err = run::process(&deps, &req, &mut |p| {
        if p.done == 1 {
            *w.chat.fail_with.lock().unwrap() = Some(refused.into());
        }
    })
    .unwrap_err();
    assert_eq!(err.code(), "engine_not_running", "{err}");
    assert!(w.doc("a").extracted_at.is_some(), "the document before the failure is done");
    for id in ["b", "c"] {
        let d = w.doc(id);
        assert!(d.pipeline_error.is_none(), "{id}: no 처리 오류 for a missing engine: {:?}", d.pipeline_error);
        assert!(d.classified_at.is_none() && d.extracted_at.is_none(), "{id} is untouched");
    }
    let status = w.db.with(repo::pipeline_status).unwrap();
    assert_eq!((status.extracted, status.failed), (1, 0));

    // An error the engine answered with is still that document's failure, and the batch goes on.
    *w.chat.fail_with.lock().unwrap() = Some("/v1/chat/completions: HTTP 400: grammar".into());
    let rest = ProcessRequest { document_ids: vec!["b".into(), "c".into()], ..Default::default() };
    let report = run::process(&deps, &rest, &mut |_| {}).unwrap();
    assert_eq!(report.failed.len(), 2, "{report:?}");
    assert!(w.doc("b").pipeline_error.is_some() && w.doc("c").pipeline_error.is_some());
}

/// Audit PIP-5: a processing job that was retried or resumed replayed its request from the first
/// document — 60 of 100 done, stop, continue: the 60 were extracted again (the time once more, and
/// whatever had been reviewed in between was replaced).
#[test]
fn a_replayed_request_goes_on_where_it_stopped() {
    let w = World::new();
    for id in ["a", "b", "c"] {
        w.add_doc(id, &format!("{id}.pdf"), SourceKind::Pdf, &[None]);
    }
    w.chat.push("classify", classify_reply("receipt", 0));
    w.chat.push_ocr(RECEIPT_OCR);
    w.chat.push("kie_receipt", receipt_kie());
    let ids: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
    let req = ProcessRequest { document_ids: ids.clone(), request_id: Some("req1".into()), ..Default::default() };
    let runs = |id: &str| w.db.with(|c| repo::list_runs(c, id)).unwrap().len();

    // First attempt: the job is cancelled when the second document starts.
    let cancel = AtomicBool::new(false);
    let deps = w.deps(Some(&cancel));
    let report = run::process(&deps, &req, &mut |p| {
        if p.done == 1 {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    })
    .unwrap();
    assert!(report.cancelled);
    assert_eq!((report.processed.clone(), report.resumed.len()), (vec!["a".to_string()], 0));
    assert_eq!((runs("a"), runs("b"), runs("c")), (1, 0, 0));
    // Meanwhile the user reviews the finished document.
    review::approve_document(&w.db, "a", None).unwrap();
    let reviewed = w.doc("a").status;

    // The job is retried: the same request, replayed. The finished document is left alone.
    cancel.store(false, std::sync::atomic::Ordering::Relaxed);
    let report = run::process(&deps, &req, &mut |_| {}).unwrap();
    assert_eq!(report.resumed, vec!["a".to_string()]);
    assert_eq!(report.processed, vec!["b".to_string(), "c".to_string()]);
    assert_eq!((runs("a"), runs("b"), runs("c")), (1, 1, 1), "a is not extracted a second time");
    assert_eq!(w.doc("a").status, reviewed, "its review stands");
    assert_eq!(w.chat.calls("kie_receipt"), 3);

    // The request ran to its end, so its journal is gone: nothing of it is left in the store,
    // and a request without an id (or another id) processes every document as before.
    assert!(w.db.with(|c| repo::request_done(c, "req1")).unwrap().is_empty());
    let plain = ProcessRequest { document_ids: ids.clone(), ..Default::default() };
    let report = run::process(&deps, &plain, &mut |_| {}).unwrap();
    assert_eq!((report.processed.len(), report.resumed.len(), runs("a")), (3, 0, 2));

    // A document that failed is not journalled: the next attempt tries it again.
    let retry = ProcessRequest { document_ids: ids, request_id: Some("req2".into()), ..Default::default() };
    *w.chat.fail_with.lock().unwrap() = Some("POST /v1/chat/completions: io: Connection refused (os error 111)".into());
    w.db.with(|c| repo::request_mark_done(c, "req2", "a")).unwrap(); // as if `a` had been finished
    assert_eq!(run::process(&deps, &retry, &mut |_| {}).unwrap_err().code(), "engine_not_running");
    assert_eq!(w.db.with(|c| repo::request_done(c, "req2")).unwrap(), vec!["a".to_string()], "the journal is kept");
    *w.chat.fail_with.lock().unwrap() = None;
    let report = run::process(&deps, &retry, &mut |_| {}).unwrap();
    assert_eq!((report.resumed.len(), report.processed.len()), (1, 2));
}
