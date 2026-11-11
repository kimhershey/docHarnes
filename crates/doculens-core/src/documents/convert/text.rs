//! Plain-text family reader (txt / md / csv / tsv / log / json / xml / html):
//! bytes are decoded (UTF-8 with or without BOM, UTF-16 with BOM, otherwise
//! CP949 — the Korean legacy code page) and shaped into the model: CSV / TSV
//! become a table, Markdown keeps headings / lists / tables, HTML is stripped
//! to headings and paragraphs, JSON / XML / logs keep their lines.

use std::path::Path;

use crate::error::{CoreError, Result};

use super::model::{Block, Cell, DocModel, ListKind, Paragraph, Run, Table};

/// Text beyond this is dropped with a note.
pub const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TABLE_ROWS: usize = 5_000;
const MAX_TABLE_COLS: usize = 64;
/// Lines per paragraph for pre-formatted content (JSON / XML / logs).
const PRE_LINES_PER_BLOCK: usize = 120;

/// Decodes bytes: BOM → UTF-8 / UTF-16, valid UTF-8, else CP949 (EUC-KR superset), else lossy UTF-8.
/// Returns the text and the encoding label used.
pub fn decode(bytes: &[u8]) -> (String, &'static str) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(rest).into_owned(), "utf-8");
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let enc = if bytes[0] == 0xFF { encoding_rs::UTF_16LE } else { encoding_rs::UTF_16BE };
        let (text, _, _) = enc.decode(&bytes[2..]);
        return (text.into_owned(), "utf-16");
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_string(), "utf-8");
    }
    let (text, _, had_errors) = encoding_rs::EUC_KR.decode(bytes);
    if !had_errors {
        return (text.into_owned(), "cp949");
    }
    (String::from_utf8_lossy(bytes).into_owned(), "utf-8 (lossy)")
}

/// A paragraph whose lines are joined with hard breaks.
fn lines_paragraph(lines: &[&str]) -> Paragraph {
    let n = lines.len();
    Paragraph {
        runs: lines
            .iter()
            .enumerate()
            .map(|(i, l)| Run { text: l.to_string(), line_break: i + 1 < n, ..Default::default() })
            .collect(),
        ..Default::default()
    }
}

/// Blank-line separated blocks → paragraphs; the first short line is the title.
fn plain_model(text: &str) -> DocModel {
    let mut model = DocModel::default();
    let mut block: Vec<&str> = Vec::new();
    let flush = |block: &mut Vec<&str>, model: &mut DocModel| {
        if !block.is_empty() {
            model.blocks.push(Block::Paragraph(lines_paragraph(block)));
            block.clear();
        }
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            flush(&mut block, &mut model);
        } else {
            block.push(line.trim_end());
        }
    }
    flush(&mut block, &mut model);
    model.title =
        text.lines().map(str::trim).find(|l| !l.is_empty()).filter(|l| l.chars().count() <= 120).map(str::to_string);
    model
}

/// Pre-formatted content: fixed-size groups of lines, no title.
fn pre_model(text: &str) -> DocModel {
    let mut model = DocModel::default();
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    for chunk in lines.chunks(PRE_LINES_PER_BLOCK) {
        if chunk.iter().any(|l| !l.trim().is_empty()) {
            model.blocks.push(Block::Paragraph(lines_paragraph(chunk)));
        }
    }
    model
}

