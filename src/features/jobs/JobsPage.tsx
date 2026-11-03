import { Trash2, X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { isJobFinished, type Job } from "@/lib/types";
import { formatBytes, formatDate } from "@/lib/utils";
import { useJobsStore } from "@/stores/jobs";

function stateVariant(j: Job): "ok" | "warn" | "danger" | "muted" | "info" {
  switch (j.state) {
    case "done":
      return "ok";
    case "failed":
      return "danger";
    case "cancelled":
      return "muted";
    case "running":
      return "info";
    default:
      return "warn";
  }
}

function JobRow({ job, onCancel }: { job: Job; onCancel: (id: string) => void }) {
  const { t, i18n } = useTranslation();
  const finished = isJobFinished(job);
  return (
    <li className="flex flex-col gap-2 py-3" data-testid={`job-${job.kind}-${job.subject}`}>
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="outline">{t(`jobs.kind.${job.kind}`)}</Badge>
        <span className="font-medium selectable">{job.subject}</span>
        <Badge variant={stateVariant(job)} data-testid="job-state">
          {t(`jobs.state.${job.state}`)}
        </Badge>
        <span className="text-xs text-fg-subtle">{formatDate(job.created_at, i18n.language)}</span>
        <span className="ml-auto flex items-center gap-2">
          {job.progress != null && !finished && <span className="text-xs tabular-nums text-fg-muted">{Math.round(job.progress * 100)}%</span>}
          {!finished && (
            <Button variant="ghost" size="sm" disabled={job.cancel_requested} onClick={() => onCancel(job.id)} data-testid="job-cancel">
              <X />
              {t("jobs.cancel")}
            </Button>
          )}
        </span>
      </div>
      {!finished && <Progress value={job.state === "queued" ? null : job.progress} label={job.subject} />}
      <div className="flex flex-wrap gap-3 text-xs text-fg-muted">
        {job.bytes_total != null && (
          <span className="tabular-nums">
            {formatBytes(job.bytes_done ?? 0)} / {formatBytes(job.bytes_total)}
          </span>
        )}
        {job.detail && <span className="selectable">{job.detail}</span>}
        {job.error && <span className="text-danger selectable">{job.error}</span>}
      </div>
    </li>
  );
}

export function JobsPage() {
  const { t } = useTranslation();
  const jobs = useJobsStore((s) => s.jobs);
  const cancel = useJobsStore((s) => s.cancel);
  const clearFinished = useJobsStore((s) => s.clearFinished);
  const [tab, setTab] = useState("active");
  const active = jobs.filter((j) => !isJobFinished(j));
  const finished = jobs.filter(isJobFinished).slice().reverse();

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-4 p-6" data-testid="page-jobs">
      <div className="flex items-center justify-between gap-2">
        <h1 className="text-xl font-semibold">{t("jobs.title")}</h1>
        <Button variant="ghost" size="sm" disabled={finished.length === 0} onClick={() => void clearFinished()}>
          <Trash2 />
          {t("jobs.clear_finished")}
        </Button>
      </div>
      <Card>
        <CardHeader>
          <CardTitle>{t("jobs.title")}</CardTitle>
          <CardDescription>{t("jobs.p5_hint")}</CardDescription>
        </CardHeader>
        <CardContent>
          <Tabs value={tab} onValueChange={setTab}>
            <TabsList>
              <TabsTrigger value="active" data-testid="jobs-tab-active">
                {t("jobs.tabs.active")} ({active.length})
              </TabsTrigger>
              <TabsTrigger value="finished" data-testid="jobs-tab-finished">
                {t("jobs.tabs.finished")} ({finished.length})
              </TabsTrigger>
            </TabsList>
            <TabsContent value="active">
              {active.length === 0 ? (
                <p className="py-4 text-sm text-fg-muted">{t("jobs.empty")}</p>
              ) : (
                <ul className="divide-y divide-border">
                  {active.map((j) => (
                    <JobRow key={j.id} job={j} onCancel={(id) => void cancel(id)} />
                  ))}
                </ul>
              )}
            </TabsContent>
            <TabsContent value="finished">
              {finished.length === 0 ? (
                <p className="py-4 text-sm text-fg-muted">{t("jobs.empty_finished")}</p>
              ) : (
                <ul className="divide-y divide-border">
                  {finished.map((j) => (
                    <JobRow key={j.id} job={j} onCancel={() => undefined} />
                  ))}
                </ul>
              )}
            </TabsContent>
          </Tabs>
        </CardContent>
      </Card>
    </div>
  );
}
