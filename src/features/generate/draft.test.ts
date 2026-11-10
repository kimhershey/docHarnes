// Draft edit helpers of the 문서 생성 screen: field / cell edits, applying validator
// suggestions, body block text round-trips, source lists and the form picker mapping.
import { describe, expect, it } from "vitest";

import type { Draft, DraftIssue, SchemaRecord, TemplateInfo } from "@/lib/types";

import { addRow, applySuggestion, blockText, blockWithText, documentSourceIds, formRefToPicker, hasBlockingIssues, insertBlock, moveBlock, newBlock, parseIssuePath, pickerToFormRef, pickerToReviseScope, regenerationRequest, removeBlock, removeRow, reviseRequest, reviseScopeLabel, reviseScopeToPicker, sectionAt, sectionsOf, setBlockLevel, setFieldValue, toggleExcluded, withDocumentSources } from "./draft";

function draft(): Draft {
  return {
    id: "d1",
    title: "견적서",
    template: { kind: "builtin", template: "quotation" },
    doc_type: "quotation",
    fields: [
      { key: "recipient", label: "수신", kind: "string", required: true, value: "㈜한빛", normalized: null, source: null, user_edited: false },
      { key: "total", label: "합계금액", kind: "amount", required: true, value: "13,200,000", normalized: null, source: null, user_edited: false },
    ],
    tables: [{ key: "items", label: "견적 품목", columns: [{ key: "item_name", label: "품명", kind: "string" }, { key: "quantity", label: "수량", kind: "integer" }, { key: "unit_price", label: "단가", kind: "amount" }, { key: "amount", label: "금액", kind: "amount" }], rows: [["노트북", "10", "1,200,000", "12,500,000"]] }],
    body: [
      { kind: "heading", level: 2, text: "견적 조건" },
      { kind: "paragraph", text: "부가세 별도." },
      { kind: "list", items: ["a", "b"], ordered: false },
      { kind: "table_ref", key: "items" },
    ],
    references: [],
    issues: [
      { path: "fields.total", code: "sum_mismatch", message: "합계 불일치", blocking: true, suggestion: "12,000,000" },
      { path: "tables.items[0].amount", code: "sum_mismatch", message: "행 불일치", blocking: true, suggestion: "12,000,000" },
    ],
    language: "ko",
    tone: "formal",
    length: "medium",
    created_at: "2026-11-08T00:00:00Z",
    updated_at: "2026-11-08T00:00:00Z",
    model: "gemma4",
    request: null,
    notes: [],
    saved_path: null,
    saved_doc_id: null,
  };
}

