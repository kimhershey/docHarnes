//! 0.8.0 audit fixes in the executor and its previews (PLN-3, PLN-4, PLN-5, PLN-6, PLN-8,
//! PLN-18, PLN-19, CON-3, PRJ-12), run against a temporary workspace with a scripted model.

use super::*;

use crate::knowledge::index::{index_document, IndexDeps};
use crate::pipeline::review;
use crate::pipeline::FieldStatus;
use crate::projects::{repo as projects_repo, ProjectInput, Rule, RuleOp, RuleOperand};

fn sel(ids: &[&str]) -> Scope {
    Scope { doc_ids: ids.iter().map(|s| s.to_string()).collect(), ..Scope::default() }
}

fn folder(name: &str) -> Scope {
    Scope { folder: Some(name.into()), ..Scope::default() }
}

fn of_type(slug: &str) -> Scope {
    Scope { filter: ScopeFilter { doc_type: Some(slug.into()), ..ScopeFilter::default() }, ..Scope::default() }
}

fn run(w: &World, plan: &Plan) -> exec::PlanExecution {
    exec::run(&w.deps(None), plan, &mut |_| {}).unwrap()
}

fn preview(w: &World, plan: &Plan) -> exec::PlanPreview {
    exec::preview(&w.db, &w.paths, plan, &ProcessingSettings::default(), Language::Ko).unwrap()
}

/// Replies for one receipt through classify → OCR → extract (the last reply of a stage repeats).
fn script_receipt(w: &World) {
    w.chat.push("classify", json!({ "doc_type": "receipt", "rotation": 0, "is_blank": false, "language": "ko" }));
    w.chat.push_ocr(RECEIPT_OCR);
    w.chat.push("kie_receipt", receipt_kie());
}

fn extract(force: Option<bool>) -> Op {
    Op::Extract { schema_ref: Some("receipt".into()), fields: Vec::new(), force }
}

fn field_of(w: &World, doc: &str, key: &str) -> crate::pipeline::FieldRecord {
    w.db.with(|c| pipeline_repo::latest_fields(c, doc)).unwrap().into_iter().find(|f| f.key == key).unwrap()
}

/// An image file outside the workspace (`seed` makes its content — and hash — unique).
fn outside_image(dir: &std::path::Path, name: &str, seed: u32) -> String {
    let path = dir.join(name);
    image::RgbImage::from_fn(320 + seed, 240, |x, y| {
        if (x / 40 + y / 40) % 2 == 0 {
            image::Rgb([250, 250, 250])
        } else {
            image::Rgb([20, 20, 20])
        }
    })
    .save(&path)
    .unwrap();
    path.to_string_lossy().to_string()
}

fn import_files(paths: Vec<String>) -> Op {
    Op::Import { source: ImportFrom::Files, path: None, paths, recursive: None, auto_pipeline: None }
}

fn tag(name: &str) -> Op {
    Op::Tag { add: vec![name.into()], remove: Vec::new() }
}

