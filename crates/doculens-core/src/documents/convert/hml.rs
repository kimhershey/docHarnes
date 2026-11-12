//! HWPML (`.hml`, Hancom's XML dialect of HWP 5) through the shared XML parser: `BODY/SECTION/P`
//! paragraphs (`TEXT/CHAR` runs with `TAB` / `LINEBREAK`), `TABLE/ROW/CELL/PARALIST` tables with
//! spans, section ends as page breaks; pictures and shapes are noted. Paragraph shapes / styles are
//! not resolved, so headings are not recovered beyond the title heuristic.

use std::path::Path;

use crate::error::{CoreError, Result};

use super::model::{Block, Cell, DocModel, Paragraph, Run, Table};
use super::text::decode;
use super::xml::{self, Element};

fn paragraph(p: &Element, notes: &mut Vec<String>) -> (Paragraph, Vec<Table>) {
    let mut para = Paragraph::default();
    let mut tables = Vec::new();
    let mut current = String::new();
    for text in p.children_named("TEXT") {
        for node in text.elements() {
            match node.name.as_str() {
                "CHAR" => {
                    for child in &node.children {
                        match child {
                            xml::Node::Text(t) => current.push_str(t),
                            xml::Node::Element(e) if e.name == "TAB" => current.push('\t'),
                            xml::Node::Element(e) if e.name == "LINEBREAK" => {
                                para.runs.push(Run {
                                    text: std::mem::take(&mut current),
                                    line_break: true,
                                    ..Default::default()
                                });
                            }
                            xml::Node::Element(e) if e.name == "NBSPACE" => current.push('\u{a0}'),
                            xml::Node::Element(e) if e.name == "HYPEN" || e.name == "HYPHEN" => current.push('-'),
                            xml::Node::Element(_) => {}
                        }
                    }
                }
                "TABLE" => tables.push(table(node, notes)),
                "PICTURE" | "DRAWINGOBJECT" | "OLE" | "EQUATION" => {
                    notes.push(format!(
                        "{}은(는) 변환본에 포함되지 않습니다",
                        match node.name.as_str() {
                            "PICTURE" => "그림",
                            "EQUATION" => "수식",
                            "OLE" => "포함된 개체",
                            _ => "그리기 개체",
                        }
                    ));
                }
                _ => {}
            }
        }
    }
    if !current.is_empty() {
        para.runs.push(Run { text: current, ..Default::default() });
    }
    (para, tables)
}

