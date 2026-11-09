//! Content-based file type detection for imports. Extensions lie — downloads
//! named `.pdf` that are HTML pages, HWPX saved as `.zip`, files without an
//! extension, legacy `.doc` renamed `.docx` — so the import planner reads the
//! first bytes and, for zip / CFB containers, the entry names, and lets the
//! content decide (`import::classify`).

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use super::SourceKind;

/// What the bytes say about a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sniff {
    /// A format the pipeline reads. `Kind(Text)` is a weak signal (any printable bytes qualify):
    /// `classify` only lets it promote files without an extension.
    Kind(SourceKind),
    /// An HTML page (`<!DOCTYPE html` / `<html`): read as text whatever the extension says.
    Html,
    /// Recognised but not importable; the string is the `documents.unsupported.*` reason key.
    Unsupported(&'static str),
    /// No recognisable signature (the extension decides).
    Unknown,
}

const HEAD_BYTES: usize = 64 * 1024;

fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut f = File::open(path).ok()?;
    let mut buf = vec![0u8; HEAD_BYTES];
    let mut filled = 0;
    while filled < buf.len() {
        match f.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return None,
        }
    }
    buf.truncate(filled);
    Some(buf)
}

/// UTF-16LE encoding of an ASCII stream name (CFB directory entries).
fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// OOXML / OWPML packages share the zip container: the entry prefixes tell them apart.
fn sniff_zip(path: &Path) -> Sniff {
    let Ok(file) = File::open(path) else { return Sniff::Unknown };
    let Ok(mut archive) = zip::ZipArchive::new(BufReader::new(file)) else { return Sniff::Unsupported("archive") };
    let names: Vec<String> = archive.file_names().filter_map(|n| n.ok().map(|c| c.into_owned())).collect();
    let has = |prefix: &str| {
        names.iter().any(|n| n.starts_with(prefix) || n.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase()))
    };
    if has("word/") {
        return Sniff::Kind(SourceKind::Docx);
    }
    if has("xl/") {
        return Sniff::Kind(SourceKind::Xlsx);
    }
    if has("ppt/") {
        return Sniff::Kind(SourceKind::Pptx);
    }
    if names.iter().any(|n| n.eq_ignore_ascii_case("mimetype")) {
        if let Ok(mut e) = archive.by_name("mimetype") {
            let mut m = String::new();
            if e.by_ref().take(256).read_to_string(&mut m).is_ok() && m.contains("hwp+zip") {
                return Sniff::Kind(SourceKind::Hwpx);
            }
        }
    }
    if has("Contents/") {
        return Sniff::Kind(SourceKind::Hwpx);
    }
    Sniff::Unsupported("archive")
}

/// CFB (OLE2) compound files: HWP 5.0 is readable; legacy Office and encrypted OOXML are not.
fn sniff_cfb(head: &[u8]) -> Sniff {
    if contains(head, &utf16le("EncryptedPackage")) {
        return Sniff::Unsupported("encrypted");
    }
    if contains(head, &utf16le("HwpSummaryInformation"))
        || contains(head, &utf16le("BodyText"))
        || contains(head, b"HWP Document File")
    {
        return Sniff::Kind(SourceKind::Hwp);
    }
    if contains(head, &utf16le("WordDocument"))
        || contains(head, &utf16le("Workbook"))
        || contains(head, &utf16le("Book"))
        || contains(head, &utf16le("PowerPoint Document"))
    {
        return Sniff::Unsupported("office_legacy");
    }
    Sniff::Unknown
}

/// Text when the head has no NUL bytes and decodes as UTF-8 / UTF-16 / CP949 with (nearly) only printable characters.
fn looks_like_text(head: &[u8]) -> bool {
    if head.is_empty() || head.contains(&0) && !(head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF])) {
        return false;
    }
    let (text, _) = super::convert::text::decode(head);
    let total = text.chars().count().max(1);
    let printable = text.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t')).count();
    printable * 100 / total >= 97
}

