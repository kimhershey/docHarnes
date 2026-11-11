// Rule grid helpers against the real Korean strings: field options of nested
// schemas, completeness, unresolved operands and the one-line summary.
import i18next from "i18next";
import { describe, expect, it } from "vitest";

import en from "@/i18n/en.json";
import ko from "@/i18n/ko.json";
import type { DocTypeInfo, FieldDef, Rule, SchemaRecord } from "@/lib/types";

import { evidenceStatusLabel, fieldOptions, newRule, operandLabel, parseTolerance, ruleComplete, ruleSummary, schemaForType, unresolvedOperands } from "./rules";

const instance = i18next.createInstance();
await instance.init({ lng: "ko", fallbackLng: false, resources: { ko: { translation: ko }, en: { translation: en } }, interpolation: { escapeValue: false } });
const t = instance.t.bind(instance);

const f = (key: string, label_ko: string, kind: FieldDef["kind"] = "string", fields: FieldDef[] = []): FieldDef => ({ key, label_ko, label_en: key, kind, required: false, regex_hint: null, example: null, description: null, options: [], fields, mask: false });

const schemas: SchemaRecord[] = [
  { id: "ti", name: "세금계산서", name_en: "Tax invoice", doc_type: "tax_invoice", source: "builtin", version: 1, description_ko: null, description_en: null, definition: { fields: [f("supplier", "공급자", "group", [f("brn", "공급자 사업자등록번호", "brn"), f("name", "공급자 상호")]), f("grand_total", "합계금액", "amount"), f("items", "품목", "table", [f("item", "품목")])], rules: [] }, form_document_id: null, builtin_key: "tax_invoice", enabled: true, is_default: true, use_count: 0, created_at: "", updated_at: "" },
  { id: "br", name: "사업자등록증", name_en: "Business registration", doc_type: "business_registration", source: "builtin", version: 1, description_ko: null, description_en: null, definition: { fields: [f("registration_no", "등록번호", "brn"), f("company_name", "상호")], rules: [] }, form_document_id: null, builtin_key: "business_registration", enabled: true, is_default: true, use_count: 0, created_at: "", updated_at: "" },
];
const types: DocTypeInfo[] = [
  { slug: "tax_invoice", label_ko: "세금계산서", label_en: "Tax invoice", schema_id: "ti", builtin: true, use_count: 0 },
  { slug: "business_registration", label_ko: "사업자등록증", label_en: "Business registration", schema_id: "br", builtin: true, use_count: 0 },
];

const rule: Rule = { id: "r1", description: "사업자번호 일치", left: { doc_type: "tax_invoice", field: "supplier.brn", value: null }, op: "eq", right: { doc_type: "business_registration", field: "registration_no", value: null }, tolerance: null, required: true, enabled: true };

