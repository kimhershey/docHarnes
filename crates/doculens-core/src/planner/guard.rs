//! Post-model sanity rule for sentences that carry their own text.
//!
//! "다음 문장을 다듬어 정리해줘: …" asks about the text inside the sentence, not about the library, yet a
//! planner model that only knows document ops answers it with `summarize` over every document (one
//! retrieval per document, 55 of them on the first report). The model's reading stays authoritative for
//! everything else; this only re-targets a knowledge-base op without an explicit folder / filter when the
//! sentence demonstrably brought the material along.

use serde_json::Value;

use super::settings_catalog::{self as catalog, ValueType};
use super::{
    fastpath, AnswerMode, GroupBy, ModelAction, Op, OpKind, Plan, PlanIssue, Scope, SummarizeTarget, SummaryGroup,
};

/// A folder or a content filter names documents on purpose. A date range alone does not: the date
/// resolver fills it from any date in the sentence, including dates inside the pasted text.
fn explicit_scope(plan: &Plan) -> bool {
    let f = &plan.scope.filter;
    plan.scope.folder.is_some()
        || !plan.scope.doc_ids.is_empty()
        || f.doc_type.is_some()
        || f.keyword.is_some()
        || f.vendor.is_some()
        || f.status.is_some()
        || !f.tags.is_empty()
        || f.min_confidence.is_some()
        || f.review_status.is_some()
}

/// Verbs that act on text the user supplied.
const REWRITE_KO: &[&str] = &[
    "다듬",
    "정리해",
    "정리 해",
    "교정",
    "맞춤법",
    "번역",
    "고쳐",
    "다시 써",
    "다시써",
    "윤문",
    "요약해",
    "줄여",
    "풀어 써",
    "풀어써",
    "바꿔 써",
    "존댓말",
    "반말",
    "문장으로",
    "개조식으로",
    "매끄럽",
];
const REWRITE_EN: &[&str] = &[
    "rewrite",
    "polish",
    "proofread",
    "translate",
    "tidy",
    "rephrase",
    "paraphrase",
    "summarise",
    "summarize",
    "shorten",
    "grammar",
    "formal",
    "bullet",
];
/// Words of an instruction that asks for a document to be written, not for its text to be worked on.
const DRAFT_WORDS: &[&str] =
    &["작성", "초안", "만들", "문서로", "양식", "draft", "create", "compose", "generate", "document"];
/// Shortest body that counts as "the text came with the request".
const MIN_BODY_CHARS: usize = 60;
/// Longest instruction part; anything longer is a question in its own right.
const MAX_INSTRUCTION_CHARS: usize = 160;

fn has_rewrite_verb(s: &str) -> bool {
    let lower = s.to_lowercase();
    REWRITE_KO.iter().chain(REWRITE_EN).any(|w| lower.contains(w))
}

/// The text a request brings along: "instruction + separator + body" (colon, newline or quotes) or
/// "body + newline + instruction", with a rewrite verb in the instruction and a body of at least
/// `MIN_BODY_CHARS` characters.
pub fn carried_text(text: &str) -> Option<&str> {
    // Of a reply to the assistant's question ("command / question / reply") only the command can carry
    // a text: the question and the reply are not something to work on (and the question's own colon
    // and line breaks would read as "instruction: text").
    let text = super::rules::command_of(text).trim();
    for sep in [':', '：', '\n', '"', '“', '「', '『'] {
        // ("10:30" and "3:1" are not "instruction: text")
        let between_digits = |pos: usize| {
            text[..pos].chars().next_back().is_some_and(|c| c.is_ascii_digit())
                && text[pos + sep.len_utf8()..].chars().next().is_some_and(|c| c.is_ascii_digit())
        };
        if let Some(pos) = text.match_indices(sep).map(|(i, _)| i).find(|i| !between_digits(*i)) {
            let (head, tail) = text.split_at(pos);
            let body = tail
                .trim_start_matches(sep)
                .trim()
                .trim_start_matches(['"', '“', '「', '『'])
                .trim_end_matches(['"', '”', '」', '』'])
                .trim();
            if head.chars().count() <= MAX_INSTRUCTION_CHARS
                && has_rewrite_verb(head)
                && body.chars().count() >= MIN_BODY_CHARS
            {
                return Some(body);
            }
        }
    }
    if let Some(pos) = text.rfind('\n') {
        let (body, tail) = text.split_at(pos);
        let tail = tail.trim();
        if tail.chars().count() <= MAX_INSTRUCTION_CHARS
            && has_rewrite_verb(tail)
            && body.trim().chars().count() >= MIN_BODY_CHARS
        {
            return Some(body.trim());
        }
    }
    None
}

/// Turns `plan` into one text-only `answer` over the user's whole sentence.
pub fn make_text_only(text: &str, plan: &mut Plan, ko: bool) {
    plan.intent = OpKind::Answer;
    plan.scope = Scope::default();
    plan.operations = vec![Op::Answer {
        question: text.trim().to_string(),
        cite: Some(false),
        knowledge: Some(false),
        mode: None,
        reply: None,
    }];
    plan.requires_confirmation = false;
    plan.clarification_question = None;
    plan.unmet = None;
    plan.summary = Some(
        if ko {
            "보내 주신 글을 문서 검색 없이 요청대로 다듬습니다"
        } else {
            "Works on the text you sent, without looking documents up"
        }
        .to_string(),
    );
}

/// Re-targets a plan made of knowledge-base ops (summarize / answer / explain / compare / search) with
/// no folder or filter to one text-only `answer` when the sentence carries the text it talks about.
/// Returns the note shown on the card when it did.
pub fn prefer_text_only_answer(text: &str, plan: &mut Plan, ko: bool) -> Option<PlanIssue> {
    let body = carried_text(text)?;
    if plan.operations.is_empty() || explicit_scope(plan) {
        return None;
    }
    // A text to polish that the model turned into a new document ("다음 문장을 다듬어줘: …" came back from
    // the real model as `draft_document` over that text) is still a text to polish — unless the
    // instruction itself asks for a document.
    let instruction = super::rules::command_of(text).replace(body, " ").to_lowercase();
    let asks_for_document = DRAFT_WORDS.iter().any(|w| instruction.contains(w));
    let knowledge_only = plan.operations.iter().all(|op| match op {
        Op::Summarize { .. } | Op::Explain { .. } | Op::Compare { .. } | Op::Search { .. } => true,
        Op::Answer { knowledge, .. } => knowledge.unwrap_or(true),
        Op::DraftDocument { sources, .. } => !asks_for_document && sources.is_empty(),
        _ => false,
    });
    if !knowledge_only {
        return None;
    }
    make_text_only(text, plan, ko);
    Some(PlanIssue::warn(
        "operations",
        "text_only_answer",
        if ko {
            "문장에 글이 들어 있어 문서를 찾는 대신 그 글로 답합니다"
        } else {
            "the sentence carries its own text; answering from it instead of searching documents"
        },
    ))
}

// ---------------------------------------------------------------------------
// 0.8.0 plan §1.2 – §1.5: what a model plan must pass before it may run unasked
// ---------------------------------------------------------------------------

/// Bodies longer than this are not sent to the planner in full (audit PLN-16).
const LONG_BODY_CHARS: usize = 240;

/// The text the planner model reads. A sentence that carries a long text of its own ("다음 글을
/// 다듬어줘: …") keeps its instruction and an excerpt of the body: the model only has to decide
/// what to do with the text, and copying a long body into `answer.question` would not fit the
/// reply budget. `restore_carried_text` puts the whole text back afterwards.
pub fn planning_text(text: &str) -> std::borrow::Cow<'_, str> {
    let Some(body) = carried_text(text) else {
        return std::borrow::Cow::Borrowed(text);
    };
    let n = body.chars().count();
    if n <= LONG_BODY_CHARS {
        return std::borrow::Cow::Borrowed(text);
    }
    let Some(start) = text.find(body) else {
        return std::borrow::Cow::Borrowed(text);
    };
    let head: String = body.chars().take(80).collect();
    let tail: String = body.chars().skip(n - 40).collect();
    let excerpt = format!("{head} … {tail} [붙여 넣은 글 {n}자 / pasted text, {n} characters]");
    std::borrow::Cow::Owned(format!("{}{}{}", &text[..start], excerpt, &text[start + body.len()..]))
}

/// After planning from `planning_text`: a text-only answer gets the user's whole sentence back.
pub fn restore_carried_text(text: &str, plan: &mut Plan) {
    if carried_text(text).is_none() {
        return;
    }
    // (Of a reply to the assistant's question the whole exchange goes back in: the reply — "영어로" —
    // is part of what to do with the text.)
    for op in &mut plan.operations {
        if let Op::Answer { question, knowledge: Some(false), mode: None, .. } = op {
            *question = text.trim().to_string();
        }
    }
}

/// Whether the user's words name the project the assistant's scope is set to ("한빛 프로젝트 문서를
/// 프로젝트별로 …"): then its id in a plan's scope is the user's, not the default scope's.
pub fn names_scope_project(text: &str, ctx: &super::PlanContext) -> bool {
    let Some(id) = ctx.scope_project.as_deref() else { return false };
    let said = super::compact(&super::rules::user_words(text));
    ctx.projects.iter().any(|p| {
        p.id == id && {
            let name = super::compact(&p.name);
            !name.is_empty() && said.contains(&name)
        }
    })
}

/// Whether the plan only says "nothing to do".
pub fn is_noop_only(plan: &Plan) -> bool {
    !plan.operations.is_empty() && plan.operations.iter().all(|o| matches!(o, Op::Noop {}))
}

/// A plan that only says "nothing to do" is a turn that was not a command: answer it instead of
/// showing an empty plan card and a "skipped" result (audit USR-3). When the model also said what
/// it could not do (`unmet`), that is the answer — said as it is, without another model call.
pub fn noop_to_chat(text: &str, plan: &mut Plan, ko: bool) -> bool {
    if !is_noop_only(plan) {
        return false;
    }
    let unmet = plan.unmet.as_deref().map(str::trim).filter(|u| !u.is_empty()).map(str::to_string);
    let reply = unmet.as_ref().map(|u| {
        if ko {
            format!("이 요청은 지금 할 수 없습니다: {u}")
        } else {
            format!("I cannot do this: {u}")
        }
    });
    plan.operations = vec![Op::Answer {
        question: text.trim().to_string(),
        cite: Some(false),
        knowledge: Some(false),
        mode: Some(AnswerMode::Chat),
        reply,
    }];
    plan.intent = OpKind::Answer;
    plan.scope = Scope::default();
    plan.requires_confirmation = false;
    plan.unmet = None;
    // The model's confidence in "nothing to do" is not a reason to ask "did I understand you?".
    plan.confidence = plan.confidence.max(super::MIN_CONFIDENCE);
    plan.summary = Some(
        match (unmet.is_some(), ko) {
            (true, true) => "할 수 없는 요청이라고 답합니다",
            (true, false) => "Says that it cannot do this",
            (false, true) => "명령이 아닌 말에 답합니다",
            (false, false) => "Replies to a turn that is not a command",
        }
        .to_string(),
    );
    true
}

/// What a sentence asks to be split by ("프로젝트별", "유형 별로", "각 프로젝트", "폴더마다",
/// "by project", "per vendor" …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitBy {
    /// 문서별 · 파일별 — one result per document.
    Document,
    Project,
    DocType,
    Folder,
    Vendor,
    Month,
    /// A criterion no op can express (담당자별, 분기별 …).
    Other,
}

const SPLIT_NOUNS: &[(&str, SplitBy)] = &[
    ("프로젝트", SplitBy::Project),
    ("유형", SplitBy::DocType),
    ("종류", SplitBy::DocType),
    ("타입", SplitBy::DocType),
    ("폴더", SplitBy::Folder),
    ("거래처", SplitBy::Vendor),
    ("업체", SplitBy::Vendor),
    ("월", SplitBy::Month),
    ("담당자", SplitBy::Other),
    ("연도", SplitBy::Other),
    ("년도", SplitBy::Other),
    ("분기", SplitBy::Other),
    ("날짜", SplitBy::Other),
    ("문서", SplitBy::Document),
    ("파일", SplitBy::Document),
];
const SPLIT_PHRASES_EN: &[(&str, SplitBy)] = &[
    ("by project", SplitBy::Project),
    ("per project", SplitBy::Project),
    ("each project", SplitBy::Project),
    ("by type", SplitBy::DocType),
    ("per type", SplitBy::DocType),
    ("by document type", SplitBy::DocType),
    ("by folder", SplitBy::Folder),
    ("per folder", SplitBy::Folder),
    ("each folder", SplitBy::Folder),
    ("by vendor", SplitBy::Vendor),
    ("per vendor", SplitBy::Vendor),
    ("by month", SplitBy::Month),
    ("per month", SplitBy::Month),
    ("monthly", SplitBy::Month),
    ("by owner", SplitBy::Other),
    ("by quarter", SplitBy::Other),
    ("by year", SplitBy::Other),
    ("per document", SplitBy::Document),
    ("each document", SplitBy::Document),
    ("by document", SplitBy::Document),
];

/// Endings of a word after "N별" that make the pair a title, not a way of splitting:
/// "거래처별 단가표", "월별 보고서", "유형별 폴더" — against "거래처별 합계", "프로젝트 별 내용".
const TITLE_ENDINGS: &[&str] = &[
    "표",
    "보고서",
    "리포트",
    "폴더",
    "목록",
    "대장",
    "명세서",
    "내역서",
    "시트",
    "파일",
    "자료",
    "양식",
    "report",
    "sheet",
    "folder",
];
const NOUN_PARTICLES: &[&str] = &["으로", "에서", "의", "을", "를", "은", "는", "이", "가", "로", "에", "도"];

/// `noun` + "별" as the way to split: "프로젝트별로", "프로젝트 별 내용을 요약", "거래처별 합계" —
/// not "프로젝트 별도 관리", "월별표" or a title such as "월별 보고서".
fn has_byeol(text: &str, noun: &str) -> bool {
    [format!("{noun}별"), format!("{noun} 별")].iter().any(|needle| {
        text.match_indices(needle.as_str()).any(|(at, _)| {
            let rest = &text[at + needle.len()..];
            if rest.is_empty() || rest.starts_with('로') {
                return true;
            }
            // Glued to another syllable ("별도", "별표"): not the suffix.
            if rest.chars().next().is_some_and(|c| ('가'..='힣').contains(&c)) {
                return false;
            }
            let mut next = rest.split_whitespace().next().unwrap_or_default();
            next = next.trim_end_matches(|c: char| !c.is_alphanumeric());
            if let Some(stem) = NOUN_PARTICLES.iter().find_map(|p| next.strip_suffix(p)) {
                next = stem;
            }
            !TITLE_ENDINGS.iter().any(|t| next.ends_with(t))
        })
    })
}

/// The criterion the sentence asks to split the result by, if any.
pub fn split_of(text: &str) -> Option<SplitBy> {
    let t = text.to_lowercase();
    for (noun, by) in SPLIT_NOUNS {
        let each = [format!("{noun}마다"), format!("{noun} 마다"), format!("각 {noun}"), format!("각각의 {noun}")];
        // ("월" alone is too short for "각 월…": 각 월요일)
        let named = has_byeol(&t, noun) || (*noun != "월" && each.iter().any(|e| t.contains(e.as_str())));
        if named {
            return Some(*by);
        }
    }
    SPLIT_PHRASES_EN.iter().find(|(phrase, _)| t.contains(phrase)).map(|(_, by)| *by)
}

/// The grouping of a summary the sentence names, when `summarize` can express it.
pub fn summary_group(text: &str) -> Option<SummaryGroup> {
    match split_of(text)? {
        SplitBy::Project => Some(SummaryGroup::Project),
        SplitBy::DocType => Some(SummaryGroup::DocType),
        SplitBy::Folder => Some(SummaryGroup::Folder),
        _ => None,
    }
}

/// The grouping of an aggregate the sentence names, when `aggregate` can express it.
fn aggregate_group(text: &str) -> Option<GroupBy> {
    match split_of(text)? {
        SplitBy::Project => Some(GroupBy::Project),
        SplitBy::DocType => Some(GroupBy::DocType),
        SplitBy::Vendor => Some(GroupBy::Vendor),
        SplitBy::Month => Some(GroupBy::Month),
        _ => None,
    }
}