/// PLN-3: the assistant's `extract` ran over every document in scope again, replacing reviewed
/// results without a preview. Extracted documents are now kept unless the run is forced, and a
/// forced run that replaces review work is confirmed.
#[test]
fn extract_keeps_existing_results_unless_forced() {
    let w = World::new();
    w.add_image("r1", "inbox/a.jpg");
    w.add_image("r2", "inbox/b.jpg");
    script_receipt(&w);
    // r1 was extracted earlier and the user corrected its total in review.
    let ex = run(&w, &plan_of(vec![extract(None)], sel(&["r1"])));
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    let first_run = w.doc("r1").last_run_id.clone().expect("extracted");
    review::correct(&w.db, field_of(&w, "r1", "total").id, "13,000").unwrap();
    assert_eq!(w.chat.calls("kie_receipt"), 1);

    // "inbox 문서 추출해줘": r1 keeps its extraction, only r2 is extracted — and the card says so.
    let plan = plan_of(vec![extract(None)], folder("inbox"));
    let p = preview(&w, &plan);
    let step = &p.steps[0];
    assert_eq!(
        (
            step.payload["documents"].as_u64(),
            step.payload["extract"].as_u64(),
            step.payload["already_extracted"].as_u64()
        ),
        (Some(2), Some(1), Some(1)),
        "{step:?}"
    );
    assert_eq!((step.payload["replace"].as_u64(), step.confirm), (Some(0), false), "{step:?}");
    assert!(step.summary.contains("이미 추출된 1건"), "{}", step.summary);
    let ex = run(&w, &plan);
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert!(ex.steps[0].summary.contains("이미 추출된 1건은 건너뜀"), "{}", ex.steps[0].summary);
    assert_eq!(ex.steps[0].payload["processed"], json!(["r2"]));
    assert_eq!(ex.steps[0].payload["already_extracted"], json!(["r1"]));
    assert_eq!(w.chat.calls("kie_receipt"), 2, "one more extraction, for r2 only");
    assert_eq!(w.doc("r1").last_run_id.as_deref(), Some(first_run.as_str()), "r1 keeps its run");
    let total = field_of(&w, "r1", "total");
    assert_eq!((total.status, total.normalized.as_deref()), (FieldStatus::Corrected, Some("13000")), "{total:?}");
    assert_eq!(ex.documents, vec!["r2".to_string()], "r1 was not touched");

    // Everything is extracted now: the step does nothing and needs no model.
    let mut deps = w.deps(None);
    deps.chat = None;
    deps.model = None;
    let ex = exec::run(&deps, &plan, &mut |_| {}).unwrap();
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert_eq!(ex.steps[0].payload["already_extracted"], json!(["r1", "r2"]));
    assert_eq!(w.chat.calls("kie_receipt"), 2);

    // "다시 추출": the card counts what is replaced and asks first, because r1 carries review work.
    let forced = plan_of(vec![extract(Some(true))], folder("inbox"));
    assert!(!forced.requires_confirmation, "the op alone needs no card");
    let p = preview(&w, &forced);
    let step = &p.steps[0];
    assert_eq!(
        (step.payload["extract"].as_u64(), step.payload["replace"].as_u64(), step.payload["reviewed"].as_u64()),
        (Some(2), Some(2), Some(1)),
        "{step:?}"
    );
    assert!(step.confirm && p.requires_confirmation, "{step:?}");
    assert!(
        step.summary.contains("기존 결과 2건 대체") && step.summary.contains("검토한 문서 1건"),
        "{}",
        step.summary
    );
    let ex = run(&w, &forced);
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert_eq!(w.chat.calls("kie_receipt"), 4);
    assert_ne!(w.doc("r1").last_run_id.as_deref(), Some(first_run.as_str()), "the forced run replaced r1's run");
    // A forced run over documents without review work needs no confirmation.
    let p = preview(&w, &plan_of(vec![extract(Some(true))], sel(&["r2"])));
    assert_eq!((p.steps[0].payload["reviewed"].as_u64(), p.steps[0].confirm), (Some(0), false), "{:?}", p.steps[0]);
    // After an import the working set is not known yet: a forced extract is confirmed, a plain one is not.
    let after = |force| {
        let plan = plan_of(
            vec![
                Op::Import {
                    source: ImportFrom::Inbox,
                    path: None,
                    paths: Vec::new(),
                    recursive: None,
                    auto_pipeline: None,
                },
                extract(force),
            ],
            folder("inbox"),
        );
        preview(&w, &plan).steps[1].clone()
    };
    assert_eq!((after(None).confirm, after(Some(true)).confirm), (false, true));
    assert_eq!(after(None).payload["after_import"], json!(true));
}

