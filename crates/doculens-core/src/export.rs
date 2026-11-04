//! Export wizard back end (design §3.7 내보내기 마법사): the extracted fields,
//! the OCR / text-layer tables or the page texts of the documents in a plan
//! scope as XLSX (one sheet per document type plus a long-form 품목 sheet by
//! default), CSV (UTF-8 with BOM so Excel reads Korean), JSONL, Markdown, or a
//! searchable PDF (the cached page renders with an invisible text layer).
//! Targets stay inside the workspace (`exports/YYYY-MM-DD_<유형>.xlsx` by
//! default), are written to `<path>.part` and renamed, and existing files are
//! only replaced with `overwrite`. Stored values are already PII-masked (§3.8)
//! and are exported as stored.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::NaiveDate;
use rust_xlsxwriter::Workbook;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::db::Db;
use crate::documents::repo::list_pages;
use crate::documents::{from_rel_path, DocumentCache, DocumentRecord, PageRecord, ReviewStatus};
use crate::error::{CoreError, Result};
use crate::organize::template::{sanitize_component, UNKNOWN};
use crate::paths::WorkspacePaths;
use crate::pipeline::repo::{latest_fields, schema_for_doc_type};
use crate::pipeline::run::document_tables;
use crate::pipeline::validate::format_number;
use crate::pipeline::{ExtractedTable, FieldKind, FieldRecord, FieldStatus, SchemaRecord};
use crate::planner::scope::{basis_date, resolve};
use crate::planner::{compact, ExportFormat, ExportInclude, ExportLayout, ExportTarget, Scope};
use crate::settings::Language;
use crate::version::WORKSPACE_DIR_NAME;

/// Excel refuses cell strings above 32,767 characters; longer texts are cut.
const XLSX_MAX_CHARS: usize = 32_000;
/// Excel's sheet-name limit (UTF-16 code units).
const SHEET_NAME_UNITS: usize = 31;
/// Non-embedded Windows font named in the searchable PDF (never rendered: the
/// text layer is invisible, readers only need the ToUnicode map).
const PDF_FONT: &[u8] = b"MalgunGothic";

/// One export request (planner `export` op + scope, the wizard, the viewer toolbar).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportRequest {
    pub format: ExportFormat,
    pub target: ExportTarget,
    /// Workspace-relative target; `None` = [`default_path`].
    pub path: Option<String>,
    /// Field keys / paths / labels to keep (empty = every scalar field).
    pub columns: Vec<String>,
    pub layout: ExportLayout,
    pub include: Vec<ExportInclude>,
    pub scope: Scope,
    /// Leave review-pending documents out.
    pub only_reviewed: bool,
    pub overwrite: bool,
    /// Language of column labels and sheet names.
    pub language: Language,
}

impl Default for ExportRequest {
    fn default() -> Self {
        Self {
            format: ExportFormat::Xlsx,
            target: ExportTarget::Fields,
            path: None,
            columns: Vec::new(),
            layout: ExportLayout::SheetPerType,
            include: Vec::new(),
            scope: Scope::default(),
            only_reviewed: false,
            overwrite: false,
            language: Language::Ko,
        }
    }
}

impl ExportRequest {
    fn ko(&self) -> bool {
        self.language == Language::Ko
    }
    fn includes(&self, what: ExportInclude) -> bool {
        self.include.contains(&what)
    }
}

/// What an export would write (confirmation card / wizard summary).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportPreview {
    /// Workspace-relative target (a folder for several searchable PDFs).
    pub path: String,
    pub exists: bool,
    pub documents: u64,
    /// Review-pending documents left out by `only_reviewed`.
    pub skipped: u64,
    /// Column labels (fields / text targets).
    pub columns: Vec<String>,
    /// Sheet names (XLSX).
    pub sheets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportResult {
    /// Workspace-relative path with `/` separators (a folder for several searchable PDFs).
    pub path: String,
    pub absolute: PathBuf,
    pub format: ExportFormat,
    pub target: ExportTarget,
    pub documents: u64,
    /// Data rows (documents + item rows, table rows, pages …).
    pub rows: u64,
    pub sheets: Vec<String>,
    pub overwritten: bool,
    /// `(document id, reason)`: `review_pending`, `no_pages`, `no_tables`, `no_text`.
    pub skipped: Vec<(String, String)>,
    /// Further files written (`<stem>_items.csv`, per-document PDFs), workspace-relative.
    pub extra_files: Vec<String>,
}

/// Default target: `exports/<today>_<type label>.<ext>` (`문서` without a
/// single type, `_표` / `_텍스트` for the table / text targets,
/// `exports/<today>_검색가능.pdf` for searchable PDFs).
pub fn default_path(
    format: ExportFormat,
    target: ExportTarget,
    doc_type_label: Option<&str>,
    today: NaiveDate,
) -> String {
    let name = if format == ExportFormat::PdfSearchable {
        "검색가능".to_string()
    } else {
        let label = doc_type_label
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(sanitize_component)
            .unwrap_or_else(|| "문서".to_string());
        let suffix = match target {
            ExportTarget::Fields => "",
            ExportTarget::Table => "_표",
            ExportTarget::Text => "_텍스트",
        };
        format!("{label}{suffix}")
    };
    format!("exports/{}_{name}.{}", today.format("%Y-%m-%d"), format.extension())
}

/// Describes the export without writing anything.
pub fn preview(db: &Db, paths: &WorkspacePaths, req: &ExportRequest) -> Result<ExportPreview> {
    let p = prepare(db, paths, req)?;
    let ko = req.ko();
    let (columns, sheets) = match (req.format, req.target) {
        (ExportFormat::PdfSearchable, _) => (Vec::new(), Vec::new()),
        (format, ExportTarget::Fields) => {
            let (grids, items) = fields_grids(&p, req);
            let all: Vec<&Doc> = p.docs.iter().collect();
            let columns = fields_header(&union_columns(&all, &p.schemas, req), req);
            let sheets = if format == ExportFormat::Xlsx {
                sheet_names(grids.iter().chain(std::iter::once(&items)).map(|g| g.name.as_str()))
            } else {
                Vec::new()
            };
            (columns, sheets)
        }
        (format, ExportTarget::Table) => {
            let sheets = if format == ExportFormat::Xlsx {
                let mut names = Vec::new();
                for doc in &p.docs {
                    for t in document_tables(db, &doc.rec.id)? {
                        names.push(table_sheet_name(doc, &t));
                    }
                }
                sheet_names(names.iter().map(String::as_str))
            } else {
                Vec::new()
            };
            (Vec::new(), sheets)
        }
        (format, ExportTarget::Text) => {
            let columns = text_header(ko);
            let sheets =
                if format == ExportFormat::Xlsx { vec![tr(ko, "텍스트", "Text").to_string()] } else { Vec::new() };
            (columns, sheets)
        }
    };
    Ok(ExportPreview {
        path: p.rel,
        exists: p.exists,
        documents: p.docs.len() as u64,
        skipped: p.skipped.len() as u64,
        columns,
        sheets,
    })
}

/// Writes the export. `cancel` is checked between documents; a cancelled or
/// failed export leaves no partial target behind.
pub fn export(
    db: &Db,
    paths: &WorkspacePaths,
    req: &ExportRequest,
    cancel: Option<&AtomicBool>,
) -> Result<ExportResult> {
    let p = prepare(db, paths, req)?;
    if p.exists && !req.overwrite {
        return Err(CoreError::Invalid("export_exists".into()));
    }
    if p.docs.is_empty() {
        return Err(CoreError::Invalid("no_documents".into()));
    }
    let rendered = match (req.format, req.target) {
        (ExportFormat::PdfSearchable, _) => render_pdf(db, paths, &p, cancel)?,
        (_, ExportTarget::Fields) => render_fields(req, &p)?,
        (_, ExportTarget::Table) => render_tables(db, req, &p, cancel)?,
        (_, ExportTarget::Text) => render_text(db, req, &p, cancel)?,
    };
    if cancelled(cancel) {
        return Err(CoreError::Cancelled);
    }
    for (rel, bytes) in &rendered.files {
        write_atomic(&from_rel_path(&paths.root, rel), bytes)?;
    }
    let mut skipped = p.skipped;
    skipped.extend(rendered.skipped);
    let extra_files = rendered.files.iter().skip(usize::from(!p.folder)).map(|(r, _)| r.clone()).collect();
    Ok(ExportResult {
        path: p.rel,
        absolute: p.absolute,
        format: req.format,
        target: req.target,
        documents: rendered.documents,
        rows: rendered.rows,
        sheets: rendered.sheets,
        overwritten: p.exists,
        skipped,
        extra_files,
    })
}

// ---------------------------------------------------------------------------
// Preparation: scope, paths
// ---------------------------------------------------------------------------

struct Doc {
    rec: DocumentRecord,
    /// Basis date (`YYYY-MM-DD`).
    date: Option<String>,
    /// Fields of the latest run (fields target only).
    fields: Vec<FieldRecord>,
}

struct Prepared {
    /// Workspace-relative target: the file, or the folder of per-document PDFs.
    rel: String,
    absolute: PathBuf,
    exists: bool,
    docs: Vec<Doc>,
    skipped: Vec<(String, String)>,
    /// Default schema per document type in scope.
    schemas: BTreeMap<String, SchemaRecord>,
    /// Searchable PDFs of several documents go to a folder.
    folder: bool,
}

