import { create } from "zustand";

import { api } from "@/lib/ipc";
import type { Answer, AnswerRequest, ChatMessageRow, ChatSessionSummary, Op, Plan, PlanExecution, PlanIssue, PlanResponse, SearchMode, SearchResponse, UiPlanContext } from "@/lib/types";
import { errorCode, errorMessage } from "@/lib/utils";

import { useAttachmentsStore } from "./attachments";

/** Plan card lifecycle (design §3.7.6). */
export type PlanStatus = "pending" | "executing" | "done" | "cancelled";
/** Clarification card lifecycle: waiting for the user's reply, replied to (되묻기), or cancelled. */
export type ClarifyStatus = "pending" | "answered" | "cancelled";

export type Message =
  | { id: number; createdAt?: string; role: "user"; text: string; attachments?: { id: string; name: string }[] }
  | { id: number; createdAt?: string; role: "assistant"; kind: "executed"; text: string; source: string; confidence: number }
  | { id: number; createdAt?: string; role: "assistant"; kind: "answer"; requestId: string; request: AnswerRequest; answer: Answer | null; error: unknown; pending: boolean }
  | { id: number; createdAt?: string; role: "assistant"; kind: "help" }
  | { id: number; createdAt?: string; role: "assistant"; kind: "plan"; text: string; response: PlanResponse; plan: Plan; context: UiPlanContext; status: PlanStatus; errors: PlanIssue[] | null; editing: boolean }
  | { id: number; createdAt?: string; role: "assistant"; kind: "execution"; text: string; response: PlanResponse; plan: Plan; jobId: string }
  | { id: number; createdAt?: string; role: "assistant"; kind: "result"; text: string; response: PlanResponse; plan: Plan; execution: PlanExecution }
  | { id: number; createdAt?: string; role: "assistant"; kind: "clarify"; text: string; response: PlanResponse; status: ClarifyStatus }
  /** "근거만 찾기" (0.5.0): the hits of a knowledge search, no model call. */
  | { id: number; createdAt?: string; role: "assistant"; kind: "evidence"; query: string; mode: SearchMode; response: SearchResponse | null; error: unknown; pending: boolean }
  | { id: number; createdAt?: string; role: "assistant"; kind: "error"; text: string };

export type PlanMessage = Extract<Message, { kind: "plan" }>;
type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;
export type NewMessage = DistributiveOmit<Message, "id" | "createdAt">;

/** The stable empty list (a selector must return the same reference while nothing changes). */
export const EMPTY_MESSAGES: Message[] = [];
/** Auto titles: the first user message clipped to this many characters (the core applies the same rule). */
export const AUTO_TITLE_CHARS = 40;
const ACTIVE_KEY = "doculens.assistant.active";

/**
 * Assistant conversations (0.7.0 §1.6): chat sessions of the open workspace, each with its
 * message list, stored in the workspace DB through the `chat_*` commands (the UI's message
 * JSON as it is; the core only derives the auto title). The store is the live cache: the
 * session list and the active session's messages are hydrated when the workspace is bound,
 * the other sessions' messages when they are selected. Messages are written after `push`
 * and after every `patch` (a card settling: answer done, plan status change, edit) — never
 * on stream deltas, which live in the knowledge store. The `ui_actions` owed by job-backed
 * plans are tracked by job id per workspace, whichever session is open, so the assistant
 * page can run them exactly once when it observes the finished job.
 */
