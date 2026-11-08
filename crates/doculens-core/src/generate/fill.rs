//! In-place filling of registered HWPX / DOCX forms (§3.13 "원본 양식을
//! 템플릿으로 그대로 채움"). The zip is streamed entry by entry; every part
//! is copied raw except the main XML (`word/document.xml`,
//! `Contents/section*.xml`), which is rewritten from its quick-xml events:
//!
//! * (a) a table cell whose text is a field label → the value goes into the
//!   next cell of the same row (its first text node is replaced, the others
//!   cleared; a run is added when the cell has no text at all);
//! * (b) `라벨: ____` / `라벨 : ______` / `라벨:` paragraphs → the blank (or the
//!   end of the line) receives the value;
//! * (c) item tables: the header row whose cells match the draft table's
//!   columns is found, the first data row becomes the row template and is
//!   cloned once per draft row (HWPX `rowAddr` / `rowCnt` are renumbered),
//!   remaining empty template rows are dropped;
//! * (d) fields that could not be placed (and empty required fields) are
//!   reported as warnings.
//!
//! Labels are matched after normalisation (whitespace / punctuation removed,
//! `pipeline::schema::key_from_label` for spelling variants such as 사업자번호).

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::sync::OnceLock;

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer, XmlVersion};
use regex::Regex;

use crate::documents::SourceKind;
use crate::error::{CoreError, Result};
use crate::pipeline::schema::{key_from_label, FieldKind};
use crate::pipeline::validate::normalize_amount;
use crate::settings::Language;

use super::templates::compact;
use super::validate::format_won;
use super::{Draft, DraftField};

/// Largest main XML part rewritten (decompressed).
const MAX_XML_BYTES: u64 = 64 * 1024 * 1024;

fn tr(ko: bool, k: String, e: String) -> String {
    if ko {
        k
    } else {
        e
    }
}

/// Label text normalised for comparison.
fn norm_label(s: &str) -> String {
    compact(s.trim().trim_end_matches([':', '：', '*', '※', '▶', '□', '☐']).trim())
}

fn is_placeholder(s: &str) -> bool {
    let t = s.trim();
    t.is_empty()
        || t.chars().all(|c| {
            matches!(
                c,
                '_' | ' '
                    | '.'
                    | '·'
                    | '('
                    | ')'
                    | '（'
                    | '）'
                    | '□'
                    | '☐'
                    | '-'
                    | '－'
                    | '~'
                    | '∼'
                    | '/'
                    | '\u{a0}'
                    | '\t'
            )
        })
}

/// Column-label aliases keyed by column key (Korean form headers vary).
fn column_aliases(key: &str) -> &'static [&'static str] {
    match key {
        "item_name" | "item" | "name" => &["품명", "품목", "품목명", "상품명", "내역", "항목", "품명및규격"],
        "spec" => &["규격", "사양", "모델"],
        "quantity" => &["수량", "qty", "수"],
        "unit_price" => &["단가", "unitprice"],
        "supply_amount" | "amount" => &["공급가액", "금액", "공급가", "합계", "amount"],
        "tax_amount" | "tax" | "vat" => &["세액", "부가세", "부가가치세", "tax", "vat"],
        "note" => &["비고", "note", "remarks"],
        "document" => &["대상문서", "문서", "문서명", "서류", "서류명"],
        "issue" => &["지적사항", "지적내용", "불일치", "내용", "보완사항", "보완내용"],
        "evidence" => &["근거", "사유", "근거자료"],
        "action" => &["조치요청", "조치사항", "요청사항", "조치", "보완방법"],
        "owner" => &["담당자", "담당"],
        "due_date" => &["기한", "완료일", "일정"],
        _ => &[],
    }
}

fn is_row_number_header(s: &str) -> bool {
    matches!(norm_label(s).as_str(), "no" | "번호" | "순번" | "연번" | "순" | "num" | "#")
}

/// A field to place in the form.
struct Target {
    key: String,
    labels: Vec<String>,
    value: String,
    required: bool,
    placed: bool,
}

impl Target {
    fn matches(&self, cell_text: &str) -> bool {
        if self.placed {
            return false;
        }
        let n = norm_label(cell_text);
        if n.is_empty() || n.chars().count() > 30 {
            return false;
        }
        if self.labels.contains(&n) {
            return true;
        }
        let derived = key_from_label(cell_text.trim().trim_end_matches([':', '：', '*', '※']), "");
        !derived.is_empty() && (derived == self.key || self.key.ends_with(&format!(".{derived}")))
    }
}

fn display_value(f: &DraftField) -> String {
    let v = f.value.trim();
    match FieldKind::parse(&f.kind) {
        Some(FieldKind::Amount) => normalize_amount(v).map(format_won).unwrap_or_else(|| v.to_string()),
        Some(FieldKind::Date | FieldKind::Brn | FieldKind::Phone) => {
            f.normalized.clone().unwrap_or_else(|| v.to_string())
        }
        _ => v.to_string(),
    }
}

/// State shared across the sections of one form.
struct FillState {
    ko: bool,
    targets: Vec<Target>,
    /// Draft tables with values: (key, column keys/labels, rows); `placed` when a form table was found.
    tables: Vec<TableTarget>,
    warnings: Vec<String>,
}

