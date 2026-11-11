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
    let text = text.trim();
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
    carried_text(text)?;
    if plan.operations.is_empty() || explicit_scope(plan) {
        return None;
    }
    let knowledge_only = plan.operations.iter().all(|op| match op {
        Op::Summarize { .. } | Op::Explain { .. } | Op::Compare { .. } | Op::Search { .. } => true,
        Op::Answer { knowledge, .. } => knowledge.unwrap_or(true),
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
    for op in &mut plan.operations {
        if let Op::Answer { question, knowledge: Some(false), mode: None, .. } = op {
            *question = text.trim().to_string();
        }
    }
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
    // The split may belong to another step ("유형별 폴더로 정리하고 요약해줘"): a plan that
    // already groups something has taken it.
    let taken = plan.operations.iter().any(|op| match op {
        Op::Move { group_by, .. } => !matches!(group_by, None | Some(GroupBy::None)),
        Op::Organize { .. } | Op::Export { .. } | Op::ClassifyProjects { .. } => true,
        Op::Aggregate { group_by, .. } => !matches!(group_by, None | Some(GroupBy::None)),
        Op::Summarize { group_by, .. } => !matches!(group_by, None | Some(SummaryGroup::None)),
        _ => false,
    });
    if taken {
        return false;
    }
    let mut dropped = false;
    for op in &mut plan.operations {
        match op {
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

/// `Some(what)` when a change that runs without confirmation is not something the sentence names
/// (audit USR-1: "UI 배율 120%" ran `set_profile balanced` on the model's own confidence of 0.95).
/// Both have to be in the sentence: the setting and the value ("영어로 번역해줘" has a language,
/// not the language setting).
fn ungrounded(text: &str, op: &Op) -> Option<String> {
    let named = |key: &str| {
        catalog::find(key).is_some_and(|(_, spec)| catalog::names_of(spec).iter().any(|n| fastpath::mentions(text, n)))
    };
    let grounded = |key: &str, value: &Value| {
        catalog::find(key)
            .is_some_and(|(canonical, spec)| named(canonical) && value_is_grounded(text, canonical, spec.ty, value))
    };
    match op {
        Op::SetSetting { key, value } => {
            let (_, spec) = catalog::find(key)?;
            (!grounded(key, value)).then(|| format!("{} → {}", spec.label_ko, value_text(value)))
        }
        Op::SetTheme { theme } => {
            let v = Value::String(theme.as_str().to_string());
            (!grounded("ui.theme", &v)).then(|| format!("테마 → {}", theme.as_str()))
        }
        Op::SetLanguage { lang } => {
            let v = Value::String(lang.as_str().to_string());
            (!grounded("ui.language", &v)).then(|| format!("언어 → {}", lang.as_str()))
        }
        Op::SetProfile { profile } => {
            let v = serde_json::to_value(profile).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            (!grounded("models.profile", &Value::String(v.clone()))).then(|| format!("추론 프로파일 → {v}"))
        }
        Op::SetInboxWatch { enabled } => (!grounded("processing.inbox_watch", &Value::Bool(*enabled)))
            .then(|| format!("inbox 감시 → {}", if *enabled { "켜기" } else { "끄기" })),
        Op::SetModel { id } => (!model_is_named(text, id)).then(|| format!("모델 → {id}")),
        Op::ManageModel { action: ModelAction::SetTier, tier: Some(tier), .. } => {
            (!grounded("models.manual_tier", &Value::String(tier.clone()))).then(|| format!("티어 → {tier}"))
        }
        Op::ManageModel { action: ModelAction::SetBackend, backend: Some(backend), .. } => {
            (!grounded("models.backend", &Value::String(backend.clone()))).then(|| format!("백엔드 → {backend}"))
        }
        _ => None,
    }
}

/// The split the sentence asks for ("~별", "마다", "by …"): filled in when the op has the criterion,
/// shown and confirmed when it has not (담당자별 …). Also applied to plans made by the phrase rules.
pub fn split_check(text: &str, plan: &mut Plan, ko: bool) -> Option<PlanIssue> {
    if plan.is_clarification() || !keep_the_split(text, plan) {
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

/// Checks a model plan against the sentence it was made from (0.8.0 plan §1.3 / §1.5). Anything
/// found turns the plan into one the user confirms, with a note for the card; the model's own
/// `confidence` plays no part.
pub fn contract_checks(text: &str, plan: &mut Plan, ko: bool) -> Vec<PlanIssue> {
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
    let ungrounded_ops: Vec<(usize, String)> =
        plan.operations.iter().enumerate().filter_map(|(i, op)| ungrounded(text, op).map(|w| (i, w))).collect();
    for (i, what) in ungrounded_ops {
        plan.requires_confirmation = true;
        notes.push(PlanIssue::warn(
            format!("operations[{i}]"),
            "ungrounded_change",
            if ko {
                format!("문장에서 이 변경의 근거를 찾지 못했습니다({what}). 맞으면 승인해 주세요")
            } else {
                format!("the sentence does not name this change ({what}); approve it if it is what you meant")
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
        let mut plan = kb_plan(Op::Summarize { target: Some(SummarizeTarget::EachDoc), length: None, group_by: None });
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
        let mut scoped = kb_plan(Op::Summarize { target: None, length: None, group_by: None });
        scoped.scope.folder = Some("documents/2026".into());
        assert!(prefer_text_only_answer(&text, &mut scoped, true).is_none());
        let mut validate = kb_plan(Op::Validate {});
        assert!(prefer_text_only_answer(&text, &mut validate, true).is_none());
        let mut plain = kb_plan(Op::Summarize { target: None, length: None, group_by: None });
        assert!(prefer_text_only_answer("지난달 계약서 요약해줘", &mut plain, true).is_none());
        // Documents the plan names are a scope too: the "text" is an instruction about them.
        let mut selected = kb_plan(Op::Summarize { target: None, length: None, group_by: None });
        selected.scope.doc_ids = vec!["d1".into()];
        assert!(prefer_text_only_answer(&text, &mut selected, true).is_none());
        // A date range the resolver filled from "12월말" inside the body is not an explicit scope.
        let mut dated = kb_plan(Op::Summarize { target: Some(SummarizeTarget::EachDoc), length: None, group_by: None });
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
        // Ops that are not settings changes are not this check's business.
        let mut p = model_plan(vec![Op::Search { query: "한빛".into(), mode: None, top_k: None }]);
        assert!(codes("한빛 계약 찾아줘", &mut p).is_empty());
    }

    /// Audit USR-4: "프로젝트 별 내용을 요약해줘" became one summary of everything.
    #[test]
    fn what_the_plan_leaves_out_is_shown_and_confirmed() {
        let summarize = |target, group_by| Op::Summarize { target, length: None, group_by };
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
}
