//! Format-preserving edits of a document's own file (1.0.0 §2-1, step 1): the characters of a matched
//! text change inside the original package — HWPX, DOCX, PPTX — and nothing else does.
//!
//! Every part of the zip is copied as it is except the text parts, whose XML is passed through event
//! by event. A replacement lands in the run that held the start of the match, so the font, size and
//! colour of that run, the tables, the pictures and the page setup stay what they were:
//!
//! * a paragraph's text is the text of its `<t>` nodes joined, so a word that is split over runs of
//!   different formatting is still found; the new text goes into the first run, the rest of the match
//!   is taken out of the others;
//! * HWPX keeps a line-layout cache per paragraph (`hp:linesegarray`): it is dropped from a changed
//!   paragraph so that 한컴오피스 lays the paragraph out again;
//! * DOCX text with leading / trailing blanks gets `xml:space="preserve"`.
//!
//! Left alone, with a reason: documents with tracked changes, digitally signed packages. The edited file
//! is written to a new path, never over the source; registering it as a new edition is the caller's job.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;

use quick_xml::events::{BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer, XmlVersion};
use serde::{Deserialize, Serialize};

use crate::documents::import::{self, ImportDeps};
use crate::documents::{self, convert, repo as docs, DocumentRecord, NewDocument, SourceKind};
use crate::error::{CoreError, Result};
use crate::knowledge::{repo as knowledge_repo, VersionInput};
use crate::projects::assign::{AssignRequest, Placement};

/// Largest text part rewritten (decompressed).
const MAX_XML_BYTES: u64 = 64 * 1024 * 1024;
/// Replacements one request may carry.
pub const MAX_EDITS: usize = 200;
/// Changes listed in a report (every change is still made and counted).
const MAX_CHANGES: usize = 300;
/// Characters of the paragraph shown on each side of a change.
const CONTEXT_CHARS: usize = 40;

/// Reason codes of `CoreError::Invalid` (the UI has a sentence for each).
pub const UNSUPPORTED: &str = "edit_unsupported_format";
pub const TRACKED_CHANGES: &str = "edit_tracked_changes";
pub const SIGNED: &str = "edit_signed";
pub const EMPTY_FIND: &str = "edit_find_empty";
pub const LINE_BREAK: &str = "edit_line_break";
pub const TOO_MANY: &str = "edit_too_many";
pub const SAME_FILE: &str = "edit_same_file";
pub const NOTHING_FOUND: &str = "edit_nothing_found";

/// One replacement: `find` exactly as it stands in the document (inside one paragraph) becomes `replace`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replacement {
    pub find: String,
    pub replace: String,
    /// Only the n-th occurrence in the document (1-based, reading order); `None` = every occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<u32>,
}

/// One change: where it is and the text around it before and after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// The part of the package (`word/document.xml`, `Contents/section0.xml`, `ppt/slides/slide3.xml`).
    pub part: String,
    /// The slide (PPTX) or section (HWPX) number, 1-based, when the part has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<u32>,
    /// Which replacement of the request (0-based).
    pub edit: usize,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EditReport {
    /// Occurrences found, per replacement of the request (what `occurrence` counts over).
    pub found: Vec<u32>,
    /// Occurrences replaced, per replacement of the request.
    pub replaced: Vec<u32>,
    /// The changes in reading order (the first few hundred).
    pub changes: Vec<Change>,
    /// Parts of the package that were rewritten.
    pub parts: Vec<String>,
}

impl EditReport {
    pub fn total(&self) -> u32 {
        self.replaced.iter().sum()
    }
}

/// Whether a file of this kind can be edited in place.
pub fn supported(kind: SourceKind) -> bool {
    matches!(kind, SourceKind::Hwpx | SourceKind::Docx | SourceKind::Pptx)
}

/// What `apply` would change, without writing anything.
pub fn preview(src: &Path, kind: SourceKind, edits: &[Replacement]) -> Result<EditReport> {
    run(src, kind, edits, None)
}

/// Writes the edited copy of `src` to `out`.
pub fn apply(src: &Path, kind: SourceKind, edits: &[Replacement], out: &Path) -> Result<EditReport> {
    if out == src {
        return Err(CoreError::Invalid(SAME_FILE.into()));
    }
    run(src, kind, edits, Some(out))
}

// ---------------------------------------------------------------------------
// A new edition
// ---------------------------------------------------------------------------

/// The edited copy, registered: a new document that is the latest edition of the one it was made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edition {
    pub report: EditReport,
    /// The new document.
    pub doc_id: String,
    pub rel_path: String,
    pub version_label: String,
    /// Projects the new edition was put into (the ones of the original, on the same side).
    pub projects: u32,
    pub warnings: Vec<String>,
}

/// `계약서.docx` → `계약서 (수정).docx`, `계약서 (수정 2).docx` …: the first name that is neither a file
/// nor a library row. An edition of an edition counts on from the same stem.
fn edition_name(doc: &DocumentRecord, imp: &ImportDeps<'_>) -> Result<(std::path::PathBuf, String)> {
    let abs = doc.absolute_path(imp.paths);
    let dir = abs.parent().ok_or_else(|| CoreError::Invalid("edit_no_folder".into()))?.to_path_buf();
    let file = documents::nfc(&abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let (stem, ext) = match file.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (file.clone(), String::new()),
    };
    // "… (수정)" / "… (수정 7)" at the end of the stem is an earlier edition's mark.
    let base = match stem.rfind(" (수정") {
        Some(at)
            if stem.ends_with(')')
                && stem[at + " (수정".len()..stem.len() - 1].trim().chars().all(|c| c.is_ascii_digit()) =>
        {
            stem[..at].to_string()
        }
        _ => stem,
    };
    let folder = doc.rel_path.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
    for n in 1..=999u32 {
        let name = if n == 1 { format!("{base} (수정){ext}") } else { format!("{base} (수정 {n}){ext}") };
        let rel = format!("{folder}{name}");
        let taken = dir.join(&name).exists() || imp.db.with(|c| docs::find_by_rel_path(c, &rel))?.is_some();
        if !taken {
            return Ok((dir.join(&name), rel));
        }
    }
    Err(CoreError::Invalid("edit_too_many_editions".into()))
}

