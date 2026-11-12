import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SelectAllBar } from "@/components/ui/select-all-bar";
import { docTypeLabel } from "@/features/pipeline/labels";
import { useDocTypes } from "@/lib/queries";
import type { DocumentView, Project } from "@/lib/types";
import { cn } from "@/lib/utils";

import { selectRange, setCells, setKind, toggleCell, type Kind, type SheetRow } from "./assign";

type CellState = "member" | "added" | "suggested" | "removed" | "empty";

function cellState(row: SheetRow, projectId: string): CellState {
  const ticked = row.ticked.includes(projectId);
  const before = row.before[projectId] !== undefined;
  if (ticked && before) return "member";
  if (ticked) return row.suggested.includes(projectId) ? "suggested" : "added";
  return before ? "removed" : "empty";
}

const CELL_TONE: Record<CellState, string> = {
  member: "",
  empty: "",
  added: "bg-accent-soft outline outline-1 outline-offset-[-2px] outline-accent",
  suggested: "bg-accent-soft outline-dashed outline-1 outline-offset-[-2px] outline-accent",
  removed: "bg-danger-soft outline outline-1 outline-offset-[-2px] outline-danger",
};

/**
 * The "문서별로 지정" table of "프로젝트에 넣기" (1.0.0 plan §12-B): one row per document, one column per
 * project, a tick per pair. A project the document is in starts ticked and unticking it takes the document
 * out; the kind (업무 문서 / 참고 자료) is picked per row. Rows are narrowed by a search and "지정 안 된 문서만",
 * picked with their checkbox (Shift for a range) and changed together in the bar above the table.
 * The table scrolls inside its own box — header row and document column stay in view — so the dialog
 * never grows wider than the window at any UI 배율.
 */
