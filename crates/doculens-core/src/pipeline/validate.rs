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

/// Parses a printed amount: `1,250,000원`, `₩12,500`, `(3,000)` (negative),
/// `-500`, `12만원`, `1억 2,000만원`, `1.5만`. Returns `None` when no digit is found.
pub fn normalize_amount(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let mut negative =
        t.starts_with('-') || t.starts_with('(') && t.ends_with(')') || t.starts_with('△') || t.starts_with('▲');
    let cleaned: String = t
        .chars()
        .filter(|c| !matches!(c, ',' | ' ' | '\u{a0}' | '₩' | '\\' | '$' | '원' | '정' | '(' | ')' | '△' | '▲'))
        .collect();
    let cleaned = cleaned.trim_end_matches("KRW").trim_end_matches("krw").trim().to_string();
    let cleaned = cleaned
        .strip_prefix('-')
        .map(|r| {
            negative = true;
            r.to_string()
        })
        .unwrap_or(cleaned);
    // Korean magnitude words.
    if cleaned.contains('억') || cleaned.contains('만') {
        let mut total = 0f64;
        let mut rest = cleaned.as_str();
        if let Some((a, r)) = rest.split_once('억') {
            total += a.trim().parse::<f64>().ok()? * 100_000_000.0;
            rest = r;
        }
        if let Some((m, r)) = rest.split_once('만') {
            total += m.trim().parse::<f64>().ok()? * 10_000.0;
            rest = r;
        }
        let rest = rest.trim();
        if !rest.is_empty() {
            total += rest.parse::<f64>().ok()?;
        }
        return Some(if negative { -total } else { total });
    }
    let digits: String = cleaned.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    if digits.is_empty() || !digits.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let v: f64 = digits.trim_matches('.').parse().ok()?;
    Some(if negative { -v } else { v })
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
/// `20261014`, `2026-10-14 12:31`). Month/day order is year-first only.
pub fn normalize_date(s: &str) -> Option<String> {
    static FULL: OnceLock<Regex> = OnceLock::new();
    static SHORT: OnceLock<Regex> = OnceLock::new();
    static COMPACT: OnceLock<Regex> = OnceLock::new();
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let make = |y: i64, m: i64, d: i64| -> Option<String> {
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1900..=2100).contains(&y) {
            return None;
        }
        Some(format!("{y:04}-{m:02}-{d:02}"))
    };
    let full = re(&FULL, r"(?P<y>(?:19|20)\d{2})\s*[.\-/년]\s*(?P<m>\d{1,2})\s*[.\-/월]\s*(?P<d>\d{1,2})\s*일?");
    if let Some(c) = full.captures(t) {
        return make(c["y"].parse().ok()?, c["m"].parse().ok()?, c["d"].parse().ok()?);
    }
    let compact = re(&COMPACT, r"\b(?P<y>(?:19|20)\d{2})(?P<m>\d{2})(?P<d>\d{2})\b");
    if let Some(c) = compact.captures(t) {
        return make(c["y"].parse().ok()?, c["m"].parse().ok()?, c["d"].parse().ok()?);
    }
    let short = re(&SHORT, r"\b(?P<y>\d{2})\s*[.\-/년]\s*(?P<m>\d{1,2})\s*[.\-/월]\s*(?P<d>\d{1,2})\s*일?");
    if let Some(c) = short.captures(t) {
        let y: i64 = c["y"].parse().ok()?;
        return make(2000 + y, c["m"].parse().ok()?, c["d"].parse().ok()?);
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
    // 6 digits, optional separator, gender digit 1-8 + 6 digits; not inside a longer digit run.
    re(&RRN, r"(?P<a>(?:^|[^0-9]))(?P<ymd>\d{6})\s*[-–]?\s*(?P<g>[1-8])(?P<rest>\d{6})(?P<z>[^0-9]|$)")
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
        FieldKind::Date => match normalize_date(v) {
            Some(d) => Some(d),
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
            if digits.len() < 2 {
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
        let def = &schema::builtins()[0].def; // tax invoice
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
        let receipt = &schema::builtins()[2].def;
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
        let contract = &schema::builtins()[4].def;
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
