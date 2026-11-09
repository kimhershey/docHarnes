//! PowerPoint 97–2003 binary decks (`.ppt`, [MS-PPT]) through the `cfb` container (MIT): the
//! `PowerPoint Document` stream is a tree of records (8-byte headers); every `Slide` container's text
//! atoms (`TextCharsAtom` UTF-16LE, `TextBytesAtom` 8-bit) become the slide's paragraphs, the title
//! text (per the preceding `TextHeaderAtom`) its heading. Master / notes / handout containers are
//! skipped. Formatting, pictures and the persist directory (edit history) are not read: slides saved
//! more than once by incremental saves can appear twice, so identical consecutive slides are dropped.

use std::path::Path;

use crate::error::{CoreError, Result};

use super::doc::read_stream;
use super::model::{Block, DocModel, Paragraph};

const RT_DOCUMENT: u16 = 0x03E8;
const RT_SLIDE: u16 = 0x03EE;
const RT_NOTES: u16 = 0x03F0;
const RT_MAIN_MASTER: u16 = 0x03F8;
const RT_HANDOUT: u16 = 0x0FC9;
const RT_TEXT_HEADER_ATOM: u16 = 0x0F9F;
const RT_TEXT_CHARS_ATOM: u16 = 0x0FA0;
const RT_TEXT_BYTES_ATOM: u16 = 0x0FA8;

/// Text placeholder types of `TextHeaderAtom` that are titles.
fn is_title_type(tx_type: u32) -> bool {
    matches!(tx_type, 0 | 6)
}

#[derive(Debug, Default)]
struct SlideText {
    paragraphs: Vec<(bool, String)>, // (is_title, text)
}

