-- Classification keywords of a project (2026-11-09, 프로젝트 자동 분류): a JSON array of strings
-- matched against document titles / file names / text when library or knowledge-base documents
-- are sorted into projects (projects::classify).
ALTER TABLE projects ADD COLUMN keywords TEXT NOT NULL DEFAULT '[]';
