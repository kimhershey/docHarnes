//! Rich Text Format (`.rtf`) → text: a small tokenizer over groups and control words. Destinations
//! that hold no body text (font / colour / style tables, document info, pictures, field
//! instructions, …) are skipped; `\uN` Unicode escapes honour `\ucN`, `\'hh` bytes follow the
//! document code page (`\ansicpgN`, CP949 for Korean documents); `\par` / `\line` break paragraphs
//! and lines, `\cell` / `\row` build tables.

use std::path::Path;

use crate::error::{CoreError, Result};

use super::model::{Block, Cell, DocModel, Paragraph, Run, Table};

const SKIP_DESTINATIONS: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "object",
    "header",
    "footer",
    "headerl",
    "headerr",
    "headerf",
    "footerl",
    "footerr",
    "footerf",
    "footnote",
    "xmlnstbl",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "generator",
    "themedata",
    "colorschememapping",
    "datastore",
    "latentstyles",
    "pgdsctbl",
    "fldinst",
    "filetbl",
    "revtbl",
    "protusertbl",
    "mmathPr",
    "shpinst",
    "shprslt",
    "nonshppict",
    "template",
    "userprops",
    "docvar",
    "passwordhash",
    "xmlopen",
    "wgrffmtfilter",
    "panose",
    "falt",
    "upr",
    "ud",
    "atnid",
    "atnauthor",
    "annotation",
    "bkmkstart",
    "bkmkend",
    "tc",
    "xe",
    "pntxta",
    "pntxtb",
    "pn",
    "liststylename",
    "listname",
    "levelnumbers",
    "leveltext",
    "pnseclvl",
    "lfolevel",
    "listid",
    "listtemplateid",
    "operator",
    "author",
    "company",
    "title",
    "subject",
    "keywords",
    "comment",
    "doccomm",
    "creatim",
    "revtim",
    "printim",
    "buptim",
    "ftnsep",
    "ftnsepc",
    "ftncn",
    "aftnsep",
    "aftnsepc",
    "aftncn",
];

#[derive(Clone)]
struct Group {
    skip: bool,
    uc: usize,
}

