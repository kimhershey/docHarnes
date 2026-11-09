import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { PipelineStatusChip } from "@/features/pipeline/PipelineStatusChip";
import { api, isTauri } from "@/lib/ipc";
import { useAssessment, useModelIndex } from "@/lib/queries";
import { queryKeys } from "@/lib/query-client";
import { SCREEN_ROUTES } from "@/lib/types";
import { formatBytes, formatMiB } from "@/lib/utils";
import { useEngineStore } from "@/stores/engine";
import { useActiveJobs } from "@/stores/jobs";
import { useWorkspace } from "@/stores/workspace";

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

/** "배치 작업 중 — 약 N초 후 응답" (P5 engine gate): seconds rounded up, omitted when the estimate is 0. */
export function gateNotice(gate: { batch_busy: boolean; estimated_wait_ms: number } | null | undefined, t: (k: string, o?: Record<string, unknown>) => string): string | null {
  if (!gate?.batch_busy) return null;
  const seconds = Math.ceil((gate.estimated_wait_ms ?? 0) / 1000);
  return seconds > 0 ? t("statusbar.gate_busy", { seconds }) : t("statusbar.gate_busy_short");
}

export function StatusBar() {
  const { t } = useTranslation();
  const online = useOnline();
  const workspace = useWorkspace();
  const { data: assessment } = useAssessment();
  const { data: index } = useModelIndex();
  const status = useEngineStore((s) => s.status);
  const jobs = useActiveJobs();
  const batchRunning = jobs.some((j) => (j.kind === "pipeline" || j.kind === "plan") && j.state === "running");
  // The gate flips at page boundaries while a batch runs: poll a little faster than the change events then.
  const { data: pipeline } = useQuery({ queryKey: queryKeys.pipelineStatus, queryFn: () => api.pipelineStatus(), enabled: !!workspace, staleTime: 5_000, refetchInterval: batchRunning ? 3_000 : false });
  // "no model" only when nothing loadable exists at all (design §3.3.4 "모델 미설치 상태").
  const installed = (assessment?.active_model_installed?.installed ?? false) || (index?.discovered.some((d) => d.health.kind === "ready") ?? false);
  const ind = engineIndicator(status, installed);
  const loaded = status?.models.find((m) => m.status === "loaded" || m.status === "sleeping") ?? null;
  const total = assessment?.tier.total_vram_mib;
  const vram = loaded?.memory_effective_bytes != null ? `${formatBytes(loaded.memory_effective_bytes, 1)} / ${formatMiB(total)}` : assessment?.tier.free_vram_mib != null ? `${formatMiB(assessment.tier.free_vram_mib)} / ${formatMiB(total)}` : "—";
  const modelLabel = loaded ? loaded.id : (assessment?.active_model?.display ?? t("topbar.no_model"));
  const running = jobs.filter((j) => j.state === "running").length;
  const paused = jobs.filter((j) => j.state === "paused").length;
  const queued = jobs.length - running - paused;
  const queueTooltip = t("statusbar.queue_tooltip", { running, queued, paused });
  const gate = gateNotice(pipeline?.gate, t);

  return (
    <footer className="flex min-h-7 shrink-0 flex-wrap items-center gap-x-4 border-t border-border bg-bg-elevated px-3 text-xs text-fg-muted [&>*]:whitespace-nowrap" data-testid="statusbar">
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
      <Link to={SCREEN_ROUTES.jobs} className="hover:text-fg" title={queueTooltip} aria-label={`${t("statusbar.queue", { count: jobs.length })} — ${queueTooltip}`} data-testid="statusbar-jobs" data-paused={paused}>
        {t("statusbar.queue", { count: jobs.length })}
        {paused > 0 && <span className="ml-1 text-warn">({t("jobs.state.paused")} {paused})</span>}
      </Link>
      {gate && (
        <span className="text-warn" role="status" aria-live="polite" data-testid="statusbar-gate">
          {gate}
        </span>
      )}
      <PipelineStatusChip className="hidden md:inline-flex" />
      <span className="ml-auto">{online ? t("statusbar.network_online") : t("statusbar.network_offline")}</span>
      {!isTauri && <span className="rounded bg-warn-soft px-1.5 text-warn">{t("statusbar.preview")}</span>}
    </footer>
  );
}
