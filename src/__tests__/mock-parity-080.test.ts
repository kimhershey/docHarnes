// 0.8.0 backend rules the browser mocks mirror (the Rust core is the source of truth; each test names the
// rule it follows): forced OCR and hand-corrected pages, the last `general` form, attach-only imports,
// project phases / classification targets / partial checks / supplement errors / the manual purge, superseded
// editions, pinned download addresses, Korean particles and the scope of a question.
import { beforeEach, describe, expect, it } from "vitest";

import { defaultImportOptions, type Answer, type CheckRun, type ClassifyResult, type Collection, type ImportOptions, type ImportReport, type MaintenanceStatus, type PageView, type ProjectDetail, type Rule, type SchemaRecord, type SearchResponse, type SearchScope } from "@/lib/types";

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
const pipeline = await import("@/lib/mock-pipeline");
const projects = await import("@/lib/mock-projects");
const knowledge = await import("@/lib/mock-knowledge");

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

/** Imports `names` (rendered, with the mock's page text, no automatic processing) and returns name → id. */
async function importDocs(names: string[], extra: Partial<ImportOptions> = {}): Promise<Record<string, string>> {
  const paths = names.map((n) => `C:\\scan\\${n}`);
  const options = { ...defaultImportOptions(), auto_level: "none", ...extra } as ImportOptions;
  const plan = await backend.mockInvoke<{ token: string }>("documents_import_plan", { args: { paths, options, source: "folder" } });
  const job = await backend.mockInvoke<string>("documents_import_run", { args: { token: plan.token, paths, options, source: "folder" } });
  await waitJob(job);
  const docs = backend.wsDocs();
  return Object.fromEntries(names.map((n) => [n, docs.find((d) => d.original_name === n)!.id]));
}

async function indexAll(): Promise<void> {
  for (let i = 0; i < 2000; i++) {
    if (!backend.listJobs().some((j) => j.kind === "knowledge_index" && !FINISHED.includes(j.state))) break;
    await sleep(5);
  }
  const id = await backend.mockInvoke<string | null>("knowledge_index", { ids: null, force: true });
  if (id) await waitJob(id);
}

async function found(query: string, scope?: SearchScope): Promise<Set<string>> {
  const r = await backend.mockInvoke<SearchResponse>("knowledge_search", { req: { query, limit: 50, per_document: 1, scope } });
  return new Set(r.hits.map((h) => h.document_id));
}

const process = (req: Parameters<typeof pipeline.runProcess>[0]) => pipeline.runProcess(req, null, () => undefined);
const pages = (id: string) => backend.mockInvoke<PageView[]>("documents_pages", { id, withText: true });

beforeEach(async () => {
  backend.resetMock();
  for (const key of ["doculens.mock.projects", "doculens.mock.knowledge", "doculens.mock.pipeline", "doculens.mock.maintenance"]) localStorage.removeItem(key);
  localStorage.setItem("doculens.mock.fast", "1");
  await backend.mockInvoke("workspace_create", { args: { path: "C:\\Work\\Docs", name: null, locale: null, force_unlock: false } });
});

