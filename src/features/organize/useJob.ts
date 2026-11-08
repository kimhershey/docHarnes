import { useJobsStore } from "@/stores/jobs";
import type { Job } from "@/lib/types";

/** The live record of one job from the jobs store (`jobs://update`), or undefined when unknown. */
export function useJob(jobId: string | null): Job | undefined {
  return useJobsStore((s) => (jobId ? s.jobs.find((j) => j.id === jobId) : undefined));
}
