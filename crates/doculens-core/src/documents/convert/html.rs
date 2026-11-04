//! `DocModel` → self-contained HTML bundle (`index.html` + `img/`) that the
//! shell prints to PDF through the platform webview (WebView2 `PrintToPdf`,
//! WebKitGTK print operation). The page geometry mirrors an A4 sheet so the
//! rendered pages match the PDF pipeline (design §3.4).

use std::fmt::Write as _;
use std::path::Path;

use crate::error::{CoreError, Result};

use super::model::{Align, Block, Cell, DocModel, Paragraph, Run, Table};

/// Escapes text for element content / attribute values.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

const CSS: &str = r#"
@page { size: A4; margin: 0; }
html, body { margin: 0; padding: 0; background: #fff; color: #111; }
body { font-family: "Pretendard Variable", Pretendard, "Noto Sans KR", "Malgun Gothic", "Apple SD Gothic Neo", "Nanum Gothic", sans-serif;
       font-size: 10.5pt; line-height: 1.55; word-break: keep-all; overflow-wrap: anywhere; }
/* Margins live in the content (not @page) so WebView2 and WebKitGTK produce the same sheet. */
article { padding: 18mm 16mm 20mm; }
p { margin: 0 0 0.45em; min-height: 1em; }
h1, h2, h3, h4, h5, h6 { margin: 0.9em 0 0.4em; line-height: 1.3; page-break-after: avoid; }
h1 { font-size: 18pt; } h2 { font-size: 15pt; } h3 { font-size: 13pt; } h4, h5, h6 { font-size: 11.5pt; }
table { border-collapse: collapse; margin: 0.4em 0 0.8em; width: 100%; table-layout: fixed; page-break-inside: auto; }
tr { page-break-inside: avoid; }
td, th { border: 0.5pt solid #444; padding: 2pt 4pt; vertical-align: top; font-size: 9.5pt; }
th { background: #f0f0f0; font-weight: 600; }
td p, th p { margin: 0; }
img.doc-image { max-width: 100%; height: auto; display: block; margin: 0.5em auto; page-break-inside: avoid; }
.page-break { page-break-before: always; break-before: page; height: 0; }
.al-center { text-align: center; } .al-right { text-align: right; } .al-justify { text-align: justify; }
ul, ol { margin: 0 0 0.45em; padding-left: 1.6em; }
"#;

fn run_html(r: &Run, out: &mut String) {
    let mut text = escape(&r.text);
    if text.is_empty() && !r.line_break {
        return;
    }
    // Preserve consecutive spaces (form-like layouts rely on them).
    if text.contains("  ") {
        text = text.replace("  ", " &nbsp;");
    }
    let mut open = String::new();
    let mut close = String::new();
    if r.bold {
        open.push_str("<b>");
        close.insert_str(0, "</b>");
    }
    if r.italic {
        open.push_str("<i>");
        close.insert_str(0, "</i>");
    }
    if r.underline {
        open.push_str("<u>");
        close.insert_str(0, "</u>");
    }
    if let Some(pt) = r.size_pt.filter(|p| *p > 0.0 && (*p - 10.5).abs() > 0.6) {
        let _ = write!(open, "<span style=\"font-size:{:.1}pt\">", pt.clamp(5.0, 72.0));
        close.insert_str(0, "</span>");
    }
    out.push_str(&open);
    out.push_str(&text);
    out.push_str(&close);
    if r.line_break {
        out.push_str("<br>");
    }
}

fn align_class(a: Align) -> &'static str {
    match a {
        Align::Left => "",
        Align::Center => " class=\"al-center\"",
        Align::Right => " class=\"al-right\"",
        Align::Justify => " class=\"al-justify\"",
    }
}

fn paragraph_html(p: &Paragraph, out: &mut String) {
    let tag = match p.heading {
        Some(l) => format!("h{}", l.clamp(1, 6)),
        None => "p".to_string(),
    };
    let _ = write!(out, "<{tag}{}>", align_class(p.align));
    if p.runs.iter().all(|r| r.text.is_empty() && !r.line_break) {
        out.push_str("&nbsp;");
    }
    for r in &p.runs {
        run_html(r, out);
    }
    let _ = writeln!(out, "</{tag}>");
}

fn cell_html(c: &Cell, out: &mut String) {
    let tag = if c.header { "th" } else { "td" };
    out.push('<');
    out.push_str(tag);
    if c.row_span > 1 {
        let _ = write!(out, " rowspan=\"{}\"", c.row_span);
    }
    if c.col_span > 1 {
        let _ = write!(out, " colspan=\"{}\"", c.col_span);
    }
    out.push('>');
    if c.blocks.is_empty() {
        out.push_str("&nbsp;");
    }
    blocks_html(&c.blocks, out);
    let _ = write!(out, "</{tag}>");
}

fn table_html(t: &Table, out: &mut String) {
    out.push_str("<table>");
    if !t.col_widths_mm.is_empty() {
        let total: f32 = t.col_widths_mm.iter().sum();
        if total > 0.0 {
            out.push_str("<colgroup>");
            for w in &t.col_widths_mm {
                let _ = write!(out, "<col style=\"width:{:.2}%\">", w / total * 100.0);
            }
            out.push_str("</colgroup>");
        }
    }
    for row in &t.rows {
        out.push_str("<tr>");
        for c in row {
            cell_html(c, out);
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
}

fn blocks_html(blocks: &[Block], out: &mut String) {
    let mut list_open: Option<&'static str> = None;
    for b in blocks {
        let list_tag = match b {
            Block::Paragraph(p) => p.list.map(|k| match k {
                super::model::ListKind::Bullet => "ul",
                super::model::ListKind::Number => "ol",
            }),
            _ => None,
        };
        if list_open != list_tag {
            if let Some(t) = list_open {
                let _ = writeln!(out, "</{t}>");
            }
            if let Some(t) = list_tag {
                let _ = writeln!(out, "<{t}>");
            }
            list_open = list_tag;
        }
        match b {
            Block::Paragraph(p) => {
                if list_tag.is_some() {
                    out.push_str("<li>");
                    paragraph_html(p, out);
                    out.push_str("</li>");
                } else {
                    paragraph_html(p, out);
                }
            }
            Block::Table(t) => table_html(t, out),
            Block::Image { id, width_mm, .. } => {
                let style = width_mm.filter(|w| *w > 0.0).map(|w| format!(" style=\"width:{:.1}mm\"", w.min(178.0)));
                let _ = writeln!(
                    out,
                    "<img class=\"doc-image\" src=\"img/{}\" alt=\"\"{}>",
                    escape(id),
                    style.unwrap_or_default()
                );
            }
            Block::PageBreak => out.push_str("<div class=\"page-break\"></div>\n"),
        }
    }
    if let Some(t) = list_open {
        let _ = writeln!(out, "</{t}>");
    }
}

/// Renders the model as a complete HTML document. Image blocks reference
/// `img/<id>` relative files; see [`write_bundle`].
pub fn to_html(model: &DocModel) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str("<!doctype html>\n<html lang=\"ko\"><head><meta charset=\"utf-8\">\n<title>");
    out.push_str(&escape(model.title.as_deref().unwrap_or("document")));
    out.push_str("</title>\n<meta name=\"viewport\" content=\"width=device-width\">\n<style>");
    out.push_str(CSS);
    out.push_str("</style></head>\n<body><article>\n");
    blocks_html(&model.blocks, &mut out);
    out.push_str("</article></body></html>\n");
    out
}

/// File name (inside `img/`) for an embedded image id.
pub fn image_file_name(id: &str, mime: &str) -> String {
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => "bin",
    };
    let safe: String =
        id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{safe}.{ext}")
}

/// Writes `index.html` and `img/*` into `dir`.
pub fn write_bundle(model: &DocModel, dir: &Path) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir).map_err(|e| CoreError::io(dir, e))?;
    let img_dir = dir.join("img");
    if !model.images.is_empty() {
        std::fs::create_dir_all(&img_dir).map_err(|e| CoreError::io(&img_dir, e))?;
    }
    // Image blocks reference the id; map ids to the file name with extension.
    let mut model_for_html = model.clone();
    for img in &model.images {
        let file = image_file_name(&img.id, &img.mime);
        let path = img_dir.join(&file);
        std::fs::write(&path, &img.bytes).map_err(|e| CoreError::io(&path, e))?;
        rename_image_refs(&mut model_for_html.blocks, &img.id, &file);
    }
    let html = to_html(&model_for_html);
    let index = dir.join("index.html");
    std::fs::write(&index, html).map_err(|e| CoreError::io(&index, e))?;
    Ok(index)
}

