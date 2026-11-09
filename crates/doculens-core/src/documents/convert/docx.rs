//! DOCX (WordprocessingML) reader on `zip` + `quick-xml`: paragraphs with
//! heading / alignment / list hints, runs with basic formatting, tables with
//! `gridSpan` / `vMerge` resolution, inline images and page breaks.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::error::{CoreError, Result};

use super::model::{Align, Block, Cell, DocModel, ImageData, ListKind, Paragraph, Run, Table};
use super::xml::{self, Element};

/// EMU → mm (914400 EMU per inch).
fn emu_to_mm(v: f32) -> f32 {
    v / 914_400.0 * 25.4
}

/// Twentieths of a point → mm.
fn twips_to_mm(v: f32) -> f32 {
    v / 20.0 / 72.0 * 25.4
}

#[derive(Debug, Default)]
struct Styles {
    /// styleId → heading level (from the style name "heading N" or outlineLvl).
    heading: HashMap<String, u8>,
    /// styleId → basedOn
    based_on: HashMap<String, String>,
    list_styles: HashMap<String, ListKind>,
}

fn parse_styles(root: &Element) -> Styles {
    let mut s = Styles::default();
    for st in root.children_named("style") {
        let Some(id) = st.attr("styleId") else { continue };
        let name = st.child("name").and_then(|n| n.attr("val")).unwrap_or("").to_lowercase();
        let outline = st.child("pPr").and_then(|p| p.child("outlineLvl")).and_then(|o| o.attr_u32("val"));
        let from_name = name
            .strip_prefix("heading ")
            .or_else(|| name.strip_prefix("제목 "))
            .and_then(|r| r.trim().parse::<u8>().ok());
        if let Some(level) = from_name.or(outline.map(|o| o as u8 + 1)) {
            s.heading.insert(id.to_string(), level.clamp(1, 6));
        }
        if name.contains("list bullet") {
            s.list_styles.insert(id.to_string(), ListKind::Bullet);
        } else if name.contains("list number") {
            s.list_styles.insert(id.to_string(), ListKind::Number);
        }
        if let Some(b) = st.child("basedOn").and_then(|b| b.attr("val")) {
            s.based_on.insert(id.to_string(), b.to_string());
        }
    }
    s
}

impl Styles {
    fn heading_for(&self, style_id: &str) -> Option<u8> {
        let mut cur = Some(style_id.to_string());
        let mut hops = 0;
        while let Some(id) = cur {
            if let Some(l) = self.heading.get(&id) {
                return Some(*l);
            }
            hops += 1;
            if hops > 8 {
                break;
            }
            cur = self.based_on.get(&id).cloned();
        }
        None
    }
}

struct Ctx<'a> {
    styles: &'a Styles,
    rels: &'a HashMap<String, String>, // rId → target (relative to word/)
    archive: &'a mut zip::ZipArchive<BufReader<File>>,
    images: Vec<ImageData>,
    loaded: HashMap<String, Option<String>>,
    notes: Vec<String>,
    /// numId → list kind (from numbering.xml); unknown ids default to bullets.
    num_kinds: &'a HashMap<String, ListKind>,
}

impl Ctx<'_> {
    fn image_for(&mut self, r_id: &str) -> Option<String> {
        if let Some(v) = self.loaded.get(r_id) {
            return v.clone();
        }
        let result = self.load_image(r_id);
        self.loaded.insert(r_id.to_string(), result.clone());
        result
    }

    fn load_image(&mut self, r_id: &str) -> Option<String> {
        let target = self.rels.get(r_id)?.clone();
        let candidates =
            [format!("word/{}", target.trim_start_matches('/')), target.trim_start_matches('/').to_string()];
        let name = candidates.iter().find_map(|c| xml::find_zip_entry(self.archive, c))?;
        let mime = xml::mime_for_name(&name).to_string();
        if !xml::is_web_image(&mime) {
            self.notes.push(format!("이미지 형식 미지원: {name} ({mime})"));
            return None;
        }
        match xml::read_zip_entry(self.archive, &name, xml::MAX_BINARY_BYTES) {
            Ok(Some(bytes)) if !bytes.is_empty() => {
                let total: usize = self.images.iter().map(|i| i.bytes.len()).sum();
                if total + bytes.len() > xml::MAX_IMAGE_TOTAL_BYTES {
                    self.notes.push(format!("이미지 생략(용량 상한): {name}"));
                    return None;
                }
                let id = format!("img{}", self.images.len() + 1);
                self.images.push(ImageData { id: id.clone(), mime, bytes });
                Some(id)
            }
            Ok(_) => None,
            Err(e) => {
                self.notes.push(format!("이미지 읽기 실패: {name}: {e}"));
                None
            }
        }
    }
}

