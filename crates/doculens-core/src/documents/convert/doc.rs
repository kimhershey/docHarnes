//! Word 97–2003 binary documents (`.doc`, [MS-DOC]) through the `cfb` container (MIT): the FIB of the
//! `WordDocument` stream points at the piece table (Clx in `0Table` / `1Table`), whose pieces map the
//! character positions of the main text to byte ranges (UTF-16LE or 8-bit code page). The characters
//! are then split into paragraphs (`CR`), table rows / cells (`0x07` marks), page breaks (`0x0C`) and
//! line breaks (`0x0B`); field codes are dropped and field results kept. Of the formatting only what
//! the table structure needs is read: the paragraph properties (PlcfBtePapx → PapxFkp pages) say
//! which paragraph is inside a table (`sprmPFInTable`) and which `0x07` mark ends a row
//! (`sprmPFTtp`) — a cell mark and a row-end mark are the same character, so without them an empty
//! cell cannot be told from the end of the row. Styles and character formatting are not read — this
//! is a text extractor, good enough for search, questions and extraction.
//! Word 6 / 95 files (no piece table) fall back to the raw `fcMin..fcMac` text and to the guess
//! "an empty cell ends the row".

use std::io::Read;
use std::path::Path;

use crate::error::{CoreError, Result};

use super::model::{Block, Cell, DocModel, Paragraph, Run, Table};

/// Largest `WordDocument` / table stream we read (OOM guard; .doc files are rarely above a few MB).
const MAX_STREAM: u64 = 256 * 1024 * 1024;

pub(crate) fn read_stream<F: Read + std::io::Seek>(
    comp: &mut cfb::CompoundFile<F>,
    name: &str,
) -> Result<Option<Vec<u8>>> {
    // Stream names are case-insensitive in practice (some writers use `1table`); walk the root.
    let path = comp
        .walk()
        .filter(|e| e.is_stream())
        .find(|e| e.name().eq_ignore_ascii_case(name))
        .map(|e| e.path().to_path_buf());
    let Some(path) = path else { return Ok(None) };
    let mut stream = comp.open_stream(&path).map_err(|e| CoreError::Convert(format!("stream {name}: {e}")))?;
    let len = stream.len();
    if len > MAX_STREAM {
        return Err(CoreError::Convert(format!("stream {name} is too large ({len} bytes)")));
    }
    let mut buf = Vec::with_capacity(len as usize);
    stream.read_to_end(&mut buf).map_err(|e| CoreError::Convert(format!("stream {name}: {e}")))?;
    Ok(Some(buf))
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// One piece of the piece table: `cp_start..cp_end` characters stored at `fc` (bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Piece {
    cp_start: u32,
    cp_end: u32,
    fc: u32,
    compressed: bool,
}

/// Parses the Clx (`Prc`* then `Pcdt` → `PlcPcd`).
fn parse_clx(clx: &[u8]) -> Option<Vec<Piece>> {
    let mut i = 0usize;
    while i < clx.len() {
        match clx[i] {
            0x01 => {
                let len = u16_at(clx, i + 1)? as usize;
                i += 3 + len;
            }
            0x02 => {
                let lcb = u32_at(clx, i + 1)? as usize;
                let plc = clx.get(i + 5..i + 5 + lcb)?;
                if lcb < 4 || (lcb - 4) % 12 != 0 {
                    return None;
                }
                let n = (lcb - 4) / 12;
                let mut pieces = Vec::with_capacity(n);
                for k in 0..n {
                    let cp_start = u32_at(plc, k * 4)?;
                    let cp_end = u32_at(plc, (k + 1) * 4)?;
                    let pcd = (n + 1) * 4 + k * 8;
                    let fc_raw = u32_at(plc, pcd + 2)?;
                    let compressed = fc_raw & 0x4000_0000 != 0;
                    let fc = fc_raw & 0x3FFF_FFFF;
                    pieces.push(Piece { cp_start, cp_end, fc: if compressed { fc / 2 } else { fc }, compressed });
                }
                return Some(pieces);
            }
            _ => return None,
        }
    }
    None
}