interface AssistantState {
  /** Workspace the conversations belong to (null = none yet). */
  workspace: string | null;
  /** The session list / active messages are being loaded (the composer waits). */
  hydrating: boolean;
  sessions: ChatSessionSummary[];
  activeId: string | null;
  messagesBySession: Record<string, Message[]>;
  /** Sessions whose messages were loaded from the backend. */
  loaded: Record<string, boolean>;
  /** Job id → UI ops the plan contained (replaced by the execution's `ui_actions` once known) that have not run yet. */
  pendingUiActions: Record<string, Op[]>;
  nextId: number;
  /** Binds the conversations to a workspace; a different path starts afresh and hydrates. */
  bind: (workspace: string | null) => void;
  hydrate: () => Promise<void>;
  /** Appends to the session captured when the request started (never "whatever is active now"). */
  push: (sessionId: string, message: NewMessage) => number;
  patch: (sessionId: string, id: number, f: (m: Message) => Message) => void;
  /** "새 채팅": a fresh session (the active one is reused while it is still empty). Resolves to its id. */
  newSession: () => Promise<string | null>;
  select: (id: string) => Promise<void>;
  rename: (id: string, title: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
  trackJob: (jobId: string, ops: Op[]) => void;
  /**
   * Claims the `ui_actions` of a finished job: returns them (the execution's list when given,
   * else the tracked plan ops) the first time and null afterwards or for unknown jobs.
   */
  claimUiActions: (jobId: string, executed?: Op[] | null) => Op[] | null;
}

/** Messages of the active session (the stable empty list when there is none). */
export function selectMessages(s: Pick<AssistantState, "activeId" | "messagesBySession">): Message[] {
  return (s.activeId ? s.messagesBySession[s.activeId] : undefined) ?? EMPTY_MESSAGES;
}

/** The auto title of a session from its first user message (null for other messages / empty text). */
export function autoTitle(message: { role: string; text?: string }): string | null {
  if (message.role !== "user" || typeof message.text !== "string") return null;
  const text = message.text.replace(/\s+/g, " ").trim();
  if (!text) return null;
  const chars = [...text];
  return chars.length > AUTO_TITLE_CHARS ? `${chars.slice(0, AUTO_TITLE_CHARS - 1).join("")}…` : text;
}

/** An `error` field as it can be stored and shown again after a reload (`describeError` reads code / message). */
function storableError(e: unknown): unknown {
  if (e === null || e === undefined) return null;
  if (typeof e === "object" && !(e instanceof Error)) return e;
  return { code: errorCode(e) ?? "generic", message: errorMessage(e) };
}

/** A plan context without the conversation turns it embeds (they are the other stored messages). */
function withoutHistory<T extends { history?: unknown }>(ctx: T): T {
  return "history" in ctx ? { ...ctx, history: [] } : ctx;
}

/**
 * The message as it is written: errors flattened, and plan / execution / result / clarify cards without the
 * conversation history their `context` and `response.context` carry (otherwise every card would store every
 * earlier turn again).
 */
function storable(m: Message): unknown {
  if (m.role !== "assistant") return m;
  if (m.kind === "answer" || m.kind === "evidence") return { ...m, error: storableError(m.error) };
  if (m.kind === "plan") return { ...m, context: withoutHistory(m.context), response: { ...m.response, context: withoutHistory(m.response.context) } };
  if (m.kind === "execution" || m.kind === "result" || m.kind === "clarify") {
    return { ...m, response: { ...m.response, context: withoutHistory(m.response.context) } };
  }
  return m;
}

const INTERRUPTED = { code: "chat_interrupted", message: "chat_interrupted" };

/**
 * A stored message back into the list: what cannot continue after a reload is settled — a pending
 * answer / evidence search becomes an interrupted one, a plan caught mid-execution waits for approval again.
 */
export function restoreMessage(row: ChatMessageRow): Message | null {
  const p = row.payload;
  if (!p || typeof p !== "object" || typeof (p as { role?: unknown }).role !== "string") return null;
  const m = { ...(p as Message), id: row.seq, createdAt: (p as { createdAt?: string }).createdAt ?? row.created_at } as Message;
  if (m.role === "assistant" && m.kind === "answer" && m.pending) return { ...m, pending: false, error: m.answer ? m.error : INTERRUPTED };
  if (m.role === "assistant" && m.kind === "evidence" && m.pending) return { ...m, pending: false, error: m.response ? m.error : INTERRUPTED };
  if (m.role === "assistant" && m.kind === "plan" && m.status === "executing") return { ...m, status: "pending", editing: false };
  return m;
}

function readActive(workspace: string): string | null {
  try {
    return localStorage.getItem(`${ACTIVE_KEY}:${workspace}`);
  } catch {
    return null;
  }
}

function rememberActive(workspace: string | null, id: string | null): void {
  if (!workspace || !id) return;
  try {
    localStorage.setItem(`${ACTIVE_KEY}:${workspace}`, id);
  } catch {
    // per-viewer convenience only
  }
}

/** The session summary after a message was written: count, recency (the list is newest first) and the auto title. */
function touch(sessions: ChatSessionSummary[], id: string, message: Message, createdAt: string, created: boolean): ChatSessionSummary[] {
  const cur = sessions.find((s) => s.id === id);
  if (!cur) return sessions;
  const title = cur.title_auto && cur.title === "" ? (autoTitle(message) ?? "") : cur.title;
  const next = { ...cur, title, updated_at: createdAt, message_count: created ? cur.message_count + 1 : cur.message_count };
  return [next, ...sessions.filter((s) => s.id !== id)];
}

/** Patches of one message are written together, this long after the last one (plan edits are keystroke-wise). */
export const PATCH_WRITE_DELAY_MS = 300;
const pendingWrites = new Map<string, { timer: ReturnType<typeof setTimeout>; write: () => void }>();

/** Writes every pending patch now (tests; the app relies on the delay being short). */
export function flushChatWrites(): void {
  for (const [key, w] of pendingWrites) {
    clearTimeout(w.timer);
    pendingWrites.delete(key);
    w.write();
  }
}

export const useAssistantStore = create<AssistantState>((set, get) => {
  const write = (sessionId: string, m: Message) => {
    void api.chatMessagePut(sessionId, m.id, storable(m)).catch((e: unknown) => console.warn("chat message not stored", e));
  };
  /** A new message is written at once. */
  const persist = (sessionId: string, m: Message) => {
    const key = `${sessionId}/${m.id}`;
    const w = pendingWrites.get(key);
    if (w) {
      clearTimeout(w.timer);
      pendingWrites.delete(key);
    }
    write(sessionId, m);
  };
  /** A patched message is written once the patches settle (the latest state of the message at that time). */
  const persistSoon = (sessionId: string, id: number) => {
    const key = `${sessionId}/${id}`;
    const prev = pendingWrites.get(key);
    if (prev) clearTimeout(prev.timer);
    const doWrite = () => {
      pendingWrites.delete(key);
      const m = (get().messagesBySession[sessionId] ?? []).find((x) => x.id === id);
      if (m) write(sessionId, m);
    };
    pendingWrites.set(key, { timer: setTimeout(doWrite, PATCH_WRITE_DELAY_MS), write: doWrite });
  };
  const bumpNextId = (rows: ChatMessageRow[]) => {
    const max = rows.reduce((acc, r) => Math.max(acc, r.seq), 0);
    if (max >= get().nextId) set({ nextId: max + 1 });
  };
  /** Loads a session's messages once (the workspace may change meanwhile: then the result is dropped). */
  const load = async (id: string) => {
    const ws = get().workspace;
    if (get().loaded[id]) return;
    try {
      const rows = await api.chatMessagesList(id);
      if (get().workspace !== ws) return;
      const messages = rows.map(restoreMessage).filter((m): m is Message => m !== null);
      bumpNextId(rows);
      set((s) => ({ messagesBySession: { ...s.messagesBySession, [id]: messages }, loaded: { ...s.loaded, [id]: true } }));
    } catch (e) {
      console.warn("chat session not loaded", e);
      if (get().workspace !== ws) return;
      set((s) => ({ messagesBySession: { ...s.messagesBySession, [id]: s.messagesBySession[id] ?? [] }, loaded: { ...s.loaded, [id]: true } }));
    }
  };

  return {
    workspace: null,
    hydrating: false,
    sessions: [],
    activeId: null,
    messagesBySession: {},
    loaded: {},
    pendingUiActions: {},
    nextId: 1,

    bind: (workspace) => {
      if (get().workspace === workspace) return;
      set({ workspace, sessions: [], activeId: null, messagesBySession: {}, loaded: {}, pendingUiActions: {}, hydrating: workspace !== null });
      if (workspace !== null) void get().hydrate();
    },

    hydrate: async () => {
      const ws = get().workspace;
      if (!ws) return;
      try {
        let sessions = await api.chatSessionsList();
        if (get().workspace !== ws) return;
        if (sessions.length === 0) sessions = [await api.chatSessionCreate(null)];
        if (get().workspace !== ws) return;
        const remembered = readActive(ws);
        const activeId = (sessions.find((s) => s.id === remembered) ?? sessions[0]!).id;
        set({ sessions, activeId });
        useAttachmentsStore.getState().switchSession(null, activeId);
        await load(activeId);
      } catch (e) {
        console.warn("chat sessions not loaded", e);
      } finally {
        if (get().workspace === ws) set({ hydrating: false });
      }
    },

    push: (sessionId, message) => {
      const id = get().nextId;
      const createdAt = new Date().toISOString();
      const m = { ...message, id, createdAt } as Message;
      set((s) => ({
        nextId: id + 1,
        messagesBySession: { ...s.messagesBySession, [sessionId]: [...(s.messagesBySession[sessionId] ?? []), m] },
        sessions: touch(s.sessions, sessionId, m, createdAt, true),
      }));
      persist(sessionId, m);
      return id;
    },

    patch: (sessionId, id, f) => {
      let patched = false;
      set((s) => {
        const list = s.messagesBySession[sessionId];
        if (!list) return s;
        const next = list.map((m) => {
          if (m.id !== id) return m;
          patched = true;
          return f(m);
        });
        return { messagesBySession: { ...s.messagesBySession, [sessionId]: next } };
      });
      if (patched) persistSoon(sessionId, id);
    },

    newSession: async () => {
      const { activeId, messagesBySession, workspace } = get();
      if (!workspace) return null;
      if (activeId && (messagesBySession[activeId] ?? []).length === 0) return activeId;
      const created = await api.chatSessionCreate(null);
      if (get().workspace !== workspace) return null;
      set((s) => ({
        sessions: [created, ...s.sessions.filter((x) => x.id !== created.id)],
        activeId: created.id,
        messagesBySession: { ...s.messagesBySession, [created.id]: [] },
        loaded: { ...s.loaded, [created.id]: true },
      }));
      useAttachmentsStore.getState().switchSession(activeId, created.id);
      rememberActive(workspace, created.id);
      return created.id;
    },

    select: async (id) => {
      const { activeId, workspace } = get();
      if (id === activeId || !get().sessions.some((s) => s.id === id)) return;
      set({ activeId: id });
      useAttachmentsStore.getState().switchSession(activeId, id);
      rememberActive(workspace, id);
      await load(id);
    },

    rename: async (id, title) => {
      const clean = title.trim();
      if (!clean) return;
      await api.chatSessionRename(id, clean);
      set((s) => ({ sessions: s.sessions.map((x) => (x.id === id ? { ...x, title: clean, title_auto: false } : x)) }));
    },

    remove: async (id) => {
      const workspace = get().workspace;
      await api.chatSessionDelete(id);
      if (get().workspace !== workspace) return;
      const wasActive = get().activeId === id;
      set((s) => {
        const messagesBySession = { ...s.messagesBySession };
        delete messagesBySession[id];
        const loaded = { ...s.loaded };
        delete loaded[id];
        return { sessions: s.sessions.filter((x) => x.id !== id), messagesBySession, loaded, activeId: wasActive ? null : s.activeId };
      });
      if (!wasActive) {
        useAttachmentsStore.getState().dropSession(id);
        return;
      }
      let next = get().sessions[0]?.id ?? null;
      if (!next) {
        const created = await api.chatSessionCreate(null);
        if (get().workspace !== workspace) return;
        set((s) => ({ sessions: [created, ...s.sessions], messagesBySession: { ...s.messagesBySession, [created.id]: [] }, loaded: { ...s.loaded, [created.id]: true } }));
        next = created.id;
      }
      set({ activeId: next });
      // Switch first (the current attachments move to the next session), then forget the deleted one.
      useAttachmentsStore.getState().switchSession(id, next);
      useAttachmentsStore.getState().dropSession(id);
      rememberActive(workspace, next);
      await load(next);
    },

    trackJob: (jobId, ops) => set((s) => ({ pendingUiActions: { ...s.pendingUiActions, [jobId]: ops } })),

    claimUiActions: (jobId, executed) => {
      const pending = get().pendingUiActions;
      if (!(jobId in pending)) return null;
      const rest = { ...pending };
      const tracked = rest[jobId] ?? [];
      delete rest[jobId];
      set({ pendingUiActions: rest });
      return executed ?? tracked;
    },
  };
});

/** Workspace switch: the conversations and the owed UI actions belong to the old workspace. */
export function resetAssistant(): void {
  useAssistantStore.setState({ workspace: null, hydrating: false, sessions: [], activeId: null, messagesBySession: {}, loaded: {}, pendingUiActions: {} });
}
