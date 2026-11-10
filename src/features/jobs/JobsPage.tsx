import { ArrowUp, Pause, Play, RotateCw, Trash2, X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { describeError } from "@/lib/errors";
import { isJobFinished, type Job } from "@/lib/types";
import { formatBytes, formatDate } from "@/lib/utils";
import { useJobResultLoaded, useJobsStore } from "@/stores/jobs";
import { useUiStore } from "@/stores/ui";

import { describeJobDetail, describeJobError, describeJobPriority, describeJobSubject } from "./describe";
import { JobResult } from "./JobResult";

/** Engine start cannot be interrupted (P1); every other kind unloads / discards partial files on cancel. */
const CANCELLABLE: Job["kind"][] = [
  "model_load",
  "model_import",
  "model_download",
  "model_hash",
  "document_import",
  "document_rescan",
  "document_render",
  "knowledge_index",
  "knowledge_embed",
  "pipeline",
  "document_split",
  "document_merge",
  "plan",
  "export",
  "organize",
  "undo",
  "redo",
  "draft",
  "maintenance",
  "schema_discovery",
];

type TabKey = "queued" | "active" | "finished" | "failed";

/** 대기 (queued · paused) / 진행 중 (running) / 완료 (done · cancelled) / 실패 (failed · interrupted). */
export function jobTab(job: Job): TabKey {
  switch (job.state) {
    case "queued":
    case "paused":
      return "queued";
    case "running":
      return "active";
    case "failed":
    case "interrupted":
      return "failed";
    default:
      return "finished";
  }
}

function stateVariant(j: Job): "ok" | "warn" | "danger" | "muted" | "info" {
  switch (j.state) {
    case "done":
      return "ok";
    case "failed":
    case "interrupted":
      return "danger";
    case "cancelled":
      return "muted";
    case "running":
      return "info";
    default:
      return "warn";
  }
}

function priorityVariant(p: Job["priority"]): "ok" | "warn" | "danger" | "muted" | "info" | "outline" {
  switch (p) {
    case "high":
    case "interactive":
      return "warn";
    case "low":
      return "muted";
    default:
      return "outline";
  }
}

type Action = "cancel" | "pause" | "resume" | "retry" | "prioritize";

function JobRow({ job, onAction }: { job: Job; onAction: (action: Action, id: string) => void }) {
  const { t, i18n } = useTranslation();
  const finished = isJobFinished(job);
  const subject = describeJobSubject(job, t);
  const detail = describeJobDetail(job, t);
  const error = describeJobError(job, t);
  const persistent = job.persistent === true;
  // `jobs://update` omits `result`; the store fetches it once the job finishes.
  const resultLoaded = useJobResultLoaded(job.id);
  const priority = job.priority ?? "normal";
  const canCancel = !finished && (persistent || CANCELLABLE.includes(job.kind));
  const canPause = persistent && (job.state === "running" || job.state === "queued");
  const canResume = job.state === "paused";
  const canRetry = persistent && (job.state === "failed" || job.state === "cancelled" || job.state === "interrupted");
  const canPrioritize = persistent && job.state === "queued" && priority !== "high" && priority !== "interactive";
  return (
    <li className="flex flex-col gap-2 py-3" data-testid={`job-${job.id}`} data-job-kind={job.kind} data-job-subject={job.subject} data-job-state={job.state}>
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="outline">{t(`jobs.kind.${job.kind}`)}</Badge>
        <span className="font-medium selectable">{subject}</span>
        {job.label && job.label !== job.subject && (
          <span className="truncate text-xs text-fg-muted selectable" data-testid="job-label">
            {job.label}
          </span>
        )}
        <Badge variant={stateVariant(job)} data-testid="job-state">
          {t(`jobs.state.${job.state}`)}
        </Badge>
        {persistent && (
          <Badge variant={priorityVariant(priority)} data-testid="job-priority" data-priority={priority}>
            {describeJobPriority(priority, t)}
          </Badge>
        )}
        {(job.attempts ?? 1) > 1 && (
          <span className="text-xs text-fg-subtle" data-testid="job-attempts">
            {t("jobs.attempts", { count: job.attempts })}
          </span>
        )}
        <span className="text-xs text-fg-subtle">{formatDate(job.created_at, i18n.language)}</span>
        <span className="ml-auto flex items-center gap-1">
          {job.progress != null && !finished && <span className="text-xs tabular-nums text-fg-muted">{Math.round(job.progress * 100)}%</span>}
          {canPrioritize && (
            <Button variant="ghost" size="sm" onClick={() => onAction("prioritize", job.id)} data-testid="job-prioritize">
              <ArrowUp />
              {t("jobs.prioritize")}
            </Button>
          )}
          {canPause && (
            <Button variant="ghost" size="sm" onClick={() => onAction("pause", job.id)} data-testid="job-pause">
              <Pause />
              {t("jobs.pause")}
            </Button>
          )}
          {canResume && (
            <Button variant="ghost" size="sm" onClick={() => onAction("resume", job.id)} data-testid="job-resume">
              <Play />
              {t("jobs.resume")}
            </Button>
          )}
          {canRetry && (
            <Button variant="ghost" size="sm" onClick={() => onAction("retry", job.id)} data-testid="job-retry">
              <RotateCw />
              {t("jobs.retry")}
            </Button>
          )}
          {canCancel && (
            <Button variant="ghost" size="sm" disabled={job.cancel_requested} onClick={() => onAction("cancel", job.id)} data-testid="job-cancel">
              <X />
              {t("jobs.cancel")}
            </Button>
          )}
        </span>
      </div>
      {!finished && <Progress value={job.state === "queued" || job.state === "paused" ? null : job.progress} label={subject} />}
      <div className="flex flex-wrap gap-3 text-xs text-fg-muted">
        {job.bytes_total != null && (
          <span className="tabular-nums">
            {formatBytes(job.bytes_done ?? 0)} / {formatBytes(job.bytes_total)}
          </span>
        )}
        {detail && (
          <span className="selectable" data-testid="job-detail">
            {detail}
          </span>
        )}
        {error && (
          <span className="text-danger selectable" data-testid="job-error">
            {error}
          </span>
        )}
      </div>
      {finished && persistent && !resultLoaded && job.result == null && (
        <span className="text-xs text-fg-subtle" role="status" data-testid="job-result-loading">
          {t("jobs.result_loading")}
        </span>
      )}
      {finished && job.result != null && (
        <details className="text-xs" data-testid="job-result">
          <summary className="cursor-pointer text-fg-muted">{t("jobs.result")}</summary>
          <div className="mt-1">
            <JobResult job={job} />
          </div>
        </details>
      )}
    </li>
  );
}

export function JobsPage() {
  const { t } = useTranslation();
  const jobs = useJobsStore((s) => s.jobs);
  const store = useJobsStore();
  const toast = useUiStore((s) => s.toast);
  const [tab, setTab] = useState<string>("active");
  const act = (action: Action, id: string) => {
    const fn = action === "cancel" ? store.cancel : action === "pause" ? store.pause : action === "resume" ? store.resume : action === "retry" ? store.retry : store.prioritize;
    fn(id).catch((e: unknown) => toast(describeError(e, t), "danger"));
  };
  const clearFinished = () => store.clearFinished().catch((e: unknown) => toast(describeError(e, t), "danger"));
  const byTab: Record<TabKey, Job[]> = { queued: [], active: [], finished: [], failed: [] };
  for (const j of jobs) byTab[jobTab(j)].push(j);
  byTab.finished.reverse();
  byTab.failed.reverse();
  const finishedCount = byTab.finished.length + byTab.failed.length;
  const tabs: { key: TabKey; empty: string }[] = [
    { key: "queued", empty: "jobs.empty_queued" },
    { key: "active", empty: "jobs.empty" },
    { key: "finished", empty: "jobs.empty_finished" },
    { key: "failed", empty: "jobs.empty_failed" },
  ];

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-4 p-6" data-testid="page-jobs">
      <div className="flex items-center justify-between gap-2">
        <h1 className="text-xl font-semibold">{t("jobs.title")}</h1>
        <Button variant="ghost" size="sm" disabled={finishedCount === 0} onClick={() => void clearFinished()}>
          <Trash2 />
          {t("jobs.clear_finished")}
        </Button>
      </div>
      <Card>
        <CardHeader>
          <CardTitle>{t("jobs.title")}</CardTitle>
        </CardHeader>
        <CardContent>
          <Tabs value={tab} onValueChange={setTab}>
            <TabsList>
              {tabs.map(({ key }) => (
                <TabsTrigger key={key} value={key} data-testid={`jobs-tab-${key}`}>
                  {t(`jobs.tabs.${key}`)} ({byTab[key].length})
                </TabsTrigger>
              ))}
            </TabsList>
            {tabs.map(({ key, empty }) => (
              <TabsContent key={key} value={key}>
                {byTab[key].length === 0 ? (
                  <p className="py-4 text-sm text-fg-muted">{t(empty)}</p>
                ) : (
                  <ul className="divide-y divide-border">
                    {byTab[key].map((j) => (
                      <JobRow key={j.id} job={j} onAction={act} />
                    ))}
                  </ul>
                )}
              </TabsContent>
            ))}
          </Tabs>
        </CardContent>
      </Card>
    </div>
  );
}
