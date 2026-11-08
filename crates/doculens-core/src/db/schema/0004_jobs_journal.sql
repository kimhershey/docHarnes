-- Phase 5: the file-operation journal behind undo / redo (design §3.5
-- "되돌리기", §3.6 ops_journal) and the persistent job queue (design §3.9
-- scheduling: priority queue, pause / resume, retry across restarts).

CREATE TABLE IF NOT EXISTS journal_batches (
  id          TEXT PRIMARY KEY NOT NULL,
  kind        TEXT NOT NULL,                 -- organize | rename | move | delete | undo | redo
  label       TEXT,                          -- human summary shown in the batch history
  source      TEXT,                          -- planner | organize | library | viewer
  plan_json   TEXT,                          -- request that produced the batch (diagnostics)
  created_at  TEXT NOT NULL,
  entries     INTEGER NOT NULL DEFAULT 0,
  undone_at   TEXT,
  undone_by   TEXT,                          -- id of the undo batch that reverted this one
  redo_of     TEXT                           -- undo batches: the batch they reverted
);

CREATE TABLE IF NOT EXISTS ops_journal (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  batch_id     TEXT NOT NULL REFERENCES journal_batches(id) ON DELETE CASCADE,
  seq          INTEGER NOT NULL,
  kind         TEXT NOT NULL,                -- move | rename | delete
  document_id  TEXT,                         -- no FK: survives document deletion
  from_path    TEXT NOT NULL,                -- workspace-relative, '/' separators
  to_path      TEXT,                         -- NULL for deletions
  trash_path   TEXT,                         -- workspace trash fallback location (deletions)
  content_hash TEXT,                         -- blake3 of the file before the operation
  size_bytes   INTEGER,
  created_at   TEXT NOT NULL,
  undone_at    TEXT,
  undo_error   TEXT                          -- why the entry could not be reverted
);
CREATE INDEX IF NOT EXISTS idx_journal_batch ON ops_journal(batch_id, seq);
CREATE INDEX IF NOT EXISTS idx_journal_created ON ops_journal(created_at);

CREATE TABLE IF NOT EXISTS jobs (
  id          TEXT PRIMARY KEY NOT NULL,
  kind        TEXT NOT NULL,                 -- pipeline | plan | export | organize | undo | redo
  subject     TEXT NOT NULL,
  spec        TEXT,                          -- JSON (jobs::JobSpec), replayable after a restart
  priority    INTEGER NOT NULL DEFAULT 1,    -- 0 low · 1 normal · 2 high · 3 interactive
  state       TEXT NOT NULL,                 -- queued | running | paused | done | failed | cancelled | interrupted
  progress    REAL,
  detail      TEXT,
  error       TEXT,
  result      TEXT,                          -- JSON result (plan execution, export path …)
  attempts    INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL,
  updated_at  TEXT NOT NULL,
  started_at  TEXT,
  finished_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_jobs_state ON jobs(state, priority, created_at);
