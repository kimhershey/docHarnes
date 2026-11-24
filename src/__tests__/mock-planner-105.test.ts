// 1.0.5 planner decisions as the browser mock mirrors them (the Rust core is the source of truth:
// `planner/tests/v105.rs`, `planner/tests/exec_tests/v105.rs`): A "알려줘" is an answer, B the document type as
// a scope condition, C a question with its readings as choices — and the flow of a reading the user picks.
import { beforeEach, describe, expect, it } from "vitest";

import { defaultImportOptions, type ExecuteResponse, type ImportOptions, type Plan, type PlanResponse } from "@/lib/types";

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
const planner = await import("@/lib/mock-planner");
const { choicesOf, chosenResponse, plannedTextOf } = await import("@/features/assistant/choices");
const { planKind, requiresConfirmation } = await import("@/features/assistant/describePlan");

const FINISHED = ["done", "failed", "cancelled", "interrupted"];

async function waitJob(id: string): Promise<void> {
  for (let i = 0; i < 2000; i++) {
    const j = backend.getJob(id);
    if (j && FINISHED.includes(j.state)) return;
    await new Promise((r) => setTimeout(r, 5));
  }
  throw new Error(`job ${id} did not finish`);
}

/** Imports `names` (rendered, no automatic processing) and returns name → id. */
async function importDocs(names: string[]): Promise<Record<string, string>> {
  const paths = names.map((n) => `C:\\scan\\${n}`);
  const options = { ...defaultImportOptions(), auto_level: "none" } as ImportOptions;
  const plan = await backend.mockInvoke<{ token: string }>("documents_import_plan", { args: { paths, options, source: "folder" } });
  const job = await backend.mockInvoke<string>("documents_import_run", { args: { token: plan.token, paths, options, source: "folder" } });
  await waitJob(job);
  const docs = backend.wsDocs();
  return Object.fromEntries(names.map((n) => [n, docs.find((d) => d.original_name === n)!.id]));
}

const keysOf = (r: { choices?: { key: string }[] }) => (r.choices ?? []).map((c) => c.key);
const topicOf = (plan: Plan) => (plan.operations[0] as { topic?: string | null }).topic ?? null;

beforeEach(async () => {
  backend.resetMock();
  for (const key of ["doculens.mock.projects", "doculens.mock.knowledge", "doculens.mock.pipeline", "doculens.mock.maintenance"]) localStorage.removeItem(key);
  localStorage.setItem("doculens.mock.fast", "1");
  await backend.mockInvoke("workspace_create", { args: { path: "C:\\Work\\Docs105", name: null, locale: null, force_unlock: false } });
});

