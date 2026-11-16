//! Cells of a workbook (1.0.0 §2-1, step 4): a cell gets a new value where it stands — its style
//! (number format, font, fill, borders) stays, because only the value inside `<c>` changes.
//!
//! * The cell is named by its address: `B3`, `견적!B3`, `'1차 견적'!B3` (no sheet = the first one).
//! * A number goes in as a number, a date into a cell that shows dates as that date, anything else as
//!   text stored in the cell itself (`inlineStr`), so the shared string table is not touched.
//! * A cell that holds a formula is left alone.
//! * Formulas that read a changed cell keep a result that was computed before the change. This reader
//!   has no formula engine, so those stored results are taken out (the cell reads empty until a
//!   spreadsheet program computes it) and the workbook is marked to be computed in full when it is
//!   opened. A formula whose reads cannot be followed (a name, a table, a shared formula without its
//!   text) counts as reading everything.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;

use chrono::NaiveDate;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::XmlVersion;

use super::{local, Archive, CoreError, Part, Result};

/// Reason codes.
pub const ADDRESS: &str = "edit_cell_address";
pub const FORMULA: &str = "edit_cell_formula";
/// The header cell of a table (Excel's "표"): its text is the column's name in the table's own part too.
pub const TABLE_HEADER: &str = "edit_table_header";
/// Note: results of formulas were taken out to be computed again (with their number).
pub const FORMULAS_CLEARED: &str = "edit_formulas_cleared";

/// Excel's last column (`XFD`) and row.
const MAX_COL: u32 = 16_384;
const MAX_ROW: u32 = 1_048_576;
/// Formula cells above which the reads are not followed: every stored result goes.
const MAX_FOLLOWED_FORMULAS: usize = 20_000;

/// What the cells module has to know of the workbook around the sheets.
pub struct Workbook {
    /// Sheet names in their order, with the part of each (lower case).
    pub sheets: Vec<(String, String)>,
    pub date1904: bool,
    /// Per cell style (`s`): the cell shows a date.
    pub date_styles: Vec<bool>,
    /// The header rows of the tables (Excel's "표") of a sheet, by its part (lower case):
    /// `(first column, last column, row)`. A header cell's text is the name of the column in the
    /// table's own part and in the formulas that name it; Excel takes a file where the two differ
    /// for a damaged one, so those cells are left to Excel.
    pub table_headers: HashMap<String, Vec<(u32, u32, u32)>>,
}

impl Workbook {
    pub fn in_table_header(&self, part: &str, col: u32, row: u32) -> bool {
        self.table_headers
            .get(&part.to_ascii_lowercase())
            .is_some_and(|headers| headers.iter().any(|(from, to, r)| *r == row && (*from..=*to).contains(&col)))
    }
}

