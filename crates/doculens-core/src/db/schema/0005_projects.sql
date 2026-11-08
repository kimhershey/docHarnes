-- Phase 6: projects (design §3.15 프로젝트 단위 다중 문서 분석·정합성 검증):
-- document groups (심사 건·거래처 건) with a template checklist of required
-- document types, consistency rules (rule JSON), check runs and their
-- per-rule results with evidence.

CREATE TABLE IF NOT EXISTS project_templates (
  id                 TEXT PRIMARY KEY NOT NULL,
  name_ko            TEXT NOT NULL,
  name_en            TEXT NOT NULL,
  description_ko     TEXT,
  description_en     TEXT,
  required_doc_types TEXT NOT NULL DEFAULT '[]',  -- JSON array of doc-type slugs
  rules              TEXT NOT NULL DEFAULT '[]',  -- JSON array (projects::Rule)
  builtin            INTEGER NOT NULL DEFAULT 0,  -- 1 = bundled (§3.15 examples), refreshed while unedited
  created_at         TEXT NOT NULL,
  updated_at         TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
  id            TEXT PRIMARY KEY NOT NULL,
  name          TEXT NOT NULL,
  template_id   TEXT,                          -- no FK: a deleted template leaves the project intact
  status        TEXT NOT NULL DEFAULT 'intake', -- intake | extracting | validating | supplement | done
  intake_no     TEXT,                          -- 접수번호
  owner         TEXT,                          -- 담당
  due_date      TEXT,                          -- 기한 (ISO date)
  notes         TEXT,
  inbox_folder  TEXT,                          -- inbox/<folder> auto-mapping source
  created_at    TEXT NOT NULL,
  updated_at    TEXT NOT NULL,
  last_check_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_projects_status ON projects(status, updated_at);
CREATE INDEX IF NOT EXISTS idx_projects_inbox ON projects(inbox_folder);

CREATE TABLE IF NOT EXISTS project_documents (
  project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  document_id TEXT NOT NULL,                   -- no FK: history survives document deletion
  added_at    TEXT NOT NULL,
  PRIMARY KEY (project_id, document_id)
);
CREATE INDEX IF NOT EXISTS idx_project_documents_doc ON project_documents(document_id);

CREATE TABLE IF NOT EXISTS project_rules (
  id          TEXT PRIMARY KEY NOT NULL,
  project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  rule        TEXT NOT NULL,                   -- JSON (projects::Rule)
  position    INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL,
  updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_project_rules_project ON project_rules(project_id, position);

CREATE TABLE IF NOT EXISTS project_runs (
  id                TEXT PRIMARY KEY NOT NULL,
  project_id        TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  started_at        TEXT NOT NULL,
  finished_at       TEXT NOT NULL,
  pass              INTEGER NOT NULL DEFAULT 0,
  mismatch          INTEGER NOT NULL DEFAULT 0,
  undetermined      INTEGER NOT NULL DEFAULT 0,
  missing           INTEGER NOT NULL DEFAULT 0,
  missing_doc_types TEXT NOT NULL DEFAULT '[]' -- JSON array of slugs absent at check time
);
CREATE INDEX IF NOT EXISTS idx_project_runs_project ON project_runs(project_id, started_at);

CREATE TABLE IF NOT EXISTS project_rule_results (
  id          TEXT PRIMARY KEY NOT NULL,
  run_id      TEXT NOT NULL REFERENCES project_runs(id) ON DELETE CASCADE,
  rule_id     TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',        -- rule text at check time (rules may change later)
  verdict     TEXT NOT NULL,                   -- pass | mismatch | undetermined | missing
  required    INTEGER NOT NULL DEFAULT 1,
  left_json   TEXT,                            -- JSON (projects::Evidence)
  right_json  TEXT,                            -- JSON (projects::Evidence)
  message     TEXT NOT NULL DEFAULT '',
  checked_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_project_rule_results_run ON project_rule_results(run_id);
