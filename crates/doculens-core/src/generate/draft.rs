//! Template resolution, source gathering, the model call and the rule-based
//! 보완요청서 path.
//!
//! `resolve_template`: `form_ref` → registered schema (id / name / doc-type
//! slug; user forms before built-in pipeline schemas) → built-in template by
//! name / keywords → keywords of `format_description` and `content` → 보고서.
//!
//! `draft`: numbered sources (latest extracted fields + page text of library
//! documents, aggregate results, pasted text, knowledge-base hits for explicit
//! queries and — with `use_knowledge` — for the content itself, minus the
//! excluded documents) → `prompts` → `Vlm::call_json(Stage::NlSchema, …)`
//! under the grammar of `schema` (one retry with a reminder when the reply is
//! not JSON) → `Draft` with citations resolved from `source_index` →
//! `validate`. Without a model only the 보완요청서 can be composed, by rules
//! from the request's text sources (`source = "rules"`).

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use chrono::{Duration, Local};
use serde_json::{json, Value};

use crate::db::Db;
use crate::documents::repo as docs;
use crate::engine::client::user_message;
use crate::error::{CoreError, Result};
use crate::knowledge::search::{search, SearchDeps};
use crate::knowledge::{SearchMode, SearchRequest, SearchScope};
use crate::pipeline::forms::slug_for_name;
use crate::pipeline::prompts::{truncate, Stage};
use crate::pipeline::repo as prepo;
use crate::pipeline::schema::{SchemaRecord, SchemaSource};
use crate::pipeline::validate::{normalize_date, value_text};
use crate::pipeline::vlm::{Reply, Vlm};
use crate::settings::Language;

use super::prompts::{self, SourceBlock};
use super::schema;
use super::templates::{self, compact, TemplateSpec};
use super::{
    sections_of, Draft, DraftBlock, DraftCitation, DraftField, DraftOutcome, DraftRequest, DraftTable, GenerateDeps,
    ReviseScope, SourceRef, TemplateKind, TemplateRef,
};

/// Upper bound on numbered sources shown to the model.
pub const MAX_SOURCES: usize = 12;
/// Knowledge hits taken for an explicit `SourceRef::Knowledge` / for the content query.
const KNOWLEDGE_HITS_EXPLICIT: usize = 6;
const KNOWLEDGE_HITS_CONTENT: usize = 4;
/// Page text added for a document source (after its fields).
const PAGE_TEXT_CHARS: usize = 800;
const EXCERPT_CHARS: usize = 160;
/// Default 보완 기한 when the request names none.
const DEFAULT_DUE_DAYS: i64 = 14;

fn tr(ko: bool, k: &str, e: &str) -> String {
    if ko {
        k.to_string()
    } else {
        e.to_string()
    }
}

/// A numbered source: what the model sees plus the citation attached to values taken from it.
pub struct Source {
    pub citation: DraftCitation,
    pub text: String,
    /// Raw title of a `SourceRef::Text` (`project`, `finding:<verdict>:<rule_id>` from the
    /// projects module, or the user's caption); empty for other kinds.
    pub tag: String,
}

// ---------------------------------------------------------------------------
// Template resolution
// ---------------------------------------------------------------------------

fn schema_matches(s: &SchemaRecord, r: &str, rc: &str) -> bool {
    s.id == r
        || compact(&s.name) == rc
        || s.name_en.as_deref().is_some_and(|n| compact(n) == rc)
        || s.doc_type == r
        || compact(&s.doc_type) == rc
}

/// Registered schema named by `r` (id, name, English name or doc-type slug). User
/// forms win over natural-language / user schemas; built-in pipeline schemas only
/// match when `include_builtin` is set (a generic template is usually better).
fn find_schema(db: &Db, r: &str, include_builtin: bool) -> Result<Option<SchemaRecord>> {
    let all = db.with(prepo::list_schemas)?;
    let rc = compact(r);
    if rc.is_empty() {
        return Ok(None);
    }
    let mut candidates: Vec<&SchemaRecord> = all
        .iter()
        .filter(|s| schema_matches(s, r, &rc))
        .filter(|s| include_builtin || s.source != SchemaSource::Builtin)
        .collect();
    candidates.sort_by_key(|s| {
        (s.source != SchemaSource::Form, !s.enabled, s.source == SchemaSource::Builtin, !s.is_default)
    });
    Ok(candidates.first().map(|s| (*s).clone()))
}

/// The template the request resolves to, with its fields.
pub fn resolve_spec(db: &Db, req: &DraftRequest) -> Result<TemplateSpec> {
    if let Some(r) = req.form_ref.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(k) = TemplateKind::parse(r) {
            return Ok(TemplateSpec::builtin(k));
        }
        if let Some(rec) = find_schema(db, r, false)? {
            return Ok(TemplateSpec::from_schema(&rec));
        }
        if let Some(k) = templates::match_keywords_any(r) {
            return Ok(TemplateSpec::builtin(k));
        }
        if let Some(rec) = find_schema(db, r, true)? {
            return Ok(TemplateSpec::from_schema(&rec));
        }
        if let Some(k) = TemplateKind::parse(&slug_for_name(r)) {
            return Ok(TemplateSpec::builtin(k));
        }
    }
    // A regeneration without an explicit form keeps the previous draft's template.
    if req.form_ref.is_none() {
        if let Some(prev) = req.previous.as_deref() {
            match &prev.template {
                TemplateRef::Builtin { template } => return Ok(TemplateSpec::builtin(*template)),
                TemplateRef::Form { schema_id, .. } => {
                    if let Some(rec) = db.with(|c| prepo::get_schema(c, schema_id))? {
                        return Ok(TemplateSpec::from_schema(&rec));
                    }
                }
            }
        }
    }
    for text in [req.format_description.as_deref().unwrap_or(""), req.content.as_str()] {
        if let Some(k) = templates::match_keywords(text) {
            return Ok(TemplateSpec::builtin(k));
        }
    }
    Ok(TemplateSpec::builtin(TemplateKind::Report))
}

