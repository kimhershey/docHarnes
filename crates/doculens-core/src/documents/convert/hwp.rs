//! HWP 5.0 (binary CFB) reader on the `hwp` crate (hahnlee/hwp-rs,
//! Apache-2.0, based on Hancom's public format specification). The crate
//! panics on malformed input, so parsing runs under `catch_unwind` and any
//! panic surfaces as a conversion error. Text, tables, pictures and text
//! boxes are mapped to the document model; headers/footers are skipped.

use std::path::Path;

use hwp::hwp::doc_info::paragraph_shape::{Align as HAlign, ParagraphHeadingKind};
use hwp::hwp::paragraph::char::{Char, CharControls};
use hwp::hwp::paragraph::control::Control;
use hwp::hwp::paragraph::Paragraph as HParagraph;
use hwp::HWP;

use crate::error::{CoreError, Result};

use super::hwpx::hwpunit_to_mm;
use super::model::{Align, Block, Cell, DocModel, ImageData, ListKind, Paragraph, Run, Table};
use super::xml;

/// Largest HWP file we parse in memory.
pub const MAX_HWP_BYTES: u64 = 512 * 1024 * 1024;

struct Ctx<'a> {
    doc: &'a HWP,
    images: Vec<ImageData>,
    notes: Vec<String>,
    loaded: std::collections::HashMap<u16, Option<String>>,
}

impl Ctx<'_> {
    fn image_for(&mut self, bin_item_id: u16) -> Option<String> {
        if let Some(v) = self.loaded.get(&bin_item_id) {
            return v.clone();
        }
        let result = self.load_image(bin_item_id);
        self.loaded.insert(bin_item_id, result.clone());
        result
    }

    fn load_image(&mut self, bin_item_id: u16) -> Option<String> {
        // bin_item_id is 1-based into the BinData list; entries carry their id too.
        let items = &self.doc.doc_info.id_mappings.binary_data;
        let item = items
            .iter()
            .find(|b| b.id == Some(bin_item_id))
            .or_else(|| items.get(bin_item_id.checked_sub(1)? as usize))?;
        let name = item.cfb_file_name()?;
        let file = self.doc.bin_data.iter().find(|f| f.name.eq_ignore_ascii_case(&name))?;
        let mime = xml::mime_for_name(&name).to_string();
        if !xml::is_web_image(&mime) || file.data.is_empty() {
            self.notes.push(format!("이미지 형식 미지원: {name} ({mime})"));
            return None;
        }
        let id = format!("img{}", self.images.len() + 1);
        self.images.push(ImageData { id: id.clone(), mime, bytes: file.data.clone() });
        Some(id)
    }
}

fn char_props(ctx: &Ctx<'_>, shape_id: u32) -> (bool, bool, bool, Option<f32>) {
    match ctx.doc.doc_info.id_mappings.char_shapes.get(shape_id as usize) {
        Some(cs) => {
            let underline = !matches!(cs.underline_kind, hwp::hwp::doc_info::char_shape::UnderlineKind::None);
            let size = if cs.base_size > 0 { Some(cs.base_size as f32 / 100.0) } else { None };
            (cs.bold, cs.italic, underline, size)
        }
        None => (false, false, false, None),
    }
}

