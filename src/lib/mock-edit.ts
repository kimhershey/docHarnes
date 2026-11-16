// Browser-mode stand-in for the core's format-preserving edit (1.0.0 §2-1, `documents::edit`): the mock has
// no files, so a replacement runs over the page text of the document — as it was before any correction made
// in the viewer, which is what the file still holds —, one line = one paragraph (a workbook: one line = one
// row of the sheet, cells separated by tabs), and every document has the same three pictures. The rules
// are the core's — which formats, what a request may hold, how occurrences are counted, how the edited copy
// is named and registered as the latest edition next to its original.
import { emit, err, upsertDoc, wsDocs, type MockDoc } from "./mock-backend";
import { EVENTS, type DocumentsChanged, type EditChange, type EditPicture, type EditReplacement, type EditReport, type EditSuggestion, type Edition, type VersionGroup } from "./types";

const EDITABLE = ["hwpx", "docx", "pptx", "xlsx"];
/** Paragraphs and table rows are a document's, cells a workbook's (core `Action::structural`). */
const STRUCTURAL = ["insert_after", "insert_before", "delete_paragraph", "insert_row_after", "insert_row_before", "delete_row"];
const MAX_EDITS = 200;
const MAX_CHANGES = 300;
const CONTEXT = 40;

function around(text: string, from: number, to: number): string {
  const head = Array.from(text.slice(0, from));
  const tail = Array.from(text.slice(to));
  return `${head.length > CONTEXT ? "…" : ""}${head.slice(-CONTEXT).join("")}${text.slice(from, to)}${tail.slice(0, CONTEXT).join("")}${tail.length > CONTEXT ? "…" : ""}`;
}

function partOf(d: MockDoc, page: number): { part: string; unit: number | null } {
  if (d.source_kind === "pptx") return { part: `ppt/slides/slide${page}.xml`, unit: page };
  // A workbook's texts are its shared strings, whichever sheet shows them.
  if (d.source_kind === "xlsx") return { part: "xl/sharedStrings.xml", unit: null };
  if (d.source_kind === "hwpx") return { part: "Contents/section0.xml", unit: 1 };
  return { part: "word/document.xml", unit: null };
}

/** The file holds what a page read before the viewer's corrections: they change the page text, not the file. */
function fileTexts(d: MockDoc): (string | null)[] {
  return d.pages.map((p) => {
    const first = (d.text_history ?? []).find((row) => row.page_no === p.page_no);
    return first ? first.previous.text : p.text;
  });
}

/** Whether the mock has a loaded model (`mock-pipeline.ts`: `doculens.mock.vision`). */
function modelLoaded(): boolean {
  try {
    const mode = localStorage.getItem("doculens.mock.vision");
    return !(mode === "not_loaded" || mode === "none" || mode === "engine_off");
  } catch {
    return true;
  }
}

/**
 * `documents_edit_suggest` (core `documents::edit::suggest`). The mock has no model: an instruction of the
 * shape "A를 B로 바꿔줘" / "replace A with B" names the text itself, and the first paragraph that holds it is
 * the place — pinned by `occurrence`, as the core pins the place the model points at.
 */
function suggest(d: MockDoc, instruction: string): EditSuggestion {
  if (!instruction.trim()) throw err("invalid", "description_empty");
  if (!EDITABLE.includes(d.source_kind)) throw err("invalid", "edit_unsupported_format");
  if (!modelLoaded()) throw err("invalid", "no_model");
  const paragraphs = fileTexts(d)
    .flatMap((text) => (text ?? "").split("\n"))
    .filter((line) => line.trim() !== "");
  const counts = { shown: paragraphs.length, paragraphs: paragraphs.length };
  const said = instruction.match(/^(.+?)[을를]\s+(.+?)(?:으로|로)\s*(?:바꿔|고쳐|수정|변경)/u) ?? instruction.match(/^replace\s+(.+?)\s+with\s+(.+?)[.!]?$/i);
  if (!said) return { edits: [], dropped: 0, note: "어느 글을 무엇으로 바꿀지 문장에서 찾지 못했습니다.", ...counts };
  const [find, replace] = [said[1]!.trim(), said[2]!.trim()];
  if (!paragraphs.some((line) => line.includes(find))) return { edits: [], dropped: 1, note: "", ...counts };
  return { edits: [{ find, replace, occurrence: 1 }], dropped: 0, note: "", ...counts };
}

