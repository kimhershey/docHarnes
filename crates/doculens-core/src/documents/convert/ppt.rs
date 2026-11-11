//! PowerPoint 97–2003 binary decks (`.ppt`, [MS-PPT]) through the `cfb` container (MIT): the
//! `PowerPoint Document` stream is a sequence of records (8-byte headers, containers nest).
//!
//! Where the text of a slide is: PowerPoint keeps the text of the title / body placeholders in the
//! document container's slide list (`SlideListWithText`, instance 0: per slide a `SlidePersistAtom`
//! followed by `TextHeaderAtom` + `TextCharsAtom` / `TextBytesAtom` pairs); the placeholder shape in
//! the `Slide` container only refers to its entry (`OutlineTextRefAtom`, an index). Text boxes that
//! are not placeholders — and everything in decks written by LibreOffice — carry their text atoms
//! inside the `Slide` container.
//!
//! Which records are live: every save appends changed objects and a new persist directory; the
//! `Current User` stream points at the newest `UserEditAtom`, whose chain of directories maps
//! persist ids to stream offsets (the newest entry wins). The reader follows that: document →
//! slide list (slide order) → slides, so earlier versions of a slide or of the document are not
//! read. A stream without a usable directory (the minimal test fixture, damaged files) is read in
//! stream order instead: every `Slide` container with the text inside it, an exact repetition of the
//! previous slide dropped.
//!
//! Master / notes / handout text is not slide content. Formatting and pictures are not read.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{CoreError, Result};

use super::doc::read_stream;
use super::model::{Block, DocModel, Paragraph};

const RT_DOCUMENT: u16 = 0x03E8;
const RT_SLIDE: u16 = 0x03EE;
const RT_NOTES: u16 = 0x03F0;
const RT_SLIDE_PERSIST_ATOM: u16 = 0x03F3;
const RT_MAIN_MASTER: u16 = 0x03F8;
const RT_HANDOUT: u16 = 0x0FC9;
const RT_OUTLINE_TEXT_REF_ATOM: u16 = 0x0F9E;
const RT_TEXT_HEADER_ATOM: u16 = 0x0F9F;
const RT_TEXT_CHARS_ATOM: u16 = 0x0FA0;
const RT_TEXT_BYTES_ATOM: u16 = 0x0FA8;
const RT_SLIDE_LIST_WITH_TEXT: u16 = 0x0FF0;
const RT_USER_EDIT_ATOM: u16 = 0x0FF5;
const RT_PERSIST_DIRECTORY_ATOM: u16 = 0x1772;

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

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// One record: header fields and its body (cut at the end of the data when the length overruns).
#[derive(Debug, Clone, Copy)]
struct Record<'a> {
    ver_inst: u16,
    rec_type: u16,
    body: &'a [u8],
}

impl Record<'_> {
    fn is_container(&self) -> bool {
        self.ver_inst & 0x000F == 0x000F
    }

    fn instance(&self) -> u16 {
        self.ver_inst >> 4
    }
}

fn record_at(data: &[u8], at: usize) -> Option<Record<'_>> {
    let head = data.get(at..at.checked_add(8)?)?;
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    let start = at + 8;
    let end = start.saturating_add(len).min(data.len());
    Some(Record {
        ver_inst: u16::from_le_bytes([head[0], head[1]]),
        rec_type: u16::from_le_bytes([head[2], head[3]]),
        body: &data[start..end],
    })
}

/// The records of a container body (or of the stream), with their offsets.
fn records(data: &[u8]) -> impl Iterator<Item = (usize, Record<'_>)> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let rec = record_at(data, at)?;
        let here = at;
        at += 8 + rec.body.len();
        Some((here, rec))
    })
}

