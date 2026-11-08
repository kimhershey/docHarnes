import { ArrowLeft, ArrowRight, Briefcase, LayoutGrid, List, Plus, Settings2 } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects, useProjects, useProjectTemplates } from "@/lib/queries";
import { PROJECT_STATUSES, type Project, type ProjectStatus } from "@/lib/types";
import { cn, formatDate } from "@/lib/utils";
import { useProjectsStore } from "@/stores/projects";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

import { NewProjectDialog } from "./NewProjectDialog";
import { nextStatus } from "./rules";
import { TemplatesDialog } from "./TemplatesDialog";

function statusVariant(s: ProjectStatus): "ok" | "warn" | "danger" | "muted" | "info" {
  switch (s) {
    case "done":
      return "ok";
    case "supplement":
      return "warn";
    case "validating":
      return "info";
    default:
      return "muted";
  }
}

function ProjectCard({ project, onMove, templateName, compact }: { project: Project; onMove: (p: Project, to: ProjectStatus) => void; templateName: string | null; compact: boolean }) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const prev = nextStatus(project.status, -1);
  const next = nextStatus(project.status, 1);
  return (
    <li className="flex flex-col gap-1.5 rounded-lg border border-border bg-bg-elevated p-3 text-sm" data-testid={`project-card-${project.id}`} data-project-status={project.status} data-project-name={project.name}>
      <div className="flex items-start gap-2">
        <button type="button" className="min-w-0 flex-1 text-left font-medium hover:underline cursor-pointer" onClick={() => navigate(`/projects/${project.id}`)} data-testid="project-open">
          {project.name}
        </button>
        {!compact && <Badge variant={statusVariant(project.status)}>{t(`projects.status.${project.status}`)}</Badge>}
      </div>
      <div className="flex flex-wrap gap-x-3 gap-y-0.5 text-xs text-fg-muted">
        {project.intake_no && <span>{t("projects.card.intake_no", { no: project.intake_no })}</span>}
        {project.owner && <span>{t("projects.card.owner", { owner: project.owner })}</span>}
        {project.due_date && <span>{t("projects.card.due", { date: project.due_date })}</span>}
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
      </div>
      <div className="flex items-center gap-1">
        <Button size="icon-sm" variant="ghost" disabled={!prev} onClick={() => prev && onMove(project, prev)} aria-label={t("projects.card.move_prev")} data-testid="project-move-prev">
          <ArrowLeft />
        </Button>
        <Button size="icon-sm" variant="ghost" disabled={!next} onClick={() => next && onMove(project, next)} aria-label={t("projects.card.move_next")} data-testid="project-move-next">
          <ArrowRight />
        </Button>
        <span className="ml-auto text-[11px] text-fg-subtle">{formatDate(project.updated_at, i18n.language)}</span>
      </div>
    </li>
  );
}

/**
 * 프로젝트 (design §3.15): list / kanban of the workspace's cases (접수 → 추출 → 검증 → 보완요청 → 완료) with
 * keyboard-usable move buttons, the "새 프로젝트" dialog and the templates manager.
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
  const ko = i18n.language.startsWith("ko");
  const templateName = (id: string | null) => {
    const tpl = templates?.find((x) => x.id === id);
    return tpl ? (ko ? tpl.name_ko : tpl.name_en) : null;
  };
  const move = async (p: Project, to: ProjectStatus) => {
    try {
      await api.projectSetStatus(p.id, to);
      invalidateProjects([p.id]);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const list = projects ?? [];
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
          <div className="grid min-w-[60rem] grid-cols-5 gap-3" data-testid="projects-kanban">
            {PROJECT_STATUSES.map((status) => {
              const column = list.filter((p) => p.status === status);
              return (
                <section key={status} className="flex min-h-40 flex-col gap-2 rounded-lg bg-bg-sunken p-2" aria-labelledby={`kanban-${status}`} data-testid={`kanban-${status}`}>
                  <h2 id={`kanban-${status}`} className="flex items-center gap-2 px-1 text-xs font-semibold uppercase tracking-wide text-fg-subtle">
                    {t(`projects.status.${status}`)}
                    <Badge variant="muted">{column.length}</Badge>
                  </h2>
                  <ul className="flex flex-col gap-2">
                    {column.map((p) => (
                      <ProjectCard key={p.id} project={p} onMove={(x, to) => void move(x, to)} templateName={templateName(p.template_id)} compact />
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
                <th className="py-2 pr-3">{t("projects.columns.due")}</th>
                <th className="py-2 pr-3">{t("projects.columns.documents")}</th>
                <th className="py-2 pr-3">{t("projects.columns.last_check")}</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {list.map((p) => (
                <tr key={p.id} className={cn("cursor-pointer hover:bg-bg-sunken")} onClick={() => navigate(`/projects/${p.id}`)} data-testid={`project-row-${p.id}`} data-project-status={p.status}>
                  <td className="py-2 pr-3 font-medium">{p.name}</td>
                  <td className="py-2 pr-3">
                    <Badge variant={statusVariant(p.status)}>{t(`projects.status.${p.status}`)}</Badge>
                  </td>
                  <td className="py-2 pr-3 text-fg-muted">{p.intake_no ?? "—"}</td>
                  <td className="py-2 pr-3 text-fg-muted">{p.owner ?? "—"}</td>
                  <td className="py-2 pr-3 text-fg-muted">{p.due_date ?? "—"}</td>
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
              ))}
            </tbody>
          </table>
        )}
      </div>
      <NewProjectDialog open={newOpen} onOpenChange={setNewOpen} onSaved={(p) => navigate(`/projects/${p.id}`)} />
      <TemplatesDialog open={templatesOpen} onOpenChange={setTemplatesOpen} />
    </div>
  );
}
