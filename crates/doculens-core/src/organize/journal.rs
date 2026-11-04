//! File-operation journal (design §3.5 "되돌리기"): every move / rename /
//! deletion the app performs is recorded as a batch of entries in
//! `ops_journal` (plus an append-only `journal/ops-YYYYMM.jsonl` mirror).
//! Undo reverts a batch LIFO after checking the file hash — entries whose file
//! was changed or moved since are skipped with a reason; redo reverts the
//! last undo. OS-recycle-bin restores are left to the OS; the workspace trash
//! fallback is restored in-app.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::documents::import::blake3_file;
use crate::documents::{from_rel_path, repo as docs};
use crate::error::{CoreError, Result};
use crate::paths::WorkspacePaths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Move,
    Rename,
    Delete,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Move => "move",
            EntryKind::Rename => "rename",
            EntryKind::Delete => "delete",
        }
    }
    pub fn parse(s: &str) -> Option<EntryKind> {
        match s {
            "move" => Some(EntryKind::Move),
            "rename" => Some(EntryKind::Rename),
            "delete" => Some(EntryKind::Delete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: i64,
    pub batch_id: String,
    pub seq: u32,
    pub kind: EntryKind,
    pub document_id: Option<String>,
    pub from_path: String,
    pub to_path: Option<String>,
    pub trash_path: Option<String>,
    pub content_hash: Option<String>,
    pub size_bytes: Option<u64>,
    pub created_at: DateTime<Utc>,
    pub undone_at: Option<DateTime<Utc>>,
    pub undo_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalBatch {
    pub id: String,
    /// `organize` | `rename` | `move` | `delete` | `undo` | `redo`
    pub kind: String,
    pub label: Option<String>,
    pub source: Option<String>,
    pub created_at: DateTime<Utc>,
    pub entries: u32,
    pub undone_at: Option<DateTime<Utc>>,
    pub undone_by: Option<String>,
    pub redo_of: Option<String>,
}

impl JournalBatch {
    /// A batch that can still be reverted (not an undo itself, not yet undone, has entries).
    pub fn undoable(&self) -> bool {
        self.kind != "undo" && self.undone_at.is_none() && self.entries > 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewEntry {
    pub kind: EntryKind,
    pub document_id: Option<String>,
    pub from_path: String,
    pub to_path: Option<String>,
    pub trash_path: Option<String>,
    pub content_hash: Option<String>,
    pub size_bytes: Option<u64>,
}

fn ts(t: &DateTime<Utc>) -> String {
    t.to_rfc3339()
}

fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now())
}

pub fn begin_batch(
    conn: &Connection,
    id: &str,
    kind: &str,
    label: Option<&str>,
    source: Option<&str>,
    plan_json: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO journal_batches (id, kind, label, source, plan_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, kind, label, source, plan_json, ts(&Utc::now())],
    )?;
    Ok(())
}

/// Appends one entry (and mirrors it into `journal/ops-YYYYMM.jsonl`, best effort).
pub fn append(
    conn: &Connection,
    paths: &WorkspacePaths,
    batch_id: &str,
    seq: u32,
    e: &NewEntry,
) -> rusqlite::Result<i64> {
    let now = Utc::now();
    conn.execute(
        "INSERT INTO ops_journal (batch_id, seq, kind, document_id, from_path, to_path, trash_path, content_hash, size_bytes, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            batch_id,
            seq as i64,
            e.kind.as_str(),
            e.document_id,
            e.from_path,
            e.to_path,
            e.trash_path,
            e.content_hash,
            e.size_bytes.map(|s| s as i64),
            ts(&now)
        ],
    )?;
    conn.execute("UPDATE journal_batches SET entries = entries + 1 WHERE id = ?1", [batch_id])?;
    let line = serde_json::json!({
        "batch": batch_id, "seq": seq, "kind": e.kind.as_str(), "document_id": e.document_id,
        "from": e.from_path, "to": e.to_path, "trash": e.trash_path, "hash": e.content_hash, "at": ts(&now),
    });
    mirror(paths, &now, &line.to_string());
    Ok(conn.last_insert_rowid())
}

fn mirror(paths: &WorkspacePaths, now: &DateTime<Utc>, line: &str) {
    use std::io::Write;
    let dir = paths.journal_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let file = dir.join(format!("ops-{}.jsonl", now.format("%Y%m")));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{line}");
    }
}

