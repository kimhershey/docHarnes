//! `DocModel` → self-contained HTML bundle (`index.html` + `img/`) that the
//! shell prints to PDF through the platform webview (WebView2 `PrintToPdf`,
//! WebKitGTK print operation). Flow documents use the source page size (A4 when
//! unknown); slide decks (`DocModel::slides`) are printed one slide per page at
//! the slide's own size with absolutely positioned shapes (design §3.4).

use std::fmt::Write as _;
use std::path::Path;

use crate::error::{CoreError, Result};

use super::model::{Align, Block, Cell, DocModel, ListKind, Paragraph, Run, Shape, ShapeKind, Slide, Table, VAnchor};

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
table.wide td, table.wide th { font-size: 7pt; padding: 1pt 2pt; }
table.wider td, table.wider th { font-size: 5.5pt; padding: 0.5pt 1pt; }
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
    let size = r
        .size_pt
        .filter(|p| *p > 0.0 && (*p - 10.5).abs() > 0.6)
        .map(|pt| format!("font-size:{:.1}pt;", pt.clamp(5.0, 72.0)));
    let color = r.color.as_deref().filter(|c| is_css_color(c)).map(|c| format!("color:{c};"));
    if size.is_some() || color.is_some() {
        let _ = write!(open, "<span style=\"{}{}\">", size.unwrap_or_default(), color.unwrap_or_default());
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
    // Spreadsheets: many columns shrink the type so the sheet still fits an A4 page.
    let cols = t.rows.iter().map(|r| r.iter().map(|c| c.col_span.max(1) as usize).sum::<usize>()).max().unwrap_or(0);
    out.push_str(if cols > 16 {
        "<table class=\"wider\">"
    } else if cols > 8 {
        "<table class=\"wide\">"
    } else {
        "<table>"
    });
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

/// `#rrggbb` only (anything else is dropped rather than injected into the style attribute).
fn is_css_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit())
}

/// The sheet the HTML is laid out for: the source page size when it declares one, else A4.
pub fn page_size_mm(model: &DocModel) -> (f32, f32) {
    match model.page_mm {
        Some((w, h)) if (50.0..=2000.0).contains(&w) && (50.0..=2000.0).contains(&h) => (w, h),
        _ => (210.0, 297.0),
    }
}

/// Renders the model as a complete HTML document. Image blocks reference
/// `img/<id>` relative files; see [`write_bundle`].
pub fn to_html(model: &DocModel) -> String {
    if model.slides.is_empty() {
        render_html(model.title.as_deref(), &model.blocks, page_size_mm(model))
    } else {
        render_slides_html(model.title.as_deref(), &model.slides, |id| format!("img/{}", escape(id)))
    }
}

fn head(out: &mut String, title: Option<&str>, (w, h): (f32, f32)) {
    out.push_str("<!doctype html>\n<html lang=\"ko\"><head><meta charset=\"utf-8\">\n<title>");
    out.push_str(&escape(title.unwrap_or("document")));
    out.push_str("</title>\n<meta name=\"viewport\" content=\"width=device-width\">\n<style>");
    let _ = write!(out, "@page {{ size: {w:.2}mm {h:.2}mm; margin: 0; }}");
    out.push_str(CSS);
}

fn render_html(title: Option<&str>, blocks: &[Block], page: (f32, f32)) -> String {
    let mut out = String::with_capacity(4096);
    head(&mut out, title, page);
    out.push_str("</style></head>\n<body><article>\n");
    blocks_html(blocks, &mut out);
    out.push_str("</article></body></html>\n");
    out
}

// ---------------------------------------------------------------------------
// Slides (one page per slide, absolutely positioned shapes)
// ---------------------------------------------------------------------------

