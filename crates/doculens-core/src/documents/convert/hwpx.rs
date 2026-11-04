//! HWPX (OWPML — Hancom's zip + XML format, decision 15) reader. Reads the
//! spine sections from `Contents/content.hpf`, paragraph/char properties from
//! `Contents/header.xml` and embedded images from `BinData/`. Structure
//! reference: Hancom's `hwpx-owpml-model` (Apache-2.0); this is an independent
//! implementation on `zip` + `quick-xml`.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::error::{CoreError, Result};

use super::model::{Align, Block, Cell, DocModel, ImageData, ListKind, Paragraph, Run, Table};
use super::xml::{self, Element, Node};

/// 1 HWPUNIT = 1/7200 inch.
pub fn hwpunit_to_mm(v: f32) -> f32 {
    v / 7200.0 * 25.4
}

#[derive(Debug, Default, Clone)]
struct CharPr {
    bold: bool,
    italic: bool,
    underline: bool,
    size_pt: Option<f32>,
}

#[derive(Debug, Default, Clone)]
struct ParaPr {
    align: Align,
    heading: Option<u8>,
    list: Option<ListKind>,
}

#[derive(Debug, Default)]
struct Header {
    char_prs: HashMap<String, CharPr>,
    para_prs: HashMap<String, ParaPr>,
    /// styleIDRef → paraPrIDRef (styles carry their own paragraph property).
    style_para: HashMap<String, String>,
    style_heading: HashMap<String, u8>,
}

fn parse_header(root: &Element) -> Header {
    let mut h = Header::default();
    let mut nodes = Vec::new();
    root.find_all("charPr", &mut nodes);
    for e in nodes.drain(..) {
        let Some(id) = e.attr("id") else { continue };
        let pr = CharPr {
            bold: e.child("bold").is_some(),
            italic: e.child("italic").is_some(),
            underline: e.child("underline").is_some_and(|u| u.attr("type").is_none_or(|t| t != "NONE")),
            size_pt: e.attr_f32("height").map(|v| v / 100.0),
        };
        h.char_prs.insert(id.to_string(), pr);
    }
    root.find_all("paraPr", &mut nodes);
    for e in nodes.drain(..) {
        let Some(id) = e.attr("id") else { continue };
        let align = match e.child("align").and_then(|a| a.attr("horizontal")).unwrap_or("") {
            "CENTER" => Align::Center,
            "RIGHT" => Align::Right,
            "JUSTIFY" | "DISTRIBUTE" | "DISTRIBUTE_SPACE" => Align::Justify,
            _ => Align::Left,
        };
        let (heading, list) = match e.child("heading") {
            Some(hd) => {
                let level = hd.attr_u32("level").unwrap_or(0) as u8;
                match hd.attr("type").unwrap_or("NONE") {
                    "OUTLINE" => (Some((level + 1).min(6)), None),
                    "NUMBER" => (None, Some(ListKind::Number)),
                    "BULLET" => (None, Some(ListKind::Bullet)),
                    _ => (None, None),
                }
            }
            None => (None, None),
        };
        h.para_prs.insert(id.to_string(), ParaPr { align, heading, list });
    }
    root.find_all("style", &mut nodes);
    for e in nodes.drain(..) {
        let Some(id) = e.attr("id") else { continue };
        if let Some(p) = e.attr("paraPrIDRef") {
            h.style_para.insert(id.to_string(), p.to_string());
        }
        // "개요 1" … "개요 7" / "Outline 1"
        let name = e.attr("name").unwrap_or("");
        let eng = e.attr("engName").unwrap_or("");
        for n in [name, eng] {
            let lower = n.to_lowercase();
            let rest = lower.strip_prefix("개요").or_else(|| lower.strip_prefix("outline"));
            if let Some(level) = rest.and_then(|r| r.trim().parse::<u8>().ok()) {
                h.style_heading.insert(id.to_string(), level.clamp(1, 6));
            }
        }
    }
    h
}

