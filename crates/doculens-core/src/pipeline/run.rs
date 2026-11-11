//! The pipeline runner (§3.4 flow): classification → OCR → extraction →
//! validation / confidence → review flags → DB + `extractions/` mirror, with
//! per-document failure isolation, cancellation between model calls and
//! progress callbacks for the job book.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde_json::{json, Value};

use crate::db::Db;
use crate::documents::import::{self, ImportDeps};
use crate::documents::{repo as docs, DocumentCache, DocumentRecord, PageRecord};
use crate::error::{CoreError, Result};
use crate::knowledge::answer::ChatBackend;
use crate::paths::WorkspacePaths;
use crate::settings::{ProcessingSettings, Profile};

use super::extract::{self, FieldOptions, PageInput};
use super::image::{self, PreparedImage};
use super::prompts::{Stage, CLASSIFY_IMAGE_TOKENS};
use super::repo::{self, NewField, NewRun, RunOutcome};
use super::schema::SchemaRecord;
use super::vlm::{Usage, Vlm};
use super::{
    classify, ocr, validate, DocTypeInfo, PipelineLevel, ProcessProgress, ProcessReport, ProcessRequest, SchemaChoice,
};

/// Pages classified per document (first N plus the last one; the rest inherit).
pub const CLASSIFY_MAX_PAGES: usize = 8;

/// Stage progress callback: `(stage, Some((page, total)))`.
pub type StageProgress<'a> = &'a mut dyn FnMut(&str, Option<(u32, u32)>);

pub struct PipelineDeps<'a> {
    pub import: ImportDeps<'a>,
    pub chat: &'a dyn ChatBackend,
    pub model: String,
    pub settings: ProcessingSettings,
    pub profile: Profile,
    pub pii_masking: bool,
    pub cancel: Option<&'a AtomicBool>,
    /// Context size of the loaded model instance (prompt budgeting); `None` = unknown.
    pub ctx: Option<u32>,
}

impl PipelineDeps<'_> {
    fn db(&self) -> &Db {
        self.import.db
    }
    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }
}