pub fn resolve_template(db: &Db, req: &DraftRequest) -> Result<TemplateRef> {
    Ok(resolve_spec(db, req)?.template)
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

fn document_source(db: &Db, doc_id: &str, ko: bool) -> Result<Option<Source>> {
    let Some(doc) = db.with(|c| docs::get_document(c, doc_id))? else { return Ok(None) };
    let fields = db.with(|c| prepo::latest_fields(c, doc_id))?;
    let title = doc.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| doc.original_name.clone());
    let mut text = String::new();
    if let Some(dt) = &doc.doc_type {
        text.push_str(&format!("doc_type: {dt}\n"));
    }
    if let Some(d) = &doc.doc_date {
        text.push_str(&format!("doc_date: {d}\n"));
    }
    let mut page: Option<u32> = None;
    let mut rows: BTreeMap<(String, u32), Vec<String>> = BTreeMap::new();
    let mut scalar_lines = 0usize;
    for f in &fields {
        let Some(v) = f.effective_value().map(str::trim).filter(|v| !v.is_empty()) else { continue };
        if page.is_none() {
            page = f.page;
        }
        match f.row {
            Some(r) => {
                let table = f.path.split('[').next().unwrap_or("").to_string();
                rows.entry((table, r)).or_default().push(format!("{}={}", f.key, v));
            }
            None => {
                let label = if ko { &f.label_ko } else { &f.label_en };
                let label = if label.is_empty() { &f.label_ko } else { label };
                text.push_str(&format!("{} ({label}): {v}\n", f.path));
                scalar_lines += 1;
            }
        }
    }
    for ((table, r), cells) in rows {
        text.push_str(&format!("{table}[{r}]: {}\n", cells.join(" | ")));
    }
    // Page text (first page) when there are no / few extracted fields.
    if scalar_lines < 3 {
        let pages = db.with(|c| docs::list_pages(c, doc_id, true))?;
        if let Some(t) = pages.iter().find_map(|p| p.text.as_deref()).map(str::trim).filter(|t| !t.is_empty()) {
            text.push_str(if ko { "[본문]\n" } else { "[text]\n" });
            text.push_str(&truncate(t, PAGE_TEXT_CHARS));
            text.push('\n');
            if page.is_none() {
                page = Some(1);
            }
        }
    }
    if text.trim().is_empty() {
        text.push_str(if ko { "(추출된 내용 없음)" } else { "(nothing extracted)" });
    }
    Ok(Some(Source {
        citation: DraftCitation {
            doc_id: Some(doc.id.clone()),
            title,
            page,
            chunk_id: None,
            source: "document".into(),
            excerpt: Some(truncate(text.trim(), EXCERPT_CHARS)),
        },
        text,
        tag: String::new(),
    }))
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn aggregate_source(result: &Value, ko: bool) -> Source {
    let s = |k: &str| result.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut text = String::new();
    let func = s("func");
    let field = result
        .get("field_label")
        .and_then(Value::as_str)
        .or_else(|| result.get("field").and_then(Value::as_str))
        .unwrap_or("");
    text.push_str(&format!("{}: {func} {field}\n", if ko { "집계" } else { "aggregate" }));
    let doc_type = s("doc_type");
    if !doc_type.is_empty() {
        text.push_str(&format!("doc_type: {doc_type}\n"));
    }
    let (from, to) = (s("date_from"), s("date_to"));
    if !from.is_empty() || !to.is_empty() {
        text.push_str(&format!("{}: {from} ~ {to}\n", if ko { "기간" } else { "period" }));
    }
    if let Some(v) = result.get("value").and_then(num) {
        text.push_str(&format!("{}: {}\n", if ko { "값" } else { "value" }, super::validate::format_won(v)));
    }
    if let Some(c) = result.get("count").and_then(Value::as_u64) {
        text.push_str(&format!("{}: {c}\n", if ko { "문서 수" } else { "documents" }));
    }
    if let Some(m) = result.get("missing").and_then(Value::as_u64).filter(|m| *m > 0) {
        text.push_str(&format!("{}: {m}\n", if ko { "값 없는 문서" } else { "without a value" }));
    }
    if let Some(groups) = result.get("groups").and_then(Value::as_array) {
        for g in groups.iter().take(12) {
            let key = g.get("key").and_then(Value::as_str).unwrap_or("");
            let v = g.get("value").and_then(num).map(super::validate::format_won).unwrap_or_default();
            let c = g.get("count").and_then(Value::as_u64).unwrap_or(0);
            text.push_str(&format!("- {key}: {v} ({c})\n"));
        }
    }
    if let Some(docs) = result.get("documents").and_then(Value::as_array) {
        for d in docs.iter().take(10) {
            let name = d.get("name").and_then(Value::as_str).unwrap_or("");
            let date = d.get("date").and_then(Value::as_str).unwrap_or("");
            let v = d.get("value").and_then(num).map(super::validate::format_won).unwrap_or_default();
            let vendor = d.get("vendor").and_then(Value::as_str).unwrap_or("");
            text.push_str(&format!("  · {date} {name} {vendor} {v}\n"));
        }
    }
    Source {
        citation: DraftCitation {
            doc_id: None,
            title: tr(ko, "집계 결과", "Aggregate result"),
            page: None,
            chunk_id: None,
            source: "aggregate".into(),
            excerpt: Some(truncate(text.trim(), EXCERPT_CHARS)),
        },
        text,
        tag: String::new(),
    }
}

/// Pretty title of a project-module text source (`finding:…` carries JSON with `doc_name`).
fn text_title(raw: &str, text: &str, ko: bool) -> String {
    if raw == "project" {
        return tr(ko, "프로젝트 정보", "Project");
    }
    if raw.starts_with("finding:") {
        let doc = serde_json::from_str::<Value>(text.trim())
            .ok()
            .and_then(|v| v.get("doc_name").and_then(Value::as_str).map(str::trim).map(str::to_string))
            .filter(|d| !d.is_empty());
        return doc.unwrap_or_else(|| tr(ko, "정합성 검사 결과", "Consistency check"));
    }
    raw.to_string()
}

fn text_source(title: Option<&str>, text: &str, ko: bool) -> Source {
    let raw = title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or("");
    let title = if raw.is_empty() { tr(ko, "입력 텍스트", "Pasted text") } else { text_title(raw, text, ko) };
    Source {
        citation: DraftCitation {
            doc_id: None,
            title,
            page: None,
            chunk_id: None,
            source: "text".into(),
            excerpt: Some(truncate(text.trim(), EXCERPT_CHARS)),
        },
        text: text.to_string(),
        tag: raw.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
/// Where a draft's knowledge searches look: the request's project (plan A) or everything.
fn knowledge_scope(req: &DraftRequest) -> SearchScope {
    match req.project_id.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => SearchScope::project(p),
        None => SearchScope::default(),
    }
}

/// One knowledge lookup for a draft: where to look, what for, how many hits, and whether the
/// user asked for it explicitly (`SourceRef::Knowledge`) or it backs the content.
struct KnowledgeLookup<'a> {
    scope: &'a SearchScope,
    query: &'a str,
    limit: usize,
    explicit: bool,
}

fn knowledge_sources(
    deps: &GenerateDeps<'_>,
    lookup: KnowledgeLookup<'_>,
    excluded: &HashSet<&str>,
    seen: &mut HashSet<i64>,
    out: &mut Vec<Source>,
    notes: &mut Vec<String>,
) -> Result<()> {
    let KnowledgeLookup { scope, query, limit, explicit } = lookup;
    let ko = deps.ko();
    let Some(kdeps): Option<&SearchDeps<'_>> = deps.knowledge.as_ref() else {
        let n = tr(
            ko,
            "지식 베이스를 사용할 수 없어 근거 검색을 건너뛰었습니다.",
            "Knowledge base unavailable; evidence search skipped.",
        );
        if !notes.contains(&n) {
            notes.push(n);
        }
        return Ok(());
    };
    let q = query.trim();
    if q.is_empty() {
        return Ok(());
    }
    let req = SearchRequest {
        query: q.to_string(),
        mode: SearchMode::Hybrid,
        scope: scope.clone(),
        limit: (limit * 3) as u32,
        expand_glossary: true,
        per_document: 2,
    };
    let resp = search(kdeps, &req)?;
    let mut taken = 0usize;
    for h in resp.hits {
        if taken >= limit {
            break;
        }
        if excluded.contains(h.document_id.as_str()) || !seen.insert(h.chunk_id) {
            continue;
        }
        let mut text = String::new();
        if let Some(hp) = &h.heading_path {
            text.push_str(&format!("[{hp}] "));
        }
        if let Some(c) = &h.clause {
            text.push_str(&format!("({c}) "));
        }
        text.push_str(h.text.trim());
        out.push(Source {
            citation: DraftCitation {
                doc_id: Some(h.document_id.clone()),
                title: h.title.clone(),
                page: Some(h.page_start),
                chunk_id: Some(h.chunk_id),
                source: "knowledge".into(),
                excerpt: Some(truncate(h.snippet.trim(), EXCERPT_CHARS)),
            },
            text,
            tag: String::new(),
        });
        taken += 1;
    }
    if taken == 0 && explicit {
        notes.push(if ko {
            format!("'{}'에 대한 지식 베이스 근거를 찾지 못했습니다.", truncate(q, 40))
        } else {
            format!("No knowledge-base evidence found for '{}'.", truncate(q, 40))
        });
    }
    Ok(())
}