/// `xl/worksheets` + `../tables/table1.xml` → `xl/tables/table1.xml`.
fn resolve(base: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for step in target.split('/') {
        match step {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// The header rows of the tables a sheet shows (its relationships name the table parts).
fn table_headers(archive: &mut Archive, sheet_part: &str) -> Vec<(u32, u32, u32)> {
    let Some((folder, file)) = sheet_part.rsplit_once('/') else { return Vec::new() };
    let Some(rels) = read_text(archive, &format!("{folder}/_rels/{file}.rels")) else { return Vec::new() };
    let mut out = Vec::new();
    for rel in start_tags(&rels).iter().filter(|e| local(e.name().as_ref()) == "Relationship") {
        if !attribute(rel, "Type").is_some_and(|t| t.ends_with("/table")) {
            continue;
        }
        let Some(xml) = attribute(rel, "Target").and_then(|t| read_text(archive, &resolve(folder, &t))) else {
            continue;
        };
        let Some(table) = start_tags(&xml).into_iter().find(|e| local(e.name().as_ref()) == "table") else { continue };
        // A table has one header row unless it says it has none.
        if attribute(&table, "headerRowCount").is_some_and(|n| n.trim() == "0") {
            continue;
        }
        let Some(range) = attribute(&table, "ref") else { continue };
        let (from, to) = range.split_once(':').unwrap_or((range.as_str(), range.as_str()));
        if let (Some((c1, r1)), Some((c2, r2))) = (parse_cell(from), parse_cell(to)) {
            out.push((c1.min(c2), c1.max(c2), r1.min(r2)));
        }
    }
    out
}

fn attribute(e: &BytesStart<'_>, key: &str) -> Option<String> {
    e.try_get_attribute(key)
        .ok()
        .flatten()
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
}

fn read_text(archive: &mut Archive, name: &str) -> Option<String> {
    let wanted = name.to_ascii_lowercase();
    let index = (0..archive.len()).find(|i| {
        archive.by_index(*i).ok().and_then(|e| e.name().ok().map(|n| n.to_ascii_lowercase() == wanted)) == Some(true)
    })?;
    let mut text = String::new();
    archive.by_index(index).ok()?.take(64 * 1024 * 1024).read_to_string(&mut text).ok()?;
    Some(text)
}

/// The start tags of a part's XML, in order.
fn start_tags(xml: &str) -> Vec<BytesStart<'static>> {
    let mut reader = quick_xml::Reader::from_reader(xml.as_bytes());
    let mut buf = Vec::new();
    let mut out = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => out.push(e.into_owned()),
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// Reads the sheet list, the date system and which cell styles show dates.
pub fn workbook(archive: &mut Archive) -> Result<Workbook> {
    let xml = read_text(archive, "xl/workbook.xml")
        .ok_or_else(|| CoreError::Convert("xl/workbook.xml missing (not an XLSX file)".into()))?;
    let rels: HashMap<String, String> = read_text(archive, "xl/_rels/workbook.xml.rels")
        .map(|rels| {
            start_tags(&rels)
                .iter()
                .filter(|e| local(e.name().as_ref()) == "Relationship")
                .filter_map(|e| Some((attribute(e, "Id")?, attribute(e, "Target")?)))
                .collect()
        })
        .unwrap_or_default();
    let tags = start_tags(&xml);
    let date1904 = tags
        .iter()
        .find(|e| local(e.name().as_ref()) == "workbookPr")
        .and_then(|e| attribute(e, "date1904"))
        .is_some_and(|v| matches!(v.trim(), "1" | "true"));
    let sheets = tags
        .iter()
        .filter(|e| local(e.name().as_ref()) == "sheet")
        .filter_map(|e| {
            let name = attribute(e, "name")?;
            let id = e.attributes().flatten().find_map(|a| {
                (local(a.key.as_ref()) == "id" && a.key.as_ref() != "sheetId")
                    .then(|| a.normalized_value(XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
                    .flatten()
            })?;
            let target = rels.get(&id)?;
            let part = match target.strip_prefix('/') {
                Some(absolute) => absolute.to_string(),
                None => format!("xl/{target}"),
            };
            Some((name, part.to_ascii_lowercase()))
        })
        .collect();
    let sheets: Vec<(String, String)> = sheets;
    let date_styles = crate::documents::convert::xlsx::date_styles(archive);
    let table_headers = sheets
        .iter()
        .map(|(_, part)| (part.clone(), table_headers(archive, part)))
        .filter(|(_, headers)| !headers.is_empty())
        .collect();
    Ok(Workbook { sheets, date1904, date_styles, table_headers })
}

/// `B3` → (2, 3); `$B$3` too.
fn parse_cell(text: &str) -> Option<(u32, u32)> {
    let text = text.trim();
    let letters: String = text.chars().filter(|c| *c != '$').take_while(char::is_ascii_alphabetic).collect();
    let rest: String = text.chars().filter(|c| *c != '$').skip(letters.len()).collect();
    if letters.is_empty() || letters.len() > 3 || rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let col = letters.chars().fold(0u32, |n, c| n * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1));
    let row: u32 = rest.parse().ok()?;
    ((1..=MAX_COL).contains(&col) && (1..=MAX_ROW).contains(&row)).then_some((col, row))
}

fn column_name(mut col: u32) -> String {
    let mut name = String::new();
    while col > 0 {
        let rem = (col - 1) % 26;
        name.insert(0, (b'A' + rem as u8) as char);
        col = (col - 1) / 26;
    }
    name
}

/// An address as the user writes it: `B3`, `견적!B3`, `'1차 견적'!B3` → (sheet, column, row).
pub fn parse_address(text: &str) -> Option<(Option<String>, u32, u32)> {
    let text = text.trim();
    let (sheet, cell) = match text.rsplit_once('!') {
        Some((sheet, cell)) => {
            let sheet = sheet.trim();
            let name = match sheet.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
                Some(quoted) => quoted.replace("''", "'"),
                None => sheet.to_string(),
            };
            if name.is_empty() {
                return None;
            }
            (Some(name), cell)
        }
        None => (None, text),
    };
    let (col, row) = parse_cell(cell)?;
    Some((sheet, col, row))
}

/// The day a date text names: `2026-12-31`, `2026.12.31`, `2026/12/31`, `2026년 12월 31일`.
fn parse_date(text: &str) -> Option<NaiveDate> {
    let numbers: Vec<u32> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<u32>().ok())
        .collect::<Option<_>>()?;
    let separators_only =
        text.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | '.' | '/' | ' ' | '년' | '월' | '일'));
    match numbers.as_slice() {
        [y, m, d] if separators_only && (1900..=9999).contains(y) => NaiveDate::from_ymd_opt(*y as i32, *m, *d),
        _ => None,
    }
}

