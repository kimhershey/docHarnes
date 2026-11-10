//! Chunk embeddings (EmbeddingGemma 2 Q8_0, 768-d, through llama-server
//! `/v1/embeddings`, CPU) stored in a sqlite-vec `vec0` table keyed by chunk id.
//! The table is created once the dimension is known and rebuilt when the
//! embedding model changes. Models that expect task prefixes (EmbeddingGemma:
//! `task: search result | query: ` / `title: none | text: `) get them through
//! [`EmbedPrefixes`]; the stored text stays prefix-free.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::index::indexed_text;
use super::{repo, EmbeddingInfo};
use crate::db::Db;
use crate::engine::RouterClient;
use crate::error::{CoreError, Result};

pub const KV_EMBEDDING_MODEL: &str = "knowledge.embedding_model";
pub const KV_EMBEDDING_DIM: &str = "knowledge.embedding_dim";
/// Longest input per chunk (the embedding preset runs with a 2,048-token context).
pub const MAX_EMBED_CHARS: usize = 2_000;
pub const DEFAULT_BATCH: usize = 16;

/// Text prefixes an embedding model expects in front of queries and documents
/// (registry `defaults.query_prefix` / `defaults.document_prefix`). Empty for
/// BERT-style models; EmbeddingGemma 2 uses `task: search result | query: ` and
/// `title: none | text: `. Applied after the length cap so the prefix is never cut.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedPrefixes {
    pub query: String,
    pub document: String,
}

impl EmbedPrefixes {
    /// Reads `query_prefix` / `document_prefix` from a registry `defaults` object.
    pub fn from_defaults(defaults: &serde_json::Value) -> EmbedPrefixes {
        let s = |k: &str| defaults.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        EmbedPrefixes { query: s("query_prefix"), document: s("document_prefix") }
    }
}

pub trait Embedder: Send + Sync {
    fn model_id(&self) -> &str;
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>>;
    /// Prefixes prepended to query / document texts (none by default).
    fn prefixes(&self) -> EmbedPrefixes {
        EmbedPrefixes::default()
    }
}

/// `/v1/embeddings` on the running llama-server router.
pub struct RouterEmbedder {
    pub client: RouterClient,
    pub model: String,
    pub prefixes: EmbedPrefixes,
}

impl RouterEmbedder {
    pub fn new(client: RouterClient, model: String, prefixes: EmbedPrefixes) -> RouterEmbedder {
        RouterEmbedder { client, model, prefixes }
    }
}

impl Embedder for RouterEmbedder {
    fn model_id(&self) -> &str {
        &self.model
    }
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        self.client.embeddings(&self.model, inputs)
    }
    fn prefixes(&self) -> EmbedPrefixes {
        self.prefixes.clone()
    }
}

/// Deterministic character-bigram hashing embeddings: no model needed, similar
/// texts get similar vectors. Used by tests and the browser mock.
pub struct HashEmbedder {
    pub dim: usize,
    pub model: String,
}

impl HashEmbedder {
    pub fn new(dim: usize) -> HashEmbedder {
        HashEmbedder { dim: dim.max(4), model: format!("hash-{dim}") }
    }
}

impl Embedder for HashEmbedder {
    fn model_id(&self) -> &str {
        &self.model
    }
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(inputs
            .iter()
            .map(|s| {
                let mut v = vec![0f32; self.dim];
                let chars: Vec<char> = s.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
                for w in chars.windows(2) {
                    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
                    for c in w {
                        h ^= *c as u64;
                        h = h.wrapping_mul(0x0100_0000_01b3);
                    }
                    let idx = (h % self.dim as u64) as usize;
                    let sign = if (h >> 32) & 1 == 0 { 1.0 } else { -1.0 };
                    v[idx] += sign;
                }
                l2_normalize(&mut v);
                v
            })
            .collect())
    }
}

pub fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

pub fn to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

use super::repo::{kv_get, kv_set};
/// Consecutive failed batches that abort an embedding run (engine gone).
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

pub fn embedding_info(conn: &Connection) -> rusqlite::Result<EmbeddingInfo> {
    let (total, embedded) = repo::chunk_counts(conn)?;
    Ok(EmbeddingInfo {
        model: kv_get(conn, KV_EMBEDDING_MODEL)?,
        dim: kv_get(conn, KV_EMBEDDING_DIM)?.and_then(|d| d.parse().ok()),
        chunks_total: total,
        chunks_embedded: embedded,
    })
}

