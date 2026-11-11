//! Deterministic validation (design §3.4.1 검증기): value normalisation
//! (amounts, dates, numbers), the 사업자등록번호 checksum, cross-field rules
//! (합계식), PII masking (§3.8) and the confidence formula that combines the
//! model's self-report with evidence checks and validation results.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use super::schema::{self, FieldDef, FieldKind, SchemaDef};
use super::ValidationIssue;

fn issue(rule: &str, level: &str, message: impl Into<String>) -> ValidationIssue {
    ValidationIssue { rule: rule.into(), level: level.into(), message: message.into() }
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static regex"))
}

// ---------------------------------------------------------------------------
// Normalisation
// ---------------------------------------------------------------------------

/// Korean numeral syllables (`일` … `구`, `영`).
fn han_digit(c: char) -> Option<f64> {
    "영일이삼사오육칠팔구".chars().position(|d| d == c).map(|i| i as f64)
}

/// `십` / `백` / `천`: multipliers inside one group of four digits.
fn small_unit(c: char) -> Option<f64> {
    match c {
        '십' => Some(10.0),
        '백' => Some(100.0),
        '천' => Some(1_000.0),
        _ => None,
    }
}

/// `만` / `억` / `조`: the group units.
fn big_unit(c: char) -> Option<f64> {
    match c {
        '만' => Some(1e4),
        '억' => Some(1e8),
        '조' => Some(1e12),
        _ => None,
    }
}

fn is_blank(c: char) -> bool {
    c == ' ' || c == '\u{a0}'
}

/// Reads a printed number at `i`: digits with thousands commas (`1,250,000`), thousands separated
/// by single spaces (`1 250 000`) and one decimal part. Returns the value and the index after it.
fn read_number(chars: &[char], i: usize) -> Option<(f64, usize)> {
    let n = chars.len();
    let digit = |k: usize| k < n && chars[k].is_ascii_digit();
    let mut text = String::new();
    let (mut k, mut commas) = (i, false);
    while digit(k) || (k < n && chars[k] == ',' && digit(k + 1)) {
        if chars[k] == ',' {
            commas = true;
        } else {
            text.push(chars[k]);
        }
        k += 1;
    }
    if !commas && text.len() <= 3 {
        while k < n && is_blank(chars[k]) && digit(k + 1) && digit(k + 2) && digit(k + 3) && !digit(k + 4) {
            text.extend(&chars[k + 1..k + 4]);
            k += 4;
        }
    }
    if k < n && chars[k] == '.' && digit(k + 1) {
        text.push('.');
        k += 1;
        while digit(k) {
            text.push(chars[k]);
            k += 1;
        }
        // `1.250.000` is not a decimal number.
        if k < n && chars[k] == '.' && digit(k + 1) {
            return None;
        }
    }
    text.parse::<f64>().ok().map(|v| (v, k))
}

/// One amount expression found in a text.
#[derive(Debug, Clone, Copy)]
struct AmountToken {
    value: f64,
    /// A currency mark stands next to it (`₩` / `$` / `금` before, `원` / `KRW` after).
    currency: bool,
    /// Directly followed by `%` or a counter word (`1식`, `3건`, `2026년`): a count, a rate, a date part.
    count_like: bool,
}

/// Reads one amount expression at `i`: a number, optionally with Korean numerals and units
/// (`3천만`, `삼백만`, `1억 2,000만`, `12만 5천`). Returns the value, the index after it and whether
/// it holds a digit / a unit.
fn read_amount(chars: &[char], i: usize) -> Option<(f64, usize, bool, bool)> {
    let n = chars.len();
    let (mut total, mut group) = (0f64, 0f64);
    let mut pending: Option<f64> = None;
    let (mut j, mut has_digit, mut has_unit) = (i, false, false);
    while j < n {
        let c = chars[j];
        if c.is_ascii_digit() {
            if pending.is_some() {
                break;
            }
            let (v, end) = read_number(chars, j)?;
            pending = Some(v);
            has_digit = true;
            j = end;
        } else if let Some(d) = han_digit(c) {
            if pending.is_some() {
                break;
            }
            pending = Some(d);
            j += 1;
        } else if let Some(u) = small_unit(c) {
            group += pending.take().unwrap_or(1.0) * u;
            has_unit = true;
            j += 1;
        } else if let Some(u) = big_unit(c) {
            let g = group + pending.take().unwrap_or(0.0);
            total += if g == 0.0 { u } else { g * u };
            group = 0.0;
            has_unit = true;
            j += 1;
            // "1억 2,000만", "12만 5천원": one blank inside the amount, when what follows goes on
            // with a unit or the currency ("1억 2명" does not).
            if j + 1 < n && is_blank(chars[j]) {
                let next = j + 1;
                let goes_on = if chars[next].is_ascii_digit() {
                    read_number(chars, next).is_some_and(|(_, end)| {
                        end < n
                            && (small_unit(chars[end]).is_some()
                                || big_unit(chars[end]).is_some()
                                || chars[end] == '원')
                    })
                } else {
                    han_digit(chars[next]).is_some() || small_unit(chars[next]).is_some()
                };
                if goes_on {
                    j = next;
                }
            }
        } else {
            break;
        }
    }
    (j > i).then(|| (total + group + pending.unwrap_or(0.0), j, has_digit, has_unit))
}