describe("1.0.5 — the mock planner", () => {
  it("A: '알려줘' is an answer; with a place and a subject it is the summary about that subject", async () => {
    await backend.mockInvoke("project_create", { input: { name: "AAA" } });
    const ids = await importDocs(["계약서_한빛.pdf"]);
    const ctx = planner.buildContext();
    const opened = planner.buildContext({ current_document: ids["계약서_한빛.pdf"]! });
    expect(planner.planText("납품 기한 알려줘", ctx).plan.operations[0]).toMatchObject({ op: "answer", question: "납품 기한 알려줘" });
    // Without a place it stays a question.
    expect(planner.planText("검수 기준에 대해 알려줘", ctx).plan.operations[0]).toMatchObject({ op: "answer" });
    // A project named: what its documents say about the subject, as "설명해줘" plans it.
    const inProject = planner.planText("AAA 프로젝트의 검수 기준에 대해 알려줘", ctx).plan;
    expect(inProject.operations[0]).toMatchObject({ op: "summarize", topic: "검수 기준" });
    expect(inProject.scope.filter?.project_id).toBe(ctx.projects[0]!.id);
    // The open document pointed at.
    const here = planner.planText("이 문서에서 검수 기준에 대해 알려줘", opened).plan;
    expect(here.operations[0]).toMatchObject({ op: "summarize", topic: "검수 기준" });
    expect(here.scope.doc_ids).toEqual([ids["계약서_한빛.pdf"]]);
    // What is to be told is a list or a count: not a summary about a subject.
    expect(planner.summaryTopic("AAA 프로젝트에서 위약금 관련 문서 목록 알려줘", ctx)).toBeNull();
    expect(planner.summaryTopic("AAA 프로젝트에서 계약 변경에 대해 몇 건인지 알려줘", ctx)).toBeNull();
  });

  it("B: a type condition takes the documents classified as the type and the ones named after it", async () => {
    const ids = await importDocs(["a.pdf", "한빛_용역계약서_초안.pdf", "b.pdf", "메모.pdf"]);
    await backend.mockInvoke("documents_set_doc_type", { id: ids["a.pdf"], docType: "dev_contract" });
    await backend.mockInvoke("documents_set_title", { id: ids["b.pdf"], title: "새봄 ERP 개발 계약서 (서명본)" });
    // ("계약" alone — two characters — says nothing about the type)
    await backend.mockInvoke("documents_set_title", { id: ids["메모.pdf"], title: "계약 관련 메모" });
    const resolved = (how?: "classified" | "named" | "both") =>
      planner
        .resolveScope({ filter: { doc_type: "dev_contract", ...(how ? { type_match: how } : {}) } })
        .map((d) => d.original_name)
        .sort();
    expect(resolved("classified")).toEqual(["a.pdf"]);
    expect(resolved("named")).toEqual(["b.pdf", "한빛_용역계약서_초안.pdf"]);
    expect(resolved("both")).toEqual(["a.pdf", "b.pdf", "한빛_용역계약서_초안.pdf"]);
    expect(resolved()).toEqual(resolved("both"));
    // A condition that matches nothing matches nothing: it is never dropped.
    expect(planner.resolveScope({ filter: { doc_type: "wbs" } })).toEqual([]);
    expect(planner.typeNames("dev_contract")).toEqual(expect.arrayContaining(["계약서", "용역계약서", "contract"]));
    expect(planner.typeNames("dev_contract")).not.toContain("계약");
  });

  it("C: a subject that is also the name of a project is asked about with three readings", async () => {
    const project = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "AAA" } });
    const ctx = planner.buildContext();
    const asked = planner.planText("AAA에 대해서 요약해줘", ctx);
    expect(asked.plan.intent).toBe("clarify");
    expect(asked.plan.clarification_question).toBe("'AAA' 프로젝트가 있습니다. 어느 쪽을 말씀하신 건가요?");
    expect(keysOf(asked)).toEqual(["project", "topic", "project_topic"]);
    const [whole, anywhere, inside] = asked.choices!;
    expect(whole!.label).toBe("'AAA' 프로젝트의 문서 전체를 요약");
    expect(whole!.plan).toMatchObject({ intent: "summarize", scope: { filter: { project_id: project.id, with_reference: true } } });
    expect(topicOf(whole!.plan)).toBeNull();
    expect(topicOf(anywhere!.plan)).toBe("AAA");
    expect(anywhere!.plan.scope.filter?.project_id ?? null).toBeNull();
    expect(topicOf(inside!.plan)).toBe("AAA");
    expect(inside!.plan.scope.filter?.project_id).toBe(project.id);
    // Told rather than summarised: the word everywhere is a grounded answer.
    const told = planner.planText("AAA에 대해 알려줘", ctx);
    expect(keysOf(told)).toEqual(["project", "topic", "project_topic"]);
    expect(told.choices![1]!.plan.operations[0]).toMatchObject({ op: "answer", question: "AAA에 대해 알려줘" });
    expect(told.choices![1]!.label).toBe("모든 문서에서 'AAA'에 관해 찾아 답하기");
    // The sentence settles it itself, or the name is no project's: no question.
    for (const text of ["AAA 프로젝트에 대해 요약해줘", "AAA 프로젝트 요약해줘", "AA에 대해서 요약해줘"]) {
      const r = planner.planText(text, ctx);
      expect(keysOf(r), text).toEqual([]);
      expect(r.plan.intent, text).toBe("summarize");
    }
    // The reply to the question is not asked about again.
    const reply = "AAA에 대해서 요약해줘\n\n[어시스턴트 질문] 'AAA' 프로젝트가 있습니다. 어느 쪽을 말씀하신 건가요?\n[사용자 답변] 문서 전체에서 찾아줘";
    expect(keysOf(planner.planText(reply, ctx))).toEqual([]);
  });

  it("C: a subject that is also the name of a document type is asked about with two readings", () => {
    const ctx = planner.buildContext();
    const asked = planner.planText("견적서에 대해 요약해줘", ctx);
    expect(asked.plan.intent).toBe("clarify");
    expect(asked.plan.clarification_question).toBe("'견적서'는 문서 유형의 이름이기도 합니다. 어느 쪽을 말씀하신 건가요?");
    expect(keysOf(asked)).toEqual(["type", "topic"]);
    expect(asked.choices![0]!.plan.scope.filter).toMatchObject({ doc_type: "estimate" });
    expect(topicOf(asked.choices![0]!.plan)).toBeNull();
    expect(topicOf(asked.choices![1]!.plan)).toBe("견적서");
    expect(asked.choices![1]!.plan.scope.filter?.doc_type ?? null).toBeNull();
    expect(planner.planText("회의록에 대해 알려줘", ctx).choices![1]!.plan.operations[0]).toMatchObject({ op: "answer" });
    // A subject that only contains a type's name is a subject; a type outside a subject is a place.
    for (const text of ["계약서 해지 조건에 대해 요약해줘", "견적서 요약해줘"]) expect(keysOf(planner.planText(text, ctx)), text).toEqual([]);
  });

  it("C: documents of a type are asked about when classified and named differ, and a picked reading runs without another question", async () => {
    const ids = await importDocs(["a.pdf", "한빛_용역계약서_초안.pdf", "b.pdf"]);
    await backend.mockInvoke("documents_set_doc_type", { id: ids["a.pdf"], docType: "dev_contract" });
    const sentence = "개발 계약서 요약해줘";
    const asked = await backend.mockInvoke<PlanResponse>("planner_plan", { text: sentence, context: {} });
    expect(asked.result.plan.intent).toBe("clarify");
    expect(asked.preview).toBeNull();
    expect(keysOf(asked.result)).toEqual(["classified", "named", "both"]);
    expect(choicesOf(asked).map((c) => c.label)).toEqual(["'소프트웨어 개발 계약서'로 분류된 문서 1건", "제목이나 파일 이름에 유형 이름이 들어 있는 문서 1건", "둘 다 2건"]);
    expect(asked.result.plan.clarification_question).toContain("분류된 문서가 1건");

    // The flow of a pick (`AssistantPage.chooseReading`): the ready-made plan is checked and previewed, not planned again.
    const both = choicesOf(asked)[2]!;
    const checked = await backend.mockInvoke<ExecuteResponse>("planner_execute", { args: { plan: both.plan, text: sentence, context: {}, preview_only: true } });
    expect(checked.job_id).toBeNull();
    expect(checked.execution).toBeNull();
    expect(checked.preview!.preview.document_count).toBe(2);
    const response = chosenResponse(asked, checked.preview!);
    expect(response.result.choices).toBeUndefined();
    expect(response.result.plan.scope.filter).toMatchObject({ doc_type: "dev_contract", type_match: "both" });
    expect(response.result.source).toBe(asked.result.source);
    // It is handled like a freshly planned plan: a summary needs no confirmation and runs at once.
    expect(planKind(response.result.plan)).toBe("work");
    expect(requiresConfirmation(response)).toBe(false);
    expect(plannedTextOf(sentence, asked)).toBeUndefined();
    expect(plannedTextOf("다른 문장", asked)).toBe(sentence);
    // Each reading covers what its button says.
    for (const [choice, count] of [
      [choicesOf(asked)[0]!, 1],
      [choicesOf(asked)[1]!, 1],
    ] as const) {
      const res = await backend.mockInvoke<ExecuteResponse>("planner_execute", { args: { plan: choice.plan, text: sentence, context: {}, preview_only: true } });
      expect(res.preview!.preview.document_count, choice.key).toBe(count);
    }
    // A plan that came from a choice is never asked about again.
    expect(planner.typeChoices(both.plan, planner.buildContext(), true)).toBeNull();
    // Nothing ran: the question is still the only thing that happened.
    expect(backend.listJobs().filter((j) => j.kind === "plan")).toEqual([]);

    // One kind of document only: nothing to ask — the condition takes what there is.
    await backend.mockInvoke("documents_set_doc_type", { id: ids["한빛_용역계약서_초안.pdf"], docType: "dev_contract" });
    const plain = await backend.mockInvoke<PlanResponse>("planner_plan", { text: sentence, context: {} });
    expect(plain.result.plan.intent).toBe("summarize");
    expect(keysOf(plain.result)).toEqual([]);
    expect(plain.preview!.document_count).toBe(2);
  });

  it("G: the reason of a step without documents names reference material only when it is what is missing", async () => {
    const ids = await importDocs(["지침.pdf"]);
    const project = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "AAA" } });
    await backend.mockInvoke("project_add_documents", { id: project.id, docIds: [ids["지침.pdf"]], asReference: true });
    const ctx = planner.buildContext();
    const run = async (filter: NonNullable<Plan["scope"]["filter"]>) => {
      const plan: Plan = { intent: "summarize", scope: { folder: null, doc_ids: [], filter }, operations: [{ op: "summarize" }], requires_confirmation: false, confidence: 0.9, clarification_question: null, summary: "s" };
      planner.validatePlan(plan, ctx, null);
      return (await planner.runPlan(plan, ctx, null, () => undefined)).steps[0]!;
    };
    // The project has reference material; a tag no document carries is what empties the scope.
    const tagged = await run({ project_id: project.id, role: "knowledge", tags: ["AAA"] });
    expect(tagged.status).toBe("failed");
    expect(tagged.error).toContain("no_documents");
    expect(tagged.error).not.toContain("no_reference");
    // A named project is where a summary looks, on the rules path too ("AAA 참고자료 요약해줘").
    const named = planner.planText("AAA 참고자료 요약해줘", planner.buildContext({ current_document: ids["지침.pdf"]! }));
    expect(named.plan.operations[0]).toMatchObject({ op: "summarize" });
    expect(named.plan.scope.filter).toMatchObject({ project_id: project.id, role: "knowledge" });
    expect(named.plan.scope.doc_ids ?? []).toEqual([]);
  });
});
