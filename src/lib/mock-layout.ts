// Browser-mock twin of `documents::layout` (Rust): a synthetic word layout for a page's text and the
// word-range edits of the on-image text selection. The mock has no page image or PDFium, so the lines
// are laid onto the grey bars of the mock page SVG (first bar at 20 % of the height, 18 bars over 70 %).

import type { LayoutSource, PageLayout, PageWord } from "./types";

const MIN_SIZE = 0.002;
const BARS = 18;

/** Display width of a word in "units" (CJK ≈ 1, Latin / digits ≈ 0.55), as in the Rust estimator. */
export function units(word: string): number {
  let sum = 0;
  for (const ch of word) {
    const c = ch.codePointAt(0) ?? 0;
    const cjk =
      (c >= 0xac00 && c <= 0xd7a3) ||
      (c >= 0x1100 && c <= 0x11ff) ||
      (c >= 0x3130 && c <= 0x318f) ||
      (c >= 0x4e00 && c <= 0x9fff) ||
      (c >= 0x3400 && c <= 0x4dbf) ||
      (c >= 0x3000 && c <= 0x303f) ||
      (c >= 0xff00 && c <= 0xffef) ||
      (c >= 0x3040 && c <= 0x30ff);
    sum += cjk ? 1 : 0.55;
  }
  return Math.max(0.55, sum);
}

function finishBox(x: number, y: number, w: number, h: number): [number, number, number, number] {
  const bw = Math.min(1, Math.max(MIN_SIZE, w));
  const bh = Math.min(1, Math.max(MIN_SIZE, h));
  const bx = Math.min(1 - bw, Math.max(0, Number.isFinite(x) ? x : 0));
  const by = Math.min(1 - bh, Math.max(0, Number.isFinite(y) ? y : 0));
  return [bx, by, bw, bh];
}

/** Lays `words` across `[x0, x0 + width]` at `(y, h)`, proportionally to their display width. */
export function placeWords(words: string[], x0: number, width: number, y: number, h: number, line: number, edited: boolean): PageWord[] {
  if (words.length === 0) return [];
  const gap = 0.5;
  const total = words.reduce((s, w) => s + units(w), 0) + gap * (words.length - 1);
  const scale = Math.max(MIN_SIZE, width) / Math.max(0.55, total);
  let x = x0;
  return words.map((t) => {
    const ww = units(t) * scale;
    const [bx, by, bw, bh] = finishBox(x, y, ww, h);
    x += ww + gap * scale;
    const word: PageWord = { t, x: bx, y: by, w: bw, h: bh, line };
    if (edited) word.edited = true;
    return word;
  });
}

/** Visible text lines of a page text: OCR markup becomes line breaks / cell separators. */
export function textLines(text: string): string[] {
  const html = text
    .replace(/<\/?(br|tr|p|div|li|table|thead|tbody|h[1-6])\b[^>]*>/gi, "\n")
    .replace(/<\/(td|th)>/gi, " ")
    .replace(/<[^>]*>/g, "")
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'");
  return html
    .split("\n")
    .map((l) => l.split(/\s+/).filter(Boolean).join(" "))
    .filter((l) => l.length > 0);
}

/** A layout for `text` on the mock page: one text line per grey bar (lines beyond the bars stack below). */
export function synthLayout(text: string | null | undefined, source: LayoutSource): PageLayout {
  const lines = text ? textLines(text) : [];
  const pitch = 0.7 / BARS;
  const words: PageWord[] = [];
  lines.forEach((line, i) => {
    const y = 0.2 + i * pitch - pitch * 0.35;
    const len = 0.5 + ((i * 7) % 5) / 10; // the bar length pattern of `svgPage`
    words.push(...placeWords(line.split(" "), 0.1, 0.8 * len, y, pitch * 0.7, i, false));
  });
  return { version: 2, source, words };
}

/** Byte-free (UTF-16 index) spans of every word in `text`, found in order; `null` when absent. */
function locateAll(text: string, words: PageWord[]): ([number, number] | null)[] {
  let cursor = 0;
  return words.map((w) => {
    if (!w.t || cursor > text.length) return null;
    const i = text.indexOf(w.t, cursor);
    if (i < 0) return null;
    cursor = i + w.t.length;
    return [i, cursor];
  });
}