fn row_to_batch(r: &Row<'_>) -> rusqlite::Result<JournalBatch> {
    Ok(JournalBatch {
        id: r.get(0)?,
        kind: r.get(1)?,
        label: r.get(2)?,
        source: r.get(3)?,
        created_at: parse_ts(&r.get::<_, String>(4)?),
        entries: r.get::<_, i64>(5)?.max(0) as u32,
        undone_at: r.get::<_, Option<String>>(6)?.as_deref().map(parse_ts),
        undone_by: r.get(7)?,
        redo_of: r.get(8)?,
    })
}

const BATCH_COLUMNS: &str = "id, kind, label, source, created_at, entries, undone_at, undone_by, redo_of";

pub fn list_batches(conn: &Connection, limit: u32) -> rusqlite::Result<Vec<JournalBatch>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {BATCH_COLUMNS} FROM journal_batches ORDER BY created_at DESC, rowid DESC LIMIT ?1"
    ))?;
    let rows = st.query_map([limit.max(1) as i64], row_to_batch)?;
    rows.collect()
}

pub fn get_batch(conn: &Connection, id: &str) -> rusqlite::Result<Option<JournalBatch>> {
    conn.query_row(&format!("SELECT {BATCH_COLUMNS} FROM journal_batches WHERE id = ?1"), [id], row_to_batch).optional()
}

fn row_to_entry(r: &Row<'_>) -> rusqlite::Result<JournalEntry> {
    Ok(JournalEntry {
        id: r.get(0)?,
        batch_id: r.get(1)?,
        seq: r.get::<_, i64>(2)?.max(0) as u32,
        kind: EntryKind::parse(&r.get::<_, String>(3)?).unwrap_or(EntryKind::Move),
        document_id: r.get(4)?,
        from_path: r.get(5)?,
        to_path: r.get(6)?,
        trash_path: r.get(7)?,
        content_hash: r.get(8)?,
        size_bytes: r.get::<_, Option<i64>>(9)?.map(|v| v.max(0) as u64),
        created_at: parse_ts(&r.get::<_, String>(10)?),
        undone_at: r.get::<_, Option<String>>(11)?.as_deref().map(parse_ts),
        undo_error: r.get(12)?,
    })
}

const ENTRY_COLUMNS: &str =
    "id, batch_id, seq, kind, document_id, from_path, to_path, trash_path, content_hash, size_bytes, created_at, undone_at, undo_error";

pub fn entries(conn: &Connection, batch_id: &str) -> rusqlite::Result<Vec<JournalEntry>> {
    let mut st =
        conn.prepare_cached(&format!("SELECT {ENTRY_COLUMNS} FROM ops_journal WHERE batch_id = ?1 ORDER BY seq"))?;
    let rows = st.query_map([batch_id], row_to_entry)?;
    rows.collect()
}

/// Latest batch that `undo` can revert.
pub fn last_undoable(conn: &Connection) -> rusqlite::Result<Option<JournalBatch>> {
    conn.query_row(
        &format!("SELECT {BATCH_COLUMNS} FROM journal_batches WHERE kind != 'undo' AND undone_at IS NULL AND entries > 0 ORDER BY created_at DESC, rowid DESC LIMIT 1"),
        [],
        row_to_batch,
    )
    .optional()
}

