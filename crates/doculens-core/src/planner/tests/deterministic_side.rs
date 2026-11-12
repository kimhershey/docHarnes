//! 0.8.0 audit fixes on the planner's deterministic side: validation, scope resolution and the
//! phrase rules (the executor cases live in `exec_tests/audit_fixes.rs`).

use super::*;

/// UI-13: summaries pick the particle from the word instead of printing "을(를)" / "(으)로".
#[test]
fn rule_summaries_pick_the_korean_particle() {
    let summary = |text: &str| planned(text).plan.summary.unwrap_or_default();
    // The sentence of the audit ("문서을(를) 유형별 폴더로 이동합니다").
    assert_eq!(summary("전체 문서를 유형별로 정리해줘"), "문서를 유형별 폴더로 이동합니다");
    assert_eq!(summary("세금계산서 → 엑셀"), "세금계산서를 XLSX 파일로 내보냅니다");
    assert_eq!(summary("영수증 csv로 내보내"), "영수증을 CSV 파일로 내보냅니다");
    assert_eq!(summary("작년 계약서 전부 삭제"), "계약서를 휴지통으로 보냅니다");
    assert_eq!(summary("이 문서를 참고 자료로 표시"), "참고 자료로 표시합니다");
    assert_eq!(summary("이 문서를 양식으로 표시"), "양식으로 표시합니다");
    assert_eq!(summary("제목을 '2026 공급계약'으로 바꿔"), "제목을 '2026 공급계약'으로 바꿉니다");
    assert_eq!(summary("제목을 '한빛 2차'로 바꿔"), "제목을 '한빛 2차'로 바꿉니다");
    assert_eq!(summary("'사규' 참고 자료 묶음 만들어"), "참고 자료 묶음 '사규'를 만듭니다");
    assert_eq!(summary("'규정집' 참고 자료 묶음 만들어"), "참고 자료 묶음 '규정집'을 만듭니다");
    assert_eq!(summary("'㈜한빛'을 '(주)한빛'으로 바꿔줘"), "'㈜한빛'을 '(주)한빛'으로 바꿉니다");
    assert_eq!(summary("선택 문서에 태그 경비, 10월 추가해줘"), "태그 경비, 10월을 추가합니다");
    assert_eq!(summary("원본 문서는 무기한 보존"), "원본 문서 보존 기간을 무기한으로 설정합니다");
    assert_eq!(summary("저널은 30일만 보관해"), "파일 조작 저널 보존 기간을 30일로 설정합니다");
    // A Latin id never carries the particle: it follows a Korean noun instead.
    assert_eq!(summary("12B 모델 받아줘"), "gemma4-12b-q4km 모델을 내려받습니다");
    for text in [
        "inbox 폴더의 영수증을 전부 처리하고 월별 폴더로 정리해줘",
        "검토 완료된 영수증 거래처별 폴더로 옮겨",
        "이 문서 요약해줘",
        "거래처명이 ㈜한빛인 문서 찾아줘",
        "영수증 양식 기본으로",
        "지난달 영수증 집계 결과로 경비 보고서 초안 작성해줘 pdf로",
    ] {
        let s = summary(text);
        assert!(!s.is_empty() && !s.contains("을(를)") && !s.contains("(으)로"), "{text} → {s}");
    }
}

/// 1.0.0: the phrase rules with the two thresholds and the "이미지 내 글자 추출" setting.
#[test]
fn rules_ask_which_threshold_and_do_not_extract_for_a_settings_name() {
    let op = |text: &str| planned(text).plan.operations.into_iter().next();
    let setting = |text: &str| match op(text) {
        Some(Op::SetSetting { key, value }) => Some((key, value)),
        _ => None,
    };
    // A threshold the sentence names is set; a bare one is asked about (there are two since 1.0.0).
    assert_eq!(setting("검토 임계값 85%로 바꿔줘"), Some(("processing.review_threshold".into(), json!(0.85))));
    assert_eq!(setting("신뢰도 기준 0.9로 설정"), Some(("processing.review_threshold".into(), json!(0.9))));
    assert_eq!(
        setting("2순위 추천 임계값을 70%로 올려줘").map(|(k, _)| k),
        Some("processing.project_second_choice".to_string())
    );
    let bare = planned("임계값 70%로 바꿔줘").plan;
    assert!(bare.is_clarification(), "{bare:?}");
    assert!(bare.clarification_question.as_deref().unwrap().contains("2순위 추천 임계값"));
    // The setting's name contains "추출": a report about it must not start an extraction.
    for text in ["이미지 내 글자 추출 켰어", "그림 글자 추출이 켜져 있나?", "이미지 내 글자 추출 설정 바꿨어"]
    {
        let p = planned(text).plan;
        assert!(!p.operations.iter().any(|o| matches!(o, Op::Extract { .. } | Op::Ocr { .. })), "{text} → {p:?}");
    }
    // …while a request to extract still is one.
    assert!(matches!(op("선택한 문서 필드 추출해줘"), Some(Op::Extract { .. })));
}