fn epoch(date1904: bool) -> NaiveDate {
    if date1904 {
        NaiveDate::from_ymd_opt(1904, 1, 1).expect("date")
    } else {
        NaiveDate::from_ymd_opt(1899, 12, 30).expect("date")
    }
}

/// What goes into a cell.
enum Value {
    Number(String),
    Text(String),
    Empty,
}

/// A number as a number (thousands separators aside), a date for a cell that shows dates, anything
/// else as text — and digits that start with a zero are a code, not a number.
fn typed(value: &str, shows_a_date: bool, date1904: bool) -> Value {
    let value = value.trim();
    if value.is_empty() {
        return Value::Empty;
    }
    if shows_a_date {
        if let Some(day) = parse_date(value) {
            return Value::Number((day - epoch(date1904)).num_days().to_string());
        }
    }
    let plain = value.replace(',', "");
    let digits = plain.trim_start_matches(['-', '+']);
    let is_code = digits.len() > 1 && digits.starts_with('0') && !digits.starts_with("0.");
    let is_number = !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
        && digits.matches('.').count() <= 1
        && digits != "."
        && plain.parse::<f64>().is_ok();
    if is_number && !is_code {
        Value::Number(plain.trim_start_matches('+').to_string())
    } else {
        Value::Text(value.to_string())
    }
}

/// One `<c>` of a row: what it holds.
struct Found {
    style: Option<String>,
    kind: Option<String>,
    value: Option<String>,
    inline: String,
    formula: bool,
}

/// A cell [`Part::set_cell`] made, kept until the sheet's new cells are written together.
pub(super) struct NewCell {
    events: Vec<Event<'static>>,
    /// What the cell shows now (the "before" of a later edit of the same cell).
    shown: String,
    style: Option<String>,
}

impl Part {
    /// The event index of the end tag of the element at `i` (`i` for an empty element).
    fn close_of(&self, i: usize) -> usize {
        self.end_of(i)
    }

