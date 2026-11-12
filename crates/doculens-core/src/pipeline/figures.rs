//! Text inside the pictures of a page (1.0.0 plan §6, 설정 "이미지 내 글자 추출").
//!
//! A page with its own text layer — a digital PDF, the converted PDF of a PPTX / DOCX / HWP — is not
//! sent through OCR: its text is exact. What the layer does not hold is the text *inside* its pictures
//! (a system diagram, a screen capture, a table pasted as an image). This pass reads those pictures
//! only, one model call each, and puts what it reads into the page's word layout where the picture
//! sits, so the words can be selected on the page image, searched and asked about like the rest
//! (user decisions 2026-11-12: pictures only, no "[그림] …" label, small and repeated pictures skipped).
//!
//! * which pictures: [`choose`] — not the small ones (icons, logos), not rules, not a picture that
//!   has layer text drawn over it (a scan with a text layer, a slide background);
//! * the same picture again (a logo on every page): [`PictureSignature`], read once per document;
//! * what the model answers: [`figure_text`] turns it into plain lines for the page text and into the
//!   cell markup the layout estimator understands.

use image::{imageops::FilterType, DynamicImage, GenericImageView};

use crate::documents::layout::{self, PageLayout};
use crate::engine::client::user_message_with_image;
use crate::error::Result;

use super::ocr;
use super::prompts::{self, Stage};
use super::vlm::{Reply, Vlm};

/// A picture box on the page: `(x, y, w, h)` as fractions of the rendered page.
pub type FigureBox = (f32, f32, f32, f32);

/// Smallest share of the page area a picture must cover (a 3 × 1.5 cm logo on A4 is 0.7 %).
const MIN_AREA: f32 = 0.012;
/// Smallest size on the rendered page (pixels): below it the text is not legible to the model anyway.
const MIN_PIXELS: (u32, u32) = (96, 48);
/// Longest side over shortest side: a rule or a decorative band is not a picture with text.
const MAX_ASPECT: f32 = 20.0;
/// Layer words lying over a picture from which the picture counts as "already has its text".
const TEXT_OVER_PICTURE: usize = 3;
/// Pictures read per page (largest first) and model calls per document.
pub const MAX_PER_PAGE: usize = 6;
pub const MAX_PER_DOCUMENT: usize = 40;

fn area(b: &FigureBox) -> f32 {
    b.2 * b.3
}

fn same_place(a: &FigureBox, b: &FigureBox) -> bool {
    const TOL: f32 = 0.01;
    (a.0 - b.0).abs() <= TOL
        && (a.1 - b.1).abs() <= TOL
        && (a.0 + a.2 - b.0 - b.2).abs() <= TOL
        && (a.1 + a.3 - b.1 - b.3).abs() <= TOL
}

/// The pictures of a page worth reading, top to bottom. `boxes` are the image objects of the page
/// (fractions), `page_px` the size of its render, `layout` the page's words.
pub fn choose(boxes: &[FigureBox], page_px: (u32, u32), layout: &PageLayout) -> Vec<FigureBox> {
    let (pw, ph) = (page_px.0.max(1) as f32, page_px.1.max(1) as f32);
    let mut kept: Vec<FigureBox> = Vec::new();
    for b in boxes {
        let (w_px, h_px) = (b.2 * pw, b.3 * ph);
        let aspect = if w_px >= h_px { w_px / h_px.max(1.0) } else { h_px / w_px.max(1.0) };
        if area(b) < MIN_AREA || w_px < MIN_PIXELS.0 as f32 || h_px < MIN_PIXELS.1 as f32 || aspect > MAX_ASPECT {
            continue;
        }
        // An image and its soft mask, or a picture placed twice at the same spot.
        if kept.iter().any(|k| same_place(k, b)) {
            continue;
        }
        if layout.words_inside(*b) >= TEXT_OVER_PICTURE {
            continue;
        }
        kept.push(*b);
    }
    kept.sort_by(|a, b| area(b).total_cmp(&area(a)));
    kept.truncate(MAX_PER_PAGE);
    kept.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
    kept
}

/// Cuts the picture out of the page render.
pub fn crop(page: &DynamicImage, bbox: FigureBox) -> DynamicImage {
    let (pw, ph) = page.dimensions();
    let x = ((bbox.0 * pw as f32).floor().max(0.0) as u32).min(pw.saturating_sub(1));
    let y = ((bbox.1 * ph as f32).floor().max(0.0) as u32).min(ph.saturating_sub(1));
    let w = ((bbox.2 * pw as f32).ceil() as u32).clamp(1, pw - x);
    let h = ((bbox.3 * ph as f32).ceil() as u32).clamp(1, ph - y);
    page.crop_imm(x, y, w, h)
}

/// What a picture looks like, comparable across pages: the same logo cut out of two page renders is
/// the same picture although the JPEG noise differs. A 16 × 16 grey thumbnail, one bit per cell
/// (brighter than the picture's mean or not), plus the shape of the picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictureSignature {
    bits: [u64; 4],
    /// Width over height in sixteenths (capped), so a banner and a square with the same pattern differ.
    shape: u8,
}