/// Puts the split the sentence names into the ops that can express it ("프로젝트별로 요약해줘" with a
/// plan that summarises everything as one): the criterion is in the sentence, so this is a repair,
/// not a guess. Returns whether an op that drops the split is left.
fn keep_the_split(text: &str, plan: &mut Plan) -> bool {
    let Some(split) = split_of(text) else { return false };
    // A grouping the model chose where the sentence names another one — or one these ops do not have
    // ("담당자별로 요약" came back as `group_by: folder` from the real model and ran) — is a guess: the
    // sentence's own criterion goes in, or the grouping is taken out and the split reported as dropped.
    let another_step_groups = plan.operations.iter().any(|op| match op {
        Op::Move { group_by, .. } => !matches!(group_by, None | Some(GroupBy::None)),
        Op::Organize { .. } | Op::Export { .. } | Op::ClassifyProjects { .. } => true,
        _ => false,
    });
    let mut guessed = false;
    if split != SplitBy::Document && !another_step_groups {
        for op in &mut plan.operations {
            match op {
                Op::Summarize { group_by, .. } if !matches!(group_by, None | Some(SummaryGroup::None)) => {
                    match summary_group(text) {
                        Some(group) => *group_by = Some(group),
                        None => {
                            *group_by = None;
                            guessed = true;
                        }
                    }
                }
                Op::Aggregate { group_by, .. } if !matches!(group_by, None | Some(GroupBy::None)) => {
                    match aggregate_group(text) {
                        Some(group) => *group_by = Some(group),
                        None => {
                            *group_by = None;
                            guessed = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if guessed {
        return true;
    }
    // The split may belong to another step ("유형별 폴더로 정리하고 요약해줘"): a plan that
    // already groups something has taken it.
    let taken = plan.operations.iter().any(|op| match op {
        Op::Move { group_by, .. } => !matches!(group_by, None | Some(GroupBy::None)),
        Op::Organize { .. } | Op::Export { .. } | Op::ClassifyProjects { .. } => true,
        Op::Aggregate { group_by, .. } => !matches!(group_by, None | Some(GroupBy::None)),
        Op::Summarize { group_by, .. } | Op::Search { group_by, .. } => {
            !matches!(group_by, None | Some(SummaryGroup::None))
        }
        _ => false,
    });
    if taken {
        return false;
    }
    let mut dropped = false;
    for op in &mut plan.operations {
        match op {
            // A summary about a subject is one summary of the passages found (1.0.2): no split.
            Op::Summarize { topic: Some(_), .. } => dropped = true,
            Op::Summarize { target, group_by, .. } if matches!(group_by, None | Some(SummaryGroup::None)) => {
                if split == SplitBy::Document {
                    if *target == Some(SummarizeTarget::Set) {
                        *target = Some(SummarizeTarget::EachDoc);
                    }
                } else if let Some(group) = summary_group(text) {
                    *group_by = Some(group);
                } else {
                    dropped = true;
                }
            }
            // "프로젝트별로 검색해줘" (0.8.1): the hits per project / type / folder.
            Op::Search { group_by, .. }
                if split != SplitBy::Document && matches!(group_by, None | Some(SummaryGroup::None)) =>
            {
                match summary_group(text) {
                    Some(group) => *group_by = Some(group),
                    None => dropped = true,
                }
            }
            Op::Aggregate { group_by, .. }
                if split != SplitBy::Document && matches!(group_by, None | Some(GroupBy::None)) =>
            {
                match aggregate_group(text) {
                    Some(group) => *group_by = Some(group),
                    None => dropped = true,
                }
            }
            _ => {}
        }
    }
    dropped
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.to_lowercase(),
        other => other.to_string(),
    }
}

/// The value is something the sentence says: the number, an on / off word, the value's name.
fn value_is_grounded(text: &str, key: &str, ty: ValueType, value: &Value) -> bool {
    let said = |v: f64| fastpath::numbers_in(text).iter().any(|n| (n - v).abs() < 1e-9);
    match ty {
        ValueType::Bool => value.as_bool().is_some_and(|b| fastpath::polarity(text) == Some(b)),
        ValueType::Int { .. } | ValueType::IntChoice(_) => value.as_f64().is_some_and(said),
        // A ratio may be said as a percentage ("90%" for 0.9); nothing else is divided.
        ValueType::Float { percent, .. } => {
            value.as_f64().is_some_and(|v| said(v) || (percent && text.contains('%') && said(v * 100.0)))
        }
        ValueType::Enum(_) => {
            let v = value_text(value);
            fastpath::mentions(text, &v)
                || fastpath::mentions(text, &v.replace('_', " "))
                || catalog::value_names(key)
                    .iter()
                    .any(|(name, said)| *name == v && said.iter().any(|s| fastpath::mentions(text, s)))
        }
        ValueType::Str => {
            let v = value_text(value);
            !v.is_empty() && text.to_lowercase().contains(&v)
        }
    }
}

/// A model id is named when the sentence has the part that tells the models apart: its size
/// ("12b", "e4b") — the family ("gemma4") is in every id — or the whole id.
fn model_is_named(text: &str, id: &str) -> bool {
    let t: String = text.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let id = id.to_lowercase();
    if t.contains(&id) {
        return true;
    }
    let is_size = |part: &str| {
        let digits = part.trim_start_matches('e').trim_end_matches('b');
        part.ends_with('b') && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
    };
    let sizes: Vec<&str> = id.split(['-', '_']).filter(|part| is_size(part)).collect();
    !sizes.is_empty() && sizes.iter().all(|size| t.contains(size))
}

/// A settings change of the plan: what it changes, and whether the sentence names it
/// (audit USR-1: "UI 배율 120%" ran `set_profile balanced` on the model's own confidence of 0.95).
/// Both have to be in the sentence: the setting and the value ("영어로 번역해줘" has a language,
/// not the language setting).
fn setting_change(text: &str, op: &Op) -> Option<(String, bool)> {
    let named = |key: &str| {
        catalog::find(key).is_some_and(|(_, spec)| catalog::names_of(spec).iter().any(|n| fastpath::mentions(text, n)))
    };
    let grounded = |key: &str, value: &Value| {
        catalog::find(key)
            .is_some_and(|(canonical, spec)| named(canonical) && value_is_grounded(text, canonical, spec.ty, value))
    };
    match op {
        Op::SetSetting { key, value } => {
            let (canonical, spec) = catalog::find(key)?;
            // A model is named by what tells the models apart ("12B"), not by the setting's label.
            if let ("models.active_model", Some(id)) = (canonical, value.as_str().filter(|v| *v != "auto")) {
                return Some((format!("모델 → {id}"), model_is_named(text, id)));
            }
            Some((format!("{} → {}", spec.label_ko, value_text(value)), grounded(key, value)))
        }
        Op::SetTheme { theme } => {
            let v = Value::String(theme.as_str().to_string());
            Some((format!("테마 → {}", theme.as_str()), grounded("ui.theme", &v)))
        }
        Op::SetLanguage { lang } => {
            let v = Value::String(lang.as_str().to_string());
            Some((format!("언어 → {}", lang.as_str()), grounded("ui.language", &v)))
        }
        Op::SetProfile { profile } => {
            let v = serde_json::to_value(profile).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            let ok = grounded("models.profile", &Value::String(v.clone()));
            Some((format!("추론 프로파일 → {v}"), ok))
        }
        Op::SetInboxWatch { enabled } => Some((
            format!("inbox 감시 → {}", if *enabled { "켜기" } else { "끄기" }),
            grounded("processing.inbox_watch", &Value::Bool(*enabled)),
        )),
        Op::SetModel { id } => Some((format!("모델 → {id}"), model_is_named(text, id))),
        Op::ManageModel { action: ModelAction::SetTier, tier: Some(tier), .. } => {
            Some((format!("티어 → {tier}"), grounded("models.manual_tier", &Value::String(tier.clone()))))
        }
        Op::ManageModel { action: ModelAction::SetBackend, backend: Some(backend), .. } => {
            Some((format!("백엔드 → {backend}"), grounded("models.backend", &Value::String(backend.clone()))))
        }
        _ => None,
    }
}

/// The split the sentence asks for ("~별", "마다", "by …"): filled in when the op has the criterion,
/// shown and confirmed when it has not (담당자별 …). Also applied to plans made by the phrase rules.
pub fn split_check(text: &str, plan: &mut Plan, ko: bool) -> Option<PlanIssue> {
    let said = super::rules::user_words(text);
    if plan.is_clarification() || !keep_the_split(&said, plan) {
        return None;
    }
    plan.requires_confirmation = true;
    if plan.unmet.is_none() {
        let what = if ko { "묶음별로 나누는 부분('~별')" } else { "the split into groups ('by …')" };
        plan.unmet = Some(what.to_string());
    }
    Some(PlanIssue::warn(
        "operations",
        "grouping_dropped",
        if ko {
            "문장은 나눠서 처리하라고 하는데 이 기준으로는 나눌 수 없어 한 덩어리로 처리합니다"
        } else {
            "the sentence asks for one result per group, which this plan cannot do; it makes a single one"
        },
    ))
}

/// "배율 130으로 바꾸고 견적서 요약해줘" came back from the real model as a summary with an empty scope:
/// every document instead of the estimates. When the user's words name exactly one document type,
/// the plan names no documents at all and its ops take a type as a filter (never as a value — a
/// classification target, a draft's template, a form), the type goes into the scope, as the phrase
/// rules do for the same sentence. Returns the note for the card.
pub fn type_filter_repair(text: &str, plan: &mut Plan, ctx: &super::PlanContext, ko: bool) -> Option<PlanIssue> {
    if plan.is_clarification() || !plan.scope.is_empty() {
        return None;
    }
    let filters_by_type = |op: &Op| {
        matches!(
            op,
            Op::Summarize { .. }
                | Op::Aggregate { .. }
                | Op::Export { .. }
                | Op::Extract { .. }
                | Op::Ocr { .. }
                | Op::Validate {}
                | Op::Organize { .. }
                | Op::Rename { .. }
                | Op::Move { .. }
                | Op::Tag { .. }
                | Op::Delete { .. }
        )
    };
    // Settings, navigation and the like may accompany them; anything else that works on documents or
    // takes a type as its value leaves the plan alone.
    let neutral = |op: &Op| {
        matches!(
            op,
            Op::SetSetting { .. }
                | Op::SetTheme { .. }
                | Op::SetLanguage { .. }
                | Op::SetProfile { .. }
                | Op::SetModel { .. }
                | Op::SetInboxWatch { .. }
                | Op::Open { .. }
                | Op::Help {}
        )
    };
    if !plan.operations.iter().any(filters_by_type)
        || !plan.operations.iter().all(|op| filters_by_type(op) || neutral(op))
    {
        return None;
    }
    // (a type the subject of a summary names is part of the subject, not a filter)
    let named = super::rules::doc_types_named(&scope_words(text, plan), ctx);
    let [only] = named.as_slice() else { return None };
    plan.scope.filter.doc_type = Some(only.slug.clone());
    Some(PlanIssue::warn(
        "scope.filter.doc_type",
        "scope_filled",
        if ko {
            format!("문장의 '{}'을(를) 대상 조건으로 넣었습니다", only.label_ko)
        } else {
            format!("'{}' from the sentence was added as the document filter", only.label_en)
        },
    ))
}

/// Plans that only ask about what the documents say: questions, searches, explanations,
/// comparisons and summaries. Nothing is changed, so they are answered rather than asked back.
pub fn only_asks(plan: &Plan) -> bool {
    !plan.operations.is_empty()
        && plan.operations.iter().all(|op| {
            matches!(
                op,
                Op::Answer { .. } | Op::Search { .. } | Op::Explain { .. } | Op::Compare { .. } | Op::Summarize { .. }
            )
        })
}

/// What a model writes into the type slot to say "whatever the type".
fn every_type(value: &str) -> bool {
    const ALL: &[&str] = &[
        "",
        "all",
        "any",
        "every",
        "everything",
        "none",
        "null",
        "alltypes",
        "anytype",
        "alldocuments",
        "document",
        "documents",
        "전체",
        "전부",
        "모두",
        "모든",
        "문서",
        "모든문서",
        "전체문서",
        "모든유형",
        "전체유형",
        "없음",
        "상관없음",
        "무관",
    ];
    ALL.contains(&super::compact(value).as_str())
}

/// 0.8.2 — a type is a condition only when the user's words name one. The scope the model writes has
/// no place for "this project" or "everything", so on the user's PC "AAA 프로젝트 내에서 AAA가 무엇인지
/// 설명해줘" came back with the type slot filled with `all` (the validator rejected the plan twice:
/// "unknown document type 'all'"). Run before the validator:
/// - "all" / "전체" in the type slot is no condition, for every plan;
/// - a type that does not exist is left out of a plan that only asks (a command keeps the
///   validator's error and its re-query: leaving the condition out would widen what it changes);
/// - a type the user's words do not name is left out of a question / search / explanation /
///   comparison (a summary keeps the type the model chose: it reads every document in scope).
pub fn untyped_scope(text: &str, plan: &mut Plan, ctx: &super::PlanContext, ko: bool) -> Option<PlanIssue> {
    let written = plan.scope.filter.doc_type.clone()?;
    if ctx.doc_types.is_empty() {
        return None;
    }
    let Some(known) = ctx.doc_type(&written) else {
        if every_type(&written) {
            plan.scope.filter.doc_type = None;
            return None;
        }
        if !only_asks(plan) {
            return None;
        }
        plan.scope.filter.doc_type = None;
        // The project's name in the type slot: `where_to_look` scopes the plan to it, nothing to say.
        let key = super::compact(&written);
        // (the real model also writes "project" and "project:AAA" there)
        let is_project = key.starts_with("project")
            || key.contains("프로젝트")
            || (key.chars().count() >= 2
                && ctx.projects.iter().any(|p| {
                    let name = super::compact(&p.name);
                    !name.is_empty() && (name.contains(&key) || key.contains(&name))
                }));
        return (!is_project).then(|| {
            PlanIssue::warn(
                "scope.filter.doc_type",
                "doc_type_ignored",
                if ko {
                    format!("'{written}' 유형은 없어 유형 조건 없이 진행합니다")
                } else {
                    format!("there is no document type '{written}'; continuing without a type condition")
                },
            )
        });
    };
    let retrieves = plan
        .operations
        .iter()
        .all(|op| matches!(op, Op::Answer { .. } | Op::Search { .. } | Op::Explain { .. } | Op::Compare { .. }));
    if plan.operations.is_empty() || !retrieves {
        return None;
    }
    let named = super::rules::doc_types_named(&super::rules::user_words(text), ctx);
    if named.iter().any(|d| d.slug == known.slug) {
        return None;
    }
    let (label_ko, label_en) = (known.label_ko.clone(), known.label_en.clone());
    plan.scope.filter.doc_type = None;
    Some(PlanIssue::warn(
        "scope.filter.doc_type",
        "doc_type_ignored",
        if ko {
            format!("문장에 없는 유형 조건('{label_ko}')은 빼고 찾습니다")
        } else {
            format!("the type condition '{label_en}' is not in the sentence and was left out")
        },
    ))
}

/// 0.8.2 — a sentence the routing call read as a question / search / summary is answered, not
/// answered with a question the app can settle itself: which type of document (never needed to
/// read), or where to look when the sentence names a project or one is open. A clarification
/// becomes a grounded answer to the user's words (`where_to_look` then scopes it); a plan that only
/// asks and came with a low confidence runs as planned. Any other question is still asked.
pub fn answer_instead_of_asking(
    text: &str,
    plan: &mut Plan,
    ctx: &super::PlanContext,
    families: &[super::OpFamily],
    ko: bool,
) -> bool {
    if families != [super::OpFamily::Ask] || carried_text(text).is_some() {
        return false;
    }
    // Either the model asked back, or it planned a question / search / summary it was unsure about
    // (which `finalize` would turn into "… 맞나요?").
    let unsure = !plan.is_clarification() && only_asks(plan) && plan.confidence < super::MIN_CONFIDENCE;
    if !plan.is_clarification() && !unsure {
        return false;
    }
    let question = plan
        .clarification_question
        .clone()
        .or_else(|| {
            plan.operations.iter().find_map(|op| match op {
                Op::Clarify { question } => Some(question.clone()),
                _ => None,
            })
        })
        .unwrap_or_default();
    let lower = question.to_lowercase();
    let about_type = ["유형", "종류", "형식", "doc_type", "document type", "type of document"]
        .iter()
        .any(|w| lower.contains(w))
        || ctx.doc_types.iter().any(|d| d.slug.len() >= 3 && lower.contains(&format!("'{}'", d.slug.to_lowercase())));
    if !(about_type || project_named(text, ctx).is_some() || ctx.scope_project.is_some()) {
        return false;
    }
    plan.clarification_question = None;
    plan.confidence = plan.confidence.max(super::MIN_CONFIDENCE);
    if unsure {
        return true;
    }
    let said = super::rules::user_words(text);
    plan.operations =
        vec![Op::Answer { question: said.trim().to_string(), cite: None, knowledge: None, mode: None, reply: None }];
    plan.intent = super::OpKind::Answer;
    plan.requires_confirmation = false;
    plan.summary = Some(if ko {
        "문서에서 찾아 답합니다".to_string()
    } else {
        "Answers from the documents".to_string()
    });
    true
}

/// A subject a sentence names — "…에 대해(서)", "…에 관해(서)", "…에 대한", "…에 관한", "… 관련",
/// "about …" — and the sentence without the phrase that names it.
#[derive(Debug, Clone, PartialEq)]
pub struct About {
    pub subject: String,
    /// The rest of the sentence: where to look and what to do ("한빛 프로젝트에서 요약해줘").
    pub rest: String,
}

/// What the sentence asks to hear about: "한빛 프로젝트에서 검수 기준에 대해 요약해줘" → `검수 기준`
/// (rest "한빛 프로젝트에서 요약해줘"). Nothing for a sentence that brings its own text, and nothing
/// when the words before the marker only point at something ("이 문서에 대해", "그것에 관해", "이
/// 프로젝트에 대해") or name a project as such ("AAA 프로젝트에 대해"): what is pointed at is where
/// to look, not something to look up.
pub fn about(text: &str) -> Option<About> {
    about_phrase(text).filter(|a| names_a_subject(&a.subject))
}

/// The phrase that names what a sentence is about, cut off the sentence — whatever its words are
/// (a subject, a pointer, the name of a project).
fn about_phrase(text: &str) -> Option<About> {
    if carried_text(text).is_some() {
        return None;
    }
    let words = super::rules::user_words(text);
    let line: String = words.split_whitespace().collect::<Vec<_>>().join(" ");
    phrase_ko(&line).or_else(|| phrase_en(&line))
}

/// Whether the words of an "about" phrase name something to look up: not a pointer at what is open
/// or selected ("이 문서", "그것", "this document"), not everything ("전체", "모든 문서"), not a kind
/// of document ("참고 자료") and not a project called one ("AAA 프로젝트").
fn names_a_subject(subject: &str) -> bool {
    const POINTERS: &[&str] = &[
        "이",
        "그",
        "저",
        "위",
        "위의",
        "아래",
        "해당",
        "선택한",
        "선택된",
        "첨부한",
        "첨부된",
        "방금",
        "앞의",
        "앞서",
    ];
    const POINTED: &[&str] = &[
        "이것",
        "이거",
        "그것",
        "그거",
        "저것",
        "저거",
        "여기",
        "이번",
        "이내용",
        "위내용",
        "위의내용",
        "아래내용",
        "선택문서",
        "문서",
        "문서들",
        "자료",
        "내용",
        "전체",
        "모두",
        "모든문서",
        // (a kind of document is where to look — `kind_said`)
        "참고자료",
        "참고문서",
        "레퍼런스",
        "업무문서",
        "작업문서",
        "it",
        "this",
        "that",
        "them",
        "these",
        "those",
        "everything",
        "thedocuments",
        "alldocuments",
    ];
    const POINTING: &[&str] = &["this ", "that ", "these ", "those ", "the selected", "the above", "the attached"];
    let key = super::compact(subject);
    let lower = subject.to_lowercase();
    let points = subject.split(' ').next().is_some_and(|w| POINTERS.contains(&w))
        || POINTED.contains(&key.as_str())
        || POINTING.iter().any(|p| lower.starts_with(p));
    let project = key.ends_with("프로젝트") || lower.ends_with(" project");
    !key.is_empty() && subject.chars().count() <= 80 && !points && !project
}

fn phrase_ko(line: &str) -> Option<About> {
    // (longest first: of the markers that start at one place the longest is the marker)
    const MARKERS: &[&str] = &[
        "에 대해서는",
        "에 대해서도",
        "에 관해서는",
        "과 관련해서",
        "와 관련해서",
        "과 관련하여",
        "와 관련하여",
        "에 대해서",
        "에 대하여",
        "에 관해서",
        "에 관하여",
        "에대해서",
        "에대하여",
        "에관해서",
        "에관하여",
        "과 관련해",
        "와 관련해",
        "과 관련된",
        "와 관련된",
        "과 관련한",
        "와 관련한",
        " 관련해서",
        " 관련하여",
        " 관련 내용",
        " 관련 사항",
        " 관련 부분",
        "에 대해",
        "에 대한",
        "에 관해",
        "에 관한",
        "에대해",
        "에대한",
        "에관해",
        "에관한",
        " 관련해",
        " 관련된",
        " 관련한",
        " 관련",
    ];
    let (at, marker) = MARKERS
        .iter()
        .filter_map(|m| {
            line.match_indices(m)
                // ("관련" as a word of its own: not "관련자", "관련성")
                .find(|(i, _)| *m != " 관련" || line[i + m.len()..].chars().next().is_none_or(char::is_whitespace))
                .map(|(i, _)| (i, *m))
        })
        .min_by_key(|(i, m)| (*i, std::cmp::Reverse(m.len())))?;
    let before = &line[..at];
    let after = &line[at + marker.len()..];
    // Where to look comes before the subject: "… 프로젝트에서 ", "계약서 중에서 ", "… 프로젝트의 ".
    const PLACES: &[&str] = &["에서는 ", "에서 ", "에 있는 ", "안의 ", "내의 ", "프로젝트의 ", "문서의 ", "자료의 "];
    let cut = PLACES.iter().filter_map(|p| before.rfind(p).map(|i| i + p.len())).max().unwrap_or(0);
    let (mut place, mut subject) = (before[..cut].to_string(), before[cut..].trim());
    // Words in front of the subject that are not part of it: "그리고", "먼저", "프로젝트별로".
    while let Some((first, tail)) = subject.split_once(' ') {
        // (how to say it — "자세히", "짧게" — is not what it is about either)
        const FILLERS: &[&str] = &[
            "그리고",
            "그럼",
            "그러면",
            "혹시",
            "먼저",
            "우선",
            "좀",
            "또",
            "자세히",
            "자세하게",
            "상세히",
            "상세하게",
            "간단히",
            "간단하게",
            "간략히",
            "간략하게",
            "짧게",
            "길게",
        ];
        let filler = FILLERS.contains(&first) || first.ends_with("별로");
        if !filler {
            break;
        }
        place.push_str(first);
        place.push(' ');
        subject = tail.trim();
    }
    let subject = subject.trim_matches(|c: char| c.is_whitespace() || "\"'“”‘’「」『』()[]".contains(c));
    let rest = format!("{} {}", place.trim(), after.trim()).trim().to_string();
    Some(About { subject: subject.to_string(), rest })
}

fn phrase_en(line: &str) -> Option<About> {
    let lower = line.to_lowercase();
    let (at, marker) = [" about ", " regarding ", " concerning "]
        .iter()
        .filter_map(|m| lower.find(m).map(|i| (i, *m)))
        .min_by_key(|(i, _)| *i)?;
    // (lower-casing ASCII letters keeps the byte offsets; a sentence with other cased letters is left alone)
    if lower.len() != line.len() {
        return None;
    }
    let tail = &line[at + marker.len()..];
    let tail_lower = &lower[at + marker.len()..];
    let end = [" in the ", " in ", " from the ", " from ", " within "]
        .iter()
        .filter_map(|p| tail_lower.find(p))
        .min()
        .unwrap_or(tail.len());
    let subject = tail[..end].trim().trim_end_matches(['.', '?', '!', ',']).trim_matches(['"', '\'']).trim();
    let rest = format!("{} {}", line[..at].trim(), tail[end..].trim()).trim().to_string();
    Some(About { subject: subject.to_string(), rest })
}

/// Whether `subject` is nothing but the name of a document type ("계약서", "견적서들", "계약서 내용"):
/// "계약서에 대해 요약해줘" asks for the contracts to be summarised, not for passages about the word.
fn is_type_name(subject: &str, ctx: &super::PlanContext) -> bool {
    let key = super::compact(subject);
    let key = key.strip_prefix("모든").unwrap_or(&key);
    let key = ["들", "문서", "내용", "전체", "전부"].iter().find_map(|w| key.strip_suffix(w)).unwrap_or(key);
    !key.is_empty()
        && ctx.doc_types.iter().any(|d| {
            let aliases = crate::pipeline::schema::doc_type_aliases(&d.slug);
            [d.label_ko.as_str(), d.label_en.as_str(), d.slug.as_str()]
                .into_iter()
                .chain(aliases.iter().copied())
                .any(|label| super::compact(label) == key)
        })
}

/// The project `subject` is the name of: "AAA", "AAA 프로젝트", "the AAA project".
fn project_of<'a>(subject: &str, ctx: &'a super::PlanContext) -> Option<&'a super::ProjectSummary> {
    let bare = |s: &str| {
        let key = super::compact(s);
        let cut = key.strip_suffix("프로젝트").or_else(|| key.strip_suffix("project")).unwrap_or(&key);
        if cut.is_empty() {
            key
        } else {
            cut.to_string()
        }
    };
    let said = bare(subject);
    let said_without_article = subject.to_lowercase().strip_prefix("the ").map(bare);
    ctx.projects.iter().find(|p| {
        let name = bare(&p.name);
        name.chars().count() >= 2 && (name == said || said_without_article.as_deref() == Some(name.as_str()))
    })
}

/// Whether the words outside the phrase that names the subject ask for a summary to be made: "요약해줘",
/// "요약 좀", "… 관련 요약". A summary that already exists is not one to make — "지난 회의에 대한 요약본
/// 찾아줘", "…에 관한 요약 보고서 열어줘" — and "요약에 대해 알려줘" asks none.
fn asks_summary(rest: &str) -> bool {
    const ASKS: &[&str] = &[
        "요약해",
        "요약하",
        "요약좀",
        "요약부탁",
        "요약을해",
        "요약을부탁",
        "요약정리",
        "요약바",
        "요약가능",
        "요약만",
        "요약도",
    ];
    let said = super::compact(rest);
    ASKS.iter().any(|a| said.contains(a))
        || said.ends_with("요약")
        || said.ends_with("요약을")
        || rest.to_lowercase().contains("summar")
}

/// The project a subject stands for: the subject is its name, and the rest of the sentence does not
/// already name that project as the place to look ("AAA 프로젝트에서 AAA에 대해 요약해줘" asks about
/// AAA inside the project — a subject like any other).
fn subject_project<'a>(a: &About, ctx: &'a super::PlanContext) -> Option<&'a super::ProjectSummary> {
    let project = project_of(&a.subject, ctx)?;
    let place = super::compact(&a.rest);
    (!place.contains(super::compact(&project.name).as_str())).then_some(project)
}

/// The subject of a summary the sentence asks for: "AA에 대해서 요약해줘" → `AA`. The request to
/// summarise stands outside the phrase that names the subject, and the subject is not just a set of
/// documents — a kind of document ("견적서에 대해 요약해줘": the estimates) or a project by its name
/// ("AAA에 대해서 요약해줘" with a project AAA: its documents, `summary_project`).
pub fn summary_topic(text: &str, ctx: &super::PlanContext) -> Option<About> {
    let a = about(text)?;
    (asks_summary(&a.rest) && !is_type_name(&a.subject, ctx) && subject_project(&a, ctx).is_none()).then_some(a)
}

/// The project a sentence asks a summary of by naming it as the subject — "AAA에 대해서 요약해줘",
/// "AAA 프로젝트에 관해 요약해줘" — and the rest of the sentence.
pub fn summary_project<'a>(text: &str, ctx: &'a super::PlanContext) -> Option<(&'a super::ProjectSummary, String)> {
    let a = about_phrase(text)?;
    if !asks_summary(&a.rest) {
        return None;
    }
    subject_project(&a, ctx).map(|p| (p, a.rest))
}

