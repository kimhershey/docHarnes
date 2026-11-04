import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { usePipelineStatus } from "@/lib/queries";
import type { PipelineStatus } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useActiveJobs } from "@/stores/jobs";
import { useWorkspace } from "@/stores/workspace";

export type PipelineReadiness = "ready" | "not_loaded" | "none" | "engine_off";

/** Whether processing can start right away, from the vision-model part of the pipeline status. */
export function pipelineReadiness(status: PipelineStatus): PipelineReadiness {
  if (status.model.model === null) return "none";
  if (!status.model.engine_running) return "engine_off";
  if (!status.model.loaded) return "not_loaded";
  return "ready";
}

/**
 * Small readiness chip (design §3.7.1 / review M10): vision model ready · not loaded · none · engine off,
 * plus "처리 중 N · 대기 N" while a pipeline job runs or requests are queued behind it. Mounted in the
 * status bar and in the library next to the 처리 menu. Renders nothing until the status is known.
 */
export function PipelineStatusChip({ className }: { className?: string }) {
  const { t } = useTranslation();
  const workspace = useWorkspace();
  const { data: status } = usePipelineStatus(!!workspace);
  const jobs = useActiveJobs();
  if (!status) return null;
  const readiness = pipelineReadiness(status);
  const model = status.model.model ?? "";
  const detail =
    readiness === "ready"
      ? t("pipeline.process.model_ready", { model })
      : readiness === "not_loaded"
        ? t("pipeline.process.model_not_loaded", { model })
        : readiness === "engine_off"
          ? t("pipeline.process.engine_off")
          : t("pipeline.process.model_none");
  const variant = readiness === "ready" ? "ok" : readiness === "none" ? "danger" : "warn";
  // The job book knows the running job before the status query refetches; its subject is "<n> documents".
  const runningJob = jobs.find((j) => j.kind === "pipeline");
  const running = status.running || runningJob !== undefined;
  const docs = runningJob?.subject.match(/^(\d+) documents$/);
  const activity = [
    running ? (docs ? t("pipeline.status.running_count", { count: Number(docs[1]) }) : t("pipeline.status.running")) : null,
    status.queued > 0 ? t("pipeline.status.queued", { count: status.queued }) : null,
  ]
    .filter((x): x is string => x !== null)
    .join(" · ");
  return (
    <span
      className={cn("inline-flex items-center gap-1.5 text-xs", className)}
      role="status"
      aria-label={`${t("pipeline.status.label")}: ${detail}${activity ? ` · ${activity}` : ""}`}
      title={detail}
      data-testid="pipeline-status-chip"
      data-readiness={readiness}
      data-running={running}
      data-queued={status.queued}
    >
      <Badge variant={variant}>{t(`pipeline.status.${readiness}`)}</Badge>
      {activity && (
        <span className="text-fg-muted" data-testid="pipeline-status-activity">
          {activity}
        </span>
      )}
    </span>
  );
}
