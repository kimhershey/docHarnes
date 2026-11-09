//! XLSX (SpreadsheetML) reader on `zip` + `quick-xml`: every visible sheet
//! becomes a heading (the sheet name) and a table of its used range. Shared /
//! inline strings, numbers (dates by number format), booleans and error values
//! are read; formulas contribute their cached value; merged cells become
//! spans and column widths are kept. Hidden sheets are skipped with a note.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::error::{CoreError, Result};

use super::model::{Block, Cell, DocModel, Paragraph, Table};
use super::xml::{self, Element};

/// Rows / columns kept per sheet (the rest is dropped with a note).
pub const MAX_SHEET_ROWS: usize = 5_000;
pub const MAX_SHEET_COLS: usize = 64;

type Archive = zip::ZipArchive<BufReader<File>>;

/// Relationship id → target of an OOXML `.rels` part.
pub(crate) fn read_rels(archive: &mut Archive, name: &str) -> HashMap<String, String> {
    let Some(n) = xml::find_zip_entry(archive, name) else { return HashMap::new() };
    let Ok(Some(bytes)) = xml::read_zip_entry(archive, &n, xml::MAX_XML_BYTES) else { return HashMap::new() };
    let Ok(root) = xml::parse(&bytes) else { return HashMap::new() };
    root.children_named("Relationship")
        .filter_map(|r| Some((r.attr("Id")?.to_string(), r.attr("Target")?.to_string())))
        .collect()
}