/// Decodes the RTF bytes into paragraphs (`\n`-separated lines; cells separated by `\t`, rows end in `\n`).
pub fn to_text(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut groups: Vec<Group> = vec![Group { skip: false, uc: 1 }];
    let mut i = 0usize;
    let mut pending_bytes: Vec<u8> = Vec::new();
    let mut codepage: Option<u32> = None;
    let mut skip_chars = 0usize; // characters to drop after \uN
    let mut first_in_group = false; // right after '{': a `\*` makes the group ignorable
    let decode_pending = |pending: &mut Vec<u8>, out: &mut String, cp: Option<u32>| {
        if pending.is_empty() {
            return;
        }
        let enc = match cp {
            Some(949) | Some(1361) => encoding_rs::EUC_KR,
            Some(932) => encoding_rs::SHIFT_JIS,
            Some(936) => encoding_rs::GBK,
            Some(950) => encoding_rs::BIG5,
            Some(65001) => encoding_rs::UTF_8,
            _ => encoding_rs::WINDOWS_1252,
        };
        out.push_str(&enc.decode(pending).0);
        pending.clear();
    };
    while i < bytes.len() {
        let b = bytes[i];
        let skipping = groups.last().is_some_and(|g| g.skip);
        match b {
            b'{' => {
                decode_pending(&mut pending_bytes, &mut out, codepage);
                let parent = groups.last().cloned().unwrap_or(Group { skip: false, uc: 1 });
                groups.push(parent);
                first_in_group = true;
                i += 1;
                continue;
            }
            b'}' => {
                decode_pending(&mut pending_bytes, &mut out, codepage);
                if groups.len() > 1 {
                    groups.pop();
                }
                i += 1;
                first_in_group = false;
                continue;
            }
            b'\\' => {
                let next = bytes.get(i + 1).copied();
                match next {
                    Some(b'*') if first_in_group => {
                        // `{\*\dest …}`: an ignorable destination unless it is one we read (none).
                        if let Some(g) = groups.last_mut() {
                            g.skip = true;
                        }
                        i += 2;
                        first_in_group = false;
                        continue;
                    }
                    Some(b'\'') => {
                        let hex = bytes
                            .get(i + 2..i + 4)
                            .and_then(|h| std::str::from_utf8(h).ok())
                            .and_then(|h| u8::from_str_radix(h, 16).ok());
                        i += 4;
                        first_in_group = false;
                        if skip_chars > 0 {
                            skip_chars -= 1;
                            continue;
                        }
                        if !skipping {
                            if let Some(v) = hex {
                                pending_bytes.push(v);
                            }
                        }
                        continue;
                    }
                    Some(c @ (b'{' | b'}' | b'\\')) => {
                        if !skipping && skip_chars == 0 {
                            decode_pending(&mut pending_bytes, &mut out, codepage);
                            out.push(c as char);
                        } else {
                            skip_chars = skip_chars.saturating_sub(1);
                        }
                        i += 2;
                        first_in_group = false;
                        continue;
                    }
                    Some(b'~') => {
                        if !skipping {
                            decode_pending(&mut pending_bytes, &mut out, codepage);
                            out.push('\u{a0}');
                        }
                        i += 2;
                        continue;
                    }
                    Some(b'-') => {
                        i += 2;
                        continue;
                    }
                    Some(b'_') => {
                        if !skipping {
                            decode_pending(&mut pending_bytes, &mut out, codepage);
                            out.push('-');
                        }
                        i += 2;
                        continue;
                    }
                    Some(b'\r') | Some(b'\n') => {
                        // `\` + newline is a paragraph break in some writers.
                        if !skipping {
                            decode_pending(&mut pending_bytes, &mut out, codepage);
                            out.push('\n');
                        }
                        i += 2;
                        continue;
                    }
                    Some(c) if c.is_ascii_alphabetic() => {
                        // Control word with optional signed numeric parameter and one trailing space.
                        let mut j = i + 1;
                        while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                            j += 1;
                        }
                        let word = std::str::from_utf8(&bytes[i + 1..j]).unwrap_or("");
                        let mut k = j;
                        if k < bytes.len() && (bytes[k] == b'-' || bytes[k].is_ascii_digit()) {
                            k += 1;
                            while k < bytes.len() && bytes[k].is_ascii_digit() {
                                k += 1;
                            }
                        }
                        let param: Option<i64> = std::str::from_utf8(&bytes[j..k]).ok().and_then(|p| p.parse().ok());
                        if k < bytes.len() && bytes[k] == b' ' {
                            k += 1;
                        }
                        i = k;
                        let is_first = first_in_group;
                        first_in_group = false;
                        if is_first && SKIP_DESTINATIONS.contains(&word) {
                            if let Some(g) = groups.last_mut() {
                                g.skip = true;
                            }
                            continue;
                        }
                        if word == "fldrslt" {
                            // The field result is body text even inside a field group.
                            if let Some(g) = groups.last_mut() {
                                g.skip = false;
                            }
                            continue;
                        }
                        if word == "ansicpg" {
                            codepage = param.map(|p| p as u32);
                            continue;
                        }
                        if word == "uc" {
                            if let Some(g) = groups.last_mut() {
                                g.uc = param.unwrap_or(1).clamp(0, 10) as usize;
                            }
                            continue;
                        }
                        if skipping {
                            continue;
                        }
                        decode_pending(&mut pending_bytes, &mut out, codepage);
                        match word {
                            "u" => {
                                if let Some(p) = param {
                                    let code = if p < 0 { p + 65_536 } else { p } as u32;
                                    if let Some(ch) = char::from_u32(code) {
                                        out.push(ch);
                                    }
                                    skip_chars = groups.last().map(|g| g.uc).unwrap_or(1);
                                }
                            }
                            "par" | "row" | "sect" | "page" => out.push('\n'),
                            "line" => out.push('\n'),
                            "cell" | "nestcell" => out.push('\t'),
                            "tab" => out.push('\t'),
                            "emdash" => out.push('—'),
                            "endash" => out.push('–'),
                            "lquote" => out.push('‘'),
                            "rquote" => out.push('’'),
                            "ldblquote" => out.push('“'),
                            "rdblquote" => out.push('”'),
                            "bullet" => out.push('•'),
                            "emspace" | "enspace" | "qmspace" => out.push(' '),
                            _ => {}
                        }
                        continue;
                    }
                    _ => {
                        i += 2;
                        continue;
                    }
                }
            }
            b'\r' | b'\n' => {
                i += 1;
                continue;
            }
            _ => {
                first_in_group = false;
                i += 1;
                if skip_chars > 0 {
                    skip_chars -= 1;
                    continue;
                }
                if !skipping {
                    if b < 0x80 {
                        decode_pending(&mut pending_bytes, &mut out, codepage);
                        out.push(b as char);
                    } else {
                        pending_bytes.push(b);
                    }
                }
            }
        }
    }
    decode_pending(&mut pending_bytes, &mut out, codepage);
    out
}

