// The assignment sheet of "프로젝트에 넣기" (1.0.0 plan §12 / §12-B): rows from the documents' memberships,
// ticks, the kind per row, the suggestion fill with the second-choice threshold, and the request it sends.
import { describe, expect, it } from "vitest";

import type { ClassifyCandidate, ClassifyResult, ClassifySuggestion, DocumentProject, DocumentRole, DocumentView } from "@/lib/types";

import { fillFromSuggestions, sameForAllRequest, selectRange, setCells, setKind, sheetColumns, sheetRequest, sheetRows, sheetSummary, suggestedProjects, toggleCell } from "./assign";

function doc(id: string, role: DocumentRole = "work", projects: DocumentProject[] = []): DocumentView {
  return { id, role, projects } as unknown as DocumentView;
}
const inProject = (id: string, reference = false): DocumentProject => ({ id, name: id, reference });
const cand = (project_id: string, confidence: number): ClassifyCandidate => ({ project_id, project_name: project_id, score: confidence * 10, confidence, reasons: [] });
function suggestion(doc_id: string, project_id: string | null, alternatives: ClassifyCandidate[] = []): ClassifySuggestion {
  return { doc_id, title: doc_id, doc_type: null, project_id, project_name: project_id, confidence: project_id ? 0.8 : 0.2, reasons: [], source: project_id ? "rules" : "none", alternatives, already_in: [] };
}
const preview = (...suggestions: ClassifySuggestion[]): ClassifyResult => ({ suggestions, suggested: 0, undecided: 0, model_calls: 0, notes: [] });
const pairs = (list: { doc_id: string; project_id: string; reference?: boolean }[]) => list.map((x) => `${x.doc_id}>${x.project_id}${x.reference ? "(ref)" : ""}`).sort();

