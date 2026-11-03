//! Deterministic natural-language pre-router (design §3.5, stage 1).
//!
//! Handles what never needs the VLM planner: slash commands, unambiguous
//! settings sentences (theme / language / open screen / help) and relative
//! date phrases. Everything else is passed through (`matched == false`) so the
//! planner (phase 5) can produce a typed plan. Korean is the primary language;
//! English is also recognised.
//!
//! The phrase rules are deliberately conservative: a sentence is only
//! pre-routed when it consists of the setting noun/verb and nothing that looks
//! like document work (번역·요약·OCR·추출·정리·검색 …). False negatives are
//! cheap (the planner sees the text); false positives hijack real work.

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
            Screen::Library => &[
                "라이브러리",
                "문서라이브러리",
                "문서목록",
                "문서함",
                "library",
                "documentlibrary",
                "documents",
                "docs",
            ],
            Screen::Search => &["검색", "search"],
            Screen::Assistant => &["어시스턴트", "비서", "채팅", "대화", "assistant", "chat"],
            Screen::Review => &["검토", "검토큐", "검토대기", "review", "reviewqueue", "hitl"],
            Screen::Schemas => &["스키마", "추출스키마", "양식", "서식", "schemas", "schema", "forms", "form"],
            Screen::Knowledge => &["지식", "지식베이스", "지식창고", "knowledge", "knowledgebase", "kb", "rag"],
            Screen::Generate => &["문서생성", "generate", "draft", "compose"],
            Screen::Projects => &["프로젝트", "케이스", "projects", "project", "cases"],
            Screen::Organize => &["정리", "내보내기", "정리규칙", "organize", "organise", "export", "exports"],
            Screen::Jobs => &["작업", "작업목록", "잡", "jobs", "job", "tasks", "queue"],
            Screen::Models => &["모델", "모델관리자", "모델관리", "엔진", "models", "model", "modelmanager", "engine"],
            Screen::Settings => {
                &["설정", "환경설정", "옵션", "settings", "setting", "preferences", "options", "config"]
            }
        }
    }

    /// Exact match against the aliases (whitespace/dash/underscore removed,
    /// Korean particles stripped). Never matches substrings.
    pub fn parse(s: &str) -> Option<Screen> {
        let key: String =
            s.trim().to_lowercase().chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').collect();
        if key.is_empty() {
            return None;
        }
        for sc in Screen::ALL {
            if sc.aliases().contains(&key.as_str()) {
                return Some(sc);
            }
        }
        let stripped = strip_korean_particles(&key);
        Screen::ALL.into_iter().find(|sc| sc.aliases().contains(&stripped.as_str()))
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

/// Words that mark document work. A sentence containing one is never
/// pre-routed as a settings command (slash commands are exempt).
const WORK_WORDS: &[&str] = &[
    "번역",
    "요약",
    "ocr",
    "추출",
    "처리",
    "정리해",
    "정리하",
    "옮겨",
    "이동해",
    "이동시",
    "찾아",
    "검색해",
    "검색하",
    "내보내",
    "출력",
    "삭제",
    "지워",
    "합계",
    "집계",
    "스크린샷",
    "스캔",
    "분류",
    "가져와",
    "가져오",
    "업로드",
    "템플릿",
    "양식으로",
    "작성",
    "생성",
    "저장",
    "인쇄",
    "비교",
    "분석",
    "translate",
    "summar",
    "extract",
    "process",
    "organi",
    "search",
    "find ",
    "rename",
    "delete",
    "export",
    "import",
    "scan",
    "screenshot",
    "total",
    "sum ",
    "template",
    "draft",
    "write",
    "classify",
    "analy",
    "compare",
    "print",
    "save ",
];

const KO_SET_VERBS: &[&str] =
    &["바꿔", "바꾸", "변경", "전환", "설정", "해줘", "해줄", "해주", "적용", "써줘", "켜", "선택", "사용"];

const OPEN_VERBS: &[&str] = &[
    "열어",
    "열기",
    "보여",
    "이동",
    "가자",
    "가줘",
    "가 줘",
    "갈래",
    "켜줘",
    "띄워",
    "take me to",
    "go to",
    "goto",
    "open",
    "show",
    "navigate to",
    "switch to",
];
const OPEN_FILLERS: &[&str] = &[
    "주세요",
    "해주세요",
    "화면",
    "페이지",
    "메뉴",
    "탭",
    "으로",
    "줘",
    "좀",
    "요",
    "the ",
    "screen",
    "page",
    "tab",
    "please",
    "me ",
    "my ",
    "to ",
];

fn mentions_work(lower: &str, compact: &str) -> bool {
    WORK_WORDS.iter().any(|w| compact.contains(w) || lower.contains(w))
}

fn has_change_verb(lower: &str) -> bool {
    [
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
    .any(|k| lower.contains(k))
}

/// Theme phrases: "다크 모드로 바꿔", "switch to light theme", "고대비 켜줘".
fn theme_target(lower: &str, compact: &str) -> Option<Theme> {
    let mentions_theme =
        ["테마", "모드", "theme", "mode", "고대비", "contrast", "스킨"].iter().any(|k| compact.contains(k));
    if !mentions_theme || mentions_work(lower, compact) {
        return None;
    }
    if !(has_change_verb(lower) || compact.chars().count() <= 12) {
        return None;
    }
    let words: &[(&str, Theme)] = &[
        ("고대비", Theme::HighContrast),
        ("high-contrast", Theme::HighContrast),
        ("highcontrast", Theme::HighContrast),
        ("다크", Theme::Dark),
        ("어둡게", Theme::Dark),
        ("어두운", Theme::Dark),
        ("dark", Theme::Dark),
        ("라이트", Theme::Light),
        ("밝게", Theme::Light),
        ("밝은", Theme::Light),
        ("화이트", Theme::Light),
        ("light", Theme::Light),
        ("시스템", Theme::System),
        ("system", Theme::System),
    ];
    words.iter().find(|(w, _)| compact.contains(w)).map(|(_, t)| *t)
}

/// Language phrases, anchored: "영어로 바꿔줘", "언어를 한국어로 변경해줘",
/// "switch language to english", "use korean", "english ui".
fn language_target(lower: &str, compact: &str) -> Option<Language> {
    if mentions_work(lower, compact) {
        return None;
    }
    // Korean: <lang>(으)로 <set-verb>…  or  언어 … <lang>
    for (word, lang) in [("영어", Language::En), ("한국어", Language::Ko), ("한글", Language::Ko)] {
        if let Some(idx) = compact.find(word) {
            let after = &compact[idx + word.len()..];
            let tail = after.strip_prefix("으로").or_else(|| after.strip_prefix("로"));
            match tail {
                Some(tail) if tail.is_empty() || KO_SET_VERBS.iter().any(|v| tail.starts_with(v)) => return Some(lang),
                _ => {}
            }
            if compact.contains("언어") && after.chars().count() <= 8 {
                return Some(lang);
            }
        }
    }
    // English
    let en_word = |s: &str| -> Option<Language> {
        if s.contains("english") {
            Some(Language::En)
        } else if s.contains("korean") {
            Some(Language::Ko)
        } else {
            None
        }
    };
    let lang = en_word(compact)?;
    if compact.contains("language") || compact.contains("ui") && compact.chars().count() <= 20 {
        return Some(lang);
    }
    let rest =
        ["use ", "switch to ", "set to ", "change to ", "set "].iter().find_map(|p| lower.trim().strip_prefix(p))?;
    let rest =
        rest.replace("the ", "").replace("language", "").replace("ui", "").replace("mode", "").trim().to_string();
    if rest == "english" || rest == "korean" {
        return Some(lang);
    }
    None
}

/// Open phrases: after removing the open verb and fillers, what remains must
/// be exactly one screen alias ("설정 열어줘", "모델 관리자로 가자", "go to the jobs page").
fn open_target(lower: &str) -> Option<Screen> {
    if !OPEN_VERBS.iter().any(|v| lower.contains(v)) {
        return None;
    }
    let mut candidate = lower.to_string();
    let mut verbs: Vec<&str> = OPEN_VERBS.to_vec();
    verbs.sort_by_key(|v| std::cmp::Reverse(v.len()));
    for v in verbs {
        candidate = candidate.replace(v, " ");
    }
    let mut fillers: Vec<&str> = OPEN_FILLERS.to_vec();
    fillers.sort_by_key(|f| std::cmp::Reverse(f.len()));
    for f in fillers {
        candidate = candidate.replace(f, " ");
    }
    let tokens: Vec<&str> = candidate.split_whitespace().collect();
    if tokens.is_empty() || tokens.len() > 3 {
        return None;
    }
    Screen::parse(&tokens.concat())
}

const HELP_PHRASES: &[&str] = &[
    "도움말",
    "도움",
    "도와줘",
    "도와주세요",
    "사용법",
    "뭘할수있어",
    "뭘할수있니",
    "뭘할수있나요",
    "무엇을할수있어",
    "무엇을할수있나요",
    "명령어",
    "명령목록",
    "help",
    "helpme",
    "whatcanyoudo",
    "howdoiusethis",
    "howtouse",
    "commands",
];

fn is_help(compact: &str) -> bool {
    let normalized = compact.trim_end_matches(['?', '!', '.', '~', ' ']);
    HELP_PHRASES.contains(&normalized)
}

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

    if is_help(&compact) {
        return PreRouteResult::hit(PreRoutedOp::Help, RouteSource::Phrase, text);
    }
    if let Some(theme) = theme_target(&lower, &compact) {
        return PreRouteResult::hit(PreRoutedOp::SetTheme { theme }, RouteSource::Phrase, text);
    }
    if let Some(language) = language_target(&lower, &compact) {
        return PreRouteResult::hit(PreRoutedOp::SetLanguage { language }, RouteSource::Phrase, text);
    }
    if let Some(screen) = open_target(&lower) {
        return PreRouteResult::hit(PreRoutedOp::Open { screen }, RouteSource::Phrase, text);
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
    /// Stable label such as `last_month`, `this_week`, `last_7_days`, `month`, `day`.
    pub label: String,
}

fn range(from: NaiveDate, to: NaiveDate, label: &str) -> Option<DateRange> {
    Some(DateRange { from, to, label: label.into() })
}

fn month_range(year: i32, month: u32) -> Option<DateRange> {
    let from = NaiveDate::from_ymd_opt(year, month, 1)?;
    let to = from.checked_add_months(Months::new(1))?.checked_sub_days(Days::new(1))?;
    range(from, to, "month")
}

fn week_range(d: NaiveDate) -> Option<DateRange> {
    let monday = d.checked_sub_days(Days::new(u64::from(d.weekday().num_days_from_monday())))?;
    range(monday, monday.checked_add_days(Days::new(6))?, "week")
}

fn leading_digits(s: &str) -> (u32, usize) {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).take(9).collect();
    (digits.parse().unwrap_or(0), digits.len())
}

/// Parses an absolute date: `YYYY년 M월 [D일]`, `YYYY-MM[-DD]`, `YYYY.MM[.DD]`,
/// `YYYY/MM[/DD]`, `YYYY년` (whole year) or `M월 [D일]` (current year, or the
/// previous year when the month lies in the future).
fn absolute_date(compact: &str, today: NaiveDate) -> Option<DateRange> {
    let bytes: Vec<char> = compact.chars().collect();
    // 1) four-digit year
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..i + 4].iter().all(|c| c.is_ascii_digit()) && (i == 0 || !bytes[i - 1].is_ascii_digit()) {
            let year: i32 = bytes[i..i + 4].iter().collect::<String>().parse().ok()?;
            if !(1900..=2200).contains(&year) {
                i += 4;
                continue;
            }
            let rest: String = bytes[i + 4..].iter().collect();
            let (sep_korean, rest) = match rest.chars().next() {
                Some('년') => (true, rest['년'.len_utf8()..].to_string()),
                Some(c) if ['-', '.', '/'].contains(&c) => (false, rest[1..].to_string()),
                _ => {
                    return range(NaiveDate::from_ymd_opt(year, 1, 1)?, NaiveDate::from_ymd_opt(year, 12, 31)?, "year")
                }
            };
            let (month, mlen) = leading_digits(&rest);
            if mlen == 0 || !(1..=12).contains(&month) {
                // "2026년" alone, possibly with "M월" elsewhere ("5월 2026년")
                if sep_korean {
                    if let Some((m, d)) = bare_month_day(&compact.replace(&format!("{year}년"), "")) {
                        return month_or_day(year, m, d);
                    }
                    return range(NaiveDate::from_ymd_opt(year, 1, 1)?, NaiveDate::from_ymd_opt(year, 12, 31)?, "year");
                }
                i += 4;
                continue;
            }
            let after = &rest[mlen..];
            let after = if sep_korean { after.strip_prefix('월').unwrap_or(after) } else { after };
            let day = if sep_korean {
                let (d, dlen) = leading_digits(after);
                if dlen > 0 && after[dlen..].starts_with('일') {
                    Some(d)
                } else {
                    None
                }
            } else if let Some(stripped) = after.strip_prefix(['-', '.', '/']) {
                let (d, dlen) = leading_digits(stripped);
                if dlen > 0 {
                    Some(d)
                } else {
                    None
                }
            } else {
                None
            };
            return month_or_day(year, month, day);
        }
        i += 1;
    }
    // 2) bare "M월 [D일]"
    let (m, d) = bare_month_day(compact)?;
    let year = if m > today.month() { today.year() - 1 } else { today.year() };
    month_or_day(year, m, d)
}