    /// The direct children of the element `start..=end` that are elements: `(index, name)`.
    fn children(&self, start: usize, end: usize) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        let mut i = start + 1;
        while i < end {
            if let Event::Start(e) | Event::Empty(e) = &self.events[i] {
                out.push((i, local(e.name().as_ref()).to_string()));
                i = self.close_of(i) + 1;
            } else {
                i += 1;
            }
        }
        out
    }

    fn start_tag(&self, i: usize) -> Option<&BytesStart<'static>> {
        match &self.events[i] {
            Event::Start(e) | Event::Empty(e) => Some(e),
            _ => None,
        }
    }

    /// The text between the start tag at `i` and its end tag.
    fn inner_text(&self, i: usize) -> String {
        let end = self.close_of(i);
        self.events[i..=end]
            .iter()
            .filter_map(|ev| match ev {
                Event::Text(t) => Some(t.xml_content(XmlVersion::Implicit1_0).into_owned()),
                Event::CData(c) => Some(c.xml_content(XmlVersion::Implicit1_0).into_owned()),
                Event::GeneralRef(r) => super::entity(r),
                _ => None,
            })
            .collect()
    }

    fn read_cell(&self, start: usize) -> Found {
        let end = self.close_of(start);
        let tag = self.start_tag(start);
        let mut found = Found {
            style: tag.and_then(|e| attribute(e, "s")),
            kind: tag.and_then(|e| attribute(e, "t")),
            value: None,
            inline: String::new(),
            formula: false,
        };
        for (i, name) in self.children(start, end) {
            match name.as_str() {
                "f" => found.formula = true,
                "v" => found.value = Some(self.inner_text(i)),
                // The phonetic guide of an inline string is not its text.
                "is" => {
                    let close = self.close_of(i);
                    let mut k = i + 1;
                    while k < close {
                        match &self.events[k] {
                            Event::Start(e) if local(e.name().as_ref()) == "rPh" => k = self.close_of(k) + 1,
                            Event::Start(e) if local(e.name().as_ref()) == "t" => {
                                found.inline.push_str(&self.inner_text(k));
                                k = self.close_of(k) + 1;
                            }
                            _ => k += 1,
                        }
                    }
                }
                _ => {}
            }
        }
        found
    }

    /// What a cell shows, as far as this reader can tell: the text, the number as stored, or the date.
    fn shown_value(&self, cell: &Found, wb: &Workbook, shared: &[String]) -> String {
        let raw = cell.value.clone().unwrap_or_default();
        match cell.kind.as_deref() {
            Some("s") => raw.trim().parse::<usize>().ok().and_then(|i| shared.get(i).cloned()).unwrap_or_default(),
            Some("inlineStr") => cell.inline.clone(),
            // A number: the default, which openpyxl and LibreOffice write out as `t="n"`.
            None | Some("n") => {
                let dated = cell
                    .style
                    .as_deref()
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .is_some_and(|s| wb.date_styles.get(s).copied().unwrap_or(false));
                match (dated, raw.trim().parse::<f64>()) {
                    (true, Ok(serial)) if serial.fract() == 0.0 && (1.0..=2_958_465.0).contains(&serial) => {
                        (epoch(wb.date1904) + chrono::Duration::days(serial as i64)).format("%Y-%m-%d").to_string()
                    }
                    _ => raw,
                }
            }
            // A boolean, an error value, a formula's text result.
            Some(_) => raw,
        }
    }

    /// The sheet's `sheetData` element, its end and the prefix its element names carry.
    fn sheet_data(&self) -> Option<(usize, usize, String, String)> {
        let start = (0..self.events.len())
            .find(|i| self.start_tag(*i).is_some_and(|e| local(e.name().as_ref()) == "sheetData"))?;
        let qualified = self.start_tag(start).map(|e| e.name().as_ref().to_string()).unwrap_or_default();
        let prefix = qualified.rsplit_once(':').map(|(p, _)| format!("{p}:")).unwrap_or_default();
        Some((start, self.close_of(start), qualified, prefix))
    }

    /// The rows of the sheet that carry their number: `(event index, row number)`.
    fn numbered_rows(&self, sheet_data: usize, data_end: usize) -> Vec<(usize, u32)> {
        self.children(sheet_data, data_end)
            .into_iter()
            .filter(|(_, name)| name == "row")
            .filter_map(|(i, _)| {
                let n = self.start_tag(i).and_then(|e| attribute(e, "r"))?.trim().parse::<u32>().ok()?;
                Some((i, n))
            })
            .collect()
    }

    /// The cells of the row at `r` that carry their address: `(event index, column)`.
    fn addressed_cells(&self, r: usize) -> Vec<(usize, u32)> {
        self.children(r, self.close_of(r))
            .into_iter()
            .filter(|(_, name)| name == "c")
            .filter_map(|(i, _)| {
                let at = self.start_tag(i).and_then(|e| attribute(e, "r"))?;
                Some((i, parse_cell(&at)?.0))
            })
            .collect()
    }

    /// Sets the value of the cell at (`col`, `row`) of this sheet. Returns what the cell showed and
    /// what it shows now. The cell is kept aside until [`Part::place_cells`] writes every new cell
    /// of the sheet at once — rows and cells stand in order in a sheet, whatever the order of the
    /// request, and a cell or a row asked for twice is written once.
    pub(super) fn set_cell(
        &mut self,
        col: u32,
        row: u32,
        value: &str,
        wb: &Workbook,
        shared: &[String],
    ) -> std::result::Result<(String, String), &'static str> {
        let (sheet_data, data_end, _, prefix) = self.sheet_data().ok_or(ADDRESS)?;
        let address = format!("{}{row}", column_name(col));

        // What the cell holds: the value an earlier edit of this request gave it, or the file's.
        let (style, before) = match self.new_cells.get(&(row, col)) {
            Some(earlier) => (earlier.style.clone(), earlier.shown.clone()),
            None => {
                let own_row = self.numbered_rows(sheet_data, data_end).into_iter().find(|(_, n)| *n == row);
                let old = own_row
                    .and_then(|(r, _)| self.addressed_cells(r).into_iter().find(|(_, c)| *c == col))
                    .map(|(i, _)| self.read_cell(i));
                if old.as_ref().is_some_and(|c| c.formula) {
                    return Err(FORMULA);
                }
                let before = old.as_ref().map(|c| self.shown_value(c, wb, shared)).unwrap_or_default();
                (old.and_then(|c| c.style), before)
            }
        };
        let shows_a_date = style
            .as_deref()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .is_some_and(|s| wb.date_styles.get(s).copied().unwrap_or(false));
        let new = typed(value, shows_a_date, wb.date1904);

        // -- the cell as it is written
        let c_name = format!("{prefix}c");
        let mut head = BytesStart::new(c_name.clone());
        head.push_attribute(("r", address.as_str()));
        if let Some(s) = &style {
            head.push_attribute(("s", s.as_str()));
        }
        let mut cell: Vec<Event<'static>> = Vec::new();
        match &new {
            Value::Empty => cell.push(Event::Empty(head)),
            Value::Number(n) => {
                let v = format!("{prefix}v");
                cell.push(Event::Start(head));
                cell.push(Event::Start(BytesStart::new(v.clone())));
                cell.push(Event::Text(BytesText::new(n).into_owned()));
                cell.push(Event::End(BytesEnd::new(v)));
                cell.push(Event::End(BytesEnd::new(c_name)));
            }
            Value::Text(text) => {
                head.push_attribute(("t", "inlineStr"));
                let (is, t) = (format!("{prefix}is"), format!("{prefix}t"));
                let mut t_head = BytesStart::new(t.clone());
                if text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace) {
                    t_head.push_attribute(("xml:space", "preserve"));
                }
                cell.push(Event::Start(head));
                cell.push(Event::Start(BytesStart::new(is.clone())));
                cell.push(Event::Start(t_head));
                cell.push(Event::Text(BytesText::new(text).into_owned()));
                cell.push(Event::End(BytesEnd::new(t)));
                cell.push(Event::End(BytesEnd::new(is)));
                cell.push(Event::End(BytesEnd::new(c_name)));
            }
        }
        let after = match new {
            Value::Empty => String::new(),
            Value::Text(text) => text,
            Value::Number(n) if shows_a_date => {
                parse_date(value).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or(n)
            }
            Value::Number(n) => n,
        };
        self.new_cells.insert((row, col), NewCell { events: cell, shown: after.clone(), style });
        Ok((before, after))
    }

    /// Writes the cells [`Part::set_cell`] kept aside into the sheet, rows and cells in their order:
    /// a cell in place of the one that was there, a new cell before the first cell to its right (or
    /// at the end of its row), a new row — one for all its cells — before the first row below it (or
    /// at the end of the sheet's rows).
    pub(super) fn place_cells(&mut self) {
        let new_cells = std::mem::take(&mut self.new_cells);
        if new_cells.is_empty() {
            return;
        }
        let Some((sheet_data, data_end, qualified, prefix)) = self.sheet_data() else { return };
        let rows = self.numbered_rows(sheet_data, data_end);
        let mut by_row: BTreeMap<u32, Vec<(u32, Vec<Event<'static>>)>> = BTreeMap::new();
        for ((row, col), cell) in new_cells {
            by_row.entry(row).or_default().push((col, cell.events));
        }
        // The rows of a sheet that has none yet (`<sheetData/>`).
        let mut first_rows: Vec<Event<'static>> = Vec::new();
        for (row, cells) in by_row {
            match rows.iter().find(|(_, n)| *n == row).map(|(r, _)| *r) {
                Some(r) => {
                    let row_end = self.close_of(r);
                    if row_end == r {
                        // `<row r="3"/>`: the row opens up.
                        let Some(head) = self.start_tag(r).cloned() else { continue };
                        let name = head.name().as_ref().to_string();
                        self.cut.push((r, r));
                        let slot = self.before.entry(r).or_default();
                        slot.push(Event::Start(head));
                        slot.extend(cells.into_iter().flat_map(|(_, events)| events));
                        slot.push(Event::End(BytesEnd::new(name)));
                        continue;
                    }
                    let there = self.addressed_cells(r);
                    for (col, events) in cells {
                        let at = match there.iter().find(|(_, c)| *c == col) {
                            Some((old, _)) => {
                                self.cut.push((*old, self.close_of(*old)));
                                *old
                            }
                            None => there.iter().find(|(_, c)| *c > col).map_or(row_end, |(next, _)| *next),
                        };
                        self.before.entry(at).or_default().extend(events);
                    }
                }
                None => {
                    let row_name = format!("{prefix}row");
                    let mut head = BytesStart::new(row_name.clone());
                    head.push_attribute(("r", row.to_string().as_str()));
                    let mut new_row = vec![Event::Start(head)];
                    new_row.extend(cells.into_iter().flat_map(|(_, events)| events));
                    new_row.push(Event::End(BytesEnd::new(row_name)));
                    if data_end == sheet_data {
                        first_rows.extend(new_row);
                    } else {
                        let at = rows.iter().find(|(_, n)| *n > row).map_or(data_end, |(next, _)| *next);
                        self.before.entry(at).or_default().extend(new_row);
                    }
                }
            }
        }
        if !first_rows.is_empty() {
            self.cut.push((sheet_data, sheet_data));
            let slot = self.before.entry(sheet_data).or_default();
            slot.push(Event::Start(BytesStart::new(qualified.clone())));
            slot.extend(first_rows);
            slot.push(Event::End(BytesEnd::new(qualified)));
        }
    }

    /// The cells of this sheet whose text comes from one of the shared strings `changed`, or from an
    /// inline string that was edited: `(column, row)`.
    pub(super) fn cells_with_changed_text(&self, changed: &HashSet<usize>) -> Vec<(u32, u32)> {
        let edited_inline: HashSet<usize> =
            self.paragraphs.iter().filter(|p| p.changed).filter_map(|p| p.parent).collect();
        let mut out = Vec::new();
        for i in 0..self.events.len() {
            let Some(tag) = self.start_tag(i).filter(|e| local(e.name().as_ref()) == "c") else { continue };
            let Some((col, row)) = attribute(tag, "r").and_then(|r| parse_cell(&r)) else { continue };
            let shared = attribute(tag, "t").as_deref() == Some("s") && {
                let cell = self.read_cell(i);
                cell.value.and_then(|v| v.trim().parse::<usize>().ok()).is_some_and(|s| changed.contains(&s))
            };
            if shared || edited_inline.contains(&i) {
                out.push((col, row));
            }
        }
        out
    }
}

