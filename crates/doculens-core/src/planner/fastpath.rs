//! The whole-sentence fast path (0.8.0 plan §1.6): the two kinds of turn that are read without
//! the model, before it, in every planning mode.
//!
//! * A **settings sentence** — exactly a setting's name, one value of that setting's type and
//!   nothing but particles / a change verb around them: "UI 배율 120%", "배율을 130 으로 변경해줘",
//!   "DPI 를 200 으로 변경해줘", "프로파일 정확도", "다크 모드로 바꿔줘". The names, types and ranges
//!   come from the settings catalog.
//! * A **fixed greeting or thanks** — "안녕하세요", "고마워", "hello".
//!
//! This is not the keyword routing that was removed on 2026-11-09 (rules that fired on a word
//! inside any sentence and turned questions into commands). Here the WHOLE sentence has to be
//! accounted for; a question, an extra word, two settings or a value of the wrong type all fall
//! through to the model. A value outside the setting's range is not applied: the reply states the
//! range instead.

use serde_json::Value;

use crate::settings::{Language, Profile, Theme};

use super::settings_catalog::{self as catalog, SettingSpec, ValueType};
use super::{AnswerMode, Op, Plan, PlanContext};

/// Reads `text` without the model, or `None` when the model should read it.
pub fn plan(text: &str, ctx: &PlanContext) -> Option<Plan> {
    let ko = ctx.language == Language::Ko;
    greeting(text, ko).or_else(|| setting(text, ko))
}

// ---------------------------------------------------------------------------
// Greetings
// ---------------------------------------------------------------------------

const GREETINGS: &[&str] = &[
    "안녕",
    "안녕하세요",
    "안녕하십니까",
    "안녕하세용",
    "하이",
    "헬로",
    "헬로우",
    "반가워",
    "반가워요",
    "반갑습니다",
    "좋은 아침",
    "좋은 아침이에요",
    "hi",
    "hello",
    "hey",
    "hi there",
    "hello there",
    "good morning",
    "good afternoon",
    "good evening",
];
const THANKS: &[&str] = &[
    "고마워",
    "고마워요",
    "고맙습니다",
    "감사",
    "감사해요",
    "감사합니다",
    "수고했어",
    "수고했어요",
    "수고하셨습니다",
    "잘했어",
    "잘했어요",
    "좋아요",
    "thanks",
    "thank you",
    "thanks a lot",
    "thx",
    "great",
    "nice",
];

