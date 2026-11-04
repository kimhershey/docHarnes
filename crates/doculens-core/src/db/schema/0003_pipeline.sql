-- Document pipeline (design §3.4 / §3.6): extraction schemas (built-in samples,
-- natural-language and form-derived), extraction runs, fields with confidence /
-- validation / review state, and the field change history behind "restore".

ALTER TABLE documents ADD COLUMN doc_type_source TEXT;                 -- auto | manual | form
ALTER TABLE documents ADD COLUMN classified_at   TEXT;
ALTER TABLE documents ADD COLUMN ocr_at          TEXT;
ALTER TABLE documents ADD COLUMN extracted_at    TEXT;
ALTER TABLE documents ADD COLUMN last_run_id     TEXT;
ALTER TABLE documents ADD COLUMN schema_id       TEXT;                 -- per-document schema override ("스키마 변경")
ALTER TABLE documents ADD COLUMN review_status   TEXT NOT NULL DEFAULT 'none';  -- none | pending | done
ALTER TABLE documents ADD COLUMN confidence      REAL;                 -- lowest field confidence of the latest run
ALTER TABLE documents ADD COLUMN pipeline_error  TEXT;

ALTER TABLE pages ADD COLUMN text_source   TEXT;                       -- layer | converted | ocr
ALTER TABLE pages ADD COLUMN ocr_at        TEXT;
ALTER TABLE pages ADD COLUMN ocr_profile   TEXT;
ALTER TABLE pages ADD COLUMN ocr_model     TEXT;
ALTER TABLE pages ADD COLUMN classified_at TEXT;
ALTER TABLE pages ADD COLUMN page_doc_type TEXT;
ALTER TABLE pages ADD COLUMN page_language TEXT;
ALTER TABLE pages ADD COLUMN page_rotation INTEGER;                    -- rotation the classifier reported (0/90/180/270)

UPDATE pages SET text_source = 'layer' WHERE has_text = 1 AND text_source IS NULL;

CREATE TABLE IF NOT EXISTS schemas (
  id               TEXT PRIMARY KEY NOT NULL,
  name             TEXT NOT NULL,
  name_en          TEXT,
  doc_type         TEXT NOT NULL,              -- slug used by the classifier enum
  source           TEXT NOT NULL,              -- builtin | nl | user | form
  version          INTEGER NOT NULL DEFAULT 1,
  description_ko   TEXT,
  description_en   TEXT,
  definition       TEXT NOT NULL,              -- JSON (pipeline::schema::SchemaDef)
  form_document_id TEXT REFERENCES documents(id) ON DELETE SET NULL,
  builtin_key      TEXT,                       -- stable key of a bundled sample form
  enabled          INTEGER NOT NULL DEFAULT 1,
  is_default       INTEGER NOT NULL DEFAULT 1, -- the schema `doc_type` maps to (one per type)
  use_count        INTEGER NOT NULL DEFAULT 0,
  note             TEXT,                       -- change note of the current version
  created_at       TEXT NOT NULL,
  updated_at       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_schemas_doc_type ON schemas(doc_type);

CREATE TABLE IF NOT EXISTS schema_versions (
  schema_id  TEXT NOT NULL REFERENCES schemas(id) ON DELETE CASCADE,
  version    INTEGER NOT NULL,
  definition TEXT NOT NULL,
  note       TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY (schema_id, version)
);

CREATE TABLE IF NOT EXISTS extraction_runs (
  id                TEXT PRIMARY KEY NOT NULL,
  document_id       TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  schema_id         TEXT,
  schema_version    INTEGER,
  doc_type          TEXT,
  model             TEXT,
  prompt_version    TEXT,
  profile           TEXT,
  precise           INTEGER NOT NULL DEFAULT 0,
  status            TEXT NOT NULL,             -- running | done | failed | cancelled
  error             TEXT,
  started_at        TEXT NOT NULL,
  finished_at       TEXT,
  duration_ms       INTEGER,
  prompt_tokens     INTEGER NOT NULL DEFAULT 0,
  completion_tokens INTEGER NOT NULL DEFAULT 0,
  pages_done        INTEGER NOT NULL DEFAULT 0,
  result_json       TEXT,                      -- merged raw model output
  confidence        REAL,
  field_count       INTEGER NOT NULL DEFAULT 0,
  review_count      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_runs_doc ON extraction_runs(document_id, started_at);

CREATE TABLE IF NOT EXISTS fields (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id           TEXT NOT NULL REFERENCES extraction_runs(id) ON DELETE CASCADE,
  document_id      TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  path             TEXT NOT NULL,              -- "supplier_brn" or "items[2].amount"
  key              TEXT NOT NULL,
  row              INTEGER,                    -- line-item row (NULL for scalar fields)
  label_ko         TEXT,
  label_en         TEXT,
  kind             TEXT NOT NULL,              -- pipeline::schema::FieldKind
  required         INTEGER NOT NULL DEFAULT 0,
  value            TEXT,                       -- current value (masked when PII)
  normalized       TEXT,                       -- ISO date / plain number / digits
  original_value   TEXT,                       -- model output before any correction
  page             INTEGER,
  bbox             TEXT,                       -- JSON [x0, y0, x1, y1] in 0..1000, NULL = page-level
  evidence         TEXT,
  confidence       REAL NOT NULL DEFAULT 0,
  model_confidence INTEGER,
  status           TEXT NOT NULL DEFAULT 'auto',  -- auto | approved | corrected | rejected | held
  review_reason    TEXT,                       -- low_confidence | validation_failed | required_missing | audit
  validation       TEXT,                       -- JSON [{rule, level, message}]
  reviewed_at      TEXT
);
CREATE INDEX IF NOT EXISTS idx_fields_run ON fields(run_id);
CREATE INDEX IF NOT EXISTS idx_fields_doc ON fields(document_id);
CREATE INDEX IF NOT EXISTS idx_fields_review ON fields(review_reason, status);

CREATE TABLE IF NOT EXISTS field_history (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  field_id    INTEGER NOT NULL REFERENCES fields(id) ON DELETE CASCADE,
  document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  path        TEXT NOT NULL,
  old_value   TEXT,
  new_value   TEXT,
  old_status  TEXT NOT NULL,
  new_status  TEXT NOT NULL,
  changed_at  TEXT NOT NULL,
  note        TEXT
);
CREATE INDEX IF NOT EXISTS idx_field_history_doc ON field_history(document_id, changed_at);
