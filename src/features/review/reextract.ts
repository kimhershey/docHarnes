import type { ProcessRequest } from "@/lib/types";

/** What the reviewer asked for: the fine-print profile or the precise (second-pass) mode, for one document. */
export interface ReextractRequest {
  documentId: string;
  documentName: string;
  mode: "fine_print" | "precise";
}

/**
 * The job of a confirmed re-extraction from the 검토함. Never `force` on its own (PIP-2): a forced run classifies
 * and reads the pages again — that happens only when the reviewer ticked "start over". Its hint says that page
 * text corrected by hand is replaced by the new OCR result too, which since 0.8.0 a forced run only does when
 * the request asks for it (`reocr_manual`; `pipeline::run` keeps manual corrections otherwise), so "start over"
 * sends both. Without it the pages are read at the fine-print resolution for the extraction only.
 */
export function reextractProcessRequest(request: ReextractRequest, startOver: boolean): ProcessRequest {
  const base: ProcessRequest = { document_ids: [request.documentId], level: "extract" };
  if (request.mode === "precise") return { ...base, precise: true };
  return startOver ? { ...base, profile: "fine_print", force: true, reocr_manual: true } : { ...base, profile: "fine_print" };
}
