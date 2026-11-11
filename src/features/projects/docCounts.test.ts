import { describe, expect, it } from "vitest";

import type { DocumentRecord, ProjectDocument } from "@/lib/types";

import { projectDocCounts } from "./docCounts";

const doc = (id: string) => ({ id }) as DocumentRecord;
const member = (id: string): ProjectDocument => ({ document: doc(id), added_at: "2026-11-11T00:00:00Z" });

describe("project document counts (PRJ-16)", () => {
  it("counts work and reference documents apart", () => {
    expect(projectDocCounts([member("a"), member("b")], [doc("r")])).toEqual({ work: 2, reference: 1 });
  });

  it("counts a document that is in both lists once, as 참고 자료", () => {
    expect(projectDocCounts([member("a"), member("both")], [doc("both"), doc("r")])).toEqual({ work: 1, reference: 2 });
  });

  it("counts every project document as 업무 문서 until the reference group has loaded", () => {
    expect(projectDocCounts([member("a"), member("b")], undefined)).toEqual({ work: 2, reference: 0 });
    expect(projectDocCounts([], [])).toEqual({ work: 0, reference: 0 });
  });
});
