import { useMemo } from "react";
import { create } from "zustand";

import { api, subscribe } from "@/lib/ipc";
import { EVENTS, isJobFinished, type Job } from "@/lib/types";

interface JobsState {
  jobs: Job[];
  initialized: boolean;
  init: () => Promise<void>;
  refresh: () => Promise<void>;
  cancel: (jobId: string) => Promise<boolean>;
  clearFinished: () => Promise<void>;
  // -- persistent runner (P5) --
  pause: (jobId: string) => Promise<boolean>;
  resume: (jobId: string) => Promise<boolean>;
  retry: (jobId: string) => Promise<boolean>;
  prioritize: (jobId: string) => Promise<boolean>;
}

export const useJobsStore = create<JobsState>((set, get) => ({
  jobs: [],
  initialized: false,

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });
    await subscribe<Job[]>(EVENTS.jobsUpdate, (jobs) => set({ jobs }));
    await get().refresh();
  },

  refresh: async () => {
    try {
      set({ jobs: await api.jobsList() });
    } catch {
      /* ignore */
    }
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
