-- 0.5.0 (plan A, follow-up to 0010): a project's reference group carries the project's name even when
-- a group of the 공통 참고 자료 has the same name, so the table-wide UNIQUE(name) of 0002 goes away
-- (uniqueness among the common groups is enforced case-insensitively by `save_collection`; one group
-- per project by `project_reference_group`). SQLite cannot drop an inline constraint, so the table is
-- rebuilt. `collection_docs` is rebuilt with it: a RENAME re-points its foreign key at the old table and
-- dropping that table would then cascade through the memberships.
ALTER TABLE collections RENAME TO collections_old;
CREATE TABLE collections (
  id           TEXT PRIMARY KEY NOT NULL,
  name         TEXT NOT NULL,
  description  TEXT,
  color        TEXT,
  active       INTEGER NOT NULL DEFAULT 1,
  watch_folder TEXT,                            -- unused since 0.5.0 (kept for older builds)
  kind         TEXT NOT NULL DEFAULT 'reference', -- 'reference' (공통 참고 자료) | 'project'
  project_id   TEXT REFERENCES projects(id) ON DELETE CASCADE,
  created_at   TEXT NOT NULL,
  updated_at   TEXT NOT NULL
);
INSERT INTO collections (id, name, description, color, active, watch_folder, kind, project_id, created_at, updated_at)
  SELECT id, name, description, color, active, watch_folder, kind, project_id, created_at, updated_at FROM collections_old;
CREATE TABLE collection_docs_new (
  collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
  document_id   TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  added_at      TEXT NOT NULL,
  PRIMARY KEY (collection_id, document_id)
);
INSERT INTO collection_docs_new (collection_id, document_id, added_at)
  SELECT collection_id, document_id, added_at FROM collection_docs;
DROP TABLE collection_docs;
ALTER TABLE collection_docs_new RENAME TO collection_docs;
DROP TABLE collections_old;
CREATE INDEX IF NOT EXISTS idx_collection_docs_doc ON collection_docs(document_id);
CREATE INDEX IF NOT EXISTS idx_collections_project ON collections(project_id);
CREATE INDEX IF NOT EXISTS idx_collections_name ON collections(name COLLATE NOCASE);
