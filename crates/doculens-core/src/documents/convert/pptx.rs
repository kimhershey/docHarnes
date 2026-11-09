//! PPTX (PresentationML) reader on `zip` + `quick-xml`.
//!
//! Two views of the same deck are produced:
//! * `DocModel::blocks` — slides in deck order as reading-order text (title placeholder first, then
//!   the other shapes top → bottom, left → right: text bodies with bullets, tables with merged cells,
//!   pictures), followed by the speaker notes; slides are separated by page breaks. Search, field
//!   discovery and the text fallback use this view.
//! * `DocModel::slides` — one [`Slide`] per slide with absolutely positioned [`Shape`]s (millimetres
//!   from the slide corner, z-order = document order): text boxes with anchor / insets / autofit scale
//!   and theme colours, pictures (cropped per `srcRect`, TIFF / BMP converted to PNG), tables, lines,
//!   and labelled frames for charts / SmartArt / OLE / vector images the converter cannot draw. The
//!   HTML emitter prints these on pages of the slide size, so the converted PDF keeps the slide's
//!   aspect ratio (user report 2026-11-09: slides were flowed onto A4 portrait, pictures stretched).
//!
//! Placeholder geometry and text defaults are inherited from the slide layout and master; colours
//! resolve through the master colour map and the theme (`schemeClr` + `lumMod` / `lumOff` / `tint`
//! / `shade`).

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::Path;

use crate::error::{CoreError, Result};

use super::model::{
    Align, Block, Cell, DocModel, ImageData, ListKind, Paragraph, Run, Shape, ShapeKind, Slide, Table, VAnchor,
};
use super::xlsx::{read_rels, resolve_target};
use super::xml::{self, Element};

type Archive = zip::ZipArchive<BufReader<File>>;

/// EMU → mm (914400 EMU per inch).
fn emu_to_mm(v: f32) -> f32 {
    v / 914_400.0 * 25.4
}

/// EMU → points (12700 EMU per point).
fn emu_to_pt(v: f32) -> f32 {
    v / 12_700.0
}

fn read_part(archive: &mut Archive, name: &str) -> Result<Option<Element>> {
    let Some(n) = xml::find_zip_entry(archive, name) else { return Ok(None) };
    let Some(bytes) = xml::read_zip_entry(archive, &n, xml::MAX_XML_BYTES)? else { return Ok(None) };
    Ok(Some(xml::parse(&bytes)?))
}

// ---------------------------------------------------------------------------
// Colours (theme + colour map + modifiers)
// ---------------------------------------------------------------------------

/// Theme colour scheme (`dk1`, `lt1`, `accent1`, … → `#rrggbb`) plus the master's colour map
/// (`bg1` → `lt1`, `tx1` → `dk1`, …).
#[derive(Debug, Clone, Default)]
pub struct Colors {
    scheme: HashMap<String, String>,
    map: HashMap<String, String>,
}

impl Colors {
    fn from_theme(theme: Option<&Element>) -> Colors {
        let mut c = Colors::default();
        if let Some(scheme) = theme.and_then(|t| t.child("themeElements")).and_then(|t| t.child("clrScheme")) {
            for entry in scheme.elements() {
                if let Some(v) = entry.elements().next().and_then(base_color) {
                    c.scheme.insert(entry.name.clone(), v);
                }
            }
        }
        c
    }

    fn set_map(&mut self, clr_map: Option<&Element>) {
        self.map.clear();
        for (k, v) in [("bg1", "lt1"), ("tx1", "dk1"), ("bg2", "lt2"), ("tx2", "dk2")] {
            self.map.insert(k.into(), v.into());
        }
        if let Some(m) = clr_map {
            for (k, v) in &m.attrs {
                self.map.insert(k.clone(), v.clone());
            }
        }
    }

    fn scheme(&self, name: &str) -> Option<String> {
        let key = self.map.get(name).map(String::as_str).unwrap_or(name);
        self.scheme.get(key).cloned()
    }

    /// Resolves one colour element (`srgbClr`, `schemeClr`, `sysClr`, `prstClr`) with its modifiers.
    pub fn resolve(&self, el: &Element) -> Option<String> {
        let base = match el.name.as_str() {
            "schemeClr" => self.scheme(el.attr("val")?)?,
            _ => base_color(el)?,
        };
        let rgb = parse_hex(&base)?;
        Some(to_css(apply_modifiers(rgb, el)))
    }

    /// Colour of a fill container (`spPr`, `rPr`, `bgPr`, `tcPr` …): `Some(None)` for `noFill`,
    /// `Some(Some(css))` for a solid (or the first gradient stop), `None` when nothing is declared.
    pub fn fill(&self, parent: &Element) -> Option<Option<String>> {
        if parent.child("noFill").is_some() {
            return Some(None);
        }
        if let Some(sf) = parent.child("solidFill") {
            return Some(sf.elements().next().and_then(|c| self.resolve(c)));
        }
        if let Some(gf) = parent.child("gradFill") {
            let stop = gf.child("gsLst").and_then(|l| l.children_named("gs").next());
            return Some(stop.and_then(|gs| gs.elements().next()).and_then(|c| self.resolve(c)));
        }
        None
    }

    /// Outline of a shape (`spPr/ln`): colour and width in points; `None` without a visible line.
    pub fn line(&self, sp_pr: &Element) -> Option<(String, f32)> {
        let ln = sp_pr.child("ln")?;
        if ln.child("noFill").is_some() {
            return None;
        }
        let width = ln.attr_f32("w").map(emu_to_pt).unwrap_or(0.75).max(0.25);
        // Without an explicit colour the line comes from the shape style: skipped rather than guessed.
        let color = self.fill(ln).flatten()?;
        Some((color, width))
    }
}

fn base_color(el: &Element) -> Option<String> {
    match el.name.as_str() {
        "srgbClr" => el.attr("val").map(|v| format!("#{}", v.trim().to_ascii_uppercase())),
        "sysClr" => el.attr("lastClr").map(|v| format!("#{}", v.trim().to_ascii_uppercase())),
        "prstClr" => preset_color(el.attr("val")?).map(str::to_string),
        _ => None,
    }
}

fn preset_color(name: &str) -> Option<&'static str> {
    Some(match name {
        "white" => "#FFFFFF",
        "black" => "#000000",
        "red" => "#FF0000",
        "green" => "#008000",
        "blue" => "#0000FF",
        "yellow" => "#FFFF00",
        "gray" | "grey" => "#808080",
        "ltGray" | "lightGray" => "#D3D3D3",
        "dkGray" | "darkGray" => "#A9A9A9",
        "orange" => "#FFA500",
        "purple" => "#800080",
        "navy" => "#000080",
        _ => return None,
    })
}

fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn to_css((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn rgb_to_hsl((r, g, b): (u8, u8, u8)) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-6 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if (max - r).abs() < 1e-6 {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if (max - g).abs() < 1e-6 {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

fn hsl_to_rgb((h, s, l): (f32, f32, f32)) -> (u8, u8, u8) {
    let hue = |p: f32, q: f32, mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let (r, g, b) = if s.abs() < 1e-6 {
        (l, l, l)
    } else {
        let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p = 2.0 * l - q;
        (hue(p, q, h + 1.0 / 3.0), hue(p, q, h), hue(p, q, h - 1.0 / 3.0))
    };
    let to = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    (to(r), to(g), to(b))
}

/// `lumMod` / `lumOff` (HSL luminance), `tint` (towards white), `shade` (towards black).
fn apply_modifiers(mut rgb: (u8, u8, u8), el: &Element) -> (u8, u8, u8) {
    let pct = |m: &Element| m.attr_f32("val").map(|v| v / 100_000.0);
    for m in el.elements() {
        match m.name.as_str() {
            "lumMod" => {
                if let Some(v) = pct(m) {
                    let (h, s, l) = rgb_to_hsl(rgb);
                    rgb = hsl_to_rgb((h, s, (l * v).clamp(0.0, 1.0)));
                }
            }
            "lumOff" => {
                if let Some(v) = pct(m) {
                    let (h, s, l) = rgb_to_hsl(rgb);
                    rgb = hsl_to_rgb((h, s, (l + v).clamp(0.0, 1.0)));
                }
            }
            "tint" => {
                if let Some(v) = pct(m) {
                    let f = |c: u8| ((c as f32) * v + 255.0 * (1.0 - v)).round().clamp(0.0, 255.0) as u8;
                    rgb = (f(rgb.0), f(rgb.1), f(rgb.2));
                }
            }
            "shade" => {
                if let Some(v) = pct(m) {
                    let f = |c: u8| ((c as f32) * v).round().clamp(0.0, 255.0) as u8;
                    rgb = (f(rgb.0), f(rgb.1), f(rgb.2));
                }
            }
            _ => {}
        }
    }
    rgb
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// A shape transform in EMU (slide coordinates after group transforms are applied).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Xfrm {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    rot_deg: f32,
    flip_h: bool,
    flip_v: bool,
}

/// `a:xfrm` of a shape (`spPr`), group (`grpSpPr`) or graphic frame (`p:xfrm`).
fn xfrm_of(el: &Element) -> Option<Xfrm> {
    let xfrm = ["spPr", "grpSpPr"]
        .iter()
        .find_map(|n| el.child(n).and_then(|p| p.child("xfrm")))
        .or_else(|| el.child("xfrm"))?;
    let off = xfrm.child("off")?;
    let ext = xfrm.child("ext")?;
    Some(Xfrm {
        x: off.attr_f32("x")?,
        y: off.attr_f32("y")?,
        w: ext.attr_f32("cx")?,
        h: ext.attr_f32("cy")?,
        rot_deg: xfrm.attr_f32("rot").unwrap_or(0.0) / 60_000.0,
        flip_h: xfrm.attr_bool("flipH").unwrap_or(false),
        flip_v: xfrm.attr_bool("flipV").unwrap_or(false),
    })
}

/// Child → parent mapping of a group shape (`grpSpPr/xfrm`: `off`/`ext` outside, `chOff`/`chExt` inside).
#[derive(Debug, Clone, Copy)]
struct GroupTx {
    ox: f32,
    oy: f32,
    chx: f32,
    chy: f32,
    sx: f32,
    sy: f32,
}

impl GroupTx {
    fn of(grp: &Element) -> Option<GroupTx> {
        let xfrm = grp.child("grpSpPr")?.child("xfrm")?;
        let off = xfrm.child("off")?;
        let ext = xfrm.child("ext")?;
        let (cx, cy) = (ext.attr_f32("cx")?, ext.attr_f32("cy")?);
        let (ox, oy) = (off.attr_f32("x")?, off.attr_f32("y")?);
        let (chx, chy) = match xfrm.child("chOff") {
            Some(c) => (c.attr_f32("x").unwrap_or(0.0), c.attr_f32("y").unwrap_or(0.0)),
            None => (ox, oy),
        };
        let (chw, chh) = match xfrm.child("chExt") {
            Some(c) => (c.attr_f32("cx").unwrap_or(cx), c.attr_f32("cy").unwrap_or(cy)),
            None => (cx, cy),
        };
        Some(GroupTx {
            ox,
            oy,
            chx,
            chy,
            sx: if chw > 0.0 { cx / chw } else { 1.0 },
            sy: if chh > 0.0 { cy / chh } else { 1.0 },
        })
    }

    fn apply(&self, x: Xfrm) -> Xfrm {
        Xfrm {
            x: self.ox + (x.x - self.chx) * self.sx,
            y: self.oy + (x.y - self.chy) * self.sy,
            w: x.w * self.sx,
            h: x.h * self.sy,
            ..x
        }
    }
}

/// Applies the group stack (innermost group last in `stack`) to a child transform.
fn to_slide(stack: &[GroupTx], mut x: Xfrm) -> Xfrm {
    for g in stack.iter().rev() {
        x = g.apply(x);
    }
    x
}

// ---------------------------------------------------------------------------
// Placeholders (layout / master inheritance)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Placeholder {
    kind: Option<String>,
    idx: Option<String>,
}

fn placeholder_of(sp: &Element) -> Option<Placeholder> {
    let ph = sp
        .child("nvSpPr")
        .or_else(|| sp.child("nvPicPr"))
        .or_else(|| sp.child("nvGraphicFramePr"))?
        .child("nvPr")?
        .child("ph")?;
    Some(Placeholder { kind: ph.attr("type").map(str::to_string), idx: ph.attr("idx").map(str::to_string) })
}

/// The placeholder type family used for inheritance (`ctrTitle` ≈ `title`, `obj` ≈ `body`).
fn ph_family(kind: Option<&str>) -> &'static str {
    match kind.unwrap_or("body") {
        "title" | "ctrTitle" => "title",
        "subTitle" => "subTitle",
        "body" | "obj" | "" => "body",
        "dt" => "dt",
        "ftr" => "ftr",
        "sldNum" => "sldNum",
        "pic" => "pic",
        "chart" => "chart",
        "tbl" => "tbl",
        _ => "other",
    }
}

/// One placeholder of a layout / master: geometry and first-level text defaults.
#[derive(Debug, Clone)]
struct PhEntry {
    ph: Placeholder,
    xfrm: Option<Xfrm>,
    size_pt: Option<f32>,
    color: Option<String>,
}

/// Placeholder shapes of a layout / master shape tree.
#[derive(Debug, Clone, Default)]
struct PhDefaults {
    entries: Vec<PhEntry>,
}

impl PhDefaults {
    fn from_tree(tree: Option<&Element>, colors: &Colors) -> PhDefaults {
        let mut d = PhDefaults::default();
        let Some(tree) = tree else { return d };
        for sp in tree.elements() {
            if !matches!(sp.name.as_str(), "sp" | "pic" | "graphicFrame") {
                continue;
            }
            let Some(ph) = placeholder_of(sp) else { continue };
            // First-level default run properties of the placeholder's own list style.
            let def_rpr = sp
                .child("txBody")
                .and_then(|b| b.child("lstStyle"))
                .and_then(|l| l.child("lvl1pPr"))
                .and_then(|p| p.child("defRPr"));
            d.entries.push(PhEntry {
                ph,
                xfrm: xfrm_of(sp),
                size_pt: def_rpr.and_then(|r| r.attr_f32("sz")).map(|s| s / 100.0),
                color: def_rpr.and_then(|r| colors.fill(r)).flatten(),
            });
        }
        d
    }

    fn lookup(&self, ph: &Placeholder) -> Option<&PhEntry> {
        let fam = ph_family(ph.kind.as_deref());
        // Exact idx + compatible family, then idx alone (bodies), then the family.
        self.entries
            .iter()
            .find(|e| ph.idx.is_some() && e.ph.idx == ph.idx && ph_family(e.ph.kind.as_deref()) == fam)
            .or_else(|| self.entries.iter().find(|e| ph.idx.is_some() && e.ph.idx == ph.idx && fam == "body"))
            .or_else(|| self.entries.iter().find(|e| ph_family(e.ph.kind.as_deref()) == fam))
    }
}

/// Master text styles: default run size and colour per placeholder family.
#[derive(Debug, Clone, Default)]
struct TextStyles {
    title: (Option<f32>, Option<String>),
    body: (Option<f32>, Option<String>),
    other: (Option<f32>, Option<String>),
}

impl TextStyles {
    fn from_master(master: Option<&Element>, colors: &Colors) -> TextStyles {
        let mut t = TextStyles::default();
        let Some(styles) = master.and_then(|m| m.child("txStyles")) else { return t };
        let read = |name: &str| -> (Option<f32>, Option<String>) {
            let def = styles.child(name).and_then(|s| s.child("lvl1pPr")).and_then(|p| p.child("defRPr"));
            (def.and_then(|r| r.attr_f32("sz")).map(|s| s / 100.0), def.and_then(|r| colors.fill(r)).flatten())
        };
        t.title = read("titleStyle");
        t.body = read("bodyStyle");
        t.other = read("otherStyle");
        t
    }

    fn for_family(&self, fam: &str, is_placeholder: bool) -> (f32, Option<String>) {
        let (size, color) = match fam {
            "title" => (self.title.0.or(Some(44.0)), self.title.1.clone()),
            "subTitle" => (self.body.0.map(|s| (s * 0.85).round()).or(Some(24.0)), self.body.1.clone()),
            "body" if is_placeholder => (self.body.0.or(Some(28.0)), self.body.1.clone()),
            _ => (self.other.0.or(Some(18.0)), self.other.1.clone()),
        };
        (size.unwrap_or(18.0), color)
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// Placeholder type of a shape (`""` for an untyped body placeholder) used by the reading-order view.
fn placeholder(sp: &Element) -> Option<String> {
    placeholder_of(sp).map(|p| p.kind.unwrap_or_default())
}

/// Top-left offset of a shape in EMU (for reading order).
fn offset(sp: &Element) -> Option<(f32, f32)> {
    let x = xfrm_of(sp)?;
    Some((x.y, x.x))
}

fn run_props(r: &Element, colors: Option<&Colors>) -> (bool, bool, bool, Option<f32>, Option<String>) {
    let Some(pr) = r.child("rPr") else { return (false, false, false, None, None) };
    (
        pr.attr_bool("b").unwrap_or(false),
        pr.attr_bool("i").unwrap_or(false),
        pr.attr("u").is_some_and(|u| u != "none"),
        pr.attr_f32("sz").map(|s| s / 100.0),
        colors.and_then(|c| c.fill(pr)).flatten(),
    )
}

/// One `a:p` → paragraph. `bulleted_default` says whether the shape inherits bullets (body placeholders).
fn paragraph(p: &Element, bulleted_default: bool, colors: Option<&Colors>) -> Paragraph {
    let mut para = Paragraph::default();
    if let Some(pr) = p.child("pPr") {
        para.align = match pr.attr("algn") {
            Some("ctr") => Align::Center,
            Some("r") => Align::Right,
            Some("just") | Some("dist") => Align::Justify,
            _ => Align::Left,
        };
        para.level = pr.attr_u32("lvl").unwrap_or(0).min(8) as u8;
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
                let (bold, italic, underline, size_pt, color) = run_props(child, colors);
                let text = child.child("t").map(|t| t.text()).unwrap_or_default();
                if !text.is_empty() {
                    para.runs.push(Run { text, bold, italic, underline, size_pt, line_break: false, color });
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

fn text_body(body: &Element, heading: Option<u8>, bulleted_default: bool, out: &mut Vec<Block>) {
    for p in body.children_named("p") {
        let mut para = paragraph(p, bulleted_default && heading.is_none(), None);
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
fn table(tbl: &Element, colors: Option<&Colors>) -> Table {
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
                    let para = paragraph(p, false, colors);
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

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

/// `a:srcRect` crop in fractions of the source image (left, top, right, bottom).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Crop {
    l: f32,
    t: f32,
    r: f32,
    b: f32,
}

impl Crop {
    fn of(blip_fill: &Element) -> Option<Crop> {
        let rect = blip_fill.child("srcRect")?;
        let f = |n: &str| rect.attr_f32(n).unwrap_or(0.0) / 100_000.0;
        let c = Crop { l: f("l"), t: f("t"), r: f("r"), b: f("b") };
        let visible = c.l.max(0.0) + c.r.max(0.0) < 0.98 && c.t.max(0.0) + c.b.max(0.0) < 0.98;
        let any = c.l.abs() > 0.001 || c.t.abs() > 0.001 || c.r.abs() > 0.001 || c.b.abs() > 0.001;
        (visible && any).then_some(c)
    }

    fn key(&self) -> String {
        format!("{:.3},{:.3},{:.3},{:.3}", self.l, self.t, self.r, self.b)
    }
}

struct Ctx<'a> {
    archive: &'a mut Archive,
    /// Relationship id → resolved part name of the current slide.
    rels: HashMap<String, String>,
    images: Vec<ImageData>,
    /// (part name, crop key) → image id (pictures reused across slides are stored once).
    loaded: HashMap<(String, String), Option<String>>,
    notes: Vec<String>,
    /// Total bytes of embedded images kept (OOM guard).
    image_bytes: usize,
}

impl Ctx<'_> {
    fn image_for(&mut self, r_id: &str, crop: Option<Crop>) -> Option<String> {
        let part = self.rels.get(r_id)?.clone();
        let key = (part.clone(), crop.map(|c| c.key()).unwrap_or_default());
        if let Some(v) = self.loaded.get(&key) {
            return v.clone();
        }
        let result = self.load_image(&part, crop);
        self.loaded.insert(key, result.clone());
        result
    }

    fn load_image(&mut self, part: &str, crop: Option<Crop>) -> Option<String> {
        let name = xml::find_zip_entry(self.archive, part)?;
        let mime = xml::mime_for_name(&name).to_string();
        let decodable = matches!(
            mime.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/bmp" | "image/webp" | "image/tiff"
        );
        if !xml::is_web_image(&mime) && !decodable {
            self.notes.push(format!("이미지 형식 미지원: {name} ({mime}) — 자리만 표시"));
            return None;
        }
        let bytes = match xml::read_zip_entry(self.archive, &name, xml::MAX_BINARY_BYTES) {
            Ok(Some(b)) if !b.is_empty() => b,
            _ => return None,
        };
        if self.image_bytes + bytes.len() > xml::MAX_IMAGE_TOTAL_BYTES {
            self.notes.push("이미지 용량 상한을 넘어 일부 이미지를 생략했습니다".into());
            return None;
        }
        // Crops and non-web formats go through the image crate (→ PNG); everything else is kept as is.
        let needs_decode = crop.is_some() || !xml::is_web_image(&mime);
        let (mime, bytes) = if needs_decode && decodable {
            match recode_png(&bytes, crop) {
                Some(png) => ("image/png".to_string(), png),
                None if xml::is_web_image(&mime) => {
                    if crop.is_some() {
                        self.notes.push(format!("이미지 자르기 미적용: {name}"));
                    }
                    (mime, bytes)
                }
                None => {
                    self.notes.push(format!("이미지를 읽을 수 없습니다: {name} ({mime})"));
                    return None;
                }
            }
        } else {
            (mime, bytes)
        };
        let id = format!("img{}", self.images.len() + 1);
        self.image_bytes += bytes.len();
        self.images.push(ImageData { id: id.clone(), mime, bytes });
        Some(id)
    }
}

/// Decodes `bytes`, applies the crop (fractions) and re-encodes as PNG.
fn recode_png(bytes: &[u8], crop: Option<Crop>) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = match crop {
        Some(c) => {
            let (w, h) = (img.width() as f32, img.height() as f32);
            let x0 = (w * c.l.max(0.0)).round().clamp(0.0, w - 1.0) as u32;
            let y0 = (h * c.t.max(0.0)).round().clamp(0.0, h - 1.0) as u32;
            let x1 = (w * (1.0 - c.r.max(0.0))).round().clamp(x0 as f32 + 1.0, w) as u32;
            let y1 = (h * (1.0 - c.b.max(0.0))).round().clamp(y0 as f32 + 1.0, h) as u32;
            img.crop_imm(x0, y0, x1 - x0, y1 - y0)
        }
        None => img,
    };
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

// ---------------------------------------------------------------------------
// Reading-order view (blocks)
// ---------------------------------------------------------------------------

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

fn blip_of(sp: &Element) -> (Option<String>, Option<Crop>) {
    let blip_fill = sp.child("blipFill");
    let r_id =
        blip_fill.and_then(|f| f.child("blip")).and_then(|b| b.attr("embed").or(b.attr("link"))).map(|s| s.to_string());
    (r_id, blip_fill.and_then(Crop::of))
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
                text_body(body, heading, bulleted_default, out);
                if heading == Some(1) && title.is_none() {
                    *title = out[start..].iter().find_map(|b| match b {
                        Block::Paragraph(p) => Some(p.text().trim().chars().take(120).collect::<String>()),
                        _ => None,
                    });
                }
            }
        }
        "pic" => {
            let (r_id, crop) = blip_of(sp);
            let geo = xfrm_of(sp);
            let width_mm = geo.map(|g| emu_to_mm(g.w));
            let height_mm = geo.map(|g| emu_to_mm(g.h));
            if let Some(id) = r_id.and_then(|r| ctx.image_for(&r, crop)) {
                out.push(Block::Image { id, width_mm, height_mm });
            }
        }
        "graphicFrame" => {
            if let Some(tbl) = sp.find("tbl") {
                out.push(Block::Table(table(tbl, None)));
            } else if sp.find("chart").is_some() {
                ctx.notes.push("차트는 변환본에 자리만 표시됩니다".into());
            } else if sp.find("relIds").is_some() {
                ctx.notes.push("SmartArt는 변환본에 자리만 표시됩니다".into());
            } else if sp.find("oleObj").is_some() {
                ctx.notes.push("포함된 개체(OLE)는 변환본에 자리만 표시됩니다".into());
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
                let mut para = paragraph(p, false, None);
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

// ---------------------------------------------------------------------------
// Positioned view (slides)
// ---------------------------------------------------------------------------

/// Everything a slide inherits: layout + master placeholders, master text styles, colours.
struct Inherited {
    layout: PhDefaults,
    master: PhDefaults,
    styles: TextStyles,
    colors: Colors,
    background: Option<String>,
}

const DEFAULT_INSETS_MM: [f32; 4] = [2.54, 1.27, 2.54, 1.27];

fn body_props(body: &Element) -> (VAnchor, [f32; 4], f32) {
    let pr = body.child("bodyPr");
    let anchor = match pr.and_then(|p| p.attr("anchor")) {
        Some("ctr") => VAnchor::Middle,
        Some("b") => VAnchor::Bottom,
        _ => VAnchor::Top,
    };
    let inset = |n: &str, default: f32| pr.and_then(|p| p.attr_f32(n)).map(emu_to_mm).unwrap_or(default);
    let insets = [inset("lIns", 2.54), inset("tIns", 1.27), inset("rIns", 2.54), inset("bIns", 1.27)];
    let scale = pr
        .and_then(|p| p.child("normAutofit"))
        .and_then(|a| a.attr_f32("fontScale"))
        .map(|s| (s / 100_000.0).clamp(0.2, 1.0))
        .unwrap_or(1.0);
    (anchor, insets, scale)
}

fn geometry_of(sp_pr: Option<&Element>) -> Option<String> {
    sp_pr.and_then(|p| p.child("prstGeom")).and_then(|g| g.attr("prst")).filter(|g| *g != "rect").map(str::to_string)
}

fn mm_shape(x: Xfrm, kind: ShapeKind) -> Shape {
    Shape {
        x_mm: emu_to_mm(x.x),
        y_mm: emu_to_mm(x.y),
        w_mm: emu_to_mm(x.w).max(0.1),
        h_mm: emu_to_mm(x.h).max(0.1),
        rotation_deg: x.rot_deg,
        flip_h: x.flip_h,
        flip_v: x.flip_v,
        fill: None,
        line: None,
        geometry: None,
        kind,
    }
}

fn slide_shapes(ctx: &mut Ctx<'_>, tree: &Element, inh: &Inherited, stack: &mut Vec<GroupTx>, out: &mut Vec<Shape>) {
    for sp in tree.elements() {
        match sp.name.as_str() {
            "sp" => {
                let ph = placeholder_of(sp);
                let kind = ph.as_ref().and_then(|p| p.kind.as_deref());
                if matches!(kind, Some("sldNum") | Some("dt") | Some("ftr")) {
                    continue;
                }
                let inherited = ph.as_ref().and_then(|p| inh.layout.lookup(p).or_else(|| inh.master.lookup(p)));
                let Some(geo) = xfrm_of(sp).or_else(|| inherited.and_then(|e| e.xfrm)) else { continue };
                let geo = to_slide(stack, geo);
                let sp_pr = sp.child("spPr");
                let fill = sp_pr.and_then(|p| inh.colors.fill(p)).flatten();
                let line = sp_pr.and_then(|p| inh.colors.line(p));
                let fam = ph_family(kind);
                let body = sp.child("txBody");
                let paragraphs: Vec<Paragraph> = body
                    .map(|b| {
                        let bulleted = ph.is_some() && fam == "body";
                        b.children_named("p").map(|p| paragraph(p, bulleted, Some(&inh.colors))).collect()
                    })
                    .unwrap_or_default();
                let has_text = paragraphs.iter().any(|p| !p.is_empty());
                if !has_text && fill.is_none() && line.is_none() {
                    continue;
                }
                let (anchor, insets_mm, font_scale) =
                    body.map(body_props).unwrap_or((VAnchor::Top, DEFAULT_INSETS_MM, 1.0));
                let (style_pt, style_color) = inh.styles.for_family(fam, ph.is_some());
                let default_pt = inherited.and_then(|e| e.size_pt).unwrap_or(style_pt);
                let default_color = inherited.and_then(|e| e.color.clone()).or(style_color);
                let mut shape = mm_shape(
                    geo,
                    ShapeKind::Text { paragraphs, anchor, font_scale, insets_mm, default_pt, default_color },
                );
                shape.fill = fill;
                shape.line = line;
                shape.geometry = geometry_of(sp_pr);
                out.push(shape);
            }
            "pic" => {
                let Some(geo) = xfrm_of(sp) else { continue };
                let geo = to_slide(stack, geo);
                let (r_id, crop) = blip_of(sp);
                let kind = match r_id.and_then(|r| ctx.image_for(&r, crop)) {
                    Some(id) => ShapeKind::Picture { id },
                    None => ShapeKind::Placeholder { label: "이미지".into() },
                };
                let mut shape = mm_shape(geo, kind);
                shape.line = sp.child("spPr").and_then(|p| inh.colors.line(p));
                shape.geometry = geometry_of(sp.child("spPr"));
                out.push(shape);
            }
            "graphicFrame" => {
                let Some(geo) = xfrm_of(sp) else { continue };
                let geo = to_slide(stack, geo);
                let kind = if let Some(tbl) = sp.find("tbl") {
                    ShapeKind::Table(table(tbl, Some(&inh.colors)))
                } else if sp.find("chart").is_some() {
                    ShapeKind::Placeholder { label: "차트".into() }
                } else if sp.find("relIds").is_some() {
                    ShapeKind::Placeholder { label: "SmartArt".into() }
                } else if sp.find("oleObj").is_some() {
                    ShapeKind::Placeholder { label: "포함된 개체".into() }
                } else {
                    continue;
                };
                out.push(mm_shape(geo, kind));
            }
            "cxnSp" => {
                let Some(geo) = xfrm_of(sp) else { continue };
                let geo = to_slide(stack, geo);
                let mut shape = mm_shape(geo, ShapeKind::Line);
                shape.line = sp.child("spPr").and_then(|p| inh.colors.line(p)).or(Some(("#000000".into(), 0.75)));
                out.push(shape);
            }
            "grpSp" => {
                let tx = GroupTx::of(sp);
                if let Some(t) = tx {
                    stack.push(t);
                }
                slide_shapes(ctx, sp, inh, stack, out);
                if tx.is_some() {
                    stack.pop();
                }
            }
            _ => {}
        }
    }
}

/// Solid background declared on a slide / layout / master `cSld`.
fn background_of(root: &Element, colors: &Colors) -> Option<String> {
    let bg = root.child("cSld")?.child("bg")?;
    if let Some(pr) = bg.child("bgPr") {
        return colors.fill(pr).flatten();
    }
    let bg_ref = bg.child("bgRef")?;
    bg_ref.elements().next().and_then(|c| colors.resolve(c))
}

/// The part a relationship of `kind` (`slideLayout`, `slideMaster`, `theme`, `notesSlide`) points to.
fn related_part(rels: &HashMap<String, String>, kind: &str) -> Option<String> {
    // Parts live in `slideLayouts/`, `slideMasters/`, `notesSlides/` — but `theme/` is singular.
    let plural = format!("/{kind}s/");
    let singular = format!("/{kind}/");
    rels.values().find(|v| v.contains(&plural) || v.contains(&singular)).cloned()
}

fn rels_of(archive: &mut Archive, part: &str) -> HashMap<String, String> {
    let dir = part.rsplit_once('/').map(|(d, _)| d).unwrap_or("ppt");
    let file_name = part.rsplit('/').next().unwrap_or_default();
    read_rels(archive, &format!("{dir}/_rels/{file_name}.rels"))
        .into_iter()
        .map(|(k, v)| (k, resolve_target(dir, &v)))
        .collect()
}

fn cached_part(
    archive: &mut Archive,
    cache: &mut HashMap<String, Option<Element>>,
    part: &str,
) -> Result<Option<Element>> {
    if let Some(e) = cache.get(part) {
        return Ok(e.clone());
    }
    let e = read_part(archive, part)?;
    cache.insert(part.to_string(), e.clone());
    Ok(e)
}

pub fn read(path: &Path) -> Result<DocModel> {
    let file = File::open(path).map_err(|e| CoreError::io(path, e))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| CoreError::Convert(format!("not a zip archive: {e}")))?;
    let presentation = read_part(&mut archive, "ppt/presentation.xml")?
        .ok_or_else(|| CoreError::Convert("ppt/presentation.xml missing (not a PPTX file)".into()))?;
    let mut model = DocModel::default();
    let mut slide_size_mm = (254.0f32, 190.5f32);
    if let Some(sz) = presentation.child("sldSz") {
        if let (Some(cx), Some(cy)) = (sz.attr_f32("cx"), sz.attr_f32("cy")) {
            if cx > 0.0 && cy > 0.0 {
                slide_size_mm = (emu_to_mm(cx), emu_to_mm(cy));
            }
        }
    }
    model.page_mm = Some(slide_size_mm);
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
        image_bytes: 0,
    };
    // Layouts / masters / themes are shared between slides: parse each once.
    let mut parts: HashMap<String, Option<Element>> = HashMap::new();
    let mut title: Option<String> = None;
    let mut slide_no = 0usize;
    for part in slide_parts {
        let Some(slide) = read_part(ctx.archive, &part)? else { continue };
        slide_no += 1;
        ctx.rels = rels_of(ctx.archive, &part);

        // -- inheritance chain: slide → layout → master → theme
        let layout_part = related_part(&ctx.rels, "slideLayout");
        let mut layout_rels = HashMap::new();
        let layout = match &layout_part {
            Some(p) => {
                layout_rels = rels_of(ctx.archive, p);
                cached_part(ctx.archive, &mut parts, p)?
            }
            None => None,
        };
        let master_part = related_part(&layout_rels, "slideMaster");
        let mut master_rels = HashMap::new();
        let master = match &master_part {
            Some(p) => {
                master_rels = rels_of(ctx.archive, p);
                cached_part(ctx.archive, &mut parts, p)?
            }
            None => None,
        };
        let theme = match related_part(&master_rels, "theme") {
            Some(p) => cached_part(ctx.archive, &mut parts, &p)?,
            None => None,
        };
        let mut colors = Colors::from_theme(theme.as_ref());
        colors.set_map(master.as_ref().and_then(|m| m.child("clrMap")));
        if let Some(over) = slide.child("clrMapOvr").and_then(|o| o.child("overrideClrMapping")) {
            colors.set_map(Some(over));
        }
        let background = background_of(&slide, &colors)
            .or_else(|| layout.as_ref().and_then(|l| background_of(l, &colors)))
            .or_else(|| master.as_ref().and_then(|m| background_of(m, &colors)));
        fn tree_of(e: &Option<Element>) -> Option<&Element> {
            e.as_ref().and_then(|x| x.child("cSld")).and_then(|c| c.child("spTree"))
        }
        let inh = Inherited {
            layout: PhDefaults::from_tree(tree_of(&layout), &colors),
            master: PhDefaults::from_tree(tree_of(&master), &colors),
            styles: TextStyles::from_master(master.as_ref(), &colors),
            colors,
            background,
        };

        // -- reading-order view
        if slide_no > 1 {
            model.blocks.push(Block::PageBreak);
        }
        let start = model.blocks.len();
        let mut slide_title: Option<String> = None;
        let tree = slide.child("cSld").and_then(|c| c.child("spTree"));
        if let Some(tree) = tree {
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

        // -- positioned view
        let mut shapes = Vec::new();
        if let Some(tree) = tree {
            let mut stack = Vec::new();
            slide_shapes(&mut ctx, tree, &inh, &mut stack, &mut shapes);
        }

        // Speaker notes.
        let mut notes_paras = Vec::new();
        if let Some(np) = related_part(&ctx.rels, "notesSlide") {
            if let Some(notes) = read_part(ctx.archive, &np)? {
                notes_paras = notes_paragraphs(&notes);
                if !notes_paras.is_empty() {
                    model
                        .blocks
                        .push(Block::Paragraph(Paragraph { heading: Some(4), ..Paragraph::plain("발표자 노트") }));
                    model.blocks.extend(notes_paras.iter().cloned().map(Block::Paragraph));
                }
            }
        }
        model.slides.push(Slide {
            width_mm: slide_size_mm.0,
            height_mm: slide_size_mm.1,
            background: inh.background,
            shapes,
            notes: notes_paras,
        });
    }
    if slide_no == 0 {
        model.notes.push("슬라이드가 없습니다".into());
    }
    model.title = title;
    model.images = std::mem::take(&mut ctx.images);
    let mut notes = std::mem::take(&mut ctx.notes);
    notes.sort();
    notes.dedup();
    model.notes.extend(notes);
    Ok(model)
}

// ---------------------------------------------------------------------------
// Test fixture writer
// ---------------------------------------------------------------------------

/// Writes a two-slide 16:9 deck with a theme, a master (colour map, text styles) and a layout:
/// a title slide whose centred title inherits its geometry from the layout, then a slide with a
/// bulleted body placeholder, a free text box placed above it, a 2×2 table, a picture cropped to its
/// right half, a group holding a theme-coloured rounded rectangle, a connector and speaker notes.
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
    let rel_ns = r#"xmlns="http://schemas.openxmlformats.org/package/2006/relationships""#;
    let rel = |id: &str, kind: &str, target: &str| {
        format!(
            r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#
        )
    };
    let rels = |items: &[String]| {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships {rel_ns}>{}</Relationships>"#,
            items.concat()
        )
    };
    w(&mut z, "[Content_Types].xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/><Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/><Override PartName="/ppt/notesSlides/notesSlide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/></Types>"#)?;
    w(&mut z, "_rels/.rels", rels(&[rel("rId1", "officeDocument", "ppt/presentation.xml")]).as_bytes())?;
    w(&mut z, "ppt/presentation.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId3"/></p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/></p:presentation>"#)?;
    w(
        &mut z,
        "ppt/_rels/presentation.xml.rels",
        rels(&[
            rel("rId1", "slideMaster", "slideMasters/slideMaster1.xml"),
            rel("rId2", "slide", "slides/slide1.xml"),
            rel("rId3", "slide", "slides/slide2.xml"),
        ])
        .as_bytes(),
    )?;
    let ns = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
    // Theme: dark text / light background, accent1 blue.
    w(&mut z, "ppt/theme/theme1.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office"><a:themeElements><a:clrScheme name="Office"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme></a:themeElements></a:theme>"#)?;
    // Master: colour map, white background, title / body text styles (44 pt / 28 pt), placeholder geometry.
    w(&mut z, "ppt/slideMasters/slideMaster1.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sldMaster {ns}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title Placeholder"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="838200" y="365125"/><a:ext cx="10515600" cy="1325563"/></a:xfrm></p:spPr><p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:r><a:t>마스터 제목</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Body Placeholder"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="838200" y="1825625"/><a:ext cx="10515600" cy="4351338"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>마스터 본문</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr><a:defRPr sz="4400"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:defRPr></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr><a:defRPr sz="2800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:defRPr></a:lvl1pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"/></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#).as_bytes())?;
    w(
        &mut z,
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        rels(&[
            rel("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"),
            rel("rId2", "theme", "../theme/theme1.xml"),
        ])
        .as_bytes(),
    )?;
    // Layout: a centred title (ctrTitle) at a layout-specific position and size, a subtitle with a lightened colour.
    w(&mut z, "ppt/slideLayouts/slideLayout1.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sldLayout {ns} type="title"><p:cSld name="Title Slide"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr/><p:nvPr><p:ph type="ctrTitle"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="1524000" y="1122363"/><a:ext cx="9144000" cy="2387600"/></a:xfrm></p:spPr><p:txBody><a:bodyPr anchor="b"/><a:lstStyle><a:lvl1pPr><a:defRPr sz="6000"/></a:lvl1pPr></a:lstStyle><a:p><a:r><a:t>제목</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Subtitle 2"/><p:cNvSpPr/><p:nvPr><p:ph type="subTitle" idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="1524000" y="3602038"/><a:ext cx="9144000" cy="1655762"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle><a:lvl1pPr><a:defRPr sz="2400"><a:solidFill><a:schemeClr val="tx1"><a:lumMod val="65000"/><a:lumOff val="35000"/></a:schemeClr></a:solidFill></a:defRPr></a:lvl1pPr></a:lstStyle><a:p><a:r><a:t>부제목</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sldLayout>"#).as_bytes())?;
    w(
        &mut z,
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        rels(&[rel("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")]).as_bytes(),
    )?;
    // Slide 1: the title has no xfrm (inherits the layout's), the subtitle has its own.
    w(&mut z, "ppt/slides/slide1.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld {ns}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="ctrTitle"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:pPr algn="ctr"/><a:r><a:rPr lang="ko-KR" b="1" sz="4400"/><a:t>2026 사업 계획</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Subtitle"/><p:cNvSpPr/><p:nvPr><p:ph type="subTitle" idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="1000000" y="3800000"/><a:ext cx="10000000" cy="800000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:rPr lang="ko-KR"/><a:t>경영기획팀</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#).as_bytes())?;
    w(
        &mut z,
        "ppt/slides/_rels/slide1.xml.rels",
        rels(&[rel("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")]).as_bytes(),
    )?;
    // Slide 2: body (autofit 85 %), free text, title, table, picture cropped to its right half, a group
    // (scaled ×0.5) with a rounded accent-coloured box, a connector and a slide-number placeholder.
    w(&mut z, "ppt/slides/slide2.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld {ns}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Body"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="800000" y="2500000"/><a:ext cx="5000000" cy="3000000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr><a:normAutofit fontScale="85000"/></a:bodyPr><a:p><a:r><a:rPr lang="ko-KR"/><a:t>매출 목표 12% 성장</a:t></a:r></a:p><a:p><a:pPr lvl="1"><a:buAutoNum type="arabicPeriod"/></a:pPr><a:r><a:rPr lang="ko-KR" i="1"/><a:t>신규 거래처 20곳</a:t></a:r><a:br/><a:r><a:t>기존 거래처 유지</a:t></a:r></a:p><a:p><a:pPr><a:buNone/></a:pPr><a:r><a:t>※ 환율 가정 1,350원</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Free text"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="800000" y="600000"/><a:ext cx="5000000" cy="600000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:rPr lang="ko-KR" u="sng"><a:solidFill><a:srgbClr val="C00000"/></a:solidFill></a:rPr><a:t>핵심 지표</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="4" name="Title 2"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="800000" y="1400000"/><a:ext cx="10000000" cy="800000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>목표</a:t></a:r></a:p></p:txBody></p:sp><p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="5" name="Table"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="6200000" y="2500000"/><a:ext cx="5000000" cy="1200000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="1"/><a:tblGrid><a:gridCol w="2500000"/><a:gridCol w="2500000"/></a:tblGrid><a:tr h="600000"><a:tc gridSpan="2"><a:txBody><a:bodyPr/><a:p><a:r><a:t>분기별 매출</a:t></a:r></a:p></a:txBody></a:tc><a:tc hMerge="1"><a:txBody><a:bodyPr/><a:p/></a:txBody></a:tc></a:tr><a:tr h="600000"><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>1분기</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>3.2억</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame><p:pic><p:nvPicPr><p:cNvPr id="6" name="Picture"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/><a:srcRect l="50000"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="6200000" y="4000000"/><a:ext cx="914400" cy="457200"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr></p:pic><p:grpSp><p:nvGrpSpPr><p:cNvPr id="8" name="Group"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="7000000" y="5000000"/><a:ext cx="2000000" cy="1000000"/><a:chOff x="0" y="0"/><a:chExt cx="4000000" cy="2000000"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="9" name="Box"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="400000" y="200000"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="roundRect"/><a:solidFill><a:schemeClr val="accent1"><a:lumMod val="75000"/></a:schemeClr></a:solidFill><a:ln w="19050"><a:solidFill><a:srgbClr val="1F3864"/></a:solidFill></a:ln></p:spPr><p:txBody><a:bodyPr anchor="ctr"/><a:p><a:pPr algn="ctr"/><a:r><a:rPr sz="1200"><a:solidFill><a:schemeClr val="bg1"/></a:solidFill></a:rPr><a:t>핵심</a:t></a:r></a:p></p:txBody></p:sp></p:grpSp><p:cxnSp><p:nvCxnSpPr><p:cNvPr id="10" name="Line"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm><a:off x="800000" y="5600000"/><a:ext cx="5000000" cy="0"/></a:xfrm><a:prstGeom prst="line"/><a:ln w="12700"><a:solidFill><a:srgbClr val="7F7F7F"/></a:solidFill></a:ln></p:spPr></p:cxnSp><p:sp><p:nvSpPr><p:cNvPr id="7" name="Slide Number"/><p:cNvSpPr/><p:nvPr><p:ph type="sldNum"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:fld id="x" type="slidenum"><a:t>2</a:t></a:fld></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#).as_bytes())?;
    w(
        &mut z,
        "ppt/slides/_rels/slide2.xml.rels",
        rels(&[
            rel("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"),
            rel("rId2", "image", "../media/image1.png"),
            rel("rId3", "notesSlide", "../notesSlides/notesSlide1.xml"),
        ])
        .as_bytes(),
    )?;
    w(&mut z, "ppt/notesSlides/notesSlide1.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:notes {ns}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide Image"/><p:cNvSpPr/><p:nvPr><p:ph type="sldImg"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes"/><p:cNvSpPr/><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>환율 가정은 기재부 전망 기준</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:notes>"#).as_bytes())?;
    w(&mut z, "ppt/media/image1.png", png)?;
    z.finish().map_err(|e| CoreError::Convert(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
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

    /// A 2 × 2 PNG (red | green over blue | white) so crops are observable.
    pub(crate) fn png_2x2() -> Vec<u8> {
        let mut img = image::RgbaImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
        img.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        img.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
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
        assert_eq!(s2[3].level, 1);
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

    #[test]
    fn positions_shapes_on_slides_with_inheritance_colours_and_crops() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deck.pptx");
        write_sample_pptx(&path, &png_2x2()).unwrap();
        let model = read(&path).unwrap();
        assert_eq!(model.slides.len(), 2);
        let s1 = &model.slides[0];
        assert!((s1.width_mm - 338.67).abs() < 0.1 && (s1.height_mm - 190.5).abs() < 0.1);
        assert_eq!(s1.background.as_deref(), Some("#FFFFFF"), "master bgRef bg1 → lt1 via the colour map");
        // The title has no geometry of its own: it inherits the layout's ctrTitle box and 60 pt default.
        let title = s1
            .shapes
            .iter()
            .find(|s| matches!(&s.kind, ShapeKind::Text { paragraphs, .. } if paragraphs.iter().any(|p| p.text() == "2026 사업 계획")))
            .expect("title shape");
        assert!((title.x_mm - emu_to_mm(1_524_000.0)).abs() < 0.01, "{title:?}");
        assert!((title.y_mm - emu_to_mm(1_122_363.0)).abs() < 0.01);
        match &title.kind {
            ShapeKind::Text { default_pt, anchor, .. } => {
                assert_eq!(*default_pt, 60.0);
                assert_eq!(*anchor, VAnchor::Top, "the slide's own bodyPr wins over the layout's anchor");
            }
            _ => unreachable!(),
        }
        // Subtitle: its own geometry, default colour from the layout list style (tx1 lightened).
        let sub = s1
            .shapes
            .iter()
            .find(|s| matches!(&s.kind, ShapeKind::Text { paragraphs, .. } if paragraphs[0].text() == "경영기획팀"))
            .unwrap();
        match &sub.kind {
            ShapeKind::Text { default_pt, default_color, .. } => {
                assert_eq!(*default_pt, 24.0);
                assert_eq!(default_color.as_deref(), Some("#595959"), "lumMod 65 % + lumOff 35 % of black");
            }
            _ => unreachable!(),
        }

        let s2 = &model.slides[1];
        // Z-order: body, free text, title, table, picture, group box, line (slide number skipped).
        let kinds: Vec<String> = s2.shapes.iter().map(|s| format!("{:?}", s.kind).chars().take(24).collect()).collect();
        assert_eq!(s2.shapes.len(), 7, "{kinds:?}");
        let body = &s2.shapes[0];
        match &body.kind {
            ShapeKind::Text { font_scale, default_pt, paragraphs, .. } => {
                assert!((font_scale - 0.85).abs() < 1e-6, "normAutofit fontScale");
                // PowerPoint inherits by placeholder idx first: idx 1 on this layout is the 24 pt subtitle.
                assert_eq!(*default_pt, 24.0, "layout placeholder idx 1");
                assert_eq!(paragraphs[1].level, 1);
            }
            _ => unreachable!(),
        }
        let free = &s2.shapes[1];
        match &free.kind {
            ShapeKind::Text { paragraphs, default_pt, .. } => {
                assert_eq!(paragraphs[0].runs[0].color.as_deref(), Some("#C00000"));
                assert_eq!(*default_pt, 18.0, "non-placeholder text uses otherStyle");
            }
            _ => unreachable!(),
        }
        assert!(matches!(&s2.shapes[3].kind, ShapeKind::Table(t) if t.rows.len() == 2));
        assert!((s2.shapes[3].x_mm - emu_to_mm(6_200_000.0)).abs() < 0.01);
        // The picture is cropped to its right half: 2 × 2 px → 1 × 2 px PNG (green over white).
        let pic = &s2.shapes[4];
        let ShapeKind::Picture { id } = &pic.kind else { panic!("picture expected, got {:?}", pic.kind) };
        let img = model.images.iter().find(|i| &i.id == id).unwrap();
        assert_eq!(img.mime, "image/png");
        let decoded = image::load_from_memory(&img.bytes).unwrap().to_rgba8();
        assert_eq!((decoded.width(), decoded.height()), (1, 2));
        assert_eq!(decoded.get_pixel(0, 0).0, [0, 255, 0, 255]);
        assert!((pic.w_mm - 25.4).abs() < 0.01 && (pic.h_mm - 12.7).abs() < 0.01);
        // The group scales its children by 0.5 and offsets them: child (400000, 200000) → (7200000, 5100000).
        let boxed = &s2.shapes[5];
        assert!((boxed.x_mm - emu_to_mm(7_200_000.0)).abs() < 0.01, "{boxed:?}");
        assert!((boxed.y_mm - emu_to_mm(5_100_000.0)).abs() < 0.01);
        assert!((boxed.w_mm - emu_to_mm(1_000_000.0)).abs() < 0.01);
        assert_eq!(boxed.geometry.as_deref(), Some("roundRect"));
        assert_eq!(boxed.fill.as_deref(), Some("#2F5597"), "accent1 4472C4 at 75 % luminance");
        assert_eq!(
            boxed.line.as_ref().map(|(c, w)| (c.as_str(), (*w * 100.0).round() / 100.0)),
            Some(("#1F3864", 1.5))
        );
        match &boxed.kind {
            ShapeKind::Text { paragraphs, anchor, .. } => {
                assert_eq!(*anchor, VAnchor::Middle);
                assert_eq!(paragraphs[0].runs[0].color.as_deref(), Some("#FFFFFF"), "bg1 → lt1");
            }
            _ => unreachable!(),
        }
        let line = &s2.shapes[6];
        assert!(matches!(line.kind, ShapeKind::Line));
        assert_eq!(line.line.as_ref().map(|(c, _)| c.as_str()), Some("#7F7F7F"));
        assert_eq!(s2.notes.len(), 1);
        assert_eq!(s2.notes[0].text(), "환율 가정은 기재부 전망 기준");
    }

    #[test]
    fn colour_modifiers_follow_powerpoint() {
        let parse = |xml_text: &str| xml::parse(xml_text.as_bytes()).unwrap();
        let colors = Colors::default();
        let el = parse(r#"<a:srgbClr xmlns:a="x" val="4472C4"><a:lumMod val="75000"/></a:srgbClr>"#);
        assert_eq!(colors.resolve(&el).as_deref(), Some("#2F5597"));
        let el =
            parse(r#"<a:srgbClr xmlns:a="x" val="000000"><a:lumMod val="65000"/><a:lumOff val="35000"/></a:srgbClr>"#);
        assert_eq!(colors.resolve(&el).as_deref(), Some("#595959"));
        let el = parse(r#"<a:srgbClr xmlns:a="x" val="FF0000"><a:tint val="50000"/></a:srgbClr>"#);
        assert_eq!(colors.resolve(&el).as_deref(), Some("#FF8080"));
        let el = parse(r#"<a:srgbClr xmlns:a="x" val="FF0000"><a:shade val="50000"/></a:srgbClr>"#);
        assert_eq!(colors.resolve(&el).as_deref(), Some("#800000"));
        let el = parse(r#"<a:prstClr xmlns:a="x" val="white"/>"#);
        assert_eq!(colors.resolve(&el).as_deref(), Some("#FFFFFF"));
    }

    #[test]
    fn crops_are_applied_to_pixels() {
        let png = recode_png(&png_2x2(), Some(Crop { l: 0.0, t: 0.5, r: 0.0, b: 0.0 })).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!((img.width(), img.height()), (2, 1));
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 255, 255], "bottom row kept");
        assert!(recode_png(b"not an image", None).is_none());
    }
}