/// 8-bit pieces use the document's code page: Korean documents (`lid` 0x0412) are CP949, others CP1252.
fn decode_8bit(bytes: &[u8], korean: bool) -> String {
    let enc = if korean { encoding_rs::EUC_KR } else { encoding_rs::WINDOWS_1252 };
    enc.decode(bytes).0.into_owned()
}

fn decode_utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

/// The main-document characters (first `ccp_text` of them) assembled from the pieces, and the
/// stream offset (FC) of every paragraph / cell mark (`\r`, `0x07`) in text order: the n-th mark
/// character of the text is the n-th entry (neither can be half of a surrogate pair or a
/// double-byte character).
fn main_text(word: &[u8], pieces: &[Piece], ccp_text: u32, korean: bool) -> (String, Vec<u32>) {
    let mut out = String::new();
    let mut marks: Vec<u32> = Vec::new();
    let is_mark = |unit: u16| unit == 0x0D || unit == 0x07;
    let mut remaining = ccp_text as usize;
    for p in pieces {
        if remaining == 0 {
            break;
        }
        let count = (p.cp_end.saturating_sub(p.cp_start) as usize).min(remaining);
        let start = p.fc as usize;
        let text = if p.compressed {
            let end = (start + count).min(word.len());
            if start >= end {
                break;
            }
            let bytes = &word[start..end];
            marks
                .extend(bytes.iter().enumerate().filter(|(_, b)| is_mark(**b as u16)).map(|(k, _)| (start + k) as u32));
            decode_8bit(bytes, korean)
        } else {
            let end = (start + count * 2).min(word.len());
            if start >= end {
                break;
            }
            let bytes = &word[start..end];
            marks.extend(
                bytes
                    .chunks_exact(2)
                    .enumerate()
                    .filter(|(_, c)| is_mark(u16::from_le_bytes([c[0], c[1]])))
                    .map(|(k, _)| (start + k * 2) as u32),
            );
            decode_utf16(bytes)
        };
        remaining -= count.min(remaining);
        out.push_str(&text);
    }
    (out, marks)
}

/// What the paragraph properties say about the paragraph a mark ends.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct MarkProps {
    /// `sprmPFInTable`: the paragraph is inside a table.
    pub in_table: bool,
    /// `sprmPFTtp`: the paragraph is the end of a table row (its `0x07` is the row mark).
    pub row_end: bool,
}

const SPRM_P_F_IN_TABLE: u16 = 0x2416;
const SPRM_P_F_TTP: u16 = 0x2417;
const SPRM_P_HUGE_PAPX: u16 = 0x6646;
const SPRM_P_HUGE_PAPX_OLD: u16 = 0x6645;
const SPRM_T_DEF_TABLE: u16 = 0xD608;
const SPRM_P_CHG_TABS: u16 = 0xC615;

/// Walks a `grpprl` (property modifiers, [MS-DOC] 2.6) and picks up the table flags. `data` is the
/// `Data` stream: a paragraph whose properties do not fit the page (a row of many cells — its
/// `sprmTDefTable` is large) stores them there behind `sprmPHugePapx`.
fn table_flags(grpprl: &[u8], data: Option<&[u8]>, props: &mut MarkProps, depth: u8) {
    let mut i = 0usize;
    while i + 2 <= grpprl.len() {
        let sprm = u16::from_le_bytes([grpprl[i], grpprl[i + 1]]);
        i += 2;
        // Operand size by the `spra` bits; 6 = variable with its own length.
        let size = match sprm >> 13 {
            0 | 1 => 1,
            2 | 4 | 5 => 2,
            3 => 4,
            7 => 3,
            _ => match sprm {
                // cb (2 bytes) counts the rest of the operand plus one.
                SPRM_T_DEF_TABLE => match u16_at(grpprl, i) {
                    Some(cb) => cb as usize + 1,
                    None => return,
                },
                SPRM_P_CHG_TABS => match grpprl.get(i) {
                    Some(255) => {
                        // cTabsDel, 4 bytes per deleted tab, cTabsAdd, 3 bytes per added tab.
                        let Some(del) = grpprl.get(i + 1).map(|d| *d as usize) else { return };
                        let Some(add) = grpprl.get(i + 2 + del * 4).map(|a| *a as usize) else { return };
                        1 + 1 + del * 4 + 1 + add * 3
                    }
                    Some(cb) => *cb as usize + 1,
                    None => return,
                },
                _ => match grpprl.get(i) {
                    Some(cb) => *cb as usize + 1,
                    None => return,
                },
            },
        };
        let Some(operand) = grpprl.get(i..i + size) else { return };
        match sprm {
            SPRM_P_F_IN_TABLE => props.in_table = operand[0] != 0,
            SPRM_P_F_TTP => props.row_end = operand[0] != 0,
            SPRM_P_HUGE_PAPX | SPRM_P_HUGE_PAPX_OLD if depth == 0 => {
                // PrcData in the Data stream: cbGrpprl (2 bytes), then the real grpprl.
                let at = u32::from_le_bytes([operand[0], operand[1], operand[2], operand[3]]) as usize;
                if let Some(d) = data {
                    if let Some(cb) = u16_at(d, at) {
                        if let Some(huge) = d.get(at + 2..at + 2 + cb as usize) {
                            table_flags(huge, data, props, depth + 1);
                        }
                    }
                }
            }
            _ => {}
        }
        i += size;
    }
}

