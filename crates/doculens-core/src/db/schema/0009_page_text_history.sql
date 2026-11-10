-- Page text edit history (0.3.0 item 7, design §3.7 "직접 수정"): every on-image / manual correction
-- of a page's text records the page state before and after (text, word layout JSON, text source), so
-- the viewer can undo / redo a correction (Ctrl+Z scoped to the open page) and the 이력 tab can take a
-- page back to an earlier state. `kind` is 'edit' for a correction (`source` = layer | add | manual) and
-- 'revert' / 'redo' for the undo bookkeeping (`target_id` = the edit undone / re-applied). Original
-- files are never touched; the undo history follows the journal retention.
CREATE TABLE IF NOT EXISTS page_text_history (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  document_id     TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  page_no         INTEGER NOT NULL,
  kind            TEXT NOT NULL,
  source          TEXT NOT NULL,
  target_id       INTEGER,
  previous_text   TEXT,
  previous_layout TEXT,
  previous_source TEXT,
  new_text        TEXT,
  new_layout      TEXT,
  new_source      TEXT,
  edited_at       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_page_text_history_page ON page_text_history(document_id, page_no, id);
