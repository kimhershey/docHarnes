import { BookOpen, Briefcase, Plus, Sparkles } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import type { TFunction } from "i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SelectAllBar } from "@/components/ui/select-all-bar";
import { Spinner } from "@/components/ui/spinner";
import { addToReferenceGroup, invalidateReference, useReferenceGroups } from "@/features/knowledge/reference";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects, useDocuments, useProjects } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import { COMMON_REFERENCE_ID, SCREEN_ROUTES, type AssignOutcome, type AssignRequest, type ClassifyResult, type DocumentFilter, type DocumentView, type Project } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useProjectsStore } from "@/stores/projects";
import { useSettings } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";

import { AssignSheet } from "./AssignSheet";
import { fillFromSuggestions, sameForAllRequest, setCells, sheetColumns, sheetRequest, sheetRows, sheetSummary, suggestedProjects, type SheetRow } from "./assign";

const NEW_GROUP = "__new__";
/** The documents of the sheet come from the library list (it carries each document's projects). */
const SHEET_DOCS: DocumentFilter = { sort: "imported_desc" };
/** Mirrors the settings default of `processing.project_second_choice` until the settings are loaded. */
const DEFAULT_SECOND_CHOICE = 0.6;

export type PutTarget = "project" | "common";
export type PutKind = "work" | "reference";
/** "모두 같은 프로젝트에" (every document into every ticked project) or "문서별로 지정" (the sheet). */
export type PutMode = "same" | "sheet";

/** What "프로젝트에 넣기" did, for callers that follow up (clear a selection, refresh a badge …). */
export interface PutResult {
  target: PutTarget;
  kind: PutKind;
  /** The project, when exactly one was involved. */
  projectId: string | null;
  projectIds: string[];
  groupId: string | null;
  count: number;
}

/** "AAA: 넣음 3건, 뺌 1건 · BBB: 넣음 2건" plus what was skipped. */
function outcomeMessage(out: AssignOutcome, t: TFunction): string {
  if (out.added + out.removed + out.moved === 0) return t("projects.put.done_nothing");
  const detail = out.projects
    .map((p) => {
      const parts = [p.added > 0 ? t("projects.put.done_added", { count: p.added }) : "", p.removed > 0 ? t("projects.put.done_removed", { count: p.removed }) : "", p.moved > 0 ? t("projects.put.done_moved", { count: p.moved }) : ""].filter(Boolean);
      return t("projects.put.done_piece", { name: p.project_name, parts: parts.join(", ") });
    })
    .join(" · ");
  let message = t("projects.put.done_assign", { detail });
  if (out.unchanged > 0) message += ` ${t("projects.put.done_unchanged", { count: out.unchanged })}`;
  if (out.missing_projects.length > 0) message += ` ${t("projects.put.done_missing", { count: out.missing_projects.length })}`;
  return message;
}

function refreshAfter(out: AssignOutcome): void {
  invalidateProjects(out.projects.map((p) => p.project_id));
  invalidateReference();
  void queryClient.invalidateQueries({ queryKey: queryKeys.documents });
}

/**
 * "프로젝트에 넣기" (0.5.0 plan §2; 1.0.0 plan §12 / §12-B). Target = projects (as 업무 문서 or as their 참고 자료)
 * or the common reference material (a group, new or existing). Projects are ticked, several at once:
 * "모두 같은 프로젝트에" puts every document into every ticked project; "문서별로 지정" opens the sheet where each
 * document has its own projects and kind and a project it is already in can be unticked to take it out.
 * Either way one `project_assign` request applies it, and the toast offers to undo it.
 * "자동 분류 제안" runs the classify preview: the suggested project is ticked — and the second candidate from
 * the 2순위 추천 임계값 on —, per document when the suggestions differ.
 */