/// Writes the edited copy of `doc` next to it and registers it as the latest edition of the same
/// document (1.0.0 §2-1): the original file is not touched, its row becomes the earlier edition. The new
/// document keeps the title, type, tags, role and the projects / reference groups of the original, and
/// is converted and rendered like an imported file.
pub fn save_edition(imp: &ImportDeps<'_>, doc: &DocumentRecord, edits: &[Replacement], ko: bool) -> Result<Edition> {
    let kind = doc.source_kind;
    check(kind, edits)?;
    let src = doc.absolute_path(imp.paths);
    let (target, rel) = edition_name(doc, imp)?;
    let discard = |e: CoreError| {
        let _ = std::fs::remove_file(&target);
        e
    };
    let report = apply(&src, kind, edits, &target).map_err(discard)?;
    if report.total() == 0 {
        return Err(discard(CoreError::Invalid(NOTHING_FOUND.into())));
    }
    let mut warnings = Vec::new();
    // The app's own reader has to open what was written, and see the change.
    let text = convert::read_document(&target, kind).map_err(discard)?.text();
    let left_over = edits.iter().zip(&report.replaced).any(|(e, n)| {
        *n > 0 && e.occurrence.is_none() && !e.replace.contains(e.find.as_str()) && text.contains(e.find.as_str())
    });
    if left_over {
        warnings.push(if ko {
            "바꾼 뒤에도 같은 글이 문서에 남아 있습니다(여러 문단에 걸친 글은 바꾸지 않습니다). 수정본을 열어 확인해 주세요.".to_string()
        } else {
            "The text is still found after the change (a text that spans paragraphs is not replaced). Please check the edited copy.".to_string()
        });
    }

    // -- the library row
    let meta = std::fs::metadata(&target).map_err(|e| discard(CoreError::io(&target, e)))?;
    let hash = import::blake3_file(&target).map_err(discard)?;
    let name = documents::nfc(&target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let id = documents::new_id();
    imp.db
        .with(|c| {
            docs::insert_document(
                c,
                &NewDocument {
                    id: id.clone(),
                    rel_path: rel.clone(),
                    original_name: name.clone(),
                    source_kind: kind,
                    content_hash: hash.clone(),
                    size_bytes: meta.len(),
                    file_modified_at: meta.modified().ok().map(chrono::DateTime::<chrono::Utc>::from),
                    role: doc.role,
                    import_batch_id: None,
                },
            )?;
            docs::set_doc_type(c, &id, doc.doc_type.as_deref(), doc.doc_type_source.as_deref())?;
            docs::set_title(c, &id, doc.title.as_deref())?;
            docs::set_tags(c, &id, &doc.tags).map(|_| ())
        })
        .map_err(discard)?;

    // -- editions: the original steps back, the copy is the latest
    let now = chrono::Local::now();
    let version_label = format!("{} {}", if ko { "수정" } else { "edited" }, now.format("%Y-%m-%d %H:%M"));
    imp.db.with(|c| {
        let before = knowledge_repo::get_version(c, &doc.id)?;
        let group_key = before
            .as_ref()
            .map(|v| v.group_key.clone())
            .or_else(|| doc.title.clone().filter(|t| !t.trim().is_empty()))
            .unwrap_or_else(|| doc.original_name.clone());
        knowledge_repo::set_version(
            c,
            &VersionInput {
                document_id: doc.id.clone(),
                group_key: group_key.clone(),
                version_label: before
                    .as_ref()
                    .and_then(|v| v.version_label.clone())
                    .or_else(|| Some(if ko { "원본" } else { "original" }.to_string())),
                effective_date: before.as_ref().and_then(|v| v.effective_date.clone()),
                is_latest: false,
                note: before.as_ref().and_then(|v| v.note.clone()),
            },
        )?;
        knowledge_repo::set_version(
            c,
            &VersionInput {
                document_id: id.clone(),
                group_key,
                version_label: Some(version_label.clone()),
                effective_date: Some(now.format("%Y-%m-%d").to_string()),
                is_latest: true,
                note: None,
            },
        )
    })?;

    // -- where the original is, the edition is: projects (work documents / reference material) and
    //    the common reference groups
    let places = imp.db.with(crate::projects::repo::document_projects)?.remove(&doc.id).unwrap_or_default();
    let add: Vec<Placement> = places
        .iter()
        .map(|p| Placement { doc_id: id.clone(), project_id: p.id.clone(), reference: p.reference })
        .collect();
    let mut projects = 0u32;
    if !add.is_empty() {
        match crate::projects::assign_documents(imp.db, imp.paths, &AssignRequest { add, ..Default::default() }) {
            Ok(_) => projects = places.len() as u32,
            Err(e) => warnings.push(if ko {
                format!("수정본을 원본의 프로젝트에 넣지 못했습니다: {e}")
            } else {
                format!("The edited copy could not be put into the original's projects: {e}")
            }),
        }
    }
    imp.db.with(|c| {
        for (collection, _) in knowledge_repo::collections_of_document(c, &doc.id)? {
            let common = knowledge_repo::get_collection(c, &collection)?.is_some_and(|col| col.is_common());
            if common {
                knowledge_repo::add_docs_to_collection(c, &collection, std::slice::from_ref(&id))?;
            }
        }
        // The role follows the original whatever the memberships made of it.
        docs::set_role(c, &id, doc.role)
    })?;

    // -- conversion and page images, like an imported file
    let row = imp.db.with(|c| docs::get_document(c, &id))?.ok_or_else(|| CoreError::NotFound(id.clone()))?;
    if let Err(e) = import::process_document(imp, &row, true, None, None, &mut |_| {}) {
        warnings.push(if ko {
            format!("수정본을 저장하고 등록했지만 변환 / 렌더링에 실패했습니다: {e}")
        } else {
            format!("The edited copy was saved and registered, but conversion / rendering failed: {e}")
        });
    }
    Ok(Edition { report, doc_id: id, rel_path: rel, version_label, projects, warnings })
}

// ---------------------------------------------------------------------------
// Package
// ---------------------------------------------------------------------------

/// The text parts of a package and the order they are read in: `(order, unit)` of a part name.
fn text_part(kind: SourceKind, name: &str) -> Option<(u32, Option<u32>)> {
    let lower = name.to_ascii_lowercase();
    let numbered = |prefix: &str| -> Option<u32> {
        let digits = lower.strip_prefix(prefix)?.strip_suffix(".xml")?;
        if digits.is_empty() {
            return Some(0);
        }
        digits.parse::<u32>().ok()
    };
    match kind {
        SourceKind::Docx => {
            if lower == "word/document.xml" {
                return Some((0, None));
            }
            // After the body: headers, footers, notes.
            let groups = ["word/header", "word/footer", "word/footnotes", "word/endnotes"];
            groups
                .iter()
                .enumerate()
                .find_map(|(g, p)| numbered(p).map(|n| (1_000 * (g as u32 + 1) + n.min(999), None)))
        }
        SourceKind::Hwpx => numbered("contents/section").map(|n| (n, Some(n + 1))),
        SourceKind::Pptx => numbered("ppt/slides/slide").map(|n| (n, Some(n))),
        _ => None,
    }
}

fn signed(kind: SourceKind, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    match kind {
        SourceKind::Hwpx => lower == "meta-inf/signatures.xml" || lower.starts_with("meta-inf/documentsignatures"),
        _ => lower.starts_with("_xmlsignatures/"),
    }
}

fn check(kind: SourceKind, edits: &[Replacement]) -> Result<()> {
    if !supported(kind) {
        return Err(CoreError::Invalid(UNSUPPORTED.into()));
    }
    if edits.is_empty() || edits.len() > MAX_EDITS {
        return Err(CoreError::Invalid(if edits.is_empty() { EMPTY_FIND } else { TOO_MANY }.into()));
    }
    for e in edits {
        if e.find.trim().is_empty() {
            return Err(CoreError::Invalid(EMPTY_FIND.into()));
        }
        // A paragraph is one line of text here: a line break is markup, not a character.
        if e.find.contains(['\n', '\r']) || e.replace.contains(['\n', '\r']) {
            return Err(CoreError::Invalid(LINE_BREAK.into()));
        }
    }
    Ok(())
}

fn run(src: &Path, kind: SourceKind, edits: &[Replacement], out: Option<&Path>) -> Result<EditReport> {
    check(kind, edits)?;
    let file = File::open(src).map_err(|e| CoreError::io(src, e))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| CoreError::Convert(format!("not a zip archive: {e}")))?;

    // -- read the text parts, in reading order
    let mut parts: Vec<Part> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| CoreError::Convert(format!("zip entry {i}: {e}")))?;
        let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
        if signed(kind, &name) {
            return Err(CoreError::Invalid(SIGNED.into()));
        }
        let Some((order, unit)) = text_part(kind, &name) else { continue };
        if entry.size() > MAX_XML_BYTES {
            return Err(CoreError::Convert(format!("{name} is too large")));
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .by_ref()
            .take(MAX_XML_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|e| CoreError::Convert(format!("{name}: {e}")))?;
        parts.push(Part::parse(name, order, unit, &bytes, kind)?);
    }
    if parts.is_empty() {
        return Err(CoreError::Convert(format!("no text part in the package (not a {} file)", kind.as_str())));
    }
    parts.sort_by_key(|p| p.order);

    // -- plan: one replacement after the other, each over the text the ones before it left
    let mut report = EditReport { found: vec![0; edits.len()], replaced: vec![0; edits.len()], ..Default::default() };
    for (n, edit) in edits.iter().enumerate() {
        let mut seen = 0u32;
        for part in &mut parts {
            part.replace(n, edit, &mut seen, &mut report);
        }
        report.found[n] = seen;
    }
    report.parts = parts.iter().filter(|p| p.changed()).map(|p| p.name.clone()).collect();

    // -- write: the changed parts from their events, everything else as it is
    let Some(out) = out else { return Ok(report) };
    let rewritten: HashMap<&str, Vec<u8>> =
        parts.iter().filter(|p| p.changed()).map(|p| Ok((p.name.as_str(), p.write(kind)?))).collect::<Result<_>>()?;
    let out_file = File::create(out).map_err(|e| CoreError::io(out, e))?;
    let mut writer = zip::ZipWriter::new(out_file);
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| CoreError::Convert(format!("zip entry {i}: {e}")))?;
        let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
        match rewritten.get(name.as_str()) {
            Some(bytes) => {
                let opts = entry.options();
                drop(entry);
                writer.start_file(name.as_str(), opts).map_err(|e| CoreError::Convert(format!("{name}: {e}")))?;
                writer.write_all(bytes).map_err(|e| CoreError::io(out, e))?;
            }
            None => writer.raw_copy_file(entry).map_err(|e| CoreError::Convert(format!("{name}: {e}")))?,
        }
    }
    writer.finish().map_err(|e| CoreError::Convert(e.to_string()))?;
    Ok(report)
}

