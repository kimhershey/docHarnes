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

  it("drafts a software estimate from the sentence with a sum mismatch the validator can fix", () => {
    // The picker offers the 12 software templates + 보완요청서; the generic 0.1 set is hidden but still resolvable by slug.
    const kinds = mg.listTemplates().map((t) => t.kind);
    expect(kinds).toEqual(["rfp", "proposal", "sow", "srs", "dev_contract", "wbs", "progress_report", "meeting_minutes", "test_report", "change_request", "acceptance", "estimate", "supplement_request"]);
    expect(mg.templateInfo("quotation").kind).toBe("quotation");
    const req = { form_ref: "estimate", format_description: null, content: "DocuLens에 백엔드개발 고급 3MM 단가 2,500,000원, 유효기간 30일 견적서 만들어줘", sources: [], output: "hwpx" as const, tone: "formal" as const, length: "medium" as const, language: "ko" as const, use_knowledge: false, feedback: null, previous: null, excluded_sources: [], max_tokens: null };
    const draft = mg.buildDraft(req);
    expect(draft.template).toEqual({ kind: "builtin", template: "estimate" });
    expect(draft.doc_type).toBe("estimate");
    expect(draft.fields.find((f) => f.key === "recipient")?.value).toBe("DocuLens");
    const items = draft.tables.find((t) => t.key === "items")!;
    expect(items.rows).toEqual([["백엔드개발", "고급", "3", "2,500,000", "7,500,000", ""]]);
    expect(draft.fields.find((f) => f.key === "supply_total")?.value).toBe("7,500,000");
    expect(draft.fields.find((f) => f.key === "tax_total")?.value).toBe("750,000");
    // The simulated model forgot the VAT in the grand total: the validator offers 공급가액 + 세액.
    const mismatch = draft.issues.find((i) => i.code === "sum_mismatch" && i.path === "fields.grand_total");
    expect(mismatch?.suggestion).toBe("8,250,000");
    expect(draft.fields.find((f) => f.key === "grand_total")?.value).toBe("7,500,000");
    // Applying the suggestion and validating again clears the issue.
    const total = draft.fields.find((f) => f.key === "grand_total")!;
    total.value = mismatch!.suggestion!;
    mg.validateDraft(draft);
    expect(draft.issues.filter((i) => i.code === "sum_mismatch")).toEqual([]);
    expect(total.normalized).toBe("8250000");
    // Free text picks the software templates only; an explicit reference may still name a hidden one.
    expect(mg.resolveTemplate({ ...req, form_ref: null, content: "10월 진척 보고서 써줘" })).toEqual({ kind: "builtin", template: "progress_report" });
    expect(mg.resolveTemplate({ ...req, form_ref: null, content: "협조 공문 써줘" })).toEqual({ kind: "builtin", template: "report" });
    expect(mg.resolveTemplate({ ...req, form_ref: "A4 공문 형식", content: "x" })).toEqual({ kind: "builtin", template: "official_letter" });
  });

  it("keeps user edits across regeneration and flags required fields", () => {
    const req = { form_ref: "official_letter", format_description: null, content: "협조문 작성", sources: [], output: "docx" as const, tone: "plain" as const, length: "short" as const, language: "ko" as const, use_knowledge: false, feedback: null, previous: null, excluded_sources: [], max_tokens: null };
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

  it("saves like the core: an existing file is export_exists, overwrite replaces it (no numbering), warnings are sentences", () => {
    const req = { form_ref: "estimate", format_description: null, content: "DocuLens에 백엔드개발 고급 3MM 단가 2,500,000원 견적서", sources: [], output: "hwpx" as const, tone: "formal" as const, length: "medium" as const, language: "ko" as const, use_knowledge: false, feedback: null, previous: null, excluded_sources: [], max_tokens: null };
    const draft = mg.buildDraft(req);
    const first = mg.saveDraft({ draft, output: "hwpx", file_name: "견적서", folder: null, register: true, overwrite: false });
    expect(first.rel_path).toBe("documents/generated/견적서.hwpx");
    expect(first.doc_id).toBeTruthy();
    // The draft still has the deliberate sum mismatch (blocking): saved as "검토 전" with the core's sentence.
    expect(first.warnings).toEqual(["검증에서 확인이 필요한 항목이 있는 상태로 저장했습니다 (검토 전)."]);
    expect(() => mg.saveDraft({ draft, output: "hwpx", file_name: "견적서", folder: null, register: true, overwrite: false })).toThrow(/export_exists/);
    const again = mg.saveDraft({ draft, output: "hwpx", file_name: "견적서", folder: null, register: true, overwrite: true });
    expect(again.rel_path).toBe("documents/generated/견적서.hwpx");
    expect(again.doc_id).toBe(first.doc_id);
    const docs = backend.wsDocs().filter((d) => d.rel_path.startsWith("documents/generated/"));
    expect(docs).toHaveLength(1);
    // Markdown is an input format too (text family): the file registers as a generated text document.
    const md = mg.saveDraft({ draft, output: "md", file_name: "견적서", folder: null, register: true, overwrite: false });
    expect(md.doc_id).toBeTruthy();
    expect(md.warnings.some((w) => w.includes("Markdown"))).toBe(false);
    const mdDoc = backend.wsDocs().find((d) => d.id === md.doc_id)!;
    expect(mdDoc.source_kind).toBe("text");
    expect(mdDoc.role).toBe("generated");
    expect(backend.wsDocs().filter((d) => d.rel_path.startsWith("documents/generated/"))).toHaveLength(2);
  });

  it("parses natural-language rules against the built-in schemas", () => {
    const eq = mp.parseRule("개발 계약서의 계약금액은 제안서의 제안 금액과 같아야 해");
    expect(eq.rule.left).toEqual({ doc_type: "dev_contract", field: "amount", value: null });
    expect(eq.rule.op).toBe("eq");
    expect(eq.rule.right).toEqual({ doc_type: "proposal", field: "price", value: null });
    expect(eq.unresolved).toEqual([]);
    expect(eq.confidence).toBeGreaterThan(0.8);

    const gte = mp.parseRule("테스트 결과서의 통과율은 95 이상이어야 해");
    expect(gte.rule.op).toBe("gte");
    expect(gte.rule.left).toEqual({ doc_type: "test_report", field: "pass_rate", value: null });
    expect(gte.rule.right?.value).toBe("95");

    const present = mp.parseRule("개발 계약서 서명 필수");
    expect(present.rule.op).toBe("present");
    expect(present.rule.left).toEqual({ doc_type: "dev_contract", field: "signed", value: null });

    const pct = mp.parseRule("과업지시서의 과업 금액은 개발 계약서의 계약금액과 10% 이내여야 해");
    expect(pct.rule.op).toBe("within_pct");
    expect(pct.rule.tolerance).toBe(10);
    expect(pct.rule.left).toEqual({ doc_type: "sow", field: "amount", value: null });
    expect(pct.rule.right).toEqual({ doc_type: "dev_contract", field: "amount", value: null });

    const unknown = mp.parseRule("개발 계약서의 우주선 번호는 제안서의 제안 금액과 같아야 해");
    expect(unknown.unresolved).toContain("우주선 번호");
    expect(unknown.issues.length).toBeGreaterThan(0);
  });

  it("runs the project commands end to end: create → rules → check → report", async () => {
    const tpl = (await backend.mockInvoke<{ id: string }[]>("project_templates")).find((t) => t.id === "builtin-internal_dev")!;
    const project = await backend.mockInvoke<{ id: string; status: string }>("project_create", { input: { name: "한빛 포털", template_id: tpl.id } });
    const detail = await backend.mockInvoke<{ rules: unknown[]; checklist: { missing: boolean }[] }>("project_get", { id: project.id });
    expect(detail.rules).toHaveLength(3);
    expect(detail.checklist.every((c) => c.missing)).toBe(true);
    const run = await backend.mockInvoke<{ missing: number; pass: number; results: { verdict: string }[] }>("project_check", { id: project.id, ruleIds: null });
    expect(run.missing).toBe(3);
    expect(run.results.every((r) => r.verdict === "missing")).toBe(true);
    const report = await backend.mockInvoke<{ markdown: string }>("project_report", { id: project.id });
    expect(report.markdown).toContain("# 프로젝트 리포트 — 한빛 포털");
    await expect(backend.mockInvoke("project_request_supplement", { id: project.id, findingIds: null, output: null })).resolves.toMatchObject({ job_id: expect.any(String) });
    const status = await backend.mockInvoke<{ running: boolean; last_run_at: string | null }>("maintenance_status");
    expect(status.last_run_at).toBeNull();
  });
});
