//! Planner tests: the design's example commands through the rules / pre-router
//! (no model), the appendix-D spec ↔ serde agreement, validator rejections and
//! normalisation, and the model path with one re-query.

use chrono::NaiveDate;
use serde_json::{json, Value};

use crate::pipeline::vlm::testing::StageChat;
use crate::prerouter::DateRange;
use crate::settings::{Language, Profile, Theme};

use super::exec;
use super::llm::{self, plan, PlannerDeps};
use super::prompts;
use super::rules;
use super::schema::{self, ParamType, OP_SPECS};
use super::validate;
use super::PlanMode;
use super::*;

fn field(key: &str, ko: &str, en: &str, kind: &str) -> FieldSummary {
    FieldSummary { key: key.into(), label_ko: ko.into(), label_en: en.into(), kind: kind.into() }
}

fn ctx() -> PlanContext {
    PlanContext {
        draft_id: None,
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
        model_ids: vec!["gemma4-e4b-q5km".into(), "gemma4-12b-q4km".into()],
        projects: Vec::new(),
        current_project: None,
        scope_project: None,
        history: Vec::new(),
    }
}

/// 0.3.0 item 6: "프로젝트별" groups folders / sheets / totals by project; a named project is the scope.
#[test]
fn project_group_words_and_project_scope() {
    let mut c = ctx();
    c.projects = vec![
        ProjectSummary { id: "p-hanbit".into(), name: "한빛 심사".into(), status: "active".into() },
        ProjectSummary { id: "p-acme".into(), name: "Acme onboarding".into(), status: "done".into() },
    ];
    let planned = |text: &str| plan(None, text, &c, PlanMode::RulesFirst).unwrap();
    let grouped_by_project = |r: &PlanResult| {
        matches!(
            &r.plan.operations[0],
            Op::Organize { group_by: Some(GroupBy::Project), .. } | Op::Move { group_by: Some(GroupBy::Project), .. }
        )
    };
    let r = planned("전체 문서를 프로젝트별로 정리해줘");
    assert!(grouped_by_project(&r), "{r:?}");
    assert!(r.plan.scope.filter.project_id.is_none(), "the group word is not a project reference: {r:?}");
    let r = planned("검토 완료된 문서 전부 프로젝트별 폴더로 옮겨");
    assert!(grouped_by_project(&r), "{r:?}");
    let r = planned("프로젝트별 영수증 합계 알려줘");
    assert!(matches!(&r.plan.operations[0], Op::Aggregate { group_by: Some(GroupBy::Project), .. }), "{r:?}");
    let r = planned("Organize all documents by project");
    assert!(grouped_by_project(&r), "{r:?}");
    // A named project narrows the scope and the per-type layout stays.
    let r = planned("한빛 심사 프로젝트 문서 엑셀로 내보내");
    assert!(matches!(&r.plan.operations[0], Op::Export { format: ExportFormat::Xlsx, .. }), "{r:?}");
    assert_eq!(r.plan.scope.filter.project_id.as_deref(), Some("p-hanbit"), "{r:?}");
    let r = planned("Acme onboarding 프로젝트 계약서 월별로 정리");
    assert!(
        matches!(
            &r.plan.operations[0],
            Op::Organize { group_by: Some(GroupBy::Month), .. } | Op::Move { group_by: Some(GroupBy::Month), .. }
        ),
        "{r:?}"
    );
    assert_eq!(r.plan.scope.filter.project_id.as_deref(), Some("p-acme"));
    assert_eq!(r.plan.scope.filter.doc_type.as_deref(), Some("contract"));
    // The schema accepts the new group / layout values and the create_project dates.
    let schema = schema::plan_schema();
    let text = serde_json::to_string(&schema).unwrap();
    assert!(
        text.contains("\"project\"") && text.contains("sheet_per_project") && text.contains("start_date"),
        "{text}"
    );
}

fn kinds(p: &Plan) -> Vec<OpKind> {
    p.operations.iter().map(Op::kind).collect()
}