/// A workspace with the project "한빛 심사" open and chosen as the assistant's scope, nothing selected.
fn in_project() -> PlanContext {
    let mut c = ctx();
    c.projects = vec![ProjectSummary { id: "p-hanbit".into(), name: "한빛 심사".into(), status: "active".into() }];
    c.current_project = Some("p-hanbit".into());
    c.scope_project = Some("p-hanbit".into());
    c.current_document = None;
    c.current_folder = None;
    c
}

fn project_of(p: &Plan) -> Option<&str> {
    p.scope.filter.project_id.as_deref()
}

/// PLN-6: the "현재 프로젝트" default scope used to satisfy the documents-required check, so a
/// model plan that named no documents rotated / re-rendered / removed … the whole open project.
#[test]
fn the_project_default_never_stands_in_for_documents_a_plan_must_name() {
    let c = in_project();
    for ops in [
        json!([{ "op": "rotate", "degrees": 90 }]),
        json!([{ "op": "rerender" }]),
        json!([{ "op": "set_role", "role": "knowledge" }]),
        json!([{ "op": "set_version", "label": "2024" }]),
        json!([{ "op": "remove_from_project" }]),
        json!([{ "op": "tag", "add": ["긴급"] }]),
        json!([{ "op": "delete" }]),
        json!([{ "op": "move", "folder": "documents/{{year}}" }]),
        json!([{ "op": "review", "action": "approve" }]),
    ] {
        let mut p = plan_of(ops.clone(), json!({}));
        let issues = validate::validate(&mut p, &c, None);
        assert!(!has_errors(&issues), "{ops}: {issues:?}");
        assert!(p.scope.is_empty(), "{ops}: the open project is not a target the user named: {:?}", p.scope);
        assert!(validate::scope_required(&p).is_some(), "{ops}");
        // Without "전부" the gate asks; with it "all" is the project, not the library.
        let mut asked = p.clone();
        assert_eq!(
            validate::settle_scope(&mut asked, &c, "그렇게 해줘").map(|i| i.code),
            Some("scope_required".to_string()),
            "{ops}"
        );
        assert!(asked.scope.is_empty(), "{ops}");
        assert!(validate::settle_scope(&mut p, &c, "전부 그렇게 해줘").is_none(), "{ops}");
        assert_eq!(project_of(&p), Some("p-hanbit"), "{ops}");
    }
    // Plans that only read or process keep the project as their default scope …
    for ops in [
        json!([{ "op": "answer", "question": "납품 기한은?" }]),
        json!([{ "op": "search", "query": "납품" }]),
        json!([{ "op": "aggregate", "fn": "count" }]),
        json!([{ "op": "summarize" }]),
        json!([{ "op": "classify" }]),
        json!([{ "op": "extract" }]),
    ] {
        let mut p = plan_of(ops.clone(), json!({}));
        validate::validate(&mut p, &c, None);
        assert_eq!(project_of(&p), Some("p-hanbit"), "{ops}");
    }
    // … and a filter the command names is joined with the project.
    let mut p = plan_of(json!([{ "op": "tag", "add": ["경비"] }]), json!({ "filter": { "doc_type": "영수증" } }));
    validate::validate(&mut p, &c, None);
    assert_eq!((project_of(&p), p.scope.filter.doc_type.as_deref()), (Some("p-hanbit"), Some("receipt")));
    assert!(validate::scope_required(&p).is_none());
}