/** A paragraph's (or a row's) text as a report shows it (core `shown`). */
function shown(text: string): string {
  const chars = Array.from(text);
  return chars.length > 2 * CONTEXT ? `${chars.slice(0, 2 * CONTEXT).join("")}…` : text;
}

/** The cell texts of a new row (core `Replacement::row_cells`). */
function cellsOf(e: EditReplacement): string[] {
  if (e.cells && e.cells.length > 0) return e.cells.map((c) => c.trim());
  return e.replace.trim() ? e.replace.split("|").map((c) => c.trim()) : [];
}

/**
 * One structure edit (core `structure`): a paragraph is a line of the page text, a table row a line whose
 * cells are separated by tabs (what the core's reader makes of a table). Returns the reason it cannot be
 * made as asked, or null.
 */
function structure(d: MockDoc, pages: (string | null)[], e: EditReplacement, n: number, report: EditReport, parts: Set<string>): string | null {
  const action = e.action!;
  const onRows = action.includes("row");
  const places: { page: number; line: number }[] = [];
  let outsideATable = false;
  pages.forEach((text, i) => {
    (text ?? "").split("\n").forEach((line, k) => {
      if (text == null || !line.includes(e.find)) return;
      if (onRows && !line.includes("\t")) outsideATable = true;
      else places.push({ page: i, line: k });
    });
  });
  report.found[n] = places.length;
  if (places.length === 0) return onRows && outsideATable ? "edit_not_in_table" : null;
  if (e.occurrence != null && (e.occurrence < 1 || e.occurrence > places.length)) return null;
  if (e.occurrence == null && places.length > 1) return "edit_anchor_ambiguous";
  const at = places[(e.occurrence ?? 1) - 1]!;
  const lines = pages[at.page]!.split("\n");
  const anchor = lines[at.line]!;
  const isRow = (k: number) => lines[k] !== undefined && lines[k]!.includes("\t");
  let after = "";
  switch (action) {
    case "insert_after":
    case "insert_before":
      lines.splice(at.line + (action === "insert_after" ? 1 : 0), 0, e.replace);
      after = e.replace;
      break;
    case "delete_paragraph":
      if (lines.length <= 1) return "edit_last_paragraph";
      lines.splice(at.line, 1);
      break;
    case "insert_row_after":
    case "insert_row_before": {
      const cells = cellsOf(e);
      const width = anchor.split("\t").length;
      if (cells.length > width) return "edit_row_cells";
      lines.splice(at.line + (action === "insert_row_after" ? 1 : 0), 0, Array.from({ length: width }, (_, c) => cells[c] ?? "").join("\t"));
      after = cells.join(" | ");
      break;
    }
    case "delete_row":
      if (!isRow(at.line - 1) && !isRow(at.line + 1)) return "edit_last_row";
      lines.splice(at.line, 1);
      break;
    default:
      return null;
  }
  pages[at.page] = lines.join("\n");
  const where = partOf(d, d.pages[at.page]!.page_no);
  parts.add(where.part);
  report.replaced[n] = 1;
  if (report.changes.length < MAX_CHANGES) report.changes.push({ part: where.part, unit: where.unit, edit: n, action, before: shown(onRows ? anchor.split("\t").join(" | ") : anchor), after });
  return null;
}

// -- XLSX cells (core `documents::edit::cells`). The mock's workbook is the page text: page k is the sheet
// `Sheet{k}`, a line is a row, its cells are separated by tabs, and a cell that starts with `=` is a formula.

const columnNumber = (letters: string) => Array.from(letters.toUpperCase()).reduce((n, c) => n * 26 + (c.charCodeAt(0) - 64), 0);

/** `B3`, `Sheet2!b3`, `'Sheet 2'!$B$3` → sheet name (null = the first), column, row (core `parse_address`). */
function parseAddress(text: string): { sheet: string | null; col: number; row: number } | null {
  const bang = text.lastIndexOf("!");
  let sheet: string | null = null;
  if (bang >= 0) {
    const name = text.slice(0, bang).trim();
    sheet = /^'.*'$/.test(name) ? name.slice(1, -1).replace(/''/g, "'") : name;
    if (!sheet) return null;
  }
  const cell = /^([A-Za-z]{1,3})(\d+)$/.exec((bang >= 0 ? text.slice(bang + 1) : text).trim().replace(/\$/g, ""));
  if (!cell) return null;
  const [col, row] = [columnNumber(cell[1]!), Number(cell[2])];
  return col >= 1 && col <= 16_384 && row >= 1 && row <= 1_048_576 ? { sheet, col, row } : null;
}

