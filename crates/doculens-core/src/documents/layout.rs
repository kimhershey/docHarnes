//! Positioned text layer of a page (design §3.7 "직접 수정"; user decision 2026-11-09: text is
//! selected and corrected on the page image itself, the viewer no longer has a text tab).
//!
//! * Digital PDFs and converted office documents: PDFium character boxes → words → lines.
//! * Scanned pages / photos (OCR text) and pages whose text was replaced by hand: no character
//!   boxes exist, so the text lines are laid onto line bands estimated from the ink profile of the
//!   page image (`source = estimated`; the viewer shows a "위치 추정" badge).
//! * An edit replaces a word range: the page text is rewritten in place (markup such as OCR tables
//!   around the words survives) and the new words inherit the boxes of the replaced ones. Edited
//!   layouts persist in `pages.layout`; a computed layout is cached there too.
//!
//! Coordinates are fractions (0..1) of the rendered page image *after* the stored rotation, so one
//! layout serves every zoom level and DPI.

use image::{GrayImage, Luma};
use serde::{Deserialize, Serialize};

pub const LAYOUT_VERSION: u8 = 2;

/// Version 1 layouts numbered lines in PDF text order (table rows came out as many lines); their
/// rows are re-derived from the boxes on load.
const LAYOUT_VERSION_SEQUENTIAL_LINES: u8 = 1;

/// How the word boxes were obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutSource {
    /// PDFium character boxes of the PDF text layer.
    Layer,
    /// Line bands estimated from the page image with the text lines laid onto them.
    Estimated,
}

/// One word of the text layer; `x`/`y`/`w`/`h` are fractions of the page image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub t: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Reading-order line index (0-based).
    pub line: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub edited: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageLayout {
    pub version: u8,
    pub source: LayoutSource,
    pub words: Vec<Word>,
}

/// A character box reported by PDFium: PDF points, origin bottom-left, unrotated page.
#[derive(Debug, Clone, PartialEq)]
pub struct CharBox {
    pub ch: char,
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
    pub top: f32,
}

/// The displayed page box in PDF content space (points, origin bottom-left): character boxes are
/// normalised against it, so pages whose MediaBox does not start at (0, 0) or that carry an
/// intrinsic `/Rotate` still land on the rendered image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageBox {
    pub left: f32,
    pub bottom: f32,
    pub width: f32,
    pub height: f32,
}

impl PageBox {
    pub fn new(width: f32, height: f32) -> PageBox {
        PageBox { left: 0.0, bottom: 0.0, width, height }
    }
}

/// Smallest box a word may have (fractions): keeps tiny glyphs clickable.
const MIN_SIZE: f32 = 0.002;
/// Pixel luma below which a pixel counts as ink (same threshold as blank-page detection).
const INK_LUMA: u8 = 160;

