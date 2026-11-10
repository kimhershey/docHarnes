import { Briefcase, ClipboardCheck, FilePlus2, FolderOpen, ListTodo, PanelLeftClose, PanelLeftOpen, ScanSearch, Settings, Sparkles } from "lucide-react";
import { useTranslation } from "react-i18next";
import { NavLink, useLocation, useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { docsFromIds, importDroppedFiles, useDocumentDropTarget, type DropTargetHandlers } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects, usePipelineStatus } from "@/lib/queries";
import { SCREEN_ROUTES, type Screen } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useAttachmentsStore } from "@/stores/attachments";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

/**
 * The seven screens of 0.5.0 (plan §1): 문서함 · AI 어시스턴트 · 검토함 · 프로젝트 · 문서 작성 · 작업 현황 · 설정.
 * Models, forms / extraction fields, reference material and organise / export live inside the settings,
 * the projects screen, the document library and the jobs screen; their routes still resolve.
 */
const MAIN: { screen: Screen; icon: typeof FolderOpen }[] = [
  { screen: "library", icon: FolderOpen },
  { screen: "assistant", icon: Sparkles },
  { screen: "review", icon: ClipboardCheck },
  { screen: "projects", icon: Briefcase },
  { screen: "generate", icon: FilePlus2 },
  { screen: "jobs", icon: ListTodo },
  { screen: "settings", icon: Settings },
];

/**
 * Sidebar entries as drop targets (design §3.4 "끌어다 놓기"): files dropped on 문서함 open the import
 * dialog, documents / files dropped on 프로젝트 go to the open project (or wait for a pick) and on
 * 문서 작성 become draft sources. Every entry spring-loads: resting on it while dragging switches
 * screens so the drop can land on a specific place of the destination (the assistant's composer, a
 * project card, the form registration of 설정 > 양식·추출 항목).
 */
function useNavDrop(screen: Screen, enabled: boolean) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const toast = useUiStore((s) => s.toast);
  const route = SCREEN_ROUTES[screen];
  const go = (to: string = route) => {
    if (location.pathname !== to) navigate(to);
  };
  const setPending = (patch: Parameters<ReturnType<typeof useAttachmentsStore.getState>["setPending"]>[0]) => useAttachmentsStore.getState().setPending(patch);
  const handlers: DropTargetHandlers = { disabled: !enabled, onHover: () => go() };
  switch (screen) {
    case "library":
      handlers.onFiles = (paths) => {
        setPending({ pendingLibraryPaths: paths });
        go();
      };
      break;
    case "projects": {
      const current = useProjectsStore.getState().currentProject;
      const addTo = async (pid: string, ids: string[]) => {
        try {
          await api.projectAddDocuments(pid, ids);
          invalidateProjects([pid]);
          toast(t("projects.documents.added", { count: ids.length }), "ok");
          go(`/projects/${pid}`);
        } catch (e) {
          toast(describeError(e, t), "danger");
        }
      };
      handlers.onDocuments = (docs) => {
        if (current) void addTo(current, docs.map((d) => d.id));
        else {
          setPending({ pendingProjectDocs: docs });
          go();
        }
      };
      handlers.onFiles = (paths) => {
        if (current) {
          void importDroppedFiles(paths, { projectId: current }, t).then((ids) => {
            invalidateProjects([current]);
            if (ids.length > 0) toast(t("projects.documents.added", { count: ids.length }), "ok");
            go(`/projects/${current}`);
          });
        } else {
          setPending({ pendingProjectPaths: paths });
          go();
        }
      };
      break;
    }
    case "generate":
      handlers.onDocuments = (docs) => {
        setPending({ pendingGenerateSources: docs });
        go();
      };
      handlers.onFiles = (paths) =>
        void importDroppedFiles(paths, {}, t)
          .then(docsFromIds)
          .then((docs) => {
            if (docs.length === 0) return;
            setPending({ pendingGenerateSources: docs });
            go();
          });
      break;
    default:
      // Spring-loading only: the screen itself takes the drop (the assistant attaches, 양식 등록 registers).
      break;
  }
  return useDocumentDropTarget(handlers);
}