// ---------------------------------------------------------------------------
// One text part
// ---------------------------------------------------------------------------

/// A stretch of character data inside a `<t>` element: the events `first..=last` and its text now.
struct Node {
    first: usize,
    last: usize,
    /// Event index of the `<t>` start tag the node sits in.
    element: usize,
    text: String,
    changed: bool,
}

struct Paragraph {
    nodes: Vec<usize>,
    /// Event ranges of the paragraph's own layout caches (`hp:linesegarray`).
    caches: Vec<(usize, usize)>,
    /// Inside `mc:Fallback`: a second copy of a text box for older programs — kept in step with the
    /// first copy but not counted.
    fallback: bool,
    changed: bool,
}

struct Part {
    name: String,
    order: u32,
    unit: Option<u32>,
    events: Vec<Event<'static>>,
    nodes: Vec<Node>,
    paragraphs: Vec<Paragraph>,
}

/// An open element while a part is read: what it means for the text inside it.
#[derive(Clone, Copy, PartialEq)]
enum Open {
    Paragraph(usize),
    /// A `<t>` element (the event index of its start tag).
    Text(usize),
    /// A layout cache (the event index of its start tag).
    Cache(usize),
    Fallback,
    Other,
}

fn local(name: &str) -> &str {
    match name.rsplit_once(':') {
        Some((_, local)) => local,
        None => name,
    }
}

