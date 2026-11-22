// 0.8.0 backend rules the browser mocks mirror (the Rust core is the source of truth; each test names the
// rule it follows): forced OCR and hand-corrected pages, the last `general` form, attach-only imports,
// project phases / classification targets / partial checks / supplement errors / the manual purge, superseded
// editions, pinned download addresses, Korean particles and the scope of a question.
import { beforeEach, describe, expect, it } from "vitest";

import { defaultImportOptions, type Answer, type CheckRun, type ClassifyResult, type Collection, type ImportOptions, type ImportReport, type MaintenanceStatus, type PageView, type Plan, type ProjectDetail, type Rule, type SchemaRecord, type SearchResponse, type SearchScope } from "@/lib/types";

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

type EditRequest = import("@/lib/types").EditReplacement;
type EditOutcome = import("@/lib/types").EditReport;

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

  it("a greeting with a question mark is left to the model, which answers it as small talk (`fastpath::greeting`)", () => {
    const ctx = planner.buildContext();
    // Nothing but a greeting: the fast path replies itself, without the model.
    for (const text of ["안녕", "안녕하세요.", "고마워!"]) {
      const r = planner.planText(text, ctx, { model: false });
      expect(r.source, text).toBe("fast");
      expect(r.plan.operations[0], text).toMatchObject({ op: "answer", mode: "chat" });
      expect((r.plan.operations[0] as { reply?: string }).reply, text).toBeTruthy();
    }
    // "안녕?" is not "nothing but a greeting" for the core (it keeps `?` / `？`): no fast path …
    for (const text of ["안녕?", "안녕하세요？"]) {
      const r = planner.planText(text, ctx);
      expect(r.source, text).toBe("model");
      expect(r.route, text).toBe("chat");
      expect(r.plan.operations[0], text).toMatchObject({ op: "answer", mode: "chat", knowledge: false });
      // … so the reply is the model's, not a ready one.
      expect((r.plan.operations[0] as { reply?: string }).reply, text).toBeUndefined();
      // Without a model such a sentence cannot be answered by the fast path either.
      expect(planner.planText(text, ctx, { model: false }).issues.map((i) => i.code), text).toContain("no_model");
    }
  });

  it("an answer plan says what it leaves out: '… 알려주고 메일로 보내줘' carries `unmet` and the `unmet_request` note", () => {
    const ctx = planner.buildContext();
    const r = planner.planText("위약금 조항 알려주고 메일로 보내줘", ctx);
    expect(r.plan.operations.map((o) => o.op)).toEqual(["answer"]);
    expect(r.plan.unmet).toContain("메일");
    const note = r.issues.find((i) => i.code === "unmet_request");
    expect(note).toMatchObject({ blocking: false });
    expect(note!.message).toContain("메일");
    // A question that leaves nothing out has no note.
    const plain = planner.planText("위약금 조항 알려줘", ctx);
    expect(plain.plan.unmet ?? null).toBeNull();
    expect(plain.issues.map((i) => i.code)).not.toContain("unmet_request");
  });

  it("a settings change whose value the sentence does not name waits for approval — numbers, themes, switches (`guard::contract_checks`)", () => {
    const ctx = planner.buildContext();
    const read = (text: string) => {
      const r = planner.planText(text, ctx);
      return { ops: r.plan.operations, confirm: r.plan.requires_confirmation, notes: r.issues.filter((i) => i.code === "ungrounded_change").map((i) => i.message) };
    };
    // A vague wish: the (simulated) model picks a theme on its own.
    const vague = read("눈이 편한 화면으로 해줘");
    // (1.0.0 §4-2: a settings change is `set_setting`, whichever path planned it — core `Op::canonical`.)
    expect(vague.ops).toEqual([{ op: "set_setting", key: "ui.theme", value: "dark" }]);
    expect(vague.confirm).toBe(true);
    expect(vague.notes).toHaveLength(1);
    expect(vague.notes[0]).toContain("ui.theme → dark");
    // The number case as before.
    expect(read("글자를 더 크게 해줘")).toMatchObject({ confirm: true, notes: [expect.stringContaining("ui.scale_percent")] });
    // Named values and switches the sentence says are grounded: no note, no approval.
    for (const text of ["다크 모드로 바꾸고 언어를 영어로", "시스템 테마로 바꾸고 언어를 한국어로"]) {
      const said = read(text);
      expect(said.ops.map((o) => (o.op === "set_setting" ? o.key : o.op)), text).toEqual(["ui.theme", "ui.language"]);
      expect(said.notes, text).toEqual([]);
      expect(said.confirm, text).toBe(false);
    }
  });

  // 1.0.3 (user decision 2026-11-22 — "파일별 요약은 없애고 모든 파일의 내용을 하나의 결과로 요약"): a summary step
  // makes one summary of everything in scope, or one per group — never one per document.
  it("a summary is one result of the documents in scope — or one per group — and none per document (`exec::summarize`)", async () => {
    const ids = await importDocs(["견적서_한빛.pdf", "계약서_한빛.pdf", "지침.pdf"]);
    await indexAll();
    const [estimate, contract, guide] = [ids["견적서_한빛.pdf"]!, ids["계약서_한빛.pdf"]!, ids["지침.pdf"]!];
    const a = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "한빛 포털" } });
    const b = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "새봄 ERP" } });
    await backend.mockInvoke("project_add_documents", { id: a.id, docIds: [estimate, contract] });
    // The contract belongs to both projects.
    await backend.mockInvoke("project_add_documents", { id: b.id, docIds: [contract] });
    const ctx = planner.buildContext();
    type Payload = { summaries: unknown[]; groups: { key: string; label: string; document_ids: string[]; answer: Answer }[]; group_by: string; skipped?: { name: string; reason: string }[] };
    const planOf = (group_by: "project" | "doc_type" | "folder" | null, docs = [estimate, contract, guide]): Plan => ({ intent: "summarize", scope: { folder: null, doc_ids: docs, filter: {} }, operations: [{ op: "summarize", length: null, ...(group_by ? { group_by } : {}) }], requires_confirmation: false, confidence: 0.9, summary: "요약" });
    const run = async (group_by: "project" | "doc_type" | "folder" | null, docs = [estimate, contract, guide]) => {
      const exec = await planner.runPlan(planOf(group_by, docs), ctx, null, () => undefined);
      expect(exec.steps[0]).toMatchObject({ op: "summarize", status: "done" });
      return { summary: exec.steps[0]!.summary, payload: exec.steps[0]!.payload as Payload };
    };

    // No criterion: the three documents in one summary — its sources are the passages it cites, of each document.
    const whole = await run(null);
    expect(whole.payload.summaries).toEqual([]);
    expect(whole.payload.group_by).toBe("none");
    expect(whole.payload.groups.map((g) => [g.key, g.label, g.document_ids])).toEqual([["all", "전체", [estimate, contract, guide]]]);
    expect(whole.summary).toBe("문서 3건의 내용을 하나로 요약");
    expect(planner.planPreview(planOf(null), ctx).steps[0]!.summary).toBe("문서 3건의 내용을 하나로 요약");
    const one = whole.payload.groups[0]!.answer;
    expect(one).toMatchObject({ kind: "summary", document_id: null });
    expect(one.text.split("\n")).toHaveLength(3);
    expect([...new Set(one.sources.map((c) => c.document_id))]).toEqual([estimate, contract, guide]);
    for (const sentence of one.sentences) for (const n of sentence.citations) expect(one.sources.some((c) => c.index === n), `[${n}]`).toBe(true);
    // One document: the summary is that document's, named after it.
    const single = await run(null, [contract]);
    expect(single.payload.groups.map((g) => [g.key, g.label, g.document_ids])).toEqual([["all", "계약서_한빛.pdf", [contract]]]);
    expect(single.payload.groups[0]!.answer.document_id).toBe(contract);
    expect(single.summary).toBe("문서 1건 요약");
    expect(planner.planPreview(planOf(null, [contract]), ctx).steps[0]!.summary).toBe("문서 1건");

    const byProject = await run("project");
    const groups = byProject.payload.groups;
    expect(byProject.payload.summaries).toEqual([]);
    // One group per project, the documents outside every project last; the shared contract is in both.
    expect(groups.map((g) => g.label).sort()).toEqual(["새봄 ERP", "프로젝트 없음", "한빛 포털"].sort());
    expect(groups[groups.length - 1]).toMatchObject({ key: "", label: "프로젝트 없음", document_ids: [guide] });
    const hanbit = groups.find((g) => g.key === a.id)!;
    const saebom = groups.find((g) => g.key === b.id)!;
    expect([...hanbit.document_ids].sort()).toEqual([estimate, contract].sort());
    expect(saebom.document_ids).toEqual([contract]);
    // A group's summary is made from the passages of its documents: it has their sources.
    expect([...new Set(hanbit.answer.sources.map((c) => c.document_id))].sort()).toEqual([estimate, contract].sort());
    expect(hanbit.answer.text.split("\n")).toHaveLength(2);
    // A group of one document is that document's summary.
    expect(saebom.answer).toMatchObject({ kind: "summary", document_id: contract });
    expect(saebom.answer.sources.length).toBeGreaterThan(0);
    expect(byProject.summary).toBe("문서 3건 요약 · 프로젝트별 묶음 3개");

    // No document has a type yet: one group, labelled "유형 없음" ("No type" in English).
    const byType = await run("doc_type");
    expect(byType.payload.groups.map((g) => [g.key, g.label, g.document_ids.length])).toEqual([["", "유형 없음", 3]]);

    // A plan stored before 1.0.3 carries `target`: the validator clears it, and the result is the same one summary.
    const stored = planOf(null);
    (stored.operations[0] as { target?: string }).target = "each_doc";
    expect(planner.validatePlan(stored, ctx, null)).toEqual([]);
    expect(stored.operations[0]).toEqual({ op: "summarize", length: null });
  });

  // 1.0.3: `guard::per_document` / `split_check` — a sentence that asks for a summary of each document gets the
  // one summary and is told that summaries per document are not made (confirmed first).
  it("a summary of each document is not made, and the card says so", async () => {
    const ids = await importDocs(["견적서_한빛.pdf", "계약서_한빛.pdf"]);
    await indexAll();
    const ctx = planner.buildContext({ selected_doc_ids: [ids["견적서_한빛.pdf"]!, ids["계약서_한빛.pdf"]!] });
    for (const text of ["선택한 문서를 문서별로 요약해줘", "전체 문서를 파일별로 요약해줘", "선택한 문서를 각각 요약해줘", "선택한 문서를 하나씩 요약해줘", "각 문서를 요약해줘", "summarize each document"]) {
      const r = planner.planText(text, ctx);
      expect(r.plan.operations.map((op) => op.op), text).toEqual(["summarize"]);
      expect(r.plan.operations[0], text).not.toHaveProperty("target");
      expect(r.plan.requires_confirmation, text).toBe(true);
      expect(r.plan.unmet, text).toBeTruthy();
      const note = r.issues.find((i) => i.code === "grouping_dropped");
      expect(note?.message ?? "", text).toMatch(/문서별 요약은 만들지 않습니다|summaries per document are not made/);
      // (the card's own sentence does not promise a summary of each document)
      expect(r.plan.summary ?? "", text).not.toMatch(/문서별|파일별|각각|하나씩|each document/);
    }
    expect(planner.planText("선택한 문서를 문서별로 요약해줘", ctx).plan.unmet).toBe("문서마다 따로 요약하는 부분");
    // The plain request, "하나로 묶어" and a split the op has are not such sentences.
    for (const text of ["선택한 문서 요약해줘", "선택한 문서를 하나로 묶어 요약해줘", "전체 문서를 요약해줘", "프로젝트별로 각각 요약해줘"]) {
      const r = planner.planText(text, ctx);
      expect(r.plan.operations.map((op) => op.op), text).toEqual(["summarize"]);
      expect(r.plan.requires_confirmation, text).toBe(false);
      expect(r.issues.map((i) => i.code), text).not.toContain("grouping_dropped");
      expect(r.plan.operations[0], text).not.toHaveProperty("target");
    }
    expect(planner.planText("프로젝트별로 각각 요약해줘", ctx).plan.operations[0]).toMatchObject({ op: "summarize", group_by: "project" });
  });

  // 1.0.0 §2-1: `edit_text.original` — `rules::document_edit_rule`, `validate`, `exec` preview / run.
  it("a replacement asked for 'in the original' changes the file itself and saves a new edition", async () => {
    const ids = await importDocs(["개발계약서.docx", "스캔본.pdf"]);
    const [docx, pdf] = [ids["개발계약서.docx"]!, ids["스캔본.pdf"]!];
    const ctx = planner.buildContext({ selected_doc_ids: [docx] });
    const first = (text: string) => planner.rulesPlan(text, ctx)?.operations[0];
    // The page text, unless the sentence names the original, the file itself or a new edition.
    expect(first("'12,500원'을 '13,000원'으로 바꿔줘")).toEqual({ op: "edit_text", find: "12,500원", replace: "13,000원", pages: [] });
    // A word between the quotes is the text to change, not a word about where.
    expect(first("'원본'을 '사본'으로 바꿔줘")).toEqual({ op: "edit_text", find: "원본", replace: "사본", pages: [] });
    for (const text of ["원본에서 '12,500원'을 '13,000원'으로 바꿔줘", "원본에서 '12,500원'을 '13,000원'으로 고쳐줘", "'12,500원'을 '13,000원'으로 바꿔서 새 판으로 저장해줘", "replace '12,500원' with '13,000원' in the original file"]) {
      expect(first(text), text).toEqual({ op: "edit_text", find: "12,500원", replace: "13,000원", pages: [], original: true });
    }

    // A change of a file is always shown first; the preview counts what the file holds.
    const planned = planner.planText("원본에서 '12,500원'을 '13,000원'으로 바꿔줘", ctx);
    expect(planned.issues.filter((i) => i.blocking)).toEqual([]);
    expect(planned.plan.requires_confirmation).toBe(true);
    const preview = planner.planPreview(planned.plan, ctx);
    expect(preview.requires_confirmation).toBe(true);
    expect(preview.steps[0]).toMatchObject({ op: "edit_text", confirm: true, summary: "원본 파일에서 '12,500원' 2곳 → '13,000원' (서식 유지, 새 판으로 저장)", payload: { matches: 2, original: true } });

    // A file that cannot be edited in place, or more than one document: said before anything runs.
    const withScope = (doc_ids: string[]): Plan => ({ ...planned.plan, scope: { folder: null, doc_ids, filter: {} } });
    expect(planner.planPreview(withScope([pdf]), ctx).steps[0]).toMatchObject({ summary: "원본 파일을 고칠 수 없습니다: HWPX · DOCX · PPTX 파일만 원본을 고칠 수 있습니다", payload: { matches: 0, error: "edit_unsupported_format" } });
    expect(planner.planPreview(withScope([docx, pdf]), ctx).steps[0]).toMatchObject({ summary: "원본 파일을 고칠 수 없습니다: 문서 한 건만 열거나 선택해 주세요", payload: { error: "edit_original_needs_one_document" } });
    const tooMany = await planner.runPlan(withScope([docx, pdf]), ctx, null, () => undefined);
    // (the shell's text is "invalid value: <code>"; the result card words either form — `stepErrorText`)
    expect(tooMany.steps[0]).toMatchObject({ status: "failed", error: "edit_original_needs_one_document" });

    // The validator: a file has no pages, a paragraph of it is one line, `original: false` is no parameter.
    const check = (op: Plan["operations"][number]) => {
      const plan: Plan = { ...planned.plan, operations: [op] };
      return { codes: planner.validatePlan(plan, ctx, null).map((i) => i.code), op: plan.operations[0] };
    };
    expect(check({ op: "edit_text", find: "a", replace: "b", pages: [3, 1, 3], original: true }).op).toEqual({ op: "edit_text", find: "a", replace: "b", pages: [], original: true });
    expect(check({ op: "edit_text", find: "a", replace: "b", pages: [3, 1, 3], original: false }).op).toEqual({ op: "edit_text", find: "a", replace: "b", pages: [1, 3] });
    expect(check({ op: "edit_text", find: "a\nb", replace: "c", original: true }).codes).toContain("edit_line_break");
    expect(check({ op: "edit_text", find: "a\nb", replace: "c" }).codes).not.toContain("edit_line_break");

    // Run: the edited copy is a new document — the latest edition — and the original keeps its text.
    const exec = await planner.runPlan(planned.plan, ctx, null, () => undefined);
    const step = exec.steps[0]!;
    expect(step.status).toBe("done");
    const payload = step.payload as { documents: string[]; original: string; replaced: number; edition: { doc_id: string; rel_path: string } };
    expect(payload).toMatchObject({ original: docx, replaced: 2, documents: [payload.edition.doc_id] });
    expect(payload.edition.rel_path).toMatch(/개발계약서 \(수정\)\.docx$/);
    expect(exec.documents).toEqual([payload.edition.doc_id]);
    const text = async (id: string) => (await pages(id)).map((p) => p.text ?? "").join("\n");
    expect(await text(payload.edition.doc_id)).toContain("13,000원");
    expect(await text(payload.edition.doc_id)).not.toContain("12,500원");
    expect(await text(docx)).toContain("12,500원");
    // Nothing left to change in the new edition: the step fails with the reason code the card words.
    const again = await planner.runPlan(withScope([payload.edition.doc_id]), ctx, null, () => undefined);
    expect(again.steps[0]).toMatchObject({ status: "failed", error: "edit_nothing_found" });
  });

  // 1.0.0, user decision 2026-11-16 (plan 4-3): "참고 자료로 지정해줘" opens "프로젝트에 넣기" on 참고 자료 —
  // `rules::document_edit_rule`, `exec` (`set_role` knowledge → `open` / `assign`), `validate`.
  it("documents asked to become reference material are handed to the put-in-project dialog; nothing changes before that", async () => {
    const ids = await importDocs(["보안지침.pdf", "개발계약서.docx"]);
    const [a, b] = [ids["보안지침.pdf"]!, ids["개발계약서.docx"]!];
    const ctx = planner.buildContext({ selected_doc_ids: [a, b] });
    for (const text of ["이 문서를 참고 자료로 지정해줘", "선택한 문서를 참고 자료로 표시", "mark these as reference material"]) {
      const planned = planner.planText(text, ctx);
      expect(planned.plan.operations, text).toEqual([{ op: "set_role", role: "knowledge" }]);
      expect(planned.plan.requires_confirmation, text).toBe(false);
      expect(planned.issues.filter((i) => i.blocking), text).toEqual([]);
    }
    const planned = planner.planText("이 문서를 참고 자료로 지정해줘", ctx);
    expect(planned.plan.summary).toBe("문서를 프로젝트의 참고 자료로 넣는 화면을 엽니다");
    expect(planner.planPreview(planned.plan, ctx).steps[0]).toMatchObject({ op: "set_role", summary: "문서 2건: 프로젝트의 참고 자료로 넣는 화면을 엽니다" });

    const role = async (id: string) => (await backend.mockInvoke<{ role: string }>("documents_get", { id })).role;
    const exec = await planner.runPlan(planned.plan, ctx, null, () => undefined);
    expect(exec.steps[0]).toMatchObject({ status: "done", payload: { documents: expect.arrayContaining([a, b]), role: "knowledge", assign: true } });
    // (the documents come in the order of the working set, not of the selection)
    const handed = (exec.steps[0]!.payload as { documents: string[] }).documents;
    expect(handed).toHaveLength(2);
    expect(exec.ui_actions).toEqual([{ op: "open", target: "assign", ref: `reference:${handed.join(",")}` }]);
    expect([await role(a), await role(b)]).toEqual(["work", "work"]);
    // A work document / a form is still a mark on the document itself.
    const form = planner.planText("이 문서를 양식으로 표시", ctx);
    const marked = await planner.runPlan(form.plan, ctx, null, () => undefined);
    expect(marked.ui_actions).toEqual([]);
    expect(await role(a)).toBe("form");
    // Only the executor writes the dialog's target: a plan that holds it is refused.
    const direct: Plan = { ...planned.plan, operations: [{ op: "open", target: "assign", ref: `reference:${a}` }] };
    expect(planner.validatePlan(direct, ctx, null).map((i) => i.code)).toContain("target_unknown");
  });

  // 1.0.1, user report 2026-11-17 ("요약이 자꾸 실패" — a card that said "참고 자료만 … 대상 문서 0건", and "프로젝트
  // 단위에서 모든 검색 및 요약 등 명령을 참고자료와 업무문서를 둘 다 포함하도록"): `guard::kind_named`,
  // `validate::project_reference`, `scope::resolve`, `exec::preview`, `exec::no_documents`.
  // 1.0.2 (user report 2026-11-19 — "AA에 대해서 요약해줘 … 요약만 하고 AA에 대한 내용을 수집하지 않음"):
  // `guard::about` / `summary_topic`, `rules::process`, `exec::summarize::run_topic`.
  it("a summary about a subject gathers what the documents say about it", async () => {
    const of = (text: string) => {
      const a = planner.aboutOf(text);
      return a ? [a.subject, a.rest] : null;
    };
    // The same sentences as `guard::tests::the_subject_of_a_sentence`.
    expect(of("AA에 대해서 요약해줘")).toEqual(["AA", "요약해줘"]);
    expect(of("납품 기한에 대해 요약해줘")).toEqual(["납품 기한", "요약해줘"]);
    expect(of("보안 요구사항에 관해서 정리해줘")).toEqual(["보안 요구사항", "정리해줘"]);
    expect(of("지체상금에 대한 내용을 요약해줘")).toEqual(["지체상금", "내용을 요약해줘"]);
    expect(of("하자보수 관련 내용을 요약해줘")).toEqual(["하자보수", "을 요약해줘"]);
    expect(of("보안 관련 요약")).toEqual(["보안", "요약"]);
    expect(of("개인정보와 관련된 조항을 알려줘")).toEqual(["개인정보", "조항을 알려줘"]);
    expect(of("한빛 납품 프로젝트에서 검수 기준에 대해 요약해줘")).toEqual(["검수 기준", "한빛 납품 프로젝트에서 요약해줘"]);
    expect(of("AAA 프로젝트의 일정에 대해 알려줘")).toEqual(["일정", "AAA 프로젝트의 알려줘"]);
    expect(of("프로젝트별로 지체상금에 대해 요약해줘")).toEqual(["지체상금", "프로젝트별로 요약해줘"]);
    expect(of("자세히 계약 금액에 대해 요약해줘")).toEqual(["계약 금액", "자세히 요약해줘"]);
    expect(of("계약 금액에 대해 자세히 요약해줘")).toEqual(["계약 금액", "자세히 요약해줘"]);
    expect(of("summarise what the documents say about late fees.")).toEqual(["late fees", "summarise what the documents say"]);
    expect(of("Summarize about security requirements in the Hanbit project")).toEqual(["security requirements", "Summarize in the Hanbit project"]);
    for (const text of [
      "이 문서에 대해 요약해줘",
      "그것에 대해 더 알려줘",
      "위 내용에 대해 요약해줘",
      "선택한 문서에 대해 요약해줘",
      "이 프로젝트에 대해 요약해줘",
      "AAA 프로젝트에 대해 요약해줘",
      "참고 자료에 대해 요약해줘",
      "문서에 대해 요약해줘",
      "summarise about this document",
      "tell me about it",
      "요약해줘",
      "관련자 명단 요약해줘",
      `다음 글을 요약해줘: ${"계약 해지에 대한 조항은 다음과 같다. ".repeat(6)}`,
    ]) {
      expect(of(text), text).toBeNull();
    }

    const ids = await importDocs(["개발계약서.docx", "보안지침.pdf", "회의록_10월.pdf"]);
    await indexAll();
    const contract = ids["개발계약서.docx"]!;
    const ctx = planner.buildContext();
    const topic = (text: string) => planner.summaryTopic(text, ctx)?.subject ?? null;
    expect(topic("AA에 대해서 요약해줘")).toBe("AA");
    expect(topic("보안 요구사항에 대해 요약해줘")).toBe("보안 요구사항");
    expect(topic("AA에 대해 알려줘")).toBeNull();
    expect(topic("요약에 대해 알려줘")).toBeNull();
    expect(topic("견적서에 대해 요약해줘"), "the estimates are summarised").toBeNull();
    // A summary to make — not one that exists already.
    for (const text of ["AA에 대해 요약 좀 해줘", "AA에 대해서 요약 부탁해", "AA 관련 요약", "AA에 관해 간단히 요약해 주세요"]) expect(topic(text), text).toBe("AA");
    for (const text of ["지난 회의에 대한 요약본 찾아줘", "AA에 관한 요약 보고서 열어줘", "AA에 대해 요약된 문서 보여줘"]) expect(topic(text), text).toBeNull();

    // The plan: one summary of the subject, over every document when no scope is named — nothing is asked back.
    const said = planner.planText("계약 금액에 대해서 요약해줘", ctx);
    expect(said.plan.operations).toEqual([{ op: "summarize", length: null, topic: "계약 금액" }]);
    expect(planner.isClarification(said.plan)).toBe(false);
    expect(said.plan.scope).toMatchObject({ folder: null, doc_ids: [] });
    expect(said.plan.summary).toBe("'계약 금액'에 관한 내용을 문서에서 찾아 요약합니다");
    const preview = planner.planPreview(said.plan, ctx);
    expect(preview.steps[0]).toMatchObject({ summary: "'계약 금액'에 관한 내용을 찾아 요약 · 문서 3건", payload: { documents: 3, topic: "계약 금액" } });
    // "보안 요구사항" names no document type ("요구사항" belongs to the subject); a type outside the subject does.
    expect(planner.planText("보안 요구사항에 대해 요약해줘", ctx).plan.scope.filter?.doc_type ?? null).toBeNull();
    expect(planner.planText("계약서에서 지체상금에 대해 요약해줘", ctx).plan.scope.filter?.doc_type).toBe("dev_contract");
    expect(planner.planText("자세히 계약 금액에 대해 요약해줘", ctx).plan.operations[0]).toMatchObject({ length: "long", topic: "계약 금액" });
    // A summary without a subject is the summary of the documents, as before.
    expect(planner.planText("계약서 요약해줘", ctx).plan.operations[0]).toEqual({ op: "summarize", length: null });
    expect((planner.planText("계약서 요약해줘", ctx).plan.operations[0] as { topic?: string }).topic).toBeUndefined();

    // The step: one summary with its sources; the documents that say nothing about it are not read.
    const exec = await planner.runPlan(said.plan, ctx, null, () => undefined);
    expect(exec.steps[0]).toMatchObject({ op: "summarize", status: "done" });
    const payload = exec.steps[0]!.payload as { topic: string; summaries: unknown[]; groups: { key: string; label: string; document_ids: string[]; answer: Answer }[] };
    expect(payload).toMatchObject({ topic: "계약 금액", summaries: [], group_by: "none" });
    expect(payload.groups).toHaveLength(1);
    expect(payload.groups[0]).toMatchObject({ key: "topic", label: "'계약 금액'에 관한 내용", document_ids: [contract] });
    expect(payload.groups[0]!.answer).toMatchObject({ kind: "summary", question: "계약 금액", no_evidence: false });
    expect(payload.groups[0]!.answer.sources.every((x) => x.document_id === contract)).toBe(true);
    expect(exec.steps[0]!.summary).toContain("'계약 금액'에 관한 내용 요약 · 문서 1건");
    // Nothing about the subject: said, not an error.
    const none = await planner.runPlan(planner.planText("양자 컴퓨터에 대해 요약해줘", ctx).plan, ctx, null, () => undefined);
    expect(none.steps[0]).toMatchObject({ status: "done", summary: "'양자 컴퓨터'에 관한 내용을 범위의 문서에서 찾지 못했습니다" });
    // A scope that matches no document is told, not widened.
    const empty: Plan = { ...structuredClone(said.plan), scope: { folder: null, doc_ids: [], filter: { doc_type: "estimate" } } };
    planner.validatePlan(empty, ctx, null);
    const failed = await planner.runPlan(empty, ctx, null, () => undefined);
    expect(failed.steps[0]).toMatchObject({ status: "failed" });
    expect(failed.steps[0]!.error).toContain("no_documents");
    // The validator: a subject makes one summary; a grouping is dropped with a note; an empty subject is none.
    const grouped: Plan = { ...structuredClone(said.plan), operations: [{ op: "summarize", group_by: "project", topic: "  계약   금액 " }] };
    expect(planner.validatePlan(grouped, ctx, null).map((i) => i.code)).toContain("grouping_dropped");
    expect(grouped.operations[0]).toEqual({ op: "summarize", group_by: null, topic: "계약 금액" });
    const blank: Plan = { ...structuredClone(said.plan), operations: [{ op: "summarize", topic: "   " }] };
    planner.validatePlan(blank, ctx, null);
    expect(blank.operations[0]).toEqual({ op: "summarize" });

    // What is open is where to look when the words point at it — and only then.
    const open = { ...ctx, current_document: contract };
    expect(planner.planText("이 문서에서 계약 금액에 대해 요약해줘", open).plan).toMatchObject({ operations: [{ op: "summarize", topic: "계약 금액" }], scope: { doc_ids: [contract] } });
    expect(planner.planText("계약 금액에 대해 요약해줘", open).plan.scope.doc_ids).toEqual([]);
    expect(topic("계약서 내용에 대해 요약해줘"), "the contracts are summarised").toBeNull();
    // Two slots, each filled by what the sentence says (user decision 2026-11-20; `guard::tests::the_project_slot_and_the_content_slot`).
    const aaa = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "AAA" } });
    const named = planner.buildContext();
    const slots = (text: string, c = named) => {
      const r = planner.planText(text, c);
      const op = r.plan.operations[0] as { op: string; topic?: string | null; group_by?: string | null };
      // (`group`: a summary is one result unless a grouping says otherwise — there is no `target` since 1.0.3)
      expect(op, text).not.toHaveProperty("target");
      return { op: op.op, project: r.plan.scope.filter?.project_id ?? null, topic: op.topic ?? null, group: op.group_by ?? null, docs: r.plan.scope.doc_ids ?? [], codes: r.issues.map((i) => i.code), plan: r.plan };
    };
    // 1. Both: the content inside the project (its work documents and its reference material) — a summary or an explanation.
    for (const text of ["AAA 프로젝트의 계약 금액 내용에 대해 요약해줘", "AAA 프로젝트의 계약 금액 내용에 대해 설명해줘", "AAA 프로젝트 계약 금액에 대해 요약해줘", "AAA의 계약 금액에 대해 요약해줘"]) {
      const s1 = slots(text);
      expect({ op: s1.op, project: s1.project, topic: s1.topic }, text).toEqual({ op: "summarize", project: aaa.id, topic: "계약 금액" });
      expect(s1.plan.scope.filter?.with_reference, text).toBe(true);
      expect(s1.plan.summary, text).toBe("'AAA' 프로젝트에서 '계약 금액'에 관한 내용을 찾아 요약합니다");
    }
    // 2. The content alone: every document, no project — also when a project is called so (the card names the other reading).
    expect(slots("계약 금액에 대해서 요약해줘")).toMatchObject({ op: "summarize", project: null, topic: "계약 금액", docs: [] });
    const collision = slots("AAA에 대해서 요약해줘");
    expect(collision).toMatchObject({ op: "summarize", project: null, topic: "AAA", group: null });
    expect(collision.codes).toContain("topic_names_project");
    expect(slots("계약 금액에 대해서 요약해줘").codes).not.toContain("topic_names_project");
    // (an explanation without a place stays a question: it may be answered from general knowledge)
    expect(slots("계약 금액에 대해 설명해줘").op).not.toBe("summarize");
    // 3. The content inside what is open: no project; with nothing open the assistant asks.
    const nothingOpen = planner.planText("현재 문서에서 계약 금액 내용에 대해서 요약해줘", named).plan;
    expect(planner.isClarification(nothingOpen)).toBe(true);
    expect(nothingOpen.clarification_question).toContain("열려 있거나 선택한 문서가 없습니다");
    const opened = { ...named, current_document: contract };
    for (const text of ["현재 문서에서 계약 금액 내용에 대해서 요약해줘", "이 문서에서 계약 금액에 대해 설명해줘"]) {
      expect(slots(text, opened), text).toMatchObject({ op: "summarize", project: null, topic: "계약 금액", docs: [contract] });
      expect(slots(text, opened).plan.summary, text).toBe("열린 문서에서 '계약 금액'에 관한 내용을 찾아 요약합니다");
    }
    // 4. The project alone: all its documents, one summary of them — by its name with the word, as a subject or not.
    for (const text of ["AAA 프로젝트 요약해줘", "AAA 프로젝트에 대해 요약해줘", "AAA 프로젝트에 관해서 자세히 요약해줘"]) {
      const s4 = slots(text);
      expect({ op: s4.op, project: s4.project, topic: s4.topic, group: s4.group }, text).toEqual({ op: "summarize", project: aaa.id, topic: null, group: null });
      expect(s4.plan.scope.filter, text).toMatchObject({ project_id: aaa.id, with_reference: true });
      expect(s4.plan.summary, text).toBe("'AAA' 프로젝트의 문서를 요약합니다");
      expect(planner.summaryTopic(text, named), text).toBeNull();
    }
    // ("all of it, in short" is no subject: the summary of the project)
    for (const text of ["AAA 프로젝트의 주요 내용에 대해 요약해줘", "AAA 프로젝트의 전체 내용에 대해 요약해줘", "AAA 프로젝트의 핵심에 대해 요약해줘"]) {
      expect(slots(text), text).toMatchObject({ op: "summarize", project: aaa.id, topic: null, group: null });
    }
    // (the words may narrow the project further; a summary of each document is not made any more — 1.0.3 —
    // and the sentence that asks for one is told so and confirmed)
    const each = slots("AAA 프로젝트 문서를 각각 요약해줘");
    expect(each).toMatchObject({ op: "summarize", project: aaa.id, topic: null, group: null });
    expect(each.codes).toContain("grouping_dropped");
    expect(each.plan.requires_confirmation).toBe(true);
    expect(slots("AAA 프로젝트의 계약서 요약해줘")).toMatchObject({ project: aaa.id, group: null });
    expect(slots("AAA 프로젝트의 계약서 요약해줘").plan.scope.filter?.doc_type).toBe("dev_contract");
    // A project the workspace does not have is said, not silently widened.
    expect(slots("없는 프로젝트의 계약 금액에 대해 요약해줘")).toMatchObject({ project: null, topic: "계약 금액" });
    expect(slots("없는 프로젝트의 계약 금액에 대해 요약해줘").codes).toContain("project_not_found");
    expect(planner.summaryTopic("AA에 대해서 요약해줘", named)?.subject, "another word than the project's name").toBe("AA");
    // The project named as the place and its name as the subject: a subject like any other.
    expect(planner.planText("AAA 프로젝트에서 AAA에 대해 요약해줘", named).plan).toMatchObject({
      operations: [{ op: "summarize", topic: "AAA" }],
      scope: { filter: { project_id: aaa.id } },
    });
    expect(planner.planText("AAA 프로젝트에서 계약 금액에 대해 요약해줘", named).plan).toMatchObject({
      operations: [{ op: "summarize", topic: "계약 금액" }],
      scope: { filter: { project_id: aaa.id } },
    });
  });

  it("a kind named as one more thing to cover is no restriction; inside a project a plan that reads covers both kinds", async () => {
    for (const text of ["참고자료도 포함해서 요약해줘", "참고 자료까지 요약해줘", "참고자료와 문서를 모두 요약해줘", "참고자료 포함해서 프로젝트 요약해줘", "참고자료를 포함해서 요약해줘", "참고 자료랑 같이 요약해줘", "업무 문서도 같이 요약해줘", "참고자료와 업무문서를 둘 다 요약해줘", "summarize the documents and reference material", "summarize it including the reference documents", "summarize the reference material and the contracts"]) {
      expect(planner.kindNamed(text), text).toBeNull();
    }
    for (const [text, kind] of [
      ["참고 자료 요약해줘", "knowledge"],
      ["참고자료만 요약해줘", "knowledge"],
      ["참고자료를 요약해줘", "knowledge"],
      ["참고 자료 중에서 과업 범위를 요약해줘", "knowledge"],
      ["참고자료 과업 범위 요약", "knowledge"],
      ["참고 자료 말고 업무 문서에서 납품 기한 찾아줘", "work"],
      ["summarize the reference material", "knowledge"],
      ["understand reference material first", "knowledge"],
      ["업무문서를 요약해줘", "work"],
    ] as const) {
      expect(planner.kindNamed(text), text).toBe(kind);
    }

    const ids = await importDocs(["개발계약서.docx", "보안지침.pdf", "개발표준.pdf", "회의록_10월.pdf"]);
    await indexAll();
    const [work, ref, shared, other] = [ids["개발계약서.docx"]!, ids["보안지침.pdf"]!, ids["개발표준.pdf"]!, ids["회의록_10월.pdf"]!];
    const hanbit = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "한빛 포털" } });
    const saebom = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "새봄 ERP" } });
    const empty = await backend.mockInvoke<{ id: string }>("project_create", { input: { name: "빈 프로젝트" } });
    // 한빛: two work documents and a reference document; "개발표준" is reference material of 새봄 as well.
    await backend.mockInvoke("project_add_documents", { id: hanbit.id, docIds: [work, shared] });
    await backend.mockInvoke("project_add_documents", { id: hanbit.id, docIds: [ref], asReference: true });
    await backend.mockInvoke("project_add_documents", { id: saebom.id, docIds: [shared], asReference: true });
    const ctx = planner.buildContext();
    const inScope = (filter: NonNullable<Plan["scope"]["filter"]>) => planner.resolveScope({ filter }).map((d) => d.id).sort();
    const sorted = (list: string[]) => [...list].sort();
    // Where the project holds a document decides its kind there, not the document's own mark.
    expect(inScope({ project_id: hanbit.id })).toEqual(sorted([work, shared]));
    expect(inScope({ project_id: hanbit.id, role: "work" })).toEqual(sorted([work, shared]));
    expect(inScope({ project_id: hanbit.id, role: "knowledge" })).toEqual([ref]);
    expect(inScope({ project_id: hanbit.id, with_reference: true })).toEqual(sorted([work, shared, ref]));
    expect(inScope({ project_id: hanbit.id, role: "knowledge", with_reference: true })).toEqual([ref]);
    expect(inScope({ project_id: saebom.id })).toEqual([]);
    expect(inScope({ project_id: saebom.id, with_reference: true })).toEqual([shared]);
    expect(inScope({ role: "knowledge" })).toEqual(sorted([ref, shared]));
    expect(inScope({})).toHaveLength(4);
    expect(other).toBeTruthy();

    // The validator sets the flag for plans that only read, and for nothing else.
    const planOf = (operations: Plan["operations"], filter: NonNullable<Plan["scope"]["filter"]>): Plan => {
      const base = planner.planText("한빛 포털 프로젝트 요약해줘", ctx).plan;
      const plan: Plan = { ...structuredClone(base), operations, scope: { folder: null, doc_ids: [], filter } };
      planner.validatePlan(plan, ctx, null);
      return plan;
    };
    const summarize = { op: "summarize", length: null, group_by: null } as const;
    expect(planOf([summarize], { project_id: hanbit.id }).scope.filter?.with_reference).toBe(true);
    expect(planOf([summarize], { project_id: hanbit.id, role: "knowledge" }).scope.filter?.with_reference).toBeUndefined();
    expect(planOf([{ op: "tag", add: ["확인"], remove: [] }], { project_id: hanbit.id, with_reference: true }).scope.filter?.with_reference).toBeUndefined();
    expect(planOf([summarize], {}).scope.filter?.with_reference).toBeUndefined();

    // The sentence names the project: the plan covers both kinds and the card says how many are reference material.
    const named = planner.planText("한빛 포털 프로젝트 요약해줘", ctx);
    expect(named.plan.scope.filter).toMatchObject({ project_id: hanbit.id, with_reference: true });
    expect(planner.planPreview(named.plan, ctx)).toMatchObject({ document_count: 3, reference_count: 1 });
    const exec = await planner.runPlan(named.plan, ctx, null, () => undefined);
    expect(exec.steps[0]).toMatchObject({ op: "summarize", status: "done" });
    // (1.0.3: one summary of the three — the work documents and the reference material — and none per document)
    const summarised = exec.steps[0]!.payload as { summaries: unknown[]; groups: { key: string; document_ids: string[] }[] };
    expect(summarised.summaries).toEqual([]);
    expect(summarised.groups.map((g) => g.key)).toEqual(["all"]);
    expect([...summarised.groups[0]!.document_ids].sort()).toEqual(sorted([work, shared, ref]));
    // A project that holds reference material only is summarised too.
    const onlyReference = await planner.runPlan(planOf([summarize], { project_id: saebom.id }), ctx, null, () => undefined);
    expect(onlyReference.steps[0]).toMatchObject({ status: "done" });
    // "참고 자료만" where there is none: the reason says what is missing.
    const none = await planner.runPlan(planOf([summarize], { project_id: empty.id, role: "knowledge" }), ctx, null, () => undefined);
    expect(none.steps[0]).toMatchObject({ status: "failed" });
    expect(none.steps[0]!.error).toContain("no_reference_in_project");
    // A sentence that adds the reference material is not read as "reference material only".
    const added = planner.planText("한빛 포털 프로젝트 참고자료도 포함해서 요약해줘", ctx);
    expect(added.plan.scope.filter).toMatchObject({ project_id: hanbit.id, with_reference: true });
    expect(added.plan.scope.filter?.role ?? null).toBeNull();
    expect(added.issues.map((i) => i.message).join(" ")).not.toContain("참고 자료만");
  });
});

