//! Planner tests: the design's example commands through the rules / pre-router
//! (no model), the appendix-D spec ↔ serde agreement, validator rejections and
//! normalisation, and the model path with one re-query.

use chrono::NaiveDate;
use serde_json::{json, Value};

use crate::pipeline::vlm::testing::StageChat;
use crate::prerouter::DateRange;
use crate::settings::{Language, Profile, Theme};

use super::llm::{self, plan, PlannerDeps};
use super::prompts;
use super::rules;
use super::schema::{self, ParamType, OP_SPECS};
use super::validate;
use super::*;

fn field(key: &str, ko: &str, en: &str, kind: &str) -> FieldSummary {
    FieldSummary { key: key.into(), label_ko: ko.into(), label_en: en.into(), kind: kind.into() }
}

fn ctx() -> PlanContext {
    PlanContext {
        today: NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(),
        language: Language::Ko,
        folders: vec!["inbox".into(), "documents/2026/10".into()],
        doc_types: vec![
            DocTypeSummary { slug: "receipt".into(), label_ko: "영수증".into(), label_en: "Receipt".into() },
            DocTypeSummary {
                slug: "tax_invoice".into(),
                label_ko: "세금계산서".into(),
                label_en: "Tax invoice".into(),
            },
            DocTypeSummary {
                slug: "business_registration".into(),
                label_ko: "사업자등록증".into(),
                label_en: "Business registration".into(),
            },
            DocTypeSummary { slug: "contract".into(), label_ko: "계약서".into(), label_en: "Contract".into() },
            DocTypeSummary { slug: "quotation".into(), label_ko: "견적서".into(), label_en: "Quotation".into() },
        ],
        schemas: vec![
            SchemaSummary {
                id: "s-receipt".into(),
                name: "영수증".into(),
                name_en: "Receipt".into(),
                doc_type: "receipt".into(),
                fields: vec![
                    field("store_name", "상호", "Store", "string"),
                    field("datetime", "일시", "Date / time", "date"),
                    field("supply_amount", "공급가액", "Supply amount", "amount"),
                    field("vat", "부가세", "VAT", "amount"),
                    field("total", "합계", "Total", "amount"),
                    field("approval_no", "승인번호", "Approval no.", "string"),
                ],
            },
            SchemaSummary {
                id: "s-tax".into(),
                name: "세금계산서".into(),
                name_en: "Tax invoice".into(),
                doc_type: "tax_invoice".into(),
                fields: vec![
                    field("issue_date", "작성일자", "Issue date", "date"),
                    field("supply_total", "공급가액 합계", "Supply total", "amount"),
                    field("tax_total", "세액 합계", "Tax total", "amount"),
                    field("grand_total", "합계금액", "Grand total", "amount"),
                    field("nts_approval_no", "국세청 승인번호", "NTS approval no.", "string"),
                ],
            },
            SchemaSummary {
                id: "s-brn".into(),
                name: "사업자등록증".into(),
                name_en: "Business registration".into(),
                doc_type: "business_registration".into(),
                fields: vec![
                    field("registration_no", "등록번호", "Registration no.", "brn"),
                    field("company_name", "상호(법인명)", "Company name", "string"),
                ],
            },
        ],
        tags: vec!["경비".into()],
        current_folder: Some("inbox".into()),
        selected_doc_ids: Vec::new(),
        current_document: Some("d1a2".into()),
        current_page: Some(1),
        document_count: 42,
        model_ids: vec!["qwen3.5-4b-q6k".into(), "qwen3.5-9b-q4km".into()],
        projects: Vec::new(),
        current_project: None,
    }
}

fn kinds(p: &Plan) -> Vec<OpKind> {
    p.operations.iter().map(Op::kind).collect()
}

fn planned(text: &str) -> PlanResult {
    plan(None, text, &ctx()).unwrap()
}

