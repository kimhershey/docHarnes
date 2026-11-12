//! Format-independent document model produced by the HWPX / DOCX / HWP
//! readers and consumed by the HTML emitter and the text extractor. Only the
//! structure the pipeline needs survives (paragraphs, headings, tables with
//! merged cells, images, page breaks); layout fidelity is approximate by
//! design (§3.4: converted documents carry a "변환본" badge).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DocModel {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
    /// Embedded binaries referenced by `Block::Image { id }`.
    pub images: Vec<ImageData>,
    /// Non-fatal reader notes (unsupported objects, skipped sections).
    pub notes: Vec<String>,
    /// Page size in millimetres when the source declares one.
    pub page_mm: Option<(f32, f32)>,
    /// Slide decks (PPTX): one page per slide with absolutely positioned shapes. `blocks` still
    /// holds the reading-order text of the same slides (search, field discovery, text fallback).
    #[serde(default)]
    pub slides: Vec<Slide>,
}

/// Vertical anchoring of the text inside a slide shape (`a:bodyPr anchor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VAnchor {
    #[default]
    Top,
    Middle,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShapeKind {
    Text {
        paragraphs: Vec<Paragraph>,
        anchor: VAnchor,
        /// `normAutofit fontScale` (1.0 = none).
        font_scale: f32,
        /// Text insets in millimetres: left, top, right, bottom.
        insets_mm: [f32; 4],
        /// Default size of runs without an explicit size (placeholder level), points.
        default_pt: f32,
        /// Default text colour of the shape (master text style), CSS.
        default_color: Option<String>,
    },
    Picture {
        id: String,
    },
    Table(Table),
    /// Content the converter cannot draw (chart, SmartArt, OLE, vector image): a labelled frame.
    Placeholder {
        label: String,
    },
    /// Straight connector drawn along the box (horizontal when wide, vertical when tall).
    Line,
}

/// A positioned shape of a slide; coordinates in millimetres from the slide's top-left corner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub x_mm: f32,
    pub y_mm: f32,
    pub w_mm: f32,
    pub h_mm: f32,
    pub rotation_deg: f32,
    pub flip_h: bool,
    pub flip_v: bool,
    /// CSS colour (`#rrggbb`) of a solid fill.
    pub fill: Option<String>,
    /// Outline colour and width in points.
    pub line: Option<(String, f32)>,
    /// Preset geometry hint (`ellipse`, `roundRect`, …); `None` = rectangle.
    pub geometry: Option<String>,
    pub kind: ShapeKind,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Slide {
    pub width_mm: f32,
    pub height_mm: f32,
    /// Solid background colour when the slide / layout / master declares one.
    pub background: Option<String>,
    /// Z-order (document order): later shapes draw over earlier ones.
    pub shapes: Vec<Shape>,
    pub notes: Vec<Paragraph>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageData {
    pub id: String,
    /// `image/png`, `image/jpeg`, …
    pub mime: String,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Paragraph(Paragraph),
    Table(Table),
    Image { id: String, width_mm: Option<f32>, height_mm: Option<f32> },
    PageBreak,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Paragraph {
    pub runs: Vec<Run>,
    /// 1-6 for headings, `None` for body text.
    pub heading: Option<u8>,
    pub align: Align,
    /// Ordered/unordered list item.
    pub list: Option<ListKind>,
    /// Indent level (slide bullets: `a:pPr lvl`), 0 = top level.
    #[serde(default)]
    pub level: u8,
}

impl Paragraph {
    pub fn text(&self) -> String {
        let mut s = String::new();
        for r in &self.runs {
            s.push_str(&r.text);
        }
        s
    }

    pub fn is_empty(&self) -> bool {
        self.runs.iter().all(|r| r.text.trim().is_empty())
    }

    pub fn plain(text: impl Into<String>) -> Paragraph {
        Paragraph { runs: vec![Run { text: text.into(), ..Default::default() }], ..Default::default() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    Bullet,
    Number,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Font size in points when known.
    pub size_pt: Option<f32>,
    /// Hard line break *after* this run.
    pub line_break: bool,
    /// CSS text colour (`#rrggbb`) when the source sets one (slides).
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Table {
    pub rows: Vec<Vec<Cell>>,
    /// Column widths in millimetres when known (same length as the widest row).
    pub col_widths_mm: Vec<f32>,
    /// Row heights in millimetres when the source gives them (1.0.0: PPTX tables); a row grows with its text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub row_heights_mm: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Cell {
    pub blocks: Vec<Block>,
    pub row_span: u16,
    pub col_span: u16,
    pub header: bool,
    /// How the cell is drawn when the source says so (1.0.0: PPTX table styles and cell properties).
    /// `None`: the writer's own plain look.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<Box<CellStyle>>,
}

/// The look of a table cell: what a PPTX table style and the cell's own properties add up to.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CellStyle {
    /// Background (`#RRGGBB`); `None` = transparent.
    pub fill: Option<String>,
    /// Text colour the table style gives the cell (a run with a colour of its own keeps it).
    pub color: Option<String>,
    /// The table style sets the cell's text bold (header row, first column …).
    pub bold: bool,
    /// Borders left, right, top, bottom as `(colour, width in pt)`; `None` = no line.
    pub borders: [Option<(String, f32)>; 4],
    /// `top` | `middle` | `bottom`.
    pub valign: Option<String>,
}

impl Cell {
    pub fn text(&self) -> String {
        blocks_text(&self.blocks)
    }
}

/// Plain text of a block list (paragraph per line, table cells tab-separated).
pub fn blocks_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    for b in blocks {
        match b {
            Block::Paragraph(p) => {
                let t = p.text();
                if !t.trim().is_empty() {
                    out.push_str(t.trim_end());
                    out.push('\n');
                }
            }
            Block::Table(t) => {
                for row in &t.rows {
                    let cells: Vec<String> =
                        row.iter().map(|c| c.text().replace('\n', " ").trim().to_string()).collect();
                    out.push_str(&cells.join("\t"));
                    out.push('\n');
                }
            }
            Block::Image { .. } | Block::PageBreak => {}
        }
    }
    out
}

impl DocModel {
    pub fn text(&self) -> String {
        blocks_text(&self.blocks)
    }

    /// Rough page estimate (A4, ~45 lines) used only for progress / preview.
    pub fn estimated_pages(&self) -> u32 {
        let mut lines = 0usize;
        for b in &self.blocks {
            match b {
                Block::Paragraph(p) => lines += 1 + p.text().chars().count() / 60,
                Block::Table(t) => lines += t.rows.len() + 1,
                Block::Image { .. } => lines += 12,
                Block::PageBreak => lines += 45 - (lines % 45),
            }
        }
        (lines.div_ceil(45)).max(1) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_estimate() {
        let m = DocModel {
            blocks: vec![
                Block::Paragraph(Paragraph { heading: Some(1), ..Paragraph::plain("제목") }),
                Block::Paragraph(Paragraph::plain("본문 ")),
                Block::Table(Table {
                    rows: vec![vec![
                        Cell { blocks: vec![Block::Paragraph(Paragraph::plain("a"))], ..Default::default() },
                        Cell { blocks: vec![Block::Paragraph(Paragraph::plain("b\nc"))], ..Default::default() },
                    ]],
                    col_widths_mm: vec![],
                    row_heights_mm: Vec::new(),
                }),
                Block::PageBreak,
                Block::Paragraph(Paragraph::plain("둘째 쪽")),
            ],
            ..Default::default()
        };
        assert_eq!(m.text(), "제목\n본문\na\tb c\n둘째 쪽\n");
        assert_eq!(m.estimated_pages(), 2);
    }
}
