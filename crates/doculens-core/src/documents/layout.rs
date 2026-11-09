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

use image::GrayImage;
use serde::{Deserialize, Serialize};

pub const LAYOUT_VERSION: u8 = 1;

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

    pub fn from_json(json: &str) -> Option<PageLayout> {
        serde_json::from_str::<PageLayout>(json).ok().filter(|l| l.version == LAYOUT_VERSION)
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

struct Run {
    text: String,
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
}

/// Builds the layout of a PDF page from its character boxes (content space, see [`PageBox`]).
/// `rotation` is the total clockwise rotation of the rendered image relative to that space: the page's
/// intrinsic `/Rotate` plus the user's stored rotation.
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
    // 2. Lines: a run starts a new line when its vertical centre leaves the current line or it jumps
    //    back to the left.
    let mut line = 0u32;
    let mut line_centre = f32::NAN;
    let mut line_h = 0.0f32;
    let mut last_right = f32::MIN;
    for (i, r) in runs.iter().enumerate() {
        let centre = (r.bottom + r.top) / 2.0;
        let h = (r.top - r.bottom).max(0.5);
        if i > 0 {
            let tol = 0.5 * line_h.max(h);
            if (centre - line_centre).abs() > tol || r.left < last_right - h {
                line += 1;
                line_centre = centre;
                line_h = h;
            } else {
                line_h = line_h.max(h);
            }
        } else {
            line_centre = centre;
            line_h = h;
        }
        last_right = r.right;
        // 3. Fractions of the unrotated page box (top-left origin), then the rotation.
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

/// The visible text lines of a page text: OCR markup (`<table>`, `<br>`, `<stamp>` …) is reduced to
/// line breaks / cell separators and entities are decoded.
pub fn text_lines(text: &str) -> Vec<String> {
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
            "td" | "th" if closing => out.push(' '),
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
    decoded.split('\n').map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect()
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

/// Estimates word boxes for `text` on the page image `img` (grey, any size): ink bands become
/// text lines, matched by count to the text lines, and the words of each line are spread over the
/// band's ink extent proportionally to their display width.
pub fn estimate(img: &GrayImage, text: &str) -> PageLayout {
    let mut layout = PageLayout::empty(LayoutSource::Estimated);
    let lines = text_lines(text);
    if lines.is_empty() {
        return layout;
    }
    let (w, h) = img.dimensions();
    let n = lines.len();
    let mut bands = if w > 0 && h > 0 { ink_bands(img) } else { Vec::new() };
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
    for (i, (band, line)) in bands.iter().zip(lines.iter()).enumerate() {
        let (x0, x1) = match band_extent(img, *band) {
            Some((a, b)) if img.width() > 0 => (a as f32 / w, b as f32 / w),
            _ => (0.08, 0.92),
        };
        let words: Vec<&str> = line.split_whitespace().collect();
        let y = band.top as f32 / h;
        let bh = band.height().max(1) as f32 / h;
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
    use image::Luma;

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

    #[test]
    fn estimate_lays_text_lines_onto_ink_bands() {
        // 400 × 600 white page with three dark bands of text-like rows.
        let mut img = GrayImage::from_pixel(400, 600, Luma([255]));
        for &(top, bottom, x0, x1) in &[(100u32, 112u32, 40u32, 300u32), (140, 152, 40, 200), (300, 312, 60, 360)] {
            for y in top..bottom {
                for x in x0..x1 {
                    img.put_pixel(x, y, Luma([0]));
                }
            }
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
        for y in 20..30 {
            for x in 10..190 {
                img.put_pixel(x, y, Luma([0]));
            }
        }
        for y in 60..70 {
            for x in 10..150 {
                img.put_pixel(x, y, Luma([0]));
            }
        }
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
}
