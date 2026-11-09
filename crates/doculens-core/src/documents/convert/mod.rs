//! Office formats → `DocModel` → HTML → PDF (design §3.4):
//!
//! * HWPX (OWPML, zip + XML) and DOCX (WordprocessingML, zip + XML) are read
//!   by small purpose-built readers on `zip` + `quick-xml` — paragraphs, runs,
//!   headings, tables with merged cells, images and page breaks.
//! * HWP 5.0 (binary CFB) is read through the `hwp` crate (Apache-2.0).
//! * XLSX (SpreadsheetML: sheets → tables) and PPTX (PresentationML: slides →
//!   headings, text, tables, pictures) use the same `zip` + `quick-xml` readers.
//! * The plain-text family (txt / md / csv / tsv / log / json / xml / html) is
//!   decoded (UTF-8 / UTF-16 BOM / CP949) and shaped into paragraphs and tables.
//! * The HTML bundle is printed to PDF by a [`PdfPrinter`] the shell provides
//!   (the platform webview); the core only knows the trait.
//!
//! Text is extracted straight from the model, so converted documents are
//! searchable without OCR even when printing fails.

pub mod doc;
pub mod docx;
pub mod hml;
pub mod html;
pub mod hwp;
pub mod hwpx;
pub mod model;
pub mod ppt;
pub mod pptx;
pub mod rtf;
pub mod text;
pub mod xls;
pub mod xlsx;
pub mod xml;

use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};

use super::SourceKind;
pub use model::DocModel;

/// Prints an HTML file (with relative resources next to it) to a PDF.
/// Sheet size the HTML bundle is printed on (millimetres). Slide decks are landscape 16:9 / 4:3,
/// office documents follow their own page setup; A4 when nothing is known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize {
    pub width_mm: f32,
    pub height_mm: f32,
}

impl PageSize {
    pub const A4: PageSize = PageSize { width_mm: 210.0, height_mm: 297.0 };

    /// The size the HTML emitter laid the model out for.
    pub fn for_model(model: &DocModel) -> PageSize {
        let (w, h) = html::page_size_mm(model);
        PageSize { width_mm: w, height_mm: h }
    }

    pub fn landscape(&self) -> bool {
        self.width_mm > self.height_mm
    }

    /// Width / height in PDF points.
    pub fn points(&self) -> (f32, f32) {
        (self.width_mm / 25.4 * 72.0, self.height_mm / 25.4 * 72.0)
    }
}

pub trait PdfPrinter: Send + Sync {
    /// Prints `html` to `pdf` on sheets of `page` (the bundle's `@page` rule says the same size).
    fn print_to_pdf(&self, html: &Path, pdf: &Path, page: PageSize) -> Result<()>;
    /// Human-readable backend name for notes / logs.
    fn name(&self) -> &'static str;
}

/// Printer used when no webview is available (tests, CLI). Writes a minimal
/// PDF whose page count follows the model's estimate so downstream rendering
/// still works; the pages are blank except for a note.
pub struct StubPdfPrinter;

impl PdfPrinter for StubPdfPrinter {
    fn print_to_pdf(&self, html: &Path, pdf: &Path, page: PageSize) -> Result<()> {
        let html_text = std::fs::read_to_string(html).map_err(|e| CoreError::io(html, e))?;
        // Slide decks: one page per section; flow documents: one page per explicit break plus one.
        let slides = html_text.matches("<section class=\"slide").count() as u32;
        let pages = if slides > 0 { slides } else { html_text.matches("class=\"page-break\"").count() as u32 + 1 };
        write_blank_pdf_sized(pdf, pages, &[], page.points())
    }
    fn name(&self) -> &'static str {
        "stub"
    }
}

/// Writes a valid single-font PDF with `pages` A4 pages; `texts[i]` (ASCII
/// only) is drawn on page `i`. Used by the stub printer and by tests.
pub fn write_blank_pdf(path: &Path, pages: u32, texts: &[String]) -> Result<()> {
    write_blank_pdf_sized(path, pages, texts, PageSize::A4.points())
}

/// [`write_blank_pdf`] with a page size in points (width, height).
pub fn write_blank_pdf_sized(path: &Path, pages: u32, texts: &[String], (w_pt, h_pt): (f32, f32)) -> Result<()> {
    let pages = pages.max(1);
    let mut objects: Vec<Vec<u8>> = Vec::new();
    // 1: catalog, 2: pages, 3: font, then page/content pairs.
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let mut kids = String::new();
    for i in 0..pages {
        kids.push_str(&format!("{} 0 R ", 4 + i * 2));
    }
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>").into_bytes());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for i in 0..pages {
        let content_obj = 5 + i * 2;
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w_pt:.2} {h_pt:.2}] /Resources << /Font << /F1 3 0 R >> >> /Contents {content_obj} 0 R >>"
            )
            .into_bytes(),
        );
        let text = texts.get(i as usize).map(|s| s.as_str()).unwrap_or("");
        let safe: String = text
            .chars()
            .filter(|c| c.is_ascii() && !c.is_ascii_control())
            .map(|c| match c {
                '(' | ')' | '\\' => format!("\\{c}"),
                c => c.to_string(),
            })
            .collect();
        let stream = if safe.is_empty() {
            String::new()
        } else {
            format!("BT /F1 12 Tf 56 {:.2} Td ({safe}) Tj ET", h_pt - 61.89)
        };
        objects.push(format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()).into_bytes());
    }
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(obj);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes(),
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    std::fs::write(path, out).map_err(|e| CoreError::io(path, e))
}

