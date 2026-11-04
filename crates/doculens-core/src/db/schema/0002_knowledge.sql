-- Knowledge base (design §3.14): chunks + morpheme/trigram full-text indexes,
-- collections, document versions, glossary and saved answers. Vectors live in
-- a sqlite-vec `chunks_vec` virtual table that is created lazily once the
-- embedding dimension is known (knowledge::embed::ensure_vec_table).

ALTER TABLE documents ADD COLUMN indexed_at    TEXT;              -- last successful chunking
ALTER TABLE documents ADD COLUMN index_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE documents ADD COLUMN index_error   TEXT;
ALTER TABLE documents ADD COLUMN text_hash     TEXT;              -- blake3 of the page texts that were chunked
ALTER TABLE documents ADD COLUMN chunk_count   INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS collections (
  id           TEXT PRIMARY KEY NOT NULL,
  name         TEXT NOT NULL UNIQUE,
  description  TEXT,
  color        TEXT,
  active       INTEGER NOT NULL DEFAULT 1,      -- inactive collections are excluded from default search scope
  watch_folder TEXT,                            -- workspace-relative folder synced into this collection
  created_at   TEXT NOT NULL,
  updated_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS collection_docs (
  collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
  document_id   TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  added_at      TEXT NOT NULL,
  PRIMARY KEY (collection_id, document_id)
);
CREATE INDEX IF NOT EXISTS idx_collection_docs_doc ON collection_docs(document_id);

CREATE TABLE IF NOT EXISTS doc_versions (
  document_id    TEXT PRIMARY KEY NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  group_key      TEXT NOT NULL,                 -- documents with the same key are editions of one title
  version_label  TEXT,
  effective_date TEXT,
  is_latest      INTEGER NOT NULL DEFAULT 1,
  note           TEXT,
  updated_at     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_doc_versions_group ON doc_versions(group_key);

CREATE TABLE IF NOT EXISTS chunks (
  id             INTEGER PRIMARY KEY AUTOINCREMENT, -- never reused: FTS / vec rows and citations key on it
  document_id    TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  chunk_index    INTEGER NOT NULL,
  page_start     INTEGER NOT NULL,
  page_end       INTEGER NOT NULL,
  kind           TEXT NOT NULL DEFAULT 'text',  -- text | table | heading | list
  heading_path   TEXT,                          -- "제3장 복무 > 제12조 출장"
  clause         TEXT,                          -- "제12조 제2항"
  text           TEXT NOT NULL,
  tokens         TEXT NOT NULL,                 -- morpheme tokens (space separated) indexed by chunks_fts
  token_estimate INTEGER NOT NULL,
  text_hash      TEXT NOT NULL,
  embedded_at    TEXT,                          -- set when chunks_vec holds this chunk's vector
  UNIQUE (document_id, chunk_index)
);
CREATE INDEX IF NOT EXISTS idx_chunks_doc ON chunks(document_id);
CREATE INDEX IF NOT EXISTS idx_chunks_embedded ON chunks(embedded_at);

-- External-content FTS tables: `tokens` is pre-analysed by lindera (ko-dic) or
-- the built-in fallback, so unicode61 only splits on whitespace; the trigram
-- index covers substrings (numbers, codes, inflected forms) of the raw text.
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
  tokens, content='chunks', content_rowid='id', tokenize='unicode61 remove_diacritics 0'
);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts_tri USING fts5(
  text, content='chunks', content_rowid='id', tokenize='trigram'
);
CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
  INSERT INTO chunks_fts(rowid, tokens) VALUES (new.id, new.tokens);
  INSERT INTO chunks_fts_tri(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
  INSERT INTO chunks_fts(chunks_fts, rowid, tokens) VALUES ('delete', old.id, old.tokens);
  INSERT INTO chunks_fts_tri(chunks_fts_tri, rowid, text) VALUES ('delete', old.id, old.text);
END;

CREATE TABLE IF NOT EXISTS glossary (
  id         INTEGER PRIMARY KEY,
  term       TEXT NOT NULL UNIQUE COLLATE NOCASE,
  definition TEXT,
  synonyms   TEXT NOT NULL DEFAULT '[]',        -- JSON array of strings
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS answers (
  id            INTEGER PRIMARY KEY,
  kind          TEXT NOT NULL DEFAULT 'answer', -- answer | explain | summary | compare
  question      TEXT NOT NULL,
  answer        TEXT NOT NULL,
  scope         TEXT NOT NULL,                  -- JSON SearchScope
  citations     TEXT NOT NULL,                  -- JSON [Citation]
  sentences     TEXT NOT NULL,                  -- JSON [AnswerSentence]
  model         TEXT,
  no_evidence   INTEGER NOT NULL DEFAULT 0,
  used_general  INTEGER NOT NULL DEFAULT 0,
  document_id   TEXT REFERENCES documents(id) ON DELETE SET NULL,
  feedback      INTEGER,                        -- 1 helpful, -1 wrong
  feedback_note TEXT,
  created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_answers_created ON answers(created_at);
CREATE INDEX IF NOT EXISTS idx_answers_doc ON answers(document_id);