/// The table flags of every paragraph that has any, as `(fc_start, fc_end, flags)` sorted by
/// offset: PlcfBtePapx (table stream, FIB 0x102 / 0x106) lists the 512-byte PapxFkp pages of the
/// `WordDocument` stream; a page holds the offsets of its paragraphs and, per paragraph, where its
/// property list sits in the page. `None` when the structure is missing or damaged.
fn table_paragraphs(word: &[u8], table: &[u8], data: Option<&[u8]>) -> Option<Vec<(u32, u32, MarkProps)>> {
    // FibRgFcLcb97 pair 13 (fcPlcfBtePapx / lcbPlcfBtePapx).
    if (u16_at(word, 0x98)? as usize) < 14 {
        return None;
    }
    let fc = u32_at(word, 0x102)? as usize;
    let lcb = u32_at(word, 0x106)? as usize;
    if lcb < 12 || (lcb - 4) % 8 != 0 {
        return None;
    }
    let plc = table.get(fc..fc.checked_add(lcb)?)?;
    let n = (lcb - 4) / 8;
    let mut out = Vec::new();
    for k in 0..n {
        let pn = (u32_at(plc, (n + 1) * 4 + k * 4)? & 0x003F_FFFF) as usize;
        let Some(page) = word.get(pn * 512..pn * 512 + 512) else { continue };
        let cpara = page[511] as usize;
        // rgfc (cpara + 1 offsets), then one 13-byte BxPap per paragraph.
        if cpara == 0 || (cpara + 1) * 4 + cpara * 13 > 511 {
            continue;
        }
        for i in 0..cpara {
            let (Some(start), Some(end)) = (u32_at(page, i * 4), u32_at(page, (i + 1) * 4)) else { continue };
            let at = page[(cpara + 1) * 4 + i * 13] as usize * 2;
            if at == 0 {
                continue; // default properties: not in a table
            }
            // PapxInFkp: cb (or 0 and cb'), then istd (2 bytes) and the grpprl.
            let (from, len) = match page.get(at) {
                Some(0) => match page.get(at + 1) {
                    Some(cb) => (at + 2, *cb as usize * 2),
                    None => continue,
                },
                Some(cb) => (at + 1, (*cb as usize * 2).saturating_sub(1)),
                None => continue,
            };
            let Some(papx) = page.get(from..(from + len).min(511)) else { continue };
            if papx.len() < 2 {
                continue;
            }
            let mut props = MarkProps::default();
            table_flags(&papx[2..], data, &mut props, 0);
            if props.in_table || props.row_end {
                out.push((start, end, props));
            }
        }
    }
    out.sort_by_key(|(start, _, _)| *start);
    Some(out)
}

/// Flags per mark (`marks`: stream offsets from [`main_text`]) looked up in the table paragraphs.
fn mark_props(marks: &[u32], paragraphs: &[(u32, u32, MarkProps)]) -> Vec<MarkProps> {
    marks
        .iter()
        .map(|fc| {
            let i = paragraphs.partition_point(|(start, _, _)| start <= fc);
            match i.checked_sub(1).map(|i| paragraphs[i]) {
                Some((_, end, props)) if *fc < end => props,
                _ => MarkProps::default(),
            }
        })
        .collect()
}

