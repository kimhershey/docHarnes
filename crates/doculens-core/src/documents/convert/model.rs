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
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Table {
    pub rows: Vec<Vec<Cell>>,
    /// Column widths in millimetres when known (same length as the widest row).
    pub col_widths_mm: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Cell {
    pub blocks: Vec<Block>,
    pub row_span: u16,
    pub col_span: u16,
    pub header: bool,
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
