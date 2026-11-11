import type { TFunction } from "i18next";

import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { defaultImportOptions, isJobFinished, type DocumentRole, type ImportReport, type ImportSource, type Job } from "@/lib/types";
import { useJobsStore } from "@/stores/jobs";
import { useUiStore } from "@/stores/ui";

import type { DragDoc } from "./store";

/** Resolves once the job is finished (with its structured result when the backend attached one); null on timeout. */
export function waitForJob(jobId: string, timeoutMs = 15 * 60_000): Promise<Job | null> {
  return new Promise((resolve) => {
    let done = false;
    let unsub = () => undefined as void;
    const finish = (job: Job | null) => {
      if (done) return;
      done = true;
      unsub();
      clearInterval(poll);
      clearTimeout(limit);
      resolve(job);
    };
    const check = async () => {
      if (done) return;
      const known = useJobsStore.getState().jobs.find((j) => j.id === jobId);
      if (known && isJobFinished(known)) {
        const full = await useJobsStore.getState().fetch(jobId);
        finish(full ?? known);
        return;
      }
      if (!known) {
        // The mirror has not seen the job yet (store not initialised on this screen): ask directly.
        const job = await api.jobsGet(jobId).catch(() => null);
        if (job && isJobFinished(job)) finish(job);
      }
    };
    unsub = useJobsStore.subscribe(() => void check());
    const poll = setInterval(() => void check(), 1000);
    const limit = setTimeout(() => finish(null), timeoutMs);
    void check();
  });
}

export interface DroppedImportOptions {
  /** Role of the new documents (`knowledge` for collections). */
  role?: DocumentRole;
  /** Server-side attach targets (the whole batch, re-encountered documents included). */
  collectionId?: string | null;
  projectId?: string | null;
  source?: ImportSource;
}

function isReport(v: unknown): v is ImportReport {
  return !!v && typeof v === "object" && Array.isArray((v as ImportReport).imported);
}

/**
 * The documents a finished import stands for: the ones it registered or relinked plus the registered documents
 * the batch met again (`ImportReport.existing`, 0.8.0). `planned`: the documents the plan said were already
 * there — used for a report without the `existing` list.
 */
export function importedDocumentIds(report: ImportReport, planned: Iterable<string> = []): string[] {
  return [...new Set([...report.imported, ...report.relinked, ...(report.existing ?? planned)])];
}

/**
 * Imports OS files dropped on a target and returns the ids of every document the drop stands
 * for: documents already in the library (duplicates / re-registered / relinked) plus the ones the
 * import job created. Progress and failures are reported with toasts.
 *
 * With a target (`collectionId` / `projectId`) every returned document has joined it: the import job attaches
 * the whole batch, the documents it merely met again included — and since 0.8.0 it runs for a drop that holds
 * nothing new as well: the job then only puts the registered documents into the target and names them in
 * `ImportReport.existing` (PRJ-1: such a drop used to stop at "nothing to import" and report "added" without
 * adding anything). Without a target a drop of registered files starts no job (the backend refuses a run with
 * nothing to import and nothing to attach): the caller uses the ids itself — the assistant attaches them.
 */
export async function importDroppedFiles(paths: string[], opts: DroppedImportOptions, t: TFunction): Promise<string[]> {
  const toast = useUiStore.getState().toast;
  const options = { ...defaultImportOptions(), role: opts.role ?? "work", collection_id: opts.collectionId ?? null, project_id: opts.projectId ?? null };
  const source = opts.source ?? "drop";
  const ids = new Set<string>();
  const hasTarget = !!(opts.collectionId || opts.projectId);
  // After a failed import nothing is known to have joined the target, so nothing is reported as added; without a
  // target the documents already in the library still stand for the drop.
  const afterFailure = () => (hasTarget ? [] : [...ids]);
  try {
    const plan = await api.documentsImportPlan(paths, options, source);
    for (const item of plan.items) {
      const s = item.status;
      if (s.kind === "already_registered" || s.kind === "relink") ids.add(s.id);
      else if (s.kind === "duplicate") ids.add(s.of);
    }
    const importable = plan.new_count + plan.relink_count;
    if (importable === 0) {
      if (ids.size === 0) {
        toast(t("dnd.nothing"), "warn");
        return [];
      }
      // Nothing new and no target: the registered documents stand for the drop as they are.
      if (!hasTarget) return [...ids];
      // With a target the job runs all the same: it attaches the registered documents (below).
    } else toast(t("dnd.importing", { count: importable }), "info");
    const jobId = await api.documentsImportRun(plan.token, paths, options, source);
    const job = await waitForJob(jobId);
    const report = job && isReport(job.result) ? job.result : null;
    if (report) {
      if (report.failed.length > 0) toast(t("dnd.partial_failed", { count: report.failed.length }), "warn");
      // What the job says it imported and met again — a document deleted since the preview is not among them.
      return importedDocumentIds(report, ids);
    } else if (job?.state === "failed") {
      toast(t("dnd.import_failed", { error: job.error ?? "" }), "danger");
      return afterFailure();
    } else if (!job) {
      toast(t("dnd.import_failed", { error: "timeout" }), "danger");
      return afterFailure();
    }
    return [...ids];
  } catch (e) {
    // A backend error is a `{ code, message }` object, not an `Error` (`String(e)` would print "[object Object]").
    toast(t("dnd.import_failed", { error: describeError(e, t) }), "danger");
    return afterFailure();
  }
}

/** Drag payloads for document ids (names from the library; an unknown id keeps its short id). */
export async function docsFromIds(ids: string[]): Promise<DragDoc[]> {
  return Promise.all(
    ids.map((id) =>
      api
        .documentsGet(id)
        .then((d): DragDoc => ({ id, name: d.title ?? d.original_name, absolute_path: d.absolute_path }))
        .catch((): DragDoc => ({ id, name: id.slice(0, 8) })),
    ),
  );
}
