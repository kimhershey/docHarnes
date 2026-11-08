// The browser mocks of P6: the simulated draft (item rows from the sentence, the deliberate
// sum mismatch with its suggestion, reverse validation) and the natural-language rule parser.
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
const mg = await import("@/lib/mock-generate");
const mp = await import("@/lib/mock-projects");

async function workspace() {
  await backend.mockInvoke("workspace_create", { args: { path: "C:\\Work\\Docs", name: null, locale: null, force_unlock: false } });
}

describe("mock generate", () => {
  beforeEach(async () => {
    backend.resetMock();
    await workspace();
  });

  it("drafts a quotation from the sentence with a sum mismatch the validator can fix", () => {
    expect(mg.listTemplates().map((t) => t.kind)).toContain("quotation");
    const req = { form_ref: "quotation", format_description: null, content: "㈜한빛에 노트북 10대, 대당 1,200,000원, 유효기간 30일 견적서 만들어줘", sources: [], output: "hwpx" as const, tone: "formal" as const, length: "medium" as const, language: "ko" as const, use_knowledge: false, feedback: null, previous: null, excluded_sources: [] };
    const draft = mg.buildDraft(req);
    expect(draft.template).toEqual({ kind: "builtin", template: "quotation" });
    expect(draft.fields.find((f) => f.key === "recipient")?.value).toBe("㈜한빛");
    const items = draft.tables.find((t) => t.key === "items")!;
    expect(items.rows).toEqual([["노트북", "", "10", "1,200,000", "12,000,000"]]);
    const mismatch = draft.issues.find((i) => i.code === "sum_mismatch" && i.path === "fields.total");
    expect(mismatch?.suggestion).toBe("12,000,000");
    expect(draft.fields.find((f) => f.key === "total")?.value).toBe("13,200,000");
    // Applying the suggestion and validating again clears the issue.
    const total = draft.fields.find((f) => f.key === "total")!;
    total.value = mismatch!.suggestion!;
    mg.validateDraft(draft);
    expect(draft.issues.filter((i) => i.code === "sum_mismatch")).toEqual([]);
    expect(total.normalized).toBe("12000000");
  });

  it("keeps user edits across regeneration and flags required fields", () => {
    const req = { form_ref: "official_letter", format_description: null, content: "협조문 작성", sources: [], output: "docx" as const, tone: "plain" as const, length: "short" as const, language: "ko" as const, use_knowledge: false, feedback: null, previous: null, excluded_sources: [] };
    const first = mg.buildDraft(req);
    expect(first.issues.some((i) => i.code === "required_missing" && i.path === "fields.recipient")).toBe(true);
    const edited = { ...first, fields: first.fields.map((f) => (f.key === "recipient" ? { ...f, value: "강남세무서장", user_edited: true } : f)) };
    const again = mg.buildDraft({ ...req, previous: edited, feedback: "길게" });
    expect(again.fields.find((f) => f.key === "recipient")?.value).toBe("강남세무서장");
    expect(again.length).toBe("long");
    expect(again.id).toBe(first.id);
    expect(mg.toMarkdown(again)).toContain("# ");
    expect(mg.toHtml(again)).toContain("<table");
  });

  it("parses natural-language rules against the built-in schemas", () => {
    const eq = mp.parseRule("세금계산서의 공급자 사업자번호는 사업자등록증의 등록번호와 같아야 해");
    expect(eq.rule.left).toEqual({ doc_type: "tax_invoice", field: "supplier.brn", value: null });
    expect(eq.rule.op).toBe("eq");
    expect(eq.rule.right).toEqual({ doc_type: "business_registration", field: "registration_no", value: null });
    expect(eq.unresolved).toEqual([]);
    expect(eq.confidence).toBeGreaterThan(0.8);

    const gte = mp.parseRule("세금계산서의 합계금액은 2,000,000 이상이어야 해");
    expect(gte.rule.op).toBe("gte");
    expect(gte.rule.left.field).toBe("grand_total");
    expect(gte.rule.right?.value).toBe("2000000");

    const present = mp.parseRule("신청서 서명 필수");
    expect(present.rule.op).toBe("present");
    expect(present.rule.left).toEqual({ doc_type: "application_form", field: "signed", value: null });

    const pct = mp.parseRule("세금계산서의 합계금액은 계약서의 계약금액과 10% 이내여야 해");
    expect(pct.rule.op).toBe("within_pct");
    expect(pct.rule.tolerance).toBe(10);
    expect(pct.rule.right).toEqual({ doc_type: "contract", field: "amount", value: null });

    const unknown = mp.parseRule("세금계산서의 우주선 번호는 사업자등록증의 등록번호와 같아야 해");
    expect(unknown.unresolved).toContain("우주선 번호");
    expect(unknown.issues.length).toBeGreaterThan(0);
  });

  it("runs the project commands end to end: create → rules → check → report", async () => {
    const tpl = (await backend.mockInvoke<{ id: string }[]>("project_templates")).find((t) => t.id === "builtin-vendor_onboarding")!;
    const project = await backend.mockInvoke<{ id: string; status: string }>("project_create", { input: { name: "한빛 거래처", template_id: tpl.id } });
    const detail = await backend.mockInvoke<{ rules: unknown[]; checklist: { missing: boolean }[] }>("project_get", { id: project.id });
    expect(detail.rules).toHaveLength(2);
    expect(detail.checklist.every((c) => c.missing)).toBe(true);
    const run = await backend.mockInvoke<{ missing: number; pass: number; results: { verdict: string }[] }>("project_check", { id: project.id, ruleIds: null });
    expect(run.missing).toBe(2);
    expect(run.results.every((r) => r.verdict === "missing")).toBe(true);
    const report = await backend.mockInvoke<{ markdown: string }>("project_report", { id: project.id });
    expect(report.markdown).toContain("# 프로젝트 리포트 — 한빛 거래처");
    await expect(backend.mockInvoke("project_request_supplement", { id: project.id, findingIds: null, output: null })).resolves.toMatchObject({ job_id: expect.any(String) });
    const status = await backend.mockInvoke<{ running: boolean; last_run_at: string | null }>("maintenance_status");
    expect(status.last_run_at).toBeNull();
  });
});
