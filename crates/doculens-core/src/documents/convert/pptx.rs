//! PPTX (PresentationML) reader on `zip` + `quick-xml`: slides in deck order,
//! each as the title placeholder (heading), the other shapes in reading order
//! (top → bottom, left → right) — text bodies with bullets and formatting,
//! tables with merged cells, pictures — followed by the speaker notes. Slides
//! are separated by page breaks; charts / SmartArt leave a note.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::error::{CoreError, Result};

use super::model::{Align, Block, Cell, DocModel, ImageData, ListKind, Paragraph, Run, Table};
use super::xlsx::{read_rels, resolve_target};
use super::xml::{self, Element};

type Archive = zip::ZipArchive<BufReader<File>>;

/// EMU → mm (914400 EMU per inch).
fn emu_to_mm(v: f32) -> f32 {
    v / 914_400.0 * 25.4
}

fn read_part(archive: &mut Archive, name: &str) -> Result<Option<Element>> {
    let Some(n) = xml::find_zip_entry(archive, name) else { return Ok(None) };
    let Some(bytes) = xml::read_zip_entry(archive, &n, xml::MAX_XML_BYTES)? else { return Ok(None) };
    Ok(Some(xml::parse(&bytes)?))
}

struct Ctx<'a> {
    archive: &'a mut Archive,
    /// Relationship id → resolved part name of the current slide.
    rels: HashMap<String, String>,
    images: Vec<ImageData>,
    /// Part name → image id (pictures reused across slides are stored once).
    loaded: HashMap<String, Option<String>>,
    notes: Vec<String>,
}

