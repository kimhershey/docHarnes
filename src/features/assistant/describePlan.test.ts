// Plan-describing helpers against the real Korean strings: op labels and
// parameter summaries (plan card rows), the impact line, the only_reviewed
// toggle, the UI context and the result-card action derivation.
import i18next from "i18next";
import { describe, expect, it } from "vitest";

import en from "@/i18n/en.json";
import ko from "@/i18n/ko.json";
import type { UiOpResult } from "@/lib/commands";
import type { CheckRun, Draft, DraftOutcome, Plan, PlanContext, PlanExecution, PlanPreview, PlanResponse, Project, ProjectDetail, RuleDraft } from "@/lib/types";

import { agentStepDetail, deriveResultActions, formatAmount, impactLine, impactOf, isPlanExecution, listTotal, onlyReviewedValue, opLabel, organizePreview, planKind, projectName, requiresConfirmation, ruleDraftPreview, summarizeOp, uiContextFor, uiResultsByStep, withOnlyReviewed } from "./describePlan";
import { OP_KINDS, OP_SPECS, UI_OPS } from "./opSpecs";

const instance = i18next.createInstance();
await instance.init({ lng: "ko", fallbackLng: false, resources: { ko: { translation: ko }, en: { translation: en } }, interpolation: { escapeValue: false } });
const t = instance.t.bind(instance);

const context: PlanContext = {
  today: "2026-11-04",
  language: "ko",
  folders: ["inbox", "documents/2026/10"],
  doc_types: [{ slug: "receipt", label_ko: "영수증", label_en: "Receipt" }],
  schemas: [{ id: "s1", name: "영수증", name_en: "Receipt", doc_type: "receipt", fields: [{ key: "total", label_ko: "합계", label_en: "Total", kind: "amount" }] }],
  tags: [],
  current_folder: null,
  selected_doc_ids: [],
  current_document: null,
  current_page: null,
  document_count: 2,
  model_ids: [],
  projects: [{ id: "p1", name: "인증 심사 A", status: "active" }],
  current_project: null,
  history: [],
};

function plan(ops: Plan["operations"], extra: Partial<Plan> = {}): Plan {
  return { intent: ops[0]?.op ?? "noop", scope: {}, operations: ops, requires_confirmation: false, confidence: 0.9, summary: "요약", ...extra };
}

describe("op specs mirror", () => {
  it("covers every op kind with a Korean label", () => {
    expect(OP_KINDS).toHaveLength(63);
    for (const op of OP_KINDS) {
      expect(OP_SPECS[op].op).toBe(op);
      expect(opLabel(op, t)).not.toBe(`planner.ops.${op}`);
    }
    expect(UI_OPS.has("set_theme")).toBe(true);
    expect(UI_OPS.has("export")).toBe(false);
  });
});

