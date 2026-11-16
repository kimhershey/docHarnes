// Browser-mode stand-in for the core's format-preserving edit (1.0.0 §2-1, `documents::edit`): the mock has
// no files, so a replacement runs over the page text of the document — as it was before any correction made
// in the viewer, which is what the file still holds —, one line = one paragraph. The rules
// are the core's — which formats, what a request may hold, how occurrences are counted, how the edited copy
// is named and registered as the latest edition next to its original.
import { emit, err, upsertDoc, wsDocs, type MockDoc } from "./mock-backend";
import { EVENTS, type DocumentsChanged, type EditChange, type EditReplacement, type EditReport, type Edition, type VersionGroup } from "./types";

const EDITABLE = ["hwpx", "docx", "pptx"];
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
  if (d.source_kind === "hwpx") return { part: "Contents/section0.xml", unit: 1 };
  return { part: "word/document.xml", unit: null };
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

/** The page texts after the edits, and what changed. */
function plan(d: MockDoc, edits: EditReplacement[]): { report: EditReport; pages: (string | null)[] } {
  if (!EDITABLE.includes(d.source_kind)) throw err("invalid", "edit_unsupported_format");
  if (edits.length === 0 || edits.some((e) => !e.find.trim())) throw err("invalid", "edit_find_empty");
  if (edits.length > MAX_EDITS) throw err("invalid", "edit_too_many");
  if (edits.some((e) => [e.find, e.replace, ...(e.cells ?? [])].some((text) => /[\r\n]/.test(text)))) throw err("invalid", "edit_line_break");
  // The file holds what the page read before the viewer's corrections: they change the page text, not the file.
  const pages = d.pages.map((p) => {
    const first = (d.text_history ?? []).find((row) => row.page_no === p.page_no);
    return first ? first.previous.text : p.text;
  });
  const report: EditReport = { found: edits.map(() => 0), replaced: edits.map(() => 0), changes: [], parts: [] };
  const parts = new Set<string>();
  edits.forEach((e, n) => {
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
  report.parts = [...parts];
  return { report, pages };
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
export async function mockEdit(cmd: string, a: Record<string, any>): Promise<EditReport | Edition> {
  const d = wsDocs().find((x) => x.id === a.id);
  if (!d) throw err("not_found", String(a.id));
  const edits = (a.edits ?? []) as EditReplacement[];
  const { report, pages } = plan(d, edits);
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