/// Resolves a relationship target against the directory of its source part
/// (`xl/` + `worksheets/sheet1.xml`, `ppt/slides/` + `../media/image1.png`, `/xl/…`).
pub(crate) fn resolve_target(base_dir: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut parts: Vec<&str> = base_dir.trim_end_matches('/').split('/').filter(|p| !p.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn read_part(archive: &mut Archive, name: &str) -> Result<Option<Element>> {
    let Some(n) = xml::find_zip_entry(archive, name) else { return Ok(None) };
    let Some(bytes) = xml::read_zip_entry(archive, &n, xml::MAX_XML_BYTES)? else { return Ok(None) };
    Ok(Some(xml::parse(&bytes)?))
}

/// `sst/si` entries: plain `t` or rich runs `r/t`; phonetic runs (`rPh`) are skipped.
fn shared_strings(archive: &mut Archive) -> Result<Vec<String>> {
    let Some(root) = read_part(archive, "xl/sharedStrings.xml")? else { return Ok(Vec::new()) };
    Ok(root
        .children_named("si")
        .map(|si| {
            let mut s = String::new();
            for c in si.elements() {
                match c.name.as_str() {
                    "t" => s.push_str(&c.text()),
                    "r" => {
                        if let Some(t) = c.child("t") {
                            s.push_str(&t.text());
                        }
                    }
                    _ => {}
                }
            }
            s
        })
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumKind {
    Plain,
    Date,
    DateTime,
    Time,
}

/// Cell style index → how its numbers should be shown (dates / times from the number format).
fn style_kinds(archive: &mut Archive) -> Result<Vec<NumKind>> {
    let Some(root) = read_part(archive, "xl/styles.xml")? else { return Ok(Vec::new()) };
    let mut custom: HashMap<u32, NumKind> = HashMap::new();
    if let Some(fmts) = root.child("numFmts") {
        for f in fmts.children_named("numFmt") {
            if let (Some(id), Some(code)) = (f.attr_u32("numFmtId"), f.attr("formatCode")) {
                custom.insert(id, classify_format(code));
            }
        }
    }
    let builtin = |id: u32| match id {
        14..=17 | 27..=31 | 34..=36 | 50..=58 => NumKind::Date,
        22 => NumKind::DateTime,
        18..=21 | 32 | 33 | 45..=47 => NumKind::Time,
        _ => NumKind::Plain,
    };
    Ok(root
        .child("cellXfs")
        .map(|xfs| {
            xfs.children_named("xf")
                .map(|xf| {
                    let id = xf.attr_u32("numFmtId").unwrap_or(0);
                    custom.get(&id).copied().unwrap_or_else(|| builtin(id))
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Date / time detection in a custom format code (quoted text and `[…]` sections ignored).
fn classify_format(code: &str) -> NumKind {
    let mut bare = String::new();
    let mut in_quote = false;
    let mut in_bracket = false;
    for c in code.chars() {
        match c {
            '"' => in_quote = !in_quote,
            '[' if !in_quote => in_bracket = true,
            ']' if !in_quote => in_bracket = false,
            _ if !in_quote && !in_bracket => bare.push(c.to_ascii_lowercase()),
            _ => {}
        }
    }
    let date =
        bare.contains('y') || bare.contains('d') || (bare.contains('m') && !bare.contains(':') && !bare.contains('0'));
    let time = bare.contains('h') || bare.contains(':') && bare.contains('s');
    match (date, time) {
        (true, true) => NumKind::DateTime,
        (true, false) => NumKind::Date,
        (false, true) => NumKind::Time,
        (false, false) => NumKind::Plain,
    }
}

/// Excel serial (1900 or 1904 date system) → text.
fn format_serial(v: f64, kind: NumKind, date1904: bool) -> Option<String> {
    use chrono::{Duration, NaiveDate};
    let base = if date1904 { NaiveDate::from_ymd_opt(1904, 1, 1)? } else { NaiveDate::from_ymd_opt(1899, 12, 30)? };
    let days = v.floor();
    let secs = ((v - days) * 86_400.0).round() as i64;
    let date = base.checked_add_signed(Duration::days(days as i64))?;
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    Some(match kind {
        NumKind::Date => date.format("%Y-%m-%d").to_string(),
        NumKind::DateTime => format!("{} {h:02}:{m:02}", date.format("%Y-%m-%d")),
        NumKind::Time => format!("{h:02}:{m:02}:{s:02}"),
        NumKind::Plain => return None,
    })
}

/// Plain number text: drops a trailing ".0", keeps everything else as stored.
fn number_text(v: &str) -> String {
    let t = v.trim();
    if let Ok(f) = t.parse::<f64>() {
        if f.fract() == 0.0 && f.abs() < 1e15 && !t.contains('E') && !t.contains('e') {
            return format!("{}", f as i64);
        }
    }
    t.to_string()
}

/// `AB12` → (column 28, row 12), both 1-based.
pub(crate) fn parse_ref(r: &str) -> Option<(usize, usize)> {
    let letters: String = r.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let digits: String = r.chars().skip(letters.len()).take_while(|c| c.is_ascii_digit()).collect();
    if letters.is_empty() || digits.is_empty() {
        return None;
    }
    let col = letters.chars().fold(0usize, |acc, c| acc * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1));
    Some((col, digits.parse().ok()?))
}

struct Merge {
    col: usize,
    row: usize,
    col_span: u16,
    row_span: u16,
}

fn parse_merges(sheet: &Element) -> Vec<Merge> {
    let Some(mc) = sheet.child("mergeCells") else { return Vec::new() };
    mc.children_named("mergeCell")
        .filter_map(|m| {
            let (a, b) = m.attr("ref")?.split_once(':')?;
            let (c1, r1) = parse_ref(a)?;
            let (c2, r2) = parse_ref(b)?;
            Some(Merge {
                col: c1.min(c2),
                row: r1.min(r2),
                col_span: (c1.abs_diff(c2) + 1).min(u16::MAX as usize) as u16,
                row_span: (r1.abs_diff(r2) + 1).min(u16::MAX as usize) as u16,
            })
        })
        .collect()
}

/// Column widths (Excel character units → mm, ~1.9 mm per unit) for columns `first..=last`.
fn column_widths(sheet: &Element, first: usize, last: usize) -> Vec<f32> {
    let mut widths = vec![16.0f32; last - first + 1];
    if let Some(cols) = sheet.child("cols") {
        for c in cols.children_named("col") {
            let (Some(min), Some(max), Some(w)) = (c.attr_u32("min"), c.attr_u32("max"), c.attr_f32("width")) else {
                continue;
            };
            for col in (min as usize).max(first)..=(max as usize).min(last) {
                widths[col - first] = (w * 1.9).clamp(4.0, 120.0);
            }
        }
    }
    widths
}

struct SheetGrid {
    cells: HashMap<(usize, usize), String>,
    max_row: usize,
    max_col: usize,
    truncated_rows: usize,
    truncated_cols: usize,
}

fn read_grid(sheet: &Element, shared: &[String], styles: &[NumKind], date1904: bool) -> SheetGrid {
    let mut grid = SheetGrid { cells: HashMap::new(), max_row: 0, max_col: 0, truncated_rows: 0, truncated_cols: 0 };
    let Some(data) = sheet.child("sheetData") else { return grid };
    let mut row_no = 0usize;
    for row in data.children_named("row") {
        row_no = row.attr_u32("r").map(|r| r as usize).unwrap_or(row_no + 1);
        if row_no > MAX_SHEET_ROWS {
            grid.truncated_rows = grid.truncated_rows.max(row_no);
            continue;
        }
        let mut col_no = 0usize;
        for c in row.children_named("c") {
            let (col, _) = c.attr("r").and_then(parse_ref).unwrap_or((col_no + 1, row_no));
            col_no = col;
            if col > MAX_SHEET_COLS {
                grid.truncated_cols = grid.truncated_cols.max(col);
                continue;
            }
            let value = c.child("v").map(|v| v.text());
            let text = match c.attr("t") {
                Some("s") => value
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .and_then(|i| shared.get(i).cloned())
                    .unwrap_or_default(),
                Some("inlineStr") => c.child("is").map(|is| is.text()).unwrap_or_default(),
                Some("b") => match value.as_deref().map(str::trim) {
                    Some("1") => "TRUE".to_string(),
                    Some("0") => "FALSE".to_string(),
                    _ => String::new(),
                },
                Some("str") | Some("e") => value.unwrap_or_default(),
                _ => {
                    let Some(v) = value else { continue };
                    let kind = c.attr_u32("s").and_then(|s| styles.get(s as usize).copied()).unwrap_or(NumKind::Plain);
                    match (kind, v.trim().parse::<f64>()) {
                        (NumKind::Plain, _) => number_text(&v),
                        (k, Ok(f)) => format_serial(f, k, date1904).unwrap_or_else(|| number_text(&v)),
                        (_, Err(_)) => v,
                    }
                }
            };
            if text.trim().is_empty() {
                continue;
            }
            grid.max_row = grid.max_row.max(row_no);
            grid.max_col = grid.max_col.max(col);
            grid.cells.insert((col, row_no), text);
        }
    }
    grid
}

/// One sheet → table (`None` when it holds no text).
fn sheet_table(
    sheet: &Element,
    shared: &[String],
    styles: &[NumKind],
    date1904: bool,
    notes: &mut Vec<String>,
    name: &str,
) -> Option<Table> {
    let grid = read_grid(sheet, shared, styles, date1904);
    let merges: Vec<Merge> =
        parse_merges(sheet).into_iter().filter(|m| m.row <= MAX_SHEET_ROWS && m.col <= MAX_SHEET_COLS).collect();
    let mut max_row = grid.max_row;
    let mut max_col = grid.max_col;
    for m in &merges {
        if grid.cells.contains_key(&(m.col, m.row)) {
            max_row = max_row.max((m.row + m.row_span as usize - 1).min(MAX_SHEET_ROWS));
            max_col = max_col.max((m.col + m.col_span as usize - 1).min(MAX_SHEET_COLS));
        }
    }
    if grid.cells.is_empty() {
        return None;
    }
    if grid.truncated_rows > 0 {
        notes.push(format!("시트 '{name}': {}행 중 {MAX_SHEET_ROWS}행까지만 변환", grid.truncated_rows));
    }
    if grid.truncated_cols > 0 {
        notes.push(format!("시트 '{name}': {}열 중 {MAX_SHEET_COLS}열까지만 변환", grid.truncated_cols));
    }
    let min_row =
        grid.cells.keys().map(|k| k.1).min().unwrap_or(1).min(merges.iter().map(|m| m.row).min().unwrap_or(usize::MAX));
    let min_col =
        grid.cells.keys().map(|k| k.0).min().unwrap_or(1).min(merges.iter().map(|m| m.col).min().unwrap_or(usize::MAX));
    // Cells hidden under a merge anchor.
    let mut covered: HashSet<(usize, usize)> = HashSet::new();
    let mut anchors: HashMap<(usize, usize), (u16, u16)> = HashMap::new();
    for m in &merges {
        anchors.insert((m.col, m.row), (m.col_span, m.row_span));
        for r in m.row..m.row + m.row_span as usize {
            for c in m.col..m.col + m.col_span as usize {
                if (c, r) != (m.col, m.row) {
                    covered.insert((c, r));
                }
            }
        }
    }
    let mut rows = Vec::with_capacity(max_row - min_row + 1);
    for r in min_row..=max_row {
        let mut row = Vec::new();
        for c in min_col..=max_col {
            if covered.contains(&(c, r)) {
                continue;
            }
            let (col_span, row_span) = anchors.get(&(c, r)).copied().unwrap_or((1, 1));
            let blocks = match grid.cells.get(&(c, r)) {
                Some(t) => vec![Block::Paragraph(Paragraph::plain(t.clone()))],
                None => Vec::new(),
            };
            row.push(Cell { blocks, row_span, col_span, header: false });
        }
        rows.push(row);
    }
    Some(Table { rows, col_widths_mm: column_widths(sheet, min_col, max_col) })
}

pub fn read(path: &Path) -> Result<DocModel> {
    let file = File::open(path).map_err(|e| CoreError::io(path, e))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| CoreError::Convert(format!("not a zip archive: {e}")))?;
    let workbook = read_part(&mut archive, "xl/workbook.xml")?
        .ok_or_else(|| CoreError::Convert("xl/workbook.xml missing (not an XLSX file)".into()))?;
    let date1904 = workbook.child("workbookPr").and_then(|p| p.attr_bool("date1904")).unwrap_or(false);
    let rels = read_rels(&mut archive, "xl/_rels/workbook.xml.rels");
    let shared = shared_strings(&mut archive)?;
    let styles = style_kinds(&mut archive)?;
    let sheets: Vec<(String, String, bool)> = workbook
        .child("sheets")
        .map(|s| {
            s.children_named("sheet")
                .filter_map(|sh| {
                    let name = sh.attr("name").unwrap_or("Sheet").to_string();
                    let rid = sh.attr("id")?.to_string();
                    let hidden = matches!(sh.attr("state"), Some("hidden") | Some("veryHidden"));
                    Some((name, rid, hidden))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut model = DocModel::default();
    let mut first = true;
    for (name, rid, hidden) in sheets {
        if hidden {
            model.notes.push(format!("숨김 시트 생략: {name}"));
            continue;
        }
        let Some(target) = rels.get(&rid) else { continue };
        let part = resolve_target("xl", target);
        let Some(sheet) = read_part(&mut archive, &part)? else { continue };
        let Some(table) = sheet_table(&sheet, &shared, &styles, date1904, &mut model.notes, &name) else { continue };
        if !first {
            model.blocks.push(Block::PageBreak);
        }
        first = false;
        if model.title.is_none() {
            // A lone cell in the first row reads as the sheet's title.
            if let Some(first_row) = table.rows.first() {
                let texts: Vec<String> =
                    first_row.iter().map(|c| c.text().trim().to_string()).filter(|t| !t.is_empty()).collect();
                if texts.len() == 1 && texts[0].chars().count() <= 120 {
                    model.title = Some(texts[0].clone());
                }
            }
        }
        model.blocks.push(Block::Paragraph(Paragraph { heading: Some(2), ..Paragraph::plain(name.clone()) }));
        model.blocks.push(Block::Table(table));
    }
    if model.blocks.is_empty() {
        model.notes.push("내용이 있는 시트가 없습니다".into());
    }
    Ok(model)
}

// ---------------------------------------------------------------------------
// Test fixture writer
// ---------------------------------------------------------------------------

/// Writes a small workbook: sheet "거래내역" (merged title row, header row, dates /
/// amounts / a formula with cached value / a boolean / an inline string), an
/// empty "메모" sheet and a hidden "설정" sheet.
pub fn write_sample_xlsx(path: &Path) -> Result<()> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let file = File::create(path).map_err(|e| CoreError::io(path, e))?;
    let mut z = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let w = |z: &mut zip::ZipWriter<File>, name: &str, data: &[u8]| -> Result<()> {
        z.start_file(name, opts).map_err(|e| CoreError::Convert(e.to_string()))?;
        z.write_all(data).map_err(|e| CoreError::io(path, e))
    };
    w(&mut z, "[Content_Types].xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet3.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#)?;
    w(&mut z, "_rels/.rels", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#)?;
    w(&mut z, "xl/workbook.xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><workbookPr/><sheets><sheet name="거래내역" sheetId="1" r:id="rId1"/><sheet name="메모" sheetId="2" r:id="rId2"/><sheet name="설정" sheetId="3" state="hidden" r:id="rId3"/></sheets></workbook>"#.as_bytes())?;
    w(&mut z, "xl/_rels/workbook.xml.rels", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet3.xml"/><Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#)?;
    w(&mut z, "xl/sharedStrings.xml", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" count=\"6\" uniqueCount=\"6\"><si><t>2026년 3월 거래내역</t></si><si><t>일자</t></si><si><t>거래처</t></si><si><r><rPr><b/></rPr><t>공급가액</t></r><r><t xml:space=\"preserve\">(원)</t></r></si><si><t>㈜한빛</t></si><si><t>합계</t></si></sst>".as_bytes())?;
    // numFmt 14 = built-in date; custom 164 = "yyyy-mm-dd hh:mm"; cellXfs: 0 plain, 1 date, 2 custom date-time.
    w(&mut z, "xl/styles.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="yyyy-mm-dd hh:mm"/></numFmts><cellXfs count="3"><xf numFmtId="0"/><xf numFmtId="14" applyNumberFormat="1"/><xf numFmtId="164" applyNumberFormat="1"/></cellXfs></styleSheet>"#)?;
    w(&mut z, "xl/worksheets/sheet1.xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cols><col min="1" max="1" width="12"/><col min="2" max="2" width="20"/><col min="3" max="3" width="14"/></cols><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row><row r="2"><c r="A2" t="s"><v>1</v></c><c r="B2" t="s"><v>2</v></c><c r="C2" t="s"><v>3</v></c></row><row r="3"><c r="A3" s="1"><v>46082</v></c><c r="B3" t="s"><v>4</v></c><c r="C3"><v>1200000</v></c></row><row r="4"><c r="A4" s="2"><v>46083.5</v></c><c r="B4" t="inlineStr"><is><t>현금 매출</t></is></c><c r="C4"><v>350000.5</v></c></row><row r="5"><c r="A5" t="s"><v>5</v></c><c r="B5" t="b"><v>1</v></c><c r="C5"><f>SUM(C3:C4)</f><v>1550000.5</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="A1:C1"/></mergeCells></worksheet>"#.as_bytes())?;
    w(&mut z, "xl/worksheets/sheet2.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#)?;
    w(&mut z, "xl/worksheets/sheet3.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>secret</t></is></c></row></sheetData></worksheet>"#)?;
    z.finish().map_err(|e| CoreError::Convert(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_and_targets() {
        assert_eq!(parse_ref("A1"), Some((1, 1)));
        assert_eq!(parse_ref("AB12"), Some((28, 12)));
        assert_eq!(parse_ref("12"), None);
        assert_eq!(resolve_target("xl", "worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(resolve_target("ppt/slides", "../media/image1.png"), "ppt/media/image1.png");
        assert_eq!(resolve_target("xl", "/xl/sharedStrings.xml"), "xl/sharedStrings.xml");
    }

    #[test]
    fn number_formats() {
        assert_eq!(classify_format("yyyy-mm-dd"), NumKind::Date);
        assert_eq!(classify_format("h:mm:ss"), NumKind::Time);
        assert_eq!(classify_format("yyyy-mm-dd hh:mm"), NumKind::DateTime);
        assert_eq!(classify_format("#,##0.00"), NumKind::Plain);
        assert_eq!(classify_format("[Red]\"yes\"0"), NumKind::Plain);
        assert_eq!(format_serial(46082.0, NumKind::Date, false).as_deref(), Some("2026-03-01"));
        assert_eq!(format_serial(46083.5, NumKind::DateTime, false).as_deref(), Some("2026-03-02 12:00"));
        assert_eq!(number_text("1200000"), "1200000");
        assert_eq!(number_text("350000.5"), "350000.5");
        assert_eq!(number_text("12.0"), "12");
    }

    #[test]
    fn reads_generated_xlsx() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.xlsx");
        write_sample_xlsx(&path).unwrap();
        let model = read(&path).unwrap();
        assert_eq!(model.title.as_deref(), Some("2026년 3월 거래내역"));
        assert!(model.notes.iter().any(|n| n.contains("숨김 시트 생략: 설정")), "{:?}", model.notes);
        // One visible sheet with content → heading + table, no page break; the empty "메모" sheet is skipped.
        assert!(!model.blocks.iter().any(|b| matches!(b, Block::PageBreak)));
        let heading = model.blocks.iter().find_map(|b| match b {
            Block::Paragraph(p) if p.heading == Some(2) => Some(p.text()),
            _ => None,
        });
        assert_eq!(heading.as_deref(), Some("거래내역"));
        let table = model
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert_eq!(table.rows.len(), 5);
        assert_eq!(table.rows[0].len(), 1, "merged title row collapses to one cell");
        assert_eq!(table.rows[0][0].col_span, 3);
        assert_eq!(table.rows[0][0].text().trim(), "2026년 3월 거래내역");
        assert_eq!(table.rows[1][2].text().trim(), "공급가액(원)", "rich-text shared string");
        assert_eq!(table.rows[2][0].text().trim(), "2026-03-01", "built-in date format");
        assert_eq!(table.rows[3][0].text().trim(), "2026-03-02 12:00", "custom date-time format");
        assert_eq!(table.rows[3][1].text().trim(), "현금 매출", "inline string");
        assert_eq!(table.rows[4][1].text().trim(), "TRUE");
        assert_eq!(table.rows[4][2].text().trim(), "1550000.5", "formula cached value");
        assert_eq!(table.col_widths_mm.len(), 3);
        assert!(table.col_widths_mm[1] > table.col_widths_mm[0]);
        let text = model.text();
        assert!(text.contains("㈜한빛\t1200000"), "{text}");
        assert!(!text.contains("secret"), "hidden sheet content is not extracted");
    }
}
