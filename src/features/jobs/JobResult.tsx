import { Check, FolderOpen, X } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { isPlanExecution, isRecord, opLabel } from "@/features/assistant/describePlan";
import { revealPath } from "@/lib/ipc";
import type { Job } from "@/lib/types";

/** `pipeline::run::ProcessReport` (pipeline jobs, extract / classify / ocr steps). */
function isProcessReport(v: unknown): v is { processed: unknown[]; failed: unknown[]; classified: number; ocr_pages: number; extracted: number; review_pending?: number } {
  return isRecord(v) && Array.isArray(v.processed) && Array.isArray(v.failed) && typeof v.classified === "number" && typeof v.ocr_pages === "number" && typeof v.extracted === "number";
}

/**
 * Structured outcome of a finished persistent job (`Job.result`): plan steps,
 * export path with "폴더 열기", organise / undo counts; other shapes list
 * their scalar fields.
 */
export function JobResult({ job }: { job: Job }) {
  const { t } = useTranslation();
  const result = job.result;
  if (result == null) return null;
  if (isPlanExecution(result)) {
    return (
      <ol className="flex flex-col gap-0.5 text-xs" data-testid="job-result-steps">
        {result.steps.map((s) => (
          <li key={s.index} className="flex items-start gap-1.5" data-step-status={s.status}>
            {s.status === "failed" ? <X className="mt-0.5 size-3.5 shrink-0 text-danger" aria-hidden="true" /> : <Check className={`mt-0.5 size-3.5 shrink-0 ${s.status === "done" || s.status === "ui" ? "text-ok" : "text-fg-subtle"}`} aria-hidden="true" />}
            <span className="font-medium">{opLabel(s.op, t)}</span>
            {s.summary && <span className="text-fg-muted selectable">{s.summary}</span>}
            {s.error && <span className="text-danger selectable">{s.error}</span>}
            {(s.status === "skipped" || s.status === "cancelled") && <span className="text-fg-subtle">{t(`planner.result.status.${s.status}`)}</span>}
          </li>
        ))}
      </ol>
    );
  }
  if (isRecord(result) && typeof result.path === "string" && typeof result.absolute === "string") {
    return (
      <div className="flex flex-wrap items-center gap-2 text-xs" data-testid="job-result-export">
        <span className="font-mono selectable">{result.path}</span>
        {typeof result.documents === "number" && <span className="text-fg-muted">{t("jobs.result_export", { documents: result.documents, rows: typeof result.rows === "number" ? result.rows : 0 })}</span>}
        <Button size="sm" variant="ghost" onClick={() => void revealPath(String(result.absolute))} data-testid="job-open-folder">
          <FolderOpen />
          {t("common.open_folder")}
        </Button>
      </div>
    );
  }
  if (isRecord(result) && typeof result.moved === "number" && Array.isArray(result.items)) {
    return (
      <span className="text-xs text-fg-muted" data-testid="job-result-organize">
        {t("jobs.detail.organize_summary", { moved: result.moved, failed: Array.isArray(result.failed) ? result.failed.length : 0, skipped: typeof result.skipped === "number" ? result.skipped : 0 })}
      </span>
    );
  }
  if (isProcessReport(result)) {
    return (
      <span className="text-xs text-fg-muted" data-testid="job-result-process">
        {t("jobs.detail.process_summary", { processed: result.processed.length, classified: result.classified, ocr: result.ocr_pages, extracted: result.extracted, review: typeof result.review_pending === "number" ? result.review_pending : 0, failed: result.failed.length })}
      </span>
    );
  }
  if (isRecord(result) && typeof result.restored === "number" && Array.isArray(result.skipped)) {
    return (
      <span className="text-xs text-fg-muted" data-testid="job-result-undo">
        {t("jobs.detail.undo_summary", { restored: result.restored, skipped: result.skipped.length })}
      </span>
    );
  }
  if (isRecord(result)) {
    const scalars = Object.entries(result).filter(([, v]) => typeof v === "number" || typeof v === "string" || typeof v === "boolean");
    if (scalars.length === 0) return null;
    return (
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-xs" data-testid="job-result-fields">
        {scalars.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-fg-muted">{k}</dt>
            <dd className="selectable">{String(v)}</dd>
          </div>
        ))}
      </dl>
    );
  }
  return null;
}
