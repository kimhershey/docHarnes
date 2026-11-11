import { ChevronDown, ChevronRight, ClipboardCheck, Cpu, FileText, Inbox } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { engineIndicator } from "@/app/layout/StatusBar";
import { Badge } from "@/components/ui/badge";
import { settingsTabPath } from "@/features/settings/tabs";
import { useAssessment, useDocumentStats, useDocuments, useModelIndex, usePipelineStatus } from "@/lib/queries";
import { SCREEN_ROUTES } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useDocumentsStore } from "@/stores/documents";
import { useEngineStore } from "@/stores/engine";
import { useWorkspace } from "@/stores/workspace";

import { folderCount } from "./folders";

const OPEN_KEY = "doculens.library.today";

function loadOpen(): boolean {
  try {
    return localStorage.getItem(OPEN_KEY) !== "0";
  } catch {
    return true;
  }
}

function saveOpen(open: boolean): void {
  try {
    localStorage.setItem(OPEN_KEY, open ? "1" : "0");
  } catch {
    /* private window / blocked storage: the choice lasts for the session */
  }
}

/**
 * "오늘 할 일" strip at the top of 문서함 (0.5.0 plan §1, the former home screen): 받은 문서함 count, what
 * is new / processing / awaiting review / done, the AI engine's state with its model (→ 설정 > AI 모델)
 * and the recently imported documents.
 */
export function TodayStrip() {
  const { t } = useTranslation();
  const workspace = useWorkspace();
  const setFolder = useDocumentsStore((s) => s.setFolder);
  const { data: stats } = useDocumentStats(!!workspace);
  const { data: pipeline } = usePipelineStatus(!!workspace);
  const { data: recent } = useDocuments({ limit: 5, sort: "imported_desc" }, !!workspace);
  const { data: assessment } = useAssessment();
  const { data: index } = useModelIndex();
  const status = useEngineStore((s) => s.status);
  const [open, setOpen] = useState(loadOpen);
  const toggle = () =>
    setOpen((o) => {
      saveOpen(!o);
      return !o;
    });

  const by = stats?.by_status ?? {};
  // The same count as the folder the chip selects (the inbox and the folders below it).
  const inbox = folderCount(stats?.folders ?? [], "inbox");
  const counts: { key: "inbox" | "new" | "processing" | "review" | "done"; value: number }[] = [
    { key: "inbox", value: inbox },
    { key: "new", value: (by.new ?? 0) + (by.importing ?? 0) },
    { key: "processing", value: by.rendered ?? 0 },
    { key: "review", value: pipeline?.review_pending ?? by.extracted ?? 0 },
    { key: "done", value: by.reviewed ?? 0 },
  ];
  // "no model" only when nothing loadable exists at all (design §3.3.4 "모델 미설치 상태").
  const installed = (assessment?.active_model_installed?.installed ?? false) || (index?.discovered.some((d) => d.health.kind === "ready") ?? false);
  const ind = engineIndicator(status, installed);
  const loaded = status?.models.find((m) => m.status === "loaded" || m.status === "sleeping") ?? null;
  const modelLabel = loaded ? loaded.id : (assessment?.active_model?.display ?? t("topbar.no_model"));
  const chip = "flex items-center gap-1 rounded-full border border-border bg-bg px-2 py-0.5";

  return (
    <section className="border-b border-border bg-bg-sunken/40 px-4 py-1.5 text-xs text-fg-muted" aria-label={t("library.today.title")} data-testid="library-today" data-open={open ? "true" : "false"}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <button type="button" className="flex items-center gap-1 font-medium text-fg cursor-pointer" onClick={toggle} aria-expanded={open} title={workspace ? t("library.today.workspace", { path: workspace.path }) : t("library.today.no_workspace")} data-testid="library-today-toggle">
          {open ? <ChevronDown className="size-3.5" aria-hidden="true" /> : <ChevronRight className="size-3.5" aria-hidden="true" />}
          {t("library.today.title")}
        </button>
        {counts.map(({ key, value }) =>
          key === "inbox" ? (
            <button key={key} type="button" className={cn(chip, "hover:border-accent hover:text-fg cursor-pointer")} onClick={() => setFolder(value > 0 ? "inbox" : null)} data-testid="library-today-stat-inbox">
              <Inbox className="size-3.5" aria-hidden="true" />
              {t("library.today.inbox", { count: stats ? value : 0 })}
            </button>
          ) : key === "review" ? (
            <Link key={key} to={SCREEN_ROUTES.review} className={cn(chip, value > 0 && "border-warn text-warn", "hover:border-accent hover:text-fg")} data-testid="library-today-stat-review">
              <ClipboardCheck className="size-3.5" aria-hidden="true" />
              {t("library.today.review", { count: stats ? value : 0 })}
            </Link>
          ) : (
            <span key={key} className={chip} data-testid={`library-today-stat-${key}`}>
              {t(`library.today.${key}`, { count: stats ? value : 0 })}
            </span>
          ),
        )}
        {pipeline && pipeline.review_fields > 0 && (
          <Link to={SCREEN_ROUTES.review} className="text-accent hover:underline" data-testid="library-today-review-fields">
            {t("library.today.review_fields", { count: pipeline.review_fields })}
          </Link>
        )}
        <Link to={settingsTabPath("models")} className={cn(chip, "ml-auto max-w-72 hover:border-accent hover:text-fg")} title={t("library.today.engine_open")} data-testid="library-today-engine">
          <span className={`size-2 shrink-0 rounded-full ${ind.color}`} aria-hidden="true" />
          <Cpu className="size-3.5" aria-hidden="true" />
          <span className="truncate">
            {t("library.today.engine")}: {t(`statusbar.${ind.key}`)} · {modelLabel}
          </span>
          {assessment && (
            <Badge variant={assessment.active_model_installed?.installed ? "ok" : "warn"} className="text-[0.625rem]">
              {assessment.active_model_installed?.installed ? t("common.installed") : t("common.not_installed")}
            </Badge>
          )}
        </Link>
      </div>
      {open && (
        <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1" data-testid="library-today-recent">
          <span className="flex items-center gap-1">
            <FileText className="size-3.5" aria-hidden="true" />
            {t("library.today.recent")}:
          </span>
          {recent && recent.length > 0 ? (
            recent.map((d) => (
              <Link key={d.id} to={`${SCREEN_ROUTES.library}/${d.id}`} className="max-w-56 truncate rounded-full bg-bg px-2 py-0.5 hover:text-fg hover:underline" title={d.original_name} data-testid="library-today-recent-item">
                {d.title ?? d.original_name}
              </Link>
            ))
          ) : (
            <span className="text-fg-subtle">{t("library.today.recent_empty")}</span>
          )}
        </div>
      )}
    </section>
  );
}