export function PutInProjectDialog({
  open,
  onOpenChange,
  docIds,
  defaultProjectId = null,
  defaultTarget = "project",
  autoSuggest = false,
  onDone,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  docIds: string[];
  defaultProjectId?: string | null;
  /** Pre-selected target (`common` for the old "지식 베이스에 추가" entry points). */
  defaultTarget?: PutTarget;
  /** Run the classify preview as soon as the dialog opens (the library's "프로젝트로 분류"). */
  autoSuggest?: boolean;
  onDone?: (result: PutResult) => void;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const currentProject = useProjectsStore((s) => s.currentProject);
  const threshold = useSettings()?.processing.project_second_choice ?? DEFAULT_SECOND_CHOICE;
  const { data: projects } = useProjects(open);
  const { groups } = useReferenceGroups(open);
  const [target, setTarget] = useState<PutTarget>("project");
  const [mode, setMode] = useState<PutMode>("same");
  const [kind, setKind] = useState<PutKind>("work");
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState("");
  const [newProject, setNewProject] = useState<{ open: boolean; name: string }>({ open: false, name: "" });
  const [groupId, setGroupId] = useState<string>(NEW_GROUP);
  const [groupTouched, setGroupTouched] = useState(false);
  const [newGroupName, setNewGroupName] = useState("");
  const [busy, setBusy] = useState<"suggest" | "create" | "confirm" | null>(null);
  const [suggestion, setSuggestion] = useState<ClassifyResult | null>(null);
  const [wantSuggest, setWantSuggest] = useState(false);
  const [rows, setRows] = useState<SheetRow[] | null>(null);
  const [pendingFill, setPendingFill] = useState<ClassifyResult | null>(null);
  const [fillNote, setFillNote] = useState<string | null>(null);
  const [askRemovals, setAskRemovals] = useState(false);

  useEffect(() => {
    if (!open) return;
    // Only a project the caller names is ticked. The current project is listed first instead of being
    // ticked: with several ticks possible, a tick nobody made would put the documents in one project more.
    const first = defaultProjectId;
    setTarget(defaultTarget);
    setMode("same");
    setKind(defaultTarget === "common" ? "reference" : "work");
    setPicked(new Set(first ? [first] : []));
    setQuery("");
    setNewProject({ open: false, name: "" });
    setGroupId(NEW_GROUP);
    setGroupTouched(false);
    setNewGroupName("");
    setSuggestion(null);
    setBusy(null);
    setWantSuggest(autoSuggest);
    setRows(null);
    setPendingFill(null);
    setFillNote(null);
    setAskRemovals(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, docIds]);
  // The first common group is the default pick once the list is known (never after the user chose).
  useEffect(() => {
    if (open && !groupTouched && groups && groups.length > 0 && groupId === NEW_GROUP) setGroupId(groups[0]!.id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, groups === undefined]);

  const sheet = target === "project" && mode === "sheet";
  const { data: library } = useDocuments(SHEET_DOCS, open && sheet);
  const sheetDocs = useMemo<DocumentView[] | null>(() => {
    if (!library) return null;
    const byId = new Map(library.map((d) => [d.id, d]));
    return docIds.flatMap((id) => byId.get(id) ?? []);
  }, [library, docIds]);
  // The rows are built once per opening: a refetch of the library must not throw the user's ticks away.
  useEffect(() => {
    if (open && sheet && rows === null && sheetDocs) setRows(sheetRows(sheetDocs));
  }, [open, sheet, rows, sheetDocs]);
  // A suggestion that arrived before the rows (or that switched to the sheet) fills them once they exist.
  useEffect(() => {
    if (!rows || !pendingFill) return;
    const filled = fillFromSuggestions(rows, pendingFill, threshold);
    const cells = filled.rows.reduce((n, r, i) => n + r.ticked.length - (rows[i]?.ticked.length ?? 0), 0);
    setRows(filled.rows);
    setPicked((prev) => new Set([...prev, ...filled.projects]));
    setFillNote(cells > 0 ? t("projects.put.suggest_filled", { count: cells }) : t("projects.put.suggest_filled_none"));
    setPendingFill(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rows, pendingFill]);

  const list = useMemo(() => {
    const q = query.trim().toLowerCase();
    const found = (projects ?? []).filter((p) => !q || p.name.toLowerCase().includes(q) || (p.intake_no ?? "").toLowerCase().includes(q) || (p.owner ?? "").toLowerCase().includes(q));
    return [...found.filter((p) => p.id === currentProject), ...found.filter((p) => p.id !== currentProject)];
  }, [projects, query, currentProject]);
  /** The ticked projects in the order of the project list. */
  const pickedProjects = useMemo(() => (projects ?? []).filter((p) => picked.has(p.id)), [projects, picked]);
  const columns = useMemo(() => (rows ? sheetColumns(rows, pickedProjects.map((p) => p.id)) : []), [rows, pickedProjects]);
  /** A project a picked document is in keeps its column: no membership is hidden from the sheet. */
  const locked = useMemo(() => new Set((rows ?? []).flatMap((r) => Object.keys(r.before))), [rows]);
  const summary = useMemo(() => (rows ? sheetSummary(rows) : null), [rows]);
  const projectName = (id: string) => projects?.find((p) => p.id === id)?.name ?? id;
  const docName = (id: string) => {
    const d = sheetDocs?.find((x) => x.id === id);
    return d ? d.title?.trim() || d.original_name : id;
  };
  const ticked = (id: string) => (sheet ? columns.includes(id) : picked.has(id));

  /** Ticks / unticks projects in the list. In the sheet an unticked project loses its column and the new ticks in it. */
  const setProjects = (next: Set<string>) => {
    if (sheet && rows) {
      let r = rows;
      for (const id of columns) if (!next.has(id) && !locked.has(id)) r = setCells(r, r.map((x) => x.id), id, false);
      if (r !== rows) setRows(r);
    }
    setPicked(next);
  };
  const toggleProject = (id: string, on: boolean) => {
    const next = new Set(sheet ? columns : picked);
    if (on) next.add(id);
    else next.delete(id);
    setProjects(next);
  };

  const suggest = async () => {
    setBusy("suggest");
    try {
      const r = await api.projectClassifyPreview({ doc_ids: docIds, use_model: false, include_assigned: true });
      setSuggestion(r);
      setFillNote(null);
      const sets = r.suggestions.map((s) => suggestedProjects(s, threshold)).filter((x) => x.length > 0);
      const distinct = new Set(sets.map((x) => [...x].sort().join("\n")));
      setTarget("project");
      if (sheet) {
        setPendingFill(r);
      } else if (docIds.length > 1 && distinct.size > 1) {
        // The documents point to different projects: only the sheet can hold that.
        setMode("sheet");
        setPendingFill(r);
        toast(t("projects.put.suggest_switched"), "info");
      } else if (sets[0]) {
        setPicked(new Set(sets[0]));
      }
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(null);
    }
  };
  useEffect(() => {
    if (!open || !wantSuggest || !projects) return;
    setWantSuggest(false);
    if (projects.length > 0 && docIds.length > 0) void suggest();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, wantSuggest, projects]);

  const createProject = async () => {
    const name = newProject.name.trim();
    if (!name) return;
    setBusy("create");
    try {
      const p = await api.projectCreate({ name });
      invalidateProjects([p.id]);
      // A project made here is the one these documents are for; in the sheet it joins the columns.
      setPicked(sheet ? new Set([...columns, p.id]) : new Set([p.id]));
      setTarget("project");
      setNewProject({ open: false, name: "" });
      toast(t("projects.put.created", { name: p.name }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(null);
    }
  };

  const request: AssignRequest | null = target !== "project" ? null : sheet ? (rows ? sheetRequest(rows) : null) : sameForAllRequest(docIds, pickedProjects.map((p) => p.id), kind);
  const canConfirm =
    docIds.length > 0 && busy === null && (target === "project" ? !!request && request.add.length + request.remove.length > 0 : groupId !== NEW_GROUP || !!newGroupName.trim());

  const undo = async (out: AssignOutcome) => {
    try {
      refreshAfter(await api.projectAssign(out.undo));
      toast(t("projects.put.undone"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const assign = async (req: AssignRequest) => {
    const out = await api.projectAssign(req);
    refreshAfter(out);
    const single: Project | null = !sheet && pickedProjects.length === 1 ? pickedProjects[0]! : null;
    const changed = out.added + out.removed + out.moved;
    if (single && changed > 0) {
      // One project, as before: say what went in and offer to open it.
      const openTo = kind === "work" ? `${SCREEN_ROUTES.projects}/${single.id}` : `${SCREEN_ROUTES.projects}/${single.id}?tab=documents`;
      toast(t(kind === "work" ? "projects.put.done_work" : "projects.put.done_reference", { name: single.name, count: changed }), "ok", { label: t("projects.put.open_target"), onClick: () => navigate(openTo) });
    } else {
      toast(outcomeMessage(out, t), changed > 0 ? "ok" : "info", changed > 0 ? { label: t("projects.put.undo"), onClick: () => void undo(out) } : undefined);
    }
    const ids = out.projects.map((p) => p.project_id);
    return { target, kind, projectId: ids.length === 1 ? ids[0]! : null, projectIds: ids, groupId: null, count: docIds.length } satisfies PutResult;
  };
  const confirm = async () => {
    if (!canConfirm) return;
    // Taking documents out of a project is confirmed on its own, with the pairs spelled out.
    if (sheet && summary && summary.remove > 0 && !askRemovals) return setAskRemovals(true);
    setBusy("confirm");
    try {
      let result: PutResult;
      if (target === "project" && request) {
        result = await assign(request);
      } else {
        let gid = groupId;
        let gname = groups?.find((g) => g.id === groupId)?.name ?? "";
        if (gid === NEW_GROUP) {
          const created = await api.knowledgeCollectionSave({ name: newGroupName.trim() });
          gid = created.id;
          gname = created.name;
        }
        await addToReferenceGroup(gid, docIds);
        result = { target: "common", kind: "reference", projectId: null, projectIds: [], groupId: gid, count: docIds.length };
        void queryClient.invalidateQueries({ queryKey: queryKeys.documents });
        toast(t("projects.put.done_reference", { name: gname, count: docIds.length }), "ok", { label: t("projects.put.open_target"), onClick: () => navigate(`${SCREEN_ROUTES.projects}/${COMMON_REFERENCE_ID}`) });
      }
      onOpenChange(false);
      onDone?.(result);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(null);
    }
  };

  const choice = (selected: boolean) => cn("flex cursor-pointer items-center gap-2 rounded-md border px-3 py-2 text-sm", selected ? "border-accent bg-accent-soft" : "border-border hover:border-accent");
  const legend = "text-xs font-semibold uppercase tracking-wide text-fg-subtle";
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={sheet ? "max-w-5xl" : "max-w-2xl"} closeLabel={t("common.close")} data-testid="put-dialog" data-mode={target === "project" ? mode : "common"}>
        <DialogHeader>
          <DialogTitle>{askRemovals ? t("projects.put.remove_confirm_title", { count: summary?.remove ?? 0 }) : t("projects.put.title")}</DialogTitle>
          <DialogDescription>{askRemovals ? t("projects.put.remove_confirm_body") : t("projects.put.hint", { count: docIds.length })}</DialogDescription>
        </DialogHeader>
        {askRemovals ? (
          <ul className="max-h-72 min-w-0 divide-y divide-border overflow-y-auto rounded-md border border-border text-sm" data-testid="put-remove-confirm">
            {(summary?.removals ?? []).map((r) => (
              <li key={`${r.doc_id}\n${r.project_id}`} className="flex min-w-0 items-center gap-2 px-3 py-1.5">
                <span className="min-w-0 flex-1 truncate" title={docName(r.doc_id)}>
                  {docName(r.doc_id)}
                </span>
                <Badge variant="outline" className="max-w-[12rem] shrink-0 truncate" title={projectName(r.project_id)}>
                  {projectName(r.project_id)}
                </Badge>
              </li>
            ))}
          </ul>
        ) : (
          <div className="flex min-w-0 flex-col gap-4">
            <fieldset className="flex flex-col gap-2">
              <legend className={legend}>{t("projects.put.target")}</legend>
              <div className="grid gap-2 sm:grid-cols-2">
                <label className={choice(target === "project")}>
                  <input type="radio" name="put-target" checked={target === "project"} onChange={() => setTarget("project")} data-testid="put-target-project" />
                  <Briefcase className="size-4 shrink-0" aria-hidden="true" />
                  {t("projects.put.target_project")}
                </label>
                <label className={choice(target === "common")}>
                  <input
                    type="radio"
                    name="put-target"
                    checked={target === "common"}
                    onChange={() => {
                      setTarget("common");
                      setKind("reference");
                    }}
                    data-testid="put-target-common"
                  />
                  <BookOpen className="size-4 shrink-0" aria-hidden="true" />
                  {t("projects.put.target_common")}
                </label>
              </div>
            </fieldset>
            {target === "project" && docIds.length > 1 && (
              <fieldset className="flex flex-col gap-2">
                <legend className={legend}>{t("projects.put.mode")}</legend>
                <div className="grid gap-2 sm:grid-cols-2">
                  <label className={choice(mode === "same")}>
                    <input type="radio" name="put-mode" checked={mode === "same"} onChange={() => setMode("same")} data-testid="put-mode-same" />
                    <span className="flex min-w-0 flex-col">
                      <span>{t("projects.put.mode_same")}</span>
                      <span className="text-xs text-fg-subtle">{t("projects.put.mode_same_hint")}</span>
                    </span>
                  </label>
                  <label className={choice(mode === "sheet")}>
                    <input type="radio" name="put-mode" checked={mode === "sheet"} onChange={() => setMode("sheet")} data-testid="put-mode-sheet" />
                    <span className="flex min-w-0 flex-col">
                      <span>{t("projects.put.mode_sheet")}</span>
                      <span className="text-xs text-fg-subtle">{t("projects.put.mode_sheet_hint")}</span>
                    </span>
                  </label>
                </div>
              </fieldset>
            )}
            {target === "project" ? (
              <div className="flex min-w-0 flex-col gap-2">
                <span className={legend}>{t("projects.put.projects_pick")}</span>
                <div className="flex flex-wrap items-center gap-2">
                  <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("projects.put.search")} aria-label={t("projects.put.search")} className="h-8 min-w-40 flex-1" data-testid="put-project-search" />
                  <Button size="sm" variant="outline" onClick={() => void suggest()} disabled={busy !== null || docIds.length === 0 || (projects?.length ?? 0) === 0} title={t("projects.classify.hint")} data-testid="put-suggest">
                    {busy === "suggest" ? <Spinner className="size-4" /> : <Sparkles />}
                    {t("projects.put.suggest")}
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setNewProject((s) => ({ ...s, open: !s.open }))} aria-expanded={newProject.open} data-testid="put-project-new">
                    <Plus />
                    {t("projects.put.new_project")}
                  </Button>
                </div>
                {newProject.open && (
                  <div className="flex items-center gap-2">
                    <Input value={newProject.name} onChange={(e) => setNewProject((s) => ({ ...s, name: e.target.value }))} placeholder={t("projects.put.new_project_name")} aria-label={t("projects.put.new_project_name")} className="h-8" data-testid="put-project-new-name" />
                    <Button size="sm" onClick={() => void createProject()} disabled={busy !== null || !newProject.name.trim()} data-testid="put-project-create">
                      {busy === "create" ? <Spinner className="size-4" /> : null}
                      {t("projects.put.create")}
                    </Button>
                  </div>
                )}
                {suggestion && !sheet && (
                  <div className="rounded-md border border-border bg-bg-sunken px-3 py-2 text-xs" data-testid="put-suggestions" data-suggested={suggestion.suggested}>
                    {suggestion.suggested === 0 ? (
                      <span className="text-fg-muted">{t("projects.put.suggested_none")}</span>
                    ) : (
                      <ul className="flex max-h-28 flex-col gap-0.5 overflow-y-auto">
                        {suggestion.suggestions.map((s) => (
                          <li key={s.doc_id} className="flex flex-wrap items-center gap-1" data-testid={`put-suggestion-${s.doc_id}`} data-suggested={s.project_id ?? ""}>
                            <span className="min-w-0 flex-1 truncate">{s.title}</span>
                            <span className="text-fg-muted">
                              → {s.project_name ?? t("projects.classify.undecided")}
                              {s.project_id ? ` (${Math.round(s.confidence * 100)}%)` : ""}
                            </span>
                          </li>
                        ))}
                      </ul>
                    )}
                  </div>
                )}
                {sheet && fillNote && (
                  <p className="rounded-md border border-border bg-bg-sunken px-3 py-2 text-xs text-fg-muted" data-testid="put-fill-note">
                    {fillNote}
                  </p>
                )}
                <SelectAllBar visibleIds={list.map((p) => p.id)} picked={new Set(sheet ? columns : picked)} onChange={setProjects} disabled={busy !== null} testId="put-projects-select" />
                <div className={cn("overflow-y-auto rounded-md border border-border", sheet ? "max-h-32" : "max-h-56")} role="group" aria-label={t("projects.put.projects_pick")}>
                  {(projects?.length ?? 0) === 0 ? (
                    <p className="p-3 text-sm text-fg-muted" data-testid="put-no-projects">
                      {t("projects.put.no_projects")}
                    </p>
                  ) : list.length === 0 ? (
                    <p className="p-3 text-sm text-fg-muted">{t("projects.put.no_match")}</p>
                  ) : (
                    <ul className="divide-y divide-border text-sm">
                      {list.map((p) => {
                        const fixed = sheet && locked.has(p.id);
                        return (
                          <li key={p.id}>
                            <label className={cn("flex cursor-pointer items-center gap-3 px-3 py-2 hover:bg-bg-sunken", ticked(p.id) && "bg-accent-soft")} title={fixed ? t("projects.put.column_locked") : undefined}>
                              <input type="checkbox" name="put-project" checked={ticked(p.id)} disabled={fixed} onChange={(e) => toggleProject(p.id, e.target.checked)} data-testid={`put-project-${p.id}`} />
                              <span className="min-w-0 flex-1 truncate font-medium">{p.name}</span>
                              {p.id === currentProject && (
                                <Badge variant="info" className="shrink-0" data-testid="put-project-current">
                                  {t("projects.put.current")}
                                </Badge>
                              )}
                              <Badge variant="outline" className="shrink-0">
                                {t(`projects.phase.${p.phase}`)}
                              </Badge>
                              <span className="shrink-0 text-xs text-fg-subtle">{t("projects.card.documents", { count: p.document_count })}</span>
                            </label>
                          </li>
                        );
                      })}
                    </ul>
                  )}
                </div>
                {sheet && <p className="text-xs text-fg-subtle">{t("projects.put.columns_hint")}</p>}
              </div>
            ) : (
              <div className="flex flex-col gap-2">
                <p className="text-xs text-fg-muted">{t("projects.put.common_hint")}</p>
                <Select
                  value={groupId}
                  onValueChange={(v) => {
                    setGroupTouched(true);
                    setGroupId(v);
                  }}
                >
                  <SelectTrigger aria-label={t("projects.put.group")} data-testid="put-group-select">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {(groups ?? []).map((g) => (
                      <SelectItem key={g.id} value={g.id} data-testid={`put-group-${g.id}`}>
                        {g.name}
                      </SelectItem>
                    ))}
                    <SelectItem value={NEW_GROUP} data-testid="put-group-new">
                      {t("projects.put.new_group")}
                    </SelectItem>
                  </SelectContent>
                </Select>
                {groupId === NEW_GROUP && <Input value={newGroupName} onChange={(e) => setNewGroupName(e.target.value)} placeholder={t("projects.put.new_group_name")} aria-label={t("projects.put.new_group_name")} data-testid="put-group-new-name" />}
              </div>
            )}
            {sheet ? (
              rows && sheetDocs ? (
                <>
                  <AssignSheet docs={sheetDocs} rows={rows} onRows={setRows} columns={columns} projects={projects ?? []} onColumn={(id) => setPicked((prev) => new Set([...prev, id]))} disabled={busy !== null} />
                  {summary && (
                    <div className="flex min-w-0 flex-col gap-1" data-testid="put-summary" data-add={summary.add} data-remove={summary.remove} data-move={summary.move} data-unassigned={summary.unassigned}>
                      <span className={legend}>{t("projects.put.summary")}</span>
                      <div className="flex min-w-0 flex-wrap gap-1 text-xs">
                        {summary.add + summary.remove + summary.move === 0 && <span className="text-fg-muted">{t("projects.put.summary_none")}</span>}
                        {summary.projects.flatMap((p) => {
                          const name = projectName(p.id);
                          const chip = (key: string, text: string, tone?: string) => (
                            <Badge key={`${p.id}-${key}`} variant="outline" className={cn("max-w-full truncate", tone)} title={text} data-testid={`put-summary-${key}-${p.id}`}>
                              {text}
                            </Badge>
                          );
                          return [
                            p.add > 0 ? chip("add", t("projects.put.summary_add", { name, count: p.add })) : null,
                            p.remove > 0 ? chip("remove", t("projects.put.summary_remove", { name, count: p.remove }), "border-danger text-danger") : null,
                            p.move > 0 ? chip("move", t("projects.put.summary_move", { name, count: p.move })) : null,
                          ];
                        })}
                        {summary.unassigned > 0 && (
                          <Badge variant="outline" className="max-w-full truncate text-fg-muted" data-testid="put-summary-unassigned">
                            {t("projects.put.summary_unassigned", { count: summary.unassigned })}
                          </Badge>
                        )}
                      </div>
                    </div>
                  )}
                </>
              ) : (
                <p className="flex items-center gap-2 text-sm text-fg-muted" data-testid="put-sheet-loading">
                  <Spinner className="size-4" />
                  {t("projects.put.sheet_loading")}
                </p>
              )
            ) : (
              <fieldset className="flex flex-col gap-2">
                <legend className={legend}>{t("projects.put.kind")}</legend>
                <div className="grid gap-2 sm:grid-cols-2">
                  <label className={cn(choice(kind === "work"), target === "common" && "cursor-not-allowed opacity-50")}>
                    <input type="radio" name="put-kind" checked={kind === "work"} disabled={target === "common"} onChange={() => setKind("work")} data-testid="put-kind-work" />
                    <span className="flex min-w-0 flex-col">
                      <span>{t("projects.put.kind_work")}</span>
                      <span className="text-xs text-fg-subtle">{t("projects.put.kind_work_hint")}</span>
                    </span>
                  </label>
                  <label className={choice(kind === "reference")}>
                    <input type="radio" name="put-kind" checked={kind === "reference"} onChange={() => setKind("reference")} data-testid="put-kind-reference" />
                    <span className="flex min-w-0 flex-col">
                      <span>{t("projects.put.kind_reference")}</span>
                      <span className="text-xs text-fg-subtle">{t("projects.put.kind_reference_hint")}</span>
                    </span>
                  </label>
                </div>
                {target === "common" && <p className="text-xs text-fg-subtle">{t("projects.put.kind_reference_only")}</p>}
              </fieldset>
            )}
          </div>
        )}
        <DialogFooter>
          {askRemovals ? (
            <>
              <Button variant="secondary" onClick={() => setAskRemovals(false)} disabled={busy !== null} data-testid="put-remove-back">
                {t("projects.put.remove_confirm_back")}
              </Button>
              <Button variant="danger" onClick={() => void confirm()} disabled={!canConfirm} data-testid="put-remove-ok">
                {busy === "confirm" ? <Spinner className="size-4" /> : null}
                {t("projects.put.remove_confirm_ok")}
              </Button>
            </>
          ) : (
            <>
              <Button variant="secondary" onClick={() => onOpenChange(false)}>
                {t("common.cancel")}
              </Button>
              <Button onClick={() => void confirm()} disabled={!canConfirm} data-testid="put-confirm">
                {busy === "confirm" ? <Spinner className="size-4" /> : null}
                {t("projects.put.confirm")}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