/// The characters of a text atom. `TextBytesAtom` holds the low bytes of UTF-16 units whose high
/// byte is zero ([MS-PPT] 2.9.43) — Latin-1, not a Windows code page.
fn atom_text(rec: &Record<'_>) -> Option<String> {
    match rec.rec_type {
        RT_TEXT_CHARS_ATOM => {
            let units: Vec<u16> = rec.body.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            Some(String::from_utf16_lossy(&units))
        }
        RT_TEXT_BYTES_ATOM => Some(rec.body.iter().map(|b| *b as char).collect()),
        _ => None,
    }
}

/// The text of one `Slide` container in shape order: text atoms inside it, and the entries of
/// the slide's `outline` list (type, text) its placeholders refer to. Entries no shape refers to
/// still belong to the slide and come first.
fn slide_text(body: &[u8], outline: &[(u32, String)]) -> SlideText {
    fn collect(data: &[u8], outline: &[(u32, String)], used: &mut [bool], out: &mut Vec<(bool, String)>, depth: usize) {
        if depth > 64 {
            return;
        }
        let mut tx_type: Option<u32> = None;
        for (_, rec) in records(data) {
            match rec.rec_type {
                RT_TEXT_HEADER_ATOM => tx_type = u32_at(rec.body, 0),
                RT_TEXT_CHARS_ATOM | RT_TEXT_BYTES_ATOM => {
                    let title = tx_type.take().is_some_and(is_title_type);
                    let text = atom_text(&rec).unwrap_or_default();
                    out.extend(split_paragraphs(&text).into_iter().map(|p| (title, p)));
                }
                RT_OUTLINE_TEXT_REF_ATOM => {
                    let index = u32_at(rec.body, 0).map(|i| i as usize);
                    if let Some((i, (tx, text))) = index.and_then(|i| outline.get(i).map(|e| (i, e))) {
                        used[i] = true;
                        out.extend(split_paragraphs(text).into_iter().map(|p| (is_title_type(*tx), p)));
                    }
                }
                _ if rec.is_container() => collect(rec.body, outline, used, out, depth + 1),
                _ => {}
            }
        }
    }
    let mut used = vec![false; outline.len()];
    let mut placed = Vec::new();
    collect(body, outline, &mut used, &mut placed, 0);
    let mut paragraphs: Vec<(bool, String)> = outline
        .iter()
        .zip(&used)
        .filter(|(_, used)| !**used)
        .flat_map(|((tx, text), _)| split_paragraphs(text).into_iter().map(|p| (is_title_type(*tx), p)))
        .collect();
    paragraphs.extend(placed);
    SlideText { paragraphs }
}

/// Persist id → stream offset of the live objects, and the persist id of the document container:
/// the chain of `UserEditAtom`s from the newest edit (`current_edit`, from the `Current User`
/// stream; else the last one in the stream) back to the first, each with its directory.
fn persist_offsets(data: &[u8], current_edit: Option<usize>) -> Option<(HashMap<u32, usize>, u32)> {
    let last_in_stream = || records(data).filter(|(_, r)| r.rec_type == RT_USER_EDIT_ATOM).map(|(at, _)| at).last();
    let is_edit = |at: &usize| record_at(data, *at).is_some_and(|r| r.rec_type == RT_USER_EDIT_ATOM);
    let mut edit_at = current_edit.filter(is_edit).or_else(last_in_stream)?;
    let mut offsets: HashMap<u32, usize> = HashMap::new();
    let mut document: Option<u32> = None;
    // Bounded: a damaged chain must not loop.
    for _ in 0..10_000 {
        let Some(edit) = record_at(data, edit_at).filter(|r| r.rec_type == RT_USER_EDIT_ATOM) else { break };
        // lastSlideIdRef, version (4), offsetLastEdit, offsetPersistDirectory, docPersistIdRef.
        let (Some(previous), Some(directory), Some(doc)) =
            (u32_at(edit.body, 8), u32_at(edit.body, 12), u32_at(edit.body, 16))
        else {
            break;
        };
        document.get_or_insert(doc);
        if let Some(dir) = record_at(data, directory as usize).filter(|r| r.rec_type == RT_PERSIST_DIRECTORY_ATOM) {
            // Entries: persistId (20 bits) + count (12 bits), then `count` offsets for consecutive ids.
            let mut i = 0usize;
            while let Some(head) = u32_at(dir.body, i) {
                i += 4;
                let (first, count) = (head & 0x000F_FFFF, head >> 20);
                for k in 0..count {
                    let Some(offset) = u32_at(dir.body, i) else { break };
                    i += 4;
                    offsets.entry(first + k).or_insert(offset as usize); // the newer edit was read first
                }
            }
        }
        if previous == 0 || previous as usize >= edit_at {
            break;
        }
        edit_at = previous as usize;
    }
    Some((offsets, document?))
}

