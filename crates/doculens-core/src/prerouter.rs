//! Deterministic natural-language pre-router (design §3.5, stage 1).
//!
//! Handles what never needs the VLM planner: slash commands, unambiguous
//! settings sentences (theme / language / open screen / help) and relative
//! date phrases. Everything else is passed through (`matched == false`) so the
//! planner (phase 5) can produce a typed plan. Korean is the primary language;
//! English is also recognised.

use chrono::{Datelike, Days, Months, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::settings::{Language, Theme};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PreRoutedOp {
    SetTheme { theme: Theme },
    SetLanguage { language: Language },
    Open { screen: Screen },
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Screen {
    Home,
    Library,
    Search,
    Assistant,
    Review,
    Schemas,
    Knowledge,
    Generate,
    Projects,
    Organize,
    Jobs,
    Models,
    Settings,
}

impl Screen {
    pub const ALL: [Screen; 13] = [
        Screen::Home,
        Screen::Library,
        Screen::Search,
        Screen::Assistant,
        Screen::Review,
        Screen::Schemas,
        Screen::Knowledge,
        Screen::Generate,
        Screen::Projects,
        Screen::Organize,
        Screen::Jobs,
        Screen::Models,
        Screen::Settings,
    ];

    /// Route path used by the UI router.
    pub fn route(self) -> &'static str {
        match self {
            Screen::Home => "/",
            Screen::Library => "/library",
            Screen::Search => "/search",
            Screen::Assistant => "/assistant",
            Screen::Review => "/review",
            Screen::Schemas => "/schemas",
            Screen::Knowledge => "/knowledge",
            Screen::Generate => "/generate",
            Screen::Projects => "/projects",
            Screen::Organize => "/organize",
            Screen::Jobs => "/jobs",
            Screen::Models => "/models",
            Screen::Settings => "/settings",
        }
    }

    /// Korean and English names (lower-case, no spaces) that identify a screen.
    fn aliases(self) -> &'static [&'static str] {
        match self {
            Screen::Home => &["홈", "대시보드", "home", "dashboard", "메인"],
            Screen::Library => &["라이브러리", "문서라이브러리", "문서목록", "문서함", "library", "documents", "docs"],
            Screen::Search => &["검색", "search", "find"],
            Screen::Assistant => &["어시스턴트", "비서", "채팅", "대화", "assistant", "chat"],
            Screen::Review => &["검토", "검토큐", "검토대기", "review", "reviewqueue", "hitl"],
            Screen::Schemas => &["스키마", "추출스키마", "양식", "서식", "schemas", "schema", "forms", "form"],
            Screen::Knowledge => &["지식", "지식베이스", "지식창고", "knowledge", "knowledgebase", "kb", "rag"],
            Screen::Generate => &["문서생성", "생성", "작성", "generate", "draft", "compose"],
            Screen::Projects => &["프로젝트", "케이스", "projects", "project", "cases"],
            Screen::Organize => &["정리", "내보내기", "정리규칙", "organize", "organise", "export", "exports"],
            Screen::Jobs => &["작업", "작업목록", "잡", "jobs", "job", "tasks", "queue"],
            Screen::Models => &["모델", "모델관리자", "모델관리", "엔진", "models", "model", "engine"],
            Screen::Settings => {
                &["설정", "환경설정", "옵션", "settings", "setting", "preferences", "options", "config"]
            }
        }
    }

    pub fn parse(s: &str) -> Option<Screen> {
        let key: String =
            s.trim().to_lowercase().chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').collect();
        if key.is_empty() {
            return None;
        }
        // exact alias first, then suffix match for Korean particles ("설정을", "모델로")
        for sc in Screen::ALL {
            if sc.aliases().iter().any(|a| *a == key) {
                return Some(sc);
            }
        }
        let stripped = strip_korean_particles(&key);
        Screen::ALL.into_iter().find(|sc| sc.aliases().iter().any(|a| *a == stripped))
    }
}

const KO_PARTICLES: &[&str] = &["으로", "에서", "을", "를", "로", "은", "는", "이", "가", "에", "도"];

