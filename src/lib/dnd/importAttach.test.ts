// What a drop stands for after its import job: imported ∪ relinked ∪ existing (`ImportReport.existing`, 0.8.0).
import { describe, expect, it } from "vitest";

import type { ImportReport } from "@/lib/types";

import { importedDocumentIds } from "./importAttach";

const report = (patch: Partial<ImportReport>): ImportReport => ({ batch_id: "b1", imported: [], relinked: [], failed: [], skipped: 0, cancelled: false, ...patch });

describe("importedDocumentIds", () => {
  it("an attach-only run (every file already registered) stands for the documents it met again", () => {
    expect(importedDocumentIds(report({ existing: ["a", "b"] }))).toEqual(["a", "b"]);
  });

  it("a mixed batch: the new, the relinked and the registered ones, each once", () => {
    expect(importedDocumentIds(report({ imported: ["n1"], relinked: ["r1"], existing: ["a", "n1"] }))).toEqual(["n1", "r1", "a"]);
  });

  it("the report decides, not the preview: a document deleted since the plan is not among them", () => {
    expect(importedDocumentIds(report({ imported: ["n1"], existing: ["a"] }), ["a", "gone"])).toEqual(["n1", "a"]);
    expect(importedDocumentIds(report({ imported: ["n1"], existing: [] }), ["gone"])).toEqual(["n1"]);
  });

  it("a report without the list (an older shell) falls back to what the plan said was registered", () => {
    expect(importedDocumentIds(report({ imported: ["n1"] }), new Set(["a"]))).toEqual(["n1", "a"]);
  });
});