impl Ctx<'_> {
    fn image_for(&mut self, r_id: &str) -> Option<String> {
        let part = self.rels.get(r_id)?.clone();
        if let Some(v) = self.loaded.get(&part) {
            return v.clone();
        }
        let result = self.load_image(&part);
        self.loaded.insert(part, result.clone());
        result
    }

    fn load_image(&mut self, part: &str) -> Option<String> {
        let name = xml::find_zip_entry(self.archive, part)?;
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

/// Placeholder type of a shape (`p:nvSpPr/p:nvPr/p:ph@type`), `""` when the `ph` element has no type
/// (a body placeholder), `None` for free shapes.
fn placeholder(sp: &Element) -> Option<String> {
    let ph = sp.child("nvSpPr").or_else(|| sp.child("nvPicPr"))?.child("nvPr")?.child("ph")?;
    Some(ph.attr("type").unwrap_or("").to_string())
}

/// Top-left offset of a shape in EMU (for reading order).
fn offset(sp: &Element) -> Option<(f32, f32)> {
    let off = sp
        .child("spPr")
        .or_else(|| sp.child("xfrm"))
        .and_then(|p| if p.name == "xfrm" { Some(p) } else { p.child("xfrm") })?
        .child("off")?;
    Some((off.attr_f32("y")?, off.attr_f32("x")?))
}

fn run_props(r: &Element) -> (bool, bool, bool, Option<f32>) {
    let Some(pr) = r.child("rPr") else { return (false, false, false, None) };
    (
        pr.attr_bool("b").unwrap_or(false),
        pr.attr_bool("i").unwrap_or(false),
        pr.attr("u").is_some_and(|u| u != "none"),
        pr.attr_f32("sz").map(|s| s / 100.0),
    )
}

/// One `a:p` → paragraph. `bulleted_default` says whether the shape inherits bullets (body placeholders).
fn paragraph(p: &Element, bulleted_default: bool) -> Paragraph {
    let mut para = Paragraph::default();
    if let Some(pr) = p.child("pPr") {
        para.align = match pr.attr("algn") {
            Some("ctr") => Align::Center,
            Some("r") => Align::Right,
            Some("just") | Some("dist") => Align::Justify,
            _ => Align::Left,
        };
        para.list = if pr.child("buNone").is_some() {
            None
        } else if pr.child("buAutoNum").is_some() {
            Some(ListKind::Number)
        } else if pr.child("buChar").is_some() || bulleted_default {
            Some(ListKind::Bullet)
        } else {
            None
        };
    } else if bulleted_default {
        para.list = Some(ListKind::Bullet);
    }
    for child in p.elements() {
        match child.name.as_str() {
            "r" | "fld" => {
                let (bold, italic, underline, size_pt) = run_props(child);
                let text = child.child("t").map(|t| t.text()).unwrap_or_default();
                if !text.is_empty() {
                    para.runs.push(Run { text, bold, italic, underline, size_pt, line_break: false });
                }
            }
            "br" => {
                if let Some(last) = para.runs.last_mut() {
                    last.line_break = true;
                } else {
                    para.runs.push(Run { text: String::new(), line_break: true, ..Default::default() });
                }
            }
            _ => {}
        }
    }
    if para.is_empty() {
        para.list = None;
    }
    para
}

fn text_body(sp: &Element, body: &Element, heading: Option<u8>, bulleted_default: bool, out: &mut Vec<Block>) {
    let _ = sp;
    for p in body.children_named("p") {
        let mut para = paragraph(p, bulleted_default && heading.is_none());
        if para.is_empty() {
            continue;
        }
        para.heading = heading;
        if heading.is_some() {
            para.list = None;
        }
        out.push(Block::Paragraph(para));
    }
}

/// `a:tbl` → table (`gridSpan` / `rowSpan` spans; `hMerge` / `vMerge` continuation cells dropped).
fn table(tbl: &Element) -> Table {
    let header_first = tbl.child("tblPr").and_then(|p| p.attr_bool("firstRow")).unwrap_or(false);
    let col_widths_mm: Vec<f32> = tbl
        .child("tblGrid")
        .map(|g| g.children_named("gridCol").filter_map(|c| c.attr_f32("w")).map(emu_to_mm).collect())
        .unwrap_or_default();
    let mut rows = Vec::new();
    for (i, tr) in tbl.children_named("tr").enumerate() {
        let mut row = Vec::new();
        for tc in tr.children_named("tc") {
            if tc.attr_bool("hMerge").unwrap_or(false) || tc.attr_bool("vMerge").unwrap_or(false) {
                continue;
            }
            let mut blocks = Vec::new();
            if let Some(body) = tc.child("txBody") {
                for p in body.children_named("p") {
                    let para = paragraph(p, false);
                    if !para.is_empty() {
                        blocks.push(Block::Paragraph(para));
                    }
                }
            }
            row.push(Cell {
                blocks,
                row_span: tc.attr_u32("rowSpan").unwrap_or(1).max(1).min(u16::MAX as u32) as u16,
                col_span: tc.attr_u32("gridSpan").unwrap_or(1).max(1).min(u16::MAX as u32) as u16,
                header: header_first && i == 0,
            });
        }
        if !row.is_empty() {
            rows.push(row);
        }
    }
    Table { rows, col_widths_mm }
}

/// Shapes of a shape tree in reading order; group shapes are flattened.
fn shapes_in_order(tree: &Element) -> Vec<&Element> {
    let mut items: Vec<(f32, f32, usize, &Element)> = Vec::new();
    let mut seq = 0usize;
    for sp in tree.elements() {
        if !matches!(sp.name.as_str(), "sp" | "pic" | "graphicFrame" | "grpSp") {
            continue;
        }
        let (y, x) = offset(sp).unwrap_or((f32::MAX, f32::MAX));
        items.push((y, x, seq, sp));
        seq += 1;
    }
    // Stable by (y, x); shapes without geometry keep their document order at the end.
    items.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    items.into_iter().map(|(_, _, _, e)| e).collect()
}

fn shape_blocks(ctx: &mut Ctx<'_>, sp: &Element, out: &mut Vec<Block>, title: &mut Option<String>) {
    match sp.name.as_str() {
        "sp" => {
            let ph = placeholder(sp);
            // Slide number / date / footer placeholders carry no content worth keeping.
            if matches!(ph.as_deref(), Some("sldNum") | Some("dt") | Some("ftr")) {
                return;
            }
            let heading = match ph.as_deref() {
                Some("title") | Some("ctrTitle") => Some(1),
                Some("subTitle") => Some(2),
                _ => None,
            };
            let bulleted_default = matches!(ph.as_deref(), Some("") | Some("body") | Some("obj"));
            if let Some(body) = sp.child("txBody") {
                let start = out.len();
                text_body(sp, body, heading, bulleted_default, out);
                if heading == Some(1) && title.is_none() {
                    *title = out[start..].iter().find_map(|b| match b {
                        Block::Paragraph(p) => Some(p.text().trim().chars().take(120).collect::<String>()),
                        _ => None,
                    });
                }
            }
        }
        "pic" => {
            let r_id = sp
                .child("blipFill")
                .and_then(|f| f.child("blip"))
                .and_then(|b| b.attr("embed").or(b.attr("link")))
                .map(|s| s.to_string());
            let ext = sp.child("spPr").and_then(|p| p.child("xfrm")).and_then(|x| x.child("ext"));
            let width_mm = ext.and_then(|e| e.attr_f32("cx")).map(emu_to_mm);
            let height_mm = ext.and_then(|e| e.attr_f32("cy")).map(emu_to_mm);
            if let Some(id) = r_id.and_then(|r| ctx.image_for(&r)) {
                out.push(Block::Image { id, width_mm, height_mm });
            }
        }
        "graphicFrame" => {
            if let Some(tbl) = sp.find("tbl") {
                out.push(Block::Table(table(tbl)));
            } else if sp.find("chart").is_some() {
                ctx.notes.push("차트는 변환본에 포함되지 않습니다".into());
            } else if sp.find("relIds").is_some() {
                ctx.notes.push("SmartArt는 변환본에 포함되지 않습니다".into());
            } else if sp.find("oleObj").is_some() {
                ctx.notes.push("포함된 개체(OLE)는 변환본에 포함되지 않습니다".into());
            }
        }
        "grpSp" => {
            for child in shapes_in_order(sp) {
                shape_blocks(ctx, child, out, title);
            }
        }
        _ => {}
    }
}

/// Text of a notes slide (the slide-number / image placeholders are skipped).
fn notes_paragraphs(notes: &Element) -> Vec<Paragraph> {
    let mut out = Vec::new();
    let Some(tree) = notes.child("cSld").and_then(|c| c.child("spTree")) else { return out };
    for sp in tree.children_named("sp") {
        if matches!(
            placeholder(sp).as_deref(),
            Some("sldNum") | Some("sldImg") | Some("hdr") | Some("ftr") | Some("dt")
        ) {
            continue;
        }
        if let Some(body) = sp.child("txBody") {
            for p in body.children_named("p") {
                let mut para = paragraph(p, false);
                if para.is_empty() {
                    continue;
                }
                para.list = None;
                for r in &mut para.runs {
                    r.italic = true;
                }
                out.push(para);
            }
        }
    }
    out
}

pub fn read(path: &Path) -> Result<DocModel> {
    let file = File::open(path).map_err(|e| CoreError::io(path, e))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| CoreError::Convert(format!("not a zip archive: {e}")))?;
    let presentation = read_part(&mut archive, "ppt/presentation.xml")?
        .ok_or_else(|| CoreError::Convert("ppt/presentation.xml missing (not a PPTX file)".into()))?;
    let mut model = DocModel::default();
    if let Some(sz) = presentation.child("sldSz") {
        if let (Some(cx), Some(cy)) = (sz.attr_f32("cx"), sz.attr_f32("cy")) {
            model.page_mm = Some((emu_to_mm(cx), emu_to_mm(cy)));
        }
    }
    let pres_rels = read_rels(&mut archive, "ppt/_rels/presentation.xml.rels");
    // `p:sldId` carries both `id="256"` and `r:id="rId2"`; prefixes are dropped, so take the value the rels know.
    let slide_parts: Vec<String> = presentation
        .child("sldIdLst")
        .map(|l| {
            l.children_named("sldId")
                .filter_map(|s| s.attrs.iter().filter(|(k, _)| k == "id").find_map(|(_, v)| pres_rels.get(v)))
                .map(|t| resolve_target("ppt", t))
                .collect()
        })
        .unwrap_or_default();
    let mut ctx = Ctx {
        archive: &mut archive,
        rels: HashMap::new(),
        images: Vec::new(),
        loaded: HashMap::new(),
        notes: Vec::new(),
    };
    let mut title: Option<String> = None;
    let mut slide_no = 0usize;
    for part in slide_parts {
        let Some(slide) = read_part(ctx.archive, &part)? else { continue };
        slide_no += 1;
        let dir = part.rsplit_once('/').map(|(d, _)| d).unwrap_or("ppt/slides");
        let file_name = part.rsplit('/').next().unwrap_or_default();
        let rels_raw = read_rels(ctx.archive, &format!("{dir}/_rels/{file_name}.rels"));
        ctx.rels = rels_raw.iter().map(|(k, v)| (k.clone(), resolve_target(dir, v))).collect();
        if slide_no > 1 {
            model.blocks.push(Block::PageBreak);
        }
        let start = model.blocks.len();
        let mut slide_title: Option<String> = None;
        if let Some(tree) = slide.child("cSld").and_then(|c| c.child("spTree")) {
            for sp in shapes_in_order(tree) {
                shape_blocks(&mut ctx, sp, &mut model.blocks, &mut slide_title);
            }
        }
        // Keep the title first on the slide, then everything else in reading order.
        if let Some(pos) =
            model.blocks[start..].iter().position(|b| matches!(b, Block::Paragraph(p) if p.heading == Some(1)))
        {
            if pos > 0 {
                let t = model.blocks.remove(start + pos);
                model.blocks.insert(start, t);
            }
        } else {
            model.blocks.insert(
                start,
                Block::Paragraph(Paragraph {
                    heading: Some(3), ..Paragraph::plain(format!("슬라이드 {slide_no}"))
                }),
            );
        }
        if title.is_none() {
            title = slide_title;
        }
        // Speaker notes.
        let notes_part = ctx.rels.values().find(|v| v.contains("notesSlides/")).cloned();
        if let Some(np) = notes_part {
            if let Some(notes) = read_part(ctx.archive, &np)? {
                let paras = notes_paragraphs(&notes);
                if !paras.is_empty() {
                    model
                        .blocks
                        .push(Block::Paragraph(Paragraph { heading: Some(4), ..Paragraph::plain("발표자 노트") }));
                    model.blocks.extend(paras.into_iter().map(Block::Paragraph));
                }
            }
        }
    }
    if slide_no == 0 {
        model.notes.push("슬라이드가 없습니다".into());
    }
    model.title = title;
    model.images = std::mem::take(&mut ctx.images);
    let mut notes = std::mem::take(&mut ctx.notes);
    notes.dedup();
    model.notes.extend(notes);
    Ok(model)
}

