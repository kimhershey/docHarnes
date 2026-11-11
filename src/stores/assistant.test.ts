// Assistant conversation store (0.7.0 §1.6): chat sessions hydrated from the backend, messages
// written to the session a request started in, the owed `ui_actions` of a job-backed plan claimed
// exactly once. The `chat_*` commands are faked in memory.
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ChatMessageRow, ChatSessionSummary, Op, OrganizeItem, OrganizePlan, Plan, PlanContext, PlanExecution, PlanResult } from "@/lib/types";

const fake = vi.hoisted(() => {
  // One workspace database at a time, like the backend: `switchWorkspace` opens another (empty) one.
  const state = {
    sessions: [] as ChatSessionSummary[],
    messages: {} as Record<string, ChatMessageRow[]>,
    seq: 1,
    puts: 0,
    clock: 0,
    log: [] as string[],
    /** While set, `chatMessagePut` / `chatMessagesList` wait for it (a call still on its way). */
    putGate: null as Promise<void> | null,
    listGate: null as Promise<void> | null,
    /** This many `chatMessagesList` calls fail. */
    listFails: 0,
  };
  const stamp = () => new Date(Date.UTC(2026, 10, 10, 0, 0, state.clock++)).toISOString();
  const api = {
    chatSessionsList: async () =>
      [...state.sessions].sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map((s) => ({ ...s, message_count: (state.messages[s.id] ?? []).length })),
    chatSessionCreate: async (title: string | null) => {
      const now = stamp();
      const s: ChatSessionSummary = { id: `s${state.seq++}`, title: title ?? "", title_auto: !title, created_at: now, updated_at: now, message_count: 0 };
      state.sessions.push(s);
      state.messages[s.id] = [];
      return { ...s };
    },
    chatSessionRename: async (id: string, title: string) => {
      const s = state.sessions.find((x) => x.id === id);
      if (!s) return;
      s.title = title;
      s.title_auto = false;
    },
    chatSessionDelete: async (id: string) => {
      state.log.push(`delete ${id}`);
      state.sessions = state.sessions.filter((x) => x.id !== id);
      delete state.messages[id];
    },
    chatMessagesList: async (id: string) => {
      if (state.listGate) await state.listGate;
      if (state.listFails > 0) {
        state.listFails -= 1;
        throw { code: "internal", message: "database is locked" };
      }
      return [...(state.messages[id] ?? [])];
    },
    // Like the core (`chat::put_message`): a session this database does not have is re-created, untitled.
    chatMessagePut: async (id: string, seq: number, payload: unknown) => {
      if (state.putGate) await state.putGate;
      // Like `chat_message_put` (`commands/knowledge.rs`): a message above 2,000,000 bytes of JSON is refused.
      if (new TextEncoder().encode(JSON.stringify(payload)).length > 2_000_000) {
        state.log.push(`too large ${id}/${seq}`);
        throw { code: "invalid", message: "chat_message_too_large" };
      }
      state.log.push(`put ${id}/${seq}`);
      state.puts += 1;
      let s = state.sessions.find((x) => x.id === id);
      if (!s) {
        s = { id, title: "", title_auto: true, created_at: stamp(), updated_at: stamp(), message_count: 0 };
        state.sessions.push(s);
      }
      const rows = state.messages[id] ?? [];
      const row = rows.find((r) => r.seq === seq);
      if (row) row.payload = payload;
      else rows.push({ seq, payload, created_at: stamp() });
      state.messages[id] = rows;
      s.updated_at = stamp();
      const p = payload as { role?: string; text?: string };
      if (s.title_auto && s.title === "" && p.role === "user" && p.text) s.title = p.text.slice(0, 40);
    },
  };
  const switchWorkspace = () => {
    state.sessions = [];
    state.messages = {};
  };
  const reset = () => {
    switchWorkspace();
    state.seq = 1;
    state.puts = 0;
    state.clock = 0;
    state.log = [];
    state.putGate = null;
    state.listGate = null;
    state.listFails = 0;
  };
  return { state, api, reset, switchWorkspace };
});

vi.mock("@/lib/ipc", () => ({ api: fake.api }));

import { useAttachmentsStore } from "./attachments";
import { DROPPED, PATCH_WRITE_DELAY_MS, flushChatWrites, resetAssistant, restoreMessage, selectMessages, storedMessage, useAssistantStore, type Message, type NewMessage, type PlanMessage } from "./assistant";

const theme: Op = { op: "set_theme", theme: "dark" };
const open: Op = { op: "open", target: "screen", ref: "jobs" };

