import type { DocumentRecord, DocumentStatus, Job, JobKind } from "@/lib/types";

import { statusVariant } from "./status";

/**
 * The error text `documents::import::process_document` stores on a document before it converts / renders it —
 * crash safety: a row that dies inside a parser must not stay `importing`. While the work runs the row therefore
 * reads `status: error` with exactly this text (DOC-8).
 */
export const PROCESSING_MARKER = "interrupted while processing";

/** What the status badge shows: the stored status, or one of the two readings of the marker. */
export type DocumentDisplayStatus = DocumentStatus | "processing" | "interrupted";

/** Job kinds that convert / render documents. */
const DOCUMENT_JOB_KINDS: ReadonlySet<JobKind> = new Set<JobKind>(["document_import", "document_rescan", "document_render", "document_split", "document_merge"]);
/** A plan job's progress line ("2/3 · rerender · name (1/4)") at a step that converts / renders documents. */
const DOCUMENT_PLAN_STEP = /^\d+\/\d+ · (?:import|rerender|rotate|split|merge)(?: · |$)/;

/** True while a job that converts / renders documents is running in the workspace. */
export function documentWorkRunning(jobs: readonly Pick<Job, "kind" | "state" | "detail">[]): boolean {
  return jobs.some((j) => j.state === "running" && (DOCUMENT_JOB_KINDS.has(j.kind) || (j.kind === "plan" && DOCUMENT_PLAN_STEP.test(j.detail ?? ""))));
}

/**
 * The status to show for a document. A row that carries the marker is being worked on while such a job runs —
 * "처리 중", not the red "오류" that made users re-render or delete a large file in the middle of its conversion.
 * With no job alive the marker means what it says: the work was cut off ("중단됨"; re-rendering processes it again).
 * Any other error stays an error.
 */
export function documentDisplayStatus(doc: Pick<DocumentRecord, "status" | "error">, working: boolean): DocumentDisplayStatus {
  if (doc.status !== "error" || doc.error !== PROCESSING_MARKER) return doc.status;
  return working ? "processing" : "interrupted";
}

/** Badge colour of a display status. */
export function displayStatusVariant(s: DocumentDisplayStatus): ReturnType<typeof statusVariant> {
  if (s === "processing") return "info";
  if (s === "interrupted") return "warn";
  return statusVariant(s);
}
