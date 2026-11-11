//! Document split / merge (§3.4 "다중 페이지/분할"): scanner batch PDFs are
//! cut at page boundaries into new documents, and several documents are
//! merged into one PDF — without re-rendering, through lopdf (MIT). Image
//! documents are wrapped into single-page PDFs for merging.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

use crate::documents::import::{self, ImportDeps};
use crate::documents::{repo as docs, DocumentCache, DocumentRecord, DocumentStatus, NewDocument, SourceKind};
use crate::error::{CoreError, Result};

fn pdf_err(ctx: &str, e: lopdf::Error) -> CoreError {
    CoreError::Convert(format!("pdf {ctx}: {e}"))
}

/// Page ranges (1-based, inclusive) from "new document starts here" boundaries.
pub fn ranges(total: u32, boundaries: &[u32]) -> Vec<(u32, u32)> {
    let mut b: Vec<u32> = boundaries.iter().copied().filter(|p| *p > 1 && *p <= total).collect();
    b.sort_unstable();
    b.dedup();
    let mut out = Vec::new();
    let mut start = 1;
    for p in b {
        out.push((start, p - 1));
        start = p;
    }
    out.push((start, total));
    out
}

/// Writes one PDF per range next to `out_dir/<stem>_partN.pdf`; returns `(path, pages)`.
pub fn split_pdf(src: &Path, boundaries: &[u32], out_dir: &Path, stem: &str) -> Result<Vec<(PathBuf, u32)>> {
    split_pdf_avoiding(src, boundaries, out_dir, stem, &|p| p.exists())
}

/// [`split_pdf`] with the caller's notion of a taken file name (existing files plus, for
/// documents, paths that are registered in the workspace database).
fn split_pdf_avoiding(
    src: &Path,
    boundaries: &[u32],
    out_dir: &Path,
    stem: &str,
    taken: &dyn Fn(&Path) -> bool,
) -> Result<Vec<(PathBuf, u32)>> {
    let doc = Document::load(src).map_err(|e| pdf_err("load", e))?;
    let total = doc.get_pages().len() as u32;
    if total < 2 {
        return Err(CoreError::Invalid("a single-page document cannot be split".into()));
    }
    let parts = ranges(total, boundaries);
    if parts.len() < 2 {
        return Err(CoreError::Invalid("no split boundary inside the document".into()));
    }
    std::fs::create_dir_all(out_dir).map_err(|e| CoreError::io(out_dir, e))?;
    let mut out = Vec::new();
    for (i, (first, last)) in parts.iter().enumerate() {
        let mut part = doc.clone();
        let remove: Vec<u32> = (1..=total).filter(|p| p < first || p > last).collect();
        part.delete_pages(&remove);
        part.prune_objects();
        part.renumber_objects();
        let path = import::unique_target(out_dir, &format!("{stem}_part{}.pdf", i + 1), taken);
        part.save(&path).map_err(|e| CoreError::io(&path, e))?;
        out.push((path, last - first + 1));
    }
    Ok(out)
}

fn inherited(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = page_id;
    let mut depth = 0;
    loop {
        let node = doc.get_dictionary(current).ok()?;
        if let Ok(v) = node.get(key) {
            if !matches!(v, Object::Null) {
                return Some(v.clone());
            }
        }
        match node.get(b"Parent").ok().and_then(|p| p.as_reference().ok()) {
            Some(p) if depth < 64 => {
                current = p;
                depth += 1;
            }
            _ => return None,
        }
    }
}

