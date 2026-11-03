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
}));

/** Selectors must return stable references (zustand v5 / useSyncExternalStore): derive with useMemo. */
export function useActiveJobs(): Job[] {
  const jobs = useJobsStore((s) => s.jobs);
  return useMemo(() => jobs.filter((j) => !isJobFinished(j)), [jobs]);
}

export function useJobs(): Job[] {
  return useJobsStore((s) => s.jobs);
}