fn run_props(r: &Element) -> (bool, bool, bool, Option<f32>) {
    let Some(pr) = r.child("rPr") else { return (false, false, false, None) };
    let on = |name: &str| pr.child(name).is_some_and(|e| e.attr("val").is_none_or(|v| v != "0" && v != "false"));
    let underline = pr.child("u").is_some_and(|u| u.attr("val").is_none_or(|v| v != "none"));
    let size = pr.child("sz").and_then(|s| s.attr_f32("val")).map(|v| v / 2.0);
    (on("b"), on("i"), underline, size)
}

/// Appends the runs of `r` (text, tabs, breaks, drawings) to the paragraph.
fn run_blocks(ctx: &mut Ctx<'_>, r: &Element, para: &mut Paragraph, pending: &mut Vec<Block>) {
    let (bold, italic, underline, size_pt) = run_props(r);
    let mut text = String::new();
    let mk = |text: &mut String, line_break: bool| Run {
        text: std::mem::take(text),
        bold,
        italic,
        underline,
        size_pt,
        line_break,
        color: None,
    };
    for child in r.elements() {
        match child.name.as_str() {
            "t" => text.push_str(&child.text()),
            "tab" => text.push('\t'),
            "noBreakHyphen" => text.push('-'),
            "softHyphen" => {}
            "sym" => {
                if let Some(c) =
                    child.attr("char").and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32)
                {
                    text.push(c);
                }
            }
            "br" | "cr" => {
                if child.attr("type") == Some("page") {
                    if !text.is_empty() {
                        para.runs.push(mk(&mut text, false));
                    }
                    pending.push(Block::PageBreak);
                } else {
                    para.runs.push(mk(&mut text, true));
                }
            }
            "drawing" | "pict" | "object" => {
                let mut blips = Vec::new();
                child.find_all("blip", &mut blips);
                let mut r_ids: Vec<String> =
                    blips.iter().filter_map(|b| b.attr("embed").or(b.attr("link")).map(|s| s.to_string())).collect();
                let mut datas = Vec::new();
                child.find_all("imagedata", &mut datas);
                r_ids.extend(datas.iter().filter_map(|d| d.attr("id").map(|s| s.to_string())));
                let extent = child.find("extent");
                let width_mm = extent.and_then(|e| e.attr_f32("cx")).map(emu_to_mm);
                let height_mm = extent.and_then(|e| e.attr_f32("cy")).map(emu_to_mm);
                for r_id in r_ids {
                    if let Some(id) = ctx.image_for(&r_id) {
                        pending.push(Block::Image { id, width_mm, height_mm });
                    }
                }
                // Text boxes inside drawings.
                let mut txs = Vec::new();
                child.find_all("txbxContent", &mut txs);
                for tx in txs {
                    body_blocks(ctx, tx, pending);
                }
            }
            "lastRenderedPageBreak" => {}
            _ => {}
        }
    }
    if !text.is_empty() {
        para.runs.push(mk(&mut text, false));
    }
}