/// Merges PDFs into `out` (pages in order); returns the page count.
pub fn merge_pdfs(sources: &[PathBuf], out: &Path) -> Result<u32> {
    if sources.is_empty() {
        return Err(CoreError::Invalid("no documents to merge".into()));
    }
    let mut max_id = 1;
    let mut pages: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    for src in sources {
        let mut doc = Document::load(src).map_err(|e| pdf_err("load", e))?;
        // Resolve inherited page attributes before the Pages tree is rebuilt.
        for page_id in doc.get_pages().into_values() {
            let mut page = doc.get_dictionary(page_id).map_err(|e| pdf_err("page", e))?.clone();
            for key in ["MediaBox", "CropBox", "Resources", "Rotate"] {
                if let Some(v) = inherited(&doc, page_id, key.as_bytes()) {
                    page.set(key, v);
                }
            }
            doc.objects.insert(page_id, Object::Dictionary(page));
        }
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;
        for page_id in doc.get_pages().into_values() {
            if let Ok(o) = doc.get_object(page_id) {
                pages.insert(page_id, o.clone());
            }
        }
        objects.extend(doc.objects);
    }
    let mut merged = Document::with_version("1.5");
    let mut catalog: Option<(ObjectId, Dictionary)> = None;
    let mut pages_root: Option<(ObjectId, Dictionary)> = None;
    for (id, object) in objects {
        match object.type_name().unwrap_or(b"") {
            b"Catalog" => {
                if catalog.is_none() {
                    catalog = Some((id, object.as_dict().cloned().unwrap_or_default()));
                }
            }
            b"Pages" => {
                if let Ok(d) = object.as_dict() {
                    match &mut pages_root {
                        None => pages_root = Some((id, d.clone())),
                        Some((_, existing)) => existing.extend(d),
                    }
                }
            }
            b"Page" | b"Outlines" | b"Outline" => {}
            _ => {
                merged.objects.insert(id, object);
            }
        }
    }
    let (pages_id, mut pages_dict) = pages_root.ok_or_else(|| CoreError::Convert("pdf merge: no Pages root".into()))?;
    let (catalog_id, mut catalog_dict) = catalog.ok_or_else(|| CoreError::Convert("pdf merge: no Catalog".into()))?;
    let count = pages.len() as u32;
    for (id, object) in &pages {
        if let Ok(d) = object.as_dict() {
            let mut d = d.clone();
            d.set("Parent", pages_id);
            merged.objects.insert(*id, Object::Dictionary(d));
        }
    }
    pages_dict.remove(b"Parent");
    for key in ["MediaBox", "CropBox", "Resources", "Rotate"] {
        pages_dict.remove(key.as_bytes());
    }
    pages_dict.set("Count", count);
    pages_dict.set("Kids", pages.keys().map(|id| Object::Reference(*id)).collect::<Vec<_>>());
    merged.objects.insert(pages_id, Object::Dictionary(pages_dict));
    catalog_dict.set("Pages", pages_id);
    catalog_dict.remove(b"Outlines");
    catalog_dict.remove(b"PageMode");
    merged.objects.insert(catalog_id, Object::Dictionary(catalog_dict));
    merged.trailer.set("Root", catalog_id);
    merged.max_id = merged.objects.len() as u32;
    merged.renumber_objects();
    merged.compress();
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    merged.save(out).map_err(|e| CoreError::io(out, e))?;
    Ok(count)
}

/// Wraps an image into a one-page PDF (re-encoded as RGB JPEG, DCTDecode) so image
/// documents can take part in a merge. `dpi` sets the page size.
pub fn jpeg_to_pdf(source: &Path, out: &Path, dpi: f32) -> Result<()> {
    let img = crate::documents::render::decode_image(source)?;
    let (w, h) = (img.width(), img.height());
    let bytes = super::image::encode_jpeg(&img)?;
    let scale = 72.0 / dpi.max(36.0);
    let (w_pt, h_pt) = (w as f32 * scale, h as f32 * scale);
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let image = Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
            "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "Filter" => "DCTDecode",
        },
        bytes,
    );
    let image_id = doc.add_object(image);
    let resources_id = doc.add_object(dictionary! { "XObject" => dictionary! { "Im1" => image_id } });
    let content = Content {
        operations: vec![
            Operation::new("q", vec![]),
            Operation::new("cm", vec![w_pt.into(), 0.into(), 0.into(), h_pt.into(), 0.into(), 0.into()]),
            Operation::new("Do", vec!["Im1".into()]),
            Operation::new("Q", vec![]),
        ],
    };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().map_err(|e| pdf_err("content", e))?));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id, "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), w_pt.into(), h_pt.into()],
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    doc.save(out).map_err(|e| CoreError::io(out, e))?;
    Ok(())
}

/// The PDF a document renders from (original or converted copy).
fn pdf_source(deps: &ImportDeps<'_>, doc: &DocumentRecord) -> Result<PathBuf> {
    let abs = doc.absolute_path(deps.paths);
    match doc.source_kind {
        SourceKind::Pdf => Ok(abs),
        SourceKind::Image => {
            // Wrap the rendered page (EXIF-corrected, rotated) into a PDF in the cache.
            let cache = DocumentCache::for_document(deps.paths, &doc.id);
            let page = cache.page_file(1);
            let src = if page.is_file() { page } else { abs };
            let out = cache.converted_dir.join("image.pdf");
            jpeg_to_pdf(&src, &out, doc.render_dpi.unwrap_or(200) as f32)?;
            Ok(out)
        }
        _ => {
            let converted = DocumentCache::for_document(deps.paths, &doc.id).converted_pdf();
            if converted.is_file() {
                Ok(converted)
            } else {
                Err(CoreError::Unsupported(format!("{}: no converted PDF", doc.original_name)))
            }
        }
    }
}