/// PLN-6, phrase rules: the same commands ask which documents are meant; "전부" means the project.
#[test]
fn rules_ask_for_documents_inside_a_project_unless_the_sentence_says_all() {
    let c = in_project();
    let planned = |text: &str| plan(None, text, &c, PlanMode::RulesFirst).unwrap();
    for text in [
        "90도 돌려줘",
        "태그 긴급 달아줘",
        "월별 폴더로 정리해줘",
        "참고 자료로 표시해줘",
        "다시 렌더해줘",
        "문서 삭제해줘",
        "프로젝트에서 빼줘",
    ] {
        let r = planned(text);
        assert!(r.plan.is_clarification(), "{text} → {:?}", r.plan);
    }
    for (text, kind) in [
        ("전부 90도 돌려줘", OpKind::Rotate),
        ("전부 태그 긴급 달아줘", OpKind::Tag),
        ("문서 전부 월별 폴더로 정리해줘", OpKind::Move),
        ("전부 다시 렌더해줘", OpKind::Rerender),
    ] {
        let r = planned(text);
        assert_eq!(kinds(&r.plan), [kind], "{text} → {:?}", r.plan);
        assert_eq!(project_of(&r.plan), Some("p-hanbit"), "{text}: all = the project");
    }
    // Reading and processing commands keep the project as their default scope (0.5.0).
    let r = planned("요약해줘");
    assert_eq!((kinds(&r.plan), project_of(&r.plan)), (vec![OpKind::Summarize], Some("p-hanbit")), "{:?}", r.plan);
    let r = planned("영수증 몇 건이야?");
    assert_eq!((kinds(&r.plan), project_of(&r.plan)), (vec![OpKind::Aggregate], Some("p-hanbit")), "{:?}", r.plan);
    // A type names the targets: the project joins it.
    let r = planned("영수증 삭제해줘");
    assert_eq!(kinds(&r.plan), [OpKind::Delete], "{:?}", r.plan);
    assert_eq!((project_of(&r.plan), r.plan.scope.filter.doc_type.as_deref()), (Some("p-hanbit"), Some("receipt")));
}

/// PLN-6, model path: the planner's scope guard now sees the empty scope of such a plan.
#[test]
fn a_model_plan_without_documents_asks_inside_a_project() {
    let c = in_project();
    let rotate = || json!({ "summary": "회전", "intent": "rotate", "scope": {}, "operations": [{ "op": "rotate", "degrees": 90 }], "requires_confirmation": false, "confidence": 0.9 });
    let chat = StageChat::new();
    chat.push("plan", rotate());
    let r = plan(Some(&deps(&chat)), "90도 돌려줘", &c, PlanMode::ModelFirst).unwrap();
    assert_eq!(r.source, PlanSource::Model);
    assert!(r.plan.is_clarification(), "{:?}", r.plan);
    let chat = StageChat::new();
    chat.push("plan", rotate());
    let r = plan(Some(&deps(&chat)), "전부 90도 돌려줘", &c, PlanMode::ModelFirst).unwrap();
    assert!(matches!(r.plan.operations[0], Op::Rotate { degrees: 90, .. }), "{:?}", r.plan);
    // "all" inside the project scope is the project — settled by `plan` itself, so the shell,
    // the evaluation and every other caller see the same plan.
    assert_eq!(project_of(&r.plan), Some("p-hanbit"));
}