#[test]
fn example_commands_plan_without_a_model() {
    // 1. inbox → classify → ocr → extract(receipt) → move (confirmation)
    let r = planned("inbox 폴더의 영수증을 전부 처리하고 월별 폴더로 정리해줘");
    assert_eq!(r.source, PlanSource::Rules, "{r:?}");
    assert_eq!(kinds(&r.plan), [OpKind::Import, OpKind::Classify, OpKind::Ocr, OpKind::Extract, OpKind::Move]);
    assert_eq!(r.plan.scope.folder.as_deref(), Some("inbox"));
    assert_eq!(r.plan.scope.filter.doc_type.as_deref(), Some("receipt"));
    assert!(
        matches!(&r.plan.operations[4], Op::Move { folder, group_by: Some(GroupBy::Month), .. } if folder.contains("{{year}}/{{month}}"))
    );
    assert!(r.plan.requires_confirmation);
    assert!(!has_errors(&r.issues));

    // 2. extract three fields of tax invoices, then export as XLSX
    let r = planned("세금계산서에서 공급가액·세액·합계만 뽑아서 엑셀로");
    assert_eq!(kinds(&r.plan), [OpKind::Extract, OpKind::Export]);
    assert_eq!(r.plan.scope.filter.doc_type.as_deref(), Some("tax_invoice"));
    match &r.plan.operations[0] {
        Op::Extract { schema_ref, fields, .. } => {
            assert_eq!(schema_ref.as_deref(), Some("s-tax"));
            assert_eq!(fields, &["supply_total", "tax_total", "grand_total"]);
        }
        o => panic!("{o:?}"),
    }
    assert!(
        matches!(&r.plan.operations[1], Op::Export { format: ExportFormat::Xlsx, columns, .. } if columns.len() == 3)
    );
    assert!(!r.plan.requires_confirmation);

    // 3. a question about the open contract → answer on that document
    let r = planned("이 계약서의 계약기간과 위약금 조항 알려줘");
    assert_eq!(kinds(&r.plan), [OpKind::Answer]);
    assert_eq!(r.plan.scope.doc_ids, vec!["d1a2".to_string()]);

    // 4. search (Korean / English)
    let r = planned("거래처명이 ㈜한빛인 문서 찾아줘");
    assert!(matches!(&r.plan.operations[0], Op::Search { query, .. } if query == "㈜한빛"), "{r:?}");
    let r = planned("find documents from Hanbit");
    assert!(matches!(&r.plan.operations[0], Op::Search { query, .. } if query == "Hanbit"), "{r:?}");

    // 5. schema modification → define_schema (confirmed on the schema screen)
    let r = planned("사업자등록증 스키마에 '사업자단위과세' 필드 추가해");
    assert!(
        matches!(&r.plan.operations[0], Op::DefineSchema { mode: Some(SchemaMode::Modify), base: Some(b), .. } if b == "사업자등록증"),
        "{r:?}"
    );
    assert!(r.plan.requires_confirmation);

    // 6. two settings in one sentence
    let r = planned("다크 테마로 바꾸고 모델을 9B로 올려");
    assert_eq!(kinds(&r.plan), [OpKind::SetTheme, OpKind::SetModel], "{r:?}");
    assert!(matches!(&r.plan.operations[0], Op::SetTheme { theme: Theme::Dark }));
    assert!(matches!(&r.plan.operations[1], Op::SetModel { id } if id == "qwen3.5-9b-q4km"));

    // 7. relative date + total → aggregate with the resolved range
    let r = planned("지난달 영수증 합계 얼마야?");
    assert!(
        matches!(&r.plan.operations[0], Op::Aggregate { func: AggregateFn::Sum, field: Some(f), .. } if f == "total"),
        "{r:?}"
    );
    assert_eq!(r.plan.scope.filter.doc_type.as_deref(), Some("receipt"));
    assert_eq!(r.plan.scope.filter.date_from.as_deref(), Some("2026-10-01"));
    assert_eq!(r.plan.scope.filter.date_to.as_deref(), Some("2026-10-31"));
    assert_eq!(r.date.as_ref().map(|d| d.label.as_str()), Some("last_month"));

    // 8. re-OCR the selection at the fine-print budget on the current page
    let r = planned("선택 영역 다시 OCR 해줘");
    assert!(
        matches!(&r.plan.operations[0], Op::Ocr { pages, profile: Some(OcrProfile::FinePrint), force: Some(true) } if pages == &[1]),
        "{r:?}"
    );
    assert_eq!(r.plan.scope.doc_ids, vec!["d1a2".to_string()]);

    // 9. undo
    let r = planned("방금 이름 바꾼 거 되돌려");
    assert_eq!(kinds(&r.plan), [OpKind::Undo]);
    assert!(r.plan.requires_confirmation);

    // 10. job control (both languages)
    assert!(matches!(&planned("작업 일시정지").plan.operations[0], Op::Job { action: JobOp::Pause, .. }));
    assert!(matches!(&planned("pause the job").plan.operations[0], Op::Job { action: JobOp::Pause, .. }));

    // 11-12. the example chips
    assert_eq!(kinds(&planned("inbox 처리").plan), [OpKind::Import, OpKind::Classify, OpKind::Ocr, OpKind::Extract]);
    let r = planned("세금계산서 → 엑셀");
    assert!(
        matches!(&r.plan.operations[0], Op::Export { format: ExportFormat::Xlsx, columns, .. } if columns.is_empty()),
        "{r:?}"
    );
    assert_eq!(r.plan.scope.filter.doc_type.as_deref(), Some("tax_invoice"));

    // pre-router commands still go straight through
    let r = planned("/테마 다크");
    assert_eq!(r.source, PlanSource::Prerouter);
    assert!(matches!(&r.plan.operations[0], Op::SetTheme { theme: Theme::Dark }));
    let r = planned("설정 열어줘");
    assert!(
        matches!(&r.plan.operations[0], Op::Open { target: OpenTarget::Screen, reference } if reference == "settings")
    );

    // A short topic without a command verb is a knowledge-base lookup (P3 behaviour kept).
    let r = planned("xyzzy 플루토늄 원자로");
    assert!(matches!(&r.plan.operations[0], Op::Answer { .. }), "{r:?}");
    assert!(r.plan.confidence >= MIN_CONFIDENCE);
    // A bare question about an amount is Q&A, not a library-wide aggregate.
    let r = planned("합계 금액은?");
    assert_eq!(kinds(&r.plan), [OpKind::Answer], "{r:?}");
    let r = planned("전체 영수증 합계");
    assert!(matches!(&r.plan.operations[0], Op::Aggregate { func: AggregateFn::Sum, .. }), "{r:?}");

    // more single-intent phrases
    let r = planned("영수증 몇 건이야?");
    assert!(matches!(&r.plan.operations[0], Op::Aggregate { func: AggregateFn::Count, field: None, .. }), "{r:?}");
    let r = planned("선택 문서에 태그 경비, 10월 추가해줘");
    assert!(matches!(&r.plan.operations[0], Op::Tag { add, .. } if add == &["경비", "10월"]), "{r:?}");
    assert_eq!(r.plan.scope.doc_ids, vec!["d1a2".to_string()]);
    let r = planned("이 문서 요약해줘");
    assert!(matches!(&r.plan.operations[0], Op::Summarize { target: Some(SummarizeTarget::EachDoc), .. }), "{r:?}");
    let r = planned("검토 완료된 영수증 거래처별 폴더로 옮겨");
    assert!(
        matches!(&r.plan.operations[0], Op::Move { group_by: Some(GroupBy::Vendor), only_reviewed: Some(true), .. }),
        "{r:?}"
    );
    let r = planned("작년 계약서 전부 삭제");
    assert!(matches!(&r.plan.operations[0], Op::Delete { .. }), "{r:?}");
    assert_eq!(r.plan.scope.filter.date_from.as_deref(), Some("2025-01-01"));
    // a deletion without any scope asks first
    let mut c = ctx();
    c.current_document = None;
    let r = plan(None, "문서 삭제해", &c).unwrap();
    assert!(r.plan.is_clarification(), "{r:?}");
}