/// RFC 4180-ish parser: quoted fields with `""` escapes and embedded newlines.
fn parse_delimited(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            c if c == delim => row.push(std::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    rows
}

/// The delimiter that splits the first line into the most fields (`,` `;` tab).
fn guess_delimiter(text: &str) -> char {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    [',', ';', '\t'].into_iter().max_by_key(|d| first.matches(*d).count()).unwrap_or(',')
}

fn rows_to_table(rows: Vec<Vec<String>>, header: bool, notes: &mut Vec<String>) -> Table {
    let total_rows = rows.len();
    let total_cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if total_rows > MAX_TABLE_ROWS {
        notes.push(format!("{total_rows}행 중 {MAX_TABLE_ROWS}행까지만 변환"));
    }
    if total_cols > MAX_TABLE_COLS {
        notes.push(format!("{total_cols}열 중 {MAX_TABLE_COLS}열까지만 변환"));
    }
    let cols = total_cols.min(MAX_TABLE_COLS);
    let rows: Vec<Vec<Cell>> = rows
        .into_iter()
        .take(MAX_TABLE_ROWS)
        .enumerate()
        .map(|(i, r)| {
            (0..cols)
                .map(|c| {
                    let text = r.get(c).map(|s| s.trim()).unwrap_or("");
                    Cell {
                        blocks: if text.is_empty() {
                            Vec::new()
                        } else {
                            vec![Block::Paragraph(Paragraph::plain(text))]
                        },
                        row_span: 1,
                        col_span: 1,
                        header: header && i == 0,
                    }
                })
                .collect()
        })
        .collect();
    Table { rows, col_widths_mm: Vec::new() }
}

fn csv_model(text: &str, delim: char) -> DocModel {
    let mut model = DocModel::default();
    let rows = parse_delimited(text, delim);
    if rows.is_empty() {
        return model;
    }
    let table = rows_to_table(rows, true, &mut model.notes);
    model.blocks.push(Block::Table(table));
    model
}

/// Strips `**` / `__` / backticks and turns `[text](url)` into `text`.
fn inline_markdown(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '_' | '`' => {
                while chars.peek() == Some(&c) {
                    chars.next();
                }
            }
            '[' => {
                let mut label = String::new();
                let mut closed = false;
                for d in chars.by_ref() {
                    if d == ']' {
                        closed = true;
                        break;
                    }
                    label.push(d);
                }
                if closed && chars.peek() == Some(&'(') {
                    for d in chars.by_ref() {
                        if d == ')' {
                            break;
                        }
                    }
                    out.push_str(&inline_markdown(&label));
                } else {
                    out.push('[');
                    out.push_str(&label);
                    if closed {
                        out.push(']');
                    }
                }
            }
            c => out.push(c),
        }
    }
    out
}

fn markdown_model(text: &str) -> DocModel {
    let mut model = DocModel::default();
    let mut para: Vec<String> = Vec::new();
    let mut table: Vec<Vec<String>> = Vec::new();
    let mut code: Option<Vec<String>> = None;
    fn flush_para(para: &mut Vec<String>, model: &mut DocModel) {
        if !para.is_empty() {
            let refs: Vec<&str> = para.iter().map(String::as_str).collect();
            model.blocks.push(Block::Paragraph(lines_paragraph(&refs)));
            para.clear();
        }
    }
    fn flush_table(table: &mut Vec<Vec<String>>, model: &mut DocModel) {
        if !table.is_empty() {
            let rows = std::mem::take(table);
            model.blocks.push(Block::Table(rows_to_table(rows, true, &mut model.notes)));
        }
    }
    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(buf) = code.as_mut() {
            if line.trim_start().starts_with("```") {
                let refs: Vec<&str> = buf.iter().map(String::as_str).collect();
                if !refs.is_empty() {
                    model.blocks.push(Block::Paragraph(lines_paragraph(&refs)));
                }
                code = None;
            } else {
                buf.push(line.to_string());
            }
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            flush_para(&mut para, &mut model);
            flush_table(&mut table, &mut model);
            code = Some(Vec::new());
            continue;
        }
        if trimmed.starts_with('|') {
            flush_para(&mut para, &mut model);
            let cells: Vec<String> = trimmed.trim_matches('|').split('|').map(|c| inline_markdown(c.trim())).collect();
            // Separator rows (`|---|:--:|`) are layout only.
            if !cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')) {
                table.push(cells);
            }
            continue;
        }
        flush_table(&mut table, &mut model);
        if trimmed.is_empty() {
            flush_para(&mut para, &mut model);
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed.chars().nth(hashes) == Some(' ') {
            flush_para(&mut para, &mut model);
            let text = inline_markdown(trimmed[hashes..].trim().trim_end_matches('#').trim());
            if model.title.is_none() {
                model.title = Some(text.clone());
            }
            model.blocks.push(Block::Paragraph(Paragraph { heading: Some(hashes as u8), ..Paragraph::plain(text) }));
            continue;
        }
        let list = if let Some(rest) =
            trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).or_else(|| trimmed.strip_prefix("+ "))
        {
            Some((ListKind::Bullet, rest))
        } else {
            let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0
                && matches!(trimmed.chars().nth(digits), Some('.') | Some(')'))
                && trimmed.chars().nth(digits + 1) == Some(' ')
            {
                Some((ListKind::Number, trimmed[digits + 2..].trim_start()))
            } else {
                None
            }
        };
        if let Some((kind, rest)) = list {
            flush_para(&mut para, &mut model);
            model.blocks.push(Block::Paragraph(Paragraph {
                list: Some(kind),
                ..Paragraph::plain(inline_markdown(rest.trim()))
            }));
            continue;
        }
        let body = trimmed.strip_prefix("> ").unwrap_or(trimmed);
        para.push(inline_markdown(body));
    }
    if let Some(buf) = code {
        let refs: Vec<&str> = buf.iter().map(String::as_str).collect();
        if !refs.is_empty() {
            model.blocks.push(Block::Paragraph(lines_paragraph(&refs)));
        }
    }
    flush_para(&mut para, &mut model);
    flush_table(&mut table, &mut model);
    if model.title.is_none() {
        model.title = model.blocks.iter().find_map(|b| match b {
            Block::Paragraph(p) if !p.is_empty() => {
                Some(p.text().lines().next().unwrap_or("").trim().chars().take(120).collect())
            }
            _ => None,
        });
    }
    model
}

