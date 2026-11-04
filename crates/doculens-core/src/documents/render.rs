//! Page rendering and the page-image cache (design §3.4 / §3.6).
//!
//! * PDFs are rasterised by PDFium through `pdfium-render`; the library
//!   (`pdfium.dll` / `libpdfium.so`, bblanchon/pdfium-binaries) is loaded at
//!   runtime from a path the shell resolves. pdfium-render binds the library
//!   once per process, so a single [`RenderService`] owns it and runs every
//!   request on one worker thread.
//! * Images (JPG/PNG/TIFF/WebP/BMP/GIF) are decoded with the `image` crate,
//!   EXIF orientation applied, and treated as one page.
//!
//! Cached files: `cache/pages/<doc>/p0001.jpg` (quality 90, long edge ≤
//! `max_long_edge`) and `cache/thumbs/<doc>/p0001.jpg` (long edge 320).

use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageDecoder, RgbImage};
use pdfium_render::prelude::*;

use crate::error::{CoreError, Result};

use super::{DocumentCache, NewPage};

pub const DEFAULT_DPI: u16 = 200;
/// Hard cap on the long edge of cached page images.
pub const MAX_LONG_EDGE: u32 = 4096;
/// Long edge of a cached image page when no DPI applies (A4 at ~300 dpi).
pub const IMAGE_LONG_EDGE: u32 = 3508;
pub const THUMB_LONG_EDGE: u32 = 320;
pub const PAGE_JPEG_QUALITY: u8 = 90;
pub const THUMB_JPEG_QUALITY: u8 = 80;
/// Pages per worker job while rendering a whole document (keeps the worker
/// responsive to single-page requests from the viewer).
const CHUNK_PAGES: u32 = 8;
/// Environment variable naming the PDFium library (file or directory).
pub const PDFIUM_ENV: &str = "DOCULENS_PDFIUM";

#[derive(Debug, Clone, PartialEq)]
pub struct RenderOptions {
    pub dpi: u16,
    pub max_long_edge: u32,
    pub thumbnails: bool,
    /// Extract the PDF text layer into `NewPage::text`.
    pub extract_text: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { dpi: DEFAULT_DPI, max_long_edge: MAX_LONG_EDGE, thumbnails: true, extract_text: true }
    }
}

/// Finds the PDFium shared library: `DOCULENS_PDFIUM` (file or directory),
/// then `candidates` (each a directory or a file).
pub fn locate_pdfium(candidates: &[PathBuf]) -> Option<PathBuf> {
    let lib_name = Pdfium::pdfium_platform_library_name();
    let resolve = |p: &Path| -> Option<PathBuf> {
        if p.is_file() {
            return Some(p.to_path_buf());
        }
        let f = p.join(&lib_name);
        if f.is_file() {
            return Some(f);
        }
        let f = p.join("lib").join(&lib_name);
        if f.is_file() {
            return Some(f);
        }
        None
    };
    if let Some(env) = std::env::var_os(PDFIUM_ENV) {
        if let Some(p) = resolve(Path::new(&env)) {
            return Some(p);
        }
    }
    candidates.iter().find_map(|c| resolve(c))
}

type Job = Box<dyn FnOnce(&Pdfium) + Send>;

/// Process-wide PDFium worker. Create with [`RenderService::global`].
pub struct RenderService {
    tx: Mutex<mpsc::Sender<Job>>,
    pub library: PathBuf,
}

static GLOBAL: OnceLock<RenderService> = OnceLock::new();
static GLOBAL_INIT: Mutex<()> = Mutex::new(());

