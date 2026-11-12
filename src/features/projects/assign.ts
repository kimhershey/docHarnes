// The assignment sheet behind "프로젝트에 넣기" (1.0.0 plan §12 / §12-B): which document goes into which
// projects. One row per document, one tick per (document, project) pair; a pair the document is already
// in starts ticked and leaves the project when it is unticked. Everything here is plain data — the dialog
// renders it and sends `sheetRequest` to `project_assign` in one call.
import type { AssignRequest, ClassifyResult, ClassifySuggestion, DocumentView } from "@/lib/types";

/** 업무 문서 or 참고 자료 — the side of a project a document sits on. */
export type Kind = "work" | "reference";

export interface SheetRow {
  id: string;
  /** The kind the document has now, and the kind picked for it in this sheet. */
  kindBefore: Kind;
  kind: Kind;
  /** The side the document is on in each project it is in now. */
  before: Record<string, Kind>;
  /** Ticked projects (the ones it is in now start ticked). */
  ticked: string[];
  /** Ticks a suggestion made that the user has not touched since. */
  suggested: string[];
  /** Further projects a suggestion scored for the document, not ticked: project id → confidence (0..1). */
  candidates: Record<string, number>;
}

/** Float noise of a 32-bit score read as a double (0.6 arrives as 0.6000000238 or 0.5999999). */
const EPSILON = 1e-6;

export function kindOf(doc: Pick<DocumentView, "role">): Kind {
  return doc.role === "knowledge" ? "reference" : "work";
}

export function sheetRows(docs: DocumentView[]): SheetRow[] {
  return docs.map((doc) => {
    const before: Record<string, Kind> = {};
    for (const p of doc.projects ?? []) before[p.id] = p.reference ? "reference" : "work";
    const kind = kindOf(doc);
    return { id: doc.id, kindBefore: kind, kind, before, ticked: Object.keys(before), suggested: [], candidates: {} };
  });
}

/** The columns of the sheet: the picked projects in their order, then every project a row is in or ticked for — no membership stays hidden. */
export function sheetColumns(rows: SheetRow[], picked: string[]): string[] {
  const out = [...picked];
  for (const r of rows) for (const p of [...Object.keys(r.before), ...r.ticked]) if (!out.includes(p)) out.push(p);
  return out;
}

function setTick(row: SheetRow, projectId: string, on: boolean): SheetRow {
  const has = row.ticked.includes(projectId);
  const wasSuggested = row.suggested.includes(projectId);
  if (has === on && !wasSuggested) return row;
  return {
    ...row,
    ticked: on ? (has ? row.ticked : [...row.ticked, projectId]) : row.ticked.filter((p) => p !== projectId),
    // The user decided this cell: it is no longer "what the suggestion ticked".
    suggested: row.suggested.filter((p) => p !== projectId),
  };
}

/** Ticks / unticks one cell. */
export function toggleCell(rows: SheetRow[], docId: string, projectId: string): SheetRow[] {
  return rows.map((r) => (r.id === docId ? setTick(r, projectId, !r.ticked.includes(projectId)) : r));
}

/** Ticks / unticks a project for many rows — a column header (the visible rows) or "고른 행에 추가 / 빼기". */
export function setCells(rows: SheetRow[], docIds: Iterable<string>, projectId: string, on: boolean): SheetRow[] {
  const ids = new Set(docIds);
  return rows.map((r) => (ids.has(r.id) ? setTick(r, projectId, on) : r));
}

export function setKind(rows: SheetRow[], docIds: Iterable<string>, kind: Kind): SheetRow[] {
  const ids = new Set(docIds);
  return rows.map((r) => (ids.has(r.id) && r.kind !== kind ? { ...r, kind } : r));
}

/**
 * The projects a suggestion ticks for its document: the suggested project and — when its confidence
 * reaches `threshold` (설정 "프로젝트 분류(2순위 추천 임계값)") — the second candidate as well. A document
 * the classifier left undecided gets its two best candidates when both reach the threshold (it could not
 * choose between two strong projects, and the sheet can hold both); otherwise nothing is ticked.
 */
export function suggestedProjects(s: ClassifySuggestion, threshold: number): string[] {
  const strong = (i: number) => (s.alternatives[i]?.confidence ?? 0) + EPSILON >= threshold;
  if (s.project_id) return strong(0) ? [s.project_id, s.alternatives[0]!.project_id] : [s.project_id];
  return strong(0) && strong(1) ? [s.alternatives[0]!.project_id, s.alternatives[1]!.project_id] : [];
}

