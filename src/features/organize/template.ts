// Client-side twin of crates/doculens-core/src/organize/template.rs: renders the
// file-name / folder templates for the live example in the organise screen.
// The backend renders the real plan; this only has to agree with it for the
// sample document so the example the user sees matches what the preview shows.

import { TEMPLATE_VARIABLES } from "@/lib/types";

/** Placeholder for a missing value (design: "null은 `미상` + 검토 필요 표시"). */
export const UNKNOWN = "미상";
/** Windows `MAX_PATH` headroom: workspace root + relative path must stay short. */
export const MAX_PATH_CHARS = 240;
export const MAX_COMPONENT_CHARS = 120;

export type TemplateVariable = (typeof TEMPLATE_VARIABLES)[number];

export type Token = { kind: "lit"; text: string } | { kind: "var"; name: string };

/** Splits a template into literal runs and `{{var}}` tokens (unknown names stay tokens). */
export function parseTemplate(template: string): Token[] {
  const out: Token[] = [];
  let lit = "";
  let rest = template;
  for (;;) {
    const start = rest.indexOf("{{");
    if (start < 0) break;
    lit += rest.slice(0, start);
    const after = rest.slice(start + 2);
    const end = after.indexOf("}}");
    if (end < 0) {
      lit += "{{";
      rest = after;
      continue;
    }
    if (lit) {
      out.push({ kind: "lit", text: lit });
      lit = "";
    }
    out.push({ kind: "var", name: after.slice(0, end).trim() });
    rest = after.slice(end + 2);
  }
  lit += rest;
  if (lit) out.push({ kind: "lit", text: lit });
  return out;
}

/** Variable names in the template that the backend does not know (sorted, unique). */
export function unknownVariables(template: string): string[] {
  const known = TEMPLATE_VARIABLES as readonly string[];
  const out = new Set<string>();
  for (const tok of parseTemplate(template)) {
    if (tok.kind === "var" && !known.includes(tok.name)) out.add(tok.name);
  }
  return [...out].sort();
}

/** Variables used by the template, in order of appearance. */
export function templateVariables(template: string): string[] {
  return parseTemplate(template).flatMap((t) => (t.kind === "var" ? [t.name] : []));
}

