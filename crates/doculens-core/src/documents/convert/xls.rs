//! Legacy / other spreadsheet workbooks through `calamine` (MIT): XLS (BIFF 5/8), XLSB and ODS.
//! Each visible sheet becomes a heading + table, like the own XLSX reader; typed dates / durations
//! are formatted, numbers keep their stored text (trailing `.0` dropped), formulas yield their cached
//! value. Merged regions are not reported for these formats, so cells are emitted as they are.

use std::path::Path;

use calamine::{open_workbook_auto, Data, Reader, SheetVisible};

use crate::error::{CoreError, Result};

use super::model::{Block, Cell, DocModel, Paragraph, Table};
use super::xlsx::number_text;
use super::xml::{MAX_TABLE_COLS, MAX_TABLE_ROWS};

fn cell_text(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        Data::String(s) => s.trim_end().to_string(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => number_text(&format!("{f}")),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Data::DateTime(dt) => {
            if dt.is_duration() {
                match dt.as_duration() {
                    Some(d) => {
                        let secs = d.num_seconds();
                        format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
                    }
                    None => format!("{}", dt.as_f64()),
                }
            } else {
                match dt.as_datetime() {
                    Some(t) if t.time() == chrono::NaiveTime::MIN => t.format("%Y-%m-%d").to_string(),
                    Some(t) => t.format("%Y-%m-%d %H:%M").to_string(),
                    None => format!("{}", dt.as_f64()),
                }
            }
        }
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#{e:?}").to_ascii_uppercase(),
    }
}

pub fn read(path: &Path) -> Result<DocModel> {
    let mut wb = open_workbook_auto(path).map_err(|e| CoreError::Convert(format!("workbook: {e}")))?;
    let sheets: Vec<(String, bool)> =
        wb.sheets_metadata().iter().map(|s| (s.name.clone(), s.visible == SheetVisible::Visible)).collect();
    let mut model = DocModel::default();
    let mut first = true;
    for (name, visible) in sheets {
        if !visible {
            model.notes.push(format!("숨김 시트 생략: {name}"));
            continue;
        }
        let range = match wb.worksheet_range(&name) {
            Ok(r) => r,
            Err(e) => {
                model.notes.push(format!("시트를 읽을 수 없습니다: {name} ({e})"));
                continue;
            }
        };
        if range.is_empty() {
            continue;
        }
        let (height, width) = range.get_size();
        if height > MAX_TABLE_ROWS || width > MAX_TABLE_COLS {
            model.notes.push(format!(
                "시트 {name}: {height}행 × {width}열 중 {MAX_TABLE_ROWS}행 × {MAX_TABLE_COLS}열까지만 읽었습니다"
            ));
        }
        let mut rows: Vec<Vec<Cell>> = Vec::new();
        for row in range.rows().take(MAX_TABLE_ROWS) {
            let mut cells: Vec<Cell> = row
                .iter()
                .take(MAX_TABLE_COLS)
                .map(|d| {
                    let text = cell_text(d);
                    Cell {
                        blocks: if text.is_empty() {
                            Vec::new()
                        } else {
                            vec![Block::Paragraph(Paragraph::plain(text))]
                        },
                        row_span: 1,
                        col_span: 1,
                        header: false,
                        style: None,
                    }
                })
                .collect();
            while cells.last().is_some_and(|c| c.blocks.is_empty()) {
                cells.pop();
            }
            rows.push(cells);
        }
        while rows.last().is_some_and(|r| r.is_empty()) {
            rows.pop();
        }
        if rows.iter().all(|r| r.is_empty()) {
            continue;
        }
        // Rows end at different columns; pad to the widest so the HTML table keeps its grid.
        let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        for r in &mut rows {
            while r.len() < cols {
                r.push(Cell { row_span: 1, col_span: 1, ..Default::default() });
            }
        }
        if !first {
            model.blocks.push(Block::PageBreak);
        }
        first = false;
        if model.title.is_none() {
            // A lone cell in the first row reads as the sheet's title (merged title rows).
            if let Some(first_row) = rows.first() {
                let texts: Vec<String> =
                    first_row.iter().map(|c| c.text().trim().to_string()).filter(|t| !t.is_empty()).collect();
                if texts.len() == 1 && texts[0].chars().count() <= 120 {
                    model.title = Some(texts[0].clone());
                }
            }
        }
        model.blocks.push(Block::Paragraph(Paragraph { heading: Some(2), ..Paragraph::plain(name.clone()) }));
        model.blocks.push(Block::Table(Table { rows, col_widths_mm: Vec::new(), row_heights_mm: Vec::new() }));
    }
    if model.blocks.is_empty() {
        model.notes.push("내용이 있는 시트가 없습니다".into());
    }
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy/sample.xls")
    }

    #[test]
    fn reads_a_biff8_workbook() {
        let model = read(&fixture()).unwrap();
        assert_eq!(model.title.as_deref(), Some("2026년 1분기 매출 현황"), "merged title row");
        let tables: Vec<&Table> = model
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(tables.len(), 2, "two sheets");
        assert_eq!(model.blocks.iter().filter(|b| matches!(b, Block::PageBreak)).count(), 1);
        let sales = tables[0];
        let header: Vec<String> = sales.rows[1].iter().map(|c| c.text().trim().to_string()).collect();
        assert_eq!(header, ["거래처", "품목", "수량", "금액"]);
        assert_eq!(sales.rows[2][0].text().trim(), "㈜한빛");
        assert_eq!(sales.rows[2][2].text().trim(), "10", "integers lose the .0");
        assert_eq!(sales.rows[2][3].text().trim(), "12000000");
        assert_eq!(sales.rows[6][1].text().trim(), "2026-04-02", "date-formatted cell");
        assert_eq!(tables[1].rows[0][0].text().trim(), "환율 가정 1,350원");
        assert_eq!(tables[1].rows[1][0].text().trim(), "3.5");
        let text = model.text();
        assert!(text.contains("대성상사\t모니터\t25\t7500000"), "{text}");
        // Not a workbook → conversion error, not a panic.
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("x.xls");
        std::fs::write(&bad, b"not a workbook").unwrap();
        assert!(matches!(read(&bad), Err(CoreError::Convert(_))));
    }
}