/// Collects the numbered sources of a request (documents, aggregates, text, knowledge).
pub fn gather_sources(deps: &GenerateDeps<'_>, req: &DraftRequest, notes: &mut Vec<String>) -> Result<Vec<Source>> {
    let ko = deps.ko();
    let scope = knowledge_scope(req);
    let excluded: HashSet<&str> = req.excluded_sources.iter().map(String::as_str).collect();
    let mut seen: HashSet<i64> = HashSet::new();
    let mut out: Vec<Source> = Vec::new();
    for s in &req.sources {
        if deps.cancelled() {
            return Err(CoreError::Cancelled);
        }
        match s {
            SourceRef::Document { doc_id } => match document_source(deps.db, doc_id, ko)? {
                Some(src) => out.push(src),
                None => notes.push(if ko {
                    format!("문서 {}을(를) 찾을 수 없습니다.", truncate(doc_id, 12))
                } else {
                    format!("Document {} was not found.", truncate(doc_id, 12))
                }),
            },
            SourceRef::Aggregate { result } => out.push(aggregate_source(result, ko)),
            SourceRef::Text { title, text } => {
                if !text.trim().is_empty() {
                    out.push(text_source(title.as_deref(), text, ko));
                }
            }
            SourceRef::Knowledge { query } => {
                let lookup = KnowledgeLookup { scope: &scope, query, limit: KNOWLEDGE_HITS_EXPLICIT, explicit: true };
                knowledge_sources(deps, lookup, &excluded, &mut seen, &mut out, notes)?
            }
        }
    }
    if req.use_knowledge && !req.content.trim().is_empty() {
        let q = truncate(req.content.trim(), 300);
        let lookup = KnowledgeLookup { scope: &scope, query: &q, limit: KNOWLEDGE_HITS_CONTENT, explicit: false };
        knowledge_sources(deps, lookup, &excluded, &mut seen, &mut out, notes)?;
    }
    out.truncate(MAX_SOURCES);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Reply → Draft
// ---------------------------------------------------------------------------

fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Null => String::new(),
        other => value_text(other).unwrap_or_default(),
    }
}

fn parse_reply(v: &Value, spec: &TemplateSpec, req: &DraftRequest, sources: &[Source]) -> Draft {
    let ko = req.language == Language::Ko;
    let cite = |idx: &Value| -> Option<DraftCitation> {
        let i = idx.as_u64()?;
        if i == 0 {
            return None;
        }
        sources.get(i as usize - 1).map(|s| s.citation.clone())
    };
    let prev = req.previous.as_deref();
    let mut d = Draft::default();
    if let Some(p) = prev {
        d.id = p.id.clone();
        d.created_at = p.created_at;
    }
    let title = str_of(&v["title"]);
    d.title = if title.is_empty() { spec.name(ko).to_string() } else { title };

    let reply_fields: Vec<(String, String, Option<DraftCitation>)> = v["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let key = f.get("key").and_then(Value::as_str)?.trim().to_string();
            Some((
                key,
                str_of(f.get("value").unwrap_or(&Value::Null)),
                cite(f.get("source_index").unwrap_or(&Value::Null)),
            ))
        })
        .collect();
    for f in spec.scalars() {
        let found = reply_fields.iter().find(|(k, _, _)| *k == f.key);
        let mut field = DraftField {
            key: f.key.clone(),
            label: f.label(ko).to_string(),
            kind: f.kind.clone(),
            required: f.required,
            value: found.map(|(_, v, _)| v.clone()).unwrap_or_default(),
            normalized: None,
            source: found.and_then(|(_, _, c)| c.clone()),
            user_edited: false,
        };
        if let Some(p) = prev.and_then(|p| p.field(&f.key)).filter(|p| p.user_edited) {
            field.value = p.value.clone();
            field.source = p.source.clone();
            field.normalized = p.normalized.clone();
            field.user_edited = true;
        }
        d.fields.push(field);
    }
    // Keys outside the template (impossible under the grammar, tolerated for hand-made replies).
    for (k, value, c) in &reply_fields {
        if spec.field(k).is_none() && !value.is_empty() && d.field(k).is_none() {
            d.fields.push(DraftField {
                key: k.clone(),
                label: k.clone(),
                kind: "string".into(),
                required: false,
                value: value.clone(),
                normalized: None,
                source: c.clone(),
                user_edited: false,
            });
        }
    }
    for t in spec.tables() {
        let rows: Vec<Vec<String>> = v["tables"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|x| x.get("key").and_then(Value::as_str) == Some(t.key.as_str()))
            .flat_map(|x| x.get("rows").and_then(Value::as_array).cloned().unwrap_or_default())
            .take(schema::MAX_TABLE_ROWS)
            .map(|row| {
                let mut cells: Vec<String> = row.as_array().map(|r| r.iter().map(str_of).collect()).unwrap_or_default();
                cells.resize(t.columns.len(), String::new());
                cells
            })
            .filter(|row| row.iter().any(|c| !c.is_empty()))
            .collect();
        d.tables.push(DraftTable {
            key: t.key.clone(),
            label: t.label(ko).to_string(),
            columns: t.columns.clone(),
            rows,
        });
    }
    let refs: Vec<&Source> = sources.iter().collect();
    d.body = parse_body(&v["body"], &refs);
    d.notes = v["notes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(str_of)
        .filter(|s| !s.is_empty())
        .take(schema::MAX_NOTES)
        .collect();
    d.references = references(sources);
    finish_draft(&mut d, spec, req);
    d
}

/// Body blocks of a reply (`body` of the one-shot draft, `blocks` of a section call). `sources` are
/// numbered from 1 in `source_index`; headings come out at level 2.
fn parse_body(v: &Value, sources: &[&Source]) -> Vec<DraftBlock> {
    let cite = |idx: &Value| -> Option<DraftCitation> {
        let i = idx.as_u64()?;
        if i == 0 {
            return None;
        }
        sources.get(i as usize - 1).map(|s| s.citation.clone())
    };
    let mut body = Vec::new();
    for b in v.as_array().into_iter().flatten().take(schema::MAX_BODY_BLOCKS) {
        let kind = b.get("kind").and_then(Value::as_str).unwrap_or("paragraph");
        let text = str_of(b.get("text").unwrap_or(&Value::Null));
        let items: Vec<String> = b
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(str_of)
            .filter(|s| !s.is_empty())
            .collect();
        match kind {
            "heading" => {
                if !text.is_empty() {
                    body.push(DraftBlock::Heading { level: 2, text });
                }
            }
            "list" => {
                let items = if items.is_empty() {
                    text.lines()
                        .map(|l| l.trim().trim_start_matches(['-', '•', '*']).trim().to_string())
                        .filter(|l| !l.is_empty())
                        .collect()
                } else {
                    items
                };
                if !items.is_empty() {
                    body.push(DraftBlock::List { items, ordered: false });
                }
            }
            _ => {
                if !text.is_empty() {
                    body.push(DraftBlock::Paragraph {
                        text,
                        source: cite(b.get("source_index").unwrap_or(&Value::Null)),
                    });
                }
            }
        }
    }
    body
}

/// Distinct citations of the offered sources (the "참고 문서" panel).
fn references(sources: &[Source]) -> Vec<DraftCitation> {
    let mut out: Vec<DraftCitation> = Vec::new();
    for s in sources {
        let c = &s.citation;
        if !out
            .iter()
            .any(|o| o.doc_id == c.doc_id && o.chunk_id == c.chunk_id && o.title == c.title && o.page == c.page)
        {
            out.push(c.clone());
        }
    }
    out
}

fn finish_draft(d: &mut Draft, spec: &TemplateSpec, req: &DraftRequest) {
    d.template = spec.template.clone();
    d.doc_type = Some(spec.doc_type.clone());
    d.language = req.language;
    d.tone = req.tone;
    d.length = req.length;
    // Keep the request for regeneration without nesting the whole previous draft inside it (nor the
    // 0.7.0 revision scope, which belongs to one call).
    let mut stored = req.clone();
    stored.previous = None;
    stored.revise = None;
    d.request = Some(Box::new(stored));
}

// ---------------------------------------------------------------------------
// Rule-based 보완요청서
// ---------------------------------------------------------------------------

