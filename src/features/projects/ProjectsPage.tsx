import { AlertTriangle, Briefcase, LayoutGrid, List, Plus, Settings2, Sparkles } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { DropHint, importDroppedFiles, useDocumentDropTarget, type DragDoc } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects, useProjects, useProjectTemplates } from "@/lib/queries";
import { PROJECT_PHASES, type Project, type ProjectPhase } from "@/lib/types";
import { cn, formatDate } from "@/lib/utils";
import { useAttachmentsStore } from "@/stores/attachments";
import { useProjectsStore } from "@/stores/projects";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

import { ClassifyDialog } from "./ClassifyDialog";
import { NewProjectDialog } from "./NewProjectDialog";
import { PickProjectDialog } from "./PickProjectDialog";
import { TemplatesDialog } from "./TemplatesDialog";

export function phaseVariant(p: ProjectPhase): "ok" | "warn" | "danger" | "muted" | "info" {
  switch (p) {
    case "done":
      return "ok";
    case "active":
      return "info";
    default:
      return "muted";
  }
}

/** Badges of the computed phase and the 기한 경과 flag (plan §5, decision 5). */
export function PhaseBadges({ project, compact }: { project: Pick<Project, "phase" | "overdue">; compact?: boolean }) {
  const { t } = useTranslation();
  return (
    <>
      <Badge variant={phaseVariant(project.phase)} data-testid="project-phase" data-phase={project.phase}>
        {t(`projects.phase.${project.phase}`)}
      </Badge>
      {project.overdue && (
        <Badge variant="danger" title={t("projects.badge.overdue_hint")} data-testid="project-overdue">
          <AlertTriangle className={cn("size-3", compact && "hidden")} aria-hidden="true" />
          {t("projects.badge.overdue")}
        </Badge>
      )}
    </>
  );
}

/** "시작 2026-02-01 · 종료 2027-01-31" (or 종료일 없음). */
export function DatesLine({ project }: { project: Pick<Project, "start_date" | "end_date"> }) {
  const { t } = useTranslation();
  return (
    <>
      {project.start_date && <span data-testid="project-start-date">{t("projects.card.start", { date: project.start_date })}</span>}
      <span data-testid="project-end-date">{project.end_date ? t("projects.card.end", { date: project.end_date }) : t("projects.card.open_ended")}</span>
    </>
  );
}

type DropHandlers = { onDropDocs: (p: Project, docs: DragDoc[]) => void; onDropFiles: (p: Project, paths: string[]) => void };

function ProjectCard({ project, templateName, compact, onDropDocs, onDropFiles }: { project: Project; templateName: string | null; compact: boolean } & DropHandlers) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const drop = useDocumentDropTarget({ onDocuments: (docs) => onDropDocs(project, docs), onFiles: (paths) => onDropFiles(project, paths) });
  return (
    <li className={cn("relative flex flex-col gap-1.5 rounded-lg border border-border bg-bg-elevated p-3 text-sm", drop.isOver && "border-accent")} data-testid={`project-card-${project.id}`} data-project-phase={project.phase} data-project-overdue={project.overdue} data-project-name={project.name} {...drop.props}>
      <DropHint active={drop.isActive} over={drop.isOver} className="text-[0.6875rem]" testId={`project-card-drop-${project.id}`}>
        {t("projects.card.drop_add", { name: project.name })}
      </DropHint>
      <div className="flex items-start gap-2">
        <button type="button" className="min-w-0 flex-1 text-left font-medium hover:underline cursor-pointer" onClick={() => navigate(`/projects/${project.id}`)} data-testid="project-open">
          {project.name}
        </button>
        <span className="flex flex-wrap justify-end gap-1">
          {!compact && <PhaseBadges project={project} compact />}
          {compact && project.overdue && (
            <Badge variant="danger" title={t("projects.badge.overdue_hint")} data-testid="project-overdue">
              {t("projects.badge.overdue")}
            </Badge>
          )}
        </span>
      </div>
      <div className="flex flex-wrap gap-x-3 gap-y-0.5 text-xs text-fg-muted">
        {project.intake_no && <span>{t("projects.card.intake_no", { no: project.intake_no })}</span>}
        {project.owner && <span>{t("projects.card.owner", { owner: project.owner })}</span>}
        <DatesLine project={project} />
        {templateName && <span>{templateName}</span>}
      </div>
      <div className="flex flex-wrap items-center gap-1.5 text-xs">
        <Badge variant="outline">{t("projects.card.documents", { count: project.document_count })}</Badge>
        {project.missing_count > 0 && (
          <Badge variant="danger" data-testid="project-missing">
            {t("projects.card.missing", { count: project.missing_count })}
          </Badge>
        )}
        <span className="text-fg-subtle" data-testid="project-verdicts">
          {project.last_check_at ? t("projects.card.verdicts", { pass: project.last_pass, mismatch: project.last_mismatch, undetermined: project.last_undetermined }) : t("projects.card.no_check")}
        </span>
        <span className="ml-auto text-[0.6875rem] text-fg-subtle">{formatDate(project.updated_at, i18n.language)}</span>
      </div>
    </li>
  );
}