fn prepare(db: &Db, paths: &WorkspacePaths, req: &ExportRequest) -> Result<Prepared> {
    let want_fields = req.format != ExportFormat::PdfSearchable && req.target == ExportTarget::Fields;
    let (docs, skipped, schemas) = db.with(|c| {
        let mut docs = Vec::new();
        let mut skipped = Vec::new();
        let mut schemas: BTreeMap<String, SchemaRecord> = BTreeMap::new();
        for rec in resolve(c, &req.scope)? {
            if req.only_reviewed && rec.review_status == ReviewStatus::Pending {
                skipped.push((rec.id.clone(), "review_pending".to_string()));
                continue;
            }
            if let Some(dt) = &rec.doc_type {
                if !schemas.contains_key(dt) {
                    if let Some(s) = schema_for_doc_type(c, dt)? {
                        schemas.insert(dt.clone(), s);
                    }
                }
            }
            let fields = if want_fields && rec.last_run_id.is_some() { latest_fields(c, &rec.id)? } else { Vec::new() };
            let date = basis_date(&rec).0;
            docs.push(Doc { rec, date, fields });
        }
        Ok((docs, skipped, schemas))
    })?;
    let label = scope_type_label(req, &docs, &schemas);
    let mut rel = match req.path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => normalize_rel_path(p)?,
        None => default_path(req.format, req.target, label.as_deref(), chrono::Local::now().date_naive()),
    };
    rel = ensure_extension(&rel, req.format.extension());
    let folder = req.format == ExportFormat::PdfSearchable && docs.len() > 1;
    if folder {
        rel = strip_extension(&rel);
    }
    let absolute = from_rel_path(&paths.root, &rel);
    let exists = absolute.exists();
    Ok(Prepared { rel, absolute, exists, docs, skipped, schemas, folder })
}

/// Label for the default file name: the scope's type, or the one type every
/// document in scope shares.
fn scope_type_label(req: &ExportRequest, docs: &[Doc], schemas: &BTreeMap<String, SchemaRecord>) -> Option<String> {
    let ko = req.ko();
    let label_of =
        |slug: &str| schemas.get(slug).map(|s| s.display_name(ko).to_string()).unwrap_or_else(|| slug.to_string());
    if let Some(slug) = req.scope.filter.doc_type.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return Some(label_of(slug));
    }
    let first = docs.first()?.rec.doc_type.as_deref()?;
    docs.iter().all(|d| d.rec.doc_type.as_deref() == Some(first)).then(|| label_of(first))
}

/// Workspace-relative, `/`-separated, sanitised path; absolute paths, `..`
/// and the `.doculens` folder are rejected (`path_outside`).
fn normalize_rel_path(input: &str) -> Result<String> {
    let outside = || CoreError::Invalid("path_outside".into());
    let s = input.trim().replace('\\', "/");
    let bytes = s.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if s.starts_with('/') || drive {
        return Err(outside());
    }
    let mut parts = Vec::new();
    for seg in s.split('/').map(str::trim) {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." || seg.eq_ignore_ascii_case(WORKSPACE_DIR_NAME) {
            return Err(outside());
        }
        parts.push(sanitize_component(seg));
    }
    if parts.is_empty() {
        return Err(outside());
    }
    Ok(parts.join("/"))
}

fn ensure_extension(rel: &str, ext: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if name.to_lowercase().ends_with(&format!(".{ext}")) {
        rel.to_string()
    } else {
        format!("{rel}.{ext}")
    }
}

fn strip_extension(rel: &str) -> String {
    match rel.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/{}", stem_of(name)),
        None => stem_of(rel).to_string(),
    }
}

fn stem_of(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}

fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(Ordering::Relaxed))
}

/// Writes `<path>.part` and renames it over `path`.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    let mut part = path.as_os_str().to_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::write(&part, bytes).map_err(|e| CoreError::io(&part, e))?;
    std::fs::rename(&part, path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        CoreError::io(path, e)
    })
}

// ---------------------------------------------------------------------------
// Grids (shared by XLSX / CSV / Markdown)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Empty,
    Text(String),
    Number(f64),
}

impl Cell {
    fn text(&self) -> String {
        match self {
            Cell::Empty => String::new(),
            Cell::Text(s) => s.clone(),
            Cell::Number(n) => format_number(*n),
        }
    }
    fn from_opt(s: Option<&str>) -> Cell {
        match s {
            Some(s) if !s.is_empty() => Cell::Text(s.to_string()),
            _ => Cell::Empty,
        }
    }
}

struct Grid {
    name: String,
    header: Vec<String>,
    rows: Vec<Vec<Cell>>,
}

/// Everything one export produced; `files` are `(workspace-relative path, bytes)`,
/// the main target first (per-document PDFs when the target is a folder).
struct Rendered {
    files: Vec<(String, Vec<u8>)>,
    sheets: Vec<String>,
    rows: u64,
    documents: u64,
    skipped: Vec<(String, String)>,
}

fn tr(ko: bool, k: &'static str, e: &'static str) -> &'static str {
    if ko {
        k
    } else {
        e
    }
}

fn review_text(rec: &DocumentRecord, ko: bool) -> &'static str {
    match rec.review_status {
        ReviewStatus::None if rec.last_run_id.is_some() => tr(ko, "검토 불필요", "not needed"),
        ReviewStatus::None => tr(ko, "미추출", "not extracted"),
        ReviewStatus::Pending => tr(ko, "검토 대기", "pending"),
        ReviewStatus::Done => tr(ko, "검토 완료", "done"),
    }
}

fn field_status_text(status: FieldStatus, ko: bool) -> &'static str {
    match status {
        FieldStatus::Auto => tr(ko, "자동", "auto"),
        FieldStatus::Approved => tr(ko, "승인", "approved"),
        FieldStatus::Corrected => tr(ko, "수정", "corrected"),
        FieldStatus::Rejected => tr(ko, "반려", "rejected"),
        FieldStatus::Held => tr(ko, "보류", "held"),
    }
}

fn type_label(doc: &Doc, schemas: &BTreeMap<String, SchemaRecord>, ko: bool) -> String {
    match doc.rec.doc_type.as_deref() {
        Some(slug) => schemas.get(slug).map(|s| s.display_name(ko).to_string()).unwrap_or_else(|| slug.to_string()),
        None => tr(ko, UNKNOWN, "unknown").to_string(),
    }
}

/// Value as a cell: numbers for amounts / integers with a numeric normalised
/// form, the printed value otherwise; rejected fields are blank.
fn value_cell(f: &FieldRecord) -> Cell {
    let Some(printed) = f.effective_value() else { return Cell::Empty };
    if matches!(f.kind, FieldKind::Amount | FieldKind::Integer) {
        if let Some(n) = f.normalized.as_deref().and_then(|n| n.trim().parse::<f64>().ok()).filter(|n| n.is_finite()) {
            return Cell::Number(n);
        }
    }
    Cell::from_opt(Some(printed))
}

/// One scalar column of the fields target.
#[derive(Debug, Clone)]
struct Column {
    /// Field path (`total`, `supplier.brn`).
    path: String,
    key: String,
    label_ko: String,
    label_en: String,
}

impl Column {
    fn label(&self, ko: bool) -> &str {
        if ko || self.label_en.is_empty() {
            &self.label_ko
        } else {
            &self.label_en
        }
    }
}

/// Scalar columns of one document: its type's schema (group members with the
/// group label prefixed), or, without a schema, the scalar rows it has.
/// `req.columns` restricts them (schema order kept).
fn doc_columns(doc: &Doc, schemas: &BTreeMap<String, SchemaRecord>, req: &ExportRequest) -> Vec<Column> {
    let schema = doc.rec.doc_type.as_deref().and_then(|dt| schemas.get(dt));
    let mut out: Vec<Column> = Vec::new();
    match schema {
        Some(s) => {
            for f in &s.definition.fields {
                match f.kind {
                    FieldKind::Table => {}
                    FieldKind::Group => {
                        let join = |g: &str, m: &str| if m.starts_with(g) { m.to_string() } else { format!("{g} {m}") };
                        for c in f.fields.iter().filter(|c| c.kind.is_scalar()) {
                            out.push(Column {
                                path: format!("{}.{}", f.key, c.key),
                                key: c.key.clone(),
                                label_ko: join(&f.label_ko, &c.label_ko),
                                label_en: join(&f.label_en, &c.label_en),
                            });
                        }
                    }
                    _ => out.push(Column {
                        path: f.key.clone(),
                        key: f.key.clone(),
                        label_ko: f.label_ko.clone(),
                        label_en: f.label_en.clone(),
                    }),
                }
            }
        }
        None => {
            for f in doc.fields.iter().filter(|f| f.row.is_none()) {
                let or_path = |l: &str| if l.is_empty() { f.path.clone() } else { l.to_string() };
                out.push(Column {
                    path: f.path.clone(),
                    key: f.key.clone(),
                    label_ko: or_path(&f.label_ko),
                    label_en: or_path(&f.label_en),
                });
            }
        }
    }
    let wanted: Vec<String> = req.columns.iter().map(|c| compact(c)).filter(|c| !c.is_empty()).collect();
    if !wanted.is_empty() {
        out.retain(|c| {
            wanted.iter().any(|w| {
                *w == compact(&c.path)
                    || *w == compact(&c.key)
                    || *w == compact(&c.label_ko)
                    || *w == compact(&c.label_en)
            })
        });
    }
    out
}

