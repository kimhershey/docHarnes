// Browser-mock twin of the page text edit history (documents/import.rs, 0.3.0 item 7): every
// correction of a page records the page state before and after; undo takes back the newest applied
// correction, redo re-applies the oldest undone one, a new correction drops the undone branch and the
// oldest corrections beyond the cap go with their bookkeeping. Pure functions over the rows a mock
// document keeps in `text_history`.
import type { PageLayout, PageTextEdit, PageTextHistory } from "./types";

export interface MockTextState {
  text: string | null;
  layout: PageLayout | null;
  source: string | null;
}

export interface MockTextHistoryRow {
  id: number;
  page_no: number;
  kind: "edit" | "revert" | "redo";
  source: string;
  target_id: number | null;
  previous: MockTextState;
  new: MockTextState;
  edited_at: string;
}

export const PAGE_TEXT_HISTORY_CAP = 50;
const CHANGE_SUMMARY_CHARS = 60;

/** The words that differ between two page texts (common head and tail stripped, clipped). */
export function changeSummary(before: string | null, after: string | null): [string, string] {
  const a = (before ?? "").split(/\s+/).filter(Boolean);
  const b = (after ?? "").split(/\s+/).filter(Boolean);
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head += 1;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail += 1;
  const clip = (words: string[]) => {
    const joined = words.join(" ");
    return Array.from(joined).length > CHANGE_SUMMARY_CHARS ? `${Array.from(joined).slice(0, CHANGE_SUMMARY_CHARS).join("")}…` : joined;
  };
  return [clip(a.slice(head, a.length - tail)), clip(b.slice(head, b.length - tail))];
}

function sameState(a: MockTextState, b: MockTextState): boolean {
  return a.text === b.text && a.source === b.source && JSON.stringify(a.layout) === JSON.stringify(b.layout);
}

/** Corrections of a page with their state (undone when the latest revert / redo targeting them is a revert). */
function editStates(rows: MockTextHistoryRow[], pageNo: number | null): { row: MockTextHistoryRow; undone: boolean }[] {
  const scope = pageNo === null ? rows : rows.filter((r) => r.page_no === pageNo);
  return scope
    .filter((r) => r.kind === "edit")
    .map((row) => {
      const last = [...scope].reverse().find((b) => b.kind !== "edit" && b.target_id === row.id);
      return { row, undone: last?.kind === "revert" };
    });
}

export function historyView(rows: MockTextHistoryRow[], pageNo: number | null): PageTextHistory {
  const states = editStates(rows, pageNo);
  const entries: PageTextEdit[] = [...states].reverse().map(({ row, undone }) => {
    const [before, after] = changeSummary(row.previous.text, row.new.text);
    return { id: row.id, page_no: row.page_no, source: row.source, edited_at: row.edited_at, undone, before, after };
  });
  return {
    entries,
    can_undo: pageNo !== null && states.some((s) => !s.undone),
    can_redo: pageNo !== null && states.some((s) => s.undone),
  };
}

/** Records a correction (nothing when the state did not change); returns the rows to keep. */
export function recordEdit(rows: MockTextHistoryRow[], pageNo: number, source: string, before: MockTextState, after: MockTextState): MockTextHistoryRow[] {
  if (sameState(before, after)) return rows;
  const states = editStates(rows, pageNo);
  const gone = new Set(states.filter((s) => s.undone).map((s) => s.row.id));
  const kept = states.length - gone.size;
  if (kept + 1 > PAGE_TEXT_HISTORY_CAP) {
    for (const s of states.filter((x) => !x.undone).slice(0, kept + 1 - PAGE_TEXT_HISTORY_CAP)) gone.add(s.row.id);
  }
  const next = rows.filter((r) => !gone.has(r.id) && !(r.target_id !== null && gone.has(r.target_id)));
  const id = rows.reduce((m, r) => Math.max(m, r.id), 0) + 1;
  next.push({ id, page_no: pageNo, kind: "edit", source, target_id: null, previous: before, new: after, edited_at: new Date().toISOString() });
  return next;
}

/** The correction Ctrl+Z takes back (newest applied) or redo re-applies (oldest undone). */
export function moveTarget(rows: MockTextHistoryRow[], pageNo: number, redo: boolean): MockTextHistoryRow | null {
  const states = editStates(rows, pageNo);
  const pick = redo ? states.find((s) => s.undone) : [...states].reverse().find((s) => !s.undone);
  return pick?.row ?? null;
}

/** Records an undo / redo of `target` from the page's `current` state; returns the state to apply. */
export function recordMove(rows: MockTextHistoryRow[], target: MockTextHistoryRow, redo: boolean, current: MockTextState): MockTextState {
  const state = redo ? target.new : target.previous;
  const id = rows.reduce((m, r) => Math.max(m, r.id), 0) + 1;
  rows.push({ id, page_no: target.page_no, kind: redo ? "redo" : "revert", source: target.source, target_id: target.id, previous: current, new: state, edited_at: new Date().toISOString() });
  return state;
}

/** The applied corrections to undo (newest first) so the page reads as before `historyId`. */
export function revertTargets(rows: MockTextHistoryRow[], historyId: number): { pageNo: number; targets: MockTextHistoryRow[] } | { error: "not_found" | "undone" } {
  const target = rows.find((r) => r.id === historyId && r.kind === "edit");
  if (!target) return { error: "not_found" };
  const states = editStates(rows, target.page_no);
  if (states.some((s) => s.row.id === historyId && s.undone)) return { error: "undone" };
  return { pageNo: target.page_no, targets: [...states].reverse().filter((s) => !s.undone && s.row.id >= historyId).map((s) => s.row) };
}
