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
 * What the import job does for the documents a batch merely re-encountered (`import::run`): they join the
 * collection — taking the batch role when that is not the default — and / or the project.
 */
async function attachExisting(ids: string[], opts: DroppedImportOptions): Promise<void> {
  if (opts.collectionId) await api.knowledgeCollectionAdd(opts.collectionId, ids, (opts.role ?? "work") !== "work");
  if (opts.projectId) await api.projectAddDocuments(opts.projectId, ids);
}

/**
 * Imports OS files dropped on a target and returns the ids of every document the drop stands
 * for: documents already in the library (duplicates / re-registered / relinked) plus the ones the
 * import job created. Progress and failures are reported with toasts.
 *
 * With a target (`collectionId` / `projectId`) every returned document has joined it: the import job attaches
 * the batch, and when the drop holds nothing new — so no job runs — the documents are attached here (PRJ-1: the
 * drop used to report "added" without adding anything). Without a target the caller uses the ids itself.
 */
export async function importDroppedFiles(paths: string[], opts: DroppedImportOptions, t: TFunction): Promise<string[]> {
  const toast = useUiStore.getState().toast;
  const options = { ...defaultImportOptions(), role: opts.role ?? "work", collection_id: opts.collectionId ?? null, project_id: opts.projectId ?? null };
  const source = opts.source ?? "drop";
  const ids = new Set<string>();
  // After a failed import nothing is known to have joined the target, so nothing is reported as added; without a
  // target the documents already in the library still stand for the drop.
  const afterFailure = () => (opts.collectionId || opts.projectId ? [] : [...ids]);
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
      // Nothing new, so no import job attaches the batch: the documents the files stand for join the target here.
      try {
        await attachExisting([...ids], opts);
      } catch (e) {
        toast(describeError(e, t), "danger");
        return [];
      }
      return [...ids];
    }
    toast(t("dnd.importing", { count: importable }), "info");
    const jobId = await api.documentsImportRun(plan.token, paths, options, source);
    const job = await waitForJob(jobId);
    const report = job && isReport(job.result) ? job.result : null;
    if (report) {
      for (const id of [...report.imported, ...report.relinked]) ids.add(id);
      if (report.failed.length > 0) toast(t("dnd.partial_failed", { count: report.failed.length }), "warn");
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