fn entity(r: &quick_xml::events::BytesRef<'_>) -> Option<String> {
    match r.resolve_char_ref() {
        Ok(Some(c)) => Some(c.to_string()),
        Ok(None) => match r.xml_content(XmlVersion::Implicit1_0).as_ref() {
            "amp" => Some("&".into()),
            "lt" => Some("<".into()),
            "gt" => Some(">".into()),
            "quot" => Some("\"".into()),
            "apos" => Some("'".into()),
            _ => None,
        },
        Err(_) => None,
    }
}

impl Part {
    fn parse(name: String, order: u32, unit: Option<u32>, bytes: &[u8], kind: SourceKind) -> Result<Part> {
        let mut reader = Reader::from_reader(bytes);
        reader.config_mut().trim_text(false);
        reader.config_mut().expand_empty_elements = false;
        let mut events: Vec<Event<'static>> = Vec::new();
        let mut buf = Vec::new();
        loop {
            let ev = reader.read_event_into(&mut buf).map_err(|e| CoreError::Convert(format!("{name}: xml: {e}")))?;
            if matches!(ev, Event::Eof) {
                break;
            }
            events.push(ev.into_owned());
            buf.clear();
        }
        let mut part = Part { name, order, unit, events, nodes: Vec::new(), paragraphs: Vec::new() };

        let mut stack: Vec<Open> = Vec::new();
        // The node being collected: (first event, text so far).
        let mut run: Option<(usize, String)> = None;
        for i in 0..part.events.len() {
            // Character data continues the current node; anything else ends it.
            let piece = match &part.events[i] {
                Event::Text(t) => Some(t.xml_content(XmlVersion::Implicit1_0).into_owned()),
                Event::CData(c) => Some(c.xml_content(XmlVersion::Implicit1_0).into_owned()),
                Event::GeneralRef(r) => Some(entity(r).unwrap_or_default()),
                _ => None,
            };
            let in_text = match stack.last() {
                Some(Open::Text(element)) => Some(*element),
                _ => None,
            };
            if let (Some(piece), Some(_)) = (&piece, in_text) {
                match run.as_mut() {
                    Some((_, text)) => text.push_str(piece),
                    None => run = Some((i, piece.clone())),
                }
                continue;
            }
            if let (Some((first, text)), Some(element)) = (run.take(), in_text) {
                part.push_node(
                    first,
                    i - 1,
                    element,
                    text,
                    &stack.iter().rev().find_map(|o| match o {
                        Open::Paragraph(p) => Some(*p),
                        _ => None,
                    }),
                );
            }
            let opened = |start: &BytesStart<'_>, stack: &[Open], paragraphs: &mut Vec<Paragraph>| -> Result<Open> {
                let qualified = start.name();
                let name = local(qualified.as_ref());
                // Tracked changes: a replacement would have to be a tracked change itself.
                let tracked = match kind {
                    SourceKind::Docx => {
                        matches!(name, "ins" | "del" | "moveFrom" | "moveTo") && qualified.as_ref().starts_with("w:")
                    }
                    SourceKind::Hwpx => matches!(name, "insertBegin" | "deleteBegin"),
                    _ => false,
                };
                if tracked {
                    return Err(CoreError::Invalid(TRACKED_CHANGES.into()));
                }
                Ok(match name {
                    "p" => {
                        paragraphs.push(Paragraph {
                            nodes: Vec::new(),
                            caches: Vec::new(),
                            fallback: stack.contains(&Open::Fallback),
                            changed: false,
                        });
                        Open::Paragraph(paragraphs.len() - 1)
                    }
                    "t" => Open::Text(i),
                    "linesegarray" => Open::Cache(i),
                    "Fallback" => Open::Fallback,
                    _ => Open::Other,
                })
            };
            match &part.events[i] {
                Event::Start(s) => {
                    let open = opened(s, &stack, &mut part.paragraphs)?;
                    stack.push(open);
                }
                Event::Empty(s) => {
                    // `<hp:linesegarray/>` and the like: opened and closed at once.
                    if let Open::Cache(at) = opened(s, &stack, &mut part.paragraphs)? {
                        part.cache(&stack, at, i);
                    }
                }
                Event::End(_) => {
                    if let Some(Open::Cache(at)) = stack.pop() {
                        part.cache(&stack, at, i);
                    }
                }
                _ => {}
            }
        }
        Ok(part)
    }

    fn push_node(&mut self, first: usize, last: usize, element: usize, text: String, paragraph: &Option<usize>) {
        // Text outside every paragraph is not a place a replacement can land.
        let Some(p) = paragraph else { return };
        self.nodes.push(Node { first, last, element, text, changed: false });
        self.paragraphs[*p].nodes.push(self.nodes.len() - 1);
    }

    /// A layout cache belongs to the paragraph it is a child of.
    fn cache(&mut self, stack: &[Open], from: usize, to: usize) {
        if let Some(Open::Paragraph(p)) = stack.last() {
            self.paragraphs[*p].caches.push((from, to));
        }
    }

    fn changed(&self) -> bool {
        self.paragraphs.iter().any(|p| p.changed)
    }

    /// Applies one replacement to the part. `seen` counts the occurrences over the whole document.
    fn replace(&mut self, n: usize, edit: &Replacement, seen: &mut u32, report: &mut EditReport) {
        for p in 0..self.paragraphs.len() {
            let joined: String = self.paragraphs[p].nodes.iter().map(|i| self.nodes[*i].text.as_str()).collect();
            let fallback = self.paragraphs[p].fallback;
            let mut wanted: Vec<usize> = Vec::new();
            for (at, _) in joined.match_indices(edit.find.as_str()) {
                if fallback {
                    // The copy follows the original only when every occurrence is replaced.
                    if edit.occurrence.is_none() {
                        wanted.push(at);
                    }
                    continue;
                }
                *seen += 1;
                if edit.occurrence.is_none_or(|o| o == *seen) {
                    wanted.push(at);
                }
            }
            if wanted.is_empty() {
                continue;
            }
            // From the end, so that the offsets before a change stay valid.
            for at in wanted.iter().rev() {
                self.splice(p, *at, at + edit.find.len(), &edit.replace);
            }
            self.paragraphs[p].changed = true;
            if fallback {
                continue;
            }
            report.replaced[n] += wanted.len() as u32;
            let after: String = self.paragraphs[p].nodes.iter().map(|i| self.nodes[*i].text.as_str()).collect();
            let grown = edit.replace.len() as isize - edit.find.len() as isize;
            for (k, at) in wanted.iter().enumerate() {
                if report.changes.len() >= MAX_CHANGES {
                    break;
                }
                let moved = (*at as isize + grown * k as isize) as usize;
                report.changes.push(Change {
                    part: self.name.clone(),
                    unit: self.unit,
                    edit: n,
                    before: around(&joined, *at, at + edit.find.len()),
                    after: around(&after, moved, moved + edit.replace.len()),
                });
            }
        }
    }

    /// Replaces the bytes `from..to` of paragraph `p`'s joined text: the new text goes into the node
    /// that holds `from`, the rest of the range is taken out of the nodes after it.
    fn splice(&mut self, p: usize, from: usize, to: usize, with: &str) {
        let mut start = 0usize;
        let mut placed = false;
        for i in self.paragraphs[p].nodes.clone() {
            let node = &mut self.nodes[i];
            let end = start + node.text.len();
            if end > from && start < to {
                let (a, b) = (from.max(start) - start, to.min(end) - start);
                let insert = if placed { "" } else { with };
                node.text.replace_range(a..b, insert);
                node.changed = true;
                placed = true;
            }
            start = end;
            if start >= to && placed {
                break;
            }
        }
    }

    fn write(&self, kind: SourceKind) -> Result<Vec<u8>> {
        let err = |e: std::io::Error| CoreError::Convert(format!("{}: xml write: {e}", self.name));
        let new_text: BTreeMap<usize, &Node> = self.nodes.iter().filter(|n| n.changed).map(|n| (n.first, n)).collect();
        let mut dropped: HashSet<usize> = HashSet::new();
        for n in new_text.values() {
            dropped.extend(n.first + 1..=n.last);
        }
        for p in self.paragraphs.iter().filter(|p| p.changed) {
            for (from, to) in &p.caches {
                dropped.extend(*from..=*to);
            }
        }
        // DOCX keeps the blanks at the ends of a `<w:t>` only when it is told to.
        let preserve: HashSet<usize> = match kind {
            SourceKind::Docx => new_text
                .values()
                .filter(|n| n.text.starts_with(char::is_whitespace) || n.text.ends_with(char::is_whitespace))
                .map(|n| n.element)
                .collect(),
            _ => HashSet::new(),
        };
        let mut writer = Writer::new(Vec::with_capacity(self.events.len() * 24));
        for (i, ev) in self.events.iter().enumerate() {
            if let Some(node) = new_text.get(&i) {
                if !node.text.is_empty() {
                    writer.write_event(Event::Text(BytesText::new(&node.text))).map_err(err)?;
                }
                continue;
            }
            if dropped.contains(&i) {
                continue;
            }
            match ev {
                Event::Start(s)
                    if preserve.contains(&i) && s.try_get_attribute("xml:space").ok().flatten().is_none() =>
                {
                    let mut start = s.to_owned();
                    start.push_attribute(("xml:space", "preserve"));
                    writer.write_event(Event::Start(start)).map_err(err)?;
                }
                _ => writer.write_event(ev.borrow()).map_err(err)?,
            }
        }
        Ok(writer.into_inner())
    }
}

