import type { EditReplacement, PageTextEdit } from "@/lib/types";

/**
 * The corrections made in the viewer as replacements for the original file (1.0.0 §2-1 step 3): a word that
 * was put right on the page — the page text is what search and questions read — is still wrong in the
 * document's own file, and "before → after" of the correction is the replacement that puts it right there.
 *
 * `entries` are newest first (`documents_page_text_history`); the result is oldest first, so that a word
 * corrected twice (A → B, then B → C) is replaced in the same order, and holds each pair once. Left out:
 * corrections that were taken back, words that were only added (nothing to find), and corrections that span
 * lines (a replacement stays inside one paragraph).
 */
export function correctionsOf(entries: PageTextEdit[]): EditReplacement[] {
  const seen = new Set<string>();
  const out: EditReplacement[] = [];
  for (const e of [...entries].reverse()) {
    const find = e.before.trim();
    const replace = e.after.trim();
    if (e.undone || find === "" || find === replace || /[\r\n]/.test(find) || /[\r\n]/.test(replace)) continue;
    const key = `${find}\u0000${replace}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push({ find, replace });
  }
  return out;
}