struct TableTarget {
    key: String,
    columns: Vec<(String, String, String)>, // (key, label, kind)
    rows: Vec<Vec<String>>,
    placed: bool,
}

impl FillState {
    fn new(draft: &Draft) -> FillState {
        let ko = draft.language == Language::Ko;
        let targets = draft
            .fields
            .iter()
            .map(|f| Target {
                key: f.key.clone(),
                labels: {
                    let mut v = vec![norm_label(&f.label)];
                    // Group fields ("공급자 상호") also match the bare member label ("상호").
                    if let Some((_, member)) = f.label.rsplit_once(' ') {
                        v.push(norm_label(member));
                    }
                    v.retain(|l| !l.is_empty());
                    v
                },
                value: display_value(f),
                required: f.required,
                placed: false,
            })
            .collect();
        let tables = draft
            .tables
            .iter()
            .filter(|t| !t.columns.is_empty())
            .map(|t| TableTarget {
                key: t.key.clone(),
                columns: t.columns.iter().map(|c| (c.key.clone(), c.label.clone(), c.kind.clone())).collect(),
                rows: t
                    .rows
                    .iter()
                    .map(|r| {
                        (0..t.columns.len())
                            .map(|i| {
                                let v = r.get(i).map(|s| s.trim()).unwrap_or("");
                                match FieldKind::parse(&t.columns[i].kind) {
                                    Some(FieldKind::Amount) => {
                                        normalize_amount(v).map(format_won).unwrap_or_else(|| v.to_string())
                                    }
                                    _ => v.to_string(),
                                }
                            })
                            .collect()
                    })
                    .collect(),
                placed: false,
            })
            .collect();
        FillState { ko, targets, tables, warnings: Vec::new() }
    }

    fn finish(mut self) -> Vec<String> {
        let ko = self.ko;
        let mut unplaced_optional = 0usize;
        for t in &self.targets {
            if t.required && t.value.is_empty() {
                self.warnings.push(tr(
                    ko,
                    format!("필수 항목 '{}'의 값이 비어 있습니다.", t.key),
                    format!("Required field '{}' has no value.", t.key),
                ));
            } else if !t.placed && !t.value.is_empty() {
                if t.required {
                    self.warnings.push(tr(
                        ko,
                        format!("필수 항목 '{}'의 입력란을 양식에서 찾지 못했습니다.", t.key),
                        format!("No slot for the required field '{}' was found in the form.", t.key),
                    ));
                } else {
                    unplaced_optional += 1;
                }
            }
        }
        if unplaced_optional > 0 {
            self.warnings.push(tr(
                ko,
                format!("입력란을 찾지 못한 선택 항목 {unplaced_optional}개는 양식에 기록되지 않았습니다."),
                format!("{unplaced_optional} optional field(s) had no matching slot and were left out."),
            ));
        }
        for t in &self.tables {
            if !t.placed && !t.rows.is_empty() {
                self.warnings.push(tr(
                    ko,
                    format!(
                        "표 '{}'의 머리행을 양식에서 찾지 못해 행 {}개를 기록하지 못했습니다.",
                        t.key,
                        t.rows.len()
                    ),
                    format!("No header row for table '{}' was found; {} row(s) were not written.", t.key, t.rows.len()),
                ));
            }
        }
        self.warnings
    }
}

/// Fills `src` (a DOCX or HWPX form) with the draft's values and writes `out`.
/// Returns non-fatal warnings (unplaced fields, missing values).
pub fn fill_form(src: &Path, kind: SourceKind, draft: &Draft, out: &Path) -> Result<Vec<String>> {
    if !matches!(kind, SourceKind::Docx | SourceKind::Hwpx) {
        return Err(CoreError::Unsupported(format!("{} forms cannot be filled in place", kind.as_str())));
    }
    let file = File::open(src).map_err(|e| CoreError::io(src, e))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| CoreError::Convert(format!("not a zip archive: {e}")))?;
    let out_file = File::create(out).map_err(|e| CoreError::io(out, e))?;
    let mut writer = zip::ZipWriter::new(out_file);
    let mut state = FillState::new(draft);
    let mut main_parts = 0usize;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| CoreError::Convert(format!("zip entry {i}: {e}")))?;
        let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        let is_main = match kind {
            SourceKind::Docx => lower == "word/document.xml",
            _ => lower.starts_with("contents/section") && lower.ends_with(".xml"),
        };
        if is_main {
            if entry.size() > MAX_XML_BYTES {
                return Err(CoreError::Convert(format!("{name} is too large")));
            }
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry
                .by_ref()
                .take(MAX_XML_BYTES)
                .read_to_end(&mut bytes)
                .map_err(|e| CoreError::Convert(format!("{name}: {e}")))?;
            let opts = entry.options();
            drop(entry);
            let rewritten = rewrite(&bytes, kind, &mut state)?;
            writer.start_file(name.as_str(), opts).map_err(|e| CoreError::Convert(format!("{name}: {e}")))?;
            writer.write_all(&rewritten).map_err(|e| CoreError::io(out, e))?;
            main_parts += 1;
        } else {
            writer.raw_copy_file(entry).map_err(|e| CoreError::Convert(format!("{name}: {e}")))?;
        }
    }
    writer.finish().map_err(|e| CoreError::Convert(e.to_string()))?;
    if main_parts == 0 {
        return Err(CoreError::Convert(match kind {
            SourceKind::Docx => "word/document.xml missing (not a DOCX file)".into(),
            _ => "no Contents/section*.xml (not an HWPX file)".into(),
        }));
    }
    Ok(state.finish())
}

