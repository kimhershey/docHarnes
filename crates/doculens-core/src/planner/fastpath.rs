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
    // A reply to the assistant's question arrives as "command / question / reply" (`askBackText`
    // in the UI). Only the user's words count, and a reply that is the value the question asked
    // for ("배율 바꿔줘" → "값을 알려 주세요" → "120") completes the setting.
    let words = super::rules::user_words(text);
    if words != text {
        let (command, reply) = words.rsplit_once('\n')?;
        // Only the fast path's own question is answered here ("which value?", "that value is out of
        // range"): the command alone must already be a settings sentence without a usable value. A
        // reply to any other question is the model's — "이 문서를 다른 언어로 번역해줘" → "어떤 언어로
        // 번역할까요?" → "영어" names a setting ("언어") and a value of it, and is not a settings command.
        if !setting(command, ko).is_some_and(|p| p.is_clarification()) {
            return None;
        }
        if let Some(p) = setting(&words, ko).filter(|p| !p.is_clarification()) {
            return Some(p);
        }
        // The command already carried a value that was refused ("UI 배율 200" → "150").
        let chars: Vec<char> = plain(command).chars().collect();
        let (_, span) = named_setting(&chars)?;
        let name: String = chars[span.0..span.1].iter().collect();
        return setting(&format!("{name} {reply}"), ko);
    }
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
    "thanks",
    "thank you",
    "thanks a lot",
    "thx",
];
// ("좋아요", "great" and the like are not here: they also answer a question the assistant asked.)

/// Whether the words are greeting / thanks phrases and nothing else ("고마워, 수고했어"), and
/// whether one of them is a thanks. Phrases are up to three words ("thanks a lot").
fn only_pleasantries(words: &[&str]) -> Option<bool> {
    if words.is_empty() || words.len() > 6 {
        return None;
    }
    let (mut i, mut thanks) = (0, false);
    while i < words.len() {
        let step = (1..=3.min(words.len() - i)).rev().find_map(|n| {
            let phrase = words[i..i + n].join(" ");
            if THANKS.contains(&phrase.as_str()) {
                Some((n, true))
            } else if GREETINGS.contains(&phrase.as_str()) {
                Some((n, false))
            } else {
                None
            }
        });
        let (n, is_thanks) = step?;
        thanks |= is_thanks;
        i += n;
    }
    Some(thanks)
}