/// Latest undo batch that `redo` can re-apply.
pub fn last_redoable(conn: &Connection) -> rusqlite::Result<Option<JournalBatch>> {
    conn.query_row(
        &format!("SELECT {BATCH_COLUMNS} FROM journal_batches WHERE kind = 'undo' AND undone_at IS NULL AND entries > 0 ORDER BY created_at DESC, rowid DESC LIMIT 1"),
        [],
        row_to_batch,
    )
    .optional()
}

/// Removes batches (and their entries) older than `days` (retention §3.8 "저널 90일").
pub fn purge(conn: &Connection, days: u32) -> rusqlite::Result<usize> {
    let cutoff = ts(&(Utc::now() - chrono::Duration::days(i64::from(days))));
    conn.execute("DELETE FROM journal_batches WHERE created_at < ?1", [cutoff])
}

// ---------------------------------------------------------------------------
// Undo / redo
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UndoReport {
    /// The batch that was reverted.
    pub source_batch: String,
    /// Kind of the source batch (`organize`, `delete`, `undo` …).
    pub source_kind: String,
    /// The new batch recording the reversal (itself undoable / redoable).
    pub batch_id: String,
    pub restored: u32,
    /// `(path, reason)` for entries that could not be reverted.
    pub skipped: Vec<(String, String)>,
    /// Files brought back from the workspace trash (the caller re-imports them).
    pub restored_paths: Vec<String>,
    /// Documents whose path changed.
    pub documents: Vec<String>,
}

fn move_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|e| CoreError::io(from, e))?;
    std::fs::remove_file(from).map_err(|e| CoreError::io(from, e))?;
    Ok(())
}

fn hash_matches(path: &Path, expected: Option<&str>) -> bool {
    match expected {
        None => true,
        Some(h) => blake3_file(path).map(|x| x == h).unwrap_or(false),
    }
}

