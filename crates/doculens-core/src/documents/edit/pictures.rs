//! Pictures of a package (1.0.0 §2-1, step 4): what there is to replace, and the bytes a replacement
//! puts into the package.
//!
//! A picture is a part of its own (`word/media/image1.png`, `ppt/media/image3.jpeg`, `xl/media/…`,
//! HWPX `BinData/image1.png`); the frame that shows it — size, position, cropping — is in the text
//! parts and is not touched. So a replacement keeps the part's name and its format: a new picture
//! of another format is written in the format of the old one, and the content types, relationships
//! and manifests of the package stay true as they are.

use std::io::{Cursor, Read};
use std::path::Path;

use image::{DynamicImage, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};

use super::{Archive, CoreError, Result, SourceKind};

/// Largest picture file read (the new one, or one of the package for its thumbnail).
const MAX_PICTURE_BYTES: u64 = 50 * 1024 * 1024;
/// Pixels above which a picture is listed without a thumbnail (it would have to be decoded whole).
const MAX_THUMBNAIL_PIXELS: u64 = 40_000_000;
/// Longest side of a thumbnail.
const THUMBNAIL_PX: u32 = 160;
/// Pictures listed.
const MAX_PICTURES: usize = 300;

/// Reason codes.
pub const MISSING: &str = "edit_picture_missing";
pub const FORMAT: &str = "edit_picture_format";
pub const UNREADABLE: &str = "edit_picture_unreadable";
/// Note: the new picture's proportions differ from the old one's (the frame keeps the old ones).
pub const RATIO: &str = "edit_picture_ratio";

/// One picture of the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picture {
    /// The part of the package — what a replacement names.
    pub part: String,
    /// `png`, `jpeg`, `gif`, `bmp`, or the extension of a format that cannot be replaced (`emf` …).
    pub format: String,
    /// Pixels (0 when the picture cannot be read).
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    /// Whether a replacement can be written in this format.
    pub replaceable: bool,
    /// The parts that show the picture (`word/document.xml`, `ppt/slides/slide3.xml`, a layout …).
    pub used_in: Vec<String>,
    /// A small PNG as a data URL, for the list.
    pub thumbnail: Option<String>,
}

/// Where a package keeps its pictures.
fn media_folder(kind: SourceKind) -> Option<&'static str> {
    match kind {
        SourceKind::Docx => Some("word/media/"),
        SourceKind::Pptx => Some("ppt/media/"),
        SourceKind::Xlsx => Some("xl/media/"),
        SourceKind::Hwpx => Some("bindata/"),
        _ => None,
    }
}

/// The format of a media part, by its extension: `(name, the format a replacement is written in)`.
fn format_of(part: &str) -> (String, Option<ImageFormat>) {
    let ext = part.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let format = match ext.as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" | "jpe" => Some(ImageFormat::Jpeg),
        "gif" => Some(ImageFormat::Gif),
        "bmp" => Some(ImageFormat::Bmp),
        _ => None,
    };
    let name = match format {
        Some(ImageFormat::Jpeg) => "jpeg".to_string(),
        _ => ext,
    };
    (name, format)
}

fn is_media(kind: SourceKind, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    media_folder(kind).is_some_and(|folder| lower.starts_with(folder) && !lower.ends_with('/'))
}

fn read_entry(archive: &mut Archive, index: usize) -> Result<(String, Vec<u8>)> {
    let mut entry = archive.by_index(index).map_err(|e| CoreError::Convert(format!("zip entry {index}: {e}")))?;
    let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
    let mut bytes = Vec::with_capacity(entry.size().min(MAX_PICTURE_BYTES) as usize);
    entry
        .by_ref()
        .take(MAX_PICTURE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| CoreError::Convert(format!("{name}: {e}")))?;
    Ok((name, bytes))
}

fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()
}

fn decode(bytes: &[u8]) -> Option<DynamicImage> {
    ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?.decode().ok()
}

fn thumbnail(bytes: &[u8], width: u32, height: u32) -> Option<String> {
    if u64::from(width) * u64::from(height) > MAX_THUMBNAIL_PIXELS {
        return None;
    }
    let small = decode(bytes)?.thumbnail(THUMBNAIL_PX, THUMBNAIL_PX);
    let mut png = Vec::new();
    small.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).ok()?;
    Some(crate::engine::client::data_url("image/png", &png))
}

/// `word/_rels/document.xml.rels` → `word/document.xml`.
fn owner_of(rels: &str) -> Option<String> {
    let (folder, file) = rels.rsplit_once('/')?;
    let folder = folder.strip_suffix("_rels")?;
    Some(format!("{folder}{}", file.strip_suffix(".rels")?))
}

/// `ppt/slides` + `../media/image1.png` → `ppt/media/image1.png`.
fn resolve(base: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for step in target.split('/') {
        match step {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// The media parts every relationship part points at: `(owner part, media part)`.
fn uses(archive: &mut Archive) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for index in 0..archive.len() {
        let is_rels = archive
            .by_index(index)
            .ok()
            .and_then(|e| e.name().ok().map(|n| n.to_ascii_lowercase().ends_with(".rels")))
            .unwrap_or(false);
        if !is_rels {
            continue;
        }
        let (name, bytes) = read_entry(archive, index)?;
        let Some(owner) = owner_of(&name) else { continue };
        let base = owner.rsplit_once('/').map(|(folder, _)| folder).unwrap_or("");
        let mut reader = quick_xml::Reader::from_reader(bytes.as_slice());
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(quick_xml::events::Event::Empty(e) | quick_xml::events::Event::Start(e)) => {
                    let external = e
                        .try_get_attribute("TargetMode")
                        .ok()
                        .flatten()
                        .is_some_and(|a| a.value.as_ref().eq_ignore_ascii_case("external"));
                    let target = e.try_get_attribute("Target").ok().flatten().and_then(|a| {
                        a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|v| v.into_owned())
                    });
                    if let (false, Some(target)) = (external, target) {
                        out.push((owner.clone(), resolve(base, &target)));
                    }
                }
                Ok(quick_xml::events::Event::Eof) | Err(_) => break,
                _ => {}
            }
            buf.clear();
        }
    }
    Ok(out)
}