// 1.0.0 §2-1 step 2 (`documents::edit`: `structure`, `Part::{insert_paragraph, delete_paragraph, insert_row,
// delete_row}`): a paragraph is a line of the mock's page text, a table row a line with tabs.
describe("original-file edits: paragraphs and table rows (documents::edit)", () => {
  const preview = (id: string, edits: EditRequest[]) => backend.mockInvoke<EditOutcome>("documents_edit_preview", { id, edits });
  const text = async (id: string) => (await pages(id)).map((p) => p.text ?? "");
  /** What the file holds on a page (set without a correction: the viewer's corrections do not reach the file). */
  const setFileText = (id: string, pageNo: number, value: string) => {
    const d = backend.wsDocs().find((x) => x.id === id)!;
    backend.upsertDoc({ ...d, pages: d.pages.map((p) => (p.page_no === pageNo ? { ...p, text: value } : p)) });
  };

  it("a paragraph is added next to the one that holds the text, or taken out; several places ask which one", async () => {
    const ids = await importDocs(["개발계약서.docx"]);
    const doc = ids["개발계약서.docx"]!;
    setFileText(doc, 1, "제3조 납품\n① 납품 기한은 11월 30일로 한다.\n② 납품 장소는 발주처로 한다.");
    setFileText(doc, 2, "산출물\t기한\n설계서\t10월 31일\n소스 코드\t11월 30일\n끝.");

    // "납품" is in three paragraphs: nothing is done before the request says which one.
    const vague = await preview(doc, [{ find: "납품", replace: "③ 검수는 14일 안에 마친다.", action: "insert_after" }]);
    expect(vague).toMatchObject({ found: [3], replaced: [0], problems: [{ edit: 0, code: "edit_anchor_ambiguous" }], changes: [] });
    await expect(backend.mockInvoke("documents_edit_save", { id: doc, edits: [{ find: "납품", replace: "x", action: "insert_after" }] })).rejects.toMatchObject({ message: "edit_anchor_ambiguous" });

    const edits: EditRequest[] = [
      { find: "② 납품 장소", replace: "③ 검수는 14일 안에 마친다.", action: "insert_after" },
      { find: "납품", replace: "(2026년 계약)", action: "insert_before", occurrence: 1 },
      { find: "① 납품 기한", replace: "", action: "delete_paragraph" },
    ];
    const report = await preview(doc, edits);
    expect(report.problems ?? []).toEqual([]);
    expect(report.replaced).toEqual([1, 1, 1]);
    expect(report.changes.map((c) => [c.action, c.before, c.after])).toEqual([
      ["insert_after", "② 납품 장소는 발주처로 한다.", "③ 검수는 14일 안에 마친다."],
      ["insert_before", "제3조 납품", "(2026년 계약)"],
      ["delete_paragraph", "① 납품 기한은 11월 30일로 한다.", ""],
    ]);
    // Nothing was written by the previews; the save makes a new edition and leaves the original as it was.
    const edition = await backend.mockInvoke<{ doc_id: string; report: EditOutcome }>("documents_edit_save", { id: doc, edits });
    expect((await text(edition.doc_id))[0]).toBe("(2026년 계약)\n제3조 납품\n② 납품 장소는 발주처로 한다.\n③ 검수는 14일 안에 마친다.");
    expect((await text(doc))[0]).toBe("제3조 납품\n① 납품 기한은 11월 30일로 한다.\n② 납품 장소는 발주처로 한다.");
    // A line break is markup, not a character — in a new paragraph too.
    await expect(preview(doc, [{ find: "제3조", replace: "가\n나", action: "insert_after" }])).rejects.toMatchObject({ message: "edit_line_break" });
    // A character XML cannot hold (pasted text brings a vertical tab along) is refused; a tab is a character.
    await expect(preview(doc, [{ find: "제3조", replace: "가\u000b나" }])).rejects.toMatchObject({ message: "edit_control_character" });
    expect((await preview(doc, [{ find: "제3조", replace: "제3조\t(납품)" }])).problems ?? []).toEqual([]);
  });

  it("a table row is added above / below the row that holds the text, or taken out", async () => {
    const ids = await importDocs(["개발계약서.docx"]);
    const doc = ids["개발계약서.docx"]!;
    setFileText(doc, 1, "납품 기한은 11월 30일로 한다.");
    setFileText(doc, 2, "산출물\t기한\n설계서\t10월 31일\n소스 코드\t11월 30일\n끝.");
    const edits: EditRequest[] = [
      { find: "설계서", replace: "시험 결과서 | 11월 15일", action: "insert_row_after" },
      { find: "설계서", replace: "", cells: ["착수 보고서"], action: "insert_row_before" },
      // "11월 30일" is in a paragraph and in a row: the row is the one place for a row edit.
      { find: "11월 30일", replace: "", action: "delete_row" },
    ];
    const report = await preview(doc, edits);
    expect(report).toMatchObject({ found: [1, 1, 1], replaced: [1, 1, 1] });
    expect(report.changes.map((c) => [c.action, c.before, c.after])).toEqual([
      ["insert_row_after", "설계서 | 10월 31일", "시험 결과서 | 11월 15일"],
      ["insert_row_before", "설계서 | 10월 31일", "착수 보고서"],
      ["delete_row", "소스 코드 | 11월 30일", ""],
    ]);
    const edition = await backend.mockInvoke<{ doc_id: string }>("documents_edit_save", { id: doc, edits });
    expect((await text(edition.doc_id))[1]).toBe("산출물\t기한\n착수 보고서\t\n설계서\t10월 31일\n시험 결과서\t11월 15일\n끝.");

    const code = async (edit: EditRequest) => (await preview(doc, [edit])).problems?.[0]?.code ?? null;
    expect(await code({ find: "설계서", replace: "a | b | c", action: "insert_row_after" })).toBe("edit_row_cells");
    expect(await code({ find: "납품 기한", replace: "", action: "delete_row" })).toBe("edit_not_in_table");
    expect(await code({ find: "없는 글", replace: "", action: "delete_row" })).toBeNull();
    // The only paragraph of a page stays; the last row of a table stays.
    expect(await code({ find: "납품 기한", replace: "", action: "delete_paragraph" })).toBe("edit_last_paragraph");
    setFileText(doc, 2, "산출물\t기한\n끝.");
    expect(await code({ find: "산출물", replace: "", action: "delete_row" })).toBe("edit_last_row");
  });

  // Step 3 (`documents::edit::suggest`): an instruction in words becomes replacements pinned to one place.
  it("an instruction in words becomes a replacement at the place it names", async () => {
    const ids = await importDocs(["개발계약서.docx", "스캔본.pdf"]);
    const doc = ids["개발계약서.docx"]!;
    const suggest = (id: string, instruction: string) => backend.mockInvoke<import("@/lib/types").EditSuggestion>("documents_edit_suggest", { id, instruction });
    // The amount is on both pages: the suggestion names the first place only.
    const s = await suggest(doc, "12,500원을 13,000원으로 바꿔줘");
    expect(s).toMatchObject({ edits: [{ find: "12,500원", replace: "13,000원", occurrence: 1 }], dropped: 0, note: "" });
    expect((await preview(doc, s.edits)).replaced).toEqual([1]);
    // What is not in the document is left out; a sentence that names no change says so.
    expect(await suggest(doc, "99,999원을 1원으로 바꿔줘")).toMatchObject({ edits: [], dropped: 1 });
    expect((await suggest(doc, "좀 더 보기 좋게")).note).not.toBe("");
    await expect(suggest(doc, "  ")).rejects.toMatchObject({ message: "description_empty" });
    await expect(suggest(ids["스캔본.pdf"]!, "가를 나로 바꿔줘")).rejects.toMatchObject({ message: "edit_unsupported_format" });
    // No model loaded: there is nobody to ask.
    localStorage.setItem("doculens.mock.vision", "none");
    await expect(suggest(doc, "12,500원을 13,000원으로 바꿔줘")).rejects.toMatchObject({ message: "no_model" });
    localStorage.removeItem("doculens.mock.vision");
  });

  // Step 3: the file still holds what the page read before a correction in the viewer.
  it("a correction made in the viewer does not reach the file until it is made there too", async () => {
    const ids = await importDocs(["개발계약서.docx"]);
    const doc = ids["개발계약서.docx"]!;
    const before = (await text(doc))[1]!;
    expect(before).toContain("합계 12,500원");
    await backend.mockInvoke("documents_set_page_text", { id: doc, pageNo: 2, text: before.replace("12,500원", "13,500원") });
    expect((await text(doc))[1]).toContain("합계 13,500원");
    // The corrected word is not in the file; the one it replaced is — once on each page.
    expect((await preview(doc, [{ find: "13,500원", replace: "x" }])).found).toEqual([0]);
    const history = await backend.mockInvoke<import("@/lib/types").PageTextHistory>("documents_page_text_history", { id: doc, pageNo: null });
    const { correctionsOf } = await import("@/features/library/editCorrections");
    const edits = correctionsOf(history.entries);
    expect(edits).toEqual([{ find: "12,500원", replace: "13,500원" }]);
    expect((await preview(doc, edits)).replaced).toEqual([2]);
    const edition = await backend.mockInvoke<{ doc_id: string }>("documents_edit_save", { id: doc, edits });
    expect(await text(edition.doc_id)).toEqual([expect.stringContaining("합계 13,500원"), expect.stringContaining("합계 13,500원")]);
  });

  // Step 4 (`documents::edit::cells`): a workbook has cells, not paragraphs.
  it("a workbook cell gets a new value; a formula stays, and the ones that read the cell are counted", async () => {
    const ids = await importDocs(["견적서.xlsx", "개발계약서.docx"]);
    const book = ids["견적서.xlsx"]!;
    setFileText(book, 1, "항목\t단가\t수량\t금액\n설계\t1250000\t4\t=B2*C2\n개발\t350000\t2\t=B3*C3\n합계\t\t\t=SUM(D2:D3)");
    setFileText(book, 2, "총액\t=Sheet1!D4\n담당\t김하늘\n코드\t");
    const edits: EditRequest[] = [
      { find: "b2", replace: "1,300,000", action: "set_cell" },
      { find: "Sheet2!B2", replace: "이바다", action: "set_cell" },
      // A code keeps its zeros; a cell that was not there is made.
      { find: "'Sheet2'!$B$3", replace: "007", action: "set_cell" },
      { find: "Sheet1!F6", replace: "비고", action: "set_cell" },
      { find: "설계", replace: "분석 · 설계" },
    ];
    const report = await preview(book, edits);
    expect(report.problems ?? []).toEqual([]);
    expect(report).toMatchObject({ found: [1, 1, 1, 1, 1], replaced: [1, 1, 1, 1, 1] });
    expect(report.changes.map((c) => [c.action ?? "replace", c.part, c.before, c.after])).toEqual([
      ["set_cell", "xl/worksheets/sheet1.xml", "Sheet1!B2 = 1250000", "Sheet1!B2 = 1300000"],
      ["set_cell", "xl/worksheets/sheet2.xml", "Sheet2!B2 = 김하늘", "Sheet2!B2 = 이바다"],
      ["set_cell", "xl/worksheets/sheet2.xml", "Sheet2!B3 = ", "Sheet2!B3 = 007"],
      ["set_cell", "xl/worksheets/sheet1.xml", "Sheet1!F6 = ", "Sheet1!F6 = 비고"],
      ["replace", "xl/sharedStrings.xml", expect.stringContaining("설계"), expect.stringContaining("분석 · 설계")],
    ]);
    // D2 reads B2, D4 reads D2 (a range), Sheet2!B1 reads D4: three results are computed again; D3 is not.
    expect(report.notes).toEqual([{ edit: 0, code: "edit_formulas_cleared", count: 3 }]);
    const edition = await backend.mockInvoke<{ doc_id: string; rel_path: string }>("documents_edit_save", { id: book, edits });
    expect(edition.rel_path).toMatch(/견적서 \(수정\)\.xlsx$/);
    expect(await text(edition.doc_id)).toEqual([
      "항목\t단가\t수량\t금액\n분석 · 설계\t1300000\t4\t=B2*C2\n개발\t350000\t2\t=B3*C3\n합계\t\t\t=SUM(D2:D3)\n\n\t\t\t\t\t비고",
      "총액\t=Sheet1!D4\n담당\t이바다\n코드\t007",
    ]);
    expect((await text(book))[0]).toContain("1250000");

    // What is typed and what is stored (core `cells::number`): signs, a bare fraction, commas in and out of place.
    const stored = async (value: string) => (await preview(book, [{ find: "Sheet1!H9", replace: value, action: "set_cell" }])).changes[0]!.after.replace("Sheet1!H9 = ", "");
    expect(await Promise.all(["+7", ".5", "5.", "-150,000.50", "1,2,3", "12,34", "1.2.3", "-", "00", "1e5"].map(stored))).toEqual(["7", "0.5", "5", "-150000.50", "1,2,3", "12,34", "1.2.3", "-", "00", "1e5"]);

    const code = async (edit: EditRequest) => (await preview(book, [edit])).problems?.[0]?.code ?? null;
    expect(await code({ find: "D2", replace: "1", action: "set_cell" })).toBe("edit_cell_formula");
    expect(await code({ find: "없는시트!A1", replace: "1", action: "set_cell" })).toBe("edit_cell_address");
    expect(await code({ find: "둘째 칸", replace: "1", action: "set_cell" })).toBe("edit_cell_address");
    expect(await code({ find: "XFE1", replace: "1", action: "set_cell" })).toBe("edit_cell_address");
    // An emptied cell is a change too: D3 reads C3, and the sum and the total read on from there.
    expect(await preview(book, [{ find: "C3", replace: " ", action: "set_cell" }])).toMatchObject({ replaced: [1], notes: [{ edit: 0, code: "edit_formulas_cleared", count: 3 }] });
    // Paragraphs and table rows are a document's, cells a workbook's.
    await expect(preview(book, [{ find: "설계", replace: "x", action: "insert_after" }])).rejects.toMatchObject({ message: "edit_action_unsupported" });
    await expect(preview(ids["개발계약서.docx"]!, [{ find: "B2", replace: "1", action: "set_cell" }])).rejects.toMatchObject({ message: "edit_action_unsupported" });
  });

  // Step 4 (`documents::edit::pictures`): a picture is replaced inside its frame, in its own format.
  it("a picture of the file is replaced by another one; what cannot be written is refused", async () => {
    const ids = await importDocs(["제안서.pptx", "스캔본.pdf"]);
    const deck = ids["제안서.pptx"]!;
    const listed = await backend.mockInvoke<import("@/lib/types").EditPicture[]>("documents_edit_pictures", { id: deck });
    expect(listed.map((p) => [p.part, p.format, p.width, p.height, p.replaceable])).toEqual([
      ["ppt/media/image1.png", "png", 640, 360, true],
      ["ppt/media/image2.jpeg", "jpeg", 800, 600, true],
      ["ppt/media/image3.emf", "emf", 0, 0, false],
    ]);
    expect(listed[0]).toMatchObject({ used_in: ["ppt/slides/slide1.xml"], thumbnail: expect.stringMatching(/^data:image\//) });
    await expect(backend.mockInvoke("documents_edit_pictures", { id: ids["스캔본.pdf"]! })).rejects.toMatchObject({ message: "edit_unsupported_format" });

    const edits: EditRequest[] = [
      { find: "ppt/media/image1.png", replace: "D:\\그림\\새 로고 800x800.jpg", action: "replace_picture" },
      { find: "PPT/media/image2.jpeg", replace: "D:\\그림\\표지.jpeg", action: "replace_picture" },
      { find: "2쪽", replace: "둘째 장" },
    ];
    const report = await preview(deck, edits);
    expect(report.problems ?? []).toEqual([]);
    expect(report.replaced).toEqual([1, 1, 1]);
    expect(report.changes.filter((c) => c.action === "replace_picture").map((c) => [c.part, c.before, c.after])).toEqual([
      ["ppt/media/image1.png", "640×360 PNG", "800×800 JPEG → PNG"],
      ["PPT/media/image2.jpeg", "800×600 JPEG", "800×600 JPEG"],
    ]);
    // The square picture goes into a wide frame.
    expect(report.notes).toEqual([{ edit: 0, code: "edit_picture_ratio", count: 0 }]);
    expect(report.parts).toEqual(expect.arrayContaining(["ppt/media/image1.png", "ppt/slides/slide2.xml"]));
    const edition = await backend.mockInvoke<{ doc_id: string }>("documents_edit_save", { id: deck, edits });
    const after = await backend.mockInvoke<import("@/lib/types").EditPicture[]>("documents_edit_pictures", { id: edition.doc_id });
    expect(after.map((p) => [p.width, p.height])).toEqual([[800, 800], [800, 600], [0, 0]]);
    expect((await backend.mockInvoke<import("@/lib/types").EditPicture[]>("documents_edit_pictures", { id: deck }))[0]).toMatchObject({ width: 640, height: 360 });

    const code = async (edit: EditRequest) => (await preview(deck, [edit])).problems?.[0]?.code ?? null;
    expect(await code({ find: "ppt/media/image9.png", replace: "D:\\a.png", action: "replace_picture" })).toBe("edit_picture_missing");
    expect(await code({ find: "ppt/media/image3.emf", replace: "D:\\a.png", action: "replace_picture" })).toBe("edit_picture_format");
    expect(await code({ find: "ppt/media/image1.png", replace: "D:\\메모.txt", action: "replace_picture" })).toBe("edit_picture_unreadable");
    await expect(backend.mockInvoke("documents_edit_save", { id: deck, edits: [{ find: "ppt/media/image3.emf", replace: "D:\\a.png", action: "replace_picture" }] })).rejects.toMatchObject({ message: "edit_picture_format" });
  });
});
