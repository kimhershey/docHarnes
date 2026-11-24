//! 1.0.6 — "다듬어줘 / 수정해줘" is not document writing (user decision 2026-11-24: "다듬어줘/수정해줘
//! 는 문서 작성이 아님. 문서 작성은 새 문서 혹은 문서를 명시했을 때만 문서로 작성하도록 함."): what
//! counts as a request to polish wording (`guard::polish_request`), the plans Gemma 4 E4B wrote for
//! such sentences replayed (`eval/results/planner-real-model/gemma4-e4b-q4km-1.0.6-polish-before.json`),
//! and the phrase rule.

use super::*;
use crate::knowledge::ConversationTurn;

const REPLY: &str =
    "납품 기한은 계약 체결일로부터 60일입니다 [1].\n지체 시에는 지체상금이 부과됩니다 [2].\n검수는 납품 후 14일 안에 마칩니다 [3].";
const BODY: &str =
    "본 계약의 납품 기한은 계약 체결일로부터 60일로 하며 지체 시에는 지체상금을 부과하는 것으로 한다고 되어 있음";

/// The context of the measurement (a document is open) …
fn open_document() -> PlanContext {
    let mut c = ctx();
    c.current_folder = None;
    c.projects = vec![ProjectSummary { id: "p-hanbit".into(), name: "한빛 납품".into(), status: "active".into() }];
    c
}

/// … and the same after an answer.
fn after_an_answer() -> PlanContext {
    let mut c = open_document();
    c.history = vec![ConversationTurn::user("납품 기한 알려줘"), ConversationTurn::assistant(REPLY)];
    c
}

fn modelled(c: &PlanContext, text: &str, intent: &str, operations: Value) -> PlanResult {
    let chat = StageChat::new();
    chat.push(
        "plan",
        json!({ "summary": "s", "unmet": "", "intent": intent, "scope": {}, "operations": operations, "requires_confirmation": false, "confidence": 0.9 }),
    );
    let r = plan(Some(&deps(&chat)), text, c, PlanMode::ModelFirst).unwrap();
    assert_eq!(r.source, PlanSource::Model, "{text}: {r:?}");
    r
}

fn ruled(c: &PlanContext, text: &str) -> PlanResult {
    plan(None, text, c, PlanMode::RulesFirst).unwrap()
}

/// The question of the text-only answer a result holds — `None` for any other plan.
fn worked_on(r: &PlanResult) -> Option<&str> {
    match r.plan.operations.as_slice() {
        [Op::Answer { question, cite: None, knowledge: Some(false), mode: None, reply: None }]
            if r.plan.scope.is_empty() && !r.plan.requires_confirmation && r.plan.intent == OpKind::Answer =>
        {
            Some(question.as_str())
        }
        _ => None,
    }
}

fn asks_for_the_text(r: &PlanResult) -> bool {
    r.plan.is_clarification() && r.plan.clarification_question.as_deref() == Some(guard::polish_what(true))
}

/// The five sentences of the measurement that ask for wording to be worked on.
const MEASURED: [&str; 4] =
    ["방금 답변 좀 더 자연스럽게 다듬어줘", "위 내용 수정해줘", "더 간결하게 수정해줘", "요구사항 부분을 좀 다듬어줘"];