/// The slides in deck order through the persist directory; `None` when the stream has none or the
/// document / slide list cannot be found.
fn slides_by_directory(data: &[u8], current_edit: Option<usize>) -> Option<Vec<SlideText>> {
    let (offsets, doc_id) = persist_offsets(data, current_edit)?;
    let document = record_at(data, *offsets.get(&doc_id)?).filter(|r| r.rec_type == RT_DOCUMENT)?;
    // Instance 0 lists the slides (1 = masters, 2 = notes).
    let (_, list) = records(document.body).find(|(_, r)| r.rec_type == RT_SLIDE_LIST_WITH_TEXT && r.instance() == 0)?;
    // Per slide: its persist id and the outline texts that follow its SlidePersistAtom.
    let mut entries: Vec<(u32, Vec<(u32, String)>)> = Vec::new();
    for (_, rec) in records(list.body) {
        match rec.rec_type {
            RT_SLIDE_PERSIST_ATOM => entries.push((u32_at(rec.body, 0)?, Vec::new())),
            RT_TEXT_HEADER_ATOM => {
                if let Some((_, texts)) = entries.last_mut() {
                    texts.push((u32_at(rec.body, 0).unwrap_or(4), String::new()));
                }
            }
            RT_TEXT_CHARS_ATOM | RT_TEXT_BYTES_ATOM => {
                if let Some((_, text)) = entries.last_mut().and_then(|(_, texts)| texts.last_mut()) {
                    *text = atom_text(&rec).unwrap_or_default();
                }
            }
            _ => {}
        }
    }
    let mut slides = Vec::new();
    for (persist_id, outline) in &entries {
        let slide = offsets.get(persist_id).and_then(|at| record_at(data, *at)).filter(|r| r.rec_type == RT_SLIDE);
        let text = match slide {
            Some(s) => slide_text(s.body, outline),
            // The slide object is missing: its placeholder texts are still the slide's text.
            None => slide_text(&[], outline),
        };
        if !text.paragraphs.is_empty() {
            slides.push(text);
        }
    }
    Some(slides)
}

/// Stream order, without the directory: every `Slide` container with the text inside it.
fn slides_in_stream_order(data: &[u8], slides: &mut Vec<SlideText>, depth: usize) {
    if depth > 64 {
        return;
    }
    for (_, rec) in records(data) {
        match rec.rec_type {
            RT_SLIDE if rec.is_container() => {
                let s = slide_text(rec.body, &[]);
                // Incremental saves repeat slides: drop an exact copy of the previous one.
                if !s.paragraphs.is_empty() && slides.last().map(|l| l.paragraphs == s.paragraphs) != Some(true) {
                    slides.push(s);
                }
            }
            // Master / notes / handout text is not slide content.
            RT_MAIN_MASTER | RT_NOTES | RT_HANDOUT if rec.is_container() => {}
            _ if rec.is_container() => slides_in_stream_order(rec.body, slides, depth + 1),
            _ => {}
        }
    }
}