/// `true` when a workspace path already belongs to a registered document (any status: a
/// `missing` row still owns its `rel_path`).
fn is_registered(deps: &ImportDeps<'_>, path: &Path) -> bool {
    crate::documents::to_rel_path(&deps.paths.root, path)
        .is_some_and(|rel| deps.db.with(|c| docs::find_by_rel_path(c, &rel)).ok().flatten().is_some())
}

/// The row of a PDF this module wrote (a split part, a merge result), not inserted yet.
fn new_row(deps: &ImportDeps<'_>, path: &Path, role: crate::documents::DocumentRole) -> Result<NewDocument> {
    let rel = crate::documents::to_rel_path(&deps.paths.root, path)
        .ok_or_else(|| CoreError::InvalidWorkspace(format!("{} is outside the workspace", path.display())))?;
    let meta = std::fs::metadata(path).map_err(|e| CoreError::io(path, e))?;
    Ok(NewDocument {
        id: crate::documents::new_id(),
        rel_path: rel,
        original_name: crate::documents::nfc(
            &path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        ),
        source_kind: SourceKind::Pdf,
        content_hash: import::blake3_file(path)?,
        size_bytes: meta.len(),
        file_modified_at: meta.modified().ok().map(chrono::DateTime::<chrono::Utc>::from),
        role,
        import_batch_id: None,
    })
}

/// Renders a freshly inserted row.
fn render_row(deps: &ImportDeps<'_>, id: &str, cancel: Option<&AtomicBool>) -> Result<()> {
    let record = deps.db.with(|c| docs::get_document(c, id))?.ok_or_else(|| CoreError::NotFound(id.to_string()))?;
    import::process_document(deps, &record, true, None, cancel, &mut |_| {})
}

fn register(
    deps: &ImportDeps<'_>,
    path: &Path,
    role: crate::documents::DocumentRole,
    cancel: Option<&AtomicBool>,
) -> Result<String> {
    let new = new_row(deps, path, role)?;
    deps.db.with(|c| docs::insert_document(c, &new))?;
    render_row(deps, &new.id, cancel)?;
    Ok(new.id)
}

/// Splits a document at the given boundaries into new documents stored next
/// to the original (`<stem>_partN.pdf`); the original is archived. Returns the new ids.
pub fn split_document(
    deps: &ImportDeps<'_>,
    doc: &DocumentRecord,
    boundaries: &[u32],
    cancel: Option<&AtomicBool>,
) -> Result<Vec<String>> {
    split_document_observed(deps, doc, boundaries, cancel, &mut |_| {})
}