/// Characters → paragraphs / tables / page breaks (field codes dropped, special marks removed),
/// without paragraph properties: an empty cell mark is taken for the end of the row.
#[cfg(test)]
pub(crate) fn blocks_from_chars(text: &str) -> Vec<Block> {
    blocks_from_marks(text, None)
}

/// Characters → paragraphs / tables / page breaks. `props` holds the table flags of the n-th
/// paragraph / cell mark (`\r`, `0x07`) of the text when the paragraph properties could be read:
/// the rows then end where the file says they do, so an empty cell stays a cell and a cell with
/// several paragraphs stays one cell. Without them (Word 6 / 95, damaged files) an empty mark
/// after a cell is taken for the end of the row.
pub(crate) fn blocks_from_marks(text: &str, props: Option<&[MarkProps]>) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut mark_no = 0usize;
    let mut para = String::new();
    let mut runs: Vec<Run> = Vec::new();
    let mut cells: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut in_table_row = false;
    // Field state: depth of `0x13` begins, and whether we are inside the instruction part.
    let mut field_depth = 0u32;
    let mut in_instruction = 0u32;

    fn flush_para(para: &mut String, runs: &mut Vec<Run>, blocks: &mut Vec<Block>) {
        if !para.is_empty() {
            runs.push(Run { text: std::mem::take(para), ..Default::default() });
        }
        if runs.iter().any(|r| !r.text.trim().is_empty()) {
            blocks.push(Block::Paragraph(Paragraph { runs: std::mem::take(runs), ..Default::default() }));
        } else {
            runs.clear();
        }
    }
    fn flush_table(rows: &mut Vec<Vec<Cell>>, blocks: &mut Vec<Block>) {
        if rows.is_empty() {
            return;
        }
        let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        for r in rows.iter_mut() {
            while r.len() < cols {
                r.push(Cell { row_span: 1, col_span: 1, ..Default::default() });
            }
        }
        blocks.push(Block::Table(Table {
            rows: std::mem::take(rows),
            col_widths_mm: Vec::new(),
            row_heights_mm: Vec::new(),
        }));
    }

    for ch in text.chars() {
        // Marks are counted before anything is skipped (the list is in text order).
        let mark = if ch == '\r' || ch == '\u{7}' {
            mark_no += 1;
            props.and_then(|p| p.get(mark_no - 1).copied())
        } else {
            None
        };
        match ch {
            '\u{13}' => {
                field_depth += 1;
                in_instruction += 1;
                continue;
            }
            '\u{14}' => {
                in_instruction = in_instruction.saturating_sub(1);
                continue;
            }
            '\u{15}' => {
                field_depth = field_depth.saturating_sub(1);
                in_instruction = in_instruction.min(field_depth);
                continue;
            }
            _ if in_instruction > 0 => continue,
            '\u{7}' => {
                // Cell mark or row mark. Text before a line break of this paragraph waits in
                // `runs` (the first cell of a row is not known to be one until its mark).
                let mut cell = String::new();
                for r in runs.drain(..) {
                    cell.push_str(&r.text);
                    if r.line_break {
                        cell.push('\n');
                    }
                }
                cell.push_str(&std::mem::take(&mut para));
                let cell = cell.trim().to_string();
                // The file says which mark ends the row; without that, a mark with nothing since
                // the previous one does.
                let row_end = match mark {
                    Some(m) => m.row_end,
                    None => cell.is_empty() && in_table_row,
                };
                if row_end {
                    if !cell.is_empty() {
                        cells.push(cell);
                    }
                    let row: Vec<Cell> = std::mem::take(&mut cells)
                        .into_iter()
                        .map(|t| Cell {
                            blocks: if t.is_empty() { Vec::new() } else { vec![Block::Paragraph(Paragraph::plain(t))] },
                            row_span: 1,
                            col_span: 1,
                            header: false,
                            style: None,
                        })
                        .collect();
                    if !row.is_empty() {
                        rows.push(row);
                    }
                    in_table_row = false;
                } else {
                    cells.push(cell);
                    in_table_row = true;
                }
            }
            '\r' => {
                if mark.map_or(in_table_row, |m| m.in_table) {
                    // Paragraph break inside a cell: keep it as a line break within the cell text.
                    para.push('\n');
                } else {
                    flush_table(&mut rows, &mut blocks);
                    flush_para(&mut para, &mut runs, &mut blocks);
                }
            }
            '\u{c}' => {
                flush_table(&mut rows, &mut blocks);
                flush_para(&mut para, &mut runs, &mut blocks);
                blocks.push(Block::PageBreak);
            }
            '\u{b}' => {
                if in_table_row {
                    para.push('\n');
                } else {
                    runs.push(Run { text: std::mem::take(&mut para), line_break: true, ..Default::default() });
                }
            }
            '\t' => para.push('\t'),
            '\u{1e}' => para.push('-'),
            '\u{1f}' | '\u{1}' | '\u{2}' | '\u{3}' | '\u{4}' | '\u{5}' | '\u{8}' => {}
            c if c.is_control() => {}
            c => para.push(c),
        }
    }
    if in_table_row {
        cells.push(std::mem::take(&mut para));
        let row: Vec<Cell> = std::mem::take(&mut cells)
            .into_iter()
            .map(|t| Cell {
                blocks: if t.trim().is_empty() {
                    Vec::new()
                } else {
                    vec![Block::Paragraph(Paragraph::plain(t.trim().to_string()))]
                },
                row_span: 1,
                col_span: 1,
                header: false,
                style: None,
            })
            .collect();
        rows.push(row);
    }
    flush_table(&mut rows, &mut blocks);
    flush_para(&mut para, &mut runs, &mut blocks);
    blocks
}