/// Lowercase, sentence punctuation dropped, whitespace collapsed.
fn plain(text: &str) -> String {
    let lower = text.trim().to_lowercase();
    let cleaned: String =
        lower.chars().map(|c| if matches!(c, '.' | '!' | '~' | ',' | '^' | '♡' | '♥') { ' ' } else { c }).collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn greeting(text: &str, ko: bool) -> Option<Plan> {
    let p = plain(text);
    let reply = if GREETINGS.contains(&p.as_str()) {
        if ko {
            "안녕하세요! 문서에 대해 물어보시거나, 가져오기·정리·추출·요약 같은 작업을 말씀해 주세요."
        } else {
            "Hello! Ask me about your documents, or tell me what to do — import, organise, extract, summarise."
        }
    } else if THANKS.contains(&p.as_str()) {
        if ko {
            "도움이 되어 다행입니다. 더 필요한 것이 있으면 말씀해 주세요."
        } else {
            "Glad that helped. Tell me if you need anything else."
        }
    } else {
        return None;
    };
    let mut plan = Plan::single(
        Op::Answer {
            question: text.trim().to_string(),
            cite: Some(false),
            knowledge: Some(false),
            mode: Some(AnswerMode::Chat),
            reply: Some(reply.to_string()),
        },
        1.0,
        if ko { "인사에 답합니다" } else { "Replies to the greeting" },
    );
    plan.requires_confirmation = false;
    Some(plan)
}

// ---------------------------------------------------------------------------
// Settings sentences
// ---------------------------------------------------------------------------

/// Words that make a sentence a question or a report — never a settings command.
const QUESTION_MARKS: &[&str] = &[
    "?",
    "？",
    "뭐야",
    "뭐지",
    "뭔가",
    "뭐예요",
    "뭐에요",
    "무엇",
    "무슨",
    "알려",
    "어떻게",
    "얼마",
    "어디",
    "왜",
    "인가요",
    "인지",
    "인가",
    "했어",
    "했다",
    "했습니다",
    "됐어",
    "되나",
    "될까",
    "있어",
    "있나",
    "what",
    "how",
    "why",
    "where",
    "which",
    "does",
    "is it",
    "can i",
];

/// Particles that may surround the name and the value.
const PARTICLES: &[&str] =
    &["으로", "로", "을", "를", "은", "는", "이", "가", "에", "의", "도", "만", "좀", "값", "쪽", "모드"];
/// Stems of the verbs (and requests) that mean "change it": the rest of such a word is its ending.
const CHANGE_STEMS: &[&str] = &[
    "바꿔",
    "바꾸",
    "변경",
    "설정",
    "적용",
    "맞춰",
    "맞추",
    "세팅",
    "올려",
    "올리",
    "내려",
    "내리",
    "낮춰",
    "낮추",
    "높여",
    "높이",
    "키워",
    "키우",
    "줄여",
    "줄이",
    "해",
    "하자",
    "하게",
    "하기",
    "하도록",
    "함",
    "합",
    "줘",
    "주세요",
    "부탁",
    "set",
    "change",
    "switch",
    "make",
    "use",
    "please",
    "to",
    "the",
    "it",
    "as",
];
/// Units that may follow a number.
const UNITS: &[&str] = &[
    "퍼센트",
    "프로",
    "기가바이트",
    "기가",
    "gb",
    "dpi",
    "토큰",
    "페이지",
    "초",
    "분",
    "일",
    "개",
    "쪽",
    "장",
    "px",
    "pt",
];
const ON_WORDS: &[&str] = &["켜", "켬", "on", "사용", "활성", "허용", "enable", "true"];
const OFF_WORDS: &[&str] = &["꺼", "끄", "끔", "off", "해제", "비활성", "금지", "disable", "false", "중지"];
const NEGATIONS: &[&str] = &["안", "않", "no", "not"];

fn is_question(p: &str) -> bool {
    QUESTION_MARKS.iter().any(|q| p.contains(q))
}

fn is_letter(c: char) -> bool {
    c.is_alphanumeric()
}

/// Finds `name` in `text` (both lowercase): a space in the name matches any run of spaces (or
/// none), and the match must start at a word start. Returns the char range.
fn find_name(text: &[char], name: &str) -> Option<(usize, usize)> {
    let name: Vec<char> = name.chars().collect();
    'start: for start in 0..text.len() {
        if start > 0 && is_letter(text[start - 1]) && is_letter(text[start]) {
            continue;
        }
        let mut i = start;
        for &nc in &name {
            if nc == ' ' {
                while i < text.len() && text[i] == ' ' {
                    i += 1;
                }
                continue;
            }
            if i >= text.len() || text[i] != nc {
                continue 'start;
            }
            i += 1;
        }
        if i > start {
            return Some((start, i));
        }
    }
    None
}

/// `name` is mentioned in `text` (case-insensitive, spaces flexible, at a word start).
pub(super) fn mentions(text: &str, name: &str) -> bool {
    let chars: Vec<char> = plain(text).chars().collect();
    find_name(&chars, &name.to_lowercase()).is_some()
}

/// The numbers written in `text`.
pub(super) fn numbers_in(text: &str) -> Vec<f64> {
    lex(&plain(text))
        .into_iter()
        .filter_map(|t| match t {
            Tok::Num(n) => Some(n),
            _ => None,
        })
        .collect()
}