/// A rectangle of cells a formula reads.
struct Read_ {
    /// The sheet it names (`None` = the formula's own).
    sheet: Option<String>,
    cols: (u32, u32),
    rows: (u32, u32),
}

/// The cells a formula reads, or `None` when it reads something that cannot be followed here.
fn reads(formula: &str) -> Option<Vec<Read_>> {
    let chars: Vec<char> = formula.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut sheet: Option<String> = None;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            // A text: up to the closing quote (a doubled quote is a quote inside it).
            i += 1;
            while i < chars.len() && !(chars[i] == '"' && chars.get(i + 1) != Some(&'"')) {
                i += if chars[i] == '"' { 2 } else { 1 };
            }
            i += 1;
        } else if c == '\'' {
            let mut name = String::new();
            i += 1;
            while i < chars.len() && !(chars[i] == '\'' && chars.get(i + 1) != Some(&'\'')) {
                name.push(chars[i]);
                i += if chars[i] == '\'' { 2 } else { 1 };
            }
            i += 1;
            if chars.get(i) != Some(&'!') || name.contains('[') {
                return None;
            }
            i += 1;
            sheet = Some(name);
        } else if c == '[' || c == '#' && chars.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic()) {
            // Another workbook, a table column, an error value.
            return None;
        } else if c.is_alphabetic() || c == '$' || c == '_' {
            let from = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || matches!(chars[i], '$' | '_' | '.')) {
                i += 1;
            }
            let token: String = chars[from..i].iter().collect();
            if chars.get(i) == Some(&'!') {
                sheet = Some(token);
                i += 1;
                continue;
            }
            let mut k = i;
            while chars.get(k) == Some(&' ') {
                k += 1;
            }
            if chars.get(k) == Some(&'(') {
                sheet = None;
                continue;
            }
            let second = |i: &mut usize| -> Option<String> {
                if chars.get(*i) != Some(&':') {
                    return None;
                }
                let from = *i + 1;
                let mut k = from;
                while k < chars.len() && (chars[k].is_ascii_alphanumeric() || chars[k] == '$') {
                    k += 1;
                }
                let token: String = chars[from..k].iter().collect();
                *i = k;
                Some(token)
            };
            let letters_only = |t: &str| {
                let t = t.replace('$', "");
                (!t.is_empty() && t.len() <= 3 && t.chars().all(|c| c.is_ascii_alphabetic()))
                    .then(|| t.chars().fold(0u32, |n, c| n * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1)))
            };
            if let Some((c1, r1)) = parse_cell(&token) {
                let (c2, r2) = match second(&mut i) {
                    Some(other) => parse_cell(&other)?,
                    None => (c1, r1),
                };
                out.push(Read_ { sheet: sheet.take(), cols: (c1.min(c2), c1.max(c2)), rows: (r1.min(r2), r1.max(r2)) });
            } else if let (Some(c1), Some(':')) = (letters_only(&token), chars.get(i).copied()) {
                // Whole columns: `A:C`.
                let c2 = letters_only(&second(&mut i)?)?;
                out.push(Read_ { sheet: sheet.take(), cols: (c1.min(c2), c1.max(c2)), rows: (1, MAX_ROW) });
            } else if matches!(token.to_ascii_uppercase().as_str(), "TRUE" | "FALSE") {
                sheet = None;
            } else {
                // A defined name, a table: not followed.
                return None;
            }
        } else if c.is_ascii_digit() {
            let from = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // Whole rows: `2:5`.
            if chars.get(i) == Some(&':') && chars.get(i + 1).is_some_and(char::is_ascii_digit) {
                let r1: u32 = chars[from..i].iter().collect::<String>().parse().ok()?;
                let from2 = i + 1;
                i = from2;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let r2: u32 = chars[from2..i].iter().collect::<String>().parse().ok()?;
                out.push(Read_ { sheet: sheet.take(), cols: (1, MAX_COL), rows: (r1.min(r2), r1.max(r2)) });
            } else if matches!(chars.get(i), Some('E' | 'e')) {
                // `1E+5`
                i += 1;
                if matches!(chars.get(i), Some('+' | '-')) {
                    i += 1;
                }
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
        } else {
            i += 1;
        }
    }
    Some(out)
}