#[test]
fn what_counts_as_a_request_to_polish_wording() {
    let c = after_an_answer();
    let pasted = format!("이 문장 다듬어줘: {BODY}");
    // The measured sentences, and what people say next to them.
    for text in MEASURED.iter().copied().chain([
        pasted.as_str(),
        "다듬어줘",
        "다시 써줘",
        "더 간결하게",
        "좀 더 자연스럽게 써줘",
        "더 짧게 다시 작성해줘",
        "방금 답변 존댓말로 고쳐줘",
        "오타 고쳐줘",
        "맞춤법 교정해줘",
        "위 답변을 표로 정리해서 다듬어줘",
        "이 문장 교정",
        "다듬어줘: 짧은 글입니다",
        "'본 계약의 납품 기한은 60일로 한다' 이 문장 다듬어줘",
        "polish the last answer",
        "rewrite it more formally",
        "Could you revise the answer?",
    ]) {
        assert!(guard::polish_request(text, &c).is_some(), "{text}");
    }
    for text in [
        // What to find and what to put instead: a replacement in the documents.
        "'갑'을 '을'로 수정해줘",
        "원본에서 '11월 30일'을 '12월 31일'로 고쳐줘",
        // A document is asked for, or named.
        "다듬어서 새 문서로 만들어줘",
        "이 내용으로 보고서 작성해줘",
        "방금 답변 다듬어서 문서로 만들어줘",
        "회의록 초안 수정해줘",
        "계약서 문구 수정해줘",
        "이 문서 다듬어줘",
        "선택한 것 내용 수정해줘",
        "영수증 내용 수정해줘",
        // A thing of the app, a field of a document.
        "테마 수정해줘",
        "공급가액 수정해줘",
        "프로젝트 이름 수정해줘",
        "경비 태그 내용 수정해줘",
        "제목 다듬어줘",
        "2쪽 내용 수정해줘",
        "종료일 수정해줘",
        "스키마 필드 수정해줘",
        // A section of the draft.
        "3절 다시 써줘",
        "rewrite section 2 more formally",
        // Nothing says that wording is meant.
        "수정해줘",
        "지난달 영수증 합계 다시 써줘",
        "더 간결하게 요약해줘",
        "더 간결하게 작성해줘",
        "make it more concise",
        // Something else is asked for too; a question; a report; a refusal.
        "이 답변 수정해서 메일로 보내줘",
        "방금 답변 다듬어서 저장해줘",
        "지체상금 조항을 어떻게 다듬어야 해?",
        "글 다듬는 법 좀 알려줘",
        "다듬을 수 있어?",
        "어떤 부분을 다듬었는지 알려줘",
        "수정된 내용 알려줘",
        "교정 일정 알려줘",
        "다듬지 말고 그대로 둬",
    ] {
        assert!(guard::polish_request(text, &c).is_none(), "{text}");
    }
    // With a draft open in 문서 작성 a revision of a section is the draft's.
    let mut drafting = after_an_answer();
    drafting.draft_id = Some("dr-1".into());
    assert!(guard::polish_request("3절 다시 써줘", &drafting).is_none());
    assert!(guard::polish_request("초안 전체 격식체로 다듬어줘", &drafting).is_none());
}

