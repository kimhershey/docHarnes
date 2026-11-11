import { describe, expect, it } from "vitest";

import { reextractProcessRequest, type ReextractRequest } from "./reextract";

const doc = (mode: ReextractRequest["mode"]): ReextractRequest => ({ documentId: "d1", documentName: "견적서.pdf", mode });

describe("re-extraction from the review queue (PIP-2)", () => {
  it("the fine-print profile does not force classification / OCR — hand-corrected page text stays", () => {
    const req = reextractProcessRequest(doc("fine_print"), false);
    expect(req).toEqual({ document_ids: ["d1"], level: "extract", profile: "fine_print" });
    expect(req.force).toBeUndefined();
  });

  it("starts over (classification and OCR again) only on the reviewer's explicit choice", () => {
    expect(reextractProcessRequest(doc("fine_print"), true)).toEqual({ document_ids: ["d1"], level: "extract", profile: "fine_print", force: true });
  });

  it("the precise mode is an extraction with the second pass, never forced", () => {
    expect(reextractProcessRequest(doc("precise"), false)).toEqual({ document_ids: ["d1"], level: "extract", precise: true });
    expect(reextractProcessRequest(doc("precise"), true)).toEqual({ document_ids: ["d1"], level: "extract", precise: true });
  });
});