/**
 * What a typed value is stored as (core `typed` / `number`): a number loses its thousands separators (a comma
 * counts only in its place — `1,2,3` is a text), `.5` is `0.5`, and a code like `007` stays text.
 */
function typed(value: string): string {
  const v = value.trim();
  const m = /^([-+]?)((?:\d{1,3}(?:,\d{3})+)|\d*)(?:\.(\d*))?$/.exec(v);
  if (!m) return v;
  const whole = m[2]!.replace(/,/g, "");
  const fraction = m[3] ?? "";
  if ((whole === "" && fraction === "") || (whole.length > 1 && whole.startsWith("0"))) return v;
  return `${m[1] === "-" ? "-" : ""}${whole || "0"}${fraction ? `.${fraction}` : ""}`;
}

/** Whether the formula of a cell on sheet `own` reads the cell (`sheet`, `col`, `row`) — alone or inside a range. */
function reads(formula: string, own: number, sheet: number, col: number, row: number): boolean {
  for (const m of formula.matchAll(/(?:(?:'([^']+)'|([A-Za-z0-9_가-힣]+))!)?\$?([A-Za-z]{1,3})\$?(\d+)(?::\$?([A-Za-z]{1,3})\$?(\d+))?/g)) {
    const named = m[1] ?? m[2];
    const on = named ? Number(/^sheet(\d+)$/i.exec(named)?.[1] ?? Number.NaN) - 1 : own;
    if (on !== sheet) continue;
    const [c1, r1] = [columnNumber(m[3]!), Number(m[4])];
    const [c2, r2] = m[5] ? [columnNumber(m[5]), Number(m[6])] : [c1, r1];
    if (col >= Math.min(c1, c2) && col <= Math.max(c1, c2) && row >= Math.min(r1, r2) && row <= Math.max(r1, r2)) return true;
  }
  return false;
}

/** How many formulas read one of the changed cells, directly or through other formulas (core `clear_dependents`). */
function dependents(pages: (string | null)[], changed: { sheet: number; col: number; row: number }[]): number {
  const formulas = pages.flatMap((text, sheet) =>
    (text ?? "").split("\n").flatMap((line, r) => line.split("\t").map((cell, c) => ({ sheet, col: c + 1, row: r + 1, formula: cell.trim() }))).filter((x) => x.formula.startsWith("=")),
  );
  const seen = [...changed];
  const stale = new Set<string>();
  for (let grew = true; grew; ) {
    grew = false;
    for (const f of formulas) {
      const key = `${f.sheet}!${f.col},${f.row}`;
      if (stale.has(key) || !seen.some((x) => reads(f.formula, f.sheet, x.sheet, x.col, x.row))) continue;
      stale.add(key);
      seen.push(f);
      grew = true;
    }
  }
  return stale.size;
}

/** One `set_cell` edit. Returns the reason it cannot be made, or the cell that changed. */
function setCell(pages: (string | null)[], e: EditReplacement, n: number, report: EditReport, parts: Set<string>): string | { sheet: number; col: number; row: number } {
  const at = parseAddress(e.find);
  if (!at) return "edit_cell_address";
  const sheet = at.sheet === null ? 0 : Number(/^sheet(\d+)$/i.exec(at.sheet)?.[1] ?? Number.NaN) - 1;
  if (!(sheet >= 0 && sheet < pages.length) || pages[sheet] == null) return "edit_cell_address";
  report.found[n] = 1;
  const lines = pages[sheet]!.split("\n");
  while (lines.length < at.row) lines.push("");
  const cells = lines[at.row - 1]!.split("\t");
  while (cells.length < at.col) cells.push("");
  const before = cells[at.col - 1]!;
  if (before.trim().startsWith("=")) return "edit_cell_formula";
  const after = typed(e.replace);
  cells[at.col - 1] = after;
  lines[at.row - 1] = cells.join("\t");
  pages[sheet] = lines.join("\n");
  report.replaced[n] = 1;
  const part = `xl/worksheets/sheet${sheet + 1}.xml`;
  parts.add(part);
  const address = `Sheet${sheet + 1}!${e.find.trim().split("!").pop()!.replace(/\$/g, "").toUpperCase()}`;
  if (report.changes.length < MAX_CHANGES) report.changes.push({ part, unit: null, edit: n, action: "set_cell", before: `${address} = ${before}`, after: `${address} = ${after}` });
  return { sheet, col: at.col, row: at.row };
}

// -- pictures (core `documents::edit::pictures`). The mock's files have no pictures of their own: every
// editable document is given the same three — two that can be replaced and a drawing (EMF) that cannot.

const MEDIA: Record<string, { folder: string; shown: string[] }> = {
  docx: { folder: "word/media/", shown: ["word/document.xml"] },
  pptx: { folder: "ppt/media/", shown: ["ppt/slides/slide1.xml"] },
  xlsx: { folder: "xl/media/", shown: ["xl/drawings/drawing1.xml"] },
  hwpx: { folder: "BinData/", shown: [] },
};
const FORMATS: Record<string, string> = { png: "PNG", jpg: "JPEG", jpeg: "JPEG", gif: "GIF", bmp: "BMP", webp: "WebP", tif: "TIFF", tiff: "TIFF" };

function thumbnailOf(width: number, height: number): string {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}"><rect width="100%" height="100%" fill="#cbd5e1"/><circle cx="${width / 2}" cy="${height / 2}" r="${Math.min(width, height) / 4}" fill="#64748b"/></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

/** `documents_edit_pictures` (core `documents::edit::pictures`). */
function pictures(d: MockDoc): EditPicture[] {
  const media = MEDIA[d.source_kind];
  if (!media) throw err("invalid", "edit_unsupported_format");
  const sized = (part: string, width: number, height: number): [number, number] => d.picture_sizes?.[part.toLowerCase()] ?? [width, height];
  const one = (name: string, format: string, w: number, h: number, bytes: number): EditPicture => {
    const part = media.folder + name;
    const [width, height] = sized(part, w, h);
    return { part, format, width, height, bytes, replaceable: width > 0, used_in: media.shown, thumbnail: width > 0 ? thumbnailOf(width, height) : null };
  };
  return [one("image1.png", "png", 640, 360, 48_213), one("image2.jpeg", "jpeg", 800, 600, 91_027), { part: `${media.folder}image3.emf`, format: "emf", width: 0, height: 0, bytes: 12_480, replaceable: false, used_in: media.shown, thumbnail: null }];
}

/**
 * One `replace_picture` edit (core `pictures::swap`). The mock cannot read the new file: its format is its
 * extension and its size the one its name carries (`로고 800x800.png`), else the size of the old picture.
 */
function swapPicture(d: MockDoc, e: EditReplacement, n: number, report: EditReport, sizes: Record<string, [number, number]>): string | null {
  const part = e.find.trim();
  const old = pictures(d).find((p) => p.part.toLowerCase() === part.toLowerCase());
  if (!old) return "edit_picture_missing";
  if (!old.replaceable) return "edit_picture_format";
  const path = e.replace.trim();
  const source = FORMATS[(/\.([A-Za-z]+)$/.exec(path)?.[1] ?? "").toLowerCase()];
  if (!source) return "edit_picture_unreadable";
  const hint = /(\d{2,5})\s*[x×]\s*(\d{2,5})/i.exec(path.split(/[\\/]/).pop() ?? "");
  const [width, height] = hint ? [Number(hint[1]), Number(hint[2])] : [old.width, old.height];
  const target = FORMATS[old.format]!;
  report.found[n] = 1;
  report.replaced[n] = 1;
  if (Math.abs(old.width / old.height - width / height) / (old.width / old.height) > 0.02) (report.notes ??= []).push({ edit: n, code: "edit_picture_ratio", count: 0 });
  report.changes.push({ part, unit: null, edit: n, action: "replace_picture", before: `${old.width}×${old.height} ${target}`, after: source === target ? `${width}×${height} ${source}` : `${width}×${height} ${source} → ${target}` });
  sizes[part.toLowerCase()] = [width, height];
  return null;
}

/** The page texts after the edits, the pictures that were replaced, and what changed. */
function plan(d: MockDoc, edits: EditReplacement[]): { report: EditReport; pages: (string | null)[]; sizes: Record<string, [number, number]> } {
  if (!EDITABLE.includes(d.source_kind)) throw err("invalid", "edit_unsupported_format");
  if (edits.length === 0 || edits.some((e) => !e.find.trim())) throw err("invalid", "edit_find_empty");
  if (edits.length > MAX_EDITS) throw err("invalid", "edit_too_many");
  const workbook = d.source_kind === "xlsx";
  if (edits.some((e) => (workbook ? STRUCTURAL.includes(e.action ?? "replace") : e.action === "set_cell"))) throw err("invalid", "edit_action_unsupported");
  if (edits.some((e) => [e.find, e.replace, ...(e.cells ?? [])].some((text) => /[\r\n]/.test(text)))) throw err("invalid", "edit_line_break");
  // What XML cannot hold (core `check`): the control characters below a blank but the tab, U+FFFE, U+FFFF.
  // eslint-disable-next-line no-control-regex
  if (edits.some((e) => [e.find, e.replace, ...(e.cells ?? [])].some((text) => /[\u0000-\u0008\u000b\u000c\u000e-\u001f\ufffe\uffff]/.test(text)))) throw err("invalid", "edit_control_character");
  const pages = fileTexts(d);
  const report: EditReport = { found: edits.map(() => 0), replaced: edits.map(() => 0), changes: [], parts: [] };
  const parts = new Set<string>();
  const sizes: Record<string, [number, number]> = {};
  const cells: { sheet: number; col: number; row: number }[] = [];
  edits.forEach((e, n) => {
    if (e.action === "set_cell") {
      const done = setCell(pages, e, n, report, parts);
      if (typeof done === "string") (report.problems ??= []).push({ edit: n, code: done });
      else cells.push(done);
      return;
    }
    if (e.action === "replace_picture") {
      const code = swapPicture(d, e, n, report, sizes);
      if (code) (report.problems ??= []).push({ edit: n, code });
      return;
    }
    if (e.action && e.action !== "replace") {
      const code = structure(d, pages, e, n, report, parts);
      if (code) (report.problems ??= []).push({ edit: n, code });
      return;
    }
    let seen = 0;
    pages.forEach((text, i) => {
      if (text == null) return;
      const where = partOf(d, d.pages[i]!.page_no);
      pages[i] = text
        .split("\n")
        .map((line) => {
          const taken: number[] = [];
          let out = "";
          let from = 0;
          for (let at = line.indexOf(e.find); at >= 0; at = line.indexOf(e.find, from)) {
            seen += 1;
            const take = e.occurrence == null || e.occurrence === seen;
            out += line.slice(from, at) + (take ? e.replace : e.find);
            if (take) taken.push(at);
            from = at + e.find.length;
          }
          if (taken.length === 0) return line;
          out += line.slice(from);
          report.replaced[n]! += taken.length;
          parts.add(where.part);
          taken.forEach((at, k) => {
            if (report.changes.length >= MAX_CHANGES) return;
            const moved = at + (e.replace.length - e.find.length) * k;
            const change: EditChange = { part: where.part, unit: where.unit, edit: n, before: around(line, at, at + e.find.length), after: around(out, moved, moved + e.replace.length) };
            report.changes.push(change);
          });
          return out;
        })
        .join("\n");
    });
    report.found[n] = seen;
  });
  // Formulas that read a changed cell are computed again by the spreadsheet program.
  const cleared = cells.length > 0 ? dependents(pages, cells) : 0;
  if (cleared > 0) (report.notes ??= []).push({ edit: edits.findIndex((e) => e.action === "set_cell"), code: "edit_formulas_cleared", count: cleared });
  report.parts = [...parts, ...edits.filter((e, n) => e.action === "replace_picture" && report.replaced[n]! > 0).map((e) => e.find.trim())];
  return { report, pages, sizes };
}

/** `계약서.docx` → `계약서 (수정).docx`, `계약서 (수정 2).docx` …; an edition of an edition counts on from the same stem. */
function editionPath(d: MockDoc): { rel: string; name: string } {
  const dot = d.original_name.lastIndexOf(".");
  const ext = dot > 0 ? d.original_name.slice(dot) : "";
  const stem = (dot > 0 ? d.original_name.slice(0, dot) : d.original_name).replace(/ \(수정(?: \d+)?\)$/, "");
  const slash = d.rel_path.lastIndexOf("/");
  const folder = slash >= 0 ? d.rel_path.slice(0, slash + 1) : "";
  const taken = new Set(wsDocs().map((x) => x.rel_path));
  for (let n = 1; n < 1000; n += 1) {
    const name = n === 1 ? `${stem} (수정)${ext}` : `${stem} (수정 ${n})${ext}`;
    if (!taken.has(folder + name)) return { rel: folder + name, name };
  }
  throw err("invalid", "edit_too_many_editions");
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export async function mockEdit(cmd: string, a: Record<string, any>): Promise<EditReport | Edition | EditSuggestion | EditPicture[]> {
  const d = wsDocs().find((x) => x.id === a.id);
  if (!d) throw err("not_found", String(a.id));
  if (cmd === "documents_edit_suggest") return suggest(d, String(a.instruction ?? ""));
  if (cmd === "documents_edit_pictures") return pictures(d);
  const edits = (a.edits ?? []) as EditReplacement[];
  const { report, pages, sizes } = plan(d, edits);
  if (cmd === "documents_edit_preview") return report;
  // Nothing is written while one of the edits cannot be made as asked.
  if (report.problems?.[0]) throw err("invalid", report.problems[0].code);
  if (report.replaced.reduce((x, y) => x + y, 0) === 0) throw err("invalid", "edit_nothing_found");

  const stamp = new Date();
  const iso = stamp.toISOString();
  const { rel, name } = editionPath(d);
  const copy: MockDoc = {
    ...structuredClone(d),
    id: crypto.randomUUID().replace(/-/g, ""),
    rel_path: rel,
    original_name: name,
    content_hash: `edited-${d.id}-${iso}`,
    file_modified_at: iso,
    imported_at: iso,
    updated_at: iso,
    text_history: [],
    picture_sizes: { ...d.picture_sizes, ...sizes },
    pages: d.pages.map((p, i) => ({ ...structuredClone(p), text: pages[i] ?? p.text, layout: null })),
  };
  upsertDoc(copy);

  // Editions: the original steps back (kept as "원본" when it had no label), the copy is the latest.
  const kb = await import("./mock-knowledge");
  const groups = (await kb.mockKnowledge("knowledge_versions", {})) as VersionGroup[];
  const before = groups.flatMap((g) => g.documents).find((v) => v.document_id === d.id) ?? null;
  const group = before?.group_key ?? (d.title?.trim() || d.original_name);
  const label = `수정 ${stamp.getFullYear()}-${pad(stamp.getMonth() + 1)}-${pad(stamp.getDate())} ${pad(stamp.getHours())}:${pad(stamp.getMinutes())}`;
  await kb.mockKnowledge("knowledge_version_set", {
    input: { document_id: d.id, group_key: group, version_label: before?.version_label ?? "원본", effective_date: before?.effective_date ?? null, is_latest: false, note: before?.note ?? null },
  });
  await kb.mockKnowledge("knowledge_version_set", {
    input: { document_id: copy.id, group_key: group, version_label: label, effective_date: iso.slice(0, 10), is_latest: true, note: null },
  });

  // Where the original is, the edition is: its projects, on the same side.
  const pj = await import("./mock-projects");
  const add = pj
    .projectSummaries()
    .map((p) => (pj.isMember(p.id, d.id) ? { doc_id: copy.id, project_id: p.id, reference: false } : kb.projectReferenceDocIds(p.id).includes(d.id) ? { doc_id: copy.id, project_id: p.id, reference: true } : null))
    .filter((x): x is { doc_id: string; project_id: string; reference: boolean } => x !== null);
  if (add.length > 0) pj.assignDocuments({ remove: [], add });
  // The memberships may have changed the role: an edition keeps its original's.
  const stored = wsDocs().find((x) => x.id === copy.id);
  if (stored && stored.role !== d.role) upsertDoc({ ...stored, role: d.role });

  emit(EVENTS.documentsChanged, { kind: "imported", ids: [copy.id] } satisfies DocumentsChanged);
  emit(EVENTS.documentsChanged, { kind: "updated", ids: [d.id] } satisfies DocumentsChanged);
  return { report, doc_id: copy.id, rel_path: rel, version_label: label, projects: add.length, warnings: [] };
}
