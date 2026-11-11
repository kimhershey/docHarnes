// 문서 생성 store: per-workspace binding, the draft hand-over from the assistant / jobs page, "새로 만들기"
// and the draft on the screen against its stored copy (GEN-4 / GEN-7).
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Draft, DraftSummary } from "@/lib/types";

// `openDraft` reads the stored draft through the IPC layer: a draft store in memory stands in for it.
const stored = new Map<string, Draft>();
let clock = Date.parse("2026-11-11T09:00:00Z");
const tickClock = () => new Date((clock += 1000)).toISOString();
const summaryOf = (d: Draft): DraftSummary => ({ id: d.id, title: d.title, template: d.template, updated_at: d.updated_at, issues: d.issues.length, saved_path: d.saved_path });
const io = {
  get: vi.fn(async (id: string): Promise<Draft> => {
    const d = stored.get(id);
    if (!d) throw { code: "not_found", message: `not found: draft ${id}` };
    return structuredClone(d);
  }),
  // Like `generate::store::save`: the stored copy gets a new `updated_at`, the caller's object is left alone.
  put: vi.fn(async (draft: Draft): Promise<DraftSummary> => {
    const copy = { ...structuredClone(draft), updated_at: tickClock() };
    stored.set(copy.id, copy);
    return summaryOf(copy);
  }),
};
vi.mock("@/lib/ipc", () => ({ api: { generateDraftGet: (id: string) => io.get(id), generateDraftPut: (draft: Draft) => io.put(draft) } }));

const { persistDraft, resetGenerate, sameDraftContent, syncDraftWithStore, useGenerateStore } = await import("./generate");

const draft: Draft = {
  id: "d1",
  title: "견적서",
  template: { kind: "builtin", template: "quotation" },
  doc_type: "quotation",
  fields: [{ key: "note", label: "비고", kind: "string", required: false, value: "", normalized: null, source: null, user_edited: false }],
  tables: [],
  body: [{ kind: "heading", level: 2, text: "개요" }, { kind: "paragraph", text: "처음 생성한 문단.", source: null }],
  references: [],
  issues: [],
  language: "ko",
  tone: "concise",
  length: "short",
  created_at: "2026-11-11T08:00:00Z",
  updated_at: "2026-11-11T08:00:00Z",
  model: null,
  request: { form_ref: "quotation", format_description: null, content: "노트북 10대", sources: [], output: "pdf", tone: "concise", length: "short", language: "ko", use_knowledge: false, feedback: "더 짧게", previous: null, excluded_sources: ["x"] },
  notes: [],
  saved_path: null,
  saved_doc_id: null,
};
const state = () => useGenerateStore.getState();
/** Lets the sync started by `openDraft` (and the writes before it) settle. */
const settle = async () => {
  for (let i = 0; i < 5; i += 1) await syncDraftWithStore();
};

beforeEach(async () => {
  await settle();
  resetGenerate();
  stored.clear();
  io.get.mockClear();
  io.put.mockClear();
});

describe("generate store", () => {
  it("binds to a workspace, keeps a draft handed over before the first mount and starts over for another one", () => {
    state().openDraft(draft, { fresh: true });
    state().bind("C:\\\\A", "ko");
    expect(state().draft).toBe(draft);
    expect(state().handoff).toBe(true);
    state().setRequest({ content: "hello" });
    state().bind("C:\\\\A", "ko");
    expect(state().request.content).toBe("hello");
    state().bind("C:\\\\B", "en");
    expect(state().draft).toBeNull();
    expect(state().request).toMatchObject({ content: "", language: "en" });
  });

  it("opens a handed-over draft with its request (feedback / previous cleared) and announces it once", () => {
    state().openDraft(draft, { fresh: true });
    const s = state();
    expect(s.draft).toBe(draft);
    expect(s.handoff).toBe(true);
    expect(s.jobId).toBeNull();
    expect(s.request).toMatchObject({ form_ref: "quotation", content: "노트북 10대", output: "pdf", feedback: null, previous: null, excluded_sources: ["x"] });
    s.consumeHandoff();
    expect(state().handoff).toBe(false);
  });

  it("resets the draft but keeps the form preferences", () => {
    const s = state();
    s.setRequest({ form_ref: "report", output: "docx", content: "x", use_knowledge: false });
    s.setDraft(draft);
    s.setJobId("job");
    state().reset("ko");
    const after = state();
    expect(after.draft).toBeNull();
    expect(after.jobId).toBeNull();
    expect(after.request).toMatchObject({ form_ref: "report", output: "docx", content: "", use_knowledge: false });
  });

  it("updates the draft in place", () => {
    const s = state();
    s.setDraft(draft);
    s.updateDraft((d) => ({ ...d, title: "edited" }));
    expect(state().draft?.title).toBe("edited");
  });
});