/// Reverts one batch LIFO. Entries whose target file was changed, moved away
/// or whose original location is occupied again are skipped with a reason; the
/// rest are restored. The reversal is journaled as a new batch of kind
/// `undo` (or `redo` when `batch` is itself an undo), so it can be undone again.
pub fn undo_batch(db: &Db, paths: &WorkspacePaths, batch_id: &str) -> Result<UndoReport> {
    let (batch, entries) = db.with(|c| {
        let b = get_batch(c, batch_id)?;
        let e = entries(c, batch_id)?;
        Ok((b, e))
    })?;
    let batch = batch.ok_or_else(|| CoreError::NotFound(format!("journal batch {batch_id}")))?;
    if batch.undone_at.is_some() {
        return Err(CoreError::Invalid("batch_already_undone".into()));
    }
    let new_kind = if batch.kind == "undo" { "redo" } else { "undo" };
    let new_id = crate::documents::new_id();
    let label = match batch.label.as_deref() {
        Some(l) => format!("{}: {l}", if new_kind == "undo" { "되돌리기" } else { "다시 실행" }),
        None => {
            if new_kind == "undo" {
                "되돌리기".to_string()
            } else {
                "다시 실행".to_string()
            }
        }
    };
    db.with(|c| begin_batch(c, &new_id, new_kind, Some(&label), batch.source.as_deref(), None))?;
    let mut report = UndoReport {
        source_batch: batch.id.clone(),
        source_kind: batch.kind.clone(),
        batch_id: new_id.clone(),
        ..UndoReport::default()
    };
    let mut seq = 0u32;
    for e in entries.iter().rev() {
        if e.undone_at.is_some() {
            continue;
        }
        let outcome: std::result::Result<NewEntry, String> = match e.kind {
            EntryKind::Move | EntryKind::Rename => {
                let Some(to_rel) = e.to_path.as_deref() else { continue };
                let to_abs = from_rel_path(&paths.root, to_rel);
                let from_abs = from_rel_path(&paths.root, &e.from_path);
                if !to_abs.is_file() {
                    Err("missing".into())
                } else if !hash_matches(&to_abs, e.content_hash.as_deref()) {
                    Err("changed".into())
                } else if from_abs.exists() {
                    Err("target_occupied".into())
                } else {
                    match move_file(&to_abs, &from_abs) {
                        Ok(()) => {
                            if let Some(doc_id) = e.document_id.as_deref() {
                                let name = e.from_path.rsplit('/').next().unwrap_or(&e.from_path).to_string();
                                let changed = db.with(|c| {
                                    let current: Option<String> = c
                                        .query_row("SELECT rel_path FROM documents WHERE id = ?1", [doc_id], |r| {
                                            r.get(0)
                                        })
                                        .optional()?;
                                    if current.as_deref() == Some(to_rel) {
                                        docs::set_rel_path(c, doc_id, &e.from_path, &name)?;
                                        Ok(true)
                                    } else {
                                        Ok(false)
                                    }
                                })?;
                                if changed {
                                    report.documents.push(doc_id.to_string());
                                }
                            }
                            Ok(NewEntry {
                                kind: e.kind,
                                document_id: e.document_id.clone(),
                                from_path: to_rel.to_string(),
                                to_path: Some(e.from_path.clone()),
                                trash_path: None,
                                content_hash: e.content_hash.clone(),
                                size_bytes: e.size_bytes,
                            })
                        }
                        Err(err) => Err(err.to_string()),
                    }
                }
            }
            EntryKind::Delete => match e.trash_path.as_deref() {
                None => Err("os_trash".into()),
                Some(trash_rel) => {
                    let trash_abs = from_rel_path(&paths.root, trash_rel);
                    let from_abs = from_rel_path(&paths.root, &e.from_path);
                    if !trash_abs.is_file() {
                        Err("missing".into())
                    } else if from_abs.exists() {
                        Err("target_occupied".into())
                    } else {
                        match move_file(&trash_abs, &from_abs) {
                            Ok(()) => {
                                report.restored_paths.push(e.from_path.clone());
                                Ok(NewEntry {
                                    kind: EntryKind::Move,
                                    document_id: None,
                                    from_path: trash_rel.to_string(),
                                    to_path: Some(e.from_path.clone()),
                                    trash_path: None,
                                    content_hash: e.content_hash.clone(),
                                    size_bytes: e.size_bytes,
                                })
                            }
                            Err(err) => Err(err.to_string()),
                        }
                    }
                }
            },
        };
        match outcome {
            Ok(new_entry) => {
                db.with(|c| {
                    append(c, paths, &new_id, seq, &new_entry)?;
                    c.execute(
                        "UPDATE ops_journal SET undone_at = ?2, undo_error = NULL WHERE id = ?1",
                        params![e.id, ts(&Utc::now())],
                    )
                })?;
                seq += 1;
                report.restored += 1;
            }
            Err(reason) => {
                db.with(|c| c.execute("UPDATE ops_journal SET undo_error = ?2 WHERE id = ?1", params![e.id, reason]))?;
                report.skipped.push((e.to_path.clone().unwrap_or_else(|| e.from_path.clone()), reason));
            }
        }
    }
    db.with(|c| {
        c.execute(
            "UPDATE journal_batches SET undone_at = ?2, undone_by = ?3 WHERE id = ?1",
            params![batch.id, ts(&Utc::now()), new_id],
        )?;
        c.execute("UPDATE journal_batches SET redo_of = ?2 WHERE id = ?1", params![new_id, batch.id])
    })?;
    Ok(report)
}

/// Reverts the latest undoable batch (`None` when the journal has nothing to undo).
pub fn undo_last(db: &Db, paths: &WorkspacePaths) -> Result<Option<UndoReport>> {
    match db.with(last_undoable)? {
        Some(b) => undo_batch(db, paths, &b.id).map(Some),
        None => Ok(None),
    }
}

/// Re-applies the latest undo (one step, design §3.5).
pub fn redo_last(db: &Db, paths: &WorkspacePaths) -> Result<Option<UndoReport>> {
    match db.with(last_redoable)? {
        Some(b) => undo_batch(db, paths, &b.id).map(Some),
        None => Ok(None),
    }
}