#[test]
fn the_text_to_work_on_is_what_the_sentence_brings_or_the_last_reply_in_full() {
    // ① The sentence brings it: the request as the user wrote it.
    let pasted = format!("이 문장 다듬어줘: {BODY}");
    let asked = guard::polish_request(&pasted, &after_an_answer()).unwrap();
    assert_eq!(asked.question.as_deref(), Some(pasted.as_str()));
    assert!(!asked.from_reply);
    let short = guard::polish_request("다듬어줘: 짧은 글입니다", &after_an_answer()).unwrap();
    assert_eq!(short.question.as_deref(), Some("다듬어줘: 짧은 글입니다"), "however short the text is");
    // (the apostrophes of an English sentence quote nothing: the reply is worked on)
    let english = "Please polish the last answer, it's too long and doesn't read well";
    let asked = guard::polish_request(english, &after_an_answer()).unwrap();
    assert!(asked.from_reply && asked.question.as_deref().is_some_and(|q| q.ends_with(REPLY)), "{asked:?}");
    // ③ Else the last reply of the assistant, whole, after the request — the way `carried_text` reads
    //    "instruction: text" back.
    let asked = guard::polish_request("위 내용 수정해줘", &after_an_answer()).unwrap();
    assert_eq!(asked.instruction, "위 내용 수정해줘");
    assert_eq!(asked.question, Some(format!("위 내용 수정해줘:\n{REPLY}")));
    assert!(asked.from_reply);
    assert_eq!(guard::carried_text(asked.question.as_deref().unwrap()), Some(REPLY));
    // The shell sends the reply in full next to the history, whose turns are cut for the prompt.
    let whole: String = (1..=80).map(|i| format!("{i}번 조항은 납품과 검수에 관한 내용입니다.\n")).collect();
    let mut c = after_an_answer();
    c.history[1] = ConversationTurn::assistant(&format!("{}…", whole.chars().take(499).collect::<String>()));
    c.last_reply = Some(whole.clone());
    let asked = guard::polish_request("방금 답변 다듬어줘", &c).unwrap();
    assert_eq!(asked.question, Some(format!("방금 답변 다듬어줘:\n{}", whole.trim())));
    // "There is none" from the shell (the last card was not an answer) is none: the history's last
    // turn — the text of some card — is not polished instead.
    c.last_reply = Some(String::new());
    let asked = guard::polish_request("방금 답변 다듬어줘", &c).unwrap();
    assert_eq!((asked.question, asked.from_reply), (None, false));
    // No conversation: nothing to work on.
    assert_eq!(guard::polish_request("방금 답변 다듬어줘", &open_document()).unwrap().question, None);
    // A reply to the question for the text is the text; a short one is more of the request, and the
    // question itself is never "the last reply".
    let question = guard::polish_what(true);
    let mut c = after_an_answer();
    c.history.push(ConversationTurn::user("위 내용 수정해줘"));
    c.history.push(ConversationTurn::assistant(question));
    let replied = format!("위 내용 수정해줘\n\n[어시스턴트 질문] {question}\n[사용자 답변] {BODY}");
    assert_eq!(guard::polish_request(&replied, &c).unwrap().question, Some(format!("위 내용 수정해줘:\n{BODY}")));
    let replied = format!("위 내용 수정해줘\n\n[어시스턴트 질문] {question}\n[사용자 답변] 더 짧게");
    assert_eq!(
        guard::polish_request(&replied, &c).unwrap().question,
        Some(format!("위 내용 수정해줘 더 짧게:\n{REPLY}"))
    );
    // (`PlanContext::last_reply` is not part of a context written out without it)
    assert!(!serde_json::to_string(&open_document()).unwrap().contains("last_reply"));
}