// ---------------------------------------------------------------------------
// Test fixture writer
// ---------------------------------------------------------------------------

/// Writes a two-slide deck: a title slide (title + subtitle), then a slide with a
/// bulleted body placeholder, a free text box placed above it, a 2×2 table, a
/// picture and speaker notes.
pub fn write_sample_pptx(path: &Path, png: &[u8]) -> Result<()> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let file = File::create(path).map_err(|e| CoreError::io(path, e))?;
    let mut z = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let w = |z: &mut zip::ZipWriter<File>, name: &str, data: &[u8]| -> Result<()> {
        z.start_file(name, opts).map_err(|e| CoreError::Convert(e.to_string()))?;
        z.write_all(data).map_err(|e| CoreError::io(path, e))
    };
    w(&mut z, "[Content_Types].xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/><Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/><Override PartName="/ppt/notesSlides/notesSlide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml"/></Types>"#)?;
    w(&mut z, "_rels/.rels", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#)?;
    w(&mut z, "ppt/presentation.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId3"/></p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/></p:presentation>"#)?;
    w(&mut z, "ppt/_rels/presentation.xml.rels", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/></Relationships>"#)?;
    let ns = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
    w(&mut z, "ppt/slides/slide1.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld {ns}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="ctrTitle"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="1000000" y="2000000"/><a:ext cx="10000000" cy="1500000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:pPr algn="ctr"/><a:r><a:rPr lang="ko-KR" b="1" sz="4400"/><a:t>2026 사업 계획</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Subtitle"/><p:cNvSpPr/><p:nvPr><p:ph type="subTitle" idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="1000000" y="3800000"/><a:ext cx="10000000" cy="800000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:rPr lang="ko-KR"/><a:t>경영기획팀</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#).as_bytes())?;
    w(&mut z, "ppt/slides/slide2.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld {ns}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Body"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="800000" y="2500000"/><a:ext cx="5000000" cy="3000000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:rPr lang="ko-KR"/><a:t>매출 목표 12% 성장</a:t></a:r></a:p><a:p><a:pPr lvl="1"><a:buAutoNum type="arabicPeriod"/></a:pPr><a:r><a:rPr lang="ko-KR" i="1"/><a:t>신규 거래처 20곳</a:t></a:r><a:br/><a:r><a:t>기존 거래처 유지</a:t></a:r></a:p><a:p><a:pPr><a:buNone/></a:pPr><a:r><a:t>※ 환율 가정 1,350원</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Free text"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="800000" y="600000"/><a:ext cx="5000000" cy="600000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:rPr lang="ko-KR" u="sng"/><a:t>핵심 지표</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="4" name="Title 2"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="800000" y="1400000"/><a:ext cx="10000000" cy="800000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>목표</a:t></a:r></a:p></p:txBody></p:sp><p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="5" name="Table"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="6200000" y="2500000"/><a:ext cx="5000000" cy="1200000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="1"/><a:tblGrid><a:gridCol w="2500000"/><a:gridCol w="2500000"/></a:tblGrid><a:tr h="600000"><a:tc gridSpan="2"><a:txBody><a:bodyPr/><a:p><a:r><a:t>분기별 매출</a:t></a:r></a:p></a:txBody></a:tc><a:tc hMerge="1"><a:txBody><a:bodyPr/><a:p/></a:txBody></a:tc></a:tr><a:tr h="600000"><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>1분기</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>3.2억</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame><p:pic><p:nvPicPr><p:cNvPr id="6" name="Picture"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/></p:blipFill><p:spPr><a:xfrm><a:off x="6200000" y="4000000"/><a:ext cx="914400" cy="457200"/></a:xfrm></p:spPr></p:pic><p:sp><p:nvSpPr><p:cNvPr id="7" name="Slide Number"/><p:cNvSpPr/><p:nvPr><p:ph type="sldNum"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:fld id="x" type="slidenum"><a:t>2</a:t></a:fld></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#).as_bytes())?;
    w(&mut z, "ppt/slides/_rels/slide2.xml.rels", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide1.xml"/></Relationships>"#)?;
    w(&mut z, "ppt/notesSlides/notesSlide1.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:notes {ns}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide Image"/><p:cNvSpPr/><p:nvPr><p:ph type="sldImg"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes"/><p:cNvSpPr/><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>환율 가정은 기재부 전망 기준</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:notes>"#).as_bytes())?;
    w(&mut z, "ppt/media/image1.png", png)?;
    z.finish().map_err(|e| CoreError::Convert(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::hwpx::tests::TINY_PNG;
    use super::*;

    fn paragraphs(model: &DocModel) -> Vec<&Paragraph> {
        model
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(p) => Some(p),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn reads_generated_pptx() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deck.pptx");
        write_sample_pptx(&path, TINY_PNG).unwrap();
        let model = read(&path).unwrap();
        assert_eq!(model.title.as_deref(), Some("2026 사업 계획"));
        assert_eq!(model.page_mm.map(|(w, h)| (w.round(), h.round())), Some((339.0, 191.0)), "16:9 slide size");
        assert_eq!(model.blocks.iter().filter(|b| matches!(b, Block::PageBreak)).count(), 1);
        assert_eq!(model.images.len(), 1);
        let paras = paragraphs(&model);
        // Slide 1: centred bold title, subtitle as a level-2 heading.
        assert_eq!(paras[0].heading, Some(1));
        assert_eq!(paras[0].align, Align::Center);
        assert!(paras[0].runs[0].bold && paras[0].runs[0].size_pt == Some(44.0));
        assert_eq!(paras[1].heading, Some(2));
        assert_eq!(paras[1].text(), "경영기획팀");
        // Slide 2: the title placeholder leads even though the free text box sits higher on the slide;
        // then reading order: free text (y=600000) before the body (y=2500000).
        let s2: Vec<&&Paragraph> = paras.iter().skip(2).collect();
        assert_eq!(s2[0].text(), "목표");
        assert_eq!(s2[0].heading, Some(1));
        assert_eq!(s2[1].text(), "핵심 지표");
        assert!(s2[1].runs[0].underline && s2[1].list.is_none(), "free text boxes are not bulleted");
        assert_eq!(s2[2].text(), "매출 목표 12% 성장");
        assert_eq!(s2[2].list, Some(ListKind::Bullet), "body placeholders inherit bullets");
        assert_eq!(s2[3].list, Some(ListKind::Number));
        assert!(s2[3].runs[0].italic && s2[3].runs[0].line_break, "a:br sets the break on the preceding run");
        assert_eq!(s2[3].text(), "신규 거래처 20곳기존 거래처 유지");
        assert_eq!(s2[4].list, None, "buNone turns bullets off");
        assert!(!paras.iter().any(|p| p.text() == "2"), "slide-number placeholder skipped");
        let table = model
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].len(), 1, "hMerge continuation dropped");
        assert_eq!(table.rows[0][0].col_span, 2);
        assert!(table.rows[0][0].header);
        assert_eq!(table.rows[1][1].text().trim(), "3.2억");
        assert_eq!(table.col_widths_mm.len(), 2);
        let image = model.blocks.iter().find(|b| matches!(b, Block::Image { .. })).unwrap();
        if let Block::Image { width_mm, .. } = image {
            assert_eq!(width_mm.map(|w| w.round()), Some(25.0));
        }
        let notes = paras.iter().find(|p| p.heading == Some(4)).expect("notes heading");
        assert_eq!(notes.text(), "발표자 노트");
        let last = paras.last().unwrap();
        assert_eq!(last.text(), "환율 가정은 기재부 전망 기준");
        assert!(last.runs[0].italic);
        assert!(model.text().contains("분기별 매출"));
    }
}
