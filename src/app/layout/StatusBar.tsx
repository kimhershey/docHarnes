import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { PipelineStatusChip } from "@/features/pipeline/PipelineStatusChip";
import { isTauri } from "@/lib/ipc";
import { useAssessment, useModelIndex } from "@/lib/queries";
import { SCREEN_ROUTES } from "@/lib/types";
import { formatBytes, formatMiB } from "@/lib/utils";
import { useEngineStore } from "@/stores/engine";
import { useActiveJobs } from "@/stores/jobs";

function useOnline(): boolean {
  const [online, setOnline] = useState(typeof navigator === "undefined" ? false : navigator.onLine);
  useEffect(() => {
    const on = () => setOnline(true);
    const off = () => setOnline(false);
    window.addEventListener("online", on);
    window.addEventListener("offline", off);
    return () => {
      window.removeEventListener("online", on);
      window.removeEventListener("offline", off);
    };
  }, []);
  return online;
}

/** Engine dot + label per design §3.7.1: ● 준비 / 로딩 / 모델 없음 / 오프라인. */
export function engineIndicator(
  status: ReturnType<typeof useEngineStore.getState>["status"],
  installed: boolean,
): { key: string; color: string } {
  const state = status?.state.kind ?? "stopped";
  const loading = status?.models.some((m) => m.status === "loading") ?? false;
  const loaded = status?.models.some((m) => m.status === "loaded" || m.status === "sleeping") ?? false;
  if (state === "degraded") return { key: "engine_degraded", color: "bg-danger" };
  if (state === "starting") return { key: "engine_starting", color: "bg-warn" };
  if (state === "stopping") return { key: "engine_stopping", color: "bg-warn" };
  if (state === "restarting") return { key: "engine_restarting", color: "bg-warn" };
  if (state === "running") {
    if (loading) return { key: "engine_loading", color: "bg-warn" };
    if (loaded) return { key: "engine_ready", color: "bg-ok" };
    return { key: installed ? "engine_idle" : "engine_no_model", color: installed ? "bg-info" : "bg-fg-subtle" };
  }
  return { key: installed ? "engine_stopped" : "engine_no_model", color: "bg-fg-subtle" };
}

export function StatusBar() {
  const { t } = useTranslation();
  const online = useOnline();
  const { data: assessment } = useAssessment();
  const { data: index } = useModelIndex();
  const status = useEngineStore((s) => s.status);
  const jobs = useActiveJobs();
  // "no model" only when nothing loadable exists at all (design §3.3.4 "모델 미설치 상태").
  const installed = (assessment?.active_model_installed?.installed ?? false) || (index?.discovered.some((d) => d.health.kind === "ready") ?? false);
  const ind = engineIndicator(status, installed);
  const loaded = status?.models.find((m) => m.status === "loaded" || m.status === "sleeping") ?? null;
  const total = assessment?.tier.total_vram_mib;
  const vram = loaded?.memory_effective_bytes != null ? `${formatBytes(loaded.memory_effective_bytes, 1)} / ${formatMiB(total)}` : assessment?.tier.free_vram_mib != null ? `${formatMiB(assessment.tier.free_vram_mib)} / ${formatMiB(total)}` : "—";
  const modelLabel = loaded ? loaded.id : (assessment?.active_model?.display ?? t("topbar.no_model"));

  return (
    <footer className="flex h-7 shrink-0 items-center gap-4 border-t border-border bg-bg-elevated px-3 text-xs text-fg-muted" data-testid="statusbar">
      <Link to={SCREEN_ROUTES.models} className="flex items-center gap-1.5 hover:text-fg" data-testid="statusbar-engine">
        <span className={`size-2 rounded-full ${ind.color}`} aria-hidden="true" />
        {t("statusbar.engine")}: {t(`statusbar.${ind.key}`)}
      </Link>
      <span data-testid="statusbar-vram">
        {loaded?.memory_effective_bytes != null ? t("statusbar.vram_used") : t("statusbar.vram")} {vram}
      </span>
      <span className="truncate" data-testid="statusbar-model">
        {t("statusbar.model")}: {modelLabel}
      </span>
      <Link to={SCREEN_ROUTES.jobs} className="hover:text-fg" data-testid="statusbar-jobs">
        {t("statusbar.queue", { count: jobs.length })}
      </Link>
      <PipelineStatusChip className="hidden md:inline-flex" />
      <span className="ml-auto">{online ? t("statusbar.network_online") : t("statusbar.network_offline")}</span>
      {!isTauri && <span className="rounded bg-warn-soft px-1.5 text-warn">{t("statusbar.preview")}</span>}
    </footer>
  );
}
