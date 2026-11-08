// Scope section state shared by the organise and export tabs → planner `Scope`.

import type { Scope, ScopeFilter } from "@/lib/types";

export type ScopeMode = "all" | "selection" | "folder";

export interface ScopeState {
  mode: ScopeMode;
  /** Workspace-relative folder when `mode === "folder"` ("" = root). */
  folder: string | null;
  /** Document type slug filter (null = every type). */
  docType: string | null;
  /** ISO dates (empty = open-ended). */
  dateFrom: string;
  dateTo: string;
}

export function defaultScope(selection: boolean): ScopeState {
  return { mode: selection ? "selection" : "all", folder: null, docType: null, dateFrom: "", dateTo: "" };
}

/** Planner scope for a request; only known keys are emitted (the Rust struct denies unknown fields). */
export function toScope(s: ScopeState, selectedIds: string[]): Scope {
  const filter: ScopeFilter = {};
  if (s.docType) filter.doc_type = s.docType;
  if (s.dateFrom) filter.date_from = s.dateFrom;
  if (s.dateTo) filter.date_to = s.dateTo;
  const scope: Scope = {};
  if (s.mode === "selection") scope.doc_ids = selectedIds;
  else if (s.mode === "folder" && s.folder !== null) scope.folder = s.folder;
  if (Object.keys(filter).length > 0) scope.filter = filter;
  return scope;
}