describe("draft edits", () => {
  it("marks edited fields and keeps the others", () => {
    const d = setFieldValue(draft(), "recipient", "㈜두리");
    expect(d.fields[0]).toMatchObject({ value: "㈜두리", user_edited: true, normalized: null });
    expect(d.fields[1]!.user_edited).toBe(false);
  });

  it("applies a field suggestion and drops that issue only", () => {
    const d = draft();
    const next = applySuggestion(d, d.issues[0]!);
    expect(next.fields[1]!.value).toBe("12,000,000");
    expect(next.fields[1]!.user_edited).toBe(true);
    expect(next.issues).toHaveLength(1);
    expect(next.issues[0]!.path).toBe("tables.items[0].amount");
    expect(hasBlockingIssues(next)).toBe(true);
  });

  it("applies a table cell suggestion by column key", () => {
    const d = draft();
    const next = applySuggestion(d, d.issues[1]!);
    expect(next.tables[0]!.rows[0]![3]).toBe("12,000,000");
    expect(next.issues.map((i) => i.path)).toEqual(["fields.total"]);
    const noop: DraftIssue = { path: "body[1]", code: "uncited_claim", message: "", blocking: false };
    expect(applySuggestion(d, noop)).toBe(d);
  });

  it("parses issue paths", () => {
    expect(parseIssuePath("fields.total")).toEqual({ kind: "field", key: "total" });
    expect(parseIssuePath("tables.items[2].amount")).toEqual({ kind: "cell", table: "items", row: 2, column: "amount" });
    expect(parseIssuePath("body[3]")).toEqual({ kind: "body", index: 3 });
    expect(parseIssuePath("x")).toEqual({ kind: "other" });
  });

  it("adds and removes table rows", () => {
    const d = addRow(draft(), "items");
    expect(d.tables[0]!.rows).toHaveLength(2);
    expect(d.tables[0]!.rows[1]).toEqual(["", "", "", ""]);
    expect(removeRow(d, "items", 0).tables[0]!.rows).toHaveLength(1);
  });

  it("round-trips block text (lists one item per line)", () => {
    const list = draft().body[2]!;
    expect(blockText(list)).toBe("a\nb");
    expect(blockWithText(list, "x\n\ny ")).toEqual({ kind: "list", items: ["x", "y"], ordered: false });
    expect(blockWithText(draft().body[1]!, "새 문단")).toEqual({ kind: "paragraph", text: "새 문단" });
    expect(blockText(draft().body[3]!)).toBe("");
  });

  it("builds the regeneration request with previous, feedback and exclusions", () => {
    const d = draft();
    const req = regenerationRequest({ form_ref: "quotation", format_description: null, content: "c", sources: [], output: "hwpx", tone: "formal", length: "medium", language: "ko", use_knowledge: true, feedback: null, previous: null, excluded_sources: [], max_tokens: null }, d, "  더 짧게 ", ["doc1"]);
    expect(req.previous).toBe(d);
    expect(req.feedback).toBe("더 짧게");
    expect(req.excluded_sources).toEqual(["doc1"]);
    expect(toggleExcluded(["a"], "a")).toEqual([]);
    expect(toggleExcluded(["a"], "b")).toEqual(["a", "b"]);
  });

  // -- 0.7.0 §1.2: body blocks ---------------------------------------------------------------------
  it("inserts, removes and moves body blocks and re-indexes the positional issues", () => {
    const d = { ...draft(), issues: [{ path: "body[1]", code: "uncited_claim", message: "a", blocking: false }, { path: "body[3]", code: "uncited_claim", message: "b", blocking: false }, { path: "fields.total", code: "sum_mismatch", message: "c", blocking: true }] };
    // Insert a paragraph after the heading: the issues of the later blocks move one down.
    const inserted = insertBlock(d, 1, newBlock("paragraph"));
    expect(inserted.body).toHaveLength(5);
    expect(inserted.body[1]).toEqual({ kind: "paragraph", text: "", source: null });
    expect(inserted.issues.map((i) => i.path)).toEqual(["body[2]", "body[4]", "fields.total"]);
    // Append (at = length) and clamp.
    expect(insertBlock(d, 99, newBlock("list")).body[4]).toEqual({ kind: "list", items: [], ordered: false });
    expect(insertBlock(d, -5, newBlock("heading", 3)).body[0]).toEqual({ kind: "heading", level: 3, text: "" });
    // Remove the paragraph with an issue: its issue is dropped, the later one moves up.
    const removed = removeBlock(d, 1);
    expect(removed.body.map((b) => b.kind)).toEqual(["heading", "list", "table_ref"]);
    expect(removed.issues.map((i) => i.path)).toEqual(["body[2]", "fields.total"]);
    expect(removeBlock(d, 9)).toBe(d);
    // Move the list up above the paragraph: issues follow their blocks.
    const moved = moveBlock(d, 2, 1);
    expect(moved.body.map((b) => b.kind)).toEqual(["heading", "list", "paragraph", "table_ref"]);
    expect(moved.issues.map((i) => i.path)).toEqual(["body[2]", "body[3]", "fields.total"]);
    const movedDown = moveBlock(d, 1, 3);
    expect(movedDown.body.map((b) => b.kind)).toEqual(["heading", "list", "table_ref", "paragraph"]);
    expect(movedDown.issues.map((i) => i.path)).toEqual(["body[3]", "body[2]", "fields.total"]);
    expect(moveBlock(d, 1, 1)).toBe(d);
    // Heading levels are clamped to 1..4; other blocks are left alone.
    expect(setBlockLevel(d, 0, 3).body[0]).toEqual({ kind: "heading", level: 3, text: "견적 조건" });
    expect(setBlockLevel(d, 0, 9).body[0]).toMatchObject({ level: 4 });
    expect(setBlockLevel(d, 1, 3)).toBe(d);
  });

  // -- 0.7.0 §1.5: sections and the revision request ---------------------------------------------------
  it("splits the body into sections at level ≤ 2 headings", () => {
    const body = draft().body;
    expect(sectionsOf([])).toEqual([]);
    expect(sectionsOf(body)).toEqual([{ ordinal: 1, start: 0, end: 4, heading: "견적 조건" }]);
    const two = [...body, { kind: "heading", level: 2, text: "결론" } as const, { kind: "paragraph", text: "끝." } as const];
    expect(sectionsOf(two).map((s) => [s.ordinal, s.start, s.end, s.heading])).toEqual([
      [1, 0, 4, "견적 조건"],
      [2, 4, 6, "결론"],
    ]);
    // A leading run without a section heading is section 1 (untitled); level-3 headings do not start a section.
    const lead = [{ kind: "paragraph", text: "머리말" } as const, { kind: "heading", level: 3, text: "소제목" } as const, ...two];
    expect(sectionsOf(lead).map((s) => [s.ordinal, s.start, s.heading])).toEqual([
      [1, 0, null],
      [2, 2, "견적 조건"],
      [3, 6, "결론"],
    ]);
    // No headings at all: the whole body is one section.
    expect(sectionsOf([{ kind: "paragraph", text: "a" }, { kind: "paragraph", text: "b" }])).toEqual([{ ordinal: 1, start: 0, end: 2, heading: null }]);
    expect(sectionAt(two, 5)?.ordinal).toBe(2);
    expect(sectionAt([], 0)).toBeNull();
    expect(reviseScopeLabel({ scope: "section", section: 2 }, two, true)).toBe("2. 결론");
    expect(reviseScopeLabel({ scope: "insert_after", section: null }, two, true)).toBe("끝에 새 절");
    expect(reviseScopeLabel({ scope: "insert_after", section: 1 }, two, false)).toBe("new section after 1. 견적 조건");
    expect(reviseScopeLabel({ scope: "whole" }, two, true)).toBe("전체");
  });

  it("builds the revision request and maps the scope picker", () => {
    const d = draft();
    const req = reviseRequest({ form_ref: "quotation", format_description: null, content: "c", sources: [], output: "hwpx", tone: "formal", length: "medium", language: "ko", use_knowledge: true, feedback: null, previous: null, excluded_sources: [], max_tokens: null }, d, " 더 자세히 ", { scope: "section", section: 1 }, ["doc1"]);
    expect(req.previous).toBe(d);
    expect(req.feedback).toBe("더 자세히");
    expect(req.revise).toEqual({ scope: "section", section: 1 });
    expect(req.excluded_sources).toEqual(["doc1"]);
    expect(reviseScopeToPicker({ scope: "whole" })).toBe("whole");
    expect(reviseScopeToPicker({ scope: "section", section: 3 })).toBe("section:3");
    expect(reviseScopeToPicker({ scope: "insert_after", section: null })).toBe("insert_after:end");
    expect(reviseScopeToPicker({ scope: "insert_after", section: 2 })).toBe("insert_after:2");
    expect(pickerToReviseScope("section:3")).toEqual({ scope: "section", section: 3 });
    expect(pickerToReviseScope("insert_after:end", " 결론 ")).toEqual({ scope: "insert_after", section: null, heading: "결론" });
    expect(pickerToReviseScope("insert_after:2", "")).toEqual({ scope: "insert_after", section: 2, heading: null });
    expect(pickerToReviseScope("whole")).toEqual({ scope: "whole" });
    expect(pickerToReviseScope("section:x")).toEqual({ scope: "section", section: 1 });
  });

  it("maps document sources and the template picker", () => {
    const sources = withDocumentSources([{ kind: "text", title: null, text: "t" }], ["d1", "d2"]);
    expect(documentSourceIds(sources)).toEqual(["d1", "d2"]);
    expect(sources[2]).toEqual({ kind: "text", title: null, text: "t" });
    const templates = [{ kind: "quotation" } as TemplateInfo];
    const schemas = [{ id: "s1", name: "견적서 양식", doc_type: "quotation_form" } as SchemaRecord];
    expect(formRefToPicker(null, templates, schemas)).toBe("auto");
    expect(formRefToPicker("quotation", templates, schemas)).toBe("tpl:quotation");
    expect(formRefToPicker("s1", templates, schemas)).toBe("form:s1");
    expect(formRefToPicker("견적서 양식", templates, schemas)).toBe("form:s1");
    expect(pickerToFormRef("tpl:report")).toBe("report");
    expect(pickerToFormRef("form:s1")).toBe("s1");
    expect(pickerToFormRef("auto")).toBeNull();
  });
});