/// PLN-7 (and the hole behind it): in an ask-back text only the command and the user's reply say
/// whether everything is meant — the assistant's own question mentions "전부".
#[test]
fn only_the_users_words_of_an_ask_back_mean_all() {
    let question = "어떤 문서를 대상으로 할까요? 라이브러리에서 선택하거나 폴더·유형·날짜를 말씀해 주세요(전체면 '전부'라고 해 주세요).";
    let ko = |reply: &str| format!("태그 '긴급' 달아줘\n\n[어시스턴트 질문] {question}\n[사용자 답변] {reply}");
    assert!(rules::mentions_all(&ko("전부")));
    assert!(rules::mentions_all(&ko("응, 전체 다")));
    assert!(!rules::mentions_all(&ko("영수증만")));
    assert!(!rules::mentions_all(&ko("응")));
    assert_eq!(rules::user_words(&ko("영수증만")), "태그 '긴급' 달아줘\n영수증만");
    let en = |reply: &str| {
        format!("tag them urgent\n\n[Assistant question] Which documents? Select them in the library or name a folder / type / date range (say 'all documents' for the whole library).\n[User reply] {reply}")
    };
    assert!(rules::mentions_all(&en("all of them")));
    assert!(!rules::mentions_all(&en("the receipts")));
    // Ordinary sentences are read whole.
    assert!(rules::mentions_all("모든 문서에 태그 2026 추가해"));
    assert!(!rules::mentions_all("태그 '긴급' 달아줘"));
    assert_eq!(rules::user_words("태그 '긴급' 달아줘"), "태그 '긴급' 달아줘");
    // The model path: a reply that names nothing is asked again instead of running on everything.
    let c = ctx();
    let tag = || json!({ "summary": "태그", "intent": "tag", "scope": {}, "operations": [{ "op": "tag", "add": ["긴급"] }], "requires_confirmation": false, "confidence": 0.9 });
    let chat = StageChat::new();
    chat.push("plan", tag());
    let r = plan(Some(&deps(&chat)), &ko("응"), &c, PlanMode::ModelFirst).unwrap();
    assert!(r.plan.is_clarification(), "{:?}", r.plan);
    let chat = StageChat::new();
    chat.push("plan", tag());
    let r = plan(Some(&deps(&chat)), &ko("전부"), &c, PlanMode::ModelFirst).unwrap();
    assert!(matches!(&r.plan.operations[0], Op::Tag { add, .. } if add == &["긴급"]), "{:?}", r.plan);
    // Executing that card: the scope gate passes with the text the plan was made from
    // (`PlanResult.text`), not with the bare command the card shows.
    let mut card = r.plan.clone();
    assert_eq!(
        validate::settle_scope(&mut card, &c, "태그 '긴급' 달아줘").map(|i| i.code),
        Some("scope_required".to_string())
    );
    assert!(validate::settle_scope(&mut card, &c, &r.text).is_none());
}

/// PLN-5: with a project open, "'한빛 2차 심사' 프로젝트 만들어줘" copied every document of the
/// open project into the new one — the default scope was given to an op that takes none.
#[test]
fn ops_that_take_no_documents_get_no_default_scope() {
    let c = in_project();
    for ops in [
        json!([{ "op": "create_project", "name": "한빛 2차 심사" }]),
        json!([{ "op": "update_project", "name": "한빛 3차" }]),
        json!([{ "op": "define_term", "term": "MM", "definition": "맨먼스" }]),
        json!([{ "op": "manage_template", "name": "유지보수" }]),
        json!([{ "op": "check_consistency" }]),
        json!([{ "op": "set_theme", "theme": "dark" }]),
        json!([{ "op": "classify_projects" }]),
        json!([{ "op": "remove_from_project", "doc_ids": ["d1", "d2"] }]),
    ] {
        let mut p = plan_of(ops.clone(), json!({}));
        let issues = validate::validate(&mut p, &c, None);
        assert!(!has_errors(&issues), "{ops}: {issues:?}");
        assert!(p.scope.is_empty(), "{ops}: {:?}", p.scope);
    }
    let r = plan(None, "'한빛 2차 심사' 프로젝트 만들어줘", &c, PlanMode::RulesFirst).unwrap();
    assert!(matches!(&r.plan.operations[0], Op::CreateProject { name, .. } if name == "한빛 2차 심사"), "{:?}", r.plan);
    assert!(r.plan.scope.is_empty() && !validate::uses_scope(&r.plan), "{:?}", r.plan.scope);
    // Documents the command names still go into the new project.
    let mut p =
        plan_of(json!([{ "op": "create_project", "name": "한빛 2차 심사" }]), json!({ "doc_ids": ["d1", "d2"] }));
    validate::validate(&mut p, &c, None);
    assert_eq!(p.scope.doc_ids, ["d1", "d2"]);
    assert!(validate::uses_scope(&p) && project_of(&p).is_none());
    // Several documents taken out of a project at once are confirmed (their copies are deleted).
    let mut p = plan_of(json!([{ "op": "remove_from_project", "doc_ids": ["d1", "d2"] }]), json!({}));
    validate::validate(&mut p, &c, None);
    assert!(p.requires_confirmation);
    let mut p = plan_of(json!([{ "op": "remove_from_project", "doc_ids": ["d1"] }]), json!({}));
    validate::validate(&mut p, &c, None);
    assert!(!p.requires_confirmation);
}

