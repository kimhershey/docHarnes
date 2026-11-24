//! 1.0.5 — the planner decisions of that version, on the deterministic side and with the plans
//! Gemma 4 E4B wrote (`eval/results/planner-real-model/gemma4-e4b-q4km-1.0.5-project-reference-before.json`):
//! A "알려줘" is an answer, B the document type as a scope condition, C a question with the readings
//! as choices, G scope conditions the sentence does not say. (`scope::resolve` with a type and
//! `choices::type_overlap` need a workspace: `exec_tests/v105.rs`.)

use super::*;

/// The context of the measurement: the projects AAA and 한빛 납품, a requirements type, a tag.
fn ctx105() -> PlanContext {
    let mut c = ctx();
    c.doc_types.push(DocTypeSummary {
        slug: "srs".into(),
        label_ko: "요구사항 정의서".into(),
        label_en: "Software requirements specification".into(),
    });
    c.tags = vec!["경비".into(), "중요".into()];
    c.current_folder = None;
    c.projects = vec![
        ProjectSummary { id: "p-hanbit".into(), name: "한빛 납품".into(), status: "active".into() },
        ProjectSummary { id: "p-aaa".into(), name: "AAA".into(), status: "active".into() },
    ];
    c
}

/// One sentence through the model path with the plan the model is scripted to write.
fn modelled(c: &PlanContext, text: &str, intent: &str, scope: Value, operations: Value) -> PlanResult {
    let chat = StageChat::new();
    chat.push(
        "plan",
        json!({ "summary": "s", "unmet": "", "intent": intent, "scope": scope, "operations": operations, "requires_confirmation": false, "confidence": 0.9 }),
    );
    let r = plan(Some(&deps(&chat)), text, c, PlanMode::ModelFirst).unwrap();
    assert_eq!(r.source, PlanSource::Model, "{text}: {r:?}");
    r
}

fn ruled(c: &PlanContext, text: &str) -> PlanResult {
    plan(None, text, c, PlanMode::RulesFirst).unwrap()
}

fn codes(r: &PlanResult) -> Vec<&str> {
    r.issues.iter().map(|i| i.code.as_str()).collect()
}

fn keys(r: &PlanResult) -> Vec<&str> {
    r.choices.iter().map(|c| c.key.as_str()).collect()
}