fn paragraph(ctx: &mut Ctx<'_>, p: &Element, out: &mut Vec<Block>) {
    let ppr = p.child("pPr");
    if ppr.and_then(|pp| pp.child("pageBreakBefore")).is_some() && !out.is_empty() {
        out.push(Block::PageBreak);
    }
    let style_id = ppr.and_then(|pp| pp.child("pStyle")).and_then(|s| s.attr("val")).unwrap_or("");
    let heading = ctx.styles.heading_for(style_id);
    let align = match ppr.and_then(|pp| pp.child("jc")).and_then(|j| j.attr("val")).unwrap_or("") {
        "center" => Align::Center,
        "right" | "end" => Align::Right,
        "both" | "distribute" => Align::Justify,
        _ => Align::Left,
    };
    let list = ppr
        .and_then(|pp| pp.child("numPr"))
        .map(|np| {
            np.child("numId")
                .and_then(|n| n.attr("val"))
                .and_then(|id| ctx.num_kinds.get(id).copied())
                .unwrap_or(ListKind::Bullet)
        })
        .or_else(|| ctx.styles.list_styles.get(style_id).copied());
    let mut para = Paragraph { heading, align, list, runs: Vec::new(), level: 0 };
    let mut pending = Vec::new();
    collect_runs(ctx, p, &mut para, &mut pending);
    if !para.runs.is_empty() || pending.is_empty() {
        out.push(Block::Paragraph(para));
    }
    out.append(&mut pending);
}

/// Runs may be wrapped in hyperlinks, smart tags, content controls, insertions.
fn collect_runs(ctx: &mut Ctx<'_>, el: &Element, para: &mut Paragraph, pending: &mut Vec<Block>) {
    for child in el.elements() {
        match child.name.as_str() {
            "r" => run_blocks(ctx, child, para, pending),
            "hyperlink" | "smartTag" | "ins" | "fldSimple" | "customXml" | "sdtContent" | "moveTo" | "bdo" | "dir" => {
                collect_runs(ctx, child, para, pending)
            }
            "sdt" => {
                if let Some(c) = child.child("sdtContent") {
                    collect_runs(ctx, c, para, pending);
                }
            }
            "pPr" | "del" | "moveFrom" | "proofErr" | "bookmarkStart" | "bookmarkEnd" | "commentRangeStart"
            | "commentRangeEnd" => {}
            _ => {}
        }
    }
}

fn table(ctx: &mut Ctx<'_>, tbl: &Element) -> Table {
    let col_widths: Vec<f32> = tbl
        .child("tblGrid")
        .map(|g| g.children_named("gridCol").filter_map(|c| c.attr_f32("w")).map(twips_to_mm).collect())
        .unwrap_or_default();
    // (grid column, row span owner index) bookkeeping for vMerge.
    struct Raw {
        cell: Cell,
        grid_col: usize,
        v_continue: bool,
    }
    let mut raw_rows: Vec<Vec<Raw>> = Vec::new();
    for tr in tbl.children_named("tr").take(xml::MAX_TABLE_ROWS) {
        let mut row = Vec::new();
        let mut grid_col = 0usize;
        for tc in tr.children_named("tc") {
            let pr = tc.child("tcPr");
            let span = pr
                .and_then(|p| p.child("gridSpan"))
                .and_then(|g| g.attr_u32("val"))
                .unwrap_or(1)
                .clamp(1, xml::MAX_TABLE_COLS as u32);
            if grid_col >= xml::MAX_TABLE_COLS {
                break; // attribute-driven geometry beyond any real document
            }
            let v_merge = pr.and_then(|p| p.child("vMerge"));
            let v_continue = v_merge.is_some_and(|v| v.attr("val").is_none_or(|val| val == "continue"));
            let mut blocks = Vec::new();
            body_blocks(ctx, tc, &mut blocks);
            row.push(Raw {
                cell: Cell { blocks, row_span: 1, col_span: span as u16, header: false },
                grid_col,
                v_continue,
            });
            grid_col += span as usize;
        }
        raw_rows.push(row);
    }
    // Resolve vertical merges: a `continue` cell extends the nearest cell
    // above in the same grid column.
    let mut rows: Vec<Vec<Cell>> = Vec::with_capacity(raw_rows.len());
    let mut owners: Vec<Vec<Option<(usize, usize)>>> = Vec::new(); // per row: grid_col → (row idx, cell idx)
    for (ri, raw) in raw_rows.into_iter().enumerate() {
        let mut row_cells: Vec<Cell> = Vec::new();
        let mut owner_row: Vec<Option<(usize, usize)>> = Vec::new();
        for r in raw {
            if r.v_continue {
                let above = owners.get(ri.wrapping_sub(1)).and_then(|o| o.get(r.grid_col).copied().flatten());
                if let Some((orow, ocell)) = above {
                    if let Some(c) = rows.get_mut(orow).and_then(|rw| rw.get_mut(ocell)) {
                        c.row_span += 1;
                    }
                    if owner_row.len() <= r.grid_col {
                        owner_row.resize(r.grid_col + 1, None);
                    }
                    owner_row[r.grid_col] = Some((orow, ocell));
                    continue;
                }
            }
            let idx = row_cells.len();
            if owner_row.len() <= r.grid_col {
                owner_row.resize(r.grid_col + 1, None);
            }
            owner_row[r.grid_col] = Some((ri, idx));
            row_cells.push(r.cell);
        }
        rows.push(row_cells);
        owners.push(owner_row);
    }
    let header_rows =
        tbl.children_named("tr").take_while(|tr| tr.child("trPr").and_then(|p| p.child("tblHeader")).is_some()).count();
    for row in rows.iter_mut().take(header_rows) {
        for c in row {
            c.header = true;
        }
    }
    rows.retain(|r| !r.is_empty());
    Table { rows, col_widths_mm: col_widths }
}