const jsonBytes = (value: unknown) => new TextEncoder().encode(JSON.stringify(value)).length;
/** Document ids as the core makes them: 32 hex characters. */
const docIdOf = (i: number) => i.toString(16).padStart(32, "0");
const planContext: PlanContext = { today: "2026-11-11", language: "ko", folders: [], doc_types: [], schemas: [], tags: [], current_folder: null, selected_doc_ids: [], current_document: null, current_page: null, document_count: 0, model_ids: [], projects: [], current_project: null, history: [] };
const planResult: Omit<PlanResult, "plan"> = { text: "", source: "model", issues: [], date: null, attempts: 1, model: "gemma4-e4b-q5km", prompt_tokens: 900, completion_tokens: 40 };
const tagPlan: Plan = { intent: "tag", scope: {}, operations: [{ op: "tag", add: ["2025"] }], requires_confirmation: true, confidence: 0.9, summary: "태그 2025을(를) 추가합니다" };

type NewPlanCard = Extract<NewMessage, { kind: "plan" }>;

/** A "월별 폴더로 옮겨" plan card over `count` receipts as the planner returns it: the preview lists every file. */
function organisePlanCard(count: number): NewPlanCard {
  const items: OrganizeItem[] = Array.from({ length: count }, (_, i) => {
    const n = String(i + 1).padStart(5, "0");
    const name = `2026-10-03 한빛소프트 유지보수 견적서 ${n}.pdf`;
    return { doc_id: docIdOf(i), name, from: `inbox/2026-10/스캔 문서/${name}`, to: `documents/2026/10/견적서/2026-10-03_견적서_한빛소프트_12500000_${n}.pdf`, status: "move", reason: null, missing: [] };
  });
  const plan: Plan = { intent: "move", scope: { filter: { doc_type: "estimate" } }, operations: [{ op: "move", folder: "documents/{{year}}/{{month}}/{{doc_type}}", group_by: "month" }], requires_confirmation: true, confidence: 0.86, summary: "견적서을(를) 월별 폴더로 이동합니다" };
  const preview: OrganizePlan = { items, moves: count, unchanged: 0, held: 0, missing_files: 0, needs_review: 0, name_template: null, folder_template: "documents/{{year}}/{{month}}/{{doc_type}}", only_reviewed: true };
  return {
    role: "assistant",
    kind: "plan",
    text: "견적서 월별 폴더로 옮겨",
    response: {
      result: { ...planResult, text: "견적서 월별 폴더로 옮겨", plan },
      preview: { document_count: count, document_ids: items.map((i) => i.doc_id), steps: [{ index: 0, op: "move", summary: "", payload: preview, confirm: true }], requires_confirmation: true },
      context: { ...planContext, document_count: count },
    },
    plan,
    context: {},
    status: "pending",
    errors: null,
    editing: false,
  };
}

async function settled(): Promise<void> {
  for (let i = 0; i < 50 && useAssistantStore.getState().hydrating; i += 1) await new Promise((r) => setTimeout(r, 0));
  await new Promise((r) => setTimeout(r, 0));
}

async function boundTo(workspace: string): Promise<string> {
  useAssistantStore.getState().bind(workspace);
  await settled();
  const id = useAssistantStore.getState().activeId;
  if (!id) throw new Error("no active session after hydrate");
  return id;
}

beforeEach(() => {
  fake.reset();
  resetAssistant();
  useAttachmentsStore.setState({ attached: [], attachedBySession: {} });
});