export function AssignSheet({
  docs,
  rows,
  onRows,
  columns,
  projects,
  onColumn,
  disabled = false,
}: {
  docs: DocumentView[];
  rows: SheetRow[];
  onRows: (next: SheetRow[]) => void;
  /** Project ids, in column order. */
  columns: string[];
  projects: Project[];
  /** A project that needs a column (a candidate was ticked, a bulk add named it). */
  onColumn: (projectId: string) => void;
  disabled?: boolean;
}) {
  const { t, i18n } = useTranslation();
  const { data: docTypes } = useDocTypes();
  const [query, setQuery] = useState("");
  const [unassignedOnly, setUnassignedOnly] = useState(false);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [anchor, setAnchor] = useState<string | null>(null);
  const [bulkProject, setBulkProject] = useState<string>("");

  const docById = useMemo(() => new Map(docs.map((d) => [d.id, d])), [docs]);
  const projectName = (id: string) => projects.find((p) => p.id === id)?.name ?? id;
  const nameOf = (id: string) => {
    const d = docById.get(id);
    return d ? d.title?.trim() || d.original_name : id;
  };
  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return rows.filter((r) => {
      if (unassignedOnly && r.ticked.length > 0) return false;
      if (!q) return true;
      const d = docById.get(r.id);
      if (!d) return false;
      const type = docTypeLabel(d.doc_type, docTypes, t, i18n.language);
      return [d.title ?? "", d.original_name, d.rel_path, type].some((s) => s.toLowerCase().includes(q));
    });
  }, [rows, query, unassignedOnly, docById, docTypes, t, i18n.language]);
  const visibleIds = useMemo(() => visible.map((r) => r.id), [visible]);
  const pickedVisible = visibleIds.filter((id) => picked.has(id));

  const kindLabel = (k: Kind) => t(k === "work" ? "projects.put.kind_work" : "projects.put.kind_reference");
  const bulkTick = (on: boolean) => {
    if (!bulkProject || pickedVisible.length === 0) return;
    if (on) onColumn(bulkProject);
    onRows(setCells(rows, pickedVisible, bulkProject, on));
  };

  return (
    <div className="flex min-w-0 flex-col gap-2" data-testid="put-sheet">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("projects.put.sheet_search")} aria-label={t("projects.put.sheet_search")} className="h-8 min-w-40 flex-1" data-testid="put-sheet-search" />
        <label className="flex cursor-pointer items-center gap-1.5 text-xs text-fg-muted">
          <input type="checkbox" checked={unassignedOnly} onChange={(e) => setUnassignedOnly(e.target.checked)} data-testid="put-sheet-unassigned" />
          {t("projects.put.sheet_unassigned_only")}
        </label>
      </div>
      <SelectAllBar visibleIds={visibleIds} picked={picked} onChange={setPicked} disabled={disabled} testId="put-sheet-select" />
      {pickedVisible.length > 0 && (
        <div className="flex min-w-0 flex-wrap items-center gap-1.5 rounded-md border border-border bg-bg-sunken px-2 py-1.5 text-xs" data-testid="put-sheet-bulk">
          <span className="shrink-0 text-fg-muted">{t("projects.put.sheet_bulk", { count: pickedVisible.length })}</span>
          <Select value={bulkProject} onValueChange={setBulkProject}>
            <SelectTrigger className="h-7 w-auto min-w-0 max-w-[14rem] text-xs" aria-label={t("projects.put.sheet_bulk_project")} data-testid="put-sheet-bulk-project">
              <SelectValue placeholder={t("projects.put.sheet_bulk_project")} />
            </SelectTrigger>
            <SelectContent>
              {projects.map((p) => (
                <SelectItem key={p.id} value={p.id} data-testid={`put-sheet-bulk-option-${p.id}`}>
                  <span className="block max-w-[18rem] truncate" title={p.name}>
                    {p.name}
                  </span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button size="sm" variant="outline" onClick={() => bulkTick(true)} disabled={disabled || !bulkProject} data-testid="put-sheet-bulk-add">
            {t("projects.put.sheet_bulk_add")}
          </Button>
          <Button size="sm" variant="outline" onClick={() => bulkTick(false)} disabled={disabled || !bulkProject} data-testid="put-sheet-bulk-remove">
            {t("projects.put.sheet_bulk_remove")}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => onRows(setKind(rows, pickedVisible, "work"))} disabled={disabled} data-testid="put-sheet-bulk-work">
            {t("projects.put.sheet_bulk_work")}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => onRows(setKind(rows, pickedVisible, "reference"))} disabled={disabled} data-testid="put-sheet-bulk-reference">
            {t("projects.put.sheet_bulk_reference")}
          </Button>
        </div>
      )}
      <div className="max-h-[22rem] min-w-0 overflow-auto rounded-md border border-border" data-testid="put-sheet-scroll">
        {columns.length === 0 ? (
          <p className="p-3 text-sm text-fg-muted" data-testid="put-sheet-no-columns">
            {t("projects.put.sheet_no_columns")}
          </p>
        ) : (
          <table className="w-max min-w-full border-separate border-spacing-0 text-sm" data-testid="put-sheet-table">
            <thead>
              <tr>
                <th scope="col" className="sticky left-0 top-0 z-30 w-[17rem] max-w-[17rem] border-b border-border bg-bg-elevated px-2 py-1.5 text-left text-xs font-semibold text-fg-subtle">
                  {t("projects.put.sheet_doc")}
                </th>
                <th scope="col" className="sticky top-0 z-20 border-b border-border bg-bg-elevated px-2 py-1.5 text-left text-xs font-semibold text-fg-subtle">
                  {t("projects.put.kind")}
                </th>
                {columns.map((pid) => {
                  const on = visible.filter((r) => r.ticked.includes(pid)).length;
                  const name = projectName(pid);
                  return (
                    <th key={pid} scope="col" className="sticky top-0 z-20 border-b border-border bg-bg-elevated px-2 py-1.5 text-xs font-semibold" data-testid={`put-sheet-head-${pid}`}>
                      <label className="flex cursor-pointer flex-col items-center gap-1" title={t("projects.put.sheet_column_all", { name })}>
                        <span className="block max-w-[8rem] truncate">{name}</span>
                        <input
                          type="checkbox"
                          checked={visible.length > 0 && on === visible.length}
                          ref={(el) => {
                            if (el) el.indeterminate = on > 0 && on < visible.length;
                          }}
                          onChange={(e) => onRows(setCells(rows, visibleIds, pid, e.target.checked))}
                          disabled={disabled || visible.length === 0}
                          aria-label={t("projects.put.sheet_column_all", { name })}
                          data-testid={`put-sheet-col-${pid}`}
                        />
                      </label>
                    </th>
                  );
                })}
              </tr>
            </thead>
            <tbody>
              {visible.length === 0 && (
                <tr>
                  <td colSpan={columns.length + 2} className="p-3 text-sm text-fg-muted">
                    {t("projects.put.sheet_no_rows")}
                  </td>
                </tr>
              )}
              {visible.map((row) => {
                const d = docById.get(row.id);
                const name = nameOf(row.id);
                const candidates = Object.entries(row.candidates)
                  .filter(([pid]) => !row.ticked.includes(pid))
                  .sort((a, b) => b[1] - a[1])
                  .slice(0, 2);
                return (
                  <tr key={row.id} data-testid={`put-sheet-row-${row.id}`} data-kind={row.kind} data-ticked={row.ticked.length}>
                    <td className="sticky left-0 z-10 w-[17rem] max-w-[17rem] border-b border-border bg-bg-elevated px-2 py-1.5">
                      <div className="flex min-w-0 items-start gap-2">
                        <input
                          type="checkbox"
                          className="mt-1 shrink-0"
                          checked={picked.has(row.id)}
                          onChange={() => undefined}
                          onClick={(e) => {
                            const on = !picked.has(row.id);
                            setPicked(e.shiftKey ? selectRange(visibleIds, picked, anchor, row.id, on) : selectRange(visibleIds, picked, null, row.id, on));
                            setAnchor(row.id);
                          }}
                          disabled={disabled}
                          aria-label={t("projects.put.sheet_row_select", { doc: name })}
                          data-testid={`put-sheet-pick-${row.id}`}
                        />
                        <div className="flex min-w-0 flex-col">
                          <span className="truncate font-medium" title={d?.rel_path ?? name}>
                            {name}
                          </span>
                          <span className="truncate text-xs text-fg-subtle">{d ? docTypeLabel(d.doc_type, docTypes, t, i18n.language) : ""}</span>
                          {candidates.length > 0 && (
                            <span className="mt-0.5 flex min-w-0 flex-wrap gap-1">
                              {candidates.map(([pid, confidence]) => (
                                <button
                                  key={pid}
                                  type="button"
                                  className="max-w-full cursor-pointer truncate rounded-sm border border-dashed border-border px-1 text-xs text-fg-muted hover:border-accent hover:text-fg"
                                  title={t("projects.put.sheet_candidate_hint")}
                                  onClick={() => {
                                    onColumn(pid);
                                    onRows(toggleCell(rows, row.id, pid));
                                  }}
                                  disabled={disabled}
                                  data-testid={`put-sheet-candidate-${row.id}-${pid}`}
                                >
                                  {t("projects.put.sheet_candidate", { name: projectName(pid), percent: Math.round(confidence * 100) })}
                                </button>
                              ))}
                            </span>
                          )}
                        </div>
                      </div>
                    </td>
                    <td className="border-b border-border px-2 py-1.5">
                      <button
                        type="button"
                        role="switch"
                        aria-checked={row.kind === "reference"}
                        className={cn(
                          "cursor-pointer whitespace-nowrap rounded-md border px-1.5 py-0.5 text-xs",
                          row.kind !== row.kindBefore ? "border-accent bg-accent-soft" : "border-border hover:border-accent",
                        )}
                        title={t("projects.put.sheet_kind_switch")}
                        onClick={() => onRows(setKind(rows, [row.id], row.kind === "work" ? "reference" : "work"))}
                        disabled={disabled}
                        data-testid={`put-sheet-kind-${row.id}`}
                        data-kind={row.kind}
                      >
                        {kindLabel(row.kind)}
                      </button>
                    </td>
                    {columns.map((pid) => {
                      const state = cellState(row, pid);
                      return (
                        <td key={pid} className={cn("border-b border-border p-0 text-center", CELL_TONE[state])}>
                          <label className="flex cursor-pointer items-center justify-center px-2 py-2">
                            <input
                              type="checkbox"
                              checked={row.ticked.includes(pid)}
                              onChange={() => onRows(toggleCell(rows, row.id, pid))}
                              disabled={disabled}
                              aria-label={t("projects.put.sheet_cell", { doc: name, project: projectName(pid) })}
                              data-testid={`put-sheet-cell-${row.id}-${pid}`}
                              data-state={state}
                            />
                          </label>
                        </td>
                      );
                    })}
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
      <p className="text-xs text-fg-subtle">{t("projects.put.sheet_legend")}</p>
    </div>
  );
}