/// Body-level children: paragraphs, tables, content controls, section breaks.
fn body_blocks(ctx: &mut Ctx<'_>, el: &Element, out: &mut Vec<Block>) {
    for child in el.elements() {
        match child.name.as_str() {
            "p" => paragraph(ctx, child, out),
            "tbl" => {
                let t = table(ctx, child);
                out.push(Block::Table(t));
            }
            "sdt" => {
                if let Some(c) = child.child("sdtContent") {
                    body_blocks(ctx, c, out);
                }
            }
            "sdtContent" | "customXml" | "txbxContent" => body_blocks(ctx, child, out),
            "sectPr" | "tcPr" | "tblPr" | "tblGrid" => {}
            _ => {}
        }
    }
}

fn parse_rels(root: &Element) -> HashMap<String, String> {
    root.children_named("Relationship")
        .filter_map(|r| Some((r.attr("Id")?.to_string(), r.attr("Target")?.to_string())))
        .collect()
}

fn parse_numbering(root: &Element) -> HashMap<String, ListKind> {
    // abstractNum → kind (first level), then num → abstractNum.
    let mut abstract_kinds: HashMap<String, ListKind> = HashMap::new();
    for a in root.children_named("abstractNum") {
        let Some(id) = a.attr("abstractNumId") else { continue };
        let fmt = a.child("lvl").and_then(|l| l.child("numFmt")).and_then(|f| f.attr("val")).unwrap_or("bullet");
        abstract_kinds.insert(id.to_string(), if fmt == "bullet" { ListKind::Bullet } else { ListKind::Number });
    }
    let mut out = HashMap::new();
    for n in root.children_named("num") {
        if let (Some(id), Some(a)) = (n.attr("numId"), n.child("abstractNumId").and_then(|a| a.attr("val"))) {
            if let Some(kind) = abstract_kinds.get(a) {
                out.insert(id.to_string(), *kind);
            }
        }
    }
    out
}