impl RenderService {
    /// Binds the library once per process. Later calls return the same
    /// service (the path is ignored after the first successful bind); a failed
    /// bind is not cached, so a later call with a fixed installation can succeed.
    pub fn global(library: &Path) -> Result<&'static RenderService> {
        if let Some(s) = GLOBAL.get() {
            return Ok(s);
        }
        let _guard = GLOBAL_INIT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = GLOBAL.get() {
            return Ok(s);
        }
        let service = Self::start(library)?;
        Ok(GLOBAL.get_or_init(|| service))
    }

    /// The service if it was already started.
    pub fn get() -> Option<&'static RenderService> {
        GLOBAL.get()
    }

    fn start(library: &Path) -> Result<RenderService> {
        if !library.is_file() {
            return Err(CoreError::Render(format!("pdfium library not found at {}", library.display())));
        }
        let bindings = Pdfium::bind_to_library(library)
            .map_err(|e| CoreError::Render(format!("cannot load {}: {e:?}", library.display())))?;
        let pdfium = Pdfium::new(bindings);
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("doculens-pdfium".into())
            .spawn(move || {
                let pdfium = pdfium;
                while let Ok(job) = rx.recv() {
                    // A panicking job must not take the (irreplaceable) worker down with it.
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&pdfium)));
                    if outcome.is_err() {
                        log::error!("pdfium worker: a render job panicked; the worker keeps running");
                    }
                }
            })
            .map_err(|e| CoreError::Render(format!("pdfium worker: {e}")))?;
        Ok(RenderService { tx: Mutex::new(tx), library: library.to_path_buf() })
    }

    /// Runs `f` on the worker thread and waits for its result. A panic inside
    /// `f` surfaces as a render error (the job's reply channel is dropped).
    pub fn run<T: Send + 'static>(&self, f: impl FnOnce(&Pdfium) -> Result<T> + Send + 'static) -> Result<T> {
        let (rtx, rrx) = mpsc::channel::<Result<T>>();
        let job: Job = Box::new(move |p| {
            let _ = rtx.send(f(p));
        });
        self.tx
            .lock()
            .map_err(|_| CoreError::Render("render queue poisoned".into()))?
            .send(job)
            .map_err(|_| CoreError::Render("render worker stopped".into()))?;
        rrx.recv().map_err(|_| CoreError::Render("render job aborted (renderer panic)".into()))?
    }
}

fn pdfium_err(what: &str, e: PdfiumError) -> CoreError {
    let text = format!("{e:?}");
    if text.contains("PasswordError") {
        return CoreError::Unsupported("password_protected".into());
    }
    if text.contains("FormatError") || text.contains("FileError") {
        return CoreError::Render(format!("{what}: not a readable PDF ({text})"));
    }
    CoreError::Render(format!("{what}: {text}"))
}

fn rotation_of(deg: u16) -> PdfPageRenderRotation {
    match deg % 360 {
        90 => PdfPageRenderRotation::Degrees90,
        180 => PdfPageRenderRotation::Degrees180,
        270 => PdfPageRenderRotation::Degrees270,
        _ => PdfPageRenderRotation::None,
    }
}

/// Scale factor for `dpi` capped so the long edge stays within `max_long_edge`.
fn scale_for(width_pt: f32, height_pt: f32, dpi: u16, max_long_edge: u32) -> f32 {
    let scale = dpi.max(36) as f32 / 72.0;
    let long = width_pt.max(height_pt).max(1.0);
    let max = max_long_edge.max(64) as f32;
    if long * scale > max {
        max / long
    } else {
        scale
    }
}

/// Writes `img` as JPEG; returns (width, height).
pub fn write_jpeg(img: &RgbImage, path: &Path, quality: u8) -> Result<(u32, u32)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    let tmp = path.with_extension("jpg.part");
    {
        let file = File::create(&tmp).map_err(|e| CoreError::io(&tmp, e))?;
        let mut enc = JpegEncoder::new_with_quality(BufWriter::new(file), quality);
        enc.encode_image(img).map_err(|e| CoreError::Render(format!("jpeg {}: {e}", path.display())))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| CoreError::io(path, e))?;
    Ok((img.width(), img.height()))
}

/// Downscales so the long edge is at most `long_edge` (never upscales).
pub fn fit_long_edge(img: &DynamicImage, long_edge: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let long = w.max(h);
    if long <= long_edge || long == 0 {
        return img.clone();
    }
    let f = long_edge as f64 / long as f64;
    let nw = ((w as f64 * f).round() as u32).max(1);
    let nh = ((h as f64 * f).round() as u32).max(1);
    img.resize(nw, nh, image::imageops::FilterType::Triangle)
}