/// Longest character reference looked for after an `&` (bytes, the `&` and the `;` included).
const MAX_ENTITY_BYTES: usize = 12;

/// Decodes the common named and numeric character references.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        // The `;` is searched in the bytes: a `&` followed by Korean text ("R&D 센터") has no char
        // boundary at byte 12, and `;` is ASCII, so its byte index is always a boundary.
        let window = &rest.as_bytes()[..rest.len().min(MAX_ENTITY_BYTES)];
        let Some(end) = window.iter().position(|b| *b == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            "middot" => Some('·'),
            "hellip" => Some('…'),
            "ndash" => Some('–'),
            "mdash" => Some('—'),
            "lsquo" => Some('‘'),
            "rsquo" => Some('’'),
            "ldquo" => Some('“'),
            "rdquo" => Some('”'),
            "bull" => Some('•'),
            "times" => Some('×'),
            "copy" => Some('©'),
            "reg" => Some('®'),
            n if n.starts_with("#x") || n.starts_with("#X") => {
                u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32)
            }
            n if n.starts_with('#') => n[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Removes `<tag …>…</tag>` blocks (case-insensitive) such as scripts and styles.
fn strip_element(html: &str, tag: &str) -> String {
    // ASCII lower-casing keeps every byte offset valid for `html` (Unicode lower-casing changes
    // the length of some characters, e.g. `İ`, and the offsets would then split a character).
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    while let Some(start) = lower[pos..].find(&open) {
        let start = pos + start;
        out.push_str(&html[pos..start]);
        match lower[start..].find(&close) {
            Some(end) => pos = start + end + close.len(),
            None => return out,
        }
    }
    out.push_str(&html[pos..]);
    out
}

fn html_model(html: &str) -> DocModel {
    let mut model = DocModel::default();
    let body = strip_element(&strip_element(html, "script"), "style");
    let lower = body.to_ascii_lowercase();
    if let Some(s) = lower.find("<title") {
        if let Some(gt) = lower[s..].find('>') {
            if let Some(e) = lower[s + gt..].find("</title>") {
                let t = decode_entities(body[s + gt + 1..s + gt + e].trim());
                if !t.is_empty() {
                    model.title = Some(t.chars().take(120).collect());
                }
            }
        }
    }
    // Linearise: text with structural newlines, headings and list items marked.
    #[derive(Default)]
    struct Line {
        text: String,
        heading: Option<u8>,
        list: Option<ListKind>,
    }
    let mut lines: Vec<Line> = vec![Line::default()];
    let mut heading: Option<u8> = None;
    let mut list: Option<ListKind> = None;
    let mut in_head = false;
    let mut chars = body.char_indices().peekable();
    let push_break = |lines: &mut Vec<Line>, heading: Option<u8>, list: Option<ListKind>| {
        if !lines.last().map(|l| l.text.trim().is_empty()).unwrap_or(true) || heading.is_some() {
            lines.push(Line { text: String::new(), heading, list });
        } else if let Some(l) = lines.last_mut() {
            l.heading = heading;
            l.list = list;
        }
    };
    while let Some((i, c)) = chars.next() {
        if c != '<' {
            if !in_head {
                let cur = lines.last_mut().unwrap();
                cur.text.push(if c.is_whitespace() { ' ' } else { c });
            }
            continue;
        }
        let Some(end) = body[i..].find('>') else { break };
        let tag = &body[i + 1..i + end];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        let closing = tag.starts_with('/');
        match name.as_str() {
            "head" => in_head = !closing,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = name[1..].parse::<u8>().ok();
                if closing {
                    heading = None;
                    push_break(&mut lines, None, None);
                } else {
                    heading = level;
                    push_break(&mut lines, level, None);
                }
            }
            "li" => {
                if closing {
                    push_break(&mut lines, None, None);
                } else {
                    push_break(&mut lines, heading, Some(list.unwrap_or(ListKind::Bullet)));
                }
            }
            "ol" => list = if closing { None } else { Some(ListKind::Number) },
            "ul" => list = if closing { None } else { Some(ListKind::Bullet) },
            "p" | "div" | "br" | "tr" | "section" | "article" | "table" | "blockquote" | "pre" | "hr" => {
                push_break(&mut lines, heading, None)
            }
            "td" | "th" if closing => lines.last_mut().unwrap().text.push('\t'),
            _ => {}
        }
        // Skip to the end of the tag.
        while let Some((j, _)) = chars.peek() {
            if *j >= i + end {
                chars.next();
                break;
            }
            chars.next();
        }
    }
    let mut para: Vec<String> = Vec::new();
    let flush = |para: &mut Vec<String>, model: &mut DocModel| {
        if !para.is_empty() {
            let refs: Vec<&str> = para.iter().map(String::as_str).collect();
            model.blocks.push(Block::Paragraph(lines_paragraph(&refs)));
            para.clear();
        }
    };
    for line in lines {
        // Collapse runs of spaces but keep the tabs that separate table cells.
        let collapsed = line
            .text
            .split('\t')
            .map(|seg| seg.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\t");
        let text = decode_entities(collapsed.trim());
        if text.is_empty() {
            flush(&mut para, &mut model);
            continue;
        }
        if line.heading.is_some() || line.list.is_some() {
            flush(&mut para, &mut model);
            model.blocks.push(Block::Paragraph(Paragraph {
                heading: line.heading,
                list: line.list,
                ..Paragraph::plain(text)
            }));
        } else {
            para.push(text);
        }
    }
    flush(&mut para, &mut model);
    if model.title.is_none() {
        model.title = model.blocks.iter().find_map(|b| match b {
            Block::Paragraph(p) if !p.is_empty() => {
                Some(p.text().lines().next().unwrap_or("").trim().chars().take(120).collect())
            }
            _ => None,
        });
    }
    model
}

pub fn read(path: &Path) -> Result<DocModel> {
    let meta = std::fs::metadata(path).map_err(|e| CoreError::io(path, e))?;
    let truncated = meta.len() > MAX_TEXT_BYTES;
    let bytes = if truncated {
        use std::io::Read;
        let mut buf = vec![0u8; MAX_TEXT_BYTES as usize];
        let mut f = std::fs::File::open(path).map_err(|e| CoreError::io(path, e))?;
        let n = f.read(&mut buf).map_err(|e| CoreError::io(path, e))?;
        buf.truncate(n);
        buf
    } else {
        std::fs::read(path).map_err(|e| CoreError::io(path, e))?
    };
    let (text, encoding) = decode(&bytes);
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
    let head: String = text.trim_start().chars().take(64).collect::<String>().to_lowercase();
    let looks_html = head.contains("<html") || head.starts_with("<!doctype");
    let mut model = match ext.as_str() {
        "csv" => csv_model(&text, guess_delimiter(&text)),
        "tsv" => csv_model(&text, '\t'),
        "md" | "markdown" => markdown_model(&text),
        "html" | "htm" => html_model(&text),
        "json" | "xml" | "log" => pre_model(&text),
        _ if looks_html => html_model(&text),
        _ => plain_model(&text),
    };
    if encoding != "utf-8" {
        model.notes.push(format!("문자 인코딩: {encoding}"));
    }
    if truncated {
        model.notes.push(format!(
            "{} MB까지만 변환(원본 {:.1} MB)",
            MAX_TEXT_BYTES / 1024 / 1024,
            meta.len() as f64 / 1024.0 / 1024.0
        ));
    }
    Ok(model)
}

// ---------------------------------------------------------------------------
// Sample writers (fixtures)
// ---------------------------------------------------------------------------

/// Writes a Markdown meeting note and a CP949 encoded CSV ledger.
pub fn write_sample_texts(dir: &Path) -> Result<Vec<String>> {
    let md = dir.join("sample-notes.md");
    std::fs::write(
        &md,
        "# 주간 회의록\n\n일시: 2026-03-02 · 참석: 경영기획팀\n\n## 결정 사항\n\n- **매출 목표** 12% 상향\n- 신규 거래처 [목록](https://example.invalid/list) 공유\n\n| 항목 | 담당 | 기한 |\n|---|---|---|\n| 견적서 양식 갱신 | 김 | 3/10 |\n| 계약서 검토 | 이 | 3/15 |\n\n```\n참고: 환율 1,350원 가정\n```\n",
    )
    .map_err(|e| CoreError::io(&md, e))?;
    let csv = dir.join("sample-ledger.csv");
    let (bytes, _, _) = encoding_rs::EUC_KR.encode(
        "일자,거래처,공급가액,세액,비고\n2026-03-01,\"㈜한빛, 서울\",1200000,120000,노트북 10대\n2026-03-02,현금 매출,350000,35000,\n2026-03-05,㈜다온,980000,98000,\"유지보수 계약, 1차\"\n2026-03-09,㈜한빛,450000,45000,추가 납품\n",
    );
    std::fs::write(&csv, bytes).map_err(|e| CoreError::io(&csv, e))?;
    Ok(vec!["sample-notes.md".into(), "sample-ledger.csv".into()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf8_utf16_and_cp949() {
        assert_eq!(decode("가나".as_bytes()).1, "utf-8");
        assert_eq!(decode(&[0xEF, 0xBB, 0xBF, b'a']), ("a".to_string(), "utf-8"));
        let (bytes, _, _) = encoding_rs::EUC_KR.encode("한글 텍스트");
        assert_eq!(decode(&bytes), ("한글 텍스트".to_string(), "cp949"));
        let mut utf16 = vec![0xFF, 0xFE];
        for u in "ab".encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode(&utf16), ("ab".to_string(), "utf-16"));
    }

    #[test]
    fn csv_quotes_and_delimiters() {
        let rows = parse_delimited("a,\"b,1\",\"say \"\"hi\"\"\"\r\n1,2,3\n", ',');
        assert_eq!(rows, vec![vec!["a", "b,1", "say \"hi\""], vec!["1", "2", "3"]]);
        assert_eq!(guess_delimiter("x;y;z\n1;2;3"), ';');
        assert_eq!(guess_delimiter("x\ty\n"), '\t');
        let model = csv_model("h1,h2\nv1,v2\n", ',');
        let Block::Table(t) = &model.blocks[0] else { panic!() };
        assert!(t.rows[0][0].header && !t.rows[1][0].header);
        assert_eq!(t.rows[1][1].text().trim(), "v2");
    }

    #[test]
    fn markdown_structure() {
        let dir = tempfile::tempdir().unwrap();
        let names = write_sample_texts(dir.path()).unwrap();
        let model = read(&dir.path().join(&names[0])).unwrap();
        assert_eq!(model.title.as_deref(), Some("주간 회의록"));
        let paras: Vec<&Paragraph> =
            model.blocks.iter().filter_map(|b| if let Block::Paragraph(p) = b { Some(p) } else { None }).collect();
        assert_eq!(paras[0].heading, Some(1));
        assert_eq!(paras[2].heading, Some(2));
        let bullets: Vec<&&Paragraph> = paras.iter().filter(|p| p.list == Some(ListKind::Bullet)).collect();
        assert_eq!(bullets.len(), 2);
        assert_eq!(bullets[0].text(), "매출 목표 12% 상향", "emphasis markers stripped");
        assert_eq!(bullets[1].text(), "신규 거래처 목록 공유", "link reduced to its label");
        let table = model.blocks.iter().find_map(|b| if let Block::Table(t) = b { Some(t) } else { None }).unwrap();
        assert_eq!(table.rows.len(), 3, "separator row dropped");
        assert!(table.rows[0][0].header);
        assert_eq!(table.rows[2][0].text().trim(), "계약서 검토");
        assert!(paras.last().unwrap().text().contains("환율 1,350원"), "fenced code kept as text");
        // CP949 CSV
        let ledger = read(&dir.path().join(&names[1])).unwrap();
        assert!(ledger.notes.iter().any(|n| n.contains("cp949")), "{:?}", ledger.notes);
        let t = ledger.blocks.iter().find_map(|b| if let Block::Table(t) = b { Some(t) } else { None }).unwrap();
        assert_eq!(t.rows[1][1].text().trim(), "㈜한빛, 서울");
        assert_eq!(t.rows.len(), 5);
        assert_eq!(t.rows[3][4].text().trim(), "유지보수 계약, 1차");
    }

    #[test]
    fn html_and_plain_text() {
        let dir = tempfile::tempdir().unwrap();
        let html = dir.path().join("page.html");
        std::fs::write(&html, "<html><head><title>세금계산서 안내</title><style>p{}</style></head><body><h1>안내</h1><p>공급가액은 &amp; 세액을 <b>합산</b>합니다.<br>둘째 줄</p><ul><li>항목 하나</li><li>항목 둘</li></ul><script>alert(1)</script><table><tr><td>A</td><td>B</td></tr></table></body></html>").unwrap();
        let model = read(&html).unwrap();
        assert_eq!(model.title.as_deref(), Some("세금계산서 안내"));
        let paras: Vec<&Paragraph> =
            model.blocks.iter().filter_map(|b| if let Block::Paragraph(p) = b { Some(p) } else { None }).collect();
        assert_eq!(paras[0].heading, Some(1));
        assert_eq!(paras[0].text(), "안내");
        assert_eq!(paras[1].text(), "공급가액은 & 세액을 합산합니다.둘째 줄");
        assert!(paras[1].runs[0].line_break, "<br> becomes a hard break");
        assert_eq!(paras.iter().filter(|p| p.list == Some(ListKind::Bullet)).count(), 2);
        assert!(!model.text().contains("alert"), "scripts stripped");
        assert!(model.text().contains("A\tB"), "table cells tab-separated");
        let txt = dir.path().join("memo.txt");
        std::fs::write(&txt, "제목 줄\n본문 첫 줄\n본문 둘째 줄\n\n두 번째 문단\n").unwrap();
        let model = read(&txt).unwrap();
        assert_eq!(model.title.as_deref(), Some("제목 줄"));
        assert_eq!(model.blocks.len(), 2);
        // A .pdf download that is really an HTML page is still read as HTML.
        let fake = dir.path().join("invoice.txt");
        std::fs::write(&fake, "<!DOCTYPE html><html><body><h2>영수증</h2><p>합계 10,000원</p></body></html>").unwrap();
        let model = read(&fake).unwrap();
        assert_eq!(model.title.as_deref(), Some("영수증"));
    }

    /// Audit DOC-1: `&` followed by Korean text put byte 12 of the entity window inside a Hangul
    /// syllable and the slice panicked (the import job failed, the rest of the batch was left).
    #[test]
    fn ampersand_before_korean_text_is_not_a_panic() {
        for (input, expected) in [
            ("R&D 센터 소개", "R&D 센터 소개"),
            ("기획&middot;조정", "기획·조정"),
            ("연구&amp;개발본부", "연구&개발본부"),
            ("매출&이익 분석", "매출&이익 분석"),
            ("A&B", "A&B"),
            ("R&D센터", "R&D센터"),
            ("끝&", "끝&"),
            ("&#44032;&#xB098;", "가나"),
            ("&unknown; 그대로", "&unknown; 그대로"),
        ] {
            assert_eq!(decode_entities(input), expected, "{input}");
        }
        // The whole reader, through the guarded entry point the import uses.
        let dir = tempfile::tempdir().unwrap();
        let html = dir.path().join("rnd.html");
        std::fs::write(
            &html,
            "<html><head><title>R&D 센터 소개</title></head><body><h1>기획&middot;조정</h1>\
             <p>연구&amp;개발본부 매출&이익 분석</p></body></html>",
        )
        .unwrap();
        let model = crate::documents::convert::read_document(&html, crate::documents::SourceKind::Text).unwrap();
        assert_eq!(model.title.as_deref(), Some("R&D 센터 소개"));
        let text = model.text();
        assert!(text.contains("기획·조정") && text.contains("연구&개발본부 매출&이익 분석"), "{text}");
        // Characters whose lower-case form has another byte length (İ → i̇) used to shift the tag
        // offsets taken from the lower-cased copy.
        let tr = dir.path().join("tr.html");
        std::fs::write(&tr, "<html><body><p>İSTANBUL İİİ</p><SCRIPT>x()</SCRIPT><p>본문 İ</p></body></html>").unwrap();
        let text = read(&tr).unwrap().text();
        assert!(text.contains("İSTANBUL İİİ") && text.contains("본문 İ") && !text.contains("x()"), "{text}");
    }
}