#[test]
fn p6_rules_plan_drafts_projects_explanations_and_retention() {
    use super::{ProjectSummary, RetentionTarget};
    use crate::generate::OutputFormat;
    let mut c = ctx();
    c.projects = vec![
        ProjectSummary { id: "p-hanbit".into(), name: "한빛 심사".into(), status: "intake".into() },
        ProjectSummary { id: "p-acme".into(), name: "Acme onboarding".into(), status: "validating".into() },
    ];
    c.current_project = Some("p-hanbit".into());
    let planned = |text: &str| plan(None, text, &c).unwrap();

    // Drafting: a built-in template picked from the keyword, the whole sentence as content.
    let r = planned("등록된 견적서 양식으로 ㈜한빛에 노트북 10대, 대당 1,200,000원, 유효기간 30일 견적서 만들어줘");
    assert_eq!(kinds(&r.plan), [OpKind::DraftDocument], "{r:?}");
    assert!(
        matches!(&r.plan.operations[0], Op::DraftDocument { form_ref, content, .. } if form_ref.as_deref() == Some("quotation") && content.contains("노트북"))
    );
    assert!(r.plan.requires_confirmation);
    assert!(!has_errors(&r.issues), "{:?}", r.issues);
    // Rewrite the open contract → the selection is a source.
    let r = planned("이 계약서 내용으로 갑·을 바꿔서 계약서 다시 써줘");
    assert!(
        matches!(&r.plan.operations[0], Op::DraftDocument { form_ref, sources, .. } if form_ref.as_deref() == Some("contract") && sources == &["d1a2".to_string()]),
        "{r:?}"
    );
    // A report from the aggregate card, as PDF: aggregate the receipts first, then draft from the totals.
    let r = planned("지난달 영수증 집계 결과로 경비 보고서 초안 작성해줘 pdf로");
    assert_eq!(kinds(&r.plan), [OpKind::Aggregate, OpKind::DraftDocument], "{r:?}");
    assert_eq!(r.plan.scope.filter.doc_type.as_deref(), Some("receipt"));
    assert_eq!(r.plan.scope.filter.date_from.as_deref(), Some("2026-10-01"));
    assert!(
        matches!(&r.plan.operations[1], Op::DraftDocument { form_ref, sources, output: Some(OutputFormat::Pdf), .. } if form_ref.as_deref() == Some("report") && sources.contains(&"aggregate".to_string())),
        "{r:?}"
    );
    // Field labels that contain 작성 do not trigger drafting; organising wins.
    let r = planned("계약서 작성일자별로 정리해줘");
    assert!(!matches!(&r.plan.operations[0], Op::DraftDocument { .. }), "{r:?}");

    // Projects.
    let r = planned("'한빛 2차 심사' 프로젝트 만들어줘");
    assert!(matches!(&r.plan.operations[0], Op::CreateProject { name, .. } if name == "한빛 2차 심사"), "{r:?}");
    let r = planned("create project Acme renewal");
    assert!(matches!(&r.plan.operations[0], Op::CreateProject { name, .. } if name == "Acme renewal"), "{r:?}");
    let r = planned("선택한 문서를 한빛 심사 프로젝트에 추가해");
    assert!(matches!(&r.plan.operations[0], Op::AddToProject { project: Some(p), .. } if p == "p-hanbit"), "{r:?}");
    assert_eq!(r.plan.scope.doc_ids, vec!["d1a2".to_string()]);
    let r = planned("정합성 검사 돌려줘");
    assert!(
        matches!(&r.plan.operations[0], Op::CheckConsistency { project: Some(p), .. } if p == "p-hanbit"),
        "the open project is filled in: {r:?}"
    );
    assert!(!has_errors(&r.issues), "{:?}", r.issues);
    let r = planned("세금계산서의 공급자 사업자번호는 사업자등록증 번호와 같아야 해");
    assert!(
        matches!(&r.plan.operations[0], Op::DefineRule { description, .. } if description.contains("같아야")),
        "{r:?}"
    );
    assert!(r.plan.requires_confirmation);
    let r = planned("보완요청서 만들어줘");
    assert!(matches!(&r.plan.operations[0], Op::RequestSupplement { .. }), "{r:?}");
    // Without an open project the project ops ask for one.
    let mut c2 = c.clone();
    c2.current_project = None;
    let r = plan(None, "정합성 검사 돌려줘", &c2).unwrap();
    assert!(r.plan.is_clarification() || has_errors(&r.issues), "{r:?}");

    // Explain / compare.
    let r = planned("이 문장 설명해줘");
    assert!(matches!(&r.plan.operations[0], Op::Explain { .. }), "{r:?}");
    let mut c3 = c.clone();
    c3.selected_doc_ids = vec!["a".into(), "b".into()];
    let r = plan(None, "선택한 두 문서 비교해줘", &c3).unwrap();
    assert!(matches!(&r.plan.operations[0], Op::Compare { doc_ids, .. } if doc_ids.len() == 2), "{r:?}");
    let r = planned("이 두 문서 비교해줘");
    assert!(r.plan.is_clarification(), "one selected document: {r:?}");

    // Retention and form registration (UI ops).
    let r = planned("저널은 30일만 보관해");
    assert!(matches!(&r.plan.operations[0], Op::SetRetention { item: RetentionTarget::Journal, days: 30 }), "{r:?}");
    let r = planned("keep chat history for 6 months");
    assert!(matches!(&r.plan.operations[0], Op::SetRetention { item: RetentionTarget::Chats, days: 180 }), "{r:?}");
    let r = planned("원본 문서는 무기한 보존");
    assert!(matches!(&r.plan.operations[0], Op::SetRetention { item: RetentionTarget::Documents, days: -1 }));
    let r = planned("이 문서를 '휴가신청서' 양식으로 등록해");
    assert!(
        matches!(&r.plan.operations[0], Op::RegisterForm { doc_ref: Some(d), name: Some(n), .. } if d == "d1a2" && n == "휴가신청서"),
        "{r:?}"
    );
    assert!(OpKind::RegisterForm.runs_in_ui() && OpKind::SetRetention.runs_in_ui());
}

#[test]
fn p6_rules_ignore_statements_and_questions() {
    use super::{ProjectSummary, SettingKey};
    let mut c = ctx();
    c.projects = vec![
        ProjectSummary { id: "p-hanbit".into(), name: "한빛 심사".into(), status: "intake".into() },
        ProjectSummary { id: "p-acme".into(), name: "Acme onboarding".into(), status: "validating".into() },
    ];
    c.current_project = Some("p-hanbit".into());
    let not = |text: &str, bad: &[OpKind]| {
        let r = plan(None, text, &c).unwrap();
        let ks = kinds(&r.plan);
        assert!(!ks.iter().any(|k| bad.contains(k)), "{text} → {ks:?} ({r:?})");
    };
    use OpKind::*;
    // Retention statements / questions never become set_retention (the purge chain starts there).
    for s in [
        "문서 3년 보관 의무가 있나요?",
        "문서 90일 보관했어?",
        "원본 문서는 5년 보존이 법적 의무야",
        "추출 결과 30일 지나면 삭제돼?",
        "채팅 기록은 6개월 보관이 기본이지?",
        "임시로 저장한 거 지워지면 안 돼, 계속 보관해",
    ] {
        not(s, &[SetRetention]);
    }
    // Project chatter is not project creation.
    for s in [
        "새 프로젝트 매니저가 왔어",
        "케이스 스터디 자료 만들어줘",
        "프로젝트 새로 시작하자",
        "start the project kickoff meeting notes",
        "create a project plan for Q1",
    ] {
        not(s, &[CreateProject]);
    }
    // Questions about rules / results do not define rules or re-run checks.
    for s in [
        "세금계산서 필수 항목이 뭐야?",
        "견적서 유효기간은 보통 30일 이내여야 해?",
        "the contract amount must match the quotation, does it?",
        "정합성 검사 결과 알려줘",
        "the supplement request must be sent by Friday",
    ] {
        not(s, &[DefineRule, CheckConsistency, RequestSupplement]);
    }
    // Settings chatter.
    not("해상도 300으로 스캔했어", &[SetSetting]);
    not("인덴스 펀드 다시 알아봐줘", &[Reindex]);
    not("인덱스 펀드 다시 알아봐줘", &[Reindex]);
    not("양식 등록은 어디서 해?", &[RegisterForm]);
    not("양식 등록 화면 열어줘", &[RegisterForm]);
    // Drafting needs a template or an explicit document request.
    not("내일 회의 자료 만들어줘", &[DraftDocument]);
    not("write down the total of last month's receipts", &[DraftDocument]);
    not("draft folder is empty", &[DraftDocument]);
    // Still recognised: explicit commands.
    let r = plan(None, "한빛 심사 프로젝트에 이 문서 추가해줘", &c).unwrap();
    assert!(matches!(&r.plan.operations[0], Op::AddToProject { project: Some(p), .. } if p == "p-hanbit"), "{r:?}");
    let r = plan(None, "'한빛 2차 심사' 프로젝트 만들어줘", &c).unwrap();
    assert!(r.plan.requires_confirmation && matches!(&r.plan.operations[0], Op::CreateProject { .. }), "{r:?}");
    let r = plan(None, "검색 색인 다시 만들어줘", &c).unwrap();
    assert!(matches!(&r.plan.operations[0], Op::Reindex { .. }), "{r:?}");
    let r = plan(None, "DPI를 300으로 바꿔줘", &c).unwrap();
    assert!(matches!(&r.plan.operations[0], Op::SetSetting { key: SettingKey::RenderDpi, .. }), "{r:?}");
}