/// Whether a subject the model wrote is one the user's words carry: every word of it stands in
/// `said` — the subject of the sentence when it names one, else the sentence ("AA에 대해서 요약해줘"
/// came back with the topic "AAA", the name of a project in the context — eval run of 2026-11-19) —
/// and it names something to look up.
fn said_subject(topic: &str, said: &str, ctx: &super::PlanContext) -> bool {
    let sentence = super::compact(said);
    let words: Vec<String> =
        topic.split(|c: char| !c.is_alphanumeric()).map(super::compact).filter(|w| !w.is_empty()).collect();
    !words.is_empty()
        && words.iter().all(|w| sentence.contains(w.as_str()))
        && !words.iter().any(|w| w.contains("요약") || w.contains("summar"))
        && names_a_subject(topic)
        && !is_type_name(topic, ctx)
}

/// The subject a summary ends with: the one the model wrote when the user's words carry it, else the
/// subject of the sentence, else none (a sentence that names a project as the subject asks for the
/// project's documents — `summary_project`).
fn settled_topic(text: &str, written: Option<&str>, ctx: &super::PlanContext) -> Option<String> {
    if summary_project(text, ctx).is_some() {
        return None;
    }
    let said = super::rules::user_words(text);
    let about = summary_topic(text, ctx);
    let written = written.map(|w| w.split_whitespace().collect::<Vec<_>>().join(" "));
    let own = written.filter(|w| match &about {
        // (the model's subject may be a part of the sentence's: "3월 회의록 보안에 대해" → "보안")
        Some(a) => said_subject(w, &a.subject, ctx),
        // No phrase names a subject: the name of a project is where to look, not what to look up.
        None => said_subject(w, &said, ctx) && project_of(w, ctx).is_none(),
    });
    match own {
        Some(own) => Some(own),
        None => about.map(|a| a.subject),
    }
}

/// Before the validator (1.0.2): the subject of a summary is one the user's words name. A topic the
/// model added on its own — "견적서에 대해 요약해줘" came back as `topic: "견적서"`, "한빛 납품에 대해서
/// 요약해줘" as `topic: "한빛 납품"`, the name of the project (eval run of 2026-11-19) — is taken out
/// before the validator makes the plan the single summary a topic asks for.
pub fn unsaid_topic(text: &str, plan: &mut Plan, ctx: &super::PlanContext) {
    for op in &mut plan.operations {
        if let Op::Summarize { topic, .. } = op {
            if topic.is_some() {
                *topic = settled_topic(text, topic.as_deref(), ctx);
            }
        }
    }
}

/// The user's words without the subject of the summary the plan makes: what is left says where to
/// look ("보안 요구사항에 대해 요약해줘" names no document type — "요구사항" belongs to the subject).
fn scope_words(text: &str, plan: &Plan) -> String {
    let said = super::rules::user_words(text);
    let [Op::Summarize { topic: Some(topic), .. }] = plan.operations.as_slice() else { return said };
    let line: String = said.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(at) = line.find(topic.as_str()) {
        return format!("{} {}", &line[..at], &line[at + topic.len()..]);
    }
    // (a topic in the model's own spelling: its words go one by one)
    let mut rest = line;
    for word in topic.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        if let Some(at) = rest.find(word) {
            rest.replace_range(at..at + word.len(), " ");
        }
    }
    rest
}

/// 1.0.2 — "AA에 대해서 요약해줘" (user report 2026-11-19: "요약만 하고 AA에 대한 내용을 수집하지 않음").
/// Gemma 4 E4B plans the sentence as a plain `summarize` (eval run of 2026-11-19: six of six): every
/// document in scope is summarised — all of the library when no scope is set — and none of it is
/// about AA. When the user's words name the subject of a summary, a plan that only reads becomes a
/// summary of that subject (`summarize.topic`): the passages about it are looked up in the scope and
/// summarised. The model's own answer, search, explanation or question back for such a sentence
/// becomes the same step — the sentence says what is wanted, and where to look needs no asking.
///
/// A subject the model wrote itself stays when the user's words carry it (`said_subject`); one they
/// do not carry gives way to the subject of the sentence, or is dropped when the sentence names
/// none ("이 문서 요약해줘" is a summary of the document whatever topic came with it).
///
/// A subject that is the name of a project means its documents ("AAA에 대해서 요약해줘", project AAA):
/// one summary of the project, not one per project of the library (the model's `group_by: project`
/// with an empty scope in the same run).
///
/// What the scope says about where to look stays, except what was taken from the subject itself: a
/// file-name keyword that is the subject, and a document type that only the subject's words name.
/// A sentence that points at what is open or selected ("이 문서에서 …") is about that document.
pub fn summary_about(text: &str, plan: &mut Plan, ctx: &super::PlanContext, ko: bool) -> Option<PlanIssue> {
    let (length, written) = match plan.operations.as_slice() {
        [Op::Summarize { length, topic, .. }] => (*length, topic.clone()),
        [Op::Answer { reply: None, .. }] | [Op::Search { .. }] | [Op::Explain { .. }] | [Op::Clarify { .. }] | [] => {
            (None, None)
        }
        _ => return None,
    };
    if plan.operations.is_empty() && plan.clarification_question.is_none() {
        return None;
    }
    let summarize = |plan: &mut Plan, topic: Option<String>| {
        plan.operations =
            vec![Op::Summarize { target: Some(super::SummarizeTarget::Set), length, group_by: None, topic }];
        plan.intent = super::OpKind::Summarize;
        plan.clarification_question = None;
        plan.requires_confirmation = false;
        plan.confidence = plan.confidence.max(super::MIN_CONFIDENCE);
    };
    // The subject is a project: its documents are what is summarised.
    if let Some((project, _)) = summary_project(text, ctx) {
        // (its name is not something to look up in its documents)
        for op in &mut plan.operations {
            if let Op::Summarize { topic, .. } = op {
                *topic = None;
            }
        }
        let named_documents = !plan.scope.doc_ids.is_empty() || plan.scope.folder.is_some();
        let as_asked = matches!(
            plan.operations.as_slice(),
            [Op::Summarize { topic: None, group_by: None | Some(super::SummaryGroup::None), .. }]
        ) && plan.scope.filter.project_id.as_deref() == Some(project.id.as_str());
        if named_documents || as_asked {
            return None;
        }
        summarize(plan, None);
        plan.scope.filter.project_id = Some(project.id.clone());
        // (the project's name is not a word to look for in file names)
        if plan.scope.filter.keyword.as_deref().is_some_and(|k| project_of(k, ctx).is_some()) {
            plan.scope.filter.keyword = None;
        }
        plan.summary = Some(if ko {
            format!("'{}' 프로젝트의 문서를 요약합니다", project.name)
        } else {
            format!("Summarise the documents of the project '{}'", project.name)
        });
        super::validate::project_reference(plan);
        return Some(PlanIssue::warn(
            "scope.filter.project_id",
            "scope_filled",
            if ko {
                format!("문장의 '{}' 프로젝트를 대상으로 합니다", project.name)
            } else {
                format!("the project '{}' from the sentence is the scope", project.name)
            },
        ));
    }
    let Some(subject) = settled_topic(text, written.as_deref(), ctx) else {
        // A topic nobody asked for: the plain summary the sentence asks.
        for op in &mut plan.operations {
            if let Op::Summarize { topic, .. } = op {
                *topic = None;
            }
        }
        return None;
    };
    let changed = written.as_deref() != Some(subject.as_str());
    if changed {
        summarize(plan, Some(subject.clone()));
    }
    // (whoever wrote the plan, the card says the same)
    plan.summary = Some(if ko {
        format!("'{subject}'에 관한 내용을 문서에서 찾아 요약합니다")
    } else {
        format!("Summarise what the documents say about '{subject}'")
    });
    // Where to look: the words outside the subject.
    let rest = scope_words(text, plan);
    let f = &mut plan.scope.filter;
    if f.keyword.as_deref().is_some_and(|k| {
        let (k, s) = (super::compact(k), super::compact(&subject));
        !k.is_empty() && (s.contains(&k) || k.contains(&s))
    }) {
        f.keyword = None;
    }
    if let Some(written) = f.doc_type.clone() {
        let named = super::rules::doc_types_named(&rest, ctx);
        if !named.iter().any(|d| d.slug == written) {
            f.doc_type = None;
        }
    }
    if plan.scope.is_empty() {
        plan.scope.doc_ids = super::rules::pointed_at(&rest, ctx);
    }
    // "… 자세히 요약해줘": how long, when the plan does not say (the model left it out — same run).
    if let [Op::Summarize { length: length @ None, .. }] = plan.operations.as_mut_slice() {
        *length = super::rules::length_said(&rest);
    }
    changed.then(|| {
        PlanIssue::warn(
            "operations[0].topic",
            "summary_topic",
            if ko {
                format!("문장의 '{subject}'에 관한 내용만 모아 요약합니다")
            } else {
                format!("only what the documents say about '{subject}' is summarised")
            },
        )
    })
}