fn table(tbl: &Element, notes: &mut Vec<String>) -> Table {
    let mut rows = Vec::new();
    for row in tbl.children_named("ROW") {
        let mut cells = Vec::new();
        for cell in row.children_named("CELL") {
            let mut blocks = Vec::new();
            if let Some(list) = cell.child("PARALIST") {
                for p in list.children_named("P") {
                    let (para, tables) = paragraph(p, notes);
                    if !para.is_empty() {
                        blocks.push(Block::Paragraph(para));
                    }
                    blocks.extend(tables.into_iter().map(Block::Table));
                }
            }
            cells.push(Cell {
                blocks,
                row_span: cell.attr_u32("RowSpan").unwrap_or(1).max(1).min(u16::MAX as u32) as u16,
                col_span: cell.attr_u32("ColSpan").unwrap_or(1).max(1).min(u16::MAX as u32) as u16,
                header: cell.attr_bool("Header").unwrap_or(false),
                style: None,
            });
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    Table { rows, col_widths_mm: Vec::new(), row_heights_mm: Vec::new() }
}

pub fn read(path: &Path) -> Result<DocModel> {
    let bytes = std::fs::read(path).map_err(|e| CoreError::io(path, e))?;
    let (text, _) = decode(&bytes);
    let root = xml::parse(text.as_bytes())?;
    if root.name != "HWPML" {
        return Err(CoreError::Convert("not an HWPML document (root is not <HWPML>)".into()));
    }
    let mut model = DocModel::default();
    let body = root.child("BODY").ok_or_else(|| CoreError::Convert("HWPML without <BODY>".into()))?;
    let mut first_section = true;
    for section in body.children_named("SECTION") {
        if !first_section {
            model.blocks.push(Block::PageBreak);
        }
        first_section = false;
        for p in section.children_named("P") {
            let (para, tables) = paragraph(p, &mut model.notes);
            if p.attr_bool("PageBreak").unwrap_or(false) && !model.blocks.is_empty() {
                model.blocks.push(Block::PageBreak);
            }
            if !para.is_empty() {
                model.blocks.push(Block::Paragraph(para));
            }
            model.blocks.extend(tables.into_iter().map(Block::Table));
        }
    }
    // Page size from the first section definition (HWPUNIT = 1/7200 inch).
    if let Some(def) = body.find("SECDEF").and_then(|s| s.child("PAGEDEF")) {
        if let (Some(w), Some(h)) = (def.attr_f32("Width"), def.attr_f32("Height")) {
            let landscape = def.attr("Landscape").is_some_and(|v| v != "0" && !v.eq_ignore_ascii_case("false"));
            let (w, h) = (w / 7200.0 * 25.4, h / 7200.0 * 25.4);
            model.page_mm = Some(if landscape { (h.max(w), w.min(h)) } else { (w, h) });
        }
    }
    model.title = root
        .child("HEAD")
        .and_then(|h| h.child("DOCSUMMARY"))
        .and_then(|s| s.child("TITLE"))
        .map(|t| t.text().trim().to_string())
        .filter(|t| !t.is_empty())
        .or_else(|| {
            model.blocks.iter().find_map(|b| match b {
                Block::Paragraph(p) => {
                    let t = p.text().trim().to_string();
                    (!t.is_empty() && t.chars().count() <= 80).then_some(t)
                }
                _ => None,
            })
        });
    model.notes.sort();
    model.notes.dedup();
    if model.blocks.is_empty() {
        model.notes.push("본문 텍스트가 없습니다".into());
    }
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?><HWPML Version="2.8"><HEAD><DOCSUMMARY><TITLE>휴가 신청서</TITLE></DOCSUMMARY></HEAD><BODY><SECTION Id="0"><P><TEXT><SECDEF><PAGEDEF Width="59528" Height="84188" Landscape="0"/></SECDEF><CHAR>휴가 신청서</CHAR></TEXT></P><P><TEXT><CHAR>성명: 홍길동<TAB/>부서: 경영기획팀<LINEBREAK/>기간: 2026-05-01 ~ 05-03</CHAR></TEXT></P><P><TEXT><TABLE><ROW><CELL ColSpan="2" Header="1"><PARALIST><P><TEXT><CHAR>결재</CHAR></TEXT></P></PARALIST></CELL></ROW><ROW><CELL><PARALIST><P><TEXT><CHAR>담당</CHAR></TEXT></P></PARALIST></CELL><CELL><PARALIST><P><TEXT><CHAR>팀장</CHAR></TEXT></P></PARALIST></CELL></ROW></TABLE></TEXT></P><P><TEXT><PICTURE/><CHAR>서명란</CHAR></TEXT></P></SECTION><SECTION Id="1"><P><TEXT><CHAR>둘째 구역</CHAR></TEXT></P></SECTION></BODY></HWPML>"#;

    #[test]
    fn reads_paragraphs_tables_sections_and_page_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("form.hml");
        std::fs::write(&p, SAMPLE).unwrap();
        let model = read(&p).unwrap();
        assert_eq!(model.title.as_deref(), Some("휴가 신청서"));
        let paras: Vec<String> = model
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(p) => Some(p.text()),
                _ => None,
            })
            .collect();
        assert_eq!(paras[0], "휴가 신청서");
        assert_eq!(paras[1], "성명: 홍길동\t부서: 경영기획팀기간: 2026-05-01 ~ 05-03");
        let table = model
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert_eq!(table.rows[0][0].col_span, 2);
        assert!(table.rows[0][0].header);
        assert_eq!(table.rows[1][1].text().trim(), "팀장");
        assert_eq!(model.blocks.iter().filter(|b| matches!(b, Block::PageBreak)).count(), 1, "second section");
        assert!(model.notes.iter().any(|n| n.contains("그림")));
        let (w, h) = model.page_mm.unwrap();
        assert!((w - 210.0).abs() < 0.5 && (h - 297.0).abs() < 0.5, "{w} × {h}");
        assert!(model.text().contains("둘째 구역"));
        // UTF-16 input is decoded too.
        let utf16: Vec<u8> =
            [0xFF, 0xFE].into_iter().chain(SAMPLE.encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        std::fs::write(&p, utf16).unwrap();
        assert_eq!(read(&p).unwrap().title.as_deref(), Some("휴가 신청서"));
        std::fs::write(&p, "<html><body>x</body></html>").unwrap();
        assert!(matches!(read(&p), Err(CoreError::Convert(_))));
    }
}
