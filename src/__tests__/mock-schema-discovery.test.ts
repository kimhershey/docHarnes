// The browser mock of 0.3.0 §2: several sample documents → merged schema draft (union, majority
// threshold, required = on every sample, inferred rules) and the duplicate-name guard.
import { beforeEach, describe, expect, it } from "vitest";

const store = new Map<string, string>();
(globalThis as unknown as { localStorage: Storage }).localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
  removeItem: (k: string) => void store.delete(k),
  clear: () => store.clear(),
  key: () => null,
  length: 0,
} as Storage;

const backend = await import("@/lib/mock-backend");
const pl = await import("@/lib/mock-pipeline");
const { newFieldDef } = await import("@/lib/types");
type FieldDef = import("@/lib/types").FieldDef;
type FieldKind = import("@/lib/types").FieldKind;
type SchemaDraft = import("@/lib/types").SchemaDraft;

function field(key: string, label: string, kind: FieldKind = "string", extra: Partial<FieldDef> = {}): FieldDef {
  return { ...newFieldDef(key, kind), label_ko: label, label_en: key, ...extra };
}

function draft(name: string, fields: FieldDef[], doc: string | null = null): SchemaDraft {
  return { id: null, name, name_en: null, doc_type: pl.slugForName(name), source: "form", description_ko: null, description_en: null, definition: { fields, rules: [] }, form_document_id: doc, note: null };
}

describe("schema discovery (mock)", () => {
  beforeEach(async () => {
    backend.resetMock();
    await backend.mockInvoke("workspace_create", { args: { path: "C:\\Work\\Docs", name: null, locale: null, force_unlock: false } });
  });

  it("merges two samples into a union, marks the common fields required and names the form after the hint", () => {
    const blank = draft("견적서 양식", [field("recipient", "수신"), field("date", "견적일자", "date"), field("total", "합계금액", "amount", { required: true }), field("note", "비고")], "d1");
    const filled = draft("견적서", [field("project_name", "사업명"), field("recipient", "수신"), field("date", "견적일자", "date"), field("total", "합계금액", "amount")], "d2");
    const { draft: merged, adopted, dropped } = pl.mergeDrafts([blank, filled], "견적서", 1);
    expect(merged.name).toBe("견적서");
    expect(merged.doc_type).toBe("estimate");
    expect(merged.definition.fields.map((f) => f.key)).toEqual(["recipient", "date", "total", "note", "project_name"]);
    expect(merged.definition.fields.filter((f) => f.required).map((f) => f.key)).toEqual(["recipient", "date", "total"]);
    expect([adopted, dropped]).toEqual([5, 0]);
    expect(merged.form_document_id).toBe("d1");
    expect(merged.sample_document_ids).toEqual(["d2"]);
    // Three samples: a field seen once falls below the ⌈n/2⌉ threshold.
    const third = draft("견적서", [field("recipient", "수신")], "d3");
    const three = pl.mergeDrafts([blank, filled, third], null, 2);
    expect(three.draft.definition.fields.map((f) => f.key)).toEqual(["recipient", "date", "total"]);
    expect(three.draft.definition.fields.filter((f) => f.required).map((f) => f.key)).toEqual(["recipient"]);
    expect(three.dropped).toBe(2);
  });

  it("merges table columns by union and lets a typed kind win a tie against string", () => {
    const a = draft("견적서", [field("items", "견적 내역", "table", { fields: [field("item_name", "품명"), field("quantity", "수량", "integer")] }), field("valid_until", "유효기간", "string")]);
    const b = draft("견적서", [field("items", "견적 내역", "table", { fields: [field("item_name", "품명"), field("unit_price", "단가", "amount")] }), field("valid_until", "유효기간", "date")]);
    const { draft: merged } = pl.mergeDrafts([a, b], null, 1);
    const items = merged.definition.fields.find((f) => f.key === "items")!;
    expect(items.fields.map((c) => c.key)).toEqual(["item_name", "quantity", "unit_price"]);
    expect(merged.definition.fields.find((f) => f.key === "valid_until")?.kind).toBe("date");
  });

  it("attaches only the deterministic rules the merged keys support", () => {
    const d = draft("견적서", [
      field("supply_total", "공급가액 합계", "amount"),
      field("tax_total", "세액 합계", "amount"),
      field("grand_total", "합계금액", "amount"),
      field("items", "견적 내역", "table", { fields: [field("supply_amount", "공급가액", "amount")] }),
      field("period_start", "시작", "date"),
      field("period_end", "종료", "date"),
    ]);
    expect(pl.inferRules(d.definition)).toEqual(["supply_plus_tax_equals_total", "tax_is_ten_percent", "items_sum_equals_supply", "period_start_before_end"]);
    expect(pl.inferRules({ fields: [field("name", "성명")], rules: [] })).toEqual([]);
  });

  it("refuses a duplicate schema name (the built-in 견적서) and proposes a free one", async () => {
    expect(await backend.mockInvoke("pipeline_schema_name_exists", { name: "견적서", excludingId: null })).toBe(true);
    expect(await backend.mockInvoke("pipeline_schema_name_exists", { name: " 견 적 서 ", excludingId: null })).toBe(true);
    expect(await backend.mockInvoke("pipeline_schema_name_exists", { name: "견적서 (2)", excludingId: null })).toBe(false);
    expect(pl.freeName("견적서")).toBe("견적서 (2)");
    let failed = "";
    try {
      await backend.mockInvoke("pipeline_schema_save", { draft: { ...draft("견적서", [field("total", "합계", "amount")]), source: "user" } });
    } catch (e) {
      failed = String((e as { message?: unknown }).message ?? e);
    }
    expect(failed).toContain("schema_name_duplicate");
    const saved = await backend.mockInvoke<{ id: string; name: string }>("pipeline_schema_save", { draft: { ...draft("견적서 (2)", [field("total", "합계", "amount")]), source: "user" } });
    expect(saved.name).toBe("견적서 (2)");
    // Editing the saved schema under its own name is not a duplicate.
    expect(await backend.mockInvoke("pipeline_schema_name_exists", { name: "견적서 (2)", excludingId: saved.id })).toBe(false);
  });
});