/// G — the plans of the measurement, replayed: a tag that is the project's name, a status, "knowledge"
/// as a tag and a step that changes reference material came with sentences that only ask for a
/// summary of a project's reference material. No document carried those marks: "이 범위에는 참고
/// 자료가 없습니다".
#[test]
fn scope_conditions_the_sentence_does_not_say_are_left_out() {
    let c = ctx105();
    let summarize = json!([{ "op": "summarize", "group_by": "project" }]);
    let about_aaa = json!([{ "op": "summarize", "group_by": "project", "topic": "AAA" }]);
    for (text, scope, ops, project) in [
        // p-5
        (
            "AAA 참고자료 요약해줘",
            json!({ "filter": { "doc_type": "knowledge", "status": "reviewed" } }),
            about_aaa.clone(),
            "p-aaa",
        ),
        // p-6
        (
            "한빛 납품 프로젝트의 참고 자료를 요약해줘",
            json!({ "filter": { "doc_type": "knowledge", "tags": ["한빛 납품"] } }),
            summarize.clone(),
            "p-hanbit",
        ),
        // p-8
        (
            "AAA프로젝트 참고자료 요약",
            json!({ "filter": { "doc_type": "knowledge", "tags": ["AAA"] } }),
            summarize.clone(),
            "p-aaa",
        ),
    ] {
        let r = modelled(&c, text, "summarize", scope, ops);
        assert!(
            matches!(r.plan.operations.as_slice(), [Op::Summarize { group_by: None, topic: None, .. }]),
            "{text}: {:?}",
            r.plan
        );
        let f = &r.plan.scope.filter;
        assert_eq!(
            (f.project_id.as_deref(), f.role, f.tags.len(), f.status.as_deref(), f.doc_type.as_deref()),
            (Some(project), Some(RoleParam::Knowledge), 0, None, None),
            "{text}: {f:?}"
        );
        assert!(r.plan.scope.doc_ids.is_empty() && !r.plan.requires_confirmation, "{text}: {:?}", r.plan);
        assert_eq!(codes(&r).iter().filter(|c| **c == "scope_ignored").count(), 1, "{text}: {:?}", r.issues);
    }
    // p-7: an answer, with the project's name as a tag.
    let r = modelled(
        &c,
        "AAA 프로젝트의 참고자료에서 보안 요구사항 알려줘",
        "answer",
        json!({ "doc_ids": [], "filter": { "doc_type": "knowledge", "tags": ["AAA"] } }),
        json!([{ "op": "answer", "question": "AAA 프로젝트의 참고 자료에서 보안 요구사항은 무엇인가요?", "cite": true, "knowledge": false }]),
    );
    assert!(matches!(r.plan.operations.as_slice(), [Op::Answer { knowledge: None, mode: None, .. }]), "{:?}", r.plan);
    let f = &r.plan.scope.filter;
    assert_eq!(
        (f.project_id.as_deref(), f.role, f.tags.len()),
        (Some("p-aaa"), Some(RoleParam::Knowledge), 0),
        "{f:?}"
    );
    // p-3: a step that changes reference material for a sentence that only asks for a summary, and
    // no project in the scope. The step goes, the summary stays — of the project's reference
    // material, the requirements documents among them.
    let r = modelled(
        &c,
        "AAA 프로젝트의 참고자료에서 요구사항 정의서 요약해줘",
        "summarize",
        json!({ "doc_ids": [], "filter": { "doc_type": "srs", "tags": ["knowledge"] } }),
        json!([{ "op": "manage_reference", "action": "add", "project": "AAA" }, { "op": "summarize", "group_by": "project" }]),
    );
    assert!(
        matches!(r.plan.operations.as_slice(), [Op::Summarize { group_by: None, topic: None, .. }]),
        "{:?}",
        r.plan
    );
    assert_eq!(r.plan.intent, OpKind::Summarize);
    let f = &r.plan.scope.filter;
    assert_eq!(
        (f.project_id.as_deref(), f.role, f.doc_type.as_deref(), f.tags.len()),
        (Some("p-aaa"), Some(RoleParam::Knowledge), Some("srs"), 0),
        "{f:?}"
    );
    assert!(!r.plan.requires_confirmation && !r.plan.is_clarification(), "{:?}", r.plan);
    let note = r.issues.iter().find(|i| i.code == "scope_ignored").expect("a note");
    assert!(!note.blocking && note.message.contains("태그") && note.message.contains("단계"), "{note:?}");

    // What the sentence does say stays: a tag it names as a tag, a tag of the workspace, a status,
    // a period, a file-name keyword.
    let one = json!([{ "op": "summarize" }]);
    let r =
        modelled(&c, "'긴급' 태그 문서 요약해줘", "summarize", json!({ "filter": { "tags": ["긴급"] } }), one.clone());
    assert_eq!(r.plan.scope.filter.tags, vec!["긴급".to_string()], "{:?}", r.plan.scope);
    assert!(!codes(&r).contains(&"scope_ignored"), "{:?}", r.issues);
    let r = modelled(&c, "경비 문서 요약해줘", "summarize", json!({ "filter": { "tags": ["경비"] } }), one.clone());
    assert_eq!(r.plan.scope.filter.tags, vec!["경비".to_string()]);
    let r = modelled(
        &c,
        "검토 끝난 문서 중에 지난달 것만 요약해줘",
        "summarize",
        json!({ "filter": { "status": "reviewed", "date_from": "2026-10-01", "date_to": "2026-10-31" } }),
        one.clone(),
    );
    let f = &r.plan.scope.filter;
    assert_eq!((f.status.as_deref(), f.date_from.as_deref()), (Some("reviewed"), Some("2026-10-01")), "{f:?}");
    let r = modelled(
        &c,
        "파일 이름에 '한빛'이 들어간 문서 요약해줘",
        "summarize",
        json!({ "filter": { "keyword": "한빛" } }),
        one.clone(),
    );
    assert_eq!(r.plan.scope.filter.keyword.as_deref(), Some("한빛"));
    // … and what it does not say goes, each kind of it: a period, a confidence, a vendor, a keyword
    // that is the name of the project the sentence names.
    let r = modelled(
        &c,
        "AAA 프로젝트 문서 요약해줘",
        "summarize",
        json!({ "filter": { "keyword": "AAA", "vendor": "㈜한빛", "min_confidence": 0.8, "review_status": "done", "date_from": "2026-01-01", "date_to": "2026-12-31" } }),
        one.clone(),
    );
    let f = &r.plan.scope.filter;
    assert_eq!(
        (f.keyword.as_deref(), f.vendor.as_deref(), f.min_confidence, f.review_status, f.date_from.as_deref()),
        (None, None, None, None, None),
        "{f:?}"
    );
    assert_eq!(f.project_id.as_deref(), Some("p-aaa"));

    // A plan that changes documents keeps every condition the model wrote: one condition less would
    // widen what it changes; one too many only finds nothing.
    let r = modelled(
        &c,
        "AAA 프로젝트 문서 휴지통으로 보내줘",
        "delete",
        json!({ "filter": { "project_id": "p-aaa", "tags": ["AAA"], "status": "reviewed" } }),
        json!([{ "op": "delete" }]),
    );
    let f = &r.plan.scope.filter;
    assert_eq!((f.tags.len(), f.status.as_deref()), (1, Some("reviewed")), "{f:?}");
    // A step that puts documents somewhere stays when the sentence asks for it.
    let r = modelled(
        &c,
        "이 문서 요약하고 AAA 프로젝트 참고자료로 추가해줘",
        "summarize",
        json!({ "doc_ids": ["d1a2"] }),
        json!([{ "op": "summarize" }, { "op": "manage_reference", "action": "add", "project": "AAA" }]),
    );
    assert_eq!(kinds(&r.plan), vec![OpKind::Summarize, OpKind::ManageReference], "{:?}", r.plan);
}

