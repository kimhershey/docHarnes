// Where a question looks: the previous evidence is kept only for the same scope (KNO-1), and a plan that is a
// single `answer` asks within the documents its scope resolves to (KNO-5).
import { describe, expect, it } from "vitest";

import type { Plan, Scope } from "@/lib/types";

import { planAnswerScope, planScopeIsEmpty, sameSearchScope } from "./answerScope";

describe("sameSearchScope", () => {
  it("treats absent lists, switches at their default and another order as the same place", () => {
    expect(sameSearchScope(undefined, {})).toBe(true);
    expect(sameSearchScope({ latest_only: true }, {})).toBe(true);
    expect(sameSearchScope({ documents: [] }, { folder: null, project_id: null })).toBe(true);
    expect(sameSearchScope({ documents: ["a", "b"] }, { documents: ["b", "a", "a"] })).toBe(true);
    expect(sameSearchScope({ project_id: "p1", latest_only: true }, { project_id: "p1" })).toBe(true);
  });

  it("tells a changed scope: other documents, another project, a folder, the edition switch", () => {
    // The audit's case: answered over everything, then a document is attached.
    expect(sameSearchScope({ latest_only: true }, { documents: ["y"], latest_only: false })).toBe(false);
    expect(sameSearchScope({ documents: ["x"] }, { documents: ["y"] })).toBe(false);
    expect(sameSearchScope({ documents: ["x"] }, { documents: ["x", "y"] })).toBe(false);
    expect(sameSearchScope({ project_id: "p1" }, { project_id: "p2" })).toBe(false);
    expect(sameSearchScope({ project_id: "p1" }, {})).toBe(false);
    expect(sameSearchScope({}, { folder: "inbox" })).toBe(false);
    expect(sameSearchScope({}, { latest_only: false })).toBe(false);
    expect(sameSearchScope({ collections: ["c1"] }, { collections: ["c2"] })).toBe(false);
    expect(sameSearchScope({}, { roles: ["knowledge"] })).toBe(false);
    expect(sameSearchScope({}, { active_collections_only: false })).toBe(false);
  });
});

const plan = (scope: Scope): Pick<Plan, "scope"> => ({ scope });
const preview = (...document_ids: string[]) => ({ document_ids });

describe("planAnswerScope", () => {
  it("leaves the selector's scope alone when the plan names none", () => {
    expect(planScopeIsEmpty({})).toBe(true);
    expect(planScopeIsEmpty({ folder: null, doc_ids: [], filter: { tags: [] } })).toBe(true);
    expect(planAnswerScope(plan({ folder: null, doc_ids: [], filter: {} }), preview("a", "b"))).toEqual({ kind: "selector" });
    // A response without a preview says nothing about what the scope resolves to.
    expect(planAnswerScope(plan({ filter: { doc_type: "estimate" } }), null)).toEqual({ kind: "selector" });
  });

  it("asks within the documents the preview resolved for a type, a date range, a vendor, tags, a folder or documents", () => {
    for (const scope of [
      { filter: { doc_type: "meeting_minutes" } },
      { filter: { date_from: "2026-10-01", date_to: "2026-10-31" } },
      { filter: { vendor: "한빛" } },
      { filter: { tags: ["긴급"] } },
      { filter: { keyword: "납품" } },
      { filter: { review_status: "pending" as const } },
      { filter: { min_confidence: 0 } },
      { folder: "inbox" },
      { doc_ids: ["a"] },
    ] satisfies Scope[]) {
      expect(planScopeIsEmpty(scope), JSON.stringify(scope)).toBe(false);
      expect(planAnswerScope(plan(scope), preview("a", "b")), JSON.stringify(scope)).toEqual({ kind: "documents", documents: ["a", "b"], projectId: null });
      // Nothing matches: "no evidence" for that scope — never an empty list, which would mean everything.
      expect(planAnswerScope(plan(scope), preview()), JSON.stringify(scope)).toEqual({ kind: "none" });
    }
  });

  it("carries the project of the scope, alone or with other conditions", () => {
    expect(planAnswerScope(plan({ filter: { project_id: "p1" } }), preview("a"))).toEqual({ kind: "documents", documents: ["a"], projectId: "p1" });
    expect(planAnswerScope(plan({ filter: { project_id: "p1", doc_type: "sow" } }), preview("a"))).toEqual({ kind: "documents", documents: ["a"], projectId: "p1" });
    // A project without work documents still has its reference material: the project scope alone applies.
    expect(planScopeIsEmpty({ filter: { project_id: "p1" } }, true)).toBe(true);
    expect(planAnswerScope(plan({ filter: { project_id: "p1" } }), preview())).toEqual({ kind: "documents", documents: [], projectId: "p1" });
    // A type inside the project that no document has: nothing to ask.
    expect(planAnswerScope(plan({ filter: { project_id: "p1", doc_type: "sow" } }), preview())).toEqual({ kind: "none" });
  });
});