describe("assignment sheet", () => {
  it("starts from where the documents are: memberships ticked, the kind of the document, no hidden membership", () => {
    const rows = sheetRows([doc("d1", "work", [inProject("A")]), doc("d2", "knowledge", [inProject("B", true), inProject("Z")]), doc("d3")]);
    expect(rows.map((r) => [r.id, r.kind, r.ticked])).toEqual([
      ["d1", "work", ["A"]],
      ["d2", "reference", ["B", "Z"]],
      ["d3", "work", []],
    ]);
    expect(rows[1]!.before).toEqual({ B: "reference", Z: "work" });
    // The picked projects come first; a project a row is in gets its column even when it was not picked.
    expect(sheetColumns(rows, ["C", "A"])).toEqual(["C", "A", "B", "Z"]);
    // Nothing was changed, so nothing is asked for.
    expect(sheetRequest(rows)).toEqual({ remove: [], add: [] });
    expect(sheetSummary(rows)).toMatchObject({ add: 0, remove: 0, move: 0, kindChanged: 0, unassigned: 1, projects: [] });
  });

  it("handles the reported case: all in AAA, some in BBB as well, some in CCC as well", () => {
    const ids = ["d1", "d2", "d3", "d4", "d5", "d6"];
    let rows = sheetRows(ids.map((id) => doc(id)));
    rows = setCells(rows, ids, "AAA", true); // the AAA column header
    rows = setCells(rows, ["d3", "d4"], "BBB", true); // picked rows → "추가"
    rows = setCells(rows, ["d5", "d6"], "CCC", true);
    const req = sheetRequest(rows);
    expect(req.remove).toEqual([]);
    expect(pairs(req.add)).toEqual(["d1>AAA", "d2>AAA", "d3>AAA", "d3>BBB", "d4>AAA", "d4>BBB", "d5>AAA", "d5>CCC", "d6>AAA", "d6>CCC"]);
    const summary = sheetSummary(rows);
    expect(summary.projects).toEqual([
      { id: "AAA", add: 6, remove: 0, move: 0 },
      { id: "BBB", add: 2, remove: 0, move: 0 },
      { id: "CCC", add: 2, remove: 0, move: 0 },
    ]);
    expect([summary.add, summary.unassigned]).toEqual([10, 0]);
    // The same set for every document is the first mode's request.
    expect(pairs(sameForAllRequest(["d1", "d2"], ["AAA", "BBB"], "reference").add)).toEqual(["d1>AAA(ref)", "d1>BBB(ref)", "d2>AAA(ref)", "d2>BBB(ref)"]);
  });

  it("unticking a membership takes the document out; a tick put back changes nothing", () => {
    let rows = sheetRows([doc("d1", "work", [inProject("A"), inProject("B")])]);
    rows = toggleCell(rows, "d1", "B");
    expect(sheetRequest(rows)).toEqual({ remove: [{ doc_id: "d1", project_id: "B" }], add: [] });
    expect(sheetSummary(rows)).toMatchObject({ remove: 1, removals: [{ doc_id: "d1", project_id: "B" }] });
    rows = toggleCell(rows, "d1", "B");
    expect(sheetRequest(rows)).toEqual({ remove: [], add: [] });
    // A whole column off: every visible row leaves that project.
    rows = setCells(rows, ["d1"], "A", false);
    expect(pairs(sheetRequest(rows).remove)).toEqual(["d1>A"]);
  });

  it("the kind is per document: a changed row moves the memberships it keeps, an unchanged row leaves them", () => {
    let rows = sheetRows([doc("d1", "work", [inProject("A"), inProject("B")]), doc("d2", "work", [inProject("A")]), doc("d3")]);
    rows = setKind(rows, ["d1", "d3"], "reference");
    rows = toggleCell(rows, "d1", "B"); // d1 leaves B, stays in A as reference material
    rows = toggleCell(rows, "d1", "C");
    rows = toggleCell(rows, "d2", "C"); // d2 keeps its kind: A is not touched
    const req = sheetRequest(rows);
    expect(pairs(req.add)).toEqual(["d1>A(ref)", "d1>C(ref)", "d2>C"]);
    expect(pairs(req.remove)).toEqual(["d1>B"]);
    const summary = sheetSummary(rows);
    expect(summary).toMatchObject({ add: 2, move: 1, remove: 1, kindChanged: 1, unassigned: 1 });
    expect(summary.projects.find((p) => p.id === "A")).toEqual({ id: "A", add: 0, remove: 0, move: 1 });
    // d3 changed its kind but goes nowhere: nothing to apply, and it does not count as a kind change.
    expect(req.add.some((a) => a.doc_id === "d3")).toBe(false);
  });

  it("a suggestion ticks the first choice and the second one from the threshold on", () => {
    const s = (second: number) => suggestion("d1", "A", [cand("B", second), cand("C", 0.9)]);
    expect(suggestedProjects(s(0.59), 0.6)).toEqual(["A"]);
    expect(suggestedProjects(s(0.6), 0.6)).toEqual(["A", "B"]);
    expect(suggestedProjects(s(0.6000000238418579), 0.6)).toEqual(["A", "B"]);
    expect(suggestedProjects(s(0.5999999), 0.6)).toEqual(["A", "B"]);
    // The setting moves the line: at 70 % a 65 % second choice is not ticked; at 100 % none is.
    expect(suggestedProjects(s(0.65), 0.7)).toEqual(["A"]);
    expect(suggestedProjects(s(0.98), 1)).toEqual(["A"]);
    // Never the third, however strong.
    expect(suggestedProjects(s(0.95), 0.6)).toEqual(["A", "B"]);
    // Undecided between two strong projects: both; a strong one next to a weak one: none.
    expect(suggestedProjects(suggestion("d1", null, [cand("A", 0.67), cand("B", 0.6)]), 0.6)).toEqual(["A", "B"]);
    expect(suggestedProjects(suggestion("d1", null, [cand("A", 0.67), cand("B", 0.5)]), 0.6)).toEqual([]);
    expect(suggestedProjects(suggestion("d1", null), 0.6)).toEqual([]);
    // A candidate without a confidence (an older backend) is never a second choice.
    expect(suggestedProjects(suggestion("d1", "A", [{ project_id: "B", project_name: "B", score: 9, reasons: [] }]), 0.6)).toEqual(["A"]);
  });

  it("filling from suggestions only adds ticks, marks them, and keeps the rest as candidates", () => {
    let rows = sheetRows([doc("d1", "work", [inProject("Z")]), doc("d2"), doc("d3")]);
    const filled = fillFromSuggestions(rows, preview(suggestion("d1", "A", [cand("B", 0.7), cand("C", 0.4)]), suggestion("d2", "Z", [cand("A", 0.3)]), suggestion("d3", null, [cand("C", 0.3)])), 0.6);
    rows = filled.rows;
    expect(filled.projects).toEqual(["A", "B", "Z"]);
    expect(rows.map((r) => [r.ticked, r.suggested, r.candidates])).toEqual([
      [["Z", "A", "B"], ["A", "B"], { C: 0.4 }],
      [["Z"], ["Z"], { A: 0.3 }],
      [[], [], { C: 0.3 }],
    ]);
    // The membership d1 had stays; the request holds the new pairs only.
    expect(pairs(sheetRequest(rows).add)).toEqual(["d1>A", "d1>B", "d2>Z"]);
    expect(sheetRequest(rows).remove).toEqual([]);
    // The user unticks a suggested cell: it is theirs now. Filling again does not duplicate anything.
    rows = toggleCell(rows, "d1", "B");
    expect(rows[0]!.suggested).toEqual(["A"]);
    const again = fillFromSuggestions(rows, preview(suggestion("d1", "A", [cand("B", 0.7)])), 0.6);
    expect(again.rows[0]!.ticked).toEqual(["Z", "A", "B"]);
    expect(again.rows[0]!.suggested).toEqual(["A", "B"]);
  });

  it("shift-click selects the rows in between", () => {
    const visible = ["a", "b", "c", "d", "e"];
    expect([...selectRange(visible, new Set(["a"]), "b", "d", true)].sort()).toEqual(["a", "b", "c", "d"]);
    expect([...selectRange(visible, new Set(visible), "d", "b", false)].sort()).toEqual(["a", "e"]);
    // No anchor (or one that is filtered away): only the clicked row.
    expect([...selectRange(visible, new Set(), null, "c", true)]).toEqual(["c"]);
    expect([...selectRange(visible, new Set(), "zzz", "c", true)]).toEqual(["c"]);
  });
});