/// A date in a sentence that points at explicit documents, or that reads none, is a value and not
/// a filter: "이 문서를 2024년판으로 표시" used to drop the open document unless it was dated 2024.
#[test]
fn a_date_is_not_a_filter_on_documents_the_user_named() {
    let c = ctx();
    let year = DateRange {
        from: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        to: NaiveDate::from_ymd_opt(2024, 12, 31).unwrap(),
        label: "year".into(),
    };
    let mut p = plan_of(json!([{ "op": "set_version", "label": "2024" }]), json!({ "doc_ids": ["d1a2"] }));
    validate::validate(&mut p, &c, Some(&year));
    assert!(p.scope.filter.date_from.is_none(), "{:?}", p.scope);
    let mut p = plan_of(json!([{ "op": "create_project", "name": "2024 감사 대응" }]), json!({}));
    validate::validate(&mut p, &c, Some(&year));
    assert!(p.scope.is_empty(), "{:?}", p.scope);
    // A plan over a filter (or the whole library) still gets the range.
    let mut p = plan_of(json!([{ "op": "search", "query": "위약금" }]), json!({}));
    validate::validate(&mut p, &c, Some(&year));
    assert_eq!(p.scope.filter.date_from.as_deref(), Some("2024-01-01"));
    for text in ["이 문서를 2024년판으로 표시", "제목을 '2026 공급계약'으로 바꿔", "2026 감사 대응 프로젝트 만들어줘"]
    {
        let r = planned(text);
        assert!(!r.plan.is_clarification() && r.plan.scope.filter.date_from.is_none(), "{text} → {:?}", r.plan.scope);
    }
    assert_eq!(planned("작년 계약서 전부 삭제").plan.scope.filter.date_from.as_deref(), Some("2025-01-01"));
}

/// PLN-11: ops the app performs run after the job's document steps; one that a later step
/// depends on is rejected instead of silently taking effect too late.
#[test]
fn an_app_side_change_before_a_step_that_depends_on_it_is_rejected() {
    let c = ctx();
    let errors = |ops: Value| -> Vec<String> {
        let mut p = plan_of(ops, json!({ "doc_ids": ["d1a2"] }));
        validate::validate(&mut p, &c, None).into_iter().filter(|i| i.blocking).map(|i| i.code).collect()
    };
    for ops in [
        json!([{ "op": "set_model", "id": "gemma4-12b-q4km" }, { "op": "extract", "force": true }]),
        json!([{ "op": "set_profile", "profile": "accuracy" }, { "op": "ocr" }]),
        json!([{ "op": "set_setting", "key": "processing.render_dpi", "value": 300 }, { "op": "rerender" }]),
        json!([{ "op": "workspace", "action": "open", "path": "D:\\Work" }, { "op": "tag", "add": ["x"] }]),
        json!([{ "op": "register_form", "doc_ref": "d1a2" }, { "op": "extract" }]),
        json!([{ "op": "reindex" }, { "op": "search", "query": "한빛" }]),
        json!([{ "op": "engine", "action": "restart" }, { "op": "summarize" }]),
    ] {
        assert_eq!(errors(ops.clone()), ["ui_op_order"], "{ops}");
    }
    for ops in [
        json!([{ "op": "extract", "force": true }, { "op": "set_model", "id": "gemma4-12b-q4km" }]),
        json!([{ "op": "set_theme", "theme": "dark" }, { "op": "extract" }]),
        json!([{ "op": "set_model", "id": "gemma4-12b-q4km" }, { "op": "tag", "add": ["x"] }]),
        json!([{ "op": "set_setting", "key": "ui.scale_percent", "value": 120 }, { "op": "ocr" }]),
        json!([{ "op": "set_model", "id": "gemma4-12b-q4km" }, { "op": "set_theme", "theme": "dark" }]),
        json!([{ "op": "engine", "action": "start" }, { "op": "summarize" }]),
        json!([{ "op": "open", "target": "screen", "ref": "review" }, { "op": "extract" }]),
    ] {
        assert_eq!(errors(ops.clone()), Vec::<String>::new(), "{ops}");
    }
    // The finding names both steps in the UI language and reaches the user as the question.
    let mut p = plan_of(
        json!([{ "op": "set_model", "id": "gemma4-12b-q4km" }, { "op": "extract", "force": true }]),
        json!({ "doc_ids": ["d1a2"] }),
    );
    let issue = validate::validate(&mut p, &c, None).into_iter().find(|i| i.code == "ui_op_order").unwrap();
    assert_eq!(issue.path, "operations[0]");
    assert!(issue.message.contains("'set_model' 단계") && issue.message.contains("'extract'"), "{}", issue.message);
    let chat = StageChat::new();
    let reply = json!({ "summary": "모델 변경 후 재추출", "intent": "set_model", "scope": { "doc_ids": ["d1a2"] }, "operations": [{ "op": "set_model", "id": "gemma4-12b-q4km" }, { "op": "extract", "force": true }], "requires_confirmation": false, "confidence": 0.9 });
    for _ in 0..3 {
        chat.push("plan", reply.clone());
    }
    let r = plan(Some(&deps(&chat)), "모델을 12B로 바꾸고 이 문서 다시 추출해줘", &c, PlanMode::ModelFirst).unwrap();
    assert!(r.plan.is_clarification(), "{:?}", r.plan);
    assert!(r.plan.clarification_question.as_deref().unwrap_or_default().contains("set_model"), "{:?}", r.plan);
}