fn strip_bullet(line: &str) -> &str {
    let t = line.trim();
    let t = t.trim_start_matches(['-', '•', '*', '·', '▪', '○', '●']).trim_start();
    // "1." / "1)" / "①"
    let t = match t.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ')')) {
        Some(i) if i > 0 && t[..i].chars().any(|c| c.is_ascii_digit()) => t[i..].trim_start(),
        _ => t,
    };
    t.trim_start_matches(['①', '②', '③', '④', '⑤', '⑥', '⑦', '⑧', '⑨', '⑩']).trim_start()
}

/// Project card the projects module sends as `SourceRef::Text { title: "project" }`.
#[derive(Default)]
struct ProjectInfo {
    name: String,
    intake_no: String,
    owner: String,
    due_date: Option<String>,
}

fn json_str(obj: &serde_json::Map<String, Value>, key: &str) -> String {
    match obj.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.trim().to_string(),
        Some(other) => value_text(other).unwrap_or_default(),
    }
}

fn project_info(sources: &[Source]) -> Option<ProjectInfo> {
    let s = sources.iter().find(|s| s.citation.source == "text" && s.tag == "project")?;
    let v: Value = serde_json::from_str(s.text.trim()).ok()?;
    let obj = v.as_object()?;
    Some(ProjectInfo {
        name: json_str(obj, "name"),
        intake_no: json_str(obj, "intake_no"),
        owner: json_str(obj, "owner"),
        due_date: obj.get("due_date").and_then(Value::as_str).and_then(normalize_date),
    })
}

fn verdict_label(verdict: &str, ko: bool) -> &'static str {
    match verdict.trim().to_ascii_lowercase().as_str() {
        "mismatch" | "fail" | "failed" | "conflict" => {
            if ko {
                "불일치"
            } else {
                "mismatch"
            }
        }
        "missing" | "missing_doc" | "absent" => {
            if ko {
                "누락"
            } else {
                "missing"
            }
        }
        "undetermined" | "unknown" | "indeterminate" => {
            if ko {
                "판단불가"
            } else {
                "undetermined"
            }
        }
        _ => "",
    }
}

/// A `finding:<verdict>:<rule_id>` JSON source (`{rule, verdict, expected, found, doc_name,
/// page, message}`) → `대상 문서 | 지적 사항 | 근거 | 조치 요청`.
fn finding_row(tag: &str, text: &str, ko: bool) -> Option<Vec<String>> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    let obj = v.as_object()?;
    let title_verdict = tag.split(':').nth(1).unwrap_or("").trim();
    let verdict = {
        let v = json_str(obj, "verdict");
        if v.is_empty() {
            title_verdict.to_string()
        } else {
            v
        }
    };
    let rule = json_str(obj, "rule");
    let message = json_str(obj, "message");
    let expected = json_str(obj, "expected");
    let found = json_str(obj, "found");
    let doc_name = json_str(obj, "doc_name");
    let page = json_str(obj, "page");
    let document = match (doc_name.is_empty(), page.is_empty()) {
        (true, _) => String::new(),
        (false, true) => doc_name,
        (false, false) => {
            if ko {
                format!("{doc_name} {page}쪽")
            } else {
                format!("{doc_name} p.{page}")
            }
        }
    };
    let label = verdict_label(&verdict, ko);
    let issue = match (label.is_empty(), rule.is_empty()) {
        (false, false) => format!("[{label}] {rule}"),
        (true, false) => rule,
        (false, true) => label.to_string(),
        (true, true) => message.clone(),
    };
    let mut evidence: Vec<String> = Vec::new();
    if !expected.is_empty() {
        evidence.push(if ko { format!("기대 {expected}") } else { format!("expected {expected}") });
    }
    if !found.is_empty() {
        evidence.push(if ko { format!("확인 {found}") } else { format!("found {found}") });
    }
    if issue.is_empty() && document.is_empty() {
        return None;
    }
    Some(vec![document, issue, evidence.join(" / "), message])
}

/// Finding rows (`대상 문서 | 지적 사항 | 근거 | 조치 요청`) from the text sources: JSON findings
/// of the projects module, else pipe / tab separated lines (a line without separators is
/// a finding with the source title as the document). The `project` card is not a finding.
fn findings_from_sources(sources: &[Source], ko: bool) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for s in sources.iter().filter(|s| s.citation.source == "text") {
        if s.tag == "project" {
            continue;
        }
        if s.tag.starts_with("finding:") {
            if let Some(row) = finding_row(&s.tag, &s.text, ko) {
                rows.push(row);
                continue;
            }
        }
        for raw in s.text.lines() {
            let line = strip_bullet(raw);
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            let parts: Vec<String> = line.split(['|', '\t']).map(|p| p.trim().to_string()).collect();
            let mut row = vec![String::new(); 4];
            match parts.len() {
                1 => {
                    row[0] = s.citation.title.clone();
                    row[1] = parts[0].clone();
                }
                _ => {
                    for (i, p) in parts.into_iter().take(4).enumerate() {
                        row[i] = p;
                    }
                }
            }
            if row[1].is_empty() && row[0].is_empty() {
                continue;
            }
            rows.push(row);
        }
    }
    rows
}

/// `라벨: 값` hints in the content (수신, 발신, 제목, 기한, 담당자, 연락처, 문서번호, 건명).
fn hints(content: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in content.lines() {
        if let Some((l, v)) = line.split_once([':', '：']) {
            let key = compact(l);
            let v = v.trim();
            if !key.is_empty() && key.chars().count() <= 8 && !v.is_empty() {
                out.entry(key).or_insert_with(|| v.to_string());
            }
        }
    }
    out
}

fn hint<'a>(h: &'a BTreeMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| h.get(&compact(k)).map(String::as_str))
}

fn first_date(text: &str) -> Option<String> {
    text.split([',', '\n', '(', ')']).find_map(|part| normalize_date(part.trim()))
}