/// PLN-4: after an import step the working set was the import united with the whole scope, so
/// "D:\스캔\a.pdf 가져와서 요약해줘" worked on the whole library, and a type filter dropped the
/// files that had just come in (they have no type before the classify step).
#[test]
fn steps_after_an_import_work_on_the_documents_of_that_import() {
    let w = World::new();
    w.add_image("d1", "inbox/a.jpg");
    w.add_image("d2", "documents/b.jpg");
    let outside = tempfile::tempdir().unwrap();
    let tags = |id: &str| w.doc(id).tags;

    // 1. A file from outside: the later step sees that document only.
    let ex = run(
        &w,
        &plan_of(vec![import_files(vec![outside_image(outside.path(), "scan.jpg", 1)]), tag("새로")], Scope::default()),
    );
    assert!(ex.steps.iter().all(|s| s.status == StepStatus::Done), "{:?}", ex.steps);
    let new_id = ex.steps[0].payload["imported"][0].as_str().expect("one new document").to_string();
    assert_eq!(ex.steps[1].payload["documents"], json!([new_id]));
    assert_eq!(tags(&new_id), ["새로"]);
    assert!(tags("d1").is_empty() && tags("d2").is_empty(), "the library was not touched");

    // 2. A type in the scope: classify still sees the new file, the steps after it only what matches.
    w.chat.push("classify", json!({ "doc_type": "receipt", "rotation": 0, "is_blank": false, "language": "ko" }));
    let plan = plan_of(
        vec![
            import_files(vec![outside_image(outside.path(), "receipt.jpg", 2)]),
            Op::Classify { doc_types: Vec::new(), force: None },
            tag("영수증"),
        ],
        of_type("receipt"),
    );
    let ex = run(&w, &plan);
    assert!(ex.steps.iter().all(|s| s.status == StepStatus::Done), "{:?}", ex.steps);
    let receipt_id = ex.steps[0].payload["imported"][0].as_str().unwrap().to_string();
    assert_eq!(ex.steps[1].payload["classified"], json!(1), "the new file reached the classifier: {:?}", ex.steps[1]);
    assert_eq!(w.doc(&receipt_id).doc_type.as_deref(), Some("receipt"));
    assert_eq!(ex.steps[2].payload["documents"], json!([receipt_id]));
    // Without a classify step the type is unknown: nothing is tagged (never the wrong documents).
    let plan =
        plan_of(vec![import_files(vec![outside_image(outside.path(), "x.jpg", 3)]), tag("영수증")], of_type("receipt"));
    let ex = run(&w, &plan);
    assert_eq!(ex.steps[1].payload["documents"], json!([]), "{:?}", ex.steps[1]);

    // 3. A file that is already registered: the import brings nothing new, the step still works on that document.
    let registered = w.paths.root.join("inbox/a.jpg").to_string_lossy().to_string();
    let ex = run(&w, &plan_of(vec![import_files(vec![registered]), tag("다시")], Scope::default()));
    assert!(ex.steps.iter().all(|s| s.status == StepStatus::Done), "{:?}", ex.steps);
    assert_eq!(
        (ex.steps[0].payload["imported"].clone(), ex.steps[0].payload["existing"].clone()),
        (json!([]), json!(["d1"]))
    );
    assert_eq!(ex.steps[1].payload["documents"], json!(["d1"]));
    assert_eq!(tags("d1"), ["다시"]);

    // 4. An import that brings no document at all: the later steps do nothing — not the whole library.
    let missing = outside.path().join("nope.jpg").to_string_lossy().to_string();
    let ex = run(&w, &plan_of(vec![import_files(vec![missing]), tag("전체")], Scope::default()));
    assert_eq!(ex.steps[1].status, StepStatus::Done, "{:?}", ex.steps[1]);
    assert_eq!(ex.steps[1].payload["documents"], json!([]));
    assert!(!tags("d2").contains(&"전체".to_string()) && !tags("d1").contains(&"전체".to_string()));
}

/// PLN-8: `review correct` and `review show` passed validation and the confirmation card, then
/// the executor did nothing and reported "0 fields rejected".
#[test]
fn review_correct_changes_the_value_and_show_opens_the_queue() {
    let w = World::new();
    w.add_image("r1", "inbox/a.jpg");
    script_receipt(&w);
    run(&w, &plan_of(vec![extract(None)], sel(&["r1"])));
    assert_eq!(field_of(&w, "r1", "total").value.as_deref(), Some("12,500"));

    // "이 문서 합계를 13,000원으로 수정해줘": the field is named by its label.
    let correct = |field: &str, value: &str| {
        plan_of(
            vec![Op::Review {
                action: ReviewOp::Correct,
                field: Some(field.into()),
                value: Some(value.into()),
                only: None,
            }],
            sel(&["r1"]),
        )
    };
    let plan = correct("합계", "13,000원");
    assert!(plan.requires_confirmation);
    let p = preview(&w, &plan);
    assert!(
        p.steps[0].summary.contains("합계")
            && p.steps[0].summary.contains("12,500")
            && p.steps[0].summary.contains("13,000원"),
        "{}",
        p.steps[0].summary
    );
    assert_eq!(p.steps[0].payload["matches"].as_array().map(Vec::len), Some(1), "{:?}", p.steps[0]);
    let ex = run(&w, &plan);
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert_eq!(ex.steps[0].summary, "필드 1개 수정");
    assert_eq!(ex.steps[0].payload["changed"], json!(1));
    let total = field_of(&w, "r1", "total");
    assert_eq!((total.status, total.normalized.as_deref()), (FieldStatus::Corrected, Some("13000")), "{total:?}");
    // By key as well; a field the document does not have fails instead of "0 fields".
    let ex = run(&w, &correct("store_name", "스타벅스 강남점"));
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert_eq!(field_of(&w, "r1", "store_name").value.as_deref(), Some("스타벅스 강남점"));
    let ex = run(&w, &correct("없는 필드", "1"));
    assert_eq!(ex.steps[0].status, StepStatus::Failed, "{:?}", ex.steps[0]);
    assert!(ex.steps[0].error.as_deref().unwrap_or_default().contains("field_not_found"), "{:?}", ex.steps[0]);
    assert!(preview(&w, &correct("없는 필드", "1")).steps[0].summary.contains("찾을 수 없음"));

    // "검토할 것 보여줘": the review screen is handed to the app.
    let show =
        plan_of(vec![Op::Review { action: ReviewOp::Show, field: None, value: None, only: None }], Scope::default());
    assert!(!show.requires_confirmation);
    let ex = run(&w, &show);
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert_eq!(ex.ui_actions, vec![Op::Open { target: OpenTarget::Screen, reference: "review".into() }]);
    assert!(ex.steps[0].summary.contains("검토함"), "{}", ex.steps[0].summary);
    assert!(ex.steps[0].payload["review_fields"].as_u64().unwrap_or(0) >= 1, "{:?}", ex.steps[0].payload);
    assert_eq!(ex.steps[0].payload["documents"], json!(["r1"]));
    // Approve / reject keep their own wording.
    let approve =
        plan_of(vec![Op::Review { action: ReviewOp::Approve, field: None, value: None, only: None }], sel(&["r1"]));
    let ex = run(&w, &approve);
    assert!(ex.steps[0].summary.ends_with("승인"), "{}", ex.steps[0].summary);
}

