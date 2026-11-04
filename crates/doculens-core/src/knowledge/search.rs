//! Hybrid search (design §3.14.2): morpheme FTS (BM25) + trigram FTS + LIKE
//! fallback for ≤ 2-character queries + vector KNN, fused with reciprocal rank
//! fusion. Glossary synonyms widen the morpheme query; the scope limits the
//! documents; snippets carry char-offset highlights for the UI.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use super::embed::{self, Embedder};
use super::repo::{self, ChunkRow, FtsTable};
use super::tokenize::{fts_quote, normalize, Analyzer};
use super::{SearchChannel, SearchHit, SearchMode, SearchRequest, SearchResponse};
use crate::db::Db;
use crate::error::Result;

pub struct SearchDeps<'a> {
    pub db: &'a Db,
    pub analyzer: &'a Analyzer,
    pub embedder: Option<&'a dyn Embedder>,
}

const RRF_K: f32 = 60.0;
const SNIPPET_CHARS: usize = 240;

fn weight(channel: SearchChannel) -> f32 {
    match channel {
        SearchChannel::Morph => 1.0,
        SearchChannel::Trigram => 0.7,
        SearchChannel::Like => 0.5,
        SearchChannel::Vector => 1.0,
    }
}

/// Words of the raw query (whitespace-separated, punctuation trimmed).
fn raw_words(query: &str) -> Vec<String> {
    query
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '?' | '!' | '"' | '\'' | '(' | ')' | '[' | ']'))
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .map(normalize)
        .collect()
}

/// Morpheme MATCH expression: one group per term, synonyms OR-ed inside.
fn morph_expr(analyzer: &Analyzer, terms: &[String], expansions: &[(String, Vec<String>)], all: bool) -> String {
    let groups: Vec<String> = terms
        .iter()
        .map(|t| {
            let mut alts = vec![fts_quote(t)];
            if let Some((_, forms)) = expansions.iter().find(|(term, _)| term == t) {
                for f in forms {
                    let toks = analyzer.query_terms(f);
                    if !toks.is_empty() {
                        alts.push(fts_quote(&toks.join(" ")));
                    }
                }
            }
            if alts.len() == 1 {
                alts.remove(0)
            } else {
                format!("({})", alts.join(" OR "))
            }
        })
        .collect();
    groups.join(if all { " AND " } else { " OR " })
}

/// Trigram MATCH expression: substrings of ≥ 3 chars (terms, codes, raw words).
fn trigram_expr(terms: &[String], words: &[String], expansions: &[(String, Vec<String>)]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |s: &str| {
        let s = s.trim();
        if s.chars().count() >= 3 && seen.insert(s.to_string()) {
            parts.push(fts_quote(s));
        }
    };
    for t in terms {
        push(t);
    }
    for w in words {
        push(w);
    }
    for (_, forms) in expansions {
        for f in forms {
            push(&normalize(f));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" OR "))
    }
}

