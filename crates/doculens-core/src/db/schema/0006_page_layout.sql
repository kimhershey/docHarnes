-- Text layer positions (viewer "직접 수정" on the page image, 2026-11-09): the words of a page's
-- text with their boxes as fractions of the rendered page image (JSON, see documents::layout).
-- Computed lazily from PDFium character boxes or estimated from the page image; edits persist here.
ALTER TABLE pages ADD COLUMN layout TEXT;