const SLIDE_CSS: &str = r#"
html, body { margin: 0; padding: 0; background: #fff; color: #111; }
body { font-family: "Pretendard Variable", Pretendard, "Noto Sans KR", "Malgun Gothic", "Apple SD Gothic Neo", "Nanum Gothic", sans-serif; }
.slide { position: relative; overflow: hidden; background: #fff; page-break-after: always; break-after: page; }
.slide:last-child { page-break-after: auto; break-after: auto; }
.shape { position: absolute; box-sizing: border-box; overflow: hidden; }
.shape .tx { position: absolute; left: 0; right: 0; display: flex; flex-direction: column; box-sizing: border-box; }
.tx.top { top: 0; } .tx.middle { top: 0; bottom: 0; justify-content: center; } .tx.bottom { bottom: 0; }
.shape p { margin: 0; line-height: 1.2; word-break: keep-all; overflow-wrap: anywhere; }
.shape ul, .shape ol { margin: 0; padding-left: 1.1em; }
.shape li p { display: inline; }
.shape img { display: block; width: 100%; height: 100%; object-fit: fill; }
.shape table { width: 100%; height: 100%; border-collapse: collapse; table-layout: fixed; }
.shape td, .shape th { border: 0.5pt solid #777; padding: 1mm 1.5mm; vertical-align: middle; }
.shape th { background: #e8e8e8; font-weight: 600; }
.ph { border: 0.6pt dashed #999; color: #777; display: flex; align-items: center; justify-content: center; text-align: center; font-size: 10pt; }
.line { border: 0; }
.notes { padding: 14mm 18mm; font-size: 12pt; line-height: 1.5; }
.notes h4 { margin: 0 0 4mm; font-size: 12pt; color: #555; }
.notes p { margin: 0 0 0.5em; }
.al-center { text-align: center; } .al-right { text-align: right; } .al-justify { text-align: justify; }
"#;

fn mm(v: f32) -> String {
    format!("{:.2}mm", v)
}

/// Paragraphs of a slide text box: run sizes scaled by `font_scale`, unsized runs at `default_pt`
/// (one step smaller per indent level), bullet lists kept, colours from the runs or the shape default.
fn slide_paragraphs_html(
    paras: &[Paragraph],
    default_pt: f32,
    font_scale: f32,
    default_color: Option<&str>,
    out: &mut String,
) {
    let mut list_open: Option<&'static str> = None;
    for p in paras {
        let list_tag = p.list.map(|k| match k {
            ListKind::Bullet => "ul",
            ListKind::Number => "ol",
        });
        if list_open != list_tag {
            if let Some(t) = list_open {
                let _ = writeln!(out, "</{t}>");
            }
            if let Some(t) = list_tag {
                let _ = writeln!(out, "<{t}>");
            }
            list_open = list_tag;
        }
        let level_pt = default_pt * (1.0 - 0.1 * p.level.min(4) as f32);
        let base_pt = (p.runs.iter().find_map(|r| r.size_pt).unwrap_or(level_pt) * font_scale).clamp(4.0, 200.0);
        let indent = if list_tag.is_none() && p.level > 0 {
            format!("margin-left:{:.1}em;", 1.2 * p.level as f32)
        } else {
            String::new()
        };
        let color = default_color.filter(|c| is_css_color(c)).map(|c| format!("color:{c};")).unwrap_or_default();
        if list_tag.is_some() {
            let _ =
                write!(out, "<li style=\"font-size:{base_pt:.1}pt;{color}margin-left:{:.1}em\">", 1.2 * p.level as f32);
        }
        let _ = write!(out, "<p{} style=\"font-size:{base_pt:.1}pt;{color}{indent}\">", align_class(p.align));
        if p.runs.iter().all(|r| r.text.is_empty() && !r.line_break) {
            out.push_str("&nbsp;");
        }
        for r in &p.runs {
            let mut scaled = r.clone();
            scaled.size_pt =
                r.size_pt.map(|s| (s * font_scale).clamp(4.0, 200.0)).filter(|s| (*s - base_pt).abs() > 0.05);
            run_html(&scaled, out);
        }
        out.push_str("</p>");
        if list_tag.is_some() {
            out.push_str("</li>");
        }
        out.push('\n');
    }
    if let Some(t) = list_open {
        let _ = writeln!(out, "</{t}>");
    }
}

fn shape_html(sh: &Shape, img_src: &dyn Fn(&str) -> String, out: &mut String) {
    let mut style = format!("left:{};top:{};width:{};height:{};", mm(sh.x_mm), mm(sh.y_mm), mm(sh.w_mm), mm(sh.h_mm));
    if let Some(f) = sh.fill.as_deref().filter(|c| is_css_color(c)) {
        let _ = write!(style, "background:{f};");
    }
    if let Some((c, w)) = &sh.line {
        if is_css_color(c) && !matches!(sh.kind, ShapeKind::Line) {
            let _ = write!(style, "border:{:.2}pt solid {c};", w.max(0.25));
        }
    }
    match sh.geometry.as_deref() {
        Some("ellipse") => style.push_str("border-radius:50%;"),
        Some("roundRect") | Some("round2SameRect") | Some("snipRoundRect") => style.push_str("border-radius:2.5mm;"),
        _ => {}
    }
    let mut transform = String::new();
    if sh.rotation_deg.abs() > 0.01 {
        let _ = write!(transform, "rotate({:.2}deg) ", sh.rotation_deg);
    }
    if sh.flip_h {
        transform.push_str("scaleX(-1) ");
    }
    if sh.flip_v {
        transform.push_str("scaleY(-1) ");
    }
    if !transform.is_empty() {
        let _ = write!(style, "transform:{};transform-origin:center;", transform.trim_end());
    }
    match &sh.kind {
        ShapeKind::Text { paragraphs, anchor, font_scale, insets_mm, default_pt, default_color } => {
            let _ = write!(out, "<div class=\"shape\" style=\"{style}\">");
            let anchor_class = match anchor {
                VAnchor::Top => "top",
                VAnchor::Middle => "middle",
                VAnchor::Bottom => "bottom",
            };
            let _ = write!(
                out,
                "<div class=\"tx {anchor_class}\" style=\"padding:{} {} {} {}\">",
                mm(insets_mm[1]),
                mm(insets_mm[2]),
                mm(insets_mm[3]),
                mm(insets_mm[0])
            );
            slide_paragraphs_html(paragraphs, *default_pt, *font_scale, default_color.as_deref(), out);
            out.push_str("</div></div>\n");
        }
        ShapeKind::Picture { id } => {
            let _ =
                writeln!(out, "<div class=\"shape\" style=\"{style}\"><img src=\"{}\" alt=\"\"></div>", img_src(id));
        }
        ShapeKind::Table(t) => {
            let _ = write!(out, "<div class=\"shape\" style=\"{style}\">");
            slide_table_html(t, out);
            out.push_str("</div>\n");
        }
        ShapeKind::Placeholder { label } => {
            let _ = writeln!(out, "<div class=\"shape ph\" style=\"{style}\">{}</div>", escape(label));
        }
        ShapeKind::Line => {
            let (c, w) = sh.line.clone().unwrap_or(("#000000".into(), 0.75));
            let c = if is_css_color(&c) { c } else { "#000000".into() };
            // A thin box gets one border along its long side; a diagonal keeps the top edge (approximation).
            let edge = if sh.h_mm <= sh.w_mm { "border-top" } else { "border-left" };
            let _ = writeln!(
                out,
                "<div class=\"shape line\" style=\"{style}{edge}:{:.2}pt solid {c};\"></div>",
                w.max(0.25)
            );
        }
    }
}

fn slide_table_html(t: &Table, out: &mut String) {
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
            let paras: Vec<Paragraph> = c
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Paragraph(p) => Some(p.clone()),
                    _ => None,
                })
                .collect();
            if paras.is_empty() {
                out.push_str("&nbsp;");
            } else {
                slide_paragraphs_html(&paras, 14.0, 1.0, None, out);
            }
            let _ = write!(out, "</{tag}>");
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
}

/// Slide deck → HTML: `@page` at the slide size, one `<section class="slide">` per slide, a notes
/// page after every slide that has speaker notes.
pub fn render_slides_html(title: Option<&str>, slides: &[Slide], img_src: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(8192);
    let (w, h) = slides.first().map(|s| (s.width_mm, s.height_mm)).unwrap_or((254.0, 190.5));
    out.push_str("<!doctype html>\n<html lang=\"ko\"><head><meta charset=\"utf-8\">\n<title>");
    out.push_str(&escape(title.unwrap_or("slides")));
    out.push_str("</title>\n<meta name=\"viewport\" content=\"width=device-width\">\n<style>");
    let _ = write!(out, "@page {{ size: {w:.2}mm {h:.2}mm; margin: 0; }}");
    out.push_str(SLIDE_CSS);
    let _ = write!(out, ".slide {{ width: {}; height: {}; }}", mm(w), mm(h));
    out.push_str("</style></head>\n<body>\n");
    for (i, slide) in slides.iter().enumerate() {
        let bg = slide
            .background
            .as_deref()
            .filter(|c| is_css_color(c))
            .map(|c| format!(" style=\"background:{c}\""))
            .unwrap_or_default();
        let _ = writeln!(out, "<section class=\"slide\" data-slide=\"{}\"{bg}>", i + 1);
        for sh in &slide.shapes {
            shape_html(sh, &img_src, &mut out);
        }
        out.push_str("</section>\n");
        if !slide.notes.is_empty() {
            let _ = writeln!(
                out,
                "<section class=\"slide notes\" data-notes-for=\"{}\"><h4>발표자 노트 — 슬라이드 {}</h4>",
                i + 1,
                i + 1
            );
            for p in &slide.notes {
                paragraph_html(p, &mut out);
            }
            out.push_str("</section>\n");
        }
    }
    out.push_str("</body></html>\n");
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
    // Only the block tree is copied (not the image bytes).
    let mut blocks = model.blocks.clone();
    let mut files: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for img in &model.images {
        let file = image_file_name(&img.id, &img.mime);
        let path = img_dir.join(&file);
        std::fs::write(&path, &img.bytes).map_err(|e| CoreError::io(&path, e))?;
        rename_image_refs(&mut blocks, &img.id, &file);
        files.insert(img.id.clone(), file);
    }
    let html = if model.slides.is_empty() {
        render_html(model.title.as_deref(), &blocks, page_size_mm(model))
    } else {
        render_slides_html(model.title.as_deref(), &model.slides, |id| {
            format!("img/{}", escape(files.get(id).map(String::as_str).unwrap_or(id)))
        })
    };
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
    use super::super::model::{ImageData, Shape, ShapeKind, Slide, VAnchor};

    #[test]
    fn slides_render_one_page_per_slide_at_the_slide_size() {
        let text = |t: &str| Paragraph::plain(t);
        let slide = Slide {
            width_mm: 338.67,
            height_mm: 190.5,
            background: Some("#102040".into()),
            shapes: vec![
                Shape {
                    x_mm: 10.0,
                    y_mm: 20.0,
                    w_mm: 100.0,
                    h_mm: 30.0,
                    rotation_deg: 0.0,
                    flip_h: false,
                    flip_v: false,
                    fill: Some("#2F5597".into()),
                    line: Some(("#1F3864".into(), 1.5)),
                    geometry: Some("roundRect".into()),
                    kind: ShapeKind::Text {
                        paragraphs: vec![
                            text("제목"),
                            Paragraph { level: 1, list: Some(ListKind::Bullet), ..text("항목") },
                        ],
                        anchor: VAnchor::Middle,
                        font_scale: 0.5,
                        insets_mm: [2.54, 1.27, 2.54, 1.27],
                        default_pt: 40.0,
                        default_color: Some("#FFFFFF".into()),
                    },
                },
                Shape {
                    x_mm: 200.0,
                    y_mm: 50.0,
                    w_mm: 60.0,
                    h_mm: 40.0,
                    rotation_deg: 90.0,
                    flip_h: true,
                    flip_v: false,
                    fill: None,
                    line: None,
                    geometry: None,
                    kind: ShapeKind::Picture { id: "img1".into() },
                },
                Shape {
                    x_mm: 10.0,
                    y_mm: 150.0,
                    w_mm: 300.0,
                    h_mm: 0.1,
                    rotation_deg: 0.0,
                    flip_h: false,
                    flip_v: false,
                    fill: None,
                    line: Some(("#7F7F7F".into(), 1.0)),
                    geometry: Some("line".into()),
                    kind: ShapeKind::Line,
                },
                Shape {
                    x_mm: 10.0,
                    y_mm: 100.0,
                    w_mm: 80.0,
                    h_mm: 40.0,
                    rotation_deg: 0.0,
                    flip_h: false,
                    flip_v: false,
                    fill: None,
                    line: None,
                    geometry: None,
                    kind: ShapeKind::Placeholder { label: "차트".into() },
                },
            ],
            notes: vec![text("발표 메모")],
        };
        let model = DocModel {
            title: Some("덱".into()),
            slides: vec![slide.clone(), Slide { notes: vec![], ..slide }],
            images: vec![ImageData { id: "img1".into(), mime: "image/png".into(), bytes: vec![1, 2, 3] }],
            ..Default::default()
        };
        let html = to_html(&model);
        assert!(html.contains("@page { size: 338.67mm 190.50mm; margin: 0; }"), "{html}");
        assert_eq!(html.matches("<section class=\"slide\"").count(), 2);
        assert_eq!(html.matches("class=\"slide notes\"").count(), 1, "only the first slide has notes");
        assert!(html.contains("background:#102040"));
        assert!(html.contains("left:10.00mm;top:20.00mm;width:100.00mm;height:30.00mm;background:#2F5597;border:1.50pt solid #1F3864;border-radius:2.5mm;"));
        assert!(html.contains("class=\"tx middle\""));
        // 40 pt default × 0.5 autofit = 20 pt; the level-1 bullet is one step smaller (36 × 0.5).
        assert!(html.contains("font-size:20.0pt;color:#FFFFFF;"), "{html}");
        assert!(html.contains("<li style=\"font-size:18.0pt;color:#FFFFFF;margin-left:1.2em\">"), "{html}");
        assert!(html.contains("transform:rotate(90.00deg) scaleX(-1);transform-origin:center;"));
        assert!(html.contains("<img src=\"img/img1\" alt=\"\">"));
        assert!(html.contains("border-top:1.00pt solid #7F7F7F;"));
        assert!(html.contains("class=\"shape ph\"") && html.contains(">차트</div>"));
        assert!(html.contains("발표자 노트 — 슬라이드 1"));
        // The bundle maps image ids to files with extensions.
        let dir = tempfile::tempdir().unwrap();
        let index = write_bundle(&model, dir.path()).unwrap();
        let written = std::fs::read_to_string(index).unwrap();
        assert!(written.contains("<img src=\"img/img1.png\""));
        assert!(dir.path().join("img/img1.png").is_file());
    }

    #[test]
    fn flow_documents_follow_the_declared_page_size() {
        let model = DocModel { page_mm: Some((182.0, 257.0)), ..Default::default() };
        assert!(to_html(&model).contains("@page { size: 182.00mm 257.00mm; margin: 0; }"));
        let unknown = DocModel { page_mm: Some((5.0, 9000.0)), ..Default::default() };
        assert_eq!(page_size_mm(&unknown), (210.0, 297.0), "implausible sizes fall back to A4");
        assert!(to_html(&DocModel::default()).contains("@page { size: 210.00mm 297.00mm; margin: 0; }"));
        let colored = Run { text: "빨강".into(), color: Some("#FF0000".into()), ..Default::default() };
        let mut out = String::new();
        run_html(&colored, &mut out);
        assert_eq!(out, "<span style=\"color:#FF0000;\">빨강</span>");
        let unsafe_color = Run { text: "x".into(), color: Some("red;background:url(x)".into()), ..Default::default() };
        out.clear();
        run_html(&unsafe_color, &mut out);
        assert_eq!(out, "x", "only #rrggbb reaches the style attribute");
    }
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