/// PLN-19: a scope whose filters match no document meant "every document" to the search (an
/// empty id list), so a search / answer over "2019년 계약서" fell back to the whole library.
#[test]
fn a_scope_that_matches_nothing_finds_nothing() {
    let w = World::new();
    w.add_image("d1", "inbox/a.jpg");
    w.db.with(|c| docs::set_page_text(c, "d1", 1, "스타벅스 역삼점 영수증 합계 12,500원")).unwrap();
    index_document(&IndexDeps { db: &w.db, analyzer: &w.analyzer, options: Default::default() }, &w.doc("d1"), true)
        .unwrap();
    let search =
        |scope: Scope| run(&w, &plan_of(vec![Op::Search { query: "스타벅스".into(), mode: None, top_k: None }], scope));
    // The whole library finds the page.
    let ex = search(Scope::default());
    assert_eq!(ex.steps[0].payload["hits"].as_array().map(Vec::len), Some(1), "{:?}", ex.steps[0]);
    // There is no contract: nothing is found (it used to return the receipt).
    let ex = search(of_type("contract"));
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert_eq!(ex.steps[0].payload["hits"].as_array().map(Vec::len), Some(0), "{:?}", ex.steps[0]);
    assert!(ex.steps[0].summary.contains("범위에 해당하는 문서 없음"), "{}", ex.steps[0].summary);
    // A filter that does match still searches inside it.
    w.db.with(|c| docs::set_doc_type(c, "d1", Some("receipt"), Some("manual"))).unwrap();
    let ex = search(of_type("receipt"));
    assert_eq!(ex.steps[0].payload["hits"].as_array().map(Vec::len), Some(1), "{:?}", ex.steps[0]);
    // An answer over such a scope is "no evidence", without a model call.
    let before = w.chat.requests.lock().unwrap().len();
    let ask = plan_of(
        vec![Op::Answer {
            question: "스타벅스 합계는?".into(), cite: None, knowledge: None, mode: None, reply: None
        }],
        of_type("contract"),
    );
    let ex = run(&w, &ask);
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert_eq!(ex.steps[0].payload["no_evidence"], json!(true));
    assert_eq!(w.chat.requests.lock().unwrap().len(), before, "the model was not asked");
    // So is an explanation.
    let explain = plan_of(
        vec![Op::Explain {
            selection: Some("스타벅스".into()),
            question: Some("합계는?".into()),
            scope: None,
            allow_general_knowledge: None,
        }],
        of_type("contract"),
    );
    let ex = run(&w, &explain);
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert_eq!(
        (ex.steps[0].payload["no_evidence"].clone(), ex.steps[0].payload["kind"].clone()),
        (json!(true), json!("explain"))
    );
    assert_eq!(w.chat.requests.lock().unwrap().len(), before, "the model was not asked");
    // Project suggestions: an empty id list means every unassigned document to the classifier,
    // so a scope that matches nothing must suggest nothing.
    w.db.with(|c| {
        projects_repo::save_project(
            c,
            &ProjectInput {
                name: "스타벅스 정산".into(),
                keywords: Some(vec!["스타벅스".into(), "역삼점".into()]),
                ..Default::default()
            },
        )
    })
    .unwrap();
    let suggested = |scope: Scope| {
        let ex = run(&w, &plan_of(vec![Op::ClassifyProjects { apply: None, use_model: None }], scope));
        assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
        ex.steps[0].payload["suggested"].clone()
    };
    assert_eq!(suggested(Scope::default()), json!(1), "the receipt matches the project's keywords");
    assert_eq!(suggested(of_type("contract")), json!(0));
}