/// The pictures of a document, in the order of the package.
pub fn list(archive: &mut Archive, kind: SourceKind) -> Result<Vec<Picture>> {
    let used = uses(archive)?;
    let mut out = Vec::new();
    for index in 0..archive.len() {
        let (name, size) = {
            let entry = archive.by_index(index).map_err(|e| CoreError::Convert(format!("zip entry {index}: {e}")))?;
            (entry.name().map(|n| n.into_owned()).unwrap_or_default(), entry.size())
        };
        if !is_media(kind, &name) || out.len() >= MAX_PICTURES {
            continue;
        }
        let (format, writable) = format_of(&name);
        let bytes = if size <= MAX_PICTURE_BYTES { read_entry(archive, index)?.1 } else { Vec::new() };
        let (width, height) = dimensions(&bytes).unwrap_or((0, 0));
        let lower = name.to_ascii_lowercase();
        let mut used_in: Vec<String> = used
            .iter()
            .filter(|(_, media)| media.to_ascii_lowercase() == lower)
            .map(|(owner, _)| owner.clone())
            .collect();
        used_in.sort();
        used_in.dedup();
        out.push(Picture {
            thumbnail: if width > 0 { thumbnail(&bytes, width, height) } else { None },
            part: name,
            format,
            width,
            height,
            bytes: size,
            replaceable: writable.is_some() && width > 0,
            used_in,
        });
    }
    Ok(out)
}

/// What a replacement writes into the package, and how the report words it.
pub struct Swap {
    pub bytes: Vec<u8>,
    /// "640×480 PNG"
    pub before: String,
    /// "800×600 JPEG → PNG"
    pub after: String,
    /// The proportions differ by more than a little: the frame will stretch the new picture.
    pub stretched: bool,
}

fn label(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::WebP => "WebP",
        _ => "?",
    }
}

/// The bytes that take the place of the picture `part`: the file at `path`, written in the format of
/// the old picture when its own is another one.
pub fn swap(
    archive: &mut Archive,
    kind: SourceKind,
    part: &str,
    path: &Path,
) -> std::result::Result<Swap, &'static str> {
    let wanted = part.trim().to_ascii_lowercase();
    let index = (0..archive.len())
        .find(|i| {
            archive.by_index(*i).ok().and_then(|e| e.name().ok().map(|n| n.to_ascii_lowercase() == wanted))
                == Some(true)
        })
        .filter(|_| is_media(kind, &wanted))
        .ok_or(MISSING)?;
    let (name, old) = read_entry(archive, index).map_err(|_| MISSING)?;
    let (_, Some(target)) = format_of(&name) else { return Err(FORMAT) };
    let (old_w, old_h) = dimensions(&old).ok_or(FORMAT)?;

    let size = std::fs::metadata(path).map_err(|_| UNREADABLE)?.len();
    if size == 0 || size > MAX_PICTURE_BYTES {
        return Err(UNREADABLE);
    }
    let new = std::fs::read(path).map_err(|_| UNREADABLE)?;
    let reader = ImageReader::new(Cursor::new(new.as_slice())).with_guessed_format().map_err(|_| UNREADABLE)?;
    let source = reader.format().ok_or(UNREADABLE)?;
    let image = reader.decode().map_err(|_| UNREADABLE)?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return Err(UNREADABLE);
    }
    // The same format goes in as it is (no second compression); another one is written anew.
    let bytes = if source == target {
        new
    } else {
        let mut out = Vec::new();
        let written = match target {
            // No transparency in JPEG: what was see-through becomes white.
            ImageFormat::Jpeg => {
                let mut flat = image::RgbImage::from_pixel(width, height, image::Rgb([255, 255, 255]));
                for (x, y, pixel) in image.to_rgba8().enumerate_pixels() {
                    let alpha = u32::from(pixel[3]);
                    let mix = |c: u8| ((u32::from(c) * alpha + 255 * (255 - alpha)) / 255) as u8;
                    flat.put_pixel(x, y, image::Rgb([mix(pixel[0]), mix(pixel[1]), mix(pixel[2])]));
                }
                let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90);
                encoder.encode_image(&flat)
            }
            other => DynamicImage::ImageRgba8(image.to_rgba8()).write_to(&mut Cursor::new(&mut out), other),
        };
        written.map_err(|_| UNREADABLE)?;
        out
    };
    let ratio = |w: u32, h: u32| f64::from(w) / f64::from(h.max(1));
    let (was, is) = (ratio(old_w, old_h), ratio(width, height));
    Ok(Swap {
        bytes,
        before: format!("{old_w}×{old_h} {}", label(target)),
        after: if source == target {
            format!("{width}×{height} {}", label(source))
        } else {
            format!("{width}×{height} {} → {}", label(source), label(target))
        },
        stretched: (was - is).abs() / was.max(f64::EPSILON) > 0.02,
    })
}