/// The plans of the measurement (no conversation in that context) and the ones the user reported
/// (with a conversation: `draft_document` / `revise_draft`), replayed with and without a reply to
/// work on.
#[test]
fn a_model_plan_that_takes_a_polish_request_elsewhere_is_answered_in_the_chat() {
    let measured: [(&str, &str, Value); 5] = [
        // ko-170 with a reply behind it: fields extracted and "요구사항" replaced in the documents
        (
            MEASURED[3],
            "edit_text",
            json!([
                { "op": "extract", "schema_ref": "s-srs" },
                { "op": "edit_text", "find": "요구사항", "replace": "다듬어진 요구사항", "pages": [] }
            ]),
        ),
        // d-2: a replacement in the open document
        (
            MEASURED[0],
            "edit_text",
            json!([{ "op": "edit_text", "find": "방금 답변", "replace": "더 자연스러운 표현", "pages": [] }]),
        ),
        // d-3, d-4: a question back
        (
            MEASURED[1],
            "clarify",
            json!([{ "op": "clarify", "question": "어떤 부분을 어떻게 수정해야 할지 구체적으로 알려주세요." }]),
        ),
        (MEASURED[2], "clarify", json!([{ "op": "clarify", "question": "어떤 내용을 더 간결하게 수정할까요?" }])),
        // d-5: the library is opened
        (MEASURED[3], "open", json!([{ "op": "open", "target": "folder", "ref": "documents" }])),
    ];
    let reported = |text: &str| {
        [
            ("draft_document", json!([{ "op": "draft_document", "content": text, "output": "md", "tone": "concise" }])),
            ("revise_draft", json!([{ "op": "revise_draft", "scope": "whole", "instruction": text }])),
            ("summarize", json!([{ "op": "summarize" }])),
            ("search", json!([{ "op": "search", "query": text }])),
            ("answer", json!([{ "op": "answer", "question": text }])),
            ("answer", json!([{ "op": "answer", "question": text, "knowledge": false, "mode": "chat" }])),
            ("noop", json!([{ "op": "noop" }])),
        ]
    };
    for (text, intent, ops) in &measured {
        let plans = std::iter::once((*intent, ops.clone())).chain(reported(text));
        for (intent, ops) in plans {
            // After an answer: the reply is worked on, in the chat.
            let r = modelled(&after_an_answer(), text, intent, ops.clone());
            assert_eq!(worked_on(&r), Some(format!("{text}:\n{REPLY}").as_str()), "{text} / {intent}: {:?}", r.plan);
            assert_eq!(r.plan.summary.as_deref(), Some("글을 다듬어 답합니다"));
            assert_eq!(r.issues.iter().map(|i| i.code.as_str()).collect::<Vec<_>>(), ["text_only_answer"], "{text}");
            assert!(r.choices.is_empty() && r.plan.confidence >= MIN_CONFIDENCE);
            // With nothing to work on: the text is asked for — nothing is written, changed or opened.
            let r = modelled(&open_document(), text, intent, ops);
            assert!(asks_for_the_text(&r), "{text} / {intent}: {:?}", r.plan);
            assert!(r.issues.is_empty(), "{:?}", r.issues);
        }
    }
    // d-1: the sentence brings its text. The model wrote a new document over it; the existing guard
    // already answers from the text, and the answer stays as it is.
    let pasted = format!("이 문장 다듬어줘: {BODY}");
    let draft = json!([{ "op": "draft_document", "content": BODY, "output": "md", "tone": "concise" }]);
    let r = modelled(&after_an_answer(), &pasted, "draft_document", draft);
    assert!(
        matches!(r.plan.operations.as_slice(), [Op::Answer { question, knowledge: Some(false), mode: None, .. }] if question == &pasted),
        "{:?}",
        r.plan
    );
    // A pasted text with a plan over documents the model named (a folder) keeps that reading, as before.
    let chat = StageChat::new();
    chat.push(
        "plan",
        json!({ "summary": "s", "unmet": "", "intent": "summarize", "scope": { "folder": "documents/2026/10" }, "operations": [{ "op": "summarize" }], "requires_confirmation": false, "confidence": 0.9 }),
    );
    let r = plan(Some(&deps(&chat)), &pasted, &after_an_answer(), PlanMode::ModelFirst).unwrap();
    assert!(matches!(r.plan.operations.as_slice(), [Op::Summarize { .. }]), "{:?}", r.plan);
    // "수정해줘: …" brings its text the way "다듬어줘: …" does; "수정된 …: …" is not a request.
    assert_eq!(guard::carried_text(&format!("다음 내용 수정해줘: {BODY}")), Some(BODY));
    assert_eq!(guard::carried_text(&format!("수정된 계약 조항 요약: {BODY}")), None);
    // … also when a document is open and the words say "이 문장" (not a question about that document),
    // and when the text is short and the model asked back or opened something.
    let short = "이 문장 다듬어줘: 갑은 을에게 돈을 준다";
    for (intent, ops) in [
        ("clarify", json!([{ "op": "clarify", "question": "어떤 문서인가요?" }])),
        ("answer", json!([{ "op": "answer", "question": "이 문장 다듬어줘" }])),
        (
            "edit_text",
            json!([{ "op": "edit_text", "find": "갑은 을에게 돈을 준다", "replace": "갑은 을에게 대금을 지급한다" }]),
        ),
    ] {
        let r = modelled(&after_an_answer(), short, intent, ops);
        assert_eq!(worked_on(&r), Some(short), "{intent}: {:?}", r.plan);
    }
}

