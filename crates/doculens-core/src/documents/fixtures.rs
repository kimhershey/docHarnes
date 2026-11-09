//! Sample document generator for manual testing and the headless smoke run
//! (`cargo test -p doculens-core --lib -- --ignored gen_fixtures` with
//! `DOCULENS_FIXTURE_OUT=<dir>`). The files are synthetic: a 100-page PDF
//! with a text layer, a 3-section HWPX, a 2-page DOCX and a photo-sized JPEG.

use std::path::Path;

use image::{Rgb, RgbImage};

use crate::error::Result;

use super::convert::{docx, hwpx, pptx, text, write_blank_pdf, xlsx};

/// 1×1 red PNG (embedded image for the office fixtures).
pub const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49,
    0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0,
    0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// Writes the sample set into `dir` and returns the file names.
pub fn write_sample_set(dir: &Path) -> Result<Vec<String>> {
    std::fs::create_dir_all(dir).map_err(|e| crate::CoreError::io(dir, e))?;
    let texts: Vec<String> = (1..=100).map(|i| format!("DocuLens sample page {i} of 100")).collect();
    write_blank_pdf(&dir.join("sample-100p.pdf"), 100, &texts)?;
    hwpx::write_sample_hwpx(&dir.join("sample-quote.hwpx"), 3, TINY_PNG)?;
    docx::write_sample_docx(&dir.join("sample-contract.docx"), 2, TINY_PNG)?;
    xlsx::write_sample_xlsx(&dir.join("sample-ledger.xlsx"))?;
    pptx::write_sample_pptx(&dir.join("sample-plan.pptx"), TINY_PNG)?;
    let mut names = text::write_sample_texts(dir)?;
    let mut img = RgbImage::from_pixel(1200, 1600, Rgb([245, 245, 240]));
    for y in (200..1400).step_by(60) {
        for x in 120..1080 {
            if (x / 40) % 3 != 2 {
                img.put_pixel(x, y, Rgb([30, 30, 30]));
                img.put_pixel(x, y + 1, Rgb([30, 30, 30]));
            }
        }
    }
    let photo = dir.join("sample-photo.jpg");
    img.save(&photo).map_err(|e| crate::CoreError::Render(e.to_string()))?;
    names.extend([
        "sample-100p.pdf".to_string(),
        "sample-quote.hwpx".into(),
        "sample-contract.docx".into(),
        "sample-ledger.xlsx".into(),
        "sample-plan.pptx".into(),
        "sample-photo.jpg".into(),
    ]);
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_set_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let names = write_sample_set(dir.path()).unwrap();
        assert_eq!(names.len(), 8);
        for n in &names {
            assert!(dir.path().join(n).metadata().unwrap().len() > 100, "{n}");
        }
    }

    /// `DOCULENS_FIXTURE_OUT=/some/dir cargo test -p doculens-core --lib -- --ignored gen_fixtures`
    #[test]
    #[ignore]
    fn gen_fixtures() {
        let out = std::env::var_os("DOCULENS_FIXTURE_OUT").expect("set DOCULENS_FIXTURE_OUT");
        let names = write_sample_set(Path::new(&out)).unwrap();
        eprintln!("wrote {names:?} to {}", Path::new(&out).display());
    }
}
