//! OCR (§3.4 "VLM 2차"): page image → Markdown with HTML tables and the
//! structural tags; density check for the fine-print re-pass; table parsing
//! for the viewer / exports.

use crate::engine::client::user_message_with_image;
use crate::error::Result;

use super::prompts::{self, Stage};
use super::vlm::{Reply, Vlm};
use super::ExtractedTable;

/// Runs OCR on one page image. `max_tokens` lets the fine-print pass widen the budget.
pub fn ocr_page(vlm: &Vlm<'_>, image_url: &str, max_tokens: Option<u32>) -> Result<(Option<String>, Reply)> {
    let messages = vec![
        serde_json::json!({ "role": "system", "content": prompts::SYSTEM_OCR }),
        user_message_with_image(&prompts::ocr(), image_url),
    ];
    let reply = vlm.call(Stage::Ocr, &messages, None, max_tokens)?;
    let text = clean(&reply.content);
    Ok((text, reply))
}

/// Strips code fences / a trailing fence and maps the `null` answer to `None`.
pub fn clean(raw: &str) -> Option<String> {
    let mut t = raw.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.trim_start_matches(|c: char| c.is_ascii_alphabetic());
        t = rest.trim_start();
        if let Some(stripped) = t.strip_suffix("```") {
            t = stripped;
        }
    }
    let t = t.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("null") || t == "`null`" {
        return None;
    }
    Some(t.to_string())
}

/// Number of table cells (`<td>` + `<th>`) — the §3.4 density signal.
pub fn table_cells(text: &str) -> usize {
    let lower = text.to_ascii_lowercase();
    lower.matches("<td").count() + lower.matches("<th").count()
}

/// Parses HTML tables (and simple Markdown pipe tables) out of a page text.
pub fn parse_tables(text: &str, page: u32) -> Vec<ExtractedTable> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(start) = lower[pos..].find("<table") {
        let start = pos + start;
        let Some(end_rel) = lower[start..].find("</table>") else { break };
        let end = start + end_rel;
        let body = &text[start..end];
        let mut table = ExtractedTable { page, index: out.len() as u32, header: Vec::new(), rows: Vec::new() };
        let body_lower = body.to_ascii_lowercase();
        let mut rpos = 0;
        while let Some(tr) = body_lower[rpos..].find("<tr") {
            let tr = rpos + tr;
            let Some(tr_end_rel) = body_lower[tr..].find("</tr>") else { break };
            let tr_end = tr + tr_end_rel;
            let row_html = &body[tr..tr_end];
            let row_lower = &body_lower[tr..tr_end];
            let mut cells = Vec::new();
            let mut header = false;
            let mut cpos = 0;
            loop {
                let td = row_lower[cpos..].find("<td");
                let th = row_lower[cpos..].find("<th");
                let (idx, is_th) = match (td, th) {
                    (Some(a), Some(b)) => {
                        if a < b {
                            (a, false)
                        } else {
                            (b, true)
                        }
                    }
                    (Some(a), None) => (a, false),
                    (None, Some(b)) => (b, true),
                    (None, None) => break,
                };
                let open = cpos + idx;
                let Some(gt) = row_lower[open..].find('>') else { break };
                let content_start = open + gt + 1;
                let close_tag = if is_th { "</th>" } else { "</td>" };
                let content_end =
                    row_lower[content_start..].find(close_tag).map(|e| content_start + e).unwrap_or(row_lower.len());
                let raw = &row_html[content_start..content_end];
                let span = row_lower[open..content_start]
                    .split("colspan=")
                    .nth(1)
                    .and_then(|s| {
                        s.trim_matches(|c| c == '"' || c == '\'' || c == ' ')
                            .split(|c: char| !c.is_ascii_digit())
                            .next()?
                            .parse::<usize>()
                            .ok()
                    })
                    .unwrap_or(1)
                    .clamp(1, 20);
                let cell = strip_tags(raw);
                header |= is_th;
                cells.push(cell);
                for _ in 1..span {
                    cells.push(String::new());
                }
                cpos = content_end.min(row_lower.len());
                if cpos >= row_lower.len() {
                    break;
                }
            }
            if !cells.is_empty() {
                if header && table.header.is_empty() && table.rows.is_empty() {
                    table.header = cells;
                } else {
                    table.rows.push(cells);
                }
            }
            rpos = tr_end + 5;
        }
        if !table.header.is_empty() || !table.rows.is_empty() {
            out.push(table);
        }
        pos = end + 8;
    }
    // Markdown pipe tables.
    let mut current: Option<ExtractedTable> = None;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('|') && l.ends_with('|') && l.len() > 2 {
            let cells: Vec<String> = l[1..l.len() - 1].split('|').map(|c| c.trim().to_string()).collect();
            if cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')) {
                continue; // separator row
            }
            let t = current.get_or_insert_with(|| ExtractedTable {
                page,
                index: out.len() as u32,
                header: Vec::new(),
                rows: Vec::new(),
            });
            if t.header.is_empty() && t.rows.is_empty() {
                t.header = cells;
            } else {
                t.rows.push(cells);
            }
        } else if let Some(t) = current.take() {
            if !t.rows.is_empty() {
                out.push(t);
            }
        }
    }
    if let Some(t) = current.take() {
        if !t.rows.is_empty() {
            out.push(t);
        }
    }
    for (i, t) in out.iter_mut().enumerate() {
        t.index = i as u32;
    }
    out
}