/// 0.8.2 — the real model (Gemma 4 E4B) plans "X가 무엇인지 설명해줘" as `explain` and writes into
/// `selection` whatever comes to it: the term ("AAA"), the whole sentence, or a note to itself
/// ("… question 파라미터를 사용합니다. scope는 project로 설정합니다."). `explain` explains a piece of
/// text the user pointed at, and only a part of the user's own words is one; anything else is the
/// question itself and becomes a grounded answer to the user's sentence.
pub fn explain_what_was_said(text: &str, plan: &mut Plan) -> bool {
    if carried_text(text).is_some() {
        return false;
    }
    let said = super::rules::user_words(text);
    let key = super::compact(&said);
    let mut changed = false;
    for op in &mut plan.operations {
        let Op::Explain { selection, .. } = op else { continue };
        let part = selection.as_deref().map(super::compact).filter(|s| !s.is_empty());
        let pointed_at = part
            .as_ref()
            .is_some_and(|s| key.contains(s.as_str()) && s.chars().count() * 10 <= key.chars().count() * 7);
        if pointed_at {
            continue;
        }
        *op = Op::Answer { question: said.trim().to_string(), cite: None, knowledge: None, mode: None, reply: None };
        changed = true;
    }
    if changed && !plan.operations.iter().any(|o| o.kind() == plan.intent) {
        plan.recompute_intent();
    }
    changed
}

/// 1.0.0 §4-1 — with `explain` out of the model's op table, Gemma 4 E4B writes "AAA가 무엇인지 설명해줘" as
/// an `answer` that looks nothing up: `knowledge: false` (meant for a text the user pasted) or
/// `mode: general` (eval run of 2026-11-16: three of the five user sentences of 0.8.2). A question
/// that names a project of the workspace and brings no text of its own is about that project's
/// documents, whatever the model called it: the answer becomes a grounded one (`where_to_look` then
/// scopes it to the project). So is a question that points at what is open or selected — "이 조항이
/// 무슨 뜻인지 설명해줘" came out as `knowledge: false` + `mode: general` with an empty scope (same
/// run, ko-55): it is answered from that document. Any other question keeps the model's reading.
pub fn document_question(text: &str, plan: &mut Plan, ctx: &super::PlanContext) -> bool {
    // "instruction: text" keeps the model's reading however short the text is (`carried_text` asks
    // for a text of some length before it takes the plan over).
    let command = super::rules::command_of(text).trim();
    let brings_a_text = command.contains([':', '：', '\n', '"', '“', '「', '『']) && has_rewrite_verb(command);
    if brings_a_text {
        return false;
    }
    let at_hand = super::rules::pointed_at(text, ctx);
    let place = !at_hand.is_empty() || project_named(text, ctx).is_some();
    // 1.0.2: "AA에 대해 알려줘" names what to look up. With `knowledge: false` — meant for a text the
    // user brought — nothing would be looked up at all; a `general` answer does look, and stays.
    if !place && about(text).is_none() {
        return false;
    }
    let mut changed = false;
    for op in &mut plan.operations {
        let Op::Answer { knowledge, mode, reply, .. } = op else { continue };
        let looks_nothing_up = *knowledge == Some(false) || (place && *mode == Some(super::AnswerMode::General));
        if reply.is_none() && *mode != Some(super::AnswerMode::Chat) && looks_nothing_up {
            *knowledge = None;
            *mode = None;
            changed = true;
        }
    }
    // The model left the scope out with the lookup: the document the words point at.
    if changed && !at_hand.is_empty() && plan.scope.is_empty() {
        plan.scope.doc_ids = at_hand;
    }
    changed
}

/// 1.0.0 §2-1 — a text replacement as Gemma 4 E4B planned it (eval run of 2026-11-16): the date
/// between the quotes of "원본에서 '11월 30일'을 '12월 31일'로 바꿔줘" came back as the scope's date
/// range, and "'㈜한빛'을 '(주)한빛'으로 바꿔줘" with no scope at all although a document was open (the
/// app then asked which documents). The texts between the quotes are what changes, never where to
/// look: a date range the words outside them do not carry is dropped, so is a keyword that is the
/// text itself, and a replacement that then names no documents works on what the phrase rules would
/// take — the selection, or else the open document (`rules::edit_scope`). The step is confirmed
/// either way.
pub fn edit_target(text: &str, plan: &mut Plan, ctx: &super::PlanContext) -> bool {
    if plan.is_clarification() {
        return false;
    }
    let texts: Vec<String> = plan
        .operations
        .iter()
        .filter_map(|op| match op {
            Op::EditText { find, replace, .. } => Some([super::compact(find), super::compact(replace)]),
            _ => None,
        })
        .flatten()
        .collect();
    if texts.is_empty() {
        return false;
    }
    let mut changed = false;
    let filter = &mut plan.scope.filter;
    if (filter.date_from.is_some() || filter.date_to.is_some()) && !super::rules::dated_outside_quotes(text, ctx.today)
    {
        filter.date_from = None;
        filter.date_to = None;
        changed = true;
    }
    if filter.keyword.as_deref().map(super::compact).is_some_and(|k| texts.contains(&k)) {
        filter.keyword = None;
        changed = true;
    }
    if plan.scope.is_empty() {
        let scope = super::rules::edit_scope(text, ctx);
        if !scope.is_empty() {
            plan.scope = scope;
            changed = true;
        }
    }
    changed
}

/// 1.0.0, user decision 2026-11-16 (plan 4-3) — "참고 자료로 지정" names no place, so the documents
/// go to the "프로젝트에 넣기" dialog and the user picks the projects there (`set_role` to
/// `knowledge`). Gemma 4 E4B answered "선택한 문서들 참고자료로 지정" with `manage_reference add` and a
/// project the sentence never names (eval run of the same day): the documents would have gone into
/// a project of the model's choosing, behind one confirmation. A plan that puts documents into
/// reference material for a sentence that only marks them, at a place the user's words do not
/// name, becomes the step that opens the dialog. A place the words do name — a project, a group,
/// the common material — is the user's and stays. The plan of that step, whoever wrote it, says
/// what it does and is not confirmed twice: nothing changes before the dialog is.
pub fn reference_place(text: &str, plan: &mut Plan, ctx: &super::PlanContext, ko: bool) -> bool {
    let opens = matches!(plan.operations.as_slice(), [Op::SetRole { role: super::RoleParam::Knowledge }]);
    let puts_in = matches!(
        plan.operations.as_slice(),
        [Op::ManageReference { action: super::ReferenceAction::Add, .. }]
            | [Op::AddToProject { as_reference: true, .. }]
    );
    let repaired = puts_in && super::rules::marks_as_reference(text) && !names_a_place(text, ctx);
    if repaired {
        plan.operations = vec![Op::SetRole { role: super::RoleParam::Knowledge }];
        plan.intent = OpKind::SetRole;
    }
    if opens || repaired {
        plan.summary = Some(super::rules::reference_dialog_summary(ko).to_string());
        plan.requires_confirmation = false;
    }
    repaired
}

/// Whether the user's words say where reference material goes: a project (the word, or the name of
/// one), a group, the common material.
fn names_a_place(text: &str, ctx: &super::PlanContext) -> bool {
    const PLACE: [&str; 9] = ["프로젝트", "project", "묶음", "그룹", "컬렉션", "collection", "group", "공통", "common"];
    let said = super::compact(&super::rules::user_words(text));
    PLACE.iter().any(|w| said.contains(w))
        || ctx.projects.iter().any(|p| {
            let name = super::compact(&p.name);
            !name.is_empty() && said.contains(&name)
        })
}

/// The words around a phrase of the sentence, as the user wrote them: the word before it, the
/// particle attached to it ("참고자료**도**") and the word after it ("참고자료 **포함해서**"), all in
/// lower case. `start..end` is the phrase in the compact form of `text` ([`super::compact`]).
fn around(text: &str, start: usize, end: usize) -> (String, String, String) {
    // Compact offsets → offsets in the text (a compact character is one alphanumeric character).
    let (mut seen, mut from, mut to) = (0usize, None, text.len());
    for (i, c) in text.char_indices() {
        if seen >= end {
            to = i;
            break;
        }
        if c.is_alphanumeric() {
            if seen >= start && from.is_none() {
                from = Some(i);
            }
            seen += c.to_lowercase().map(char::len_utf8).sum::<usize>();
        }
    }
    let word = |t: &str| t.chars().take_while(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let rest = &text[to..];
    let attached = word(rest);
    let next = word(rest[attached.len().min(rest.len())..].trim_start_matches(|c: char| !c.is_alphanumeric()));
    // The word before the phrase; an article is not it ("including the reference documents").
    let last_word = |head: &str| -> (String, usize) {
        let head = head.trim_end_matches(|c: char| !c.is_alphanumeric());
        let word: String =
            head.chars().rev().take_while(|c| c.is_alphanumeric()).collect::<Vec<_>>().into_iter().rev().collect();
        (word.to_lowercase(), head.len() - word.len())
    };
    let (mut before, at) = last_word(&text[..from.unwrap_or(0)]);
    if ["the", "a", "an", "all"].contains(&before.as_str()) {
        before = last_word(&text[..at]).0;
    }
    (before, attached, next)
}

/// The kind of document the user's words name (0.8.1): "참고 자료에서 …" → reference material,
/// "업무 문서만 …" → work documents. Both named: the one that is not ruled out ("참고 자료 말고").
///
/// 1.0.1: a kind that is named as one more thing to cover — "참고자료**도**", "참고자료**까지**",
/// "참고자료**와** 문서", "참고자료 **포함해서**" — is not a restriction. The sentence used to be read
/// as "참고 자료만", and a summary of a project without reference material failed with no document.
/// What a sentence says about the kind of document to look at.
#[derive(Debug, Clone, Copy, PartialEq)]
enum KindSaid {
    /// One kind, as the place to look: "참고 자료에서 …", "업무 문서만 …".
    Only(super::RoleParam),
    /// A kind as one more thing to cover, or both kinds: "참고자료도 포함해서 …".
    Also,
    Nothing,
}

fn kind_said(text: &str) -> KindSaid {
    const REFERENCE: &[&str] =
        &["참고자료", "참고문서", "레퍼런스", "referencematerial", "referencedocument", "references"];
    const WORK: &[&str] = &["업무문서", "작업문서", "workdocument"];
    const NOT: &[&str] = &["말고", "빼고", "제외", "아니라", "except", "notthe", "butnot"];
    // Particles that add ("도", "까지") or join ("와 문서"), the words that say "as well" after the
    // kind, and the English ones that come before it ("including reference material").
    const JOINED: &[&str] = &["도", "까지", "와", "과", "랑", "이랑", "하고", "및"];
    const AS_WELL: &[&str] = &["포함", "및", "그리고", "같이", "함께", "and", "plus", "too", "aswell"];
    const LEADING: &[&str] = &["including", "with", "plus", "and", "also"];
    let words = super::rules::user_words(text);
    let said = super::compact(&words);
    // The first place a kind is named: `(start, end)` in the compact text.
    let find = |words: &[&str]| words.iter().filter_map(|w| said.find(w).map(|at| (at, at + w.len()))).min();
    let ruled_out = |end: usize| {
        let after: String = said[end..].chars().take(6).collect();
        NOT.iter().any(|n| after.contains(n))
    };
    let added = |(start, end): (usize, usize)| {
        let (before, attached, next) = around(&words, start, end);
        // (an English plural: "reference documents and …")
        let english = said[..end].ends_with(|c: char| c.is_ascii_alphabetic());
        let attached = if english { attached.trim_start_matches('s') } else { attached.as_str() };
        JOINED.contains(&attached)
            || attached.starts_with("포함")
            || AS_WELL.iter().any(|w| next.starts_with(w))
            || LEADING.contains(&before.as_str())
    };
    let only = |kind: super::RoleParam, place: (usize, usize)| match (ruled_out(place.1), added(place)) {
        (true, _) => KindSaid::Nothing,
        (false, true) => KindSaid::Also,
        (false, false) => KindSaid::Only(kind),
    };
    match (find(REFERENCE), find(WORK)) {
        (Some(r), None) => only(super::RoleParam::Knowledge, r),
        (None, Some(w)) => only(super::RoleParam::Work, w),
        (Some(r), Some(w)) => match (ruled_out(r.1), ruled_out(w.1)) {
            (true, false) => KindSaid::Only(super::RoleParam::Work),
            (false, true) => KindSaid::Only(super::RoleParam::Knowledge),
            (false, false) => KindSaid::Also,
            (true, true) => KindSaid::Nothing,
        },
        (None, None) => KindSaid::Nothing,
    }
}

/// A search query without the words that named the kind of document to look at ("참고 자료에서 납품
/// 기한" → "납품 기한"); with `also`, without the words that added it ("참고자료 포함해서 …") as well.
/// A query that would be left empty stays as it was.
fn query_without_kind(query: &str, also: bool) -> String {
    let cleaned = [
        "참고 자료에서",
        "참고자료에서",
        "참고 자료만",
        "참고자료만",
        "참고 자료",
        "참고자료",
        "업무 문서에서",
        "업무문서에서",
        "업무 문서만",
        "업무 문서",
        "업무문서",
    ]
    .iter()
    .fold(query.to_string(), |q, w| q.replace(w, " "));
    // (the phrase rules may have taken "자료" / "문서" out of the query already)
    let cleaned = cleaned
        .split_whitespace()
        .filter(|w| !["참고", "업무", "에서", "만", "참고에서", "업무에서"].contains(w))
        .filter(|w| {
            !also
                || !(w.starts_with("포함")
                    || ["도", "까지", "와", "과", "랑", "같이", "함께", "및", "참고도", "참고까지", "참고와", "참고랑"]
                        .contains(w))
        })
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        query.to_string()
    } else {
        cleaned
    }
}

/// The one project the user's words name (a name that is part of a longer named one counts for the longer).
fn project_named<'a>(text: &str, ctx: &'a super::PlanContext) -> Option<&'a super::ProjectSummary> {
    let said = super::compact(&super::rules::user_words(text));
    let hits: Vec<(&super::ProjectSummary, String)> = ctx
        .projects
        .iter()
        .map(|p| (p, super::compact(&p.name)))
        .filter(|(_, name)| name.chars().count() >= 2 && said.contains(name.as_str()))
        .collect();
    let keys: Vec<&String> = hits.iter().map(|(_, k)| k).collect();
    let mut own = hits.iter().filter(|(_, k)| !keys.iter().any(|o| *o != k && o.contains(k.as_str())));
    match (own.next(), own.next()) {
        (Some((p, _)), None) => Some(p),
        _ => None,
    }
}

/// Ops that only read the documents in scope: the scope may be narrowed for them from the user's words.
fn reads_documents(op: &Op) -> bool {
    super::validate::only_reads(op)
}