/// Creates `chunks_vec` for `(model, dim)`; a different model or dimension
/// drops the old vectors (returns true when a reset happened).
pub fn ensure_vec_table(conn: &Connection, model: &str, dim: usize) -> rusqlite::Result<bool> {
    let exists = repo::vec_table_exists(conn)?;
    let stored_model = kv_get(conn, KV_EMBEDDING_MODEL)?;
    let stored_dim: Option<usize> = kv_get(conn, KV_EMBEDDING_DIM)?.and_then(|d| d.parse().ok());
    let same = exists && stored_model.as_deref() == Some(model) && stored_dim == Some(dim);
    if same {
        return Ok(false);
    }
    let reset = exists || stored_model.is_some();
    conn.execute_batch("DROP TABLE IF EXISTS chunks_vec; UPDATE chunks SET embedded_at = NULL;")?;
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE chunks_vec USING vec0(embedding float[{dim}] distance_metric=cosine)"
    ))?;
    kv_set(conn, KV_EMBEDDING_MODEL, model)?;
    kv_set(conn, KV_EMBEDDING_DIM, &dim.to_string())?;
    Ok(reset)
}

/// Drops all vectors (model removed / user reset).
pub fn reset_embeddings(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("DROP TABLE IF EXISTS chunks_vec; UPDATE chunks SET embedded_at = NULL;")?;
    conn.execute("DELETE FROM kv WHERE key IN (?1, ?2)", params![KV_EMBEDDING_MODEL, KV_EMBEDDING_DIM])?;
    Ok(())
}