fn rename_image_refs(blocks: &mut [Block], id: &str, file: &str) {
    for b in blocks {
        match b {
            Block::Image { id: bid, .. } if bid == id => *bid = file.to_string(),
            Block::Table(t) => {
                for row in &mut t.rows {
                    for c in row {
                        rename_image_refs(&mut c.blocks, id, file);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::{ImageData, ListKind};
    use super::*;

    #[test]
    fn emits_headings_tables_breaks_and_images() {
        let model = DocModel {
            title: Some("견적서 <v1>".into()),
            blocks: vec![
                Block::Paragraph(Paragraph { heading: Some(1), align: Align::Center, ..Paragraph::plain("견적서") }),
                Block::Paragraph(Paragraph {
                    runs: vec![
                        Run { text: "굵게".into(), bold: true, ..Default::default() },
                        Run { text: " 작게".into(), size_pt: Some(8.0), line_break: true, ..Default::default() },
                        Run { text: "a  b".into(), ..Default::default() },
                    ],
                    ..Default::default()
                }),
                Block::Paragraph(Paragraph { list: Some(ListKind::Bullet), ..Paragraph::plain("항목") }),
                Block::Table(Table {
                    rows: vec![
                        vec![Cell {
                            blocks: vec![Block::Paragraph(Paragraph::plain("품목"))],
                            header: true,
                            col_span: 2,
                            ..Default::default()
                        }],
                        vec![
                            Cell {
                                blocks: vec![Block::Paragraph(Paragraph::plain("A"))],
                                row_span: 2,
                                ..Default::default()
                            },
                            Cell { blocks: vec![], ..Default::default() },
                        ],
                    ],
                    col_widths_mm: vec![50.0, 50.0],
                }),
                Block::PageBreak,
                Block::Image { id: "img1".into(), width_mm: Some(300.0), height_mm: None },
            ],
            images: vec![ImageData { id: "img1".into(), mime: "image/png".into(), bytes: vec![1, 2, 3] }],
            ..Default::default()
        };
        let html = to_html(&model);
        assert!(html.contains("<title>견적서 &lt;v1&gt;</title>"));
        assert!(html.contains("<h1 class=\"al-center\">견적서</h1>"));
        assert!(html.contains("<b>굵게</b>"));
        assert!(html.contains("font-size:8.0pt"));
        assert!(html.contains("<br>"));
        assert!(html.contains("a &nbsp;b"));
        assert!(html.contains("<ul>\n<li><p>항목</p>\n</li></ul>"));
        assert!(html.contains("<th colspan=\"2\">"));
        assert!(html.contains("<td rowspan=\"2\">"));
        assert!(html.contains("<td>&nbsp;</td>"));
        assert!(html.contains("<col style=\"width:50.00%\">"));
        assert!(html.contains("class=\"page-break\""));
        assert!(html.contains("width:178.0mm"));

        let dir = tempfile::tempdir().unwrap();
        let index = write_bundle(&model, dir.path()).unwrap();
        let written = std::fs::read_to_string(index).unwrap();
        assert!(written.contains("src=\"img/img1.png\""));
        assert_eq!(std::fs::read(dir.path().join("img").join("img1.png")).unwrap(), vec![1, 2, 3]);
    }
}
