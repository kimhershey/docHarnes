// 0.5.0 참고 자료 (plan §2) in the browser mocks: collections carry `kind` / `project_id`, a project's reference
// group is created on demand and goes with the project, the search / answer scope `project_id` is the union of
// the work documents, the project's group and the active common groups, and the planner's `add_to_project`
// takes `as_reference`.
import { beforeEach, describe, expect, it } from "vitest";

import { defaultImportOptions, type Answer, type Collection, type DocumentView, type ImportOptions, type KnowledgeStatus, type PlanContext, type Project, type SearchResponse, type SearchScope } from "@/lib/types";

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
const pl = await import("@/lib/mock-planner");

const FINISHED = ["done", "failed", "cancelled", "interrupted"];

async function sleep(ms: number): Promise<void> {
  await new Promise((r) => setTimeout(r, ms));
}

async function waitJob(id: string): Promise<void> {
  for (let i = 0; i < 2000; i++) {
    const j = backend.getJob(id);
    if (j && FINISHED.includes(j.state)) return;
    await sleep(5);
  }
  throw new Error(`job ${id} did not finish`);
}

/** Imports `names` (rendered, with the mock's page text) and returns name → id. */
async function importDocs(names: string[], extra: Partial<ImportOptions> = {}): Promise<Record<string, string>> {
  const paths = names.map((n) => `C:\\scan\\${n}`);
  const options = { ...defaultImportOptions(), auto_level: "none", ...extra } as ImportOptions;
  const plan = await backend.mockInvoke<{ token: string }>("documents_import_plan", { args: { paths, options, source: "folder" } });
  const job = await backend.mockInvoke<string>("documents_import_run", { args: { token: plan.token, paths, options, source: "folder" } });
  await waitJob(job);
  const docs = backend.wsDocs();
  return Object.fromEntries(names.map((n) => [n, docs.find((d) => d.original_name === n)!.id]));
}

/** Waits for the import's auto index, then forces one more pass so every document is chunked. */
async function indexAll(): Promise<void> {
  for (let i = 0; i < 2000; i++) {
    if (!backend.listJobs().some((j) => j.kind === "knowledge_index" && !FINISHED.includes(j.state))) break;
    await sleep(5);
  }
  const id = await backend.mockInvoke<string | null>("knowledge_index", { ids: null, force: true });
  if (id) await waitJob(id);
}

async function found(scope?: SearchScope): Promise<Set<string>> {
  const r = await backend.mockInvoke<SearchResponse>("knowledge_search", { req: { query: "합계", limit: 50, per_document: 1, scope } });
  return new Set(r.hits.map((h) => h.document_id));
}

