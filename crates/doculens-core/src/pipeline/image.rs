//! Page images for the VLM: the cached page render (JPEG, ≤ 4096 px long
//! edge) is downscaled to a token budget (Qwen vision: one token per 32×32 px
//! after patch merging, §3.4 "렌더 예산") and sent as a `data:` URL.

use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, GenericImageView};

use crate::engine::client::data_url;
use crate::error::{CoreError, Result};

/// Pixels per vision token side (Qwen3.5 / Qwen3-VL: 16 px patch × 2×2 merge).
pub const PIXELS_PER_TOKEN: u32 = 32;
const JPEG_QUALITY: u8 = 85;

/// Vision tokens of an image of this size.
pub fn tokens_for(width: u32, height: u32) -> u32 {
    width.div_ceil(PIXELS_PER_TOKEN) * height.div_ceil(PIXELS_PER_TOKEN)
}

/// Downscales so the image needs at most `max_tokens` tokens (never upscales;
/// the server's `--image-min-tokens` handles the lower bound).
pub fn fit_tokens(img: &DynamicImage, max_tokens: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || tokens_for(w, h) <= max_tokens {
        return img.clone();
    }
    let budget_px = (max_tokens as f64) * (PIXELS_PER_TOKEN as f64).powi(2);
    let f = (budget_px / (w as f64 * h as f64)).sqrt();
    let nw = ((w as f64 * f).floor() as u32).max(PIXELS_PER_TOKEN);
    let nh = ((h as f64 * f).floor() as u32).max(PIXELS_PER_TOKEN);
    // Rounding can leave the result one token over; shave until it fits.
    let (mut nw, mut nh) = (nw, nh);
    while tokens_for(nw, nh) > max_tokens && nw > PIXELS_PER_TOKEN && nh > PIXELS_PER_TOKEN {
        nw -= PIXELS_PER_TOKEN / 2;
        nh = ((h as f64) * (nw as f64 / w as f64)).floor() as u32;
    }
    img.resize_exact(nw, nh, image::imageops::FilterType::Triangle)
}

/// A prepared page image: data URL + final size.
#[derive(Debug, Clone)]
pub struct PreparedImage {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub tokens: u32,
}

pub fn encode_jpeg(img: &DynamicImage) -> Result<Vec<u8>> {
    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    let mut enc = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
    enc.encode_image(&rgb).map_err(|e| CoreError::Render(format!("jpeg encode: {e}")))?;
    Ok(out)
}

/// Loads a cached page file and fits it to the token budget.
pub fn prepare(path: &Path, max_tokens: u32) -> Result<PreparedImage> {
    let img = image::ImageReader::open(path)
        .map_err(|e| CoreError::io(path, e))?
        .with_guessed_format()
        .map_err(|e| CoreError::io(path, e))?
        .decode()
        .map_err(|e| CoreError::Render(format!("{}: {e}", path.display())))?;
    prepare_image(&img, max_tokens)
}

pub fn prepare_image(img: &DynamicImage, max_tokens: u32) -> Result<PreparedImage> {
    let fitted = fit_tokens(img, max_tokens.max(64));
    let (w, h) = fitted.dimensions();
    let bytes = encode_jpeg(&fitted)?;
    Ok(PreparedImage { url: data_url("image/jpeg", &bytes), width: w, height: h, tokens: tokens_for(w, h) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn fits_a4_renders_into_token_budgets() {
        // 200 dpi A4 render ≈ 1654×2339 → 3,784 tokens.
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(1654, 2339, Rgb([255, 255, 255])));
        assert_eq!(tokens_for(1654, 2339), 52 * 74);
        let c = fit_tokens(&img, 1_024);
        assert!(tokens_for(c.width(), c.height()) <= 1_024, "{}x{}", c.width(), c.height());
        assert!(c.width() >= 800, "keeps most of the width: {}", c.width());
        let o = fit_tokens(&img, 1_500);
        assert!(tokens_for(o.width(), o.height()) <= 1_500 && o.width() > c.width());
        let same = fit_tokens(&img, 10_000);
        assert_eq!((same.width(), same.height()), (1654, 2339), "never upscales or shrinks when within budget");
        let p = prepare_image(&img, 1_024).unwrap();
        assert!(p.url.starts_with("data:image/jpeg;base64,") && p.tokens <= 1_024);
    }
}