pub fn read(path: &Path) -> Result<DocModel> {
    let file = File::open(path).map_err(|e| CoreError::io(path, e))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| CoreError::Convert(format!("not a zip archive: {e}")))?;
    let doc_name = xml::find_zip_entry(&archive, "word/document.xml")
        .ok_or_else(|| CoreError::Convert("word/document.xml missing (not a DOCX file)".into()))?;
    let styles = match xml::find_zip_entry(&archive, "word/styles.xml") {
        Some(n) => {
            let bytes = xml::read_zip_entry(&mut archive, &n, xml::MAX_XML_BYTES)?.unwrap_or_default();
            xml::parse(&bytes).map(|r| parse_styles(&r)).unwrap_or_default()
        }
        None => Styles::default(),
    };
    let rels = match xml::find_zip_entry(&archive, "word/_rels/document.xml.rels") {
        Some(n) => {
            let bytes = xml::read_zip_entry(&mut archive, &n, xml::MAX_XML_BYTES)?.unwrap_or_default();
            xml::parse(&bytes).map(|r| parse_rels(&r)).unwrap_or_default()
        }
        None => HashMap::new(),
    };
    let num_kinds = match xml::find_zip_entry(&archive, "word/numbering.xml") {
        Some(n) => {
            let bytes = xml::read_zip_entry(&mut archive, &n, xml::MAX_XML_BYTES)?.unwrap_or_default();
            xml::parse(&bytes).map(|r| parse_numbering(&r)).unwrap_or_default()
        }
        None => HashMap::new(),
    };
    let bytes = xml::read_zip_entry(&mut archive, &doc_name, xml::MAX_XML_BYTES)?.unwrap_or_default();
    let root = xml::parse(&bytes)?;
    let body = root.child("body").ok_or_else(|| CoreError::Convert("document.xml has no body".into()))?;
    let mut model = DocModel::default();
    if let Some(pg) = body.child("sectPr").and_then(|s| s.child("pgSz")) {
        if let (Some(w), Some(h)) = (pg.attr_f32("w"), pg.attr_f32("h")) {
            model.page_mm = Some((twips_to_mm(w), twips_to_mm(h)));
        }
    }
    let mut ctx = Ctx {
        styles: &styles,
        rels: &rels,
        archive: &mut archive,
        images: Vec::new(),
        loaded: HashMap::new(),
        notes: Vec::new(),
        num_kinds: &num_kinds,
    };
    body_blocks(&mut ctx, body, &mut model.blocks);
    model.images = std::mem::take(&mut ctx.images);
    model.notes = std::mem::take(&mut ctx.notes);
    model.title = model.blocks.iter().find_map(|b| match b {
        Block::Paragraph(p) if !p.is_empty() => Some(p.text().trim().chars().take(120).collect::<String>()),
        _ => None,
    });
    Ok(model)
}

// ---------------------------------------------------------------------------
// Test fixture writer
// ---------------------------------------------------------------------------