/// Detects the type of `path` from its content (`Unknown` when unreadable or unrecognised).
pub fn sniff(path: &Path) -> Sniff {
    let Some(head) = read_head(path) else { return Sniff::Unknown };
    if head.is_empty() {
        return Sniff::Unknown;
    }
    if head[..head.len().min(1024)].windows(4).any(|w| w == b"%PDF") {
        return Sniff::Kind(SourceKind::Pdf);
    }
    if head.starts_with(&[0x89, b'P', b'N', b'G'])
        || head.starts_with(&[0xFF, 0xD8, 0xFF])
        || head.starts_with(b"GIF87a")
        || head.starts_with(b"GIF89a")
        || head.starts_with(b"BM")
        || (head.starts_with(b"RIFF") && head.len() >= 12 && &head[8..12] == b"WEBP")
        || head.starts_with(b"II*\0")
        || head.starts_with(b"MM\0*")
    {
        return Sniff::Kind(SourceKind::Image);
    }
    if head.starts_with(b"PK\x03\x04") {
        return sniff_zip(path);
    }
    if head.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return sniff_cfb(&head);
    }
    if head.starts_with(b"MZ") || head.starts_with(&[0x7F, b'E', b'L', b'F']) {
        return Sniff::Unsupported("executable");
    }
    if head.starts_with(b"{\\rtf") {
        return Sniff::Unsupported("office_legacy");
    }
    if head.starts_with(b"Rar!") || head.starts_with(&[0x1F, 0x8B]) || head.starts_with(b"7z\xBC\xAF") {
        return Sniff::Unsupported("archive");
    }
    // ISO base media files: HEIC / HEIF (unsupported), also video containers.
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand = &head[8..12];
        return if brand.starts_with(b"hei") || brand.starts_with(b"hev") || brand == b"mif1" || brand == b"msf1" {
            Sniff::Unsupported("heic")
        } else {
            Sniff::Unsupported("unsupported_extension")
        };
    }
    if looks_like_text(&head) {
        let (text, _) = super::convert::text::decode(&head[..head.len().min(4096)]);
        let start: String = text.trim_start().chars().take(64).collect::<String>().to_lowercase();
        if start.starts_with("<!doctype html") || start.contains("<html") {
            return Sniff::Html;
        }
        return Sniff::Kind(SourceKind::Text);
    }
    Sniff::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::convert::{docx, hwpx, pptx, write_blank_pdf, xlsx};

    #[test]
    fn signatures_and_containers() {
        let dir = tempfile::tempdir().unwrap();
        let p = |n: &str| dir.path().join(n);
        write_blank_pdf(&p("a.bin"), 1, &[]).unwrap();
        assert_eq!(sniff(&p("a.bin")), Sniff::Kind(SourceKind::Pdf));
        std::fs::write(p("img"), hwpx::tests::TINY_PNG).unwrap();
        assert_eq!(sniff(&p("img")), Sniff::Kind(SourceKind::Image));
        docx::write_sample_docx(&p("renamed.zip"), 1, hwpx::tests::TINY_PNG).unwrap();
        assert_eq!(sniff(&p("renamed.zip")), Sniff::Kind(SourceKind::Docx));
        xlsx::write_sample_xlsx(&p("book")).unwrap();
        assert_eq!(sniff(&p("book")), Sniff::Kind(SourceKind::Xlsx));
        pptx::write_sample_pptx(&p("deck.dat"), hwpx::tests::TINY_PNG).unwrap();
        assert_eq!(sniff(&p("deck.dat")), Sniff::Kind(SourceKind::Pptx));
        hwpx::write_sample_hwpx(&p("doc.zip"), 1, hwpx::tests::TINY_PNG).unwrap();
        assert_eq!(sniff(&p("doc.zip")), Sniff::Kind(SourceKind::Hwpx));
        // A plain zip of nothing we read.
        {
            use std::io::Write;
            let f = File::create(p("plain.zip")).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file("readme.txt", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b"hi").unwrap();
            z.finish().unwrap();
        }
        assert_eq!(sniff(&p("plain.zip")), Sniff::Unsupported("archive"));
        // CFB variants: legacy Word, encrypted OOXML, HWP.
        let cfb = |streams: &[&str]| {
            let mut v = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
            v.resize(512, 0);
            for s in streams {
                v.extend(utf16le(s));
                v.extend([0u8; 16]);
            }
            v
        };
        std::fs::write(p("old.doc"), cfb(&["Root Entry", "WordDocument"])).unwrap();
        assert_eq!(sniff(&p("old.doc")), Sniff::Unsupported("office_legacy"));
        std::fs::write(p("locked.docx"), cfb(&["EncryptedPackage"])).unwrap();
        assert_eq!(sniff(&p("locked.docx")), Sniff::Unsupported("encrypted"));
        std::fs::write(p("doc.hwp"), cfb(&["FileHeader", "HwpSummaryInformation", "BodyText"])).unwrap();
        assert_eq!(sniff(&p("doc.hwp")), Sniff::Kind(SourceKind::Hwp));
        std::fs::write(p("prog.exe"), b"MZ\x90\x00binary").unwrap();
        assert_eq!(sniff(&p("prog.exe")), Sniff::Unsupported("executable"));
        std::fs::write(p("page.pdf"), "<!DOCTYPE html><html><body>not a pdf</body></html>").unwrap();
        assert_eq!(sniff(&p("page.pdf")), Sniff::Html);
        let mut heic = vec![0, 0, 0, 24];
        heic.extend_from_slice(b"ftypheic");
        heic.extend_from_slice(&[0u8; 64]);
        std::fs::write(p("photo.bin"), heic).unwrap();
        assert_eq!(sniff(&p("photo.bin")), Sniff::Unsupported("heic"));
        let (euc, _, _) = encoding_rs::EUC_KR.encode("한글 메모\n둘째 줄");
        std::fs::write(p("memo"), &*euc).unwrap();
        assert_eq!(sniff(&p("memo")), Sniff::Kind(SourceKind::Text));
        std::fs::write(p("noise.bin"), (0u8..=255).cycle().take(4096).collect::<Vec<u8>>()).unwrap();
        assert_eq!(sniff(&p("noise.bin")), Sniff::Unknown);
    }
}