/// Composes a 보완요청서 from the request alone (no model): fields from `라벨: 값` hints and
/// defaults, the item table from the text-source findings, a standard three-section body.
pub fn supplement_draft(spec: &TemplateSpec, req: &DraftRequest, sources: &[Source], ui_ko: bool) -> Draft {
    let ko = req.language == Language::Ko;
    let today = Local::now().date_naive();
    let h = hints(&req.content);
    let findings = findings_from_sources(sources, ko);
    let project = project_info(sources).unwrap_or_default();
    let project_subject = match (project.name.is_empty(), project.intake_no.is_empty()) {
        (false, false) => {
            if ko {
                format!("{} (접수번호 {})", project.name, project.intake_no)
            } else {
                format!("{} (intake no. {})", project.name, project.intake_no)
            }
        }
        (false, true) => project.name.clone(),
        (true, false) => project.intake_no.clone(),
        (true, true) => String::new(),
    };
    let subject = hint(&h, &["건명", "대상 건명", "대상", "건", "프로젝트", "subject", "case"])
        .map(str::to_string)
        .or_else(|| (!project_subject.is_empty()).then(|| project_subject.clone()));
    let due = hint(&h, &["기한", "보완 기한", "제출 기한", "deadline", "due"])
        .and_then(normalize_date)
        .or_else(|| first_date(&req.content))
        .or_else(|| project.due_date.clone())
        .unwrap_or_else(|| (today + Duration::days(DEFAULT_DUE_DAYS)).format("%Y-%m-%d").to_string());
    let recipient_default = if project.name.is_empty() { project.intake_no.clone() } else { project.name.clone() };
    let text_cite = sources.iter().find(|s| s.citation.source == "text").map(|s| s.citation.clone());
    let mut d = Draft::default();
    if let Some(p) = req.previous.as_deref() {
        d.id = p.id.clone();
        d.created_at = p.created_at;
    }
    d.title = hint(&h, &["제목", "subject", "title"]).map(str::to_string).unwrap_or_else(|| match &subject {
        Some(s) if ko => format!("{s} 보완 요청"),
        Some(s) => format!("Request for supplementary documents: {s}"),
        None => tr(ko, "제출 서류 보완 요청", "Request for supplementary documents"),
    });
    for f in spec.scalars() {
        let value = match f.key.as_str() {
            "issue_date" => today.format("%Y-%m-%d").to_string(),
            "recipient" => hint(&h, &["수신", "수신처", "to", "recipient"]).unwrap_or(&recipient_default).to_string(),
            "sender" => hint(&h, &["발신", "발신처", "from", "sender"]).unwrap_or("").to_string(),
            "title" => d.title.clone(),
            "subject" => subject.clone().unwrap_or_default(),
            "due_date" => due.clone(),
            "contact_person" => hint(&h, &["담당자", "담당", "contact"]).unwrap_or(&project.owner).to_string(),
            "phone" => hint(&h, &["연락처", "전화", "전화번호", "phone"]).unwrap_or("").to_string(),
            "document_no" => hint(&h, &["문서번호", "document no", "no"]).unwrap_or("").to_string(),
            _ => String::new(),
        };
        d.fields.push(DraftField {
            key: f.key.clone(),
            label: f.label(ko).to_string(),
            kind: f.kind.clone(),
            required: f.required,
            value,
            normalized: None,
            source: None,
            user_edited: false,
        });
    }
    // Keep user edits of a previous draft.
    if let Some(p) = req.previous.as_deref() {
        for pf in p.fields.iter().filter(|f| f.user_edited) {
            if let Some(f) = d.fields.iter_mut().find(|f| f.key == pf.key) {
                f.value = pf.value.clone();
                f.user_edited = true;
            }
        }
    }
    for t in spec.tables() {
        let rows = if t.key == "items" { findings.clone() } else { Vec::new() };
        let rows = rows
            .into_iter()
            .map(|mut r| {
                r.resize(t.columns.len(), String::new());
                r
            })
            .collect();
        d.tables.push(DraftTable {
            key: t.key.clone(),
            label: t.label(ko).to_string(),
            columns: t.columns.clone(),
            rows,
        });
    }
    let n = findings.len();
    let sections = spec.sections(ko);
    let section = |i: usize, fallback: &str| sections.get(i).cloned().unwrap_or_else(|| fallback.to_string());
    d.body.push(DraftBlock::Heading { level: 2, text: section(0, &tr(ko, "근거", "Basis")) });
    d.body.push(DraftBlock::Paragraph {
        text: if ko {
            "제출하신 서류를 검토한 결과 아래와 같은 불일치·누락 사항이 확인되어 관련 규정에 따라 보완을 요청드립니다.".to_string()
        } else {
            "Our review of the submitted documents found the discrepancies and omissions listed below; please supplement them in accordance with the applicable rules.".to_string()
        },
        source: text_cite.clone(),
    });
    for s in sources.iter().filter(|s| s.citation.source == "knowledge").take(2) {
        d.body.push(DraftBlock::Paragraph { text: truncate(s.text.trim(), 400), source: Some(s.citation.clone()) });
    }
    d.body.push(DraftBlock::Heading {
        level: 2, text: section(1, &tr(ko, "보완 요청 사항", "Items to supplement"))
    });
    d.body.push(DraftBlock::Paragraph {
        text: if ko {
            format!("보완이 필요한 항목은 총 {n}건이며 세부 내용은 아래 표와 같습니다.")
        } else {
            format!("{n} item(s) require supplementation; see the table below.")
        },
        source: text_cite,
    });
    d.body.push(DraftBlock::TableRef { key: "items".into() });
    d.body.push(DraftBlock::Heading { level: 2, text: section(2, &tr(ko, "안내", "Notice")) });
    d.body.push(DraftBlock::Paragraph {
        text: if ko {
            format!("위 사항을 {due}까지 보완하여 제출해 주시기 바랍니다. 기한 내에 보완되지 않을 경우 처리가 지연되거나 반려될 수 있습니다.")
        } else {
            format!("Please submit the supplemented documents by {due}. Processing may be delayed or the case returned if the deadline is missed.")
        },
        source: None,
    });
    let contact: Vec<String> =
        [("contact_person", if ko { "담당자" } else { "Contact" }), ("phone", if ko { "연락처" } else { "Phone" })]
            .iter()
            .filter_map(|(k, l)| {
                d.field(k).map(|f| f.value.trim().to_string()).filter(|v| !v.is_empty()).map(|v| format!("{l} {v}"))
            })
            .collect();
    if !contact.is_empty() {
        d.body.push(DraftBlock::Paragraph {
            text: format!("{}: {}", tr(ko, "문의", "Enquiries"), contact.join(", ")),
            source: None,
        });
    }
    d.references = references(sources);
    d.notes.push(tr(ui_ko, "모델 없이 규칙으로 작성된 초안입니다.", "Draft composed by rules (no model)."));
    if n == 0 {
        d.notes.push(tr(
            ui_ko,
            "보완 항목 표가 비어 있습니다 — 지적 사항을 입력하세요.",
            "The item table is empty — add the findings.",
        ));
    }
    finish_draft(&mut d, spec, req);
    d
}

