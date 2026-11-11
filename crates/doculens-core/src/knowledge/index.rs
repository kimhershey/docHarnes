//! Chunking + index maintenance over the `documents` table: which documents
//! need (re)chunking, hashing of their page text, and the status dashboard.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use unicode_normalization::UnicodeNormalization;

use super::chunker::{chunk_document, ChunkOptions, PageText};
use super::tokenize::Analyzer;
use super::{embed, repo, IndexStatus, INDEX_VERSION};

/// `kv` key holding the analyzer + chunk-size signature the index was built with.
pub const KV_INDEX_SIGNATURE: &str = "knowledge.index_signature";

/// Identity of the chunker configuration: a different dictionary or chunk size
/// means every document is re-chunked on the next run.
pub fn index_signature(analyzer: &Analyzer, options: &ChunkOptions) -> String {
    let info = analyzer.info();
    format!(
        "v{}|{}@{}|target={}",
        INDEX_VERSION,
        info.kind,
        info.version.unwrap_or_else(|| "-".into()),
        options.target_tokens
    )
}
use crate::db::Db;
use crate::documents::{repo as docs, DocumentRecord, PageRecord};
use crate::error::Result;

pub struct IndexDeps<'a> {
    pub db: &'a Db,
    pub analyzer: &'a Analyzer,
    pub options: ChunkOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexOutcome {
    Indexed(u32),
    Unchanged,
    NoText,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IndexReport {
    pub indexed: Vec<String>,
    pub unchanged: u32,
    pub no_text: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub chunks: u64,
    pub cancelled: bool,
}

pub struct IndexProgress<'a> {
    pub done: u32,
    pub total: u32,
    pub current: &'a str,
}

/// Hash of the page texts in page order (detects content changes cheaply).
pub fn page_text_hash(pages: &[PageRecord]) -> String {
    let mut h = blake3::Hasher::new();
    for p in pages {
        h.update(&p.page_no.to_le_bytes());
        h.update(p.text.as_deref().unwrap_or("").as_bytes());
        h.update(b"\0");
    }
    h.finalize().to_hex().to_string()
}

/// Text that is analysed / embedded for a chunk: heading path first so a
/// clause keeps its context ("제3장 복무 > 제12조 출장\n① 출장비는…").
pub fn indexed_text(heading_path: Option<&str>, text: &str) -> String {
    match heading_path {
        Some(h) if !h.trim().is_empty() => format!("{h}\n{text}"),
        _ => text.to_string(),
    }
}

/// Chunks one document unless its text is unchanged since the last run.
pub fn index_document(deps: &IndexDeps<'_>, doc: &DocumentRecord, force: bool) -> Result<IndexOutcome> {
    // Resident numbers are masked here when `privacy.pii_masking` is on (PIP-7).
    let pages = deps.db.with(|c| docs::list_pages_for_index(c, &doc.id))?;
    let hash = page_text_hash(&pages);
    if !force {
        if let Some(state) = deps.db.with(|c| repo::index_state(c, &doc.id))? {
            if state.indexed_at.is_some()
                && state.index_version == INDEX_VERSION
                && state.text_hash.as_deref() == Some(hash.as_str())
            {
                // Metadata-only updates (tags, title, role…) bumped `updated_at`; note the check
                // so the document stops showing up as pending.
                let id = doc.id.clone();
                deps.db.with(|c| repo::touch_indexed(c, &id))?;
                return Ok(IndexOutcome::Unchanged);
            }
        }
    }
    // NFC so that decomposed Hangul (macOS-made PDFs) matches composed queries in the
    // trigram / LIKE channels.
    let normalized: Vec<(u32, String)> = pages
        .iter()
        .filter_map(|p| {
            p.text.as_deref().filter(|t| !t.trim().is_empty()).map(|t| (p.page_no, t.nfc().collect::<String>()))
        })
        .collect();
    let texts: Vec<PageText<'_>> =
        normalized.iter().map(|(page_no, text)| PageText { page_no: *page_no, text: text.as_str() }).collect();
    if texts.is_empty() {
        let id = doc.id.clone();
        deps.db.with_tx(|tx| {
            repo::clear_chunks(tx, &id)?;
            tx.execute(
                "UPDATE documents SET indexed_at = ?2, index_version = ?3, text_hash = ?4, index_error = NULL WHERE id = ?1",
                rusqlite::params![id, chrono::Utc::now().to_rfc3339(), INDEX_VERSION as i64, hash],
            )?;
            Ok(())
        })?;
        return Ok(IndexOutcome::NoText);
    }
    let drafts = chunk_document(&texts, &deps.options);
    let tokens: Vec<String> =
        drafts.iter().map(|d| deps.analyzer.index_tokens(&indexed_text(d.heading_path.as_deref(), &d.text))).collect();
    let id = doc.id.clone();
    let n = deps.db.with_tx(|tx| repo::replace_chunks(tx, &id, &drafts, &tokens, &hash))?;
    Ok(IndexOutcome::Indexed(n))
}

/// Indexes every candidate document (or the given ids), isolating failures
/// per document and honouring `cancel` between documents.
pub fn index_pending(
    deps: &IndexDeps<'_>,
    ids: Option<&[String]>,
    force: bool,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(&IndexProgress<'_>),
) -> Result<IndexReport> {
    // A different analyzer / chunk size invalidates every chunk.
    let signature = index_signature(deps.analyzer, &deps.options);
    let stored = deps.db.with(|c| repo::kv_get(c, KV_INDEX_SIGNATURE))?;
    let signature_changed = stored.as_deref().is_some_and(|s| s != signature);
    let force = force || signature_changed;
    let ids = if signature_changed { None } else { ids };
    let candidates = deps.db.with(|c| repo::documents_to_index(c, ids, force))?;
    let total = candidates.len() as u32;
    let mut report = IndexReport::default();
    for (i, doc) in candidates.iter().enumerate() {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            report.cancelled = true;
            break;
        }
        progress(&IndexProgress { done: i as u32, total, current: &doc.original_name });
        match index_document(deps, doc, force) {
            Ok(IndexOutcome::Indexed(n)) => {
                report.indexed.push(doc.id.clone());
                report.chunks += n as u64;
            }
            Ok(IndexOutcome::Unchanged) => report.unchanged += 1,
            Ok(IndexOutcome::NoText) => report.no_text.push(doc.id.clone()),
            Err(e) => {
                let msg = e.to_string();
                let id = doc.id.clone();
                let _ = deps.db.with(|c| repo::set_index_error(c, &id, &msg));
                report.failed.push((doc.id.clone(), msg));
            }
        }
    }
    progress(&IndexProgress { done: total, total, current: "" });
    if !report.cancelled {
        deps.db.with(|c| repo::kv_set(c, KV_INDEX_SIGNATURE, &signature))?;
    }
    Ok(report)
}

/// Dashboard numbers plus tokenizer / embedding info.
pub fn index_status(deps: &IndexDeps<'_>) -> Result<IndexStatus> {
    let (mut status, model, dim) = deps.db.with_read(|c| {
        let status = repo::index_status(c)?;
        let model = repo::kv_get(c, embed::KV_EMBEDDING_MODEL)?;
        let dim = repo::kv_get(c, embed::KV_EMBEDDING_DIM)?.and_then(|d| d.parse().ok());
        Ok((status, model, dim))
    })?;
    status.tokenizer = deps.analyzer.info();
    status.embedding_model = model;
    status.embedding_dim = dim;
    Ok(status)
}