/// 0.8.1: what the user's words say about where to look goes into a plan that left it out — the kind
/// of document ("참고 자료에서 …"; the plan grammar had no place for it before), the project a sentence
/// names ("한빛 프로젝트에서 …"; the model's scope cannot name a project), and "X 프로젝트 열어줘", which
/// the model could only express as a folder or the library screen. Applied to model and rule plans.
pub fn where_to_look(text: &str, plan: &mut Plan, ctx: &super::PlanContext, ko: bool) -> Vec<PlanIssue> {
    let mut notes = Vec::new();
    if plan.is_clarification() || plan.operations.is_empty() {
        return notes;
    }
    let said = super::compact(&super::rules::user_words(text));
    // "X 프로젝트 열어줘": the project's page, not a folder or the library.
    if let [Op::Open { target, reference }] = plan.operations.as_mut_slice() {
        let wants_folder = ["폴더", "folder"].iter().any(|w| said.contains(w));
        if !matches!(target, super::OpenTarget::Project | super::OpenTarget::Page) && !wants_folder {
            if let Some(p) = project_named(text, ctx) {
                *target = super::OpenTarget::Project;
                *reference = p.id.clone();
                plan.summary = Some(if ko {
                    format!("'{}' 프로젝트를 엽니다", p.name)
                } else {
                    format!("Open the project '{}'", p.name)
                });
            }
        }
        return notes;
    }
    if !plan.operations.iter().all(reads_documents) {
        return notes;
    }
    let named_documents = !plan.scope.doc_ids.is_empty() || plan.scope.folder.is_some();
    if plan.scope.filter.role.is_none() {
        let said_kind = kind_said(text);
        if said_kind == KindSaid::Also {
            // "참고자료 포함해서 납품 기한 찾아줘": both kinds are looked at; the words are not the query.
            for op in &mut plan.operations {
                if let Op::Search { query, .. } = op {
                    *query = query_without_kind(query, true);
                }
            }
        }
        if let KindSaid::Only(kind) = said_kind {
            plan.scope.filter.role = Some(kind);
            // (the words that named the kind are not what to search for)
            for op in &mut plan.operations {
                if let Op::Search { query, .. } = op {
                    *query = query_without_kind(query, false);
                }
            }
            let label = match (kind, ko) {
                (super::RoleParam::Knowledge, true) => "참고 자료",
                (super::RoleParam::Knowledge, false) => "reference material",
                (_, true) => "업무 문서",
                (_, false) => "work documents",
            };
            notes.push(PlanIssue::warn(
                "scope.filter.role",
                "scope_filled",
                if ko {
                    format!("문장대로 {label}만 대상으로 합니다")
                } else {
                    format!("only {label}, as the sentence says")
                },
            ));
        }
    }
    // 1.0.2 — "AAA 프로젝트 요약해줘" came back from Gemma 4 E4B as `group_by: project` with an empty
    // scope (eval run of 2026-11-19): one summary per project of the library. A sentence that names
    // one project and asks no split by project means that project — one result for it.
    if super::validate::per_project(plan)
        && plan.scope.filter.project_id.is_none()
        && !named_documents
        && split_of(&super::rules::user_words(text)) != Some(SplitBy::Project)
        && project_named(text, ctx).is_some()
    {
        for op in &mut plan.operations {
            match op {
                Op::Summarize { target, group_by, .. } if *group_by == Some(SummaryGroup::Project) => {
                    *group_by = None;
                    *target = Some(super::SummarizeTarget::Set);
                }
                Op::Search { group_by, .. } if *group_by == Some(SummaryGroup::Project) => *group_by = None,
                Op::Aggregate { group_by, .. } if *group_by == Some(GroupBy::Project) => *group_by = None,
                _ => {}
            }
        }
    }
    if plan.scope.filter.project_id.is_none() && !named_documents && !super::validate::per_project(plan) {
        if let Some(p) = project_named(text, ctx) {
            plan.scope.filter.project_id = Some(p.id.clone());
            notes.push(PlanIssue::warn(
                "scope.filter.project_id",
                "scope_filled",
                if ko {
                    format!("문장의 '{}' 프로젝트를 대상으로 합니다", p.name)
                } else {
                    format!("the project '{}' from the sentence is the scope", p.name)
                },
            ));
        }
    }
    // (a project or a kind read from the sentence changes what the project scope covers)
    super::validate::project_reference(plan);
    notes
}

