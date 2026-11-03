import { Cpu, ListTodo, Search } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Kbd } from "@/components/ui/kbd";
import { Link } from "react-router";

import { useAssessment } from "@/lib/queries";
import { SCREEN_ROUTES } from "@/lib/types";
import { useActiveJobs } from "@/stores/jobs";
import { useShortcut } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";

import { ThemeMenu } from "./ThemeMenu";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

export function TopBar() {
  const { t } = useTranslation();
  const openPalette = useUiStore((s) => s.openPalette);
  const paletteChord = useShortcut("command_palette");
  const { data: assessment } = useAssessment();
  const jobs = useActiveJobs();

  return (
    <header className="flex h-12 shrink-0 items-center gap-3 border-b border-border bg-bg-elevated px-3" data-testid="topbar">
      <WorkspaceSwitcher />
      <button
        type="button"
        className="flex h-9 min-w-0 flex-1 items-center gap-2 rounded-md border border-border bg-bg px-3 text-sm text-fg-subtle hover:border-border-strong cursor-text"
        onClick={() => openPalette()}
        data-testid="command-bar"
      >
        <Search className="size-4 shrink-0" aria-hidden="true" />
        <span className="flex-1 truncate text-left">{t("topbar.command")}</span>
        <Kbd chord={paletteChord} />
      </button>
      <Link to={SCREEN_ROUTES.jobs} data-testid="topbar-jobs">
        <Badge variant={jobs.length > 0 ? "info" : "muted"} className="gap-1">
          <ListTodo className="size-3" aria-hidden="true" />
          {t("topbar.jobs", { count: jobs.length })}
        </Badge>
      </Link>
      <Badge variant={assessment?.active_model_installed?.installed ? "ok" : "outline"} className="gap-1" data-testid="model-badge" title={assessment?.active_model?.display}>
        <Cpu className="size-3" aria-hidden="true" />
        <span className="max-w-40 truncate">
          {assessment ? `${t(`hw.tier.${assessment.tier.tier}`)} · ${assessment.active_model?.display ?? t("topbar.no_model")}` : "…"}
        </span>
      </Badge>
      <ThemeMenu />
    </header>
  );
}