/// Every amount expression of a text, in order.
fn amount_tokens(text: &str) -> Vec<AmountToken> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let is_currency_sign = |c: char| matches!(c, '₩' | '￦' | '\\' | '$');
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        let starts = c.is_ascii_digit() || han_digit(c).is_some() || small_unit(c).is_some() || big_unit(c).is_some();
        // Not inside a word / code / another number ("A123", "v1.2").
        let glued = i > 0 && (chars[i - 1].is_ascii_alphanumeric() || (chars[i - 1] == '.' && c.is_ascii_digit()));
        if !starts || glued {
            i += 1;
            continue;
        }
        let Some((value, end, has_digit, has_unit)) = read_amount(&chars, i) else {
            i += 1;
            continue;
        };
        // What stands before (blanks and currency signs skipped) and after the expression.
        let mut b = i;
        let mut sign_seen = false;
        while b > 0 && (is_blank(chars[b - 1]) || is_currency_sign(chars[b - 1])) {
            sign_seen |= is_currency_sign(chars[b - 1]);
            b -= 1;
        }
        let before = (b > 0).then(|| chars[b - 1]);
        let mut a = end;
        while a < n && is_blank(chars[a]) {
            a += 1;
        }
        let rest: String = chars[a..n.min(a + 3)].iter().collect::<String>().to_lowercase();
        let currency = sign_seen
            || before == Some('금')
            || rest.starts_with('원')
            || rest.starts_with("krw")
            || rest.starts_with("won");
        // A run of numeral syllables is a word ("삼성", "이천") unless it is written as money.
        if !has_digit && !(has_unit && currency) {
            i = end;
            continue;
        }
        let minus = matches!(before, Some('-' | '−' | '△' | '▲'));
        // Accounting style "(3,000)": a bare number in brackets is negative.
        let bracketed = !currency && before == Some('(') && a < n && chars[a] == ')';
        let count_like = end < n && (chars[end] == '%' || (('가'..='힣').contains(&chars[end]) && chars[end] != '원'));
        out.push(AmountToken { value: if minus || bracketed { -value } else { value }, currency, count_like });
        i = end;
    }
    out
}

/// Parses a printed amount: `1,250,000원`, `₩12,500`, `(3,000)` (negative), `-500`, `△3,000`,
/// `12만원`, `1억 2,000만원`, `3천만원`, `금 삼백만원정`, `1.5만`.
///
/// One amount expression is taken, never the digits of the whole text glued together: the first
/// one written as money (`₩` / `금` before, `원` / `KRW` after) — `1,100,000원 (VAT 10% 포함)`,
/// `1식 5,000,000원` — else the only plain number, else the only counted one (`3개`, `85%`).
/// Several unrelated numbers without a currency mark are not an amount (`None`).
pub fn normalize_amount(s: &str) -> Option<f64> {
    let tokens = amount_tokens(s);
    if let Some(t) = tokens.iter().find(|t| t.currency) {
        return Some(t.value);
    }
    let only = |count_like: bool| {
        let mut it = tokens.iter().filter(|t| t.count_like == count_like);
        match (it.next(), it.next()) {
            (Some(t), None) => Some(Some(t.value)),
            (Some(_), Some(_)) => Some(None),
            _ => None,
        }
    };
    // Plain numbers first; counted ones (`3개`, `85%`) only when there is nothing else.
    only(false).or_else(|| only(true)).flatten()
}

pub fn normalize_integer(s: &str) -> Option<i64> {
    let v = normalize_amount(s)?;
    if v.fract().abs() > 1e-9 {
        return None;
    }
    Some(v as i64)
}

/// Parses the common Korean / ISO date spellings into `YYYY-MM-DD`
/// (`2026-10-14`, `2026.10.14`, `2026/10/14`, `2026년 10월 14일`, `26.10.14`,
/// `20261014`, `2026-10-14 12:31`) and the spellings with the year last (`14.10.2026`,
/// `10/14/2026`). Only real calendar days are accepted.
///
/// Year last: a part above 12 is the day; when both could be the month, `/` reads month first
/// (US) and `.` / `-` day first (European) — [`parse_date`] reports that guess. A two-digit year
/// is taken from the century that keeps it at most 20 years ahead (`85.03.15` → 1985).
pub fn normalize_date(s: &str) -> Option<String> {
    use chrono::Datelike;
    parse_date(s, chrono::Local::now().year()).map(|(date, _)| date)
}

/// [`normalize_date`] with the year "now" given, plus whether the day / month order was a guess
/// (`03/04/2026`).
pub fn parse_date(s: &str, this_year: i32) -> Option<(String, bool)> {
    static FULL: OnceLock<Regex> = OnceLock::new();
    static SHORT: OnceLock<Regex> = OnceLock::new();
    static COMPACT: OnceLock<Regex> = OnceLock::new();
    static YEAR_LAST: OnceLock<Regex> = OnceLock::new();
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let make = |y: i32, m: u32, d: u32| -> Option<String> {
        if !(1900..=2100).contains(&y) {
            return None;
        }
        chrono::NaiveDate::from_ymd_opt(y, m, d).map(|date| date.format("%Y-%m-%d").to_string())
    };
    // The regex crate has no look-around: a match that starts or ends inside a longer number
    // ("2026.10.145", "12026-10-14") is not a date.
    let standalone = |c: &regex::Captures| {
        let m = c.get(0).expect("whole match");
        let digit = |ch: Option<char>| ch.is_some_and(|ch| ch.is_ascii_digit());
        !digit(t[..m.start()].chars().next_back()) && !digit(t[m.end()..].chars().next())
    };
    let num = |c: &regex::Captures, name: &str| c[name].parse::<u32>().ok();

    let full = re(&FULL, r"(?P<y>(?:19|20)\d{2})\s*[.\-/년]\s*(?P<m>\d{1,2})\s*[.\-/월]\s*(?P<d>\d{1,2})");
    for c in full.captures_iter(t).filter(|c| standalone(c)) {
        if let Some(date) = make(num(&c, "y")? as i32, num(&c, "m")?, num(&c, "d")?) {
            return Some((date, false));
        }
    }
    let compact = re(&COMPACT, r"(?P<y>(?:19|20)\d{2})(?P<m>\d{2})(?P<d>\d{2})");
    for c in compact.captures_iter(t).filter(|c| standalone(c)) {
        if let Some(date) = make(num(&c, "y")? as i32, num(&c, "m")?, num(&c, "d")?) {
            return Some((date, false));
        }
    }
    let year_last = re(&YEAR_LAST, r"(?P<a>\d{1,2})\s*(?P<s>[.\-/])\s*(?P<b>\d{1,2})\s*[.\-/]\s*(?P<y>(?:19|20)\d{2})");
    for c in year_last.captures_iter(t).filter(|c| standalone(c)) {
        let (a, b, y) = (num(&c, "a")?, num(&c, "b")?, num(&c, "y")? as i32);
        let guess = a <= 12 && b <= 12 && a != b;
        let month_first = if a > 12 || b > 12 { b > 12 } else { &c["s"] == "/" };
        let (m, d) = if month_first { (a, b) } else { (b, a) };
        if let Some(date) = make(y, m, d) {
            return Some((date, guess));
        }
    }
    let short = re(&SHORT, r"(?P<y>\d{2})\s*[.\-/년]\s*(?P<m>\d{1,2})\s*[.\-/월]\s*(?P<d>\d{1,2})");
    for c in short.captures_iter(t).filter(|c| standalone(c)) {
        let yy = num(&c, "y")? as i32;
        let century = this_year - this_year.rem_euclid(100);
        let year = if century + yy > this_year + 20 { century - 100 + yy } else { century + yy };
        if let Some(date) = make(year, num(&c, "m")?, num(&c, "d")?) {
            return Some((date, false));
        }
    }
    None
}

