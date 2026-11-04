import {
  BookOpen,
  Brain,
  Briefcase,
  ClipboardCheck,
  FilePlus2,
  FolderOpen,
  FolderTree,
  Home,
  ListTodo,
  MessageSquare,
  PanelLeftClose,
  PanelLeftOpen,
  Puzzle,
  ScanSearch,
  Search,
  Settings,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { NavLink } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { SCREEN_ROUTES, type Screen } from "@/lib/types";
import { cn } from "@/lib/utils";
import { usePipelineStatus } from "@/lib/queries";
import { useSettingsStore } from "@/stores/settings";
import { useWorkspace } from "@/stores/workspace";

const MAIN: { screen: Screen; icon: typeof Home }[] = [
  { screen: "home", icon: Home },
  { screen: "library", icon: FolderOpen },
  { screen: "search", icon: Search },
  { screen: "assistant", icon: MessageSquare },
  { screen: "review", icon: ClipboardCheck },
  { screen: "schemas", icon: Puzzle },
  { screen: "knowledge", icon: BookOpen },
  { screen: "organize", icon: FolderTree },
  { screen: "jobs", icon: ListTodo },
  { screen: "models", icon: Brain },
  { screen: "settings", icon: Settings },
];

const PHASE2: { screen: Screen; icon: typeof Home }[] = [
  { screen: "generate", icon: FilePlus2 },
  { screen: "projects", icon: Briefcase },
];

function Item({ screen, icon: Icon, collapsed, badge }: { screen: Screen; icon: typeof Home; collapsed: boolean; badge?: string }) {
  const { t } = useTranslation();
  const label = t(`nav.${screen}`);
  const link = (
    <NavLink
      to={SCREEN_ROUTES[screen]}
      end={screen === "home"}
      data-testid={`nav-${screen}`}
      className={({ isActive }) =>
        cn(
          "flex items-center gap-3 rounded-md px-2.5 py-2 text-sm text-fg-muted transition-colors hover:bg-bg-sunken hover:text-fg",
          isActive && "bg-accent-soft text-fg font-medium",
          collapsed && "justify-center px-0",
        )
      }
      aria-label={label}
    >
      <Icon className="size-4 shrink-0" aria-hidden="true" />
      {!collapsed && <span className="flex-1 truncate">{label}</span>}
      {!collapsed && badge && (
        <Badge variant="muted" className="text-[10px]">
          {badge}
        </Badge>
      )}
    </NavLink>
  );
  if (!collapsed) return link;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{link}</TooltipTrigger>
      <TooltipContent side="right">{label}</TooltipContent>
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
              <Item {...it} collapsed={collapsed} badge={it.screen === "review" && reviewFields > 0 ? String(reviewFields) : undefined} />
            </li>
          ))}
        </ul>
        {!collapsed && <p className="mt-4 px-2.5 text-[11px] font-semibold uppercase tracking-wide text-fg-subtle">{t("nav.phase2_group")}</p>}
        <ul className="mt-1 flex flex-col gap-0.5">
          {PHASE2.map((it) => (
            <li key={it.screen}>
              <Item {...it} collapsed={collapsed} badge="v1.1" />
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