/// A — "알려줘" asks for an answer, never for a list of hits (user decision).
#[test]
fn tell_me_is_an_answer_not_a_search() {
    let c = ctx105();
    let search = |query: &str| json!([{ "op": "search", "query": query }]);
    // The model's search for a sentence that asks to be told becomes a grounded answer to it.
    for text in ["납품 기한 알려줘", "지체상금이 뭐야", "tell me the delivery deadline"] {
        let r = modelled(&c, text, "search", json!({}), search("납품 기한"));
        assert!(
            matches!(r.plan.operations.as_slice(), [Op::Answer { question, knowledge: None, mode: None, reply: None, .. }] if question == text),
            "{text}: {:?}",
            r.plan
        );
        assert_eq!(r.plan.intent, OpKind::Answer, "{text}");
        assert!(!r.plan.requires_confirmation, "{text}");
    }
    // (the scope the plan had stays: the project the sentence names)
    let r = modelled(&c, "AAA 프로젝트에서 납품 기한 알려줘", "search", json!({}), search("납품 기한"));
    assert!(matches!(r.plan.operations.as_slice(), [Op::Answer { .. }]), "{:?}", r.plan);
    assert_eq!(r.plan.scope.filter.project_id.as_deref(), Some("p-aaa"));
    // A sentence that asks for the documents stays a search — and so does a search that groups.
    for text in ["납품 기한 찾아줘", "납품 기한이 어디 있는지 알려줘", "납품 기한 관련 문서 목록 알려줘"]
    {
        let r = modelled(&c, text, "search", json!({}), search("납품 기한"));
        assert!(matches!(r.plan.operations.as_slice(), [Op::Search { .. }]), "{text}: {:?}", r.plan);
    }
    let r = modelled(
        &c,
        "납품 기한을 프로젝트별로 알려줘",
        "search",
        json!({}),
        json!([{ "op": "search", "query": "납품 기한", "group_by": "project" }]),
    );
    assert!(matches!(r.plan.operations.as_slice(), [Op::Search { group_by: Some(_), .. }]), "{:?}", r.plan);

    // With a place and a subject, "알려줘" is what "설명해줘" is: the summary of what that place says
    // about the subject — whatever the model planned, and on the rules path too.
    for text in ["AAA 프로젝트의 검수 기준에 대해 알려줘", "이 문서에서 검수 기준에 대해 알려줘"]
    {
        for r in [modelled(&c, text, "search", json!({}), search("검수 기준")), ruled(&c, text)] {
            assert!(
                matches!(r.plan.operations.as_slice(), [Op::Summarize { topic: Some(t), group_by: None, .. }] if t == "검수 기준"),
                "{text}: {:?}",
                r.plan
            );
            if text.starts_with("AAA") {
                assert_eq!(r.plan.scope.filter.project_id.as_deref(), Some("p-aaa"), "{text}");
            } else {
                assert_eq!(r.plan.scope.doc_ids, vec!["d1a2".to_string()], "{text}");
            }
        }
    }
    // Without a place it stays a question (which general knowledge may answer).
    for r in [
        modelled(&c, "검수 기준에 대해 알려줘", "search", json!({}), search("검수 기준")),
        ruled(&c, "검수 기준에 대해 알려줘"),
        ruled(&c, "납품 기한 알려줘"),
    ] {
        assert!(matches!(r.plan.operations.as_slice(), [Op::Answer { .. }]), "{:?}", r.plan);
        assert!(r.plan.scope.is_empty() && r.choices.is_empty(), "{r:?}");
    }
    // What is to be told is a list or a count: a question of its own, not a summary about a subject.
    for text in ["AAA 프로젝트에서 위약금 관련 문서 목록 알려줘", "AAA 프로젝트에서 계약 변경에 대해 몇 건인지 알려줘"]
    {
        assert!(guard::summary_topic(text, &c).is_none(), "{text}");
    }
}