struct Ctx<'a> {
    header: &'a Header,
    images: Vec<ImageData>,
    bin_items: &'a HashMap<String, (String, String)>, // id → (href, mime)
    archive: &'a mut zip::ZipArchive<BufReader<File>>,
    notes: Vec<String>,
    loaded: HashMap<String, Option<String>>, // binaryItemIDRef → image id in model
}

impl Ctx<'_> {
    fn image_for(&mut self, item_ref: &str) -> Option<String> {
        if let Some(v) = self.loaded.get(item_ref) {
            return v.clone();
        }
        let result = self.load_image(item_ref);
        self.loaded.insert(item_ref.to_string(), result.clone());
        result
    }

    fn load_image(&mut self, item_ref: &str) -> Option<String> {
        let (href, mime) = match self.bin_items.get(item_ref) {
            Some((h, m)) => (h.clone(), m.clone()),
            None => {
                // Fall back to a BinData entry whose stem matches the id.
                let c = xml::zip_names(self.archive).into_iter().find(|n| {
                    let lower = n.to_ascii_lowercase();
                    lower.starts_with("bindata/")
                        && Path::new(n).file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(item_ref))
                })?;
                let mime = xml::mime_for_name(&c).to_string();
                (c, mime)
            }
        };
        let mime = if mime.is_empty() || mime == "application/octet-stream" {
            xml::mime_for_name(&href).to_string()
        } else {
            mime
        };
        if !xml::is_web_image(&mime) {
            self.notes.push(format!("이미지 형식 미지원: {href} ({mime})"));
            return None;
        }
        let name = xml::find_zip_entry(self.archive, &href)
            .or_else(|| xml::find_zip_entry(self.archive, &format!("Contents/{href}")))?;
        match xml::read_zip_entry(self.archive, &name, xml::MAX_BINARY_BYTES) {
            Ok(Some(bytes)) if !bytes.is_empty() => {
                let total: usize = self.images.iter().map(|i| i.bytes.len()).sum();
                if total + bytes.len() > xml::MAX_IMAGE_TOTAL_BYTES {
                    self.notes.push(format!("이미지 생략(용량 상한): {href}"));
                    return None;
                }
                let id = format!("img{}", self.images.len() + 1);
                self.images.push(ImageData { id: id.clone(), mime, bytes });
                Some(id)
            }
            Ok(_) => None,
            Err(e) => {
                self.notes.push(format!("이미지 읽기 실패: {href}: {e}"));
                None
            }
        }
    }
}

fn run_props(ctx: &Ctx<'_>, run: &Element) -> CharPr {
    run.attr("charPrIDRef").and_then(|id| ctx.header.char_prs.get(id)).cloned().unwrap_or_default()
}

/// Flattens the children of `<t>` into runs (tabs, line breaks, nbsp).
fn text_runs(t: &Element, pr: &CharPr, out: &mut Vec<Run>) {
    let mut current = String::new();
    let flush = |current: &mut String, out: &mut Vec<Run>, line_break: bool| {
        if !current.is_empty() || line_break {
            out.push(Run {
                text: std::mem::take(current),
                bold: pr.bold,
                italic: pr.italic,
                underline: pr.underline,
                size_pt: pr.size_pt,
                line_break,
            });
        }
    };
    for n in &t.children {
        match n {
            Node::Text(s) => current.push_str(s),
            Node::Element(e) => match e.name.as_str() {
                "tab" => current.push('\t'),
                "lineBreak" => flush(&mut current, out, true),
                "nbSpace" | "fwSpace" => current.push('\u{a0}'),
                "hyphen" => current.push('-'),
                _ => current.push_str(&e.text()),
            },
        }
    }
    flush(&mut current, out, false);
}