/// Converts one HWP paragraph (and the controls anchored in it) into blocks.
fn paragraph_blocks(ctx: &mut Ctx<'_>, p: &HParagraph, out: &mut Vec<Block>) {
    if p.header.page_break && !out.is_empty() {
        out.push(Block::PageBreak);
    }
    let shape = ctx.doc.doc_info.id_mappings.paragraph_shapes.get(p.header.paragraph_shape_id as usize);
    let align = match shape.map(|s| &s.align) {
        Some(HAlign::Center) => Align::Center,
        Some(HAlign::Right) => Align::Right,
        Some(HAlign::Justify) | Some(HAlign::Distributive) => Align::Justify,
        _ => Align::Left,
    };
    let (mut heading, list) = match shape.map(|s| (&s.heading_kind, s.heading_level)) {
        Some((ParagraphHeadingKind::Outline, level)) => (Some((level + 1).min(6)), None),
        Some((ParagraphHeadingKind::Number, _)) => (None, Some(ListKind::Number)),
        Some((ParagraphHeadingKind::Bullet, _)) => (None, Some(ListKind::Bullet)),
        _ => (None, None),
    };
    if heading.is_none() {
        if let Some(style) = ctx.doc.doc_info.id_mappings.styles.get(p.header.style_id as usize) {
            let lower = style.name.to_lowercase();
            let eng = style.english_name.to_lowercase();
            for n in [lower, eng] {
                let rest = n.strip_prefix("개요").or_else(|| n.strip_prefix("outline"));
                if let Some(level) = rest.and_then(|r| r.trim().parse::<u8>().ok()) {
                    heading = Some(level.clamp(1, 6));
                }
            }
        }
    }
    let mut para = Paragraph { heading, align, list, runs: Vec::new(), level: 0 };
    let mut pending: Vec<Block> = Vec::new();

    // Char shapes change at `start_position` (in 16-bit units; controls are 8 units).
    let mut shapes = p.char_shapes.iter().peekable();
    let mut current_shape = shapes.next().map(|s| s.shape_id).unwrap_or(0);
    let mut pos: u32 = 0;
    let mut text = String::new();
    let mut pending_high: Option<u16> = None;
    let mut control_idx = 0usize;
    let flush = |text: &mut String, shape: u32, line_break: bool, para: &mut Paragraph, ctx: &Ctx<'_>| {
        if text.is_empty() && !line_break {
            return;
        }
        let (bold, italic, underline, size_pt) = char_props(ctx, shape);
        para.runs.push(Run { text: std::mem::take(text), bold, italic, underline, size_pt, line_break, color: None });
    };
    for ch in &p.char_list.chars {
        while let Some(next) = shapes.peek() {
            if next.start_position <= pos {
                let s = shapes.next().unwrap();
                if s.shape_id != current_shape {
                    flush(&mut text, current_shape, false, &mut para, ctx);
                    current_shape = s.shape_id;
                }
            } else {
                break;
            }
        }
        match ch {
            Char::CharCode(code) => {
                pos += 1;
                let units: Vec<u16> = match pending_high.take() {
                    Some(hi) => vec![hi, *code],
                    None if (0xD800..0xDC00).contains(code) => {
                        pending_high = Some(*code);
                        continue;
                    }
                    None => vec![*code],
                };
                for c in char::decode_utf16(units) {
                    text.push(c.unwrap_or('\u{FFFD}'));
                }
            }
            Char::CharControl(ctl) => {
                pos += 1;
                match ctl {
                    CharControls::LineBreak => flush(&mut text, current_shape, true, &mut para, ctx),
                    CharControls::ParaBreak => {}
                    CharControls::Hyphen => text.push('-'),
                    CharControls::KeepWordSpace | CharControls::FixedWidthSpace => text.push('\u{a0}'),
                    _ => {}
                }
            }
            Char::InlineControl(code, _) => {
                pos += 8;
                if *code == 9 {
                    text.push('\t');
                }
            }
            Char::ExtendedControl(_, _) => {
                pos += 8;
                let control = p.controls.get(control_idx);
                control_idx += 1;
                let Some(control) = control else { continue };
                match control {
                    Control::Table(t) => {
                        flush(&mut text, current_shape, false, &mut para, ctx);
                        pending.push(Block::Table(table(ctx, t)));
                    }
                    Control::Picture(pic) => {
                        flush(&mut text, current_shape, false, &mut para, ctx);
                        if let Some(id) = ctx.image_for(pic.content.image.bin_item_id) {
                            let cp = &pic.common_properties;
                            pending.push(Block::Image {
                                id,
                                width_mm: Some(hwpunit_to_mm(cp.width as f32)),
                                height_mm: Some(hwpunit_to_mm(cp.height as f32)),
                            });
                        }
                    }
                    Control::GenShapeObject(s) => {
                        if let Some(dt) = &s.draw_text {
                            for inner in &dt.paragraph_list.paragraphs {
                                paragraph_blocks(ctx, inner, &mut pending);
                            }
                        }
                    }
                    Control::ShapeRectangle(s) => {
                        if let Some(dt) = &s.draw_text {
                            for inner in &dt.paragraph_list.paragraphs {
                                paragraph_blocks(ctx, inner, &mut pending);
                            }
                        }
                    }
                    Control::ShapeEllipse(_)
                    | Control::ShapeArc(_)
                    | Control::ShapePolygon(_)
                    | Control::ShapeCurve(_)
                    | Control::ShapeLine(_)
                    | Control::ConnectLine(_)
                    | Control::Container(_)
                    | Control::Ole(_)
                    | Control::Equation(_) => {}
                    Control::AutoNumber(n) => text.push_str(&n.to_string()),
                    Control::Header(_) | Control::Footer(_) => {}
                    Control::Footnote(f) | Control::Endnote(f) => {
                        for inner in &f.paragraph_list.paragraphs {
                            paragraph_blocks(ctx, inner, &mut pending);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    flush(&mut text, current_shape, false, &mut para, ctx);
    if !para.runs.is_empty() || pending.is_empty() {
        out.push(Block::Paragraph(para));
    }
    out.append(&mut pending);
}

fn table(ctx: &mut Ctx<'_>, t: &hwp::hwp::paragraph::control::table::TableControl) -> Table {
    let rows_n = t.record.rows as usize;
    let cols_n = t.record.cols as usize;
    let mut rows: Vec<Vec<(u16, Cell)>> = vec![Vec::new(); rows_n];
    let mut col_widths = vec![0.0f32; cols_n];
    for c in &t.cells {
        let mut blocks = Vec::new();
        for p in &c.paragraph_list.paragraphs {
            paragraph_blocks(ctx, p, &mut blocks);
        }
        let cell = Cell { blocks, row_span: c.row_span.max(1), col_span: c.col_span.max(1), header: false };
        let row = c.row as usize;
        if row >= rows.len() {
            rows.resize(row + 1, Vec::new());
        }
        if c.col_span == 1 && (c.column as usize) < col_widths.len() && col_widths[c.column as usize] == 0.0 {
            col_widths[c.column as usize] = hwpunit_to_mm(c.width as f32);
        }
        rows[row].push((c.column, cell));
    }
    let rows: Vec<Vec<Cell>> = rows
        .into_iter()
        .map(|mut r| {
            r.sort_by_key(|(col, _)| *col);
            r.into_iter().map(|(_, c)| c).collect()
        })
        .filter(|r: &Vec<Cell>| !r.is_empty())
        .collect();
    if col_widths.iter().any(|w| *w <= 0.0) {
        col_widths.clear();
    }
    Table { rows, col_widths_mm: col_widths }
}

pub fn read(path: &Path) -> Result<DocModel> {
    let meta = std::fs::metadata(path).map_err(|e| CoreError::io(path, e))?;
    if meta.len() > MAX_HWP_BYTES {
        return Err(CoreError::Convert(format!("HWP file is too large ({} bytes)", meta.len())));
    }
    let bytes = std::fs::read(path).map_err(|e| CoreError::io(path, e))?;
    // CFB magic; the hwp crate would panic on anything else.
    if !bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Err(CoreError::Convert("not an HWP 5.0 file (compound file header missing)".into()));
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<DocModel> {
        let doc = HWP::from_bytes(&bytes);
        if doc.header.flags.encrypted {
            return Err(CoreError::Unsupported("암호화된 HWP 문서는 읽을 수 없습니다".into()));
        }
        let body = doc.view_texts.as_ref().unwrap_or(&doc.body_texts);
        let mut ctx = Ctx { doc: &doc, images: Vec::new(), notes: Vec::new(), loaded: Default::default() };
        let mut model = DocModel::default();
        for (i, section) in body.sections.iter().enumerate() {
            if i > 0 && !model.blocks.is_empty() {
                model.blocks.push(Block::PageBreak);
            }
            for p in &section.paragraphs {
                paragraph_blocks(&mut ctx, p, &mut model.blocks);
            }
        }
        if doc.header.flags.distributed {
            ctx.notes.push("배포용 문서(읽기 전용)에서 추출했습니다".into());
        }
        model.images = ctx.images;
        model.notes = ctx.notes;
        model.title = model.blocks.iter().find_map(|b| match b {
            Block::Paragraph(p) if !p.is_empty() => Some(p.text().trim().chars().take(120).collect::<String>()),
            _ => None,
        });
        Ok(model)
    }));
    match result {
        Ok(r) => r,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "parser panic".to_string());
            Err(CoreError::Convert(format!("HWP 파서 오류: {msg}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_cfb_and_truncated_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.hwp");
        std::fs::write(&p, b"hello").unwrap();
        assert_eq!(read(&p).unwrap_err().code(), "convert");
        // A CFB header with nothing behind it must not panic the process.
        let mut bytes = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
        bytes.extend(std::iter::repeat_n(0u8, 600));
        std::fs::write(&p, &bytes).unwrap();
        assert_eq!(read(&p).unwrap_err().code(), "convert");
    }
}
