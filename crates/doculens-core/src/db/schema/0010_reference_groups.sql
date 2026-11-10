-- 0.5.0 (plan A, docs/plan/v0.5.0.md §2): a project = its work documents (`project_documents`) + one
-- reference group — a collection with kind = 'project' and project_id = the project. Collections
-- without a project are the groups of the workspace-wide 공통 참고 자료; every pre-0.5.0 collection
-- becomes one of those (kind 'reference', project_id NULL) with no data moved. `watch_folder` stays
-- as a column but is no longer read or written.
ALTER TABLE collections ADD COLUMN kind TEXT NOT NULL DEFAULT 'reference';
ALTER TABLE collections ADD COLUMN project_id TEXT REFERENCES projects(id) ON DELETE CASCADE;
CREATE INDEX IF NOT EXISTS idx_collections_project ON collections(project_id);