/// PLN-18 ('규칙' mode): a path with blanks was cut at the first one, a particle after a Hangul
/// name was kept, and a new workspace was created without confirmation.
#[test]
fn rules_read_paths_with_blanks_and_confirm_a_new_workspace() {
    let c = ctx();
    let planned = |text: &str| plan(None, text, &c, PlanMode::RulesFirst).unwrap();
    let workspace = |text: &str| match planned(text).plan.operations[0].clone() {
        Op::Workspace { action, path } => (action, path.unwrap_or_default()),
        other => panic!("{text} → {other:?}"),
    };
    assert_eq!(workspace("워크스페이스 D:\\업무 문서 만들어"), (WorkspaceAction::Create, "D:\\업무 문서".to_string()));
    let r = planned("워크스페이스 D:\\업무 문서 만들어");
    assert!(r.plan.requires_confirmation, "a new workspace folder is confirmed: {:?}", r.plan);
    assert!(r.plan.summary.as_deref().unwrap_or_default().contains("D:\\업무 문서"), "{:?}", r.plan.summary);
    assert!(!planned("워크스페이스 D:\\Work 열어").plan.requires_confirmation);
    assert_eq!(workspace("워크스페이스 D:\\Work 열어").1, "D:\\Work");
    assert_eq!(workspace("D:\\Work 워크스페이스로 전환해줘").1, "D:\\Work");
    // Quoted paths are taken whole.
    assert_eq!(workspace("워크스페이스 \"D:\\내 문서\\업무 2026\" 열어").1, "D:\\내 문서\\업무 2026");
    assert_eq!(workspace("open the workspace 'C:/Users/me/My Work'").1, "C:/Users/me/My Work");
    // A particle glued to the path goes, a name that merely ends in such a syllable stays.
    assert_eq!(workspace("워크스페이스 D:\\문서로 전환").1, "D:\\문서");
    assert_eq!(workspace("워크스페이스를 D:\\업무 자료에 만들어줘").1, "D:\\업무 자료");
    assert_eq!(workspace("워크스페이스 D:\\Work를 열어").1, "D:\\Work");
    assert_eq!(workspace("워크스페이스 D:\\경로 열어").1, "D:\\경로");
    assert_eq!(workspace("워크스페이스 D:\\회의 열어").1, "D:\\회의");
    assert_eq!(workspace("워크스페이스 D:\\가을 열어").1, "D:\\가을");
    // Model folders.
    let model = |text: &str| match planned(text).plan.operations[0].clone() {
        Op::ManageModel { action: ModelAction::Import, path, .. } => path.unwrap_or_default(),
        other => panic!("{text} → {other:?}"),
    };
    assert_eq!(model("모델 폴더 D:\\AI Models\\x 가져와"), "D:\\AI Models\\x");
    assert_eq!(model("모델 폴더 D:\\models\\x 가져와"), "D:\\models\\x");
    assert_eq!(model("D:\\AI Models 폴더에서 모델 가져와"), "D:\\AI Models");
    // A path that exists on disk wins over the wording: here it contains a command word.
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("작업 전환 자료");
    std::fs::create_dir_all(&real).unwrap();
    let real = real.to_string_lossy().to_string();
    assert_eq!(workspace(&format!("워크스페이스 {real} 열어")).1, real);
    assert_eq!(workspace(&format!("워크스페이스 {real}로 전환해줘")).1, real);
}