/// Columns of a set of documents, deduplicated by path in first-seen order.
fn union_columns(docs: &[&Doc], schemas: &BTreeMap<String, SchemaRecord>, req: &ExportRequest) -> Vec<Column> {
    let mut out: Vec<Column> = Vec::new();
    for doc in docs {
        for c in doc_columns(doc, schemas, req) {
            if !out.iter().any(|o| o.path == c.path) {
                out.push(c);
            }
        }
    }
    out
}

fn common_header(req: &ExportRequest) -> Vec<String> {
    let ko = req.ko();
    let mut h = vec![
        tr(ko, "파일", "File").to_string(),
        tr(ko, "유형", "Type").to_string(),
        tr(ko, "날짜", "Date").to_string(),
        tr(ko, "검토 상태", "Review").to_string(),
    ];
    if req.includes(ExportInclude::Confidence) {
        h.push(tr(ko, "신뢰도", "Confidence").to_string());
    }
    h
}

fn fields_header(cols: &[Column], req: &ExportRequest) -> Vec<String> {
    let ko = req.ko();
    let mut h = common_header(req);
    for c in cols {
        let label = c.label(ko);
        h.push(label.to_string());
        if req.includes(ExportInclude::Status) {
            h.push(format!("{label} {}", tr(ko, "(상태)", "(status)")));
        }
        if req.includes(ExportInclude::Confidence) {
            h.push(format!("{label} {}", tr(ko, "(신뢰도)", "(confidence)")));
        }
        if req.includes(ExportInclude::Evidence) {
            h.push(format!("{label} {}", tr(ko, "(근거)", "(evidence)")));
        }
    }
    h
}

fn common_cells(doc: &Doc, type_label: &str, req: &ExportRequest) -> Vec<Cell> {
    let mut row = vec![
        Cell::Text(doc.rec.rel_path.clone()),
        Cell::Text(type_label.to_string()),
        Cell::from_opt(doc.date.as_deref()),
        Cell::Text(review_text(&doc.rec, req.ko()).to_string()),
    ];
    if req.includes(ExportInclude::Confidence) {
        row.push(doc.rec.confidence.map_or(Cell::Empty, Cell::Number));
    }
    row
}

fn fields_row(doc: &Doc, type_label: &str, cols: &[Column], req: &ExportRequest) -> Vec<Cell> {
    let mut row = common_cells(doc, type_label, req);
    for c in cols {
        let f = doc.fields.iter().find(|f| f.row.is_none() && f.path == c.path);
        row.push(f.map_or(Cell::Empty, value_cell));
        if req.includes(ExportInclude::Status) {
            row.push(f.map_or(Cell::Empty, |f| Cell::Text(field_status_text(f.status, req.ko()).to_string())));
        }
        if req.includes(ExportInclude::Confidence) {
            row.push(f.map_or(Cell::Empty, |f| Cell::Number(f.confidence)));
        }
        if req.includes(ExportInclude::Evidence) {
            row.push(f.map_or(Cell::Empty, |f| Cell::from_opt(f.evidence.as_deref())));
        }
    }
    row
}

/// Table key of a cell path (`items[2].amount` → `items`).
fn table_key(path: &str) -> &str {
    path.split('[').next().unwrap_or(path)
}

/// Long-form item rows: file, type, table, row, column, value.
fn items_grid(p: &Prepared, req: &ExportRequest) -> Grid {
    let ko = req.ko();
    let header = vec![
        tr(ko, "파일", "File").to_string(),
        tr(ko, "유형", "Type").to_string(),
        tr(ko, "필드", "Field").to_string(),
        tr(ko, "행", "Row").to_string(),
        tr(ko, "열", "Column").to_string(),
        tr(ko, "값", "Value").to_string(),
    ];
    let mut rows = Vec::new();
    for doc in &p.docs {
        let label = type_label(doc, &p.schemas, ko);
        let schema = doc.rec.doc_type.as_deref().and_then(|dt| p.schemas.get(dt));
        for f in doc.fields.iter().filter(|f| f.row.is_some()) {
            let key = table_key(&f.path);
            let table = schema
                .and_then(|s| s.definition.table(key))
                .map(|t| t.label(ko).to_string())
                .unwrap_or_else(|| key.to_string());
            let column = if ko || f.label_en.is_empty() { &f.label_ko } else { &f.label_en };
            rows.push(vec![
                Cell::Text(doc.rec.rel_path.clone()),
                Cell::Text(label.clone()),
                Cell::Text(table),
                Cell::Number(f64::from(f.row.unwrap_or(0) + 1)),
                Cell::Text(if column.is_empty() { f.key.clone() } else { column.clone() }),
                value_cell(f),
            ]);
        }
    }
    Grid { name: tr(ko, "품목", "Items").to_string(), header, rows }
}

/// Main grids of the fields target (one per type, or a single one) plus the items grid.
fn fields_grids(p: &Prepared, req: &ExportRequest) -> (Vec<Grid>, Grid) {
    let ko = req.ko();
    let mut groups: Vec<(String, Vec<&Doc>)> = Vec::new();
    match req.layout {
        ExportLayout::SheetPerType => {
            for doc in &p.docs {
                let slug = doc.rec.doc_type.clone().unwrap_or_default();
                match groups.iter_mut().find(|(s, _)| *s == slug) {
                    Some((_, docs)) => docs.push(doc),
                    None => groups.push((slug, vec![doc])),
                }
            }
        }
        ExportLayout::Single => groups.push((String::new(), p.docs.iter().collect())),
    }
    let grids = groups
        .into_iter()
        .map(|(_, docs)| {
            let name = match req.layout {
                ExportLayout::SheetPerType => type_label(docs[0], &p.schemas, ko),
                ExportLayout::Single => tr(ko, "필드", "Fields").to_string(),
            };
            let cols = union_columns(&docs, &p.schemas, req);
            let header = fields_header(&cols, req);
            let rows = docs.iter().map(|d| fields_row(d, &type_label(d, &p.schemas, ko), &cols, req)).collect();
            Grid { name, header, rows }
        })
        .collect();
    (grids, items_grid(p, req))
}

fn render_fields(req: &ExportRequest, p: &Prepared) -> Result<Rendered> {
    let (grids, items) = fields_grids(p, req);
    let main_rows: u64 = grids.iter().map(|g| g.rows.len() as u64).sum();
    let rows = main_rows + items.rows.len() as u64;
    let documents = p.docs.len() as u64;
    let mut files = Vec::new();
    let mut sheets = Vec::new();
    match req.format {
        ExportFormat::Xlsx => {
            let all: Vec<&Grid> = grids.iter().chain(std::iter::once(&items)).collect();
            let (bytes, names) = write_xlsx(&all)?;
            sheets = names;
            files.push((p.rel.clone(), bytes));
        }
        ExportFormat::Csv => {
            let all: Vec<&Doc> = p.docs.iter().collect();
            let cols = union_columns(&all, &p.schemas, req);
            let header = fields_header(&cols, req);
            let rows: Vec<Vec<Cell>> =
                p.docs.iter().map(|d| fields_row(d, &type_label(d, &p.schemas, req.ko()), &cols, req)).collect();
            files.push((p.rel.clone(), csv_bytes(&header, &rows, true)?));
            if !items.rows.is_empty() {
                let rel = format!("{}_items.csv", strip_extension(&p.rel));
                files.push((rel, csv_bytes(&items.header, &items.rows, true)?));
            }
        }
        ExportFormat::Jsonl => files.push((p.rel.clone(), jsonl_fields(p, req))),
        ExportFormat::Md => {
            let mut out = String::new();
            for g in &grids {
                out.push_str(&format!("## {}\n\n", g.name));
                md_table(&mut out, &g.header, &g.rows);
                out.push('\n');
            }
            if !items.rows.is_empty() {
                out.push_str(&format!("## {}\n\n", items.name));
                md_table(&mut out, &items.header, &items.rows);
            }
            files.push((p.rel.clone(), out.into_bytes()));
        }
        ExportFormat::PdfSearchable => unreachable!("handled by render_pdf"),
    }
    Ok(Rendered { files, sheets, rows, documents, skipped: Vec::new() })
}