/// Digits of a phone number grouped Korean style (`010-1234-5678`, `02-123-4567`).
pub fn normalize_phone(s: &str) -> Option<String> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 8 || digits.len() > 12 {
        return None;
    }
    let (head, rest) = match digits.strip_prefix("02") {
        Some(r) => (2usize, r),
        None => (3usize, &digits[3..]),
    };
    if rest.len() < 7 {
        return Some(digits);
    }
    let (mid, tail) = rest.split_at(rest.len() - 4);
    Some(format!("{}-{}-{}", &digits[..head], mid, tail))
}

/// 10 digits of a 사업자등록번호 (`123-45-67890`), or `None`.
pub fn brn_digits(s: &str) -> Option<String> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    (digits.len() == 10).then_some(digits)
}

/// Checksum of a 사업자등록번호 (§3.4.1): Σ dᵢ·[1,3,7,1,3,7,1,3,5] + ⌊d₉·5/10⌋,
/// check digit d₁₀ = (10 − Σ mod 10) mod 10.
pub fn brn_valid(digits: &str) -> bool {
    let d: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    if d.len() != 10 {
        return false;
    }
    const W: [u32; 9] = [1, 3, 7, 1, 3, 7, 1, 3, 5];
    let mut sum: u32 = d.iter().take(9).zip(W.iter()).map(|(x, w)| x * w).sum();
    sum += d[8] * 5 / 10;
    (10 - sum % 10) % 10 == d[9]
}

pub fn format_brn(digits: &str) -> String {
    if digits.len() == 10 {
        format!("{}-{}-{}", &digits[..3], &digits[3..5], &digits[5..])
    } else {
        digits.to_string()
    }
}

// ---------------------------------------------------------------------------
// PII masking (§3.8)
// ---------------------------------------------------------------------------

fn rrn_regex() -> &'static Regex {
    static RRN: OnceLock<Regex> = OnceLock::new();
    // 6 digits, then either nothing or a hyphen (spaces only around a hyphen), gender digit 1-8
    // + 6 digits; not inside a longer digit run. Two plain numbers separated by a space
    // ("150000 1500000") must not be taken for a resident number.
    re(&RRN, r"(?P<a>(?:^|[^0-9]))(?P<ymd>\d{6})(?:\s*[-–]\s*)?(?P<g>[1-8])(?P<rest>\d{6})(?P<z>[^0-9]|$)")
}

/// Masks every 주민등록번호 / 외국인등록번호 in `text` to `YYMMDD-G******`.
pub fn mask_text(text: &str) -> String {
    let mut out = text.to_string();
    // Iterate because overlapping boundaries (`\d{13}` back-to-back) need a second pass.
    for _ in 0..2 {
        let next = rrn_regex().replace_all(&out, "${a}${ymd}-${g}******${z}").into_owned();
        if next == out {
            break;
        }
        out = next;
    }
    out
}

