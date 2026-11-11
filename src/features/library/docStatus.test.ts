import { describe, expect, it } from "vitest";

import type { Job, JobKind, JobState } from "@/lib/types";

import { displayStatusVariant, documentDisplayStatus, documentWorkRunning, PROCESSING_MARKER } from "./docStatus";

const job = (kind: JobKind, state: JobState = "running", detail: string | null = null): Pick<Job, "kind" | "state" | "detail"> => ({ kind, state, detail });

describe("document status on display (DOC-8)", () => {
  it("reads the marker as 처리 중 while a job converts / renders documents", () => {
    const doc = { status: "error" as const, error: PROCESSING_MARKER };
    expect(documentDisplayStatus(doc, true)).toBe("processing");
    expect(displayStatusVariant("processing")).toBe("info");
  });

  it("reads the marker as 중단됨 when no such job is alive", () => {
    expect(documentDisplayStatus({ status: "error", error: PROCESSING_MARKER }, false)).toBe("interrupted");
    expect(displayStatusVariant("interrupted")).toBe("warn");
  });

  it("keeps a real error an error and every other status as it is, whatever runs", () => {
    expect(documentDisplayStatus({ status: "error", error: "invalid value: not a PDF" }, true)).toBe("error");
    expect(documentDisplayStatus({ status: "error", error: null }, true)).toBe("error");
    // Only the exact marker counts.
    expect(documentDisplayStatus({ status: "error", error: `${PROCESSING_MARKER} (page 3)` }, true)).toBe("error");
    expect(documentDisplayStatus({ status: "rendered", error: null }, true)).toBe("rendered");
    expect(documentDisplayStatus({ status: "importing", error: PROCESSING_MARKER }, true)).toBe("importing");
    expect(displayStatusVariant("error")).toBe("danger");
    expect(displayStatusVariant("rendered")).toBe("ok");
  });

  it("counts running import / rescan / render / split / merge jobs as document work", () => {
    for (const kind of ["document_import", "document_rescan", "document_render", "document_split", "document_merge"] as const) expect(documentWorkRunning([job(kind)]), kind).toBe(true);
    expect(documentWorkRunning([])).toBe(false);
    // Not running, or another kind of work.
    expect(documentWorkRunning([job("document_import", "queued"), job("document_render", "paused"), job("document_render", "done")])).toBe(false);
    expect(documentWorkRunning([job("pipeline"), job("knowledge_index"), job("export"), job("draft")])).toBe(false);
  });

  it("counts a plan job only at a step that converts / renders documents", () => {
    expect(documentWorkRunning([job("plan", "running", "1/2 · import")])).toBe(true);
    expect(documentWorkRunning([job("plan", "running", "2/3 · rerender · 계약서.pdf (1/4)")])).toBe(true);
    expect(documentWorkRunning([job("plan", "running", "1/1 · rotate")])).toBe(true);
    expect(documentWorkRunning([job("plan", "running", "1/2 · extract · 견적서.jpg (1/1)")])).toBe(false);
    expect(documentWorkRunning([job("plan", "running", "1/2 · import_models")])).toBe(false);
    expect(documentWorkRunning([job("plan", "running", null)])).toBe(false);
  });
});