describe("rule helpers", () => {
  it("lists scalar field paths, group children and table columns with their prefixes", () => {
    const opts = fieldOptions(schemaForType(schemas, "tax_invoice"));
    expect(opts.map((o) => o.path)).toEqual(["supplier.brn", "supplier.name", "grand_total", "items.item"]);
    expect(opts[0]!.label_ko).toBe("공급자 공급자 사업자등록번호");
    // A table column is an operand like the core's `fields_of_def` lists it (PRJ-15).
    expect(opts[3]).toMatchObject({ path: "items.item", label_ko: "품목 품목", label_en: "items item", column: true });
    expect(opts.slice(0, 3).every((o) => !o.column)).toBe(true);
    expect(fieldOptions(null)).toEqual([]);
  });

  it("a rule on a table column is resolved, labelled and complete (PRJ-15)", () => {
    const onColumn: Rule = { ...rule, left: { doc_type: "tax_invoice", field: "items.item", value: null }, op: "present", right: null };
    expect(unresolvedOperands(onColumn, schemas, types)).toEqual([]);
    expect(operandLabel(onColumn.left, types, schemas, "ko")).toBe("세금계산서 · 품목 품목");
    expect(ruleComplete(onColumn)).toBe(true);
    // A column the table does not have is still a problem.
    expect(unresolvedOperands({ ...onColumn, left: { doc_type: "tax_invoice", field: "items.bogus", value: null } }, schemas, types)).toEqual(["left"]);
  });

  it("summarises a rule in Korean and labels literals", () => {
    expect(ruleSummary(rule, types, schemas, "ko", t)).toBe("세금계산서 · 공급자 공급자 사업자등록번호 같아야 함 사업자등록증 · 등록번호");
    const literal: Rule = { ...rule, op: "gte", right: { doc_type: null, field: null, value: "2000000" } };
    expect(ruleSummary(literal, types, schemas, "ko", t)).toBe("세금계산서 · 공급자 공급자 사업자등록번호 이상 '2000000'");
    expect(operandLabel(null, types, schemas, "ko")).toBe("");
    const pct: Rule = { ...rule, op: "within_pct", tolerance: 10 };
    expect(ruleSummary(pct, types, schemas, "ko", t)).toContain("오차 이내");
    expect(ruleSummary(pct, types, schemas, "ko", t)).toContain("10%");
  });

  it("checks completeness per operator", () => {
    expect(ruleComplete(rule)).toBe(true);
    expect(ruleComplete({ ...rule, right: null })).toBe(false);
    expect(ruleComplete({ ...rule, op: "present", right: null })).toBe(true);
    expect(ruleComplete({ ...rule, op: "gte", right: { doc_type: null, field: null, value: " " } })).toBe(false);
    expect(ruleComplete({ ...rule, op: "within_pct", tolerance: null })).toBe(false);
    expect(ruleComplete({ ...rule, op: "within_pct", tolerance: -5 })).toBe(false);
    expect(ruleComplete({ ...rule, tolerance: -1 })).toBe(false);
    expect(ruleComplete(newRule())).toBe(false);
  });

  it("parses the tolerance input as a finite, non-negative number", () => {
    expect(parseTolerance("")).toBeNull();
    expect(parseTolerance("  ")).toBeNull();
    expect(parseTolerance("10")).toBe(10);
    expect(parseTolerance("0.5")).toBe(0.5);
    expect(parseTolerance("-3")).toBe(0);
    expect(parseTolerance("abc")).toBeNull();
  });

  it("marks evidence whose value is not settled: 추출 전, 검토 대기, 거부, 문서 없음", () => {
    expect(evidenceStatusLabel("not_extracted", t)).toBe("추출 전");
    expect(evidenceStatusLabel("pending", t)).toBe(ko.projects.results.evidence_status.pending);
    expect(evidenceStatusLabel("rejected", t)).toBe(ko.projects.results.evidence_status.rejected);
    expect(evidenceStatusLabel("missing_document", t)).toBe(ko.projects.results.evidence_status.missing_document);
    // A settled value (approved, read automatically, a document-level value) and a plain gap carry no mark.
    for (const status of ["approved", "corrected", "auto", "document", "absent", null, undefined] as const) expect(evidenceStatusLabel(status, t), String(status)).toBeNull();
    expect(Object.keys(en.projects.results.evidence_status).sort()).toEqual(Object.keys(ko.projects.results.evidence_status).sort());
  });

  it("flags operands the schemas cannot resolve", () => {
    expect(unresolvedOperands(rule, schemas, types)).toEqual([]);
    expect(unresolvedOperands({ ...rule, left: { doc_type: "tax_invoice", field: "nope", value: null } }, schemas, types)).toEqual(["left"]);
    expect(unresolvedOperands({ ...rule, right: { doc_type: "contract", field: "amount", value: null } }, schemas, types)).toEqual(["right"]);
    expect(unresolvedOperands({ ...rule, op: "present", right: { doc_type: "contract", field: "amount", value: null } }, schemas, types)).toEqual([]);
  });
});