/// Lower-cased chars (1:1 with the original for Hangul/ASCII) for matching.
fn fold(text: &str) -> Vec<char> {
    text.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

fn find_all(hay: &[char], needle: &[char]) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() || needle.len() > hay.len() {
        return out;
    }
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == *needle {
            out.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// `[start, end)` char ranges of the needles in `text` (longest needles first, no overlaps).
pub fn highlight_ranges(text: &str, needles: &[String]) -> Vec<(usize, usize)> {
    let hay = fold(text);
    let mut needles: Vec<Vec<char>> = needles.iter().map(|n| fold(n)).filter(|n| !n.is_empty()).collect();
    needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
    needles.dedup();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for n in &needles {
        for start in find_all(&hay, n) {
            let end = start + n.len();
            if !ranges.iter().any(|(s, e)| start < *e && end > *s) {
                ranges.push((start, end));
            }
        }
    }
    ranges.sort_unstable();
    ranges
}

/// Window of `text` around the first highlight, with highlights re-based.
pub fn snippet(text: &str, needles: &[String]) -> (String, Vec<(usize, usize)>) {
    let chars: Vec<char> = text.chars().collect();
    let ranges = highlight_ranges(text, needles);
    if chars.len() <= SNIPPET_CHARS {
        return (text.to_string(), ranges);
    }
    let first = ranges.first().map(|r| r.0).unwrap_or(0);
    let mut start = first.saturating_sub(SNIPPET_CHARS / 3);
    // Begin at a word boundary when possible.
    while start > 0 && start < chars.len() && !chars[start - 1].is_whitespace() && first - start < SNIPPET_CHARS / 2 {
        start -= 1;
    }
    let end = (start + SNIPPET_CHARS).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s.insert(0, '…');
    }
    let offset = if start > 0 { 1 } else { 0 };
    if end < chars.len() {
        s.push('…');
    }
    let rebased = ranges
        .into_iter()
        .filter(|(a, b)| *a >= start && *b <= end)
        .map(|(a, b)| (a - start + offset, b - start + offset))
        .collect();
    (s, rebased)
}

struct Fusion {
    scores: HashMap<i64, f32>,
    channels: HashMap<i64, Vec<SearchChannel>>,
}

impl Fusion {
    fn new() -> Fusion {
        Fusion { scores: HashMap::new(), channels: HashMap::new() }
    }
    fn add(&mut self, channel: SearchChannel, ranked: &[i64]) {
        for (rank, id) in ranked.iter().enumerate() {
            *self.scores.entry(*id).or_insert(0.0) += weight(channel) / (RRF_K + rank as f32 + 1.0);
            let ch = self.channels.entry(*id).or_default();
            if !ch.contains(&channel) {
                ch.push(channel);
            }
        }
    }
}

/// Runs a search request.
pub fn search(deps: &SearchDeps<'_>, req: &SearchRequest) -> Result<SearchResponse> {
    let started = Instant::now();
    let query = req.query.trim();
    let mut resp = SearchResponse::default();
    if query.is_empty() {
        return Ok(resp);
    }
    let limit = req.limit.clamp(1, 200);
    let terms = deps.analyzer.query_terms(query);
    let words = raw_words(query);
    let expansions = if req.expand_glossary && !terms.is_empty() {
        deps.db.with(|c| repo::glossary_expansions(c, &terms))?
    } else {
        Vec::new()
    };
    resp.query_terms = terms.clone();
    resp.expanded_terms = expansions.iter().flat_map(|(_, f)| f.iter().cloned()).collect();

    let allowed = deps.db.with(|c| repo::scope_doc_ids(c, &req.scope))?;
    if allowed.as_ref().is_some_and(|a| a.is_empty()) {
        resp.elapsed_ms = started.elapsed().as_millis() as u64;
        return Ok(resp);
    }
    // Candidates per channel: generous when the scope filters afterwards.
    let fetch = if allowed.is_some() { limit * 20 } else { limit * 5 }.clamp(50, 2_000);
    let mut fusion = Fusion::new();
    let mut similarity: HashMap<i64, f32> = HashMap::new();

    if req.mode != SearchMode::Semantic && !terms.is_empty() {
        let expr_all = morph_expr(deps.analyzer, &terms, &expansions, true);
        let mut morph: Vec<i64> = deps
            .db
            .with(|c| repo::fts_match(c, FtsTable::Morph, &expr_all, fetch))?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if terms.len() > 1 && (morph.len() as u32) < limit {
            // Relaxed query: chunks must still cover at least half of the terms
            // (one stray word must not count as evidence).
            let expr_any = morph_expr(deps.analyzer, &terms, &expansions, false);
            let more = deps.db.with(|c| repo::fts_match(c, FtsTable::Morph, &expr_any, fetch))?;
            let ids: Vec<i64> = more.iter().map(|(id, _)| *id).filter(|id| !morph.contains(id)).collect();
            let need = if terms.len() >= 3 { terms.len().div_ceil(2) } else { 1 };
            let tokens = deps.db.with(|c| repo::chunk_tokens(c, &ids))?;
            let mut alternatives: Vec<Vec<String>> = Vec::new();
            for t in &terms {
                let mut alts = vec![t.clone()];
                if let Some((_, forms)) = expansions.iter().find(|(term, _)| term == t) {
                    alts.extend(forms.iter().map(|f| normalize(f)));
                }
                alternatives.push(alts);
            }
            for id in ids {
                let Some(tok) = tokens.get(&id) else { continue };
                let set: HashSet<&str> = tok.split(' ').collect();
                let covered = alternatives.iter().filter(|alts| alts.iter().any(|a| set.contains(a.as_str()))).count();
                if covered >= need {
                    morph.push(id);
                }
            }
        }
        fusion.add(SearchChannel::Morph, &morph);
        if let Some(expr) = trigram_expr(&terms, &words, &expansions) {
            let tri: Vec<i64> = deps
                .db
                .with(|c| repo::fts_match(c, FtsTable::Trigram, &expr, fetch))?
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            fusion.add(SearchChannel::Trigram, &tri);
        }
        if fusion.scores.is_empty() {
            // Nothing from the indexes (e.g. a 2-character word the trigram index
            // cannot see and the morpheme index tokenised differently): substring scan.
            let needles: Vec<String> = if words.is_empty() { terms.clone() } else { words.clone() };
            let first = needles[0].clone();
            let rest: Vec<String> = needles[1..].to_vec();
            let ids = deps.db.with(|c| repo::like_search(c, &first, fetch))?;
            let ids: Vec<i64> = if rest.is_empty() {
                ids
            } else {
                let rows = deps.db.with(|c| repo::chunk_rows(c, &ids))?;
                rows.into_iter()
                    .filter(|r| {
                        let folded = normalize(&r.chunk.text);
                        rest.iter().all(|n| folded.contains(n.as_str()))
                    })
                    .map(|r| r.chunk.id)
                    .collect()
            };
            if !ids.is_empty() {
                resp.used_like_fallback = true;
                fusion.add(SearchChannel::Like, &ids);
            }
        }
    }

    if req.mode != SearchMode::Keyword {
        if let Some(embedder) = deps.embedder {
            match embed::embed_query(embedder, query) {
                Ok(q) => {
                    let k = (limit * 4).clamp(20, 400) as usize;
                    let hits = deps.db.with(|c| embed::knn(c, &q, k, allowed.as_ref()))?;
                    if !hits.is_empty() {
                        resp.used_vector = true;
                        let ids: Vec<i64> = hits.iter().map(|(id, _)| *id).collect();
                        for (id, distance) in &hits {
                            similarity.insert(*id, 1.0 - distance);
                        }
                        fusion.add(SearchChannel::Vector, &ids);
                    }
                }
                Err(e) => log::warn!("semantic search skipped: {e}"),
            }
        }
    }

    resp.total_candidates = fusion.scores.len() as u32;
    let mut ranked: Vec<(i64, f32)> = fusion.scores.iter().map(|(id, s)| (*id, *s)).collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    let ids: Vec<i64> = ranked.iter().map(|(id, _)| *id).collect();
    let rows = deps.db.with(|c| repo::chunk_rows(c, &ids))?;
    let by_id: HashMap<i64, ChunkRow> = rows.into_iter().map(|r| (r.chunk.id, r)).collect();

    let mut needles: Vec<String> = Vec::new();
    needles.extend(terms.iter().cloned());
    needles.extend(words.iter().cloned());
    needles.extend(resp.expanded_terms.iter().map(|s| normalize(s)));
    needles.retain(|n| n.chars().count() >= 2 || n.chars().all(|c| c.is_ascii_digit()));
    needles.sort();
    needles.dedup();

    let mut per_doc: HashMap<String, u32> = HashMap::new();
    let mut hits = Vec::new();
    for (id, score) in ranked {
        let Some(row) = by_id.get(&id) else { continue };
        if allowed.as_ref().is_some_and(|a| !a.contains(&row.chunk.document_id)) {
            continue;
        }
        if req.per_document > 0 {
            let n = per_doc.entry(row.chunk.document_id.clone()).or_insert(0);
            if *n >= req.per_document {
                continue;
            }
            *n += 1;
        }
        let (snip, highlights) = snippet(&row.chunk.text, &needles);
        hits.push(SearchHit {
            chunk_id: id,
            document_id: row.chunk.document_id.clone(),
            title: row.title.clone(),
            rel_path: row.rel_path.clone(),
            role: row.role,
            doc_type: row.doc_type.clone(),
            page_start: row.chunk.page_start,
            page_end: row.chunk.page_end,
            kind: row.chunk.kind,
            heading_path: row.chunk.heading_path.clone(),
            clause: row.chunk.clause.clone(),
            text: row.chunk.text.clone(),
            snippet: snip,
            highlights,
            score,
            channels: fusion.channels.get(&id).cloned().unwrap_or_default(),
            vector_similarity: similarity.get(&id).copied(),
        });
        if hits.len() as u32 >= limit {
            break;
        }
    }
    resp.hits = hits;
    resp.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_do_not_overlap_and_prefer_longer_needles() {
        let r = highlight_ranges("공급가액 합계와 공급가액은", &["공급가액".into(), "공급".into(), "합계".into()]);
        assert_eq!(r, vec![(0, 4), (5, 7), (9, 13)]);
    }

    #[test]
    fn snippet_windows_around_first_hit() {
        let text = format!("{}세금계산서 발행{}", "앞 ".repeat(300), " 뒤".repeat(300));
        let (s, h) = snippet(&text, &["세금계산서".into()]);
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert!(s.chars().count() <= SNIPPET_CHARS + 2);
        let (a, b) = h[0];
        assert_eq!(s.chars().skip(a).take(b - a).collect::<String>(), "세금계산서");
    }

    #[test]
    fn raw_words_strip_punctuation_and_lowercase() {
        assert_eq!(raw_words("Invoice No. 123-45, 공급가액은?"), vec!["invoice", "no", "123-45", "공급가액은"]);
    }
}