fn split_paragraphs(text: &str) -> Vec<String> {
    text.split(['\r', '\n']).map(|p| p.replace('\u{b}', "\n").trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// Walks records in `data`; `inside_slide` says whether we are under a `Slide` container.
fn walk(data: &[u8], inside_slide: bool, slides: &mut Vec<SlideText>, current: &mut Option<SlideText>, depth: usize) {
    if depth > 64 {
        return;
    }
    let mut i = 0usize;
    let mut tx_type: Option<u32> = None;
    while i + 8 <= data.len() {
        let ver_inst = u16::from_le_bytes([data[i], data[i + 1]]);
        let rec_type = u16::from_le_bytes([data[i + 2], data[i + 3]]);
        let len = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
        let body_start = i + 8;
        let body_end = body_start.saturating_add(len).min(data.len());
        let body = &data[body_start..body_end];
        let is_container = ver_inst & 0x000F == 0x000F;
        match rec_type {
            RT_SLIDE if is_container => {
                let mut slide = Some(SlideText::default());
                walk(body, true, slides, &mut slide, depth + 1);
                if let Some(s) = slide {
                    if !s.paragraphs.is_empty() {
                        // Incremental saves repeat slides: drop an exact copy of the previous one.
                        if slides.last().map(|l| l.paragraphs == s.paragraphs) != Some(true) {
                            slides.push(s);
                        }
                    }
                }
            }
            RT_MAIN_MASTER | RT_NOTES | RT_HANDOUT if is_container => {
                // Master / notes / handout text is not slide content.
            }
            RT_TEXT_HEADER_ATOM if inside_slide => {
                tx_type = body.get(0..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            }
            RT_TEXT_CHARS_ATOM if inside_slide => {
                let units: Vec<u16> = body.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                let text = String::from_utf16_lossy(&units);
                let title = tx_type.is_some_and(is_title_type);
                if let Some(cur) = current.as_mut() {
                    cur.paragraphs.extend(split_paragraphs(&text).into_iter().map(|p| (title, p)));
                }
                tx_type = None;
            }
            RT_TEXT_BYTES_ATOM if inside_slide => {
                let text = encoding_rs::WINDOWS_1252.decode(body).0.into_owned();
                let title = tx_type.is_some_and(is_title_type);
                if let Some(cur) = current.as_mut() {
                    cur.paragraphs.extend(split_paragraphs(&text).into_iter().map(|p| (title, p)));
                }
                tx_type = None;
            }
            _ if is_container => walk(body, inside_slide, slides, current, depth + 1),
            _ => {}
        }
        if len == 0 && !is_container {
            i = body_start;
        } else {
            i = body_end.max(body_start);
        }
        if body_end == data.len() {
            break;
        }
    }
}

pub fn read(path: &Path) -> Result<DocModel> {
    let mut comp = cfb::open(path).map_err(|e| CoreError::Convert(format!("not a compound file: {e}")))?;
    let data = read_stream(&mut comp, "PowerPoint Document")?.ok_or_else(|| {
        CoreError::Convert("PowerPoint Document stream missing (not a PowerPoint binary deck)".into())
    })?;
    let mut slides = Vec::new();
    walk(&data, false, &mut slides, &mut None, 0);
    let mut model = DocModel::default();
    for (i, slide) in slides.iter().enumerate() {
        if i > 0 {
            model.blocks.push(Block::PageBreak);
        }
        let has_title = slide.paragraphs.iter().any(|(t, _)| *t);
        if !has_title {
            model.blocks.push(Block::Paragraph(Paragraph {
                heading: Some(3),
                ..Paragraph::plain(format!("슬라이드 {}", i + 1))
            }));
        }
        for (title, text) in &slide.paragraphs {
            let mut p = Paragraph::plain(text.clone());
            if *title {
                p.heading = Some(1);
                if model.title.is_none() {
                    model.title = Some(text.chars().take(120).collect());
                }
            }
            model.blocks.push(Block::Paragraph(p));
        }
    }
    if slides.is_empty() {
        model.notes.push("슬라이드 텍스트를 찾지 못했습니다".into());
    }
    model.notes.push("PPT(97-2003)는 텍스트만 변환됩니다(그림·도형 위치 생략)".into());
    Ok(model)
}

// ---------------------------------------------------------------------------
// Test fixture writer
// ---------------------------------------------------------------------------

fn record(ver_inst: u16, rec_type: u16, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(&ver_inst.to_le_bytes());
    out.extend_from_slice(&rec_type.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

fn text_chars(tx_type: u32, text: &str) -> Vec<u8> {
    let mut out = record(0x0000, RT_TEXT_HEADER_ATOM, &tx_type.to_le_bytes());
    let bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    out.extend(record(0x0000, RT_TEXT_CHARS_ATOM, &bytes));
    out
}

/// Writes a deck with a master (ignored), two slides (title + body each) and a notes container.
pub fn write_sample_ppt(path: &Path) -> Result<()> {
    use std::io::Write;
    let mut stream = Vec::new();
    let master = text_chars(0, "마스터 제목");
    stream.extend(record(0x000F, RT_MAIN_MASTER, &master));
    let mut s1 = text_chars(6, "2026 사업 계획");
    s1.extend(text_chars(1, "경영기획팀\r매출 12% 성장"));
    stream.extend(record(0x000F, RT_SLIDE, &s1));
    let mut s2 = text_chars(0, "목표");
    let body =
        "신규 거래처 20곳\u{b}기존 거래처 유지".encode_utf16().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>();
    s2.extend(record(0x0000, RT_TEXT_HEADER_ATOM, &1u32.to_le_bytes()));
    s2.extend(record(0x0000, RT_TEXT_CHARS_ATOM, &body));
    s2.extend(record(0x0000, RT_TEXT_HEADER_ATOM, &4u32.to_le_bytes()));
    s2.extend(record(0x0000, RT_TEXT_BYTES_ATOM, b"Footer text"));
    stream.extend(record(0x000F, RT_SLIDE, &s2));
    // The same slide saved again (incremental save) is dropped.
    stream.extend(record(0x000F, RT_SLIDE, &s2));
    let notes = text_chars(1, "발표자 노트 내용");
    stream.extend(record(0x000F, RT_NOTES, &notes));
    let doc = record(0x000F, RT_DOCUMENT, &[]);
    let mut all = doc;
    all.extend(stream);
    let mut comp = cfb::create(path).map_err(|e| CoreError::io(path, e))?;
    {
        let mut s = comp.create_stream("/PowerPoint Document").map_err(|e| CoreError::io(path, e))?;
        s.write_all(&all).map_err(|e| CoreError::io(path, e))?;
    }
    comp.flush().map_err(|e| CoreError::io(path, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_slide_text_and_skips_master_notes_and_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deck.ppt");
        write_sample_ppt(&path).unwrap();
        let model = read(&path).unwrap();
        assert_eq!(model.title.as_deref(), Some("2026 사업 계획"));
        assert_eq!(model.blocks.iter().filter(|b| matches!(b, Block::PageBreak)).count(), 1, "duplicate slide dropped");
        let paras: Vec<(Option<u8>, String)> = model
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(p) => Some((p.heading, p.text())),
                _ => None,
            })
            .collect();
        assert_eq!(paras[0], (Some(1), "2026 사업 계획".into()));
        assert_eq!(paras[1], (None, "경영기획팀".into()));
        assert_eq!(paras[2], (None, "매출 12% 성장".into()));
        assert_eq!(paras[3], (Some(1), "목표".into()));
        assert_eq!(paras[4].1, "신규 거래처 20곳\n기존 거래처 유지");
        assert_eq!(paras[5].1, "Footer text");
        let text = model.text();
        assert!(!text.contains("마스터 제목") && !text.contains("발표자 노트 내용"));
    }

    #[test]
    fn rejects_other_compound_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.ppt");
        let mut comp = cfb::create(&path).unwrap();
        comp.create_stream("/WordDocument").unwrap();
        comp.flush().unwrap();
        drop(comp);
        assert!(matches!(read(&path), Err(CoreError::Convert(m)) if m.contains("PowerPoint")));
    }
}