fn planned(text: &str) -> PlanResult {
    plan(None, text, &ctx(), PlanMode::RulesFirst).unwrap()
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
    let r = planned("다크 테마로 바꾸고 모델을 12B로 올려");
    assert_eq!(kinds(&r.plan), [OpKind::SetTheme, OpKind::SetModel], "{r:?}");
    assert!(matches!(&r.plan.operations[0], Op::SetTheme { theme: Theme::Dark }));
    assert!(matches!(&r.plan.operations[1], Op::SetModel { id } if id == "gemma4-12b-q4km"));
    // Gemma's effective sizes are written "E4B" in the id but said as "4B".
    let r = planned("모델을 4B로 바꿔");
    assert!(matches!(&r.plan.operations[0], Op::SetModel { id } if id == "gemma4-e4b-q5km"), "{r:?}");

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
    let r = plan(None, "문서 삭제해", &c, PlanMode::RulesFirst).unwrap();
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
    let planned = |text: &str| plan(None, text, &c, PlanMode::RulesFirst).unwrap();

    // Drafting: a built-in template picked from the keyword (견적서 → the software estimate; the context has no
    // registered 견적서 form), the whole sentence as content.
    let r = planned("등록된 견적서 양식으로 ㈜한빛에 노트북 10대, 대당 1,200,000원, 유효기간 30일 견적서 만들어줘");
    assert_eq!(kinds(&r.plan), [OpKind::DraftDocument], "{r:?}");
    assert!(
        matches!(&r.plan.operations[0], Op::DraftDocument { form_ref, content, .. } if form_ref.as_deref() == Some("estimate") && content.contains("노트북"))
    );
    assert!(r.plan.requires_confirmation);
    assert!(!has_errors(&r.issues), "{:?}", r.issues);
    // Rewrite the open contract → the selection is a source.
    let r = planned("이 계약서 내용으로 갑·을 바꿔서 계약서 다시 써줘");
    assert!(
        matches!(&r.plan.operations[0], Op::DraftDocument { form_ref, sources, .. } if form_ref.as_deref() == Some("dev_contract") && sources == &["d1a2".to_string()]),
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
    assert!(
        matches!(&r.plan.operations[0], Op::AddToProject { project: Some(p), as_reference: false, .. } if p == "p-hanbit"),
        "{r:?}"
    );
    assert_eq!(r.plan.scope.doc_ids, vec!["d1a2".to_string()]);
    // 0.5.0: "참고 자료로 / 지침으로 넣어줘" → the project's reference group.
    let r = planned("선택한 문서를 한빛 심사 프로젝트에 참고 자료로 넣어줘");
    assert!(
        matches!(&r.plan.operations[0], Op::AddToProject { project: Some(p), as_reference: true, .. } if p == "p-hanbit"),
        "{r:?}"
    );
    let r = planned("add these to the 한빛 심사 project as reference material");
    assert!(matches!(&r.plan.operations[0], Op::AddToProject { as_reference: true, .. }), "{r:?}");
    // The target project is the destination, not a membership filter, for the documents it takes.
    let r = planned("선택한 문서를 한빛 심사 프로젝트에 추가해");
    assert_eq!(r.plan.scope.filter.project_id.as_deref(), Some("p-hanbit"));
    assert!(exec::plan_scope(&r.plan).filter.project_id.is_none(), "{r:?}");
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
    let r = plan(None, "정합성 검사 돌려줘", &c2, PlanMode::RulesFirst).unwrap();
    assert!(r.plan.is_clarification() || has_errors(&r.issues), "{r:?}");

    // Explain / compare.
    let r = planned("이 문장 설명해줘");
    assert!(matches!(&r.plan.operations[0], Op::Explain { .. }), "{r:?}");
    let mut c3 = c.clone();
    c3.selected_doc_ids = vec!["a".into(), "b".into()];
    let r = plan(None, "선택한 두 문서 비교해줘", &c3, PlanMode::RulesFirst).unwrap();
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
    use super::ProjectSummary;
    let mut c = ctx();
    c.projects = vec![
        ProjectSummary { id: "p-hanbit".into(), name: "한빛 심사".into(), status: "intake".into() },
        ProjectSummary { id: "p-acme".into(), name: "Acme onboarding".into(), status: "validating".into() },
    ];
    c.current_project = Some("p-hanbit".into());
    let not = |text: &str, bad: &[OpKind]| {
        let r = plan(None, text, &c, PlanMode::RulesFirst).unwrap();
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
    let r = plan(None, "한빛 심사 프로젝트에 이 문서 추가해줘", &c, PlanMode::RulesFirst).unwrap();
    assert!(matches!(&r.plan.operations[0], Op::AddToProject { project: Some(p), .. } if p == "p-hanbit"), "{r:?}");
    let r = plan(None, "'한빛 2차 심사' 프로젝트 만들어줘", &c, PlanMode::RulesFirst).unwrap();
    assert!(r.plan.requires_confirmation && matches!(&r.plan.operations[0], Op::CreateProject { .. }), "{r:?}");
    let r = plan(None, "검색 색인 다시 만들어줘", &c, PlanMode::RulesFirst).unwrap();
    assert!(matches!(&r.plan.operations[0], Op::Reindex { .. }), "{r:?}");
    let r = plan(None, "DPI를 300으로 바꿔줘", &c, PlanMode::RulesFirst).unwrap();
    assert!(matches!(&r.plan.operations[0], Op::SetSetting { key, .. } if key == "processing.render_dpi"), "{r:?}");
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
    let r = plan(None, "태그 중요 추가해", &c, PlanMode::RulesFirst).unwrap();
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
    let r =
        plan(Some(&deps(&chat)), "한빛과 거래한 서류들 유형을 영수증으로 설정해", &c, PlanMode::RulesFirst).unwrap();
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
    // One grammar alternative per op; `set_setting` alone has one per value shape (0.8.0).
    let alternatives = s["properties"]["operations"]["items"]["anyOf"].as_array().unwrap();
    let op_of = |a: &Value| a["properties"]["op"]["const"].as_str().unwrap_or_default().to_string();
    let named: std::collections::BTreeSet<String> = alternatives.iter().map(op_of).collect();
    assert_eq!(named.len(), OP_SPECS.len());
    assert_eq!(alternatives.iter().filter(|a| op_of(a) != "set_setting").count(), OP_SPECS.len() - 1);
    assert_eq!(alternatives.iter().filter(|a| op_of(a) == "set_setting").count(), schema::setting_schemas().len());
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

/// 0.7.0 plan §1.5: draft-revision phrases → `revise_draft` on the draft open in the 문서 작성
/// screen; without an open draft the validator asks for one, and drafting / splitting sentences
/// keep their own ops.
#[test]
fn revise_draft_rules_target_the_open_draft() {
    let mut c = ctx();
    c.draft_id = Some("dr1".into());
    let planned = |text: &str| plan(None, text, &c, PlanMode::RulesFirst).unwrap();
    let op = |text: &str| planned(text).plan.operations[0].clone();
    let check = |text: &str, ok: bool| assert!(ok, "{text}: {:?}", planned(text).plan);
    check(
        "3절 다시 써줘, 숫자는 그대로",
        matches!(op("3절 다시 써줘, 숫자는 그대로"), Op::ReviseDraft { ref scope, section: Some(3), draft_id: Some(ref d), ref instruction, .. }
            if scope == "section" && d == "dr1" && instruction.starts_with("3절")),
    );
    check(
        "2절을 더 자세히",
        matches!(op("2절을 더 자세히"), Op::ReviseDraft { ref scope, section: Some(2), .. } if scope == "section"),
    );
    check(
        "결론 절 추가해줘",
        matches!(op("결론 절 추가해줘"), Op::ReviseDraft { ref scope, section: None, heading: Some(ref h), .. } if scope == "insert_after" && h == "결론"),
    );
    check(
        "2절 뒤에 '위험 관리' 절 추가",
        matches!(op("2절 뒤에 '위험 관리' 절 추가"), Op::ReviseDraft { ref scope, section: Some(2), heading: Some(ref h), .. } if scope == "insert_after" && h == "위험 관리"),
    );
    check(
        "초안 전체 격식체로 바꿔줘",
        matches!(op("초안 전체 격식체로 바꿔줘"), Op::ReviseDraft { ref scope, section: None, .. } if scope == "whole"),
    );
    check(
        "rewrite section 2 more formally",
        matches!(op("rewrite section 2 more formally"), Op::ReviseDraft { ref scope, section: Some(2), .. } if scope == "section"),
    );
    check(
        "add a conclusion section",
        matches!(op("add a conclusion section"), Op::ReviseDraft { ref scope, heading: Some(ref h), .. } if scope == "insert_after" && h == "conclusion"),
    );
    check("이 절 다시 써줘", matches!(op("이 절 다시 써줘"), Op::Clarify { .. }));
    // Not revisions.
    check("견적서 만들어줘", !matches!(op("견적서 만들어줘"), Op::ReviseDraft { .. }));
    check("이 문서 3쪽부터 나눠줘", !matches!(op("이 문서 3쪽부터 나눠줘"), Op::ReviseDraft { .. }));
    check("절차서 양식으로 등록해줘", !matches!(op("절차서 양식으로 등록해줘"), Op::ReviseDraft { .. }));
    check("절반으로 줄여서 요약해줘", !matches!(op("절반으로 줄여서 요약해줘"), Op::ReviseDraft { .. }));
    check("초안 전체 요약해줘", !matches!(op("초안 전체 요약해줘"), Op::ReviseDraft { .. }));
    check("3절 요약해줘", !matches!(op("3절 요약해줘"), Op::ReviseDraft { .. }));
    // No open draft: the plan is rejected with `draft_required`.
    let none = ctx();
    let r = plan(None, "3절 다시 써줘", &none, PlanMode::RulesFirst).unwrap();
    assert!(r.issues.iter().any(|i| i.code == "draft_required"), "{r:?}");
}

/// 0.6.0: every GUI function has a phrase rule (plan §4); the sentences of the plan parse to
/// the right op without a model.
#[test]
fn agent_rules_cover_every_gui_function() {
    let mut c = ctx();
    c.projects = vec![ProjectSummary { id: "p-hanbit".into(), name: "한빛 심사".into(), status: "active".into() }];
    c.selected_doc_ids = vec!["d1a2".into(), "d2b3".into()];
    let planned = |text: &str| plan(None, text, &c, PlanMode::RulesFirst).unwrap();
    let op = |text: &str| planned(text).plan.operations[0].clone();
    // documents
    assert!(
        matches!(op("이 문서 90도 돌려줘"), Op::Rotate { degrees: 90, .. }),
        "{:?}",
        planned("이 문서 90도 돌려줘").plan
    );
    assert!(
        matches!(op("선택한 문서 180도 회전"), Op::Rotate { degrees: 180, .. }),
        "{:?}",
        planned("선택한 문서 180도 회전").plan
    );
    assert!(
        matches!(op("선택한 문서 300 DPI로 다시 렌더"), Op::Rerender { dpi: Some(300) }),
        "{:?}",
        planned("선택한 문서 300 DPI로 다시 렌더").plan
    );
    assert!(
        matches!(op("이 문서 3쪽부터 나눠줘"), Op::Split { boundaries, every: None } if boundaries == vec![3]),
        "{:?}",
        planned("이 문서 3쪽부터 나눠줘").plan
    );
    assert!(
        matches!(op("이 문서 5쪽마다 분할"), Op::Split { every: Some(5), .. }),
        "{:?}",
        planned("이 문서 5쪽마다 분할").plan
    );
    assert!(
        matches!(op("선택한 문서 하나로 합쳐서 '계약 묶음'"), Op::Merge { name: Some(n) } if n == "계약 묶음"),
        "{:?}",
        planned("선택한 문서 하나로 합쳐서 '계약 묶음'").plan
    );
    assert!(
        matches!(op("제목을 '2026 공급계약'으로 바꿔"), Op::SetTitle { title } if title == "2026 공급계약"),
        "{:?}",
        planned("제목을 '2026 공급계약'으로 바꿔").plan
    );
    assert!(
        matches!(op("'㈜한빛'을 '(주)한빛'으로 바꿔줘"), Op::EditText { find, replace, .. } if find == "㈜한빛" && replace == "(주)한빛")
    );
    assert!(
        matches!(op("이 문서를 참고 자료로 표시"), Op::SetRole { role: RoleParam::Knowledge }),
        "{:?}",
        planned("이 문서를 참고 자료로 표시").plan
    );
    // reference material
    assert!(
        matches!(op("'사규' 참고 자료 묶음 만들어"), Op::ManageReference { action: ReferenceAction::Create, name: Some(n), .. } if n == "사규")
    );
    let r = planned("선택 문서를 사규 묶음에 넣어");
    assert!(
        matches!(&r.plan.operations[0], Op::ManageReference { action: ReferenceAction::Add, group: Some(g), .. } if g == "사규"),
        "{r:?}"
    );
    assert_eq!(r.plan.scope.doc_ids.len(), 2);
    assert!(
        matches!(op("사규 묶음 삭제해"), Op::ManageReference { action: ReferenceAction::Delete, .. }),
        "{:?}",
        planned("사규 묶음 삭제해").plan
    );
    assert!(
        matches!(op("사규 묶음 사용 안 함"), Op::ManageReference { action: ReferenceAction::Disable, .. }),
        "{:?}",
        planned("사규 묶음 사용 안 함").plan
    );
    assert!(
        matches!(op("용어 'MM'은 '맨먼스'로 등록"), Op::DefineTerm { action: TermAction::Save, term, definition: Some(d), .. } if term == "MM" && d == "맨먼스")
    );
    assert!(
        matches!(op("용어 'MM' 삭제"), Op::DefineTerm { action: TermAction::Delete, .. }),
        "{:?}",
        planned("용어 'MM' 삭제").plan
    );
    assert!(
        matches!(op("이 문서를 2024년판으로 표시"), Op::SetVersion { label: Some(l), latest: Some(true), .. } if l == "2024")
    );
    assert!(
        matches!(op("이 문서 버전 표시 해제"), Op::SetVersion { clear: Some(true), .. }),
        "{:?}",
        planned("이 문서 버전 표시 해제").plan
    );
    // projects
    assert!(
        matches!(op("한빛 심사 프로젝트 이름을 '한빛 2차'로"), Op::UpdateProject { project: Some(p), name: Some(n), .. } if p == "p-hanbit" && n == "한빛 2차")
    );
    assert!(
        matches!(op("한빛 심사 프로젝트 종료일 12월 31일"), Op::UpdateProject { end_date: Some(d), .. } if d == "2026-12-31")
    );
    assert!(
        matches!(op("한빛 심사 프로젝트 삭제"), Op::DeleteProject { project: Some(p) } if p == "p-hanbit"),
        "{:?}",
        planned("한빛 심사 프로젝트 삭제").plan
    );
    let r = planned("선택 문서를 한빛 심사 프로젝트에서 빼줘");
    assert!(
        matches!(&r.plan.operations[0], Op::RemoveFromProject { project: Some(p), .. } if p == "p-hanbit"),
        "{r:?}"
    );
    assert_eq!(r.plan.scope.doc_ids.len(), 2);
    assert!(
        matches!(op("미배정 문서 프로젝트로 자동 분류"), Op::ClassifyProjects { apply: Some(false), .. }),
        "{:?}",
        planned("미배정 문서 프로젝트로 자동 분류").plan
    );
    assert!(
        matches!(op("템플릿 '유지보수 2차' 삭제"), Op::ManageTemplate { action: TemplateAction::Delete, template: Some(n), .. } if n == "유지보수 2차")
    );
    // forms
    assert!(
        matches!(op("견적서 양식 기본으로"), Op::ManageSchema { action: SchemaAction::SetDefault, schema, .. } if schema == "quotation")
    );
    assert!(
        matches!(op("영수증 양식 끄기"), Op::ManageSchema { action: SchemaAction::Disable, schema, .. } if schema == "영수증")
    );
    // system
    assert!(
        matches!(op("UI 배율 125%로"), Op::SetSetting { key, value } if key == "ui.scale_percent" && value.as_f64() == Some(125.0))
    );
    assert!(
        matches!(op("자동 임베딩 꺼줘"), Op::SetSetting { key, value } if key == "knowledge.auto_embed" && value == json!(false))
    );
    assert!(
        matches!(op("12B 모델 받아줘"), Op::ManageModel { action: ModelAction::Download, id: Some(id), .. } if id == "gemma4-12b-q4km")
    );
    assert!(
        matches!(op("모델 폴더 D:\\models\\x 가져와"), Op::ManageModel { action: ModelAction::Import, path: Some(p), .. } if p == "D:\\models\\x")
    );
    assert!(matches!(op("엔진 꺼"), Op::Engine { action: EngineOp::Stop }), "{:?}", planned("엔진 꺼").plan);
    assert!(matches!(op("엔진 켜줘"), Op::Engine { action: EngineOp::Start }), "{:?}", planned("엔진 켜줘").plan);
    assert!(
        matches!(op("워크스페이스 D:\\Work 열어"), Op::Workspace { action: WorkspaceAction::Open, path: Some(p) } if p == "D:\\Work")
    );
    assert!(
        matches!(op("지금 파기 실행"), Op::Maintenance { action: MaintenanceAction::PurgeNow }),
        "{:?}",
        planned("지금 파기 실행").plan
    );
    assert!(
        matches!(op("끝난 작업 지워"), Op::Maintenance { action: MaintenanceAction::ClearFinishedJobs }),
        "{:?}",
        planned("끝난 작업 지워").plan
    );
    // Confirmations and scope needs.
    assert!(planned("한빛 심사 프로젝트 삭제").plan.requires_confirmation);
    assert!(planned("선택한 문서 하나로 합쳐줘").plan.requires_confirmation);
    assert!(!planned("이 문서 90도 돌려줘").plan.requires_confirmation);
    // The older rules keep their meaning next to the new ones.
    assert!(matches!(op("다크 테마로 바꿔"), Op::SetTheme { .. }), "{:?}", planned("다크 테마로 바꿔").plan);
    assert!(matches!(op("모델을 12B로 바꿔"), Op::SetModel { .. }), "{:?}", planned("모델을 12B로 바꿔").plan);
    assert!(matches!(op("inbox 감시 꺼"), Op::SetInboxWatch { enabled: false }), "{:?}", planned("inbox 감시 꺼").plan);
    assert!(matches!(op("선택한 문서 삭제"), Op::Delete { .. }), "{:?}", planned("선택한 문서 삭제").plan);
    assert!(
        matches!(op("선택한 문서를 한빛 심사 프로젝트에 추가해"), Op::AddToProject { .. }),
        "{:?}",
        planned("선택한 문서를 한빛 심사 프로젝트에 추가해").plan
    );
}

/// 0.6.0 two-stage planning: the family call narrows the op table and grammar of the plan call;
/// a wrong family is retried once with every op.
#[test]
fn two_stage_planning_routes_by_family_and_widens_on_failure() {
    let text = "한빛과 거래한 계약 중 작년 것만 골라서 보여줘";
    let chat = StageChat::new();
    chat.push("families", json!({ "families": ["ask"] }));
    chat.push("plan", json!({ "summary": "한빛 계약 검색", "intent": "search", "scope": { "filter": { "doc_type": "contract" } }, "operations": [{ "op": "search", "query": "한빛" }], "requires_confirmation": false, "confidence": 0.9 }));
    let r = plan(Some(&deps(&chat)), text, &ctx(), PlanMode::ModelFirst).unwrap();
    assert_eq!(r.families, vec![OpFamily::Ask]);
    assert!(matches!(&r.plan.operations[0], Op::Search { .. }), "{r:?}");
    let reqs = chat.requests.lock().unwrap();
    let route = reqs.iter().find(|r| StageChat::stage_of(r).as_deref() == Some("families")).unwrap();
    assert!(route.messages[0]["content"].as_str().unwrap().contains("- ask:"));
    let planned = reqs.iter().find(|r| StageChat::stage_of(r).as_deref() == Some("plan")).unwrap();
    let system = planned.messages[0]["content"].as_str().unwrap();
    assert!(system.contains("- search:") && !system.contains("- aggregate:"), "family table only");
    let ops = planned.response_format.as_ref().unwrap()["json_schema"]["schema"]["properties"]["operations"]["items"]
        ["anyOf"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(ops, OpFamily::Ask.ops().len());
    drop(reqs);
    // The family grammar cannot express the answer twice → one more attempt with every op.
    let chat = StageChat::new();
    chat.push("families", json!({ "families": ["system"] }));
    chat.push("plan", json!({ "summary": "x", "intent": "noop", "scope": {}, "operations": [], "requires_confirmation": false, "confidence": 0.8 }));
    chat.push("plan", json!({ "summary": "x", "intent": "noop", "scope": {}, "operations": [], "requires_confirmation": false, "confidence": 0.8 }));
    chat.push("plan", json!({ "summary": "한빛 계약 검색", "intent": "search", "scope": {}, "operations": [{ "op": "search", "query": "한빛" }], "requires_confirmation": false, "confidence": 0.9 }));
    let r = plan(Some(&deps(&chat)), text, &ctx(), PlanMode::ModelFirst).unwrap();
    assert!(matches!(&r.plan.operations[0], Op::Search { .. }), "{r:?}");
    assert_eq!(r.attempts, 3);
    assert_eq!(chat.calls("plan"), 3);
    // Family prompts stay small: the whole stage-1 prompt is a fraction of the single-stage one.
    let fam = prompts::message_tokens(&prompts::family_messages(&ctx(), text, None));
    let full = prompts::message_tokens(&prompts::messages_for(&ctx(), text, None, prompts::Tier::Full, None));
    assert!(fam * 4 < full, "family prompt {fam} vs full {full}");
}

/// 0.5.0: the assistant's "현재 프로젝트" scope is the default scope of commands that name none
/// (rules and model plans alike); explicit documents / folders / projects and `add_to_project` /
/// `import` keep their own scope.
#[test]
fn scope_project_is_the_default_scope_of_commands() {
    let mut c = ctx();
    c.projects = vec![ProjectSummary { id: "p-hanbit".into(), name: "한빛 심사".into(), status: "active".into() }];
    c.scope_project = Some("p-hanbit".into());
    let planned = |text: &str| plan(None, text, &c, PlanMode::RulesFirst).unwrap();
    let r = planned("전부 OCR 해줘");
    assert!(matches!(&r.plan.operations[0], Op::Ocr { .. }), "{r:?}");
    assert_eq!(r.plan.scope.filter.project_id.as_deref(), Some("p-hanbit"), "{r:?}");
    let r = planned("지난달 영수증 합계 얼마야?");
    assert!(matches!(&r.plan.operations[0], Op::Aggregate { .. }), "{r:?}");
    assert_eq!(r.plan.scope.filter.project_id.as_deref(), Some("p-hanbit"), "filters intersect: {r:?}");
    assert!(r.plan.scope.filter.date_from.is_some());
    let r = planned("inbox 폴더의 영수증을 전부 처리해줘");
    assert_eq!(r.plan.scope.folder.as_deref(), Some("inbox"));
    assert!(r.plan.scope.filter.project_id.is_none(), "a named folder wins: {r:?}");
    let r = planned("선택한 문서 요약해줘");
    assert!(!r.plan.scope.doc_ids.is_empty() && r.plan.scope.filter.project_id.is_none(), "{r:?}");
    // Model-shaped plans without a filter get the project too; add_to_project does not.
    let mut p = plan_of(json!([{ "op": "answer", "question": "납품 기한은?" }]), json!({}));
    validate::validate(&mut p, &c, None);
    assert_eq!(p.scope.filter.project_id.as_deref(), Some("p-hanbit"));
    let mut p = plan_of(json!([{ "op": "add_to_project", "doc_ids": ["d9"] }]), json!({}));
    validate::validate(&mut p, &c, None);
    assert!(p.scope.filter.project_id.is_none(), "{p:?}");
    let mut p = plan_of(json!([{ "op": "search", "query": "x" }]), json!({ "doc_ids": ["d1a2"] }));
    validate::validate(&mut p, &c, None);
    assert!(p.scope.filter.project_id.is_none(), "explicit documents win: {p:?}");
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
fn model_first_answers_a_pasted_text_from_the_text_itself_not_the_library() {
    let body = "금번 과업은 당사가 수행하며 기한은 12월말까지로 하고 산출물은 설계서와 소스코드로 함. 검수는 발주처가 납품 후 2주 내에 완료하는 것으로 한다.";
    let text = format!("다음 문장을 다듬어 정리해줘: {body}");
    let chat = StageChat::new();
    // What happened on the user's PC: the model read it as a library-wide per-document summary.
    chat.push("plan", json!({ "summary": "제공된 문장을 다듬어 정리합니다", "intent": "summarize", "scope": {}, "operations": [{ "op": "summarize", "target": "each_doc" }], "requires_confirmation": false, "confidence": 0.95 }));
    let r = plan(Some(&deps(&chat)), &text, &ctx(), PlanMode::ModelFirst).unwrap();
    assert_eq!(r.source, PlanSource::Model);
    assert!(
        matches!(&r.plan.operations[0], Op::Answer { knowledge: Some(false), question, .. } if question == &text),
        "{:?}",
        r.plan
    );
    assert_eq!(r.plan.intent, OpKind::Answer);
    assert!(!r.plan.requires_confirmation);
    assert!(r.issues.iter().any(|i| i.code == "text_only_answer" && !i.blocking), "{:?}", r.issues);
    // The model that reads it right needs no correction, and the planner prompt teaches it the rule.
    let chat = StageChat::new();
    chat.push("plan", json!({ "summary": "문장을 다듬습니다", "intent": "answer", "scope": {}, "operations": [{ "op": "answer", "question": text, "cite": false, "knowledge": false }], "requires_confirmation": false, "confidence": 0.9 }));
    let r = plan(Some(&deps(&chat)), &text, &ctx(), PlanMode::ModelFirst).unwrap();
    assert!(matches!(&r.plan.operations[0], Op::Answer { knowledge: Some(false), .. }));
    assert!(!r.issues.iter().any(|i| i.code == "text_only_answer"));
    let msgs = prompts::messages_for(&ctx(), &text, None, prompts::Tier::Full, None);
    assert!(msgs[0]["content"].as_str().unwrap().contains("knowledge=false"));
}

#[test]
fn model_path_requeries_once_with_feedback() {
    let text = "한빛과 거래한 계약 중 작년 것만 골라서 보여줘";
    let chat = StageChat::new();
    // 1st reply: an unknown parameter → rejected → re-query with the error; 2nd reply: valid.
    chat.push("plan", json!({ "summary": "x", "intent": "search", "scope": {}, "operations": [{ "op": "search", "query": "한빛", "limit": 5 }], "requires_confirmation": false, "confidence": 0.8 }));
    chat.push("plan", json!({ "summary": "한빛 계약 검색", "intent": "search", "scope": { "filter": { "doc_type": "contract" } }, "operations": [{ "op": "search", "query": "한빛" }], "requires_confirmation": false, "confidence": 0.8 }));
    let r = plan(Some(&deps(&chat)), text, &ctx(), PlanMode::RulesFirst).unwrap();
    assert_eq!(r.source, PlanSource::Model);
    assert_eq!(r.attempts, 2);
    assert!(matches!(&r.plan.operations[0], Op::Search { query, .. } if query == "한빛"));
    // "작년" was resolved deterministically and filled in.
    assert_eq!(r.plan.scope.filter.date_from.as_deref(), Some("2025-01-01"));
    assert_eq!(r.model.as_deref(), Some("scripted"));
    assert_eq!(chat.calls("plan"), 2);
    // The re-query carried the previous output and the validator's finding (the family-routing
    // call that preceded the plan calls has no scripted reply here, so the plan requests are the
    // ones with the plan schema).
    let reqs = chat.requests.lock().unwrap();
    let plans: Vec<_> = reqs.iter().filter(|r| StageChat::stage_of(r).as_deref() == Some("plan")).collect();
    let second = plans[1];
    let last_user = second.messages.last().unwrap()["content"].as_str().unwrap().to_string();
    assert!(last_user.contains("limit") || last_user.contains("unknown field"), "{last_user}");
    assert!(second.messages[0]["content"].as_str().unwrap().contains("- aggregate:"));
    assert_eq!(second.temperature, Some(0.0));
    assert!(second.response_format.as_ref().unwrap()["json_schema"]["schema"]["properties"]["operations"].is_object());

    // two rejected attempts → clarification
    let chat = StageChat::new();
    chat.push("plan", json!({ "summary": "x", "intent": "search", "scope": {}, "operations": [], "requires_confirmation": false, "confidence": 0.8 }));
    chat.push("plan", json!({ "summary": "x", "intent": "search", "scope": {}, "operations": [{ "op": "bogus" }], "requires_confirmation": false, "confidence": 0.8 }));
    let r = plan(Some(&deps(&chat)), text, &ctx(), PlanMode::RulesFirst).unwrap();
    assert!(r.plan.is_clarification());
    assert_eq!(r.attempts, 2);
    assert!(has_errors(&r.issues));

    // low confidence → clarification with the model's question
    let chat = StageChat::new();
    chat.push("plan", json!({ "summary": "?", "intent": "search", "scope": {}, "operations": [{ "op": "search", "query": "한빛" }], "requires_confirmation": false, "confidence": 0.4, "clarification_question": "어느 거래처요?" }));
    let r = plan(Some(&deps(&chat)), text, &ctx(), PlanMode::RulesFirst).unwrap();
    assert!(r.plan.is_clarification());
    assert_eq!(r.plan.clarification_question.as_deref(), Some("어느 거래처요?"));

    // model failure → clarification + model_error issue (no Err)
    let chat = StageChat::new();
    *chat.fail_with.lock().unwrap() = Some("HTTP 500".into());
    let r = plan(Some(&deps(&chat)), text, &ctx(), PlanMode::RulesFirst).unwrap();
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
    let _ = plan(Some(&d), text, &c, PlanMode::RulesFirst).unwrap();
    {
        // 4K never gets the full prompt; the exact tier depends on the estimate.
        let reqs = chat.requests.lock().unwrap();
        let planned = reqs.iter().find(|r| StageChat::stage_of(r).as_deref() == Some("plan")).unwrap();
        assert!(planned.messages.len() <= 1 + 4 * 2 + 1);
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
    let r = plan(Some(&d), "영수증 중 한빛 것만 골라서 보여줘", &ctx(), PlanMode::RulesFirst).unwrap();
    assert_eq!(r.source, PlanSource::Model, "{r:?}");
    assert_eq!(r.attempts, 1);
    // The overflow hit the (first) family-routing call; the plan call then used a tier below Full.
    let reqs = chat.requests.lock().unwrap();
    let plans: Vec<_> = reqs.iter().filter(|r| StageChat::stage_of(r).as_deref() == Some("plan")).collect();
    assert_eq!(plans.len(), 1);
    let full =
        prompts::messages_for(&ctx(), "영수증 중 한빛 것만 골라서 보여줘", None, prompts::Tier::Full, None).len();
    assert!(plans[0].messages.len() < full, "{} < {full}", plans[0].messages.len());
}

/// 0.8.0 audit fixes on the deterministic side (validation, scope, phrase rules): `tests/deterministic_side.rs`.
mod deterministic_side;

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
            db.with(pipeline_repo::enable_legacy_builtins).unwrap();
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

    /// 0.5.0: `add_to_project` takes documents from outside the target project (the scope filter
    /// names the destination), and `as_reference` fills the project's reference group with role
    /// `knowledge` instead of the work documents.
    #[test]
    fn add_to_project_takes_outside_documents_and_reference_material() {
        use crate::knowledge::repo as knowledge_repo;
        use crate::projects::{repo as projects_repo, ProjectInput};
        let w = World::new();
        w.add_image("d1", "inbox/a.jpg");
        w.add_image("d2", "inbox/b.jpg");
        let project = w
            .db
            .with(|c| projects_repo::save_project(c, &ProjectInput { name: "한빛 납품".into(), ..Default::default() }))
            .unwrap();
        let scope = Scope {
            doc_ids: vec!["d1".into()],
            filter: ScopeFilter { project_id: Some(project.id.clone()), ..ScopeFilter::default() },
            ..Scope::default()
        };
        let plan = plan_of(
            vec![Op::AddToProject { project: Some(project.id.clone()), doc_ids: Vec::new(), as_reference: false }],
            scope,
        );
        let preview = exec::preview(&w.db, &w.paths, &plan, &ProcessingSettings::default(), Language::Ko).unwrap();
        assert_eq!(preview.document_count, 1, "the target project is not a membership filter");
        let ex = exec::run(&w.deps(None), &plan, &mut |_| {}).unwrap();
        assert!(matches!(ex.steps[0].status, StepStatus::Done), "{:?}", ex.steps[0]);
        assert_eq!(ex.steps[0].payload["added"], 1);
        let members = w.db.with(|c| projects_repo::documents_of(c, &project.id)).unwrap();
        assert_eq!(members.iter().map(|m| m.document.id.as_str()).collect::<Vec<_>>(), vec!["d1"]);
        // Reference material: the group is created on first use, the document's role changes.
        let plan = plan_of(
            vec![Op::AddToProject {
                project: Some(project.id.clone()),
                doc_ids: vec!["d2".into()],
                as_reference: true,
            }],
            Scope::default(),
        );
        let ex = exec::run(&w.deps(None), &plan, &mut |_| {}).unwrap();
        assert!(matches!(ex.steps[0].status, StepStatus::Done), "{:?}", ex.steps[0]);
        assert_eq!(
            (ex.steps[0].payload["added"].as_u64(), ex.steps[0].payload["as_reference"].as_bool()),
            (Some(1), Some(true))
        );
        assert!(ex.steps[0].summary.contains("참고 자료"), "{}", ex.steps[0].summary);
        let group = w.db.with(|c| knowledge_repo::reference_group_of_project(c, &project.id)).unwrap().unwrap();
        assert_eq!(group.name, "한빛 납품");
        assert_eq!(w.db.with(|c| knowledge_repo::collection_doc_ids(c, &group.id)).unwrap(), vec!["d2".to_string()]);
        assert_eq!(w.doc("d2").role, DocumentRole::Knowledge);
        assert_eq!(
            w.db.with(|c| projects_repo::documents_of(c, &project.id)).unwrap().len(),
            1,
            "work documents unchanged"
        );
    }

    /// 0.6.0: the new core ops run against the workspace — rotate (relative), title, role,
    /// text replacement, reference groups, glossary, editions, project edits / membership /
    /// deletion, templates, schema management and project suggestions.
    #[test]
    fn agent_ops_run_against_the_workspace() {
        use crate::knowledge::repo as knowledge_repo;
        use crate::projects::{repo as projects_repo, ProjectInput};
        let w = World::new();
        w.add_image("d1", "inbox/a.jpg");
        w.add_image("d2", "inbox/b.jpg");
        w.db.with(|c| docs::set_page_text(c, "d1", 1, "㈜한빛 공급계약 ㈜한빛")).unwrap();
        let run1 = |op: Op, scope: Scope| -> serde_json::Value {
            let plan = plan_of(vec![op], scope);
            let ex = exec::run(&w.deps(None), &plan, &mut |_| {}).unwrap();
            assert!(matches!(ex.steps[0].status, StepStatus::Done), "{:?}", ex.steps[0]);
            ex.steps[0].payload.clone()
        };
        let sel = |ids: &[&str]| Scope { doc_ids: ids.iter().map(|s| s.to_string()).collect(), ..Scope::default() };
        // rotate adds to the page's rotation
        let p = run1(Op::Rotate { degrees: 90, pages: Vec::new() }, sel(&["d1"]));
        assert_eq!(p["pages"], 1);
        assert_eq!(w.db.with(|c| docs::get_page(c, "d1", 1)).unwrap().unwrap().rotation, 90);
        run1(Op::Rotate { degrees: 90, pages: vec![1] }, sel(&["d1"]));
        assert_eq!(w.db.with(|c| docs::get_page(c, "d1", 1)).unwrap().unwrap().rotation, 180);
        // title / role
        run1(Op::SetTitle { title: "2026 공급계약".into() }, sel(&["d1"]));
        assert_eq!(w.doc("d1").title.as_deref(), Some("2026 공급계약"));
        run1(Op::SetRole { role: RoleParam::Knowledge }, sel(&["d1", "d2"]));
        assert_eq!(w.doc("d2").role, DocumentRole::Knowledge);
        // find / replace with history
        let p =
            run1(Op::EditText { find: "㈜한빛".into(), replace: "(주)한빛".into(), pages: Vec::new() }, sel(&["d1"]));
        assert_eq!(p["replaced"], 2);
        let text = w.db.with(|c| docs::page_text_state(c, "d1", 1)).unwrap().unwrap().text.unwrap();
        assert!(text.contains("(주)한빛") && !text.contains('㈜'), "{text}");
        // reference group: create → add (role) → rename → delete
        run1(
            Op::ManageReference {
                action: ReferenceAction::Create,
                group: None,
                name: Some("사규".into()),
                project: None,
            },
            Scope::default(),
        );
        let p = run1(
            Op::ManageReference {
                action: ReferenceAction::Add, group: Some("사규".into()), name: None, project: None
            },
            sel(&["d1"]),
        );
        assert_eq!(p["group"]["doc_count"], 1);
        run1(
            Op::ManageReference {
                action: ReferenceAction::Rename,
                group: Some("사규".into()),
                name: Some("회사 규정".into()),
                project: None,
            },
            Scope::default(),
        );
        assert_eq!(w.db.with(knowledge_repo::list_collections).unwrap()[0].name, "회사 규정");
        run1(
            Op::ManageReference {
                action: ReferenceAction::Delete,
                group: Some("회사 규정".into()),
                name: None,
                project: None,
            },
            Scope::default(),
        );
        assert!(w.db.with(knowledge_repo::list_collections).unwrap().is_empty());
        // glossary
        run1(
            Op::DefineTerm {
                action: TermAction::Save,
                term: "MM".into(),
                definition: Some("맨먼스".into()),
                synonyms: Vec::new(),
            },
            Scope::default(),
        );
        assert_eq!(w.db.with(knowledge_repo::list_glossary).unwrap()[0].definition.as_deref(), Some("맨먼스"));
        run1(
            Op::DefineTerm { action: TermAction::Delete, term: "mm".into(), definition: None, synonyms: Vec::new() },
            Scope::default(),
        );
        assert!(w.db.with(knowledge_repo::list_glossary).unwrap().is_empty());
        // editions
        run1(
            Op::SetVersion {
                group_key: Some("공급계약".into()),
                label: Some("2024".into()),
                effective_date: None,
                latest: Some(false),
                clear: None,
            },
            sel(&["d1"]),
        );
        let groups = w.db.with(knowledge_repo::list_version_groups).unwrap();
        assert_eq!(groups[0].documents[0].version_label.as_deref(), Some("2024"));
        run1(
            Op::SetVersion { group_key: None, label: None, effective_date: None, latest: None, clear: Some(true) },
            sel(&["d1"]),
        );
        assert!(w.db.with(knowledge_repo::list_version_groups).unwrap().is_empty());
        // projects: membership, update, removal, suggestions, deletion
        let project = w
            .db
            .with(|c| projects_repo::save_project(c, &ProjectInput { name: "한빛 납품".into(), ..Default::default() }))
            .unwrap();
        run1(
            Op::AddToProject {
                project: Some(project.id.clone()),
                doc_ids: vec!["d1".into(), "d2".into()],
                as_reference: false,
            },
            Scope::default(),
        );
        let p = run1(
            Op::UpdateProject {
                project: Some(project.id.clone()),
                name: Some("한빛 2차".into()),
                template: None,
                start_date: None,
                end_date: Some("2026-12-31".into()),
                owner: Some("김담당".into()),
                intake_no: None,
                notes: None,
                keywords: None,
                inbox_folder: None,
            },
            Scope::default(),
        );
        assert_eq!(p["project"]["name"], "한빛 2차");
        assert_eq!(p["project"]["end_date"], "2026-12-31");
        assert_eq!(p["project"]["owner"], "김담당");
        let p = run1(
            Op::RemoveFromProject { project: Some(project.id.clone()), doc_ids: vec!["d2".into()] },
            Scope::default(),
        );
        assert_eq!(p["removed"], 1);
        assert_eq!(w.db.with(|c| projects_repo::documents_of(c, &project.id)).unwrap().len(), 1);
        let p = run1(Op::ClassifyProjects { apply: Some(false), use_model: Some(false) }, Scope::default());
        assert!(p["suggested"].is_number() && p["applied"] == 0, "{p}");
        // templates (user-defined only)
        run1(
            Op::ManageTemplate {
                action: TemplateAction::Save,
                template: None,
                name: Some("유지보수 2차".into()),
                doc_types_required: vec!["estimate".into()],
                description: None,
            },
            Scope::default(),
        );
        let t =
            w.db.with(projects_repo::list_templates)
                .unwrap()
                .into_iter()
                .find(|t| t.name_ko == "유지보수 2차")
                .unwrap();
        assert_eq!(t.required_doc_types, vec!["estimate".to_string()]);
        run1(
            Op::ManageTemplate {
                action: TemplateAction::Delete,
                template: Some("유지보수 2차".into()),
                name: None,
                doc_types_required: Vec::new(),
                description: None,
            },
            Scope::default(),
        );
        assert!(!w.db.with(projects_repo::list_templates).unwrap().iter().any(|t| t.name_ko == "유지보수 2차"));
        // schemas: toggle a built-in, refuse to delete it
        let p = run1(
            Op::ManageSchema { action: SchemaAction::Disable, schema: "estimate".into(), name: None },
            Scope::default(),
        );
        assert_eq!(p["schema"]["enabled"], false);
        run1(
            Op::ManageSchema { action: SchemaAction::Enable, schema: "estimate".into(), name: None },
            Scope::default(),
        );
        let plan = plan_of(
            vec![Op::ManageSchema { action: SchemaAction::Delete, schema: "estimate".into(), name: None }],
            Scope::default(),
        );
        let ex = exec::run(&w.deps(None), &plan, &mut |_| {}).unwrap();
        assert!(matches!(ex.steps[0].status, StepStatus::Failed), "{:?}", ex.steps[0]);
        assert!(ex.steps[0].error.as_deref().unwrap_or_default().contains("schema_builtin"), "{:?}", ex.steps[0].error);
        // delete the project (documents stay)
        let p = run1(Op::DeleteProject { project: Some(project.id.clone()) }, Scope::default());
        assert_eq!(p["deleted"], true);
        assert!(w.db.with(|c| projects_repo::get_project(c, &project.id)).unwrap().is_none());
        assert_eq!(w.doc("d1").id, "d1");
        // previews of the confirmed ops carry their counts
        w.db.with(|c| docs::set_page_text(c, "d2", 1, "㈜한빛")).unwrap();
        let plan = plan_of(
            vec![Op::EditText { find: "㈜한빛".into(), replace: "x".into(), pages: Vec::new() }],
            sel(&["d1", "d2"]),
        );
        let preview = exec::preview(&w.db, &w.paths, &plan, &ProcessingSettings::default(), Language::Ko).unwrap();
        assert_eq!(preview.steps[0].payload["matches"], 1, "{:?}", preview.steps[0]);
        assert!(preview.steps[0].confirm);
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
            unmet: None,
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

    #[test]
    fn define_rule_without_a_project_targets_user_templates_only() {
        use crate::projects::{repo as projects_repo, TemplateInput};
        let w = World::new();
        w.db.with(projects_repo::ensure_builtins).unwrap();
        // The rule below is written against the legacy 구매 정산 template.
        w.db.with(|c| projects_repo::ensure_templates(c, &projects_repo::legacy_templates())).unwrap();
        let tpl =
            w.db.with(|c| {
                projects_repo::save_template(
                    c,
                    &TemplateInput {
                        name_ko: "구매 정산 사본".into(),
                        required_doc_types: vec!["transaction_statement".into(), "tax_invoice".into()],
                        ..Default::default()
                    },
                )
            })
            .unwrap();

        // A confident rule-based draft is appended to the (name-resolved) template's rules.
        let plan = plan_of(
            vec![Op::DefineRule {
                project: None,
                template: Some("구매 정산 사본".into()),
                description: "거래명세서 합계 = 세금계산서 합계금액".into(),
            }],
            Scope::default(),
        );
        let ex = exec::run(&w.deps(None), &plan, &mut |_| {}).unwrap();
        assert_eq!(ex.steps[0].status, StepStatus::Done, "{:?}", ex.steps[0]);
        assert_eq!(ex.steps[0].payload["saved"], json!(true), "{:?}", ex.steps[0].payload);
        assert_eq!(ex.steps[0].payload["template_id"], json!(tpl.id));
        assert!(ex.steps[0].payload["project_id"].is_null());
        assert!(ex.steps[0].summary.contains("템플릿에 규칙 저장"), "{}", ex.steps[0].summary);
        let after = w.db.with(|c| projects_repo::get_template(c, &tpl.id)).unwrap().unwrap();
        assert_eq!(after.rules.len(), 1, "{after:?}");
        assert_eq!(after.rules[0].left.doc_type.as_deref(), Some("transaction_statement"), "{after:?}");
        assert_eq!(w.chat.calls("project_rule"), 0, "a rule the phrase rules resolve never asks the model");

        // Built-in templates take no rules; unknown templates are reported, not drafted.
        let plan = plan_of(
            vec![Op::DefineRule {
                project: None,
                template: Some("tpl_purchase_settlement".into()),
                description: "거래명세서 합계 = 세금계산서 합계금액".into(),
            }],
            Scope::default(),
        );
        let ex = exec::run(&w.deps(None), &plan, &mut |_| {}).unwrap();
        assert_eq!(ex.steps[0].status, StepStatus::Failed);
        assert!(ex.steps[0].error.as_deref().unwrap().contains("template_builtin"), "{:?}", ex.steps[0]);
        let plan = plan_of(
            vec![Op::DefineRule {
                project: None,
                template: Some("없는 템플릿".into()),
                description: "거래명세서 합계 = 세금계산서 합계금액".into(),
            }],
            Scope::default(),
        );
        let ex = exec::run(&w.deps(None), &plan, &mut |_| {}).unwrap();
        assert_eq!(ex.steps[0].status, StepStatus::Failed);
        assert!(ex.steps[0].error.as_deref().unwrap().contains("없는 템플릿"), "{:?}", ex.steps[0]);
        assert_eq!(w.db.with(|c| projects_repo::get_template(c, &tpl.id)).unwrap().unwrap().rules.len(), 1);
    }

    /// 0.8.0 audit fixes in the executor and its previews: `tests/exec_tests/audit_fixes.rs`.
    mod audit_fixes;
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
    // Two-stage path: the routing call, then one family's op table per plan call.
    eprintln!("routing: est={}", prompts::message_tokens(&prompts::family_messages(&c, text, None)));
    for family in OpFamily::ALL {
        let kinds = family.ops();
        let sizes: Vec<String> = [prompts::Tier::Full, prompts::Tier::Compact, prompts::Tier::Minimal]
            .into_iter()
            .map(|tier| {
                let msgs = prompts::messages_for_kinds(&c, text, None, tier, None, Some(&kinds));
                format!("{tier:?}={}", prompts::message_tokens(&msgs))
            })
            .collect();
        let grammar = crate::jsonorder::to_string(&schema::plan_schema_for(&kinds)).len();
        eprintln!("{}: {} · schema {} bytes", family.as_str(), sizes.join(" "), grammar);
    }
    eprintln!("all ops: schema {} bytes", crate::jsonorder::to_string(&schema::plan_schema()).len());
}

#[test]
fn context_block_lists_the_last_conversation_turns() {
    let mut c = ctx();
    c.history = (0..6).map(|i| crate::knowledge::ConversationTurn::user(&format!("명령 {i}"))).collect();
    let block = super::prompts::context_block(&c, "", false);
    assert!(block.contains("이전 대화"), "{block}");
    assert!(
        !block.contains("명령 1\n") && block.contains("- 사용자: 명령 2") && block.contains("- 사용자: 명령 5"),
        "{block}"
    );
    assert!(!super::prompts::context_block(&ctx(), "", false).contains("이전 대화"));
}

#[test]
fn model_first_mode_sends_every_sentence_but_slash_commands_to_the_model() {
    let c = ctx();
    // A sentence the phrase rules would catch goes to the model instead.
    let chat = StageChat::new();
    chat.push(
        "plan",
        json!({ "summary": "한빛 계약 검색", "unmet": "", "intent": "search", "scope": {}, "operations": [{ "op": "search", "query": "한빛 계약" }], "requires_confirmation": false, "confidence": 0.9 }),
    );
    assert_eq!(plan(None, "한빛 계약서 찾아줘", &c, PlanMode::RulesFirst).unwrap().source, PlanSource::Rules);
    let r = plan(Some(&deps(&chat)), "한빛 계약서 찾아줘", &c, PlanMode::ModelFirst).unwrap();
    assert_eq!(r.source, PlanSource::Model, "{r:?}");
    assert_eq!(chat.calls("plan"), 1);
    assert!(matches!(r.plan.operations[0], Op::Search { .. }));
    // A sentence that is nothing but a setting's name and a value is read without the model (0.8.0 fast path).
    let r = plan(Some(&deps(&chat)), "다크 테마로 바꿔줘", &c, PlanMode::ModelFirst).unwrap();
    assert_eq!(r.source, PlanSource::Fast, "{r:?}");
    assert_eq!(chat.calls("plan"), 1);
    assert!(matches!(r.plan.operations[0], Op::SetTheme { theme: Theme::Dark }));
    // Slash commands never reach the model.
    let r = plan(Some(&deps(&chat)), "/테마 라이트", &c, PlanMode::ModelFirst).unwrap();
    assert_eq!(r.source, PlanSource::Prerouter);
    assert_eq!(chat.calls("plan"), 1);
    // Without a model the result is a clarification that asks for one — no keyword guess.
    let r = plan(None, "지난달 영수증 합계", &c, PlanMode::ModelFirst).unwrap();
    assert_eq!(r.source, PlanSource::None);
    assert!(r.plan.is_clarification());
    assert!(r.issues.iter().any(|i| i.code == "no_model"));
    assert!(r.plan.clarification_question.as_deref().unwrap_or("").contains("모델"));
    // The legacy mode still answers the same sentence by rule.
    let r = plan(None, "지난달 영수증 합계", &c, PlanMode::RulesFirst).unwrap();
    assert_eq!(r.source, PlanSource::Rules);
}

// ---------------------------------------------------------------------------
// 0.8.0 plan §1: the planner contract (root causes of the 2026-11-11 report)
// ---------------------------------------------------------------------------

/// Audit PLN-1: llama.cpp writes required properties in the order of the schema's `properties`,
/// and a `serde_json::Value` map sorts them by name — the model had to commit to `confidence`
/// before it wrote what it understood, and `set_setting` had to start with a key. The request
/// body now follows each schema's `required` order.
#[test]
fn the_grammar_makes_the_model_write_what_it_understood_first() {
    use crate::jsonorder;
    let schema = schema::plan_schema();
    let body = jsonorder::to_string(&schema);
    // Top level: the interpretation, what is left out, then the plan, and the confidence last.
    let props = &body[body.find("\"properties\":{").unwrap()..];
    let pos = |key: &str| props.find(&format!("\"{key}\":")).unwrap_or_else(|| panic!("{key} missing"));
    let order = ["summary", "unmet", "intent", "scope", "operations", "requires_confirmation", "confidence"];
    for pair in order.windows(2) {
        assert!(pos(pair[0]) < pos(pair[1]), "{} must come before {}", pair[0], pair[1]);
    }
    assert_eq!(schema["required"], json!(order));
    // Every op alternative starts with its name, then its required parameters in spec order.
    for alternative in schema["properties"]["operations"]["items"]["anyOf"].as_array().unwrap() {
        let written = jsonorder::to_string(alternative);
        let keys = jsonorder::top_level_keys(&written[written.find("\"properties\":").unwrap() + 13..]);
        let required: Vec<String> =
            alternative["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        assert_eq!(required[0], "op", "{written}");
        assert_eq!(keys[..required.len()], required[..], "{written}");
    }
    // The model's own reply, written back as an example, has the same order.
    let reply = prompts::example_reply(
        &json!({ "confidence": 0.9, "intent": "search", "operations": [{ "query": "한빛", "op": "search" }], "requires_confirmation": false, "scope": {}, "summary": "한빛 검색" }),
        &schema,
    );
    assert_eq!(
        reply,
        r#"{"summary":"한빛 검색","unmet":"","intent":"search","scope":{},"operations":[{"op":"search","query":"한빛"}],"requires_confirmation":false,"confidence":0.9}"#
    );
    // Every few-shot of every tier is written that way and parses as a plan.
    for ko in [true, false] {
        let mut c = ctx();
        c.language = if ko { Language::Ko } else { Language::En };
        for tier in [prompts::Tier::Full, prompts::Tier::Compact, prompts::Tier::Minimal] {
            let messages = prompts::messages_for(&c, "x", None, tier, None);
            for m in messages.iter().filter(|m| m["role"] == "assistant") {
                let content = m["content"].as_str().unwrap();
                assert!(content.starts_with("{\"summary\":"), "{content}");
                // (an optional `clarification_question` follows the required keys)
                let keys = jsonorder::top_level_keys(content);
                assert_eq!(keys[..order.len()], order, "{content}");
                let value: Value = serde_json::from_str(content).unwrap();
                validate::parse_plan(&value).unwrap_or_else(|e| panic!("{content}: {e:?}"));
            }
        }
    }
}

/// 0.8.0 plan §1.5: a settings change pairs a key only with a value of that key's type.
#[test]
fn settings_grammar_pairs_each_key_with_its_value_type() {
    use super::settings_catalog as catalog;
    let alternatives = schema::setting_schemas();
    let keys_of = |a: &Value| -> Vec<String> {
        a["properties"]["key"]["enum"].as_array().unwrap().iter().map(|k| k.as_str().unwrap().to_string()).collect()
    };
    let all: Vec<String> = alternatives.iter().flat_map(keys_of).collect();
    let unique: std::collections::BTreeSet<&String> = all.iter().collect();
    assert_eq!(all.len(), unique.len(), "a key belongs to one alternative");
    assert_eq!(all.len(), catalog::SETTINGS.len());
    for a in &alternatives {
        assert_eq!(a["required"], json!(["op", "key", "value"]));
    }
    let alternative_of = |key: &str| alternatives.iter().find(|a| keys_of(a).iter().any(|k| k == key)).unwrap();
    assert_eq!(alternative_of("ui.scale_percent")["properties"]["value"], json!({ "type": "integer" }));
    assert_eq!(alternative_of("processing.render_dpi")["properties"]["value"], json!({ "type": "integer" }));
    assert_eq!(alternative_of("privacy.pii_masking")["properties"]["value"], json!({ "type": "boolean" }));
    let theme = &alternative_of("ui.theme")["properties"]["value"]["enum"];
    assert!(theme.as_array().unwrap().contains(&json!("dark")), "{theme}");
    assert!(!keys_of(alternative_of("ui.theme")).iter().any(|k| k == "models.profile"));
    // The prompt's table names every setting in the user's language, with its range.
    let table = schema::settings_table(true);
    assert!(table.contains("ui.scale_percent = UI 배율(%) [90~150]"), "{table}");
    assert_eq!(table.lines().count(), catalog::SETTINGS.len());
    assert!(schema::op_table_for(&OpFamily::System.ops(), true, false).contains("ui.scale_percent = UI 배율(%)"));
    assert!(!schema::op_table_for(&OpFamily::Ask.ops(), true, false).contains("ui.scale_percent"));
}

/// 0.8.0 plan §1.2 (audit USR-3 / PLN-2): a turn that is not a command is answered without a plan
/// call — small talk without any lookup, a general question with evidence when there is some.
#[test]
fn turns_that_are_not_commands_are_answered_without_a_plan() {
    let c = ctx();
    let chat = StageChat::new();
    chat.push("families", json!({ "families": ["chat"] }));
    let r = plan(Some(&deps(&chat)), "오늘 좀 피곤하네", &c, PlanMode::ModelFirst).unwrap();
    assert_eq!((r.source, r.route.as_deref(), r.attempts), (PlanSource::Model, Some("chat"), 0), "{r:?}");
    assert_eq!(chat.calls("plan"), 0);
    assert!(matches!(
        &r.plan.operations[..],
        [Op::Answer { mode: Some(AnswerMode::Chat), knowledge: Some(false), reply: None, question, .. }]
            if question == "오늘 좀 피곤하네"
    ));
    assert!(!r.plan.requires_confirmation && r.plan.scope.is_empty() && r.issues.is_empty(), "{r:?}");

    let chat = StageChat::new();
    chat.push("families", json!({ "families": ["general"] }));
    let r = plan(Some(&deps(&chat)), "양자컴퓨터가 뭐야?", &c, PlanMode::ModelFirst).unwrap();
    assert_eq!(r.route.as_deref(), Some("general"));
    assert_eq!(chat.calls("plan"), 0);
    assert!(matches!(&r.plan.operations[..], [Op::Answer { mode: Some(AnswerMode::General), knowledge: None, .. }]));
    assert!(!r.plan.requires_confirmation);

    // A reply route next to a family is a command: "고마워, 견적서 합계도 알려줘".
    let chat = StageChat::new();
    chat.push("families", json!({ "families": ["chat", "ask"] }));
    chat.push("plan", json!({ "summary": "한빛 검색", "unmet": "", "intent": "search", "scope": {}, "operations": [{ "op": "search", "query": "한빛" }], "requires_confirmation": false, "confidence": 0.9 }));
    let r = plan(Some(&deps(&chat)), "고마워, 한빛 계약도 찾아줘", &c, PlanMode::ModelFirst).unwrap();
    assert_eq!((r.route.as_deref(), r.families.clone()), (None, vec![OpFamily::Ask]), "{r:?}");
    assert!(matches!(&r.plan.operations[0], Op::Search { .. }));

    // The routing prompt and grammar know both routes; the reply plan round-trips through serde.
    let routes = schema::family_schema();
    let names = routes["properties"]["families"]["items"]["enum"].as_array().unwrap();
    assert!(names.contains(&json!("chat")) && names.contains(&json!("general")), "{routes}");
    assert_eq!(names.len(), OpFamily::ALL.len() + 2);
    let system = prompts::family_messages(&c, "x", None)[0]["content"].as_str().unwrap().to_string();
    assert!(system.contains("- chat:") && system.contains("- general:"), "{system}");
    let v = serde_json::to_value(&r.plan).unwrap();
    assert_eq!(serde_json::from_value::<Plan>(v).unwrap(), r.plan);
    let chat_plan = plan(
        Some(&deps(&{
            let chat = StageChat::new();
            chat.push("families", json!({ "families": ["chat"] }));
            chat
        })),
        "그렇구나",
        &c,
        PlanMode::ModelFirst,
    )
    .unwrap()
    .plan;
    let v = serde_json::to_value(&chat_plan).unwrap();
    assert_eq!(v["operations"][0]["mode"], "chat");
    assert_eq!(serde_json::from_value::<Plan>(v).unwrap(), chat_plan);
}

/// 0.8.0 plan §1.6 (audit USR-1 / USR-2): a sentence that is nothing but a setting's name and a
/// value never reaches the model, in either interpretation mode.
#[test]
fn whole_sentence_settings_and_greetings_skip_the_model_in_both_modes() {
    let c = ctx();
    for mode in [PlanMode::ModelFirst, PlanMode::RulesFirst] {
        let chat = StageChat::new();
        let d = deps(&chat);
        for (text, key, value) in [
            ("UI 배율 120%", "ui.scale_percent", json!(120)),
            ("ui배율을 100%로 바꿔줘", "ui.scale_percent", json!(100)),
            ("배율을 130 으로 변경해줘", "ui.scale_percent", json!(130)),
            ("DPI 를 200 으로 변경해줘", "processing.render_dpi", json!(200)),
        ] {
            let r = plan(Some(&d), text, &c, mode).unwrap();
            assert_eq!(r.source, PlanSource::Fast, "{mode:?} {text}: {r:?}");
            assert!(
                matches!(&r.plan.operations[..], [Op::SetSetting { key: k, value: v }] if k == key && *v == value),
                "{mode:?} {text}: {:?}",
                r.plan
            );
            assert!(!r.plan.requires_confirmation && !has_errors(&r.issues), "{text}: {r:?}");
        }
        let r = plan(Some(&d), "프로파일 정확도", &c, mode).unwrap();
        assert_eq!(r.source, PlanSource::Fast);
        assert!(matches!(&r.plan.operations[..], [Op::SetProfile { profile: Profile::Accuracy }]));
        let r = plan(Some(&d), "안녕하세요.", &c, mode).unwrap();
        assert_eq!(r.source, PlanSource::Fast);
        assert!(matches!(&r.plan.operations[..], [Op::Answer { mode: Some(AnswerMode::Chat), reply: Some(_), .. }]));
        // A value outside the range is asked about instead of clamped.
        let r = plan(Some(&d), "UI 배율 200", &c, mode).unwrap();
        assert_eq!(r.source, PlanSource::Fast);
        assert!(r.plan.is_clarification(), "{r:?}");
        assert!(chat.requests.lock().unwrap().is_empty(), "no model call in {mode:?}");
    }
    // Without a model the fast path still works; a slash command stays the pre-router's.
    assert_eq!(plan(None, "UI 배율 120%", &c, PlanMode::ModelFirst).unwrap().source, PlanSource::Fast);
    assert_eq!(plan(None, "/테마 라이트", &c, PlanMode::ModelFirst).unwrap().source, PlanSource::Prerouter);
}

/// 0.8.0 plan §1.3 / §1.5: a model plan is checked against the sentence before it may run unasked.
#[test]
fn model_plans_are_checked_against_the_sentence() {
    let c = ctx();
    let reply = |ops: Value, unmet: &str| json!({ "summary": "s", "unmet": unmet, "intent": ops[0]["op"], "scope": {}, "operations": ops, "requires_confirmation": false, "confidence": 0.95 });
    let run = |text: &str, families: &str, plan_reply: Value| {
        let chat = StageChat::new();
        chat.push("families", json!({ "families": [families] }));
        chat.push("plan", plan_reply);
        let r = plan(Some(&deps(&chat)), text, &c, PlanMode::ModelFirst).unwrap();
        assert_eq!(r.source, PlanSource::Model, "{text}: {r:?}");
        r
    };
    let codes = |r: &PlanResult| r.issues.iter().map(|i| i.code.clone()).collect::<Vec<_>>();
    // The 0.7.0 misreading of a settings sentence the fast path leaves to the model.
    let text = "화면 글씨가 너무 작아서 그런데 좀 키워줄 수 있어?";
    assert!(super::fastpath::plan(text, &c).is_none());
    let r = run(text, "system", reply(json!([{ "op": "set_profile", "profile": "balanced" }]), ""));
    assert_eq!(codes(&r), vec!["ungrounded_change"], "{r:?}");
    assert!(r.plan.requires_confirmation && !has_errors(&r.issues));
    assert!(matches!(&r.plan.operations[..], [Op::SetProfile { .. }]), "the plan is kept for the user to judge");
    // A change the sentence names runs as before.
    let r = run(
        "눈이 아파서 그런데 다크 테마로 바꿔줄래",
        "system",
        reply(json!([{ "op": "set_theme", "theme": "dark" }]), ""),
    );
    assert!(codes(&r).is_empty() && !r.plan.requires_confirmation, "{r:?}");
    // The reported grouping sentence: one summary of everything is not what was asked. The
    // criterion is in the sentence, so the 0.7.0 plan is repaired; the right plan passes as it is.
    let text = "프로젝트 별 내용을 요약해줘";
    for ops in [
        json!([{ "op": "summarize", "target": "set" }]),
        json!([{ "op": "summarize", "target": "set", "group_by": "project" }]),
    ] {
        let r = run(text, "ask", reply(ops, ""));
        assert!(codes(&r).is_empty() && r.plan.unmet.is_none() && !r.plan.requires_confirmation, "{r:?}");
        assert!(matches!(&r.plan.operations[..], [Op::Summarize { group_by: Some(SummaryGroup::Project), .. }]));
    }
    // A criterion the op does not have is not replaced by "everything as one" behind the user's back.
    let r = run("견적서를 담당자별로 요약해줘", "ask", reply(json!([{ "op": "summarize", "target": "set" }]), ""));
    // (the estimates the sentence names became the scope the model's plan had left empty)
    assert_eq!(codes(&r), vec!["scope_filled", "grouping_dropped"], "{r:?}");
    assert!(r.plan.requires_confirmation && r.plan.unmet.is_some());
    // What the model itself reports as left out.
    let r = run(
        "지난달 견적서를 거래처별 표로 만들어 메일로 보내줘",
        "output",
        reply(json!([{ "op": "aggregate", "fn": "sum", "field": "total", "group_by": "vendor" }]), "메일로 보내기"),
    );
    assert!(codes(&r).contains(&"unmet_request".to_string()), "{r:?}");
    assert!(r.plan.requires_confirmation);
    assert_eq!(r.plan.unmet.as_deref(), Some("메일로 보내기"));
    // A value the setting does not accept, twice: the question names the setting and its range.
    let chat = StageChat::new();
    chat.push("families", json!({ "families": ["system"] }));
    chat.push("plan", reply(json!([{ "op": "set_setting", "key": "ui.scale_percent", "value": 300 }]), ""));
    let r = plan(Some(&deps(&chat)), "글자가 너무 작으니 화면을 세 배로 키워줘", &c, PlanMode::ModelFirst).unwrap();
    assert!(r.plan.is_clarification(), "{r:?}");
    let question = r.plan.clarification_question.clone().unwrap_or_default();
    assert!(question.contains("UI 배율(%)") && question.contains("90~150"), "{question}");
    assert!(!question.contains("must be"), "{question}");
    // "Nothing to do" is answered, not shown as an empty plan.
    let r = run("음, 그렇구나.", "system", reply(json!([{ "op": "noop" }]), ""));
    assert!(matches!(&r.plan.operations[..], [Op::Answer { mode: Some(AnswerMode::Chat), .. }]), "{r:?}");
    assert!(!r.plan.requires_confirmation && r.plan.scope.is_empty());
}

/// "프로젝트별로 요약해줘" asks for every project: the assistant's "현재 프로젝트" scope is not a
/// filter on it, by rule or by model; any other summary inside the scope stays in the project.
#[test]
fn a_summary_per_project_is_not_narrowed_to_the_open_project() {
    let mut c = ctx();
    c.projects = vec![ProjectSummary { id: "p-hanbit".into(), name: "한빛 납품".into(), status: "active".into() }];
    c.scope_project = Some("p-hanbit".into());
    let project_of = |p: &Plan| p.scope.filter.project_id.clone();
    let r = plan(None, "프로젝트별로 요약해줘", &c, PlanMode::RulesFirst).unwrap();
    assert!(matches!(&r.plan.operations[..], [Op::Summarize { group_by: Some(SummaryGroup::Project), .. }]), "{r:?}");
    assert_eq!(project_of(&r.plan), None, "{r:?}");
    let chat = StageChat::new();
    chat.push("families", json!({ "families": ["ask"] }));
    chat.push("plan", json!({ "summary": "s", "unmet": "", "intent": "summarize", "scope": {}, "operations": [{ "op": "summarize", "target": "set" }], "requires_confirmation": false, "confidence": 0.9 }));
    let r = plan(Some(&deps(&chat)), "프로젝트 별 내용을 요약해줘", &c, PlanMode::ModelFirst).unwrap();
    assert!(matches!(&r.plan.operations[..], [Op::Summarize { group_by: Some(SummaryGroup::Project), .. }]), "{r:?}");
    // The validator filled the open project in before the split was put back: taken out again.
    assert_eq!(project_of(&r.plan), None, "{r:?}");
    let r = plan(None, "유형별로 요약해줘", &c, PlanMode::RulesFirst).unwrap();
    assert_eq!(project_of(&r.plan).as_deref(), Some("p-hanbit"), "{r:?}");
}