#[test]
fn rules_do_not_hijack_questions_or_everyday_words() {
    // Latin triggers are whole words, questions stay questions, nouns are not verbs.
    let answerish = |text: &str| {
        let r = planned(text);
        assert!(
            matches!(&r.plan.operations[0], Op::Answer { .. } | Op::Search { .. } | Op::Clarify { .. }),
            "{text} → {:?}",
            r.plan.operations
        );
        r
    };
    assert!(
        matches!(&answerish("find the shareholder agreement").plan.operations[0], Op::Search { query, .. } if query.contains("shareholder"))
    );
    answerish("find my resume");
    answerish("이어서 설명해줘");
    answerish("엑셀로 된 파일도 가져올 수 있어?");
    answerish("can I import spreadsheets into the workspace?");
    answerish("inbox에 뭐가 들어있는지 전부 알려줘");
    answerish("이 문서 읽어줘");
    answerish("이 계약서 내용 정리해서 알려줘");
    answerish("지난달 회의록 정리해서 알려줘");
    answerish("이 문서 삭제하면 되돌릴 수 있어?");
    answerish("what percentage of receipts failed validation?");
    // "remove these documents" is a deletion (with a selection), not a move.
    let r = planned("remove these documents");
    assert!(matches!(&r.plan.operations[0], Op::Delete { .. }), "{r:?}");
    // Tagging without any target asks instead of tagging the whole library.
    let mut c = ctx();
    c.current_document = None;
    let r = plan(None, "태그 중요 추가해", &c).unwrap();
    assert!(r.plan.is_clarification(), "{r:?}");
    // "현재 폴더 문서 분류해줘" is not a forced re-run.
    let r = planned("여기 있는 문서 분류해줘");
    assert!(matches!(&r.plan.operations[0], Op::Classify { force: None, .. }), "{r:?}");
    // Whole-library edits need the explicit word; "pause the job" still works.
    assert!(matches!(&planned("pause the job").plan.operations[0], Op::Job { action: JobOp::Pause, .. }));
    let r = planned("모든 문서에 태그 2026 추가해");
    assert!(matches!(&r.plan.operations[0], Op::Tag { .. }), "{r:?}");
    assert!(rules::mentions_all("모든 문서에 태그 2026 추가해"));
}

#[test]
fn validator_requires_scope_confirms_folder_imports_and_keeps_exports_in_exports() {
    let c = ctx();
    // Document-changing ops with an empty scope are rejected unless the text says "all".
    let p = plan_of(json!([{ "op": "set_doc_type", "doc_type": "receipt" }]), json!({}));
    assert_eq!(validate::scope_required(&p).map(|i| i.code), Some("scope_required".to_string()));
    let p = plan_of(json!([{ "op": "organize", "dry_run": true }]), json!({}));
    assert!(validate::scope_required(&p).is_none());
    let p = plan_of(json!([{ "op": "tag", "add": ["x"] }]), json!({ "doc_ids": ["d1"] }));
    assert!(validate::scope_required(&p).is_none());
    // Through the planner: a model plan without a scope becomes a clarification.
    let chat = StageChat::new();
    chat.push("plan", json!({ "summary": "x", "intent": "set_doc_type", "scope": {}, "operations": [{ "op": "set_doc_type", "doc_type": "receipt" }], "requires_confirmation": false, "confidence": 0.9 }));
    let r = plan(Some(&deps(&chat)), "한빛과 거래한 서류들 유형을 영수증으로 설정해", &c).unwrap();
    assert_eq!(r.source, PlanSource::Model, "{r:?}");
    assert!(r.plan.is_clarification(), "{r:?}");
    // Folder imports always get the card; export paths must stay under exports/.
    let mut p = plan_of(json!([{ "op": "import", "source": "folder", "path": "C:/Users/me/Scans" }]), json!({}));
    let issues = validate::validate(&mut p, &c, None);
    assert!(!has_errors(&issues), "{issues:?}");
    assert!(p.requires_confirmation);
    let mut p = plan_of(
        json!([{ "op": "export", "format": "pdf_searchable", "path": "documents/2026/10/계약서.pdf" }]),
        json!({}),
    );
    assert!(validate::validate(&mut p, &c, None).iter().any(|i| i.code == "path_outside" && i.blocking));
    let mut p = plan_of(json!([{ "op": "export", "format": "csv", "path": "exports/영수증" }]), json!({}));
    assert!(!has_errors(&validate::validate(&mut p, &c, None)));
    assert!(matches!(&p.operations[0], Op::Export { path: Some(path), .. } if path == "exports/영수증.csv"));
}

#[test]
fn unknown_commands_without_a_model_ask_for_clarification() {
    let r = planned("한빛과 거래한 계약 중 작년 것만 골라서 보여줘");
    assert_eq!(r.source, PlanSource::None);
    assert!(r.plan.is_clarification());
    assert_eq!(r.issues[0].code, "no_model");
}

fn sample(ty: ParamType) -> Value {
    match ty {
        ParamType::Str => json!("x"),
        ParamType::Bool => json!(true),
        ParamType::Int => json!(3),
        ParamType::Number => json!(0.5),
        ParamType::Date => json!("2026-10-01"),
        ParamType::Enum(v) => json!(v[0]),
        ParamType::StrList => json!(["a", "b"]),
        ParamType::IntList => json!([1, 2]),
        ParamType::EnumList(v) => json!([v[0]]),
        ParamType::Scalar => json!(200),
    }
}

#[test]
fn specs_match_serde_and_the_grammar_schema() {
    assert_eq!(OP_SPECS.len(), OpKind::ALL.len());
    for spec in OP_SPECS {
        let mut obj = serde_json::Map::new();
        obj.insert("op".into(), json!(spec.kind.as_str()));
        for p in spec.params {
            obj.insert(p.name.into(), sample(p.ty));
        }
        let op: Op = serde_json::from_value(Value::Object(obj.clone()))
            .unwrap_or_else(|e| panic!("{}: {e}", spec.kind.as_str()));
        assert_eq!(op.kind(), spec.kind);
        // Serialised keys are a subset of the spec (no hidden parameters).
        let back = serde_json::to_value(&op).unwrap();
        for key in back.as_object().unwrap().keys() {
            assert!(
                key == "op" || spec.params.iter().any(|p| p.name == key),
                "{}: {key} not in spec",
                spec.kind.as_str()
            );
        }
        // Required parameters are really required.
        for p in spec.params.iter().filter(|p| p.required) {
            let mut without = obj.clone();
            without.remove(p.name);
            assert!(
                serde_json::from_value::<Op>(Value::Object(without)).is_err(),
                "{}.{} should be required",
                spec.kind.as_str(),
                p.name
            );
        }
    }
    // Unknown parameters are rejected (appendix D is the single source of truth).
    assert!(serde_json::from_value::<Op>(json!({ "op": "search", "query": "x", "limit": 5 })).is_err());
    assert!(serde_json::from_value::<Plan>(json!({ "intent": "search", "operations": [], "extra": 1 })).is_err());
    let s = schema::plan_schema();
    assert_eq!(s["properties"]["operations"]["items"]["anyOf"].as_array().unwrap().len(), OP_SPECS.len());
    assert_eq!(s["properties"]["operations"]["maxItems"], json!(MAX_OPS));
    assert!(s["properties"]["intent"]["enum"].as_array().unwrap().len() == OpKind::ALL.len());
    assert!(schema::op_table(true, false).contains("- aggregate:"));
    assert!(schema::op_table(false, true).contains("fn:sum|count|avg|min|max"));
}