/// Lines with tabs are table rows (grouped), the rest paragraphs; consecutive blank lines collapse.
pub(crate) fn blocks_from_lines(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let flush_rows = |rows: &mut Vec<Vec<Cell>>, blocks: &mut Vec<Block>| {
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
    };
    for line in text.split('\n') {
        let trimmed = line.trim_end();
        if trimmed.contains('\t') {
            let cells: Vec<Cell> = trimmed
                .trim_end_matches('\t')
                .split('\t')
                .map(|c| {
                    let t = c.trim();
                    Cell {
                        blocks: if t.is_empty() {
                            Vec::new()
                        } else {
                            vec![Block::Paragraph(Paragraph::plain(t.to_string()))]
                        },
                        row_span: 1,
                        col_span: 1,
                        header: false,
                    }
                })
                .collect();
            if cells.iter().any(|c| !c.blocks.is_empty()) {
                rows.push(cells);
            }
            continue;
        }
        flush_rows(&mut rows, &mut blocks);
        if trimmed.trim().is_empty() {
            continue;
        }
        blocks.push(Block::Paragraph(Paragraph {
            runs: vec![Run { text: trimmed.trim().to_string(), ..Default::default() }],
            ..Default::default()
        }));
    }
    flush_rows(&mut rows, &mut blocks);
    blocks
}

pub fn read(path: &Path) -> Result<DocModel> {
    let bytes = std::fs::read(path).map_err(|e| CoreError::io(path, e))?;
    if !bytes.starts_with(b"{\\rtf") {
        return Err(CoreError::Convert("not an RTF document".into()));
    }
    let text = to_text(&bytes);
    let mut model = DocModel::default();
    model.blocks = blocks_from_lines(&text);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_unicode_codepage_bytes_fields_and_tables() {
        // Writers escape non-ASCII text as `\uN?` (Unicode with a `?` fallback) or `\'hh` bytes of
        // the document code page; both appear here.
        let uni = |s: &str| s.encode_utf16().map(|u| format!("\\u{}?", u as i16)).collect::<String>();
        let (han, _, _) = encoding_rs::EUC_KR.encode("한빛");
        let mut rtf = String::from(
            r"{\rtf1\ansi\ansicpg949\deff0{\fonttbl{\f0\fnil\fcharset129 Malgun Gothic;}}{\colortbl;\red0\green0\blue0;}{\*\generator Riched20}{\info{\title Secret}}\uc1\pard\b ",
        );
        rtf.push_str(&uni("출장"));
        rtf.push(' ');
        rtf.push_str(&uni("보고서"));
        rtf.push_str(r"\b0\par ");
        rtf.push_str(&uni("거래처"));
        rtf.push_str(": ");
        for b in han.iter() {
            rtf.push_str(&format!("\\'{b:02x}"));
        }
        rtf.push_str(r"\par {\field{\*\fldinst DATE \\@ yyyy}{\fldrslt 2026}}");
        rtf.push_str(&uni("년"));
        rtf.push_str(r"\par \trowd \cellx3000\cellx6000 ");
        rtf.push_str(&uni("품목"));
        rtf.push_str(r"\cell ");
        rtf.push_str(&uni("금액"));
        rtf.push_str(r"\cell\row \trowd \cellx3000\cellx6000 ");
        rtf.push_str(&uni("노트북"));
        rtf.push_str(r"\cell 1,200,000\cell\row \pard caf\'e9 \emdash{} end\line next\par }");
        let model = read_bytes(rtf.as_bytes());
        assert_eq!(model.title.as_deref(), Some("출장 보고서"), "\\uN? escapes");
        let text = model.text();
        assert!(text.contains("거래처: 한빛"), "cp949 bytes: {text}");
        assert!(text.contains("2026년"), "field instruction dropped, result kept: {text}");
        assert!(!text.contains("Secret") && !text.contains("Malgun") && !text.contains("Riched"));
        let table = model
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .expect("table");
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0][0].text().trim(), "품목");
        assert_eq!(table.rows[1][1].text().trim(), "1,200,000");
        assert!(text.contains("— end"), "{text}");
        assert!(text.contains("next"));
    }

    fn read_bytes(bytes: &[u8]) -> DocModel {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.rtf");
        std::fs::write(&p, bytes).unwrap();
        read(&p).unwrap()
    }

    #[test]
    fn rejects_non_rtf() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.rtf");
        std::fs::write(&p, "plain text").unwrap();
        assert!(matches!(read(&p), Err(CoreError::Convert(_))));
    }
}