/**
 * Fills the sheet from a classification preview. A suggestion only adds ticks — a membership the document
 * has is never unticked by it — and the cells it ticks are marked until the user touches them. The other
 * scored projects stay as candidates. Returns the rows and the projects that need a column.
 */
export function fillFromSuggestions(rows: SheetRow[], result: ClassifyResult, threshold: number): { rows: SheetRow[]; projects: string[] } {
  const byDoc = new Map(result.suggestions.map((s) => [s.doc_id, s]));
  const projects: string[] = [];
  const next = rows.map((row) => {
    const s = byDoc.get(row.id);
    if (!s) return row;
    const picks = suggestedProjects(s, threshold).filter((p) => !row.ticked.includes(p));
    const candidates: Record<string, number> = {};
    for (const c of s.alternatives) if (!picks.includes(c.project_id) && !row.ticked.includes(c.project_id)) candidates[c.project_id] = c.confidence ?? 0;
    for (const p of picks) if (!projects.includes(p)) projects.push(p);
    return { ...row, ticked: [...row.ticked, ...picks], suggested: [...row.suggested.filter((p) => !picks.includes(p)), ...picks], candidates };
  });
  return { rows: next, projects };
}

/**
 * What the sheet asks for. A new tick puts the document in the project as the row's kind; a tick taken
 * off a project the document is in takes it out; a row whose kind was changed moves the memberships it
 * keeps to the other side (an unchanged row leaves them where they are).
 */
export function sheetRequest(rows: SheetRow[]): AssignRequest {
  const req: AssignRequest = { remove: [], add: [] };
  for (const r of rows) {
    const reference = r.kind === "reference";
    for (const p of r.ticked) {
      const side = r.before[p];
      if (side === undefined || (r.kind !== r.kindBefore && side !== r.kind)) req.add.push({ doc_id: r.id, project_id: p, reference });
    }
    for (const p of Object.keys(r.before)) if (!r.ticked.includes(p)) req.remove.push({ doc_id: r.id, project_id: p });
  }
  return req;
}

/** "모두 같은 프로젝트에": every document into every picked project, all as one kind. */
export function sameForAllRequest(docIds: string[], projectIds: string[], kind: Kind): AssignRequest {
  const reference = kind === "reference";
  return { remove: [], add: docIds.flatMap((doc_id) => projectIds.map((project_id) => ({ doc_id, project_id, reference }))) };
}

export interface SheetSummary {
  /** Per project with a change: documents put in, taken out, moved to the other side. */
  projects: { id: string; add: number; remove: number; move: number }[];
  add: number;
  remove: number;
  move: number;
  /** Rows whose kind change takes effect (the document is in at least one project afterwards). */
  kindChanged: number;
  /** Rows with no project ticked — they stay outside every project. */
  unassigned: number;
  /** The pairs that leave a project, for the extra confirmation. */
  removals: { doc_id: string; project_id: string }[];
}

export function sheetSummary(rows: SheetRow[]): SheetSummary {
  const req = sheetRequest(rows);
  const byId = new Map(rows.map((r) => [r.id, r]));
  const out: SheetSummary = { projects: [], add: 0, remove: 0, move: 0, kindChanged: 0, unassigned: 0, removals: req.remove };
  const of = (id: string) => {
    let t = out.projects.find((p) => p.id === id);
    if (!t) {
      t = { id, add: 0, remove: 0, move: 0 };
      out.projects.push(t);
    }
    return t;
  };
  for (const a of req.add) {
    const moved = byId.get(a.doc_id)?.before[a.project_id] !== undefined;
    of(a.project_id)[moved ? "move" : "add"] += 1;
    out[moved ? "move" : "add"] += 1;
  }
  for (const r of req.remove) {
    of(r.project_id).remove += 1;
    out.remove += 1;
  }
  for (const r of rows) {
    if (r.ticked.length === 0) out.unassigned += 1;
    else if (r.kind !== r.kindBefore) out.kindChanged += 1;
  }
  return out;
}

/** Shift-click: every visible row between the last clicked one and this one joins (or leaves) the selection. */
export function selectRange(visibleIds: string[], picked: ReadonlySet<string>, anchor: string | null, id: string, on: boolean): Set<string> {
  const next = new Set(picked);
  const a = anchor ? visibleIds.indexOf(anchor) : -1;
  const b = visibleIds.indexOf(id);
  const span = a >= 0 && b >= 0 ? visibleIds.slice(Math.min(a, b), Math.max(a, b) + 1) : [id];
  for (const x of span) {
    if (on) next.add(x);
    else next.delete(x);
  }
  return next;
}