describe("assistant store", () => {
  it("hydrates a workspace: the first session is created, messages are written to it and title it", async () => {
    const sid = await boundTo("C:\\Work\\Docs");
    const s = useAssistantStore.getState();
    expect(s.sessions).toHaveLength(1);
    expect(s.hydrating).toBe(false);
    const a = s.push(sid, { role: "user", text: "inbox 처리" });
    const b = s.push(sid, { role: "assistant", kind: "help" });
    expect(b).toBeGreaterThan(a);
    expect(selectMessages(useAssistantStore.getState()).map((m) => m.id)).toEqual([a, b]);
    await settled();
    expect(fake.state.puts).toBe(2);
    expect(fake.state.messages[sid]!.map((r) => r.seq)).toEqual([a, b]);
    // The summary follows: auto title from the first user message, count, recency.
    expect(useAssistantStore.getState().sessions[0]).toMatchObject({ id: sid, title: "inbox 처리", message_count: 2 });
    // Deferred patches (the plan editor's keystrokes) are written together once they settle: the latest state.
    s.patch(sid, a, (m) => (m.role === "user" ? { ...m, text: "edit" } : m), { defer: true });
    s.patch(sid, a, (m) => (m.role === "user" ? { ...m, text: "edited" } : m), { defer: true });
    expect(selectMessages(useAssistantStore.getState())[0]).toMatchObject({ id: a, text: "edited" });
    await settled();
    expect(fake.state.puts).toBe(2);
    await flushChatWrites();
    expect(fake.state.puts).toBe(3);
    expect((fake.state.messages[sid]![0]!.payload as { text: string }).text).toBe("edited");
    // Any other patch is written at once.
    s.patch(sid, a, (m) => (m.role === "user" ? { ...m, text: "now" } : m));
    await settled();
    expect(fake.state.puts).toBe(4);
    expect((fake.state.messages[sid]![0]!.payload as { text: string }).text).toBe("now");
  });

  // KNO-9: the state a card settles in is stored when it changes, not 300 ms later — a workspace switch or
  // the app closing in between restored a finished plan as "승인 대기" (Ctrl+Enter ran it again) and a
  // finished answer as "중단됨".
  it("stores a card's settled state at once: a restart restores the plan as executed and the answer as answered", async () => {
    const sid = await boundTo("C:\\Work\\Docs");
    const s = useAssistantStore.getState();
    const plan = s.push(sid, { role: "assistant", kind: "plan", text: "견적서 옮겨", response: { context: {} } as never, plan: { operations: [] } as never, context: {}, status: "pending", errors: null, editing: false });
    const answer = s.push(sid, { role: "assistant", kind: "answer", requestId: "r1", request: { question: "q" } as never, answer: null, error: null, pending: true });
    s.patch(sid, plan, (m) => (m.role === "assistant" && m.kind === "plan" ? { ...m, status: "executing" } : m));
    s.patch(sid, plan, (m) => (m.role === "assistant" && m.kind === "plan" ? { ...m, status: "done" } : m));
    s.patch(sid, answer, (m) => (m.role === "assistant" && m.kind === "answer" ? { ...m, pending: false, answer: { text: "답", sources: [], sentences: [] } as never } : m));
    // No timer has to pass: a few ticks for the calls already made.
    await settled();
    const stored = fake.state.messages[sid]!.map((r) => r.payload as { status?: string; pending?: boolean });
    expect(stored[0]).toMatchObject({ status: "done" });
    expect(stored[1]).toMatchObject({ pending: false });
    // "Restart": the store is rebuilt from what was stored.
    resetAssistant();
    await boundTo("C:\\Work\\Docs");
    const restored = selectMessages(useAssistantStore.getState());
    expect(restored[0]).toMatchObject({ kind: "plan", status: "done" });
    expect(restored[1]).toMatchObject({ kind: "answer", pending: false, error: null });
  });

  // PLN-7: the 되묻기 text a plan was made from stays on the card — also across a restart, when the plan is
  // approved later.
  it("keeps the text a plan was made from on the stored card", async () => {
    const sid = await boundTo("C:\\Work\\Docs");
    const composed = "태그 '긴급' 달아줘\n\n[어시스턴트 질문] 어떤 문서를 대상으로 할까요?\n[사용자 답변] 전부";
    useAssistantStore.getState().push(sid, { role: "assistant", kind: "plan", text: "태그 '긴급' 달아줘", plannedText: composed, response: { context: {} } as never, plan: { operations: [] } as never, context: {}, status: "pending", errors: null, editing: false });
    await flushChatWrites();
    expect(fake.state.messages[sid]![0]!.payload).toMatchObject({ text: "태그 '긴급' 달아줘", plannedText: composed });
    resetAssistant();
    await boundTo("C:\\Work\\Docs");
    expect(selectMessages(useAssistantStore.getState())[0]).toMatchObject({ kind: "plan", status: "pending", plannedText: composed });
  });

  it("flushChatWrites sends what waits and resolves when everything is stored", async () => {
    const sid = await boundTo("C:\\Work\\Docs");
    const s = useAssistantStore.getState();
    const id = s.push(sid, { role: "user", text: "a" });
    s.patch(sid, id, (m) => (m.role === "user" ? { ...m, text: "typing" } : m), { defer: true });
    await flushChatWrites();
    expect((fake.state.messages[sid]![0]!.payload as { text: string }).text).toBe("typing");
    // Nothing left: a second flush is a no-op.
    const puts = fake.state.puts;
    await flushChatWrites();
    expect(fake.state.puts).toBe(puts);
  });

  it("writes into the session a request started in, not the one selected meanwhile", async () => {
    const first = await boundTo("C:\\Work\\Docs");
    const s = useAssistantStore.getState();
    s.push(first, { role: "user", text: "질문" });
    const second = await s.newSession();
    expect(second).not.toBe(first);
    expect(useAssistantStore.getState().activeId).toBe(second);
    // A late answer of the first session lands there.
    const id = s.push(first, { role: "assistant", kind: "executed", text: "done", source: "rule", confidence: 1 });
    s.patch(first, id, (m) => (m.role === "assistant" && m.kind === "executed" ? { ...m, text: "done!" } : m));
    expect(selectMessages(useAssistantStore.getState())).toEqual([]);
    expect(useAssistantStore.getState().messagesBySession[first]!.map((m) => m.id)).toHaveLength(2);
    // The newest session is listed first; the empty active session is reused by the next 새 채팅.
    expect(useAssistantStore.getState().sessions[0]!.id).toBe(first); // just written to
    expect(await useAssistantStore.getState().newSession()).toBe(second);
    expect(useAssistantStore.getState().sessions).toHaveLength(2);
  });

  it("restores stored sessions: the remembered or newest one is active, pending cards are settled", async () => {
    const a = await fake.api.chatSessionCreate("old");
    const b = await fake.api.chatSessionCreate("new");
    await fake.api.chatMessagePut(a.id, 1, { id: 1, role: "user", text: "q" });
    await fake.api.chatMessagePut(a.id, 2, { id: 2, role: "assistant", kind: "answer", requestId: "r", request: { question: "q" }, answer: null, error: null, pending: true });
    await fake.api.chatMessagePut(a.id, 3, { id: 3, role: "assistant", kind: "plan", text: "x", response: {}, plan: {}, context: {}, status: "executing", errors: null, editing: true });
    await fake.api.chatMessagePut(b.id, 7, { id: 7, role: "user", text: "later" });
    const active = await boundTo("C:\\Work\\Docs");
    expect(active).toBe(b.id);
    expect(useAssistantStore.getState().sessions.map((s) => s.id)).toEqual([b.id, a.id]);
    expect(selectMessages(useAssistantStore.getState())).toHaveLength(1);
    // The other session loads when selected; ids continue after the stored ones.
    await useAssistantStore.getState().select(a.id);
    const restored = selectMessages(useAssistantStore.getState());
    expect(restored.map((m) => m.id)).toEqual([1, 2, 3]);
    expect(restored[1]).toMatchObject({ kind: "answer", pending: false, error: { code: "chat_interrupted" } });
    expect(restored[2]).toMatchObject({ kind: "plan", status: "pending", editing: false });
    expect(useAssistantStore.getState().nextId).toBeGreaterThan(7);
  });

  it("renames and deletes sessions; deleting the active one moves on or starts afresh", async () => {
    const first = await boundTo("C:\\Work\\Docs");
    const s = useAssistantStore.getState();
    s.push(first, { role: "user", text: "a" });
    const second = (await s.newSession())!;
    await s.rename(second, "  둘째  ");
    expect(useAssistantStore.getState().sessions.find((x) => x.id === second)).toMatchObject({ title: "둘째", title_auto: false });
    await s.remove(second);
    expect(useAssistantStore.getState().activeId).toBe(first);
    expect(useAssistantStore.getState().sessions.map((x) => x.id)).toEqual([first]);
    await s.remove(first);
    const fresh = useAssistantStore.getState();
    expect(fresh.sessions).toHaveLength(1);
    expect(fresh.activeId).toBe(fresh.sessions[0]!.id);
    expect(fresh.activeId).not.toBe(first);
    expect(selectMessages(fresh)).toEqual([]);
  });

  it("keeps attachments per session (what was attached before hydration stays)", async () => {
    useAttachmentsStore.getState().attach([{ id: "d0", name: "early.pdf" } as never]);
    const first = await boundTo("C:\\Work\\Docs");
    expect(useAttachmentsStore.getState().attached.map((d) => d.id)).toEqual(["d0"]);
    useAttachmentsStore.getState().detach("d0");
    const att = useAttachmentsStore.getState();
    att.attach([{ id: "d1", name: "a.pdf" } as never]);
    useAssistantStore.getState().push(first, { role: "user", text: "x" });
    const second = (await useAssistantStore.getState().newSession())!;
    expect(useAttachmentsStore.getState().attached).toEqual([]);
    await useAssistantStore.getState().select(first);
    expect(useAttachmentsStore.getState().attached.map((d) => d.id)).toEqual(["d1"]);
    await useAssistantStore.getState().select(second);
    expect(useAttachmentsStore.getState().attached).toEqual([]);
  });

  it("claims the ui_actions of a tracked job exactly once", () => {
    const s = useAssistantStore.getState();
    s.trackJob("job1", [theme]);
    // The execution's list wins over the provisional plan ops.
    expect(useAssistantStore.getState().claimUiActions("job1", [theme, open])).toEqual([theme, open]);
    expect(useAssistantStore.getState().claimUiActions("job1", [theme, open])).toBeNull();
    expect(useAssistantStore.getState().claimUiActions("job1")).toBeNull();
    expect(useAssistantStore.getState().pendingUiActions).toEqual({});
  });

  it("falls back to the tracked plan ops and ignores unknown jobs", () => {
    const s = useAssistantStore.getState();
    s.trackJob("job2", [open]);
    expect(useAssistantStore.getState().claimUiActions("unknown")).toBeNull();
    expect(useAssistantStore.getState().claimUiActions("job2")).toEqual([open]);
    expect(useAssistantStore.getState().claimUiActions("job2")).toBeNull();
  });

  it("keeps owed actions across 새 채팅 but drops them with the workspace", async () => {
    const first = await boundTo("C:\\Work\\Docs");
    const s = useAssistantStore.getState();
    s.push(first, { role: "user", text: "x" });
    s.trackJob("job3", [theme]);
    await s.newSession();
    expect(selectMessages(useAssistantStore.getState())).toEqual([]);
    expect(useAssistantStore.getState().pendingUiActions).toEqual({ job3: [theme] });
    // Same workspace: nothing changes; another one starts afresh (and hydrates its own sessions).
    useAssistantStore.getState().bind("C:\\Work\\Docs");
    expect(useAssistantStore.getState().pendingUiActions).toEqual({ job3: [theme] });
    useAssistantStore.getState().bind("D:\\Other");
    expect(useAssistantStore.getState().pendingUiActions).toEqual({});
    expect(useAssistantStore.getState().workspace).toBe("D:\\Other");
    await settled();
    useAssistantStore.getState().trackJob("job4", [open]);
    resetAssistant();
    expect(useAssistantStore.getState()).toMatchObject({ workspace: null, sessions: [], activeId: null, pendingUiActions: {} });
  });

  // KNO-2: a request that outlives its conversation must write nowhere — the backend is on the other
  // workspace's database by then and would re-create the unknown session as an untitled ghost chat.
  it("drops what a request writes after the workspace was switched", async () => {
    const first = await boundTo("C:\\Work\\Docs");
    const generation = useAssistantStore.getState().generation;
    const asked = useAssistantStore.getState().push(first, { role: "user", text: "워크스페이스 D:\\Other 열어" }, generation);
    expect(asked).not.toBe(DROPPED);
    await settled();
    // The backend opens the other workspace, then the UI follows (`resetDocumentQueries`, then the page binds).
    fake.switchWorkspace();
    resetAssistant();
    const other = await boundTo("D:\\Other");
    expect(other).not.toBe(first);
    expect(useAssistantStore.getState().generation).not.toBe(generation);
    const puts = fake.state.puts;
    const s = useAssistantStore.getState();
    // The cards of the request that started in the first workspace arrive now.
    expect(s.accepts(first, generation)).toBe(false);
    expect(s.push(first, { role: "assistant", kind: "executed", text: "워크스페이스를 열었습니다", source: "model", confidence: 0.9 }, generation)).toBe(DROPPED);
    s.patch(first, asked, (m) => (m.role === "user" ? { ...m, text: "late" } : m), { generation });
    // The session id of another workspace is refused whatever the generation (it is not one of this workspace's chats)…
    expect(s.push(first, { role: "assistant", kind: "help" })).toBe(DROPPED);
    // …and a stale request cannot write into a chat of the new workspace either.
    expect(s.push(other, { role: "assistant", kind: "help" }, generation)).toBe(DROPPED);
    await flushChatWrites();
    expect(fake.state.puts).toBe(puts);
    expect(fake.state.sessions.map((x) => x.id)).toEqual([other]);
    expect(useAssistantStore.getState().messagesBySession[first]).toBeUndefined();
    expect(selectMessages(useAssistantStore.getState())).toEqual([]);
  });

  it("does not write a patch that was still waiting when the workspace changed", async () => {
    const first = await boundTo("C:\\Work\\Docs");
    const s = useAssistantStore.getState();
    const id = s.push(first, { role: "user", text: "a" });
    await settled();
    s.patch(first, id, (m) => (m.role === "user" ? { ...m, text: "edited" } : m), { defer: true });
    const puts = fake.state.puts;
    fake.switchWorkspace();
    resetAssistant();
    await new Promise((r) => setTimeout(r, PATCH_WRITE_DELAY_MS + 50));
    await flushChatWrites();
    expect(fake.state.puts).toBe(puts);
    expect(fake.state.sessions).toEqual([]);
  });

  it("ignores late writes for a deleted chat and deletes only after the writes on their way", async () => {
    const first = await boundTo("C:\\Work\\Docs");
    const s = useAssistantStore.getState();
    const generation = s.generation;
    s.push(first, { role: "user", text: "질문" });
    await settled();
    const second = (await s.newSession())!;
    // A message of the first chat is still being written when the chat is deleted: the delete waits for it
    // (the other order would let the backend re-create the session).
    let open = () => {};
    fake.state.putGate = new Promise<void>((r) => (open = r));
    fake.state.log = [];
    const answer = s.push(first, { role: "assistant", kind: "executed", text: "done", source: "rule", confidence: 1 });
    const removed = s.remove(first);
    // While the delete is under way nothing more is accepted.
    expect(useAssistantStore.getState().accepts(first, generation)).toBe(false);
    expect(s.push(first, { role: "assistant", kind: "help" }, generation)).toBe(DROPPED);
    await settled();
    expect(fake.state.log).toEqual([]);
    fake.state.putGate = null;
    open();
    await removed;
    expect(fake.state.log).toEqual([`put ${first}/${answer}`, `delete ${first}`]);
    // The plan / answer of a request that was running arrives after the delete.
    const puts = fake.state.puts;
    expect(useAssistantStore.getState().push(first, { role: "assistant", kind: "help" }, generation)).toBe(DROPPED);
    useAssistantStore.getState().patch(first, answer, (m) => m, { generation });
    await flushChatWrites();
    expect(fake.state.puts).toBe(puts);
    expect(fake.state.sessions.map((x) => x.id)).toEqual([second]);
    expect(useAssistantStore.getState().messagesBySession[first]).toBeUndefined();
    expect(useAssistantStore.getState().activeId).toBe(second);
  });

  it("never has two writes of one message in flight: the newer state follows the older one", async () => {
    const first = await boundTo("C:\\Work\\Docs");
    const s = useAssistantStore.getState();
    let open = () => {};
    fake.state.putGate = new Promise<void>((r) => (open = r));
    const id = s.push(first, { role: "user", text: "v1" });
    s.patch(first, id, (m) => (m.role === "user" ? { ...m, text: "v2" } : m));
    const flushed = flushChatWrites();
    await settled();
    expect(fake.state.puts).toBe(0);
    fake.state.putGate = null;
    open();
    await flushed;
    expect(fake.state.log).toEqual([`put ${first}/${id}`, `put ${first}/${id}`]);
    expect((fake.state.messages[first]![0]!.payload as { text: string }).text).toBe("v2");
  });

  // KNO-15: message ids continue after the highest stored one, which is only known once the session's
  // messages were read. A message sent before that (or after the read failed) got a low id and replaced
  // the stored message with the same number.
  it("does not accept a message while the chat's stored messages are still being read", async () => {
    const a = await fake.api.chatSessionCreate("old");
    for (let seq = 1; seq <= 5; seq += 1) await fake.api.chatMessagePut(a.id, seq, { id: seq, role: "user", text: `old ${seq}` });
    const b = await fake.api.chatSessionCreate("new");
    await fake.api.chatMessagePut(b.id, 1, { id: 1, role: "user", text: "later" });
    expect(await boundTo("C:\\Work\\Docs")).toBe(b.id);
    // The other chat is selected; its messages are on their way.
    let open = () => {};
    fake.state.listGate = new Promise<void>((r) => (open = r));
    const selected = useAssistantStore.getState().select(a.id);
    expect(useAssistantStore.getState().activeId).toBe(a.id);
    expect(useAssistantStore.getState().loaded[a.id]).toBeFalsy();
    expect(useAssistantStore.getState().push(a.id, { role: "user", text: "too early" })).toBe(DROPPED);
    fake.state.listGate = null;
    open();
    await selected;
    await flushChatWrites();
    expect(fake.state.messages[a.id]!.map((r) => (r.payload as { text: string }).text)).toEqual(["old 1", "old 2", "old 3", "old 4", "old 5"]);
    // Once read, the next message continues after the stored ones.
    const id = useAssistantStore.getState().push(a.id, { role: "user", text: "now" });
    expect(id).toBeGreaterThan(5);
  });

  it("keeps a chat whose messages could not be read closed for writing until a reload succeeds", async () => {
    const a = await fake.api.chatSessionCreate(null);
    for (let seq = 1; seq <= 3; seq += 1) await fake.api.chatMessagePut(a.id, seq, { id: seq, role: "user", text: `stored ${seq}` });
    fake.state.listFails = 1;
    const sid = await boundTo("C:\\Work\\Docs");
    expect(sid).toBe(a.id);
    const s = useAssistantStore.getState();
    expect(s.hydrating).toBe(false);
    expect(s.loaded[sid]).toBeFalsy();
    expect(s.loadFailed[sid]).toBe(true);
    // Nothing may be written: its id would start at 1 again and replace "stored 1".
    expect(s.push(sid, { role: "user", text: "overwrites?" })).toBe(DROPPED);
    await flushChatWrites();
    expect((fake.state.messages[sid]![0]!.payload as { text: string }).text).toBe("stored 1");
    // 다시 시도: the messages are read, the chat opens for writing after them.
    await useAssistantStore.getState().reload(sid);
    const after = useAssistantStore.getState();
    expect(after.loaded[sid]).toBe(true);
    expect(after.loadFailed[sid]).toBeFalsy();
    expect(selectMessages(after).map((m) => m.id)).toEqual([1, 2, 3]);
    expect(after.push(sid, { role: "user", text: "fine" })).toBeGreaterThan(3);
    await flushChatWrites();
    expect(fake.state.messages[sid]!.slice(0, 3).map((r) => (r.payload as { text: string }).text)).toEqual(["stored 1", "stored 2", "stored 3"]);
  });

  // CON-16: the preview of a file-op plan lists every file. Above ~5,000 documents the card is more than the
  // 2,000,000 bytes `chat_message_put` takes: it was not stored (a console warning only) and was gone from
  // the chat after a restart.
  it("stores a plan card whose preview lists thousands of files: the first rows and the counts", async () => {
    const sid = await boundTo("C:\\Work\\Docs");
    const s = useAssistantStore.getState();
    s.push(sid, { role: "user", text: "견적서 월별 폴더로 옮겨" });
    const card = organisePlanCard(6_000);
    expect(jsonBytes(card)).toBeGreaterThan(2_000_000);
    const id = s.push(sid, card);
    await flushChatWrites();
    expect(fake.state.log.filter((l) => l.startsWith("too large"))).toEqual([]);
    expect(fake.state.messages[sid]!.map((r) => r.seq)).toContain(id);
    // The card on screen is not touched: every row is there until the app closes.
    const live = selectMessages(useAssistantStore.getState()).find((m) => m.id === id) as PlanMessage;
    expect((live.response.preview!.steps[0]!.payload as OrganizePlan).items).toHaveLength(6_000);
    expect(live.clipped).toBeUndefined();

    resetAssistant();
    await boundTo("C:\\Work\\Docs");
    const restored = selectMessages(useAssistantStore.getState()).find((m) => m.id === id) as PlanMessage;
    expect(restored).toMatchObject({ kind: "plan", status: "pending", clipped: "lists" });
    // The plan itself is whole — it can still be approved — and so are the numbers of its preview.
    expect(restored.plan).toEqual(card.plan);
    expect(restored.response.result.plan).toEqual(card.plan);
    expect(restored.response.preview).toMatchObject({ document_count: 6_000, document_ids_total: 6_000 });
    const payload = restored.response.preview!.steps[0]!.payload as OrganizePlan & { items_total: number };
    expect(payload).toMatchObject({ moves: 6_000, held: 0, unchanged: 0, items_total: 6_000 });
    expect(payload.items).toEqual((card.response.preview!.steps[0]!.payload as OrganizePlan).items.slice(0, 200));
    // Approved after the restart, the stored card keeps what it has: the totals are not counted again from the rows.
    useAssistantStore.getState().patch(sid, id, (m) => (m.role === "assistant" && m.kind === "plan" ? { ...m, status: "done" } : m));
    await flushChatWrites();
    const stored = fake.state.messages[sid]!.find((r) => r.seq === id)!.payload as PlanMessage;
    expect(stored).toMatchObject({ status: "done", clipped: "lists" });
    expect(stored.response.preview!.steps[0]!.payload).toMatchObject({ items_total: 6_000 });
  });

  it("stores a result card with thousands of moved files, keeping what its buttons need", async () => {
    const sid = await boundTo("C:\\Work\\Docs");
    const plan = organisePlanCard(6_000);
    const items = (plan.response.preview!.steps[0]!.payload as OrganizePlan).items.map((item) => ({ ...item, status: "done" as const }));
    const execution: PlanExecution = {
      steps: [{ index: 0, op: "move", status: "done", summary: "6,000건 이동", payload: { batch_id: "b1", moved: 6_000, held: 0, failed: [], items }, elapsed_ms: 1 }],
      cancelled: false,
      documents: items.map((i) => i.doc_id),
      ui_actions: [theme],
      batches: ["b1"],
    };
    const id = useAssistantStore.getState().push(sid, { role: "assistant", kind: "result", text: plan.text, response: plan.response, plan: plan.plan, execution });
    await flushChatWrites();
    expect(fake.state.log.filter((l) => l.startsWith("too large"))).toEqual([]);
    resetAssistant();
    await boundTo("C:\\Work\\Docs");
    const restored = selectMessages(useAssistantStore.getState()).find((m) => m.id === id) as Extract<Message, { kind: "result" }>;
    expect(restored).toMatchObject({ kind: "result", clipped: "lists" });
    // Undo works from the batch id; the UI ops and the plan are whole.
    expect(restored.execution).toMatchObject({ batches: ["b1"], ui_actions: [theme], cancelled: false, documents_total: 6_000 });
    expect(restored.plan).toEqual(plan.plan);
    const payload = restored.execution.steps[0]!.payload as { items: unknown[] };
    expect(payload).toMatchObject({ batch_id: "b1", moved: 6_000, items_total: 6_000, failed: [] });
    expect(payload.items).toHaveLength(200);
  });

  it("a plan on more selected documents than a message can hold is stored as a record that cannot run", async () => {
    const sid = await boundTo("C:\\Work\\Docs");
    const ids = Array.from({ length: 30_000 }, (_, i) => docIdOf(i));
    const plan: Plan = { ...tagPlan, scope: { doc_ids: ids } };
    const card: NewPlanCard = {
      role: "assistant",
      kind: "plan",
      text: "선택 문서에 태그 '2025' 달아줘",
      response: {
        result: { ...planResult, text: "선택 문서에 태그 '2025' 달아줘", plan },
        preview: { document_count: ids.length, document_ids: ids, steps: [{ index: 0, op: "tag", summary: "문서 30000건", payload: { documents: ids.length }, confirm: true }], requires_confirmation: true },
        context: { ...planContext, selected_doc_ids: ids },
      },
      plan,
      context: { selected_doc_ids: ids },
      status: "pending",
      errors: null,
      editing: true,
    };
    const id = useAssistantStore.getState().push(sid, card);
    await flushChatWrites();
    expect(fake.state.log.filter((l) => l.startsWith("too large"))).toEqual([]);
    // In this session the card is whole and can be approved.
    expect(selectMessages(useAssistantStore.getState()).find((m) => m.id === id)).toMatchObject({ status: "pending", plan: { scope: { doc_ids: ids } } });
    resetAssistant();
    await boundTo("C:\\Work\\Docs");
    const restored = selectMessages(useAssistantStore.getState()).find((m) => m.id === id) as PlanMessage;
    // After a restart it is a record: the ids are a count, so nothing may run from it.
    expect(restored).toMatchObject({ kind: "plan", status: "cancelled", editing: false, clipped: "all", text: "선택 문서에 태그 '2025' 달아줘" });
    expect(restored.plan.scope).toMatchObject({ doc_ids_total: 30_000 });
    expect(restored.plan.scope.doc_ids).toEqual(ids.slice(0, 10));
    expect(restored.plan.operations).toEqual(plan.operations);
    expect(restored.response.preview).toMatchObject({ document_count: 30_000 });
    // Once it ran in this session, the record says so.
    resetAssistant();
    fake.reset();
    const again = await boundTo("C:\\Work\\Docs");
    const ran = useAssistantStore.getState().push(again, card);
    useAssistantStore.getState().patch(again, ran, (m) => (m.role === "assistant" && m.kind === "plan" ? { ...m, status: "done", editing: false } : m));
    await flushChatWrites();
    expect(fake.state.messages[again]!.find((r) => r.seq === ran)!.payload).toMatchObject({ status: "done", clipped: "all" });
  });

  it("a message that is too large in one text is stored cut, not dropped", async () => {
    const sid = await boundTo("C:\\Work\\Docs");
    // Ten UTF-16 units per repetition, the emoji's two at 7–8: the 20,000th unit is the first half of an emoji.
    const pasted = `가나${"가나다라마바사😀 ".repeat(80_000)}`;
    expect(pasted.charCodeAt(19_999)).toBe(0xd83d);
    expect(jsonBytes({ role: "user", text: pasted })).toBeGreaterThan(2_000_000);
    const id = useAssistantStore.getState().push(sid, { role: "user", text: pasted });
    await flushChatWrites();
    expect(fake.state.log.filter((l) => l.startsWith("too large"))).toEqual([]);
    const stored = fake.state.messages[sid]!.find((r) => r.seq === id)!.payload as { text: string; clipped?: string };
    expect(stored.clipped).toBe("all");
    expect(stored.text.endsWith("…")).toBe(true);
    expect(pasted.startsWith(stored.text.slice(0, -1))).toBe(true);
    // Whole characters only: half of a surrogate pair is not valid JSON for the backend.
    expect(stored.text).toHaveLength(20_000);
    expect(/[\ud800-\udbff](?![\udc00-\udfff])/.test(stored.text)).toBe(false);
    expect(jsonBytes(stored)).toBeLessThan(2_000_000);
  });

  it("storedMessage leaves a message that fits as it is, and never cuts what a card runs with", () => {
    const small = { ...organisePlanCard(20), id: 7 } as PlanMessage;
    const stored = storedMessage(small) as PlanMessage;
    expect(stored.clipped).toBeUndefined();
    expect(stored.response.preview).toEqual(small.response.preview);
    expect(stored.plan).toEqual(small.plan);
    // 12,000 selected documents and their preview rows: over the limit as it is, but the lists that are only
    // shown go first — the scope, the context the plan runs with and the interpreted plan are whole.
    const ids = Array.from({ length: 12_000 }, (_, i) => docIdOf(i));
    const base = organisePlanCard(12_000);
    const plan: Plan = { ...base.plan, scope: { doc_ids: ids } };
    const big = { ...base, id: 8, plan, context: { selected_doc_ids: ids }, response: { ...base.response, result: { ...base.response.result, plan }, context: { ...base.response.context, selected_doc_ids: ids } } } as PlanMessage;
    expect(jsonBytes(big)).toBeGreaterThan(2_000_000);
    const cut = storedMessage(big) as PlanMessage;
    expect(cut).toMatchObject({ clipped: "lists", status: "pending" });
    expect(cut.plan).toEqual(plan);
    expect(cut.context).toEqual({ selected_doc_ids: ids });
    expect(cut.response.result.plan).toEqual(plan);
    expect(cut.response.context).toMatchObject({ selected_doc_ids_total: 12_000 });
    expect(cut.response.context.selected_doc_ids.length).toBeLessThan(12_000);
    expect(jsonBytes(cut)).toBeLessThan(2_000_000);
  });

  it("restoreMessage drops rows that are not messages", () => {
    expect(restoreMessage({ seq: 1, payload: "junk", created_at: "2026-11-10T00:00:00Z" })).toBeNull();
    expect(restoreMessage({ seq: 2, payload: { role: "user", text: "ok" }, created_at: "2026-11-10T00:00:00Z" })).toMatchObject({ id: 2, role: "user", createdAt: "2026-11-10T00:00:00Z" });
  });
});