/// PLN-19: the first ten bytes of whatever the schema's date field held were sliced off as the
/// document date. A date field typed as text ("2026년10월5일") made that slice end inside a
/// Hangul character — a panic in organise / aggregate / export.
#[test]
fn only_an_iso_document_date_files_a_document() {
    let w = World::new();
    w.add_image("d1", "inbox/a.jpg");
    let mut doc = w.doc("d1");
    let imported = doc.imported_at.format("%Y-%m-%d").to_string();
    let mut basis = |date: Option<&str>| {
        doc.doc_date = date.map(str::to_string);
        crate::planner::scope::basis_date(&doc)
    };
    assert_eq!(basis(Some("2026-10-05")), (Some("2026-10-05".to_string()), "doc_date"));
    assert_eq!(basis(Some(" 2026-10-05T09:30:00 ")), (Some("2026-10-05".to_string()), "doc_date"));
    for text in ["2026년10월5일", "2026년 10월 5일 작성", "10월 중순에 작성함", "2026.10.05 (월)", "2026-13-45 "]
    {
        assert_eq!(basis(Some(text)), (Some(imported.clone()), "imported_at"), "{text}");
    }
    assert_eq!(basis(Some("2026-10")), (Some(imported.clone()), "imported_at"));
    assert_eq!(basis(None), (Some(imported.clone()), "imported_at"));
}

/// CON-3: a draft asked for inside a project looked its reference material up in the whole
/// library. The planner hands the scope's project to the generator, whose knowledge search then
/// stays inside the project (work documents ∪ reference group ∪ 공통 참고 자료).
#[test]
fn a_draft_inside_a_project_takes_its_references_from_that_project() {
    let w = World::new();
    let project =
        w.db.with(|c| projects_repo::save_project(c, &ProjectInput { name: "한빛 납품".into(), ..Default::default() }))
            .unwrap();
    w.add_image("in", "inbox/a.jpg");
    w.add_image("out", "inbox/b.jpg");
    for (id, text) in [
        ("in", "한빛 납품 회의록: 검수 기준은 기능 시험 통과로 한다"),
        ("out", "새솔 유지보수 회의록: 검수 기준은 성능 시험 통과로 한다"),
    ] {
        w.db.with(|c| docs::set_page_text(c, id, 1, text)).unwrap();
        index_document(&IndexDeps { db: &w.db, analyzer: &w.analyzer, options: Default::default() }, &w.doc(id), true)
            .unwrap();
    }
    w.db.with(|c| projects_repo::add_documents(c, &project.id, &["in".to_string()])).unwrap();
    w.chat.push(
        "draft_document",
        json!({
            "title": "검수 기준 회의록",
            "fields": [],
            "tables": [],
            "body": [{ "kind": "paragraph", "text": "검수 기준을 정리했습니다.", "items": [], "source_index": null }],
            "notes": []
        }),
    );
    // The draft's request and the documents its references come from.
    let draft = |scope: Scope| {
        let op = Op::DraftDocument {
            form_ref: Some("meeting_minutes".into()),
            format_description: None,
            content: "검수 기준 회의록".into(),
            sources: Vec::new(),
            output: None,
            tone: None,
            length: None,
        };
        let ex = run(&w, &plan_of(vec![op], scope));
        assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
        let d = &ex.steps[0].payload["draft"];
        let mut from: Vec<String> = d["references"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["doc_id"].as_str().map(str::to_string))
            .collect();
        from.sort();
        from.dedup();
        let prompt = serde_json::to_string(&w.chat.requests.lock().unwrap().last().unwrap().messages).unwrap();
        (d["request"]["project_id"].clone(), from, prompt)
    };
    // Without a project the whole library is the reference material.
    let (project_id, from, prompt) = draft(Scope::default());
    assert!(project_id.is_null(), "{project_id}");
    assert_eq!(from, ["in", "out"]);
    assert!(prompt.contains("기능 시험") && prompt.contains("성능 시험"), "{prompt}");
    // Inside the project the other customer's minutes stay out of the draft.
    let in_project = Scope {
        filter: ScopeFilter { project_id: Some(project.id.clone()), ..ScopeFilter::default() },
        ..Scope::default()
    };
    let (project_id, from, prompt) = draft(in_project);
    assert_eq!(project_id, json!(project.id));
    assert_eq!(from, ["in"]);
    assert!(prompt.contains("기능 시험") && !prompt.contains("성능 시험"), "{prompt}");
}

