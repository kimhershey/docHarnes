// Assistant conversation store (0.7.0 §1.6): chat sessions hydrated from the backend, messages
// written to the session a request started in, the owed `ui_actions` of a job-backed plan claimed
// exactly once. The `chat_*` commands are faked in memory.
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ChatMessageRow, ChatSessionSummary, Op } from "@/lib/types";

const fake = vi.hoisted(() => {
  // One workspace database at a time, like the backend: `switchWorkspace` opens another (empty) one.
  const state = { sessions: [] as ChatSessionSummary[], messages: {} as Record<string, ChatMessageRow[]>, seq: 1, puts: 0, clock: 0, log: [] as string[], putGate: null as Promise<void> | null };
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
    chatMessagesList: async (id: string) => [...(state.messages[id] ?? [])],
    // Like the core (`chat::put_message`): a session this database does not have is re-created, untitled.
    chatMessagePut: async (id: string, seq: number, payload: unknown) => {
      if (state.putGate) await state.putGate;
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
  };
  return { state, api, reset, switchWorkspace };
});

vi.mock("@/lib/ipc", () => ({ api: fake.api }));

import { useAttachmentsStore } from "./attachments";
import { DROPPED, PATCH_WRITE_DELAY_MS, flushChatWrites, resetAssistant, restoreMessage, selectMessages, useAssistantStore } from "./assistant";

const theme: Op = { op: "set_theme", theme: "dark" };
const open: Op = { op: "open", target: "screen", ref: "jobs" };

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
    // Patches are written together once they settle (the latest state of the message).
    s.patch(sid, a, (m) => (m.role === "user" ? { ...m, text: "edit" } : m));
    s.patch(sid, a, (m) => (m.role === "user" ? { ...m, text: "edited" } : m));
    expect(selectMessages(useAssistantStore.getState())[0]).toMatchObject({ id: a, text: "edited" });
    await settled();
    expect(fake.state.puts).toBe(2);
    flushChatWrites();
    await settled();
    expect(fake.state.puts).toBe(3);
    expect((fake.state.messages[sid]![0]!.payload as { text: string }).text).toBe("edited");
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
    s.patch(first, asked, (m) => (m.role === "user" ? { ...m, text: "late" } : m), generation);
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
    s.patch(first, id, (m) => (m.role === "user" ? { ...m, text: "edited" } : m));
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
    useAssistantStore.getState().patch(first, answer, (m) => m, generation);
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

  it("restoreMessage drops rows that are not messages", () => {
    expect(restoreMessage({ seq: 1, payload: "junk", created_at: "2026-11-10T00:00:00Z" })).toBeNull();
    expect(restoreMessage({ seq: 2, payload: { role: "user", text: "ok" }, created_at: "2026-11-10T00:00:00Z" })).toMatchObject({ id: 2, role: "user", createdAt: "2026-11-10T00:00:00Z" });
  });
});