describe("pipeline (pipeline::run, repo::keep_general)", () => {
  it("a forced run reads OCR pages again but keeps a page corrected by hand unless `reocr_manual` goes with it", async () => {
    const ids = await importDocs(["견적서-2p.jpg"]);
    const id = ids["견적서-2p.jpg"]!;
    await process({ document_ids: [id], level: "ocr" });
    const before = await pages(id);
    expect(before.length).toBeGreaterThan(0);
    expect(before[0]!.text_source).toBe("ocr");
    await backend.mockInvoke("documents_set_page_text", { id, pageNo: 1, text: "손으로 고친 글" });

    // force alone: the correction stays (no page of this document is read again when every other page is OCR output too → they are).
    const forced = await process({ document_ids: [id], level: "ocr", force: true });
    const kept = await pages(id);
    expect(kept[0]).toMatchObject({ text_source: "manual", text: "손으로 고친 글" });
    expect(forced.ocr_pages).toBe(before.length - 1);

    // force + reocr_manual: the correction is replaced by the new OCR result.
    const reread = await process({ document_ids: [id], level: "ocr", force: true, reocr_manual: true });
    const replaced = await pages(id);
    expect(replaced[0]!.text_source).toBe("ocr");
    expect(replaced[0]!.text).not.toBe("손으로 고친 글");
    expect(reread.ocr_pages).toBe(before.length);

    // A page the user emptied by hand has nothing to lose: force reads it again.
    await backend.mockInvoke("documents_set_page_text", { id, pageNo: 1, text: "" });
    await process({ document_ids: [id], level: "ocr", force: true });
    expect((await pages(id))[0]!.text_source).toBe("ocr");
    // Without force nothing is read again, whatever the flag says.
    await backend.mockInvoke("documents_set_page_text", { id, pageNo: 1, text: "다시 고침" });
    expect((await process({ document_ids: [id], level: "ocr", reocr_manual: true })).ocr_pages).toBe(0);
    expect((await pages(id))[0]).toMatchObject({ text_source: "manual", text: "다시 고침" });
  });

  it("the last enabled `general` form can neither be switched off nor deleted nor moved to another type", async () => {
    const schemas = await backend.mockInvoke<SchemaRecord[]>("pipeline_schemas");
    const general = schemas.find((s) => s.doc_type === "general" && s.enabled)!;
    expect(schemas.filter((s) => s.doc_type === "general" && s.enabled)).toHaveLength(1);
    const refused = { code: "invalid", message: "schema_general_required" };
    await expect(backend.mockInvoke("pipeline_schema_set_enabled", { id: general.id, enabled: false })).rejects.toMatchObject(refused);

    // A second enabled `general` form (a user's copy) unlocks either of them …
    const copy = await backend.mockInvoke<SchemaRecord>("pipeline_schema_save", { draft: { id: null, name: "일반 (사본)", name_en: null, doc_type: "general", source: "user", description_ko: null, description_en: null, definition: general.definition, form_document_id: null, note: null } });
    await backend.mockInvoke("pipeline_schema_set_enabled", { id: general.id, enabled: false });
    // … and then the copy is the last one: no delete, no switch-off, no other type.
    await expect(backend.mockInvoke("pipeline_schema_delete", { id: copy.id })).rejects.toMatchObject(refused);
    await expect(backend.mockInvoke("pipeline_schema_set_enabled", { id: copy.id, enabled: false })).rejects.toMatchObject(refused);
    await expect(backend.mockInvoke("pipeline_schema_save", { draft: { id: copy.id, name: copy.name, name_en: null, doc_type: "memo", source: "user", description_ko: null, description_en: null, definition: copy.definition, form_document_id: null, note: null } })).rejects.toMatchObject(refused);
    // With the built-in one back on, the copy goes.
    await backend.mockInvoke("pipeline_schema_set_enabled", { id: general.id, enabled: true });
    expect(await backend.mockInvoke("pipeline_schema_delete", { id: copy.id })).toBe(true);
  });
});