/// PLN-5 / PLN-6 / PLN-19 in the executor: a new project takes only documents the plan names,
/// the card of a multi-document removal asks first, and an empty template clears the template.
#[test]
fn project_ops_take_named_documents_and_confirm_removals() {
    let w = World::new();
    w.db.with(projects_repo::ensure_builtins).unwrap();
    w.add_image("d1", "inbox/a.jpg");
    w.add_image("d2", "inbox/b.jpg");
    let create = |name: &str| Op::CreateProject {
        name: name.into(),
        template: None,
        doc_types_required: Vec::new(),
        start_date: None,
        end_date: None,
    };
    // No documents named: an empty project (the open project is no longer copied into it).
    let plan = plan_of(vec![create("한빛 2차 심사")], Scope::default());
    let p = preview(&w, &plan);
    assert!(p.steps[0].summary.contains("문서 없이") && p.steps[0].confirm && !p.uses_scope, "{:?}", p.steps[0]);
    let ex = run(&w, &plan);
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    let empty = ex.steps[0].payload["id"].as_str().unwrap().to_string();
    assert!(w.db.with(|c| projects_repo::documents_of(c, &empty)).unwrap().is_empty());
    // Named documents go in, and the card says how many.
    let plan = plan_of(vec![create("한빛 3차 심사")], sel(&["d1", "d2"]));
    let p = preview(&w, &plan);
    assert!(p.steps[0].summary.contains("문서 2건") && p.uses_scope, "{:?}", p.steps[0]);
    let ex = run(&w, &plan);
    let pid = ex.steps[0].payload["id"].as_str().unwrap().to_string();
    assert_eq!(w.db.with(|c| projects_repo::documents_of(c, &pid)).unwrap().len(), 2);

    // Taking both documents out again is confirmed first; one document is not.
    let remove = |ids: &[&str]| {
        plan_of(vec![Op::RemoveFromProject { project: Some(pid.clone()), doc_ids: Vec::new() }], sel(ids))
    };
    let p = preview(&w, &remove(&["d1", "d2"]));
    assert!(p.steps[0].confirm && p.requires_confirmation, "{:?}", p.steps[0]);
    assert!(
        p.steps[0].summary.contains("한빛 3차 심사") && p.steps[0].summary.contains("문서 2건"),
        "{}",
        p.steps[0].summary
    );
    let p = preview(&w, &remove(&["d1"]));
    assert!(!p.steps[0].confirm && !p.requires_confirmation, "{:?}", p.steps[0]);

    // An empty template clears the project's template.
    let tpl = w.db.with(projects_repo::list_templates).unwrap().into_iter().next().expect("a built-in template");
    w.db.with(|c| {
        projects_repo::save_project(
            c,
            &ProjectInput {
                id: Some(pid.clone()),
                name: "한빛 3차 심사".into(),
                template_id: Some(tpl.id.clone()),
                ..Default::default()
            },
        )
    })
    .unwrap();
    let update = |template: Option<&str>| {
        plan_of(
            vec![Op::UpdateProject {
                project: Some(pid.clone()),
                name: None,
                template: template.map(str::to_string),
                start_date: None,
                end_date: None,
                owner: Some("김담당".into()),
                intake_no: None,
                notes: None,
                keywords: None,
                inbox_folder: None,
            }],
            Scope::default(),
        )
    };
    let ex = run(&w, &update(None));
    assert_eq!(ex.steps[0].payload["project"]["template_id"], json!(tpl.id), "not given = kept: {:?}", ex.steps[0]);
    let ex = run(&w, &update(Some("")));
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert!(ex.steps[0].payload["project"]["template_id"].is_null(), "{:?}", ex.steps[0].payload["project"]);
}

/// PLN-18: a new workspace was created without a card, so a path cut at a blank could not be
/// noticed. The step's preview shows the folder and asks first; opening one does not.
#[test]
fn the_card_of_a_new_workspace_shows_its_folder() {
    let w = World::new();
    let workspace = |action: WorkspaceAction, path: &str| {
        preview(&w, &plan_of(vec![Op::Workspace { action, path: Some(path.into()) }], Scope::default()))
    };
    let p = workspace(WorkspaceAction::Create, "D:\\업무 문서");
    assert!(p.steps[0].confirm && p.requires_confirmation, "{:?}", p.steps[0]);
    assert_eq!(p.steps[0].summary, "새 워크스페이스 폴더: D:\\업무 문서");
    assert_eq!(p.steps[0].payload["path"], json!("D:\\업무 문서"));
    assert!(!p.uses_scope, "no document count on the card of a workspace op");
    let p = workspace(WorkspaceAction::Open, "D:\\업무 문서");
    assert!(!p.steps[0].confirm && !p.requires_confirmation, "{:?}", p.steps[0]);
}