describe("mock reference material (0.5.0)", () => {
  beforeEach(async () => {
    backend.resetMock();
    localStorage.removeItem("doculens.mock.projects");
    localStorage.setItem("doculens.mock.fast", "1");
    await backend.mockInvoke("workspace_create", { args: { path: "C:\\Work\\Docs", name: null, locale: null, force_unlock: false } });
  });

  it("groups: common groups are `reference`, a project's group is created on demand, renamed and deleted with it", async () => {
    const ids = await importDocs(["지침.pdf"]);
    const common = await backend.mockInvoke<Collection>("knowledge_collection_save", { input: { name: "사내 규정" } });
    expect(common).toMatchObject({ kind: "reference", project_id: null, active: true });
    const p = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "한빛 포털" } });
    const group = await backend.mockInvoke<Collection>("project_reference_group", { id: p.id });
    expect(group).toMatchObject({ kind: "project", project_id: p.id, name: "한빛 포털", doc_count: 0 });
    expect((await backend.mockInvoke<Collection>("project_reference_group", { id: p.id })).id).toBe(group.id);
    // A common group may share the project's name; the common names stay unique among themselves.
    await backend.mockInvoke("knowledge_collection_save", { input: { name: "한빛 포털" } });
    await expect(backend.mockInvoke("knowledge_collection_save", { input: { name: "사내 규정" } })).rejects.toMatchObject({ code: "invalid" });
    // Adding with `set_role` marks the document as 참고 자료; the status counts the common groups only.
    await backend.mockInvoke("knowledge_collection_add", { args: { id: group.id, doc_ids: [ids["지침.pdf"]], set_role: true } });
    expect(backend.wsDocs().find((d) => d.id === ids["지침.pdf"])?.role).toBe("knowledge");
    expect((await backend.mockInvoke<DocumentView[]>("knowledge_collection_docs", { id: group.id })).map((d) => d.id)).toEqual([ids["지침.pdf"]]);
    const all = await backend.mockInvoke<Collection[]>("knowledge_collections");
    expect(all.map((c) => c.kind).sort()).toEqual(["project", "reference", "reference"]);
    expect((await backend.mockInvoke<KnowledgeStatus>("knowledge_status")).collections).toBe(2);
    // The group follows the project's name and goes with the project; the document stays.
    await backend.mockInvoke("project_update", { input: { id: p.id, name: "한빛 포털 2차" } });
    expect((await backend.mockInvoke<Collection[]>("knowledge_collections")).find((c) => c.id === group.id)?.name).toBe("한빛 포털 2차");
    await backend.mockInvoke("project_delete", { id: p.id });
    const after = await backend.mockInvoke<Collection[]>("knowledge_collections");
    expect(after.some((c) => c.id === group.id)).toBe(false);
    expect(after.map((c) => c.name).sort()).toEqual(["사내 규정", "한빛 포털"]);
    expect(backend.wsDocs().some((d) => d.id === ids["지침.pdf"])).toBe(true);
    // A project without a group deletes cleanly too.
    const q = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "빈 프로젝트" } });
    expect(await backend.mockInvoke("project_delete", { id: q.id })).toBe(true);
  });

  it("scope: project_id = work documents ∪ the project's group ∪ the active common groups (documents replace the work set)", async () => {
    const ids = await importDocs(["A-업무.pdf", "B-참고.pdf", "C-공통.pdf", "D-비활성.pdf", "E-무관.pdf"]);
    await indexAll();
    const A = ids["A-업무.pdf"]!;
    const B = ids["B-참고.pdf"]!;
    const C = ids["C-공통.pdf"]!;
    const D = ids["D-비활성.pdf"]!;
    const E = ids["E-무관.pdf"]!;
    const p = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "한빛 포털" } });
    await backend.mockInvoke("project_add_documents", { id: p.id, docIds: [A] });
    // 0.7.0 plan §1.1: `asReference` puts the document in the project's group (created on demand) with the 참고 자료 role.
    const asRef = await backend.mockInvoke<{ documents: { document: { id: string } }[] }>("project_add_documents", { id: p.id, docIds: [B], asReference: true });
    expect(asRef.documents.map((d) => d.document.id)).toEqual([A]);
    const group = await backend.mockInvoke<Collection>("project_reference_group", { id: p.id });
    expect(await backend.mockInvoke<DocumentView[]>("knowledge_collection_docs", { id: group.id })).toMatchObject([{ id: B, role: "knowledge" }]);
    const common = await backend.mockInvoke<Collection>("knowledge_collection_save", { input: { name: "사내 규정" } });
    await backend.mockInvoke("knowledge_collection_add", { args: { id: common.id, doc_ids: [C], set_role: true } });
    const off = await backend.mockInvoke<Collection>("knowledge_collection_save", { input: { name: "구 규정", active: false } });
    await backend.mockInvoke("knowledge_collection_add", { args: { id: off.id, doc_ids: [D], set_role: true } });

    expect(await found({ project_id: p.id })).toEqual(new Set([A, B, C]));
    expect(await found({ project_id: p.id, documents: [E] })).toEqual(new Set([E, B, C]));
    // Without a project: everything except the documents whose only group is switched off (an absent scope skips that rule, as the core does).
    expect(await found({})).toEqual(new Set([A, B, C, E]));
    expect(await found()).toEqual(new Set([A, B, C, D, E]));
    expect(await found({ collections: [off.id] })).toEqual(new Set([D]));
    // A work document of the project stays in scope even when its only group is switched off.
    await backend.mockInvoke("knowledge_collection_add", { args: { id: off.id, doc_ids: [A], set_role: false } });
    expect(await found({ project_id: p.id })).toEqual(new Set([A, B, C]));
    expect(backend.wsDocs().find((d) => d.id === A)?.role).toBe("work");
    // Switching the common group on brings its documents into the project scope.
    await backend.mockInvoke("knowledge_collection_save", { input: { id: off.id, name: "구 규정", active: true } });
    expect(await found({ project_id: p.id })).toEqual(new Set([A, B, C, D]));
    // The grounded answer uses the same scope.
    const ans = await backend.mockInvoke<Answer>("knowledge_answer", { req: { question: "E-무관 합계 금액은?", scope: { project_id: p.id }, save: false }, requestId: "t-scope" });
    expect(ans.sources.length).toBeGreaterThan(0);
    for (const s of ans.sources) expect([A, B, C, D]).toContain(s.document_id);
    const none = await backend.mockInvoke<Answer>("knowledge_answer", { req: { question: "E-무관 내용은?", scope: { project_id: p.id, documents: [B] }, save: false }, requestId: "t-scope-2" });
    expect(none.sources.map((s) => s.document_id)).toEqual([]);
  });

  it("planner: '참고 자료로 넣어줘' goes to the project's reference group, a plain '넣어줘' to its work documents", async () => {
    const ids = await importDocs(["지침.pdf", "계약서.pdf"]);
    const p = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "인증 심사" } });
    const ctx = pl.buildContext({ selected_doc_ids: [ids["지침.pdf"]!], current_project: p.id });
    const plan = pl.rulesPlan("선택한 문서를 프로젝트에 참고 자료로 넣어줘", ctx);
    expect(plan?.operations[0]).toMatchObject({ op: "add_to_project", as_reference: true, doc_ids: [ids["지침.pdf"]] });
    const exec = await pl.runPlan(plan!, ctx, null, () => undefined);
    expect(exec.steps[0]?.payload).toMatchObject({ project_id: p.id, added: 1, as_reference: true });
    const group = await backend.mockInvoke<Collection>("project_reference_group", { id: p.id });
    expect((await backend.mockInvoke<DocumentView[]>("knowledge_collection_docs", { id: group.id })).map((d) => d.id)).toEqual([ids["지침.pdf"]]);
    expect(backend.wsDocs().find((d) => d.id === ids["지침.pdf"])?.role).toBe("knowledge");
    expect((await backend.mockInvoke<{ documents: unknown[] }>("project_get", { id: p.id })).documents).toHaveLength(0);

    const ctx2 = pl.buildContext({ selected_doc_ids: [ids["계약서.pdf"]!], current_project: p.id });
    const plain = pl.rulesPlan("선택한 문서를 프로젝트에 넣어줘", ctx2);
    expect(plain?.operations[0]).toMatchObject({ op: "add_to_project", as_reference: false });
    const exec2 = await pl.runPlan(plain!, ctx2, null, () => undefined);
    expect(exec2.steps[0]?.payload).toMatchObject({ project_id: p.id, added: 1, as_reference: false });
    expect((await backend.mockInvoke<{ documents: unknown[] }>("project_get", { id: p.id })).documents).toHaveLength(1);
  });

  // 1.0.5 (user report "프로젝트에 문서를 추가한 뒤에도 프로젝트 문서가 0개로 표시"): the count on the cards is what the
  // project holds — work documents + reference material, each once — as `repo::fill_counters` counts it.
  it("counts: a project's document_count includes its reference material; reference_count is the reference-only part", async () => {
    const ids = await importDocs(["지침.pdf", "계약서.pdf", "제안서.pdf", "회의록.pdf"]);
    const [d1, d2, d3, d4] = [ids["지침.pdf"]!, ids["계약서.pdf"]!, ids["제안서.pdf"]!, ids["회의록.pdf"]!];
    const a = await backend.mockInvoke<Project>("project_create", { input: { name: "AAA" } });
    const b = await backend.mockInvoke<Project>("project_create", { input: { name: "BBB" } });
    /** `[document_count, reference_count]` of the list's card, checked against the project read alone. */
    const counts = async (id: string): Promise<[number, number | undefined]> => {
      const listed = (await backend.mockInvoke<Project[]>("projects_list")).find((p) => p.id === id)!;
      const one = (await backend.mockInvoke<{ project: Project }>("project_get", { id })).project;
      expect([one.document_count, one.reference_count]).toEqual([listed.document_count, listed.reference_count]);
      return [listed.document_count, listed.reference_count];
    };
    const assign = (add: { doc_id: string; project_id: string; reference: boolean }[]) => backend.mockInvoke<{ moved: number; undo: unknown }>("project_assign", { req: { remove: [], add } });
    expect([await counts(a.id), await counts(b.id)]).toEqual([[0, 0], [0, 0]]);

    // Reference material only: it counts (the card said "문서 0건").
    await backend.mockInvoke("project_add_documents", { id: a.id, docIds: [d1], asReference: true });
    expect((await backend.mockInvoke<{ documents: unknown[] }>("project_get", { id: a.id })).documents).toHaveLength(0);
    expect(await counts(a.id)).toEqual([1, 1]);
    expect(await counts(b.id)).toEqual([0, 0]);

    // Work documents next to it.
    await backend.mockInvoke("project_add_documents", { id: a.id, docIds: [d2, d3] });
    expect(await counts(a.id)).toEqual([3, 1]);

    // A work document that is in the reference group as well counts once — as a work document.
    const group = await backend.mockInvoke<Collection>("project_reference_group", { id: a.id });
    await backend.mockInvoke("knowledge_collection_add", { args: { id: group.id, doc_ids: [d2], set_role: false } });
    expect((await backend.mockInvoke<DocumentView[]>("knowledge_collection_docs", { id: group.id })).map((d) => d.id).sort()).toEqual([d1, d2].sort());
    expect(await counts(a.id)).toEqual([3, 1]);

    // Moving a work document to reference material keeps the total, and so does moving it back.
    const moved = await assign([{ doc_id: d3, project_id: a.id, reference: true }]);
    expect(moved.moved).toBe(1);
    expect(await counts(a.id)).toEqual([3, 2]);
    await backend.mockInvoke("project_assign", { req: moved.undo });
    expect(await counts(a.id)).toEqual([3, 1]);

    // The same document as reference material of two projects counts in each.
    await assign([{ doc_id: d1, project_id: b.id, reference: true }, { doc_id: d4, project_id: b.id, reference: false }]);
    expect([await counts(a.id), await counts(b.id)]).toEqual([[3, 1], [2, 1]]);

    // The assistant's cards: deleting a project says how many documents it holds; a check reads the work documents.
    const ctx: PlanContext = pl.buildContext({ current_project: a.id });
    const scope = { doc_ids: [], folder: null, filter: {} };
    const preview = (operations: unknown[]) => pl.planPreview({ intent: "command", scope, operations, requires_confirmation: false, summary: "" } as unknown as Parameters<typeof pl.planPreview>[0], ctx);
    expect(preview([{ op: "delete_project", project: a.id }]).steps[0]).toMatchObject({ payload: { documents: 3 } });
    expect(preview([{ op: "delete_project", project: a.id }]).steps[0]?.summary).toContain("문서 3건");
    expect(preview([{ op: "check_consistency", project: a.id, rules: [] }]).steps[0]?.summary).toContain("문서 2건");

    // A deleted document is not counted on either side.
    await backend.mockInvoke("documents_delete", { ids: [d1] });
    expect([await counts(a.id), await counts(b.id)]).toEqual([[2, 0], [1, 0]]);
  });
});