function ProjectRow({ p, onDropDocs, onDropFiles }: { p: Project } & DropHandlers) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const drop = useDocumentDropTarget({ onDocuments: (docs) => onDropDocs(p, docs), onFiles: (paths) => onDropFiles(p, paths) });
  return (
    <tr key={p.id} className={cn("cursor-pointer hover:bg-bg-sunken", drop.isActive && "outline-dashed outline-1 outline-offset-[-1px] outline-border", drop.isOver && "bg-accent-soft outline-accent")} {...drop.props} onClick={() => navigate(`/projects/${p.id}`)} data-testid={`project-row-${p.id}`} data-project-phase={p.phase}>
      <td className="py-2 pr-3 font-medium">
        {/* The row click is a mouse shortcut; the name is the keyboard-reachable control. */}
        <button
          type="button"
          className="text-left hover:underline cursor-pointer"
          onClick={(e) => {
            e.stopPropagation();
            navigate(`/projects/${p.id}`);
          }}
          data-testid="project-row-open"
        >
          {p.name}
        </button>
      </td>
      <td className="py-2 pr-3">
        <span className="flex flex-wrap gap-1">
          <PhaseBadges project={p} compact />
        </span>
      </td>
      <td className="py-2 pr-3 text-fg-muted">{p.intake_no ?? "—"}</td>
      <td className="py-2 pr-3 text-fg-muted">{p.owner ?? "—"}</td>
      <td className="py-2 pr-3 text-fg-muted">{p.start_date ?? "—"}</td>
      <td className="py-2 pr-3 text-fg-muted">{p.end_date ?? "—"}</td>
      <td className="py-2 pr-3">
        {p.document_count}
        {p.missing_count > 0 && (
          <Badge variant="danger" className="ml-2">
            {t("projects.card.missing", { count: p.missing_count })}
          </Badge>
        )}
      </td>
      <td className="py-2 pr-3 text-fg-muted">{p.last_check_at ? t("projects.card.verdicts", { pass: p.last_pass, mismatch: p.last_mismatch, undetermined: p.last_undetermined }) : t("projects.card.no_check")}</td>
    </tr>
  );
}

/** Kanban order inside a column: nearest end date first, open-ended last (completed: latest end first). */
function byDates(phase: ProjectPhase) {
  return (a: Project, b: Project) => {
    const ea = a.end_date ?? "9999-99-99";
    const eb = b.end_date ?? "9999-99-99";
    if (phase === "done") return eb.localeCompare(ea) || a.name.localeCompare(b.name);
    return ea.localeCompare(eb) || (a.start_date ?? "").localeCompare(b.start_date ?? "") || a.name.localeCompare(b.name);
  };
}

/**
 * 프로젝트 (design §3.15, 0.3.0 plan §5): list / kanban of the workspace's cases by their computed phase
 * (진행예정 → 진행중 → 완료 from the start / end dates, 기한 경과 badge), the "새 프로젝트" dialog and the
 * templates manager.
 */