// ---------------------------------------------------------------------------
// XML structure
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    Table,
    Row,
    Cell,
    Para,
    Run,
    Text,
    Other,
}

fn local_name(name: &str) -> &str {
    match name.rsplit_once(':') {
        Some((_, local)) => local,
        None => name,
    }
}

/// `"w:"` / `"hp:"` — the namespace prefix the document uses (empty when none).
fn prefix_of(name: &str) -> String {
    match name.rsplit_once(':') {
        Some((prefix, _)) => format!("{prefix}:"),
        None => String::new(),
    }
}

fn tag_of(kind: SourceKind, name: &str) -> Tag {
    match (kind, local_name(name)) {
        (_, "tbl") => Tag::Table,
        (_, "tr") => Tag::Row,
        (_, "tc") => Tag::Cell,
        (_, "p") => Tag::Para,
        (SourceKind::Docx, "r") | (SourceKind::Hwpx, "run") => Tag::Run,
        (_, "t") => Tag::Text,
        _ => Tag::Other,
    }
}

/// `<t>` element: `start..=end` event indices (`start == end` for `<t/>`).
#[derive(Clone, Copy)]
struct TextNode {
    start: usize,
    end: usize,
}

struct ParaInfo {
    start: usize,
    end: usize,
    texts: Vec<usize>, // TextNode indices
    runs: Vec<(usize, usize)>,
    /// Innermost cell containing the paragraph (if any).
    cell: Option<usize>,
}

struct CellInfo {
    start: usize,
    end: usize,
    paras: Vec<usize>,
    texts: Vec<usize>,
}

struct RowInfo {
    start: usize,
    end: usize,
    cells: Vec<usize>,
}

struct TableInfo {
    start: usize,
    rows: Vec<usize>,
}

struct Doc {
    events: Vec<Event<'static>>,
    texts: Vec<TextNode>,
    paras: Vec<ParaInfo>,
    cells: Vec<CellInfo>,
    rows: Vec<RowInfo>,
    tables: Vec<TableInfo>,
    prefix: String,
}

fn resolve_ref(r: &quick_xml::events::BytesRef<'_>) -> Option<String> {
    match r.resolve_char_ref() {
        Ok(Some(c)) => Some(c.to_string()),
        Ok(None) => match r.xml_content(XmlVersion::Implicit1_0).as_ref() {
            "amp" => Some("&".into()),
            "lt" => Some("<".into()),
            "gt" => Some(">".into()),
            "quot" => Some("\"".into()),
            "apos" => Some("'".into()),
            _ => None,
        },
        Err(_) => None,
    }
}

impl Doc {
    fn parse(bytes: &[u8], kind: SourceKind) -> Result<Doc> {
        let mut reader = Reader::from_reader(bytes);
        reader.config_mut().trim_text(false);
        reader.config_mut().expand_empty_elements = false;
        let mut events: Vec<Event<'static>> = Vec::new();
        let mut buf = Vec::new();
        loop {
            let ev = reader.read_event_into(&mut buf).map_err(|e| CoreError::Convert(format!("xml: {e}")))?;
            if matches!(ev, Event::Eof) {
                break;
            }
            events.push(ev.into_owned());
            buf.clear();
        }
        let mut doc = Doc {
            events,
            texts: Vec::new(),
            paras: Vec::new(),
            cells: Vec::new(),
            rows: Vec::new(),
            tables: Vec::new(),
            prefix: String::new(),
        };
        let mut stack: Vec<OpenEl> = Vec::new();
        for i in 0..doc.events.len() {
            match &doc.events[i] {
                Event::Start(s) => {
                    let tag = tag_of(kind, s.name().as_ref());
                    if tag == Tag::Cell && doc.prefix.is_empty() {
                        doc.prefix = prefix_of(s.name().as_ref());
                    }
                    stack.push(Doc::open_el(tag, i));
                }
                Event::Empty(s) => {
                    let tag = tag_of(kind, s.name().as_ref());
                    if tag == Tag::Cell && doc.prefix.is_empty() {
                        doc.prefix = prefix_of(s.name().as_ref());
                    }
                    stack.push(Doc::open_el(tag, i));
                    doc.close(&mut stack, i);
                }
                Event::End(_) => doc.close(&mut stack, i),
                _ => {}
            }
        }
        if doc.prefix.is_empty() {
            doc.prefix = match kind {
                SourceKind::Docx => "w:".into(),
                _ => "hp:".into(),
            };
        }
        Ok(doc)
    }