/// Lowercase, sentence punctuation dropped, whitespace collapsed. A `.` or `,` between two digits
/// belongs to the number ("0.6", "8,000") and stays.
fn plain(text: &str) -> String {
    let chars: Vec<char> = text.trim().to_lowercase().chars().collect();
    let digit = |i: usize| chars.get(i).is_some_and(|c| c.is_ascii_digit());
    let cleaned: String = chars
        .iter()
        .enumerate()
        .map(|(i, &c)| match c {
            '.' | ',' if i > 0 && digit(i - 1) && digit(i + 1) => c,
            '.' | '!' | '~' | ',' | '^' | '♡' | '♥' => ' ',
            _ => c,
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn greeting(text: &str, ko: bool) -> Option<Plan> {
    let p = plain(text);
    let thanks = only_pleasantries(&p.split(' ').collect::<Vec<_>>())?;
    let reply = match (thanks, ko) {
        (false, true) => "안녕하세요! 문서에 대해 물어보시거나, 가져오기·정리·추출·요약 같은 작업을 말씀해 주세요.",
        (false, false) => {
            "Hello! Ask me about your documents, or tell me what to do — import, organise, extract, summarise."
        }
        (true, true) => "도움이 되어 다행입니다. 더 필요한 것이 있으면 말씀해 주세요.",
        (true, false) => "Glad that helped. Tell me if you need anything else.",
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

// A sentence is applied only when every word of it is recognised as a whole. Words are never
// matched by their beginning: "끄지마" starts like "끄기" and means the opposite, "해제" starts
// like "해줘", "now" starts like "no" (0.8.0 review).

/// Particles that may surround the name and the value.
const PARTICLES: &[&str] =
    &["으로", "로", "을", "를", "은", "는", "이", "가", "에", "의", "도", "좀", "값", "쪽", "모드"];
/// Change verbs that say nothing about a direction, as noun + 하다 …
const SET_NOUNS: &[&str] = &["변경", "설정", "적용", "세팅", "지정"];
/// … and in their 아/어 form.
const SET_VERBS: &[&str] = &["바꿔", "맞춰", "해"];
/// Verbs that move a value. With a number they are absolute only after "(으)로" ("120으로 올려줘");
/// "10% 올려줘" is an amount to add, which needs the current value.
const MOVE_VERBS: &[&str] = &["올려", "내려", "낮춰", "높여", "키워", "줄여", "늘려"];
/// Endings of a noun + 하다 verb, all of them requests or plain statements of intent.
const HADA_ENDINGS: &[&str] = &[
    "",
    "해",
    "해줘",
    "해줘요",
    "해주세요",
    "해줄래",
    "해줄래요",
    "해라",
    "해요",
    "하자",
    "하세요",
    "합니다",
    "함",
    "하기",
    "할게",
    "해봐",
    "해주라",
    "하도록",
];
/// Endings of an 아/어-form verb.
const VERB_ENDINGS: &[&str] =
    &["", "줘", "줘요", "주세요", "줄래", "줄래요", "주라", "라", "요", "봐", "놔", "놔줘", "둬", "둬줘", "고"];
/// Whole words that only ask.
const REQUEST_WORDS: &[&str] = &["줘", "주세요", "부탁", "부탁해", "부탁해요", "부탁합니다", "부탁드립니다"];
/// English words around a setting and its value.
const FILLERS_EN: &[&str] =
    &["set", "change", "switch", "make", "use", "turn", "please", "to", "the", "it", "as", "my"];

/// On / off for a boolean setting: noun + 하다, 아/어-form verbs and whole words.
const ON_NOUNS: &[&str] = &["사용", "활성화", "활성", "허용"];
const ON_VERBS: &[&str] = &["켜"];
const ON_WORDS: &[&str] = &["켬", "켜기", "켜짐", "on", "enable", "enabled", "true"];
const OFF_NOUNS: &[&str] = &["해제", "비활성화", "비활성", "금지", "중지"];
const OFF_VERBS: &[&str] = &["꺼"];
const OFF_WORDS: &[&str] = &["끔", "끄기", "꺼짐", "off", "disable", "disabled", "false"];
/// "사용 안 함": the words that may follow an on-noun to switch it off.
const NOT_DOING: &[&str] = &["안함", "안해", "안해요", "안합니다", "안해줘"];
const NOT: &str = "안";
const DOING: &[&str] = &["함", "해", "해요", "합니다", "해줘"];

fn is_question(p: &str) -> bool {
    QUESTION_MARKS.iter().any(|q| p.contains(q))
}

/// Whether the user's words read as a question or a report ("…인가요?", "…로 바꿨어") rather than a
/// request — the test the fast path applies before it reads a setting, for the guard.
pub(super) fn reads_as_question(text: &str) -> bool {
    let p = plain(text);
    is_question(&p) || ends_in_past_tense(&p)
}

/// "…로 바꿨어", "…했습니다", "…됐네요": the sentence ends in a past-tense form — the user says what
/// happened. (A syllable with the final consonant ㅆ before a sentence ending; "있어" and "겠어" share
/// the shape and are not requests either: "…할 수 있어?", "…하면 좋겠어".)
fn ends_in_past_tense(p: &str) -> bool {
    const ENDINGS: &[&str] = &["어", "어요", "다", "습니다", "네", "네요", "지", "죠", "는데", "거든", "잖아"];
    let Some(last) = p.split_whitespace().last() else { return false };
    ENDINGS.iter().any(|e| {
        last.strip_suffix(e).and_then(|stem| stem.chars().last()).is_some_and(|c| {
            // Hangul syllables: 0xAC00 + (initial * 21 + vowel) * 28 + final; final 20 = ㅆ.
            ('가'..='힣').contains(&c) && (c as u32 - 0xAC00) % 28 == 20
        })
    })
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

/// What one word says, when the whole word is recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Word {
    /// Particles, a neutral change verb, a request: nothing but glue.
    Glue {
        verb: bool,
    },
    /// A verb that moves a value (올려줘 · 줄여줘).
    Move,
    On,
    /// An on-noun on its own ("사용"), which a following "안 함" turns off.
    OnNoun,
    Off,
    /// "안": only meaningful between an on-noun and 함 / 해.
    Not,
    /// "함" · "해" after "안".
    Doing,
    /// "안함" · "안해" as one word.
    NotDoing,
}

fn stem_with_ending(word: &str, stems: &[&str], endings: &[&str]) -> bool {
    stems.iter().any(|stem| word.strip_prefix(stem).is_some_and(|rest| endings.contains(&rest)))
}

/// Reads a word as a whole (after any leading particles); `None` when a part of it is unknown.
fn word_kind(word: &str) -> Option<Word> {
    let mut rest = word;
    loop {
        if rest.is_empty() {
            return Some(Word::Glue { verb: false });
        }
        if let Some(kind) = bare_word_kind(rest) {
            return Some(kind);
        }
        let particle = PARTICLES.iter().find(|p| rest.starts_with(**p))?;
        rest = &rest[particle.len()..];
    }
}

fn bare_word_kind(word: &str) -> Option<Word> {
    if word == NOT {
        return Some(Word::Not);
    }
    if NOT_DOING.contains(&word) {
        return Some(Word::NotDoing);
    }
    if ON_WORDS.contains(&word) || stem_with_ending(word, ON_VERBS, VERB_ENDINGS) {
        return Some(Word::On);
    }
    if OFF_WORDS.contains(&word)
        || stem_with_ending(word, OFF_VERBS, VERB_ENDINGS)
        || stem_with_ending(word, OFF_NOUNS, HADA_ENDINGS)
    {
        return Some(Word::Off);
    }
    if ON_NOUNS.contains(&word) {
        return Some(Word::OnNoun);
    }
    if stem_with_ending(word, ON_NOUNS, HADA_ENDINGS) {
        return Some(Word::On);
    }
    // "사용안함" written as one word.
    if ON_NOUNS.iter().any(|n| word.strip_prefix(n).is_some_and(|rest| NOT_DOING.contains(&rest))) {
        return Some(Word::Off);
    }
    if stem_with_ending(word, MOVE_VERBS, VERB_ENDINGS) {
        return Some(Word::Move);
    }
    if stem_with_ending(word, SET_NOUNS, HADA_ENDINGS) || stem_with_ending(word, SET_VERBS, VERB_ENDINGS) {
        return Some(Word::Glue { verb: true });
    }
    if REQUEST_WORDS.contains(&word) || FILLERS_EN.contains(&word) {
        return Some(Word::Glue { verb: false });
    }
    // "함" · "해요" after "안" (as a neutral verb "해" + ending they were caught above).
    if DOING.contains(&word) {
        return Some(Word::Doing);
    }
    None
}

/// On / off said by a run of recognised words: `Ok(None)` when none of them says either, `Err`
/// when they contradict each other or a "안" stands where it means something else.
fn switch_of(words: &[Word]) -> std::result::Result<Option<bool>, ()> {
    let mut said: Option<bool> = None;
    let say = |v: bool, said: &mut Option<bool>| match *said {
        Some(old) if old != v => Err(()),
        _ => {
            *said = Some(v);
            Ok(())
        }
    };
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            Word::On => say(true, &mut said)?,
            Word::Off => say(false, &mut said)?,
            Word::OnNoun => match (words.get(i + 1), words.get(i + 2)) {
                (Some(Word::NotDoing), _) => {
                    say(false, &mut said)?;
                    i += 1;
                }
                (Some(Word::Not), Some(Word::Doing | Word::Glue { verb: true })) => {
                    say(false, &mut said)?;
                    i += 2;
                }
                _ => say(true, &mut said)?,
            },
            Word::Not | Word::NotDoing => return Err(()),
            Word::Doing | Word::Glue { .. } | Word::Move => {}
        }
        i += 1;
    }
    Ok(said)
}

/// `Some(true)` / `Some(false)` when the sentence says on / off in so many words (켜줘 · 꺼 ·
/// 사용 안 함); `None` when it says neither, both, or anything negated ("끄지 마세요").
pub(super) fn polarity(text: &str) -> Option<bool> {
    let plain = plain(text);
    if is_refusal(&plain) {
        return None;
    }
    let words: Vec<Word> = lex(&plain)
        .into_iter()
        .filter_map(|t| match t {
            Tok::Word(w) => word_kind(&w),
            _ => None,
        })
        .collect();
    switch_of(&words).ok().flatten()
}

/// The sentence asks not to do something ("끄지 마", "바꾸지 말고", "don't").
fn is_refusal(plain: &str) -> bool {
    let compact: String = plain.chars().filter(|c| !c.is_whitespace()).collect();
    ["지마", "지말", "지않", "하지는", "않게", "않도록", "못하게", "안되게"].iter().any(|m| compact.contains(m))
        || plain.split(' ').any(|w| {
            ["don't", "dont", "don", "not", "never", "no", "마", "마세요", "마라", "말고", "말아줘"].contains(&w)
        })
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

/// Units a number may carry for a setting, with the factor to the setting's own unit. A unit the
/// setting does not have ("절전 대기 5분" for a field in seconds is fine, "캐시 상한 50%" is not)
/// leaves the sentence to the model.
fn units_of(key: &str) -> &'static [(&'static str, f64)] {
    match key {
        "ui.scale_percent" | "processing.audit_sample_percent" => &[("퍼센트", 1.0), ("프로", 1.0)],
        "processing.render_dpi" => &[("dpi", 1.0)],
        "models.sleep_idle_seconds" => {
            &[("시간", 3600.0), ("분", 60.0), ("초", 1.0), ("seconds", 1.0), ("sec", 1.0), ("s", 1.0)]
        }
        "cache.page_cache_max_gb" => &[("기가바이트", 1.0), ("기가", 1.0), ("gb", 1.0)],
        "knowledge.top_k" | "models.resident_models_max" | "processing.dense_cells" => &[("개", 1.0)],
        "processing.kie_max_pages" => &[("페이지", 1.0), ("쪽", 1.0), ("장", 1.0), ("pages", 1.0)],
        "knowledge.context_tokens"
        | "knowledge.chunk_target_tokens"
        | "generation.max_tokens_gpu"
        | "generation.max_tokens_cpu" => &[("토큰", 1.0), ("tokens", 1.0)],
        "journal_days" => &[("일", 1.0), ("days", 1.0)],
        _ => &[],
    }
}

/// What `%` after the number means for a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Percent {
    /// The setting is a percentage: "120%" is 120.
    Itself,
    /// The setting is a 0–1 ratio: "90%" is 0.9.
    Ratio,
    /// The setting is neither: "50%" of what?
    Meaningless,
}

fn percent_of(spec: &SettingSpec) -> Percent {
    match (spec.key, spec.ty) {
        ("ui.scale_percent" | "processing.audit_sample_percent", _) => Percent::Itself,
        (_, ValueType::Float { percent: true, .. }) => Percent::Ratio,
        _ => Percent::Meaningless,
    }
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
    if is_refusal(&rest) {
        return None;
    }
    // Every token has to be accounted for: numbers (with the setting's own unit), `%` where it
    // means something, and words that are recognised as a whole.
    let mut numbers: Vec<f64> = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    let (mut percent, mut absolute) = (false, false);
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Tok::Num(n) => {
                let mut value = *n;
                // What follows the number: `%`, then a word that may start with a unit and "(으)로".
                if matches!(tokens.get(i + 1), Some(Tok::Percent)) {
                    percent = true;
                    i += 1;
                }
                if let Some(Tok::Word(w)) = tokens.get(i + 1) {
                    let mut tail = w.as_str();
                    if let Some((unit, factor)) = units_of(spec.key).iter().find(|(u, _)| tail.starts_with(u)) {
                        value *= factor;
                        tail = &tail[unit.len()..];
                    }
                    if tail.starts_with("으로") || tail.starts_with("로") {
                        absolute = true;
                    }
                    if tail.len() < w.len() {
                        // The unit was taken off: the rest of the word must be glue.
                        words.push(word_kind(tail)?);
                        i += 1;
                    }
                }
                numbers.push(value);
            }
            Tok::Percent => return None,
            Tok::Word(w) => words.push(word_kind(w)?),
        }
        i += 1;
    }
    let moves = words.contains(&Word::Move);
    let has_verb = moves || words.iter().any(|w| matches!(w, Word::Glue { verb: true }));
    let switch = switch_of(&words).ok()?;
    let value = match spec.ty {
        ValueType::Bool => {
            if !numbers.is_empty() || moves {
                return None;
            }
            switch.map(Value::Bool)
        }
        ValueType::Int { .. } | ValueType::IntChoice(_) | ValueType::Float { .. } => {
            if switch.is_some() {
                return None;
            }
            match numbers.as_slice() {
                [] if percent => return None,
                [] => None,
                [n] => {
                    // "10% 올려줘" / "2000 올려줘": an amount to add, not the value.
                    if moves && !absolute {
                        return None;
                    }
                    let n = match (percent, percent_of(spec)) {
                        (false, Percent::Ratio) if *n >= 1.0 => return None, // "임계값 1": 1 % or 100 %?
                        (false, _) | (true, Percent::Itself) => *n,
                        (true, Percent::Ratio) => *n / 100.0,
                        (true, Percent::Meaningless) => return None,
                    };
                    Some(serde_json::json!(n))
                }
                _ => return None,
            }
        }
        ValueType::Enum(_) => {
            if !numbers.is_empty() || percent || moves || switch.is_some() {
                return None;
            }
            enum_value.map(|v| Value::String(v.to_string()))
        }
        // Model ids and paths are the model's to read.
        ValueType::Str => return None,
    };
    match value {
        Some(v) => Some(Reading::Value(v)),
        None if has_verb => Some(Reading::Missing),
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

    /// 0.8.0 review: words were matched by their beginning, so a negated or relative sentence was
    /// applied — and applied the wrong way round.
    #[test]
    fn negated_relative_and_foreign_unit_sentences_are_not_applied() {
        for text in [
            // the opposite of what a prefix match reads
            "개인정보 마스킹 끄지마",
            "개인정보 마스킹 끄지 마세요",
            "자동 임베딩 켜지 마",
            "배율 120으로 바꾸지마",
            "dpi 300으로 변경하지마",
            "다크 모드 해제",
            "enable masking now",
            "do not enable masking",
            "don't change the ui scale to 120",
            // an amount to add, not the value
            "검토 임계값 10% 올려줘",
            "토큰 상한 2000 올려줘",
            "저널 보관 30일 줄여줘",
            "배율 10 키워줘",
            // a unit the setting does not have, or a bare 1 for a ratio
            "캐시 상한 50%",
            "근거 개수 5%",
            "검토 임계값 1",
            // words that only start like a known one
            "마스킹 해상도",
            "배율 120 안내",
        ] {
            let got = plan(text, &ctx());
            assert!(got.as_ref().is_none_or(Plan::is_clarification), "{text} must not be applied: {got:?}");
        }
        // The same settings, said plainly.
        assert_eq!(setting_of("자동 임베딩 사용안함"), Some(("knowledge.auto_embed".into(), json!(false))));
        assert_eq!(setting_of("개인정보 마스킹 사용 안 해"), Some(("privacy.pii_masking".into(), json!(false))));
        assert_eq!(setting_of("개인정보 마스킹 사용"), Some(("privacy.pii_masking".into(), json!(true))));
        assert_eq!(setting_of("turn on pii masking"), Some(("privacy.pii_masking".into(), json!(true))));
        // 1.0.0: whether the assistant lists the operations it chose.
        let steps = |on: bool| Some(("ui.show_assistant_steps".to_string(), json!(on)));
        assert_eq!(setting_of("어시스턴트 작업 표시 꺼줘"), steps(false));
        assert_eq!(setting_of("어시스턴트 작업 표시 켜줘"), steps(true));
        assert_eq!(setting_of("고른 작업 표시 켜"), steps(true));
        assert_eq!(setting_of("작업 단계 표시 끄기"), steps(false));
        // A job notification is another setting; a sentence that is not a whole settings sentence goes to the model.
        assert_eq!(setting_of("작업 완료 알림 꺼줘").map(|(k, _)| k), Some("notifications.job_complete".to_string()));
        assert_eq!(setting_of("무슨 작업을 골랐는지 보여주지 마"), None);
        // 1.0.0: 프로젝트 분류(2순위 추천 임계값) — percent or ratio, under several names.
        let second = |v: f64| Some(("processing.project_second_choice".to_string(), json!(v)));
        assert_eq!(setting_of("프로젝트 분류 2순위 추천 임계값 70%로 바꿔줘"), second(0.7));
        assert_eq!(setting_of("2순위 추천 임계값 0.8로 설정"), second(0.8));
        assert_eq!(setting_of("2순위 임계값 55%"), second(0.55));
        assert_eq!(setting_of("프로젝트 분류 기준 90%로"), second(0.9));
        // The review threshold keeps its own name.
        assert_eq!(
            setting_of("검토 임계값 85%로 바꿔줘").map(|(k, _)| k),
            Some("processing.review_threshold".to_string())
        );
        assert_eq!(setting_of("배율 120으로 올려줘"), Some(("ui.scale_percent".into(), json!(120))));
        assert_eq!(setting_of("절전 대기 5분"), Some(("models.sleep_idle_seconds".into(), json!(300))));
        assert_eq!(setting_of("절전 대기 90초로"), Some(("models.sleep_idle_seconds".into(), json!(90))));
        assert_eq!(setting_of("검토 임계값 1%"), Some(("processing.review_threshold".into(), json!(0.01))));
        assert_eq!(setting_of("검토 임계값 0.6"), Some(("processing.review_threshold".into(), json!(0.6))));
        assert_eq!(setting_of("근거 컨텍스트 토큰 8,000"), Some(("knowledge.context_tokens".into(), json!(8000))));
        assert_eq!(setting_of("캐시 상한 50gb"), Some(("cache.page_cache_max_gb".into(), json!(50.0))));
        // On / off words mean what they say for the guard as well.
        assert_eq!(polarity("개인정보 마스킹은 꺼줘"), Some(false));
        assert_eq!(polarity("마스킹 사용 안 함"), Some(false));
        assert_eq!(polarity("마스킹 끄지 마세요"), None);
        assert_eq!(polarity("마스킹 켜고 알림은 꺼"), None);
        assert_eq!(numbers_in("임계값 0.6, 토큰 8,000"), vec![0.6, 8000.0]);
    }

    /// The reply to "which value?" finishes the command without the model.
    #[test]
    fn a_reply_to_the_assistants_question_completes_the_setting() {
        let ask = |command: &str, reply: &str| {
            format!("{command}\n\n[어시스턴트 질문] UI 배율(%) — 바꿀 값을 함께 말씀해 주세요(90~150).\n[사용자 답변] {reply}")
        };
        assert_eq!(setting_of(&ask("배율 바꿔줘", "120")), Some(("ui.scale_percent".into(), json!(120))));
        assert_eq!(setting_of(&ask("배율 바꿔줘", "120%로 해줘")), Some(("ui.scale_percent".into(), json!(120))));
        assert_eq!(setting_of(&ask("UI 배율 200", "150")), Some(("ui.scale_percent".into(), json!(150))));
        // Still out of range: asked again, not applied. The question's own "90~150" is never read as a value.
        assert!(plan(&ask("UI 배율 200", "180"), &ctx()).unwrap().is_clarification());
        assert!(plan(&ask("배율 바꿔줘", "글쎄 잘 모르겠어"), &ctx()).is_none());
        // A reply to a question about something else is the model's.
        let other = "선택한 문서 삭제해줘\n\n[어시스턴트 질문] 어떤 문서를 삭제할까요?\n[사용자 답변] 배율 120";
        assert!(plan(other, &ctx()).is_none());
        // … also when the command happens to contain a setting's name and the reply one of its values:
        // the question was the model's, about the translation and the rendering.
        let model_asked = |command: &str, question: &str, reply: &str| {
            format!("{command}\n\n[어시스턴트 질문] {question}\n[사용자 답변] {reply}")
        };
        for (command, question, reply) in [
            ("이 문서를 다른 언어로 번역해줘", "어떤 언어로 번역할까요?", "영어"),
            ("해상도 높여서 다시 렌더해줘", "해상도를 몇으로 할까요?", "300"),
            ("테마 색을 표에 맞춰 보고서 써줘", "어떤 테마로 쓸까요?", "다크"),
        ] {
            assert!(plan(&model_asked(command, question, reply), &ctx()).is_none(), "{command} / {reply}");
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
        // (the two settings that are confirmed — a path and a model — are text and never read here)
        assert!(plan("엔진 실행 파일 경로 C:\\llama\\llama-server.exe", &ctx()).is_none());
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
        // Several pleasantries in one turn are still only that.
        let p = plan("고마워, 수고했어", &ctx()).unwrap();
        assert!(matches!(&p.operations[0], Op::Answer { reply: Some(r), .. } if r.contains("다행")), "{p:?}");
        assert!(plan("Hello, thanks a lot!", &ctx()).is_some());
        // A greeting followed by a request is the model's, and so is a word that may answer a question.
        assert!(plan("안녕하세요, 견적서 합계 알려주세요", &ctx()).is_none());
        assert!(plan("고마워 그런데 이건 뭐야", &ctx()).is_none());
        assert!(plan("좋아요", &ctx()).is_none());
        assert!(plan("", &ctx()).is_none());
        assert!(plan("안녕 못한 하루", &ctx()).is_none());
    }
}