export function ProjectsPage() {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const workspace = useWorkspace();
  const view = useProjectsStore((s) => s.view);
  const setView = useProjectsStore((s) => s.setView);
  const { data: projects, isLoading } = useProjects(!!workspace);
  const { data: templates } = useProjectTemplates(!!workspace);
  const [newOpen, setNewOpen] = useState(false);
  const [templatesOpen, setTemplatesOpen] = useState(false);
  // 자동 분류: opened from the header (unassigned documents) or from the library with `?classify=<ids>`.
  const [searchParams, setSearchParams] = useSearchParams();
  const [classify, setClassify] = useState<{ open: boolean; docIds: string[] }>({ open: false, docIds: [] });
  const classifyParam = searchParams.get("classify");
  useEffect(() => {
    if (classifyParam === null || !workspace) return;
    setClassify({ open: true, docIds: classifyParam.split(",").filter(Boolean) });
    const next = new URLSearchParams(searchParams);
    next.delete("classify");
    setSearchParams(next, { replace: true });
  }, [classifyParam, searchParams, setSearchParams, workspace]);
  const ko = i18n.language.startsWith("ko");
  const templateName = (id: string | null) => {
    const tpl = templates?.find((x) => x.id === id);
    return tpl ? (ko ? tpl.name_ko : tpl.name_en) : null;
  };
  const list = projects ?? [];
  // Drops on a card / row: documents join the project, files are imported into it.
  const addDocsTo = async (p: Project, ids: string[]) => {
    if (ids.length === 0) return;
    try {
      await api.projectAddDocuments(p.id, ids);
      invalidateProjects([p.id]);
      toast(t("projects.documents.added", { count: ids.length }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const onDropDocs = (p: Project, docs: DragDoc[]) => void addDocsTo(p, docs.map((d) => d.id));
  const onDropFiles = (p: Project, paths: string[]) =>
    void importDroppedFiles(paths, { projectId: p.id }, t).then((ids) => {
      invalidateProjects([p.id]);
      if (ids.length > 0) toast(t("projects.documents.added", { count: ids.length }), "ok");
    });
  // Documents / files dropped on the sidebar's 프로젝트 entry wait for a project pick.
  const pendingDocs = useAttachmentsStore((s) => s.pendingProjectDocs);
  const pendingPaths = useAttachmentsStore((s) => s.pendingProjectPaths);
  const [pick, setPick] = useState<{ mode: "docs"; docs: DragDoc[] } | { mode: "files"; paths: string[] } | null>(null);
  useEffect(() => {
    if (!workspace) return;
    if (pendingDocs) {
      useAttachmentsStore.getState().setPending({ pendingProjectDocs: null });
      setPick({ mode: "docs", docs: pendingDocs });
    } else if (pendingPaths) {
      useAttachmentsStore.getState().setPending({ pendingProjectPaths: null });
      setPick({ mode: "files", paths: pendingPaths });
    }
  }, [pendingDocs, pendingPaths, workspace]);
  const onPick = (projectId: string) => {
    const p = list.find((x) => x.id === projectId);
    if (!p || !pick) return;
    if (pick.mode === "docs") onDropDocs(p, pick.docs);
    else onDropFiles(p, pick.paths);
    setPick(null);
  };
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="page-projects">
      <div className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-2">
        <h1 className="text-base font-semibold">{t("projects.title")}</h1>
        <Button size="sm" onClick={() => setNewOpen(true)} data-testid="projects-new">
          <Plus />
          {t("projects.new")}
        </Button>
        <Button size="sm" variant="outline" onClick={() => setTemplatesOpen(true)} data-testid="projects-templates">
          <Settings2 />
          {t("projects.templates_manage")}
        </Button>
        <Button size="sm" variant="outline" onClick={() => setClassify({ open: true, docIds: [] })} disabled={list.length === 0} title={t("projects.classify.button_hint")} data-testid="projects-classify">
          <Sparkles />
          {t("projects.classify.button")}
        </Button>
        <div className="ml-auto flex items-center gap-1" role="group" aria-label={t("projects.view.label")}>
          <Button size="icon-sm" variant={view === "kanban" ? "secondary" : "ghost"} aria-pressed={view === "kanban"} onClick={() => setView("kanban")} aria-label={t("projects.view.kanban")} data-testid="projects-view-kanban">
            <LayoutGrid />
          </Button>
          <Button size="icon-sm" variant={view === "list" ? "secondary" : "ghost"} aria-pressed={view === "list"} onClick={() => setView("list")} aria-label={t("projects.view.list")} data-testid="projects-view-list">
            <List />
          </Button>
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-4">
        {isLoading ? (
          <Spinner />
        ) : list.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-center text-fg-muted" data-testid="projects-empty">
            <Briefcase className="size-8 text-accent" aria-hidden="true" />
            <p className="font-medium text-fg">{t("projects.empty_title")}</p>
            <p className="max-w-md text-sm">{t("projects.empty_body")}</p>
            <Button onClick={() => setNewOpen(true)} data-testid="projects-empty-new">
              <Plus />
              {t("projects.new")}
            </Button>
          </div>
        ) : view === "kanban" ? (
          <div className="grid min-w-[48rem] grid-cols-3 gap-3" data-testid="projects-kanban">
            {PROJECT_PHASES.map((phase) => {
              const column = list.filter((p) => p.phase === phase).sort(byDates(phase));
              return (
                <section key={phase} className="flex min-h-40 flex-col gap-2 rounded-lg bg-bg-sunken p-2" aria-labelledby={`kanban-${phase}`} data-testid={`kanban-${phase}`}>
                  <h2 id={`kanban-${phase}`} className="flex items-center gap-2 px-1 text-xs font-semibold uppercase tracking-wide text-fg-subtle">
                    {t(`projects.phase.${phase}`)}
                    <Badge variant="muted">{column.length}</Badge>
                  </h2>
                  <ul className="flex flex-col gap-2">
                    {column.map((p) => (
                      <ProjectCard key={p.id} project={p} templateName={templateName(p.template_id)} compact onDropDocs={onDropDocs} onDropFiles={onDropFiles} />
                    ))}
                  </ul>
                </section>
              );
            })}
          </div>
        ) : (
          <table className="w-full text-sm" data-testid="projects-table">
            <thead className="text-left text-xs uppercase tracking-wide text-fg-subtle">
              <tr>
                <th className="py-2 pr-3">{t("projects.columns.name")}</th>
                <th className="py-2 pr-3">{t("projects.columns.status")}</th>
                <th className="py-2 pr-3">{t("projects.columns.intake_no")}</th>
                <th className="py-2 pr-3">{t("projects.columns.owner")}</th>
                <th className="py-2 pr-3">{t("projects.columns.start")}</th>
                <th className="py-2 pr-3">{t("projects.columns.end")}</th>
                <th className="py-2 pr-3">{t("projects.columns.documents")}</th>
                <th className="py-2 pr-3">{t("projects.columns.last_check")}</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {list.map((p) => (
                <ProjectRow key={p.id} p={p} onDropDocs={onDropDocs} onDropFiles={onDropFiles} />
              ))}
            </tbody>
          </table>
        )}
      </div>
      <NewProjectDialog open={newOpen} onOpenChange={setNewOpen} onSaved={(p) => navigate(`/projects/${p.id}`)} />
      <PickProjectDialog open={pick !== null} onOpenChange={(o) => !o && setPick(null)} projects={list} count={pick ? (pick.mode === "docs" ? pick.docs.length : pick.paths.length) : 0} mode={pick?.mode ?? "docs"} onPick={onPick} onCreate={() => setNewOpen(true)} />
      <TemplatesDialog open={templatesOpen} onOpenChange={setTemplatesOpen} />
      <ClassifyDialog open={classify.open} onOpenChange={(o) => setClassify((c) => ({ ...c, open: o }))} docIds={classify.docIds} projects={list} />
    </div>
  );
}