/// Removes HTML tags, decodes the few entities OCR produces, collapses whitespace.
pub fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// CSV text of a table (RFC 4180 quoting, UTF-8 BOM for Excel).
pub fn table_csv(t: &ExtractedTable) -> String {
    let mut s = String::from("\u{feff}");
    let quote = |c: &str| {
        // Spreadsheet formula injection guard: a cell that would be evaluated (=, @, tab, CR,
        // or +/- not followed by a digit) is stored as text.
        let mut cell = c.to_string();
        let evaluated = cell.starts_with(['=', '@', '\t', '\r'])
            || (cell.starts_with(['+', '-']) && !cell[1..].starts_with(|ch: char| ch.is_ascii_digit()));
        if evaluated {
            cell.insert(0, '\'');
        }
        if cell.contains(',') || cell.contains('"') || cell.contains('\n') {
            format!("\"{}\"", cell.replace('"', "\"\""))
        } else {
            cell
        }
    };
    if !t.header.is_empty() {
        s.push_str(&t.header.iter().map(|c| quote(c)).collect::<Vec<_>>().join(","));
        s.push_str("\r\n");
    }
    for r in &t.rows {
        s.push_str(&r.iter().map(|c| quote(c)).collect::<Vec<_>>().join(","));
        s.push_str("\r\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_fences_and_null() {
        assert_eq!(clean("```markdown\n# 제목\n```"), Some("# 제목".into()));
        assert_eq!(clean(" null "), None);
        assert_eq!(clean(""), None);
        assert_eq!(clean("본문").as_deref(), Some("본문"));
    }

    #[test]
    fn parses_html_and_markdown_tables() {
        let text = "# 영수증\n<table><tr><th>품명</th><th colspan=\"2\">금액</th></tr><tr><td>아메리카노</td><td>4,500</td><td>원</td></tr><tr><td>라떼 &amp; 우유</td><td>5,000</td><td></td></tr></table>\n\n| 항목 | 값 |\n|---|---|\n| 합계 | 9,500 |\n";
        let tables = parse_tables(text, 1);
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].header, vec!["품명", "금액", ""]);
        assert_eq!(tables[0].rows[1], vec!["라떼 & 우유", "5,000", ""]);
        assert_eq!(tables[1].header, vec!["항목", "값"]);
        assert_eq!(tables[1].rows, vec![vec!["합계", "9,500"]]);
        assert_eq!(table_cells(text), 8);
        let csv = table_csv(&tables[0]);
        assert!(csv.starts_with('\u{feff}') && csv.contains("라떼 & 우유,\"5,000\","));
        let hostile = ExtractedTable {
            page: 1,
            index: 0,
            header: vec![],
            rows: vec![vec!["=SUM(A1)".into(), "-500".into(), "+cmd".into()]],
        };
        assert!(table_csv(&hostile).contains("'=SUM(A1),-500,'+cmd"));
    }
}