describe("import (documents_import_run, ImportReport.existing)", () => {
  it("a batch of registered files runs when it has a project / group to put them into, and names them in `existing`", async () => {
    const ids = await importDocs(["계약서.pdf", "부속서.pdf"]);
    const paths = ["C:\\scan\\계약서.pdf", "C:\\scan\\부속서.pdf"];
    const project = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "드롭 프로젝트" } });

    // No target: nothing to import and nothing to attach — refused as before.
    const plain = await backend.mockInvoke<{ token: string; new_count: number; duplicate_count: number }>("documents_import_plan", { args: { paths, options: defaultImportOptions(), source: "drop" } });
    expect(plain).toMatchObject({ new_count: 0, duplicate_count: 2 });
    await expect(backend.mockInvoke("documents_import_run", { args: { token: plain.token, paths, options: defaultImportOptions(), source: "drop" } })).rejects.toMatchObject({ code: "invalid", message: "nothing to import" });

    // A project: the job only attaches the registered documents.
    const options = { ...defaultImportOptions(), project_id: project.id };
    const plan = await backend.mockInvoke<{ token: string }>("documents_import_plan", { args: { paths, options, source: "drop" } });
    const jobId = await backend.mockInvoke<string>("documents_import_run", { args: { token: plan.token, paths, options, source: "drop" } });
    await waitJob(jobId);
    const job = backend.getJob(jobId)!;
    expect(job).toMatchObject({ state: "done", subject: "2 documents" });
    const report = job.result as ImportReport;
    expect(report.imported).toEqual([]);
    expect([...(report.existing ?? [])].sort()).toEqual([ids["계약서.pdf"], ids["부속서.pdf"]].sort());
    const detail = await backend.mockInvoke<ProjectDetail>("project_get", { id: project.id });
    expect(detail.documents.map((d) => d.document.id).sort()).toEqual([ids["계약서.pdf"], ids["부속서.pdf"]].sort());

    // A mixed batch into a reference group: imported ∪ existing join it.
    const group = await backend.mockInvoke<Collection>("knowledge_collection_save", { input: { name: "규정" } });
    const mixed = ["C:\\scan\\계약서.pdf", "C:\\scan\\취업규칙.pdf"];
    const groupOptions = { ...defaultImportOptions(), role: "knowledge" as const, collection_id: group.id };
    const mixedPlan = await backend.mockInvoke<{ token: string }>("documents_import_plan", { args: { paths: mixed, options: groupOptions, source: "drop" } });
    const mixedJob = await backend.mockInvoke<string>("documents_import_run", { args: { token: mixedPlan.token, paths: mixed, options: groupOptions, source: "drop" } });
    await waitJob(mixedJob);
    const mixedReport = backend.getJob(mixedJob)!.result as ImportReport;
    expect(mixedReport.imported).toHaveLength(1);
    expect(mixedReport.existing).toEqual([ids["계약서.pdf"]]);
    const members = await backend.mockInvoke<{ id: string }[]>("knowledge_collection_docs", { id: group.id });
    expect(members.map((d) => d.id).sort()).toEqual([...mixedReport.imported, ids["계약서.pdf"]!].sort());
  });
});