pub fn pending_count(conn: &Connection) -> rusqlite::Result<u64> {
    conn.query_row("SELECT COUNT(*) FROM chunks WHERE embedded_at IS NULL", [], |r| r.get::<_, i64>(0))
        .map(|n| n as u64)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EmbedReport {
    pub embedded: u64,
    pub failed: u64,
    pub remaining: u64,
    pub cancelled: bool,
    pub dim: Option<u32>,
    pub reset: bool,
}

fn embed_input(heading_path: Option<&str>, text: &str) -> String {
    let s = indexed_text(heading_path, text);
    if s.chars().count() > MAX_EMBED_CHARS {
        s.chars().take(MAX_EMBED_CHARS).collect()
    } else {
        s
    }
}

/// Embeds chunks without a vector in batches; HTTP calls run outside the
/// write transaction. Returns after all pending chunks are done, a batch
/// fails hard (e.g. the engine stopped) or `cancel` is set.
pub fn embed_pending(
    db: &Db,
    embedder: &dyn Embedder,
    batch: usize,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<EmbedReport> {
    let batch = batch.clamp(1, 128);
    let prefixes = embedder.prefixes();
    // A different model (or a lost table) invalidates every stored vector.
    let stale = db.with(|c| {
        let stored = kv_get(c, KV_EMBEDDING_MODEL)?;
        Ok(stored.as_deref().is_some_and(|m| m != embedder.model_id())
            || (stored.is_some() && !repo::vec_table_exists(c)?))
    })?;
    let mut report = EmbedReport::default();
    if stale {
        db.with(reset_embeddings)?;
        report.reset = true;
    }
    let total = db.with(pending_count)?;
    report.remaining = total;
    let mut last_id: i64 = 0;
    let mut table_ready = false;
    let mut consecutive_failures = 0u32;
    while report.embedded + report.failed < total {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            report.cancelled = true;
            break;
        }
        progress(report.embedded, total);
        let rows: Vec<(i64, Option<String>, String, String)> = db.with(|c| {
            let mut st = c.prepare(
                "SELECT id, heading_path, text, text_hash FROM chunks WHERE embedded_at IS NULL AND id > ?1 ORDER BY id LIMIT ?2",
            )?;
            let rows =
                st.query_map(params![last_id, batch as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        if rows.is_empty() {
            break;
        }
        last_id = rows.last().map(|r| r.0).unwrap_or(last_id);
        let inputs: Vec<String> =
            rows.iter().map(|(_, h, t, _)| format!("{}{}", prefixes.document, embed_input(h.as_deref(), t))).collect();
        let vectors = match embedder.embed(&inputs) {
            Ok(v) => {
                consecutive_failures = 0;
                v
            }
            Err(e) => {
                consecutive_failures += 1;
                if (report.embedded == 0 && report.failed == 0) || consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    return Err(e);
                }
                log::warn!("embedding batch failed: {e}");
                report.failed += rows.len() as u64;
                continue;
            }
        };
        if vectors.len() != rows.len() || vectors.iter().any(|v| v.is_empty()) {
            return Err(CoreError::Invalid(format!(
                "embedding response has {} vectors for {} inputs",
                vectors.len(),
                rows.len()
            )));
        }
        let dim = vectors[0].len();
        if vectors.iter().any(|v| v.len() != dim) {
            return Err(CoreError::Invalid("embedding vectors differ in dimension".into()));
        }
        let model = embedder.model_id().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let first_batch = !table_ready;
        let (reset, stored) = db.with_tx(|tx| {
            let reset = if first_batch { ensure_vec_table(tx, &model, dim)? } else { false };
            let mut del = tx.prepare("DELETE FROM chunks_vec WHERE rowid = ?1")?;
            let mut ins = tx.prepare("INSERT INTO chunks_vec (rowid, embedding) VALUES (?1, ?2)")?;
            // Only the chunk whose text was embedded is marked: a concurrent re-index may
            // have replaced the row (same id, other text) meanwhile.
            let mut upd = tx.prepare(
                "UPDATE chunks SET embedded_at = ?2 WHERE id = ?1 AND text_hash = ?3 AND embedded_at IS NULL",
            )?;
            let mut stored = 0u64;
            for ((id, _, _, hash), v) in rows.iter().zip(vectors.iter()) {
                let mut v = v.clone();
                l2_normalize(&mut v);
                del.execute([id])?;
                if upd.execute(params![id, now, hash])? == 1 {
                    ins.execute(params![id, to_blob(&v)])?;
                    stored += 1;
                }
            }
            Ok((reset, stored))
        })?;
        if reset {
            report.reset = true;
        }
        table_ready = true;
        report.dim = Some(dim as u32);
        report.embedded += stored;
        report.failed += rows.len() as u64 - stored;
    }
    report.remaining = db.with(pending_count)?;
    progress(report.embedded, total);
    Ok(report)
}

pub fn embed_query(embedder: &dyn Embedder, text: &str) -> Result<Vec<f32>> {
    let truncated: String = text.chars().take(MAX_EMBED_CHARS).collect();
    let input = format!("{}{}", embedder.prefixes().query, truncated);
    let mut v = embedder
        .embed(&[input])?
        .into_iter()
        .next()
        .ok_or_else(|| CoreError::Invalid("empty embedding response".into()))?;
    l2_normalize(&mut v);
    Ok(v)
}

/// Nearest chunks by cosine distance (`(chunk id, distance)`, ascending).
/// With `allowed` the search widens until `k` allowed chunks are found or
/// the candidate pool is exhausted.
pub fn knn(
    conn: &Connection,
    query: &[f32],
    k: usize,
    allowed: Option<&HashSet<String>>,
) -> rusqlite::Result<Vec<(i64, f32)>> {
    if query.is_empty() || !repo::vec_table_exists(conn)? {
        return Ok(Vec::new());
    }
    let blob = to_blob(query);
    if let Some(docs) = repo::sql_scope(allowed) {
        // sqlite-vec accepts a rowid IN (...) constraint on KNN queries: the top-k is
        // then taken inside the scope. Falls back to widening when the build lacks it.
        let placeholders = (3..3 + docs.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT v.rowid, v.distance FROM chunks_vec v WHERE v.embedding MATCH ?1 AND v.k = ?2 \
             AND v.rowid IN (SELECT id FROM chunks WHERE document_id IN ({placeholders})) ORDER BY v.distance"
        );
        let k_i = k.max(1) as i64;
        let mut args: Vec<&dyn rusqlite::ToSql> = vec![&blob, &k_i];
        for d in &docs {
            args.push(d);
        }
        let attempt = conn.prepare(&sql).and_then(|mut st| {
            st.query_map(args.as_slice(), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)? as f32)))?
                .collect::<rusqlite::Result<Vec<_>>>()
        });
        match attempt {
            Ok(rows) => return Ok(rows),
            Err(e) => log::debug!("scoped KNN unsupported, widening instead: {e}"),
        }
    }
    let mut fetch = k.max(1);
    let max_fetch = 4_096;
    loop {
        let mut st = conn.prepare(
            "SELECT v.rowid, v.distance, c.document_id FROM chunks_vec v JOIN chunks c ON c.id = v.rowid \
             WHERE v.embedding MATCH ?1 AND v.k = ?2 ORDER BY v.distance",
        )?;
        let rows: Vec<(i64, f32, String)> = st
            .query_map(params![blob, fetch as i64], |r| Ok((r.get(0)?, r.get::<_, f64>(1)? as f32, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let fetched = rows.len();
        let kept: Vec<(i64, f32)> = rows
            .into_iter()
            .filter(|(_, _, doc)| allowed.is_none_or(|a| a.contains(doc)))
            .map(|(id, d, _)| (id, d))
            .take(k)
            .collect();
        if kept.len() >= k || fetched < fetch || fetch >= max_fetch {
            return Ok(kept);
        }
        fetch = (fetch * 4).min(max_fetch);
    }
}