/// B — a plan that only asks never takes the document type as a scope condition.
#[test]
fn a_question_or_a_search_takes_no_type_condition() {
    let c = ctx105();
    let typed = json!({ "filter": { "doc_type": "contract" } });
    // The sentence names the type: it still is no condition (quietly).
    let r = modelled(
        &c,
        "계약서에서 위약금이 얼마인지 알려줘",
        "answer",
        typed.clone(),
        json!([{ "op": "answer", "question": "위약금이 얼마인지" }]),
    );
    assert_eq!(r.plan.scope.filter.doc_type, None, "{:?}", r.plan.scope);
    assert!(codes(&r).is_empty(), "{:?}", r.issues);
    let r = modelled(
        &c,
        "'위약금' 들어간 계약서 찾아줘",
        "search",
        typed.clone(),
        json!([{ "op": "search", "query": "위약금" }]),
    );
    assert!(matches!(r.plan.operations.as_slice(), [Op::Search { query, .. }] if query == "위약금"), "{:?}", r.plan);
    assert_eq!(r.plan.scope.filter.doc_type, None);
    // A search left with nothing to look for looks for the type's name as the user wrote it.
    let r = modelled(&c, "계약서 찾아줘", "search", typed.clone(), json!([{ "op": "search", "query": "" }]));
    assert!(matches!(r.plan.operations.as_slice(), [Op::Search { query, .. }] if query == "계약서"), "{:?}", r.plan);
    assert_eq!(r.plan.scope.filter.doc_type, None);
    // The rules do the same: the type's name is neither a condition nor lost.
    let r = ruled(&c, "'위약금' 들어간 계약서 찾아줘");
    assert!(matches!(r.plan.operations.as_slice(), [Op::Search { query, .. }] if query == "위약금"), "{:?}", r.plan);
    assert_eq!(r.plan.scope.filter.doc_type, None, "{:?}", r.plan.scope);
    let r = ruled(&c, "계약서 찾아줘");
    assert!(matches!(r.plan.operations.as_slice(), [Op::Search { query, .. }] if query == "계약서"), "{:?}", r.plan);
    assert_eq!(r.plan.scope.filter.doc_type, None);
    let r = ruled(&c, "계약서에서 해지 조건 알려줘");
    assert!(matches!(r.plan.operations.as_slice(), [Op::Answer { .. }]), "{:?}", r.plan);
    assert_eq!(r.plan.scope.filter.doc_type, None);
    // Every other plan keeps the type as a condition: a summary, a total, a change.
    let r = modelled(&c, "계약서 요약해줘", "summarize", typed.clone(), json!([{ "op": "summarize" }]));
    assert_eq!(r.plan.scope.filter.doc_type.as_deref(), Some("contract"));
    assert_eq!(r.plan.scope.filter.type_match, None);
    for text in ["계약서 요약해줘", "지난달 영수증 합계", "작년 계약서 전부 삭제"] {
        let r = ruled(&c, text);
        assert!(r.plan.scope.filter.doc_type.is_some(), "{text}: {:?}", r.plan.scope);
    }
    // How the condition matches is the user's choice, never the model's.
    let r = modelled(
        &c,
        "계약서 요약해줘",
        "summarize",
        json!({ "filter": { "doc_type": "contract", "type_match": "named" } }),
        json!([{ "op": "summarize" }]),
    );
    assert_eq!(r.plan.scope.filter.type_match, None);
    // (the field travels with a plan from a choice, and is not part of an empty filter)
    let filter = ScopeFilter { type_match: Some(TypeMatch::Named), ..ScopeFilter::default() };
    assert!(filter.is_empty());
    assert_eq!(serde_json::to_value(&filter).unwrap(), json!({ "type_match": "named" }));
    assert_eq!(serde_json::to_value(ScopeFilter::default()).unwrap(), json!({}));
    assert!(!serde_json::to_string(&schema::plan_schema()).unwrap().contains("type_match"));
}