    fn close(&mut self, stack: &mut Vec<OpenEl>, end: usize) {
        let Some(open) = stack.pop() else { return };
        let OpenEl { tag, start, texts, paras, runs, cells, rows } = open;
        match tag {
            Tag::Text => {
                let idx = self.texts.len();
                self.texts.push(TextNode { start, end });
                // Attach to every open paragraph / cell ancestor (innermost first is enough).
                let mut attached_para = false;
                let mut attached_cell = false;
                for o in stack.iter_mut().rev() {
                    match o.tag {
                        Tag::Para if !attached_para => {
                            o.texts.push(idx);
                            attached_para = true;
                        }
                        Tag::Cell if !attached_cell => {
                            o.texts.push(idx);
                            attached_cell = true;
                        }
                        _ => {}
                    }
                    if attached_para && attached_cell {
                        break;
                    }
                }
            }
            Tag::Run => {
                if let Some(p) = stack.iter_mut().rev().find(|o| o.tag == Tag::Para) {
                    p.runs.push((start, end));
                }
            }
            Tag::Para => {
                let idx = self.paras.len();
                // `cell` is resolved when the enclosing cell closes.
                self.paras.push(ParaInfo { start, end, texts, runs, cell: None });
                if let Some(c) = stack.iter_mut().rev().find(|o| o.tag == Tag::Cell) {
                    c.paras.push(idx);
                }
            }
            Tag::Cell => {
                let idx = self.cells.len();
                for p in &paras {
                    if let Some(pi) = self.paras.get_mut(*p) {
                        pi.cell = Some(idx);
                    }
                }
                self.cells.push(CellInfo { start, end, paras, texts });
                if let Some(r) = stack.iter_mut().rev().find(|o| o.tag == Tag::Row) {
                    r.cells.push(idx);
                }
            }
            Tag::Row => {
                let idx = self.rows.len();
                self.rows.push(RowInfo { start, end, cells });
                if let Some(t) = stack.iter_mut().rev().find(|o| o.tag == Tag::Table) {
                    t.rows.push(idx);
                }
            }
            Tag::Table => {
                self.tables.push(TableInfo { start, rows });
            }
            Tag::Other => {}
        }
    }

    /// Unescaped text of a `<t>` node (tabs / line breaks of HWPX become `\t` / `\n`).
    fn text_of(&self, t: &TextNode) -> String {
        let mut s = String::new();
        if t.start == t.end {
            return s;
        }
        for ev in &self.events[t.start + 1..t.end] {
            match ev {
                Event::Text(x) => s.push_str(&x.xml_content(XmlVersion::Implicit1_0)),
                Event::CData(c) => s.push_str(&c.xml_content(XmlVersion::Implicit1_0)),
                Event::GeneralRef(r) => {
                    if let Some(c) = resolve_ref(r) {
                        s.push_str(&c);
                    }
                }
                Event::Empty(e) | Event::Start(e) => match local_name(e.name().as_ref()) {
                    "tab" => s.push('\t'),
                    "lineBreak" | "br" => s.push('\n'),
                    "nbSpace" | "fwSpace" => s.push('\u{a0}'),
                    _ => {}
                },
                _ => {}
            }
        }
        s
    }

    fn para_text(&self, p: &ParaInfo) -> String {
        p.texts.iter().map(|t| self.text_of(&self.texts[*t])).collect()
    }

    fn cell_text(&self, c: &CellInfo) -> String {
        let mut s = String::new();
        for (i, p) in c.paras.iter().enumerate() {
            if i > 0 {
                s.push('\n');
            }
            s.push_str(&self.para_text(&self.paras[*p]));
        }
        if c.paras.is_empty() {
            s = c.texts.iter().map(|t| self.text_of(&self.texts[*t])).collect();
        }
        s
    }
}

struct OpenEl {
    tag: Tag,
    start: usize,
    texts: Vec<usize>,
    paras: Vec<usize>,
    runs: Vec<(usize, usize)>,
    cells: Vec<usize>,
    rows: Vec<usize>,
}

// ---------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Edits {
    /// `<t>` start index → new text (whole node replaced).
    set_text: HashMap<usize, String>,
    /// Events inserted before the given index.
    insert_before: HashMap<usize, Vec<Event<'static>>>,
    /// Empty-element events replaced by a sequence.
    replace: HashMap<usize, Vec<Event<'static>>>,
    /// Event ranges dropped entirely (inclusive).
    skip: Vec<(usize, usize)>,
    /// Template row start → (row end, per-clone overrides).
    clones: HashMap<usize, RowClone>,
    /// Tables whose rows are renumbered (HWPX): table start → new row count.
    renumber: HashMap<usize, usize>,
}

struct RowClone {
    end: usize,
    /// One override set per output row.
    rows: Vec<Overrides>,
}

/// Per-clone edits (indices are absolute positions inside the template row).
#[derive(Default)]
struct Overrides {
    text: HashMap<usize, String>,
    inserts: HashMap<usize, Vec<Event<'static>>>,
    replace: HashMap<usize, Vec<Event<'static>>>,
}

fn make_elem(name: &str) -> (Event<'static>, Event<'static>) {
    (Event::Start(BytesStart::new(name.to_string())), Event::End(BytesEnd::new(name.to_string())))
}