/// A page is blank when almost no pixel carries ink: scanner noise is light
/// grey, so only clearly dark pixels count, and a single short line of 12 pt
/// text at 72 dpi (~0.2 % of an A4 page) must already register as content.
pub fn is_blank_page(img: &RgbImage) -> bool {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return true;
    }
    let step = ((w * h) / 200_000).max(1) as usize; // sample ≤ ~200k pixels
    let mut dark = 0usize;
    let mut total = 0usize;
    for (i, p) in img.pixels().enumerate() {
        if i % step != 0 {
            continue;
        }
        total += 1;
        let luma = (p.0[0] as u32 * 299 + p.0[1] as u32 * 587 + p.0[2] as u32 * 114) / 1000;
        if luma < 160 {
            dark += 1;
        }
    }
    total > 0 && (dark as f64 / total as f64) < 0.0003
}

fn write_page_files(
    img: &DynamicImage,
    cache: &DocumentCache,
    page_no: u32,
    opts: &RenderOptions,
) -> Result<(u32, u32, bool)> {
    let fitted = fit_long_edge(img, opts.max_long_edge);
    let rgb = fitted.to_rgb8();
    let (w, h) = write_jpeg(&rgb, &cache.page_file(page_no), PAGE_JPEG_QUALITY)?;
    if opts.thumbnails {
        let thumb = fit_long_edge(&fitted, THUMB_LONG_EDGE).to_rgb8();
        write_jpeg(&thumb, &cache.thumb_file(page_no), THUMB_JPEG_QUALITY)?;
    }
    Ok((w, h, is_blank_page(&rgb)))
}

/// Number of pages in a PDF (also validates that PDFium can open it).
pub fn pdf_page_count(svc: &RenderService, pdf: &Path) -> Result<u32> {
    let pdf = pdf.to_path_buf();
    svc.run(move |p| {
        let doc = p.load_pdf_from_file(&pdf, None).map_err(|e| pdfium_err("open", e))?;
        Ok(doc.pages().len().max(0) as u32)
    })
}

/// Renders every page of `pdf` into the cache. `rotations` holds per-page
/// user rotations; `progress(done, total)` runs between chunks and `cancel`
/// stops the job (→ `CoreError::Cancelled`).
pub fn render_pdf(
    svc: &RenderService,
    pdf: &Path,
    cache: &DocumentCache,
    opts: &RenderOptions,
    rotations: &HashMap<u32, u16>,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(u32, u32),
) -> Result<Vec<NewPage>> {
    let total = pdf_page_count(svc, pdf)?;
    let mut pages = Vec::with_capacity(total as usize);
    let mut first = 1u32;
    progress(0, total);
    while first <= total {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(CoreError::Cancelled);
        }
        let last = (first + CHUNK_PAGES - 1).min(total);
        let chunk = render_pdf_range(svc, pdf, cache, opts, rotations, first, last)?;
        pages.extend(chunk);
        progress(last, total);
        first = last + 1;
    }
    Ok(pages)
}

/// Renders pages `first..=last` (1-based) in one worker job.
pub fn render_pdf_range(
    svc: &RenderService,
    pdf: &Path,
    cache: &DocumentCache,
    opts: &RenderOptions,
    rotations: &HashMap<u32, u16>,
    first: u32,
    last: u32,
) -> Result<Vec<NewPage>> {
    let pdf = pdf.to_path_buf();
    let cache = cache.clone();
    let opts = opts.clone();
    let rotations = rotations.clone();
    svc.run(move |p| {
        let doc = p.load_pdf_from_file(&pdf, None).map_err(|e| pdfium_err("open", e))?;
        let count = doc.pages().len().max(0) as u32;
        let mut out = Vec::new();
        for page_no in first.max(1)..=last.min(count) {
            let page = doc.pages().get((page_no - 1) as PdfPageIndex).map_err(|e| pdfium_err("page", e))?;
            let rotation = rotations.get(&page_no).copied().unwrap_or(0) % 360;
            out.push(render_one(&page, &cache, page_no, rotation, &opts)?);
        }
        Ok(out)
    })
}