/// PRJ-12: `check_consistency` with rule references that match no rule ran zero rules, and that
/// empty run became the project's latest check. References are resolved (by id or by words of
/// the rule's description) and an unknown one fails the step before anything is stored.
#[test]
fn check_consistency_resolves_rule_references_or_fails() {
    let w = World::new();
    let project =
        w.db.with(|c| projects_repo::save_project(c, &ProjectInput { name: "한빛 납품".into(), ..Default::default() }))
            .unwrap();
    let rule = |description: &str, field: &str, enabled: bool| Rule {
        description: description.into(),
        left: RuleOperand::field("estimate", field),
        op: RuleOp::Present,
        enabled,
        ..Rule::default()
    };
    let amount =
        w.db.with(|c| {
            projects_repo::save_rule(
                c,
                &project.id,
                &rule("견적서 합계금액과 계약금액은 같아야 한다", "grand_total", true),
            )
        })
        .unwrap();
    let date =
        w.db.with(|c| projects_repo::save_rule(c, &project.id, &rule("견적서 작성일자는 필수", "issue_date", true)))
            .unwrap();
    w.db.with(|c| projects_repo::save_rule(c, &project.id, &rule("유효기간은 필수", "valid_until", false))).unwrap();
    let check = |rules: &[&str]| {
        plan_of(
            vec![Op::CheckConsistency {
                project: Some(project.id.clone()),
                rules: rules.iter().map(|r| r.to_string()).collect(),
            }],
            Scope::default(),
        )
    };
    let last_run = || w.db.with(|c| projects_repo::last_run(c, &project.id)).unwrap();
    // The full check runs every enabled rule.
    let ex = run(&w, &check(&[]));
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    let full = last_run().expect("stored");
    assert_eq!(full.results.len(), 2);
    // "계약금액 규칙만 검사해줘": words of the description, or the id.
    let ex = run(&w, &check(&["계약금액 규칙"]));
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert_eq!(ex.steps[0].payload["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(ex.steps[0].payload["results"][0]["rule_id"], json!(amount.id));
    assert!(ex.steps[0].summary.contains("규칙 1개만 검사"), "{}", ex.steps[0].summary);
    assert_eq!(preview(&w, &check(&["계약금액 규칙"])).steps[0].payload["rules"], json!([amount.id]));
    let ex = run(&w, &check(&[date.id.as_str()]));
    assert_eq!(ex.steps[0].payload["results"][0]["rule_id"], json!(date.id));
    // A reference that matches no rule (or only a disabled one) fails; nothing is stored.
    let stored = last_run().map(|r| r.id);
    for (reference, code) in
        [("없는 규칙", "rule_unknown"), ("r-does-not-exist", "rule_unknown"), ("유효기간", "rule_disabled")]
    {
        let ex = run(&w, &check(&[reference]));
        assert_eq!(ex.steps[0].status, StepStatus::Failed, "{reference}: {:?}", ex.steps[0]);
        assert!(
            ex.steps[0].error.as_deref().unwrap_or_default().contains(code),
            "{reference}: {:?}",
            ex.steps[0].error
        );
        assert_eq!(last_run().map(|r| r.id), stored, "{reference}: the latest check was not replaced");
    }
    assert!(preview(&w, &check(&["없는 규칙"])).steps[0].summary.contains("찾을 수 없음"));
}

/// GEN-3 in the executor: `revise_draft` stored its result over the draft unconditionally, so a
/// cancelled "초안 전체 다시 써줘" through the assistant replaced the complete draft with the
/// sections written so far. The result now goes through `store::save_job_result`.
#[test]
fn a_cancelled_revision_keeps_the_complete_draft() {
    use std::sync::atomic::AtomicUsize;

    use crate::engine::{ChatRequest, ChatResponse};
    use crate::generate::templates::TemplateSpec;
    use crate::generate::{self, Draft, DraftBlock, DraftRequest, TemplateKind, TemplateRef};
    use crate::knowledge::answer::ChatBackend;

    /// Sets the cancel flag while the model answers its `at`-th call.
    struct CancelAt<'a> {
        inner: &'a StageChat,
        cancel: &'a AtomicBool,
        at: usize,
        calls: AtomicUsize,
    }
    impl CancelAt<'_> {
        fn called(&self) {
            if self.calls.fetch_add(1, Ordering::Relaxed) + 1 == self.at {
                self.cancel.store(true, Ordering::Relaxed);
            }
        }
    }
    impl ChatBackend for CancelAt<'_> {
        fn model_id(&self) -> Option<String> {
            self.inner.model_id()
        }
        fn chat_stream(
            &self,
            req: &ChatRequest,
            on_delta: &mut dyn FnMut(&str) -> bool,
        ) -> crate::error::Result<ChatResponse> {
            self.called();
            self.inner.chat_stream(req, on_delta)
        }
        fn complete(&self, req: &ChatRequest) -> crate::error::Result<ChatResponse> {
            self.called();
            self.inner.complete(req)
        }
    }

    let w = World::new();
    // A complete progress report, drafted section by section (a budget above the split threshold).
    let headings = TemplateSpec::builtin(TemplateKind::ProgressReport).sections(true).to_vec();
    assert!(headings.len() >= 3, "{headings:?}");
    let mut complete = Draft {
        id: "dr-full".into(),
        title: "11월 2주차 진척 보고서".into(),
        template: TemplateRef::Builtin { template: TemplateKind::ProgressReport },
        doc_type: Some("progress_report".into()),
        request: Some(Box::new(DraftRequest {
            form_ref: Some("progress_report".into()),
            content: "한빛 포털 구축 진척 보고서".into(),
            max_tokens: Some(8_000),
            use_knowledge: false,
            ..Default::default()
        })),
        ..Default::default()
    };
    for heading in &headings {
        complete.body.push(DraftBlock::Heading { level: 2, text: heading.clone() });
        complete.body.push(DraftBlock::Paragraph { text: format!("{heading} — 원래 본문"), source: None });
    }
    generate::store::save(&w.paths, &complete).unwrap();
    let para = |t: &str| json!({ "kind": "paragraph", "text": t, "items": [], "source_index": null });
    w.chat.push(
        "draft_document",
        json!({
            "title": "11월 2주차 진척 보고서",
            "fields": [],
            "tables": [],
            "body": [{ "kind": "heading", "text": headings[0], "items": [], "source_index": null }, para("새 개요")],
            "notes": []
        }),
    );
    w.chat.push("draft_section", json!({ "blocks": [para("새 본문")] }));
    let plan = plan_of(
        vec![Op::ReviseDraft {
            draft_id: Some("dr-full".into()),
            scope: "whole".into(),
            section: None,
            heading: None,
            instruction: "더 간결하게".into(),
        }],
        Scope::default(),
    );
    let sections = |id: &str| generate::sections_of(&generate::store::load(&w.paths, id).unwrap().body).len();
    let first_paragraph = |id: &str| {
        generate::store::load(&w.paths, id).unwrap().body.into_iter().find_map(|b| match b {
            DraftBlock::Paragraph { text, .. } => Some(text),
            _ => None,
        })
    };

    // Cancelled while the second section is written: the step is cancelled, the complete draft
    // stays and what was written so far is stored next to it.
    let cancel = AtomicBool::new(false);
    let chat = CancelAt { inner: &w.chat, cancel: &cancel, at: 2, calls: AtomicUsize::new(0) };
    let mut deps = w.deps(Some(&cancel));
    deps.chat = Some(&chat);
    let ex = exec::run(&deps, &plan, &mut |_| {}).unwrap();
    assert_eq!(ex.steps[0].status, StepStatus::Cancelled, "{:?}", ex.steps[0]);
    assert_eq!(sections("dr-full"), headings.len(), "the complete draft is still stored");
    assert_eq!(first_paragraph("dr-full"), Some(format!("{} — 원래 본문", headings[0])));
    let listed = generate::store::list(&w.paths).unwrap();
    assert_eq!(listed.len(), 2, "{listed:?}");
    let partial = listed.iter().find(|d| d.id != "dr-full").expect("the partial result").id.clone();
    assert!(sections(&partial) < headings.len());
    let notes = generate::store::load(&w.paths, &partial).unwrap().notes;
    assert!(notes[0].starts_with("취소된 작업이 남긴 결과입니다"), "{notes:?}");

    // Not cancelled: the revision replaces the draft it started from, and the payload says so.
    let ex = run(&w, &plan);
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert_eq!(ex.steps[0].payload["draft_store"], json!({ "status": "saved" }));
    assert_eq!(ex.steps[0].payload["draft"]["id"], json!("dr-full"));
    assert_eq!(first_paragraph("dr-full").as_deref(), Some("새 개요"));
    assert_eq!(generate::store::list(&w.paths).unwrap().len(), 2);
}