/// C — a subject that is also the name of a project or of a document type: the assistant asks
/// which reading is meant, and every reading is a ready-made plan.
#[test]
fn a_subject_that_is_also_a_name_is_asked_about_with_choices() {
    let c = ctx105();
    let topic_of = |p: &Plan| match p.operations.as_slice() {
        [Op::Summarize { topic, group_by: None, .. }] => topic.clone(),
        other => panic!("{other:?}"),
    };
    // 1. The name of a project — rules and model alike.
    let by_model = modelled(
        &c,
        "AAA에 대해서 요약해줘",
        "summarize",
        json!({}),
        json!([{ "op": "summarize", "group_by": "project", "topic": "AAA" }]),
    );
    for r in [ruled(&c, "AAA에 대해서 요약해줘"), by_model] {
        assert!(r.plan.is_clarification(), "{:?}", r.plan);
        assert_eq!(
            r.plan.clarification_question.as_deref(),
            Some("'AAA' 프로젝트가 있습니다. 어느 쪽을 말씀하신 건가요?")
        );
        assert_eq!(keys(&r), vec!["project", "topic", "project_topic"]);
        let [project, topic, inside] = r.choices.as_slice() else { unreachable!() };
        // ① the project's documents, as "AAA 프로젝트 요약해줘" plans them.
        assert_eq!(topic_of(&project.plan), None);
        assert_eq!(project.plan.scope.filter.project_id.as_deref(), Some("p-aaa"));
        assert!(project.plan.scope.filter.with_reference, "work documents and reference material");
        assert_eq!(project.label, "'AAA' 프로젝트의 문서 전체를 요약");
        // ② the word in every document — what the sentence planned until 1.0.4.
        assert_eq!(topic_of(&topic.plan).as_deref(), Some("AAA"));
        assert!(topic.plan.scope.is_empty(), "{:?}", topic.plan.scope);
        // ③ the word inside the project.
        assert_eq!(topic_of(&inside.plan).as_deref(), Some("AAA"));
        assert_eq!(inside.plan.scope.filter.project_id.as_deref(), Some("p-aaa"));
        for choice in &r.choices {
            assert!(!choice.plan.is_clarification() && !choice.plan.requires_confirmation, "{choice:?}");
            assert!(!choice.label.is_empty() && choice.plan.summary.is_some(), "{choice:?}");
        }
        assert!(r.issues.is_empty(), "the notes of one reading are not the question's: {:?}", r.issues);
    }
    // "알려줘": the word everywhere is a grounded answer; inside the project it is the summary about it.
    let told = modelled(&c, "AAA에 대해 알려줘", "search", json!({}), json!([{ "op": "search", "query": "AAA" }]));
    for r in [ruled(&c, "AAA에 대해 알려줘"), told] {
        assert_eq!(keys(&r), vec!["project", "topic", "project_topic"], "{:?}", r.plan);
        let anywhere = &r.choices[1].plan;
        assert!(
            matches!(anywhere.operations.as_slice(), [Op::Answer { question, knowledge: None, mode: None, .. }] if question == "AAA에 대해 알려줘"),
            "{anywhere:?}"
        );
        assert!(anywhere.scope.is_empty(), "{:?}", anywhere.scope);
        assert_eq!(r.choices[1].label, "모든 문서에서 'AAA'에 관해 찾아 답하기");
        assert_eq!(topic_of(&r.choices[2].plan).as_deref(), Some("AAA"));
    }
    // A general-knowledge route for such a sentence is asked about as well (no plan call was made).
    let chat = StageChat::new();
    chat.push("families", json!({ "families": ["general"] }));
    let r = plan(Some(&deps(&chat)), "AAA에 대해 알려줘", &c, PlanMode::ModelFirst).unwrap();
    assert_eq!((chat.calls("plan"), keys(&r)), (0, vec!["project", "topic", "project_topic"]), "{:?}", r.plan);
    // Inside the "현재 프로젝트" scope of that very project, "everywhere" is the project: two readings.
    let mut scoped = c.clone();
    scoped.scope_project = Some("p-aaa".into());
    assert_eq!(keys(&ruled(&scoped, "AAA에 대해서 요약해줘")), vec!["project", "project_topic"]);
    // The sentence settles it itself: it says "프로젝트", names a place, or the name is not a project's.
    for text in [
        "AAA 프로젝트에 대해 요약해줘",
        "AAA 프로젝트 요약해줘",
        "한빛 납품 프로젝트에서 AAA에 대해서 요약해줘",
        "이 문서에서 AAA에 대해서 요약해줘",
        "AA에 대해서 요약해줘",
        "AAA 요약본 찾아줘",
    ] {
        let r = ruled(&c, text);
        assert!(r.choices.is_empty() && !r.plan.is_clarification(), "{text}: {:?}", r.plan);
    }
    // The reply to a question is not asked about again: the user's words settle it.
    let reply = "AAA에 대해서 요약해줘\n\n[어시스턴트 질문] 'AAA' 프로젝트가 있습니다. 어느 쪽을 말씀하신 건가요?\n[사용자 답변] 문서 전체에서 찾아줘";
    let r = ruled(&c, reply);
    assert!(r.choices.is_empty(), "{:?}", r.plan);

    // 2. The name of a document type: the documents of that type, or what the documents say about the word.
    let by_model = modelled(
        &c,
        "계약서에 대해 요약해줘",
        "summarize",
        json!({ "filter": { "doc_type": "contract" } }),
        json!([{ "op": "summarize", "topic": "계약서" }]),
    );
    for r in [ruled(&c, "계약서에 대해 요약해줘"), by_model] {
        assert!(r.plan.is_clarification(), "{:?}", r.plan);
        assert_eq!(
            r.plan.clarification_question.as_deref(),
            Some("'계약서'는 문서 유형의 이름이기도 합니다. 어느 쪽을 말씀하신 건가요?")
        );
        assert_eq!(keys(&r), vec!["type", "topic"]);
        let [of_type, word] = r.choices.as_slice() else { unreachable!() };
        assert_eq!(topic_of(&of_type.plan), None);
        assert_eq!(of_type.plan.scope.filter.doc_type.as_deref(), Some("contract"));
        assert_eq!(of_type.label, "유형이 '계약서'인 문서를 요약");
        assert_eq!(topic_of(&word.plan).as_deref(), Some("계약서"));
        assert_eq!(word.plan.scope.filter.doc_type, None);
        assert_eq!(word.label, "문서에서 '계약서'에 관한 내용을 찾아 요약");
    }
    // Told rather than summarised: the word is answered from the documents — unless a place is named.
    let r = ruled(&c, "계약서에 대해 알려줘");
    assert_eq!(keys(&r), vec!["type", "topic"], "{:?}", r.plan);
    assert!(matches!(r.choices[1].plan.operations.as_slice(), [Op::Answer { .. }]), "{:?}", r.choices[1]);
    assert_eq!(r.choices[1].plan.scope.filter.doc_type, None);
    let r = ruled(&c, "AAA 프로젝트의 계약서에 대해 알려줘");
    assert_eq!(keys(&r), vec!["type", "topic"], "{:?}", r.plan);
    for choice in &r.choices {
        assert_eq!(choice.plan.scope.filter.project_id.as_deref(), Some("p-aaa"), "{choice:?}");
    }
    assert_eq!(topic_of(&r.choices[1].plan).as_deref(), Some("계약서"));
    // A subject that only contains a type's name is a subject, and a type outside a subject a place.
    for text in ["계약서 해지 조건에 대해 요약해줘", "계약서 요약해줘", "계약서에서 위약금에 대해 요약해줘"]
    {
        let r = ruled(&c, text);
        assert!(r.choices.is_empty() && !r.plan.is_clarification(), "{text}: {:?}", r.plan);
    }
    // The result travels as JSON: the choices next to the clarification, absent when there are none.
    let value = serde_json::to_value(ruled(&c, "계약서에 대해 요약해줘")).unwrap();
    assert_eq!(value["choices"][0]["key"], json!("type"));
    assert_eq!(value["plan"]["intent"], json!("clarify"));
    assert!(serde_json::to_value(ruled(&c, "계약서 요약해줘")).unwrap().get("choices").is_none());
}