fn render_one(
    page: &PdfPage<'_>,
    cache: &DocumentCache,
    page_no: u32,
    rotation: u16,
    opts: &RenderOptions,
) -> Result<NewPage> {
    let (w_pt, h_pt) = (page.width().value, page.height().value);
    let scale = scale_for(w_pt, h_pt, opts.dpi, opts.max_long_edge);
    let cfg = PdfRenderConfig::new()
        .scale_page_by_factor(scale)
        .rotate(rotation_of(rotation), true)
        .render_form_data(true)
        .set_clear_color(PdfColor::WHITE);
    let bitmap = page.render_with_config(&cfg).map_err(|e| pdfium_err("render", e))?;
    let img = bitmap.as_image().map_err(|e| pdfium_err("bitmap", e))?;
    let (w, h, blank) = write_page_files(&img, cache, page_no, opts)?;
    let text = if opts.extract_text {
        page.text().ok().map(|t| t.all()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    } else {
        None
    };
    let effective_dpi = (scale * 72.0).round() as u16;
    Ok(NewPage {
        page_no,
        width_px: w,
        height_px: h,
        width_pt: Some(w_pt as f64),
        height_pt: Some(h_pt as f64),
        dpi: Some(effective_dpi),
        rotation,
        text,
        is_blank: Some(blank),
    })
}

/// Re-renders a single PDF page (viewer rotation / DPI change).
pub fn render_pdf_page(
    svc: &RenderService,
    pdf: &Path,
    cache: &DocumentCache,
    opts: &RenderOptions,
    page_no: u32,
    rotation: u16,
) -> Result<NewPage> {
    let mut rot = HashMap::new();
    rot.insert(page_no, rotation);
    render_pdf_range(svc, pdf, cache, opts, &rot, page_no, page_no)?
        .pop()
        .ok_or_else(|| CoreError::NotFound(format!("page {page_no}")))
}

/// Decodes an image file applying its EXIF orientation.
pub fn decode_image(path: &Path) -> Result<DynamicImage> {
    let reader = image::ImageReader::open(path)
        .map_err(|e| CoreError::io(path, e))?
        .with_guessed_format()
        .map_err(|e| CoreError::io(path, e))?;
    let mut decoder = reader.into_decoder().map_err(|e| CoreError::Render(format!("{}: {e}", path.display())))?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img =
        DynamicImage::from_decoder(decoder).map_err(|e| CoreError::Render(format!("{}: {e}", path.display())))?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn rotate_image(img: DynamicImage, rotation: u16) -> DynamicImage {
    match rotation % 360 {
        90 => img.rotate90(),
        180 => img.rotate180(),
        270 => img.rotate270(),
        _ => img,
    }
}

/// Renders an image document (one page) into the cache.
pub fn render_image(src: &Path, cache: &DocumentCache, opts: &RenderOptions, rotation: u16) -> Result<Vec<NewPage>> {
    let img = rotate_image(decode_image(src)?, rotation);
    let long_edge = opts.max_long_edge.min(IMAGE_LONG_EDGE);
    let page_opts = RenderOptions { max_long_edge: long_edge, ..opts.clone() };
    let (w, h, blank) = write_page_files(&img, cache, 1, &page_opts)?;
    Ok(vec![NewPage {
        page_no: 1,
        width_px: w,
        height_px: h,
        width_pt: None,
        height_pt: None,
        dpi: None,
        rotation: rotation % 360,
        text: None,
        is_blank: Some(blank),
    }])
}

/// Removes cached page / thumbnail files of pages beyond `keep` (after a
/// re-render that produced fewer pages).
pub fn prune_cache(cache: &DocumentCache, keep: u32) {
    for dir in [&cache.pages_dir, &cache.thumbs_dir] {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let no = name.strip_prefix('p').and_then(|s| s.strip_suffix(".jpg")).and_then(|s| s.parse::<u32>().ok());
            if no.is_some_and(|n| n > keep) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Convenience for tests / tools: the library next to the executable, in
/// `DOCULENS_PDFIUM`, or in the repo-local `vendor/pdfium` folders.
pub fn default_pdfium_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.to_path_buf());
            v.push(dir.join("pdfium"));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::convert::write_blank_pdf;
    use crate::paths::WorkspacePaths;
    use image::Rgb;

    fn service() -> Option<&'static RenderService> {
        let lib = locate_pdfium(&default_pdfium_candidates())?;
        RenderService::global(&lib).ok()
    }

    #[test]
    fn scale_caps_long_edge() {
        assert!((scale_for(595.0, 842.0, 200, 4096) - 200.0 / 72.0).abs() < 1e-5);
        assert!((scale_for(595.0, 842.0, 600, 2048) - 2048.0 / 842.0).abs() < 1e-5);
    }

    #[test]
    fn blank_detection_and_jpeg_round_trip() {
        let mut img = RgbImage::from_pixel(400, 300, Rgb([255, 255, 255]));
        assert!(is_blank_page(&img));
        for x in 0..400 {
            for y in 100..140 {
                img.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
        assert!(!is_blank_page(&img));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("p.jpg");
        assert_eq!(write_jpeg(&img, &path, 85).unwrap(), (400, 300));
        let back = decode_image(&path).unwrap();
        assert_eq!((back.width(), back.height()), (400, 300));
        let small = fit_long_edge(&back, 100);
        assert_eq!((small.width(), small.height()), (100, 75));
        assert_eq!(fit_long_edge(&back, 1000).width(), 400, "never upscale");
    }

    #[test]
    fn image_document_renders_with_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("photo.png");
        let img = RgbImage::from_pixel(640, 480, Rgb([250, 250, 250]));
        img.save(&src).unwrap();
        let ws = WorkspacePaths::new(dir.path().join("ws"));
        let cache = DocumentCache::for_document(&ws, "doc1");
        let pages = render_image(&src, &cache, &RenderOptions::default(), 90).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!((pages[0].width_px, pages[0].height_px), (480, 640));
        assert_eq!(pages[0].rotation, 90);
        assert_eq!(pages[0].is_blank, Some(true));
        assert!(cache.page_file(1).is_file() && cache.thumb_file(1).is_file());
        let thumb = decode_image(&cache.thumb_file(1)).unwrap();
        assert_eq!(thumb.height(), 320);
        prune_cache(&cache, 0);
        assert!(!cache.page_file(1).exists());
    }

    #[test]
    fn pdf_renders_pages_text_and_thumbnails() {
        let Some(svc) = service() else {
            eprintln!("render tests: skipped (set DOCULENS_PDFIUM to the pdfium library)");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("doc.pdf");
        write_blank_pdf(&pdf, 11, &["Hello DocuLens".into(), String::new(), "Page three".into()]).unwrap();
        assert_eq!(pdf_page_count(svc, &pdf).unwrap(), 11);
        let ws = WorkspacePaths::new(dir.path().join("ws"));
        let cache = DocumentCache::for_document(&ws, "doc1");
        let mut reports = Vec::new();
        let opts = RenderOptions { dpi: 72, ..Default::default() };
        let pages =
            render_pdf(svc, &pdf, &cache, &opts, &HashMap::new(), None, &mut |d, t| reports.push((d, t))).unwrap();
        assert_eq!(pages.len(), 11);
        assert_eq!(reports.first(), Some(&(0, 11)));
        assert_eq!(reports.last(), Some(&(11, 11)));
        assert_eq!((pages[0].width_px, pages[0].height_px), (595, 842));
        assert_eq!(pages[0].text.as_deref(), Some("Hello DocuLens"));
        assert_eq!(pages[1].text, None);
        assert_eq!(pages[0].is_blank, Some(false));
        assert_eq!(pages[1].is_blank, Some(true));
        assert!(cache.page_file(11).is_file() && cache.thumb_file(11).is_file());
        // Rotate one page: dimensions swap, others untouched.
        let p3 = render_pdf_page(svc, &pdf, &cache, &opts, 3, 90).unwrap();
        assert_eq!((p3.width_px, p3.height_px), (842, 595));
        assert_eq!(p3.text.as_deref(), Some("Page three"));
        // Not a PDF → render error, not a panic.
        std::fs::write(&pdf, b"nope").unwrap();
        assert!(matches!(pdf_page_count(svc, &pdf), Err(CoreError::Render(_))));
        // Cancellation between chunks.
        write_blank_pdf(&pdf, 20, &[]).unwrap();
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            render_pdf(svc, &pdf, &cache, &opts, &HashMap::new(), Some(&cancel), &mut |_, _| {}),
            Err(CoreError::Cancelled)
        ));
    }
}
