//! Documents in a workspace (design §3.4 / §3.6): records in `workspace.db`,
//! import (drop / folder / inbox watch) with duplicate detection, conversion
//! of HWP / HWPX / DOCX to PDF, page rendering through PDFium and the page
//! image cache. Everything is keyed by a document id (UUID) and stores
//! workspace-relative paths only, so a workspace folder stays relocatable.

pub mod convert;
pub mod fixtures;
pub mod import;
pub mod render;
pub mod repo;

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::paths::WorkspacePaths;

/// Input formats the pipeline accepts (design §3.4 input scope).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Pdf,
    Image,
    Hwpx,
    Docx,
    Hwp,
}

impl SourceKind {
    pub const ALL: [SourceKind; 5] =
        [SourceKind::Pdf, SourceKind::Image, SourceKind::Hwpx, SourceKind::Docx, SourceKind::Hwp];

    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Pdf => "pdf",
            SourceKind::Image => "image",
            SourceKind::Hwpx => "hwpx",
            SourceKind::Docx => "docx",
            SourceKind::Hwp => "hwp",
        }
    }

    pub fn parse(s: &str) -> Option<SourceKind> {
        SourceKind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Kind for a file name / extension, `None` when the extension is not an
    /// input format.
    pub fn from_path(path: &Path) -> Option<SourceKind> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "pdf" => Some(SourceKind::Pdf),
            "png" | "jpg" | "jpeg" | "tif" | "tiff" | "webp" | "bmp" | "gif" => Some(SourceKind::Image),
            "hwpx" => Some(SourceKind::Hwpx),
            "docx" => Some(SourceKind::Docx),
            "hwp" => Some(SourceKind::Hwp),
            _ => None,
        }
    }

    /// Office formats are converted to PDF first (design §3.4).
    pub fn needs_conversion(self) -> bool {
        matches!(self, SourceKind::Hwpx | SourceKind::Docx | SourceKind::Hwp)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    /// Inserted; conversion / rendering still running (or interrupted).
    #[default]
    Importing,
    /// Registered but not rendered (render disabled or failed softly).
    New,
    Rendered,
    Extracted,
    Reviewed,
    Archived,
    /// File no longer found at `rel_path`.
    Missing,
    Error,
}