/// A cell with a formula: where its stored result is, and what it reads.
struct FormulaCell {
    part: usize,
    at: (u32, u32),
    result: Option<(usize, usize)>,
    reads: Option<Vec<Read_>>,
}

/// Takes the stored results out of every formula that reads one of the `changed` cells — directly or
/// through other formulas — and returns how many. `sheet_of` maps a part index to the sheet's name.
pub fn clear_dependents(parts: &mut [Part], sheet_of: &HashMap<usize, String>, changed: &[(usize, u32, u32)]) -> u32 {
    if changed.is_empty() {
        return 0;
    }
    let mut formulas: Vec<FormulaCell> = Vec::new();
    for &p in sheet_of.keys() {
        let part = &parts[p];
        for i in 0..part.events.len() {
            if !matches!(&part.events[i], Event::Start(e) if local(e.name().as_ref()) == "c") {
                continue;
            }
            let Some(at) = part.start_tag(i).and_then(|e| attribute(e, "r")).and_then(|r| parse_cell(&r)) else {
                continue;
            };
            let children = part.children(i, part.close_of(i));
            let Some((f, _)) = children.iter().find(|(_, name)| name == "f") else { continue };
            let text = part.inner_text(*f);
            formulas.push(FormulaCell {
                part: p,
                at,
                result: children.iter().find(|(_, name)| name == "v").map(|(v, _)| (*v, part.close_of(*v))),
                // A shared formula's followers carry no text of their own.
                reads: if text.trim().is_empty() { None } else { reads(&text) },
            });
        }
    }
    let lower = |name: &str| name.to_lowercase();
    let part_of_sheet: HashMap<String, usize> = sheet_of.iter().map(|(p, name)| (lower(name), *p)).collect();
    let mut touched: HashSet<(usize, u32, u32)> = changed.iter().copied().collect();
    let mut stale = vec![false; formulas.len()];
    let follow = formulas.len() <= MAX_FOLLOWED_FORMULAS;
    loop {
        let mut grew = false;
        for (n, formula) in formulas.iter().enumerate() {
            if stale[n] {
                continue;
            }
            let hit = match (&formula.reads, follow) {
                (Some(reads), true) => reads.iter().any(|read| {
                    let part = match &read.sheet {
                        Some(name) => part_of_sheet.get(&lower(name)).copied(),
                        None => Some(formula.part),
                    };
                    touched.iter().any(|(p, col, row)| {
                        Some(*p) == part
                            && (read.cols.0..=read.cols.1).contains(col)
                            && (read.rows.0..=read.rows.1).contains(row)
                    })
                }),
                // Not followed: it may read anything.
                _ => true,
            };
            if hit {
                stale[n] = true;
                grew |= touched.insert((formula.part, formula.at.0, formula.at.1));
            }
        }
        if !grew {
            break;
        }
    }
    let mut cleared = 0u32;
    for (formula, _) in formulas.iter().zip(&stale).filter(|(_, stale)| **stale) {
        if let Some(range) = formula.result {
            parts[formula.part].cut.push(range);
        }
        cleared += 1;
    }
    cleared
}