pub fn profile_name(p: Profile) -> &'static str {
    match p {
        Profile::Accuracy => "accuracy",
        Profile::Balanced => "balanced",
        Profile::Speed => "speed",
        Profile::FinePrint => "fine_print",
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct DocOutcome {
    classified: bool,
    ocr_pages: u32,
    extracted: bool,
    review_pending: bool,
}

/// Runs the request over its documents; failures are recorded per document. A missing engine is
/// not a document's failure: it ends the request with an error and the remaining documents stay
/// as they were.
pub fn process(
    deps: &PipelineDeps<'_>,
    req: &ProcessRequest,
    progress: &mut dyn FnMut(&ProcessProgress),
) -> Result<ProcessReport> {
    if req.document_ids.is_empty() {
        return Err(CoreError::Invalid("no documents".into()));
    }
    let db = deps.db();
    let documents = db.with(|c| repo::candidate_documents(c, &req.document_ids))?;
    let doc_types = db.with(repo::doc_types)?;
    let profile = req.profile.unwrap_or(deps.profile);
    let vlm = Vlm::new(deps.chat, deps.model.clone(), profile, deps.cancel).with_context(deps.ctx);
    let mut report = ProcessReport::default();
    for id in &req.document_ids {
        if !documents.iter().any(|d| &d.id == id) {
            report.skipped.push(id.clone());
        }
    }
    let total = documents.len() as u32;
    for (i, doc) in documents.iter().enumerate() {
        if deps.cancelled() {
            report.cancelled = true;
            break;
        }
        let name = doc.title.clone().unwrap_or_else(|| doc.original_name.clone());
        let mut stage_progress = |stage: &str, page: Option<(u32, u32)>| {
            progress(&ProcessProgress { done: i as u32, total, current: name.clone(), stage: stage.into(), page });
        };
        match process_document(deps, &vlm, &doc_types, req, doc, &mut stage_progress) {
            Ok(out) => {
                // A completed run clears the failure of an earlier attempt (status counts).
                let _ = db.with(|c| repo::set_pipeline_error(c, &doc.id, None));
                report.processed.push(doc.id.clone());
                report.classified += out.classified as u32;
                report.ocr_pages += out.ocr_pages;
                report.extracted += out.extracted as u32;
                report.review_pending += out.review_pending as u32;
            }
            Err(CoreError::Cancelled) => {
                report.cancelled = true;
                break;
            }
            Err(e) if super::vlm::engine_unavailable(&e) => {
                // The engine is gone — stopped, crashed, its port closed (audit PIP-6). Neither
                // this document nor the ones after it are at fault: each would "fail" within
                // seconds, and a batch of 300 that lost the engine at the 40th document ended
                // with 260 documents marked 처리 오류. They stay untouched and the request fails
                // as a whole; running it again goes on from here.
                log::warn!(
                    "pipeline: engine unavailable at document {} ({e}); {} document(s) left untouched",
                    doc.id,
                    documents.len() - i
                );
                return Err(if super::vlm::transport_failure(&e) { CoreError::EngineNotRunning } else { e });
            }
            Err(e) => {
                let msg = e.to_string();
                log::warn!("pipeline {}: {msg}", doc.id);
                let _ = db.with(|c| repo::set_pipeline_error(c, &doc.id, Some(&msg)));
                report.failed.push((doc.id.clone(), msg));
            }
        }
    }
    progress(&ProcessProgress { done: total, total, current: String::new(), stage: "done".into(), page: None });
    Ok(report)
}

/// Cached page image fitted to a token budget (`None` when the page has no render).
fn page_image(cache: &DocumentCache, page: &PageRecord, vlm: &Vlm<'_>, tokens: u32) -> Result<Option<PreparedImage>> {
    if page.width_px == 0 || page.height_px == 0 {
        return Ok(None);
    }
    let path = cache.page_file(page.page_no);
    if !path.is_file() {
        return Ok(None);
    }
    image::prepare(&path, vlm.geometry, tokens).map(Some)
}

fn has_text(p: &PageRecord) -> bool {
    p.text.as_deref().is_some_and(|t| !t.trim().is_empty())
}

#[allow(clippy::needless_range_loop)]
fn process_document(
    deps: &PipelineDeps<'_>,
    vlm: &Vlm<'_>,
    doc_types: &[DocTypeInfo],
    req: &ProcessRequest,
    doc: &DocumentRecord,
    progress: StageProgress<'_>,
) -> Result<DocOutcome> {
    let db = deps.db();
    let mut doc = doc.clone();
    let mut pages = db.with(|c| docs::list_pages(c, &doc.id, true))?;
    if pages.is_empty() {
        return Err(CoreError::Invalid("no pages (render the document first)".into()));
    }
    let cache = DocumentCache::for_document(deps.import.paths, &doc.id);
    let mut out = DocOutcome::default();
    let mut usage = Usage::default();

    // ---- 1. classification -------------------------------------------------
    if req.force || doc.classified_at.is_none() {
        progress("classify", None);
        let n = pages.len();
        let mut classes = Vec::new();
        for idx in 0..n {
            if idx >= CLASSIFY_MAX_PAGES && idx + 1 < n {
                continue;
            }
            if deps.cancelled() {
                return Err(CoreError::Cancelled);
            }
            progress("classify", Some((idx as u32 + 1, n as u32)));
            let img = page_image(&cache, &pages[idx], vlm, CLASSIFY_IMAGE_TOKENS)?;
            let text = pages[idx].text.clone();
            let (mut class, reply) =
                classify::classify_page(vlm, doc_types, img.as_ref().map(|i| i.url.as_str()), text.as_deref())?;
            usage.add(reply.usage);
            if class.rotation != 0 && img.is_some() && !class.is_blank {
                // olmOCR pattern (§3.4): rotate the render, then classify once more.
                let page_no = pages[idx].page_no;
                let new_rotation = (pages[idx].rotation + class.rotation) % 360;
                match import::rotate_page(&deps.import, &doc, page_no, new_rotation) {
                    Ok(np) => {
                        pages[idx].rotation = new_rotation;
                        pages[idx].width_px = np.width_px;
                        pages[idx].height_px = np.height_px;
                        pages[idx].rendered_at = Some(chrono::Utc::now());
                        let img2 = page_image(&cache, &pages[idx], vlm, CLASSIFY_IMAGE_TOKENS)?;
                        let (c2, r2) = classify::classify_page(
                            vlm,
                            doc_types,
                            img2.as_ref().map(|i| i.url.as_str()),
                            text.as_deref(),
                        )?;
                        usage.add(r2.usage);
                        class = super::PageClass { rotation: class.rotation, ..c2 };
                    }
                    Err(e) => log::warn!("rotation of {} p{page_no} failed: {e}", doc.id),
                }
            }
            // The render-time pixel heuristic decides blankness; the model can only
            // confirm it (a weak or garbage model must never blank out real pages).
            if class.is_blank && pages[idx].is_blank == Some(false) {
                class.is_blank = false;
            }
            db.with(|c| repo::set_page_class(c, &doc.id, pages[idx].page_no, &class))?;
            if class.is_blank {
                pages[idx].is_blank = Some(true);
            }
            classes.push((pages[idx].page_no, class));
        }
        let (doc_type, language) = classify::vote(&classes);
        db.with(|c| repo::set_classified(c, &doc.id, &doc_type, Some(&language), false))?;
        doc = db.with(|c| docs::get_document(c, &doc.id))?.ok_or_else(|| CoreError::NotFound(doc.id.clone()))?;
        out.classified = true;
    }
    if req.level == PipelineLevel::Classify {
        return Ok(out);
    }

    // ---- 2. OCR ------------------------------------------------------------
    progress("ocr", None);
    let n = pages.len();
    let ocr_tokens = vlm.params(Stage::Ocr).image_tokens.unwrap_or(1_500);
    for idx in 0..n {
        let page = &pages[idx];
        let wanted = req.pages.is_empty() || req.pages.contains(&page.page_no);
        if !wanted || page.is_blank == Some(true) || page.width_px == 0 {
            continue;
        }
        // OCR output is only replaced by a forced re-run. A manual correction is the user's own
        // text: force alone keeps it (the review screen's "재추출" used to wipe it), the request has
        // to ask for it with `reocr_manual`. A page the user emptied by hand has nothing to lose.
        let do_ocr = match page.text_source.as_deref() {
            Some("manual") => req.force && (req.reocr_manual || !has_text(page)),
            Some("ocr") => req.force,
            _ if has_text(page) => deps.settings.ocr_digital_pdf && (req.force || page.ocr_at.is_none()),
            _ => true,
        };
        if !do_ocr {
            continue;
        }
        if deps.cancelled() {
            return Err(CoreError::Cancelled);
        }
        progress("ocr", Some((idx as u32 + 1, n as u32)));
        let Some(img) = page_image(&cache, page, vlm, ocr_tokens)? else { continue };
        let (mut text, reply) = ocr::ocr_page(vlm, &img.url, None)?;
        usage.add(reply.usage);
        let mut profile_used = vlm.profile;
        if deps.settings.dense_reocr && vlm.profile != Profile::FinePrint {
            if let Some(t) = &text {
                if ocr::table_cells(t) >= deps.settings.dense_cells as usize {
                    // Dense page (§3.4 밀집 영역): a second pass at the fine-print budget.
                    let fine = Vlm::new(vlm.chat, vlm.model.clone(), Profile::FinePrint, vlm.cancel)
                        .with_context(vlm.ctx)
                        .with_geometry(vlm.geometry);
                    if let Some(img2) =
                        page_image(&cache, page, &fine, fine.params(Stage::Ocr).image_tokens.unwrap_or(3_072))?
                    {
                        match ocr::ocr_page(&fine, &img2.url, None) {
                            Ok((t2, r2)) => {
                                usage.add(r2.usage);
                                if t2.as_ref().map(|s| s.chars().count()).unwrap_or(0) > t.chars().count() {
                                    text = t2;
                                    profile_used = Profile::FinePrint;
                                }
                            }
                            Err(e) => log::warn!("fine-print pass failed: {e}"),
                        }
                    }
                }
            }
        }
        if deps.pii_masking {
            text = text.map(|t| validate::mask_text(&t));
        }
        db.with(|c| {
            repo::set_page_ocr(c, &doc.id, page.page_no, text.as_deref(), profile_name(profile_used), &vlm.model)
        })?;
        let page = &mut pages[idx];
        page.text = text;
        page.text_source = Some("ocr".into());
        page.ocr_at = Some(chrono::Utc::now());
        out.ocr_pages += 1;
    }
    if req.level == PipelineLevel::Ocr {
        return Ok(out);
    }

    // ---- 3. extraction -----------------------------------------------------
    if req.skip_extracted && doc.last_run_id.is_some() {
        // An automatic run never replaces an extraction that stands (its approvals and corrections
        // would be gone); only an explicit request extracts a document again.
        log::info!("pipeline {}: already extracted, automatic run leaves it alone", doc.id);
        return Ok(out);
    }
    if extraction_pages(&pages, deps.settings.kie_max_pages as usize, &req.pages).is_empty() {
        // Blank-only / unrendered documents have nothing to extract; not an error.
        log::info!("pipeline {}: no usable pages, extraction skipped", doc.id);
        return Ok(out);
    }
    progress("extract", None);
    let schema = resolve_schema(db, &doc, &req.schema)?;
    let run_id = crate::documents::new_id();
    let precise = req.precise.unwrap_or(deps.settings.precise_mode);
    db.with(|c| {
        repo::insert_run(
            c,
            &NewRun {
                id: &run_id,
                document_id: &doc.id,
                schema: Some(&schema),
                doc_type: doc.doc_type.as_deref(),
                model: &vlm.model,
                profile: profile_name(vlm.profile),
                precise,
            },
        )
    })?;
    let started = Instant::now();
    match extract_document(deps, vlm, &schema, &pages, &req.pages, precise, progress) {
        Ok(Extracted { merged, second, usage: run_usage, pages_done, incomplete }) => {
            usage.add(run_usage);
            let texts: BTreeMap<u32, String> =
                pages.iter().filter(|p| has_text(p)).map(|p| (p.page_no, p.text.clone().unwrap_or_default())).collect();
            let opts = FieldOptions {
                review_threshold: deps.settings.review_threshold as f64,
                audit_percent: deps.settings.audit_sample_percent,
                pii_masking: deps.pii_masking,
            };
            let mut built = extract::build_fields(&schema, &merged, second.as_ref(), &texts, opts, &doc.id);
            // A page that was cut off or failed leaves the result incomplete: it goes to review and
            // the run says so (the history panel shows the note), instead of the document failing.
            extract::flag_incomplete(&mut built, &incomplete);
            let partial_note = (!incomplete.is_empty())
                .then(|| incomplete.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join("; "));
            let review_pending = built.review_count > 0;
            let field_count = built.fields.len() as u32;
            // Nothing unmasked reaches the database or the mirror (§3.8).
            let merged = extract::mask_output(&schema.definition, &merged, deps.pii_masking);
            let second = second.map(|s| extract::mask_output(&schema.definition, &s, deps.pii_masking));
            let raw = if let Some(s) = &second { json!({ "primary": merged, "second": s }) } else { merged.clone() };
            let (rid, did, sid) = (run_id.clone(), doc.id.clone(), schema.id.clone());
            db.with_tx(|tx| {
                repo::insert_fields(tx, &rid, &did, &built.fields)?;
                repo::finish_run(
                    tx,
                    &rid,
                    &RunOutcome {
                        status: "done",
                        error: partial_note.as_deref(),
                        duration_ms: started.elapsed().as_millis() as u64,
                        prompt_tokens: usage.prompt,
                        completion_tokens: usage.completion,
                        pages_done,
                        result_json: Some(&raw),
                        confidence: built.min_confidence,
                        field_count,
                        review_count: built.review_count,
                    },
                )?;
                repo::set_extracted(
                    tx,
                    &did,
                    &rid,
                    built.min_confidence,
                    review_pending,
                    built.vendor.as_deref(),
                    built.doc_date.as_deref(),
                    built.total_amount,
                )?;
                repo::bump_schema_use(tx, &sid)
            })?;
            if let Err(e) =
                write_mirror(deps.import.paths, &doc, &run_id, &schema, &vlm.model, &built.fields, &pages, &merged)
            {
                log::warn!("extraction mirror for {} not written: {e}", doc.id);
            }
            out.extracted = true;
            out.review_pending = review_pending;
            Ok(out)
        }
        Err(e) => {
            let status = if matches!(e, CoreError::Cancelled) { "cancelled" } else { "failed" };
            let msg = e.to_string();
            let _ = db.with(|c| {
                repo::finish_run(
                    c,
                    &run_id,
                    &RunOutcome {
                        status,
                        error: Some(&msg),
                        duration_ms: started.elapsed().as_millis() as u64,
                        prompt_tokens: usage.prompt,
                        completion_tokens: usage.completion,
                        pages_done: 0,
                        result_json: None,
                        confidence: None,
                        field_count: 0,
                        review_count: 0,
                    },
                )
            });
            Err(e)
        }
    }
}

/// Schema for a document: explicit choice → document override → type mapping → `general`.
pub fn resolve_schema(db: &Db, doc: &DocumentRecord, choice: &SchemaChoice) -> Result<SchemaRecord> {
    let choice = choice.clone();
    let doc = doc.clone();
    db.with(move |c| {
        if let SchemaChoice::Id { id } = &choice {
            return repo::get_schema(c, id)?.ok_or_else(|| repo::typed("schema_not_found"));
        }
        if let Some(id) = &doc.schema_id {
            if let Some(s) = repo::get_schema(c, id)? {
                return Ok(s);
            }
        }
        if let Some(dt) = doc.doc_type.as_deref().filter(|t| *t != "other") {
            if let Some(s) = repo::schema_for_doc_type(c, dt)? {
                return Ok(s);
            }
        }
        repo::schema_for_doc_type(c, "general")?.ok_or_else(|| repo::typed("schema_not_found"))
    })
}

/// Pages the extractor sees: non-blank pages with a render or text (restricted to
/// `only` when given), the first `kie_max_pages − 1` plus the last one; a budget of
/// one page means the first page.
pub(crate) fn extraction_pages<'a>(pages: &'a [PageRecord], max: usize, only: &[u32]) -> Vec<&'a PageRecord> {
    let usable: Vec<&PageRecord> = pages
        .iter()
        .filter(|p| {
            (only.is_empty() || only.contains(&p.page_no))
                && p.is_blank != Some(true)
                && (p.width_px > 0 || has_text(p))
        })
        .collect();
    let max = max.max(1);
    if usable.len() <= max {
        return usable;
    }
    if max == 1 {
        return vec![usable[0]];
    }
    let mut chosen: Vec<&PageRecord> = usable[..max - 1].to_vec();
    chosen.push(usable[usable.len() - 1]);
    chosen
}

