// Pure helpers of the on-image text layer (`PageTextLayer`): selection text, hit testing, search
// highlights over the positioned words of `PageLayout`.

import type { PageLayout, PageWord } from "@/lib/types";

/** Text of the selected words: same-line neighbours joined by spaces, line changes by newlines. */
export function selectionText(layout: PageLayout | null | undefined, indices: number[]): string {
  if (!layout) return "";
  let out = "";
  let line: number | null = null;
  for (const i of indices) {
    const w = layout.words[i];
    if (!w) continue;
    if (line !== null) out += line === w.line ? " " : "\n";
    out += w.t;
    line = w.line;
  }
  return out;
}

/** True when `indices` (sorted) are consecutive word positions — the only shape an edit accepts. */
export function isContiguous(indices: number[]): boolean {
  if (indices.length === 0) return false;
  for (let i = 1; i < indices.length; i += 1) if (indices[i] !== indices[i - 1]! + 1) return false;
  return true;
}

export function range(a: number, b: number): number[] {
  const lo = Math.min(a, b);
  const hi = Math.max(a, b);
  return Array.from({ length: hi - lo + 1 }, (_, i) => lo + i);
}

/** Bounding box `[x, y, w, h]` (fractions) of the given words. */
export function bbox(layout: PageLayout, indices: number[]): [number, number, number, number] | null {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const i of indices) {
    const w = layout.words[i];
    if (!w) continue;
    x0 = Math.min(x0, w.x);
    y0 = Math.min(y0, w.y);
    x1 = Math.max(x1, w.x + w.w);
    y1 = Math.max(y1, w.y + w.h);
  }
  return Number.isFinite(x0) ? [x0, y0, x1 - x0, y1 - y0] : null;
}

const PAD = 0.003;

function inside(w: PageWord, x: number, y: number): boolean {
  return x >= w.x - PAD && x <= w.x + w.w + PAD && y >= w.y - PAD && y <= w.y + w.h + PAD;
}

/**
 * The word under the point (fractions), or — like text selection — the nearest word of the nearest
 * line when the point falls between words. `null` only for an empty layout.
 */
export function wordAt(layout: PageLayout, x: number, y: number, nearest = true): number | null {
  const direct = layout.words.findIndex((w) => inside(w, x, y));
  if (direct >= 0) return direct;
  if (!nearest || layout.words.length === 0) return null;
  let bestLine: number | null = null;
  let bestDist = Infinity;
  for (const w of layout.words) {
    const d = Math.abs(w.y + w.h / 2 - y);
    if (d < bestDist) {
      bestDist = d;
      bestLine = w.line;
    }
  }
  let best = -1;
  let bestDx = Infinity;
  layout.words.forEach((w, i) => {
    if (w.line !== bestLine) return;
    const dx = x < w.x ? w.x - x : x > w.x + w.w ? x - (w.x + w.w) : 0;
    if (dx < bestDx) {
      bestDx = dx;
      best = i;
    }
  });
  return best >= 0 ? best : null;
}

/** Indices of the words whose boxes intersect the rectangle (fractions), in reading order. */
export function wordsInRect(layout: PageLayout, rect: [number, number, number, number]): number[] {
  const [rx, ry, rw, rh] = rect;
  const out: number[] = [];
  layout.words.forEach((w, i) => {
    if (w.x < rx + rw && w.x + w.w > rx && w.y < ry + rh && w.y + w.h > ry) out.push(i);
  });
  return out;
}

/**
 * Words touched by any of the `needles` (case-insensitive, matched against each line's text with
 * single spaces between words — so a phrase spanning words highlights all of them).
 */
export function matchWords(layout: PageLayout | null | undefined, needles: string[]): Set<number> {
  const hits = new Set<number>();
  if (!layout) return hits;
  const terms = needles.map((n) => n.trim().toLowerCase()).filter((n) => n.length > 0);
  if (terms.length === 0) return hits;
  // Build the line strings with the [start, end) offsets of each word.
  const lines = new Map<number, { text: string; spans: { index: number; start: number; end: number }[] }>();
  layout.words.forEach((w, index) => {
    let line = lines.get(w.line);
    if (!line) {
      line = { text: "", spans: [] };
      lines.set(w.line, line);
    } else {
      line.text += " ";
    }
    const start = line.text.length;
    line.text += w.t;
    line.spans.push({ index, start, end: line.text.length });
  });
  for (const line of lines.values()) {
    const lower = line.text.toLowerCase();
    for (const term of terms) {
      let from = 0;
      while (from <= lower.length) {
        const at = lower.indexOf(term, from);
        if (at < 0) break;
        const end = at + term.length;
        for (const s of line.spans) if (s.start < end && s.end > at) hits.add(s.index);
        from = end;
      }
    }
  }
  return hits;
}

/** Display width of a word in "units" (CJK ≈ 1, Latin / digits ≈ 0.55) — the Rust `layout::units`. */
export function displayUnits(text: string): number {
  let n = 0;
  for (const ch of text) {
    const c = ch.codePointAt(0) ?? 0;
    const wide =
      (c >= 0xac00 && c <= 0xd7a3) ||
      (c >= 0x1100 && c <= 0x11ff) ||
      (c >= 0x3130 && c <= 0x318f) ||
      (c >= 0x4e00 && c <= 0x9fff) ||
      (c >= 0x3400 && c <= 0x4dbf) ||
      (c >= 0x3000 && c <= 0x303f) ||
      (c >= 0xff00 && c <= 0xffef) ||
      (c >= 0x3040 && c <= 0x30ff);
    n += wide ? 1 : 0.55;
  }
  return Math.max(0.55, n);
}

/**
 * Font size (px) that fits `text` into a `w × h` px box: set by the height, shrunk until the text
 * also fits the width (a corrected word drawn visibly over the page image), never below 6 px.
 */
export function fitFontSize(text: string, w: number, h: number): number {
  const byHeight = h * 0.78;
  const byWidth = w / displayUnits(text);
  return Math.max(6, Math.min(byHeight, byWidth));
}
