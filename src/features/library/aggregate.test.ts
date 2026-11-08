import { describe, expect, it } from "vitest";

import ko from "@/i18n/ko.json";
import type { AggregateResult, SchemaRecord } from "@/lib/types";

import { amountFields, formatAggregate, formatAmount, formatGroup } from "./aggregate";

// `t` stub over ko.json with `{{var}}` interpolation, like the e2e helpers do.
function lookup(key: string): string {
  let node: unknown = ko;
  for (const part of key.split(".")) node = (node as Record<string, unknown> | undefined)?.[part];
  if (typeof node !== "string") throw new Error(`missing key ${key}`);
  return node;
}
const t = (key: string, vars: Record<string, unknown> = {}) => lookup(key).replace(/\{\{(\w+)\}\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m));

function result(patch: Partial<AggregateResult>): AggregateResult {
  return {
    func: "sum",
    field: "total",
    field_label: "합계",
    doc_type: "receipt",
    basis: "doc_date",
    date_from: null,
    date_to: null,
    include_unreviewed: false,
    value: 1_180_000,
    count: 33,
    reviewed: { value: 1_180_000, count: 33 },
    unreviewed: { value: 54_500, count: 4 },
    missing: 0,
    groups: [],
    documents: [],
    ...patch,
  };
}

describe("formatAmount", () => {
  it("groups thousands and caps decimals", () => {
    expect(formatAmount(1234500)).toBe("1,234,500");
    expect(formatAmount(12500.456)).toBe("12,500.46");
    expect(formatAmount(-999)).toBe("-999");
    expect(formatAmount(3, 0)).toBe("3");
    expect(formatAmount(Number.NaN)).toBe("–");
  });
});

describe("formatAggregate", () => {
  it("renders the design's sum line when pending values are included", () => {
    expect(formatAggregate(result({ include_unreviewed: true, value: 1_234_500, count: 37 }), t)).toBe("합계 1,234,500 = 검토 완료 33건 1,180,000 + 대기 4건 54,500");
  });

  it("marks the pending part as excluded by default and appends missing documents", () => {
    expect(formatAggregate(result({ missing: 2 }), t)).toBe("합계 1,180,000 = 검토 완료 33건 1,180,000 · 대기 4건 54,500 제외 · 값 없음 2건");
  });

  it("counts documents and lists only counts for averages", () => {
    expect(formatAggregate(result({ func: "count", value: 33, include_unreviewed: false }), t)).toBe("건수 33건 = 검토 완료 33건 · 대기 4건 제외");
    expect(formatAggregate(result({ func: "count", value: 37, include_unreviewed: true }), t)).toBe("건수 37건 = 검토 완료 33건 + 대기 4건");
    expect(formatAggregate(result({ func: "avg", value: 35757.58 }), t)).toBe("평균 35,757.58 · 검토 완료 33건 · 대기 4건 제외");
  });

  it("formats month groups", () => {
    expect(formatGroup({ key: "2026-10", value: 12500, count: 1, reviewed: { value: 12500, count: 1 }, unreviewed: { value: 0, count: 0 } }, "sum", t)).toBe("2026-10: 12,500 (1건)");
    expect(formatGroup({ key: "2026-10", value: 3, count: 3, reviewed: { value: 3, count: 3 }, unreviewed: { value: 0, count: 0 } }, "count", t)).toBe("2026-10: 3 (3건)");
  });
});

describe("amountFields", () => {
  const field = (key: string, kind: string, label_ko = key) => ({ key, label_ko, label_en: key, kind, required: false, regex_hint: null, example: null, description: null, options: [], fields: [], mask: false });
  const schema = (id: string, doc_type: string, is_default: boolean, enabled: boolean, fields: ReturnType<typeof field>[]) =>
    ({ id, name: id, name_en: null, doc_type, source: "builtin", version: 1, description_ko: null, description_en: null, definition: { fields, rules: [] }, form_document_id: null, builtin_key: null, enabled, is_default, use_count: 0, note: null }) as unknown as SchemaRecord;

  it("picks the enabled default schema per type and keeps only top-level amount fields", () => {
    const schemas = [
      schema("old", "receipt", false, true, [field("subtotal", "amount", "소계")]),
      schema("cur", "receipt", true, true, [field("store_name", "string"), field("total", "amount", "합계"), field("items", "table")]),
      schema("tax", "tax_invoice", true, true, [field("grand_total", "amount", "합계금액"), field("total", "amount", "합계")]),
      schema("off", "contract", true, false, [field("amount", "amount")]),
    ];
    expect(amountFields(schemas, ["receipt"])).toEqual([{ key: "total", label_ko: "합계", label_en: "total" }]);
    expect(amountFields(schemas, ["receipt", "tax_invoice"]).map((f) => f.key)).toEqual(["total", "grand_total"]);
    expect(amountFields(schemas, ["contract", "unknown"])).toEqual([]);
    expect(amountFields(undefined, ["receipt"])).toEqual([]);
  });
});
