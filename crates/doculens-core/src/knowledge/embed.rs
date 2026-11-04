//! Chunk embeddings (bge-m3 Q8 through llama-server `/v1/embeddings`, CPU)
//! stored in a sqlite-vec `vec0` table keyed by chunk id. The table is created
//! once the dimension is known and rebuilt when the embedding model changes.

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

pub trait Embedder: Send + Sync {
    fn model_id(&self) -> &str;
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// `/v1/embeddings` on the running llama-server router.
pub struct RouterEmbedder {
    pub client: RouterClient,
    pub model: String,
}

impl Embedder for RouterEmbedder {
    fn model_id(&self) -> &str {
        &self.model
    }
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        self.client.embeddings(&self.model, inputs)
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

fn kv_get(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    conn.query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0)).optional()
}

fn kv_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO kv (key, value, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![key, value, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

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
    while report.embedded + report.failed < total {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            report.cancelled = true;
            break;
        }
        progress(report.embedded, total);
        let rows: Vec<(i64, Option<String>, String)> = db.with(|c| {
            let mut st = c.prepare(
                "SELECT id, heading_path, text FROM chunks WHERE embedded_at IS NULL AND id > ?1 ORDER BY id LIMIT ?2",
            )?;
            let rows = st.query_map(params![last_id, batch as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        if rows.is_empty() {
            break;
        }
        last_id = rows.last().map(|r| r.0).unwrap_or(last_id);
        let inputs: Vec<String> = rows.iter().map(|(_, h, t)| embed_input(h.as_deref(), t)).collect();
        let vectors = match embedder.embed(&inputs) {
            Ok(v) => v,
            Err(e) => {
                if report.embedded == 0 && report.failed == 0 {
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
        let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
        let first_batch = !table_ready;
        let reset = db.with_tx(|tx| {
            let reset = if first_batch { ensure_vec_table(tx, &model, dim)? } else { false };
            let mut del = tx.prepare("DELETE FROM chunks_vec WHERE rowid = ?1")?;
            let mut ins = tx.prepare("INSERT INTO chunks_vec (rowid, embedding) VALUES (?1, ?2)")?;
            let mut upd = tx.prepare("UPDATE chunks SET embedded_at = ?2 WHERE id = ?1")?;
            for (id, v) in ids.iter().zip(vectors.iter()) {
                let mut v = v.clone();
                l2_normalize(&mut v);
                del.execute([id])?;
                ins.execute(params![id, to_blob(&v)])?;
                upd.execute(params![id, now])?;
            }
            Ok(reset)
        })?;
        if reset {
            report.reset = true;
        }
        table_ready = true;
        report.dim = Some(dim as u32);
        report.embedded += rows.len() as u64;
    }
    report.remaining = db.with(pending_count)?;
    progress(report.embedded, total);
    Ok(report)
}

pub fn embed_query(embedder: &dyn Embedder, text: &str) -> Result<Vec<f32>> {
    let input: String = text.chars().take(MAX_EMBED_CHARS).collect();
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