/// Checks a model plan against the sentence it was made from (0.8.0 plan §1.3 / §1.5). Anything
/// found turns the plan into one the user confirms, with a note for the card; the model's own
/// `confidence` plays no part.
pub fn contract_checks(text: &str, plan: &mut Plan, ko: bool) -> Vec<PlanIssue> {
    // Only what the user wrote grounds a plan: the assistant's question inside an ask-back text
    // names settings and ranges of its own.
    let said = super::rules::user_words(text);
    let text = said.as_str();
    let mut notes = Vec::new();
    if plan.is_clarification() {
        return notes;
    }
    // 1. The model said it left something out.
    let unmet = plan.unmet.as_deref().map(str::trim).unwrap_or_default().to_string();
    if unmet.is_empty() {
        plan.unmet = None;
    } else {
        plan.requires_confirmation = true;
        notes.push(PlanIssue::warn(
            "unmet",
            "unmet_request",
            if ko {
                format!("요청 중 반영하지 못한 부분: {unmet}")
            } else {
                format!("not covered by this plan: {unmet}")
            },
        ));
        plan.unmet = Some(unmet);
    }
    // 2. The sentence asks for one result per group and the plan makes one for everything.
    notes.extend(split_check(text, plan, ko));
    // 3. A settings change the sentence does not name.
    //    … or names in a sentence that is a question or a report, not a request ("DPI 200으로 바꿨어"
    //    came back from the real model as a change to 200 and ran).
    let not_a_request = fastpath::reads_as_question(text);
    let changes: Vec<(usize, String, bool)> = plan
        .operations
        .iter()
        .enumerate()
        .filter_map(|(i, op)| setting_change(text, op).map(|(what, grounded)| (i, what, grounded)))
        .collect();
    for (i, what, grounded) in changes {
        if grounded && !not_a_request {
            continue;
        }
        plan.requires_confirmation = true;
        notes.push(PlanIssue::warn(
            format!("operations[{i}]"),
            "ungrounded_change",
            match (grounded, ko) {
                (false, true) => format!("문장에서 이 변경의 근거를 찾지 못했습니다({what}). 맞으면 승인해 주세요"),
                (false, false) => {
                    format!("the sentence does not name this change ({what}); approve it if it is what you meant")
                }
                (true, true) => {
                    format!("문장이 요청이 아니라 질문이나 알림으로 읽힙니다({what}). 바꾸려던 것이 맞으면 승인해 주세요")
                }
                (true, false) => {
                    format!("the sentence reads as a question or a report, not a request ({what}); approve it if you meant the change")
                }
            },
        ));
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::SummarizeTarget;

    const BODY: &str = "금번 과업은 당사가 수행하며 기한은 12월말까지로 하고 산출물은 설계서와 소스코드로 함. 검수는 발주처가 납품 후 2주 내에 완료하는 것으로 한다.";

    fn kb_plan(op: Op) -> Plan {
        Plan {
            intent: OpKind::Summarize,
            scope: Scope::default(),
            operations: vec![op],
            requires_confirmation: true,
            confidence: 0.9,
            clarification_question: None,
            summary: None,
            unmet: None,
        }
    }

    #[test]
    fn a_pasted_body_after_a_rewrite_instruction_is_recognised() {
        assert_eq!(carried_text(&format!("다음 문장을 다듬어 정리해줘: {BODY}")), Some(BODY));
        assert_eq!(carried_text(&format!("교정해줘\n\"{BODY}\"")), Some(BODY));
        assert_eq!(carried_text(&format!("{BODY}\n위 문장을 매끄럽게 다듬어 줘")), Some(BODY));
        assert_eq!(carried_text(&format!("Please polish this: {BODY}")), Some(BODY));
        // No verb, a short body, or a plain question: nothing carried.
        assert_eq!(carried_text(&format!("이 내용 알려줘: {BODY}")), None);
        assert_eq!(carried_text("다듬어줘: 짧은 글"), None);
        assert_eq!(carried_text("이 계약서의 계약기간은?"), None);
        // A colon inside a time is not "instruction: text".
        assert_eq!(carried_text(&format!("회의록 요약해줘 10:30 이후 분량만 {BODY}")), None);
    }

    #[test]
    fn a_library_wide_summary_of_a_pasted_text_becomes_a_text_only_answer() {
        let text = format!("다음 문장을 다듬어 정리해줘: {BODY}");
        let mut plan = kb_plan(Op::Summarize {
            target: Some(SummarizeTarget::EachDoc),
            length: None,
            group_by: None,
            topic: None,
        });
        let note = prefer_text_only_answer(&text, &mut plan, true).expect("re-targeted");
        assert_eq!(note.code, "text_only_answer");
        assert!(!note.blocking);
        assert_eq!(plan.intent, OpKind::Answer);
        assert!(!plan.requires_confirmation);
        assert!(
            matches!(&plan.operations[0], Op::Answer { knowledge: Some(false), cite: Some(false), question, .. } if question == &text)
        );
    }

    #[test]
    fn explicit_scopes_and_non_knowledge_ops_are_left_alone() {
        let text = format!("다음 문장을 다듬어 정리해줘: {BODY}");
        let mut scoped = kb_plan(Op::Summarize { target: None, length: None, group_by: None, topic: None });
        scoped.scope.folder = Some("documents/2026".into());
        assert!(prefer_text_only_answer(&text, &mut scoped, true).is_none());
        let mut validate = kb_plan(Op::Validate {});
        assert!(prefer_text_only_answer(&text, &mut validate, true).is_none());
        let mut plain = kb_plan(Op::Summarize { target: None, length: None, group_by: None, topic: None });
        assert!(prefer_text_only_answer("지난달 계약서 요약해줘", &mut plain, true).is_none());
        // Documents the plan names are a scope too: the "text" is an instruction about them.
        let mut selected = kb_plan(Op::Summarize { target: None, length: None, group_by: None, topic: None });
        selected.scope.doc_ids = vec!["d1".into()];
        assert!(prefer_text_only_answer(&text, &mut selected, true).is_none());
        // A date range the resolver filled from "12월말" inside the body is not an explicit scope.
        let mut dated = kb_plan(Op::Summarize {
            target: Some(SummarizeTarget::EachDoc),
            length: None,
            group_by: None,
            topic: None,
        });
        dated.scope.filter.date_from = Some("2025-12-01".into());
        dated.scope.filter.date_to = Some("2025-12-31".into());
        assert!(prefer_text_only_answer(&text, &mut dated, true).is_some());
        assert!(dated.scope.is_empty());
    }

    fn model_plan(ops: Vec<Op>) -> Plan {
        Plan {
            intent: ops[0].kind(),
            scope: Scope::default(),
            operations: ops,
            requires_confirmation: false,
            confidence: 0.95,
            clarification_question: None,
            summary: Some("s".into()),
            unmet: None,
        }
    }

    fn codes(text: &str, plan: &mut Plan) -> Vec<String> {
        contract_checks(text, plan, true).into_iter().map(|i| i.code).collect()
    }

    /// Plans the real model (Gemma 4 E4B, 0.8.0 measurement) wrote for three golden sentences, replayed.
    #[test]
    fn plans_the_real_model_wrote_are_caught_or_repaired() {
        use serde_json::json;
        // "담당자별" came back grouped by folder and ran: the guess is taken out and the split reported.
        let mut p = model_plan(vec![Op::Summarize {
            target: None,
            length: None,
            group_by: Some(SummaryGroup::Folder),
            topic: None,
        }]);
        assert_eq!(codes("견적서를 담당자별로 요약해줘", &mut p), vec!["grouping_dropped"]);
        assert!(p.requires_confirmation && p.unmet.is_some());
        assert!(matches!(&p.operations[0], Op::Summarize { group_by: None, .. }));
        // A grouping other than the one the sentence names is replaced by the sentence's.
        let mut p = model_plan(vec![Op::Summarize {
            target: None,
            length: None,
            group_by: Some(SummaryGroup::Folder),
            topic: None,
        }]);
        assert!(codes("프로젝트별로 요약해줘", &mut p).is_empty());
        assert!(matches!(&p.operations[0], Op::Summarize { group_by: Some(SummaryGroup::Project), .. }));
        // The split belongs to the filing step: the summary is left alone.
        let mut p = model_plan(vec![
            Op::Move { folder: "documents/{{doc_type}}".into(), group_by: Some(GroupBy::DocType), only_reviewed: None },
            Op::Summarize { target: None, length: None, group_by: Some(SummaryGroup::DocType), topic: None },
        ]);
        assert!(codes("담당자별 폴더로 옮기고 유형별로 요약해줘", &mut p).is_empty());

        // "DPI 200으로 바꿨어" is a report: the change it names waits.
        let dpi = || Op::SetSetting { key: "processing.render_dpi".into(), value: json!(200) };
        let mut p = model_plan(vec![dpi()]);
        assert_eq!(codes("DPI 200으로 바꿨어", &mut p), vec!["ungrounded_change"]);
        assert!(p.requires_confirmation);
        let mut p = model_plan(vec![dpi()]);
        assert_eq!(codes("렌더 DPI가 200인가요?", &mut p), vec!["ungrounded_change"]);
        // The request itself still runs.
        let mut p = model_plan(vec![dpi()]);
        assert!(codes("렌더 DPI를 200으로 바꿔줘", &mut p).is_empty());
        assert!(!p.requires_confirmation);

        // "다음 문장을 다듬어줘: …" came back as a new document drafted from the sentence.
        let draft = |content: &str| Op::DraftDocument {
            form_ref: Some("dev_contract".into()),
            format_description: None,
            content: content.to_string(),
            sources: Vec::new(),
            output: None,
            tone: None,
            length: None,
        };
        let text = format!("다음 문장을 다듬어줘: {BODY}");
        let mut p = model_plan(vec![draft(BODY)]);
        p.requires_confirmation = true;
        assert!(prefer_text_only_answer(&text, &mut p, true).is_some());
        assert!(
            matches!(&p.operations[..], [Op::Answer { knowledge: Some(false), question, .. }] if question == &text)
        );
        assert!(!p.requires_confirmation);
        // An instruction that asks for a document keeps the draft.
        let text = format!("다음 내용을 다듬어서 회의록으로 작성해줘: {BODY}");
        let mut p = model_plan(vec![draft(BODY)]);
        assert!(prefer_text_only_answer(&text, &mut p, true).is_none());
        assert!(matches!(&p.operations[..], [Op::DraftDocument { .. }]));

        // "배율 130으로 바꾸고 견적서 요약해줘": the summary had no scope.
        let ctx = crate::planner::PlanContext {
            doc_types: vec![
                crate::planner::DocTypeSummary {
                    slug: "estimate".into(),
                    label_ko: "견적서".into(),
                    label_en: "Estimate".into(),
                },
                crate::planner::DocTypeSummary {
                    slug: "dev_contract".into(),
                    label_ko: "개발 계약서".into(),
                    label_en: "Development contract".into(),
                },
            ],
            ..Default::default()
        };
        let scale = || Op::SetSetting { key: "ui.scale_percent".into(), value: json!(130) };
        let summarize =
            || Op::Summarize { target: Some(SummarizeTarget::EachDoc), length: None, group_by: None, topic: None };
        let mut p = model_plan(vec![scale(), summarize()]);
        let note = type_filter_repair("배율 130으로 바꾸고 견적서 요약해줘", &mut p, &ctx, true).expect("repaired");
        assert_eq!(note.code, "scope_filled");
        assert_eq!(p.scope.filter.doc_type.as_deref(), Some("estimate"));
        assert!(!p.requires_confirmation);
        // Two types, a scope of the plan's own, or a type that is the op's value: nothing is filled in.
        let mut p = model_plan(vec![summarize()]);
        assert!(type_filter_repair("견적서와 개발 계약서 요약해줘", &mut p, &ctx, true).is_none());
        let mut p = model_plan(vec![summarize()]);
        p.scope.doc_ids = vec!["d1".into()];
        assert!(type_filter_repair("견적서 요약해줘", &mut p, &ctx, true).is_none());
        let mut p = model_plan(vec![Op::SetDocType { doc_type: "estimate".into() }]);
        assert!(type_filter_repair("선택한 문서를 견적서로 분류해줘", &mut p, &ctx, true).is_none());
        assert!(p.scope.is_empty());
        let mut p = model_plan(vec![Op::Answer {
            question: "견적서가 뭐야?".into(),
            cite: None,
            knowledge: None,
            mode: None,
            reply: None,
        }]);
        assert!(type_filter_repair("견적서가 뭐야?", &mut p, &ctx, true).is_none());
    }

    fn topic_ctx() -> crate::planner::PlanContext {
        let ty = |slug: &str, ko: &str, en: &str| crate::planner::DocTypeSummary {
            slug: slug.into(),
            label_ko: ko.into(),
            label_en: en.into(),
        };
        crate::planner::PlanContext {
            projects: vec![
                crate::planner::ProjectSummary {
                    id: "p-hanbit".into(),
                    name: "한빛 납품".into(),
                    status: "active".into(),
                },
                crate::planner::ProjectSummary { id: "p-aaa".into(), name: "AAA".into(), status: "active".into() },
            ],
            doc_types: vec![
                ty("srs", "요구사항 정의서", "Software requirements specification"),
                ty("dev_contract", "소프트웨어 개발 계약서", "Software development contract"),
                ty("acceptance", "검수확인서", "Acceptance certificate"),
                ty("estimate", "견적서", "Estimate"),
            ],
            ..Default::default()
        }
    }

    /// 1.0.2: the subject a sentence names, and the sentence without the phrase that names it.
    #[test]
    fn the_subject_of_a_sentence() {
        let of = |text: &str| about(text).map(|a| (a.subject, a.rest));
        let some = |subject: &str, rest: &str| Some((subject.to_string(), rest.to_string()));
        assert_eq!(of("AA에 대해서 요약해줘"), some("AA", "요약해줘"));
        assert_eq!(of("납품 기한에 대해 요약해줘"), some("납품 기한", "요약해줘"));
        assert_eq!(of("보안 요구사항에 관해서 정리해줘"), some("보안 요구사항", "정리해줘"));
        assert_eq!(of("지체상금에 대한 내용을 요약해줘"), some("지체상금", "내용을 요약해줘"));
        assert_eq!(of("하자보수 관련 내용을 요약해줘"), some("하자보수", "을 요약해줘"));
        assert_eq!(of("보안 관련 요약"), some("보안", "요약"));
        assert_eq!(of("개인정보와 관련된 조항을 알려줘"), some("개인정보", "조항을 알려줘"));
        // Where to look stands in front of the subject and stays in the rest.
        assert_eq!(
            of("한빛 납품 프로젝트에서 검수 기준에 대해 요약해줘"),
            some("검수 기준", "한빛 납품 프로젝트에서 요약해줘")
        );
        assert_eq!(of("AAA 프로젝트의 일정에 대해 알려줘"), some("일정", "AAA 프로젝트의 알려줘"));
        assert_eq!(of("프로젝트별로 지체상금에 대해 요약해줘"), some("지체상금", "프로젝트별로 요약해줘"));
        assert_eq!(of("자세히 계약 금액에 대해 요약해줘"), some("계약 금액", "자세히 요약해줘"));
        assert_eq!(of("계약 금액에 대해 자세히 요약해줘"), some("계약 금액", "자세히 요약해줘"));
        assert_eq!(
            of("summarise what the documents say about late fees."),
            some("late fees", "summarise what the documents say")
        );
        assert_eq!(
            of("Summarize about security requirements in the Hanbit project"),
            some("security requirements", "Summarize in the Hanbit project")
        );
        // Words that point at something are where to look, not a subject; a project named as such too.
        for text in [
            "이 문서에 대해 요약해줘",
            "그것에 대해 더 알려줘",
            "위 내용에 대해 요약해줘",
            "선택한 문서에 대해 요약해줘",
            "이 프로젝트에 대해 요약해줘",
            "AAA 프로젝트에 대해 요약해줘",
            "참고 자료에 대해 요약해줘",
            "문서에 대해 요약해줘",
            "summarise about this document",
            "tell me about it",
            "요약해줘",
            "관련자 명단 요약해줘",
        ] {
            assert_eq!(of(text), None, "{text}");
        }
        // A sentence that brings its own text names no subject to look up.
        let pasted = format!("다음 글을 요약해줘: {}", "계약 해지에 대한 조항은 다음과 같다. ".repeat(6));
        assert_eq!(of(&pasted), None);

        // The subject of a summary: the request to summarise stands outside the phrase, and a kind of
        // document is not a subject.
        let ctx = topic_ctx();
        let topic = |text: &str| summary_topic(text, &ctx).map(|a| a.subject);
        assert_eq!(topic("AA에 대해서 요약해줘").as_deref(), Some("AA"));
        assert_eq!(topic("보안 요구사항에 대해 요약해줘").as_deref(), Some("보안 요구사항"));
        assert_eq!(topic("AA에 대해 알려줘"), None);
        assert_eq!(topic("요약에 대해 알려줘"), None);
        // A summary to make — not one that exists already.
        for text in
            ["AA에 대해 요약 좀 해줘", "AA에 대해서 요약 부탁해", "AA 관련 요약", "AA에 관해 간단히 요약해 주세요"]
        {
            assert_eq!(topic(text).as_deref(), Some("AA"), "{text}");
        }
        for text in ["지난 회의에 대한 요약본 찾아줘", "AA에 관한 요약 보고서 열어줘", "AA에 대해 요약된 문서 보여줘"]
        {
            assert_eq!(topic(text), None, "{text}");
        }
        assert_eq!(topic("견적서에 대해 요약해줘"), None, "the estimates are summarised");
        assert_eq!(topic("계약서들에 대해서 요약해줘"), None);
    }

    /// 1.0.2 — "AA에 대해서 요약해줘 … 요약만 하고 AA에 대한 내용을 수집하지 않음" (user report
    /// 2026-11-19). The six plans Gemma 4 E4B wrote for such sentences (eval run of that day): a
    /// plain `summarize` each time — per document, per project, or of one document type its words
    /// took from the subject — and none of it about the subject.
    #[test]
    fn a_summary_about_a_subject_is_a_summary_of_that_subject() {
        let ctx = topic_ctx();
        let plain = |target, group_by| Op::Summarize { target, length: None, group_by, topic: None };
        let run = |text: &str, mut p: Plan| {
            let mut notes: Vec<String> = Vec::new();
            notes.extend(summary_about(text, &mut p, &ctx, true).into_iter().map(|i| i.code));
            notes.extend(type_filter_repair(text, &mut p, &ctx, true).into_iter().map(|i| i.code));
            notes.extend(where_to_look(text, &mut p, &ctx, true).into_iter().map(|i| i.code));
            (p, notes)
        };
        let topic_of = |p: &Plan| match p.operations.as_slice() {
            [Op::Summarize { topic, target: Some(SummarizeTarget::Set), group_by: None, .. }] => topic.clone(),
            other => panic!("{other:?}"),
        };
        // t-1: every document of the library, one by one.
        let (p, notes) = run("AA에 대해서 요약해줘", model_plan(vec![plain(Some(SummarizeTarget::EachDoc), None)]));
        assert_eq!(topic_of(&p).as_deref(), Some("AA"));
        assert!(p.scope.is_empty() && !p.requires_confirmation, "{p:?}");
        assert_eq!(notes, vec!["summary_topic"]);
        assert_eq!(p.summary.as_deref(), Some("'AA'에 관한 내용을 문서에서 찾아 요약합니다"));
        // t-2: one summary per project of the library — the subject is the name of a project, and
        // its documents are what the sentence asks a summary of.
        let (p, notes) = run("AAA에 대해서 요약해줘", model_plan(vec![plain(None, Some(SummaryGroup::Project))]));
        assert_eq!(topic_of(&p), None);
        assert_eq!(p.scope.filter.project_id.as_deref(), Some("p-aaa"));
        assert!(p.scope.filter.with_reference && !p.requires_confirmation, "{p:?}");
        assert_eq!(notes, vec!["scope_filled"]);
        assert_eq!(p.summary.as_deref(), Some("'AAA' 프로젝트의 문서를 요약합니다"));
        // t-4: "요구사항" is part of the subject — no document type is named.
        let (p, notes) = run("보안 요구사항에 대해 요약해줘", model_plan(vec![plain(None, Some(SummaryGroup::None))]));
        assert_eq!(topic_of(&p).as_deref(), Some("보안 요구사항"));
        assert!(p.scope.is_empty(), "{:?}", p.scope);
        assert_eq!(notes, vec!["summary_topic"]);
        // t-5: the type the model took from the subject goes; the project the sentence names is the scope.
        let mut typed = model_plan(vec![plain(None, Some(SummaryGroup::Project))]);
        typed.scope.filter.doc_type = Some("acceptance".into());
        let (p, _) = run("한빛 납품 프로젝트에서 검수 기준에 대해 요약해줘", typed);
        assert_eq!(topic_of(&p).as_deref(), Some("검수 기준"));
        assert_eq!(
            (p.scope.filter.doc_type.as_deref(), p.scope.filter.project_id.as_deref()),
            (None, Some("p-hanbit"))
        );
        // A type named outside the subject is where to look.
        let (p, notes) = run("계약서에서 지체상금에 대해 요약해줘", model_plan(vec![plain(None, None)]));
        assert_eq!(topic_of(&p).as_deref(), Some("지체상금"));
        assert_eq!(p.scope.filter.doc_type.as_deref(), Some("dev_contract"));
        assert_eq!(notes, vec!["summary_topic", "scope_filled"]);
        // A file-name keyword that is the subject is not a filter; the length asked for stays.
        let mut keyed = model_plan(vec![Op::Summarize {
            target: None,
            length: Some(crate::planner::SummaryLength::Long),
            group_by: None,
            topic: None,
        }]);
        keyed.scope.filter.keyword = Some("하자보수".into());
        let (p, _) = run("하자보수 관련 내용을 자세히 요약해줘", keyed);
        assert!(p.scope.is_empty(), "{:?}", p.scope);
        assert!(
            matches!(&p.operations[0], Op::Summarize { length: Some(crate::planner::SummaryLength::Long), topic: Some(t), .. } if t == "하자보수")
        );

        // The model's own answer, search or question back for such a sentence becomes the same step.
        let answer = Op::Answer {
            question: "AA에 대해서 요약해줘".into(),
            cite: None,
            knowledge: Some(false),
            mode: None,
            reply: None,
        };
        let search = Op::Search { query: "AA".into(), mode: None, top_k: None, group_by: None };
        for op in [answer, search] {
            let (p, _) = run("AA에 대해서 요약해줘", model_plan(vec![op]));
            assert_eq!(topic_of(&p).as_deref(), Some("AA"));
        }
        let mut asked = Plan::clarify("어떤 문서를 요약할까요?".to_string(), 0.4);
        assert_eq!(
            summary_about("AA에 대해서 요약해줘", &mut asked, &ctx, true).map(|i| i.code).as_deref(),
            Some("summary_topic")
        );
        assert!(
            !asked.is_clarification()
                && asked.clarification_question.is_none()
                && asked.confidence >= crate::planner::MIN_CONFIDENCE
        );
        assert_eq!(topic_of(&asked).as_deref(), Some("AA"));

        // Left as planned: a summary without a subject, a kind of document, what the words only point
        // at, a plan that does more than read, a pasted text.
        for text in ["요약해줘", "견적서에 대해 요약해줘", "계약서 내용에 대해 요약해줘", "이 문서에 대해 요약해줘"]
        {
            let mut p = model_plan(vec![plain(None, None)]);
            assert!(summary_about(text, &mut p, &ctx, true).is_none(), "{text}");
            assert!(matches!(&p.operations[0], Op::Summarize { topic: None, target: None, .. }), "{text}");
        }
        let mut two = model_plan(vec![plain(None, None), Op::Tag { add: vec!["확인".into()], remove: vec![] }]);
        assert!(summary_about("AA에 대해서 요약하고 확인 태그 붙여줘", &mut two, &ctx, true).is_none());
        assert_eq!(two.operations.len(), 2);

        // "AA에 대해 알려줘" planned as an answer that looks nothing up is answered from the documents
        // (a `general` answer does look documents up, and stays).
        let nothing = |knowledge, mode| Op::Answer {
            question: "AA에 대해 알려줘".into(),
            cite: None,
            knowledge,
            mode,
            reply: None,
        };
        let mut p = model_plan(vec![nothing(Some(false), None)]);
        assert!(document_question("AA에 대해 알려줘", &mut p, &ctx));
        assert!(matches!(&p.operations[0], Op::Answer { knowledge: None, mode: None, .. }));
        let mut p = model_plan(vec![nothing(None, Some(crate::planner::AnswerMode::General))]);
        assert!(!document_question("AA에 대해 알려줘", &mut p, &ctx));
        let mut p = model_plan(vec![nothing(Some(false), None)]);
        assert!(
            !document_question("안녕하세요를 영어로 번역해줘", &mut p, &ctx),
            "the phrase to translate is in the sentence"
        );
    }

    /// 1.0.2 — with `topic` in its op table Gemma 4 E4B writes the subject itself (eval run of
    /// 2026-11-19, six of six), and not always the one the sentence names: "AA에 대해서 요약해줘" came
    /// back as `topic: "AAA"` (a project of the context), and "한빛 납품 프로젝트에서 검수 기준에 대해
    /// 요약해줘" with the document type `acceptance` that only the subject's words name.
    #[test]
    fn a_subject_the_model_wrote_is_one_the_sentence_names() {
        let mut ctx = topic_ctx();
        ctx.doc_types.push(crate::planner::DocTypeSummary {
            slug: "meeting_minutes".into(),
            label_ko: "회의록".into(),
            label_en: "Meeting minutes".into(),
        });
        let about =
            |topic: &str| Op::Summarize { target: None, length: None, group_by: None, topic: Some(topic.into()) };
        let run = |text: &str, mut p: Plan, ctx: &crate::planner::PlanContext| {
            let mut notes: Vec<String> = Vec::new();
            notes.extend(summary_about(text, &mut p, ctx, true).into_iter().map(|i| i.code));
            notes.extend(type_filter_repair(text, &mut p, ctx, true).into_iter().map(|i| i.code));
            notes.extend(where_to_look(text, &mut p, ctx, true).into_iter().map(|i| i.code));
            (p, notes)
        };
        let topic_of = |p: &Plan| match p.operations.as_slice() {
            [Op::Summarize { topic, .. }] => topic.clone(),
            other => panic!("{other:?}"),
        };
        // The name of a project the user did not write: the subject is what the sentence says.
        let (p, notes) = run("AA에 대해서 요약해줘", model_plan(vec![about("AAA")]), &ctx);
        assert_eq!(topic_of(&p).as_deref(), Some("AA"));
        assert!(p.scope.is_empty(), "{:?}", p.scope);
        assert_eq!(notes, vec!["summary_topic"]);
        // … and with the project named, its documents are summarised: no topic, one summary.
        for text in ["AAA에 대해서 요약해줘", "AAA 프로젝트에 대해 요약해줘", "AAA 프로젝트에 관해서 요약해줘"]
        {
            for op in [
                about("AAA"),
                Op::Summarize { target: None, length: None, group_by: Some(SummaryGroup::Project), topic: None },
            ] {
                let (p, notes) = run(text, model_plan(vec![op]), &ctx);
                assert!(
                    matches!(
                        p.operations.as_slice(),
                        [Op::Summarize { topic: None, target: Some(SummarizeTarget::Set), group_by: None, .. }]
                    ),
                    "{text}: {p:?}"
                );
                assert_eq!(p.scope.filter.project_id.as_deref(), Some("p-aaa"), "{text}");
                assert_eq!(notes, vec!["scope_filled"], "{text}");
            }
        }
        // Without the marker ("AAA 프로젝트 요약해줘" came back as `group_by: project` + `topic: "AAA"`
        // with an empty scope): before the validator the topic goes, and the split nobody asked for
        // becomes the one summary of the project the sentence names.
        let mut split = model_plan(vec![Op::Summarize {
            target: None,
            length: None,
            group_by: Some(SummaryGroup::Project),
            topic: Some("AAA".into()),
        }]);
        unsaid_topic("AAA 프로젝트 요약해줘", &mut split, &ctx);
        let (p, notes) = run("AAA 프로젝트 요약해줘", split, &ctx);
        assert!(
            matches!(
                p.operations.as_slice(),
                [Op::Summarize { topic: None, target: Some(SummarizeTarget::Set), group_by: None, .. }]
            ),
            "{p:?}"
        );
        assert_eq!(p.scope.filter.project_id.as_deref(), Some("p-aaa"));
        assert_eq!(notes, vec!["scope_filled"]);
        // (a split the sentence asks for stays: one summary per project of the library)
        let per_project = model_plan(vec![Op::Summarize {
            target: None,
            length: None,
            group_by: Some(SummaryGroup::Project),
            topic: None,
        }]);
        let (p, _) = run("프로젝트별로 요약해줘", per_project.clone(), &ctx);
        assert!(matches!(p.operations.as_slice(), [Op::Summarize { group_by: Some(SummaryGroup::Project), .. }]));
        assert_eq!(p.scope.filter.project_id, None);
        let (p, _) = run("AAA 프로젝트와 한빛 납품 프로젝트를 프로젝트별로 요약해줘", per_project, &ctx);
        assert!(matches!(p.operations.as_slice(), [Op::Summarize { group_by: Some(SummaryGroup::Project), .. }]));
        // The project named as the place and its name as the subject: a subject like any other.
        for op in [about("AAA"), Op::Summarize { target: None, length: None, group_by: None, topic: None }] {
            let (p, _) = run("AAA 프로젝트에서 AAA에 대해 요약해줘", model_plan(vec![op]), &ctx);
            assert_eq!(topic_of(&p).as_deref(), Some("AAA"));
            assert_eq!(p.scope.filter.project_id.as_deref(), Some("p-aaa"));
        }
        // (a summary of the project as the model planned it stays as it is)
        let mut scoped = model_plan(vec![Op::Summarize { target: None, length: None, group_by: None, topic: None }]);
        scoped.scope.filter.project_id = Some("p-aaa".into());
        let (p, notes) = run("AAA 프로젝트에 대해 요약해줘", scoped, &ctx);
        assert!(matches!(p.operations.as_slice(), [Op::Summarize { topic: None, target: None, .. }]));
        assert!(notes.is_empty(), "{notes:?}");
        // The subject as the sentence has it stays, and the type its words name is no place to look.
        let mut typed = model_plan(vec![about("검수 기준")]);
        typed.scope.filter.doc_type = Some("acceptance".into());
        let (p, notes) = run("한빛 납품 프로젝트에서 검수 기준에 대해 요약해줘", typed, &ctx);
        assert_eq!(topic_of(&p).as_deref(), Some("검수 기준"));
        assert_eq!(
            (p.scope.filter.doc_type.as_deref(), p.scope.filter.project_id.as_deref()),
            (None, Some("p-hanbit"))
        );
        assert_eq!(notes, vec!["scope_filled"]);
        // A part of the phrase before the marker: the model's reading of what is the subject and
        // what is the place stays, with the type the words outside its subject name.
        let mut minutes = model_plan(vec![about("보안")]);
        minutes.scope.filter.doc_type = Some("meeting_minutes".into());
        let (p, notes) = run("3월 회의록 보안에 대해 요약해줘", minutes, &ctx);
        assert_eq!(topic_of(&p).as_deref(), Some("보안"));
        assert_eq!(p.scope.filter.doc_type.as_deref(), Some("meeting_minutes"));
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            p.summary.as_deref(),
            Some("'보안'에 관한 내용을 문서에서 찾아 요약합니다"),
            "the card says the same"
        );
        // How long is read from the words outside the subject when the plan does not say.
        let (p, _) = run("계약서에서 지체상금에 대해 자세히 요약해줘", model_plan(vec![about("지체상금")]), &ctx);
        assert!(
            matches!(&p.operations[0], Op::Summarize { length: Some(crate::planner::SummaryLength::Long), topic: Some(t), .. } if t == "지체상금"),
            "{p:?}"
        );
        assert_eq!(p.scope.filter.doc_type.as_deref(), Some("dev_contract"));
        let (p, _) = run("상세 설계에 대해 요약해줘", model_plan(vec![about("상세 설계")]), &ctx);
        assert!(
            matches!(&p.operations[0], Op::Summarize { length: None, .. }),
            "the subject's words say nothing of the length"
        );
        // A split the sentence asks for cannot be made of a summary about a subject: said, and confirmed.
        let (mut p, _) = run("프로젝트별로 지체상금에 대해 요약해줘", model_plan(vec![about("지체상금")]), &ctx);
        let notes: Vec<String> = contract_checks("프로젝트별로 지체상금에 대해 요약해줘", &mut p, true)
            .into_iter()
            .map(|i| i.code)
            .collect();
        assert_eq!(notes, vec!["grouping_dropped"]);
        assert!(p.requires_confirmation && p.unmet.is_some(), "{p:?}");
        assert!(
            matches!(p.operations.as_slice(), [Op::Summarize { group_by: None, topic: Some(t), .. }] if t == "지체상금")
        );
        let (mut p, _) = run("지체상금에 대해 요약해줘", model_plan(vec![about("지체상금")]), &ctx);
        assert!(contract_checks("지체상금에 대해 요약해줘", &mut p, true).is_empty() && !p.requires_confirmation);
        // More than the subject, or other words: the subject of the sentence.
        for written in ["AA에 대해서", "AA 시스템", "AA에 대해서 요약"] {
            let (p, notes) = run("AA에 대해서 요약해줘", model_plan(vec![about(written)]), &ctx);
            assert_eq!(topic_of(&p).as_deref(), Some("AA"), "{written}");
            assert_eq!(notes, vec!["summary_topic"], "{written}");
        }
        // A sentence without the marker: a subject its words carry stays …
        let (p, notes) = run("지체상금 요약해줘", model_plan(vec![about("지체상금")]), &ctx);
        assert_eq!(topic_of(&p).as_deref(), Some("지체상금"));
        assert!(notes.is_empty() && p.scope.is_empty(), "{notes:?} {:?}", p.scope);
        // … and a subject nobody named is dropped: the summary the sentence asks.
        for (text, written) in [
            ("이 문서 요약해줘", "문서 내용"),
            ("이 문서 요약해줘", "이 문서"),
            ("요약해줘", "프로젝트 현황"),
            ("견적서 요약해줘", "견적서"),
            ("견적서에 대해 요약해줘", "견적서"),
            ("이 문서에 대해 요약해줘", "문서"),
        ] {
            let mut p = model_plan(vec![about(written)]);
            assert!(summary_about(text, &mut p, &ctx, true).is_none(), "{text}");
            assert_eq!(topic_of(&p), None, "{text} / {written}");
        }
        // Before the validator the same holds (`unsaid_topic`): what it then sees is the summary the
        // sentence asks — the target and the grouping of a plain summary stay as the model wrote them.
        for (text, written, settled) in [
            ("견적서에 대해 요약해줘", "견적서", None),
            ("AAA에 대해서 요약해줘", "AAA", None),
            ("이 문서 요약해줘", "문서 내용", None),
            ("AA에 대해서 요약해줘", "AAA", Some("AA")),
            ("AA에 대해서 요약해줘", "AA", Some("AA")),
            ("지체상금 요약해줘", "지체상금", Some("지체상금")),
        ] {
            let mut p = model_plan(vec![Op::Summarize {
                target: None,
                length: None,
                group_by: Some(SummaryGroup::Project),
                topic: Some(written.into()),
            }]);
            unsaid_topic(text, &mut p, &ctx);
            assert!(
                matches!(&p.operations[0], Op::Summarize { target: None, group_by: Some(SummaryGroup::Project), topic, .. } if topic.as_deref() == settled),
                "{text} / {written}: {p:?}"
            );
        }
        // What is open is where to look when the words point at it.
        ctx.current_document = Some("d-open".into());
        for plan in [
            model_plan(vec![about("지체상금")]),
            model_plan(vec![Op::Summarize { target: None, length: None, group_by: None, topic: None }]),
        ] {
            let (p, _) = run("이 문서에서 지체상금에 대해 요약해줘", plan, &ctx);
            assert_eq!(topic_of(&p).as_deref(), Some("지체상금"));
            assert_eq!(p.scope.doc_ids, vec!["d-open".to_string()]);
        }
        let (p, _) = run("지체상금에 대해 요약해줘", model_plan(vec![about("지체상금")]), &ctx);
        assert!(p.scope.is_empty(), "an open document is not the scope unless the words point at it: {:?}", p.scope);
    }

    /// 0.8.1: where to look comes from the user's words — the kind of document, the project, and
    /// "X 프로젝트 열어줘" opens the project instead of a folder or the library.
    #[test]
    fn the_sentence_says_where_to_look() {
        let ctx = crate::planner::PlanContext {
            projects: vec![
                crate::planner::ProjectSummary {
                    id: "p1".into(), name: "한빛 포털".into(), status: "active".into()
                },
                crate::planner::ProjectSummary { id: "p2".into(), name: "새봄 ERP".into(), status: "active".into() },
            ],
            ..Default::default()
        };
        let search = |q: &str| Op::Search { query: q.into(), mode: None, top_k: None, group_by: None };
        let codes = |text: &str, p: &mut Plan| -> Vec<String> {
            where_to_look(text, p, &ctx, true).into_iter().map(|i| i.code).collect()
        };
        // The kind of document; the words that named it leave the query.
        let mut p = model_plan(vec![search("참고 자료에서 납품 기한")]);
        assert_eq!(codes("참고 자료에서 납품 기한 검색해줘", &mut p), vec!["scope_filled"]);
        assert_eq!(p.scope.filter.role, Some(crate::planner::RoleParam::Knowledge));
        assert!(matches!(&p.operations[0], Op::Search { query, .. } if query == "납품 기한"));
        let mut p = model_plan(vec![Op::Summarize { target: None, length: None, group_by: None, topic: None }]);
        codes("업무 문서만 요약해줘", &mut p);
        assert_eq!(p.scope.filter.role, Some(crate::planner::RoleParam::Work));
        let mut p = model_plan(vec![search("납품 기한")]);
        codes("참고 자료 말고 업무 문서에서 납품 기한 찾아줘", &mut p);
        assert_eq!(p.scope.filter.role, Some(crate::planner::RoleParam::Work));
        // 1.0.1: a kind named as one more thing to cover is not a restriction (the card used to say
        // "문장대로 참고 자료만 대상으로 합니다", and the summary failed with no document).
        for said in [
            "참고자료도 포함해서 요약해줘",
            "참고 자료까지 요약해줘",
            "참고자료와 문서를 모두 요약해줘",
            "참고자료 포함해서 프로젝트 요약해줘",
            "참고자료를 포함해서 요약해줘",
            "참고 자료랑 같이 요약해줘",
            "업무 문서도 같이 요약해줘",
            "참고자료와 업무문서를 둘 다 요약해줘",
            "summarize the documents and reference material",
            "summarize it including the reference documents",
            "summarize the reference material and the contracts",
        ] {
            let mut p = model_plan(vec![Op::Summarize { target: None, length: None, group_by: None, topic: None }]);
            assert!(codes(said, &mut p).is_empty(), "{said}");
            assert_eq!(p.scope.filter.role, None, "{said}");
        }
        // … while a kind that is the place to look or the thing asked for still is.
        for (said, kind) in [
            ("참고 자료 요약해줘", crate::planner::RoleParam::Knowledge),
            ("참고자료만 요약해줘", crate::planner::RoleParam::Knowledge),
            ("참고자료를 요약해줘", crate::planner::RoleParam::Knowledge),
            ("참고 자료 중에서 과업 범위를 요약해줘", crate::planner::RoleParam::Knowledge),
            ("참고자료 과업 범위 요약", crate::planner::RoleParam::Knowledge),
            ("summarize the reference material", crate::planner::RoleParam::Knowledge),
            ("understand reference material first", crate::planner::RoleParam::Knowledge),
            ("업무문서를 요약해줘", crate::planner::RoleParam::Work),
        ] {
            let mut p = model_plan(vec![Op::Summarize { target: None, length: None, group_by: None, topic: None }]);
            assert_eq!(codes(said, &mut p), vec!["scope_filled"], "{said}");
            assert_eq!(p.scope.filter.role, Some(kind), "{said}");
        }
        // Inside a project a plan that only reads covers both kinds unless the sentence names one.
        let in_project = |ops: Vec<Op>| {
            let mut p = model_plan(ops);
            p.scope.filter.project_id = Some("p1".into());
            p
        };
        let mut p = in_project(vec![Op::Summarize { target: None, length: None, group_by: None, topic: None }]);
        codes("참고자료도 포함해서 요약해줘", &mut p);
        assert!(p.scope.filter.with_reference && p.scope.filter.role.is_none());
        let mut p = in_project(vec![Op::Summarize { target: None, length: None, group_by: None, topic: None }]);
        p.scope.filter.with_reference = true;
        codes("참고 자료만 요약해줘", &mut p);
        assert!(!p.scope.filter.with_reference, "a kind from the sentence replaces the default of the project scope");
        let mut p = model_plan(vec![Op::Summarize { target: None, length: None, group_by: None, topic: None }]);
        codes("한빛 포털 프로젝트 요약해줘", &mut p);
        assert!(p.scope.filter.with_reference && p.scope.filter.project_id.as_deref() == Some("p1"));
        // The words that add a kind are not what to search for either.
        for (said, query, left) in [
            ("참고자료 포함해서 납품 기한 찾아줘", "참고자료 포함해서 납품 기한", "납품 기한"),
            ("참고자료 포함해서 납품 기한 찾아줘", "참고 포함해서 납품 기한", "납품 기한"),
            ("참고 자료도 같이 납품 기한 찾아줘", "참고 자료도 같이 납품 기한", "납품 기한"),
            ("참고자료와 업무문서에서 납품 기한 찾아줘", "참고자료와 업무문서에서 납품 기한", "납품 기한"),
        ] {
            let mut p = model_plan(vec![search(query)]);
            assert!(codes(said, &mut p).is_empty(), "{said}");
            assert!(
                matches!(&p.operations[0], Op::Search { query, .. } if query == left),
                "{said}: {:?}",
                p.operations
            );
        }

        // Nothing said, or an op that is about reference groups: untouched.
        let mut p = model_plan(vec![search("납품 기한")]);
        assert!(codes("납품 기한 검색해줘", &mut p).is_empty() && p.scope.filter.role.is_none());
        let mut p = model_plan(vec![Op::SetRole { role: crate::planner::RoleParam::Knowledge }]);
        assert!(codes("선택한 문서를 참고 자료로 바꿔줘", &mut p).is_empty() && p.scope.filter.role.is_none());

        // The project the sentence names is the scope.
        let mut p = model_plan(vec![search("납품 기한")]);
        assert_eq!(codes("한빛 포털 프로젝트에서 납품 기한 검색해줘", &mut p), vec!["scope_filled"]);
        assert_eq!(p.scope.filter.project_id.as_deref(), Some("p1"));
        // … not when the hits are wanted per project, and not when two projects are named.
        let mut p = model_plan(vec![Op::Search {
            query: "납품".into(),
            mode: None,
            top_k: None,
            group_by: Some(SummaryGroup::Project),
        }]);
        assert!(codes("한빛 포털 말고 프로젝트별로 납품 검색", &mut p).is_empty());
        let mut p = model_plan(vec![search("납품")]);
        assert!(codes("한빛 포털과 새봄 ERP에서 납품 검색", &mut p).is_empty() && p.scope.filter.project_id.is_none());

        // "X 프로젝트 열어줘" came back as a folder / the library screen.
        for op in [
            Op::Open { target: crate::planner::OpenTarget::Folder, reference: "한빛 포털".into() },
            Op::Open { target: crate::planner::OpenTarget::Screen, reference: "library".into() },
        ] {
            let mut p = model_plan(vec![op]);
            codes("한빛 포털 프로젝트 열어줘", &mut p);
            assert!(
                matches!(&p.operations[0], Op::Open { target: crate::planner::OpenTarget::Project, reference } if reference == "p1"),
                "{:?}",
                p.operations
            );
        }
        // A folder that is asked for stays a folder.
        let mut p = model_plan(vec![Op::Open {
            target: crate::planner::OpenTarget::Folder,
            reference: "한빛 포털".into(),
        }]);
        codes("한빛 포털 폴더 열어줘", &mut p);
        assert!(matches!(&p.operations[0], Op::Open { target: crate::planner::OpenTarget::Folder, .. }));

        // "프로젝트별로 검색해줘": the search keeps the split; one it cannot make is reported.
        let mut p = model_plan(vec![search("납품 기한")]);
        assert!(contract_checks("프로젝트별로 납품 기한 검색해줘", &mut p, true).is_empty());
        assert!(matches!(&p.operations[0], Op::Search { group_by: Some(SummaryGroup::Project), .. }));
        let mut p = model_plan(vec![search("납품 기한")]);
        let notes: Vec<String> =
            contract_checks("담당자별로 납품 기한 검색해줘", &mut p, true).into_iter().map(|i| i.code).collect();
        assert_eq!(notes, vec!["grouping_dropped"]);
    }

    /// Audit USR-1: the plans the model made of the reported sentences ran on its own confidence.
    #[test]
    fn a_settings_change_the_sentence_does_not_name_is_confirmed_first() {
        use crate::settings::{Profile, Theme};
        use serde_json::json;
        // What 0.7.0 did with "UI 배율 120%" and "DPI 를 200 으로 변경해줘".
        let mut p = model_plan(vec![Op::SetProfile { profile: Profile::Balanced }]);
        assert_eq!(codes("UI 배율 120%", &mut p), vec!["ungrounded_change"]);
        assert!(p.requires_confirmation);
        let mut p = model_plan(vec![Op::SetTheme { theme: Theme::System }]);
        assert_eq!(codes("DPI 를 200 으로 변경해줘", &mut p), vec!["ungrounded_change"]);
        assert!(p.requires_confirmation);
        // The right key with a value the sentence does not say, and the right value on another key.
        let dpi = |v: i64| Op::SetSetting { key: "processing.render_dpi".into(), value: json!(v) };
        let mut p = model_plan(vec![dpi(150)]);
        assert_eq!(codes("렌더링 해상도를 300으로 올려줘", &mut p), vec!["ungrounded_change"]);
        let mut p = model_plan(vec![Op::SetSetting { key: "knowledge.top_k".into(), value: json!(300) }]);
        assert_eq!(codes("렌더링 해상도를 300으로 올려줘", &mut p), vec!["ungrounded_change"]);
        // Named setting + said value: nothing to confirm.
        for (text, op) in [
            ("렌더링 해상도를 300으로 올려줘", dpi(300)),
            (
                "글자가 작으니 화면 배율을 120%로 해줘",
                Op::SetSetting { key: "ui.scale_percent".into(), value: json!(120) },
            ),
            (
                "이제부터 개인정보 마스킹은 꺼줘",
                Op::SetSetting { key: "privacy.pii_masking".into(), value: json!(false) },
            ),
            ("눈이 아파서 그런데 다크 테마로 바꿔줄래", Op::SetTheme { theme: Theme::Dark }),
            ("프로파일을 균형으로 돌려놔", Op::SetProfile { profile: Profile::Balanced }),
            ("모델을 12B로 올려줘", Op::SetModel { id: "gemma4-12b-q4km".into() }),
        ] {
            let mut p = model_plan(vec![op]);
            assert!(codes(text, &mut p).is_empty(), "{text}: {p:?}");
            assert!(!p.requires_confirmation, "{text}");
        }
        // 0.8.0 review: the value alone is not enough, a negated sentence grounds nothing, a model
        // family grounds no model, and a number is not divided for an integer setting.
        for (text, op) in [
            ("시스템 요구사항 문서 찾아줘", Op::SetTheme { theme: Theme::System }),
            ("이 문서를 영어로 번역해줘", Op::SetLanguage { lang: crate::settings::Language::En }),
            ("금액을 정확하게 추출해줘", Op::SetProfile { profile: Profile::Accuracy }),
            ("개인정보 마스킹 끄지 마세요", Op::SetSetting { key: "privacy.pii_masking".into(), value: json!(false) }),
            ("gemma4 e2b로 바꿔", Op::SetModel { id: "gemma4-12b-q4km".into() }),
            ("근거 개수 500", Op::SetSetting { key: "knowledge.top_k".into(), value: json!(5) }),
            ("inbox 폴더의 견적서를 처리해줘", Op::SetInboxWatch { enabled: true }),
            (
                "엔진 다시 시작해줘",
                Op::ManageModel {
                    action: ModelAction::SetBackend,
                    id: None,
                    tier: None,
                    backend: Some("cpu".into()),
                    path: None,
                },
            ),
        ] {
            let mut p = model_plan(vec![op]);
            assert_eq!(codes(text, &mut p), vec!["ungrounded_change"], "{text}");
            assert!(p.requires_confirmation, "{text}");
        }
        for (text, op) in [
            (
                "검토 임계값을 0.6으로 낮춰줘",
                Op::SetSetting { key: "processing.review_threshold".into(), value: json!(0.6) },
            ),
            (
                "검토 임계값을 90%로 해줘",
                Op::SetSetting { key: "processing.review_threshold".into(), value: json!(0.9) },
            ),
            (
                "근거 컨텍스트 토큰을 8,000으로",
                Op::SetSetting { key: "knowledge.context_tokens".into(), value: json!(8000) },
            ),
            ("inbox 감시 꺼줘", Op::SetInboxWatch { enabled: false }),
            ("모델을 e2b로 바꿔", Op::SetModel { id: "gemma4-e2b-q4km".into() }),
            (
                "엔진 백엔드를 cpu로 바꿔",
                Op::ManageModel {
                    action: ModelAction::SetBackend,
                    id: None,
                    tier: None,
                    backend: Some("cpu".into()),
                    path: None,
                },
            ),
        ] {
            let mut p = model_plan(vec![op]);
            assert!(codes(text, &mut p).is_empty(), "{text}: {p:?}");
        }
        // The assistant's question does not ground a value: only the reply does.
        let ask = |reply: &str| {
            format!("배율 바꿔줘\n\n[어시스턴트 질문] UI 배율(%) — 바꿀 값을 함께 말씀해 주세요(90~150).\n[사용자 답변] {reply}")
        };
        let scale = |v: i64| Op::SetSetting { key: "ui.scale_percent".into(), value: json!(v) };
        let mut p = model_plan(vec![scale(150)]);
        assert_eq!(codes(&ask("적당히 해줘"), &mut p), vec!["ungrounded_change"]);
        let mut p = model_plan(vec![scale(130)]);
        assert!(codes(&ask("130으로"), &mut p).is_empty());
        assert_eq!(
            carried_text(&ask(
                "다음 글을 다듬어서 요약해줘 그리고 130으로 맞춰줘 부탁해요 꼭이요 감사합니다 정말로요 진짜"
            )),
            None
        );
        // Ops that are not settings changes are not this check's business.
        let mut p = model_plan(vec![Op::Search { query: "한빛".into(), mode: None, top_k: None, group_by: None }]);
        assert!(codes("한빛 계약 찾아줘", &mut p).is_empty());
    }

    /// Audit USR-4: "프로젝트 별 내용을 요약해줘" became one summary of everything.
    #[test]
    fn what_the_plan_leaves_out_is_shown_and_confirmed() {
        let summarize = |target, group_by| Op::Summarize { target, length: None, group_by, topic: None };
        // The model says so itself.
        let mut p = model_plan(vec![summarize(Some(SummarizeTarget::Set), None)]);
        p.unmet = Some("  프로젝트별로 나누기  ".into());
        assert_eq!(codes("전체 내용을 요약해줘", &mut p), vec!["unmet_request"]);
        assert!(p.requires_confirmation);
        assert_eq!(p.unmet.as_deref(), Some("프로젝트별로 나누기"));
        // The model says nothing, the sentence does: a criterion the op has is filled in …
        let mut p = model_plan(vec![summarize(Some(SummarizeTarget::Set), None)]);
        p.unmet = Some(String::new());
        assert!(codes("프로젝트 별 내용을 요약해줘", &mut p).is_empty());
        assert!(matches!(&p.operations[0], Op::Summarize { group_by: Some(SummaryGroup::Project), .. }), "{p:?}");
        assert!(!p.requires_confirmation && p.unmet.is_none());
        let mut p = model_plan(vec![summarize(Some(SummarizeTarget::Set), Some(SummaryGroup::None))]);
        assert!(codes("각 폴더의 문서를 요약해줘", &mut p).is_empty());
        assert!(matches!(&p.operations[0], Op::Summarize { group_by: Some(SummaryGroup::Folder), .. }));
        let mut p = model_plan(vec![summarize(Some(SummarizeTarget::Set), None)]);
        assert!(codes("문서별로 요약해줘", &mut p).is_empty());
        assert!(matches!(
            &p.operations[0],
            Op::Summarize { target: Some(SummarizeTarget::EachDoc), group_by: None, .. }
        ));
        let aggregate = |group_by| Op::Aggregate {
            func: crate::planner::AggregateFn::Sum,
            field: Some("total".into()),
            group_by,
            include_unreviewed: None,
        };
        let mut p = model_plan(vec![aggregate(None)]);
        assert!(codes("거래처별 견적 합계 알려줘", &mut p).is_empty());
        assert!(matches!(&p.operations[0], Op::Aggregate { group_by: Some(GroupBy::Vendor), .. }));
        // … and one it does not have is shown and confirmed.
        let mut p = model_plan(vec![summarize(Some(SummarizeTarget::Set), None)]);
        assert_eq!(codes("견적서를 담당자별로 요약해줘", &mut p), vec!["grouping_dropped"]);
        assert!(p.requires_confirmation && p.unmet.is_some());
        let mut p = model_plan(vec![summarize(Some(SummarizeTarget::Set), None)]);
        assert_eq!(codes("월별로 요약해줘", &mut p), vec!["grouping_dropped"]);
        let mut p = model_plan(vec![aggregate(Some(GroupBy::None))]);
        assert_eq!(codes("폴더마다 합계를 내줘", &mut p), vec!["grouping_dropped"]);
        // A plan that keeps the split passes, and an empty `unmet` is dropped.
        let mut p = model_plan(vec![summarize(Some(SummarizeTarget::Set), Some(SummaryGroup::Project))]);
        p.unmet = Some(String::new());
        assert!(codes("프로젝트 별 내용을 요약해줘", &mut p).is_empty());
        assert!(!p.requires_confirmation && p.unmet.is_none());
        let mut p = model_plan(vec![summarize(Some(SummarizeTarget::EachDoc), None)]);
        assert!(codes("문서별로 요약해줘", &mut p).is_empty());
        let mut p = model_plan(vec![summarize(Some(SummarizeTarget::Set), None)]);
        assert!(codes("이 프로젝트 문서를 요약해줘", &mut p).is_empty());
        assert!(matches!(&p.operations[0], Op::Summarize { group_by: None, .. }));
        // "별" that is not the suffix.
        assert_eq!(split_of("프로젝트 별도 관리 문서를 요약해줘"), None);
        assert_eq!(split_of("월별표를 요약해줘"), None);
        assert_eq!(split_of("각 월요일 회의록 요약"), None);
        assert_eq!(split_of("프로젝트별로"), Some(SplitBy::Project));
        assert_eq!(split_of("유형 별 요약"), Some(SplitBy::DocType));
        assert_eq!(split_of("summarize the documents by project"), Some(SplitBy::Project));
        assert_eq!(split_of("월별 합계"), Some(SplitBy::Month));
        // "N별" inside a title, or a split that another step of the plan already took (0.8.0 review).
        for text in [
            "거래처별 단가표의 합계 알려줘",
            "프로젝트별 예산표 요약해줘",
            "월별 보고서 요약해줘",
            "유형별 폴더로 정리하고 요약해줘",
        ] {
            assert_eq!(split_of(text), None, "{text}");
        }
        let mut p = model_plan(vec![
            Op::Move { folder: "documents/{{doc_type}}".into(), group_by: Some(GroupBy::DocType), only_reviewed: None },
            summarize(Some(SummarizeTarget::EachDoc), None),
        ]);
        assert!(codes("유형별로 정리하고 요약해줘", &mut p).is_empty());
        assert!(matches!(&p.operations[1], Op::Summarize { group_by: None, .. }), "{p:?}");
        // Clarifications are questions already.
        let mut p = Plan::clarify("어느 프로젝트인가요?", 0.4);
        p.unmet = Some("x".into());
        assert!(codes("프로젝트별 요약", &mut p).is_empty());
    }

    /// Audit USR-3: "안녕하세요." produced a plan card "없음 · 대상 문서 11건" and a "건너뜀" result.
    #[test]
    fn a_plan_with_nothing_to_do_becomes_a_reply() {
        let mut p = model_plan(vec![Op::Noop {}]);
        p.scope.folder = Some("documents".into());
        p.requires_confirmation = true;
        assert!(noop_to_chat("그렇구나, 알겠어.", &mut p, true));
        assert!(matches!(
            &p.operations[0],
            Op::Answer { mode: Some(AnswerMode::Chat), knowledge: Some(false), reply: None, question, .. }
                if question == "그렇구나, 알겠어."
        ));
        assert!(p.scope.is_empty() && !p.requires_confirmation && p.intent == OpKind::Answer);
        // What the model said it cannot do is the reply, and a low confidence asks nothing back.
        let mut p = model_plan(vec![Op::Noop {}]);
        p.unmet = Some("메일 발송은 할 수 없습니다".into());
        p.confidence = 0.3;
        assert!(noop_to_chat("이 문서를 메일로 보내줘", &mut p, true));
        assert!(matches!(&p.operations[0], Op::Answer { reply: Some(r), .. } if r.contains("메일 발송")), "{p:?}");
        assert!(p.unmet.is_none() && p.confidence >= crate::planner::MIN_CONFIDENCE);
        // A plan that does something next to a noop, and an empty plan, are left alone.
        let mut p = model_plan(vec![Op::Noop {}, Op::Help {}]);
        assert!(!noop_to_chat("도움말", &mut p, true));
        let mut p = model_plan(vec![Op::Help {}]);
        p.operations.clear();
        assert!(!noop_to_chat("x", &mut p, true));
    }

    /// A reply to a question about a pasted text: the text is still the command's, the planner still
    /// reads an excerpt of it, and the answer works on the whole exchange.
    #[test]
    fn a_reply_about_a_pasted_text_keeps_the_text() {
        let body = BODY.repeat(6);
        let text =
            format!("다음 글 번역해줘: {body}\n\n[어시스턴트 질문] 어떤 언어로 번역할까요?\n[사용자 답변] 영어로");
        assert_eq!(carried_text(&text), Some(body.as_str()));
        let planning = planning_text(&text);
        assert!(planning.chars().count() < 340, "{}", planning.chars().count());
        assert!(
            planning.contains("[어시스턴트 질문] 어떤 언어로") && planning.ends_with("[사용자 답변] 영어로"),
            "{planning}"
        );
        // A plan that looks the text up in the library is re-targeted to the text itself.
        let mut p = model_plan(vec![Op::Answer {
            question: "번역".into(),
            cite: None,
            knowledge: None,
            mode: None,
            reply: None,
        }]);
        assert!(prefer_text_only_answer(&text, &mut p, true).is_some());
        assert!(
            matches!(&p.operations[0], Op::Answer { question, knowledge: Some(false), .. } if question.contains(&body) && question.ends_with("영어로"))
        );
        // The assistant's question alone is never a carried text ("요약" + a long question with a colon).
        let ask =
            "견적서 요약해줘\n\n[어시스턴트 질문] 어떤 문서를 요약할까요: 라이브러리에서 선택하거나 폴더·유형·날짜를 말씀해 주세요(전체면 '전부'라고 해 주세요).\n[사용자 답변] 전부";
        assert_eq!(carried_text(ask), None);
    }

    /// "프로젝트별" clears the open project as a filter — unless the sentence names that project.
    #[test]
    fn the_open_project_is_only_dropped_when_the_sentence_does_not_name_it() {
        let ctx = crate::planner::PlanContext {
            scope_project: Some("p1".into()),
            projects: vec![crate::planner::ProjectSummary {
                id: "p1".into(),
                name: "한빛 포털 구축".into(),
                status: "active".into(),
            }],
            ..Default::default()
        };
        assert!(!names_scope_project("프로젝트별로 요약해줘", &ctx));
        assert!(names_scope_project("한빛 포털 구축 문서를 프로젝트별로 요약해줘", &ctx));
        // Only the user's words count.
        let ask = "프로젝트별로 요약해줘\n\n[어시스턴트 질문] '한빛 포털 구축'만 볼까요?\n[사용자 답변] 아니 전부";
        assert!(!names_scope_project(ask, &ctx));
    }

    /// Audit PLN-16: a long pasted text is planned from its instruction and an excerpt.
    #[test]
    fn a_long_pasted_text_is_planned_from_an_excerpt_and_put_back() {
        let body = BODY.repeat(6);
        let text = format!("다음 글을 다듬어줘: {body}");
        let planning = planning_text(&text);
        assert!(planning.starts_with("다음 글을 다듬어줘: "), "{planning}");
        assert!(planning.chars().count() < 260, "{}", planning.chars().count());
        assert!(planning.contains(&format!("{}자", body.chars().count())), "{planning}");
        // Short bodies and plain commands are sent as they are.
        let short = format!("다음 글을 다듬어줘: {BODY}");
        assert_eq!(planning_text(&short), short);
        assert_eq!(planning_text("지난달 견적서 합계"), "지난달 견적서 합계");
        // The model wrote the excerpt into the question: the whole sentence comes back.
        let mut p = model_plan(vec![Op::Answer {
            question: planning.to_string(),
            cite: Some(false),
            knowledge: Some(false),
            mode: None,
            reply: None,
        }]);
        restore_carried_text(&text, &mut p);
        assert!(matches!(&p.operations[0], Op::Answer { question, .. } if question == &text));
        // Questions about the library keep the model's wording.
        let mut p = model_plan(vec![Op::Answer {
            question: "계약 기간은?".into(),
            cite: None,
            knowledge: None,
            mode: None,
            reply: None,
        }]);
        restore_carried_text(&text, &mut p);
        assert!(matches!(&p.operations[0], Op::Answer { question, .. } if question == "계약 기간은?"));
    }

    /// 1.0.0, user decision 2026-11-16 — "참고 자료로 지정" opens "프로젝트에 넣기" on 참고 자료. The
    /// plan Gemma 4 E4B wrote for "선택한 문서들 참고자료로 지정" (eval run of the same day), replayed:
    /// `manage_reference add` into a project the sentence does not name.
    #[test]
    fn reference_material_at_a_place_nobody_named_opens_the_dialog() {
        use crate::planner::{ReferenceAction, RoleParam};
        let ctx = crate::planner::PlanContext {
            projects: vec![crate::planner::ProjectSummary {
                id: "p-hanbit".into(),
                name: "한빛 납품".into(),
                status: "active".into(),
            }],
            ..Default::default()
        };
        let add = |project: Option<&str>, group: Option<&str>| Op::ManageReference {
            action: ReferenceAction::Add,
            group: group.map(str::to_string),
            name: None,
            project: project.map(str::to_string),
        };
        let opens = |p: &Plan| matches!(p.operations.as_slice(), [Op::SetRole { role: RoleParam::Knowledge }]);

        let mut p = model_plan(vec![add(Some("p-hanbit"), None)]);
        p.requires_confirmation = true;
        assert!(reference_place("선택한 문서들 참고자료로 지정", &mut p, &ctx, true));
        assert!(opens(&p) && p.intent == OpKind::SetRole && !p.requires_confirmation, "{p:?}");
        assert_eq!(p.summary.as_deref(), Some("문서를 프로젝트의 참고 자료로 넣는 화면을 엽니다"));
        // The same for the project's reference material written as a project membership.
        let mut p = model_plan(vec![Op::AddToProject { project: None, doc_ids: Vec::new(), as_reference: true }]);
        assert!(reference_place("mark these as reference material", &mut p, &ctx, false) && opens(&p));

        // A place the user's words name is the user's: the project, a group, the common material.
        for text in [
            "선택한 문서들 한빛 납품 참고자료로 지정",
            "이 문서를 이 프로젝트의 참고 자료로 지정해줘",
            "이 문서를 '사규' 묶음의 참고 자료로 지정해줘",
            "이 문서를 공통 참고 자료로 지정해줘",
        ] {
            let mut p = model_plan(vec![add(Some("p-hanbit"), None)]);
            assert!(!reference_place(text, &mut p, &ctx, true) && !opens(&p), "{text}");
        }
        // A sentence that puts documents in ("넣어줘", "추가") is not a mark: it stays what it is.
        let mut p = model_plan(vec![add(None, None)]);
        assert!(!reference_place("이 문서를 참고 자료에 넣어줘", &mut p, &ctx, true) && !opens(&p));
        // Work documents are not this guard's.
        let mut p = model_plan(vec![Op::AddToProject { project: None, doc_ids: Vec::new(), as_reference: false }]);
        assert!(!reference_place("이 문서를 참고 자료로 지정해줘", &mut p, &ctx, true) && !opens(&p));
        // The step the model wrote itself is left as it is; its plan says what the step does and
        // is not confirmed twice (the dialog is where the user confirms).
        let mut p = model_plan(vec![Op::SetRole { role: RoleParam::Knowledge }]);
        p.requires_confirmation = true;
        assert!(!reference_place("이 문서를 참고 자료로 지정해줘", &mut p, &ctx, true));
        assert!(
            opens(&p) && !p.requires_confirmation && p.summary.as_deref().is_some_and(|s| s.contains("화면을 엽니다"))
        );
        // … and a mark as a work document is untouched.
        let mut p = model_plan(vec![Op::SetRole { role: RoleParam::Work }]);
        p.requires_confirmation = true;
        assert!(!reference_place("이 문서를 업무 문서로 표시", &mut p, &ctx, true));
        assert!(p.requires_confirmation && p.summary.as_deref() == Some("s"));
    }
}
