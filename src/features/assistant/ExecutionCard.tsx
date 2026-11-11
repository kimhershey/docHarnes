import { ExternalLink, Pause, Play, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { Spinner } from "@/components/ui/spinner";
import { describeJobDetail, describeJobError } from "@/features/jobs/describe";
import { describeError } from "@/lib/errors";
import { SCREEN_ROUTES, isJobFinished, type Job, type Plan, type PlanExecution, type PlanResponse } from "@/lib/types";
import { useJob, useJobsStore } from "@/stores/jobs";
import { useUiStore } from "@/stores/ui";

import { isPlanExecution } from "./describePlan";
import { ResultCard } from "./ResultCard";

export function shortJobId(id: string): string {
  return id.slice(0, 8);
}

/** Whether the job book knows the job: unknown until the first event / `jobs_get`, removed when it was cleared. */
type Presence = "unknown" | "known" | "removed";
/** `Job.result` is not in the `jobs://update` broadcast: it is fetched once the job finishes. */
type ResultState = "idle" | "loading" | "settled";

/**
 * Execution card (design §3.7.6 "실행 중 · 작업 #"): follows the plan's job
 * in the job book — progress, stage detail, pause / resume / cancel / Jobs —
 * and turns into the result card when the job finishes. The result comes from
 * `jobs_get` (the broadcast omits it); a job the store does not know is looked
 * up once, and one that disappears without finishing is reported as removed.
 */
export function ExecutionCard({
  jobId,
  plan,
  response,
  onFinished,
}: {
  jobId: string;
  plan: Plan;
  response: PlanResponse;
  /**
   * Called once per mount when the job is finished and its result settled. `live` says whether this card
   * watched the job run: only then is the end news — a card that mounts on a job that had already ended
   * (chat reopened, app restarted) reports `false`, and the page does not announce the old failure again.
   */
  onFinished: (execution: PlanExecution | null, job: Job, live: boolean) => void;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const live = useJob(jobId);
  const last = useRef<Job | null>(null);
  if (live) last.current = live;
  const [fetched, setFetched] = useState<Job | null>(null);
  const [presence, setPresence] = useState<Presence>(live ? "known" : "unknown");
  const [resultState, setResultState] = useState<ResultState>("idle");
  const snapshot = live ?? last.current;
  const job = snapshot ?? fetched;
  const finishedRef = useRef(false);
  const finished = job ? isJobFinished(job) : false;
  // Set while this card sees the job unfinished (queued / running / paused).
  const sawUnfinished = useRef(false);
  if (job && !finished) sawUnfinished.current = true;
  const execution = snapshot && isPlanExecution(snapshot.result) ? snapshot.result : fetched && isPlanExecution(fetched.result) ? fetched.result : null;
  const resultSettled = execution !== null || resultState === "settled";

  // Not in the store yet (the event may trail the invoke) or any more (cleared / detached): ask the book once.
  useEffect(() => {
    if (live) {
      setPresence("known");
      return;
    }
    if (last.current) {
      // Known before, gone now: a finished snapshot keeps showing its result; a running one was removed.
      if (!isJobFinished(last.current)) setPresence("removed");
      return;
    }
    let cancelled = false;
    void useJobsStore
      .getState()
      .fetch(jobId)
      .then((j) => {
        if (cancelled) return;
        if (j) {
          setFetched(j);
          setPresence("known");
          if (!useJobsStore.getState().jobs.some((x) => x.id === jobId)) void useJobsStore.getState().refresh();
        } else setPresence((p) => (p === "known" ? p : "removed"));
      });
    return () => {
      cancelled = true;
    };
  }, [live, jobId]);

  // Finished without a usable result in the store: load it with `jobs_get` (once per job).
  useEffect(() => {
    if (!job || !finished || execution || resultState !== "idle") return;
    setResultState("loading");
    void useJobsStore
      .getState()
      .fetch(jobId)
      .then((j) => {
        if (j) setFetched(j);
      })
      .finally(() => setResultState("settled"));
  }, [job, finished, execution, resultState, jobId]);

  useEffect(() => {
    if (!job || !finished || !resultSettled || finishedRef.current) return;
    finishedRef.current = true;
    onFinished(execution, job, sawUnfinished.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [finished, resultSettled]);

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

  if (!job || (presence === "removed" && !finished)) {
    return (
      <div className="flex w-full flex-col gap-2 rounded-lg border border-border bg-bg-elevated p-3 text-sm" role="group" aria-label={t("planner.execution.title", { id: shortJobId(jobId) })} data-testid="assistant-card-execution" data-job-id={jobId} data-job-presence={presence}>
        {presence === "unknown" ? (
          <span className="flex items-center gap-2 text-fg-muted" role="status">
            <Spinner />
            {t("common.loading")}
          </span>
        ) : (
          <span className="text-fg-muted" data-testid="execution-removed">
            {job ? t("planner.execution.job_removed") : t("planner.execution.job_missing")}
          </span>
        )}
        <Button size="sm" variant="ghost" className="self-start" onClick={() => navigate(SCREEN_ROUTES.jobs)} data-testid="execution-open-jobs">
          <ExternalLink />
          {t("planner.execution.open_jobs")}
        </Button>
      </div>
    );
  }

  const detail = describeJobDetail(job, t);
  const error = describeJobError(job, t);
  const percent = job.progress != null ? Math.round(job.progress * 100) : null;
  const stateLabel = job.state === "paused" ? t("planner.execution.paused") : job.state === "queued" ? t("planner.execution.queued") : finished ? t(`jobs.state.${job.state}`) : null;
  // The live region names the paused state too (review L7), not only the badge.
  const liveText = [job.state === "paused" ? t("planner.execution.paused") : null, detail ?? (job.state === "queued" ? t("planner.execution.queued") : null)].filter(Boolean).join(" · ");

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
          <Progress value={job.state === "queued" || job.state === "paused" ? null : job.progress} label={t("planner.execution.title", { id: shortJobId(job.id) })} />
          <div className="flex flex-wrap items-center gap-2 text-xs text-fg-muted" aria-live="polite" aria-atomic="true" data-testid="execution-detail">
            {liveText}
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
      {finished && execution && <ResultCard execution={execution} plan={plan} response={response} error={job.state === "failed" ? error : null} />}
      {finished && !execution && !resultSettled && (
        <span className="flex items-center gap-2 text-xs text-fg-muted" role="status" data-testid="execution-result-loading">
          <Spinner />
          {t("jobs.result_loading")}
        </span>
      )}
      {finished && !execution && resultSettled && (
        <p className={job.state === "failed" ? "text-xs text-danger selectable" : "text-xs text-fg-muted selectable"} role={job.state === "failed" ? "alert" : undefined} data-testid="execution-outcome">
          {error ?? detail ?? t(`jobs.state.${job.state}`)}
        </p>
      )}
    </div>
  );
}