/// Writes a small DOCX: `pages` pages separated by page breaks, each with a
/// heading, a formatted paragraph, a list item and a 3×2 table with a merged
/// header and a vertical merge; an inline PNG on the first page.
pub fn write_sample_docx(path: &Path, pages: u32, png: &[u8]) -> Result<()> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let file = File::create(path).map_err(|e| CoreError::io(path, e))?;
    let mut z = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let w = |z: &mut zip::ZipWriter<File>, name: &str, data: &[u8]| -> Result<()> {
        z.start_file(name, opts).map_err(|e| CoreError::Convert(e.to_string()))?;
        z.write_all(data).map_err(|e| CoreError::io(path, e))
    };
    w(&mut z, "[Content_Types].xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/></Types>"#)?;
    w(&mut z, "_rels/.rels", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#)?;
    w(&mut z, "word/_rels/document.xml.rels", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#)?;
    w(&mut z, "word/styles.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:default="1" w:styleId="a"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="1"><w:name w:val="heading 1"/><w:basedOn w:val="a"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="MySub"><w:name w:val="My Sub"/><w:basedOn w:val="1"/></w:style></w:styles>"#)?;
    w(&mut z, "word/numbering.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#)?;
    let mut doc = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>"#,
    );
    for i in 0..pages {
        if i > 0 {
            doc.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
        }
        doc.push_str(&format!(r#"<w:p><w:pPr><w:pStyle w:val="1"/><w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>계약서 {}쪽</w:t></w:r></w:p>"#, i + 1));
        doc.push_str(&format!(r#"<w:p><w:pPr><w:pStyle w:val="MySub"/></w:pPr><w:r><w:t xml:space="preserve">소제목 {}</w:t></w:r></w:p>"#, i + 1));
        doc.push_str(r#"<w:p><w:r><w:t xml:space="preserve">본문 </w:t></w:r><w:hyperlink r:id="rId9"><w:r><w:rPr><w:i/><w:u w:val="single"/><w:sz w:val="16"/></w:rPr><w:t>링크</w:t></w:r></w:hyperlink><w:r><w:tab/><w:t>탭 뒤</w:t><w:br/><w:t>둘째 줄</w:t></w:r></w:p>"#);
        doc.push_str(r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>번호 항목</w:t></w:r></w:p>"#);
        doc.push_str(r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="4000"/><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>당사자</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p><w:r><w:t>갑</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>주식회사 A</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc><w:tc><w:p><w:r><w:t>대표 홍길동</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#);
        if i == 0 {
            doc.push_str(r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="457200"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:embed="rId5"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#);
        }
    }
    doc.push_str(r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body></w:document>"#);
    w(&mut z, "word/document.xml", doc.as_bytes())?;
    w(&mut z, "word/media/image1.png", png)?;
    z.finish().map_err(|e| CoreError::Convert(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::hwpx::tests::TINY_PNG;
    use super::*;

    #[test]
    fn reads_generated_docx() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.docx");
        write_sample_docx(&path, 2, TINY_PNG).unwrap();
        let model = read(&path).unwrap();
        assert_eq!(model.title.as_deref(), Some("계약서 1쪽"));
        assert_eq!(model.page_mm.map(|(w, h)| (w.round(), h.round())), Some((210.0, 297.0)));
        assert_eq!(model.images.len(), 1);
        assert_eq!(model.blocks.iter().filter(|b| matches!(b, Block::PageBreak)).count(), 1);
        let paras: Vec<&Paragraph> = model
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(p) => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(paras[0].heading, Some(1));
        assert_eq!(paras[0].align, Align::Center);
        assert!(paras[0].runs[0].bold);
        assert_eq!(paras[1].heading, Some(1), "basedOn chain resolves headings");
        let body = paras.iter().find(|p| p.text().starts_with("본문")).unwrap();
        assert_eq!(body.text(), "본문 링크\t탭 뒤둘째 줄");
        let link = &body.runs[1];
        assert!(link.italic && link.underline && link.size_pt == Some(8.0));
        assert!(body.runs.iter().any(|r| r.line_break));
        assert_eq!(paras.iter().find(|p| p.list.is_some()).unwrap().list, Some(ListKind::Number));
        let table = model
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.rows[0][0].col_span, 2);
        assert!(table.rows[0][0].header);
        assert_eq!(table.rows[1][0].row_span, 2, "vMerge continue extends the restart cell");
        assert_eq!(table.rows[2].len(), 1, "continued cell dropped");
        assert_eq!(table.rows[2][0].text().trim(), "대표 홍길동");
        assert_eq!(table.col_widths_mm.len(), 2);
        let image = model.blocks.iter().find(|b| matches!(b, Block::Image { .. })).unwrap();
        if let Block::Image { width_mm, height_mm, .. } = image {
            assert_eq!(width_mm.map(|w| w.round()), Some(25.0));
            assert_eq!(height_mm.map(|w| w.round()), Some(13.0));
        }
    }

    #[test]
    fn rejects_zip_without_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.docx");
        {
            use std::io::Write;
            let f = File::create(&path).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file("hello.txt", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b"hi").unwrap();
            z.finish().unwrap();
        }
        assert_eq!(read(&path).unwrap_err().code(), "convert");
    }
}