impl DocumentStatus {
    pub const ALL: [DocumentStatus; 8] = [
        DocumentStatus::Importing,
        DocumentStatus::New,
        DocumentStatus::Rendered,
        DocumentStatus::Extracted,
        DocumentStatus::Reviewed,
        DocumentStatus::Archived,
        DocumentStatus::Missing,
        DocumentStatus::Error,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DocumentStatus::Importing => "importing",
            DocumentStatus::New => "new",
            DocumentStatus::Rendered => "rendered",
            DocumentStatus::Extracted => "extracted",
            DocumentStatus::Reviewed => "reviewed",
            DocumentStatus::Archived => "archived",
            DocumentStatus::Missing => "missing",
            DocumentStatus::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<DocumentStatus> {
        DocumentStatus::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DocumentRole {
    #[default]
    Work,
    Knowledge,
    Form,
    Generated,
}

impl DocumentRole {
    pub const ALL: [DocumentRole; 4] =
        [DocumentRole::Work, DocumentRole::Knowledge, DocumentRole::Form, DocumentRole::Generated];

    pub fn as_str(self) -> &'static str {
        match self {
            DocumentRole::Work => "work",
            DocumentRole::Knowledge => "knowledge",
            DocumentRole::Form => "form",
            DocumentRole::Generated => "generated",
        }
    }

    pub fn parse(s: &str) -> Option<DocumentRole> {
        DocumentRole::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// One row of `documents` (plus its tag names).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentRecord {
    pub id: String,
    /// Workspace-relative path with `/` separators (NFC).
    pub rel_path: String,
    pub original_name: String,
    pub source_kind: SourceKind,
    pub content_hash: String,
    pub size_bytes: u64,
    pub file_modified_at: Option<DateTime<Utc>>,
    pub imported_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: DocumentStatus,
    pub role: DocumentRole,
    pub title: Option<String>,
    pub doc_type: Option<String>,
    pub vendor: Option<String>,
    pub doc_date: Option<String>,
    pub total_amount: Option<f64>,
    pub language: Option<String>,
    pub page_count: u32,
    pub has_text_layer: bool,
    pub converted: bool,
    pub conversion_note: Option<String>,
    pub render_dpi: Option<u16>,
    pub error: Option<String>,
    pub import_batch_id: Option<String>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
}

impl DocumentRecord {
    /// Folder part of `rel_path` (`""` for the workspace root).
    pub fn rel_dir(&self) -> &str {
        self.rel_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
    }

    pub fn absolute_path(&self, paths: &WorkspacePaths) -> PathBuf {
        from_rel_path(&paths.root, &self.rel_path)
    }
}

/// One row of `pages`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageRecord {
    pub document_id: String,
    pub page_no: u32,
    pub width_px: u32,
    pub height_px: u32,
    pub width_pt: Option<f64>,
    pub height_pt: Option<f64>,
    pub dpi: Option<u16>,
    pub rotation: u16,
    pub has_text: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub is_blank: Option<bool>,
    pub rendered_at: Option<DateTime<Utc>>,
}

/// Fields needed to insert a document.
#[derive(Debug, Clone, PartialEq)]
pub struct NewDocument {
    pub id: String,
    pub rel_path: String,
    pub original_name: String,
    pub source_kind: SourceKind,
    pub content_hash: String,
    pub size_bytes: u64,
    pub file_modified_at: Option<DateTime<Utc>>,
    pub role: DocumentRole,
    pub import_batch_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewPage {
    pub page_no: u32,
    pub width_px: u32,
    pub height_px: u32,
    pub width_pt: Option<f64>,
    pub height_pt: Option<f64>,
    pub dpi: Option<u16>,
    pub rotation: u16,
    pub text: Option<String>,
    pub is_blank: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DocumentSort {
    #[default]
    ImportedDesc,
    ImportedAsc,
    NameAsc,
    NameDesc,
    SizeDesc,
}

/// Library list filter (all fields optional; `folder` is a `rel_dir` prefix).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DocumentFilter {
    pub statuses: Vec<DocumentStatus>,
    pub kinds: Vec<SourceKind>,
    pub folder: Option<String>,
    /// Case-insensitive substring of the file name / title.
    pub query: Option<String>,
    pub tag: Option<String>,
    pub doc_type: Option<String>,
    pub include_deleted: bool,
    pub sort: DocumentSort,
    pub limit: Option<u32>,
    pub offset: u32,
}

/// Document counts per folder (library tree) and per status (dashboard).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct LibraryStats {
    pub total: u64,
    pub by_status: std::collections::BTreeMap<String, u64>,
    pub by_kind: std::collections::BTreeMap<String, u64>,
    pub folders: Vec<FolderCount>,
    pub total_pages: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FolderCount {
    /// `rel_dir` (`""` = workspace root).
    pub rel_dir: String,
    pub count: u64,
}

/// Converts an absolute path inside the workspace into the stored form:
/// relative with `/` separators. The on-disk spelling is kept as is (NTFS and
/// ext4 compare names byte-exact, so an NFD-named file must stay NFD);
/// names the app creates itself are NFC (`nfc`). `None` when outside the root.
pub fn to_rel_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            std::path::Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

pub fn from_rel_path(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(part);
    }
    p
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// NFC form of a display name (search / titles / files the app writes).
pub fn nfc(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfc().collect()
}

/// Document ids are UUID hex; anything else must never reach a file system path.
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Cache layout for one document: `cache/pages/<id>/p0001.jpg`,
/// `cache/thumbs/<id>/p0001.jpg`, `cache/converted/<id>/document.pdf`.
#[derive(Debug, Clone)]
pub struct DocumentCache {
    pub pages_dir: PathBuf,
    pub thumbs_dir: PathBuf,
    pub converted_dir: PathBuf,
}

impl DocumentCache {
    pub fn for_document(paths: &WorkspacePaths, id: &str) -> DocumentCache {
        // Defensive: callers validate ids (DB lookup / `is_safe_id`), but a path
        // separator in an id must never escape the cache folder.
        let id = if is_safe_id(id) { id.to_string() } else { "invalid".to_string() };
        DocumentCache {
            pages_dir: paths.page_cache_dir().join(&id),
            thumbs_dir: paths.thumb_cache_dir().join(&id),
            converted_dir: paths.cache_dir().join("converted").join(&id),
        }
    }

    pub fn page_file(&self, page_no: u32) -> PathBuf {
        self.pages_dir.join(format!("p{page_no:04}.jpg"))
    }

    pub fn thumb_file(&self, page_no: u32) -> PathBuf {
        self.thumbs_dir.join(format!("p{page_no:04}.jpg"))
    }

    pub fn converted_pdf(&self) -> PathBuf {
        self.converted_dir.join("document.pdf")
    }

    pub fn converted_html(&self) -> PathBuf {
        self.converted_dir.join("index.html")
    }

    /// Removes every cached artefact of the document.
    pub fn remove_all(&self) {
        for d in [&self.pages_dir, &self.thumbs_dir, &self.converted_dir] {
            if d.exists() {
                let _ = std::fs::remove_dir_all(d);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_paths_round_trip_and_normalise() {
        let root = Path::new("/w");
        assert_eq!(to_rel_path(root, Path::new("/w/inbox/a.pdf")).as_deref(), Some("inbox/a.pdf"));
        assert_eq!(to_rel_path(root, Path::new("/other/a.pdf")), None);
        assert_eq!(to_rel_path(root, Path::new("/w")), None);
        assert_eq!(from_rel_path(root, "inbox/a.pdf"), PathBuf::from("/w/inbox/a.pdf"));
        // The on-disk spelling is kept (NFD stays NFD); `nfc` is for names the app creates.
        let nfd = "inbox/\u{1112}\u{1161}\u{11ab}.pdf";
        let rel = to_rel_path(root, &Path::new("/w").join(nfd)).unwrap();
        assert_eq!(rel, nfd);
        assert_eq!(nfc("\u{1112}\u{1161}\u{11ab}"), "\u{d55c}");
        assert!(is_safe_id("0123abcdef") && !is_safe_id("../x"));
        assert!(DocumentCache::for_document(&WorkspacePaths::new("/w"), "../x").pages_dir.ends_with("invalid"));
    }

    #[test]
    fn kinds_from_extension() {
        assert_eq!(SourceKind::from_path(Path::new("a.PDF")), Some(SourceKind::Pdf));
        assert_eq!(SourceKind::from_path(Path::new("a.jpeg")), Some(SourceKind::Image));
        assert_eq!(SourceKind::from_path(Path::new("a.hwpx")), Some(SourceKind::Hwpx));
        assert_eq!(SourceKind::from_path(Path::new("a.heic")), None);
        assert!(SourceKind::Docx.needs_conversion());
        assert!(!SourceKind::Pdf.needs_conversion());
        assert_eq!(DocumentStatus::parse("rendered"), Some(DocumentStatus::Rendered));
    }
}
