import { ExternalLink, Pause, Play, X } from "lucide-react";
import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { describeJobDetail } from "@/features/jobs/describe";
import { describeError } from "@/lib/errors";
import { SCREEN_ROUTES, isJobFinished, type Job, type Plan, type PlanExecution, type PlanResponse } from "@/lib/types";
import { useJob, useJobsStore } from "@/stores/jobs";
import { useUiStore } from "@/stores/ui";

import { isPlanExecution } from "./describePlan";
import { ResultCard } from "./ResultCard";

export function shortJobId(id: string): string {
  return id.slice(0, 8);
}

/**
 * Execution card (design §3.7.6 "실행 중 · 작업 #"): follows the plan's job
 * in the job book — progress, stage detail, pause / resume / cancel / Jobs —
 * and turns into the result card when the job finishes.
 */
export function ExecutionCard({ jobId, plan, response, onFinished }: { jobId: string; plan: Plan; response: PlanResponse; onFinished: (execution: PlanExecution | null, job: Job) => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const live = useJob(jobId);
  const last = useRef<Job | null>(null);
  if (live) last.current = live;
  const job = live ?? last.current;
  const finishedRef = useRef(false);
  const finished = job ? isJobFinished(job) : false;
  const execution = job && isPlanExecution(job.result) ? job.result : null;

  useEffect(() => {
    if (!job || !finished || finishedRef.current) return;
    finishedRef.current = true;
    onFinished(execution, job);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [finished]);

  const control = async (action: "pause" | "resume" | "cancel") => {
    const store = useJobsStore.getState();
    try {
      if (action === "pause") await store.pause(jobId);
      else if (action === "resume") await store.resume(jobId);
      else await store.cancel(jobId);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  if (!job) {
    return (
      <div className="flex w-full flex-col gap-2 rounded-lg border border-border bg-bg-elevated p-3 text-sm" role="group" aria-label={t("planner.execution.title", { id: shortJobId(jobId) })} data-testid="assistant-card-execution" data-job-id={jobId}>
        <span className="text-fg-muted">{t("planner.execution.job_missing")}</span>
        <Button size="sm" variant="ghost" className="self-start" onClick={() => navigate(SCREEN_ROUTES.jobs)} data-testid="execution-open-jobs">
          <ExternalLink />
          {t("planner.execution.open_jobs")}
        </Button>
      </div>
    );
  }

  const detail = describeJobDetail(job, t);
  const percent = job.progress != null ? Math.round(job.progress * 100) : null;
  const stateLabel = job.state === "paused" ? t("planner.execution.paused") : job.state === "queued" ? t("planner.execution.queued") : finished ? t(`jobs.state.${job.state}`) : null;

  return (
    <div className="flex w-full flex-col gap-2 rounded-lg border border-border bg-bg-elevated p-3 text-sm" role="group" aria-label={t("planner.execution.title", { id: shortJobId(job.id) })} data-testid="assistant-card-execution" data-job-id={job.id} data-job-state={job.state}>
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant={finished ? (job.state === "done" ? "ok" : job.state === "failed" ? "danger" : "muted") : job.state === "paused" ? "warn" : "info"}>{finished ? t("planner.execution.finished") : t("planner.execution.title", { id: shortJobId(job.id) })}</Badge>
        {stateLabel && <span className="text-xs text-fg-muted">{stateLabel}</span>}
        {job.attempts > 1 && <span className="text-xs text-fg-subtle">{t("jobs.attempts", { count: job.attempts })}</span>}
        {!finished && percent != null && <span className="ml-auto text-xs tabular-nums text-fg-muted">{percent}%</span>}
      </div>
      {!finished && (
        <>
          <Progress value={job.state === "queued" ? null : job.progress} label={t("planner.execution.title", { id: shortJobId(job.id) })} />
          <div className="flex flex-wrap items-center gap-2 text-xs text-fg-muted" aria-live="polite" aria-atomic="true" data-testid="execution-detail">
            {detail ?? (job.state === "queued" ? t("planner.execution.queued") : "")}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            {job.persistent && job.state !== "paused" && (
              <Button size="sm" variant="outline" onClick={() => void control("pause")} data-testid="execution-pause">
                <Pause />
                {t("planner.execution.pause")}
              </Button>
            )}
            {job.state === "paused" && (
              <Button size="sm" variant="outline" onClick={() => void control("resume")} data-testid="execution-resume">
                <Play />
                {t("planner.execution.resume")}
              </Button>
            )}
            <Button size="sm" variant="ghost" disabled={job.cancel_requested} onClick={() => void control("cancel")} data-testid="execution-cancel">
              <X />
              {t("planner.execution.cancel")}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => navigate(SCREEN_ROUTES.jobs)} data-testid="execution-open-jobs">
              <ExternalLink />
              {t("planner.execution.open_jobs")}
            </Button>
          </div>
        </>
      )}
      {finished && execution && <ResultCard execution={execution} plan={plan} response={response} error={job.state === "failed" ? job.error : null} />}
      {finished && !execution && (
        <p className={job.state === "failed" ? "text-xs text-danger selectable" : "text-xs text-fg-muted selectable"} role={job.state === "failed" ? "alert" : undefined} data-testid="execution-outcome">
          {job.error ?? detail ?? t(`jobs.state.${job.state}`)}
        </p>
      )}
    </div>
  );
}