describe("summarizeOp", () => {
  const ctx = { lang: "ko", context };
  it("describes extract / organise / export like the design rows", () => {
    expect(summarizeOp({ op: "extract", schema_ref: "s1", fields: ["a", "b", "c"] }, t, ctx)).toBe("스키마 영수증 · 필드 3개");
    expect(summarizeOp({ op: "extract" }, t, ctx)).toBe("자동(유형별 스키마) · 전체 필드");
    // UI-15: no internal notation on the card (`documents/{{doc_type}} · group_by=doc_type`) — the grouping and
    // the template variables are named in words.
    expect(summarizeOp({ op: "organize", group_by: "month" }, t, ctx)).toBe("월별 폴더(documents/<연도>/<월>/<유형>)");
    expect(summarizeOp({ op: "organize", group_by: "doc_type" }, t, ctx)).toBe("유형별 폴더(documents/<유형>)");
    expect(summarizeOp({ op: "organize", template: "{{year}}-{{month}}_{{vendor}}", folder_template: "archive/{{project}}", group_by: "month", dry_run: true }, t, ctx)).toBe("파일명 <연도>-<월>_<거래처> · 폴더 archive/<프로젝트> · 미리보기만");
    expect(summarizeOp({ op: "move", folder: "documents/{{year}}", only_reviewed: false }, t, ctx)).toBe("폴더 documents/<연도> · 검토 미완료 포함");
    expect(summarizeOp({ op: "move", folder: "documents/{{vendor}}/{{doc_type}}", group_by: "vendor" }, t, ctx)).toBe("거래처별 폴더(documents/<거래처>/<유형>)");
    expect(summarizeOp({ op: "rename", template: "{{title}}_{{seq}}" }, t, ctx)).toBe("파일명 <제목>_<순번>");
    expect(summarizeOp({ op: "export", format: "xlsx", path: "exports/a.xlsx", columns: ["x"], include: ["status"] }, t, ctx)).toBe("XLSX · exports/a.xlsx · 열 1개 · 공통 열: 상태");
    expect(summarizeOp({ op: "aggregate", fn: "sum", field: "total", group_by: "month" }, t, ctx)).toBe("합계 · total · 월별");
    expect(summarizeOp({ op: "aggregate", fn: "count", group_by: "none" }, t, ctx)).toBe("건수");
    expect(summarizeOp({ op: "import", source: "inbox", auto_pipeline: "classify" }, t, ctx)).toBe("inbox/ · 자동 처리: 분류까지");
    expect(summarizeOp({ op: "reindex", scope: "vec" }, t, ctx)).toBe("의미(벡터) 색인");
    const en_ = instance.getFixedT("en");
    expect(summarizeOp({ op: "organize", group_by: "project" }, en_, { lang: "en", context })).toBe("folders per project (documents/<Project>/<Type>)");
    // No step summary shows a raw `{{variable}}` or `key=value`.
    for (const op of [{ op: "organize", group_by: "vendor" }, { op: "move", folder: "x/{{unknown_var}}" }, { op: "rename", template: "{{name}}.{{ext}}" }] as const) {
      const text = summarizeOp(op as never, t, ctx);
      expect(text, text).not.toMatch(/\{\{|=/);
    }
  });
  it("describes the P6 ops (drafts, projects, rules, retention)", () => {
    expect(summarizeOp({ op: "draft_document", form_ref: "quotation", content: "노트북 10대 견적", sources: ["a", "b"], output: "pdf" }, t, ctx)).toBe("양식 견적서 · 노트북 10대 견적 · 인용 문서 2건 · PDF");
    // 0.7.0 초안 어시스턴트 op: the scope, the new heading and the instruction.
    expect(summarizeOp({ op: "revise_draft", scope: "section", section: 3, instruction: "더 자세히" }, t, ctx)).toBe("3절 다시 쓰기 · 더 자세히");
    expect(summarizeOp({ op: "revise_draft", scope: "insert_after", section: null, heading: "결론", instruction: "기대 효과 정리" }, t, ctx)).toBe("끝에 새 절 · '결론' · 기대 효과 정리");
    expect(summarizeOp({ op: "revise_draft", scope: "insert_after", section: 2, instruction: "x" }, t, ctx)).toBe("2절 뒤에 새 절 · x");
    expect(summarizeOp({ op: "revise_draft", scope: "whole", instruction: "격식체로" }, t, ctx)).toBe("전체 다시 쓰기 · 격식체로");
    expect(planKind(plan([{ op: "revise_draft", scope: "whole", instruction: "x" }]))).toBe("work");
    expect(summarizeOp({ op: "create_project", name: "한빛", template: "builtin-vendor_onboarding" }, t, ctx)).toBe("'한빛' · 템플릿 거래처 등록");
    expect(summarizeOp({ op: "add_to_project", project: "p1", doc_ids: ["x"] }, t, ctx)).toBe("인증 심사 A · 문서 1건");
    // Without a named / open project the row says "현재 프로젝트" — no "only project" fallback: the validator is strict (project_required).
    expect(summarizeOp({ op: "check_consistency" }, t, ctx)).toBe("현재 프로젝트 · 전체 규칙");
    expect(summarizeOp({ op: "check_consistency" }, t, { ...ctx, context: { ...context, current_project: "p1" } })).toBe("인증 심사 A · 전체 규칙");
    expect(projectName(null, { ...context, current_project: "p1" }, t)).toBe("인증 심사 A");
    expect(projectName("p-unknown", context, t)).toBe("p-unknown");
    expect(summarizeOp({ op: "define_rule", description: "서명 필수" }, t, { ...ctx, context: { ...context, projects: [] } })).toBe("현재 프로젝트 · 서명 필수");
    expect(summarizeOp({ op: "set_retention", item: "chats", days: 30 }, t, ctx)).toBe("대화 기록 · 30일");
    expect(summarizeOp({ op: "set_retention", item: "journal", days: -1 }, t, ctx)).toBe("파일 조작 저널 · 무기한");
    expect(summarizeOp({ op: "compare", doc_ids: ["a", "b"], aspects: ["금액"] }, t, ctx)).toBe("문서 2건 · 금액");
    expect(planKind(plan([{ op: "register_form", doc_ref: "current" }]))).toBe("ui");
    expect(planKind(plan([{ op: "draft_document", content: "x" }]))).toBe("work");
  });
  it("resolves labels for settings, theme, type and screen ops", () => {
    expect(summarizeOp({ op: "set_theme", theme: "dark" }, t, ctx)).toBe("다크");
    expect(summarizeOp({ op: "set_doc_type", doc_type: "receipt" }, t, ctx)).toBe("영수증");
    expect(summarizeOp({ op: "open", target: "screen", ref: "settings" }, t, ctx)).toBe("화면 · 설정");
    expect(summarizeOp({ op: "set_setting", key: "render_dpi", value: 200 }, t, ctx)).toBe("기본 렌더 DPI = 200");
    expect(summarizeOp({ op: "job", action: "pause" }, t, ctx)).toBe("일시정지 · 마지막 작업");
    expect(summarizeOp({ op: "undo" }, t, ctx)).toBe("마지막 파일 작업");
  });
});

describe("plan classification and editing", () => {
  it("classifies clarify / help / answer / ui / work plans", () => {
    expect(planKind(plan([{ op: "clarify", question: "?" }], { intent: "clarify" }))).toBe("clarify");
    expect(planKind(plan([{ op: "help" }]))).toBe("help");
    expect(planKind(plan([{ op: "answer", question: "합계?" }]))).toBe("answer");
    expect(planKind(plan([{ op: "set_theme", theme: "dark" }, { op: "set_language", lang: "en" }]))).toBe("ui");
    expect(planKind(plan([{ op: "set_theme", theme: "dark" }, { op: "export", format: "csv" }]))).toBe("work");
  });
  it("binds only_reviewed to every file op", () => {
    const p = plan([{ op: "extract" }, { op: "move", folder: "x" }, { op: "rename", template: "{{name}}", only_reviewed: false }]);
    expect(onlyReviewedValue(p, true)).toBe(false);
    const next = withOnlyReviewed(p, true);
    expect(next.operations[1]).toMatchObject({ op: "move", only_reviewed: true });
    expect(next.operations[2]).toMatchObject({ op: "rename", only_reviewed: true });
    expect(next.operations[0]).toEqual({ op: "extract" });
    expect(onlyReviewedValue(plan([{ op: "export", format: "csv" }]), true)).toBeNull();
  });
  it("takes confirmation from the plan or the preview", () => {
    const response = (requires: boolean, preview: boolean | null): PlanResponse => ({
      result: { text: "x", source: "rules", plan: plan([{ op: "export", format: "csv" }], { requires_confirmation: requires }), issues: [], date: null, attempts: 1, model: null, prompt_tokens: 0, completion_tokens: 0 },
      preview: preview === null ? null : { document_count: 1, document_ids: ["d"], steps: [], requires_confirmation: preview },
      context,
    });
    expect(requiresConfirmation(response(false, null))).toBe(false);
    expect(requiresConfirmation(response(false, true))).toBe(true);
    expect(requiresConfirmation(response(true, false))).toBe(true);
  });
  it("builds the UI context from the scope selector", () => {
    expect(uiContextFor("all", "inbox", ["a"], null)).toEqual({});
    expect(uiContextFor("folder", "inbox", ["a"], null)).toEqual({ folder: "inbox" });
    expect(uiContextFor("selection", "inbox", ["a", "b"], { id: "d", page: 3 })).toEqual({ selected_doc_ids: ["a", "b"], current_document: "d", current_page: 3 });
    expect(uiContextFor("knowledge", null, [], { id: "d", page: null })).toEqual({ current_document: "d" });
  });
});

describe("impact line", () => {
  it("counts moves from the organise preview and documents in scope", () => {
    const p = plan([{ op: "extract" }, { op: "move", folder: "documents/{{year}}" }]);
    const preview: PlanPreview = {
      document_count: 31,
      document_ids: [],
      steps: [{ index: 1, op: "move", summary: "", payload: { items: [], moves: 27, held: 4, unchanged: 0 }, confirm: true }],
      requires_confirmation: true,
    };
    const impact = impactOf(p, preview);
    expect(impact).toEqual({ moves: 27, deletes: 0, exports: 0, drafts: 0, projects: 0, documents: 31 });
    expect(impactLine(impact, t)).toBe("부작용: 파일 이동 ≤27건(저널 기록, 되돌리기 가능) · 대상 문서 31건");
    expect(impactLine(impactOf(plan([{ op: "search", query: "q" }]), null), t)).toBe("부작용: 없음 · 대상 문서 0건");
  });
});

describe("result actions", () => {
  const execution: PlanExecution = {
    steps: [
      { index: 0, op: "extract", status: "done", summary: "2건 추출", payload: { processed: ["a", "b"], review_pending: 3 }, elapsed_ms: 1 },
      { index: 1, op: "move", status: "done", summary: "이동 2건", payload: { batch_id: "b1", moved: 2, held: 1, failed: [], items: [{ doc_id: "a", name: "a.pdf", from: "inbox/a.pdf", to: "documents/a.pdf", status: "done" }] }, elapsed_ms: 1 },
      { index: 2, op: "export", status: "done", summary: "→ exports/x.xlsx", payload: { path: "exports/x.xlsx", absolute: "C:\\ws\\exports\\x.xlsx", format: "xlsx", target: "fields", documents: 2, rows: 2, sheets: [], overwritten: false, skipped: [], extra_files: [] }, elapsed_ms: 1 },
      { index: 3, op: "aggregate", status: "done", summary: "합계", payload: { func: "sum", field: "total", field_label: "합계", doc_type: "receipt", basis: "doc_date", date_from: null, date_to: null, include_unreviewed: false, value: 10, count: 1, reviewed: { value: 10, count: 1 }, unreviewed: { value: 0, count: 0 }, missing: 0, groups: [], documents: [] }, elapsed_ms: 1 },
      { index: 4, op: "search", status: "failed", summary: "", payload: null, error: "boom", elapsed_ms: 1 },
      { index: 5, op: "tag", status: "skipped", summary: "", payload: null, elapsed_ms: 0 },
    ],
    cancelled: false,
    documents: ["a", "b"],
    ui_actions: [],
    batches: ["b1"],
  };
  it("derives review / moves / undo / export / aggregate actions and the failed step", () => {
    const actions = deriveResultActions(execution, plan([{ op: "extract" }, { op: "move", folder: "x" }, { op: "export", format: "xlsx" }, { op: "aggregate", fn: "sum", field: "total" }, { op: "search", query: "q" }, { op: "tag" }]));
    expect(actions.reviewPending).toBe(3);
    expect(actions.reviewDocuments).toEqual(["a", "b"]);
    expect(actions.organise).toHaveLength(1);
    expect(actions.organise[0]).toMatchObject({ batchId: "b1", moved: 2, held: 1, failed: 0 });
    expect(actions.batches).toEqual(["b1"]);
    expect(actions.exports[0]?.result.path).toBe("exports/x.xlsx");
    expect(actions.aggregates[0]?.result.value).toBe(10);
    expect(actions.aggregates[0]?.op).toMatchObject({ op: "aggregate", fn: "sum" });
    expect(actions.searches).toEqual([]);
    expect(actions.failed?.index).toBe(4);
    expect(isPlanExecution(execution)).toBe(true);
    expect(isPlanExecution({ steps: [] })).toBe(false);
  });
  it("formats amounts with thousands separators", () => {
    expect(formatAmount(1234500)).toBe("1,234,500");
    expect(formatAmount(12.345)).toBe("12.3");
  });
});

// The P6 ops in the shapes `exec.rs` really sends (review H1), plus the bare shapes the browser mock used to send.
// CON-9 / CON-10: the detail line of an agent step reads the payload `planner/exec.rs` sends.
describe("agentStepDetail", () => {
  it("classify_projects: a run that only suggests is not shown as '배정 0건'", () => {
    // `exec.rs`: `applied` is 0 (not null) without an assignment; `apply_result` is null.
    const suggested = { suggested: 3, undecided: 1, applied: 0, suggestions: [], apply_result: null };
    expect(agentStepDetail("classify_projects", suggested, t)?.text).toBe("제안 3건 — 프로젝트 화면에서 적용");
    const assigned = { suggested: 3, undecided: 1, applied: 2, suggestions: [], apply_result: { added: 2, already: 0, project_ids: ["p1"] } };
    expect(agentStepDetail("classify_projects", assigned, t)?.text).toBe("제안 3건 · 배정 2건");
    // Payloads without `apply_result` (stored cards of 0.7.0, the old mock): null = suggestions only.
    expect(agentStepDetail("classify_projects", { suggested: 3, applied: null }, t)?.text).toBe("제안 3건 — 프로젝트 화면에서 적용");
    expect(agentStepDetail("classify_projects", { suggested: 3, applied: 2 }, t)?.text).toBe("제안 3건 · 배정 2건");
  });

  it("define_term: the term comes from the glossary entry the executor sends", () => {
    const entry = { id: 7, term: "공급가액", definition: "부가세를 뺀 금액", synonyms: ["net amount"], created_at: "", updated_at: "" };
    expect(agentStepDetail("define_term", { action: "save", term: entry }, t)?.text).toBe("용어 저장 · '공급가액'");
    expect(agentStepDetail("define_term", { action: "delete", term: entry }, t)?.text).toBe("용어 삭제 · '공급가액'");
    // A bare string still works.
    expect(agentStepDetail("define_term", { action: "save", term: "합계" }, t)?.text).toBe("용어 저장 · '합계'");
  });
});

// CON-16: a card that was too large to store whole is read back with the first rows of its lists and
// `<list>_total` next to them — the numbers on the card are those of the whole.
describe("lists of a card stored smaller", () => {
  const rows = (n: number, status: "move" | "held" | "done") => Array.from({ length: n }, (_, i) => ({ doc_id: `d${i}`, name: `f${i}.pdf`, from: `inbox/f${i}.pdf`, to: `documents/f${i}.pdf`, status }));

  it("listTotal: the list's length, or the total kept beside a cut list", () => {
    expect(listTotal({ documents: ["a", "b"] }, "documents")).toBe(2);
    expect(listTotal({ documents: ["a", "b"], documents_total: 6000 }, "documents")).toBe(6000);
    expect(listTotal({}, "documents")).toBe(0);
  });

  it("the plan preview counts every row, not only the ones kept", () => {
    const step = { index: 0, op: "move" as const, summary: "", payload: { items: [...rows(150, "move"), ...rows(50, "held")], items_total: 6000, moves: 5800, held: 200, unchanged: 0 }, confirm: true };
    const preview = organizePreview(step)!;
    expect(preview.items).toHaveLength(200);
    expect(preview).toMatchObject({ total: 6000, moves: 5800, held: 200 });
    expect(impactOf(plan([{ op: "move", folder: "x" }]), { document_count: 6000, document_ids: [], steps: [step], requires_confirmation: true })).toMatchObject({ documents: 6000, moves: 5800 });
    // A whole preview: the total is the rows it shows.
    expect(organizePreview({ ...step, payload: { items: [...rows(2, "move"), { ...rows(1, "move")[0]!, status: "unchanged" }], moves: 2, held: 0, unchanged: 1 } })).toMatchObject({ total: 2 });
  });

  it("the result card counts every moved file and every document of a step", () => {
    const execution: PlanExecution = {
      steps: [
        { index: 0, op: "move", status: "done", summary: "", payload: { batch_id: "b1", moved: 5990, held: 0, failed: rows(10, "done"), failed_total: 10, items: rows(200, "done"), items_total: 6000 }, elapsed_ms: 1 },
        { index: 1, op: "rotate", status: "done", summary: "", payload: { documents: ["a", "b"], documents_total: 6000, degrees: 90 }, elapsed_ms: 1 },
      ],
      cancelled: false,
      documents: [],
      ui_actions: [],
      batches: ["b1"],
    };
    const actions = deriveResultActions(execution, plan([{ op: "move", folder: "x" }, { op: "rotate", degrees: 90 }]));
    expect(actions.organise[0]).toMatchObject({ total: 6000, moved: 5990, failed: 10, batchId: "b1" });
    expect(actions.organise[0]!.items).toHaveLength(200);
    expect(agentStepDetail("rotate", execution.steps[1]!.payload, t)?.text).toContain("6000");
  });
});

// PLN-17: the UI runs the steps the executor leaves to it; what became of each is shown on its step.
describe("uiResultsByStep", () => {
  const step = (index: number, op: PlanExecution["steps"][number]["op"], status: PlanExecution["steps"][number]["status"]): PlanExecution["steps"][number] => ({ index, op, status, summary: "", payload: null, elapsed_ms: 0 });
  const result = (op: UiOpResult["op"], ok: boolean): UiOpResult => ({ op, ok, summary: ok ? `${op} done` : "", error: ok ? null : `${op} failed` });

  it("assigns the k-th result to the k-th step the executor left to the UI", () => {
    const execution: PlanExecution = {
      steps: [step(0, "tag", "done"), step(1, "set_theme", "ui"), step(2, "noop", "skipped"), step(3, "manage_model", "ui")],
      cancelled: false,
      documents: [],
      ui_actions: [{ op: "set_theme", theme: "dark" }, { op: "noop" }, { op: "manage_model", action: "delete", id: "x" }],
      batches: [],
    };
    const map = uiResultsByStep(execution, [result("set_theme", true), result("noop", true), result("manage_model", false)]);
    expect([...map.keys()]).toEqual([1, 2, 3]);
    expect(map.get(1)).toMatchObject({ ok: true, summary: "set_theme done" });
    expect(map.get(3)).toMatchObject({ ok: false, error: "manage_model failed" });
  });

  it("leaves steps without a result alone: none yet, or a step skipped after a failure", () => {
    const execution: PlanExecution = {
      steps: [step(0, "search", "failed"), step(1, "open", "skipped"), step(2, "set_theme", "ui")],
      cancelled: false,
      documents: [],
      ui_actions: [{ op: "set_theme", theme: "dark" }],
      batches: [],
    };
    expect(uiResultsByStep(execution, null).size).toBe(0);
    expect(uiResultsByStep(execution, []).size).toBe(0);
    // `open` was skipped by the executor (not handed over): the one result is the theme's.
    expect([...uiResultsByStep(execution, [result("set_theme", false)]).keys()]).toEqual([2]);
  });
});

describe("result actions (P6 payloads)", () => {
  const draft: Draft = {
    id: "d1",
    title: "견적서",
    template: { kind: "builtin", template: "quotation" },
    doc_type: "quotation",
    fields: [{ key: "total", label: "합계", value: "1", kind: "amount", required: true, normalized: null, source: null, user_edited: false }],
    tables: [],
    body: [],
    references: [],
    issues: [],
    language: "ko",
    tone: "formal",
    length: "medium",
    created_at: "",
    updated_at: "",
    model: "gemma4-e4b-q5km",
    request: null,
    notes: [],
    saved_path: null,
    saved_doc_id: null,
  };
  const outcome: DraftOutcome = { draft, prompt_tokens: 10, completion_tokens: 5, elapsed_ms: 1, source: "model" };
  const project: Project = { id: "p1", name: "인증 심사 A", template_id: null, intake_no: null, owner: null, start_date: "2026-11-01", end_date: null, phase: "active", overdue: false, settings: { copies: true }, notes: null, inbox_folder: null, keywords: [], created_at: "", updated_at: "", document_count: 2, missing_count: 0, last_check_at: null, last_pass: 0, last_mismatch: 0, last_undetermined: 0 };
  const detail: ProjectDetail = { project, template: null, documents: [], checklist: [], rules: [], last_run: null, relations: { nodes: [], edges: [] } };
  const ruleDraft: RuleDraft = { rule: { id: "r1", description: "사업자번호 일치", left: { doc_type: "tax_invoice", field: "supplier.brn", value: null }, op: "eq", right: { doc_type: "business_registration", field: "registration_no", value: null }, tolerance: null, required: true, enabled: true }, confidence: 0.9, source: "rules", issues: [], unresolved: [] };
  const run: CheckRun = { id: "run1", project_id: "p1", started_at: "", finished_at: "", pass: 1, mismatch: 0, undetermined: 0, missing: 0, results: [], missing_doc_types: [] };
  const step = (index: number, op: PlanExecution["steps"][number]["op"], payload: unknown): PlanExecution["steps"][number] => ({ index, op, status: "done", summary: "", payload, elapsed_ms: 1 });
  const exec = (steps: PlanExecution["steps"]): PlanExecution => ({ steps, cancelled: false, documents: [], ui_actions: [], batches: [] });

  it("reads the Rust payloads: DraftOutcome, {project_id, saved, draft}, {project_id, added, project}", () => {
    const p = plan([{ op: "draft_document", content: "x" }, { op: "request_supplement", project: "p1" }, { op: "define_rule", project: "p1", description: "사업자번호 일치" }, { op: "add_to_project", project: "p1" }, { op: "create_project", name: "B" }, { op: "check_consistency", project: "p1" }]);
    const actions = deriveResultActions(exec([step(0, "draft_document", outcome), step(1, "request_supplement", { ...outcome, project_id: "p1" }), step(2, "define_rule", { project_id: "p1", saved: true, draft: ruleDraft }), step(3, "add_to_project", { project_id: "p1", added: 2, project }), step(4, "create_project", { ...project, id: "p2", name: "B" }), step(5, "check_consistency", run)]), p);
    expect(actions.drafts.map((d) => [d.index, d.draft.id, d.projectId])).toEqual([
      [0, "d1", null],
      [1, "d1", "p1"],
    ]);
    expect(actions.ruleDrafts).toEqual([{ index: 2, ruleDraft, projectId: "p1", saved: true }]);
    expect(actions.projects.map((x) => [x.index, x.projectId, x.project?.name, x.detail])).toEqual([
      [3, "p1", "인증 심사 A", null],
      [4, "p2", "B", null],
    ]);
    expect(actions.checks).toEqual([{ index: 5, run }]);
  });

  it("still accepts the bare shapes (Draft, RuleDraft, ProjectDetail) and a deleted project", () => {
    const p = plan([{ op: "draft_document", content: "x" }, { op: "define_rule", project: "p1", description: "d" }, { op: "add_to_project", project: "p1" }, { op: "add_to_project", project: "p1" }]);
    const actions = deriveResultActions(exec([step(0, "draft_document", draft), step(1, "define_rule", ruleDraft), step(2, "add_to_project", detail), step(3, "add_to_project", { project_id: "p1", added: 0, project: null })]), p);
    expect(actions.drafts).toEqual([{ index: 0, draft, projectId: null }]);
    expect(actions.ruleDrafts).toEqual([{ index: 1, ruleDraft, projectId: "p1", saved: false }]);
    expect(actions.projects[0]).toEqual({ index: 2, projectId: "p1", project, detail });
    expect(actions.projects[1]).toEqual({ index: 3, projectId: "p1", project: null, detail: null });
    // A failed / unrelated payload adds nothing.
    expect(deriveResultActions(exec([step(0, "define_rule", { description: "x" }), step(1, "draft_document", null)]), p).ruleDrafts).toEqual([]);
  });

  it("reads the define_rule preview payload ({ description, draft } or a bare RuleDraft)", () => {
    const preview = (payload: unknown) => ruleDraftPreview({ index: 0, op: "define_rule", summary: "", payload, confirm: true });
    expect(preview({ description: "사업자번호 일치", draft: ruleDraft })).toEqual({ draft: ruleDraft, description: "사업자번호 일치" });
    expect(preview({ description: "알 수 없는 문장", draft: null })).toEqual({ draft: null, description: "알 수 없는 문장" });
    expect(preview(ruleDraft)).toEqual({ draft: ruleDraft, description: "사업자번호 일치" });
    expect(preview({ items: [] })).toBeNull();
    expect(ruleDraftPreview(undefined)).toBeNull();
  });
});