/// Events for a new text run holding `value` (`<w:r><w:t>…</w:t></w:r>` / `<hp:run><hp:t>…</hp:t></hp:run>`).
fn run_events(kind: SourceKind, prefix: &str, value: &str, char_pr: Option<&str>) -> Vec<Event<'static>> {
    let mut out = Vec::new();
    match kind {
        SourceKind::Docx => {
            let (rs, re) = make_elem(&format!("{prefix}r"));
            let mut t = BytesStart::new(format!("{prefix}t"));
            t.push_attribute(("xml:space", "preserve"));
            out.push(rs);
            out.push(Event::Start(t));
            out.push(Event::Text(BytesText::new(value).into_owned()));
            out.push(Event::End(BytesEnd::new(format!("{prefix}t"))));
            out.push(re);
        }
        _ => {
            let mut r = BytesStart::new(format!("{prefix}run"));
            r.push_attribute(("charPrIDRef", char_pr.unwrap_or("0")));
            out.push(Event::Start(r));
            out.push(Event::Start(BytesStart::new(format!("{prefix}t"))));
            out.push(Event::Text(BytesText::new(value).into_owned()));
            out.push(Event::End(BytesEnd::new(format!("{prefix}t"))));
            out.push(Event::End(BytesEnd::new(format!("{prefix}run"))));
        }
    }
    out
}

fn attr_value(start: &BytesStart<'_>, key: &str) -> Option<String> {
    start
        .attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
}

/// Events that put `value` into a cell without any `<t>`: into its last paragraph's
/// last run (HWPX) / as a new run (DOCX), or a whole new paragraph.
fn fill_empty_cell(
    doc: &Doc,
    kind: SourceKind,
    cell: &CellInfo,
    value: &str,
    set_text: &mut HashMap<usize, String>,
    inserts: &mut HashMap<usize, Vec<Event<'static>>>,
    replace: &mut HashMap<usize, Vec<Event<'static>>>,
) {
    let prefix = doc.prefix.as_str();
    if let Some(&pi) = cell.paras.last() {
        let p = &doc.paras[pi];
        if let Some(&ti) = p.texts.first() {
            set_text.insert(doc.texts[ti].start, value.to_string());
            for t in p.texts.iter().skip(1) {
                set_text.insert(doc.texts[*t].start, String::new());
            }
            return;
        }
        if p.start == p.end {
            // `<p/>` → full paragraph. Keep its attributes (HWPX paraPrIDRef etc.).
            if let Event::Empty(e) = &doc.events[p.start] {
                let mut seq = vec![Event::Start(e.clone().into_owned())];
                seq.extend(run_events(kind, prefix, value, None));
                seq.push(Event::End(BytesEnd::new(e.name().as_ref().to_string())));
                replace.insert(p.start, seq);
            }
            return;
        }
        if kind == SourceKind::Hwpx {
            if let Some(&(rs, re)) = p.runs.last() {
                let char_pr = match &doc.events[rs] {
                    Event::Start(e) | Event::Empty(e) => attr_value(e, "charPrIDRef"),
                    _ => None,
                };
                if rs != re {
                    // Non-empty run without text: add the <t> before its end.
                    let mut seq = Vec::new();
                    seq.push(Event::Start(BytesStart::new(format!("{prefix}t"))));
                    seq.push(Event::Text(BytesText::new(value).into_owned()));
                    seq.push(Event::End(BytesEnd::new(format!("{prefix}t"))));
                    inserts.entry(re).or_default().extend(seq);
                } else {
                    inserts.entry(p.end).or_default().extend(run_events(kind, prefix, value, char_pr.as_deref()));
                }
                return;
            }
        }
        inserts.entry(p.end).or_default().extend(run_events(kind, prefix, value, None));
        return;
    }
    // No paragraph at all.
    match kind {
        SourceKind::Docx => {
            let (ps, pe) = make_elem(&format!("{prefix}p"));
            let mut seq = vec![ps];
            seq.extend(run_events(kind, prefix, value, None));
            seq.push(pe);
            inserts.entry(cell.end).or_default().extend(seq);
        }
        _ => {
            let mut sl = BytesStart::new(format!("{prefix}subList"));
            for (k, v) in [("id", ""), ("textDirection", "HORIZONTAL"), ("lineWrap", "BREAK"), ("vertAlign", "CENTER")]
            {
                sl.push_attribute((k, v));
            }
            let mut p = BytesStart::new(format!("{prefix}p"));
            for (k, v) in [
                ("id", "0"),
                ("paraPrIDRef", "0"),
                ("styleIDRef", "0"),
                ("pageBreak", "0"),
                ("columnBreak", "0"),
                ("merged", "0"),
            ] {
                p.push_attribute((k, v));
            }
            let mut seq = vec![Event::Start(sl), Event::Start(p)];
            seq.extend(run_events(kind, prefix, value, None));
            seq.push(Event::End(BytesEnd::new(format!("{prefix}p"))));
            seq.push(Event::End(BytesEnd::new(format!("{prefix}subList"))));
            inserts.entry(cell.start + 1).or_default().extend(seq);
        }
    }
}

/// Puts `value` into `cell` (first text node replaced, others cleared).
fn fill_cell(
    doc: &Doc,
    kind: SourceKind,
    cell: &CellInfo,
    value: &str,
    set_text: &mut HashMap<usize, String>,
    inserts: &mut HashMap<usize, Vec<Event<'static>>>,
    replace: &mut HashMap<usize, Vec<Event<'static>>>,
) {
    if let Some(&first) = cell.texts.first() {
        set_text.insert(doc.texts[first].start, value.to_string());
        for t in cell.texts.iter().skip(1) {
            set_text.insert(doc.texts[*t].start, String::new());
        }
    } else {
        fill_empty_cell(doc, kind, cell, value, set_text, inserts, replace);
    }
}

