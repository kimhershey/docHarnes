//! File-name / folder templates (design §3.7 정리/내보내기): `{{year}}`,
//! `{{month}}`, `{{day}}`, `{{doc_type}}`, `{{vendor}}`, `{{total}}`,
//! `{{title}}`, `{{original_name}}`, `{{tags}}`, `{{seq}}` (+ `{{name}}`, the
//! original stem, and `{{ext}}`). Unknown values render as `미상` and flag the
//! document for review; rendered components are NFC, free of Windows-forbidden
//! characters and reserved names, and the whole path stays ≤ 240 characters.

use unicode_normalization::UnicodeNormalization;

pub const VARIABLES: &[&str] =
    &["year", "month", "day", "doc_type", "vendor", "total", "title", "original_name", "name", "tags", "seq", "ext"];

/// Placeholder for a missing value (design: "null은 `미상` + 검토 필요 표시").
pub const UNKNOWN: &str = "미상";
/// Windows `MAX_PATH` headroom: workspace root + relative path must stay short.
pub const MAX_PATH_CHARS: usize = 240;
pub const MAX_COMPONENT_CHARS: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Lit(String),
    Var(String),
}

/// Splits a template into literal runs and `{{var}}` tokens (unknown names stay
/// tokens so `unknown_variables` can report them).
pub fn parse(template: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        lit.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim().to_string();
                if !lit.is_empty() {
                    out.push(Token::Lit(std::mem::take(&mut lit)));
                }
                out.push(Token::Var(name));
                rest = &after[end + 2..];
            }
            None => {
                lit.push_str("{{");
                rest = after;
            }
        }
    }
    lit.push_str(rest);
    if !lit.is_empty() {
        out.push(Token::Lit(lit));
    }
    out
}

/// Variable names in the template that are not in [`VARIABLES`].
pub fn unknown_variables(template: &str) -> Vec<String> {
    let mut out: Vec<String> = parse(template)
        .into_iter()
        .filter_map(|t| match t {
            Token::Var(v) if !VARIABLES.contains(&v.as_str()) => Some(v),
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Variables used by the template.
pub fn variables(template: &str) -> Vec<String> {
    parse(template)
        .into_iter()
        .filter_map(|t| match t {
            Token::Var(v) => Some(v),
            _ => None,
        })
        .collect()
}

/// Renders a template. `lookup` returns the value of a variable (`None` =
/// unknown → `미상`). The second item lists the variables that were missing.
pub fn render(template: &str, lookup: &dyn Fn(&str) -> Option<String>) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut missing = Vec::new();
    for token in parse(template) {
        match token {
            Token::Lit(s) => out.push_str(&s),
            Token::Var(v) => match lookup(&v) {
                Some(value) if !value.trim().is_empty() => out.push_str(&sanitize_component(&value)),
                _ => {
                    out.push_str(UNKNOWN);
                    missing.push(v);
                }
            },
        }
    }
    (out, missing)
}

const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2",
    "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Makes one path component safe for Windows / NTFS and ext4: NFC, forbidden
/// characters (`< > : " / \ | ? *`, controls) → `_`, no trailing dots / spaces,
/// reserved device names prefixed, length-capped.
pub fn sanitize_component(s: &str) -> String {
    let mut out: String =
        s.nfc()
            .map(|c| {
                if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
    out = out.trim().trim_end_matches(['.', ' ']).to_string();
    // Hidden / app folders (".doculens") must not be produced by a rendered value.
    while out.starts_with('.') {
        out.replace_range(..1, "_");
    }
    if out.is_empty() {
        out = "_".into();
    }
    let stem = out.split('.').next().unwrap_or("").to_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        out = format!("_{out}");
    }
    if out.chars().count() > MAX_COMPONENT_CHARS {
        out = out.chars().take(MAX_COMPONENT_CHARS).collect();
    }
    out
}

/// Sanitises every component of a relative folder path (`/`-separated), dropping
/// empty, `.` and `..` segments.
pub fn sanitize_folder(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .map(str::trim)
        .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        .map(sanitize_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// Two-digit month / day, four-digit year from an ISO date (`2026-10-14`).
pub fn date_parts(iso: &str) -> Option<(String, String, String)> {
    let mut it = iso.splitn(3, '-');
    let y = it.next()?.trim();
    let m = it.next()?.trim();
    let d = it.next().map(|d| d.trim().chars().take(2).collect::<String>()).unwrap_or_default();
    if y.len() != 4 || !y.chars().all(|c| c.is_ascii_digit()) || m.is_empty() {
        return None;
    }
    let m: u32 = m.parse().ok()?;
    let d: u32 = d.parse().unwrap_or(0);
    Some((y.to_string(), format!("{m:02}"), if d == 0 { String::new() } else { format!("{d:02}") }))
}

/// Amount formatting for file names: integer with thousands separators (`12,500`).
pub fn format_total(amount: f64) -> String {
    let neg = amount < 0.0;
    let n = amount.abs().round() as u64;
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_reports_unknown_variables() {
        let t = parse("{{year}}-{{month}}_{{vendor}}{{bogus}}.pdf");
        assert_eq!(t.len(), 7);
        assert_eq!(t[5], Token::Var("bogus".into()));
        assert_eq!(unknown_variables("{{year}}/{{bogus}}/{{nope}}"), vec!["bogus".to_string(), "nope".into()]);
        assert!(unknown_variables("plain {{ year }}").is_empty());
        assert_eq!(parse("{{open"), vec![Token::Lit("{{open".into())]);
    }

    #[test]
    fn renders_with_unknown_placeholder() {
        let (s, missing) = render("{{year}}-{{month}}_{{vendor}}_{{total}}", &|v| match v {
            "year" => Some("2026".into()),
            "month" => Some("10".into()),
            "total" => Some("12,500".into()),
            _ => None,
        });
        assert_eq!(s, "2026-10_미상_12,500");
        assert_eq!(missing, vec!["vendor".to_string()]);
    }

    #[test]
    fn sanitises_components() {
        assert_eq!(sanitize_component("a<b>:c\"d/e\\f|g?h*i"), "a_b__c_d_e_f_g_h_i");
        assert_eq!(sanitize_component("name. . "), "name");
        assert_eq!(sanitize_component("CON"), "_CON");
        assert_eq!(sanitize_component("con.txt"), "_con.txt");
        assert_eq!(sanitize_component("   "), "_");
        assert_eq!(sanitize_folder("documents\\2026/../10//영수증"), "documents/2026/10/영수증");
        // NFD → NFC
        assert_eq!(sanitize_component("한\u{1112}\u{1161}\u{11AB}"), "한한");
    }

    #[test]
    fn date_and_total_helpers() {
        assert_eq!(date_parts("2026-10-14"), Some(("2026".into(), "10".into(), "14".into())));
        assert_eq!(date_parts("2026-1"), Some(("2026".into(), "01".into(), "".into())));
        assert_eq!(date_parts("oct 2026"), None);
        assert_eq!(format_total(12500.0), "12,500");
        assert_eq!(format_total(-1234567.4), "-1,234,567");
        assert_eq!(format_total(999.0), "999");
    }
}