/// Whether page texts are masked on their way into the search index (`privacy.pii_masking`).
/// Process-wide because the indexer has no settings of its own; the shell keeps it in step with
/// the settings whenever it builds a document context. On until told otherwise.
static INDEX_MASKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Sets whether [`crate::documents::repo::list_pages_for_index`] masks resident numbers.
pub fn set_index_masking(on: bool) {
    INDEX_MASKING.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn index_masking() -> bool {
    INDEX_MASKING.load(std::sync::atomic::Ordering::Relaxed)
}

/// `true` when the text contains an unmasked resident-registration-style number.
pub fn contains_rrn(text: &str) -> bool {
    rrn_regex().is_match(text)
}

/// Masks an account / identification number to its last four digits (`***-**-1234`).
pub fn mask_account(s: &str) -> String {
    let digits = s.chars().filter(|c| c.is_ascii_digit()).count();
    if digits <= 4 {
        return s.to_string();
    }
    let mut seen = 0;
    s.chars()
        .map(|c| {
            if c.is_ascii_digit() {
                seen += 1;
                if seen > digits - 4 {
                    c
                } else {
                    '*'
                }
            } else {
                c
            }
        })
        .collect()
}

/// Masks a value according to the field definition (`mask` flag or PII kind).
pub fn mask_value(def: &FieldDef, value: &str) -> String {
    match def.kind {
        FieldKind::Rrn => {
            let masked = mask_text(value);
            if masked == value {
                // Unusual spelling: keep only the birth date part.
                let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
                if digits.len() >= 7 {
                    return format!("{}-{}******", &digits[..6], &digits[6..7]);
                }
                return "******".to_string();
            }
            masked
        }
        FieldKind::Account => mask_account(value),
        FieldKind::Date if def.mask => {
            normalize_date(value).map(|d| format!("{}-**-**", &d[..4])).unwrap_or_else(|| "****".into())
        }
        _ if def.mask => {
            let chars: Vec<char> = value.chars().collect();
            if chars.len() <= 4 {
                "*".repeat(chars.len())
            } else {
                let keep: String = chars[chars.len() - 4..].iter().collect();
                format!("{}{keep}", "*".repeat(chars.len() - 4))
            }
        }
        _ => value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Field-level validation
// ---------------------------------------------------------------------------

/// Normalises and validates one scalar value. Returns the normalised form
/// (`None` when it cannot be parsed) and the issues found.
pub fn validate_value(def: &FieldDef, value: &str) -> (Option<String>, Vec<ValidationIssue>) {
    let mut issues = Vec::new();
    let v = value.trim();
    let masked_kind = def.mask || def.kind.is_pii();
    if masked_kind && v.contains('*') {
        // Already masked (re-validation of a stored value): nothing left to parse.
        return (Some(v.to_string()), issues);
    }
    let normalized = match def.kind {
        FieldKind::Amount => match normalize_amount(v) {
            Some(n) => Some(format_number(n)),
            None => {
                issues.push(issue("amount_parse", "fail", "금액을 숫자로 읽을 수 없음 / not a number"));
                None
            }
        },
        FieldKind::Integer => match normalize_integer(v) {
            Some(n) => Some(n.to_string()),
            None => {
                issues.push(issue("integer_parse", "fail", "정수가 아님 / not an integer"));
                None
            }
        },
        FieldKind::Date => match parse_date(v, {
            use chrono::Datelike;
            chrono::Local::now().year()
        }) {
            Some((d, guess)) => {
                if guess {
                    issues.push(issue(
                        "date_ambiguous",
                        "warn",
                        "일·월 순서가 불분명한 날짜 — 확인 필요 / day and month order is a guess",
                    ));
                }
                Some(d)
            }
            None => {
                issues.push(issue("date_parse", "fail", "날짜 형식을 인식할 수 없음 / unrecognised date"));
                None
            }
        },
        FieldKind::Brn => match brn_digits(v) {
            Some(d) if brn_valid(&d) => Some(format_brn(&d)),
            Some(d) => {
                issues.push(issue("brn_checksum", "fail", "사업자등록번호 검증자리 불일치 / checksum mismatch"));
                Some(format_brn(&d))
            }
            None => {
                issues.push(issue("brn_format", "fail", "사업자등록번호는 10자리 / must be 10 digits"));
                None
            }
        },
        FieldKind::Phone => match normalize_phone(v) {
            Some(p) => Some(p),
            None => {
                issues.push(issue("phone_format", "warn", "전화번호 자릿수 이상 / unusual digit count"));
                None
            }
        },
        FieldKind::Email => {
            let ok = v.contains('@') && v.rsplit('@').next().is_some_and(|d| d.contains('.')) && !v.contains(' ');
            if !ok {
                issues.push(issue("email_format", "warn", "이메일 형식 아님 / not an email"));
            }
            Some(v.to_ascii_lowercase())
        }
        FieldKind::Boolean => match v.to_ascii_lowercase().as_str() {
            "true" | "yes" | "y" | "예" | "o" | "☑" | "✓" | "있음" | "서명" | "1" => Some("true".into()),
            "false" | "no" | "n" | "아니오" | "x" | "☐" | "없음" | "0" => Some("false".into()),
            _ => {
                issues.push(issue("boolean_parse", "warn", "예/아니오로 읽을 수 없음 / not a yes-no value"));
                None
            }
        },
        FieldKind::Enum => {
            if def.options.iter().any(|o| o == v) {
                Some(v.to_string())
            } else if let Some(o) =
                def.options.iter().find(|o| o.eq_ignore_ascii_case(v) || o.contains(v) || v.contains(o.as_str()))
            {
                Some(o.clone())
            } else {
                issues.push(issue("enum_option", "warn", "허용 값 밖 / not one of the options"));
                None
            }
        }
        FieldKind::Rrn | FieldKind::Account => Some(mask_value(def, v)),
        FieldKind::String | FieldKind::Table | FieldKind::Group => Some(v.to_string()),
    };
    if let Some(r) = &def.regex_hint {
        if let Ok(rx) = Regex::new(r) {
            let target = normalized.as_deref().unwrap_or(v);
            if !rx.is_match(target) && !rx.is_match(v) {
                issues.push(issue("regex_hint", "warn", format!("형식 불일치 / does not match {r}")));
            }
        }
    }
    // The normalised form of a masked field is masked too (§3.8: no full value anywhere).
    let normalized = if masked_kind { normalized.map(|n| mask_value(def, &n)) } else { normalized };
    (normalized, issues)
}

/// `1250000` / `12.5` without trailing zeros.
pub fn format_number(n: f64) -> String {
    if n.fract().abs() < 1e-9 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

// ---------------------------------------------------------------------------
// Cross-field rules (§3.4.1)
// ---------------------------------------------------------------------------

/// Normalised scalar values by path plus table rows by key, as produced by the extractor.
#[derive(Debug, Default, Clone)]
pub struct Values {
    /// `path` → normalised value (amounts as plain numbers, dates ISO).
    pub scalars: BTreeMap<String, String>,
    /// table key → rows → column key → normalised value
    pub tables: BTreeMap<String, Vec<BTreeMap<String, String>>>,
}

impl Values {
    pub fn amount(&self, path: &str) -> Option<f64> {
        self.scalars.get(path).and_then(|v| v.parse::<f64>().ok())
    }
    pub fn text(&self, path: &str) -> Option<&str> {
        self.scalars.get(path).map(String::as_str)
    }
    /// Sum of a column over the rows that carry a number (`None` when no row does).
    pub fn column_sum(&self, table: &str, column: &str) -> Option<f64> {
        let rows = self.tables.get(table)?;
        let nums: Vec<f64> = rows.iter().filter_map(|r| r.get(column)).filter_map(|v| v.parse::<f64>().ok()).collect();
        (!nums.is_empty()).then(|| nums.iter().sum())
    }
}

/// Tolerance for sum checks: rounding of per-item VAT can drift by a few won.
fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

fn won(n: f64) -> String {
    let i = n.round() as i64;
    let s = i.abs().to_string();
    let mut out = String::new();
    for (k, c) in s.chars().enumerate() {
        if k > 0 && (s.len() - k) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if i < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// Evaluates the schema's cross-field rules. Returns `(path, issue)` pairs;
/// the first path of a failing rule is the anchor that goes to review.
pub fn document_rules(def: &SchemaDef, values: &Values) -> Vec<(String, ValidationIssue)> {
    let mut out = Vec::new();
    for rule in &def.rules {
        match rule.as_str() {
            schema::RULE_SUPPLY_TAX_TOTAL => {
                if let (Some(s), Some(t), Some(g)) =
                    (values.amount("supply_total"), values.amount("tax_total"), values.amount("grand_total"))
                {
                    if close(s + t, g, 1.0) {
                        out.push(("grand_total".into(), issue(rule, "ok", "공급가액 + 세액 = 합계")));
                    } else {
                        let msg =
                            format!("공급가액 {} + 세액 {} ≠ 합계 {} / supply + tax ≠ total", won(s), won(t), won(g));
                        out.push(("grand_total".into(), issue(rule, "fail", msg.clone())));
                        out.push(("supply_total".into(), issue(rule, "warn", msg.clone())));
                        out.push(("tax_total".into(), issue(rule, "warn", msg)));
                    }
                }
            }
            schema::RULE_ITEMS_SUM_SUPPLY => {
                if let (Some(sum), Some(s)) =
                    (values.column_sum("items", "supply_amount"), values.amount("supply_total"))
                {
                    if close(sum, s, 1.0) {
                        out.push(("supply_total".into(), issue(rule, "ok", "Σ품목 공급가액 = 공급가액 합계")));
                    } else {
                        out.push((
                            "supply_total".into(),
                            issue(
                                rule,
                                "fail",
                                format!("Σ품목 {} ≠ 공급가액 합계 {} / items ≠ supply total", won(sum), won(s)),
                            ),
                        ));
                    }
                }
            }
            schema::RULE_TAX_RATE => {
                let taxable = values.text("tax_type").is_none_or(|t| t.contains("과세") && !t.contains("영세"));
                if let (true, Some(s), Some(t)) = (taxable, values.amount("supply_total"), values.amount("tax_total")) {
                    if s > 0.0 {
                        let expected = (s * 0.1).round();
                        if close(t, expected, (s * 0.005).max(10.0)) {
                            out.push(("tax_total".into(), issue(rule, "ok", "세액 ≈ 공급가액 × 10%")));
                        } else {
                            out.push((
                                "tax_total".into(),
                                issue(
                                    rule,
                                    "warn",
                                    format!("세액 {} ≠ 공급가액의 10% ({}) / tax ≠ 10%", won(t), won(expected)),
                                ),
                            ));
                        }
                    }
                }
            }
            schema::RULE_ZERO_RATE => {
                if values.text("tax_type").is_some_and(|t| t.contains("영세")) {
                    if let Some(t) = values.amount("tax_total") {
                        if t.abs() > 0.5 {
                            out.push((
                                "tax_total".into(),
                                issue(rule, "fail", "영세율인데 세액 ≠ 0 / zero-rated but tax ≠ 0"),
                            ));
                        } else {
                            out.push(("tax_total".into(), issue(rule, "ok", "영세율 세액 0")));
                        }
                    }
                }
            }
            schema::RULE_RECEIPT_TOTAL => {
                if let (Some(s), Some(total)) = (values.amount("supply_amount"), values.amount("total")) {
                    let parts = s
                        + values.amount("vat").unwrap_or(0.0)
                        + values.amount("service_charge").unwrap_or(0.0)
                        + values.amount("exempt_amount").unwrap_or(0.0);
                    if close(parts, total, 1.0) {
                        out.push(("total".into(), issue(rule, "ok", "공급가액 + 부가세 (+ 봉사료/면세) = 합계")));
                    } else {
                        out.push((
                            "total".into(),
                            issue(
                                rule,
                                "fail",
                                format!("구성 금액 {} ≠ 합계 {} / parts ≠ total", won(parts), won(total)),
                            ),
                        ));
                    }
                }
            }
            schema::RULE_ITEMS_SUM_TOTAL => {
                if let (Some(sum), Some(total)) = (values.column_sum("items", "amount"), values.amount("total")) {
                    if close(sum, total, 1.0) {
                        out.push(("total".into(), issue(rule, "ok", "Σ품목 = 합계")));
                    } else {
                        out.push((
                            "total".into(),
                            issue(rule, "warn", format!("Σ품목 {} ≠ 합계 {} / items ≠ total", won(sum), won(total))),
                        ));
                    }
                }
            }
            schema::RULE_PERIOD_ORDER => {
                if let (Some(a), Some(b)) = (values.text("period_start"), values.text("period_end")) {
                    if a <= b {
                        out.push(("period_end".into(), issue(rule, "ok", "기간 시작 ≤ 종료")));
                    } else {
                        out.push((
                            "period_end".into(),
                            issue(rule, "fail", "계약기간 종료가 시작보다 앞섬 / end before start"),
                        ));
                    }
                }
            }
            schema::RULE_PERCENT_RANGE => {
                // Scalars named *_percent / *_rate and the `progress` column of tables are percentages.
                let mut paths: Vec<(String, f64)> = values
                    .scalars
                    .keys()
                    .filter(|k| k.ends_with("_percent") || k.ends_with("_rate"))
                    .filter_map(|k| values.amount(k).map(|v| (k.clone(), v)))
                    .collect();
                for (table, rows) in &values.tables {
                    for (i, row) in rows.iter().enumerate() {
                        if let Some(v) = row.get("progress").and_then(|v| v.trim().parse::<f64>().ok()) {
                            paths.push((format!("{table}[{i}].progress"), v));
                        }
                    }
                }
                for (path, v) in paths {
                    if (0.0..=100.0).contains(&v) {
                        out.push((path, issue(rule, "ok", "0~100%")));
                    } else {
                        out.push((path, issue(rule, "fail", format!("{v}% is outside 0~100 / 백분율 범위 밖"))));
                    }
                }
            }
            schema::RULE_TEST_COUNTS => {
                if let Some(total) = values.amount("total_cases") {
                    let parts = values.amount("passed").unwrap_or(0.0)
                        + values.amount("failed").unwrap_or(0.0)
                        + values.amount("blocked").unwrap_or(0.0);
                    if close(parts, total, 0.5) {
                        out.push(("total_cases".into(), issue(rule, "ok", "통과 + 실패 + 보류 = 총 케이스")));
                    } else {
                        out.push((
                            "total_cases".into(),
                            issue(rule, "fail", format!("통과+실패+보류 {parts} ≠ 총 케이스 {total} / counts ≠ total")),
                        ));
                    }
                }
            }
            schema::RULE_EXEMPT_TOTAL => {
                if let (Some(s), Some(g)) = (values.amount("supply_total"), values.amount("grand_total")) {
                    if close(s, g, 1.0) {
                        out.push(("grand_total".into(), issue(rule, "ok", "공급가액 = 합계(면세)")));
                    } else {
                        out.push((
                            "grand_total".into(),
                            issue(rule, "fail", format!("공급가액 {} ≠ 합계 {} / supply ≠ total", won(s), won(g))),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Collects normalised values from raw model output + validated fields.
/// `scalars` are `(path, normalized)`; `rows` are `(table key, row index, column key, normalized)`.
pub fn values_from(scalars: &[(String, Option<String>)], rows: &[(String, u32, String, Option<String>)]) -> Values {
    let mut v = Values::default();
    for (p, n) in scalars {
        if let Some(n) = n {
            v.scalars.insert(p.clone(), n.clone());
        }
    }
    for (table, row, col, n) in rows {
        let t = v.tables.entry(table.clone()).or_default();
        while t.len() <= *row as usize {
            t.push(BTreeMap::new());
        }
        if let Some(n) = n {
            t[*row as usize].insert(col.clone(), n.clone());
        }
    }
    v
}

// ---------------------------------------------------------------------------
// Evidence lookup
// ---------------------------------------------------------------------------

/// Finds `needle` in `text` ignoring whitespace differences; returns character
/// offsets in `text`.
pub fn find_evidence(text: &str, needle: &str) -> Option<(usize, usize)> {
    let n: Vec<char> = needle.chars().filter(|c| !c.is_whitespace()).collect();
    if n.is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let positions: Vec<usize> = chars.iter().enumerate().filter(|(_, c)| !c.is_whitespace()).map(|(i, _)| i).collect();
    let compact: Vec<char> = positions.iter().map(|&i| chars[i]).collect();
    if n.len() > compact.len() {
        return None;
    }
    let lower = |c: &char| c.to_lowercase().next().unwrap_or(*c);
    let n_l: Vec<char> = n.iter().map(lower).collect();
    let c_l: Vec<char> = compact.iter().map(lower).collect();
    (0..=c_l.len() - n_l.len())
        .find(|&i| c_l[i..i + n_l.len()] == n_l[..])
        .map(|i| (positions[i], positions[i + n_l.len() - 1] + 1))
}

/// `true` when the printed value (digits only for numbers) occurs in the text.
pub fn value_in_text(def: &FieldDef, value: &str, text: &str) -> bool {
    match def.kind {
        FieldKind::Amount | FieldKind::Integer | FieldKind::Brn | FieldKind::Phone | FieldKind::Account => {
            let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() < 5 {
                // "1,000" → "1000" is a substring of almost any page's digit run: exact text only.
                return find_evidence(text, value).is_some();
            }
            let text_digits: String = text.chars().filter(|c| c.is_ascii_digit() || c.is_whitespace()).collect();
            text_digits.replace(char::is_whitespace, "").contains(&digits) || find_evidence(text, value).is_some()
        }
        _ => find_evidence(text, value).is_some(),
    }
}

// ---------------------------------------------------------------------------
// Confidence (§3.4 신뢰도)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct ConfidenceInput {
    /// Model self-report 0..100.
    pub model: Option<u8>,
    /// Evidence string found in the page text (`None` = no text to check against).
    pub evidence_found: Option<bool>,
    /// Value itself found in the page text.
    pub value_found: Option<bool>,
    /// Second sample agreed (`None` = precision mode off).
    pub agreement: Option<bool>,
    pub has_fail: bool,
    pub has_warn: bool,
}

/// Combines the signals into 0..1: the self-report is the base, evidence that
/// can be located raises it, validation failures cap it at 0.4, a disagreeing
/// second sample caps it at 0.5.
pub fn confidence(i: ConfidenceInput) -> f64 {
    let mut c = i.model.map(|m| m as f64 / 100.0).unwrap_or(0.7);
    match (i.evidence_found, i.value_found) {
        (Some(true), _) | (_, Some(true)) => c = (c + 0.05).min(1.0),
        (Some(false), Some(false)) => c *= 0.7,
        (Some(false), None) | (None, Some(false)) => c *= 0.85,
        (None, None) => c *= 0.9,
    }
    if i.has_warn {
        c *= 0.9;
    }
    if i.has_fail {
        c = c.min(0.4);
    }
    match i.agreement {
        Some(true) => c = (c + 0.1).min(1.0),
        Some(false) => c = c.min(0.5),
        None => {}
    }
    (c * 1000.0).round() / 1000.0
}

/// Parses the `value` of a model field object (`{value, evidence, confidence}`)
/// or a bare value (table cells). Booleans become `true` / `false`.
pub fn value_text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() || t.eq_ignore_ascii_case("null") {
                None
            } else {
                Some(t.to_string())
            }
        }
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_dates_numbers_normalise() {
        assert_eq!(normalize_amount("1,250,000원"), Some(1_250_000.0));
        assert_eq!(normalize_amount("₩12,500"), Some(12_500.0));
        assert_eq!(normalize_amount("(3,000)"), Some(-3_000.0));
        assert_eq!(normalize_amount("-500"), Some(-500.0));
        assert_eq!(normalize_amount("12만원"), Some(120_000.0));
        assert_eq!(normalize_amount("1억 2,000만원"), Some(120_000_000.0));
        assert_eq!(normalize_amount("1.5만"), Some(15_000.0));
        assert_eq!(normalize_amount("없음"), None);
        assert_eq!(normalize_integer("3개"), Some(3));
        assert_eq!(normalize_integer("2.5"), None);
        assert_eq!(normalize_date("2026-10-14").as_deref(), Some("2026-10-14"));
        assert_eq!(normalize_date("2026.10.14").as_deref(), Some("2026-10-14"));
        assert_eq!(normalize_date("2026년 10월 14일").as_deref(), Some("2026-10-14"));
        assert_eq!(normalize_date("2026/1/4 12:31").as_deref(), Some("2026-01-04"));
        assert_eq!(normalize_date("26.10.14").as_deref(), Some("2026-10-14"));
        assert_eq!(normalize_date("20261014").as_deref(), Some("2026-10-14"));
        assert_eq!(normalize_date("2026-13-01"), None);
        assert_eq!(normalize_date("언젠가"), None);
        assert_eq!(normalize_phone("010 1234 5678").as_deref(), Some("010-1234-5678"));
        assert_eq!(normalize_phone("021234567").as_deref(), Some("02-123-4567"));
        assert_eq!(format_number(1250000.0), "1250000");
        assert_eq!(format_number(12.5), "12.5");
        assert_eq!(won(1250000.0), "1,250,000");
        assert_eq!(won(-500.0), "-500");
    }

    /// Audit PIP-9: the two-digit-year pattern bit into a four-digit year at the end
    /// ("03/04/2026" → 2003-04-20, "14.10.2026" → 2014-10-20) and put every two-digit year
    /// into the 2000s ("85.03.15" → 2085-03-15), without a warning.
    #[test]
    fn dates_with_the_year_last_and_two_digit_years() {
        assert_eq!(normalize_date("14.10.2026").as_deref(), Some("2026-10-14"), "day first: 14 cannot be a month");
        assert_eq!(normalize_date("10-14-2026").as_deref(), Some("2026-10-14"), "month first: 14 cannot be a month");
        assert_eq!(normalize_date("26.10.2026").as_deref(), Some("2026-10-26"));
        assert_eq!(normalize_date("03/04/2026").as_deref(), Some("2026-03-04"), "slash: month first");
        assert_eq!(normalize_date("04.03.2026").as_deref(), Some("2026-03-04"), "dot: day first");
        assert_eq!(normalize_date("85.03.15").as_deref(), Some("1985-03-15"));
        assert_eq!(normalize_date("26.10.14").as_deref(), Some("2026-10-14"));
        assert_eq!(normalize_date("2026.10.145"), None, "a day followed by another digit is not a date");
        assert_eq!(normalize_date("2026-02-30"), None, "not a calendar day");
        // The century follows the year the document is read in, never more than 20 years ahead.
        assert_eq!(parse_date("46.01.02", 2026), Some(("2046-01-02".into(), false)));
        assert_eq!(parse_date("47.01.02", 2026), Some(("1947-01-02".into(), false)));
        assert_eq!(parse_date("99.12.31", 2101), Some(("2099-12-31".into(), false)));
        // A guessed day / month order is a warning on the field, an unambiguous one is not.
        let date = FieldDef::new("d", "일자", "", FieldKind::Date);
        let (n, issues) = validate_value(&date, "03/04/2026");
        assert_eq!(n.as_deref(), Some("2026-03-04"));
        assert_eq!((issues.len(), issues[0].rule.as_str(), issues[0].level.as_str()), (1, "date_ambiguous", "warn"));
        for clear in ["14.10.2026", "05.05.2026", "2026-03-04", "26.03.04"] {
            assert!(validate_value(&date, clear).1.is_empty(), "{clear}");
        }
    }

    /// Audit PIP-10: the amount normaliser glued every digit of the text together
    /// ("1,100,000원 (VAT 10% 포함)" → 110,000,010) and could not read 천 / 백 / 십 or Korean numerals.
    #[test]
    fn amounts_take_one_token_and_read_korean_units() {
        for (text, expected) in [
            ("1,100,000원 (VAT 10% 포함)", 1_100_000.0),
            ("1식 5,000,000원", 5_000_000.0),
            ("(₩1,100,000)", 1_100_000.0),
            ("3천만원", 30_000_000.0),
            ("금 삼백만원정", 3_000_000.0),
            ("3억 5천만원", 350_000_000.0),
            ("일금 일억이천삼백만원정", 123_000_000.0),
            ("12만 5천원", 125_000.0),
            ("1 250 000원", 1_250_000.0),
            ("합계 12,500", 12_500.0),
            ("총 3건 1,500,000원", 1_500_000.0),
            ("1,100,000원(공급가액 1,000,000원 + 부가세 100,000원)", 1_100_000.0),
            ("△3,000", -3_000.0),
            ("-₩500", -500.0),
            ("만원", 10_000.0),
            ("USD 1,200.50", 1_200.5),
        ] {
            assert_eq!(normalize_amount(text), Some(expected), "{text}");
        }
        // Words made of numeral syllables, several unrelated numbers, dates: not an amount.
        for text in ["삼성전자", "이천", "2 × 4,500 = 9,000", "2026년 3월 2일", "1.250.000", ""] {
            assert_eq!(normalize_amount(text), None, "{text}");
        }
        assert_eq!(normalize_integer("85%"), Some(85));
        assert_eq!(normalize_integer("2 EA"), Some(2));
    }

    #[test]
    fn brn_checksum_matches_the_nts_algorithm() {
        // 220-81-62517 (a well-known valid number) and a one-digit corruption.
        assert!(brn_valid("2208162517"));
        assert!(!brn_valid("2208162518"));
        assert!(!brn_valid("123"));
        assert_eq!(brn_digits("220-81-62517").as_deref(), Some("2208162517"));
        assert_eq!(format_brn("2208162517"), "220-81-62517");
        let def = FieldDef::new("brn", "사업자등록번호", "", FieldKind::Brn);
        let (n, issues) = validate_value(&def, "220-81-62517");
        assert_eq!(n.as_deref(), Some("220-81-62517"));
        assert!(issues.is_empty());
        let (_, issues) = validate_value(&def, "220-81-62518");
        assert_eq!(issues[0].rule, "brn_checksum");
        let (_, issues) = validate_value(&def, "12-34");
        assert_eq!(issues[0].rule, "brn_format");
    }

    #[test]
    fn pii_is_masked_everywhere() {
        assert_eq!(mask_text("주민번호 900101-1234567 입니다"), "주민번호 900101-1****** 입니다");
        assert_eq!(mask_text("9001011234567"), "900101-1******");
        assert_eq!(mask_text("계좌 110-123-456789"), "계좌 110-123-456789", "account numbers need a labelled field");
        assert_eq!(
            mask_text("승인 150000 1500000 합계"),
            "승인 150000 1500000 합계",
            "two numbers are not a resident number"
        );
        assert_eq!(mask_text("900101 - 1234567"), "900101-1******");
        assert!(contains_rrn("850315-2345678"));
        assert!(!contains_rrn("2026-10-14 12:31"));
        assert_eq!(mask_account("110-123-456789"), "***-***-**6789");
        let rrn = FieldDef::new("rrn", "주민등록번호", "", FieldKind::Rrn);
        assert_eq!(mask_value(&rrn, "900101-1234567"), "900101-1******");
        assert_eq!(mask_value(&rrn, "900101 1234567"), "900101-1******");
        let (n, _) = validate_value(&rrn, "900101-1234567");
        assert_eq!(n.as_deref(), Some("900101-1******"));
        let birth = FieldDef::new("birth", "생년월일", "", FieldKind::Date).masked();
        assert_eq!(mask_value(&birth, "1990.01.01"), "1990-**-**");
        let (n, issues) = validate_value(&birth, "1990.01.01");
        assert_eq!((n.as_deref(), issues.len()), (Some("1990-**-**"), 0), "normalised form is masked too");
        let (n, issues) = validate_value(&birth, "1990-**-**");
        assert_eq!((n.as_deref(), issues.len()), (Some("1990-**-**"), 0), "re-validating a masked value is a no-op");
        let s = FieldDef::new("s", "비밀", "", FieldKind::String).masked();
        assert_eq!(mask_value(&s, "ABCDEFGH"), "****EFGH");
    }

    #[test]
    fn field_validation_by_kind() {
        let amount = FieldDef::new("a", "금액", "", FieldKind::Amount);
        assert_eq!(validate_value(&amount, "1,000원").0.as_deref(), Some("1000"));
        assert_eq!(validate_value(&amount, "n/a").1[0].level, "fail");
        let date = FieldDef::new("d", "일자", "", FieldKind::Date);
        assert_eq!(validate_value(&date, "2026년 3월 1일").0.as_deref(), Some("2026-03-01"));
        let en = FieldDef::new("e", "선택", "", FieldKind::Enum).options(&["과세", "영세율"]);
        assert_eq!(validate_value(&en, "영세").0.as_deref(), Some("영세율"));
        assert_eq!(validate_value(&en, "면세").1[0].rule, "enum_option");
        let b = FieldDef::new("b", "여부", "", FieldKind::Boolean);
        assert_eq!(validate_value(&b, "☑").0.as_deref(), Some("true"));
        assert_eq!(validate_value(&b, "아니오").0.as_deref(), Some("false"));
        let card = FieldDef::new("c", "카드", "", FieldKind::String).regex("^[0-9]{4}$");
        assert!(validate_value(&card, "1234").1.is_empty());
        assert_eq!(validate_value(&card, "12345").1[0].rule, "regex_hint");
        let email = FieldDef::new("m", "메일", "", FieldKind::Email);
        assert_eq!(validate_value(&email, "A@B.kr").0.as_deref(), Some("a@b.kr"));
        assert_eq!(validate_value(&email, "nope").1[0].rule, "email_format");
    }

    #[test]
    fn cross_field_rules_detect_mismatches() {
        let def = &schema::builtin("tax_invoice").unwrap().def;
        let ok = values_from(
            &[
                ("supply_total".into(), Some("1000000".into())),
                ("tax_total".into(), Some("100000".into())),
                ("grand_total".into(), Some("1100000".into())),
                ("tax_type".into(), Some("과세".into())),
            ],
            &[
                ("items".into(), 0, "supply_amount".into(), Some("600000".into())),
                ("items".into(), 1, "supply_amount".into(), Some("400000".into())),
            ],
        );
        let r = document_rules(def, &ok);
        assert!(r.iter().all(|(_, i)| i.level == "ok"), "{r:?}");
        let bad = values_from(
            &[
                ("supply_total".into(), Some("1000000".into())),
                ("tax_total".into(), Some("50000".into())),
                ("grand_total".into(), Some("1100000".into())),
                ("tax_type".into(), Some("영세율".into())),
            ],
            &[],
        );
        let r = document_rules(def, &bad);
        let fails: Vec<&str> = r.iter().filter(|(_, i)| i.level == "fail").map(|(p, _)| p.as_str()).collect();
        assert!(fails.contains(&"grand_total") && fails.contains(&"tax_total"), "{r:?}");
        let receipt = &schema::builtin("receipt").unwrap().def;
        let v = values_from(
            &[
                ("supply_amount".into(), Some("10000".into())),
                ("vat".into(), Some("1000".into())),
                ("total".into(), Some("12000".into())),
            ],
            &[("items".into(), 0, "amount".into(), Some("12000".into()))],
        );
        let r = document_rules(receipt, &v);
        assert!(r.iter().any(|(p, i)| p == "total" && i.level == "fail"));
        assert!(r.iter().any(|(p, i)| p == "total" && i.rule == schema::RULE_ITEMS_SUM_TOTAL && i.level == "ok"));
        let contract = &schema::builtin("contract").unwrap().def;
        let v = values_from(
            &[("period_start".into(), Some("2026-12-01".into())), ("period_end".into(), Some("2026-01-01".into()))],
            &[],
        );
        assert_eq!(document_rules(contract, &v)[0].1.level, "fail");
    }

    #[test]
    fn evidence_lookup_ignores_whitespace_and_case() {
        let text = "합계 금액\n1,250,000 원\nSupplier: Hanbit Co.";
        assert_eq!(find_evidence(text, "합계금액 1,250,000원"), Some((0, 17)));
        assert!(find_evidence(text, "hanbit co").is_some());
        assert!(find_evidence(text, "없는 문장").is_none());
        let amount = FieldDef::new("a", "금액", "", FieldKind::Amount);
        assert!(value_in_text(&amount, "1250000", text));
        assert!(!value_in_text(&amount, "999", text));
        let s = FieldDef::new("s", "상호", "", FieldKind::String);
        assert!(value_in_text(&s, "Hanbit", text));
    }

    #[test]
    fn confidence_formula() {
        let base = ConfidenceInput { model: Some(90), evidence_found: Some(true), ..Default::default() };
        assert_eq!(confidence(base), 0.95);
        let failing = ConfidenceInput { has_fail: true, ..base };
        assert_eq!(confidence(failing), 0.4);
        let disagree = ConfidenceInput { agreement: Some(false), ..base };
        assert_eq!(confidence(disagree), 0.5);
        let unverifiable =
            ConfidenceInput { model: None, evidence_found: None, value_found: None, ..Default::default() };
        assert_eq!(confidence(unverifiable), 0.63);
        let missing_evidence = ConfidenceInput {
            model: Some(80),
            evidence_found: Some(false),
            value_found: Some(false),
            ..Default::default()
        };
        assert_eq!(confidence(missing_evidence), 0.56);
        assert_eq!(value_text(&Value::Bool(true)).as_deref(), Some("true"));
        assert_eq!(value_text(&Value::String(" null ".into())), None);
    }
}