/// CON-3: the 보완요청서 the assistant writes for a project carries that project in its request,
/// so the draft — and a later regeneration from the stored request — searches inside it.
#[test]
fn a_supplement_request_is_bound_to_its_project() {
    let w = World::new();
    let project =
        w.db.with(|c| projects_repo::save_project(c, &ProjectInput { name: "한빛 납품".into(), ..Default::default() }))
            .unwrap();
    w.db.with(|c| {
        projects_repo::save_rule(
            c,
            &project.id,
            &Rule {
                description: "견적서 작성일자는 필수".into(),
                left: RuleOperand::field("estimate", "issue_date"),
                op: RuleOp::Present,
                enabled: true,
                ..Rule::default()
            },
        )
    })
    .unwrap();
    let check =
        plan_of(vec![Op::CheckConsistency { project: Some(project.id.clone()), rules: Vec::new() }], Scope::default());
    assert_eq!(run(&w, &check).steps[0].status, StepStatus::Done);
    let plan = plan_of(
        vec![Op::RequestSupplement { project: Some(project.id.clone()), findings: Vec::new(), output: None }],
        Scope::default(),
    );
    // No model: the findings make a rule-composed draft.
    let mut deps = w.deps(None);
    deps.chat = None;
    deps.model = None;
    let ex = exec::run(&deps, &plan, &mut |_| {}).unwrap();
    assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
    assert_eq!(ex.steps[0].payload["project_id"], json!(project.id));
    assert_eq!(ex.steps[0].payload["draft"]["request"]["project_id"], json!(project.id));
}