#[test]
fn a_document_is_written_when_the_sentence_asks_for_one_and_other_readings_stay() {
    let c = after_an_answer();
    // d-7: a new document is asked for.
    let text = "다듬어서 새 문서로 만들어줘";
    let r = modelled(&c, text, "draft_document", json!([{ "op": "draft_document", "content": text }]));
    assert!(matches!(r.plan.operations.as_slice(), [Op::DraftDocument { .. }]), "{:?}", r.plan);
    // d-6: a report is named.
    let text = "이 내용으로 보고서 작성해줘";
    let r = modelled(
        &c,
        text,
        "draft_document",
        json!([{ "op": "draft_document", "form_ref": "report", "content": text }]),
    );
    assert!(matches!(r.plan.operations.as_slice(), [Op::DraftDocument { .. }]), "{:?}", r.plan);
    // With a draft open in 문서 작성 a revision of that draft stays what it is …
    let mut drafting = after_an_answer();
    drafting.draft_id = Some("dr-1".into());
    for text in ["더 간결하게 수정해줘", "3절 다시 써줘"] {
        let r = modelled(
            &drafting,
            text,
            "revise_draft",
            json!([{ "op": "revise_draft", "scope": "whole", "instruction": text }]),
        );
        assert!(
            matches!(r.plan.operations.as_slice(), [Op::ReviseDraft { draft_id: Some(id), .. }] if id == "dr-1"),
            "{text}: {:?}",
            r.plan
        );
    }
    // … while a new document nobody asked for is not written there either.
    let text = "더 간결하게 수정해줘";
    let r = modelled(&drafting, text, "draft_document", json!([{ "op": "draft_document", "content": text }]));
    assert_eq!(worked_on(&r), Some(format!("{text}:\n{REPLY}").as_str()));
    // A replacement whose texts the sentence quotes is a replacement in the documents.
    let text = "'갑'을 '을'로 수정해줘";
    let r = modelled(&c, text, "edit_text", json!([{ "op": "edit_text", "find": "갑", "replace": "을" }]));
    assert!(matches!(r.plan.operations.as_slice(), [Op::EditText { find, .. }] if find == "갑"), "{:?}", r.plan);
    assert_eq!(r.plan.scope.doc_ids, ["d1a2"], "the open document, as before");
    // A setting, a field, a tag: the model's reading stays — also when it asks back.
    let text = "테마 수정해줘";
    let r = modelled(&c, text, "clarify", json!([{ "op": "clarify", "question": "어떤 테마로 바꿀까요?" }]));
    assert_eq!(r.plan.clarification_question.as_deref(), Some("어떤 테마로 바꿀까요?"));
    let text = "공급가액 수정해줘";
    let r = modelled(&c, text, "open", json!([{ "op": "open", "target": "screen", "ref": "review" }]));
    assert!(matches!(r.plan.operations.as_slice(), [Op::Open { .. }]), "{:?}", r.plan);
    // A polish request the model read as a change of something — a title, a tag — is not taken over:
    // the plan goes the way of every change (here: which documents?).
    let text = "방금 답변 다듬어줘";
    let r = modelled(&c, text, "set_title", json!([{ "op": "set_title", "title": "방금 답변" }]));
    assert!(worked_on(&r).is_none() && !asks_for_the_text(&r) && r.plan.is_clarification(), "{:?}", r.plan);
    assert_eq!(r.plan.summary.as_deref(), Some("s"));
}