/// Reads an office document into the model.
pub fn read_document(path: &Path, kind: SourceKind) -> Result<DocModel> {
    match kind {
        SourceKind::Hwpx => hwpx::read(path),
        SourceKind::Docx => docx::read(path),
        SourceKind::Hwp => hwp::read(path),
        SourceKind::Xlsx => xlsx::read(path),
        SourceKind::Pptx => pptx::read(path),
        SourceKind::Text => text::read(path),
        SourceKind::Xls => xls::read(path),
        SourceKind::Doc => doc::read(path),
        SourceKind::Ppt => ppt::read(path),
        SourceKind::Rtf => rtf::read(path),
        SourceKind::Hml => hml::read(path),
        SourceKind::Pdf | SourceKind::Image => {
            Err(CoreError::Unsupported(format!("{} is not an office document", kind.as_str())))
        }
    }
}

/// Result of a conversion: the model (text/structure), the HTML bundle and —
/// when printing succeeded — the PDF.
#[derive(Debug)]
pub struct Conversion {
    pub model: DocModel,
    pub html: PathBuf,
    pub pdf: Option<PathBuf>,
    pub print_error: Option<String>,
    pub printer: &'static str,
}

/// Reads `source`, writes the HTML bundle into `out_dir` and prints it to
/// `out_dir/document.pdf` with `printer`. A printing failure is reported in
/// `print_error` rather than failing the whole conversion (the text is still
/// usable, design §3.4 "변환본" semantics).
pub fn convert(source: &Path, kind: SourceKind, out_dir: &Path, printer: &dyn PdfPrinter) -> Result<Conversion> {
    let model = read_document(source, kind)?;
    if out_dir.exists() {
        std::fs::remove_dir_all(out_dir).map_err(|e| CoreError::io(out_dir, e))?;
    }
    let html = html::write_bundle(&model, out_dir)?;
    let pdf = out_dir.join("document.pdf");
    let (pdf, print_error) = match printer.print_to_pdf(&html, &pdf, PageSize::for_model(&model)) {
        Ok(()) if pdf.is_file() && std::fs::metadata(&pdf).map(|m| m.len() > 0).unwrap_or(false) => (Some(pdf), None),
        Ok(()) => (None, Some("printer produced no PDF".to_string())),
        Err(e) => (None, Some(e.to_string())),
    };
    Ok(Conversion { model, html, pdf, print_error, printer: printer.name() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_printer_prints_slides_on_landscape_sheets() {
        let dir = tempfile::tempdir().unwrap();
        let png = pptx::tests::png_2x2();
        let deck = dir.path().join("deck.pptx");
        pptx::write_sample_pptx(&deck, &png).unwrap();
        let out = dir.path().join("out");
        let conv = convert(&deck, SourceKind::Pptx, &out, &StubPdfPrinter).unwrap();
        let page = PageSize::for_model(&conv.model);
        assert!(page.landscape());
        assert!((page.width_mm - 338.67).abs() < 0.1 && (page.height_mm - 190.5).abs() < 0.1);
        let pdf = std::fs::read(conv.pdf.expect("stub pdf")).unwrap();
        let text = String::from_utf8_lossy(&pdf);
        // Two slides + one notes page, each on a 960 × 540 pt sheet.
        assert_eq!(text.matches("/Type /Page ").count(), 3, "{text}");
        assert!(text.contains("/MediaBox [0 0 960.00 540.00]"), "{text}");
        let html = std::fs::read_to_string(&conv.html).unwrap();
        assert!(html.contains("@page { size: 338.67mm 190.50mm"));
        assert!(html.contains("<img src=\"img/img1.png\""));
        assert!(out.join("img").join("img1.png").is_file());
        // A flow document stays A4 unless it declares a size.
        assert_eq!(PageSize::for_model(&DocModel::default()), PageSize::A4);
        let (w_pt, h_pt) = PageSize::A4.points();
        assert!((w_pt - 595.28).abs() < 0.01 && (h_pt - 841.89).abs() < 0.01);
    }

    #[test]
    fn blank_pdf_is_well_formed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.pdf");
        write_blank_pdf(&p, 3, &["Hello (world)".into()]).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert!(bytes.ends_with(b"%%EOF\n"));
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("/Count 3"));
        assert!(s.contains("(Hello \\(world\\)) Tj"));
    }
}