/// Cells of the thumbnail that may differ between two renders of the same picture.
const SAME_PICTURE_BITS: u32 = 12;

impl PictureSignature {
    pub fn of(img: &DynamicImage) -> PictureSignature {
        let (w, h) = img.dimensions();
        let thumb = img.resize_exact(16, 16, FilterType::Triangle).to_luma8();
        let mean = thumb.pixels().map(|p| p.0[0] as u32).sum::<u32>() / 256;
        let mut bits = [0u64; 4];
        for (i, p) in thumb.pixels().enumerate() {
            if p.0[0] as u32 > mean {
                bits[i / 64] |= 1 << (i % 64);
            }
        }
        PictureSignature { bits, shape: ((w as f32 / h.max(1) as f32).clamp(0.0, 15.9) * 16.0) as u8 }
    }

    /// The same picture, give or take the noise of a render.
    pub fn same(&self, other: &PictureSignature) -> bool {
        let apart: u32 = self.bits.iter().zip(&other.bits).map(|(a, b)| (a ^ b).count_ones()).sum();
        apart <= SAME_PICTURE_BITS && self.shape.abs_diff(other.shape) <= 1
    }

    /// The signature as text, for the mark the page layout keeps ([`layout::FigureMark::hash`]).
    pub fn hex(&self) -> String {
        format!(
            "{:016x}{:016x}{:016x}{:016x}-{:02x}",
            self.bits[0], self.bits[1], self.bits[2], self.bits[3], self.shape
        )
    }
}

/// What a picture says, in the two forms the page needs.
#[derive(Debug, Clone, PartialEq)]
pub struct FigureText {
    /// Plain lines for `pages.text`: one line per label / sentence / table row (cells two spaces apart).
    pub plain: String,
    /// The same lines with table rows as `<tr><td>…</td></tr>`, for [`layout::estimate`] (cells are
    /// laid over their own ink).
    pub markup: String,
}

/// The model's answer for a picture as [`FigureText`]; `None` when it read nothing (`null`). Markup the
/// model added although it was asked not to — HTML or Markdown tables, bullets — is reduced to lines.
pub fn figure_text(raw: &str) -> Option<FigureText> {
    let cleaned = ocr::clean(raw)?;
    let mut plain: Vec<String> = Vec::new();
    let mut markup: Vec<String> = Vec::new();
    for cells in layout::line_cells(&cleaned) {
        // A Markdown table row arrives as one cell: split it; its separator row is dropped.
        let cells: Vec<String> = if cells.len() == 1 && cells[0].contains('|') {
            cells[0].split('|').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect()
        } else {
            cells
        };
        let cells: Vec<String> = cells
            .iter()
            .map(|c| c.trim().trim_start_matches(['-', '*', '•', '·']).trim_start_matches('#').trim().to_string())
            .filter(|c| !c.is_empty() && !c.chars().all(|ch| matches!(ch, '-' | ':' | '=' | '_' | ' ')))
            .collect();
        if cells.is_empty() {
            continue;
        }
        plain.push(cells.join("  "));
        markup.push(if cells.len() >= 2 {
            format!("<table><tr>{}</tr></table>", cells.iter().map(|c| format!("<td>{c}</td>")).collect::<String>())
        } else {
            cells[0].clone()
        });
    }
    if plain.is_empty() {
        return None;
    }
    Some(FigureText { plain: plain.join("\n"), markup: markup.join("\n") })
}