/// Marks the workbook to be computed in full when a spreadsheet program opens it.
pub fn recalculate_on_load(workbook: &mut Part) {
    let named = |part: &Part, i: usize, name: &str| part.start_tag(i).is_some_and(|e| local(e.name().as_ref()) == name);
    if let Some(i) = (0..workbook.events.len()).find(|i| named(workbook, *i, "calcPr")) {
        workbook.attributes.entry(i).or_default().push(("fullCalcOnLoad".into(), "1".into()));
        return;
    }
    // `calcPr` comes after the sheets, the function groups, the external references and the names.
    let after = ["definedNames", "externalReferences", "functionGroups", "sheets"]
        .iter()
        .find_map(|name| (0..workbook.events.len()).find(|i| named(workbook, *i, name)));
    let Some(i) = after else { return };
    let qualified = workbook.start_tag(i).map(|e| e.name().as_ref().to_string()).unwrap_or_default();
    let prefix = qualified.rsplit_once(':').map(|(p, _)| format!("{p}:")).unwrap_or_default();
    let mut calc = BytesStart::new(format!("{prefix}calcPr"));
    calc.push_attribute(("fullCalcOnLoad", "1"));
    let end = workbook.close_of(i);
    workbook.after.entry(end).or_default().push(Event::Empty(calc));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_values() {
        assert_eq!(parse_address("B3"), Some((None, 2, 3)));
        assert_eq!(parse_address(" $ab$12 "), Some((None, 28, 12)));
        assert_eq!(parse_address("견적!C4"), Some((Some("견적".into()), 3, 4)));
        assert_eq!(parse_address("'1차 견적'!C4"), Some((Some("1차 견적".into()), 3, 4)));
        for bad in ["", "3B", "B", "B0", "XFE1", "!B3", "견적!", "B3:C4"] {
            assert_eq!(parse_address(bad), None, "{bad}");
        }
        assert_eq!(column_name(1), "A");
        assert_eq!(column_name(28), "AB");
        assert_eq!(column_name(16_384), "XFD");

        let text = |v: Value| match v {
            Value::Number(n) => format!("n:{n}"),
            Value::Text(t) => format!("t:{t}"),
            Value::Empty => "empty".to_string(),
        };
        assert_eq!(text(typed("13,000", false, false)), "n:13000");
        assert_eq!(text(typed("-2.5", false, false)), "n:-2.5");
        assert_eq!(text(typed("  ", false, false)), "empty");
        // A code, a mixed text, a date in a cell that shows no dates: text.
        assert_eq!(text(typed("007", false, false)), "t:007");
        assert_eq!(text(typed("13,000원", false, false)), "t:13,000원");
        assert_eq!(text(typed("2026-12-31", false, false)), "t:2026-12-31");
        // … and in a cell that shows dates: the day's number.
        assert_eq!(text(typed("2026-03-01", true, false)), "n:46082");
        assert_eq!(text(typed("2026년 3월 1일", true, false)), "n:46082");
        assert_eq!(text(typed("2026-03-01", true, true)), "n:44620");
        assert_eq!(text(typed("다음 주", true, false)), "t:다음 주");
    }

    #[test]
    fn what_a_formula_reads() {
        let rect = |f: &str| reads(f).map(|r| r.into_iter().map(|x| (x.sheet, x.cols, x.rows)).collect::<Vec<_>>());
        assert_eq!(rect("B2*C2"), Some(vec![(None, (2, 2), (2, 2)), (None, (3, 3), (2, 2))]));
        assert_eq!(rect("SUM($D$2:D10)+ROUND(E1,0)"), Some(vec![(None, (4, 4), (2, 10)), (None, (5, 5), (1, 1))]));
        assert_eq!(rect("SUM(견적!A1:B2)"), Some(vec![(Some("견적".into()), (1, 2), (1, 2))]));
        assert_eq!(rect("'1차 견적'!C4*2"), Some(vec![(Some("1차 견적".into()), (3, 3), (4, 4))]));
        assert_eq!(rect("SUM(A:C)+SUM(2:3)"), Some(vec![(None, (1, 3), (1, MAX_ROW)), (None, (1, MAX_COL), (2, 3))]));
        // A function whose name looks like a cell, a text that looks like one, plain numbers.
        assert_eq!(rect("LOG10(100)+1E+5"), Some(vec![]));
        assert_eq!(rect("IF(A1>0,\"B2 \"\"x\"\"\",TRUE)"), Some(vec![(None, (1, 1), (1, 1))]));
        // What cannot be followed.
        for unknown in ["SUM(금액)", "표1[합계]", "[1]Sheet1!A1", "VLOOKUP(A1,단가표,2,0)"] {
            assert!(reads(unknown).is_none(), "{unknown}");
        }
    }
}