fn jsonl_fields(p: &Prepared, req: &ExportRequest) -> Vec<u8> {
    let mut out = Vec::new();
    for doc in &p.docs {
        let mut fields = Map::new();
        for c in doc_columns(doc, &p.schemas, req) {
            if let Some(f) = doc.fields.iter().find(|f| f.row.is_none() && f.path == c.path) {
                let rejected = f.status == FieldStatus::Rejected;
                fields.insert(
                    c.path.clone(),
                    json!({
                        "value": f.effective_value(),
                        "normalized": if rejected { None } else { f.normalized.as_deref() },
                        "confidence": f.confidence,
                        "status": f.status.as_str(),
                        "evidence": f.evidence,
                    }),
                );
            }
        }
        let mut tables: BTreeMap<&str, BTreeMap<u32, Map<String, Value>>> = BTreeMap::new();
        for f in doc.fields.iter().filter(|f| f.row.is_some()) {
            let value = f.effective_value().map_or(Value::Null, |v| Value::String(v.to_string()));
            tables
                .entry(table_key(&f.path))
                .or_default()
                .entry(f.row.unwrap_or(0))
                .or_default()
                .insert(f.key.clone(), value);
        }
        let items: Map<String, Value> = tables
            .into_iter()
            .map(|(k, rows)| (k.to_string(), Value::Array(rows.into_values().map(Value::Object).collect())))
            .collect();
        let line = json!({
            "id": doc.rec.id,
            "file": doc.rec.rel_path,
            "doc_type": doc.rec.doc_type,
            "date": doc.date,
            "review_status": doc.rec.review_status.as_str(),
            "confidence": doc.rec.confidence,
            "fields": fields,
            "items": items,
        });
        out.extend(serde_json::to_vec(&line).unwrap_or_default());
        out.push(b'\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Table target
// ---------------------------------------------------------------------------

fn table_sheet_name(doc: &Doc, t: &ExtractedTable) -> String {
    format!("{}_p{}_{}", stem_of(&doc.rec.original_name), t.page, t.index + 1)
}

fn table_header(t: &ExtractedTable) -> Vec<String> {
    if !t.header.is_empty() {
        return t.header.clone();
    }
    let width = t.rows.iter().map(Vec::len).max().unwrap_or(0);
    (1..=width).map(|i| i.to_string()).collect()
}

fn table_rows(t: &ExtractedTable) -> Vec<Vec<Cell>> {
    t.rows.iter().map(|r| r.iter().map(|c| Cell::from_opt(Some(c))).collect()).collect()
}

fn render_tables(db: &Db, req: &ExportRequest, p: &Prepared, cancel: Option<&AtomicBool>) -> Result<Rendered> {
    let mut found: Vec<(&Doc, Vec<ExtractedTable>)> = Vec::new();
    let mut skipped = Vec::new();
    for doc in &p.docs {
        if cancelled(cancel) {
            return Err(CoreError::Cancelled);
        }
        let tables = document_tables(db, &doc.rec.id)?;
        if tables.is_empty() {
            skipped.push((doc.rec.id.clone(), "no_tables".to_string()));
        } else {
            found.push((doc, tables));
        }
    }
    if found.is_empty() {
        return Err(CoreError::Invalid("no_tables".into()));
    }
    let rows: u64 = found.iter().flat_map(|(_, ts)| ts.iter()).map(|t| t.rows.len() as u64).sum();
    let documents = found.len() as u64;
    let mut sheets = Vec::new();
    let bytes = match req.format {
        ExportFormat::Xlsx => {
            let grids: Vec<Grid> = found
                .iter()
                .flat_map(|(doc, ts)| {
                    ts.iter().map(|t| Grid {
                        name: table_sheet_name(doc, t),
                        header: table_header(t),
                        rows: table_rows(t),
                    })
                })
                .collect();
            let refs: Vec<&Grid> = grids.iter().collect();
            let (bytes, names) = write_xlsx(&refs)?;
            sheets = names;
            bytes
        }
        ExportFormat::Csv => {
            let mut out: Vec<u8> = "\u{feff}".as_bytes().to_vec();
            let mut first = true;
            for (doc, ts) in &found {
                for t in ts {
                    if !first {
                        out.push(b'\n');
                    }
                    first = false;
                    out.extend(format!("# {} p{} #{}\n", doc.rec.rel_path, t.page, t.index + 1).into_bytes());
                    out.extend(csv_bytes(&table_header(t), &table_rows(t), false)?);
                }
            }
            out
        }
        ExportFormat::Jsonl => {
            let mut out = Vec::new();
            for (doc, ts) in &found {
                for t in ts {
                    let line = json!({
                        "file": doc.rec.rel_path,
                        "page": t.page,
                        "index": t.index + 1,
                        "header": t.header,
                        "rows": t.rows,
                    });
                    out.extend(serde_json::to_vec(&line).unwrap_or_default());
                    out.push(b'\n');
                }
            }
            out
        }
        ExportFormat::Md => {
            let mut out = String::new();
            for (doc, ts) in &found {
                for t in ts {
                    out.push_str(&format!("### {} · p{} · #{}\n\n", doc.rec.rel_path, t.page, t.index + 1));
                    md_table(&mut out, &table_header(t), &table_rows(t));
                    out.push('\n');
                }
            }
            out.into_bytes()
        }
        ExportFormat::PdfSearchable => unreachable!("handled by render_pdf"),
    };
    Ok(Rendered { files: vec![(p.rel.clone(), bytes)], sheets, rows, documents, skipped })
}

// ---------------------------------------------------------------------------
// Text target
// ---------------------------------------------------------------------------

fn text_header(ko: bool) -> Vec<String> {
    vec![tr(ko, "파일", "File").to_string(), tr(ko, "페이지", "Page").to_string(), tr(ko, "텍스트", "Text").to_string()]
}

fn page_text(p: &PageRecord) -> Option<&str> {
    p.text.as_deref().map(str::trim).filter(|t| !t.is_empty())
}

fn render_text(db: &Db, req: &ExportRequest, p: &Prepared, cancel: Option<&AtomicBool>) -> Result<Rendered> {
    let ko = req.ko();
    let mut found: Vec<(&Doc, Vec<PageRecord>)> = Vec::new();
    let mut skipped = Vec::new();
    for doc in &p.docs {
        if cancelled(cancel) {
            return Err(CoreError::Cancelled);
        }
        let mut pages = db.with(|c| list_pages(c, &doc.rec.id, true))?;
        pages.retain(|pg| page_text(pg).is_some());
        if pages.is_empty() {
            skipped.push((doc.rec.id.clone(), "no_text".to_string()));
        } else {
            found.push((doc, pages));
        }
    }
    if found.is_empty() {
        return Err(CoreError::Invalid("no_text".into()));
    }
    let rows: u64 = found.iter().map(|(_, pages)| pages.len() as u64).sum();
    let documents = found.len() as u64;
    let mut sheets = Vec::new();
    let bytes = match req.format {
        ExportFormat::Md => {
            let mut out = String::new();
            for (doc, pages) in &found {
                out.push_str(&format!("# {}\n\n", doc.rec.rel_path));
                for pg in pages {
                    out.push_str(&format!("## p{}\n\n{}\n\n", pg.page_no, page_text(pg).unwrap_or("")));
                }
            }
            out.into_bytes()
        }
        ExportFormat::Jsonl => {
            let mut out = Vec::new();
            for (doc, pages) in &found {
                for pg in pages {
                    let line = json!({ "file": doc.rec.rel_path, "page": pg.page_no, "text": page_text(pg) });
                    out.extend(serde_json::to_vec(&line).unwrap_or_default());
                    out.push(b'\n');
                }
            }
            out
        }
        ExportFormat::Csv | ExportFormat::Xlsx => {
            let rows: Vec<Vec<Cell>> = found
                .iter()
                .flat_map(|(doc, pages)| {
                    pages.iter().map(|pg| {
                        vec![
                            Cell::Text(doc.rec.rel_path.clone()),
                            Cell::Number(f64::from(pg.page_no)),
                            Cell::from_opt(page_text(pg)),
                        ]
                    })
                })
                .collect();
            let header = text_header(ko);
            if req.format == ExportFormat::Csv {
                csv_bytes(&header, &rows, true)?
            } else {
                let grid = Grid { name: tr(ko, "텍스트", "Text").to_string(), header, rows };
                let (bytes, names) = write_xlsx(&[&grid])?;
                sheets = names;
                bytes
            }
        }
        ExportFormat::PdfSearchable => unreachable!("handled by render_pdf"),
    };
    Ok(Rendered { files: vec![(p.rel.clone(), bytes)], sheets, rows, documents, skipped })
}

// ---------------------------------------------------------------------------
// Writers
// ---------------------------------------------------------------------------

fn xlsx_err(e: rust_xlsxwriter::XlsxError) -> CoreError {
    CoreError::Invalid(format!("xlsx: {e}"))
}

fn csv_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Invalid(format!("csv: {e}"))
}

/// Keeps the first `max` UTF-16 code units of `s`.
fn fit_utf16(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut units = 0;
    for c in s.chars() {
        units += c.len_utf16();
        if units > max {
            break;
        }
        out.push(c);
    }
    out
}

/// Excel-safe worksheet name (no `[ ] : * ? / \`, ≤ 31 UTF-16 units, no
/// leading / trailing apostrophe), made unique (case-insensitively) with
/// `_2`, `_3` … against `used`.
fn sheet_name(raw: &str, used: &mut Vec<String>) -> String {
    let mut base: String = raw
        .chars()
        .map(|c| if matches!(c, '[' | ']' | ':' | '*' | '?' | '/' | '\\') || c.is_control() { '_' } else { c })
        .collect();
    base = base.trim().trim_matches('\'').trim().to_string();
    if base.is_empty() {
        base = "Sheet".to_string();
    }
    let taken = |name: &str, used: &[String]| used.iter().any(|u| u.to_lowercase() == name.to_lowercase());
    let mut name = fit_utf16(&base, SHEET_NAME_UNITS);
    let mut n = 2;
    while taken(&name, used) {
        let suffix = format!("_{n}");
        name = format!("{}{suffix}", fit_utf16(&base, SHEET_NAME_UNITS - suffix.len()));
        n += 1;
    }
    used.push(name.clone());
    name
}

fn sheet_names<'a>(raw: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut used = Vec::new();
    for r in raw {
        sheet_name(r, &mut used);
    }
    used
}

/// One worksheet per grid: a header row, then strings / numbers.
fn write_xlsx(grids: &[&Grid]) -> Result<(Vec<u8>, Vec<String>)> {
    let mut wb = Workbook::new();
    let mut used = Vec::new();
    for g in grids {
        let name = sheet_name(&g.name, &mut used);
        let ws = wb.add_worksheet();
        ws.set_name(name).map_err(xlsx_err)?;
        for (c, h) in g.header.iter().enumerate() {
            ws.write_string(0, c as u16, xlsx_text(h)).map_err(xlsx_err)?;
        }
        for (r, row) in g.rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                match cell {
                    Cell::Empty => {}
                    Cell::Text(s) => {
                        ws.write_string(r as u32 + 1, c as u16, xlsx_text(s)).map_err(xlsx_err)?;
                    }
                    Cell::Number(n) => {
                        ws.write_number(r as u32 + 1, c as u16, *n).map_err(xlsx_err)?;
                    }
                }
            }
        }
    }
    let bytes = wb.save_to_buffer().map_err(xlsx_err)?;
    Ok((bytes, used))
}