function bbox(words: PageWord[]): [number, number, number, number] {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const w of words) {
    x0 = Math.min(x0, w.x);
    y0 = Math.min(y0, w.y);
    x1 = Math.max(x1, w.x + w.w);
    y1 = Math.max(y1, w.y + w.h);
  }
  return [x0, y0, x1 - x0, y1 - y0];
}

/** The page text implied by a layout (same-line words joined by spaces, lines by newlines). */
export function layoutText(layout: PageLayout, start = 0, end = layout.words.length): string {
  let out = "";
  let line: number | null = null;
  for (const w of layout.words.slice(start, end)) {
    if (line !== null) out += line === w.line ? " " : "\n";
    out += w.t;
    line = w.line;
  }
  return out;
}

/**
 * Replaces the words `start..end` with `replacement` (whitespace splits it into words; empty deletes).
 * Returns the new page text; the layout is updated in place as in the Rust core: equal word counts
 * swap each word in place (markup between them survives), otherwise the span is replaced.
 */
export function applyEdit(layout: PageLayout, text: string, start: number, end: number, replacement: string): string {
  end = Math.min(end, layout.words.length);
  if (start >= end) return text;
  const fresh = replacement.split(/\s+/).filter(Boolean);
  const spans = locateAll(text, layout.words);
  const hit = spans.slice(start, end).filter((s): s is [number, number] => s !== null);
  const old = layout.words.slice(start, end);
  let newText: string | null = null;
  if (hit.length === end - start && fresh.length === end - start) {
    let s = text;
    for (let i = hit.length - 1; i >= 0; i -= 1) {
      const [a, b] = hit[i]!;
      s = s.slice(0, a) + fresh[i] + s.slice(b);
    }
    newText = s;
  } else if (hit.length > 0) {
    let from = hit[0]![0];
    let to = hit[hit.length - 1]![1];
    if (fresh.length === 0) {
      if (text[to] === " ") to += 1;
      else if (from > 0 && text[from - 1] === " ") from -= 1;
    }
    newText = text.slice(0, from) + fresh.join(" ") + text.slice(to);
  }
  const line = old[0]!.line;
  let words: PageWord[];
  if (fresh.length === old.length) {
    words = old.map((o, i) => ({ ...o, t: fresh[i]!, edited: true }));
  } else if (fresh.length === 0) {
    words = [];
  } else {
    const [x, y, w, h] = bbox(old);
    const sameLine = old.every((o) => o.line === line);
    words = placeWords(fresh, x, w, sameLine ? y : old[0]!.y, sameLine ? h : old[0]!.h, line, true);
  }
  layout.words.splice(start, end - start, ...words);
  return newText ?? layoutText(layout);
}

/** Adds `newText` as words over `box` in reading order; returns the new page text. */
export function addWords(layout: PageLayout, text: string, box: [number, number, number, number], newText: string): string {
  const words = newText.split(/\s+/).filter(Boolean);
  if (words.length === 0) return text;
  const [bx, by, bw, bh] = finishBox(...box);
  const centre = by + bh / 2;
  let idx = layout.words.findIndex((w) => {
    const wc = w.y + w.h / 2;
    const tol = Math.min(w.h, bh) * 0.5;
    return wc > centre + tol || (Math.abs(wc - centre) <= tol && w.x > bx);
  });
  if (idx < 0) idx = layout.words.length;
  const spans = locateAll(text, layout.words);
  const joined = words.join(" ");
  const sameLine = idx < layout.words.length && Math.abs(layout.words[idx]!.y + layout.words[idx]!.h / 2 - centre) <= Math.min(layout.words[idx]!.h, bh) * 0.5;
  let s = text;
  const at = idx < layout.words.length ? spans[idx] : null;
  if (at) s = s.slice(0, at[0]) + joined + (sameLine ? " " : "\n") + s.slice(at[0]);
  else s = s.trimEnd() ? `${s.trimEnd()}\n${joined}` : joined;
  let line: number;
  if (idx < layout.words.length) {
    line = layout.words[idx]!.line;
    if (!sameLine) for (const w of layout.words.slice(idx)) w.line += 1;
  } else {
    line = layout.words.length ? layout.words[layout.words.length - 1]!.line + 1 : 0;
  }
  layout.words.splice(idx, 0, ...placeWords(words, bx, bw, by, bh, line, true));
  return s;
}