const FORBIDDEN = /[<>:"/\\|?*\u0000-\u001f\u007f]/g;
const RESERVED = ["con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9"];

/** One path component made safe for Windows / NTFS and ext4 (NFC, forbidden characters → `_`, no trailing dots / spaces, reserved names prefixed, length-capped). */
export function sanitizeComponent(s: string): string {
  let out = s.normalize("NFC").replace(FORBIDDEN, "_");
  out = out.trim().replace(/[. ]+$/u, "");
  if (!out) out = "_";
  const stem = (out.split(".")[0] ?? "").toLowerCase();
  if (RESERVED.includes(stem)) out = `_${out}`;
  const chars = [...out];
  if (chars.length > MAX_COMPONENT_CHARS) out = chars.slice(0, MAX_COMPONENT_CHARS).join("");
  return out;
}

/** Sanitises every component of a `/`-separated relative folder path, dropping empty, `.` and `..` segments. */
export function sanitizeFolder(path: string): string {
  return path
    .replace(/\\/g, "/")
    .split("/")
    .map((c) => c.trim())
    .filter((c) => c !== "" && c !== "." && c !== "..")
    .map(sanitizeComponent)
    .join("/");
}

/** Renders a template; `lookup` returns the value of a variable (null / empty = unknown → 미상). */
export function renderTemplate(template: string, lookup: (name: string) => string | null | undefined): { text: string; missing: string[] } {
  let text = "";
  const missing: string[] = [];
  for (const tok of parseTemplate(template)) {
    if (tok.kind === "lit") {
      text += tok.text;
      continue;
    }
    const value = lookup(tok.name);
    if (value && value.trim()) text += sanitizeComponent(value);
    else {
      text += UNKNOWN;
      missing.push(tok.name);
    }
  }
  return { text, missing };
}

/** `[year, month, day]` (two-digit month / day, day may be "") from an ISO date such as `2026-10-14`. */
export function dateParts(iso: string): [string, string, string] | null {
  const parts = iso.split("-");
  const y = (parts[0] ?? "").trim();
  const m = (parts[1] ?? "").trim();
  const dRaw = parts.length > 2 ? parts.slice(2).join("-").trim().slice(0, 2) : "";
  if (!/^\d{4}$/.test(y) || !/^\d+$/.test(m)) return null;
  const mm = Number.parseInt(m, 10);
  const dd = /^\d+$/.test(dRaw) ? Number.parseInt(dRaw, 10) : 0;
  return [y, String(mm).padStart(2, "0"), dd === 0 ? "" : String(dd).padStart(2, "0")];
}

/** Amount formatting for file names: integer with thousands separators (`12,500`). */
export function formatTotal(amount: number): string {
  const neg = amount < 0;
  const n = Math.round(Math.abs(amount));
  const s = String(n).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return neg ? `-${s}` : s;
}

/** `name.ext` → `[stem, ext]` (no split for a leading dot, an empty stem or an extension longer than 8). */
export function splitName(name: string): [string, string] {
  const idx = name.lastIndexOf(".");
  if (idx <= 0) return [name, ""];
  const stem = name.slice(0, idx);
  const ext = name.slice(idx + 1);
  if (!ext || ext.length > 8) return [name, ""];
  return [stem, ext];
}

/** The subset of a document record the templates read. */
export interface TemplateDocument {
  original_name: string;
  title: string | null;
  /** Document type slug (`receipt`); the label comes from `typeLabel`. */
  doc_type: string | null;
  vendor: string | null;
  doc_date: string | null;
  imported_at: string;
  total_amount: number | null;
  tags: string[];
  /** Current workspace-relative folder (used when the folder template is off). */
  rel_dir: string;
}

/** Sample document behind the live example (a Starbucks receipt, like the mock's). */
export const EXAMPLE_DOCUMENT: TemplateDocument = {
  original_name: "scan_0008.jpg",
  title: null,
  doc_type: "receipt",
  vendor: "스타벅스 역삼점",
  doc_date: "2026-10-14",
  imported_at: "2026-10-15T09:00:00Z",
  total_amount: 12500,
  tags: ["경비"],
  rel_dir: "inbox",
};

export interface RenderOptions {
  /** Label of a document type slug (i18n); falls back to the slug. */
  typeLabel: (slug: string) => string | null | undefined;
  /** Running number inside the target folder (`{{seq}}`). */
  seq?: number;
}

/** Value of one template variable for a document (`null` = unknown → 미상). Mirrors `organize::variable`. */
export function documentVariable(doc: TemplateDocument, name: string, opts: RenderOptions): string | null {
  const [stem, ext] = splitName(doc.original_name.split("/").pop() ?? doc.original_name);
  const basis = doc.doc_date && doc.doc_date.trim().length >= 10 ? doc.doc_date.trim().slice(0, 10) : doc.imported_at.slice(0, 10);
  const parts = dateParts(basis);
  switch (name) {
    case "year":
      return parts?.[0] ?? null;
    case "month":
      return parts?.[1] ?? null;
    case "day":
      return parts?.[2] || null;
    case "doc_type":
      return doc.doc_type ? (opts.typeLabel(doc.doc_type) ?? doc.doc_type) : null;
    case "vendor":
      return doc.vendor && doc.vendor.trim() ? doc.vendor : null;
    case "total":
      return doc.total_amount === null ? null : formatTotal(doc.total_amount);
    case "title":
      return doc.title && doc.title.trim() ? doc.title : stem;
    case "original_name":
    case "name":
      return stem;
    case "ext":
      return ext;
    case "tags":
      return doc.tags.length ? doc.tags.join("-") : null;
    case "seq":
      return opts.seq === undefined ? null : String(opts.seq).padStart(2, "0");
    default:
      return null;
  }
}

function join(folder: string, name: string, ext: string): string {
  const file = ext ? `${name}.${ext}` : name;
  return folder ? `${folder}/${file}` : file;
}

/**
 * Renders the target path of a document the way the backend plan does: folder template (if any,
 * else the current folder), then the name template (if any, else the current stem), the extension
 * kept, the whole path capped at 240 characters. `missing` lists the variables rendered as 미상.
 */
export function renderTargetPath(nameTemplate: string | null, folderTemplate: string | null, doc: TemplateDocument, opts: RenderOptions): { path: string; missing: string[] } {
  const [stem, ext] = splitName(doc.original_name.split("/").pop() ?? doc.original_name);
  const missing: string[] = [];
  let folder = doc.rel_dir;
  if (folderTemplate !== null) {
    const r = renderTemplate(folderTemplate, (v) => documentVariable(doc, v, { typeLabel: opts.typeLabel }));
    missing.push(...r.missing);
    folder = sanitizeFolder(r.text);
  }
  let name = stem;
  if (nameTemplate !== null) {
    const r = renderTemplate(nameTemplate, (v) => documentVariable(doc, v, { typeLabel: opts.typeLabel, seq: opts.seq ?? 1 }));
    missing.push(...r.missing);
    name = sanitizeComponent(r.text);
  }
  let path = join(folder, name, ext);
  const length = [...path].length;
  if (length > MAX_PATH_CHARS) {
    const overflow = length - MAX_PATH_CHARS;
    const chars = [...name];
    name = chars.slice(0, Math.max(1, chars.length - overflow)).join("");
    path = join(folder, name, ext);
  }
  return { path, missing: [...new Set(missing)].sort() };
}

/** Inserts `text` at the caret of an input value; returns the new value and caret position. */
export function insertAtCursor(value: string, selectionStart: number | null, selectionEnd: number | null, text: string): { value: string; caret: number } {
  const start = selectionStart ?? value.length;
  const end = selectionEnd ?? start;
  return { value: value.slice(0, start) + text + value.slice(end), caret: start + text.length };
}
