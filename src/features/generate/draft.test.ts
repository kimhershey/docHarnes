// Draft edit helpers of the 문서 생성 screen: field / cell edits, applying validator
// suggestions, body block text round-trips, source lists and the form picker mapping.
import { describe, expect, it } from "vitest";

import type { Draft, DraftIssue, SchemaRecord, TemplateInfo } from "@/lib/types";

import { addRow, applySuggestion, blockText, blockWithText, documentSourceIds, formRefToPicker, hasBlockingIssues, parseIssuePath, pickerToFormRef, regenerationRequest, removeRow, setFieldValue, toggleExcluded, withDocumentSources } from "./draft";

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
    model: "qwen",
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