fn xlsx_text(s: &str) -> String {
    if s.chars().count() > XLSX_MAX_CHARS {
        s.chars().take(XLSX_MAX_CHARS).collect()
    } else {
        s.to_string()
    }
}

/// Spreadsheet formula-injection guard: a cell Excel would evaluate (`=`, `@`,
/// tab, CR, or `+` / `-` not followed by a digit) is stored as text.
fn csv_text(s: &str) -> String {
    let evaluated = s.starts_with(['=', '@', '\t', '\r'])
        || (s.starts_with(['+', '-']) && !s[1..].starts_with(|c: char| c.is_ascii_digit()));
    if evaluated {
        format!("'{s}")
    } else {
        s.to_string()
    }
}

/// RFC 4180 CSV (`\n` line ends), optionally with the UTF-8 BOM Excel needs.
fn csv_bytes(header: &[String], rows: &[Vec<Cell>], bom: bool) -> Result<Vec<u8>> {
    let mut w = csv::WriterBuilder::new().from_writer(Vec::new());
    w.write_record(header.iter().map(|h| csv_text(h))).map_err(csv_err)?;
    for row in rows {
        let record: Vec<String> = row
            .iter()
            .map(|c| match c {
                Cell::Text(s) => csv_text(s),
                other => other.text(),
            })
            .collect();
        w.write_record(&record).map_err(csv_err)?;
    }
    let body = w.into_inner().map_err(csv_err)?;
    let mut out = Vec::with_capacity(body.len() + 3);
    if bom {
        out.extend("\u{feff}".as_bytes());
    }
    out.extend(body);
    Ok(out)
}

/// Markdown pipe table (`|` escaped, line breaks collapsed).
fn md_table(out: &mut String, header: &[String], rows: &[Vec<Cell>]) {
    let esc = |s: &str| s.replace('|', "\\|").replace(['\r', '\n'], " ");
    let width = header.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let line = |cells: Vec<String>| {
        let mut cells = cells;
        cells.resize(width, String::new());
        format!("| {} |\n", cells.join(" | "))
    };
    out.push_str(&line(header.iter().map(|h| esc(h)).collect()));
    out.push_str(&line(vec!["---".to_string(); width]));
    for row in rows {
        out.push_str(&line(row.iter().map(|c| esc(&c.text())).collect()));
    }
}

// ---------------------------------------------------------------------------
// Searchable PDF
// ---------------------------------------------------------------------------

struct PdfPage {
    jpeg: Vec<u8>,
    width_px: u32,
    height_px: u32,
    /// JPEG colour components (1 gray, 3 RGB, 4 CMYK).
    components: u8,
    width_pt: f32,
    height_pt: f32,
    /// Plain text lines (tags removed), top to bottom.
    lines: Vec<String>,
}

/// Dimensions and component count from the JPEG's SOF segment.
fn jpeg_info(bytes: &[u8]) -> Option<(u32, u32, u8)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            i += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
        let sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if sof {
            if i + 9 < bytes.len() {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]);
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]);
                return Some((u32::from(w), u32::from(h), bytes[i + 9]));
            }
            return None;
        }
        if marker == 0xDA {
            return None;
        }
        i += 2 + len;
    }
    None
}

/// Text of a page as plain lines: tags become spaces, whitespace collapses,
/// empty lines are dropped.
fn plain_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| {
            let mut out = String::with_capacity(line.len());
            let mut in_tag = false;
            for c in line.chars() {
                match c {
                    '<' => {
                        in_tag = true;
                        out.push(' ');
                    }
                    '>' if in_tag => in_tag = false,
                    _ if !in_tag => out.push(c),
                    _ => {}
                }
            }
            out.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .filter(|l| !l.is_empty())
        .collect()
}

fn pdf_page(page: &PageRecord, jpeg: Vec<u8>) -> PdfPage {
    let info = jpeg_info(&jpeg);
    let (width_px, height_px) = if page.width_px > 0 && page.height_px > 0 {
        (page.width_px, page.height_px)
    } else {
        info.map_or((1, 1), |(w, h, _)| (w.max(1), h.max(1)))
    };
    let dpi = f64::from(page.dpi.filter(|d| *d > 0).unwrap_or(200));
    let positive = |v: Option<f64>| v.filter(|v| *v > 0.0);
    let (mut width_pt, mut height_pt) = match (positive(page.width_pt), positive(page.height_pt)) {
        (Some(w), Some(h)) => (w, h),
        _ => (f64::from(width_px) * 72.0 / dpi, f64::from(height_px) * 72.0 / dpi),
    };
    // A page rotated at render time keeps its unrotated point size in the DB.
    if (width_px > height_px) != (width_pt > height_pt) {
        std::mem::swap(&mut width_pt, &mut height_pt);
    }
    PdfPage {
        jpeg,
        width_px,
        height_px,
        components: info.map_or(3, |i| i.2),
        width_pt: width_pt as f32,
        height_pt: height_pt as f32,
        lines: page_text(page).map(plain_lines).unwrap_or_default(),
    }
}

/// Two-byte big-endian CIDs (= UTF-16 code units) of a line; characters outside
/// the BMP are skipped, control characters become spaces.
fn encode_cids(line: &str, used: &mut BTreeSet<u16>) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len() * 2);
    for c in line.chars() {
        let c = if c.is_control() { ' ' } else { c };
        let code = c as u32;
        if code > 0xFFFF {
            continue;
        }
        let unit = code as u16;
        used.insert(unit);
        out.extend(unit.to_be_bytes());
    }
    out
}

/// A PDF whose pages are the cached renders with the page text as an invisible
/// text layer (render mode 3) in a non-embedded Identity-H CID font with a
/// ToUnicode map, so readers can search and copy the text.
fn build_searchable_pdf(pages: &[PdfPage]) -> Vec<u8> {
    use pdf_writer::types::{CidFontType, FontFlags, SystemInfo, TextRenderingMode, UnicodeCmap};
    use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str};

    let mut pdf = Pdf::new();
    let mut next = 0;
    let mut alloc = || {
        next += 1;
        Ref::new(next)
    };
    let catalog_id = alloc();
    let tree_id = alloc();
    let font_id = alloc();
    let cid_id = alloc();
    let desc_id = alloc();
    let cmap_id = alloc();
    let page_ids: Vec<Ref> = pages.iter().map(|_| alloc()).collect();
    pdf.catalog(catalog_id).pages(tree_id);
    pdf.pages(tree_id).kids(page_ids.iter().copied()).count(pages.len() as i32);

    let image_name = Name(b"Im1");
    let font_name = Name(b"F1");
    let margin = 4.0f32;
    let mut used: BTreeSet<u16> = BTreeSet::new();
    for (page, page_id) in pages.iter().zip(&page_ids) {
        let image_id = alloc();
        let content_id = alloc();
        let (w, h) = (page.width_pt.max(1.0), page.height_pt.max(1.0));
        {
            let mut p = pdf.page(*page_id);
            p.parent(tree_id).media_box(Rect::new(0.0, 0.0, w, h)).contents(content_id);
            let mut res = p.resources();
            res.x_objects().pair(image_name, image_id);
            res.fonts().pair(font_name, font_id);
            res.finish();
            p.finish();
        }
        {
            let mut img = pdf.image_xobject(image_id, &page.jpeg);
            img.filter(Filter::DctDecode);
            img.width(page.width_px as i32).height(page.height_px as i32);
            match page.components {
                1 => img.color_space().device_gray(),
                4 => img.color_space().device_cmyk(),
                _ => img.color_space().device_rgb(),
            }
            img.bits_per_component(8);
            img.finish();
        }
        let mut content = Content::new();
        content.save_state().transform([w, 0.0, 0.0, h, 0.0, 0.0]).x_object(image_name).restore_state();
        if !page.lines.is_empty() {
            let n = page.lines.len() as f32;
            let leading = ((h - 2.0 * margin) / n).max(0.5);
            let size = (leading * 0.9).clamp(0.5, 12.0);
            content.begin_text().set_text_rendering_mode(TextRenderingMode::Invisible).set_font(font_name, size);
            for (i, line) in page.lines.iter().enumerate() {
                let cids = encode_cids(line, &mut used);
                if cids.is_empty() {
                    continue;
                }
                // Glyphs are one em wide (DW 1000); squeeze long lines into the page.
                let natural = size * (cids.len() / 2) as f32;
                let scaling = (((w - 2.0 * margin) / natural) * 100.0).clamp(1.0, 100.0);
                let y = h - margin - leading * (i as f32 + 1.0);
                content
                    .set_horizontal_scaling(scaling)
                    .set_text_matrix([1.0, 0.0, 0.0, 1.0, margin, y])
                    .show(Str(&cids));
            }
            content.end_text();
        }
        pdf.stream(content_id, &content.finish());
    }

    let info = SystemInfo { registry: Str(b"Adobe"), ordering: Str(b"Identity"), supplement: 0 };
    pdf.type0_font(font_id)
        .base_font(Name(PDF_FONT))
        .encoding_predefined(Name(b"Identity-H"))
        .descendant_font(cid_id)
        .to_unicode(cmap_id);
    pdf.cid_font(cid_id)
        .subtype(CidFontType::Type2)
        .base_font(Name(PDF_FONT))
        .system_info(info)
        .font_descriptor(desc_id)
        .default_width(1000.0)
        .cid_to_gid_map_predefined(Name(b"Identity"));
    pdf.font_descriptor(desc_id)
        .name(Name(PDF_FONT))
        .flags(FontFlags::SYMBOLIC)
        .bbox(Rect::new(-1000.0, -300.0, 1000.0, 1000.0))
        .italic_angle(0.0)
        .ascent(880.0)
        .descent(-120.0)
        .cap_height(720.0)
        .stem_v(80.0);
    let mut cmap = UnicodeCmap::new(Name(b"Adobe-Identity-UCS"), info);
    for unit in &used {
        if let Some(c) = char::from_u32(u32::from(*unit)) {
            cmap.pair(*unit, c);
        }
    }
    let cmap_bytes = cmap.finish();
    pdf.cmap(cmap_id, &cmap_bytes);
    pdf.finish()
}

