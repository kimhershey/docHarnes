//! Word 97–2003 binary documents (`.doc`, [MS-DOC]) through the `cfb` container (MIT): the FIB of the
//! `WordDocument` stream points at the piece table (Clx in `0Table` / `1Table`), whose pieces map the
//! character positions of the main text to byte ranges (UTF-16LE or 8-bit code page). The characters
//! are then split into paragraphs (`CR`), table rows / cells (`0x07` marks), page breaks (`0x0C`) and
//! line breaks (`0x0B`); field codes are dropped and field results kept. Formatting (styles, PAPX /
//! CHPX) is not read — this is a text extractor, good enough for search, questions and extraction.
//! Word 6 / 95 files (no piece table) fall back to the raw `fcMin..fcMac` text.

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

/// The main-document characters (first `ccp_text` of them) assembled from the pieces.
fn main_text(word: &[u8], pieces: &[Piece], ccp_text: u32, korean: bool) -> String {
    let mut out = String::new();
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
            decode_8bit(&word[start..end], korean)
        } else {
            let end = (start + count * 2).min(word.len());
            if start >= end {
                break;
            }
            decode_utf16(&word[start..end])
        };
        remaining -= count.min(remaining);
        out.push_str(&text);
    }
    out
}

/// Characters → paragraphs / tables / page breaks (field codes dropped, special marks removed).
pub(crate) fn blocks_from_chars(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
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
        blocks.push(Block::Table(Table { rows: std::mem::take(rows), col_widths_mm: Vec::new() }));
    }

    for ch in text.chars() {
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
                // Cell mark; a mark with nothing since the previous one ends the row.
                let cell = std::mem::take(&mut para).trim().to_string();
                if cell.is_empty() && in_table_row {
                    flush_para(&mut String::new(), &mut runs, &mut Vec::new());
                    let row: Vec<Cell> = std::mem::take(&mut cells)
                        .into_iter()
                        .map(|t| Cell {
                            blocks: if t.is_empty() { Vec::new() } else { vec![Block::Paragraph(Paragraph::plain(t))] },
                            row_span: 1,
                            col_span: 1,
                            header: false,
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
                if in_table_row {
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
    let text = match (table, n_fib >= 0x65 && lcb_clx > 0) {
        (Some(tbl), true) => {
            let clx = tbl
                .get(fc_clx..fc_clx.saturating_add(lcb_clx))
                .ok_or_else(|| CoreError::Convert("piece table out of range".into()))?;
            let pieces = parse_clx(clx).ok_or_else(|| CoreError::Convert("piece table unreadable".into()))?;
            main_text(&word, &pieces, ccp_text, korean)
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
    model.blocks = blocks_from_chars(&text);
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