pub fn read(path: &Path) -> Result<DocModel> {
    let mut comp = cfb::open(path).map_err(|e| CoreError::Convert(format!("not a compound file: {e}")))?;
    let word = read_stream(&mut comp, "WordDocument")?
        .ok_or_else(|| CoreError::Convert("WordDocument stream missing (not a Word binary document)".into()))?;
    if word.len() < 0x200 || u16_at(&word, 0) != Some(0xA5EC) {
        return Err(CoreError::Convert("not a Word binary document (bad FIB)".into()));
    }
    let n_fib = u16_at(&word, 2).unwrap_or(0);
    let flags = u16_at(&word, 0x0A).unwrap_or(0);
    if flags & 0x0100 != 0 {
        return Err(CoreError::Convert("encrypted".into()));
    }
    let which_table = if flags & 0x0200 != 0 { "1Table" } else { "0Table" };
    let lid = u16_at(&word, 0x06).unwrap_or(0);
    let korean = lid & 0x3FF == 0x12;
    let fc_min = u32_at(&word, 0x18).unwrap_or(0) as usize;
    let fc_mac = u32_at(&word, 0x1C).unwrap_or(0) as usize;
    let ccp_text = u32_at(&word, 0x4C).unwrap_or(0);
    let fc_clx = u32_at(&word, 0x1A2).unwrap_or(0) as usize;
    let lcb_clx = u32_at(&word, 0x1A6).unwrap_or(0) as usize;
    let mut model = DocModel::default();
    let table = read_stream(&mut comp, which_table)?;
    let mut props: Option<Vec<MarkProps>> = None;
    let text = match (table, n_fib >= 0x65 && lcb_clx > 0) {
        (Some(tbl), true) => {
            let clx = tbl
                .get(fc_clx..fc_clx.saturating_add(lcb_clx))
                .ok_or_else(|| CoreError::Convert("piece table out of range".into()))?;
            let pieces = parse_clx(clx).ok_or_else(|| CoreError::Convert("piece table unreadable".into()))?;
            let (text, marks) = main_text(&word, &pieces, ccp_text, korean);
            // Row ends from the paragraph properties (Word 97 and later). A file whose tables
            // carry no row-end flag at all is not trusted: the guess is used instead.
            if n_fib >= 0xC1 && text.contains('\u{7}') {
                let data = read_stream(&mut comp, "Data")?;
                props = table_paragraphs(&word, &tbl, data.as_deref())
                    .map(|paragraphs| mark_props(&marks, &paragraphs))
                    .filter(|flags| flags.iter().any(|f| f.row_end));
            }
            text
        }
        _ => {
            // Word 6 / 95 or a damaged table stream: the raw text span of the FIB.
            model.notes.push("구형 Word 형식: 본문 텍스트만 읽었습니다".into());
            let end = fc_mac.min(word.len());
            if fc_min >= end {
                return Err(CoreError::Convert("no text in the Word document".into()));
            }
            let bytes = &word[fc_min..end];
            if flags & 0x0001 != 0 {
                // fDot — a template: still text.
            }
            // Word 95 stores Unicode only with the fExtChar flag (bit 12); otherwise 8-bit.
            if flags & 0x1000 != 0 {
                decode_utf16(bytes)
            } else {
                decode_8bit(bytes, korean)
            }
        }
    };
    model.blocks = blocks_from_marks(&text, props.as_deref());
    // Title: the first short paragraph.
    model.title = model.blocks.iter().find_map(|b| match b {
        Block::Paragraph(p) => {
            let t = p.text().trim().to_string();
            (!t.is_empty() && t.chars().count() <= 80).then_some(t)
        }
        _ => None,
    });
    if model.blocks.is_empty() {
        model.notes.push("본문 텍스트가 없습니다".into());
    }
    Ok(model)
}