/// `Some(true)` / `Some(false)` when the sentence says on / off (켜 · 꺼 · 사용 안 함 …).
pub(super) fn polarity(text: &str) -> Option<bool> {
    let (mut on, mut off, mut negated) = (false, false, false);
    for tok in lex(&plain(text)) {
        if let Tok::Word(w) = tok {
            let w = w.as_str();
            if OFF_WORDS.iter().any(|x| w.starts_with(x)) {
                off = true;
            } else if ON_WORDS.iter().any(|x| w.starts_with(x)) {
                on = true;
            } else if NEGATIONS.iter().any(|x| w == *x || (w.starts_with(x) && w.chars().count() <= 3)) {
                negated = true;
            }
        }
    }
    match (on, off, negated) {
        (true, false, false) => Some(true),
        (false, true, _) | (true, false, true) => Some(false),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Word(String),
    Percent,
}

/// Numbers ("1,200", "0.6"), letter runs and `%`; everything else separates tokens.
fn lex(s: &str) -> Vec<Tok> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            let mut raw = String::new();
            while i < chars.len() {
                let d = chars[i];
                let next_digit = chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
                if d.is_ascii_digit() {
                    raw.push(d);
                } else if d == '.' && next_digit {
                    raw.push('.');
                } else if d == ',' && next_digit {
                    // thousands separator
                } else {
                    break;
                }
                i += 1;
            }
            if let Ok(n) = raw.parse::<f64>() {
                out.push(Tok::Num(n));
            }
        } else if c == '%' {
            out.push(Tok::Percent);
            i += 1;
        } else if is_letter(c) || c == '_' {
            let mut w = String::new();
            while i < chars.len()
                && (is_letter(chars[i]) || chars[i] == '_' || chars[i] == '-')
                && !chars[i].is_ascii_digit()
            {
                w.push(chars[i]);
                i += 1;
            }
            if w.is_empty() {
                i += 1;
            } else {
                out.push(Tok::Word(w));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// A word made only of particles, optionally ending in a change verb ("으로변경해줘", "로", "바꿔줄래").
fn is_filler(word: &str) -> bool {
    let mut rest = word;
    loop {
        if rest.is_empty() || CHANGE_STEMS.iter().any(|s| rest.starts_with(s)) {
            return true;
        }
        match PARTICLES.iter().find(|p| rest.starts_with(**p)) {
            Some(p) => rest = &rest[p.len()..],
            None => return false,
        }
    }
}

fn has_change_verb(tokens: &[Tok]) -> bool {
    tokens.iter().any(|t| match t {
        Tok::Word(w) => {
            let mut rest = w.as_str();
            loop {
                if CHANGE_STEMS.iter().any(|s| rest.starts_with(s)) {
                    return true;
                }
                match PARTICLES.iter().find(|p| rest.starts_with(**p)) {
                    Some(p) if rest.len() > p.len() => rest = &rest[p.len()..],
                    _ => return false,
                }
            }
        }
        _ => false,
    })
}

/// A word after a number: a unit, then fillers ("퍼센트로", "gb", "초로바꿔").
fn is_unit_then_filler(word: &str) -> bool {
    UNITS.iter().any(|u| word.strip_prefix(u).is_some_and(is_filler))
}

/// The setting the sentence names: the longest name found; `None` when two different settings
/// are named side by side.
fn named_setting(text: &[char]) -> Option<(&'static SettingSpec, (usize, usize))> {
    let mut best: Option<(&'static SettingSpec, (usize, usize))> = None;
    let mut all: Vec<(&'static SettingSpec, (usize, usize))> = Vec::new();
    for spec in catalog::SETTINGS {
        // `names_of` is sorted longest first: the first hit is this setting's best match.
        if let Some(span) = catalog::names_of(spec).iter().find_map(|n| find_name(text, n)) {
            all.push((spec, span));
            let longer = best.is_none_or(|(_, b)| span.1 - span.0 > b.1 - b.0);
            if longer {
                best = Some((spec, span));
            }
        }
    }
    let (spec, span) = best?;
    // Another setting named outside the chosen span makes the sentence ambiguous.
    let overlaps = |s: (usize, usize)| s.0 < span.1 && span.0 < s.1;
    if all.iter().any(|(other, s)| other.key != spec.key && !overlaps(*s)) {
        return None;
    }
    Some((spec, span))
}

enum Reading {
    Value(Value),
    /// The name with a change verb but no value ("배율 바꿔줘").
    Missing,
}

fn read_value(spec: &SettingSpec, rest: &str) -> Option<Reading> {
    let mut rest = rest.to_string();
    // Enum values are phrases: take the value's name out of the text first.
    let mut enum_value: Option<&'static str> = None;
    if let ValueType::Enum(values) = spec.ty {
        let chars: Vec<char> = rest.chars().collect();
        let mut hits: Vec<(&'static str, (usize, usize))> = Vec::new();
        for value in values {
            let mut names: Vec<String> = vec![value.to_string(), value.replace('_', " ")];
            if let Some((_, extra)) = catalog::value_names(spec.key).iter().find(|(v, _)| v == value) {
                names.extend(extra.iter().map(|n| n.to_string()));
            }
            names.sort_by_key(|n| std::cmp::Reverse(n.chars().count()));
            if let Some(span) = names.iter().find_map(|n| find_name(&chars, n)) {
                hits.push((value, span));
            }
        }
        // A longer value name that contains a shorter one wins ("high contrast" over nothing else).
        hits.sort_by_key(|(_, s)| std::cmp::Reverse(s.1 - s.0));
        if let Some((value, span)) = hits.first().copied() {
            if hits.iter().any(|(v, s)| *v != value && !(s.0 < span.1 && span.0 < s.1)) {
                return None;
            }
            enum_value = Some(value);
            rest = chars[..span.0].iter().chain([' '].iter()).chain(chars[span.1..].iter()).collect();
        }
    }
    let tokens = lex(&rest);
    let mut numbers: Vec<f64> = Vec::new();
    let (mut on, mut off, mut negated) = (false, false, false);
    let mut after_number = false;
    for tok in &tokens {
        match tok {
            Tok::Num(n) => {
                numbers.push(*n);
                after_number = true;
                continue;
            }
            Tok::Percent => {}
            Tok::Word(w) => {
                let w = w.as_str();
                let bool_setting = matches!(spec.ty, ValueType::Bool);
                if after_number && is_unit_then_filler(w) {
                } else if bool_setting && OFF_WORDS.iter().any(|x| w.starts_with(x)) {
                    off = true;
                } else if bool_setting && ON_WORDS.iter().any(|x| w.starts_with(x)) {
                    on = true;
                } else if bool_setting && NEGATIONS.iter().any(|x| w.starts_with(x)) {
                    negated = true;
                } else if !is_filler(w) {
                    return None;
                }
            }
        }
        after_number = false;
    }
    let value = match spec.ty {
        ValueType::Bool => {
            if !numbers.is_empty() {
                return None;
            }
            match (on, off, negated) {
                (true, false, false) => Some(Value::Bool(true)),
                (false, true, false) | (true, false, true) => Some(Value::Bool(false)),
                (false, false, false) => None,
                _ => return None,
            }
        }
        ValueType::Int { .. } | ValueType::IntChoice(_) | ValueType::Float { .. } => match numbers.as_slice() {
            [] => None,
            [n] => Some(serde_json::json!(n)),
            _ => return None,
        },
        ValueType::Enum(_) => {
            if !numbers.is_empty() {
                return None;
            }
            enum_value.map(|v| Value::String(v.to_string()))
        }
        // Model ids and paths are the model's to read.
        ValueType::Str => return None,
    };
    match value {
        Some(v) => Some(Reading::Value(v)),
        None if has_change_verb(&tokens) => Some(Reading::Missing),
        None => None,
    }
}

fn label(spec: &SettingSpec, ko: bool) -> &'static str {
    if ko {
        spec.label_ko
    } else {
        spec.label_en
    }
}

fn setting_op(spec: &SettingSpec, value: Value) -> Op {
    let text = value.as_str().unwrap_or_default();
    match spec.key {
        "ui.theme" => Op::SetTheme {
            theme: match text {
                "light" => Theme::Light,
                "dark" => Theme::Dark,
                "high-contrast" => Theme::HighContrast,
                _ => Theme::System,
            },
        },
        "ui.language" => Op::SetLanguage { lang: if text == "en" { Language::En } else { Language::Ko } },
        "models.profile" => Op::SetProfile {
            profile: match text {
                "balanced" => Profile::Balanced,
                "speed" => Profile::Speed,
                "fine_print" => Profile::FinePrint,
                _ => Profile::Accuracy,
            },
        },
        // The typed op of the inbox switch predates the catalog; the executor treats both alike.
        "processing.inbox_watch" => Op::SetInboxWatch { enabled: value.as_bool().unwrap_or(false) },
        _ => Op::SetSetting { key: spec.key.to_string(), value },
    }
}

fn shown(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "on".into(),
        Value::Bool(false) => "off".into(),
        other => other.to_string(),
    }
}

fn setting(text: &str, ko: bool) -> Option<Plan> {
    let p = plain(text);
    if p.is_empty() || p.chars().count() > 60 || is_question(&p) {
        return None;
    }
    let chars: Vec<char> = p.chars().collect();
    let (spec, span) = named_setting(&chars)?;
    let rest: String = chars[..span.0].iter().chain([' '].iter()).chain(chars[span.1..].iter()).collect();
    match read_value(spec, &rest)? {
        Reading::Value(mut value) => match catalog::coerce(spec, &mut value) {
            Ok(()) => {
                let summary = format!("{} → {}", label(spec, ko), shown(&value));
                let op = setting_op(spec, value);
                let mut plan = Plan::single(op, 1.0, summary);
                plan.requires_confirmation = catalog::needs_confirmation(spec.key);
                Some(plan)
            }
            // A value the setting cannot take is not applied; say what it accepts.
            Err(_) => Some(Plan::clarify(
                if ko {
                    format!(
                        "{} 값은 {} 범위에서 고를 수 있습니다. 그 안의 값으로 다시 말씀해 주세요.",
                        spec.label_ko,
                        catalog::range_hint(spec, true)
                    )
                } else {
                    format!(
                        "{} accepts {}. Please give a value in that range.",
                        spec.label_en,
                        catalog::range_hint(spec, false)
                    )
                },
                1.0,
            )),
        },
        Reading::Missing => Some(Plan::clarify(
            if ko {
                format!("{} — 바꿀 값을 함께 말씀해 주세요({}).", spec.label_ko, catalog::range_hint(spec, true))
            } else {
                format!("{} — tell me the value to set ({}).", spec.label_en, catalog::range_hint(spec, false))
            },
            1.0,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> PlanContext {
        PlanContext::default()
    }

    fn op_of(text: &str) -> Option<Op> {
        plan(text, &ctx()).map(|p| p.operations[0].clone())
    }

    fn setting_of(text: &str) -> Option<(String, Value)> {
        match op_of(text)? {
            Op::SetSetting { key, value } => Some((key, value)),
            other => panic!("{text}: expected set_setting, got {other:?}"),
        }
    }

    #[test]
    fn the_sentences_of_the_user_report_are_read() {
        // 2026-11-11 report: these ran "프로파일: 균형" / "테마 → 시스템" through the model.
        assert_eq!(setting_of("UI 배율 120%"), Some(("ui.scale_percent".into(), json!(120))));
        assert_eq!(setting_of("배율을 130 으로 변경해줘"), Some(("ui.scale_percent".into(), json!(130))));
        assert_eq!(setting_of("DPI 를 200 으로 변경해줘"), Some(("processing.render_dpi".into(), json!(200))));
        assert_eq!(op_of("프로파일 정확도"), Some(Op::SetProfile { profile: Profile::Accuracy }));
        // … and the short forms the rule mode missed (audit USR-2).
        assert_eq!(setting_of("화면 배율 150"), Some(("ui.scale_percent".into(), json!(150))));
        assert_eq!(setting_of("렌더링 해상도 300으로"), Some(("processing.render_dpi".into(), json!(300))));
        assert_eq!(setting_of("글자 크기 110"), Some(("ui.scale_percent".into(), json!(110))));
    }

    #[test]
    fn names_values_and_word_order_vary() {
        assert_eq!(setting_of("ui배율을 100%로 바꿔줘"), Some(("ui.scale_percent".into(), json!(100))));
        assert_eq!(setting_of("120으로 ui 배율 변경"), Some(("ui.scale_percent".into(), json!(120))));
        assert_eq!(setting_of("ui scale 140"), Some(("ui.scale_percent".into(), json!(140))));
        assert_eq!(setting_of("set the ui scale to 110"), Some(("ui.scale_percent".into(), json!(110))));
        assert_eq!(op_of("다크 모드로 바꿔줘"), Some(Op::SetTheme { theme: Theme::Dark }));
        assert_eq!(op_of("테마 라이트"), Some(Op::SetTheme { theme: Theme::Light }));
        assert_eq!(op_of("테마를 고대비로"), Some(Op::SetTheme { theme: Theme::HighContrast }));
        assert_eq!(op_of("언어 영어로 바꿔줘"), Some(Op::SetLanguage { lang: Language::En }));
        assert_eq!(op_of("프로파일을 속도로"), Some(Op::SetProfile { profile: Profile::Speed }));
        assert_eq!(setting_of("자동 임베딩 꺼줘"), Some(("knowledge.auto_embed".into(), json!(false))));
        assert_eq!(op_of("inbox 감시 켜"), Some(Op::SetInboxWatch { enabled: true }));
        assert_eq!(op_of("inbox 감시 꺼"), Some(Op::SetInboxWatch { enabled: false }));
        assert_eq!(setting_of("개인정보 마스킹 사용 안 함"), Some(("privacy.pii_masking".into(), json!(false))));
        assert_eq!(setting_of("검토 임계값 80%"), Some(("processing.review_threshold".into(), json!(0.8))));
        assert_eq!(setting_of("캐시 상한 20gb로"), Some(("cache.page_cache_max_gb".into(), json!(20.0))));
        assert_eq!(setting_of("근거 개수 12"), Some(("knowledge.top_k".into(), json!(12))));
        assert_eq!(setting_of("자동 처리 분류만"), Some(("processing.auto_level".into(), json!("classify_only"))));
    }

    #[test]
    fn anything_else_is_left_to_the_model() {
        for text in [
            // questions and reports
            "UI 배율이 뭐야?",
            "배율 얼마야",
            "DPI 200으로 바꿨어",
            "테마 어떻게 바꿔",
            // more than a setting and its value
            "배율 130으로 바꾸고 견적서 요약해줘",
            "이 문서 해상도 300으로 다시 렌더해줘",
            "계약서에서 마스킹 된 부분 찾아줘",
            "지난달 견적서 합계",
            "프로젝트 별 내용을 요약해줘",
            "다크 테마 문서 찾아줘",
            // two settings, or a value of another type
            "배율 120 dpi 200",
            "배율 다크",
            "테마 120",
            "자동 임베딩 120",
            // a bare name is not a command
            "배율",
            "알림",
            // relative changes need the current value
            "배율 키워줘 조금만 더 크게",
            // free text values (model ids, paths)
            "활성 모델 gemma4-12b-q4km",
        ] {
            let got = plan(text, &ctx());
            assert!(
                got.as_ref().is_none_or(|p| p.is_clarification() && text.contains("키워줘")),
                "{text} must not be applied by the fast path: {got:?}"
            );
        }
    }

    #[test]
    fn out_of_range_values_are_not_applied() {
        for text in ["UI 배율 200", "DPI 250으로 변경해줘", "근거 개수 500"] {
            let p = plan(text, &ctx()).unwrap_or_else(|| panic!("{text}"));
            assert!(p.is_clarification(), "{text}: {p:?}");
        }
        let p = plan("UI 배율 200", &ctx()).unwrap();
        assert!(p.clarification_question.as_deref().unwrap().contains("90~150"), "{p:?}");
        // The name with a change verb but no value asks for the value.
        let p = plan("배율 바꿔줘", &ctx()).unwrap();
        assert!(p.is_clarification() && p.clarification_question.as_deref().unwrap().contains("90~150"), "{p:?}");
    }

    #[test]
    fn confirmed_settings_stay_confirmed() {
        let p = plan("db 암호화 켜줘", &ctx()).unwrap();
        assert!(p.requires_confirmation, "{p:?}");
        let p = plan("ui 배율 120", &ctx()).unwrap();
        assert!(!p.requires_confirmation);
        assert_eq!(p.confidence, 1.0);
        assert_eq!(p.summary.as_deref(), Some("UI 배율(%) → 120"));
    }

    #[test]
    fn fixed_greetings_get_a_ready_reply() {
        for text in ["안녕하세요", "안녕하세요.", "안녕!", "고마워~", "Hello", "thanks!"] {
            let p = plan(text, &ctx()).unwrap_or_else(|| panic!("{text}"));
            match &p.operations[0] {
                Op::Answer { mode: Some(AnswerMode::Chat), reply: Some(r), knowledge: Some(false), .. } => {
                    assert!(!r.is_empty())
                }
                other => panic!("{text}: {other:?}"),
            }
            assert!(!p.requires_confirmation);
        }
        // A greeting followed by a request is the model's.
        assert!(plan("안녕하세요, 견적서 합계 알려주세요", &ctx()).is_none());
        assert!(plan("안녕 못한 하루", &ctx()).is_none());
    }
}