fn label_line_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*(?P<label>[^:：_\n]{1,24}?)\s*(?:(?P<sep>[:：])\s*(?P<blank>_{2,}|\(\s*\)|（\s*）|□|☐)?|\s+(?P<blank2>_{3,}))\s*$")
            .expect("static regex")
    })
}

/// Replaces characters `[from, to)` (char offsets of the paragraph text) with `value`.
fn replace_in_paragraph(
    doc: &Doc,
    p: &ParaInfo,
    from: usize,
    to: usize,
    value: &str,
    set_text: &mut HashMap<usize, String>,
) {
    let mut offset = 0usize;
    let mut inserted = false;
    for ti in &p.texts {
        let node = &doc.texts[*ti];
        let text = doc.text_of(node);
        let len = text.chars().count();
        let (a, b) = (offset, offset + len);
        if b > from && a < to {
            let chars: Vec<char> = text.chars().collect();
            let keep_head: String = chars[..from.saturating_sub(a).min(len)].iter().collect();
            let keep_tail: String = chars[to.saturating_sub(a).min(len)..].iter().collect();
            let mut new = keep_head;
            if !inserted {
                new.push_str(value);
                inserted = true;
            }
            new.push_str(&keep_tail);
            set_text.insert(node.start, new);
        }
        offset = b;
    }
    if !inserted {
        if let Some(&last) = p.texts.last() {
            let node = &doc.texts[last];
            let mut new = doc.text_of(node);
            new.push_str(value);
            set_text.insert(node.start, new);
        }
    }
}

/// Header cell → draft column index (or row-number marker) when the cell matches.
#[derive(Clone, Copy, PartialEq)]
enum ColSource {
    Column(usize),
    RowNumber,
}

fn match_header(doc: &Doc, row: &RowInfo, table: &TableTarget) -> Option<(Vec<Option<ColSource>>, usize)> {
    let mut map: Vec<Option<ColSource>> = Vec::with_capacity(row.cells.len());
    let mut matched = 0usize;
    let mut used: HashSet<usize> = HashSet::new();
    for ci in &row.cells {
        let text = doc.cell_text(&doc.cells[*ci]);
        let n = norm_label(&text);
        if n.is_empty() {
            map.push(None);
            continue;
        }
        if is_row_number_header(&text) {
            map.push(Some(ColSource::RowNumber));
            continue;
        }
        let derived = key_from_label(text.trim(), "");
        let hit = table.columns.iter().enumerate().find(|(i, (key, label, _))| {
            !used.contains(i)
                && (norm_label(label) == n
                    || (!derived.is_empty() && derived == *key)
                    || column_aliases(key).iter().any(|a| compact(a) == n))
        });
        match hit {
            Some((i, _)) => {
                used.insert(i);
                matched += 1;
                map.push(Some(ColSource::Column(i)));
            }
            None => map.push(None),
        }
    }
    let need = (table.columns.len().div_ceil(2)).max(2).min(table.columns.len());
    (matched >= need).then_some((map, matched))
}

/// Plans the item-table clones. Returns the rows consumed (header, template, removed).
fn plan_tables(
    doc: &Doc,
    kind: SourceKind,
    state: &mut FillState,
    edits: &mut Edits,
    consumed_rows: &mut HashSet<usize>,
) {
    let mut used_tables: HashSet<usize> = HashSet::new();
    for table in state.tables.iter_mut().filter(|t| !t.placed) {
        // Best header row across tables not used yet.
        let mut best: Option<(usize, usize, Vec<Option<ColSource>>, usize)> = None; // (table idx, row pos, map, matched)
        for (ti, tbl) in doc.tables.iter().enumerate() {
            if used_tables.contains(&ti) {
                continue;
            }
            for (pos, ri) in tbl.rows.iter().enumerate() {
                if consumed_rows.contains(ri) {
                    continue;
                }
                if let Some((map, matched)) = match_header(doc, &doc.rows[*ri], table) {
                    if best.as_ref().is_none_or(|b| matched > b.3) {
                        best = Some((ti, pos, map, matched));
                    }
                }
            }
        }
        let Some((ti, header_pos, map, _)) = best else { continue };
        let tbl = &doc.tables[ti];
        let header_row = tbl.rows[header_pos];
        let Some(&template_row) = tbl.rows.get(header_pos + 1) else {
            // Header without a body row: nothing to clone.
            state.warnings.push(tr(
                state.ko,
                format!("표 '{}'의 머리행 아래에 복제할 행이 없습니다.", table.key),
                format!("Table '{}' has no body row below its header to clone.", table.key),
            ));
            consumed_rows.insert(header_row);
            used_tables.insert(ti);
            continue;
        };
        used_tables.insert(ti);
        consumed_rows.insert(header_row);
        consumed_rows.insert(template_row);
        // Extra empty rows after the template are removed.
        let mut removed = 0usize;
        for &ri in tbl.rows.iter().skip(header_pos + 2) {
            let r = &doc.rows[ri];
            let empty = r.cells.iter().all(|c| is_placeholder(&doc.cell_text(&doc.cells[*c])));
            if empty {
                edits.skip.push((r.start, r.end));
                consumed_rows.insert(ri);
                removed += 1;
            } else {
                break;
            }
        }
        let trow = &doc.rows[template_row];
        let mut clones: Vec<Overrides> = Vec::new();
        for (ri, values) in table.rows.iter().enumerate() {
            let mut o = Overrides::default();
            for (ci, cell_idx) in trow.cells.iter().enumerate() {
                let value = match map.get(ci).copied().flatten() {
                    Some(ColSource::Column(c)) => values.get(c).cloned().unwrap_or_default(),
                    Some(ColSource::RowNumber) => (ri + 1).to_string(),
                    None => String::new(),
                };
                fill_cell(doc, kind, &doc.cells[*cell_idx], &value, &mut o.text, &mut o.inserts, &mut o.replace);
            }
            clones.push(o);
        }
        let new_rows = tbl.rows.len() - 1 - removed + table.rows.len();
        edits.clones.insert(trow.start, RowClone { end: trow.end, rows: clones });
        edits.renumber.insert(tbl.start, new_rows);
        table.placed = true;
    }
}