function Item({ screen, icon: Icon, collapsed, badge, ariaLabel, dropEnabled }: { screen: Screen; icon: typeof FolderOpen; collapsed: boolean; badge?: string; ariaLabel?: string; dropEnabled: boolean }) {
  const { t } = useTranslation();
  const label = t(`nav.${screen}`);
  // The accessible name carries what the badge shows ("검토함, 대기 12"), the visible text stays short.
  const name = ariaLabel ?? label;
  const drop = useNavDrop(screen, dropEnabled);
  const link = (
    <NavLink
      to={SCREEN_ROUTES[screen]}
      data-testid={`nav-${screen}`}
      {...drop.props}
      title={drop.isActive ? t("dnd.nav_drop", { screen: label }) : undefined}
      className={({ isActive }) =>
        cn(
          "flex items-center gap-3 rounded-md px-2.5 py-2 text-sm text-fg-muted transition-colors hover:bg-bg-sunken hover:text-fg",
          isActive && "bg-accent-soft text-fg font-medium",
          collapsed && "justify-center px-0",
          drop.isActive && "outline-dashed outline-1 outline-offset-[-1px] outline-border",
          drop.isOver && "bg-accent-soft text-fg outline-accent ring-2 ring-accent/40",
        )
      }
      aria-label={name}
    >
      <Icon className="size-4 shrink-0" aria-hidden="true" />
      {!collapsed && <span className="flex-1 truncate">{label}</span>}
      {!collapsed && badge && (
        <Badge variant="muted" className="text-[0.625rem]">
          {badge}
        </Badge>
      )}
    </NavLink>
  );
  if (!collapsed) return link;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{link}</TooltipTrigger>
      <TooltipContent side="right">{name}</TooltipContent>
    </Tooltip>
  );
}

export function Sidebar() {
  const { t } = useTranslation();
  const collapsed = useSettingsStore((s) => s.settings?.ui.sidebar_collapsed ?? false);
  const update = useSettingsStore((s) => s.update);
  const workspace = useWorkspace();
  const { data: pipeline } = usePipelineStatus(!!workspace);
  const reviewFields = pipeline?.review_fields ?? 0;

  return (
    <nav
      className={cn("flex shrink-0 flex-col border-r border-border bg-bg-elevated transition-[width]", collapsed ? "w-14" : "w-56")}
      aria-label={t("nav.main")}
      data-testid="sidebar"
      data-collapsed={collapsed}
    >
      <div className={cn("flex h-12 items-center gap-2 border-b border-border px-3", collapsed && "justify-center px-0")}>
        <ScanSearch className="size-5 text-accent" aria-hidden="true" />
        {!collapsed && <span className="font-semibold tracking-tight">{t("app.name")}</span>}
      </div>
      <div className="flex-1 overflow-y-auto p-2">
        <ul className="flex flex-col gap-0.5">
          {MAIN.map((it) => (
            <li key={it.screen}>
              <Item
                {...it}
                collapsed={collapsed}
                dropEnabled={!!workspace}
                badge={it.screen === "review" && reviewFields > 0 ? String(reviewFields) : undefined}
                ariaLabel={it.screen === "review" && reviewFields > 0 ? t("nav.with_pending", { label: t("nav.review"), count: reviewFields }) : undefined}
              />
            </li>
          ))}
        </ul>
      </div>
      <button
        type="button"
        className="flex h-10 items-center justify-center gap-2 border-t border-border text-fg-muted hover:bg-bg-sunken hover:text-fg cursor-pointer text-xs"
        onClick={() => void update((s) => void (s.ui.sidebar_collapsed = !s.ui.sidebar_collapsed))}
        aria-label={collapsed ? t("nav.expand") : t("nav.collapse")}
        data-testid="sidebar-toggle"
      >
        {collapsed ? <PanelLeftOpen className="size-4" /> : <PanelLeftClose className="size-4" />}
        {!collapsed && <span>{t("nav.collapse")}</span>}
      </button>
    </nav>
  );
}