fn plan_of(ops: Value, scope: Value) -> Plan {
    validate::parse_plan(&json!({
        "summary": "s", "intent": "search", "scope": scope, "operations": ops, "requires_confirmation": false, "confidence": 0.9
    }))
    .unwrap()
}

#[test]
fn validator_rejects_and_normalises() {
    let c = ctx();
    // too many ops
    let many: Vec<Value> = (0..13).map(|_| json!({ "op": "noop" })).collect();
    let mut p = plan_of(json!(many), json!({}));
    let issues = validate::validate(&mut p, &c, None);
    assert!(issues.iter().any(|i| i.code == "ops_many" && i.blocking));
    assert_eq!(p.operations.len(), MAX_OPS);
    // bad dates / order / unknown status / outside path / clarify mixed
    let mut p = plan_of(
        json!([{ "op": "export", "format": "csv", "path": "../out.csv" }, { "op": "clarify", "question": "?" }]),
        json!({ "filter": { "date_from": "2026-13-01", "date_to": "2026-01-01", "status": "weird" } }),
    );
    let issues = validate::validate(&mut p, &c, None);
    let codes: Vec<&str> = issues.iter().map(|i| i.code.as_str()).collect();
    for code in ["date_format", "status_unknown", "path_outside", "clarify_mixed"] {
        assert!(codes.contains(&code), "{code} missing in {codes:?}");
    }
    let mut p = plan_of(json!([{ "op": "export", "format": "xlsx", "path": "C:/tmp/out" }]), json!({}));
    assert!(validate::validate(&mut p, &c, None).iter().any(|i| i.code == "path_outside"));
    let mut p = plan_of(json!([{ "op": "move", "folder": "../../etc" }]), json!({}));
    assert!(validate::validate(&mut p, &c, None).iter().any(|i| i.code == "path_outside"));
    let mut p = plan_of(json!([{ "op": "rename", "template": "{{yr}}_{{vendor}}" }]), json!({}));
    assert!(validate::validate(&mut p, &c, None).iter().any(|i| i.code == "template_variable"));
    let mut p = plan_of(
        json!([{ "op": "aggregate", "fn": "sum", "field": "없는필드" }]),
        json!({ "filter": { "doc_type": "영수증" } }),
    );
    assert!(validate::validate(&mut p, &c, None).iter().any(|i| i.code == "field_unknown" && i.blocking));
    let mut p = plan_of(json!([{ "op": "set_setting", "key": "render_dpi", "value": 123 }]), json!({}));
    assert!(validate::validate(&mut p, &c, None).iter().any(|i| i.code == "setting_value"));
    let mut p = plan_of(json!([{ "op": "set_model", "id": "llama-70b" }]), json!({}));
    assert!(validate::validate(&mut p, &c, None).iter().any(|i| i.code == "model_unknown"));

    // normalisation: label → slug, extension added, dates filled from the pre-router,
    // confirmation recomputed, intent recomputed, field label → key
    let mut p = plan_of(
        json!([
            { "op": "extract", "schema_ref": "영수증", "fields": ["합계", "상호", "없음"] },
            { "op": "export", "format": "xlsx", "path": "exports/영수증", "columns": ["합계"] },
            { "op": "move", "folder": "documents/{{year}}" }
        ]),
        json!({ "filter": { "doc_type": "영수증" } }),
    );
    let date = DateRange {
        from: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
        to: NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
        label: "last_month".into(),
    };
    let issues = validate::validate(&mut p, &c, Some(&date));
    assert!(!has_errors(&issues), "{issues:?}");
    assert!(issues.iter().any(|i| i.code == "field_unknown" && !i.blocking));
    assert_eq!(p.scope.filter.doc_type.as_deref(), Some("receipt"));
    assert_eq!(p.scope.filter.date_from.as_deref(), Some("2026-10-01"));
    assert!(
        matches!(&p.operations[0], Op::Extract { schema_ref: Some(s), fields, .. } if s == "s-receipt" && fields == &["total", "store_name"])
    );
    assert!(
        matches!(&p.operations[1], Op::Export { path: Some(path), columns, .. } if path == "exports/영수증.xlsx" && columns == &["total"])
    );
    assert!(p.requires_confirmation);
    assert_eq!(p.intent, OpKind::Extract);

    // set_setting coercions
    let mut p = plan_of(
        json!([{ "op": "set_setting", "key": "review_threshold", "value": "85%" }, { "op": "set_setting", "key": "pii_masking", "value": "off" }]),
        json!({}),
    );
    assert!(!has_errors(&validate::validate(&mut p, &c, None)));
    assert!(matches!(&p.operations[0], Op::SetSetting { value, .. } if (value.as_f64().unwrap() - 0.85).abs() < 1e-9));
    assert!(matches!(&p.operations[1], Op::SetSetting { value: Value::Bool(false), .. }));
    assert_eq!(validate::workspace_relative("documents\\2026\\10\\"), Some("documents/2026/10".into()));
    assert_eq!(validate::workspace_relative(".doculens/x"), None);
}

fn deps(chat: &StageChat) -> PlannerDeps<'_> {
    PlannerDeps { chat, model: "scripted".into(), profile: Profile::Balanced, ctx_size: None, cancel: None }
}