/// The merged model output of a document's pages.
struct Extracted {
    merged: Value,
    /// Precision-mode second sample.
    second: Option<Value>,
    usage: Usage,
    /// Pages that produced a result.
    pages_done: u32,
    /// Pages whose result is known to be incomplete — the reply was cut off, or the page failed
    /// while others answered — with the note shown to the reviewer.
    incomplete: Vec<(u32, String)>,
}

/// An error that ends the whole run: nothing about one page or one document is wrong.
fn ends_the_run(e: &CoreError) -> bool {
    matches!(e, CoreError::Cancelled) || super::vlm::engine_unavailable(e)
}

fn extract_document(
    deps: &PipelineDeps<'_>,
    vlm: &Vlm<'_>,
    schema: &SchemaRecord,
    pages: &[PageRecord],
    only: &[u32],
    precise: bool,
    progress: StageProgress<'_>,
) -> Result<Extracted> {
    let cache = DocumentCache::for_document(deps.import.paths, &pages[0].document_id);
    let chosen = extraction_pages(pages, deps.settings.kie_max_pages as usize, only);
    if chosen.is_empty() {
        return Err(CoreError::Invalid("no usable pages (blank or unrendered)".into()));
    }
    let tokens = vlm.params(Stage::Kie).image_tokens.unwrap_or(1_200);
    let mut usage = Usage::default();
    let mut primary: Vec<(u32, Value)> = Vec::new();
    let mut second: Vec<(u32, Value)> = Vec::new();
    let mut incomplete: Vec<(u32, String)> = Vec::new();
    let mut first_failure: Option<CoreError> = None;
    let total = chosen.len() as u32;
    for (i, page) in chosen.iter().enumerate() {
        if deps.cancelled() {
            return Err(CoreError::Cancelled);
        }
        progress("extract", Some((i as u32 + 1, total)));
        let img = page_image(&cache, page, vlm, tokens)?;
        let input = PageInput {
            page_no: page.page_no,
            page_count: pages.len() as u32,
            image_url: img.as_ref().map(|i| i.url.as_str()),
            text: page.text.as_deref(),
        };
        // One page must not take the document down (audit PIP-8): a page that cannot be extracted
        // is noted and the pages that answer are used.
        let page_no = page.page_no;
        match extract::extract_page(vlm, schema, &input, None) {
            Ok(x) => {
                usage.add(x.reply.usage);
                if x.partial {
                    incomplete.push((
                        page_no,
                        format!(
                            "{page_no}쪽: 추출 응답이 길이 제한으로 잘려 일부만 추출됨 / page {page_no}: reply cut off, partial result"
                        ),
                    ));
                }
                primary.push((page_no, x.value));
            }
            Err(e) if ends_the_run(&e) => return Err(e),
            Err(e) => {
                log::warn!("kie page {page_no} failed: {e}");
                incomplete.push((
                    page_no,
                    format!("{page_no}쪽: 추출 실패 — 이 쪽의 값이 빠져 있음 / page {page_no} failed: {e}"),
                ));
                first_failure.get_or_insert(e);
                continue;
            }
        }
        if precise {
            // The second sample only feeds the agreement signal: without it the page's fields
            // simply have none.
            match extract::extract_page(vlm, schema, &input, Some(0.3)) {
                Ok(x2) => {
                    usage.add(x2.reply.usage);
                    second.push((page_no, x2.value));
                }
                Err(e) if ends_the_run(&e) => return Err(e),
                Err(e) => log::warn!("kie page {page_no}: precision sample failed: {e}"),
            }
        }
    }
    if primary.is_empty() {
        // No page answered: the document fails with the first page's error, as before.
        return Err(first_failure.unwrap_or_else(|| CoreError::engine("no page could be extracted")));
    }
    let merged = extract::merge_pages(&schema.definition, &primary);
    let second = precise.then(|| extract::merge_pages(&schema.definition, &second));
    Ok(Extracted { merged, second, usage, pages_done: primary.len() as u32, incomplete })
}