/// After a model draft of a 보완요청서: findings rows / dates the model left empty are
/// filled from the deterministic parse.
fn complete_supplement(d: &mut Draft, sources: &[Source]) {
    let ko = d.language == Language::Ko;
    let findings = findings_from_sources(sources, ko);
    if let Some(t) = d.tables.iter_mut().find(|t| t.key == "items") {
        if t.rows.is_empty() && !findings.is_empty() {
            t.rows = findings
                .into_iter()
                .map(|mut r| {
                    r.resize(t.columns.len(), String::new());
                    r
                })
                .collect();
        }
    }
    let today = Local::now().date_naive();
    for f in &mut d.fields {
        if f.value.trim().is_empty() {
            match f.key.as_str() {
                "issue_date" => f.value = today.format("%Y-%m-%d").to_string(),
                "due_date" => f.value = (today + Duration::days(DEFAULT_DUE_DAYS)).format("%Y-%m-%d").to_string(),
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn is_json_failure(e: &CoreError) -> bool {
    matches!(e, CoreError::Engine(m) if m.contains("not JSON") || m.contains("cut off"))
}

/// Produces a validated draft.
pub fn draft(deps: &GenerateDeps<'_>, req: &DraftRequest) -> Result<DraftOutcome> {
    let started = Instant::now();
    if deps.cancelled() {
        return Err(CoreError::Cancelled);
    }
    // 0.7.0: a section-level revision edits `previous` in place; `Whole` is the regeneration below.
    if let Some(scope) = req.revise.as_ref().filter(|s| !matches!(s, ReviseScope::Whole)) {
        return revise(deps, req, scope, started);
    }
    let spec = resolve_spec(deps.db, req)?;
    let mut notes: Vec<String> = Vec::new();
    let sources = gather_sources(deps, req, &mut notes)?;
    if deps.cancelled() {
        return Err(CoreError::Cancelled);
    }
    let elapsed = |s: &Instant| s.elapsed().as_millis() as u64;
    let Some(vlm) = deps.vlm else {
        if spec.builtin_kind() == Some(TemplateKind::SupplementRequest) {
            let mut d = supplement_draft(&spec, req, &sources, deps.ko());
            d.notes.extend(notes);
            super::validate::validate(&mut d);
            return Ok(DraftOutcome {
                draft: d,
                prompt_tokens: 0,
                completion_tokens: 0,
                elapsed_ms: elapsed(&started),
                source: "rules".into(),
                sections: 0,
            });
        }
        return Err(CoreError::EngineNotRunning);
    };

    let prompt_ko = req.language == Language::Ko;
    let reply_tokens = prompts::reply_budget(req);
    // Stage::NlSchema (text-only JSON stage) with the drafting temperature — see `prompts`.
    let model = Vlm {
        chat: vlm.chat,
        model: vlm.model.clone(),
        profile: vlm.profile,
        cancel: vlm.cancel.or(deps.cancel),
        temperature: Some(vlm.temperature.unwrap_or(prompts::TEMPERATURE)),
        seed: vlm.seed,
        ctx: vlm.ctx,
        geometry: vlm.geometry,
    };
    if prompts::split_needed(reply_tokens, vlm.ctx) {
        return draft_split(deps, &model, &spec, req, &sources, reply_tokens, notes, started);
    }
    let budget = prompts::source_budget(vlm.ctx, reply_tokens);
    let blocks = source_blocks(&sources);
    let user_text = prompts::user(&spec, req, &blocks, budget, prompt_ko);
    let system = json!({ "role": "system", "content": prompts::system(prompt_ko) });
    let schema_v = schema::json_schema(&spec);
    let (value, reply) =
        call_json_with_retry(&model, &system, &user_text, schema::SCHEMA_NAME, &schema_v, reply_tokens, prompt_ko)?;
    let mut d = parse_reply(&value, &spec, req, &sources);
    d.model = Some(model.model.clone());
    d.notes.extend(notes);
    if reply.truncated {
        d.notes.push(tr(
            deps.ko(),
            "모델 응답이 길이 제한으로 잘렸을 수 있습니다.",
            "The model reply may have been cut by the token limit.",
        ));
    }
    if spec.builtin_kind() == Some(TemplateKind::SupplementRequest) {
        complete_supplement(&mut d, &sources);
    }
    super::validate::validate(&mut d);
    Ok(DraftOutcome {
        draft: d,
        prompt_tokens: reply.usage.prompt,
        completion_tokens: reply.usage.completion,
        elapsed_ms: elapsed(&started),
        source: "model".into(),
        sections: 1,
    })
}

/// Numbered source blocks as the prompt shows them.
fn source_blocks<'a>(sources: &'a [Source]) -> Vec<SourceBlock<'a>> {
    sources
        .iter()
        .enumerate()
        .map(|(i, s)| SourceBlock {
            index: i as u32 + 1,
            kind: &s.citation.source,
            title: &s.citation.title,
            text: &s.text,
        })
        .collect()
}

/// One JSON stage call; an unparseable first reply is retried once with a reminder, other sampling and
/// a ×1.5 budget.
fn call_json_with_retry(
    model: &Vlm<'_>,
    system: &Value,
    user_text: &str,
    name: &str,
    schema_v: &Value,
    budget: u32,
    ko: bool,
) -> Result<(Value, Reply)> {
    let messages = vec![system.clone(), user_message(user_text)];
    match model.call_json(Stage::NlSchema, &messages, name, schema_v, Some(budget)) {
        Ok(v) => Ok(v),
        Err(e) if is_json_failure(&e) => {
            log::warn!("draft ({name}): first reply unusable ({e}); retrying with a reminder");
            let retry = Vlm {
                chat: model.chat,
                model: model.model.clone(),
                profile: model.profile,
                cancel: model.cancel,
                temperature: model.temperature.map(|t| (t + 0.1).min(0.5)),
                seed: Some(model.seed.unwrap_or(7) + 1),
                ctx: model.ctx,
                geometry: model.geometry,
            };
            let messages = vec![system.clone(), user_message(&format!("{user_text}{}", prompts::retry_reminder(ko)))];
            retry.call_json(Stage::NlSchema, &messages, name, schema_v, Some(budget * 3 / 2))
        }
        Err(e) => Err(e),
    }
}

/// Tail of the body written so far (paragraph and list text), for the next section's prompt.
fn previous_summary(body: &[DraftBlock]) -> String {
    let mut text = String::new();
    for b in body {
        match b {
            DraftBlock::Heading { text: t, .. } => {
                text.push_str("\n## ");
                text.push_str(t);
                text.push('\n');
            }
            DraftBlock::Paragraph { text: t, .. } => {
                text.push_str(t);
                text.push(' ');
            }
            DraftBlock::List { items, .. } => {
                text.push_str(&items.join("; "));
                text.push(' ');
            }
            DraftBlock::TableRef { .. } => {}
        }
    }
    let count = text.chars().count();
    if count > prompts::PREVIOUS_SUMMARY_CHARS {
        let skip = count - prompts::PREVIOUS_SUMMARY_CHARS;
        format!("…{}", text.chars().skip(skip).collect::<String>().trim())
    } else {
        text.trim().to_string()
    }
}

/// Sections of the outline reply (2–12 entries), or `None` when the model gave fewer.
fn parse_outline(v: &Value) -> Option<Vec<prompts::SectionPlan>> {
    let plans: Vec<prompts::SectionPlan> = v["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| {
            let heading = str_of(x.get("heading").unwrap_or(&Value::Null));
            if heading.is_empty() {
                return None;
            }
            let weight = x.get("weight").and_then(Value::as_u64).unwrap_or(1).clamp(1, 10) as u32;
            Some(prompts::SectionPlan { heading, weight, notes: str_of(x.get("notes").unwrap_or(&Value::Null)) })
        })
        .take(schema::MAX_OUTLINE_SECTIONS)
        .collect();
    (plans.len() >= 2).then_some(plans)
}

/// The outline → sections flow (0.3.0 item 4): fields, tables and the first section come from one call,
/// every further section from its own call with the settled facts and the tail of the previous text.
/// Cancellation between sections keeps what was written (the draft says so in its notes).
#[allow(clippy::too_many_arguments)]
fn draft_split(
    deps: &GenerateDeps<'_>,
    model: &Vlm<'_>,
    spec: &TemplateSpec,
    req: &DraftRequest,
    sources: &[Source],
    total: u32,
    mut notes: Vec<String>,
    started: Instant,
) -> Result<DraftOutcome> {
    let ko = deps.ko();
    let prompt_ko = req.language == Language::Ko;
    let report = |frac: f32, detail: &str| {
        if let Some(p) = deps.progress {
            p(frac, detail);
        }
    };
    let system = json!({ "role": "system", "content": prompts::system(prompt_ko) });
    let blocks = source_blocks(sources);
    let (mut prompt_tokens, mut completion_tokens) = (0u64, 0u64);

    // 1. Section plan: the template's sections, else an outline call, else the generic outline.
    let template_sections = spec.sections(prompt_ko);
    let mut plans: Vec<prompts::SectionPlan> = if template_sections.is_empty() {
        report(0.22, "outline");
        let budget = prompts::source_budget(model.ctx, prompts::OUTLINE_TOKENS);
        let text = prompts::outline_user(spec, req, &blocks, budget, total, prompt_ko);
        match call_json_with_retry(
            model,
            &system,
            &text,
            schema::OUTLINE_SCHEMA_NAME,
            &schema::outline_schema(),
            prompts::OUTLINE_TOKENS,
            prompt_ko,
        ) {
            Ok((v, reply)) => {
                prompt_tokens += reply.usage.prompt;
                completion_tokens += reply.usage.completion;
                parse_outline(&v).unwrap_or_else(|| prompts::fallback_outline(prompt_ko))
            }
            Err(e) if is_json_failure(&e) => prompts::fallback_outline(prompt_ko),
            Err(e) => return Err(e),
        }
    } else {
        template_sections
            .iter()
            .map(|h| prompts::SectionPlan { heading: h.clone(), weight: 1, notes: String::new() })
            .collect()
    };
    if plans.len() < 2 {
        plans = prompts::fallback_outline(prompt_ko);
    }
    let n = plans.len();
    let weights_sum: u32 = plans.iter().map(|p| p.weight.max(1)).sum();

    // 2. Head call: fields, tables and the first section.
    let first_budget = prompts::section_budget(total, plans[0].weight, weights_sum);
    let head_budget = first_budget + prompts::HEAD_EXTRA_TOKENS;
    report(0.25, &format!("section 1/{n}"));
    let budget = prompts::source_budget(model.ctx, head_budget);
    let mut text = prompts::user(spec, req, &blocks, budget, prompt_ko);
    text.push_str(&prompts::split_head_note(&plans[0], n, first_budget, prompt_ko));
    let (value, reply) = call_json_with_retry(
        model,
        &system,
        &text,
        schema::SCHEMA_NAME,
        &schema::json_schema(spec),
        head_budget,
        prompt_ko,
    )?;
    prompt_tokens += reply.usage.prompt;
    completion_tokens += reply.usage.completion;
    let mut d = parse_reply(&value, spec, req, sources);
    d.model = Some(model.model.clone());
    ensure_section_heading(&mut d.body, 0, &plans[0].heading);
    if reply.truncated {
        notes.push(section_truncated_note(1, &plans[0].heading, ko));
    }
    let mut sections_done = 1u32;
    let facts: Vec<(String, String)> = std::iter::once((tr(ko, "제목", "title"), d.title.clone()))
        .chain(
            d.fields.iter().filter(|f| !f.value.trim().is_empty()).take(11).map(|f| (f.label.clone(), f.value.clone())),
        )
        .collect();
    let excluded: HashSet<&str> = req.excluded_sources.iter().map(String::as_str).collect();

    // 3. The remaining sections.
    for (i, plan) in plans.iter().enumerate().skip(1) {
        if deps.cancelled() || model.cancelled() {
            notes.push(partial_note(sections_done, n, ko));
            break;
        }
        report(0.25 + 0.7 * (i as f32 / n as f32), &format!("section {}/{n}", i + 1));
        let budget_tokens = prompts::section_budget(total, plan.weight, weights_sum);
        // Extra knowledge for this section (its heading as the query), numbered after the shared sources.
        let mut extra: Vec<Source> = Vec::new();
        if req.use_knowledge && deps.knowledge.is_some() {
            let mut seen: HashSet<i64> = HashSet::new();
            let mut scratch = Vec::new();
            let scope = knowledge_scope(req);
            let lookup = KnowledgeLookup { scope: &scope, query: &plan.heading, limit: 3, explicit: false };
            let _ = knowledge_sources(deps, lookup, &excluded, &mut seen, &mut extra, &mut scratch);
            extra.truncate(3);
        }
        let combined: Vec<&Source> = sources.iter().chain(extra.iter()).collect();
        let section_blocks: Vec<SourceBlock<'_>> = combined
            .iter()
            .enumerate()
            .map(|(k, s)| SourceBlock {
                index: k as u32 + 1,
                kind: &s.citation.source,
                title: &s.citation.title,
                text: &s.text,
            })
            .collect();
        let budget = prompts::source_budget(model.ctx, budget_tokens);
        let previous = previous_summary(&d.body);
        // 0.7.0: a regeneration passes the instruction and this section's earlier text along.
        let earlier = req.previous.as_deref().and_then(|p| earlier_section_text(&p.body, &plan.heading));
        let revision = (req.previous.is_some() || req.feedback.is_some()).then_some(prompts::SectionRevision {
            feedback: req.feedback.as_deref(),
            previous_section: earlier.as_deref(),
        });
        let text = prompts::section_user(
            spec,
            req,
            &section_blocks,
            budget,
            plan,
            i + 1,
            n,
            budget_tokens,
            &facts,
            &previous,
            revision,
            prompt_ko,
        );
        match call_json_with_retry(
            model,
            &system,
            &text,
            schema::SECTION_SCHEMA_NAME,
            &schema::section_schema(),
            budget_tokens,
            prompt_ko,
        ) {
            Ok((v, reply)) => {
                prompt_tokens += reply.usage.prompt;
                completion_tokens += reply.usage.completion;
                let start = d.body.len();
                let mut blocks = parse_body(&v["blocks"], &combined);
                // The model's own headings are sub-headings of this section.
                for b in &mut blocks {
                    if let DraftBlock::Heading { level, .. } = b {
                        *level = 3;
                    }
                }
                d.body.extend(blocks);
                ensure_section_heading(&mut d.body, start, &plan.heading);
                for s in &extra {
                    let c = &s.citation;
                    if !d
                        .references
                        .iter()
                        .any(|o| o.doc_id == c.doc_id && o.chunk_id == c.chunk_id && o.page == c.page)
                    {
                        d.references.push(c.clone());
                    }
                }
                if reply.truncated {
                    notes.push(section_truncated_note(i + 1, &plan.heading, ko));
                }
                sections_done += 1;
            }
            Err(CoreError::Cancelled) => {
                notes.push(partial_note(sections_done, n, ko));
                break;
            }
            Err(e) if is_json_failure(&e) => {
                let start = d.body.len();
                ensure_section_heading(&mut d.body, start, &plan.heading);
                notes.push(if ko {
                    format!("{}절 '{}'은 모델 응답을 읽을 수 없어 비워 두었습니다.", i + 1, plan.heading)
                } else {
                    format!("Section {} '{}' was left empty: the model reply was unusable.", i + 1, plan.heading)
                });
                sections_done += 1;
            }
            Err(e) => return Err(e),
        }
    }
    notes.push(if ko {
        format!("토큰 예산 {total}로 {n}개 절을 나눠 생성했습니다.")
    } else {
        format!("Generated in {n} sections with a budget of {total} tokens.")
    });
    d.notes.extend(notes);
    if spec.builtin_kind() == Some(TemplateKind::SupplementRequest) {
        complete_supplement(&mut d, sources);
    }
    super::validate::validate(&mut d);
    report(0.97, "finishing");
    Ok(DraftOutcome {
        draft: d,
        prompt_tokens,
        completion_tokens,
        elapsed_ms: started.elapsed().as_millis() as u64,
        source: "model".into(),
        sections: sections_done,
    })
}

/// Plain text of body blocks (headings as `## …`, lists joined), for prompts.
fn blocks_text(blocks: &[DraftBlock]) -> String {
    let mut text = String::new();
    for b in blocks {
        match b {
            DraftBlock::Heading { text: t, .. } => {
                text.push_str("\n## ");
                text.push_str(t);
                text.push('\n');
            }
            DraftBlock::Paragraph { text: t, .. } => {
                text.push_str(t);
                text.push(' ');
            }
            DraftBlock::List { items, .. } => {
                text.push_str(&items.join("; "));
                text.push(' ');
            }
            DraftBlock::TableRef { .. } => {}
        }
    }
    text.trim().to_string()
}

/// Last `max` characters of a text (prefixed with an ellipsis when cut).
fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count > max {
        format!("…{}", text.chars().skip(count - max).collect::<String>().trim_start())
    } else {
        text.to_string()
    }
}

