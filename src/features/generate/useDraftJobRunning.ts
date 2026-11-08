import { isJobFinished } from "@/lib/types";
import { useGenerateStore } from "@/stores/generate";
import { useJob } from "@/stores/jobs";

/**
 * Whether the 문서 생성 screen is still following a running draft job. Opening another draft
 * (assistant result card, jobs page) would drop that job's hand-over, so the callers confirm first.
 */
export function useDraftJobRunning(): boolean {
  const jobId = useGenerateStore((s) => s.jobId);
  const job = useJob(jobId);
  return jobId !== null && !!job && !isJobFinished(job);
}