#[test]
fn model_path_requeries_once_with_feedback() {
    let text = "한빛과 거래한 계약 중 작년 것만 골라서 보여줘";
    let chat = StageChat::new();
    // 1st reply: an unknown parameter → rejected → re-query with the error; 2nd reply: valid.
    chat.push("plan", json!({ "summary": "x", "intent": "search", "scope": {}, "operations": [{ "op": "search", "query": "한빛", "limit": 5 }], "requires_confirmation": false, "confidence": 0.8 }));
    chat.push("plan", json!({ "summary": "한빛 계약 검색", "intent": "search", "scope": { "filter": { "doc_type": "contract" } }, "operations": [{ "op": "search", "query": "한빛" }], "requires_confirmation": false, "confidence": 0.8 }));
    let r = plan(Some(&deps(&chat)), text, &ctx()).unwrap();
    assert_eq!(r.source, PlanSource::Model);
    assert_eq!(r.attempts, 2);
    assert!(matches!(&r.plan.operations[0], Op::Search { query, .. } if query == "한빛"));
    // "작년" was resolved deterministically and filled in.
    assert_eq!(r.plan.scope.filter.date_from.as_deref(), Some("2025-01-01"));
    assert_eq!(r.model.as_deref(), Some("scripted"));
    assert_eq!(chat.calls("plan"), 2);
    // The re-query carried the previous output and the validator's finding.
    let reqs = chat.requests.lock().unwrap();
    let second = &reqs[1];
    let last_user = second.messages.last().unwrap()["content"].as_str().unwrap().to_string();
    assert!(last_user.contains("limit") || last_user.contains("unknown field"), "{last_user}");
    assert!(second.messages[0]["content"].as_str().unwrap().contains("- aggregate:"));
    assert_eq!(second.temperature, Some(0.0));
    assert!(second.response_format.as_ref().unwrap()["json_schema"]["schema"]["properties"]["operations"].is_object());

    // two rejected attempts → clarification
    let chat = StageChat::new();
    chat.push("plan", json!({ "summary": "x", "intent": "search", "scope": {}, "operations": [], "requires_confirmation": false, "confidence": 0.8 }));
    chat.push("plan", json!({ "summary": "x", "intent": "search", "scope": {}, "operations": [{ "op": "bogus" }], "requires_confirmation": false, "confidence": 0.8 }));
    let r = plan(Some(&deps(&chat)), text, &ctx()).unwrap();
    assert!(r.plan.is_clarification());
    assert_eq!(r.attempts, 2);
    assert!(has_errors(&r.issues));

    // low confidence → clarification with the model's question
    let chat = StageChat::new();
    chat.push("plan", json!({ "summary": "?", "intent": "search", "scope": {}, "operations": [{ "op": "search", "query": "한빛" }], "requires_confirmation": false, "confidence": 0.4, "clarification_question": "어느 거래처요?" }));
    let r = plan(Some(&deps(&chat)), text, &ctx()).unwrap();
    assert!(r.plan.is_clarification());
    assert_eq!(r.plan.clarification_question.as_deref(), Some("어느 거래처요?"));

    // model failure → clarification + model_error issue (no Err)
    let chat = StageChat::new();
    *chat.fail_with.lock().unwrap() = Some("HTTP 500".into());
    let r = plan(Some(&deps(&chat)), text, &ctx()).unwrap();
    assert!(r.plan.is_clarification());
    assert_eq!(r.issues[0].code, "model_error");

    // Prompt tiers: Full > Compact > Minimal in size; the tier is the largest one within the
    // context budget, so a 4K context ends up with the minimal prompt (English rules, op names,
    // two examples) and the compact tier keeps only the schemas the command mentions.
    let c = ctx();
    let text = "영수증 중 한빛 것만 골라서 보여줘";
    let full = prompts::message_tokens(&prompts::messages_for(&c, text, None, prompts::Tier::Full, None));
    let compact = prompts::message_tokens(&prompts::messages_for(&c, text, None, prompts::Tier::Compact, None));
    let minimal = prompts::message_tokens(&prompts::messages_for(&c, text, None, prompts::Tier::Minimal, None));
    assert!(full > compact && compact > minimal, "{full} {compact} {minimal}");
    assert_eq!(prompts::tier_for(&c, text, None, Some(full), None), prompts::Tier::Full);
    assert_eq!(prompts::tier_for(&c, text, None, Some(compact), None), prompts::Tier::Compact);
    assert_eq!(prompts::tier_for(&c, text, None, Some(minimal), None), prompts::Tier::Minimal);
    // Below the minimal size the last-resort tier (op names only, no examples) is used.
    let tiny = prompts::message_tokens(&prompts::messages_for(&c, text, None, prompts::Tier::Tiny, None));
    assert!(tiny < minimal / 2, "{tiny} {minimal}");
    assert_eq!(prompts::tier_for(&c, text, None, Some(100), None), prompts::Tier::Tiny);
    assert_eq!(prompts::tier_for(&c, text, None, None, None), prompts::Tier::Full);
    assert!(
        minimal <= prompts::prompt_budget(Some(4096), llm::REPLY_TOKENS).unwrap(),
        "minimal prompt must fit a 4K context: {minimal}"
    );
    let compact_msgs = prompts::messages_for(&c, text, None, prompts::Tier::Compact, None);
    let system = compact_msgs[0]["content"].as_str().unwrap();
    assert!(system.contains("스키마 영수증[receipt]"));
    assert!(!system.contains("스키마 세금계산서"));
    assert_eq!(compact_msgs.len(), 1 + 4 * 2 + 1);
    let chat = StageChat::new();
    chat.push("plan", json!({ "summary": "x", "intent": "search", "scope": {}, "operations": [{ "op": "search", "query": "한빛" }], "requires_confirmation": false, "confidence": 0.9 }));
    let mut d = deps(&chat);
    d.ctx_size = Some(4096);
    let _ = plan(Some(&d), text, &c).unwrap();
    {
        // 4K never gets the full prompt; the exact tier depends on the estimate.
        let reqs = chat.requests.lock().unwrap();
        assert!(reqs[0].messages.len() <= 1 + 4 * 2 + 1);
        assert!(
            prompts::tier_for(&c, text, None, prompts::prompt_budget(Some(4096), llm::REPLY_TOKENS), None)
                < prompts::Tier::Full
        );
        let minimal_msgs = prompts::messages_for(&c, text, None, prompts::Tier::Minimal, None);
        let system = minimal_msgs[0]["content"].as_str().unwrap();
        assert!(system.starts_with("You are the command planner"));
        assert!(system.contains("- aggregate [fn:sum|count|avg|min|max]"));
        assert_eq!(minimal_msgs.len(), 1 + 2 * 2 + 1);
    }
    // The server counting more tokens than the estimate → one retry at a smaller tier.
    let chat = StageChat::new();
    chat.fail_queue
        .lock()
        .unwrap()
        .push("HTTP 400: request (5736 tokens) exceeds the available context size (4096 tokens)".into());
    chat.push("plan", json!({ "summary": "x", "intent": "search", "scope": {}, "operations": [{ "op": "search", "query": "한빛" }], "requires_confirmation": false, "confidence": 0.9 }));
    let mut d = deps(&chat);
    d.ctx_size = Some(16_384);
    let r = plan(Some(&d), "영수증 중 한빛 것만 골라서 보여줘", &ctx()).unwrap();
    assert_eq!(r.source, PlanSource::Model, "{r:?}");
    assert_eq!(r.attempts, 1);
    let reqs = chat.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(reqs[1].messages.len() < reqs[0].messages.len());
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

mod exec_tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use chrono::NaiveDate;
    use serde_json::json;

    use crate::db::Db;
    use crate::documents::convert::StubPdfPrinter;
    use crate::documents::import::ImportDeps;
    use crate::documents::render::{write_jpeg, RenderOptions};
    use crate::documents::{
        repo as docs, DocumentCache, DocumentRole, DocumentStatus, NewDocument, NewPage, ReviewStatus, SourceKind,
    };
    use crate::knowledge::tokenize::Analyzer;
    use crate::paths::WorkspacePaths;
    use crate::pipeline::repo as pipeline_repo;
    use crate::pipeline::vlm::testing::StageChat;
    use crate::planner::exec::{self, ExecDeps, StepStatus};
    use crate::planner::*;
    use crate::settings::{KnowledgeSettings, Language, ProcessingSettings, Profile};

    struct World {
        db: Db,
        paths: WorkspacePaths,
        _dir: tempfile::TempDir,
        chat: StageChat,
        analyzer: Analyzer,
    }

    impl World {
        fn new() -> World {
            let dir = tempfile::tempdir().unwrap();
            let paths = WorkspacePaths::new(dir.path());
            for d in paths.all_dirs() {
                std::fs::create_dir_all(d).unwrap();
            }
            let db = Db::open_in_memory().unwrap();
            db.with(pipeline_repo::ensure_builtins).unwrap();
            World { db, paths, _dir: dir, chat: StageChat::new(), analyzer: Analyzer::simple() }
        }

        /// A one-page scanned image at `rel` (rendered page cached as JPEG).
        fn add_image(&self, id: &str, rel: &str) {
            let abs = self.paths.root.join(rel);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            let img = image::RgbImage::from_fn(600, 400, |x, y| {
                if (x / 50 + y / 50) % 2 == 0 {
                    image::Rgb([255, 255, 255])
                } else {
                    image::Rgb([30, 30, 30])
                }
            });
            img.save(&abs).unwrap();
            let hash = crate::documents::import::blake3_file(&abs).unwrap();
            self.db
                .with(|c| {
                    docs::insert_document(
                        c,
                        &NewDocument {
                            id: id.into(),
                            rel_path: rel.into(),
                            original_name: rel.rsplit('/').next().unwrap().into(),
                            source_kind: SourceKind::Image,
                            content_hash: hash,
                            size_bytes: 10,
                            file_modified_at: None,
                            role: DocumentRole::Work,
                            import_batch_id: None,
                        },
                    )
                })
                .unwrap();
            let cache = DocumentCache::for_document(&self.paths, id);
            let (w, h) = write_jpeg(&img, &cache.page_file(1), 85).unwrap();
            let page = NewPage {
                page_no: 1,
                width_px: w,
                height_px: h,
                dpi: Some(200),
                is_blank: Some(false),
                ..Default::default()
            };
            self.db.with_tx(|tx| docs::replace_pages(tx, id, &[page], Some(200), false, None)).unwrap();
            self.db.with(|c| docs::set_status(c, id, DocumentStatus::Rendered, None)).unwrap();
        }

        fn deps<'a>(&'a self, cancel: Option<&'a AtomicBool>) -> ExecDeps<'a> {
            ExecDeps {
                import: ImportDeps {
                    db: &self.db,
                    paths: &self.paths,
                    render: None,
                    printer: &StubPdfPrinter,
                    render_options: RenderOptions::default(),
                },
                chat: Some(&self.chat),
                model: Some("scripted".into()),
                ctx_size: None,
                settings: ProcessingSettings { audit_sample_percent: 0, ..ProcessingSettings::default() },
                knowledge: KnowledgeSettings::default(),
                profile: Profile::Balanced,
                pii_masking: true,
                analyzer: &self.analyzer,
                embedder: None,
                language: Language::Ko,
                today: NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(),
                cancel,
                checkpoint: None,
                lease: None,
            }
        }

        fn doc(&self, id: &str) -> crate::documents::DocumentRecord {
            self.db.with(|c| docs::get_document(c, id)).unwrap().unwrap()
        }
    }

    fn sv(v: &str, e: &str, c: u8) -> serde_json::Value {
        json!({ "value": v, "evidence": e, "confidence": c })
    }

    const RECEIPT_OCR: &str = "# 스타벅스 역삼점\n2026-10-14 12:31\n<table><tr><th>품명</th><th>수량</th><th>금액</th></tr><tr><td>아메리카노</td><td>2</td><td>9,000</td></tr></table>\n공급가액 11,364\n부가세 1,136\n합계 12,500\n카드 승인 12345678";

    fn receipt_kie() -> serde_json::Value {
        let none = json!({ "value": null, "evidence": null, "confidence": 0 });
        json!({
            "store_name": sv("스타벅스 역삼점", "스타벅스 역삼점", 95),
            "brn": none, "merchant_no": none, "address": none, "phone": none,
            "datetime": sv("2026-10-14 12:31", "2026-10-14 12:31", 90),
            "items": [{ "name": "아메리카노", "quantity": "2", "unit_price": "4,500", "amount": "9,000" }],
            "supply_amount": sv("11,364", "공급가액 11,364", 85),
            "vat": sv("1,136", "부가세 1,136", 85),
            "service_charge": none, "exempt_amount": none,
            "total": sv("12,500", "합계 12,500", 93),
            "payment_method": sv("카드", "카드 승인", 70),
            "approval_type": none, "installment": none, "card_last4": none,
            "approval_no": sv("12345678", "카드 승인 12345678", 60),
            "cash_receipt_no": none, "cash_receipt_id": none
        })
    }

    fn plan_of(ops: Vec<Op>, scope: Scope) -> Plan {
        let mut p = Plan {
            intent: ops[0].kind(),
            scope,
            operations: ops,
            requires_confirmation: false,
            confidence: 0.9,
            clarification_question: None,
            summary: None,
        };
        p.requires_confirmation = p.operations.iter().any(Op::needs_confirmation);
        p
    }

    #[test]
    fn plan_runs_pipeline_tag_aggregate_export_move_and_undo() {
        let w = World::new();
        w.add_image("r1", "inbox/IMG_2031.jpg");
        w.chat.push("classify", json!({ "doc_type": "receipt", "rotation": 0, "is_blank": false, "language": "ko" }));
        w.chat.push_ocr(RECEIPT_OCR);
        w.chat.push("kie_receipt", receipt_kie());
        let scope = Scope { folder: Some("inbox".into()), ..Scope::default() };
        let plan = plan_of(
            vec![
                Op::Classify { doc_types: Vec::new(), force: None },
                Op::Ocr { pages: Vec::new(), profile: None, force: None },
                Op::Extract { schema_ref: Some("receipt".into()), fields: Vec::new(), force: None },
                Op::Tag { add: vec!["경비".into()], remove: Vec::new() },
                Op::Aggregate {
                    func: AggregateFn::Sum,
                    field: Some("합계".into()),
                    group_by: Some(GroupBy::Month),
                    include_unreviewed: Some(true),
                },
                Op::Export {
                    format: ExportFormat::Csv,
                    path: None,
                    columns: Vec::new(),
                    layout: None,
                    include: Vec::new(),
                    target: None,
                },
                Op::SetTheme { theme: crate::settings::Theme::Dark },
                Op::Move {
                    folder: "documents/{{year}}/{{month}}/{{doc_type}}".into(),
                    group_by: Some(GroupBy::Month),
                    only_reviewed: Some(false),
                },
                Op::Undo { target: None },
            ],
            scope,
        );
        // Preview before running: the move is a confirmed step with one item.
        let preview = exec::preview(&w.db, &w.paths, &plan, &ProcessingSettings::default(), Language::Ko).unwrap();
        assert_eq!(preview.document_count, 1);
        assert!(preview.requires_confirmation);
        let move_step = preview.steps.iter().find(|s| s.op == OpKind::Move).unwrap();
        assert!(move_step.confirm, "{move_step:?}");
        assert_eq!(move_step.payload["moves"], 1);
        let export_step = preview.steps.iter().find(|s| s.op == OpKind::Export).unwrap();
        assert_eq!(export_step.payload["exists"], false);
        assert!(export_step.payload["path"].as_str().unwrap().starts_with("exports/"));

        let mut stages = Vec::new();
        let ex = exec::run(&w.deps(None), &plan, &mut |p| stages.push(p.stage.clone())).unwrap();
        for s in &ex.steps {
            assert!(matches!(s.status, StepStatus::Done | StepStatus::Ui), "{s:?}");
        }
        assert!(!ex.cancelled);
        assert!(stages.iter().any(|s| s == "ocr") && stages.iter().any(|s| s == "extract"));
        let doc = w.doc("r1");
        assert_eq!(doc.doc_type.as_deref(), Some("receipt"));
        assert_eq!(doc.total_amount, Some(12_500.0));
        assert_eq!(doc.review_status, ReviewStatus::Pending);
        assert!(doc.tags.contains(&"경비".to_string()));
        let agg = &ex.steps[4];
        assert_eq!(agg.payload["value"], 12_500.0);
        assert_eq!(agg.payload["unreviewed"]["count"], 1);
        assert_eq!(agg.payload["groups"][0]["key"], "2026-10");
        assert!(agg.summary.contains("12,500"), "{}", agg.summary);
        let export = &ex.steps[5];
        let path = export.payload["path"].as_str().unwrap();
        assert!(w.paths.root.join(path).is_file(), "{path}");
        // A second unnamed export must not overwrite the first: it gets `_2`.
        let again = plan_of(
            vec![Op::Export {
                format: ExportFormat::Csv,
                path: None,
                columns: Vec::new(),
                layout: None,
                include: Vec::new(),
                target: None,
            }],
            Scope { folder: Some("inbox".into()), ..Scope::default() },
        );
        let ex2 = exec::run(&w.deps(None), &again, &mut |_| {}).unwrap();
        let path2 = ex2.steps[0].payload["path"].as_str().unwrap();
        assert_ne!(path, path2);
        assert!(path2.ends_with("_2.csv"), "{path2}");
        assert!(w.paths.root.join(path2).is_file());
        assert_eq!(ex.ui_actions, vec![Op::SetTheme { theme: crate::settings::Theme::Dark }]);
        assert_eq!(ex.steps[6].status, StepStatus::Ui);
        // The move happened (the undo step then reverted it), both journaled.
        let mv = &ex.steps[7];
        assert_eq!(mv.payload["moved"], 1);
        let moved_to = mv.payload["items"][0]["to"].as_str().unwrap().to_string();
        // `move` keeps the file name; only the folder template is applied.
        assert_eq!(moved_to, "documents/2026/10/영수증/IMG_2031.jpg");
        let undo = &ex.steps[8];
        assert_eq!(undo.payload["restored"], 1);
        assert_eq!(w.doc("r1").rel_path, "inbox/IMG_2031.jpg");
        assert!(w.paths.root.join("inbox/IMG_2031.jpg").is_file());
        assert_eq!(ex.batches.len(), 2);
        assert_eq!(ex.documents, vec!["r1".to_string()]);
    }

    #[test]
    fn failures_skip_later_steps_and_cancel_marks_the_execution() {
        let w = World::new();
        w.add_image("r1", "inbox/a.jpg");
        let plan = plan_of(
            vec![
                Op::Tag { add: vec!["x".into()], remove: Vec::new() },
                Op::Extract { schema_ref: Some("no-such-schema".into()), fields: Vec::new(), force: None },
                Op::Tag { add: vec!["y".into()], remove: Vec::new() },
                Op::Clarify { question: "?".into() },
            ],
            Scope { folder: Some("inbox".into()), ..Scope::default() },
        );
        let ex = exec::run(&w.deps(None), &plan, &mut |_| {}).unwrap();
        assert_eq!(ex.steps[0].status, StepStatus::Done);
        assert_eq!(ex.steps[1].status, StepStatus::Failed);
        assert!(ex.steps[1].error.as_deref().unwrap().contains("schema_not_found"));
        assert_eq!(ex.steps[2].status, StepStatus::Skipped);
        assert!(ex.failed());
        assert_eq!(w.doc("r1").tags, vec!["x".to_string()]);

        // Without a model, model steps fail with `no_model`.
        let mut deps = w.deps(None);
        deps.chat = None;
        deps.model = None;
        let plan = plan_of(
            vec![Op::Classify { doc_types: Vec::new(), force: None }],
            Scope { folder: Some("inbox".into()), ..Scope::default() },
        );
        let ex = exec::run(&deps, &plan, &mut |_| {}).unwrap();
        assert_eq!(ex.steps[0].status, StepStatus::Failed);
        assert!(ex.steps[0].error.as_deref().unwrap().contains("no_model"));

        // A cancel before the run marks every step cancelled.
        let cancel = AtomicBool::new(true);
        let plan = plan_of(vec![Op::Tag { add: vec!["z".into()], remove: Vec::new() }], Scope::default());
        let ex = exec::run(&w.deps(Some(&cancel)), &plan, &mut |_| {}).unwrap();
        assert!(ex.cancelled);
        assert_eq!(ex.steps[0].status, StepStatus::Cancelled);
        cancel.store(false, Ordering::Relaxed);

        // Search runs without a model (no hits on an un-indexed workspace, but no error).
        let plan = plan_of(vec![Op::Search { query: "스타벅스".into(), mode: None, top_k: Some(5) }], Scope::default());
        let ex = exec::run(&deps, &plan, &mut |_| {}).unwrap();
        assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
        assert_eq!(ex.steps[0].payload["hits"].as_array().map(Vec::len), Some(0));
    }
}

#[test]
#[ignore]
fn print_prompt_sizes() {
    let c = ctx();
    let text = "한빛과 거래한 계약 중 작년 것만 골라서 보여줘";
    for tier in [prompts::Tier::Full, prompts::Tier::Compact, prompts::Tier::Minimal] {
        let msgs = prompts::messages_for(&c, text, None, tier, None);
        let chars: usize = msgs.iter().map(|m| m["content"].as_str().unwrap_or("").chars().count()).sum();
        let ascii: usize =
            msgs.iter().map(|m| m["content"].as_str().unwrap_or("").chars().filter(|c| c.is_ascii()).count()).sum();
        eprintln!(
            "{tier:?}: est={} chars={chars} ascii={ascii} non_ascii={}",
            prompts::message_tokens(&msgs),
            chars - ascii
        );
    }
}
