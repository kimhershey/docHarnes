import type { ProcessRequest } from "@/lib/types";

/** What the reviewer asked for: the fine-print profile or the precise (second-pass) mode, for one document. */
export interface ReextractRequest {
  documentId: string;
  documentName: string;
  mode: "fine_print" | "precise";
}

/**
 * The job of a confirmed re-extraction from the 검토함. Never `force` on its own (PIP-2): a forced run classifies
 * and reads the pages again, which replaces page text corrected by hand (`pipeline::run`: OCR output and manual
 * corrections are only replaced by a forced re-run) — that happens only when the reviewer ticked "start over".
 * Without it the pages are read at the fine-print resolution for the extraction only.
 */
export function reextractProcessRequest(request: ReextractRequest, startOver: boolean): ProcessRequest {
  const base: ProcessRequest = { document_ids: [request.documentId], level: "extract" };
  if (request.mode === "precise") return { ...base, precise: true };
  return startOver ? { ...base, profile: "fine_print", force: true } : { ...base, profile: "fine_print" };
}