/// PLN-19: an empty `template` is the documented way to clear a project's template; the
/// validator turned it into "not given".
#[test]
fn an_empty_template_survives_validation_to_clear_the_project_template() {
    let c = in_project();
    let mut p = plan_of(json!([{ "op": "update_project", "template": "  " }]), json!({}));
    let issues = validate::validate(&mut p, &c, None);
    assert!(!has_errors(&issues), "{issues:?}");
    assert!(
        matches!(&p.operations[0], Op::UpdateProject { template: Some(t), .. } if t.is_empty()),
        "{:?}",
        p.operations
    );
    // Nothing given at all is still an error.
    let mut p = plan_of(json!([{ "op": "update_project" }]), json!({}));
    assert!(validate::validate(&mut p, &c, None).iter().any(|i| i.code == "nothing_to_change"));
}

/// PRJ-12 (validator side): rule references are tidied; they are resolved against the project's
/// rules when the check runs (`exec_tests::audit_fixes`).
#[test]
fn check_consistency_rule_references_are_tidied() {
    let c = in_project();
    let mut p =
        plan_of(json!([{ "op": "check_consistency", "rules": ["  계약금액 ", "계약금액", "", "r-1"] }]), json!({}));
    let issues = validate::validate(&mut p, &c, None);
    assert!(!has_errors(&issues), "{issues:?}");
    assert!(
        matches!(&p.operations[0], Op::CheckConsistency { project: Some(pid), rules } if pid == "p-hanbit" && rules == &["계약금액", "r-1"]),
        "{:?}",
        p.operations
    );
}

/// GEN-14 (planner side): the validator cut a draft's content at 6,000 and a revision's
/// instruction at 2,000 characters before the generator saw them, so the generator's "what was
/// left out" notes could never fire. Both keep what the generator stores.
#[test]
fn draft_text_reaches_the_generator_at_the_length_it_stores() {
    use crate::generate::MAX_STORED_TEXT_CHARS;
    let c = PlanContext { draft_id: Some("dr-1".into()), ..ctx() };
    let text = |n: usize| "가나다라마 ".chars().cycle().take(n).collect::<String>().trim().to_string();
    let lengths = |content: usize, instruction: usize| {
        let mut p = plan_of(
            json!([
                { "op": "draft_document", "content": text(content) },
                { "op": "revise_draft", "draft_id": "dr-1", "scope": "whole", "instruction": text(instruction) }
            ]),
            json!({}),
        );
        let issues = validate::validate(&mut p, &c, None);
        assert!(!has_errors(&issues), "{issues:?}");
        match (&p.operations[0], &p.operations[1]) {
            (Op::DraftDocument { content, .. }, Op::ReviseDraft { instruction, .. }) => {
                (content.chars().count(), instruction.chars().count())
            }
            other => panic!("{other:?}"),
        }
    };
    let kept = |n: usize| text(n).chars().count();
    assert_eq!(lengths(7_500, 3_000), (kept(7_500), kept(3_000)), "longer than the old cuts, kept whole");
    let (content, instruction) = lengths(MAX_STORED_TEXT_CHARS + 500, MAX_STORED_TEXT_CHARS + 500);
    assert!(content <= MAX_STORED_TEXT_CHARS && content > MAX_STORED_TEXT_CHARS - 2, "{content}");
    assert!(instruction <= MAX_STORED_TEXT_CHARS && instruction > MAX_STORED_TEXT_CHARS - 2, "{instruction}");
}
