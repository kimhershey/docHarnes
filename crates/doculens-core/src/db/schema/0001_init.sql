-- DocuLens workspace database, schema version 1 (design §3.6).
-- Paths are stored relative to the workspace root with '/' separators.

CREATE TABLE IF NOT EXISTS documents (
  id               TEXT PRIMARY KEY NOT NULL,
  rel_path         TEXT NOT NULL UNIQUE,
  original_name    TEXT NOT NULL,
  source_kind      TEXT NOT NULL,                 -- pdf | image | hwpx | docx | hwp
  content_hash     TEXT NOT NULL,                 -- blake3 of the source file
  size_bytes       INTEGER NOT NULL DEFAULT 0,
  file_modified_at TEXT,
  imported_at      TEXT NOT NULL,
  updated_at       TEXT NOT NULL,
  status           TEXT NOT NULL DEFAULT 'importing',  -- importing | new | rendered | extracted | reviewed | archived | missing | error
  role             TEXT NOT NULL DEFAULT 'work',       -- work | knowledge | form | generated
  title            TEXT,
  doc_type         TEXT,
  vendor           TEXT,
  doc_date         TEXT,
  total_amount     REAL,
  language         TEXT,
  page_count       INTEGER NOT NULL DEFAULT 0,
  has_text_layer   INTEGER NOT NULL DEFAULT 0,
  converted        INTEGER NOT NULL DEFAULT 0,    -- 1 when pages come from a converted PDF (hwpx/docx/hwp)
  conversion_note  TEXT,
  render_dpi       INTEGER,
  error            TEXT,
  import_batch_id  TEXT,
  deleted_at       TEXT
);
CREATE INDEX IF NOT EXISTS idx_documents_hash ON documents(content_hash);
CREATE INDEX IF NOT EXISTS idx_documents_status ON documents(status);
CREATE INDEX IF NOT EXISTS idx_documents_imported ON documents(imported_at);

CREATE TABLE IF NOT EXISTS pages (
  document_id  TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  page_no      INTEGER NOT NULL,                  -- 1-based
  width_px     INTEGER NOT NULL DEFAULT 0,
  height_px    INTEGER NOT NULL DEFAULT 0,
  width_pt     REAL,
  height_pt    REAL,
  dpi          INTEGER,
  rotation     INTEGER NOT NULL DEFAULT 0,        -- user rotation applied to the cached render (0/90/180/270)
  has_text     INTEGER NOT NULL DEFAULT 0,
  text         TEXT,                              -- text layer (digital PDF / converted documents)
  is_blank     INTEGER,
  rendered_at  TEXT,
  PRIMARY KEY (document_id, page_no)
);

CREATE TABLE IF NOT EXISTS tags (
  id         INTEGER PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
  color      TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS document_tags (
  document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  tag_id      INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY (document_id, tag_id)
);

CREATE TABLE IF NOT EXISTS import_batches (
  id          TEXT PRIMARY KEY NOT NULL,
  source      TEXT NOT NULL,                      -- drop | folder | inbox | onboarding | rescan
  started_at  TEXT NOT NULL,
  finished_at TEXT,
  total       INTEGER NOT NULL DEFAULT 0,
  imported    INTEGER NOT NULL DEFAULT 0,
  duplicates  INTEGER NOT NULL DEFAULT 0,
  unsupported INTEGER NOT NULL DEFAULT 0,
  failed      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS kv (
  key        TEXT PRIMARY KEY NOT NULL,
  value      TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