/// Asks the model what one picture says.
pub fn read_picture(vlm: &Vlm<'_>, image_url: &str) -> Result<(Option<String>, Reply)> {
    let messages = vec![
        serde_json::json!({ "role": "system", "content": prompts::SYSTEM_OCR }),
        user_message_with_image(&prompts::figure(), image_url),
    ];
    let reply = vlm.call(Stage::Ocr, &messages, None, None)?;
    let text = ocr::clean(&reply.content);
    Ok((text, reply))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::layout::{LayoutSource, Word, LAYOUT_VERSION};
    use image::{GrayImage, Luma};

    fn page_words(words: &[(f32, f32)]) -> PageLayout {
        PageLayout {
            version: LAYOUT_VERSION,
            source: LayoutSource::Layer,
            words: words
                .iter()
                .enumerate()
                .map(|(i, &(x, y))| Word {
                    t: format!("w{i}"),
                    x,
                    y,
                    w: 0.05,
                    h: 0.02,
                    line: i as u32,
                    edited: false,
                    fig: false,
                })
                .collect(),
            figures: Vec::new(),
        }
    }

    #[test]
    fn small_pictures_rules_and_pictures_under_text_are_not_read() {
        let page = (1654, 2339); // A4 at 200 dpi
        let empty = page_words(&[]);
        let diagram = (0.1, 0.2, 0.8, 0.3);
        let logo = (0.8, 0.02, 0.14, 0.05); // 0.7 % of the page
        let rule = (0.1, 0.9, 0.8, 0.002);
        let band = (0.0, 0.0, 1.0, 0.03); // wide and thin: 33 : 1
        let chart = (0.1, 0.6, 0.4, 0.2);
        let chosen = choose(&[chart, logo, diagram, rule, band, diagram], page, &empty);
        assert_eq!(chosen, vec![diagram, chart], "top to bottom, the double once");
        // Text of the layer over a picture: a scanned page with its own text layer, a slide background.
        let background = (0.0, 0.0, 1.0, 1.0);
        let over = page_words(&[(0.2, 0.25), (0.3, 0.3), (0.5, 0.4)]);
        assert_eq!(choose(&[background, diagram], page, &over), Vec::<FigureBox>::new());
        // One or two words over it (a caption, a number) do not hide a picture.
        let caption = page_words(&[(0.2, 0.25), (0.3, 0.3)]);
        assert_eq!(choose(&[diagram], page, &caption), vec![diagram]);
        // A low-resolution render: the same share of the page is too few pixels to read.
        assert_eq!(choose(&[(0.1, 0.1, 0.3, 0.05)], (300, 400), &empty), Vec::<FigureBox>::new());
        // No more than the page limit, the largest ones.
        let many: Vec<FigureBox> = (0..9).map(|i| (0.05, 0.05 + i as f32 * 0.1, 0.2 + i as f32 * 0.05, 0.08)).collect();
        let chosen = choose(&many, page, &empty);
        assert_eq!(chosen.len(), MAX_PER_PAGE);
        assert_eq!(chosen[0], many[3], "the three smallest are left out");
    }

    #[test]
    fn the_same_picture_hashes_alike_and_a_crop_stays_inside_the_page() {
        let mut page = GrayImage::from_pixel(400, 600, Luma([255]));
        for y in 120..180 {
            for x in 60..260 {
                page.put_pixel(x, y, Luma([if (x / 20 + y / 20) % 2 == 0 { 0 } else { 255 }]));
            }
        }
        let page = DynamicImage::ImageLuma8(page);
        let cut = crop(&page, (0.15, 0.2, 0.5, 0.1));
        assert_eq!(cut.dimensions(), (200, 60));
        // The same pixels with a little noise (another page's JPEG) are the same picture; other content is not.
        let mut noisy = cut.to_luma8();
        for (i, p) in noisy.pixels_mut().enumerate() {
            p.0[0] = if p.0[0] > 127 { 250 - (i % 7) as u8 } else { 4 + (i % 5) as u8 };
        }
        let sig = PictureSignature::of(&cut);
        assert!(sig.same(&PictureSignature::of(&DynamicImage::ImageLuma8(noisy))));
        let blank = crop(&page, (0.0, 0.5, 0.5, 0.1));
        assert!(!sig.same(&PictureSignature::of(&blank)));
        // The same pattern in another shape (a banner) is another picture.
        let banner = cut.resize_exact(400, 30, FilterType::Nearest);
        assert!(!sig.same(&PictureSignature::of(&banner)));
        assert_eq!(sig.hex().len(), 67);
        // A box over the edge is clipped to the page.
        assert_eq!(crop(&page, (0.9, 0.95, 0.5, 0.5)).dimensions(), (40, 30));
    }

    #[test]
    fn the_answer_becomes_plain_lines_and_cell_markup() {
        assert_eq!(figure_text("null"), None);
        assert_eq!(figure_text("```\nnull\n```"), None);
        let t = figure_text(
            "그림 2. 구성도\n- 인증 서버\n* 결제 게이트웨이\n\n| 구간 | 평균(ms) |\n|---|---|\n| 로그인 | 84 |",
        )
        .unwrap();
        assert_eq!(t.plain, "그림 2. 구성도\n인증 서버\n결제 게이트웨이\n구간  평균(ms)\n로그인  84");
        assert_eq!(
            t.markup,
            "그림 2. 구성도\n인증 서버\n결제 게이트웨이\n<table><tr><td>구간</td><td>평균(ms)</td></tr></table>\n<table><tr><td>로그인</td><td>84</td></tr></table>"
        );
        // A dot inside a label is part of the label; only a leading bullet is dropped.
        assert_eq!(figure_text("· 알림 서비스 (SMS·메일)").unwrap().plain, "알림 서비스 (SMS·메일)");
        // An HTML table the model wrote all the same.
        let t =
            figure_text("<table><tr><th>구간</th><th>최대</th></tr><tr><td>결제 승인</td><td>905</td></tr></table>")
                .unwrap();
        assert_eq!(t.plain, "구간  최대\n결제 승인  905");
        // The words of the markup are the words of the plain text, in order.
        let img = GrayImage::from_pixel(200, 80, Luma([255]));
        let words: Vec<String> = layout::estimate(&img, &t.markup).words.into_iter().map(|w| w.t).collect();
        assert_eq!(words, ["구간", "최대", "결제", "승인", "905"]);
        assert_eq!(t.plain.split_whitespace().collect::<Vec<_>>(), words);
    }
}