/// Human-readable mirror: `extractions/<doc>/<run>/{result.json, pNNNN.md}` + `LATEST`.
#[allow(clippy::too_many_arguments)]
pub fn write_mirror(
    paths: &WorkspacePaths,
    doc: &DocumentRecord,
    run_id: &str,
    schema: &SchemaRecord,
    model: &str,
    fields: &[NewField],
    pages: &[PageRecord],
    raw: &Value,
) -> Result<PathBuf> {
    let dir = paths.extractions_dir().join(&doc.id).join(run_id);
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    let field_rows: Vec<Value> = fields
        .iter()
        .map(|f| {
            json!({
                "path": f.path, "label_ko": f.label_ko, "label_en": f.label_en, "kind": f.kind.as_str(),
                "value": f.value, "normalized": f.normalized, "page": f.page, "evidence": f.evidence,
                "confidence": f.confidence, "model_confidence": f.model_confidence,
                "review_reason": f.review_reason.map(|r| r.as_str()), "validation": f.validation,
            })
        })
        .collect();
    let result = json!({
        "document": { "id": doc.id, "name": doc.original_name, "rel_path": doc.rel_path, "doc_type": doc.doc_type },
        "run": { "id": run_id, "model": model, "prompt_version": super::PROMPT_VERSION, "extracted_at": chrono::Utc::now().to_rfc3339() },
        "schema": { "id": schema.id, "name": schema.name, "doc_type": schema.doc_type, "version": schema.version },
        "fields": field_rows,
        "raw": raw,
    });
    let result_path = dir.join("result.json");
    std::fs::write(&result_path, serde_json::to_vec_pretty(&result).unwrap_or_default())
        .map_err(|e| CoreError::io(&result_path, e))?;
    for p in pages.iter().filter(|p| has_text(p)) {
        let md = dir.join(format!("p{:04}.md", p.page_no));
        std::fs::write(&md, p.text.as_deref().unwrap_or("")).map_err(|e| CoreError::io(&md, e))?;
    }
    let latest = paths.extractions_dir().join(&doc.id).join("LATEST");
    std::fs::write(&latest, run_id).map_err(|e| CoreError::io(&latest, e))?;
    Ok(dir)
}

/// Deletes the mirror of a document (called from document deletion).
pub fn remove_mirror(paths: &WorkspacePaths, doc_id: &str) {
    if !crate::documents::is_safe_id(doc_id) {
        return;
    }
    let dir = paths.extractions_dir().join(doc_id);
    if dir.exists() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Tables of every page with text (viewer "표" tab).
pub fn document_tables(db: &Db, doc_id: &str) -> Result<Vec<super::ExtractedTable>> {
    let pages = db.with(|c| docs::list_pages(c, doc_id, true))?;
    let mut out = Vec::new();
    for p in pages.iter().filter(|p| has_text(p)) {
        out.extend(ocr::parse_tables(p.text.as_deref().unwrap_or(""), p.page_no));
    }
    Ok(out)
}