describe("the draft on the screen and its stored copy", () => {
  it("GEN-4: a result card's snapshot is replaced by the saved draft when that is newer", async () => {
    // The user edited and saved the draft after the job finished; the card still holds the first version.
    const edited: Draft = { ...structuredClone(draft), fields: [{ ...draft.fields[0]!, value: "부가세 별도", user_edited: true }], updated_at: "2026-11-11T08:20:00Z" };
    stored.set(edited.id, edited);
    state().openDraft(draft);
    expect(state().draft).toBe(draft);
    await settle();
    expect(io.get).toHaveBeenCalledWith("d1");
    expect(state().draft?.fields[0]?.value).toBe("부가세 별도");
    expect(state()).toMatchObject({ dirty: false, reloaded: true, syncedAt: "2026-11-11T08:20:00Z", handoff: true });
    state().consumeReloaded();
    expect(state().reloaded).toBe(false);
  });

  it("keeps the snapshot when the stored copy is the same draft, is older, or is gone", async () => {
    // The job's own save: same content, a later timestamp — adopted without a notice.
    stored.set(draft.id, { ...structuredClone(draft), updated_at: "2026-11-11T08:00:02Z" });
    state().openDraft(draft);
    await settle();
    expect(state().draft).toBe(draft);
    expect(state()).toMatchObject({ reloaded: false, syncedAt: "2026-11-11T08:00:02Z" });
    // An older stored copy never wins.
    stored.set(draft.id, { ...structuredClone(draft), title: "옛 제목", updated_at: "2026-11-11T07:00:00Z" });
    state().openDraft(draft);
    await settle();
    expect(state().draft).toBe(draft);
    // Deleted from the draft store: the snapshot stays usable.
    stored.clear();
    state().openDraft(draft);
    await settle();
    expect(state().draft).toBe(draft);
    expect(state().reloaded).toBe(false);
  });

  it("GEN-7: an edit is written, then the assistant's revision of the stored draft is read back", async () => {
    state().setDraft(draft);
    const mine: Draft = { ...draft, title: "견적서 (수정)" };
    state().editDraft(mine);
    expect(state().dirty).toBe(true);
    // Leaving the screen flushes the pending autosave.
    expect(await persistDraft(mine)).toBe(true);
    expect(stored.get("d1")?.title).toBe("견적서 (수정)");
    expect(state()).toMatchObject({ dirty: false, syncedAt: stored.get("d1")!.updated_at });
    // Nothing changed elsewhere: coming back reads the store and keeps the screen's object.
    expect(await syncDraftWithStore()).toBe("unchanged");
    expect(state().draft).toBe(mine);
    // The assistant rewrites section 1 of the stored draft (which already has the edit).
    const revised: Draft = { ...structuredClone(stored.get("d1")!), body: [draft.body[0]!, { kind: "paragraph", text: "AI가 다시 쓴 문단.", source: null }], notes: ["AI 수정(1절 '개요'): 더 자세히"], updated_at: tickClock() };
    stored.set("d1", revised);
    expect(await syncDraftWithStore()).toBe("reloaded");
    expect(state().draft).toMatchObject({ title: "견적서 (수정)", notes: ["AI 수정(1절 '개요'): 더 자세히"] });
    expect(state()).toMatchObject({ dirty: false, reloaded: true, syncedAt: revised.updated_at });
  });

  it("never replaces unsaved edits: a dirty draft is written instead of read back", async () => {
    state().setDraft(draft);
    stored.set("d1", { ...structuredClone(draft), title: "다른 곳에서 고친 제목", updated_at: "2026-11-11T08:30:00Z" });
    const mine: Draft = { ...draft, title: "화면에서 고친 제목" };
    state().editDraft(mine);
    expect(await syncDraftWithStore()).toBe("written");
    expect(io.get).not.toHaveBeenCalled();
    expect(state().draft).toBe(mine);
    expect(stored.get("d1")?.title).toBe("화면에서 고친 제목");
    expect(state().dirty).toBe(false);
  });

  it("an edit made while the write is on its way stays dirty; a failed write keeps the draft dirty", async () => {
    state().setDraft(draft);
    const first: Draft = { ...draft, title: "첫 수정" };
    state().editDraft(first);
    const writing = persistDraft(first);
    const second: Draft = { ...first, title: "둘째 수정" };
    state().editDraft(second);
    await writing;
    expect(state()).toMatchObject({ dirty: true, draft: second });
    io.put.mockRejectedValueOnce({ code: "io", message: "disk full" });
    expect(await persistDraft(second)).toBe(false);
    expect(state().dirty).toBe(true);
    expect(await persistDraft(second)).toBe(true);
    expect(state().dirty).toBe(false);
  });

  it("does nothing without a draft or while a draft job runs (its result replaces the draft)", async () => {
    expect(await syncDraftWithStore()).toBe("none");
    state().setDraft(draft);
    stored.set("d1", { ...structuredClone(draft), title: "부분 초안", updated_at: "2026-11-11T08:30:00Z" });
    state().setJobId("job1");
    expect(await syncDraftWithStore()).toBe("none");
    expect(state().draft).toBe(draft);
  });

  it("compares drafts by content: key order, null vs missing and `updated_at` do not count", () => {
    const shuffled = Object.fromEntries(Object.entries({ ...draft, updated_at: "2026-11-11T10:00:00Z" }).reverse()) as unknown as Draft;
    expect(sameDraftContent(draft, shuffled)).toBe(true);
    const sparse = { ...draft } as Partial<Draft>;
    delete sparse.saved_path;
    delete sparse.model;
    expect(sameDraftContent(draft, sparse as Draft)).toBe(true);
    expect(sameDraftContent(draft, { ...draft, title: "다른 제목" })).toBe(false);
    expect(sameDraftContent(draft, { ...draft, body: [...draft.body].reverse() })).toBe(false);
  });
});