describe("projects (projects::phase_for, classify, check, report, maintenance)", () => {
  it("a project is 완료 from its end date on, and 기한 경과 only once that date has passed", () => {
    const today = "2026-11-11";
    expect(projects.phaseFor("2026-11-01", null, "2026-11-01", today)).toBe("active");
    expect(projects.phaseFor("2026-11-01", "2026-11-12", "2026-11-01", today)).toBe("active");
    // "완료 처리" stores today as the end date: 완료 at once …
    expect(projects.phaseFor("2026-11-01", today, "2026-11-01", today)).toBe("done");
    expect(projects.phaseFor("2026-11-01", "2026-11-10", "2026-11-01", today)).toBe("done");
    expect(projects.phaseFor("2026-12-01", null, "2026-11-01", today)).toBe("planned");
    // … but not overdue on that day, whatever is still open.
    const open = { phase: "done" as const, missing_count: 2, last_mismatch: 1, last_check_at: null };
    expect(projects.overdueFor({ ...open, end_date: today }, today)).toBe(false);
    expect(projects.overdueFor({ ...open, end_date: "2026-11-10" }, today)).toBe(true);
    expect(projects.overdueFor({ phase: "done", end_date: "2026-11-10", missing_count: 0, last_mismatch: 0, last_check_at: "2026-11-09T00:00:00Z" }, today)).toBe(false);
  });

  it("classification targets by default are the work documents outside every reference group", async () => {
    const ids = await importDocs(["견적서_한빛.pdf", "취업규칙.pdf", "계약서_두리.pdf", "보안지침.pdf"]);
    await backend.mockInvoke("project_create", { input: { name: "한빛 포털" } });
    // 취업규칙: a document of a reference group that kept its 업무 role; 보안지침: reference material by role.
    const group = await backend.mockInvoke<Collection>("knowledge_collection_save", { input: { name: "규정" } });
    await backend.mockInvoke("knowledge_collection_add", { args: { id: group.id, doc_ids: [ids["취업규칙.pdf"]], set_role: false } });
    await backend.mockInvoke("knowledge_set_role", { ids: [ids["보안지침.pdf"]], role: "knowledge" });
    expect(knowledge.referenceDocIds()).toEqual(new Set([ids["취업규칙.pdf"]]));

    const targets = (r: ClassifyResult) => r.suggestions.map((s) => s.doc_id).sort();
    const work = [ids["견적서_한빛.pdf"]!, ids["계약서_두리.pdf"]!].sort();
    expect(targets(await backend.mockInvoke<ClassifyResult>("project_classify_preview", { req: {} }))).toEqual(work);
    expect(targets(await backend.mockInvoke<ClassifyResult>("project_classify_preview", { req: { include_assigned: true } }))).toEqual(work);
    // Named documents are classified whatever they are.
    expect(targets(await backend.mockInvoke<ClassifyResult>("project_classify_preview", { req: { doc_ids: [ids["취업규칙.pdf"]] } }))).toEqual([ids["취업규칙.pdf"]]);
  });

  it("a check over some of the rules is returned as `partial` and not stored; evidence of a document without an extraction says `not_extracted`", async () => {
    const ids = await importDocs(["견적서_한빛.pdf", "개발계약서_한빛.pdf"]);
    const estimate = ids["견적서_한빛.pdf"]!;
    const contract = ids["개발계약서_한빛.pdf"]!;
    // Classified, not extracted: the documents have a type but no fields.
    await process({ document_ids: [estimate, contract], level: "classify" });
    const project = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "한빛 포털" } });
    await backend.mockInvoke("project_add_documents", { id: project.id, docIds: [estimate, contract] });
    const rule = (id: string, left: string): Rule => ({ id, description: id, left: { doc_type: left, field: "total", value: null }, op: "present", right: null, tolerance: null, required: true, enabled: true });
    const typeOf = (id: string) => backend.wsDocs().find((d) => d.id === id)!.doc_type!;
    await backend.mockInvoke("project_save_rule", { id: project.id, rule: { ...rule("r-estimate", typeOf(estimate)), op: "eq", right: { doc_type: typeOf(contract), field: "amount", value: null } } });
    await backend.mockInvoke("project_save_rule", { id: project.id, rule: rule("r-missing", "acceptance") });

    const partial = await backend.mockInvoke<CheckRun>("project_check", { id: project.id, ruleIds: ["r-estimate"] });
    expect(partial.partial).toBe(true);
    expect(partial.results).toHaveLength(1);
    expect(partial.results[0]).toMatchObject({ verdict: "undetermined", left: { status: "not_extracted", doc_id: estimate } });
    expect(partial.results[0]!.message).toContain("아직 추출하지 않은 문서");
    expect(await backend.mockInvoke<CheckRun[]>("project_runs", { id: project.id })).toEqual([]);
    expect((await backend.mockInvoke<ProjectDetail>("project_get", { id: project.id })).last_run).toBeNull();

    // The full check is stored and has no `partial` mark; a rule on a type without a document names that.
    const full = await backend.mockInvoke<CheckRun>("project_check", { id: project.id, ruleIds: [] });
    expect(full.partial).toBeUndefined();
    expect(full.results.find((r) => r.rule_id === "r-missing")).toMatchObject({ verdict: "missing", left: { status: "missing_document" } });
    expect(await backend.mockInvoke<CheckRun[]>("project_runs", { id: project.id })).toHaveLength(1);

    // 보완요청서: only undetermined results selected / ids of another run / nothing selected and nothing to request.
    const undetermined = full.results.find((r) => r.rule_id === "r-estimate")!;
    expect(() => projects.supplementRequest(project.id, [undetermined.id], null)).toThrowError(expect.objectContaining({ message: "invalid value: findings_not_requestable" }));
    expect(() => projects.supplementRequest(project.id, [partial.results[0]!.id], null)).toThrowError(expect.objectContaining({ message: "invalid value: findings_not_in_run" }));
    await backend.mockInvoke("project_delete_rule", { id: project.id, ruleId: "r-missing" });
    await backend.mockInvoke<CheckRun>("project_check", { id: project.id, ruleIds: [] });
    expect(() => projects.supplementRequest(project.id, [], null)).toThrowError(expect.objectContaining({ message: "invalid value: no_findings" }));
  });

  it("'지금 파기 실행' purges with the daily switch off", async () => {
    const ws = await backend.mockInvoke<{ manifest: { retention: Record<string, unknown> } }>("workspace_current");
    await backend.mockInvoke("workspace_set_retention", { retention: { ...ws.manifest.retention, purge_enabled: false } });
    expect((await backend.mockInvoke<MaintenanceStatus>("maintenance_status")).enabled).toBe(false);
    const { job_id } = await backend.mockInvoke<{ job_id: string }>("maintenance_run_now");
    await waitJob(job_id);
    expect(backend.getJob(job_id)!.state).toBe("done");
    const summary = (await backend.mockInvoke<MaintenanceStatus>("maintenance_status")).last_summary!;
    expect(summary.skipped).not.toContain("disabled");
  });

  it("organising a project leaves a document that is filed there as a copy where it is (`project_copy`)", async () => {
    const ids = await importDocs(["요구사항정의서.pdf"]);
    const srs = ids["요구사항정의서.pdf"]!;
    const a = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "A 사업" } });
    const b = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "B 사업" } });
    await backend.mockInvoke("project_add_documents", { id: a.id, docIds: [srs] });
    await backend.mockInvoke("project_add_documents", { id: b.id, docIds: [srs] });
    expect(projects.copiedDocIds(b.id)).toEqual(new Set([srs]));
    expect(projects.copiedDocIds(a.id)).toEqual(new Set());

    const request = (projectId: string) => ({ scope: { filter: { project_id: projectId } }, folder_template: "archive/{{project}}", only_reviewed: false });
    const inB = planner.organizePlan(request(b.id));
    expect(inB).toMatchObject({ moves: 0, unchanged: 1 });
    expect(inB.items[0]).toMatchObject({ doc_id: srs, status: "unchanged", reason: "project_copy", to: null });
    // In its first project the document is the original: it moves.
    expect(planner.organizePlan(request(a.id)).items[0]).toMatchObject({ doc_id: srs, status: "move", to: "archive/A 사업/요구사항정의서.pdf" });
  });
});

