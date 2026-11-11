import { describe, expect, it } from "vitest";

import type { CheckRun, Evidence, RuleResult, Verdict } from "@/lib/types";

import { defaultSupplementSelection, missingDocFindingId, selectableSupplementIds, supplementRows } from "./supplement";

function evidence(docType: string | null): Evidence {
  return { doc_id: null, doc_name: null, doc_type: docType, field: "project_name", label: null, value: null, normalized: null, page: null, status: null };
}

function result(id: string, verdict: Verdict, leftType: string | null = "srs"): RuleResult {
  return { id, run_id: "run", rule_id: `rule-${id}`, description: `규칙 ${id}`, verdict, required: true, left: evidence(leftType), right: null, message: "", checked_at: "2026-11-11T00:00:00Z" };
}

function run(results: RuleResult[], missingDocTypes: string[] = []): CheckRun {
  return { id: "run", project_id: "p", started_at: "", finished_at: "", pass: 0, mismatch: 0, undetermined: 0, missing: 0, results, missing_doc_types: missingDocTypes };
}

describe("supplement request rows (PRJ-2)", () => {
  it("uses the core's id for a missing document", () => {
    expect(missingDocFindingId("wbs")).toBe("missing_doc:wbs");
  });

  it("lists the missing documents first, then every result that did not pass", () => {
    const rows = supplementRows(run([result("a", "pass"), result("b", "mismatch"), result("c", "undetermined"), result("d", "missing", "test_report")], ["wbs", "srs"]));
    expect(rows.map((r) => r.id)).toEqual(["missing_doc:wbs", "missing_doc:srs", "b", "c", "d"]);
    expect(rows.slice(0, 2).map((r) => [r.docType, r.verdict, r.selectable])).toEqual([
      ["wbs", "missing", true],
      ["srs", "missing", true],
    ]);
    expect(rows.find((r) => r.id === "b")?.result?.description).toBe("규칙 b");
  });

  it("a 판단불가 result cannot be ticked — the request builder would answer no_findings", () => {
    const rows = supplementRows(run([result("b", "mismatch"), result("c", "undetermined")]));
    expect(rows.find((r) => r.id === "c")?.selectable).toBe(false);
    expect(selectableSupplementIds(rows)).toEqual(["b"]);
    expect(defaultSupplementSelection(supplementRows(run([result("c", "undetermined")])))).toEqual([]);
  });

  it("ticks the missing documents and the 불일치 / 누락 results by default", () => {
    const rows = supplementRows(run([result("b", "mismatch"), result("d", "missing", "test_report")], ["wbs"]));
    expect(defaultSupplementSelection(rows)).toEqual(["missing_doc:wbs", "b", "d"]);
  });

  it("leaves a 누락 result unticked when its document is one of the missing documents (same as the core's 'everything')", () => {
    const rows = supplementRows(run([result("b", "mismatch", "wbs"), result("d", "missing", "wbs"), result("e", "missing", "srs")], ["wbs"]));
    expect(rows.find((r) => r.id === "d")?.covered).toBe(true);
    expect(rows.find((r) => r.id === "b")?.covered).toBe(false);
    expect(defaultSupplementSelection(rows)).toEqual(["missing_doc:wbs", "b", "e"]);
    // Still a row the user can add.
    expect(selectableSupplementIds(rows)).toEqual(["missing_doc:wbs", "b", "d", "e"]);
  });

  it("has no rows without a run or when everything passed", () => {
    expect(supplementRows(null)).toEqual([]);
    expect(supplementRows(run([result("a", "pass")]))).toEqual([]);
  });
});