/// Plans scalar placements: label cells (a) and label lines (b).
fn plan_scalars(doc: &Doc, kind: SourceKind, state: &mut FillState, edits: &mut Edits, consumed_rows: &HashSet<usize>) {
    let mut filled_cells: HashSet<usize> = HashSet::new();
    // (a) label cell → next cell in the same row.
    for (ri, row) in doc.rows.iter().enumerate() {
        if consumed_rows.contains(&ri) {
            continue;
        }
        let mut j = 0usize;
        while j < row.cells.len() {
            let text = doc.cell_text(&doc.cells[row.cells[j]]);
            if let Some(t) = state.targets.iter().position(|t| t.matches(&text)) {
                if let Some(&next) = row.cells.get(j + 1) {
                    let next_text = doc.cell_text(&doc.cells[next]);
                    let next_is_label = state.targets.iter().any(|o| o.matches(&next_text));
                    if !next_is_label && !filled_cells.contains(&next) {
                        let value = state.targets[t].value.clone();
                        if !value.is_empty() {
                            fill_cell(
                                doc,
                                kind,
                                &doc.cells[next],
                                &value,
                                &mut edits.set_text,
                                &mut edits.insert_before,
                                &mut edits.replace,
                            );
                        }
                        state.targets[t].placed = true;
                        filled_cells.insert(next);
                        j += 2;
                        continue;
                    }
                }
            }
            j += 1;
        }
    }
    // (b) "라벨: ____" lines anywhere (outside filled cells and consumed rows).
    let consumed_cells: HashSet<usize> = doc
        .rows
        .iter()
        .enumerate()
        .filter(|(ri, _)| consumed_rows.contains(ri))
        .flat_map(|(_, r)| r.cells.iter().copied())
        .collect();
    let re = label_line_regex();
    for p in &doc.paras {
        if p.cell.is_some_and(|c| filled_cells.contains(&c) || consumed_cells.contains(&c)) {
            continue;
        }
        let text = doc.para_text(p);
        if text.trim().is_empty() || text.contains('\n') {
            continue;
        }
        let Some(cap) = re.captures(&text) else { continue };
        let label = cap.name("label").map(|m| m.as_str()).unwrap_or("");
        let Some(t) = state.targets.iter().position(|t| t.matches(label)) else { continue };
        let value = state.targets[t].value.clone();
        state.targets[t].placed = true;
        if value.is_empty() {
            continue;
        }
        let blank = cap.name("blank").or_else(|| cap.name("blank2"));
        match blank {
            Some(m) => {
                let from = text[..m.start()].chars().count();
                let to = from + m.as_str().chars().count();
                // Keep a space between the separator and the value when the blank followed one directly.
                replace_in_paragraph(doc, p, from, to, &value, &mut edits.set_text);
            }
            None => {
                let end = text.trim_end().chars().count();
                replace_in_paragraph(doc, p, end, end, &format!(" {value}"), &mut edits.set_text);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------

struct Emit<'a> {
    doc: &'a Doc,
    edits: &'a Edits,
    writer: Writer<Vec<u8>>,
    /// Current table start → next row index (HWPX renumbering).
    row_counters: HashMap<usize, usize>,
    /// Stack of open table starts (to know which counter a `<tr>` belongs to).
    open_tables: Vec<usize>,
    /// Row index of the row being emitted, per open table.
    current_row: Vec<usize>,
}

impl Emit<'_> {
    fn write(&mut self, ev: &Event<'static>) -> Result<()> {
        self.writer.write_event(ev.borrow()).map_err(|e| CoreError::Convert(format!("xml write: {e}")))
    }

    fn write_start_with(&mut self, start: &BytesStart<'static>, overrides: &[(&str, String)]) -> Result<()> {
        let mut e = BytesStart::new(start.name().as_ref().to_string());
        for a in start.attributes().flatten() {
            let key = a.key.as_ref().to_string();
            if let Some((_, v)) = overrides.iter().find(|(k, _)| *k == key) {
                e.push_attribute((key.as_str(), v.as_str()));
            } else if let Ok(v) = a.normalized_value(XmlVersion::Implicit1_0) {
                e.push_attribute((key.as_str(), v.as_ref()));
            }
        }
        self.write(&Event::Start(e))
    }

    /// Emits `[start, end]` applying the global edits plus the clone-local overrides.
    fn range(&mut self, start: usize, end: usize, local: &Overrides, allow_clones: bool) -> Result<()> {
        let events = &self.doc.events;
        let mut i = start;
        while i <= end {
            // Global row removal.
            if let Some((_, e)) = self.edits.skip.iter().find(|(s, _)| *s == i) {
                i = e + 1;
                continue;
            }
            // Row clones (template row replaced by the clones).
            if allow_clones {
                if let Some(clone) = self.edits.clones.get(&i) {
                    for o in &clone.rows {
                        self.range(i, clone.end, o, false)?;
                    }
                    i = clone.end + 1;
                    continue;
                }
            }
            if let Some(seq) = local.inserts.get(&i).or_else(|| self.edits.insert_before.get(&i)) {
                for ev in seq.clone() {
                    self.write(&ev)?;
                }
            }
            // Empty-element replacement (clone-local first, then global).
            if let Some(seq) = local.replace.get(&i).or_else(|| self.edits.replace.get(&i)) {
                for ev in seq.clone() {
                    self.write(&ev)?;
                }
                i += 1;
                continue;
            }
            // Text node override.
            let text_override = local.text.get(&i).or_else(|| self.edits.set_text.get(&i));
            if let Some(new_text) = text_override {
                if let Some(node) = self.doc.texts.iter().find(|t| t.start == i) {
                    match &events[i] {
                        Event::Start(s) | Event::Empty(s) => {
                            let name = s.name().as_ref().to_string();
                            let mut st = s.clone().into_owned();
                            if local_name(&name) == "t"
                                && name.starts_with("w:")
                                && attr_value(s, "xml:space").is_none()
                            {
                                st.push_attribute(("xml:space", "preserve"));
                            }
                            self.write(&Event::Start(st))?;
                            self.write(&Event::Text(BytesText::new(new_text).into_owned()))?;
                            self.write(&Event::End(BytesEnd::new(name)))?;
                        }
                        _ => {}
                    }
                    i = node.end + 1;
                    continue;
                }
            }
            match &events[i] {
                Event::Start(s) => {
                    let name = s.name();
                    let tag = local_name(name.as_ref());
                    if tag == "tbl" {
                        self.open_tables.push(i);
                        self.current_row.push(0);
                        if let Some(n) = self.edits.renumber.get(&i) {
                            let n = n.to_string();
                            self.write_start_with(s, &[("rowCnt", n)])?;
                            i += 1;
                            continue;
                        }
                    } else if tag == "tr" {
                        if let Some(t) = self.open_tables.last() {
                            if self.edits.renumber.contains_key(t) {
                                let c = self.row_counters.entry(*t).or_insert(0);
                                *self.current_row.last_mut().unwrap() = *c;
                                *c += 1;
                            }
                        }
                    }
                    self.write(&events[i])?;
                }
                Event::Empty(s) => {
                    let name = s.name();
                    let tag = local_name(name.as_ref());
                    if tag == "cellAddr" {
                        if let Some(t) = self.open_tables.last() {
                            if self.edits.renumber.contains_key(t) {
                                let row = self.current_row.last().copied().unwrap_or(0).to_string();
                                let mut e = BytesStart::new(s.name().as_ref().to_string());
                                for a in s.attributes().flatten() {
                                    let key = a.key.as_ref().to_string();
                                    if key == "rowAddr" {
                                        e.push_attribute(("rowAddr", row.as_str()));
                                    } else if let Ok(v) = a.normalized_value(XmlVersion::Implicit1_0) {
                                        e.push_attribute((key.as_str(), v.as_ref()));
                                    }
                                }
                                self.write(&Event::Empty(e))?;
                                i += 1;
                                continue;
                            }
                        }
                    }
                    self.write(&events[i])?;
                }
                Event::End(e) => {
                    if local_name(e.name().as_ref()) == "tbl" {
                        self.open_tables.pop();
                        self.current_row.pop();
                    }
                    self.write(&events[i])?;
                }
                ev => self.write(ev)?,
            }
            i += 1;
        }
        Ok(())
    }
}

fn rewrite(bytes: &[u8], kind: SourceKind, state: &mut FillState) -> Result<Vec<u8>> {
    let doc = Doc::parse(bytes, kind)?;
    let mut edits = Edits::default();
    let mut consumed_rows: HashSet<usize> = HashSet::new();
    plan_tables(&doc, kind, state, &mut edits, &mut consumed_rows);
    plan_scalars(&doc, kind, state, &mut edits, &consumed_rows);
    let mut emit = Emit {
        doc: &doc,
        edits: &edits,
        writer: Writer::new(Vec::with_capacity(bytes.len() + 1024)),
        row_counters: HashMap::new(),
        open_tables: Vec::new(),
        current_row: Vec::new(),
    };
    if !doc.events.is_empty() {
        emit.range(0, doc.events.len() - 1, &Overrides::default(), true)?;
    }
    Ok(emit.writer.into_inner())
}

impl Doc {
    fn open_el(tag: Tag, start: usize) -> OpenEl {
        OpenEl {
            tag,
            start,
            texts: Vec::new(),
            paras: Vec::new(),
            runs: Vec::new(),
            cells: Vec::new(),
            rows: Vec::new(),
        }
    }
}