// ---------------------------------------------------------------------------
// Test fixture writer
// ---------------------------------------------------------------------------

/// Writes a minimal Word 97 binary document holding `text` (one Unicode piece): FIB in `WordDocument`,
/// the piece table in `1Table`. Characters follow [MS-DOC] conventions (`\r` paragraph, `\x07` cell /
/// row marks, `\x0c` page break, `\x13 … \x14 … \x15` fields).
pub fn write_sample_doc(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let units: Vec<u16> = text.encode_utf16().collect();
    let text_fc: u32 = 0x0800;
    let mut word = vec![0u8; text_fc as usize];
    word[0..2].copy_from_slice(&0xA5ECu16.to_le_bytes());
    word[2..4].copy_from_slice(&0x00C1u16.to_le_bytes()); // nFib: Word 97
    word[6..8].copy_from_slice(&0x0412u16.to_le_bytes()); // lid: Korean
    word[0x0A..0x0C].copy_from_slice(&0x0200u16.to_le_bytes()); // fWhichTblStm = 1 → 1Table
    word[0x18..0x1C].copy_from_slice(&text_fc.to_le_bytes()); // fcMin
    word[0x1C..0x20].copy_from_slice(&(text_fc + units.len() as u32 * 2).to_le_bytes()); // fcMac
    word[0x20..0x22].copy_from_slice(&0x000Eu16.to_le_bytes()); // csw
    word[0x3E..0x40].copy_from_slice(&0x0016u16.to_le_bytes()); // cslw
    word[0x4C..0x50].copy_from_slice(&(units.len() as u32).to_le_bytes()); // ccpText
    word[0x98..0x9A].copy_from_slice(&0x005Du16.to_le_bytes()); // cbRgFcLcb
                                                                // Clx: Pcdt only — one piece, Unicode, fc = text_fc.
    let mut clx = vec![0x02u8];
    let n = 1u32;
    let lcb = 4 * (n + 1) + 8 * n;
    clx.extend_from_slice(&lcb.to_le_bytes());
    clx.extend_from_slice(&0u32.to_le_bytes());
    clx.extend_from_slice(&(units.len() as u32).to_le_bytes());
    clx.extend_from_slice(&0u16.to_le_bytes()); // Pcd flags
    clx.extend_from_slice(&text_fc.to_le_bytes()); // fc (bit 30 clear → Unicode)
    clx.extend_from_slice(&0u16.to_le_bytes()); // prm
    word[0x1A2..0x1A6].copy_from_slice(&0u32.to_le_bytes()); // fcClx = 0 (start of 1Table)
    word[0x1A6..0x1AA].copy_from_slice(&(clx.len() as u32).to_le_bytes()); // lcbClx
    for u in &units {
        word.extend_from_slice(&u.to_le_bytes());
    }
    let mut comp = cfb::create(path).map_err(|e| CoreError::io(path, e))?;
    {
        let mut s = comp.create_stream("/WordDocument").map_err(|e| CoreError::io(path, e))?;
        s.write_all(&word).map_err(|e| CoreError::io(path, e))?;
    }
    {
        let mut s = comp.create_stream("/1Table").map_err(|e| CoreError::io(path, e))?;
        s.write_all(&clx).map_err(|e| CoreError::io(path, e))?;
    }
    comp.flush().map_err(|e| CoreError::io(path, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_tables_fields_and_breaks_from_a_word97_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memo.doc");
        let text = "출장 보고서\r작성자: 홍길동\u{13} DATE \\@ \"yyyy\" \u{14}2026\u{15}년\r거래처\u{7}금액\u{7}\u{7}㈜한빛\u{7}1,200,000\u{7}\u{7}비고\u{b}둘째 줄\r\u{c}2쪽 내용\u{1}\r";
        write_sample_doc(&path, text).unwrap();
        let model = read(&path).unwrap();
        assert_eq!(model.title.as_deref(), Some("출장 보고서"));
        let paras: Vec<String> = model
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(p) => Some(p.text()),
                _ => None,
            })
            .collect();
        assert_eq!(paras[0], "출장 보고서");
        assert_eq!(paras[1], "작성자: 홍길동2026년", "field instruction dropped, result kept");
        let table = model
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .expect("table from cell marks");
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0][0].text().trim(), "거래처");
        assert_eq!(table.rows[1][1].text().trim(), "1,200,000");
        let bigo = model
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Paragraph(p) if p.text().starts_with("비고") => Some(p),
                _ => None,
            })
            .unwrap();
        assert!(bigo.runs[0].line_break, "0x0B is a line break");
        assert_eq!(bigo.text(), "비고둘째 줄");
        assert_eq!(model.blocks.iter().filter(|b| matches!(b, Block::PageBreak)).count(), 1);
        assert!(model.text().contains("2쪽 내용"));
        assert!(!model.text().contains('\u{1}'), "picture marks removed");
    }

    fn tables(model: &DocModel) -> Vec<Vec<Vec<String>>> {
        model
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Table(t) => {
                    Some(t.rows.iter().map(|r| r.iter().map(|c| c.text().trim().to_string()).collect()).collect())
                }
                _ => None,
            })
            .collect()
    }

    /// Audit DOC-13: a cell mark and a row mark are the same character (0x07); the reader took an
    /// empty cell for the end of the row, so a form with blanks to fill in came out with its rows
    /// cut apart. The file is a real Word 97 binary written by LibreOffice (`testdata/`, made from
    /// an HTML table): the row ends are read from its paragraph properties (`sprmPFTtp`).
    #[test]
    fn empty_cells_of_a_real_doc_table_stay_cells() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("table.doc");
        std::fs::write(&path, include_bytes!("testdata/table-empty-cells.doc")).unwrap();
        let model = read(&path).unwrap();
        let tables = tables(&model);
        assert_eq!(tables.len(), 2, "{tables:?}");
        let rows = |t: &[&[&str]]| -> Vec<Vec<String>> {
            t.iter().map(|r| r.iter().map(|c| c.to_string()).collect()).collect()
        };
        assert_eq!(
            tables[0],
            rows(&[
                &["항목", "내용", "비고"],
                &["성명", "", "필수"],
                &["", "값2", ""],
                &["", "", ""],
                &["끝", "x", "y"]
            ])
        );
        // A line break and a second paragraph inside a cell (also in the first cell of a row).
        assert_eq!(tables[1], rows(&[&["A", "B\n둘째 줄"], &["문단1\n문단2", ""]]));
        let paras: Vec<String> = model
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(p) => Some(p.text()),
                _ => None,
            })
            .collect();
        assert_eq!(paras, vec!["신청서".to_string(), "표 뒤 문단".into(), "마지막".into()]);
    }

    /// A row of many cells does not fit its properties into the page: they sit in the `Data`
    /// stream behind `sprmPHugePapx`, the row-end flag with them (48 columns, written by LibreOffice).
    #[test]
    fn row_ends_of_a_wide_table_are_read_from_the_data_stream() {
        let bytes: &[u8] = include_bytes!("testdata/table-wide-rows.doc");
        assert!(bytes.windows(2).any(|w| w == SPRM_P_HUGE_PAPX.to_le_bytes()), "the file uses sprmPHugePapx");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wide.doc");
        std::fs::write(&path, bytes).unwrap();
        let model = read(&path).unwrap();
        let tables = tables(&model);
        assert_eq!(tables.len(), 1, "{tables:?}");
        let t = &tables[0];
        assert_eq!((t.len(), t.iter().map(|r| r.len()).collect::<Vec<_>>()), (4, vec![48; 4]));
        assert_eq!((t[0][0].as_str(), t[0][47].as_str()), ("h0", "h47"));
        let every_other: Vec<String> =
            (0..48).map(|c| if c % 2 == 0 { format!("v{c}") } else { String::new() }).collect();
        assert_eq!(t[1], every_other);
        assert!(t[2].iter().all(|c| c.is_empty()), "an empty row is a row");
        assert!(t[3][..47].iter().all(|c| c.is_empty()) && t[3][47] == "last");
    }

    /// Without paragraph properties (the minimal fixture, Word 6 / 95) the old guess stays: an
    /// empty mark after a cell ends the row. A line break in the first cell no longer loses text.
    #[test]
    fn without_paragraph_properties_an_empty_mark_ends_the_row() {
        let blocks = blocks_from_chars("가\u{b}나\u{7}다\u{7}\u{7}라\u{7}마\u{7}\u{7}끝\r");
        let model = DocModel { blocks, ..Default::default() };
        let t = tables(&model);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0], vec![vec!["가\n나".to_string(), "다".into()], vec!["라".into(), "마".into()]]);
        // With the flags of the file the same characters read differently: [가나, 다, ""] is one row.
        let cell = MarkProps { in_table: true, row_end: false };
        let end = MarkProps { in_table: true, row_end: true };
        let flags = [cell, cell, cell, end, cell, end, MarkProps::default()];
        let blocks = blocks_from_marks("가\u{b}나\u{7}다\u{7}\u{7}\u{7}라\u{7}\u{7}끝\r", Some(&flags));
        let t = tables(&DocModel { blocks, ..Default::default() });
        assert_eq!(
            t[0],
            vec![vec!["가\n나".to_string(), "다".into(), "".into()], vec!["라".into(), "".into(), "".into()]]
        );
    }

    #[test]
    fn rejects_non_word_compound_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("other.doc");
        let mut comp = cfb::create(&path).unwrap();
        comp.create_stream("/Workbook").unwrap();
        comp.flush().unwrap();
        drop(comp);
        assert!(matches!(read(&path), Err(CoreError::Convert(m)) if m.contains("WordDocument")));
        std::fs::write(&path, b"plain").unwrap();
        assert!(matches!(read(&path), Err(CoreError::Convert(_))));
    }

    #[test]
    fn eight_bit_pieces_follow_the_code_page() {
        let (bytes, _, _) = encoding_rs::EUC_KR.encode("한글\r");
        assert_eq!(decode_8bit(&bytes, true), "한글\r");
        assert_eq!(decode_8bit(b"caf\xE9", false), "café");
        let pieces = parse_clx(&[
            0x02, 28, 0, 0, 0, // Pcdt lcb = 4*(2+1) + 8*2 = 28
            0, 0, 0, 0, 3, 0, 0, 0, 5, 0, 0, 0, // cps 0, 3, 5
            0, 0, 0x00, 0x08, 0, 0, 0, 0, // piece 1: Unicode at 0x800
            0, 0, 0x00, 0x10, 0, 0x40, 0, 0, // piece 2: compressed, fc 0x40001000 → 0x800 bytes
        ])
        .unwrap();
        assert_eq!(pieces.len(), 2);
        assert!(!pieces[0].compressed && pieces[0].fc == 0x800);
        assert!(pieces[1].compressed && pieces[1].fc == 0x800 && pieces[1].cp_start == 3);
    }
}