fn strip_korean_particles(s: &str) -> String {
    for p in KO_PARTICLES {
        if let Some(rest) = s.strip_suffix(p) {
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    s.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteSource {
    Slash,
    Phrase,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreRouteResult {
    pub matched: bool,
    pub source: RouteSource,
    #[serde(flatten)]
    pub op: Option<PreRoutedOp>,
    /// 1.0 for slash commands, 0.9 for phrase matches, 0.0 otherwise.
    pub confidence: f32,
    pub normalized_text: String,
}

impl PreRouteResult {
    fn none(text: &str) -> Self {
        Self {
            matched: false,
            source: RouteSource::None,
            op: None,
            confidence: 0.0,
            normalized_text: text.trim().to_string(),
        }
    }
    fn hit(op: PreRoutedOp, source: RouteSource, text: &str) -> Self {
        let confidence = if source == RouteSource::Slash { 1.0 } else { 0.9 };
        Self { matched: true, source, op: Some(op), confidence, normalized_text: text.trim().to_string() }
    }
}

const SLASH_THEME: &[&str] = &["/테마", "/theme"];
const SLASH_LANG: &[&str] = &["/언어", "/lang", "/language"];
const SLASH_OPEN: &[&str] = &["/열기", "/open", "/go", "/이동"];
const SLASH_HELP: &[&str] = &["/도움말", "/help", "/?"];

/// Routes `text`. Returns `matched == false` when the planner should handle it.
pub fn route(text: &str) -> PreRouteResult {
    let t = text.trim();
    if t.is_empty() {
        return PreRouteResult::none(text);
    }

    // 1) Slash commands: "/테마 다크", "/open settings", "/설정"
    if let Some(rest) = t.strip_prefix('/') {
        let mut parts = rest.splitn(2, char::is_whitespace);
        let cmd = format!("/{}", parts.next().unwrap_or("").trim().to_lowercase());
        let arg = parts.next().unwrap_or("").trim();
        if SLASH_HELP.contains(&cmd.as_str()) {
            return PreRouteResult::hit(PreRoutedOp::Help, RouteSource::Slash, text);
        }
        if SLASH_THEME.contains(&cmd.as_str()) {
            return match Theme::parse(arg) {
                Some(theme) => PreRouteResult::hit(PreRoutedOp::SetTheme { theme }, RouteSource::Slash, text),
                None => PreRouteResult::hit(PreRoutedOp::Open { screen: Screen::Settings }, RouteSource::Slash, text),
            };
        }
        if SLASH_LANG.contains(&cmd.as_str()) {
            return match Language::parse(arg) {
                Some(language) => PreRouteResult::hit(PreRoutedOp::SetLanguage { language }, RouteSource::Slash, text),
                None => PreRouteResult::hit(PreRoutedOp::Open { screen: Screen::Settings }, RouteSource::Slash, text),
            };
        }
        if SLASH_OPEN.contains(&cmd.as_str()) {
            if let Some(screen) = Screen::parse(arg) {
                return PreRouteResult::hit(PreRoutedOp::Open { screen }, RouteSource::Slash, text);
            }
            return PreRouteResult::none(text);
        }
        // "/설정", "/models" — bare screen names
        if let Some(screen) = Screen::parse(&cmd[1..]) {
            return PreRouteResult::hit(PreRoutedOp::Open { screen }, RouteSource::Slash, text);
        }
        return PreRouteResult::none(text);
    }

    let lower = t.to_lowercase();
    let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();

    // 2) Help
    if ["도움말", "뭘할수있", "무엇을할수있", "사용법", "help", "whatcanyoudo", "howdoiuse"]
        .iter()
        .any(|k| compact.contains(k))
        && compact.chars().count() <= 24
    {
        return PreRouteResult::hit(PreRoutedOp::Help, RouteSource::Phrase, text);
    }

    let has_change_verb = [
        "바꿔",
        "바꾸",
        "변경",
        "전환",
        "적용",
        "해줘",
        "해 줘",
        "켜",
        "설정해",
        "으로",
        "로 ",
        "switch",
        "change",
        "set ",
        "use ",
        "enable",
        "turn",
        "make",
    ]
    .iter()
    .any(|k| lower.contains(k));

    // 3) Theme phrases: "다크 모드로 바꿔", "switch to light theme", "고대비 켜줘"
    let theme_words: &[(&str, Theme)] = &[
        ("고대비", Theme::HighContrast),
        ("high-contrast", Theme::HighContrast),
        ("highcontrast", Theme::HighContrast),
        ("다크", Theme::Dark),
        ("어둡게", Theme::Dark),
        ("어두운", Theme::Dark),
        ("darkmode", Theme::Dark),
        ("darktheme", Theme::Dark),
        ("dark", Theme::Dark),
        ("라이트", Theme::Light),
        ("밝게", Theme::Light),
        ("밝은", Theme::Light),
        ("화이트", Theme::Light),
        ("lightmode", Theme::Light),
        ("lighttheme", Theme::Light),
        ("light", Theme::Light),
        ("시스템테마", Theme::System),
        ("시스템모드", Theme::System),
        ("systemtheme", Theme::System),
        ("systemmode", Theme::System),
    ];
    let mentions_theme = compact.contains("테마")
        || compact.contains("모드")
        || compact.contains("theme")
        || compact.contains("mode")
        || compact.contains("고대비")
        || compact.contains("contrast");
    if mentions_theme && (has_change_verb || compact.chars().count() <= 12) {
        for (word, theme) in theme_words {
            if compact.contains(word) {
                return PreRouteResult::hit(PreRoutedOp::SetTheme { theme: *theme }, RouteSource::Phrase, text);
            }
        }
    }

    // 4) Language phrases: "영어로 바꿔", "한국어로", "switch language to english", "use korean"
    let lang_words: &[(&str, Language)] = &[
        ("한국어", Language::Ko),
        ("한글", Language::Ko),
        ("korean", Language::Ko),
        ("영어", Language::En),
        ("english", Language::En),
    ];
    let mentions_language =
        compact.contains("언어") || compact.contains("language") || compact.contains("ui") || has_change_verb;
    if mentions_language {
        for (word, lang) in lang_words {
            if compact.contains(word) && compact.chars().count() <= 30 {
                return PreRouteResult::hit(PreRoutedOp::SetLanguage { language: *lang }, RouteSource::Phrase, text);
            }
        }
    }

    // 5) Open screen: "설정 열어줘", "모델 관리자로 가자", "open settings", "go to jobs"
    let open_verbs = [
        "열어", "열기", "보여", "이동", "가자", "가줘", "가 줘", "갈래", "켜줘", "open", "show", "goto", "go to",
        "take me",
    ];
    if open_verbs.iter().any(|v| lower.contains(v)) {
        let mut candidate = lower.clone();
        for v in open_verbs {
            candidate = candidate.replace(v, " ");
        }
        for filler in
            ["화면", "페이지", "탭", "메뉴", "the ", "screen", "page", "tab", "please", "줘", "주세요", "요", "to "]
        {
            candidate = candidate.replace(filler, " ");
        }
        for token in candidate.split_whitespace() {
            if let Some(screen) = Screen::parse(token) {
                return PreRouteResult::hit(PreRoutedOp::Open { screen }, RouteSource::Phrase, text);
            }
        }
        let joined: String = candidate.chars().filter(|c| !c.is_whitespace()).collect();
        if let Some(screen) = Screen::parse(&joined) {
            return PreRouteResult::hit(PreRoutedOp::Open { screen }, RouteSource::Phrase, text);
        }
    }

    PreRouteResult::none(text)
}

// ---------------------------------------------------------------------------
// Relative dates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateRange {
    pub from: NaiveDate,
    /// Inclusive.
    pub to: NaiveDate,
    /// Stable label such as `last_month`, `this_week`, `last_7_days`, `month`.
    pub label: String,
}

fn month_range(year: i32, month: u32) -> Option<DateRange> {
    let from = NaiveDate::from_ymd_opt(year, month, 1)?;
    let to = from.checked_add_months(Months::new(1))?.checked_sub_days(Days::new(1))?;
    Some(DateRange { from, to, label: "month".into() })
}

fn week_range(d: NaiveDate) -> DateRange {
    let monday = d - Days::new(u64::from(d.weekday().num_days_from_monday()));
    DateRange { from: monday, to: monday + Days::new(6), label: "week".into() }
}

fn parse_number(s: &str) -> Option<u32> {
    s.trim().parse::<u32>().ok()
}

/// Resolves relative date phrases (Korean/English) against `today`.
/// Examples: 오늘, 어제, 이번 주, 지난주, 이번 달, 지난달, 올해, 작년, 최근 7일,
/// 2026년 10월, 10월, 2026-10, today, yesterday, this/last week|month|year, last 30 days.
pub fn resolve_relative_date(text: &str, today: NaiveDate) -> Option<DateRange> {
    let lower = text.trim().to_lowercase();
    let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
    let (y, m) = (today.year(), today.month());

    let simple = |from: NaiveDate, to: NaiveDate, label: &str| Some(DateRange { from, to, label: label.into() });

    if compact.contains("그저께") || compact.contains("그제") || compact.contains("daybeforeyesterday") {
        let d = today - Days::new(2);
        return simple(d, d, "day_before_yesterday");
    }
    if compact.contains("어제") || compact.contains("yesterday") {
        let d = today - Days::new(1);
        return simple(d, d, "yesterday");
    }
    if compact.contains("오늘") || compact.contains("today") {
        return simple(today, today, "today");
    }
    if compact.contains("지난주")
        || compact.contains("저번주")
        || compact.contains("lastweek")
        || compact.contains("previousweek")
    {
        let w = week_range(today - Days::new(7));
        return simple(w.from, w.to, "last_week");
    }
    if compact.contains("이번주") || compact.contains("금주") || compact.contains("thisweek") {
        let w = week_range(today);
        return simple(w.from, w.to, "this_week");
    }
    if compact.contains("지난달")
        || compact.contains("저번달")
        || compact.contains("전월")
        || compact.contains("lastmonth")
        || compact.contains("previousmonth")
    {
        let first = NaiveDate::from_ymd_opt(y, m, 1)?;
        let prev = first.checked_sub_months(Months::new(1))?;
        let r = month_range(prev.year(), prev.month())?;
        return simple(r.from, r.to, "last_month");
    }
    if compact.contains("이번달")
        || compact.contains("당월")
        || compact.contains("금월")
        || compact.contains("thismonth")
    {
        let r = month_range(y, m)?;
        return simple(r.from, r.to, "this_month");
    }
    if compact.contains("작년") || compact.contains("지난해") || compact.contains("lastyear") {
        return simple(NaiveDate::from_ymd_opt(y - 1, 1, 1)?, NaiveDate::from_ymd_opt(y - 1, 12, 31)?, "last_year");
    }
    if compact.contains("올해") || compact.contains("금년") || compact.contains("thisyear") {
        return simple(NaiveDate::from_ymd_opt(y, 1, 1)?, NaiveDate::from_ymd_opt(y, 12, 31)?, "this_year");
    }
    // 최근 N일 / last N days / past N days / N일간
    for marker in ["최근", "지난", "last", "past"] {
        if let Some(idx) = compact.find(marker) {
            let rest = &compact[idx + marker.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Some(n) = parse_number(&digits) {
                let after = &rest[digits.len()..];
                if after.starts_with('일') || after.starts_with("day") {
                    let n = n.max(1);
                    return simple(today - Days::new(u64::from(n - 1)), today, &format!("last_{n}_days"));
                }
                if after.starts_with("주") || after.starts_with("week") {
                    return simple(today - Days::new(u64::from(n * 7 - 1)), today, &format!("last_{n}_weeks"));
                }
                if after.starts_with("개월") || after.starts_with("달") || after.starts_with("month") {
                    let from = today.checked_sub_months(Months::new(n))?;
                    return simple(from, today, &format!("last_{n}_months"));
                }
            }
        }
    }
    // YYYY년 M월 / YYYY-MM / YYYY.MM / YYYY/MM
    {
        let chars: Vec<char> = compact.chars().collect();
        let mut i = 0;
        while i + 4 <= chars.len() {
            if chars[i..i + 4].iter().all(|c| c.is_ascii_digit()) {
                let year: i32 = chars[i..i + 4].iter().collect::<String>().parse().ok()?;
                let mut j = i + 4;
                if j < chars.len() && ['년', '-', '.', '/'].contains(&chars[j]) {
                    j += 1;
                    let digits: String = chars[j..].iter().take_while(|c| c.is_ascii_digit()).collect();
                    if let Some(month) = parse_number(&digits) {
                        if (1..=12).contains(&month) && (1900..=2200).contains(&year) {
                            let after = &compact[compact
                                .char_indices()
                                .nth(j + digits.len())
                                .map(|(b, _)| b)
                                .unwrap_or(compact.len())..];
                            if after.starts_with('월')
                                || after.is_empty()
                                || !after.starts_with(|c: char| c.is_ascii_digit())
                            {
                                let r = month_range(year, month)?;
                                return simple(r.from, r.to, "month");
                            }
                        }
                    }
                    if (1900..=2200).contains(&year) && digits.is_empty() && chars[j - 1] == '년' {
                        return simple(
                            NaiveDate::from_ymd_opt(year, 1, 1)?,
                            NaiveDate::from_ymd_opt(year, 12, 31)?,
                            "year",
                        );
                    }
                }
                i += 4;
            } else {
                i += 1;
            }
        }
    }
    // "10월" (current year; if that month is in the future, previous year)
    if let Some(idx) = compact.find('월') {
        let before: String = compact[..idx]
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if let Some(month) = parse_number(&before) {
            if (1..=12).contains(&month) {
                let year = if month > m { y - 1 } else { y };
                let r = month_range(year, month)?;
                return simple(r.from, r.to, "month");
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Weekday;

    fn op(text: &str) -> Option<PreRoutedOp> {
        route(text).op
    }

    #[test]
    fn slash_commands() {
        assert_eq!(op("/테마 다크"), Some(PreRoutedOp::SetTheme { theme: Theme::Dark }));
        assert_eq!(op("/theme high-contrast"), Some(PreRoutedOp::SetTheme { theme: Theme::HighContrast }));
        assert_eq!(op("/언어 영어"), Some(PreRoutedOp::SetLanguage { language: Language::En }));
        assert_eq!(op("/lang ko"), Some(PreRoutedOp::SetLanguage { language: Language::Ko }));
        assert_eq!(op("/열기 설정"), Some(PreRoutedOp::Open { screen: Screen::Settings }));
        assert_eq!(op("/open models"), Some(PreRoutedOp::Open { screen: Screen::Models }));
        assert_eq!(op("/설정"), Some(PreRoutedOp::Open { screen: Screen::Settings }));
        assert_eq!(op("/jobs"), Some(PreRoutedOp::Open { screen: Screen::Jobs }));
        assert_eq!(op("/도움말"), Some(PreRoutedOp::Help));
        assert_eq!(op("/help"), Some(PreRoutedOp::Help));
        assert_eq!(route("/테마 다크").confidence, 1.0);
        assert!(!route("/unknown thing").matched);
        // slash theme without a valid argument opens settings
        assert_eq!(op("/테마"), Some(PreRoutedOp::Open { screen: Screen::Settings }));
    }

    #[test]
    fn korean_phrases() {
        assert_eq!(op("다크 모드로 바꿔"), Some(PreRoutedOp::SetTheme { theme: Theme::Dark }));
        assert_eq!(op("라이트 테마 적용해줘"), Some(PreRoutedOp::SetTheme { theme: Theme::Light }));
        assert_eq!(op("고대비 모드 켜줘"), Some(PreRoutedOp::SetTheme { theme: Theme::HighContrast }));
        assert_eq!(op("테마를 시스템 모드로 변경"), Some(PreRoutedOp::SetTheme { theme: Theme::System }));
        assert_eq!(op("영어로 바꿔줘"), Some(PreRoutedOp::SetLanguage { language: Language::En }));
        assert_eq!(op("언어를 한국어로 변경해줘"), Some(PreRoutedOp::SetLanguage { language: Language::Ko }));
        assert_eq!(op("설정 열어줘"), Some(PreRoutedOp::Open { screen: Screen::Settings }));
        assert_eq!(op("모델 관리자로 가자"), Some(PreRoutedOp::Open { screen: Screen::Models }));
        assert_eq!(op("작업 화면 보여줘"), Some(PreRoutedOp::Open { screen: Screen::Jobs }));
        assert_eq!(op("검토 큐 열기"), Some(PreRoutedOp::Open { screen: Screen::Review }));
        assert_eq!(op("지식 베이스로 이동"), Some(PreRoutedOp::Open { screen: Screen::Knowledge }));
        assert_eq!(op("도움말"), Some(PreRoutedOp::Help));
        assert_eq!(route("다크 모드로 바꿔").source, RouteSource::Phrase);
    }

    #[test]
    fn english_phrases() {
        assert_eq!(op("switch to dark mode"), Some(PreRoutedOp::SetTheme { theme: Theme::Dark }));
        assert_eq!(op("use the light theme"), Some(PreRoutedOp::SetTheme { theme: Theme::Light }));
        assert_eq!(op("enable high contrast"), Some(PreRoutedOp::SetTheme { theme: Theme::HighContrast }));
        assert_eq!(op("switch language to english"), Some(PreRoutedOp::SetLanguage { language: Language::En }));
        assert_eq!(op("use korean"), Some(PreRoutedOp::SetLanguage { language: Language::Ko }));
        assert_eq!(op("open settings"), Some(PreRoutedOp::Open { screen: Screen::Settings }));
        assert_eq!(op("go to the jobs page"), Some(PreRoutedOp::Open { screen: Screen::Jobs }));
        assert_eq!(op("show me the model manager"), Some(PreRoutedOp::Open { screen: Screen::Models }));
        assert_eq!(op("what can you do?"), Some(PreRoutedOp::Help));
    }

    #[test]
    fn passes_real_work_to_the_planner() {
        for text in [
            "inbox 폴더의 영수증을 전부 처리하고 월별 폴더로 정리해줘",
            "지난달 영수증 합계",
            "세금계산서를 엑셀로 내보내",
            "이 문서 요약해줘",
            "summarize the contract",
            "영어 계약서 찾아줘",
            "어두운 이미지 다시 OCR",
            "dark receipts need another pass",
            "",
        ] {
            let r = route(text);
            assert!(!r.matched, "{text:?} must not be pre-routed: {r:?}");
        }
    }

    #[test]
    fn relative_dates() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap(); // Tuesday
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        let r = resolve_relative_date("지난달 영수증 합계", today).unwrap();
        assert_eq!((r.from, r.to, r.label.as_str()), (d(2026, 10, 1), d(2026, 10, 31), "last_month"));
        let r = resolve_relative_date("이번 달", today).unwrap();
        assert_eq!((r.from, r.to), (d(2026, 11, 1), d(2026, 11, 30)));
        let r = resolve_relative_date("last week", today).unwrap();
        assert_eq!((r.from, r.to), (d(2026, 10, 26), d(2026, 11, 1)));
        let r = resolve_relative_date("이번 주", today).unwrap();
        assert_eq!((r.from, r.to), (d(2026, 11, 2), d(2026, 11, 8)));
        assert_eq!(resolve_relative_date("어제", today).unwrap().from, d(2026, 11, 2));
        assert_eq!(resolve_relative_date("today", today).unwrap().to, today);
        let r = resolve_relative_date("최근 7일", today).unwrap();
        assert_eq!((r.from, r.to, r.label.as_str()), (d(2026, 10, 28), today, "last_7_days"));
        let r = resolve_relative_date("last 30 days", today).unwrap();
        assert_eq!(r.from, d(2026, 10, 5));
        let r = resolve_relative_date("작년", today).unwrap();
        assert_eq!((r.from, r.to), (d(2025, 1, 1), d(2025, 12, 31)));
        let r = resolve_relative_date("2026년 10월 세금계산서", today).unwrap();
        assert_eq!((r.from, r.to), (d(2026, 10, 1), d(2026, 10, 31)));
        let r = resolve_relative_date("2025-02", today).unwrap();
        assert_eq!((r.from, r.to), (d(2025, 2, 1), d(2025, 2, 28)));
        let r = resolve_relative_date("2024년", today).unwrap();
        assert_eq!((r.from, r.to), (d(2024, 1, 1), d(2024, 12, 31)));
        // bare month: future month -> previous year
        let r = resolve_relative_date("12월 영수증", today).unwrap();
        assert_eq!((r.from, r.to), (d(2025, 12, 1), d(2025, 12, 31)));
        let r = resolve_relative_date("3월", today).unwrap();
        assert_eq!((r.from, r.to), (d(2026, 3, 1), d(2026, 3, 31)));
        assert!(resolve_relative_date("영수증 합계", today).is_none());
        // week range always starts Monday
        assert_eq!(week_range(d(2026, 11, 8)).from.weekday(), Weekday::Mon);
    }
}
