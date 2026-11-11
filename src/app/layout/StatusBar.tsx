import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { settingsTabPath } from "@/features/settings/tabs";
import { api } from "@/lib/ipc";
import { useAssessment, useModelIndex } from "@/lib/queries";
import { queryKeys } from "@/lib/query-client";
import { SCREEN_ROUTES } from "@/lib/types";
import { useEngineStore } from "@/stores/engine";
import { useActiveJobs } from "@/stores/jobs";
import { useWorkspace } from "@/stores/workspace";

import { useModelDisplay } from "../hooks/useModelDisplay";

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

/**
 * Status bar (0.5.0 plan §3): the AI engine's state with the model it serves (→ 설정 > AI 모델) and the
 * job badge (→ 작업 현황), plus the batch-gate notice while a batch holds the engine.
 */
export function StatusBar() {
  const { t } = useTranslation();
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
  const modelName = useModelDisplay();
  const modelLabel = loaded ? loaded.id : (assessment?.active_model_installed?.installed ? modelName(assessment.active_model?.display) : null) ?? null;
  const running = jobs.filter((j) => j.state === "running").length;
  const paused = jobs.filter((j) => j.state === "paused").length;
  const queued = jobs.length - running - paused;
  const queueTooltip = t("statusbar.queue_tooltip", { running, queued, paused });
  const gate = gateNotice(pipeline?.gate, t);

  return (
    <footer className="flex min-h-7 shrink-0 flex-wrap items-center gap-x-4 border-t border-border bg-bg-elevated px-3 text-xs text-fg-muted [&>*]:whitespace-nowrap" data-testid="statusbar">
      <Link to={settingsTabPath("models")} className="flex min-w-0 items-center gap-1.5 hover:text-fg" data-testid="statusbar-engine">
        <span className={`size-2 shrink-0 rounded-full ${ind.color}`} aria-hidden="true" />
        <span>
          {t("statusbar.engine")}: {t(`statusbar.${ind.key}`)}
        </span>
        {modelLabel && (
          <span className="truncate text-fg-subtle" data-testid="statusbar-model">
            · {modelLabel}
          </span>
        )}
      </Link>
      <Link to={SCREEN_ROUTES.jobs} className="hover:text-fg" title={queueTooltip} aria-label={`${t("statusbar.queue", { count: jobs.length })} — ${queueTooltip}`} data-testid="statusbar-jobs" data-paused={paused}>
        {t("statusbar.queue", { count: jobs.length })}
        {paused > 0 && <span className="ml-1 text-warn">({t("jobs.state.paused")} {paused})</span>}
      </Link>
      {gate && (
        <span className="text-warn" role="status" aria-live="polite" data-testid="statusbar-gate">
          {gate}
        </span>
      )}
    </footer>
  );
}