/// [`split_document`]; `after_write` sees the part files once they are on disk and before any of
/// them is registered (tests stand in for another importer there).
pub(crate) fn split_document_observed(
    deps: &ImportDeps<'_>,
    doc: &DocumentRecord,
    boundaries: &[u32],
    cancel: Option<&AtomicBool>,
    after_write: &mut dyn FnMut(&[(PathBuf, u32)]),
) -> Result<Vec<String>> {
    let src = pdf_source(deps, doc)?;
    let abs = doc.absolute_path(deps.paths);
    let dir = abs.parent().map(Path::to_path_buf).unwrap_or_else(|| deps.paths.documents_dir());
    let stem = abs.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    // A name a (missing) document still owns is taken even when no file is there.
    let parts = split_pdf_avoiding(&src, boundaries, &dir, &stem, &|p| is_registered(deps, p))?;
    after_write(&parts);
    let mut ids: Vec<String> = Vec::new();
    let result: Result<()> = (|| {
        // Every part gets its row before the first one is rendered (rendering takes seconds per
        // part): from here on the parts are registered documents for anyone who looks — an inbox
        // scan or a folder import never takes a part that is waiting for its turn as a new file.
        let mut rows = Vec::with_capacity(parts.len());
        for (path, _) in &parts {
            rows.push(new_row(deps, path, doc.role)?);
        }
        deps.db.with_tx(|tx| rows.iter().try_for_each(|row| docs::insert_document(tx, row)))?;
        ids = rows.into_iter().map(|r| r.id).collect();
        for id in &ids {
            render_row(deps, id, cancel)?;
            if let Some(dt) = &doc.doc_type {
                deps.db.with(|c| docs::set_doc_type(c, id, Some(dt), doc.doc_type_source.as_deref()))?;
            }
            if !doc.tags.is_empty() {
                deps.db.with(|c| docs::set_tags(c, id, &doc.tags))?;
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        rollback_parts(deps, &ids, &parts);
        return Err(e);
    }
    deps.db.with(|c| docs::set_status(c, &doc.id, DocumentStatus::Archived, None))?;
    Ok(ids)
}

/// All or nothing for a cancelled / failed split — but only for what the split itself created:
/// its rows go, and so do the part files nobody else registered. A part that another importer
/// registered in the meantime is that document's file now and stays (deleting it left a
/// document without a file, audit DOC-3).
fn rollback_parts(deps: &ImportDeps<'_>, ids: &[String], parts: &[(PathBuf, u32)]) {
    for id in ids {
        // Row + caches only; the file is removed below instead of going to the OS trash.
        let _ = import::delete_document(deps, id, true);
    }
    for (path, _) in parts {
        if !is_registered(deps, path) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A user-supplied file stem: one path component, no traversal, no control characters.
pub fn safe_stem(name: &str) -> Result<String> {
    let n = name.trim().trim_end_matches(".pdf").trim_end_matches(".PDF").trim();
    let bad = n.is_empty()
        || n.len() > 120
        || n.starts_with('.')
        || n == ".."
        || n.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        || n.chars().any(|c| c.is_control());
    if bad {
        return Err(CoreError::Invalid("invalid name".into()));
    }
    Ok(crate::documents::nfc(n))
}

/// Merges documents (in the given order) into one PDF under `documents/`.
pub fn merge_documents(
    deps: &ImportDeps<'_>,
    documents: &[DocumentRecord],
    name: Option<&str>,
    cancel: Option<&AtomicBool>,
) -> Result<String> {
    if documents.len() < 2 {
        return Err(CoreError::Invalid("merge needs at least two documents".into()));
    }
    // Validate the name before any file is written.
    let stem = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => safe_stem(n)?,
        None => {
            let first = Path::new(&documents[0].original_name)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("{first}_merged")
        }
    };
    let mut sources = Vec::new();
    for d in documents {
        sources.push(pdf_source(deps, d)?);
    }
    let dir = deps.paths.documents_dir();
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    let out = import::unique_target(&dir, &format!("{stem}.pdf"), &|p| is_registered(deps, p));
    merge_pdfs(&sources, &out)?;
    register(deps, &out, documents[0].role, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::convert::write_blank_pdf;

    #[test]
    fn split_and_merge_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("batch.pdf");
        let texts: Vec<String> = (1..=5).map(|i| format!("page {i}")).collect();
        write_blank_pdf(&src, 5, &texts).unwrap();
        assert_eq!(ranges(5, &[3, 3, 9, 1]), vec![(1, 2), (3, 5)]);
        let parts = split_pdf(&src, &[3, 5], dir.path(), "batch").unwrap();
        assert_eq!(parts.iter().map(|(_, n)| *n).collect::<Vec<_>>(), vec![2, 2, 1]);
        for (p, n) in &parts {
            let d = Document::load(p).unwrap();
            assert_eq!(d.get_pages().len() as u32, *n, "{}", p.display());
            assert!(d.extract_text(&[1]).unwrap().contains("page"));
        }
        assert!(parts[0].0.ends_with("batch_part1.pdf"));
        let merged = dir.path().join("merged.pdf");
        let srcs: Vec<PathBuf> = parts.iter().map(|(p, _)| p.clone()).collect();
        assert_eq!(merge_pdfs(&srcs, &merged).unwrap(), 5);
        let m = Document::load(&merged).unwrap();
        assert_eq!(m.get_pages().len(), 5);
        let text = m.extract_text(&[1, 2, 3, 4, 5]).unwrap();
        assert!(text.contains("page 1") && text.contains("page 5"), "{text}");
        assert!(split_pdf(&parts[2].0, &[1], dir.path(), "x").is_err(), "single page cannot be split");
    }

    #[test]
    fn jpeg_becomes_a_one_page_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let jpg = dir.path().join("photo.jpg");
        let img = image::RgbImage::from_pixel(400, 300, image::Rgb([200, 220, 240]));
        img.save(&jpg).unwrap();
        let out = dir.path().join("photo.pdf");
        jpeg_to_pdf(&jpg, &out, 200.0).unwrap();
        let d = Document::load(&out).unwrap();
        assert_eq!(d.get_pages().len(), 1);
        let merged = dir.path().join("two.pdf");
        assert_eq!(merge_pdfs(&[out.clone(), out.clone()], &merged).unwrap(), 2);
        assert_eq!(safe_stem(" 합친 문서.pdf ").unwrap(), "합친 문서");
        for bad in ["../x", "/tmp/x", "a/b", "..", ".hidden", "con:fig", ""] {
            assert!(safe_stem(bad).is_err(), "{bad}");
        }
    }
}
