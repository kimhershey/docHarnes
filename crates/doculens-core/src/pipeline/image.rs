//! Page images for the VLM: the cached page render (JPEG, ≤ 4096 px long
//! edge) is downscaled to a token budget of the loaded model's vision encoder
//! (Gemma 4: one token per 48×48 px, at most 1,120 tokens; Qwen-VL drop-ins:
//! 32×32 px, §3.4 "렌더 예산") and sent as a `data:` URL.

use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, GenericImageView};
use serde::{Deserialize, Serialize};

use crate::engine::client::data_url;
use crate::error::{CoreError, Result};

const JPEG_QUALITY: u8 = 85;

/// How a vision encoder turns pixels into tokens: used to size page renders
/// and to budget the context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionGeometry {
    /// Pixels per vision token side after patch merging / pooling.
    pub px_per_token: u32,
    /// Hard ceiling of the encoder (tokens per image).
    pub max_tokens: u32,
    /// Floor the presets ask the server for (`--image-min-tokens`).
    pub default_min_tokens: u32,
}

impl VisionGeometry {
    /// Gemma 4 (`gemma4v` / `gemma4uv` projectors): 16 px patches with 3×3 average
    /// pooling → 48 px per token, 70–1,120 tokens per image (llama.cpp mtmd).
    pub const GEMMA4: VisionGeometry = VisionGeometry { px_per_token: 48, max_tokens: 1_120, default_min_tokens: 512 };
    /// Qwen3.5 / Qwen3-VL drop-ins (`qwen3vl_merger`): 16 px patches, 2×2 merge → 32 px per
    /// token; grounding needs ≥ 1,024 tokens (llama.cpp #16842), 4,096 is the OOM guard.
    pub const QWEN: VisionGeometry = VisionGeometry { px_per_token: 32, max_tokens: 4_096, default_min_tokens: 1_024 };
    /// The family the registry ships.
    pub const DEFAULT: VisionGeometry = VisionGeometry::GEMMA4;

    /// Geometry for a GGUF `general.architecture` (the shipped family when unknown).
    pub fn for_arch(arch: &str) -> VisionGeometry {
        if arch.trim().to_ascii_lowercase().starts_with("qwen") {
            VisionGeometry::QWEN
        } else {
            VisionGeometry::GEMMA4
        }
    }

    /// Geometry for a registry prompt profile (`gemma4-doc-v1`, `qwen3.5-doc-v1`, …).
    pub fn for_profile(profile: &str) -> VisionGeometry {
        VisionGeometry::for_arch(profile)
    }

    /// Vision tokens of an image of this size.
    pub fn tokens_for(&self, width: u32, height: u32) -> u32 {
        width.div_ceil(self.px_per_token) * height.div_ceil(self.px_per_token)
    }

    /// A stage budget clamped to what the encoder can take (never below 64 tokens).
    pub fn clamp_budget(&self, tokens: u32) -> u32 {
        tokens.max(64).min(self.max_tokens)
    }
}

impl Default for VisionGeometry {
    fn default() -> Self {
        VisionGeometry::DEFAULT
    }
}

/// Downscales so the image needs at most `max_tokens` tokens of `geom` (never
/// upscales; the server's `--image-min-tokens` handles the lower bound).
pub fn fit_tokens(geom: VisionGeometry, img: &DynamicImage, max_tokens: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || geom.tokens_for(w, h) <= max_tokens {
        return img.clone();
    }
    let px = geom.px_per_token;
    let budget_px = (max_tokens as f64) * (px as f64).powi(2);
    let f = (budget_px / (w as f64 * h as f64)).sqrt();
    let nw = ((w as f64 * f).floor() as u32).max(px);
    let nh = ((h as f64 * f).floor() as u32).max(px);
    // Rounding can leave the result one token over; shave until it fits.
    let (mut nw, mut nh) = (nw, nh);
    while geom.tokens_for(nw, nh) > max_tokens && nw > px && nh > px {
        nw -= px / 2;
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

/// Loads a cached page file and fits it to the token budget of `geom`.
pub fn prepare(path: &Path, geom: VisionGeometry, max_tokens: u32) -> Result<PreparedImage> {
    let img = image::ImageReader::open(path)
        .map_err(|e| CoreError::io(path, e))?
        .with_guessed_format()
        .map_err(|e| CoreError::io(path, e))?
        .decode()
        .map_err(|e| CoreError::Render(format!("{}: {e}", path.display())))?;
    prepare_image(&img, geom, max_tokens)
}

pub fn prepare_image(img: &DynamicImage, geom: VisionGeometry, max_tokens: u32) -> Result<PreparedImage> {
    let fitted = fit_tokens(geom, img, geom.clamp_budget(max_tokens));
    let (w, h) = fitted.dimensions();
    let bytes = encode_jpeg(&fitted)?;
    Ok(PreparedImage { url: data_url("image/jpeg", &bytes), width: w, height: h, tokens: geom.tokens_for(w, h) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn fits_a4_renders_into_token_budgets_for_qwen_geometry() {
        // 200 dpi A4 render ≈ 1654×2339 → 3,784 Qwen tokens.
        let g = VisionGeometry::QWEN;
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(1654, 2339, Rgb([255, 255, 255])));
        assert_eq!(g.tokens_for(1654, 2339), 52 * 74);
        let c = fit_tokens(g, &img, 1_024);
        assert!(g.tokens_for(c.width(), c.height()) <= 1_024, "{}x{}", c.width(), c.height());
        assert!(c.width() >= 800, "keeps most of the width: {}", c.width());
        let o = fit_tokens(g, &img, 1_500);
        assert!(g.tokens_for(o.width(), o.height()) <= 1_500 && o.width() > c.width());
        let same = fit_tokens(g, &img, 10_000);
        assert_eq!((same.width(), same.height()), (1654, 2339), "never upscales or shrinks when within budget");
        let p = prepare_image(&img, g, 1_024).unwrap();
        assert!(p.url.starts_with("data:image/jpeg;base64,") && p.tokens <= 1_024);
    }

    #[test]
    fn gemma4_geometry_uses_48px_tokens_and_caps_at_1120() {
        let g = VisionGeometry::GEMMA4;
        // The same A4 render is 35×49 = 1,715 Gemma tokens → over the 1,120 ceiling.
        assert_eq!(g.tokens_for(1654, 2339), 35 * 49);
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(1654, 2339, Rgb([255, 255, 255])));
        // A 3,072-token fine-print budget is clamped to the encoder ceiling.
        let p = prepare_image(&img, g, 3_072).unwrap();
        assert!(p.tokens <= 1_120, "{} tokens", p.tokens);
        assert!(p.width >= 1_200, "keeps far more pixels than the Qwen math would: {}", p.width);
        // A 1,024-token budget keeps 2.3 MP (Qwen geometry would shrink the page to 1 MP).
        let q = prepare_image(&img, g, 1_024).unwrap();
        assert!(q.tokens <= 1_024 && q.width * q.height > 2_000_000, "{}x{}", q.width, q.height);
        assert_eq!(g.clamp_budget(16), 64);
        assert_eq!(VisionGeometry::for_arch("gemma4"), VisionGeometry::GEMMA4);
        assert_eq!(VisionGeometry::for_arch("qwen35"), VisionGeometry::QWEN);
        assert_eq!(VisionGeometry::for_profile("qwen3-vl-doc-v1"), VisionGeometry::QWEN);
        assert_eq!(VisionGeometry::for_arch("mystery"), VisionGeometry::GEMMA4);
    }
}
