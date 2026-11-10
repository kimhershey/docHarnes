import { BookOpen, Briefcase, Plus, Sparkles } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { addToReferenceGroup, useReferenceGroups } from "@/features/knowledge/reference";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects, useProjects } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import { COMMON_REFERENCE_ID, SCREEN_ROUTES, type ClassifyResult } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useProjectsStore } from "@/stores/projects";
import { useUiStore } from "@/stores/ui";

const NEW_GROUP = "__new__";

export type PutTarget = "project" | "common";
export type PutKind = "work" | "reference";

/** What "프로젝트에 넣기" did, for callers that follow up (clear a selection, refresh a badge …). */
export interface PutResult {
  target: PutTarget;
  kind: PutKind;
  projectId: string | null;
  groupId: string | null;
  count: number;
}

/**
 * "프로젝트에 넣기" (0.5.0 plan §2): the one dialog behind the old 지식 베이스에 추가 / 프로젝트에 추가 / 프로젝트로 분류.
 * Target = a project (as 업무 문서 or as its 참고 자료) or the common reference material (a group, new or
 * existing); "자동 분류 제안" runs the classify preview and pre-selects the project most documents point to.
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
  const { data: projects } = useProjects(open);
  const { groups } = useReferenceGroups(open);
  const [target, setTarget] = useState<PutTarget>("project");
  const [kind, setKind] = useState<PutKind>("work");
  const [projectId, setProjectId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [newProject, setNewProject] = useState<{ open: boolean; name: string }>({ open: false, name: "" });
  const [groupId, setGroupId] = useState<string>(NEW_GROUP);
  const [groupTouched, setGroupTouched] = useState(false);
  const [newGroupName, setNewGroupName] = useState("");
  const [busy, setBusy] = useState<"suggest" | "create" | "confirm" | null>(null);
  const [suggestion, setSuggestion] = useState<ClassifyResult | null>(null);
  const [wantSuggest, setWantSuggest] = useState(false);

  useEffect(() => {
    if (!open) return;
    setTarget(defaultTarget);
    setKind(defaultTarget === "common" ? "reference" : "work");
    setProjectId(defaultProjectId ?? currentProject ?? null);
    setQuery("");
    setNewProject({ open: false, name: "" });
    setGroupId(NEW_GROUP);
    setGroupTouched(false);
    setNewGroupName("");
    setSuggestion(null);
    setBusy(null);
    setWantSuggest(autoSuggest);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, docIds]);
  // The first common group is the default pick once the list is known (never after the user chose).
  useEffect(() => {
    if (open && !groupTouched && groups && groups.length > 0 && groupId === NEW_GROUP) setGroupId(groups[0]!.id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, groups === undefined]);

  const list = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (projects ?? []).filter((p) => !q || p.name.toLowerCase().includes(q) || (p.intake_no ?? "").toLowerCase().includes(q) || (p.owner ?? "").toLowerCase().includes(q));
  }, [projects, query]);
  const project = projects?.find((p) => p.id === projectId) ?? null;

  const suggest = async () => {
    setBusy("suggest");
    try {
      const r = await api.projectClassifyPreview({ doc_ids: docIds, use_model: false, include_assigned: true });
      setSuggestion(r);
      // The project most documents point to (confidence-weighted) becomes the pick.
      const weight = new Map<string, number>();
      for (const s of r.suggestions) if (s.project_id) weight.set(s.project_id, (weight.get(s.project_id) ?? 0) + s.confidence);
      const best = [...weight.entries()].sort((a, b) => b[1] - a[1])[0];
      if (best) {
        setTarget("project");
        setProjectId(best[0]);
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
      setProjectId(p.id);
      setTarget("project");
      setNewProject({ open: false, name: "" });
      toast(t("projects.put.created", { name: p.name }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(null);
    }
  };

  const canConfirm = docIds.length > 0 && busy === null && (target === "project" ? !!project : groupId !== NEW_GROUP || !!newGroupName.trim());
  const confirm = async () => {
    if (!canConfirm) return;
    setBusy("confirm");
    try {
      let result: PutResult;
      let message: string;
      let openTo: string;
      if (target === "project" && project) {
        if (kind === "work") {
          await api.projectAddDocuments(project.id, docIds);
          result = { target, kind, projectId: project.id, groupId: null, count: docIds.length };
          message = t("projects.put.done_work", { name: project.name, count: docIds.length });
          openTo = `${SCREEN_ROUTES.projects}/${project.id}`;
        } else {
          const group = await api.projectReferenceGroup(project.id);
          await addToReferenceGroup(group.id, docIds);
          result = { target, kind, projectId: project.id, groupId: group.id, count: docIds.length };
          message = t("projects.put.done_reference", { name: project.name, count: docIds.length });
          openTo = `${SCREEN_ROUTES.projects}/${project.id}?tab=reference`;
        }
        invalidateProjects([project.id]);
      } else {
        let gid = groupId;
        let gname = groups?.find((g) => g.id === groupId)?.name ?? "";
        if (gid === NEW_GROUP) {
          const created = await api.knowledgeCollectionSave({ name: newGroupName.trim() });
          gid = created.id;
          gname = created.name;
        }
        await addToReferenceGroup(gid, docIds);
        result = { target: "common", kind: "reference", projectId: null, groupId: gid, count: docIds.length };
        message = t("projects.put.done_reference", { name: gname, count: docIds.length });
        openTo = `${SCREEN_ROUTES.projects}/${COMMON_REFERENCE_ID}`;
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.documents });
      toast(message, "ok", { label: t("projects.put.open_target"), onClick: () => navigate(openTo) });
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
      <DialogContent className="max-w-2xl" closeLabel={t("common.close")} data-testid="put-dialog">
        <DialogHeader>
          <DialogTitle>{t("projects.put.title")}</DialogTitle>
          <DialogDescription>{t("projects.put.hint", { count: docIds.length })}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-4">
          <fieldset className="flex flex-col gap-2">
            <legend className={legend}>{t("projects.put.target")}</legend>
            <div className="grid gap-2 sm:grid-cols-2">
              <label className={choice(target === "project")}>
                <input
                  type="radio"
                  name="put-target"
                  checked={target === "project"}
                  onChange={() => setTarget("project")}
                  data-testid="put-target-project"
                />
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
          {target === "project" ? (
            <div className="flex flex-col gap-2">
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
              {suggestion && (
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
              <div className="max-h-56 overflow-y-auto rounded-md border border-border" role="radiogroup" aria-label={t("projects.put.target_project")}>
                {(projects?.length ?? 0) === 0 ? (
                  <p className="p-3 text-sm text-fg-muted" data-testid="put-no-projects">
                    {t("projects.put.no_projects")}
                  </p>
                ) : list.length === 0 ? (
                  <p className="p-3 text-sm text-fg-muted">{t("projects.put.no_match")}</p>
                ) : (
                  <ul className="divide-y divide-border text-sm">
                    {list.map((p) => (
                      <li key={p.id}>
                        <label className={cn("flex cursor-pointer items-center gap-3 px-3 py-2 hover:bg-bg-sunken", projectId === p.id && "bg-accent-soft")}>
                          <input type="radio" name="put-project" checked={projectId === p.id} onChange={() => setProjectId(p.id)} data-testid={`put-project-${p.id}`} />
                          <span className="min-w-0 flex-1 truncate font-medium">{p.name}</span>
                          <Badge variant="outline">{t(`projects.phase.${p.phase}`)}</Badge>
                          <span className="text-xs text-fg-subtle">{t("projects.card.documents", { count: p.document_count })}</span>
                        </label>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
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
        </div>
        <DialogFooter>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button onClick={() => void confirm()} disabled={!canConfirm} data-testid="put-confirm">
            {busy === "confirm" ? <Spinner className="size-4" /> : null}
            {t("projects.put.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