impl PageLayout {
    pub fn empty(source: LayoutSource) -> PageLayout {
        PageLayout { version: LAYOUT_VERSION, source, words: Vec::new() }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    /// Parses a stored layout; a version-1 layout gets its visual rows re-derived (see [`assign_rows`]).
    pub fn from_json(json: &str) -> Option<PageLayout> {
        let mut layout = serde_json::from_str::<PageLayout>(json).ok()?;
        if layout.version == LAYOUT_VERSION_SEQUENTIAL_LINES {
            layout.relabel_rows();
            layout.version = LAYOUT_VERSION;
        }
        (layout.version == LAYOUT_VERSION).then_some(layout)
    }

    /// Re-derives the `line` of every word from the boxes (fractions, top-left origin) with the
    /// row grouping of [`from_chars`]; the word order is kept. The boxes are scaled to a nominal
    /// 1000-unit page so the half-point floor of the run geometry does not matter.
    pub fn relabel_rows(&mut self) {
        const SCALE: f32 = 1000.0;
        let runs: Vec<Run> = self
            .words
            .iter()
            .map(|w| Run {
                text: w.t.clone(),
                left: w.x * SCALE,
                right: (w.x + w.w) * SCALE,
                bottom: (1.0 - (w.y + w.h)) * SCALE,
                top: (1.0 - w.y) * SCALE,
            })
            .collect();
        for (w, line) in self.words.iter_mut().zip(assign_rows(&runs)) {
            w.line = line;
        }
    }

    /// The page text implied by the layout: words of a line joined by spaces, lines by newlines.
    pub fn text(&self) -> String {
        let mut out = String::new();
        let mut line: Option<u32> = None;
        for w in &self.words {
            match line {
                Some(l) if l == w.line => out.push(' '),
                Some(_) => out.push('\n'),
                None => {}
            }
            out.push_str(&w.t);
            line = Some(w.line);
        }
        out
    }

    /// Text of the words `start..end` (same-line words joined by spaces, lines by newlines).
    pub fn range_text(&self, start: usize, end: usize) -> String {
        let end = end.min(self.words.len());
        if start >= end {
            return String::new();
        }
        PageLayout { version: self.version, source: self.source, words: self.words[start..end].to_vec() }.text()
    }

    /// Rotates every box clockwise by `delta` degrees (multiples of 90; the page image was rotated
    /// the same way).
    pub fn rotate(&mut self, delta: u16) {
        let delta = delta % 360;
        if delta == 0 {
            return;
        }
        for w in &mut self.words {
            let (x, y, width, height) = rotate_box(w.x, w.y, w.w, w.h, delta);
            w.x = x;
            w.y = y;
            w.w = width;
            w.h = height;
        }
    }

    /// Bounding box of the words `start..end`, if any.
    pub fn bbox(&self, start: usize, end: usize) -> Option<(f32, f32, f32, f32)> {
        let end = end.min(self.words.len());
        if start >= end {
            return None;
        }
        let mut x0 = f32::MAX;
        let mut y0 = f32::MAX;
        let mut x1 = f32::MIN;
        let mut y1 = f32::MIN;
        for w in &self.words[start..end] {
            x0 = x0.min(w.x);
            y0 = y0.min(w.y);
            x1 = x1.max(w.x + w.w);
            y1 = y1.max(w.y + w.h);
        }
        Some((x0, y0, x1 - x0, y1 - y0))
    }
}

fn rotate_box(x: f32, y: f32, w: f32, h: f32, delta: u16) -> (f32, f32, f32, f32) {
    match delta % 360 {
        90 => (1.0 - (y + h), x, h, w),
        180 => (1.0 - (x + w), 1.0 - (y + h), w, h),
        270 => (y, 1.0 - (x + w), h, w),
        _ => (x, y, w, h),
    }
}

fn clamp01(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn finish_box(x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
    let w = w.max(MIN_SIZE);
    let h = h.max(MIN_SIZE);
    let x = clamp01(x).min(1.0 - w);
    let y = clamp01(y).min(1.0 - h);
    (x, y, w.min(1.0), h.min(1.0))
}

// ---------------------------------------------------------------------------
// PDF text layer → words
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Run {
    text: String,
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
}

impl Run {
    fn geo(&self) -> (f32, f32, f32, f32) {
        (self.left, self.right, self.bottom, self.top)
    }

    fn height(&self) -> f32 {
        (self.top - self.bottom).max(0.5)
    }

    fn centre(&self) -> f32 {
        (self.bottom + self.top) / 2.0
    }
}

/// Vertical overlap of two boxes given as `(left, right, bottom, top)`.
fn v_overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> f32 {
    (a.3.min(b.3) - a.2.max(b.2)).max(0.0)
}

/// `b` continues `a` on the next line of the same narrow column (a table cell wrapped onto two
/// lines): directly below it, within about a line, with overlapping horizontal extents.
fn stacked_below(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    let h = (a.3 - a.2).max(b.3 - b.2).max(0.5);
    (b.2 + b.3) < (a.2 + a.3)
        && b.3 <= a.2 + 0.5 * h
        && b.3 >= a.2 - 1.2 * h
        && b.0 < a.1 - 0.1 * h
        && b.1 > a.0 + 0.1 * h
}

/// Consecutive runs on one baseline within a word gap of each other (the words of one cell line,
/// or a stretch of a paragraph line).
#[derive(Debug, Clone, Copy)]
struct Segment {
    first: usize,
    last: usize, // inclusive
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
}

impl Segment {
    fn geo(&self) -> (f32, f32, f32, f32) {
        (self.left, self.right, self.bottom, self.top)
    }

    fn height(&self) -> f32 {
        (self.top - self.bottom).max(0.5)
    }
}

/// Longest wrapped cell the row grouping accepts; taller stacks are paragraphs / columns.
const MAX_CELL_LINES: usize = 4;

/// Visual rows of the runs, in run order. PDF text comes cell by cell, so a table row whose cells
/// wrap onto two or three lines arrives as "번 / 호 / 품목 / 수 / 량 …". Runs are first grouped into
/// segments (one baseline, word gaps); a chain of segments stacked below each other is one *cell*
/// when a neighbour sits beside the chain at a cell gap's distance (≥ 0.6 em, sharing its vertical
/// span), the chain is short and its last line does not simply continue to the right (which makes
/// it a paragraph line). Every segment that overlaps the row's vertical extent joins the row.
fn assign_rows(runs: &[Run]) -> Vec<u32> {
    let n = runs.len();
    if n == 0 {
        return Vec::new();
    }
    // 1. Segments.
    let mut segs: Vec<Segment> = Vec::new();
    for (i, r) in runs.iter().enumerate() {
        let joined = segs.last_mut().filter(|s| {
            let prev = &runs[s.last];
            let h = prev.height().min(r.height());
            v_overlap(prev.geo(), r.geo()) >= 0.5 * h
                && r.left >= prev.left
                && r.left - prev.right <= 1.0 * prev.height().max(r.height())
        });
        match joined {
            Some(s) => {
                s.last = i;
                s.left = s.left.min(r.left);
                s.right = s.right.max(r.right);
                s.bottom = s.bottom.min(r.bottom);
                s.top = s.top.max(r.top);
            }
            None => {
                segs.push(Segment { first: i, last: i, left: r.left, right: r.right, bottom: r.bottom, top: r.top })
            }
        }
    }
    let m = segs.len();
    // 2. Chains of consecutive segments stacked below each other; `chain[s]` is the chain's first segment.
    let mut chain = vec![usize::MAX; m];
    for s in 1..m {
        if stacked_below(segs[s - 1].geo(), segs[s].geo()) {
            if chain[s - 1] == usize::MAX {
                chain[s - 1] = s - 1;
            }
            chain[s] = chain[s - 1];
        }
    }
    // 3. Which chains are wrapped cells.
    let mut is_cell = vec![false; m];
    let mut span = vec![(0.0f32, 0.0f32); m]; // (bottom, top) of a chain, at its head
    let mut head = 0;
    while head < m {
        if chain[head] != head {
            head += 1;
            continue;
        }
        let members: Vec<usize> = (head..m).take_while(|&s| chain[s] == head).collect();
        let bottom = members.iter().map(|&s| segs[s].bottom).fold(f32::MAX, f32::min);
        let top = members.iter().map(|&s| segs[s].top).fold(f32::MIN, f32::max);
        let left = members.iter().map(|&s| segs[s].left).fold(f32::MAX, f32::min);
        let right = members.iter().map(|&s| segs[s].right).fold(f32::MIN, f32::max);
        let h = members.iter().map(|&s| segs[s].height()).fold(0.5, f32::max);
        span[head] = (bottom, top);
        let last = segs[*members.last().unwrap_or(&head)];
        // The run after the chain continuing the last line to the right makes it a paragraph line.
        let continues = runs.get(last.last + 1).is_some_and(|r| v_overlap(r.geo(), last.geo()) >= 0.6 * h);
        // The nearest runs on either side that share the chain's vertical span: a cell border
        // (padding + rule) keeps them at least 0.6 em away, a word space does not.
        let mut nearest_right = f32::MAX;
        let mut nearest_left = f32::MAX;
        let (first_run, last_run) = (segs[head].first, last.last);
        for (j, r) in runs.iter().enumerate() {
            if (first_run..=last_run).contains(&j) || r.centre() <= bottom - 0.25 * h || r.centre() >= top + 0.25 * h {
                continue;
            }
            if r.left >= right {
                nearest_right = nearest_right.min(r.left - right);
            } else if r.right <= left {
                nearest_left = nearest_left.min(left - r.right);
            }
        }
        let has_neighbour = nearest_right < f32::MAX || nearest_left < f32::MAX;
        is_cell[head] = members.len() <= MAX_CELL_LINES
            && !continues
            && has_neighbour
            && nearest_right >= 0.6 * h
            && nearest_left >= 0.6 * h;
        head += members.len();
    }
    // 4. Rows: a segment joins the current row when it continues a wrapped cell or overlaps the
    //    row's vertical extent (a cell's whole span for the first line of a wrapped cell); otherwise
    //    it starts the next row.
    let mut rows = vec![0u32; n];
    let mut row = 0u32;
    let (mut row_bottom, mut row_top) = (segs[0].bottom, segs[0].top);
    for (s, seg) in segs.iter().enumerate() {
        if s > 0 {
            let in_cell = chain[s] != usize::MAX && is_cell[chain[s]];
            let fragment = in_cell && chain[s] != s;
            let (bottom, top) = if in_cell && chain[s] == s { span[s] } else { (seg.bottom, seg.top) };
            let overlap = (top.min(row_top) - bottom.max(row_bottom)).max(0.0);
            if fragment || overlap >= 0.45 * seg.height() {
                row_bottom = row_bottom.min(seg.bottom);
                row_top = row_top.max(seg.top);
            } else {
                row += 1;
                row_bottom = seg.bottom;
                row_top = seg.top;
            }
        }
        for r in rows.iter_mut().take(seg.last + 1).skip(seg.first) {
            *r = row;
        }
    }
    rows
}

/// Builds the layout of a PDF page from its character boxes (content space, see [`PageBox`]).
/// `rotation` is the total clockwise rotation of the rendered image relative to that space: the page's
/// intrinsic `/Rotate` plus the user's stored rotation. Words keep the PDF text order (the order of
/// `pages.text`); their `line` is the visual row (see [`assign_rows`]).
pub fn from_chars(chars: &[CharBox], page_box: PageBox, rotation: u16) -> PageLayout {
    let mut layout = PageLayout::empty(LayoutSource::Layer);
    let (width_pt, height_pt) = (page_box.width, page_box.height);
    if !(width_pt > 0.0 && height_pt > 0.0) {
        return layout;
    }
    // 1. Runs of non-blank characters; a run also breaks on a horizontal jump (missing space
    //    character, next column) or a vertical shift (next line).
    let mut runs: Vec<Run> = Vec::new();
    let mut cur: Option<Run> = None;
    for c in chars {
        if c.ch.is_whitespace() || c.ch.is_control() {
            if let Some(r) = cur.take() {
                runs.push(r);
            }
            continue;
        }
        let (l, r) = if c.left <= c.right { (c.left, c.right) } else { (c.right, c.left) };
        let (b, t) = if c.bottom <= c.top { (c.bottom, c.top) } else { (c.top, c.bottom) };
        let degenerate = !(r > l && t > b);
        match cur.as_mut() {
            None => {
                cur = Some(Run { text: c.ch.to_string(), left: l, right: r, bottom: b, top: t });
            }
            Some(run) => {
                if degenerate {
                    run.text.push(c.ch);
                    continue;
                }
                let h = (run.top - run.bottom).max(t - b).max(0.5);
                let gap = l - run.right;
                let vshift = ((b + t) / 2.0 - (run.bottom + run.top) / 2.0).abs();
                if gap > 0.5 * h || gap < -2.0 * h || vshift > 0.5 * h {
                    runs.push(std::mem::replace(
                        run,
                        Run { text: c.ch.to_string(), left: l, right: r, bottom: b, top: t },
                    ));
                } else {
                    run.text.push(c.ch);
                    run.left = run.left.min(l);
                    run.right = run.right.max(r);
                    run.bottom = run.bottom.min(b);
                    run.top = run.top.max(t);
                }
            }
        }
    }
    if let Some(r) = cur.take() {
        runs.push(r);
    }
    // 2. Visual rows, then 3. fractions of the unrotated page box (top-left origin) and the rotation.
    let rows = assign_rows(&runs);
    for (r, line) in runs.iter().zip(rows) {
        let fx = (r.left - page_box.left) / width_pt;
        let fw = (r.right - r.left) / width_pt;
        let fy = 1.0 - (r.top - page_box.bottom) / height_pt;
        let fh = (r.top - r.bottom) / height_pt;
        let (x, y, w, h) = rotate_box(fx, fy, fw, fh, rotation);
        let (x, y, w, h) = finish_box(x, y, w, h);
        layout.words.push(Word { t: r.text.clone(), x, y, w, h, line, edited: false });
    }
    layout
}

// ---------------------------------------------------------------------------
// Estimated layout (OCR / manual text on a page image)
// ---------------------------------------------------------------------------

/// Display width of a word in "units" (CJK ≈ 1, Latin / digits ≈ 0.55).
fn units(word: &str) -> f32 {
    word.chars()
        .map(|c| match c as u32 {
            0xAC00..=0xD7A3 | 0x1100..=0x11FF | 0x3130..=0x318F => 1.0,
            0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x3000..=0x303F | 0xFF00..=0xFFEF => 1.0,
            0x3040..=0x30FF => 1.0,
            _ => 0.55,
        })
        .sum::<f32>()
        .max(0.55)
}

/// Lays `words` across `[x0, x0 + width]` at `(y, h)`, proportionally to their display width.
fn place_words(words: &[&str], x0: f32, width: f32, y: f32, h: f32, line: u32, edited: bool) -> Vec<Word> {
    if words.is_empty() {
        return Vec::new();
    }
    let gap = 0.5;
    let total: f32 = words.iter().map(|w| units(w)).sum::<f32>() + gap * (words.len() as f32 - 1.0);
    let scale = width.max(MIN_SIZE) / total.max(0.55);
    let mut x = x0;
    let mut out = Vec::with_capacity(words.len());
    for w in words {
        let ww = units(w) * scale;
        let (bx, by, bw, bh) = finish_box(x, y, ww, h);
        out.push(Word { t: (*w).to_string(), x: bx, y: by, w: bw, h: bh, line, edited });
        x += ww + gap * scale;
    }
    out
}

/// Cell separator emitted for `</td>` / `</th>` while the markup is reduced.
const CELL_SEP: char = '\u{1f}';

/// The visible text lines of a page text as cells: a table row keeps its cell boundaries (every
/// `<td>` / `<th>`), any other line is a single cell. OCR markup (`<table>`, `<br>`, `<stamp>` …)
/// is reduced to line breaks / cell separators and entities are decoded; empty cells and lines are
/// dropped.
pub fn line_cells(text: &str) -> Vec<Vec<String>> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let tag = after[..end].trim();
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        match name.as_str() {
            "br" | "tr" | "p" | "div" | "li" | "table" | "thead" | "tbody" | "h1" | "h2" | "h3" | "h4" | "h5"
            | "h6" => {
                out.push('\n');
            }
            "td" | "th" if closing => out.push(CELL_SEP),
            _ => {}
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    decoded
        .split('\n')
        .map(|l| {
            l.split(CELL_SEP)
                .map(|c| c.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|c| !c.is_empty())
                .collect::<Vec<String>>()
        })
        .filter(|cells| !cells.is_empty())
        .collect()
}

/// The visible text lines of a page text (cells of a table row joined by spaces); see [`line_cells`].
pub fn text_lines(text: &str) -> Vec<String> {
    line_cells(text).into_iter().map(|cells| cells.join(" ")).collect()
}

#[derive(Debug, Clone, Copy)]
struct Band {
    top: u32,
    bottom: u32, // exclusive
}

impl Band {
    fn height(&self) -> u32 {
        self.bottom.saturating_sub(self.top)
    }
}

/// Horizontal bands of rows that carry ink.
fn ink_bands(img: &GrayImage) -> Vec<Band> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let min_ink = ((w as f32) * 0.004).max(2.0) as u32;
    let mut inked = vec![false; h as usize];
    for (y, row) in img.rows().enumerate() {
        let count = row.filter(|p| p.0[0] < INK_LUMA).count() as u32;
        inked[y] = count >= min_ink;
    }
    let mut bands: Vec<Band> = Vec::new();
    let mut y = 0u32;
    while (y as usize) < inked.len() {
        if inked[y as usize] {
            let top = y;
            while (y as usize) < inked.len() && inked[y as usize] {
                y += 1;
            }
            bands.push(Band { top, bottom: y });
        } else {
            y += 1;
        }
    }
    // Merge bands separated by hairline gaps (descenders, broken strokes) and drop specks.
    let gap_max = (h / 400).max(1);
    let mut merged: Vec<Band> = Vec::new();
    for b in bands {
        match merged.last_mut() {
            Some(last) if b.top.saturating_sub(last.bottom) <= gap_max => last.bottom = b.bottom,
            _ => merged.push(b),
        }
    }
    let min_h = (h / 300).max(2);
    merged.retain(|b| b.height() >= min_h);
    merged
}

/// Horizontal extent of the ink inside `band` (columns), if any.
fn band_extent(img: &GrayImage, band: Band) -> Option<(u32, u32)> {
    let (w, _) = img.dimensions();
    let mut first = None;
    let mut last = None;
    for x in 0..w {
        let inked = (band.top..band.bottom).any(|y| img.get_pixel(x, y).0[0] < INK_LUMA);
        if inked {
            if first.is_none() {
                first = Some(x);
            }
            last = Some(x);
        }
    }
    match (first, last) {
        (Some(a), Some(b)) if b > a => Some((a, b + 1)),
        _ => None,
    }
}

/// The image without table rulings and other long straight strokes: horizontal ink runs longer
/// than a tenth of the width and vertical runs longer than a twentieth of the height are
/// whitened, so the rows of a ruled table separate into their own ink bands and the cell borders
/// do not stretch a band's extent.
fn without_rulings(img: &GrayImage) -> GrayImage {
    let (w, h) = img.dimensions();
    let mut out = img.clone();
    if w == 0 || h == 0 {
        return out;
    }
    let min_h_run = ((w as f32) * 0.1).max(40.0) as u32;
    let min_v_run = ((h as f32) * 0.05).max(40.0) as u32;
    let ink = |x: u32, y: u32| img.get_pixel(x, y).0[0] < INK_LUMA;
    for y in 0..h {
        let mut x = 0;
        while x < w {
            if !ink(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < w && ink(x, y) {
                x += 1;
            }
            if x - start >= min_h_run {
                for xx in start..x {
                    out.put_pixel(xx, y, Luma([255]));
                }
            }
        }
    }
    for x in 0..w {
        let mut y = 0;
        while y < h {
            if !ink(x, y) {
                y += 1;
                continue;
            }
            let start = y;
            while y < h && ink(x, y) {
                y += 1;
            }
            if y - start >= min_v_run {
                for yy in start..y {
                    out.put_pixel(x, yy, Luma([255]));
                }
            }
        }
    }
    out
}

/// Column clusters of ink inside `band` (`[start, end)` in pixels): runs of inked columns, with
/// gaps of at most `min_gap` columns bridged (the spaces between the words of one cell).
fn band_clusters(img: &GrayImage, band: Band, min_gap: u32) -> Vec<(u32, u32)> {
    let w = img.width();
    let inked = |x: u32| (band.top..band.bottom).any(|y| img.get_pixel(x, y).0[0] < INK_LUMA);
    let mut clusters: Vec<(u32, u32)> = Vec::new();
    let mut x = 0;
    while x < w {
        if !inked(x) {
            x += 1;
            continue;
        }
        let start = x;
        while x < w && inked(x) {
            x += 1;
        }
        match clusters.last_mut() {
            Some(last) if start - last.1 <= min_gap => last.1 = x,
            _ => clusters.push((start, x)),
        }
    }
    clusters
}

/// Estimates word boxes for `text` on the page image `img` (grey, any size): ink bands (rulings
/// removed) become text lines, matched by count to the text lines; the words of a line are spread
/// over the band's ink extent proportionally to their display width — or, for a table row whose
/// cells match the band's ink clusters, each cell over its own cluster.
pub fn estimate(img: &GrayImage, text: &str) -> PageLayout {
    let mut layout = PageLayout::empty(LayoutSource::Estimated);
    let lines = line_cells(text);
    if lines.is_empty() {
        return layout;
    }
    let (w, h) = img.dimensions();
    let n = lines.len();
    let clean = if w > 0 && h > 0 { Some(without_rulings(img)) } else { None };
    let mut bands = clean.as_ref().map(ink_bands).unwrap_or_default();
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    if bands.is_empty() {
        // Blank-looking page (or no image): evenly spaced lines over the text area.
        let top = 0.08 * h;
        let span = 0.84 * h;
        let pitch = span / n as f32;
        let height = (pitch * 0.7).min(0.035 * h).max(1.0);
        for i in 0..n {
            let t = (top + pitch * i as f32) as u32;
            bands.push(Band { top: t, bottom: t + height as u32 });
        }
    }
    // Match the band count to the line count: lines broken into two bands sit close together
    // (merge the smallest gap); dense bands hold several lines (split the tallest).
    while bands.len() > n {
        let mut best = 0;
        let mut best_gap = u32::MAX;
        for i in 0..bands.len() - 1 {
            let gap = bands[i + 1].top.saturating_sub(bands[i].bottom);
            if gap < best_gap {
                best_gap = gap;
                best = i;
            }
        }
        let next = bands.remove(best + 1);
        bands[best].bottom = next.bottom;
    }
    while bands.len() < n {
        let Some((i, _)) = bands.iter().enumerate().max_by_key(|(_, b)| b.height()) else { break };
        let b = bands[i];
        if b.height() < 2 {
            // Nothing left to split: duplicate the last band so every line gets a box.
            bands.push(*bands.last().unwrap_or(&b));
            continue;
        }
        let mid = b.top + b.height() / 2;
        bands[i] = Band { top: b.top, bottom: mid };
        bands.insert(i + 1, Band { top: mid, bottom: b.bottom });
    }
    for (i, (band, cells)) in bands.iter().zip(lines.iter()).enumerate() {
        let y = band.top as f32 / h;
        let bh = band.height().max(1) as f32 / h;
        if cells.len() >= 2 {
            if let Some(img) = clean.as_ref() {
                // Cells over the band's ink clusters when the counts agree (extra clusters are the
                // wide gaps inside a cell: bridge the narrowest ones first).
                let min_gap = ((band.height() as f32) * 0.8).max(4.0) as u32;
                let mut clusters = band_clusters(img, *band, min_gap);
                while clusters.len() > cells.len() {
                    let mut best = 0;
                    let mut best_gap = u32::MAX;
                    for j in 0..clusters.len() - 1 {
                        let gap = clusters[j + 1].0.saturating_sub(clusters[j].1);
                        if gap < best_gap {
                            best_gap = gap;
                            best = j;
                        }
                    }
                    let next = clusters.remove(best + 1);
                    clusters[best].1 = next.1;
                }
                if clusters.len() == cells.len() {
                    for (cell, (a, b)) in cells.iter().zip(&clusters) {
                        let words: Vec<&str> = cell.split_whitespace().collect();
                        let (x0, x1) = (*a as f32 / w, *b as f32 / w);
                        layout.words.extend(place_words(&words, x0, x1 - x0, y, bh, i as u32, false));
                    }
                    continue;
                }
            }
        }
        let (x0, x1) = match clean.as_ref().and_then(|img| band_extent(img, *band)) {
            Some((a, b)) => (a as f32 / w, b as f32 / w),
            None => (0.08, 0.92),
        };
        let words: Vec<&str> = cells.iter().flat_map(|c| c.split_whitespace()).collect();
        layout.words.extend(place_words(&words, x0, x1 - x0, y, bh, i as u32, false));
    }
    layout
}

// ---------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------

/// Byte spans of every layout word in `text`, found in order (a word that cannot be found is
/// `None` and does not move the cursor), so repeated words resolve to the right occurrence.
fn locate_all(text: &str, words: &[Word]) -> Vec<Option<(usize, usize)>> {
    let mut cursor = 0usize;
    words
        .iter()
        .map(|w| {
            if w.t.is_empty() || cursor > text.len() {
                return None;
            }
            let found = text[cursor..].find(&w.t).map(|i| (cursor + i, cursor + i + w.t.len()));
            if let Some((_, end)) = found {
                cursor = end;
            }
            found
        })
        .collect()
}

/// The edit replaces the words `start..end` of `layout` with `replacement` (any whitespace splits
/// it into words; empty deletes the words). Returns the new page text; the layout is updated in
/// place and the new words are marked `edited`.
pub fn apply_edit(layout: &mut PageLayout, text: &str, start: usize, end: usize, replacement: &str) -> String {
    let end = end.min(layout.words.len());
    if start >= end {
        return text.to_string();
    }
    let new_words: Vec<&str> = replacement.split_whitespace().collect();
    let spans = locate_all(text, &layout.words);
    let hit: Vec<(usize, usize)> = spans[start..end].iter().flatten().copied().collect();
    let found_all = hit.len() == end - start;
    let new_text = if found_all && new_words.len() == end - start {
        // Same word count: swap each word in place so the separators / markup between them survive.
        let mut s = text.to_string();
        for (span, w) in hit.iter().zip(new_words.iter()).rev() {
            s.replace_range(span.0..span.1, w);
        }
        s
    } else if let (Some(first), Some(last)) = (hit.first(), hit.last()) {
        let mut s = text.to_string();
        let mut from = first.0;
        let mut to = last.1;
        if new_words.is_empty() {
            // Deleting: eat one neighbouring blank so no double space is left behind.
            if to < s.len() && s[to..].starts_with(' ') {
                to += 1;
            } else if from > 0 && s[..from].ends_with(' ') {
                from -= 1;
            }
        }
        s.replace_range(from..to, &new_words.join(" "));
        s
    } else {
        String::new() // decided below from the layout
    };
    // Layout: equal counts keep every box; otherwise the new words share the old range's box.
    let old: Vec<Word> = layout.words.drain(start..end).collect();
    let line = old[0].line;
    let fresh: Vec<Word> = if new_words.len() == old.len() {
        old.iter()
            .zip(new_words.iter())
            .map(|(o, t)| Word { t: (*t).to_string(), x: o.x, y: o.y, w: o.w, h: o.h, line: o.line, edited: true })
            .collect()
    } else if new_words.is_empty() {
        Vec::new()
    } else {
        let tmp = PageLayout { version: LAYOUT_VERSION, source: layout.source, words: old.clone() };
        let (x, y, w, h) = tmp.bbox(0, old.len()).unwrap_or((0.0, 0.0, 0.2, 0.02));
        let same_line = old.iter().all(|o| o.line == line);
        let (y, h) = if same_line { (y, h) } else { (old[0].y, old[0].h) };
        place_words(&new_words, x, w, y, h, line, true)
    };
    let fresh_len = fresh.len();
    for (i, w) in fresh.into_iter().enumerate() {
        layout.words.insert(start + i, w);
    }
    if hit.is_empty() {
        // The words were not in the text (stale layout): the layout becomes the text.
        return layout.text();
    }
    let _ = fresh_len;
    new_text
}

/// Adds `new_text` as words laid over `bbox` (fractions), in reading order, and returns the new page
/// text (the words become a line of their own, inserted before the first word below the box).
pub fn add_words(layout: &mut PageLayout, text: &str, bbox: (f32, f32, f32, f32), new_text: &str) -> String {
    let words: Vec<&str> = new_text.split_whitespace().collect();
    if words.is_empty() {
        return text.to_string();
    }
    let (bx, by, bw, bh) = finish_box(bbox.0, bbox.1, bbox.2, bbox.3);
    let centre = by + bh / 2.0;
    let idx = layout
        .words
        .iter()
        .position(|w| {
            let wc = w.y + w.h / 2.0;
            let tol = w.h.min(bh) * 0.5;
            wc > centre + tol || ((wc - centre).abs() <= tol && w.x > bx)
        })
        .unwrap_or(layout.words.len());
    let spans = locate_all(text, &layout.words);
    let joined = words.join(" ");
    let mut s = text.to_string();
    let same_line = idx < layout.words.len() && {
        let w = &layout.words[idx];
        ((w.y + w.h / 2.0) - centre).abs() <= w.h.min(bh) * 0.5
    };
    match (idx < layout.words.len(), spans.get(idx).copied().flatten()) {
        (true, Some((at, _))) => s.insert_str(at, &format!("{joined}{}", if same_line { " " } else { "\n" })),
        _ => {
            if !s.trim_end().is_empty() {
                let trimmed = s.trim_end().len();
                s.truncate(trimmed);
                s.push('\n');
            }
            s.push_str(&joined);
        }
    }
    let line = if idx < layout.words.len() {
        let l = layout.words[idx].line;
        if !same_line {
            for w in &mut layout.words[idx..] {
                w.line += 1;
            }
        }
        l
    } else {
        layout.words.last().map(|w| w.line + 1).unwrap_or(0)
    };
    let fresh = place_words(&words, bx, bw, by, bh, line, true);
    for (i, w) in fresh.into_iter().enumerate() {
        layout.words.insert(idx + i, w);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str, x0: f32, baseline: f32, size: f32) -> Vec<CharBox> {
        // Monospaced 0.5 em glyphs on one line.
        s.chars()
            .enumerate()
            .map(|(i, ch)| {
                let left = x0 + i as f32 * size * 0.5;
                CharBox {
                    ch,
                    left,
                    right: left + size * 0.5,
                    bottom: baseline - size * 0.2,
                    top: baseline + size * 0.8,
                }
            })
            .collect()
    }

    #[test]
    fn chars_become_words_and_lines_with_rotation() {
        let mut c = chars("Hello World", 56.0, 780.0, 12.0);
        c.extend(chars("Second line", 56.0, 760.0, 12.0));
        let layout = from_chars(&c, PageBox::new(595.0, 842.0), 0);
        let words: Vec<&str> = layout.words.iter().map(|w| w.t.as_str()).collect();
        assert_eq!(words, ["Hello", "World", "Second", "line"]);
        assert_eq!(layout.words.iter().map(|w| w.line).collect::<Vec<_>>(), [0, 0, 1, 1]);
        let hello = &layout.words[0];
        assert!((hello.x - 56.0 / 595.0).abs() < 1e-4);
        assert!((hello.y - (1.0 - 789.6 / 842.0)).abs() < 1e-4);
        assert!((hello.w - 30.0 / 595.0).abs() < 1e-4);
        assert_eq!(layout.text(), "Hello World\nSecond line");
        // Rotation 90° clockwise: the top-left word lands at the top-right.
        let rotated = from_chars(&c, PageBox::new(595.0, 842.0), 90);
        let r = &rotated.words[0];
        assert!(r.x > 0.9 && r.y < 0.15, "{r:?}");
        assert!((r.w - hello.h).abs() < 1e-5 && (r.h - hello.w).abs() < 1e-5);
        // Rotating the unrotated layout gives the same boxes.
        let mut turned = layout.clone();
        turned.rotate(90);
        assert!((turned.words[0].x - r.x).abs() < 1e-5 && (turned.words[0].y - r.y).abs() < 1e-5);
    }

    #[test]
    fn a_horizontal_jump_without_a_space_splits_words() {
        let mut c = chars("AB", 10.0, 100.0, 10.0);
        c.extend(chars("CD", 60.0, 100.0, 10.0));
        let layout = from_chars(&c, PageBox::new(200.0, 200.0), 0);
        assert_eq!(layout.words.iter().map(|w| w.t.as_str()).collect::<Vec<_>>(), ["AB", "CD"]);
        assert_eq!(layout.words[1].line, 0);
    }

    #[test]
    fn an_offset_page_box_is_normalised() {
        // MediaBox [100 50 695 892]: the same glyph positions shifted by the box origin.
        let c = chars("Hi", 156.0, 830.0, 12.0);
        let layout = from_chars(&c, PageBox { left: 100.0, bottom: 50.0, width: 595.0, height: 842.0 }, 0);
        let w = &layout.words[0];
        assert!((w.x - 56.0 / 595.0).abs() < 1e-4, "{w:?}");
        assert!((w.y - (1.0 - 789.6 / 842.0)).abs() < 1e-4, "{w:?}");
    }

    #[test]
    fn text_lines_strip_ocr_markup() {
        let lines = text_lines("# 스타벅스 역삼점\n<table><tr><th>품명</th><th>수량</th></tr><tr><td>아메리카노</td><td>2</td></tr></table>\n합계 12,500\n<stamp>인</stamp>");
        assert_eq!(lines, ["# 스타벅스 역삼점", "품명 수량", "아메리카노 2", "합계 12,500", "인"]);
    }

    /// Text-like ink: 5 px strokes with 1 px gaps (no long straight runs, unlike a ruling).
    fn strokes(img: &mut GrayImage, x0: u32, x1: u32, y0: u32, y1: u32) {
        for y in y0..y1 {
            for x in x0..x1 {
                if (x - x0) % 6 != 5 {
                    img.put_pixel(x, y, Luma([0]));
                }
            }
        }
    }

    #[test]
    fn estimate_lays_text_lines_onto_ink_bands() {
        // 400 × 600 white page with three dark bands of text-like rows.
        let mut img = GrayImage::from_pixel(400, 600, Luma([255]));
        for &(top, bottom, x0, x1) in &[(100u32, 112u32, 40u32, 300u32), (140, 152, 40, 200), (300, 312, 60, 360)] {
            strokes(&mut img, x0, x1, top, bottom);
        }
        let layout = estimate(&img, "첫째 줄입니다\n둘째\n셋째 줄 끝");
        assert_eq!(layout.source, LayoutSource::Estimated);
        let words: Vec<&str> = layout.words.iter().map(|w| w.t.as_str()).collect();
        assert_eq!(words, ["첫째", "줄입니다", "둘째", "셋째", "줄", "끝"]);
        let first = &layout.words[0];
        assert!((first.y - 100.0 / 600.0).abs() < 0.01 && (first.h - 12.0 / 600.0).abs() < 0.01, "{first:?}");
        assert!((first.x - 40.0 / 400.0).abs() < 0.01);
        let last = layout.words.last().unwrap();
        assert!((last.x + last.w - 360.0 / 400.0).abs() < 0.01, "{last:?}");
        assert_eq!(layout.words.iter().map(|w| w.line).collect::<Vec<_>>(), [0, 0, 1, 2, 2, 2]);
        // More text lines than bands: the tallest band is split.
        let more = estimate(&img, "a\nb\nc\nd\ne");
        assert_eq!(more.words.len(), 5);
        // Fewer: the closest bands merge (100–112 and 140–152 sit closest).
        let fewer = estimate(&img, "하나 둘\n셋");
        assert_eq!(fewer.words.len(), 3);
        assert!((fewer.words[0].y - 100.0 / 600.0).abs() < 0.01 && fewer.words[2].y > 0.45);
        // No ink at all: evenly spaced lines.
        let blank = GrayImage::from_pixel(100, 100, Luma([255]));
        let even = estimate(&blank, "x\ny");
        assert_eq!(even.words.len(), 2);
        assert!(even.words[1].y > even.words[0].y);
    }

    fn sample() -> (PageLayout, String) {
        let text = "메모 1쪽의 텍스트 레이어입니다.\n합계 12,500원 합계".to_string();
        let mut img = GrayImage::from_pixel(200, 100, Luma([255]));
        strokes(&mut img, 10, 190, 20, 30);
        strokes(&mut img, 10, 150, 60, 70);
        (estimate(&img, &text), text)
    }

    #[test]
    fn edits_rewrite_the_text_in_place_and_keep_boxes() {
        let (mut layout, text) = sample();
        assert_eq!(layout.words.len(), 7);
        // One word → one word: the box stays, the text changes in place.
        let before = layout.words[1].clone();
        let t1 = apply_edit(&mut layout, &text, 1, 2, "첫째쪽의");
        assert_eq!(t1, "메모 첫째쪽의 텍스트 레이어입니다.\n합계 12,500원 합계");
        assert_eq!(layout.words[1].t, "첫째쪽의");
        assert!(layout.words[1].edited);
        assert_eq!((layout.words[1].x, layout.words[1].w), (before.x, before.w));
        // Two words → three words: the span is replaced and the new words share the span's box.
        let t2 = apply_edit(&mut layout, &t1, 2, 4, "본문 글자 입니다");
        assert_eq!(t2, "메모 첫째쪽의 본문 글자 입니다\n합계 12,500원 합계");
        assert_eq!(layout.words.len(), 8);
        assert!((layout.words[2].x - before.x - before.w).abs() < 0.1);
        assert_eq!(layout.words[4].line, 0);
        // The second "합계" (index 7) is the one replaced, not the first.
        let t3 = apply_edit(&mut layout, &t2, 7, 8, "총계");
        assert_eq!(t3, "메모 첫째쪽의 본문 글자 입니다\n합계 12,500원 총계");
        // Deleting a word eats the neighbouring blank.
        let t4 = apply_edit(&mut layout, &t3, 6, 7, "");
        assert_eq!(t4, "메모 첫째쪽의 본문 글자 입니다\n합계 총계");
        assert_eq!(layout.words.len(), 7);
        assert_eq!(layout.range_text(0, 2), "메모 첫째쪽의");
        assert_eq!(layout.range_text(4, 7), "입니다\n합계 총계");
    }

    #[test]
    fn edits_inside_ocr_markup_keep_the_tags() {
        let text = "<table><tr><td>아메리카노</td><td>2</td></tr></table>\n합계 12,500";
        let img = GrayImage::from_pixel(100, 100, Luma([255]));
        let mut layout = estimate(&img, text);
        assert_eq!(
            layout.words.iter().map(|w| w.t.as_str()).collect::<Vec<_>>(),
            ["아메리카노", "2", "합계", "12,500"]
        );
        let out = apply_edit(&mut layout, text, 0, 2, "카페라떼 3");
        assert_eq!(out, "<table><tr><td>카페라떼</td><td>3</td></tr></table>\n합계 12,500");
    }

    #[test]
    fn stale_layout_falls_back_to_the_layout_text() {
        let (mut layout, _) = sample();
        let out = apply_edit(&mut layout, "완전히 다른 텍스트", 0, 1, "노트");
        assert_eq!(out, layout.text());
        assert!(out.starts_with("노트 1쪽의"));
    }

    #[test]
    fn added_words_land_in_reading_order() {
        let (mut layout, text) = sample();
        // A box between the two lines becomes its own line before "합계".
        let t1 = add_words(&mut layout, &text, (0.1, 0.42, 0.5, 0.08), "추가 메모");
        assert_eq!(t1, "메모 1쪽의 텍스트 레이어입니다.\n추가 메모\n합계 12,500원 합계");
        assert_eq!(layout.words[4].t, "추가");
        assert_eq!(layout.words[4].line, 1);
        assert_eq!(layout.words[6].t, "합계");
        assert_eq!(layout.words[6].line, 2);
        // A box below everything is appended.
        let t2 = add_words(&mut layout, &t1, (0.1, 0.9, 0.3, 0.05), "끝");
        assert!(t2.ends_with("\n끝"));
        assert_eq!(layout.words.last().unwrap().line, 3);
        // Empty input changes nothing.
        assert_eq!(add_words(&mut layout, &t2, (0.0, 0.0, 0.1, 0.1), "   "), t2);
        let json = layout.to_json();
        assert_eq!(PageLayout::from_json(&json).as_ref(), Some(&layout));
    }

    /// Characters of one cell line: `text` at `(x0, baseline)`, 11 pt glyphs (CJK 11 pt wide,
    /// Latin / digits 6 pt), mirroring the character boxes PDFium reports for a printed table.
    fn cell(text: &str, x0: f32, baseline: f32) -> Vec<CharBox> {
        let mut x = x0;
        text.chars()
            .map(|ch| {
                let w = if ch.is_ascii() { 6.0 } else { 11.0 };
                let c = CharBox { ch, left: x, right: x + w, bottom: baseline - 3.0, top: baseline + 11.0 };
                x += w;
                c
            })
            .collect()
    }

    #[test]
    fn wrapped_table_cells_in_pdf_order_share_a_row() {
        // Header row (cells wrapped onto two lines sit on baselines 695 / 680, single-line cells are
        // centred on 688): PDF order is cell by cell. Then a body row at 640.
        let mut c = Vec::new();
        c.extend(cell("번", 70.0, 695.0));
        c.extend(cell("호", 70.0, 680.0));
        c.extend(cell("품목", 110.0, 688.0));
        c.extend(cell("규격", 200.0, 688.0));
        c.extend(cell("수", 290.0, 695.0));
        c.extend(cell("량", 290.0, 680.0));
        c.extend(cell("단가", 330.0, 688.0));
        c.extend(cell("1", 70.0, 640.0));
        c.extend(cell("노트북", 110.0, 640.0));
        c.extend(cell("1,200,0", 330.0, 647.0));
        c.extend(cell("00", 330.0, 632.0));
        // A row whose wrapped cell holds two words on its first line and whose number wraps too.
        c.extend(cell("3", 70.0, 590.0));
        c.extend(cell("무선 키보드", 110.0, 597.0));
        c.extend(cell("세트", 110.0, 582.0));
        c.extend(cell("한글 배열", 200.0, 590.0));
        c.extend(cell("4", 290.0, 597.0));
        c.extend(cell("0", 290.0, 582.0));
        c.extend(cell("45,000", 330.0, 590.0));
        let layout = from_chars(&c, PageBox::new(595.0, 842.0), 0);
        let words: Vec<&str> = layout.words.iter().map(|w| w.t.as_str()).collect();
        assert_eq!(
            words,
            [
                "번",
                "호",
                "품목",
                "규격",
                "수",
                "량",
                "단가",
                "1",
                "노트북",
                "1,200,0",
                "00",
                "3",
                "무선",
                "키보드",
                "세트",
                "한글",
                "배열",
                "4",
                "0",
                "45,000"
            ]
        );
        assert_eq!(
            layout.words.iter().map(|w| w.line).collect::<Vec<_>>(),
            [0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2]
        );
        assert_eq!(
            layout.text(),
            "번 호 품목 규격 수 량 단가\n1 노트북 1,200,0 00\n3 무선 키보드 세트 한글 배열 4 0 45,000"
        );
        // The boxes keep their own positions: "호" sits below "번".
        assert!(layout.words[1].y > layout.words[0].y + layout.words[0].h * 0.8);
    }

    #[test]
    fn a_heading_over_its_paragraph_stays_a_line_of_its_own() {
        // "제목" directly above "본문 시작 …" (same left edge, next line) is not a wrapped cell: the
        // next run is only a word space away, not a cell gap.
        let mut c = cell("제목", 56.0, 780.0);
        c.extend(cell("본문 시작 문장", 56.0, 764.0));
        let layout = from_chars(&c, PageBox::new(595.0, 842.0), 0);
        assert_eq!(layout.words.iter().map(|w| w.line).collect::<Vec<_>>(), [0, 1, 1, 1]);
        assert_eq!(layout.text(), "제목\n본문 시작 문장");
        // Justified text leaves wide gaps: the line still continues to the right, so no cell.
        let mut c = cell("제목", 56.0, 780.0);
        c.extend(cell("본문", 56.0, 764.0));
        c.extend(cell("이어지는", 120.0, 764.0));
        c.extend(cell("글", 220.0, 764.0));
        let layout = from_chars(&c, PageBox::new(595.0, 842.0), 0);
        assert_eq!(layout.words.iter().map(|w| w.line).collect::<Vec<_>>(), [0, 1, 1, 1]);
        // A two-column page: the long stacks of each column are paragraph lines, each its own row.
        let mut c = Vec::new();
        for i in 0..6 {
            c.extend(cell("왼쪽 단의 글", 56.0, 780.0 - 16.0 * i as f32));
        }
        for i in 0..6 {
            c.extend(cell("오른쪽 단의 글", 320.0, 780.0 - 16.0 * i as f32));
        }
        let layout = from_chars(&c, PageBox::new(595.0, 842.0), 0);
        let lines: Vec<u32> = layout.words.iter().map(|w| w.line).collect();
        assert_eq!(lines[..18], [0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 5]);
        assert_eq!(lines[18..21], [6, 6, 6]);
    }

    #[test]
    fn stored_version_1_layouts_get_their_rows_re_derived() {
        let mut c = Vec::new();
        c.extend(cell("번", 70.0, 695.0));
        c.extend(cell("호", 70.0, 680.0));
        c.extend(cell("품목", 110.0, 688.0));
        c.extend(cell("1", 70.0, 640.0));
        let mut v1 = from_chars(&c, PageBox::new(595.0, 842.0), 0);
        // As the sequential numbering stored it: every baseline change was a new line.
        for (w, line) in v1.words.iter_mut().zip([0u32, 1, 2, 3]) {
            w.line = line;
        }
        v1.version = 1;
        let upgraded = PageLayout::from_json(&v1.to_json()).unwrap();
        assert_eq!(upgraded.version, LAYOUT_VERSION);
        assert_eq!(upgraded.words.iter().map(|w| w.line).collect::<Vec<_>>(), [0, 0, 0, 1]);
        assert_eq!(upgraded.words.iter().map(|w| w.t.as_str()).collect::<Vec<_>>(), ["번", "호", "품목", "1"]);
        // Unknown versions are rejected.
        v1.version = 9;
        assert!(PageLayout::from_json(&v1.to_json()).is_none());
    }

    #[test]
    fn line_cells_keep_table_cells_apart() {
        let cells = line_cells("제목\n<table><tr><th>품명</th><th>수량</th><th></th></tr><tr><td>아메리카노 라지</td><td>2</td></tr></table>");
        assert_eq!(
            cells,
            [vec!["제목".to_string()], vec!["품명".into(), "수량".into()], vec!["아메리카노 라지".into(), "2".into()]]
        );
    }

    /// A ruled table: three rows × three columns with 1 px borders, text-like blobs in the cells.
    fn ruled_table() -> GrayImage {
        let mut img = GrayImage::from_pixel(600, 400, Luma([255]));
        let fill = |img: &mut GrayImage, x0: u32, x1: u32, y0: u32, y1: u32| {
            for y in y0..y1 {
                for x in x0..x1 {
                    img.put_pixel(x, y, Luma([0]));
                }
            }
        };
        // Title line (text-like strokes).
        strokes(&mut img, 60, 300, 30, 42);
        // Borders: vertical rules through every row, horizontal rules between rows.
        for &x in &[50u32, 220, 400, 550] {
            fill(&mut img, x, x + 1, 95, 246);
        }
        for &y in &[95u32, 145, 195, 245] {
            fill(&mut img, 50, 551, y, y + 1);
        }
        // Cell text blobs (12 px high, centred in 50 px rows).
        let rows = [114u32, 164, 214];
        let cols: [(u32, u32); 3] = [(60, 120), (230, 330), (410, 500)];
        for &y in &rows {
            for &(a, b) in &cols {
                strokes(&mut img, a, b, y, y + 12);
            }
        }
        img
    }

    #[test]
    fn estimate_puts_table_cells_over_their_own_ink() {
        let img = ruled_table();
        let text = "제목 줄\n<table><tr><td>가 나</td><td>다라</td><td>마</td></tr>\
                    <tr><td>1</td><td>둘 셋</td><td>넷</td></tr><tr><td>합계</td><td>12,500</td><td>원</td></tr></table>";
        let layout = estimate(&img, text);
        let words: Vec<&str> = layout.words.iter().map(|w| w.t.as_str()).collect();
        assert_eq!(words, ["제목", "줄", "가", "나", "다라", "마", "1", "둘", "셋", "넷", "합계", "12,500", "원"]);
        // The rulings did not glue the rows together: four lines, each on its own band.
        assert_eq!(layout.words.iter().map(|w| w.line).collect::<Vec<_>>(), [0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3]);
        let within = |w: &Word, a: u32, b: u32| {
            let cx = (w.x + w.w / 2.0) * 600.0;
            cx >= a as f32 && cx <= b as f32
        };
        // Cells over their own ink clusters (the words of a cell share its cluster).
        assert!(within(&layout.words[2], 60, 120) && within(&layout.words[3], 60, 120), "{:?}", &layout.words[2..4]);
        assert!(within(&layout.words[4], 230, 330), "{:?}", layout.words[4]);
        assert!(within(&layout.words[5], 410, 500), "{:?}", layout.words[5]);
        assert!(within(&layout.words[11], 230, 330) && within(&layout.words[12], 410, 500));
        // Rows land on their bands (the second row's blobs start at y = 164).
        let row2 = &layout.words[6];
        assert!((row2.y * 400.0 - 164.0).abs() <= 2.0, "{row2:?}");
        // The title line still spans its own ink extent.
        assert!((layout.words[0].x * 600.0 - 60.0).abs() < 3.0);
    }

    #[test]
    fn rulings_are_removed_but_glyph_strokes_stay() {
        let img = ruled_table();
        let clean = without_rulings(&img);
        assert_eq!(clean.get_pixel(50, 150).0[0], 255, "vertical rule");
        assert_eq!(clean.get_pixel(300, 145).0[0], 255, "horizontal rule");
        assert_eq!(clean.get_pixel(70, 120).0[0], 0, "cell text");
        assert_eq!(clean.get_pixel(100, 35).0[0], 0, "title strokes");
        // Bands of the cleaned image: the title and the three rows (the rulings glued the rows before).
        assert_eq!(ink_bands(&clean).len(), 4);
        assert_eq!(ink_bands(&img).len(), 2);
    }
}