describe("knowledge (repo::scope_doc_ids, clear_version, answer keep_chunks)", () => {
  it("a superseded edition is hidden only while its group still has a live latest edition", async () => {
    const ids = await importDocs(["지침-v1.pdf", "지침-v2.pdf"]);
    await indexAll();
    const v1 = ids["지침-v1.pdf"]!;
    const v2 = ids["지침-v2.pdf"]!;
    await backend.mockInvoke("knowledge_version_set", { input: { document_id: v1, group_key: "보안 지침", version_label: "v1", effective_date: "2025-01-01", is_latest: false } });
    await backend.mockInvoke("knowledge_version_set", { input: { document_id: v2, group_key: "보안 지침", version_label: "v2", effective_date: "2026-01-01", is_latest: true } });
    expect(await found("합계", { latest_only: true })).toEqual(new Set([v2]));
    expect(await found("합계", { latest_only: false })).toEqual(new Set([v1, v2]));

    // The latest edition is deleted with its document: the edition that is left is all there is.
    await backend.mockInvoke("documents_delete", { ids: [v2] });
    expect(await found("합계", { latest_only: true })).toEqual(new Set([v1]));
  });

  it("clearing the latest edition's version promotes the newest one that is left", async () => {
    const ids = await importDocs(["지침-v1.pdf", "지침-v2.pdf", "지침-v3.pdf"]);
    await indexAll();
    const [v1, v2, v3] = [ids["지침-v1.pdf"]!, ids["지침-v2.pdf"]!, ids["지침-v3.pdf"]!];
    for (const [id, date, latest] of [[v1, "2024-01-01", false], [v2, "2025-01-01", false], [v3, "2026-01-01", true]] as const) {
      await backend.mockInvoke("knowledge_version_set", { input: { document_id: id, group_key: "보안 지침", version_label: null, effective_date: date, is_latest: latest } });
    }
    expect(await backend.mockInvoke("knowledge_version_clear", { documentId: v3 })).toBe(true);
    const groups = await backend.mockInvoke<{ group_key: string; documents: { document_id: string; is_latest: boolean }[] }[]>("knowledge_versions");
    expect(groups[0]!.documents.map((d) => [d.document_id, d.is_latest])).toEqual([[v2, true], [v1, false]]);
    // v3 is an ordinary document again, v2 the current edition, v1 behind it.
    expect(await found("합계", { latest_only: true })).toEqual(new Set([v2, v3]));
  });

  it("evidence kept from the previous answer counts only inside the scope of the new request (KNO-1)", async () => {
    const ids = await importDocs(["견적서_한빛.pdf", "계약서_두리.pdf"]);
    await indexAll();
    const estimate = ids["견적서_한빛.pdf"]!;
    const contract = ids["계약서_두리.pdf"]!;
    const first = await backend.mockInvoke<Answer>("knowledge_answer", { req: { question: "합계 금액은?", scope: { documents: [estimate] }, save: false }, requestId: "t-keep-1" });
    expect(first.sources.length).toBeGreaterThan(0);
    const keep = first.sources.map((s) => s.chunk_id);
    // Same scope: the kept passages lead.
    const again = await backend.mockInvoke<Answer>("knowledge_answer", { req: { question: "납품 기한은?", scope: { documents: [estimate] }, keep_chunks: keep, save: false }, requestId: "t-keep-2" });
    expect(again.sources.map((s) => s.chunk_id)).toEqual(expect.arrayContaining(keep));
    // Another document: nothing of the estimate is cited, and a question it cannot answer ends in "no evidence".
    const other = await backend.mockInvoke<Answer>("knowledge_answer", { req: { question: "합계 금액은?", scope: { documents: [contract] }, keep_chunks: keep, save: false }, requestId: "t-keep-3" });
    for (const s of other.sources) expect(s.document_id).toBe(contract);
    const none = await backend.mockInvoke<Answer>("knowledge_answer", { req: { question: "zzqqxx", scope: { documents: [contract] }, keep_chunks: keep, save: false }, requestId: "t-keep-4" });
    expect(none).toMatchObject({ no_evidence: true, sources: [] });
  });

  it("a text-only request longer than the model's context is read from its beginning and marked `truncated: input`", async () => {
    const short = await backend.mockInvoke<Answer>("knowledge_answer", { req: { question: "다음 문장을 다듬어줘: 금번 과업은 당사가 수행함.", retrieve: false, save: false }, requestId: "t-cut-1" });
    expect(short.truncated).toBeUndefined();
    const long = await backend.mockInvoke<Answer>("knowledge_answer", { req: { question: `다음 문장을 다듬어줘: ${"가나다라마바사아자차 ".repeat(1200)}`, retrieve: false, save: false }, requestId: "t-cut-2" });
    expect(long.truncated).toBe("input");
    expect(long.text.length).toBeLessThanOrEqual(knowledge.TEXT_ONLY_CHARS);
  });
});