pub fn read(path: &Path) -> Result<DocModel> {
    let mut comp = cfb::open(path).map_err(|e| CoreError::Convert(format!("not a compound file: {e}")))?;
    let data = read_stream(&mut comp, "PowerPoint Document")?.ok_or_else(|| {
        CoreError::Convert("PowerPoint Document stream missing (not a PowerPoint binary deck)".into())
    })?;
    // CurrentUserAtom: record header, size, header token, then offsetToCurrentEdit.
    let current_edit =
        read_stream(&mut comp, "Current User")?.and_then(|cu| u32_at(&cu, 16)).map(|offset| offset as usize);
    let slides = match slides_by_directory(&data, current_edit) {
        Some(slides) if !slides.is_empty() => slides,
        _ => {
            let mut slides = Vec::new();
            slides_in_stream_order(&data, &mut slides, 0);
            slides
        }
    };
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

    /// A deck laid out the way PowerPoint writes it (the layout of PowerPoint-saved files, see
    /// the module comment): placeholder text in the document's slide list, `OutlineTextRefAtom`s
    /// in the slides, three saves chained through `UserEditAtom`s and persist directories.
    /// Persist ids: 1 document, 2 master, 3 first slide, 4 its notes, 5 the slide added later.
    fn write_powerpoint_layout(path: &Path, with_current_user: bool) {
        use std::io::Write;
        fn put(stream: &mut Vec<u8>, rec: Vec<u8>) -> u32 {
            let at = stream.len() as u32;
            stream.extend(rec);
            at
        }
        let persist = |id: u32| {
            let mut body = id.to_le_bytes().to_vec();
            body.extend([0u8; 16]);
            record(0x0000, RT_SLIDE_PERSIST_ATOM, &body)
        };
        // OfficeArtClientTextbox (0xF00D) around what a shape holds.
        let textbox = |inner: Vec<u8>| record(0x000F, 0xF00D, &inner);
        let outline_ref = |i: u32| textbox(record(0x0000, RT_OUTLINE_TEXT_REF_ATOM, &i.to_le_bytes()));
        let slide_list =
            |instance: u16, body: Vec<u8>| record((instance << 4) | 0x000F, RT_SLIDE_LIST_WITH_TEXT, &body);
        let directory = |first: u32, offsets: &[u32]| {
            let mut body = (first | ((offsets.len() as u32) << 20)).to_le_bytes().to_vec();
            offsets.iter().for_each(|o| body.extend(o.to_le_bytes()));
            record(0x0000, RT_PERSIST_DIRECTORY_ATOM, &body)
        };
        let user_edit = |previous: u32, directory: u32| {
            let mut body = vec![0u8; 8]; // lastSlideIdRef, version
            for v in [previous, directory, 1u32, 6u32, 0u32] {
                body.extend(v.to_le_bytes()); // offsetLastEdit, offsetPersistDirectory, docPersistIdRef, seed, view
            }
            record(0x0000, RT_USER_EDIT_ATOM, &body)
        };
        let mut stream = Vec::new();
        // First save: one slide.
        let list = [persist(3), text_chars(6, "옛 제목"), text_chars(5, "옛 부제")].concat();
        let doc = record(0x000F, RT_DOCUMENT, &[slide_list(1, persist(2)), slide_list(0, list)].concat());
        let at_doc = put(&mut stream, doc);
        let at_master = put(&mut stream, record(0x000F, RT_MAIN_MASTER, &textbox(text_chars(0, "마스터 제목"))));
        let at_slide1 = put(&mut stream, record(0x000F, RT_SLIDE, &[outline_ref(0), outline_ref(1)].concat()));
        let at_notes = put(&mut stream, record(0x000F, RT_NOTES, &textbox(text_chars(2, "발표자 노트 내용"))));
        let at_dir = put(&mut stream, directory(1, &[at_doc, at_master, at_slide1, at_notes]));
        let at_edit1 = put(&mut stream, user_edit(0, at_dir));
        // Second save: a slide is added and moved to the front; the first slide's text is edited.
        let mut body2 = record(0x0000, RT_TEXT_HEADER_ATOM, &1u32.to_le_bytes());
        let two_lines: Vec<u8> =
            "신규 거래처 20곳\u{b}기존 거래처 유지".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        body2.extend(record(0x0000, RT_TEXT_CHARS_ATOM, &two_lines));
        let mut title2 = record(0x0000, RT_TEXT_HEADER_ATOM, &0u32.to_le_bytes());
        title2.extend(record(0x0000, RT_TEXT_BYTES_ATOM, b"Caf\xE9 2026")); // low bytes of UTF-16 units
        let list = [
            persist(5),
            title2,
            body2,
            persist(3),
            text_chars(6, "2026 사업 계획"),
            text_chars(5, "경영기획팀\r매출 12% 성장"),
        ]
        .concat();
        let doc = record(0x000F, RT_DOCUMENT, &[slide_list(1, persist(2)), slide_list(0, list)].concat());
        let at_doc = put(&mut stream, doc);
        let old_slide2 = [outline_ref(0), textbox(text_chars(4, "지운 상자"))].concat();
        let at_slide2 = put(&mut stream, record(0x000F, RT_SLIDE, &old_slide2));
        let at_dir_doc = put(&mut stream, directory(1, &[at_doc]));
        let at_edit2a = put(&mut stream, user_edit(at_edit1, at_dir_doc));
        let at_dir_slide = put(&mut stream, directory(5, &[at_slide2]));
        let at_edit2 = put(&mut stream, user_edit(at_edit2a, at_dir_slide));
        // Third save: the added slide again, with a free text box next to its placeholders.
        let slide2 = [outline_ref(0), outline_ref(1), textbox(text_chars(4, "자유 텍스트 상자"))].concat();
        let at_slide2 = put(&mut stream, record(0x000F, RT_SLIDE, &slide2));
        let at_dir = put(&mut stream, directory(5, &[at_slide2]));
        let at_edit3 = put(&mut stream, user_edit(at_edit2, at_dir));

        let mut comp = cfb::create(path).unwrap();
        comp.create_stream("/PowerPoint Document").unwrap().write_all(&stream).unwrap();
        if with_current_user {
            // CurrentUserAtom: size, header token, offsetToCurrentEdit, …
            let mut body = 0x14u32.to_le_bytes().to_vec();
            body.extend(0xE391_C05Fu32.to_le_bytes());
            body.extend(at_edit3.to_le_bytes());
            body.extend([0u8; 8]);
            comp.create_stream("/Current User").unwrap().write_all(&record(0x0000, 0x0FF6, &body)).unwrap();
        }
        comp.flush().unwrap();
    }

    /// Audit DOC-9: PowerPoint stores the text of title / body placeholders in the document's
    /// slide list, not in the slide; the reader only looked inside `Slide` containers, so a deck
    /// saved by PowerPoint came out without its titles and bullet text ("슬라이드 텍스트를 찾지
    /// 못했습니다" when it had nothing but placeholders).
    #[test]
    fn placeholder_text_of_a_powerpoint_saved_deck_is_read_in_slide_order() {
        let dir = tempfile::tempdir().unwrap();
        for with_current_user in [true, false] {
            let path = dir.path().join(format!("deck-{with_current_user}.ppt"));
            write_powerpoint_layout(&path, with_current_user);
            let model = read(&path).unwrap();
            let blocks: Vec<(Option<u8>, String)> = model
                .blocks
                .iter()
                .map(|b| match b {
                    Block::Paragraph(p) => (p.heading, p.text()),
                    Block::PageBreak => (None, "<page>".into()),
                    _ => (None, "<other>".into()),
                })
                .collect();
            let expected: Vec<(Option<u8>, String)> = vec![
                (Some(1), "Café 2026".into()),
                (None, "신규 거래처 20곳\n기존 거래처 유지".into()),
                (None, "자유 텍스트 상자".into()),
                (None, "<page>".into()),
                (Some(1), "2026 사업 계획".into()),
                (None, "경영기획팀".into()),
                (None, "매출 12% 성장".into()),
            ];
            assert_eq!(blocks, expected, "current user stream: {with_current_user}");
            assert_eq!(model.title.as_deref(), Some("Café 2026"));
            assert!(!model.notes.iter().any(|n| n.contains("찾지 못했습니다")));
        }
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