fn render_pdf(db: &Db, paths: &WorkspacePaths, p: &Prepared, cancel: Option<&AtomicBool>) -> Result<Rendered> {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut used_names: Vec<String> = Vec::new();
    let mut pages_written = 0u64;
    for doc in &p.docs {
        if cancelled(cancel) {
            return Err(CoreError::Cancelled);
        }
        let pages = db.with(|c| list_pages(c, &doc.rec.id, true))?;
        let cache = DocumentCache::for_document(paths, &doc.rec.id);
        let mut pdf_pages = Vec::new();
        for page in &pages {
            let file = cache.page_file(page.page_no);
            if !file.is_file() {
                continue;
            }
            let jpeg = std::fs::read(&file).map_err(|e| CoreError::io(&file, e))?;
            pdf_pages.push(pdf_page(page, jpeg));
        }
        if pdf_pages.is_empty() {
            skipped.push((doc.rec.id.clone(), "no_pages".to_string()));
            continue;
        }
        let bytes = build_searchable_pdf(&pdf_pages);
        let rel = if p.folder {
            let stem = sanitize_component(stem_of(&doc.rec.original_name));
            let mut name = stem.clone();
            let mut n = 2;
            while used_names.iter().any(|u| u.eq_ignore_ascii_case(&name)) {
                name = format!("{stem}_{n}");
                n += 1;
            }
            used_names.push(name.clone());
            format!("{}/{name}.pdf", p.rel)
        } else {
            p.rel.clone()
        };
        pages_written += pdf_pages.len() as u64;
        files.push((rel, bytes));
    }
    if files.is_empty() {
        return Err(CoreError::Invalid("no_pages".into()));
    }
    Ok(Rendered { documents: files.len() as u64, rows: pages_written, sheets: Vec::new(), files, skipped })
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;
    use crate::aggregate::tests::seed_all;
    use crate::documents::render::write_jpeg;
    use crate::pipeline::repo::ensure_builtins;
    use crate::planner::ScopeFilter;

    struct World {
        db: Db,
        paths: WorkspacePaths,
        _dir: tempfile::TempDir,
    }

    fn world() -> World {
        let dir = tempfile::tempdir().unwrap();
        let paths = WorkspacePaths::new(dir.path());
        for d in paths.all_dirs() {
            std::fs::create_dir_all(d).unwrap();
        }
        let db = Db::open_in_memory().unwrap();
        db.with(ensure_builtins).unwrap();
        seed_all(&db);
        // The reviewed receipt has a cached render (searchable PDF source).
        let img = image::RgbImage::from_fn(200, 300, |x, y| {
            if (x / 25 + y / 25) % 2 == 0 {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([40, 40, 40])
            }
        });
        write_jpeg(&img, &DocumentCache::for_document(&paths, "r1").page_file(1), 85).unwrap();
        World { db, paths, _dir: dir }
    }

    fn req(format: ExportFormat, target: ExportTarget, path: &str) -> ExportRequest {
        ExportRequest { format, target, path: Some(path.into()), ..Default::default() }
    }

    fn read(w: &World, rel: &str) -> String {
        String::from_utf8(std::fs::read(from_rel_path(&w.paths.root, rel)).unwrap()).unwrap()
    }

    #[test]
    fn default_path_shape() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
        assert_eq!(
            default_path(ExportFormat::Xlsx, ExportTarget::Fields, Some("영수증"), today),
            "exports/2026-11-04_영수증.xlsx"
        );
        assert_eq!(default_path(ExportFormat::Csv, ExportTarget::Fields, None, today), "exports/2026-11-04_문서.csv");
        assert_eq!(
            default_path(ExportFormat::Jsonl, ExportTarget::Table, Some("세금계산서"), today),
            "exports/2026-11-04_세금계산서_표.jsonl"
        );
        assert_eq!(
            default_path(ExportFormat::Md, ExportTarget::Text, Some(" "), today),
            "exports/2026-11-04_문서_텍스트.md"
        );
        assert_eq!(
            default_path(ExportFormat::PdfSearchable, ExportTarget::Fields, Some("영수증"), today),
            "exports/2026-11-04_검색가능.pdf"
        );
        assert_eq!(
            default_path(ExportFormat::Csv, ExportTarget::Fields, Some("a/b:c"), today),
            "exports/2026-11-04_a_b_c.csv"
        );
    }

    #[test]
    fn fields_to_xlsx_sheet_per_type_and_single() {
        let w = world();
        let res = export(&w.db, &w.paths, &ExportRequest::default(), None).unwrap();
        assert!(res.path.starts_with("exports/") && res.path.ends_with("_문서.xlsx"), "{}", res.path);
        assert_eq!(res.absolute, from_rel_path(&w.paths.root, &res.path));
        assert!(res.absolute.is_file());
        assert_eq!(res.sheets, vec!["영수증".to_string(), "세금계산서".into(), "품목".into()]);
        assert_eq!((res.documents, res.rows), (3, 3 + 5));
        assert!(!res.overwritten && res.skipped.is_empty() && res.extra_files.is_empty());
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&res.absolute).unwrap()).unwrap();
        assert!(zip.index_for_name("xl/worksheets/sheet1.xml").is_some());
        assert!(zip.index_for_name("xl/worksheets/sheet3.xml").is_some());
        let mut workbook = String::new();
        zip.by_name("xl/workbook.xml").unwrap().read_to_string(&mut workbook).unwrap();
        for name in ["영수증", "세금계산서", "품목"] {
            assert!(workbook.contains(&format!("name=\"{name}\"")), "{workbook}");
        }
        // The receipt sheet carries the receipt's scalar fields with a numeric total.
        let mut sheet = String::new();
        zip.by_name("xl/worksheets/sheet1.xml").unwrap().read_to_string(&mut sheet).unwrap();
        assert!(sheet.contains("<v>12500</v>"), "{sheet}");
        let mut strings = String::new();
        zip.by_name("xl/sharedStrings.xml").unwrap().read_to_string(&mut strings).unwrap();
        assert!(
            strings.contains("documents/a_receipt1.pdf") && strings.contains("합계") && strings.contains("검토 대기")
        );
        assert!(strings.contains("아메리카노"), "items sheet lists the line items");

        let single = ExportRequest {
            layout: ExportLayout::Single,
            path: Some("exports/single.xlsx".into()),
            language: Language::En,
            ..Default::default()
        };
        let res = export(&w.db, &w.paths, &single, None).unwrap();
        assert_eq!(res.sheets, vec!["Fields".to_string(), "Items".into()]);
        let pv = preview(&w.db, &w.paths, &single).unwrap();
        assert!(pv.exists);
        assert_eq!(pv.sheets, res.sheets);
        assert!(pv.columns.starts_with(&["File".to_string(), "Type".into(), "Date".into(), "Review".into()]));
        assert!(pv.columns.contains(&"Grand total".to_string()) && pv.columns.contains(&"Total".to_string()));
    }

    #[test]
    fn fields_to_csv_round_trips() {
        let w = world();
        let r = ExportRequest {
            include: vec![ExportInclude::Confidence, ExportInclude::Status, ExportInclude::Evidence],
            ..req(ExportFormat::Csv, ExportTarget::Fields, "exports/fields")
        };
        let res = export(&w.db, &w.paths, &r, None).unwrap();
        assert_eq!(res.path, "exports/fields.csv", "extension appended");
        assert_eq!(res.extra_files, vec!["exports/fields_items.csv".to_string()]);
        let bytes = std::fs::read(&res.absolute).unwrap();
        assert!(bytes.starts_with("\u{feff}".as_bytes()), "UTF-8 BOM for Excel");
        let mut rdr = csv::ReaderBuilder::new().from_reader(bytes.as_slice());
        let header: Vec<String> = rdr.headers().unwrap().iter().map(str::to_string).collect();
        assert_eq!(&header[..5], &["파일", "유형", "날짜", "검토 상태", "신뢰도"]);
        for col in ["상호", "합계", "합계 (상태)", "합계 (신뢰도)", "합계 (근거)", "합계금액", "공급자 상호"]
        {
            assert!(header.contains(&col.to_string()), "missing {col}: {header:?}");
        }
        let records: Vec<Vec<String>> =
            rdr.records().map(|r| r.unwrap().iter().map(str::to_string).collect()).collect();
        assert_eq!(records.len(), 3);
        let col = |name: &str| header.iter().position(|h| h == name).unwrap();
        let r1 = records.iter().find(|r| r[0] == "documents/a_receipt1.pdf").unwrap();
        assert_eq!((r1[1].as_str(), r1[2].as_str(), r1[3].as_str()), ("영수증", "2026-10-14", "검토 불필요"));
        assert_eq!(r1[col("합계")], "12500");
        assert_eq!(r1[col("합계 (상태)")], "자동");
        assert_eq!(r1[col("합계 (근거)")], "12,500 근거");
        let r2 = records.iter().find(|r| r[0] == "documents/b_receipt2.pdf").unwrap();
        assert_eq!(r2[3], "검토 대기");
        assert_eq!(r2[col("합계")], "8,900", "no numeric normalised form → printed value");
        let t1 = records.iter().find(|r| r[0] == "documents/c_invoice.pdf").unwrap();
        assert_eq!(t1[col("합계금액")], "1100000");
        assert_eq!(t1[col("합계")], "", "receipt-only column stays empty for the invoice");
        assert_eq!(t1[col("공급자 상호")], "한빛상사");

        let items = read(&w, "exports/fields_items.csv");
        let mut rdr = csv::ReaderBuilder::new().from_reader(items.as_bytes());
        let header: Vec<String> = rdr.headers().unwrap().iter().map(str::to_string).collect();
        assert_eq!(header, vec!["파일", "유형", "필드", "행", "열", "값"]);
        let rows: Vec<Vec<String>> = rdr.records().map(|r| r.unwrap().iter().map(str::to_string).collect()).collect();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0], vec!["documents/a_receipt1.pdf", "영수증", "품목", "1", "품명", "아메리카노"]);
        assert_eq!(rows[2][5], "9000");

        // Column selection by key / label, formula guard.
        let narrow = ExportRequest { columns: vec!["total".into(), "합계금액".into()], ..r.clone() };
        let narrow = ExportRequest { path: Some("exports/narrow.csv".into()), include: Vec::new(), ..narrow };
        export(&w.db, &w.paths, &narrow, None).unwrap();
        let text = read(&w, "exports/narrow.csv");
        assert_eq!(
            text.lines().next().unwrap().trim_start_matches('\u{feff}'),
            "파일,유형,날짜,검토 상태,합계,합계금액"
        );
        assert_eq!(csv_text("=SUM(A1)"), "'=SUM(A1)");
        assert_eq!(csv_text("-12"), "-12");
    }

    #[test]
    fn fields_to_jsonl_and_md() {
        let w = world();
        let res =
            export(&w.db, &w.paths, &req(ExportFormat::Jsonl, ExportTarget::Fields, "exports/f.jsonl"), None).unwrap();
        let text = read(&w, &res.path);
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 3);
        let r1 = lines.iter().find(|l| l["id"] == "r1").unwrap();
        assert_eq!(r1["file"], "documents/a_receipt1.pdf");
        assert_eq!(r1["doc_type"], "receipt");
        assert_eq!(r1["date"], "2026-10-14");
        assert_eq!(r1["review_status"], "none");
        assert_eq!(r1["fields"]["total"]["value"], "12,500");
        assert_eq!(r1["fields"]["total"]["normalized"], "12500");
        assert_eq!(r1["fields"]["total"]["status"], "auto");
        assert_eq!(r1["fields"]["total"]["evidence"], "12,500 근거");
        assert_eq!(r1["items"]["items"][0]["name"], "아메리카노");
        assert_eq!(r1["items"]["items"][1]["amount"], "3,500");
        let t1 = lines.iter().find(|l| l["id"] == "t1").unwrap();
        assert_eq!(t1["fields"]["supplier.name"]["value"], "한빛상사");
        assert_eq!(t1["fields"]["grand_total"]["confidence"], 0.95);

        let res = export(&w.db, &w.paths, &req(ExportFormat::Md, ExportTarget::Fields, "exports/f.md"), None).unwrap();
        let md = read(&w, &res.path);
        assert!(md.contains("## 영수증\n") && md.contains("## 세금계산서\n") && md.contains("## 품목\n"), "{md}");
        assert!(md.contains("| 파일 | 유형 | 날짜 | 검토 상태 | 상호 |"), "{md}");
        assert!(
            md.contains("| documents/a_receipt1.pdf | 영수증 | 2026-10-14 | 검토 불필요 | 스타벅스 역삼점 |"),
            "{md}"
        );
        assert!(md.contains("| 12500 |"), "{md}");
        assert_eq!(md_escaped("a|b\nc"), "a\\|b c");
    }

    fn md_escaped(s: &str) -> String {
        let mut out = String::new();
        md_table(&mut out, &["h".to_string()], &[vec![Cell::Text(s.to_string())]]);
        out.lines().nth(2).unwrap().trim_matches(|c| c == '|' || c == ' ').to_string()
    }

    #[test]
    fn table_and_text_targets() {
        let w = world();
        let res = export(&w.db, &w.paths, &req(ExportFormat::Md, ExportTarget::Table, "exports/t.md"), None).unwrap();
        assert_eq!((res.documents, res.rows), (1, 2));
        assert_eq!(res.skipped, vec![("r2".to_string(), "no_tables".into()), ("t1".into(), "no_tables".into())]);
        let md = read(&w, &res.path);
        assert!(md.starts_with("### documents/a_receipt1.pdf · p1 · #1\n\n| 품명 | 수량 | 금액 |\n| --- | --- | --- |\n| 아메리카노 | 2 | 9,000 |\n"), "{md}");

        let res = export(&w.db, &w.paths, &req(ExportFormat::Csv, ExportTarget::Table, "exports/t.csv"), None).unwrap();
        let csv = read(&w, &res.path);
        assert!(
            csv.starts_with("\u{feff}# documents/a_receipt1.pdf p1 #1\n품명,수량,금액\n아메리카노,2,\"9,000\"\n"),
            "{csv}"
        );

        let res =
            export(&w.db, &w.paths, &req(ExportFormat::Jsonl, ExportTarget::Table, "exports/t.jsonl"), None).unwrap();
        let line: Value = serde_json::from_str(read(&w, &res.path).lines().next().unwrap()).unwrap();
        assert_eq!(line["header"][2], "금액");
        assert_eq!(line["rows"][1][0], "쿠키");

        let res =
            export(&w.db, &w.paths, &req(ExportFormat::Xlsx, ExportTarget::Table, "exports/t.xlsx"), None).unwrap();
        assert_eq!(res.sheets, vec!["a_receipt1_p1_1".to_string()]);

        let res = export(&w.db, &w.paths, &req(ExportFormat::Md, ExportTarget::Text, "exports/x.md"), None).unwrap();
        assert_eq!((res.documents, res.rows), (2, 2));
        assert_eq!(res.skipped, vec![("r2".to_string(), "no_text".into())]);
        let md = read(&w, &res.path);
        assert!(md.starts_with("# documents/a_receipt1.pdf\n\n## p1\n\n# 스타벅스 역삼점\n"), "{md}");
        assert!(md.contains("# documents/c_invoice.pdf\n\n## p1\n\n세금계산서"), "{md}");

        let res = export(&w.db, &w.paths, &req(ExportFormat::Csv, ExportTarget::Text, "exports/x.csv"), None).unwrap();
        let text = read(&w, &res.path);
        let mut rdr = csv::ReaderBuilder::new().from_reader(text.as_bytes());
        assert_eq!(rdr.headers().unwrap().iter().collect::<Vec<_>>(), vec!["파일", "페이지", "텍스트"]);
        let rows: Vec<Vec<String>> = rdr.records().map(|r| r.unwrap().iter().map(str::to_string).collect()).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0][0].as_str(), rows[0][1].as_str()), ("documents/a_receipt1.pdf", "1"));
        assert!(rows[0][2].contains("합계 12,500"));

        let res =
            export(&w.db, &w.paths, &req(ExportFormat::Jsonl, ExportTarget::Text, "exports/x.jsonl"), None).unwrap();
        let line: Value = serde_json::from_str(read(&w, &res.path).lines().next().unwrap()).unwrap();
        assert_eq!(line["page"], 1);
        assert!(line["text"].as_str().unwrap().contains("<table>"));

        let res =
            export(&w.db, &w.paths, &req(ExportFormat::Xlsx, ExportTarget::Text, "exports/x.xlsx"), None).unwrap();
        assert_eq!(res.sheets, vec!["텍스트".to_string()]);
        let pv = preview(&w.db, &w.paths, &req(ExportFormat::Xlsx, ExportTarget::Text, "exports/y.xlsx")).unwrap();
        assert_eq!((pv.columns, pv.sheets), (text_header(true), vec!["텍스트".to_string()]));
    }

    #[test]
    fn existing_target_needs_overwrite() {
        let w = world();
        let r = req(ExportFormat::Csv, ExportTarget::Fields, "exports/x.csv");
        let first = export(&w.db, &w.paths, &r, None).unwrap();
        assert!(!first.overwritten);
        let err = export(&w.db, &w.paths, &r, None).err().unwrap();
        assert!(matches!(err, CoreError::Invalid(ref c) if c == "export_exists"), "{err}");
        let pv = preview(&w.db, &w.paths, &r).unwrap();
        assert!(pv.exists);
        assert_eq!(pv.documents, 3);
        let again = export(&w.db, &w.paths, &ExportRequest { overwrite: true, ..r }, None).unwrap();
        assert!(again.overwritten);
        assert!(again.absolute.is_file());
        assert!(!w.paths.root.join("exports").join("x.csv.part").exists(), "no temp file left");
    }

    #[test]
    fn rejects_paths_outside_the_workspace() {
        let w = world();
        for bad in [
            "/tmp/x.csv",
            "../x.csv",
            "exports/../../x.csv",
            ".doculens/x.csv",
            "C:\\x.csv",
            "exports/.DOCULENS/x",
            "\\\\nas\\share\\x.csv",
            "..",
            "/",
        ] {
            let err = export(&w.db, &w.paths, &req(ExportFormat::Csv, ExportTarget::Fields, bad), None).err().unwrap();
            assert!(matches!(err, CoreError::Invalid(ref c) if c == "path_outside"), "{bad}: {err}");
        }
        // Windows separators and odd characters are normalised, the extension is appended.
        let res = export(&w.db, &w.paths, &req(ExportFormat::Csv, ExportTarget::Fields, "exports\\sub\\re:port"), None)
            .unwrap();
        assert_eq!(res.path, "exports/sub/re_port.csv");
        assert!(res.absolute.is_file());
        let res =
            export(&w.db, &w.paths, &req(ExportFormat::Csv, ExportTarget::Fields, "exports/Report.CSV"), None).unwrap();
        assert_eq!(res.path, "exports/Report.CSV");
    }

    #[test]
    fn only_reviewed_skips_pending_documents() {
        let w = world();
        let r =
            ExportRequest { only_reviewed: true, ..req(ExportFormat::Jsonl, ExportTarget::Fields, "exports/r.jsonl") };
        let pv = preview(&w.db, &w.paths, &r).unwrap();
        assert_eq!((pv.documents, pv.skipped), (2, 1));
        let res = export(&w.db, &w.paths, &r, None).unwrap();
        assert_eq!(res.documents, 2);
        assert_eq!(res.skipped, vec![("r2".to_string(), "review_pending".into())]);
        assert_eq!(read(&w, &res.path).lines().count(), 2);
        // Scoped to the pending document only → nothing to export.
        let none = ExportRequest {
            scope: Scope { doc_ids: vec!["r2".into()], ..Default::default() },
            path: Some("exports/none.jsonl".into()),
            ..r
        };
        let err = export(&w.db, &w.paths, &none, None).err().unwrap();
        assert!(matches!(err, CoreError::Invalid(ref c) if c == "no_documents"), "{err}");
        // A type filter names the default file after the type.
        let typed = ExportRequest {
            scope: Scope {
                filter: ScopeFilter { doc_type: Some("receipt".into()), ..Default::default() },
                ..Default::default()
            },
            ..Default::default()
        };
        let pv = preview(&w.db, &w.paths, &typed).unwrap();
        assert!(pv.path.ends_with("_영수증.xlsx"), "{}", pv.path);
        assert_eq!(pv.sheets, vec!["영수증".to_string(), "품목".into()]);
    }

    #[test]
    fn cancellation_leaves_nothing_behind() {
        let w = world();
        let cancel = AtomicBool::new(true);
        let r = req(ExportFormat::Md, ExportTarget::Text, "exports/c.md");
        let err = export(&w.db, &w.paths, &r, Some(&cancel)).err().unwrap();
        assert!(matches!(err, CoreError::Cancelled));
        assert!(!w.paths.root.join("exports").join("c.md").exists());
    }

    #[test]
    fn searchable_pdf_has_pages_and_a_unicode_text_layer() {
        let w = world();
        let r = ExportRequest {
            scope: Scope { doc_ids: vec!["r1".into()], ..Default::default() },
            ..req(ExportFormat::PdfSearchable, ExportTarget::Fields, "exports/r.pdf")
        };
        let res = export(&w.db, &w.paths, &r, None).unwrap();
        assert_eq!(res.path, "exports/r.pdf");
        assert_eq!((res.documents, res.rows), (1, 1));
        assert!(res.extra_files.is_empty());
        let doc = lopdf::Document::load(&res.absolute).unwrap();
        assert_eq!(doc.get_pages().len(), 1);
        let bytes = std::fs::read(&res.absolute).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/ToUnicode"), "font has a ToUnicode map");
        assert!(text.contains("/Identity-H") && text.contains("/MalgunGothic") && text.contains("/DCTDecode"));
        assert!(text.contains("D569"), "'합' (U+D569) is mapped");
        assert!(text.contains("3 Tr"), "invisible text render mode");
        assert!(
            bytes.windows(4).any(|w| w == [0xFF, 0xD8, 0xFF, 0xE0] || w == [0xFF, 0xD8, 0xFF, 0xDB]),
            "JPEG embedded"
        );
        // Page size follows the render: 200×300 px at 200 dpi → 72×108 pt.
        assert!(text.contains("/MediaBox [0 0 72 108]"), "{}", &text[..400]);

        // Several documents go to a folder named after the target; documents without a
        // cached render are skipped.
        let all = ExportRequest { scope: Scope::default(), path: Some("exports/all.pdf".into()), ..r };
        let pv = preview(&w.db, &w.paths, &all).unwrap();
        assert_eq!(pv.path, "exports/all");
        let res = export(&w.db, &w.paths, &all, None).unwrap();
        assert_eq!(res.path, "exports/all");
        assert!(res.absolute.is_dir());
        assert_eq!(res.extra_files, vec!["exports/all/a_receipt1.pdf".to_string()]);
        assert_eq!(res.skipped, vec![("r2".to_string(), "no_pages".into()), ("t1".into(), "no_pages".into())]);
        assert_eq!(
            lopdf::Document::load(w.paths.root.join("exports/all/a_receipt1.pdf")).unwrap().get_pages().len(),
            1
        );
        let err = export(&w.db, &w.paths, &all, None).err().unwrap();
        assert!(matches!(err, CoreError::Invalid(ref c) if c == "export_exists"), "folder exists: {err}");
    }

    #[test]
    fn helpers() {
        assert_eq!(jpeg_info(&[0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, 1, 44, 0, 200, 3]), Some((200, 300, 3)));
        assert_eq!(jpeg_info(b"not a jpeg"), None);
        assert_eq!(plain_lines("a <b>bold</b>\n\n<table><tr><td>x</td><td>y</td></tr></table>"), vec!["a bold", "x y"]);
        let mut used = BTreeSet::new();
        assert_eq!(encode_cids("A합\u{1F600}\t", &mut used), vec![0x00, 0x41, 0xD5, 0x69, 0x00, 0x20]);
        assert_eq!(used.len(), 3);
        let mut names = Vec::new();
        assert_eq!(sheet_name("a/b:c*d?e[f]", &mut names), "a_b_c_d_e_f_");
        assert_eq!(sheet_name("A_B_C_D_E_F_", &mut names), "A_B_C_D_E_F__2", "case-insensitive uniqueness");
        let long = "가".repeat(40);
        assert_eq!(sheet_name(&long, &mut names).chars().count(), 31);
        assert_eq!(sheet_name(&long, &mut names), format!("{}_2", "가".repeat(29)));
        assert_eq!(sheet_name("'quoted'", &mut names), "quoted");
        assert_eq!(sheet_name("", &mut names), "Sheet");
        assert_eq!(ensure_extension("exports/a", "csv"), "exports/a.csv");
        assert_eq!(ensure_extension("exports/a.CSV", "csv"), "exports/a.CSV");
        assert_eq!(ensure_extension("exports/a.txt", "csv"), "exports/a.txt.csv");
        assert_eq!(strip_extension("exports/all.pdf"), "exports/all");
        assert_eq!(strip_extension("all.pdf"), "all");
        assert_eq!(normalize_rel_path("./exports//a/./b.csv").unwrap(), "exports/a/b.csv");
    }
}