describe("models (ModelFile::download_url)", () => {
  it("the required-files text names the pinned address of a registry file", async () => {
    const registry = await backend.mockInvoke<{ models: { id: string; files: { name: string; hf: string | null; url?: string | null }[] }[] }>("registry_list");
    const entry = registry.models.find((m) => m.files.some((f) => f.url))!;
    const text = await backend.mockInvoke<string>("registry_required_files_text", { modelId: entry.id });
    for (const f of entry.files.filter((x) => x.url)) {
      expect(text).toContain(`url: ${f.url}`);
      expect(f.url).not.toContain("/resolve/main/");
    }
    expect(text).not.toContain("/resolve/main/");
  });
});

describe("planner (rules, validate, guard)", () => {
  it("a retention period in days is read: '30일', '30 days', '30d' — not '30 documents'", () => {
    const ctx = planner.buildContext();
    const days = (text: string) => {
      const op = planner.rulesPlan(text, ctx)?.operations[0];
      return op && op.op === "set_retention" ? op.days : null;
    };
    expect(days("대화 기록 30일만 보관")).toBe(30);
    expect(days("대화 기록 보관 기간을 90 일로")).toBe(90);
    expect(days("keep chat history for 30 days")).toBe(30);
    expect(days("keep chat history 14d")).toBe(14);
    expect(days("대화 기록 무기한 보관")).toBe(-1);
    // A number that is not a number of days starts no retention change.
    expect(days("keep chat history of 30 documents")).toBeNull();
  });

  it("the mock's own sentences name their particles by the word in front of them", async () => {
    const ids = await importDocs(["견적서_한빛.pdf"]);
    const ctx = planner.buildContext({ selected_doc_ids: [ids["견적서_한빛.pdf"]!] });
    const summary = (text: string) => planner.rulesPlan(text, ctx)?.summary ?? "";
    expect(summary("선택한 문서 태그 '긴급' 달아줘")).toContain("태그 긴급을 추가합니다");
    expect(summary("'사규' 참고 자료 묶음 만들어")).toContain("묶음 '사규'를 만듭니다");
    expect(planner.planText("/테마 다크", ctx).plan.summary).toBe("테마를 dark(으)로 바꿉니다");
    // No sentence of a plan keeps a written form after a Korean word or a number.
    for (const text of ["견적서 월별 폴더로 옮겨", "선택한 문서 삭제해줘", "견적서 엑셀로 내보내", "inbox 처리해줘"]) {
      expect(summary(text), text).not.toMatch(/[가-힣0-9]['"’”)]?(?:을\(를\)|이\(가\)|은\(는\)|\(으\)로)/u);
    }
  });

  it("a question keeps the scope it names: a type named as the place to look, a project, a date (KNO-5)", async () => {
    await importDocs(["회의록_10월.pdf", "견적서_한빛.pdf"]);
    const project = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "한빛 포털" } });
    const ctx = planner.buildContext();
    const plan = (text: string) => planner.planText(text, ctx).plan;

    const minutes = plan("회의록에서 결정된 사항은?");
    expect(minutes.operations[0]).toMatchObject({ op: "answer" });
    expect(minutes.scope.filter).toMatchObject({ doc_type: "meeting_minutes" });
    expect(plan("견적서 중에서 합계가 가장 큰 것은?").scope.filter).toMatchObject({ doc_type: "estimate" });
    expect(plan("what was decided in the meeting minutes?").scope.filter).toMatchObject({ doc_type: "meeting_minutes" });
    // The type a question is about is not a filter on the documents of that type.
    expect(plan("견적서 내용은?").scope.filter?.doc_type ?? null).toBeNull();
    expect(plan("회의록이 뭐야?").scope.filter?.doc_type ?? null).toBeNull();

    const inProject = plan("한빛 포털 프로젝트 문서에서 납품 기한 알려줘");
    expect(inProject.operations[0]).toMatchObject({ op: "answer" });
    expect(inProject.scope.filter).toMatchObject({ project_id: project.id });

    const dated = plan("지난달 회의록에서 결정된 사항은?");
    expect(dated.scope.filter).toMatchObject({ doc_type: "meeting_minutes" });
    expect(dated.scope.filter?.date_from).toMatch(/^\d{4}-\d{2}-01$/);
    expect(dated.scope.filter?.date_to).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    // Small talk and a sentence that brings its own text look nothing up: no scope.
    expect(plan("넌 누구야?").scope.filter?.doc_type ?? null).toBeNull();
  });
});