fn para_props(ctx: &Ctx<'_>, p: &Element) -> ParaPr {
    let mut pr = p
        .attr("paraPrIDRef")
        .and_then(|id| ctx.header.para_prs.get(id))
        .cloned()
        .or_else(|| {
            p.attr("styleIDRef")
                .and_then(|s| ctx.header.style_para.get(s))
                .and_then(|id| ctx.header.para_prs.get(id))
                .cloned()
        })
        .unwrap_or_default();
    if pr.heading.is_none() {
        if let Some(level) = p.attr("styleIDRef").and_then(|s| ctx.header.style_heading.get(s)) {
            pr.heading = Some(*level);
        }
    }
    pr
}

/// Converts a `<p>` (possibly containing tables / pictures inside its runs)
/// into blocks.
fn paragraph_blocks(ctx: &mut Ctx<'_>, p: &Element, out: &mut Vec<Block>) {
    if p.attr_bool("pageBreak").unwrap_or(false) && !out.is_empty() {
        out.push(Block::PageBreak);
    }
    let pr = para_props(ctx, p);
    let mut para = Paragraph { heading: pr.heading, align: pr.align, list: pr.list, runs: Vec::new() };
    let mut pending: Vec<Block> = Vec::new();
    for run in p.children_named("run") {
        let cpr = run_props(ctx, run);
        for child in run.elements() {
            match child.name.as_str() {
                "t" => text_runs(child, &cpr, &mut para.runs),
                "tbl" => pending.push(Block::Table(table(ctx, child))),
                "pic" => {
                    if let Some(block) = picture(ctx, child) {
                        pending.push(block);
                    }
                }
                "secPr" | "ctrl" | "fieldBegin" | "fieldEnd" | "compose" | "dutmal" | "btn" | "bookmark" => {}
                "container" | "rect" | "ellipse" | "arc" | "polygon" | "curve" | "line" | "connectLine" | "textart"
                | "equation" | "ole" | "video" => {
                    // Text boxes / shapes: keep any text they carry (approximation).
                    let mut lists = Vec::new();
                    child.find_all("subList", &mut lists);
                    for sl in lists {
                        for inner in sl.children_named("p") {
                            paragraph_blocks(ctx, inner, &mut pending);
                        }
                    }
                    let mut pics = Vec::new();
                    child.find_all("pic", &mut pics);
                    for pic in pics {
                        if let Some(block) = picture(ctx, pic) {
                            pending.push(block);
                        }
                    }
                    if child.name == "equation" {
                        if let Some(script) = child.child("script") {
                            para.runs.push(Run { text: script.text(), ..Default::default() });
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if !para.runs.is_empty() || pending.is_empty() {
        out.push(Block::Paragraph(para));
    }
    out.append(&mut pending);
}

fn picture(ctx: &mut Ctx<'_>, pic: &Element) -> Option<Block> {
    let img = pic.find("img")?;
    let item_ref = img.attr("binaryItemIDRef")?;
    let id = ctx.image_for(item_ref)?;
    let sz = pic.child("sz").or_else(|| pic.find("sz"));
    let width_mm = sz.and_then(|s| s.attr_f32("width")).map(hwpunit_to_mm);
    let height_mm = sz.and_then(|s| s.attr_f32("height")).map(hwpunit_to_mm);
    Some(Block::Image { id, width_mm, height_mm })
}

fn table(ctx: &mut Ctx<'_>, tbl: &Element) -> Table {
    // Attribute values are untrusted: clamp before allocating.
    let row_cnt = (tbl.attr_u32("rowCnt").unwrap_or(0) as usize).min(xml::MAX_TABLE_ROWS);
    let col_cnt = (tbl.attr_u32("colCnt").unwrap_or(0) as usize).min(xml::MAX_TABLE_COLS);
    let mut rows: Vec<Vec<Cell>> =
        vec![Vec::new(); row_cnt.max(tbl.children_named("tr").count().min(xml::MAX_TABLE_ROWS))];
    let mut col_widths: Vec<f32> = vec![0.0; col_cnt];
    for (ri, tr) in tbl.children_named("tr").enumerate() {
        for tc in tr.children_named("tc") {
            let addr = tc.child("cellAddr");
            let row = addr.and_then(|a| a.attr_u32("rowAddr")).map(|r| r as usize).unwrap_or(ri);
            let col = addr.and_then(|a| a.attr_u32("colAddr")).map(|c| c as usize);
            let span = tc.child("cellSpan");
            let col_span =
                span.and_then(|s| s.attr_u32("colSpan")).unwrap_or(1).clamp(1, xml::MAX_TABLE_COLS as u32) as u16;
            let row_span =
                span.and_then(|s| s.attr_u32("rowSpan")).unwrap_or(1).clamp(1, xml::MAX_TABLE_ROWS as u32) as u16;
            if row >= xml::MAX_TABLE_ROWS || col.is_some_and(|c| c >= xml::MAX_TABLE_COLS) {
                continue;
            }
            if let (Some(c), Some(w)) = (col, tc.child("cellSz").and_then(|s| s.attr_f32("width"))) {
                if col_span == 1 && c < col_widths.len() && col_widths[c] == 0.0 {
                    col_widths[c] = hwpunit_to_mm(w);
                }
            }
            let mut blocks = Vec::new();
            if let Some(sl) = tc.child("subList") {
                for p in sl.children_named("p") {
                    paragraph_blocks(ctx, p, &mut blocks);
                }
            }
            let cell = Cell { blocks, row_span, col_span, header: tc.attr_bool("header").unwrap_or(false) };
            if row >= rows.len() {
                rows.resize(row + 1, Vec::new());
            }
            rows[row].push(cell);
        }
    }
    rows.retain(|r| !r.is_empty());
    if col_widths.iter().any(|w| *w <= 0.0) {
        col_widths.clear();
    }
    Table { rows, col_widths_mm: col_widths }
}

/// Parses `Contents/content.hpf`: ordered section hrefs and binary items.
fn parse_package(root: &Element) -> (Vec<String>, HashMap<String, (String, String)>) {
    let mut items: HashMap<String, (String, String)> = HashMap::new();
    let mut nodes = Vec::new();
    root.find_all("item", &mut nodes);
    for e in nodes.drain(..) {
        if let (Some(id), Some(href)) = (e.attr("id"), e.attr("href")) {
            items.insert(id.to_string(), (href.to_string(), e.attr("media-type").unwrap_or("").to_string()));
        }
    }
    let mut sections = Vec::new();
    root.find_all("itemref", &mut nodes);
    for e in nodes.drain(..) {
        if let Some(href) = e.attr("idref").and_then(|id| items.get(id)).map(|(h, _)| h.clone()) {
            sections.push(href);
        }
    }
    (sections, items)
}

pub fn read(path: &Path) -> Result<DocModel> {
    let file = File::open(path).map_err(|e| CoreError::io(path, e))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| CoreError::Convert(format!("not a zip archive: {e}")))?;
    if let Some(mime) = xml::read_zip_entry(&mut archive, "mimetype", 1024)? {
        let m = String::from_utf8_lossy(&mime);
        if !m.trim().starts_with("application/hwp+zip") {
            return Err(CoreError::Convert(format!("unexpected mimetype {}", m.trim())));
        }
    }
    let header = match xml::find_zip_entry(&archive, "Contents/header.xml") {
        Some(name) => {
            let bytes = xml::read_zip_entry(&mut archive, &name, xml::MAX_XML_BYTES)?.unwrap_or_default();
            parse_header(&xml::parse(&bytes)?)
        }
        None => Header::default(),
    };
    let (mut sections, bin_items) = match xml::find_zip_entry(&archive, "Contents/content.hpf") {
        Some(name) => {
            let bytes = xml::read_zip_entry(&mut archive, &name, xml::MAX_XML_BYTES)?.unwrap_or_default();
            parse_package(&xml::parse(&bytes)?)
        }
        None => (Vec::new(), HashMap::new()),
    };
    if sections.is_empty() {
        // No spine: fall back to Contents/section<N>.xml in numeric order.
        let mut names: Vec<(u32, String)> = xml::zip_names(&archive)
            .into_iter()
            .filter_map(|n| {
                let lower = n.to_ascii_lowercase();
                let stem = lower.strip_prefix("contents/section")?.strip_suffix(".xml")?;
                stem.parse::<u32>().ok().map(|i| (i, n.clone()))
            })
            .collect();
        names.sort();
        sections = names.into_iter().map(|(_, n)| n).collect();
    }
    if sections.is_empty() {
        return Err(CoreError::Convert("no sections found (Contents/section*.xml)".into()));
    }
    let mut model = DocModel::default();
    let mut ctx = Ctx {
        header: &header,
        images: Vec::new(),
        bin_items: &bin_items,
        archive: &mut archive,
        notes: Vec::new(),
        loaded: HashMap::new(),
    };
    for (i, href) in sections.iter().enumerate() {
        let name = match xml::find_zip_entry(ctx.archive, href)
            .or_else(|| xml::find_zip_entry(ctx.archive, &format!("Contents/{href}")))
        {
            Some(n) => n,
            None => {
                ctx.notes.push(format!("구역 파일 없음: {href}"));
                continue;
            }
        };
        let bytes = xml::read_zip_entry(ctx.archive, &name, xml::MAX_XML_BYTES)?.unwrap_or_default();
        let root = xml::parse(&bytes)?;
        if i > 0 && !model.blocks.is_empty() {
            model.blocks.push(Block::PageBreak);
        }
        if model.page_mm.is_none() {
            if let Some(pg) = root.find("pagePr") {
                if let (Some(w), Some(h)) = (pg.attr_f32("width"), pg.attr_f32("height")) {
                    // `landscape` only names the orientation; width/height already carry it.
                    model.page_mm = Some((hwpunit_to_mm(w), hwpunit_to_mm(h)));
                }
            }
        }
        for p in root.children_named("p") {
            paragraph_blocks(&mut ctx, p, &mut model.blocks);
        }
    }
    model.images = std::mem::take(&mut ctx.images);
    model.notes = std::mem::take(&mut ctx.notes);
    model.title = model.blocks.iter().find_map(|b| match b {
        Block::Paragraph(p) if !p.is_empty() => Some(p.text().trim().chars().take(120).collect::<String>()),
        _ => None,
    });
    Ok(model)
}

// ---------------------------------------------------------------------------
// Test fixture writer (also used by the import / render tests)
// ---------------------------------------------------------------------------

/// Writes a small but structurally faithful HWPX file: `pages` sections, one
/// heading + body paragraph + a 2×2 table with a merged cell per section, and
/// an embedded PNG on the first page.
pub fn write_sample_hwpx(path: &Path, pages: u32, png: &[u8]) -> Result<()> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let file = File::create(path).map_err(|e| CoreError::io(path, e))?;
    let mut z = zip::ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let w = |z: &mut zip::ZipWriter<File>, name: &str, data: &[u8], opts: SimpleFileOptions| -> Result<()> {
        z.start_file(name, opts).map_err(|e| CoreError::Convert(e.to_string()))?;
        z.write_all(data).map_err(|e| CoreError::io(path, e))
    };
    w(&mut z, "mimetype", b"application/hwp+zip", stored)?;
    w(&mut z, "version.xml", br##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><hv:HCFVersion xmlns:hv="http://www.hancom.co.kr/hwpml/2011/version" tagetApplication="WORDPROCESSOR" major="5" minor="1" micro="1" buildNumber="0" os="1" xmlVersion="1.4" application="Hancom Office Hangul" appVersion="11, 0, 0, 1"/>"##, deflated)?;
    w(&mut z, "META-INF/container.xml", br##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><ocf:container xmlns:ocf="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:hpf="http://www.hancom.co.kr/schema/2011/hpf"><ocf:rootfiles><ocf:rootfile full-path="Contents/content.hpf" media-type="application/hwpml-package+xml"/></ocf:rootfiles></ocf:container>"##, deflated)?;
    let mut hpf = String::from(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><opf:package xmlns:opf="http://www.idpf.org/2007/opf/" version="" unique-identifier="" id=""><opf:metadata/><opf:manifest><opf:item id="header" href="Contents/header.xml" media-type="application/xml"/>"##,
    );
    for i in 0..pages {
        hpf.push_str(&format!(
            r##"<opf:item id="section{i}" href="Contents/section{i}.xml" media-type="application/xml"/>"##
        ));
    }
    hpf.push_str(r##"<opf:item id="image1" href="BinData/image1.png" media-type="image/png" isEmbeded="1"/></opf:manifest><opf:spine>"##);
    for i in 0..pages {
        hpf.push_str(&format!(r##"<opf:itemref idref="section{i}" linear="yes"/>"##));
    }
    hpf.push_str("</opf:spine></opf:package>");
    w(&mut z, "Contents/content.hpf", hpf.as_bytes(), deflated)?;
    let header = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core" version="1.4" secCnt="1"><hh:refList><hh:charProperties itemCnt="3"><hh:charPr id="0" height="1000" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="2"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/></hh:charPr><hh:charPr id="1" height="1600" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="2"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:bold/></hh:charPr><hh:charPr id="2" height="900" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="2"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:italic/><hh:underline type="BOTTOM" shape="SOLID" color="#000000"/></hh:charPr></hh:charProperties><hh:paraProperties itemCnt="3"><hh:paraPr id="0" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0"><hh:align horizontal="JUSTIFY" vertical="BASELINE"/><hh:heading type="NONE" idRef="0" level="0"/></hh:paraPr><hh:paraPr id="1" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0"><hh:align horizontal="CENTER" vertical="BASELINE"/><hh:heading type="OUTLINE" idRef="1" level="0"/></hh:paraPr><hh:paraPr id="2" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0"><hh:align horizontal="LEFT" vertical="BASELINE"/><hh:heading type="BULLET" idRef="1" level="0"/></hh:paraPr></hh:paraProperties><hh:styles itemCnt="2"><hh:style id="0" type="PARA" name="바탕글" engName="Normal" paraPrIDRef="0" charPrIDRef="0" nextStyleIDRef="0" langID="1042" lockForm="0"/><hh:style id="1" type="PARA" name="개요 1" engName="Outline 1" paraPrIDRef="1" charPrIDRef="1" nextStyleIDRef="1" langID="1042" lockForm="0"/></hh:styles></hh:refList></hh:head>"##;
    w(&mut z, "Contents/header.xml", header.as_bytes(), deflated)?;
    for i in 0..pages {
        let mut sec = String::from(
            r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core">"##,
        );
        if i == 0 {
            sec.push_str(r##"<hp:p id="1" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:secPr id="" textDirection="HORIZONTAL" spaceColumns="1134" tabStop="8000" tabStopVal="4000" tabStopUnit="HWPUNIT" outlineShapeIDRef="1" memoShapeIDRef="0" textVerticalWidthHead="0" masterPageCnt="0"><hp:pagePr landscape="WIDELY" width="59528" height="84188" gutterType="LEFT_ONLY"><hp:margin header="4252" footer="4252" gutter="0" left="8504" right="8504" top="5668" bottom="4252"/></hp:pagePr></hp:secPr></hp:run></hp:p>"##);
        }
        sec.push_str(&format!(r##"<hp:p id="2" paraPrIDRef="1" styleIDRef="1" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="1"><hp:t>견적서 {}쪽</hp:t></hp:run></hp:p>"##, i + 1));
        sec.push_str(&format!(r##"<hp:p id="3" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>본문 문단 {} &amp; 특수문자<hp:tab/>탭<hp:lineBreak/>둘째 줄</hp:t></hp:run><hp:run charPrIDRef="2"><hp:t>작은 기울임</hp:t></hp:run></hp:p>"##, i + 1));
        sec.push_str(r##"<hp:p id="4" paraPrIDRef="2" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>항목 하나</hp:t></hp:run></hp:p>"##);
        sec.push_str(r##"<hp:p id="5" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:tbl id="10" zOrder="0" numberingType="TABLE" textWrap="TOP_AND_BOTTOM" textFlow="BOTH_SIDES" lock="0" dropcapstyle="None" pageBreak="CELL" repeatHeader="1" rowCnt="2" colCnt="2" cellSpacing="0" borderFillIDRef="3" noAdjust="0"><hp:sz width="42520" height="5000" widthRelTo="ABSOLUTE" heightRelTo="ABSOLUTE" protect="0"/><hp:pos treatAsChar="1" affectLSpacing="0" flowWithText="1" allowOverlap="0" holdAnchorAndSO="0" vertRelTo="PARA" horzRelTo="COLUMN" vertAlign="TOP" horzAlign="LEFT" vertOffset="0" horzOffset="0"/><hp:tr><hp:tc name="" header="1" hasMargin="0" protect="0" editable="0" dirty="0" borderFillIDRef="3"><hp:subList id="" textDirection="HORIZONTAL" lineWrap="BREAK" vertAlign="CENTER" linkListIDRef="0" linkListNextIDRef="0" textWidth="0" textHeight="0" hasTextRef="0" hasNumRef="0"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="1"><hp:t>품목</hp:t></hp:run></hp:p></hp:subList><hp:cellAddr colAddr="0" rowAddr="0"/><hp:cellSpan colSpan="2" rowSpan="1"/><hp:cellSz width="42520" height="2500"/><hp:cellMargin left="510" right="510" top="141" bottom="141"/></hp:tc></hp:tr><hp:tr><hp:tc name="" header="0" hasMargin="0" protect="0" editable="0" dirty="0" borderFillIDRef="3"><hp:subList id="" textDirection="HORIZONTAL" lineWrap="BREAK" vertAlign="CENTER"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>노트북</hp:t></hp:run></hp:p></hp:subList><hp:cellAddr colAddr="0" rowAddr="1"/><hp:cellSpan colSpan="1" rowSpan="1"/><hp:cellSz width="21260" height="2500"/></hp:tc><hp:tc name="" header="0" hasMargin="0" protect="0" editable="0" dirty="0" borderFillIDRef="3"><hp:subList id="" textDirection="HORIZONTAL" lineWrap="BREAK" vertAlign="CENTER"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>1,250,000</hp:t></hp:run></hp:p></hp:subList><hp:cellAddr colAddr="1" rowAddr="1"/><hp:cellSpan colSpan="1" rowSpan="1"/><hp:cellSz width="21260" height="2500"/></hp:tc></hp:tr></hp:tbl></hp:run></hp:p>"##);
        if i == 0 {
            sec.push_str(r##"<hp:p id="6" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:pic id="20" zOrder="1" numberingType="PICTURE" textWrap="TOP_AND_BOTTOM" textFlow="BOTH_SIDES" lock="0" dropcapstyle="None" href="" groupLevel="0" instid="1" reverse="0"><hp:offset x="0" y="0"/><hp:orgSz width="8504" height="8504"/><hp:curSz width="8504" height="8504"/><hp:flip horizontal="0" vertical="0"/><hp:rotationInfo angle="0" centerX="4252" centerY="4252" rotateimage="1"/><hp:renderingInfo><hc:transMatrix e1="1" e2="0" e3="0" e4="0" e5="1" e6="0"/><hc:scaMatrix e1="1" e2="0" e3="0" e4="0" e5="1" e6="0"/><hc:rotMatrix e1="1" e2="0" e3="0" e4="0" e5="1" e6="0"/></hp:renderingInfo><hp:lineShape color="#000000" width="0" style="NONE" endCap="FLAT" headStyle="NORMAL" tailStyle="NORMAL" headfill="1" tailfill="1" headSz="SMALL_SMALL" tailSz="SMALL_SMALL" outlineStyle="NORMAL" alpha="0"/><hp:imgRect><hc:pt0 x="0" y="0"/><hc:pt1 x="8504" y="0"/><hc:pt2 x="8504" y="8504"/><hc:pt3 x="0" y="8504"/></hp:imgRect><hp:imgClip left="0" right="8504" top="0" bottom="8504"/><hp:inMargin left="0" right="0" top="0" bottom="0"/><hp:imgDim dimwidth="8504" dimheight="8504"/><hc:img binaryItemIDRef="image1" bright="0" contrast="0" effect="REAL_PIC" alpha="0"/><hp:sz width="8504" height="8504" widthRelTo="ABSOLUTE" heightRelTo="ABSOLUTE" protect="0"/><hp:pos treatAsChar="1" affectLSpacing="0" flowWithText="1" allowOverlap="0" holdAnchorAndSO="0" vertRelTo="PARA" horzRelTo="COLUMN" vertAlign="TOP" horzAlign="LEFT" vertOffset="0" horzOffset="0"/></hp:pic></hp:run></hp:p>"##);
        }
        sec.push_str("</hs:sec>");
        w(&mut z, &format!("Contents/section{i}.xml"), sec.as_bytes(), deflated)?;
    }
    w(&mut z, "BinData/image1.png", png, stored)?;
    z.finish().map_err(|e| CoreError::Convert(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 1×1 red PNG.
    pub const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00,
        0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18,
        0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn reads_generated_hwpx() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.hwpx");
        write_sample_hwpx(&path, 3, TINY_PNG).unwrap();
        let model = read(&path).unwrap();
        assert_eq!(model.title.as_deref(), Some("견적서 1쪽"));
        assert_eq!(model.page_mm.map(|(w, h)| (w.round(), h.round())), Some((210.0, 297.0)));
        assert_eq!(model.images.len(), 1);
        assert_eq!(model.images[0].mime, "image/png");
        let breaks = model.blocks.iter().filter(|b| matches!(b, Block::PageBreak)).count();
        assert_eq!(breaks, 2, "one break between each section");
        let headings: Vec<&Paragraph> = model
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(p) if p.heading.is_some() => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(headings.len(), 3);
        assert_eq!(headings[0].align, Align::Center);
        assert!(headings[0].runs[0].bold);
        let body = model
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Paragraph(p) if p.text().starts_with("본문") => Some(p),
                _ => None,
            })
            .unwrap();
        assert_eq!(body.text(), "본문 문단 1 & 특수문자\t탭둘째 줄작은 기울임");
        assert!(body.runs.iter().any(|r| r.line_break));
        let small = body.runs.last().unwrap();
        assert!(small.italic && small.underline && small.size_pt == Some(9.0));
        assert!(model.blocks.iter().any(|b| matches!(b, Block::Paragraph(p) if p.list.is_some())));
        let table = model
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0][0].col_span, 2);
        assert!(table.rows[0][0].header);
        assert_eq!(table.rows[1][1].text().trim(), "1,250,000");
        assert_eq!(table.col_widths_mm.len(), 2);
        let image = model.blocks.iter().find(|b| matches!(b, Block::Image { .. })).unwrap();
        if let Block::Image { width_mm, .. } = image {
            assert_eq!(width_mm.map(|w| w.round()), Some(30.0));
        }
        let text = model.text();
        assert!(text.contains("품목\n노트북\t1,250,000"));
        assert_eq!(model.estimated_pages(), 3);
    }

    #[test]
    fn rejects_non_hwpx_zip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.hwpx");
        {
            use std::io::Write;
            let f = File::create(&path).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file("mimetype", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b"application/zip").unwrap();
            z.finish().unwrap();
        }
        let err = read(&path).unwrap_err();
        assert_eq!(err.code(), "convert");
        std::fs::write(&path, b"not a zip").unwrap();
        assert_eq!(read(&path).unwrap_err().code(), "convert");
    }
}