/// `text[from..to]` with a little of the paragraph on both sides, cut at character boundaries.
fn around(text: &str, from: usize, to: usize) -> String {
    let (from, to) = (from.min(text.len()), to.min(text.len()));
    let head: String = text[..from].chars().rev().take(CONTEXT_CHARS).collect::<Vec<_>>().into_iter().rev().collect();
    let tail: String = text[to..].chars().take(CONTEXT_CHARS).collect();
    let cut_head = text[..from].chars().count() > CONTEXT_CHARS;
    let cut_tail = text[to..].chars().count() > CONTEXT_CHARS;
    format!("{}{head}{}{tail}{}", if cut_head { "…" } else { "" }, &text[from..to], if cut_tail { "…" } else { "" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::convert;

    fn package(path: &Path, parts: &[(&str, &str)]) {
        use zip::write::SimpleFileOptions;
        let mut z = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, data) in parts {
            z.start_file(*name, SimpleFileOptions::default()).unwrap();
            z.write_all(data.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    fn part_of(path: &Path, name: &str) -> String {
        let mut archive = zip::ZipArchive::new(BufReader::new(File::open(path).unwrap())).unwrap();
        let mut s = String::new();
        archive.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    fn one(find: &str, replace: &str) -> Vec<Replacement> {
        vec![Replacement { find: find.into(), replace: replace.into(), occurrence: None }]
    }

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;
    const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:styleId="a"><w:name w:val="Normal"/></w:style></w:styles>"#;

    /// A date split over three runs — plain, bold, plain — in a paragraph, the same date in a table
    /// cell, and once more in the header.
    fn docx(dir: &Path) -> std::path::PathBuf {
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W}><w:body><w:p><w:pPr><w:jc w:val="both"/></w:pPr><w:r><w:rPr><w:rFonts w:eastAsia="바탕"/><w:sz w:val="22"/></w:rPr><w:t xml:space="preserve">제3조 납품 기한은 2026년 </w:t></w:r><w:r><w:rPr><w:b/><w:color w:val="C00000"/></w:rPr><w:t>11월 30</w:t></w:r><w:r><w:t>일까지로 한다.</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>기한</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>11월 30일</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:t>R&amp;D 비용은 별도</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="11906" w:h="16838"/></w:sectPr></w:body></w:document>"#
        );
        let header = format!(r#"<w:hdr {W}><w:p><w:r><w:t>납품 11월 30일</w:t></w:r></w:p></w:hdr>"#);
        let path = dir.join("contract.docx");
        package(&path, &[("word/styles.xml", STYLES), ("word/document.xml", &body), ("word/header1.xml", &header)]);
        path
    }

    /// The characters change where they stand: the run that held the start of the match keeps its
    /// formatting and takes the new text, every other part of the package is the same bytes.
    #[test]
    fn a_replacement_changes_the_text_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let src = docx(dir.path());
        let out = dir.path().join("contract (수정).docx");
        let edits = one("11월 30일", "12월 31일");

        // The preview says what would change and writes nothing.
        let planned = preview(&src, SourceKind::Docx, &edits).unwrap();
        assert_eq!((planned.found.clone(), planned.replaced.clone()), (vec![3], vec![3]));
        assert!(!out.exists());
        assert_eq!(planned.changes[0].before, "제3조 납품 기한은 2026년 11월 30일까지로 한다.");
        assert_eq!(planned.changes[0].after, "제3조 납품 기한은 2026년 12월 31일까지로 한다.");
        assert_eq!(planned.parts, ["word/document.xml", "word/header1.xml"]);

        let done = apply(&src, SourceKind::Docx, &edits, &out).unwrap();
        assert_eq!(done, planned);
        let body = part_of(&out, "word/document.xml");
        // The bold red run holds the start of the match: it takes the whole new text; the rest of the
        // match leaves the run after it. Run properties, paragraph properties and the page stay.
        assert!(body.contains(r#"<w:r><w:rPr><w:b/><w:color w:val="C00000"/></w:rPr><w:t>12월 31일</w:t></w:r><w:r><w:t>까지로 한다.</w:t></w:r>"#), "{body}");
        assert!(body.contains(r#"<w:rFonts w:eastAsia="바탕"/><w:sz w:val="22"/></w:rPr><w:t xml:space="preserve">제3조 납품 기한은 2026년 </w:t>"#));
        assert!(body.contains(r#"<w:tc><w:p><w:r><w:t>12월 31일</w:t></w:r></w:p></w:tc>"#));
        assert!(
            body.contains(r#"<w:pPr><w:jc w:val="both"/></w:pPr>"#)
                && body.contains(r#"<w:pgSz w:w="11906" w:h="16838"/>"#)
        );
        assert!(body.contains("R&amp;D 비용은 별도"), "untouched text keeps its escapes: {body}");
        assert!(part_of(&out, "word/header1.xml").contains("납품 12월 31일"));
        assert_eq!(part_of(&out, "word/styles.xml"), STYLES);
        // The source is as it was, and the app's own reader sees the new text.
        assert!(part_of(&src, "word/document.xml").contains("11월 30"));
        let text = convert::read_document(&out, SourceKind::Docx).unwrap().text();
        assert!(text.contains("납품 기한은 2026년 12월 31일까지로 한다.") && !text.contains("11월 30일"), "{text}");
    }

    #[test]
    fn one_occurrence_escapes_and_blanks_at_the_ends() {
        let dir = tempfile::tempdir().unwrap();
        let src = docx(dir.path());
        let out = dir.path().join("out.docx");
        // Only the second one in reading order: the table cell, not the paragraph or the header.
        let second =
            vec![Replacement { find: "11월 30일".into(), replace: "12월 1일".into(), occurrence: Some(2) }];
        let r = apply(&src, SourceKind::Docx, &second, &out).unwrap();
        assert_eq!(
            (r.found.clone(), r.replaced.clone(), r.parts.clone()),
            (vec![3], vec![1], vec!["word/document.xml".to_string()])
        );
        let body = part_of(&out, "word/document.xml");
        assert!(body.contains("<w:t>12월 1일</w:t>") && body.contains("<w:t>11월 30</w:t>"), "{body}");
        assert!(part_of(&out, "word/header1.xml").contains("11월 30일"), "a part without changes is copied as it is");

        // The text is matched unescaped and written escaped; a text that now ends in a blank is kept whole.
        let r = apply(&src, SourceKind::Docx, &one("R&D 비용은", "연구 <개발> 비용은"), &out).unwrap();
        assert_eq!(r.replaced, [1]);
        assert!(part_of(&out, "word/document.xml").contains("<w:t>연구 &lt;개발&gt; 비용은 별도</w:t>"));
        apply(&src, SourceKind::Docx, &one("비용은 별도", "비용은 별도 "), &out).unwrap();
        let body = part_of(&out, "word/document.xml");
        assert!(body.contains(r#"<w:t xml:space="preserve">R&amp;D 비용은 별도 </w:t>"#), "{body}");

        // Several replacements run one after the other; one that is not there is reported as zero.
        let both = vec![
            Replacement { find: "제3조".into(), replace: "제4조".into(), occurrence: None },
            Replacement { find: "없는 글".into(), replace: "x".into(), occurrence: None },
            Replacement { find: "제4조 납품".into(), replace: "제4조(납품)".into(), occurrence: None },
        ];
        let r = apply(&src, SourceKind::Docx, &both, &out).unwrap();
        assert_eq!((r.found.clone(), r.replaced.clone()), (vec![1, 0, 1], vec![1, 0, 1]));
        assert!(part_of(&out, "word/document.xml").contains("제4조(납품) 기한은 2026년 "));
    }

    /// HWPX: the changed paragraph loses its line-layout cache (한컴오피스 lays it out again), the others
    /// keep theirs; a tab inside the text stays where it is.
    #[test]
    fn hwpx_paragraphs_drop_their_layout_cache_when_changed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.hwpx");
        let ns = r#"xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section""#;
        let cache = r#"<hp:linesegarray><hp:lineseg textpos="0" vertpos="0" vertsize="1000" textheight="1000" baseline="850" spacing="600" horzpos="0" horzsize="42520" flags="393216"/></hp:linesegarray>"#;
        let section = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec {ns}><hp:p id="1" paraPrIDRef="3" styleIDRef="0"><hp:run charPrIDRef="7"><hp:t>검수 기간은 </hp:t></hp:run><hp:run charPrIDRef="9"><hp:t>14일<hp:tab width="4000" leader="0" type="1"/>이내</hp:t></hp:run>{cache}</hp:p><hp:p id="2" paraPrIDRef="3" styleIDRef="0"><hp:run charPrIDRef="7"><hp:t>하자보수 기간은 1년</hp:t></hp:run>{cache}</hp:p></hs:sec>"#
        );
        package(&src, &[("mimetype", "application/hwp+zip"), ("Contents/section0.xml", &section)]);
        let out = dir.path().join("b.hwpx");
        let r = apply(&src, SourceKind::Hwpx, &one("검수 기간은 14일", "검수 기간은 납품 후 21일"), &out).unwrap();
        assert_eq!((r.replaced.clone(), r.changes[0].unit), (vec![1], Some(1)));
        let xml = part_of(&out, "Contents/section0.xml");
        // The new text is in the first run (its character shape 7); the second run keeps the tab and the rest.
        assert!(
            xml.contains(r#"<hp:run charPrIDRef="7"><hp:t>검수 기간은 납품 후 21일</hp:t></hp:run><hp:run charPrIDRef="9"><hp:t><hp:tab width="4000" leader="0" type="1"/>이내</hp:t></hp:run></hp:p>"#),
            "{xml}"
        );
        assert_eq!(xml.matches("<hp:linesegarray>").count(), 1, "only the untouched paragraph keeps its cache: {xml}");
        assert!(xml.contains(&format!(r#"<hp:t>하자보수 기간은 1년</hp:t></hp:run>{cache}</hp:p>"#)));
        assert_eq!(part_of(&out, "mimetype"), "application/hwp+zip");
    }

    /// PPTX: occurrences are counted in slide order whatever the order of the entries in the zip.
    #[test]
    fn pptx_slides_are_read_in_their_order() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.pptx");
        let slide = |text: &str| {
            format!(
                r#"<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:bodyPr/><a:p><a:r><a:rPr lang="ko-KR" sz="2400" b="1"/><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
            )
        };
        package(
            &src,
            &[
                ("ppt/slides/slide10.xml", &slide("일정: 1차 납품")),
                ("ppt/slides/slide2.xml", &slide("범위: 1차 납품")),
                ("ppt/slides/_rels/slide2.xml.rels", "<Relationships/>"),
            ],
        );
        let out = dir.path().join("b.pptx");
        let first =
            vec![Replacement { find: "1차 납품".into(), replace: "최종 납품".into(), occurrence: Some(1) }];
        let r = apply(&src, SourceKind::Pptx, &first, &out).unwrap();
        assert_eq!((r.found.clone(), r.replaced.clone()), (vec![2], vec![1]));
        assert_eq!((r.changes[0].part.as_str(), r.changes[0].unit), ("ppt/slides/slide2.xml", Some(2)));
        assert!(part_of(&out, "ppt/slides/slide2.xml")
            .contains(r#"<a:rPr lang="ko-KR" sz="2400" b="1"/><a:t>범위: 최종 납품</a:t>"#));
        assert!(part_of(&out, "ppt/slides/slide10.xml").contains("일정: 1차 납품"));
        assert_eq!(part_of(&out, "ppt/slides/_rels/slide2.xml.rels"), "<Relationships/>");
    }

    /// What is not edited, with the reason: tracked changes, signed packages, other formats, a request
    /// that cannot be a replacement.
    #[test]
    fn documents_and_requests_that_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.docx");
        let code = |r: Result<EditReport>| match r {
            Err(CoreError::Invalid(code)) => code,
            other => panic!("{other:?}"),
        };
        let tracked = dir.path().join("tracked.docx");
        let body = format!(
            r#"<w:document {W}><w:body><w:p><w:r><w:t>납품 </w:t></w:r><w:ins w:id="1" w:author="a"><w:r><w:t>기한</w:t></w:r></w:ins></w:p></w:body></w:document>"#
        );
        package(&tracked, &[("word/document.xml", &body)]);
        assert_eq!(code(preview(&tracked, SourceKind::Docx, &one("납품", "인도"))), TRACKED_CHANGES);
        assert_eq!(code(apply(&tracked, SourceKind::Docx, &one("납품", "인도"), &out)), TRACKED_CHANGES);
        assert!(
            !out.exists()
                || std::fs::metadata(&out).unwrap().len() == 0
                || part_of(&tracked, "word/document.xml").contains("납품")
        );

        let signed = dir.path().join("signed.docx");
        let plain =
            format!(r#"<w:document {W}><w:body><w:p><w:r><w:t>납품 기한</w:t></w:r></w:p></w:body></w:document>"#);
        package(&signed, &[("word/document.xml", &plain), ("_xmlsignatures/sig1.xml", "<Signature/>")]);
        assert_eq!(code(preview(&signed, SourceKind::Docx, &one("납품", "인도"))), SIGNED);

        let ok = dir.path().join("ok.docx");
        package(&ok, &[("word/document.xml", &plain)]);
        assert_eq!(code(preview(&ok, SourceKind::Pdf, &one("납품", "인도"))), UNSUPPORTED);
        assert_eq!(code(preview(&ok, SourceKind::Docx, &one("  ", "인도"))), EMPTY_FIND);
        assert_eq!(code(preview(&ok, SourceKind::Docx, &[])), EMPTY_FIND);
        assert_eq!(code(preview(&ok, SourceKind::Docx, &one("납품", "인도\n기한"))), LINE_BREAK);
        assert_eq!(code(apply(&ok, SourceKind::Docx, &one("납품", "인도"), &ok)), SAME_FILE);
        // Nothing found is not an error: the report says so.
        let none = preview(&ok, SourceKind::Docx, &one("없는 글", "x")).unwrap();
        assert_eq!((none.total(), none.parts.len()), (0, 0));
        assert!(supported(SourceKind::Hwpx) && !supported(SourceKind::Hwp) && !supported(SourceKind::Xlsx));
    }

    /// The edited copy becomes the latest edition: a new file next to the original (which keeps its
    /// bytes), a new document with the original's title, type, tags and projects, and the original
    /// steps back to an earlier edition.
    #[test]
    fn an_edit_is_saved_as_the_latest_edition() {
        use crate::db::{Db, JournalMode};
        use crate::documents::convert::StubPdfPrinter;
        use crate::documents::render::RenderOptions;
        use crate::documents::DocumentRole;
        use crate::paths::WorkspacePaths;
        use crate::projects::{repo as projects_repo, ProjectInput};

        let dir = tempfile::tempdir().unwrap();
        let paths = WorkspacePaths::new(dir.path().join("ws"));
        for d in paths.all_dirs() {
            std::fs::create_dir_all(d).unwrap();
        }
        let db = Db::open(&paths.db_file(), JournalMode::Delete).unwrap();
        let folder = paths.root.join("계약");
        std::fs::create_dir_all(&folder).unwrap();
        let src = docx(&folder);
        let bytes_before = std::fs::read(&src).unwrap();
        db.with(|c| {
            docs::insert_document(
                c,
                &NewDocument {
                    id: "d1".into(),
                    rel_path: "계약/contract.docx".into(),
                    original_name: "contract.docx".into(),
                    source_kind: SourceKind::Docx,
                    content_hash: import::blake3_file(&src).unwrap(),
                    size_bytes: bytes_before.len() as u64,
                    file_modified_at: None,
                    role: DocumentRole::Work,
                    import_batch_id: None,
                },
            )?;
            docs::set_doc_type(c, "d1", Some("dev_contract"), Some("manual"))?;
            docs::set_title(c, "d1", Some("개발 용역 계약서"))?;
            docs::set_tags(c, "d1", &["한빛".to_string()]).map(|_| ())
        })
        .unwrap();
        let project = db
            .with(|c| projects_repo::save_project(c, &ProjectInput { name: "한빛 납품".into(), ..Default::default() }))
            .unwrap();
        db.with(|c| projects_repo::add_documents(c, &project.id, &["d1".to_string()])).unwrap();
        let imp = ImportDeps {
            db: &db,
            paths: &paths,
            render: None,
            printer: &StubPdfPrinter,
            render_options: RenderOptions { dpi: 72, ..Default::default() },
        };
        let original = db.with(|c| docs::get_document(c, "d1")).unwrap().unwrap();

        let edition = save_edition(&imp, &original, &one("11월 30일", "12월 31일"), true).unwrap();
        assert_eq!(
            (edition.report.total(), edition.rel_path.as_str(), edition.projects),
            (3, "계약/contract (수정).docx", 1)
        );
        assert!(edition.version_label.starts_with("수정 20"), "{}", edition.version_label);
        // The original file is the same bytes; the copy has the new text.
        assert_eq!(std::fs::read(&src).unwrap(), bytes_before);
        let copy = folder.join("contract (수정).docx");
        assert!(part_of(&copy, "word/document.xml").contains("12월 31일"));
        // The new document carries what the original had.
        let new = db.with(|c| docs::get_document(c, &edition.doc_id)).unwrap().unwrap();
        assert_eq!(
            (new.title.as_deref(), new.doc_type.as_deref(), new.role),
            (Some("개발 용역 계약서"), Some("dev_contract"), DocumentRole::Work)
        );
        assert_eq!(
            (new.tags.clone(), new.source_kind, new.original_name.as_str()),
            (vec!["한빛".to_string()], SourceKind::Docx, "contract (수정).docx")
        );
        assert_eq!(
            db.with(|c| projects_repo::member_ids(c, &project.id)).unwrap(),
            ["d1".to_string(), edition.doc_id.clone()]
        );
        // Editions: one group, the copy is the latest, the original is kept as "원본".
        let (old_v, new_v) = db
            .with(|c| {
                Ok((
                    knowledge_repo::get_version(c, "d1")?.unwrap(),
                    knowledge_repo::get_version(c, &edition.doc_id)?.unwrap(),
                ))
            })
            .unwrap();
        assert_eq!(
            (old_v.group_key.as_str(), old_v.is_latest, old_v.version_label.as_deref()),
            ("개발 용역 계약서", false, Some("원본"))
        );
        assert_eq!((new_v.group_key.as_str(), new_v.is_latest), ("개발 용역 계약서", true));
        assert_eq!(new_v.version_label.as_deref(), Some(edition.version_label.as_str()));

        // An edition of the edition counts on from the same name; the first copy steps back in turn.
        let second = save_edition(&imp, &new, &one("12월 31일", "1월 15일"), true).unwrap();
        assert_eq!(second.rel_path, "계약/contract (수정 2).docx");
        let stepped_back = db.with(|c| knowledge_repo::get_version(c, &edition.doc_id)).unwrap().unwrap();
        assert!(
            !stepped_back.is_latest && stepped_back.version_label.as_deref() == Some(edition.version_label.as_str())
        );

        // Nothing to change: no file, no document.
        let rows = || db.with(|c| c.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get::<_, i64>(0))).unwrap();
        let before = rows();
        let err = save_edition(&imp, &original, &one("없는 글", "x"), true).unwrap_err();
        assert!(matches!(&err, CoreError::Invalid(code) if code == NOTHING_FOUND), "{err:?}");
        assert!(!folder.join("contract (수정 3).docx").exists());
        assert_eq!(rows(), before);
    }

    /// A text box is stored twice (the drawing and a fallback for older programs): both copies change,
    /// the change is counted once.
    #[test]
    fn the_fallback_copy_of_a_text_box_follows() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("box.docx");
        let boxed = |tag: &str| {
            format!(r#"<mc:{tag}><w:txbxContent><w:p><w:r><w:t>대외비</w:t></w:r></w:p></w:txbxContent></mc:{tag}>"#)
        };
        let body = format!(
            r#"<w:document {W}><w:body><w:p><w:r><mc:AlternateContent>{}{}</mc:AlternateContent></w:r></w:p></w:body></w:document>"#,
            boxed("Choice"),
            boxed("Fallback")
        );
        package(&src, &[("word/document.xml", &body)]);
        let out = dir.path().join("out.docx");
        let r = apply(&src, SourceKind::Docx, &one("대외비", "사내 한정"), &out).unwrap();
        assert_eq!((r.found.clone(), r.replaced.clone(), r.changes.len()), (vec![1], vec![1], 1));
        let xml = part_of(&out, "word/document.xml");
        assert_eq!((xml.matches("사내 한정").count(), xml.matches("대외비").count()), (2, 0), "{xml}");
    }
}
