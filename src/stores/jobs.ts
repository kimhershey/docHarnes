import { useMemo } from "react";
import { create } from "zustand";

import { api, subscribe } from "@/lib/ipc";
import { EVENTS, isJobFinished, type Job } from "@/lib/types";

/**
 * Job book mirror. `jobs://update` carries the list without `Job.result` (the structured outcome
 * is large and the event fires on every progress tick), so results are fetched with `jobs_list`
 * when a job finishes and with `jobs_get` on demand, cached by id and re-attached to the
 * event-driven list. `results` holds the ids whose result state is known (value may be null).
 */
interface JobsState {
  jobs: Job[];
  /** Results delivered by `jobs_list` / `jobs_get`, by job id; a present key means "known" (null = no result). */
  results: Record<string, unknown>;
  initialized: boolean;
  init: () => Promise<void>;
  refresh: () => Promise<void>;
  /** Fetches one job with its result (null when the book no longer knows it) and merges it into the list. */
  fetch: (jobId: string) => Promise<Job | null>;
  cancel: (jobId: string) => Promise<boolean>;
  clearFinished: () => Promise<void>;
  // -- persistent runner (P5) --
  pause: (jobId: string) => Promise<boolean>;
  resume: (jobId: string) => Promise<boolean>;
  retry: (jobId: string) => Promise<boolean>;
  prioritize: (jobId: string) => Promise<boolean>;
}

const inflight = new Map<string, Promise<Job | null>>();
let refreshScheduled = false;

function hasResult(job: Job): boolean {
  return job.result !== undefined && job.result !== null;
}

/** Re-attaches cached results to an event payload; unfinished jobs (retry) drop their cached result. */
function attach(incoming: Job[], results: Record<string, unknown>): { jobs: Job[]; results: Record<string, unknown> } {
  const next: Record<string, unknown> = {};
  const jobs = incoming.map((j) => {
    if (!isJobFinished(j)) return j;
    if (hasResult(j)) {
      next[j.id] = j.result;
      return j;
    }
    if (j.id in results) {
      next[j.id] = results[j.id];
      return { ...j, result: results[j.id] };
    }
    return j;
  });
  return { jobs, results: next };
}

export const useJobsStore = create<JobsState>((set, get) => ({
  jobs: [],
  results: {},
  initialized: false,

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });
    await subscribe<Job[]>(EVENTS.jobsUpdate, (incoming) => {
      const prev = get();
      const before = new Map(prev.jobs.map((j) => [j.id, j]));
      // A job that just finished has its result only in `jobs_list`: fetch the list once per burst.
      const transitioned = incoming.some((j) => isJobFinished(j) && !hasResult(j) && !(j.id in prev.results) && !(before.get(j.id) && isJobFinished(before.get(j.id)!)));
      set(attach(incoming, prev.results));
      if (transitioned && !refreshScheduled) {
        refreshScheduled = true;
        queueMicrotask(() => {
          refreshScheduled = false;
          void get().refresh();
        });
      }
    });
    await get().refresh();
  },

  refresh: async () => {
    try {
      const list = await api.jobsList();
      const results: Record<string, unknown> = {};
      for (const j of list) if (isJobFinished(j)) results[j.id] = j.result ?? null;
      set({ jobs: list, results });
    } catch {
      /* ignore */
    }
  },

  fetch: async (jobId) => {
    const pending = inflight.get(jobId);
    if (pending) return pending;
    const p = (async () => {
      try {
        const job = await api.jobsGet(jobId);
        if (job && isJobFinished(job)) {
          set((s) => ({
            results: { ...s.results, [jobId]: job.result ?? null },
            jobs: s.jobs.map((j) => (j.id === jobId ? { ...j, ...job } : j)),
          }));
        }
        return job;
      } catch {
        return null;
      } finally {
        inflight.delete(jobId);
      }
    })();
    inflight.set(jobId, p);
    return p;
  },

  cancel: async (jobId) => {
    const ok = await api.jobsCancel(jobId);
    await get().refresh();
    return ok;
  },

  clearFinished: async () => {
    await api.jobsClearFinished();
    await get().refresh();
  },

  pause: async (jobId) => {
    const ok = await api.jobsPause(jobId);
    await get().refresh();
    return ok;
  },

  resume: async (jobId) => {
    const ok = await api.jobsResume(jobId);
    await get().refresh();
    return ok;
  },

  retry: async (jobId) => {
    const ok = await api.jobsRetry(jobId);
    await get().refresh();
    return ok;
  },

  prioritize: async (jobId) => {
    const ok = await api.jobsPrioritize(jobId);
    await get().refresh();
    return ok;
  },
}));

/** Selectors must return stable references (zustand v5 / useSyncExternalStore): derive with useMemo. */
export function useActiveJobs(): Job[] {
  const jobs = useJobsStore((s) => s.jobs);
  return useMemo(() => jobs.filter((j) => !isJobFinished(j)), [jobs]);
}

export function useJobs(): Job[] {
  return useJobsStore((s) => s.jobs);
}

/** One job by id (null until the book knows it, or after it was cleared). */
export function useJob(jobId: string | null): Job | null {
  const jobs = useJobsStore((s) => s.jobs);
  return useMemo(() => (jobId ? (jobs.find((j) => j.id === jobId) ?? null) : null), [jobs, jobId]);
}

/** Whether a finished job's result has been delivered by `jobs_list` / `jobs_get` (the broadcast omits it). */
export function useJobResultLoaded(jobId: string | null): boolean {
  return useJobsStore((s) => (jobId ? jobId in s.results : false));
}
