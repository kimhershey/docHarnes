-- 0.3.0 (plan §5 · §6): projects are classified by their start / end dates instead of the manual
-- status board, carry per-project organise / export settings, and a document that belongs to
-- several projects is kept as a tracked physical copy in each extra project folder.
ALTER TABLE projects ADD COLUMN start_date TEXT;                     -- ISO date; NULL = the creation day
ALTER TABLE projects ADD COLUMN end_date TEXT;                       -- ISO date; NULL = open-ended (진행중)
ALTER TABLE projects ADD COLUMN settings TEXT NOT NULL DEFAULT '{}'; -- JSON (projects::ProjectSettings)
UPDATE projects SET start_date = substr(created_at, 1, 10) WHERE start_date IS NULL;
UPDATE projects SET end_date = CASE WHEN status = 'done' THEN substr(updated_at, 1, 10) ELSE due_date END
  WHERE end_date IS NULL;
DROP INDEX IF EXISTS idx_projects_status;
ALTER TABLE projects DROP COLUMN status;

-- Physical copies of a document in the folders of its further projects (the `documents` row
-- stays the single record: knowledge index, review queue and aggregates count it once).
CREATE TABLE IF NOT EXISTS document_copies (
  document_id  TEXT NOT NULL,                                        -- the original (documents.id)
  project_id   TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  rel_path     TEXT NOT NULL UNIQUE,                                 -- workspace-relative path of the copy
  content_hash TEXT NOT NULL,                                        -- blake3 of the copy as written / last synced
  size_bytes   INTEGER NOT NULL DEFAULT 0,
  state        TEXT NOT NULL DEFAULT 'synced',                       -- synced | modified | missing
  created_at   TEXT NOT NULL,
  synced_at    TEXT NOT NULL,
  PRIMARY KEY (document_id, project_id)
);
CREATE INDEX IF NOT EXISTS idx_document_copies_doc ON document_copies(document_id);