fn month_or_day(year: i32, month: u32, day: Option<u32>) -> Option<DateRange> {
    match day {
        Some(d) => {
            let date = NaiveDate::from_ymd_opt(year, month, d)?;
            range(date, date, "day")
        }
        None => month_range(year, month),
    }
}

/// `M월` optionally followed by `D일`, where the digits before 월 are not part
/// of a longer number (so "2026년 10월" is handled by the year branch).
fn bare_month_day(compact: &str) -> Option<(u32, Option<u32>)> {
    let idx = compact.find('월')?;
    let before: String =
        compact[..idx].chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    if before.is_empty() || before.len() > 2 {
        return None;
    }
    let month: u32 = before.parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    let after = &compact[idx + '월'.len_utf8()..];
    let (d, dlen) = leading_digits(after);
    let day = if dlen > 0 && dlen <= 2 && after[dlen..].starts_with('일') { Some(d) } else { None };
    Some((month, day))
}

/// Resolves relative date phrases (Korean/English) against `today`.
/// Examples: 오늘, 어제, 이번 주, 지난주, 이번 달, 지난달, 올해, 작년, 최근 7일,
/// 2026년 10월, 10월 5일, 2026-10-05, today, yesterday, this/last week|month|year,
/// last 30 days. Never panics on absurd inputs (counts are clamped).
pub fn resolve_relative_date(text: &str, today: NaiveDate) -> Option<DateRange> {
    let lower = text.trim().to_lowercase();
    let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
    let (y, m) = (today.year(), today.month());

    if compact.contains("그저께") || compact.contains("그제") || compact.contains("daybeforeyesterday") {
        let d = today.checked_sub_days(Days::new(2))?;
        return range(d, d, "day_before_yesterday");
    }
    if compact.contains("어제") || compact.contains("yesterday") {
        let d = today.checked_sub_days(Days::new(1))?;
        return range(d, d, "yesterday");
    }
    if compact.contains("오늘") || compact.contains("today") {
        return range(today, today, "today");
    }
    if compact.contains("지난주")
        || compact.contains("저번주")
        || compact.contains("lastweek")
        || compact.contains("previousweek")
    {
        let w = week_range(today.checked_sub_days(Days::new(7))?)?;
        return range(w.from, w.to, "last_week");
    }
    if compact.contains("이번주") || compact.contains("금주") || compact.contains("thisweek") {
        let w = week_range(today)?;
        return range(w.from, w.to, "this_week");
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
        return range(r.from, r.to, "last_month");
    }
    if compact.contains("이번달")
        || compact.contains("당월")
        || compact.contains("금월")
        || compact.contains("thismonth")
    {
        let r = month_range(y, m)?;
        return range(r.from, r.to, "this_month");
    }
    if compact.contains("작년") || compact.contains("지난해") || compact.contains("lastyear") {
        return range(NaiveDate::from_ymd_opt(y - 1, 1, 1)?, NaiveDate::from_ymd_opt(y - 1, 12, 31)?, "last_year");
    }
    if compact.contains("올해") || compact.contains("금년") || compact.contains("thisyear") {
        return range(NaiveDate::from_ymd_opt(y, 1, 1)?, NaiveDate::from_ymd_opt(y, 12, 31)?, "this_year");
    }
    // 최근 N일 / last N days / past N days / N주 / N개월 (counts clamped, never panics)
    for marker in ["최근", "지난", "last", "past"] {
        if let Some(idx) = compact.find(marker) {
            let rest = &compact[idx + marker.len()..];
            let (n, dlen) = leading_digits(rest);
            if dlen == 0 {
                continue;
            }
            let after = &rest[dlen..];
            if after.starts_with('일') || after.starts_with("day") {
                let n = n.clamp(1, 36_500);
                return range(today.checked_sub_days(Days::new(u64::from(n - 1)))?, today, &format!("last_{n}_days"));
            }
            if after.starts_with('주') || after.starts_with("week") {
                let n = n.clamp(1, 5_200);
                return range(
                    today.checked_sub_days(Days::new(u64::from(n) * 7 - 1))?,
                    today,
                    &format!("last_{n}_weeks"),
                );
            }
            if after.starts_with("개월") || after.starts_with('달') || after.starts_with("month") {
                let n = n.clamp(1, 1_200);
                return range(today.checked_sub_months(Months::new(n))?, today, &format!("last_{n}_months"));
            }
        }
    }
    absolute_date(&compact, today)
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
        assert_eq!(op("한국어로"), Some(PreRoutedOp::SetLanguage { language: Language::Ko }));
        assert_eq!(op("설정 열어줘"), Some(PreRoutedOp::Open { screen: Screen::Settings }));
        assert_eq!(op("설정 열어주세요"), Some(PreRoutedOp::Open { screen: Screen::Settings }));
        assert_eq!(op("모델 관리자로 가자"), Some(PreRoutedOp::Open { screen: Screen::Models }));
        assert_eq!(op("작업 화면 보여줘"), Some(PreRoutedOp::Open { screen: Screen::Jobs }));
        assert_eq!(op("검토 큐 열기"), Some(PreRoutedOp::Open { screen: Screen::Review }));
        assert_eq!(op("지식 베이스로 이동"), Some(PreRoutedOp::Open { screen: Screen::Knowledge }));
        assert_eq!(op("홈으로 가줘"), Some(PreRoutedOp::Open { screen: Screen::Home }));
        assert_eq!(op("문서 라이브러리 열어줘"), Some(PreRoutedOp::Open { screen: Screen::Library }));
        assert_eq!(op("도움말"), Some(PreRoutedOp::Help));
        assert_eq!(op("뭘 할 수 있어?"), Some(PreRoutedOp::Help));
        assert_eq!(route("다크 모드로 바꿔").source, RouteSource::Phrase);
    }

    #[test]
    fn english_phrases() {
        assert_eq!(op("switch to dark mode"), Some(PreRoutedOp::SetTheme { theme: Theme::Dark }));
        assert_eq!(op("use the light theme"), Some(PreRoutedOp::SetTheme { theme: Theme::Light }));
        assert_eq!(op("enable high contrast"), Some(PreRoutedOp::SetTheme { theme: Theme::HighContrast }));
        assert_eq!(op("switch language to english"), Some(PreRoutedOp::SetLanguage { language: Language::En }));
        assert_eq!(op("use korean"), Some(PreRoutedOp::SetLanguage { language: Language::Ko }));
        assert_eq!(op("english ui"), Some(PreRoutedOp::SetLanguage { language: Language::En }));
        assert_eq!(op("open settings"), Some(PreRoutedOp::Open { screen: Screen::Settings }));
        assert_eq!(op("go to the jobs page"), Some(PreRoutedOp::Open { screen: Screen::Jobs }));
        assert_eq!(op("show me the model manager"), Some(PreRoutedOp::Open { screen: Screen::Models }));
        assert_eq!(op("take me to settings"), Some(PreRoutedOp::Open { screen: Screen::Settings }));
        assert_eq!(op("what can you do?"), Some(PreRoutedOp::Help));
        assert_eq!(op("help"), Some(PreRoutedOp::Help));
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
            // reviewer false positives
            "이 문서 영어로 번역해줘",
            "한국어로 요약해줘",
            "use the english template",
            "다크 모드 스크린샷 OCR 해줘",
            "고대비 스캔본 OCR 해줘",
            "지난주 작업 내역 보여줘",
            "정리 폴더로 이동",
            "회사 홈페이지 주소 보여줘",
            "검토가 필요한 문서 보여줘",
            "이 양식 사용법 알려줘",
            "설정 파일 열어줘",
            "모델 다운로드 받아줘",
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
        let r = resolve_relative_date("지난 2주", today).unwrap();
        assert_eq!(r.from, d(2026, 10, 21));
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
        assert_eq!(week_range(d(2026, 11, 8)).unwrap().from.weekday(), Weekday::Mon);
    }

    #[test]
    fn absolute_days_and_month_boundaries() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        for (text, expect) in [
            ("2026-10-05 영수증", d(2026, 10, 5)),
            ("2026년 10월 5일", d(2026, 10, 5)),
            ("2026.10.05", d(2026, 10, 5)),
            ("10월 5일 영수증", d(2026, 10, 5)),
            ("2024-02-29", d(2024, 2, 29)),
        ] {
            let r = resolve_relative_date(text, today).unwrap();
            assert_eq!((r.from, r.to, r.label.as_str()), (expect, expect, "day"), "{text}");
        }
        assert!(resolve_relative_date("2023-02-29", today).is_none());
        let r = resolve_relative_date("5월 2026년", today).unwrap();
        assert_eq!((r.from, r.to), (d(2026, 5, 1), d(2026, 5, 31)));
        // January -> previous December
        let jan = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let r = resolve_relative_date("지난달", jan).unwrap();
        assert_eq!((r.from, r.to), (d(2025, 12, 1), d(2025, 12, 31)));
        // leap-year February
        let r = resolve_relative_date("2024년 2월", today).unwrap();
        assert_eq!(r.to, d(2024, 2, 29));
    }

    #[test]
    fn absurd_counts_never_panic() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        for text in [
            "최근 0주",
            "지난 0주",
            "past 0 weeks",
            "최근 0일",
            "최근 4000000000일",
            "last 99999999999 days",
            "최근 999999개월",
            "9999-99-99",
            "99월 99일",
        ] {
            let _ = resolve_relative_date(text, today);
        }
        assert_eq!(resolve_relative_date("최근 0일", today).unwrap().from, today);
        assert_eq!(
            resolve_relative_date("최근 0주", today).unwrap().from,
            today.checked_sub_days(Days::new(6)).unwrap()
        );
    }
}
