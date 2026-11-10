-- 0.7.0 (docs/plan/v0.7.0.md §1.6): the AI assistant's conversations are kept per workspace as
-- chat sessions. A message's `payload` is the UI's own JSON (user text, plan / answer / result
-- cards …) and is stored as is; the core only derives the automatic session title from the first
-- user message. Sessions expire with the "대화 기록" retention item (maintenance purge).
CREATE TABLE IF NOT EXISTS chat_sessions (
  id          TEXT PRIMARY KEY,
  title       TEXT NOT NULL DEFAULT '',
  title_auto  INTEGER NOT NULL DEFAULT 1,
  created_at  TEXT NOT NULL,
  updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chat_sessions_updated ON chat_sessions(updated_at);

CREATE TABLE IF NOT EXISTS chat_messages (
  session_id  TEXT NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
  seq         INTEGER NOT NULL,
  payload     TEXT NOT NULL,
  created_at  TEXT NOT NULL,
  PRIMARY KEY (session_id, seq)
);