/// The text of the section headed `heading` in an earlier draft body (without the heading).
fn earlier_section_text(body: &[DraftBlock], heading: &str) -> Option<String> {
    let want = compact(heading);
    sections_of(body)
        .into_iter()
        .find(|s| s.heading.as_deref().map(compact) == Some(want.clone()))
        .map(|s| {
            let start = if s.heading.is_some() { s.start + 1 } else { s.start };
            truncate(&blocks_text(&body[start..s.end]), prompts::REVISION_SECTION_CHARS)
        })
        .filter(|t| !t.is_empty())
}

/// Revises `req.previous` in place (0.7.0 plan §1.5): one section rewritten or a new section
/// inserted from one model call that sees the facts, the sources, the surrounding text and the
/// instruction (`req.feedback`). Fields, tables and the other sections stay as they are.
fn revise(deps: &GenerateDeps<'_>, req: &DraftRequest, scope: &ReviseScope, started: Instant) -> Result<DraftOutcome> {
    let prev = req
        .previous
        .as_deref()
        .ok_or_else(|| CoreError::Invalid("revise needs the draft being revised (previous)".into()))?;
    let instruction = req
        .feedback
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .ok_or_else(|| CoreError::Invalid("revise needs an instruction (feedback)".into()))?;
    let ko = deps.ko();
    let prompt_ko = req.language == Language::Ko;
    let sections = sections_of(&prev.body);
    let (rewrite, start, end, index, heading) = match scope {
        ReviseScope::Section { section } => {
            let s = section.checked_sub(1).and_then(|i| sections.get(i)).ok_or_else(|| {
                CoreError::Invalid(format!("section {section} does not exist ({} sections)", sections.len()))
            })?;
            (true, s.start, s.end, *section, s.heading.clone())
        }
        ReviseScope::InsertAfter { section, heading } => {
            let (pos, index) = match section {
                Some(n) => {
                    let s = n.checked_sub(1).and_then(|i| sections.get(i)).ok_or_else(|| {
                        CoreError::Invalid(format!("section {n} does not exist ({} sections)", sections.len()))
                    })?;
                    (s.end, n + 1)
                }
                None => (prev.body.len(), sections.len() + 1),
            };
            (false, pos, pos, index, heading.as_deref().map(str::trim).filter(|h| !h.is_empty()).map(str::to_string))
        }
        ReviseScope::Whole => unreachable!("handled by draft()"),
    };
    let total = if rewrite { sections.len() } else { sections.len() + 1 };
    let spec = resolve_spec(deps.db, req)?;
    let mut notes: Vec<String> = Vec::new();
    let sources = gather_sources(deps, req, &mut notes)?;
    if deps.cancelled() {
        return Err(CoreError::Cancelled);
    }
    let Some(vlm) = deps.vlm else {
        return Err(CoreError::EngineNotRunning);
    };
    let model = Vlm {
        chat: vlm.chat,
        model: vlm.model.clone(),
        profile: vlm.profile,
        cancel: vlm.cancel.or(deps.cancel),
        temperature: Some(vlm.temperature.unwrap_or(prompts::TEMPERATURE)),
        seed: vlm.seed,
        ctx: vlm.ctx,
        geometry: vlm.geometry,
    };
    let tokens = prompts::reply_budget(req).clamp(prompts::MIN_SECTION_TOKENS, prompts::MAX_SECTION_TOKENS);
    let budget = prompts::source_budget(model.ctx, tokens);
    let blocks = source_blocks(&sources);
    let facts: Vec<(String, String)> = std::iter::once((tr(ko, "제목", "title"), prev.title.clone()))
        .chain(
            prev.fields
                .iter()
                .filter(|f| !f.value.trim().is_empty())
                .take(11)
                .map(|f| (f.label.clone(), f.value.clone())),
        )
        .collect();
    let body_start = if rewrite && heading.is_some() { start + 1 } else { start };
    let current = if rewrite {
        truncate(&blocks_text(&prev.body[body_start..end]), prompts::REVISION_SECTION_CHARS)
    } else {
        String::new()
    };
    let before = tail_chars(&blocks_text(&prev.body[..start]), prompts::REVISION_CONTEXT_CHARS);
    let after = truncate(&blocks_text(&prev.body[end..]), prompts::REVISION_CONTEXT_CHARS);
    let rv = prompts::RevisePrompt {
        rewrite,
        index,
        total,
        heading: heading.as_deref(),
        tokens,
        facts: &facts,
        current: &current,
        before: &before,
        after: &after,
        instruction,
    };
    let user_text = prompts::revise_user(&spec, req, &blocks, budget, &rv, prompt_ko);
    let system = json!({ "role": "system", "content": prompts::system(prompt_ko) });
    if let Some(p) = deps.progress {
        p(0.3, "revising");
    }
    let (value, reply) = call_json_with_retry(
        &model,
        &system,
        &user_text,
        schema::REVISE_SCHEMA_NAME,
        &schema::section_schema(),
        tokens,
        prompt_ko,
    )?;
    let refs: Vec<&Source> = sources.iter().collect();
    let mut new_blocks = parse_body(&value["blocks"], &refs);
    let mut d = prev.clone();
    if rewrite && new_blocks.is_empty() {
        d.notes.push(tr(
            ko,
            "모델 응답이 비어 있어 절을 그대로 두었습니다.",
            "The model reply was empty; the section was left as it was.",
        ));
    } else {
        let mut replacement: Vec<DraftBlock> = Vec::new();
        if rewrite {
            if let Some(h) = &heading {
                replacement.push(DraftBlock::Heading { level: 2, text: h.clone() });
            }
        } else {
            let title = match &heading {
                Some(h) => h.clone(),
                None => match new_blocks.first() {
                    Some(DraftBlock::Heading { text, .. }) if !text.trim().is_empty() => {
                        let t = text.trim().to_string();
                        new_blocks.remove(0);
                        t
                    }
                    _ => tr(ko, "새 절", "New section"),
                },
            };
            replacement.push(DraftBlock::Heading { level: 2, text: title });
        }
        // The model's own headings are sub-headings of the section.
        for b in &mut new_blocks {
            if let DraftBlock::Heading { level, .. } = b {
                *level = 3;
            }
        }
        replacement.extend(new_blocks);
        // The model cannot emit table references: the tables the section placed stay in it (after the new text).
        replacement.extend(prev.body[start..end].iter().filter(|b| matches!(b, DraftBlock::TableRef { .. })).cloned());
        d.body.splice(start..end, replacement);
    }
    for s in &sources {
        let c = &s.citation;
        if !d.references.iter().any(|o| o.doc_id == c.doc_id && o.chunk_id == c.chunk_id && o.page == c.page) {
            d.references.push(c.clone());
        }
    }
    d.model = Some(model.model.clone());
    d.updated_at = chrono::Utc::now();
    let mut stored = req.clone();
    stored.previous = None;
    stored.feedback = None;
    stored.revise = None;
    d.request = Some(Box::new(stored));
    for n in notes {
        if !d.notes.contains(&n) {
            d.notes.push(n);
        }
    }
    let where_ = match (&heading, rewrite) {
        (Some(h), true) => format!("{index}절 '{h}'"),
        (None, true) => format!("{index}절"),
        (Some(h), false) => format!("{index}절 '{h}' 추가"),
        (None, false) => format!("{index}절 추가"),
    };
    d.notes.push(if ko {
        format!("AI 수정({where_}): {}", truncate(instruction, 120))
    } else {
        format!(
            "AI revision (section {index}{}): {}",
            heading.as_deref().map(|h| format!(" '{h}'")).unwrap_or_default(),
            truncate(instruction, 120)
        )
    });
    if reply.truncated {
        d.notes.push(section_truncated_note(index, heading.as_deref().unwrap_or(""), ko));
    }
    // Keep the newest notes (the revision note last).
    let excess = d.notes.len().saturating_sub(12);
    d.notes.drain(..excess);
    super::validate::validate(&mut d);
    Ok(DraftOutcome {
        draft: d,
        prompt_tokens: reply.usage.prompt,
        completion_tokens: reply.usage.completion,
        elapsed_ms: started.elapsed().as_millis() as u64,
        source: "model".into(),
        sections: 1,
    })
}

/// Makes sure the body at `start` opens with the section's level-2 heading.
fn ensure_section_heading(body: &mut Vec<DraftBlock>, start: usize, heading: &str) {
    let has =
        matches!(body.get(start), Some(DraftBlock::Heading { level: 2, text }) if compact(text) == compact(heading));
    if !has {
        body.insert(start.min(body.len()), DraftBlock::Heading { level: 2, text: heading.to_string() });
    }
}

fn section_truncated_note(index: usize, heading: &str, ko: bool) -> String {
    if ko {
        format!("{index}절 '{heading}'의 응답이 길이 제한으로 잘렸을 수 있습니다.")
    } else {
        format!("Section {index} '{heading}' may have been cut by the token limit.")
    }
}

fn partial_note(done: u32, total: usize, ko: bool) -> String {
    if ko {
        format!("취소되어 {done}/{total}절까지만 생성했습니다 (여기까지 생성됨).")
    } else {
        format!("Cancelled after {done}/{total} sections (generated this far).")
    }
}