/// The routes around the plan call: a turn routed as conversation, and a plan the validator
/// rejects twice (`revise_draft` with no draft open — what a model writes with a conversation behind
/// the sentence).
#[test]
fn a_polish_request_is_answered_in_the_chat_whatever_route_the_turn_took() {
    let text = "방금 답변 좀 더 자연스럽게 다듬어줘";
    let expected = format!("{text}:\n{REPLY}");
    for route in ["chat", "general"] {
        let chat = StageChat::new();
        chat.push("families", json!({ "families": [route] }));
        let r = plan(Some(&deps(&chat)), text, &after_an_answer(), PlanMode::ModelFirst).unwrap();
        assert_eq!(worked_on(&r), Some(expected.as_str()), "{route}: {:?}", r.plan);
        assert_eq!(chat.calls("plan"), 0);
    }
    // (a greeting stays small talk)
    let chat = StageChat::new();
    chat.push("families", json!({ "families": ["chat"] }));
    let r = plan(Some(&deps(&chat)), "넌 누구야?", &after_an_answer(), PlanMode::ModelFirst).unwrap();
    assert!(matches!(r.plan.operations.as_slice(), [Op::Answer { mode: Some(AnswerMode::Chat), .. }]), "{:?}", r.plan);
    let chat = StageChat::new();
    for _ in 0..2 {
        chat.push(
            "plan",
            json!({ "summary": "s", "unmet": "", "intent": "revise_draft", "scope": {}, "operations": [{ "op": "revise_draft", "scope": "whole", "instruction": text }], "requires_confirmation": false, "confidence": 0.9 }),
        );
    }
    let r = plan(Some(&deps(&chat)), text, &after_an_answer(), PlanMode::ModelFirst).unwrap();
    assert_eq!(r.attempts, 2);
    assert_eq!(worked_on(&r), Some(expected.as_str()), "{:?}", r.plan);
    assert_eq!(r.issues.iter().map(|i| i.code.as_str()).collect::<Vec<_>>(), ["text_only_answer"]);
}

#[test]
fn the_phrase_rules_plan_a_polish_request_the_same_way() {
    for text in MEASURED {
        let r = ruled(&after_an_answer(), text);
        assert_eq!(r.source, PlanSource::Rules, "{text}");
        assert_eq!(worked_on(&r), Some(format!("{text}:\n{REPLY}").as_str()), "{text}: {:?}", r.plan);
        assert!(r.choices.is_empty() && r.issues.is_empty(), "{text}: {r:?}");
        let r = ruled(&open_document(), text);
        assert!(asks_for_the_text(&r), "{text}: {:?}", r.plan);
    }
    let pasted = format!("이 문장 다듬어줘: {BODY}");
    assert_eq!(worked_on(&ruled(&open_document(), &pasted)), Some(pasted.as_str()));
    // A subject that is also a project's name is not asked about: nothing is looked up.
    let text = "한빛 납품에 대한 설명 다듬어줘";
    let r = ruled(&after_an_answer(), text);
    assert_eq!(worked_on(&r), Some(format!("{text}:\n{REPLY}").as_str()), "{:?}", r.plan);
    // The readings the rules had stay: a new document, a section of the draft, a replacement, a
    // setting, sorting files, a total.
    let mut c = after_an_answer();
    c.draft_id = Some("dr-1".into());
    let kind = |text: &str| ruled(&c, text).plan.operations.iter().map(Op::kind).collect::<Vec<_>>();
    assert_eq!(kind("다듬어서 새 문서로 만들어줘"), [OpKind::DraftDocument]);
    assert_eq!(kind("이 내용으로 보고서 작성해줘"), [OpKind::DraftDocument]);
    assert_eq!(kind("3절 다시 써줘"), [OpKind::ReviseDraft]);
    assert_eq!(kind("초안 전체 격식체로 고쳐줘"), [OpKind::ReviseDraft]);
    assert_eq!(kind("'갑'을 '을'로 고쳐줘"), [OpKind::EditText]);
    assert_eq!(kind("테마를 다크로 바꿔줘"), [OpKind::SetSetting]);
    assert_eq!(kind("영수증을 월별 폴더로 정리해줘"), [OpKind::Move]);
    assert_eq!(kind("지난달 영수증 합계 다시 써줘"), kind("지난달 영수증 합계"));
    // What the rules do not read and is no polish request is still left to the model.
    for text in ["테마 수정해줘", "공급가액 수정해줘", "'갑'을 '을'로 수정해줘"] {
        let r = ruled(&c, text);
        assert!(r.issues.iter().any(|i| i.code == "no_model"), "{text}: {:?}", r.plan);
    }
}
