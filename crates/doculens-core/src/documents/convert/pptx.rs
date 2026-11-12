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
    Align, Block, Cell, CellStyle, DocModel, GeoPath, Gradient, ImageData, ListKind, Paragraph, Run, Shape, ShapeKind,
    Slide, Table, VAnchor,
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
    /// Widths of the theme's line styles (`fmtScheme/lnStyleLst`), points — what `lnRef idx` picks.
    line_widths_pt: Vec<f32>,
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
        let lines = theme
            .and_then(|t| t.child("themeElements"))
            .and_then(|t| t.child("fmtScheme"))
            .and_then(|f| f.child("lnStyleLst"));
        if let Some(lines) = lines {
            c.line_widths_pt =
                lines.children_named("ln").map(|l| l.attr_f32("w").map(emu_to_pt).unwrap_or(0.75)).collect();
        }
        c
    }

    /// The same colours with `phClr` standing for `color` — how a theme fill / a style reference is read.
    fn with_placeholder(&self, color: &str) -> Colors {
        let mut c = self.clone();
        c.scheme.insert("phClr".into(), color.to_string());
        c
    }

    /// Width of the theme's `idx`-th line style (1-based), points.
    fn line_width_pt(&self, idx: usize) -> f32 {
        idx.checked_sub(1).and_then(|i| self.line_widths_pt.get(i)).copied().unwrap_or(0.75)
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
        if let Some(pf) = parent.child("pattFill") {
            // A pattern is drawn as the colour it is laid on.
            return Some(pf.child("bgClr").and_then(|c| c.elements().next()).and_then(|c| self.resolve(c)));
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
    /// Vertical anchor of the placeholder's text (`bodyPr@anchor`), when it declares one.
    anchor: Option<VAnchor>,
    /// Alignment of its first-level paragraphs (`lstStyle/lvl1pPr@algn`), when it declares one.
    align: Option<Align>,
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
                anchor: sp.child("txBody").and_then(declared_anchor),
                align: sp
                    .child("txBody")
                    .and_then(|b| b.child("lstStyle"))
                    .and_then(|l| l.child("lvl1pPr"))
                    .and_then(declared_align),
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
    /// First-level alignment of the master's title and body styles (`lvl1pPr@algn`).
    title_align: Option<Align>,
    body_align: Option<Align>,
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
        let align = |name: &str| styles.child(name).and_then(|s| s.child("lvl1pPr")).and_then(declared_align);
        t.title_align = align("titleStyle");
        t.body_align = align("bodyStyle");
        t
    }

    /// The alignment a placeholder's paragraphs take when nothing nearer says otherwise.
    fn align_for(&self, fam: &str) -> Option<Align> {
        match fam {
            "title" => self.title_align,
            "body" | "subTitle" => self.body_align,
            _ => None,
        }
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

/// `algn` of a paragraph property element (`pPr`, `lvl1pPr`), when it declares one.
fn declared_align(pr: &Element) -> Option<Align> {
    match pr.attr("algn")? {
        "ctr" => Some(Align::Center),
        "r" => Some(Align::Right),
        "just" | "dist" => Some(Align::Justify),
        "l" => Some(Align::Left),
        _ => None,
    }
}

/// One `a:p` → paragraph. `bulleted_default` says whether the shape inherits bullets (body placeholders).
fn paragraph(p: &Element, bulleted_default: bool, colors: Option<&Colors>) -> Paragraph {
    let mut para = Paragraph::default();
    if let Some(pr) = p.child("pPr") {
        para.align = declared_align(pr).unwrap_or(Align::Left);
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

// ---------------------------------------------------------------------------
// Tables (1.0.0 plan §1-2: cell fills, borders, row heights and table styles)
// ---------------------------------------------------------------------------

/// The id of PowerPoint's default table style, "Medium Style 2 - Accent 1".
const DEFAULT_TABLE_STYLE_ID: &str = "{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}";

/// The definition of the default table style. A presentation names a built-in style by id and
/// carries no definition for it (`ppt/tableStyles.xml` lists only the styles that are not built in),
/// so the one nearly every table uses is kept here, in the form PowerPoint writes it: a header row
/// on the accent colour with white bold text, the body on a light tint of it in bands, white lines.
const DEFAULT_TABLE_STYLE: &str = r#"<a:tblStyle xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" styleId="{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}" styleName="Medium Style 2 - Accent 1"><a:wholeTbl><a:tcTxStyle><a:fontRef idx="minor"><a:prstClr val="black"/></a:fontRef><a:schemeClr val="dk1"/></a:tcTxStyle><a:tcStyle><a:tcBdr><a:left><a:ln w="12700" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:left><a:right><a:ln w="12700" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:right><a:top><a:ln w="12700" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:top><a:bottom><a:ln w="12700" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:bottom><a:insideH><a:ln w="12700" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:insideH><a:insideV><a:ln w="12700" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:insideV></a:tcBdr><a:fill><a:solidFill><a:schemeClr val="accent1"><a:tint val="20000"/></a:schemeClr></a:solidFill></a:fill></a:tcStyle></a:wholeTbl><a:band1H><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:schemeClr val="accent1"><a:tint val="40000"/></a:schemeClr></a:solidFill></a:fill></a:tcStyle></a:band1H><a:band2H><a:tcStyle><a:tcBdr/></a:tcStyle></a:band2H><a:band1V><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:schemeClr val="accent1"><a:tint val="40000"/></a:schemeClr></a:solidFill></a:fill></a:tcStyle></a:band1V><a:band2V><a:tcStyle><a:tcBdr/></a:tcStyle></a:band2V><a:lastCol><a:tcTxStyle b="on"><a:fontRef idx="minor"><a:prstClr val="black"/></a:fontRef><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></a:fill></a:tcStyle></a:lastCol><a:firstCol><a:tcTxStyle b="on"><a:fontRef idx="minor"><a:prstClr val="black"/></a:fontRef><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></a:fill></a:tcStyle></a:firstCol><a:lastRow><a:tcTxStyle b="on"><a:fontRef idx="minor"><a:prstClr val="black"/></a:fontRef><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:tcBdr><a:top><a:ln w="38100" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:top></a:tcBdr><a:fill><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></a:fill></a:tcStyle></a:lastRow><a:firstRow><a:tcTxStyle b="on"><a:fontRef idx="minor"><a:prstClr val="black"/></a:fontRef><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:tcBdr><a:bottom><a:ln w="38100" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:bottom></a:tcBdr><a:fill><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></a:fill></a:tcStyle></a:firstRow></a:tblStyle>"#;

/// The table styles of a presentation by id: the definitions in `ppt/tableStyles.xml` and the
/// built-in default. They stay XML — their colours are theme colours, resolved per slide.
#[derive(Debug, Default)]
struct TableStyles {
    defs: HashMap<String, Element>,
}

impl TableStyles {
    fn load(archive: &mut Archive) -> TableStyles {
        let mut defs = HashMap::new();
        if let Ok(builtin) = xml::parse(DEFAULT_TABLE_STYLE.as_bytes()) {
            defs.insert(DEFAULT_TABLE_STYLE_ID.to_string(), builtin);
        }
        if let Ok(Some(list)) = read_part(archive, "ppt/tableStyles.xml") {
            for style in list.children_named("tblStyle") {
                if let Some(id) = style.attr("styleId") {
                    defs.insert(id.to_string(), style.clone());
                }
            }
        }
        TableStyles { defs }
    }
}

/// A border as a table style or a cell declares it: `Some(None)` = explicitly no line.
type Border = Option<Option<(String, f32)>>;

/// What one part of a table style (`wholeTbl`, `firstRow`, `band1H` …) declares; `None` = left to
/// the parts below it.
#[derive(Debug, Clone, Default)]
struct TablePart {
    fill: Option<Option<String>>,
    /// left, right, top, bottom, insideH, insideV.
    borders: [Border; 6],
    color: Option<String>,
    bold: Option<bool>,
}

fn table_line(ln: &Element, colors: &Colors) -> Option<(String, f32)> {
    if ln.child("noFill").is_some() {
        return None;
    }
    let width = ln.attr_f32("w").map(emu_to_pt).unwrap_or(1.0).max(0.25);
    colors.fill(ln).flatten().map(|c| (c, width))
}

fn table_part(style: &Element, name: &str, colors: &Colors) -> Option<TablePart> {
    let part = style.child(name)?;
    let mut out = TablePart::default();
    if let Some(tx) = part.child("tcTxStyle") {
        out.bold = match tx.attr("b") {
            Some("on") => Some(true),
            Some("off") => Some(false),
            _ => None,
        };
        // The colour element sits next to `fontRef` (which carries a placeholder colour of its own).
        out.color = tx.elements().filter(|e| e.name != "fontRef").find_map(|e| colors.resolve(e));
    }
    if let Some(tc) = part.child("tcStyle") {
        out.fill = tc.child("fill").and_then(|f| colors.fill(f));
        if let Some(bdr) = tc.child("tcBdr") {
            for (i, side) in ["left", "right", "top", "bottom", "insideH", "insideV"].iter().enumerate() {
                if let Some(ln) = bdr.child(side).and_then(|b| b.child("ln")) {
                    out.borders[i] = Some(table_line(ln, colors));
                }
            }
        }
    }
    Some(out)
}

/// Where a cell sits in the region a style part covers (the whole table, one row, one column).
#[derive(Clone, Copy)]
struct Edges {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

/// Lays a style part over a cell: an outer border of the part goes to the cells on that edge of the
/// part's region, the inside borders to the edges between its cells.
fn overlay(style: &mut CellStyle, part: &TablePart, at: Edges) {
    if let Some(fill) = &part.fill {
        style.fill = fill.clone();
    }
    if let Some(color) = &part.color {
        style.color = Some(color.clone());
    }
    if let Some(bold) = part.bold {
        style.bold = bold;
    }
    let [left, right, top, bottom, inside_h, inside_v] = &part.borders;
    let sides = [
        if at.left { left } else { inside_v },
        if at.right { right } else { inside_v },
        if at.top { top } else { inside_h },
        if at.bottom { bottom } else { inside_h },
    ];
    for (slot, side) in style.borders.iter_mut().zip(sides) {
        if let Some(line) = side {
            *slot = line.clone();
        }
    }
}

/// `a:tbl` → table (`gridSpan` / `rowSpan` spans; `hMerge` / `vMerge` continuation cells dropped).
/// On a slide (`look` given) the cells also get their look: the table style named by
/// `tableStyleId` — whole table, bands, first / last column, first / last row, in that order —
/// and on top of it the cell's own `tcPr` (fill, `lnL` / `lnR` / `lnT` / `lnB`, anchor).
fn table(tbl: &Element, colors: Option<&Colors>, look: Option<&TableStyles>) -> Table {
    let pr = tbl.child("tblPr");
    let flag = |name: &str| pr.and_then(|p| p.attr_bool(name)).unwrap_or(false);
    let header_first = flag("firstRow");
    let col_widths_mm: Vec<f32> = tbl
        .child("tblGrid")
        .map(|g| g.children_named("gridCol").filter_map(|c| c.attr_f32("w")).map(emu_to_mm).collect())
        .unwrap_or_default();
    let style_el = match (look, colors) {
        (Some(styles), Some(_)) => pr
            .and_then(|p| p.child("tableStyleId"))
            .map(|e| e.text().trim().to_string())
            .and_then(|id| styles.defs.get(&id)),
        _ => None,
    };
    let part = |name: &str| match (style_el, colors) {
        (Some(st), Some(c)) => table_part(st, name, c),
        _ => None,
    };
    let whole = part("wholeTbl");
    let bands_h = [part("band1H"), part("band2H")];
    let bands_v = [part("band1V"), part("band2V")];
    let (first_row, last_row) = (part("firstRow"), part("lastRow"));
    let (first_col, last_col) = (part("firstCol"), part("lastCol"));
    let trs: Vec<&Element> = tbl.children_named("tr").collect();
    let n_rows = trs.len();
    let n_cols = col_widths_mm.len().max(trs.first().map_or(0, |tr| tr.children_named("tc").count()));
    let mut rows = Vec::new();
    let mut row_heights_mm = Vec::new();
    for (i, tr) in trs.iter().enumerate() {
        let mut row = Vec::new();
        for (c, tc) in tr.children_named("tc").enumerate() {
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
            let row_span = tc.attr_u32("rowSpan").unwrap_or(1).max(1).min(u16::MAX as u32) as u16;
            let col_span = tc.attr_u32("gridSpan").unwrap_or(1).max(1).min(u16::MAX as u32) as u16;
            // The cell's look: only on a slide, and only when a style or the cell itself says something.
            let mut style = None;
            if let (Some(colors), true) = (colors, look.is_some()) {
                let (last_r, last_c) = (i + row_span as usize >= n_rows, c + col_span as usize >= n_cols);
                let in_table = Edges { left: c == 0, right: last_c, top: i == 0, bottom: last_r };
                let in_row = Edges { top: true, bottom: true, ..in_table };
                let in_col = Edges { left: true, right: true, ..in_table };
                let is_first_row = flag("firstRow") && i == 0;
                let is_last_row = flag("lastRow") && last_r && n_rows > 1;
                let is_first_col = flag("firstCol") && c == 0;
                let is_last_col = flag("lastCol") && last_c && n_cols > 1;
                let mut st = CellStyle::default();
                let mut styled = false;
                if let Some(p) = &whole {
                    overlay(&mut st, p, in_table);
                    styled = true;
                }
                if flag("bandRow") && !is_first_row && !is_last_row {
                    let band = (i - flag("firstRow") as usize) % 2;
                    if let Some(p) = &bands_h[band] {
                        overlay(&mut st, p, in_row);
                        styled = true;
                    }
                }
                if flag("bandCol") && !is_first_col && !is_last_col {
                    let band = (c - flag("firstCol") as usize) % 2;
                    if let Some(p) = &bands_v[band] {
                        overlay(&mut st, p, in_col);
                        styled = true;
                    }
                }
                for (on, p, at) in [
                    (is_first_col, &first_col, in_col),
                    (is_last_col, &last_col, in_col),
                    (is_first_row, &first_row, in_row),
                    (is_last_row, &last_row, in_row),
                ] {
                    if let (true, Some(p)) = (on, p) {
                        overlay(&mut st, p, at);
                        styled = true;
                    }
                }
                if let Some(tc_pr) = tc.child("tcPr") {
                    if let Some(fill) = colors.fill(tc_pr) {
                        st.fill = fill;
                        styled = true;
                    }
                    for (slot, name) in st.borders.iter_mut().zip(["lnL", "lnR", "lnT", "lnB"]) {
                        if let Some(ln) = tc_pr.child(name) {
                            *slot = table_line(ln, colors);
                            styled = true;
                        }
                    }
                    st.valign = match tc_pr.attr("anchor") {
                        Some("ctr") => Some("middle".to_string()),
                        Some("b") => Some("bottom".to_string()),
                        Some("t") => Some("top".to_string()),
                        _ => None,
                    };
                    styled |= st.valign.is_some();
                }
                if styled {
                    style = Some(Box::new(st));
                }
            }
            row.push(Cell { blocks, row_span, col_span, header: header_first && i == 0, style });
        }
        if !row.is_empty() {
            rows.push(row);
            row_heights_mm.push(tr.attr_f32("h").map(emu_to_mm).unwrap_or(0.0));
        }
    }
    if look.is_none() || row_heights_mm.iter().all(|h| *h <= 0.0) {
        row_heights_mm.clear();
    }
    Table { rows, col_widths_mm, row_heights_mm }
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
    /// Table styles of the presentation (1.0.0).
    table_styles: TableStyles,
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

    /// The size a stored picture has on a slide at its own scale, millimetres (PowerPoint lays a
    /// picture without resolution data out at 96 dpi).
    fn image_size_mm(&self, id: &str) -> Option<(f32, f32)> {
        let img = self.images.iter().find(|i| i.id == id)?;
        let (w, h) =
            image::ImageReader::new(Cursor::new(&img.bytes)).with_guessed_format().ok()?.into_dimensions().ok()?;
        Some((w as f32 * 25.4 / 96.0, h as f32 * 25.4 / 96.0))
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
                out.push(Block::Table(table(tbl, None, None)));
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

/// The vertical anchor a text body declares itself (`None`: left to the layout / master placeholder).
fn declared_anchor(body: &Element) -> Option<VAnchor> {
    match body.child("bodyPr").and_then(|p| p.attr("anchor")) {
        Some("ctr") => Some(VAnchor::Middle),
        Some("b") => Some(VAnchor::Bottom),
        Some("t") => Some(VAnchor::Top),
        _ => None,
    }
}

fn body_props(body: &Element) -> (VAnchor, [f32; 4], f32) {
    let pr = body.child("bodyPr");
    let anchor = declared_anchor(body).unwrap_or(VAnchor::Top);
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
        fill_gradient: None,
        line: None,
        geometry: None,
        paths: Vec::new(),
        kind,
    }
}

/// The outlines of a shape that is not a box (1.0.0): a custom geometry's own paths, or one of the common
/// presets as a polygon — in a 0–1000 square stretched over the shape. Empty: drawn as a box (with the
/// `geometry` hint for rounded ones).
fn paths_of(sp_pr: Option<&Element>, own: (f32, f32), on_slide: (f32, f32)) -> Vec<GeoPath> {
    let Some(pr) = sp_pr else { return Vec::new() };
    if let Some(cust) = pr.child("custGeom") {
        return custom_paths(cust, own).unwrap_or_default();
    }
    pr.child("prstGeom")
        .and_then(|g| preset_path(g, on_slide))
        .map(|d| vec![GeoPath { d, ..Default::default() }])
        .unwrap_or_default()
}

/// `a:custGeom/pathLst` as SVG path data. `None` when a coordinate is a guide name instead of a number
/// (a formula-driven shape) or a command is unknown — the caller falls back to the box.
fn custom_paths(cust: &Element, (w, h): (f32, f32)) -> Option<Vec<GeoPath>> {
    use std::fmt::Write as _;
    let mut out = Vec::new();
    for path in cust.child("pathLst")?.children_named("path") {
        // The path's own coordinate space; without one, the shape's size in EMU.
        let pw = path.attr_f32("w").filter(|v| *v > 0.0).unwrap_or(w);
        let ph = path.attr_f32("h").filter(|v| *v > 0.0).unwrap_or(h);
        if pw <= 0.0 || ph <= 0.0 {
            return None;
        }
        let number = |e: &Element, name: &str| e.attr(name)?.trim().parse::<f32>().ok().filter(|v| v.is_finite());
        let mut d = String::new();
        let mut cur = (0.0f32, 0.0f32);
        for cmd in path.elements() {
            let pts: Vec<(f32, f32)> = cmd
                .children_named("pt")
                .map(|pt| Some((number(pt, "x")? * 1000.0 / pw, number(pt, "y")? * 1000.0 / ph)))
                .collect::<Option<_>>()?;
            match (cmd.name.as_str(), pts.as_slice()) {
                ("moveTo", [p]) => {
                    let _ = write!(d, "M{:.1} {:.1}", p.0, p.1);
                    cur = *p;
                }
                ("lnTo", [p]) => {
                    let _ = write!(d, "L{:.1} {:.1}", p.0, p.1);
                    cur = *p;
                }
                ("cubicBezTo", [a, b, c]) => {
                    let _ = write!(d, "C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}", a.0, a.1, b.0, b.1, c.0, c.1);
                    cur = *c;
                }
                ("quadBezTo", [a, b]) => {
                    let _ = write!(d, "Q{:.1} {:.1} {:.1} {:.1}", a.0, a.1, b.0, b.1);
                    cur = *b;
                }
                ("arcTo", []) => {
                    // From the current point along an ellipse of radii wR / hR: `stAng` is where the
                    // current point sits on it, `swAng` how far the arc turns (clockwise when positive).
                    let (rx, ry) = (number(cmd, "wR")? * 1000.0 / pw, number(cmd, "hR")? * 1000.0 / ph);
                    let (start, sweep) = (number(cmd, "stAng")? / 60_000.0, number(cmd, "swAng")? / 60_000.0);
                    let center = (cur.0 - rx * start.to_radians().cos(), cur.1 - ry * start.to_radians().sin());
                    // In pieces of at most a half turn, so that a full circle is drawn too.
                    let pieces = (sweep.abs() / 180.0).ceil().max(1.0);
                    for i in 1..=pieces as u32 {
                        let angle = (start + sweep * i as f32 / pieces).to_radians();
                        let end = (center.0 + rx * angle.cos(), center.1 + ry * angle.sin());
                        let _ = write!(
                            d,
                            "A{:.1} {:.1} 0 0 {} {:.1} {:.1}",
                            rx.abs(),
                            ry.abs(),
                            u8::from(sweep > 0.0),
                            end.0,
                            end.1
                        );
                        cur = end;
                    }
                }
                ("close", _) => d.push('Z'),
                _ => return None,
            }
        }
        if !d.is_empty() {
            out.push(GeoPath {
                d,
                no_fill: path.attr("fill") == Some("none"),
                no_stroke: matches!(path.attr("stroke"), Some("0") | Some("false")),
            });
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The common preset shapes that are polygons (triangles, diamond, parallelogram, trapezoid, hexagon,
/// pentagon arrow, chevron, block arrows), with their adjust values; `None` for every other preset.
fn preset_path(geom: &Element, (w, h): (f32, f32)) -> Option<String> {
    use std::fmt::Write as _;
    let prst = geom.attr("prst")?;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    // `avLst/gd name="adj" fmla="val 25000"`, in 1/100000.
    let adj = |name: &str, default: f32| {
        geom.child("avLst")
            .and_then(|l| l.children_named("gd").find(|g| g.attr("name") == Some(name)))
            .and_then(|g| g.attr("fmla"))
            .and_then(|f| f.trim().strip_prefix("val"))
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(default)
            / 100_000.0
    };
    // Lengths the presets measure against the shorter side, as x / y in the 0–1000 square.
    let short = w.min(h);
    let sx = |f: f32| (short * f / w * 1000.0).clamp(0.0, 1000.0);
    let sy = |f: f32| (short * f / h * 1000.0).clamp(0.0, 1000.0);
    let pts: Vec<(f32, f32)> = match prst {
        "triangle" => vec![((adj("adj", 50_000.0) * 1000.0).clamp(0.0, 1000.0), 0.0), (1000.0, 1000.0), (0.0, 1000.0)],
        "rtTriangle" => vec![(0.0, 0.0), (1000.0, 1000.0), (0.0, 1000.0)],
        "diamond" | "flowChartDecision" => vec![(500.0, 0.0), (1000.0, 500.0), (500.0, 1000.0), (0.0, 500.0)],
        "parallelogram" | "flowChartInputOutput" => {
            let dx = sx(adj("adj", 25_000.0));
            vec![(dx, 0.0), (1000.0, 0.0), (1000.0 - dx, 1000.0), (0.0, 1000.0)]
        }
        "trapezoid" => {
            let dx = sx(adj("adj", 25_000.0));
            vec![(dx, 0.0), (1000.0 - dx, 0.0), (1000.0, 1000.0), (0.0, 1000.0)]
        }
        "hexagon" => {
            let dx = sx(adj("adj", 25_000.0));
            vec![(dx, 0.0), (1000.0 - dx, 0.0), (1000.0, 500.0), (1000.0 - dx, 1000.0), (dx, 1000.0), (0.0, 500.0)]
        }
        "homePlate" => {
            let dx = sx(adj("adj", 50_000.0));
            vec![(0.0, 0.0), (1000.0 - dx, 0.0), (1000.0, 500.0), (1000.0 - dx, 1000.0), (0.0, 1000.0)]
        }
        "chevron" => {
            let dx = sx(adj("adj", 50_000.0));
            vec![(0.0, 0.0), (1000.0 - dx, 0.0), (1000.0, 500.0), (1000.0 - dx, 1000.0), (0.0, 1000.0), (dx, 500.0)]
        }
        "rightArrow" | "leftArrow" => {
            // adj1: thickness of the shaft (of the height), adj2: length of the head (of the shorter side).
            let (t, dx) = ((adj("adj1", 50_000.0) * 500.0).clamp(0.0, 500.0), sx(adj("adj2", 50_000.0)));
            let right = vec![
                (0.0, 500.0 - t),
                (1000.0 - dx, 500.0 - t),
                (1000.0 - dx, 0.0),
                (1000.0, 500.0),
                (1000.0 - dx, 1000.0),
                (1000.0 - dx, 500.0 + t),
                (0.0, 500.0 + t),
            ];
            if prst == "leftArrow" {
                right.into_iter().map(|(x, y)| (1000.0 - x, y)).collect()
            } else {
                right
            }
        }
        "downArrow" | "upArrow" => {
            let (t, dy) = ((adj("adj1", 50_000.0) * 500.0).clamp(0.0, 500.0), sy(adj("adj2", 50_000.0)));
            let down = vec![
                (500.0 - t, 0.0),
                (500.0 + t, 0.0),
                (500.0 + t, 1000.0 - dy),
                (1000.0, 1000.0 - dy),
                (500.0, 1000.0),
                (0.0, 1000.0 - dy),
                (500.0 - t, 1000.0 - dy),
            ];
            if prst == "upArrow" {
                down.into_iter().map(|(x, y)| (x, 1000.0 - y)).collect()
            } else {
                down
            }
        }
        _ => return None,
    };
    let mut d = String::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        let _ = write!(d, "{}{x:.1} {y:.1}", if i == 0 { 'M' } else { 'L' });
    }
    d.push('Z');
    Some(d)
}

/// What a shape's `p:style` gives where its `spPr` is silent: a shape drawn in PowerPoint keeps its fill
/// (accent colour), its outline and its text colour only there.
#[derive(Default)]
struct ShapeStyle {
    fill: Option<String>,
    line: Option<(String, f32)>,
    font: Option<String>,
}

fn shape_style(sp: &Element, colors: &Colors) -> ShapeStyle {
    let Some(style) = sp.child("style") else { return ShapeStyle::default() };
    // `idx="0"` = "none of the theme's styles"; the colour child is the style's `phClr`.
    let reference = |name: &str| {
        let r = style.child(name)?;
        let idx = r.attr("idx").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
        Some((idx, r.elements().next().and_then(|c| colors.resolve(c))))
    };
    ShapeStyle {
        fill: reference("fillRef").filter(|(idx, _)| *idx > 0).and_then(|(_, c)| c),
        line: reference("lnRef").filter(|(idx, _)| *idx > 0).and_then(|(idx, c)| Some((c?, colors.line_width_pt(idx)))),
        font: style.child("fontRef").and_then(|r| r.elements().next()).and_then(|c| colors.resolve(c)),
    }
}

/// Outline of a shape: `spPr/ln` when it names a colour, else the style's line (`ln` may still give
/// the width); `noFill` — no line.
fn outline(colors: &Colors, sp_pr: Option<&Element>, style: &ShapeStyle) -> Option<(String, f32)> {
    let Some(ln) = sp_pr.and_then(|p| p.child("ln")) else { return style.line.clone() };
    if ln.child("noFill").is_some() {
        return None;
    }
    let width = ln.attr_f32("w").map(emu_to_pt);
    match colors.fill(ln).flatten() {
        Some(c) => Some((c, width.unwrap_or(0.75).max(0.25))),
        None => style.line.clone().map(|(c, w)| (c, width.unwrap_or(w).max(0.25))),
    }
}

/// The shapes of a shape tree, positioned on the slide. `decor`: the tree is a master's or a layout's —
/// its placeholders are templates for the slides, not something drawn, so only its own shapes count.
fn slide_shapes(
    ctx: &mut Ctx<'_>,
    tree: &Element,
    inh: &Inherited,
    stack: &mut Vec<GroupTx>,
    out: &mut Vec<Shape>,
    decor: bool,
) {
    for sp in tree.elements() {
        if decor && placeholder_of(sp).is_some() {
            continue;
        }
        match sp.name.as_str() {
            "sp" => {
                let ph = placeholder_of(sp);
                let kind = ph.as_ref().and_then(|p| p.kind.as_deref());
                if matches!(kind, Some("sldNum") | Some("dt") | Some("ftr")) {
                    continue;
                }
                // A placeholder takes what it does not declare from the layout's placeholder, and what that
                // one does not declare from the master's — property by property. A layout usually repeats
                // the title placeholder without a position: stopping at the layout dropped the title (and
                // any body placed by the master) from the slide.
                let from_layout = ph.as_ref().and_then(|p| inh.layout.lookup(p));
                let from_master = ph.as_ref().and_then(|p| inh.master.lookup(p));
                let Some(geo) = xfrm_of(sp)
                    .or_else(|| from_layout.and_then(|e| e.xfrm))
                    .or_else(|| from_master.and_then(|e| e.xfrm))
                else {
                    continue;
                };
                let geo = to_slide(stack, geo);
                let sp_pr = sp.child("spPr");
                let style = shape_style(sp, &inh.colors);
                // A picture fill is drawn as a picture under the shape's text.
                let picture_fill = sp_pr.and_then(|p| p.child("blipFill")).is_some();
                let picture = sp_pr.and_then(|p| {
                    let (r_id, crop) = blip_of(p);
                    r_id.and_then(|r| ctx.image_for(&r, crop))
                });
                let fill = match sp_pr.and_then(|p| inh.colors.fill(p)) {
                    Some(declared) => declared,
                    None if picture_fill => None,
                    None => style.fill.clone(),
                };
                let fill_gradient = sp_pr.and_then(|p| p.child("gradFill")).and_then(|g| gradient_of(g, &inh.colors));
                let line = outline(&inh.colors, sp_pr, &style);
                let fam = ph_family(kind);
                let body = sp.child("txBody");
                // Alignment the paragraphs inherit: the shape's own list style, the layout's placeholder,
                // the master's, then the master's text style (a title is centred there, not on the slide).
                let inherited_align = body
                    .and_then(|b| b.child("lstStyle"))
                    .and_then(|l| l.child("lvl1pPr"))
                    .and_then(declared_align)
                    .or_else(|| from_layout.and_then(|e| e.align))
                    .or_else(|| from_master.and_then(|e| e.align))
                    .or_else(|| ph.as_ref().and_then(|_| inh.styles.align_for(fam)));
                let paragraphs: Vec<Paragraph> = body
                    .map(|b| {
                        let bulleted = ph.is_some() && fam == "body";
                        b.children_named("p")
                            .map(|p| {
                                let mut para = paragraph(p, bulleted, Some(&inh.colors));
                                let own = p.child("pPr").and_then(declared_align);
                                if let (None, Some(a)) = (own, inherited_align) {
                                    para.align = a;
                                }
                                para
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(id) = picture {
                    let mut under = mm_shape(geo, ShapeKind::Picture { id });
                    under.geometry = geometry_of(sp_pr);
                    out.push(under);
                }
                let has_text = paragraphs.iter().any(|p| !p.is_empty());
                if !has_text && fill.is_none() && line.is_none() {
                    continue;
                }
                let (_, insets_mm, font_scale) = body.map(body_props).unwrap_or((VAnchor::Top, DEFAULT_INSETS_MM, 1.0));
                let anchor = body
                    .and_then(declared_anchor)
                    .or_else(|| from_layout.and_then(|e| e.anchor))
                    .or_else(|| from_master.and_then(|e| e.anchor))
                    .unwrap_or(VAnchor::Top);
                let (style_pt, style_color) = inh.styles.for_family(fam, ph.is_some());
                let default_pt = from_layout
                    .and_then(|e| e.size_pt)
                    .or_else(|| from_master.and_then(|e| e.size_pt))
                    .unwrap_or(style_pt);
                let default_color = from_layout
                    .and_then(|e| e.color.clone())
                    .or_else(|| from_master.and_then(|e| e.color.clone()))
                    .or(style.font)
                    .or(style_color);
                let mut shape = mm_shape(
                    geo,
                    ShapeKind::Text { paragraphs, anchor, font_scale, insets_mm, default_pt, default_color },
                );
                shape.fill = fill;
                shape.fill_gradient = fill_gradient;
                shape.line = line;
                shape.geometry = geometry_of(sp_pr);
                // A custom geometry is laid out in its own shape's size, a preset by the size it has on the slide.
                let own = xfrm_of(sp).unwrap_or(geo);
                shape.paths = paths_of(sp_pr, (own.w, own.h), (geo.w, geo.h));
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
                shape.line = outline(&inh.colors, sp.child("spPr"), &shape_style(sp, &inh.colors));
                shape.geometry = geometry_of(sp.child("spPr"));
                out.push(shape);
            }
            "graphicFrame" => {
                let Some(geo) = xfrm_of(sp) else { continue };
                let geo = to_slide(stack, geo);
                let kind = if let Some(tbl) = sp.find("tbl") {
                    ShapeKind::Table(table(tbl, Some(&inh.colors), Some(&ctx.table_styles)))
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
                shape.line = outline(&inh.colors, sp.child("spPr"), &shape_style(sp, &inh.colors))
                    .or(Some(("#000000".into(), 0.75)));
                out.push(shape);
            }
            "grpSp" => {
                let tx = GroupTx::of(sp);
                if let Some(t) = tx {
                    stack.push(t);
                }
                slide_shapes(ctx, sp, inh, stack, out, decor);
                if tx.is_some() {
                    stack.pop();
                }
            }
            _ => {}
        }
    }
}

/// What the `cSld/bg` of a slide / layout / master declares (1.0.0 §1-1: pictures and gradients too).
enum Background {
    Solid(String),
    Gradient(Gradient),
    /// A picture: the relationship id in the declaring part's rels, its crop, and the scale of one
    /// tile when it is tiled instead of stretched over the slide.
    Picture {
        r_id: String,
        crop: Option<Crop>,
        tile: Option<(f32, f32)>,
    },
}

fn background_of(root: &Element, colors: &Colors, theme: Option<&Element>) -> Option<Background> {
    let bg = root.child("cSld")?.child("bg")?;
    if let Some(pr) = bg.child("bgPr") {
        if let (Some(r_id), crop) = blip_of(pr) {
            let scale =
                |t: &Element, name: &str| t.attr_f32(name).filter(|v| *v > 0.0).unwrap_or(100_000.0) / 100_000.0;
            let tile = pr.child("blipFill").and_then(|f| f.child("tile")).map(|t| (scale(t, "sx"), scale(t, "sy")));
            return Some(Background::Picture { r_id, crop, tile });
        }
        if let Some(gradient) = pr.child("gradFill").and_then(|g| gradient_of(g, colors)) {
            return Some(Background::Gradient(gradient));
        }
        return colors.fill(pr).flatten().map(Background::Solid);
    }
    // `bgRef idx="1001…"` picks one of the theme's background fills, where `phClr` stands for the colour
    // given here. A picture fill of the theme is left to that colour.
    let bg_ref = bg.child("bgRef")?;
    let color = bg_ref.elements().next().and_then(|c| colors.resolve(c))?;
    let idx = bg_ref.attr("idx").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
    let themed = idx
        .checked_sub(1001)
        .and_then(|i| theme?.child("themeElements")?.child("fmtScheme")?.child("bgFillStyleLst")?.elements().nth(i));
    if let Some(fill) = themed {
        let colors = colors.with_placeholder(&color);
        match fill.name.as_str() {
            "gradFill" => {
                if let Some(gradient) = gradient_of(fill, &colors) {
                    return Some(Background::Gradient(gradient));
                }
            }
            "solidFill" => {
                if let Some(c) = fill.elements().next().and_then(|c| colors.resolve(c)) {
                    return Some(Background::Solid(c));
                }
            }
            _ => {}
        }
    }
    Some(Background::Solid(color))
}

/// `a:gradFill` with at least two stops: linear along `lin@ang`, or — a path gradient — spreading from
/// the centre of its focus rectangle (the first stop sits there, the last at the edge).
fn gradient_of(grad: &Element, colors: &Colors) -> Option<Gradient> {
    let mut stops: Vec<(f32, String)> = grad
        .child("gsLst")?
        .children_named("gs")
        .filter_map(|gs| {
            let pos = gs.attr_f32("pos")? / 1000.0;
            let color = gs.elements().next().and_then(|c| colors.resolve(c))?;
            Some((pos.clamp(0.0, 100.0), color))
        })
        .collect();
    if stops.len() < 2 {
        return None;
    }
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    let center = grad.child("path").map(|path| {
        let rect = path.child("fillToRect");
        let side = |name: &str| rect.and_then(|r| r.attr_f32(name)).unwrap_or(0.0) / 1000.0;
        (
            ((100.0 + side("l") - side("r")) / 2.0).clamp(0.0, 100.0),
            ((100.0 + side("t") - side("b")) / 2.0).clamp(0.0, 100.0),
        )
    });
    let angle_deg = grad.child("lin").and_then(|l| l.attr_f32("ang")).unwrap_or(0.0) / 60_000.0;
    Some(Gradient { angle_deg, stops, center })
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
    let table_styles = TableStyles::load(&mut archive);
    let mut ctx = Ctx {
        archive: &mut archive,
        rels: HashMap::new(),
        images: Vec::new(),
        loaded: HashMap::new(),
        notes: Vec::new(),
        image_bytes: 0,
        table_styles,
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
        // A layout or the slide may swap the colour map (a dark slide in a light deck); what it shows of
        // the master is then drawn with that map too.
        for part in [layout.as_ref(), Some(&slide)] {
            if let Some(over) = part.and_then(|p| p.child("clrMapOvr")).and_then(|o| o.child("overrideClrMapping")) {
                colors.set_map(Some(over));
            }
        }
        // The background comes from the first of slide → layout → master that declares one; a picture
        // is looked up in the relationships of the part that declares it.
        let slide_rels = ctx.rels.clone();
        let declared = [(Some(&slide), &slide_rels), (layout.as_ref(), &layout_rels), (master.as_ref(), &master_rels)]
            .into_iter()
            .find_map(|(part, rels)| {
                part.and_then(|p| background_of(p, &colors, theme.as_ref())).map(|bg| (bg, rels.clone()))
            });
        let mut background = None;
        let mut background_gradient = None;
        let mut background_picture = None;
        let mut background_tile = None;
        match declared {
            Some((Background::Solid(c), _)) => background = Some(c),
            Some((Background::Gradient(g), _)) => {
                background = g.stops.first().map(|s| s.1.clone());
                background_gradient = Some(g);
            }
            Some((Background::Picture { r_id, crop, tile }, rels)) => {
                ctx.rels = rels;
                let id = ctx.image_for(&r_id, crop);
                ctx.rels = slide_rels.clone();
                // Tiled at the picture's own size times the tile scale; stretched over the slide otherwise
                // (and when the size cannot be read).
                let tile_mm = id
                    .as_deref()
                    .zip(tile)
                    .and_then(|(id, (sx, sy))| ctx.image_size_mm(id).map(|(w, h)| (w * sx, h * sy)));
                match (id, tile_mm) {
                    (Some(id), Some((w, h))) => background_tile = Some((id, w, h)),
                    (id, _) => background_picture = id,
                }
            }
            None => {}
        }
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

        // -- positioned view: the background picture, what the master and the layout draw themselves
        //    (logos, bands, fixed text — not their placeholders), then the slide's own shapes on top.
        let mut shapes = Vec::new();
        if let Some(id) = background_picture {
            shapes.push(Shape {
                x_mm: 0.0,
                y_mm: 0.0,
                w_mm: slide_size_mm.0,
                h_mm: slide_size_mm.1,
                rotation_deg: 0.0,
                flip_h: false,
                flip_v: false,
                fill: None,
                fill_gradient: None,
                line: None,
                geometry: None,
                paths: Vec::new(),
                kind: ShapeKind::Picture { id },
            });
        }
        // "배경 그래픽 숨기기": on the slide it hides the master's and the layout's shapes, on the layout
        // the master's.
        let shows = |part: Option<&Element>| part.and_then(|p| p.attr_bool("showMasterSp")).unwrap_or(true);
        if shows(Some(&slide)) {
            for (part, rels, shown) in [(&master, &master_rels, shows(layout.as_ref())), (&layout, &layout_rels, true)]
            {
                if let (Some(decor), true) = (tree_of(part), shown) {
                    ctx.rels = rels.clone();
                    slide_shapes(&mut ctx, decor, &inh, &mut Vec::new(), &mut shapes, true);
                }
            }
            ctx.rels = slide_rels.clone();
        }
        if let Some(tree) = tree {
            let mut stack = Vec::new();
            slide_shapes(&mut ctx, tree, &inh, &mut stack, &mut shapes, false);
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
            background_gradient,
            background_tile,
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

    /// A one-slide deck from raw parts (the parts every reader path needs are added around them).
    /// `master` = (the master's shapes, what follows its shape tree — `p:txStyles`).
    fn write_deck(path: &Path, slide: &str, layout: &str, master: (&str, &str), table_styles: Option<&str>) {
        Deck { slide, layout, master: master.0, master_tail: master.1, table_styles, ..Default::default() }.write(path);
    }

    /// The raw parts of a one-slide test deck. A `*_bg` goes before that part's shape tree (`p:bg`), a
    /// `*_attrs` onto its root element; `picture` is stored as `ppt/media/image1.png` and related as
    /// `rId9` from the slide, the layout and the master. The theme has three line widths (0.5 / 1 / 1.5 pt)
    /// and three background fills (`phClr`, `phClr` tinted, a top-to-bottom gradient `phClr` → black).
    #[derive(Default)]
    struct Deck<'a> {
        slide: &'a str,
        layout: &'a str,
        master: &'a str,
        master_tail: &'a str,
        slide_bg: &'a str,
        layout_bg: &'a str,
        master_bg: &'a str,
        slide_attrs: &'a str,
        layout_attrs: &'a str,
        table_styles: Option<&'a str>,
        picture: Option<Vec<u8>>,
    }

    impl Deck<'_> {
        fn write(&self, path: &Path) {
            use std::io::Write;
            use zip::write::SimpleFileOptions;
            let ns = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
            let rel = |id: &str, kind: &str, target: &str| {
                format!(
                    r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#
                )
            };
            let rels = |items: &[String]| {
                format!(
                    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}</Relationships>"#,
                    items.concat()
                )
            };
            // Every part may show the picture.
            let with_picture = |mut items: Vec<String>| {
                if self.picture.is_some() {
                    items.push(rel("rId9", "image", "../media/image1.png"));
                }
                rels(&items)
            };
            let tree = |bg: &str, inner: &str| {
                format!(
                    r#"<p:cSld>{bg}<p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{inner}</p:spTree></p:cSld>"#
                )
            };
            let mut parts: Vec<(&str, String)> = vec![
                ("_rels/.rels", rels(&[rel("rId1", "officeDocument", "ppt/presentation.xml")])),
                (
                    "ppt/presentation.xml",
                    format!(r#"<p:presentation {ns}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rId2"/></p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/></p:presentation>"#),
                ),
                (
                    "ppt/_rels/presentation.xml.rels",
                    rels(&[rel("rId1", "slideMaster", "slideMasters/slideMaster1.xml"), rel("rId2", "slide", "slides/slide1.xml")]),
                ),
                (
                    "ppt/slides/slide1.xml",
                    format!(r#"<p:sld {ns} {}>{}</p:sld>"#, self.slide_attrs, tree(self.slide_bg, self.slide)),
                ),
                (
                    "ppt/slides/_rels/slide1.xml.rels",
                    with_picture(vec![rel("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")]),
                ),
                (
                    "ppt/slideLayouts/slideLayout1.xml",
                    format!(r#"<p:sldLayout {ns} {}>{}</p:sldLayout>"#, self.layout_attrs, tree(self.layout_bg, self.layout)),
                ),
                (
                    "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
                    with_picture(vec![rel("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")]),
                ),
                (
                    "ppt/slideMasters/slideMaster1.xml",
                    format!(r#"<p:sldMaster {ns}>{}{}</p:sldMaster>"#, tree(self.master_bg, self.master), self.master_tail),
                ),
                (
                    "ppt/slideMasters/_rels/slideMaster1.xml.rels",
                    with_picture(vec![rel("rId1", "theme", "../theme/theme1.xml")]),
                ),
                (
                    "ppt/theme/theme1.xml",
                    r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:clrScheme name="t"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4F81BD"/></a:accent1><a:accent2><a:srgbClr val="C0504D"/></a:accent2></a:clrScheme><a:fmtScheme name="t"><a:fillStyleLst/><a:lnStyleLst><a:ln w="6350"/><a:ln w="12700"/><a:ln w="19050"/></a:lnStyleLst><a:effectStyleLst/><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="50000"/></a:schemeClr></a:solidFill><a:gradFill rotWithShape="1"><a:gsLst><a:gs pos="0"><a:schemeClr val="phClr"/></a:gs><a:gs pos="100000"><a:srgbClr val="000000"/></a:gs></a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#.to_string(),
                ),
            ];
            if let Some(styles) = self.table_styles {
                parts.push(("ppt/tableStyles.xml", styles.to_string()));
            }
            let mut z = zip::ZipWriter::new(File::create(path).unwrap());
            for (name, data) in parts {
                z.start_file(name, SimpleFileOptions::default()).unwrap();
                z.write_all(data.as_bytes()).unwrap();
            }
            if let Some(bytes) = &self.picture {
                z.start_file("ppt/media/image1.png", SimpleFileOptions::default()).unwrap();
                z.write_all(bytes).unwrap();
            }
            z.finish().unwrap();
        }
    }

    /// A PNG of one colour, `w` × `h` pixels.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(w, h, image::Rgba([40, 90, 160, 255]))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    /// A shape at a fixed place: its `spPr` extras (fill, line), its `p:style`, its text.
    fn drawn(id: u32, sp_pr: &str, style: &str, text: &str) -> String {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="S{id}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="914400" y="{}"/><a:ext cx="1828800" cy="457200"/></a:xfrm>{sp_pr}</p:spPr>{style}<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"#,
            id * 600_000
        )
    }

    fn texts(slide: &Slide) -> Vec<String> {
        slide
            .shapes
            .iter()
            .filter_map(|s| match &s.kind {
                ShapeKind::Text { paragraphs, .. } => Some(paragraphs.iter().map(|p| p.text()).collect::<String>()),
                _ => None,
            })
            .collect()
    }

    const PICTURE_BG: &str = r#"<p:bg><p:bgPr><a:blipFill><a:blip r:embed="rId9"/><a:stretch><a:fillRect/></a:stretch></a:blipFill></p:bgPr></p:bg>"#;

    /// 1.0.0 §1-1: the background is the first one declared going slide → layout → master, and it may be
    /// a picture (stretched or tiled) or a gradient (linear, from a point, or one of the theme's).
    #[test]
    fn a_slide_background_may_be_a_picture_or_a_gradient() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bg.pptx");
        let title = drawn(2, "", "", "제목");

        // The master's picture, stretched: the first thing drawn, over the whole slide.
        Deck { slide: &title, master_bg: PICTURE_BG, picture: Some(png(64, 32)), ..Default::default() }.write(&path);
        let model = read(&path).unwrap();
        let slide = &model.slides[0];
        assert_eq!((slide.background.as_deref(), &slide.background_gradient), (None, &None));
        let first = &slide.shapes[0];
        assert!(matches!(&first.kind, ShapeKind::Picture { id } if model.images.iter().any(|i| &i.id == id)));
        assert_eq!((first.x_mm, first.y_mm), (0.0, 0.0));
        assert!((first.w_mm - slide.width_mm).abs() < 0.01 && (first.h_mm - slide.height_mm).abs() < 0.01);
        assert_eq!(texts(slide), ["제목"]);

        // The slide's own gradient wins over the master's picture.
        let gradient = r#"<p:bg><p:bgPr><a:gradFill><a:gsLst><a:gs pos="100000"><a:srgbClr val="BDD7EE"/></a:gs><a:gs pos="0"><a:srgbClr val="1F3864"/></a:gs></a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill></p:bgPr></p:bg>"#;
        Deck {
            slide: &title,
            slide_bg: gradient,
            master_bg: PICTURE_BG,
            picture: Some(png(64, 32)),
            ..Default::default()
        }
        .write(&path);
        let model = read(&path).unwrap();
        let slide = &model.slides[0];
        let expected =
            Gradient { angle_deg: 90.0, stops: vec![(0.0, "#1F3864".into()), (100.0, "#BDD7EE".into())], center: None };
        assert_eq!(slide.background_gradient.as_ref(), Some(&expected));
        assert_eq!(
            slide.background.as_deref(),
            Some("#1F3864"),
            "the first stop stands in where gradients are not drawn"
        );
        assert!(!slide.shapes.iter().any(|s| matches!(s.kind, ShapeKind::Picture { .. })));
        let html = crate::documents::convert::html::to_html(&model);
        assert!(html.contains("background:linear-gradient(180.0deg, #1F3864 0.0%, #BDD7EE 100.0%);"), "{html}");

        // A gradient spreading from the centre.
        let radial = r#"<p:bg><p:bgPr><a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="FFFFFF"/></a:gs><a:gs pos="100000"><a:srgbClr val="548235"/></a:gs></a:gsLst><a:path path="circle"><a:fillToRect l="50000" t="50000" r="50000" b="50000"/></a:path></a:gradFill></p:bgPr></p:bg>"#;
        Deck { slide: &title, layout_bg: radial, ..Default::default() }.write(&path);
        let model = read(&path).unwrap();
        assert_eq!(model.slides[0].background_gradient.as_ref().and_then(|g| g.center), Some((50.0, 50.0)));
        let html = crate::documents::convert::html::to_html(&model);
        assert!(
            html.contains("background:radial-gradient(farthest-corner at 50.0% 50.0%, #FFFFFF 0.0%, #548235 100.0%);")
        );

        // The theme's third background fill, with the referenced colour as its `phClr`.
        let themed = r#"<p:bg><p:bgRef idx="1003"><a:schemeClr val="accent1"/></p:bgRef></p:bg>"#;
        Deck { slide: &title, master_bg: themed, ..Default::default() }.write(&path);
        let model = read(&path).unwrap();
        let expected =
            Gradient { angle_deg: 90.0, stops: vec![(0.0, "#4F81BD".into()), (100.0, "#000000".into())], center: None };
        assert_eq!(model.slides[0].background_gradient.as_ref(), Some(&expected));
        // … and its first: the colour itself.
        let plain = r#"<p:bg><p:bgRef idx="1001"><a:schemeClr val="accent2"/></p:bgRef></p:bg>"#;
        Deck { slide: &title, master_bg: plain, ..Default::default() }.write(&path);
        let model = read(&path).unwrap();
        assert_eq!(
            (model.slides[0].background.as_deref(), &model.slides[0].background_gradient),
            (Some("#C0504D"), &None)
        );

        // A tiled picture: one tile is the picture at 96 dpi times the tile's scale.
        let tiled = r#"<p:bg><p:bgPr><a:blipFill><a:blip r:embed="rId9"/><a:tile tx="0" ty="0" sx="50000" sy="100000" flip="none" algn="tl"/></a:blipFill></p:bgPr></p:bg>"#;
        Deck { slide: &title, slide_bg: tiled, picture: Some(png(64, 32)), ..Default::default() }.write(&path);
        let model = read(&path).unwrap();
        let slide = &model.slides[0];
        let (id, w, h) = slide.background_tile.clone().expect("tiled");
        assert!((w - 64.0 * 25.4 / 96.0 * 0.5).abs() < 0.01 && (h - 32.0 * 25.4 / 96.0).abs() < 0.01, "{w} x {h}");
        assert!(!slide.shapes.iter().any(|s| matches!(s.kind, ShapeKind::Picture { .. })), "not stretched as well");
        let html = crate::documents::convert::html::to_html(&model);
        assert!(
            html.contains(&format!(
                "background-image:url('img/{id}');background-size:8.47mm 8.47mm;background-repeat:repeat;"
            )),
            "{html}"
        );
    }

    /// 1.0.0 §1-1: what the master and the layout draw themselves (a band, a logo, fixed text) is on
    /// every slide under the slide's own shapes — their placeholders are not, and "hide background
    /// graphics" (`showMasterSp="0"`) takes them away.
    #[test]
    fn a_slide_shows_what_its_master_and_layout_draw() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("decor.pptx");
        let placeholder = |kind: &str, text: &str| {
            format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="P"/><p:cNvSpPr/><p:nvPr><p:ph type="{kind}"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="457200" y="274638"/><a:ext cx="8229600" cy="1143000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"#
            )
        };
        let band = r#"<a:solidFill><a:srgbClr val="C00000"/></a:solidFill>"#;
        let logo = r#"<p:pic><p:nvPicPr><p:cNvPr id="7" name="Logo"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId9"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="7315200" y="228600"/><a:ext cx="1463040" cy="487680"/></a:xfrm></p:spPr></p:pic>"#;
        let master =
            [placeholder("title", "마스터 제목 서식"), drawn(3, band, "", "사내 한정"), logo.to_string()].concat();
        let layout = [placeholder("title", "레이아웃 제목 서식"), drawn(4, "", "", "레이아웃 글")].concat();
        let slide = placeholder("title", "분기별 실적");
        let deck =
            Deck { slide: &slide, layout: &layout, master: &master, picture: Some(png(48, 16)), ..Default::default() };

        deck.write(&path);
        let model = read(&path).unwrap();
        let shapes = &model.slides[0].shapes;
        assert_eq!(
            texts(&model.slides[0]),
            ["사내 한정", "레이아웃 글", "분기별 실적"],
            "master, layout, then the slide"
        );
        assert_eq!(shapes[0].fill.as_deref(), Some("#C00000"));
        assert!(
            matches!(shapes[1].kind, ShapeKind::Picture { .. }),
            "the master's logo, from the master's relationships"
        );
        assert!((shapes[1].x_mm - emu_to_mm(7_315_200.0)).abs() < 0.01);
        // The reading-order view stays the slide's own content.
        let flow: String = model.blocks.iter().map(|b| format!("{b:?}")).collect();
        assert!(flow.contains("분기별 실적") && !flow.contains("사내 한정") && !flow.contains("레이아웃 글"));

        // Hidden on the slide: neither the layout's nor the master's.
        Deck { slide_attrs: r#"showMasterSp="0""#, ..deck }.write(&path);
        let model = read(&path).unwrap();
        assert_eq!(texts(&model.slides[0]), ["분기별 실적"]);
        assert_eq!(model.slides[0].shapes.len(), 1);

        // Hidden on the layout: the layout's own stay, the master's go.
        let deck =
            Deck { slide: &slide, layout: &layout, master: &master, picture: Some(png(48, 16)), ..Default::default() };
        Deck { layout_attrs: r#"showMasterSp="0""#, ..deck }.write(&path);
        let model = read(&path).unwrap();
        assert_eq!(texts(&model.slides[0]), ["레이아웃 글", "분기별 실적"]);
        assert!(!model.slides[0].shapes.iter().any(|s| matches!(s.kind, ShapeKind::Picture { .. })));
    }

    /// 1.0.0 §1-1: a shape drawn in PowerPoint names no colours in its `spPr` — fill, outline and text
    /// colour come from its `p:style`; what `spPr` does say wins (no fill, a gradient, a picture).
    #[test]
    fn a_drawn_shape_takes_its_colours_from_its_style() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("style.pptx");
        let style = r#"<p:style><a:lnRef idx="2"><a:schemeClr val="accent2"/></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></p:style>"#;
        let none = r#"<p:style><a:lnRef idx="0"><a:schemeClr val="accent2"/></a:lnRef><a:fillRef idx="0"><a:schemeClr val="accent1"/></a:fillRef><a:fontRef idx="minor"><a:schemeClr val="dk1"/></a:fontRef></p:style>"#;
        let gradient = r#"<a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="C00000"/></a:gs><a:gs pos="100000"><a:srgbClr val="FFC000"/></a:gs></a:gsLst><a:lin ang="0" scaled="0"/></a:gradFill>"#;
        let picture = r#"<a:blipFill><a:blip r:embed="rId9"/><a:stretch><a:fillRect/></a:stretch></a:blipFill>"#;
        let slide = [
            drawn(1, "", style, "스타일"),
            drawn(2, r#"<a:noFill/><a:ln><a:noFill/></a:ln>"#, style, "없음"),
            drawn(3, r#"<a:ln w="38100"/>"#, style, "선 굵기만"),
            drawn(4, gradient, style, "그라데이션"),
            drawn(5, picture, style, "그림"),
            drawn(6, "", none, "스타일 없음"),
        ]
        .concat();
        Deck { slide: &slide, picture: Some(png(48, 16)), ..Default::default() }.write(&path);
        let model = read(&path).unwrap();
        let slide = &model.slides[0];
        let shape = |text: &str| {
            slide
                .shapes
                .iter()
                .find(|s| matches!(&s.kind, ShapeKind::Text { paragraphs, .. } if paragraphs[0].text() == text))
                .unwrap_or_else(|| panic!("{text}"))
        };
        let text_color = |s: &Shape| match &s.kind {
            ShapeKind::Text { default_color, .. } => default_color.clone(),
            _ => None,
        };
        let styled = shape("스타일");
        assert_eq!(styled.fill.as_deref(), Some("#4F81BD"));
        assert_eq!(styled.line, Some(("#C0504D".to_string(), 1.0)), "the theme's second line width");
        assert_eq!(text_color(styled).as_deref(), Some("#FFFFFF"));
        assert_eq!((shape("없음").fill.as_deref(), &shape("없음").line), (None, &None));
        assert_eq!(
            shape("선 굵기만").line,
            Some(("#C0504D".to_string(), 3.0)),
            "the style's colour at the shape's width"
        );
        let g = shape("그라데이션");
        assert_eq!(g.fill.as_deref(), Some("#C00000"));
        assert_eq!(g.fill_gradient.as_ref().map(|g| (g.angle_deg, g.stops.len())), Some((0.0, 2)));
        // A picture fill: the picture, then the text over it without the style's fill.
        let at = slide.shapes.iter().position(|s| std::ptr::eq(s, shape("그림"))).unwrap();
        assert!(matches!(slide.shapes[at - 1].kind, ShapeKind::Picture { .. }));
        assert_eq!((slide.shapes[at - 1].x_mm, slide.shapes[at - 1].w_mm), (shape("그림").x_mm, shape("그림").w_mm));
        assert_eq!(shape("그림").fill, None);
        let bare = shape("스타일 없음");
        assert_eq!((bare.fill.as_deref(), &bare.line), (None, &None), "idx 0 = none of the theme's styles");
        assert_eq!(text_color(bare).as_deref(), Some("#000000"));
        let html = crate::documents::convert::html::to_html(&model);
        assert!(html.contains("background:linear-gradient(90.0deg, #C00000 0.0%, #FFC000 100.0%);"), "{html}");
    }

    /// 1.0.0 §1-1: a shape that is not a box is drawn by its outline — a custom geometry's own path
    /// (lines, curves, arcs) or a common preset as a polygon; a formula-driven custom geometry stays a box.
    #[test]
    fn shapes_that_are_not_boxes_keep_their_outline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outline.pptx");
        let fill = r#"<a:solidFill><a:srgbClr val="4F81BD"/></a:solidFill>"#;
        let custom = |points: &str| {
            format!(
                r#"<a:custGeom><a:avLst/><a:gdLst/><a:pathLst><a:path w="1000" h="1000">{points}</a:path></a:pathLst></a:custGeom>{fill}"#
            )
        };
        // A flat-bottomed arch: half a turn clockwise from the left end over the top.
        let arch = custom(
            r#"<a:moveTo><a:pt x="0" y="500"/></a:moveTo><a:arcTo wR="500" hR="500" stAng="10800000" swAng="10800000"/><a:lnTo><a:pt x="1000" y="1000"/></a:lnTo><a:lnTo><a:pt x="0" y="1000"/></a:lnTo><a:close/>"#,
        );
        let curve = custom(
            r#"<a:moveTo><a:pt x="0" y="600"/></a:moveTo><a:cubicBezTo><a:pt x="250" y="0"/><a:pt x="500" y="1000"/><a:pt x="1000" y="200"/></a:cubicBezTo><a:quadBezTo><a:pt x="1000" y="900"/><a:pt x="0" y="1000"/></a:quadBezTo><a:close/>"#,
        );
        let guided =
            custom(r#"<a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:lnTo><a:pt x="r" y="b"/></a:lnTo><a:close/>"#);
        let preset =
            |prst: &str, av: &str| format!(r#"<a:prstGeom prst="{prst}"><a:avLst>{av}</a:avLst></a:prstGeom>{fill}"#);
        let slide = [
            drawn(1, &arch, "", "아치"),
            drawn(2, &curve, "", "곡선"),
            drawn(3, &guided, "", "수식"),
            // 4 : 1 boxes — the point of a chevron is half the shorter side deep.
            drawn(4, &preset("chevron", ""), "", "단계"),
            drawn(5, &preset("rightArrow", r#"<a:gd name="adj2" fmla="val 100000"/>"#), "", "화살표")
                .replace("<a:xfrm>", r#"<a:xfrm flipH="1">"#),
            drawn(6, &preset("roundRect", ""), "", "둥근"),
        ]
        .concat();
        Deck { slide: &slide, ..Default::default() }.write(&path);
        let model = read(&path).unwrap();
        let shape = |text: &str| {
            model.slides[0]
                .shapes
                .iter()
                .find(|s| matches!(&s.kind, ShapeKind::Text { paragraphs, .. } if paragraphs[0].text() == text))
                .unwrap_or_else(|| panic!("{text}"))
        };
        let d = |text: &str| shape(text).paths.iter().map(|p| p.d.as_str()).collect::<Vec<_>>().join(" | ");
        assert_eq!(d("아치"), "M0.0 500.0A500.0 500.0 0 0 1 1000.0 500.0L1000.0 1000.0L0.0 1000.0Z");
        assert_eq!(d("곡선"), "M0.0 600.0C250.0 0.0 500.0 1000.0 1000.0 200.0Q1000.0 900.0 0.0 1000.0Z");
        assert!(shape("수식").paths.is_empty(), "a guide name is not a coordinate: drawn as a box");
        assert_eq!(d("단계"), "M0.0 0.0L875.0 0.0L1000.0 500.0L875.0 1000.0L0.0 1000.0L125.0 500.0Z");
        assert_eq!(d("화살표"), "M0.0 250.0L750.0 250.0L750.0 0.0L1000.0 500.0L750.0 1000.0L750.0 750.0L0.0 750.0Z");
        assert!(shape("둥근").paths.is_empty() && shape("둥근").geometry.as_deref() == Some("roundRect"));

        let html = crate::documents::convert::html::to_html(&model);
        assert!(html.contains(r##"<path d="M0.0 0.0L875.0 0.0L1000.0 500.0L875.0 1000.0L0.0 1000.0L125.0 500.0Z" fill="#4F81BD" stroke="none""##), "{html}");
        // The flip turns the outline; the text is not mirrored with it.
        assert!(html.contains(
            r#"<svg class="geo" viewBox="0 0 1000 1000" preserveAspectRatio="none" style="transform:scaleX(-1)">"#
        ));
        assert!(!html.contains("transform:scaleX(-1);transform-origin"), "{html}");
        // An outlined shape has no box behind it; a box (the formula-driven one, the rounded one) does.
        assert_eq!(html.matches("background:#4F81BD;").count(), 2, "{html}");
    }

    fn table_xml(style_id: &str, flags: &str, cells: &[&[&str]], first_cell_pr: &str) -> String {
        let cols = cells[0].len();
        let grid: String = (0..cols).map(|_| r#"<a:gridCol w="1800000"/>"#).collect();
        let mut rows = String::new();
        for (r, row) in cells.iter().enumerate() {
            rows.push_str(r#"<a:tr h="360000">"#);
            for (c, text) in row.iter().enumerate() {
                let pr = if r == 0 && c == 0 { first_cell_pr } else { "<a:tcPr/>" };
                rows.push_str(&format!(
                    r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></a:txBody>{pr}</a:tc>"#
                ));
            }
            rows.push_str("</a:tr>");
        }
        format!(
            r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="9" name="T"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="900000" y="1800000"/><a:ext cx="5400000" cy="1440000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr {flags}><a:tableStyleId>{style_id}</a:tableStyleId></a:tblPr><a:tblGrid>{grid}</a:tblGrid>{rows}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#
        )
    }

    fn slide_tables(model: &DocModel) -> Vec<&Table> {
        model.slides[0]
            .shapes
            .iter()
            .filter_map(|s| match &s.kind {
                ShapeKind::Table(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    /// 1.0.0 §1-2: a table keeps its look — the built-in default style, a style the file defines, the
    /// cell's own fill / border / anchor on top, row heights — and the page view keeps it in the HTML.
    #[test]
    fn slide_tables_keep_their_style_fills_and_borders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tables.pptx");
        let cells: &[&[&str]] = &[&["구분", "1분기", "2분기"], &["매출", "1,200", "1,450"], &["비용", "800", "910"]];
        let own = r#"<a:tcPr anchor="ctr"><a:lnB w="25400"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:lnB><a:solidFill><a:srgbClr val="FFE066"/></a:solidFill></a:tcPr>"#;
        let custom_id = "{11111111-2222-3333-4444-555555555555}";
        let custom = format!(
            r#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{DEFAULT_TABLE_STYLE_ID}"><a:tblStyle styleId="{custom_id}" styleName="Ours"><a:wholeTbl><a:tcTxStyle><a:schemeClr val="dk1"/></a:tcTxStyle><a:tcStyle><a:tcBdr><a:insideH><a:ln w="6350"><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></a:ln></a:insideH><a:left><a:ln><a:noFill/></a:ln></a:left></a:tcBdr><a:fill><a:noFill/></a:fill></a:tcStyle></a:wholeTbl><a:firstCol><a:tcTxStyle b="on"><a:schemeClr val="accent2"/></a:tcTxStyle><a:tcStyle><a:tcBdr/></a:tcStyle></a:firstCol></a:tblStyle></a:tblStyleLst>"#
        );
        let slide = [
            table_xml(DEFAULT_TABLE_STYLE_ID, r#"firstRow="1" bandRow="1""#, cells, own),
            table_xml(custom_id, r#"firstCol="1""#, cells, "<a:tcPr/>"),
            table_xml("{DEADBEEF-0000-0000-0000-000000000000}", r#"firstRow="1""#, cells, "<a:tcPr/>"),
        ]
        .concat();
        write_deck(&path, &slide, "", ("", ""), Some(&custom));
        let model = read(&path).unwrap();
        let tables = slide_tables(&model);
        assert_eq!(tables.len(), 3);
        let style = |t: &Table, r: usize, c: usize| t.rows[r][c].style.as_deref().cloned();
        let line = |color: &str, pt: f32| Some((color.to_string(), pt));

        // The default style: header on the accent colour in white bold with a 3 pt rule under it, the
        // body in two tints of it, 1 pt white lines; row heights as the file gives them.
        let t = tables[0];
        assert_eq!(t.row_heights_mm.len(), 3);
        assert!((t.row_heights_mm[0] - emu_to_mm(360_000.0)).abs() < 0.01);
        let head = style(t, 0, 1).unwrap();
        assert_eq!((head.fill.as_deref(), head.color.as_deref(), head.bold), (Some("#4F81BD"), Some("#FFFFFF"), true));
        assert_eq!(
            head.borders,
            [line("#FFFFFF", 1.0), line("#FFFFFF", 1.0), line("#FFFFFF", 1.0), line("#FFFFFF", 3.0)]
        );
        let (band1, band2) = (style(t, 1, 0).unwrap(), style(t, 2, 0).unwrap());
        assert_eq!((band1.fill.as_deref(), band2.fill.as_deref()), (Some("#B9CDE5"), Some("#DCE6F2")));
        assert!(!band1.bold && band1.color.as_deref() == Some("#000000"));
        // The cell's own properties lie on top of the style: fill, bottom line, anchor.
        let own = style(t, 0, 0).unwrap();
        assert_eq!(own.fill.as_deref(), Some("#FFE066"));
        assert_eq!(own.borders[3], line("#FF0000", 2.0));
        assert_eq!(own.valign.as_deref(), Some("middle"));
        assert!(own.bold, "still a header cell of the style");

        // A style the file defines: no fill, lines between the rows only, the first column emphasised.
        let t = tables[1];
        let first = style(t, 1, 0).unwrap();
        assert_eq!((first.fill.as_deref(), first.color.as_deref(), first.bold), (None, Some("#C0504D"), true));
        assert_eq!(first.borders, [None, None, line("#C0504D", 0.5), line("#C0504D", 0.5)]);
        let inner = style(t, 0, 1).unwrap();
        assert_eq!((inner.color.as_deref(), inner.bold), (Some("#000000"), false));
        assert_eq!(inner.borders, [None, None, None, line("#C0504D", 0.5)], "no line on the table's outer top edge");
        // A style nobody defines: the plain look, as before.
        assert!(tables[2].rows.iter().flatten().all(|c| c.style.is_none()));
        assert!(tables[2].rows[0][0].header);

        // The page view writes the look into the cells; the reading-order view stays plain.
        let html = super::super::html::to_html(&model);
        assert!(html.contains(r#"<tr style="height:33.33%">"#), "{}", &html[..html.len().min(400)]);
        assert!(html.contains("background:#4F81BD;color:#FFFFFF;font-weight:700;"));
        assert!(
            html.contains("background:#FFE066;")
                && html.contains("border-bottom:2.00pt solid #FF0000;vertical-align:middle;")
        );
        assert!(html.contains("background:transparent;color:#C0504D;font-weight:700;border-left:none;"));
        let flow: Vec<&Table> = model
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .collect();
        assert!(flow.iter().all(|t| t.row_heights_mm.is_empty() && t.rows.iter().flatten().all(|c| c.style.is_none())));
    }

    /// 1.0.0 §1: a placeholder takes what it leaves out from the layout's placeholder and, past that, from
    /// the master's — a layout that repeats the title without a position used to lose the title.
    #[test]
    fn a_placeholder_inherits_through_the_layout_to_the_master() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inherit.pptx");
        let ph = |extra: &str, text: &str| {
            format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>{extra}<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"#
            )
        };
        let slide = ph("<p:spPr/>", "분기별 실적");
        let layout = ph("<p:spPr/>", "레이아웃 제목");
        // The master places the title, anchors it in the middle; its title style centres it at 40 pt.
        let master_title = r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="457200" y="274638"/><a:ext cx="8229600" cy="1143000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:r><a:t>마스터 제목</a:t></a:r></a:p></p:txBody></p:sp>"#;
        let master_tail = r#"<p:txStyles><p:titleStyle><a:lvl1pPr algn="ctr"><a:defRPr sz="4000"/></a:lvl1pPr></p:titleStyle></p:txStyles>"#;
        write_deck(&path, &slide, &layout, (master_title, master_tail), None);
        let model = read(&path).unwrap();
        let title = model.slides[0]
            .shapes
            .iter()
            .find(|s| matches!(&s.kind, ShapeKind::Text { paragraphs, .. } if paragraphs[0].text() == "분기별 실적"))
            .expect("the title is on the slide");
        assert!((title.x_mm - emu_to_mm(457_200.0)).abs() < 0.01 && (title.y_mm - emu_to_mm(274_638.0)).abs() < 0.01);
        assert!((title.w_mm - emu_to_mm(8_229_600.0)).abs() < 0.01);
        match &title.kind {
            ShapeKind::Text { paragraphs, anchor, default_pt, .. } => {
                assert_eq!((*anchor, *default_pt), (VAnchor::Middle, 40.0));
                assert_eq!(paragraphs[0].align, Align::Center, "the master's title style centres it");
            }
            _ => unreachable!(),
        }
    }

    /// Diagnostic (ignored): `DOCULENS_PPTX_DUMP=<pptx> [DOCULENS_PPTX_DUMP_OUT=<html>]` converts a real
    /// presentation, prints what each slide holds and writes the HTML the PDF is printed from.
    #[test]
    #[ignore]
    fn dump_pptx() {
        let Some(path) = std::env::var_os("DOCULENS_PPTX_DUMP").map(std::path::PathBuf::from) else { return };
        let model = read(&path).unwrap();
        for (i, slide) in model.slides.iter().enumerate() {
            println!(
                "slide {}: background {:?} gradient {:?} tile {:?}, {} shape(s)",
                i + 1,
                slide.background,
                slide.background_gradient,
                slide.background_tile,
                slide.shapes.len()
            );
            for sh in &slide.shapes {
                match &sh.kind {
                    ShapeKind::Table(t) => {
                        println!("  table {}x{} heights {:?}", t.rows.len(), t.col_widths_mm.len(), t.row_heights_mm);
                        for row in &t.rows {
                            for c in row {
                                println!("    [{}] {:?}", c.text().trim(), c.style);
                            }
                        }
                    }
                    other => println!(
                        "  {:.0}x{:.0} mm at {:.0},{:.0} fill {:?}{} line {:?} {}",
                        sh.w_mm,
                        sh.h_mm,
                        sh.x_mm,
                        sh.y_mm,
                        sh.fill,
                        if sh.fill_gradient.is_some() { " (gradient)" } else { "" },
                        sh.line,
                        format!("{other:?}").chars().take(90).collect::<String>()
                    ),
                }
            }
        }
        // A directory: `index.html` with its pictures under `img/`, as the PDF printer gets them.
        if let Some(out) = std::env::var_os("DOCULENS_PPTX_DUMP_OUT") {
            crate::documents::convert::html::write_bundle(&model, Path::new(&out)).unwrap();
        }
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
                // The slide's `<a:bodyPr/>` declares no anchor: the layout placeholder's `anchor="b"` holds
                // (1.0.0 — an attribute a placeholder leaves out is inherited, not reset).
                assert_eq!(*anchor, VAnchor::Bottom);
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
