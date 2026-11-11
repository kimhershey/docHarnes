import { create } from "zustand";

import type { UiOpResult } from "@/lib/commands";
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
  /** UI ops that ran at once. `failures`: the errors of the ops that failed (absent = all ran), `ran`: how many did run. */
  | { id: number; createdAt?: string; role: "assistant"; kind: "executed"; text: string; source: string; confidence: number; failures?: string[]; ran?: number }
  | { id: number; createdAt?: string; role: "assistant"; kind: "answer"; requestId: string; request: AnswerRequest; answer: Answer | null; error: unknown; pending: boolean }
  | { id: number; createdAt?: string; role: "assistant"; kind: "help" }
  | { id: number; createdAt?: string; role: "assistant"; kind: "plan"; text: string; response: PlanResponse; plan: Plan; context: UiPlanContext; status: PlanStatus; errors: PlanIssue[] | null; editing: boolean }
  /** `uiResults`: what became of the execution's `ui_actions` once the UI ran them (one per action, in order). */
  | { id: number; createdAt?: string; role: "assistant"; kind: "execution"; text: string; response: PlanResponse; plan: Plan; jobId: string; uiResults?: UiOpResult[] }
  | { id: number; createdAt?: string; role: "assistant"; kind: "result"; text: string; response: PlanResponse; plan: Plan; execution: PlanExecution; uiResults?: UiOpResult[] }
  | { id: number; createdAt?: string; role: "assistant"; kind: "clarify"; text: string; response: PlanResponse; status: ClarifyStatus }
  /** "근거만 찾기" (0.5.0): the hits of a knowledge search, no model call. */
  | { id: number; createdAt?: string; role: "assistant"; kind: "evidence"; query: string; mode: SearchMode; response: SearchResponse | null; error: unknown; pending: boolean }
  | { id: number; createdAt?: string; role: "assistant"; kind: "error"; text: string };

export type PlanMessage = Extract<Message, { kind: "plan" }>;

export interface PatchOptions {
  /** The workspace generation the request started in (see `accepts`). */
  generation?: number;
  /** Write after `PATCH_WRITE_DELAY_MS` without further patches instead of at once (plan editor input). */
  defer?: boolean;
}
type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;
export type NewMessage = DistributiveOmit<Message, "id" | "createdAt">;

/** The stable empty list (a selector must return the same reference while nothing changes). */
export const EMPTY_MESSAGES: Message[] = [];
/** What `push` returns for a message that was not accepted (see `accepts`): no message has this id. */
export const DROPPED = -1;
/** Auto titles: the first user message clipped to this many characters (the core applies the same rule). */
export const AUTO_TITLE_CHARS = 40;
const ACTIVE_KEY = "doculens.assistant.active";

/**
 * Assistant conversations (0.7.0 §1.6): chat sessions of the open workspace, each with its
 * message list, stored in the workspace DB through the `chat_*` commands (the UI's message
 * JSON as it is; the core only derives the auto title). The store is the live cache: the
 * session list and the active session's messages are hydrated when the workspace is bound,
 * the other sessions' messages when they are selected. Messages are written at `push` and
 * at every `patch` (a card settling: answer done, plan status change) — at once, so a plan
 * that ran is not restored as "승인 대기" (and run again) when the app closes right after;
 * only the plan editor's keystrokes are written together after a short delay
 * (`flushChatWrites` sends those before the workspace is switched or the page goes away).
 * Stream deltas are never written; they live in the knowledge store. The `ui_actions` owed by job-backed
 * plans are tracked by job id per workspace, whichever session is open, so the assistant
 * page can run them exactly once when it observes the finished job.
 *
 * A request can outlive its conversation: the workspace is switched (by the user, or by the
 * assistant's own "워크스페이스 … 열어") or the chat is deleted while a plan / answer is still on
 * its way. What such a request writes afterwards must go nowhere — the backend is already on the
 * other workspace's database and re-creates a session it does not know, which leaves an untitled
 * ghost chat there. So a request captures `generation` together with its session id when it
 * starts, and `push` / `patch` drop what arrives for another generation or for a session that
 * is no longer listed (`accepts`).
 */
interface AssistantState {
  /** Workspace the conversations belong to (null = none yet). */
  workspace: string | null;
  /** Counts the workspaces this store was bound to: it changes on every switch (`bind` to another path, `resetAssistant`). */
  generation: number;
  /** The session list / active messages are being loaded (the composer waits). */
  hydrating: boolean;
  sessions: ChatSessionSummary[];
  activeId: string | null;
  messagesBySession: Record<string, Message[]>;
  /**
   * Sessions whose messages were loaded from the backend. Message ids continue after the highest stored
   * one, so nothing is accepted for a session before it is loaded: a message numbered without knowing the
   * stored ones would replace the stored message with the same number (KNO-15).
   */
  loaded: Record<string, boolean>;
  /** Sessions whose stored messages could not be read; they stay closed for writing until `reload` succeeds. */
  loadFailed: Record<string, boolean>;
  /** Job id → UI ops the plan contained (replaced by the execution's `ui_actions` once known) that have not run yet. */
  pendingUiActions: Record<string, Op[]>;
  nextId: number;
  /** Binds the conversations to a workspace; a different path starts afresh and hydrates. */
  bind: (workspace: string | null) => void;
  hydrate: () => Promise<void>;
  /**
   * Whether a request that started in `generation` (omitted = now) may still write into `sessionId`:
   * the store is on the same workspace and the session is one of its chats (not deleted meanwhile).
   */
  accepts: (sessionId: string, generation?: number) => boolean;
  /**
   * Appends to the session captured when the request started (never "whatever is active now").
   * Returns the message id, or `DROPPED` when the conversation is gone (`accepts`) or its stored
   * messages are not loaded (yet).
   */
  push: (sessionId: string, message: NewMessage, generation?: number) => number;
  /** "다시 시도": reads the messages of a session whose load failed. */
  reload: (id: string) => Promise<void>;
  /**
   * Changes a message. The new state is written at once — a card that settled (plan executed / cancelled,
   * answer finished, question answered) must not be restored in its earlier state after a restart —
   * unless `defer` is set: keystroke-wise edits are written together shortly after the last one.
   */
  patch: (sessionId: string, id: number, f: (m: Message) => Message, options?: PatchOptions) => void;
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

/** Deferred patches of one message (plan edits are keystroke-wise) are written together, this long after the last one. */
export const PATCH_WRITE_DELAY_MS = 300;

/**
 * One slot per message with something to write. At most one write of a message is in flight — a newer
 * state waits for it, so the stored message can never go back to an older one — and a waiting patch is
 * written once its delay has passed. A slot belongs to the workspace generation it was opened in.
 */
interface WriteSlot {
  sessionId: string;
  id: number;
  generation: number;
  timer: ReturnType<typeof setTimeout> | null;
  inflight: Promise<void> | null;
  /** The message changed while a write was in flight: write once more when that one is done. */
  again: boolean;
}
const writeSlots = new Map<string, WriteSlot>();
/** Sessions being deleted: nothing more is written into them. */
const removing = new Set<string>();

/** Writes the message's latest state now (after the write in flight, if there is one). */
function writeNow(key: string, slot: WriteSlot): void {
  if (slot.timer) {
    clearTimeout(slot.timer);
    slot.timer = null;
  }
  if (slot.inflight) {
    slot.again = true;
    return;
  }
  slot.again = false;
  const s = useAssistantStore.getState();
  // The workspace may have changed or the chat been deleted while the write waited: then it goes nowhere.
  const m = s.accepts(slot.sessionId, slot.generation) ? (s.messagesBySession[slot.sessionId] ?? []).find((x) => x.id === slot.id) : undefined;
  if (!m) {
    if (writeSlots.get(key) === slot) writeSlots.delete(key);
    return;
  }
  slot.inflight = api
    .chatMessagePut(slot.sessionId, m.id, storable(m))
    .then(
      () => undefined,
      (e: unknown) => console.warn("chat message not stored", e),
    )
    .then(() => {
      slot.inflight = null;
      if (slot.again) writeNow(key, slot);
      else if (!slot.timer && writeSlots.get(key) === slot) writeSlots.delete(key);
    });
}

function scheduleWrite(sessionId: string, id: number, generation: number, delayMs: number): void {
  const key = `${sessionId}/${id}`;
  let slot = writeSlots.get(key);
  if (!slot || slot.generation !== generation) {
    slot = { sessionId, id, generation, timer: null, inflight: null, again: false };
    writeSlots.set(key, slot);
  }
  const current = slot;
  if (delayMs <= 0) {
    writeNow(key, current);
    return;
  }
  if (current.timer) clearTimeout(current.timer);
  current.timer = setTimeout(() => {
    current.timer = null;
    writeNow(key, current);
  }, delayMs);
}

/** Forgets what is still waiting (all of it, or one session's); writes in flight finish on their own. */
function dropWaitingWrites(sessionId?: string): void {
  for (const [key, slot] of [...writeSlots]) {
    if (sessionId !== undefined && slot.sessionId !== sessionId) continue;
    if (slot.timer) clearTimeout(slot.timer);
    slot.timer = null;
    slot.again = false;
    if (sessionId === undefined || !slot.inflight) writeSlots.delete(key);
  }
}

/** Resolves once no write of the session is in flight any more. */
async function writesSettled(sessionId: string): Promise<void> {
  for (;;) {
    const inflight = [...writeSlots.values()].filter((s) => s.sessionId === sessionId && s.inflight).map((s) => s.inflight);
    if (inflight.length === 0) return;
    await Promise.all(inflight);
  }
}

/**
 * Writes every waiting patch now; resolves once nothing is in flight any more. To be awaited before the
 * backend is told to open / close a workspace: afterwards the writes would reach the other database, so
 * `bind` / `resetAssistant` can only drop what is still waiting.
 */
export async function flushChatWrites(): Promise<void> {
  for (;;) {
    for (const [key, slot] of [...writeSlots]) if (slot.timer) writeNow(key, slot);
    const inflight = [...writeSlots.values()].filter((s) => s.inflight).map((s) => s.inflight);
    if (inflight.length === 0) return;
    await Promise.all(inflight);
  }
}

// The page is going away (window closed, reload): send what still waits. Best effort — the calls are
// asynchronous — which is why state transitions never wait in the first place.
if (typeof window !== "undefined") window.addEventListener("pagehide", () => void flushChatWrites());

export const useAssistantStore = create<AssistantState>((set, get) => {
  const bumpNextId = (rows: ChatMessageRow[]) => {
    const max = rows.reduce((acc, r) => Math.max(acc, r.seq), 0);
    if (max >= get().nextId) set({ nextId: max + 1 });
  };
  /**
   * Loads a session's messages once (the workspace may change meanwhile: then the result is dropped).
   * A failed read leaves the session unloaded — shown with a retry, closed for writing — instead of
   * pretending it is empty.
   */
  const load = async (id: string) => {
    const generation = get().generation;
    if (get().loaded[id]) return;
    try {
      const rows = await api.chatMessagesList(id);
      if (get().generation !== generation || !get().sessions.some((x) => x.id === id)) return;
      const messages = rows.map(restoreMessage).filter((m): m is Message => m !== null);
      bumpNextId(rows);
      set((s) => {
        const loadFailed = { ...s.loadFailed };
        delete loadFailed[id];
        return { messagesBySession: { ...s.messagesBySession, [id]: messages }, loaded: { ...s.loaded, [id]: true }, loadFailed };
      });
    } catch (e) {
      console.warn("chat session not loaded", e);
      if (get().generation !== generation) return;
      set((s) => ({ loadFailed: { ...s.loadFailed, [id]: true } }));
    }
  };

  return {
    workspace: null,
    generation: 0,
    hydrating: false,
    sessions: [],
    activeId: null,
    messagesBySession: {},
    loaded: {},
    loadFailed: {},
    pendingUiActions: {},
    nextId: 1,

    bind: (workspace) => {
      if (get().workspace === workspace) return;
      // The backend is already on the other workspace: what still waits to be written belongs to the old one.
      dropWaitingWrites();
      set((s) => ({ workspace, generation: s.generation + 1, sessions: [], activeId: null, messagesBySession: {}, loaded: {}, loadFailed: {}, pendingUiActions: {}, hydrating: workspace !== null }));
      if (workspace !== null) void get().hydrate();
    },

    hydrate: async () => {
      const ws = get().workspace;
      const generation = get().generation;
      if (!ws) return;
      try {
        let sessions = await api.chatSessionsList();
        if (get().generation !== generation) return;
        if (sessions.length === 0) sessions = [await api.chatSessionCreate(null)];
        if (get().generation !== generation) return;
        const remembered = readActive(ws);
        const activeId = (sessions.find((s) => s.id === remembered) ?? sessions[0]!).id;
        set({ sessions, activeId });
        useAttachmentsStore.getState().switchSession(null, activeId);
        await load(activeId);
      } catch (e) {
        console.warn("chat sessions not loaded", e);
      } finally {
        if (get().generation === generation) set({ hydrating: false });
      }
    },

    accepts: (sessionId, generation) => {
      const s = get();
      if (generation !== undefined && generation !== s.generation) return false;
      return !removing.has(sessionId) && s.sessions.some((x) => x.id === sessionId);
    },

    push: (sessionId, message, generation) => {
      if (!get().accepts(sessionId, generation) || !get().loaded[sessionId]) return DROPPED;
      const id = get().nextId;
      const createdAt = new Date().toISOString();
      const m = { ...message, id, createdAt } as Message;
      set((s) => ({
        nextId: id + 1,
        messagesBySession: { ...s.messagesBySession, [sessionId]: [...(s.messagesBySession[sessionId] ?? []), m] },
        sessions: touch(s.sessions, sessionId, m, createdAt, true),
      }));
      // A new message is written at once.
      scheduleWrite(sessionId, id, get().generation, 0);
      return id;
    },

    patch: (sessionId, id, f, options) => {
      if (!get().accepts(sessionId, options?.generation)) return;
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
      // Either way the latest state of the message at the time of the write is what gets stored.
      if (patched) scheduleWrite(sessionId, id, get().generation, options?.defer ? PATCH_WRITE_DELAY_MS : 0);
    },

    newSession: async () => {
      const { activeId, messagesBySession, loaded, workspace, generation } = get();
      if (!workspace) return null;
      // (An active chat that is not loaded is not known to be empty.)
      if (activeId && loaded[activeId] && (messagesBySession[activeId] ?? []).length === 0) return activeId;
      const created = await api.chatSessionCreate(null);
      if (get().generation !== generation) return null;
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

    reload: async (id) => {
      if (!get().sessions.some((s) => s.id === id)) return;
      set((s) => {
        const loadFailed = { ...s.loadFailed };
        delete loadFailed[id];
        return { loadFailed };
      });
      await load(id);
    },

    rename: async (id, title) => {
      const clean = title.trim();
      if (!clean) return;
      await api.chatSessionRename(id, clean);
      set((s) => ({ sessions: s.sessions.map((x) => (x.id === id ? { ...x, title: clean, title_auto: false } : x)) }));
    },

    remove: async (id) => {
      const { workspace, generation } = get();
      // From here on nothing is written into the session: a message that reached the backend after the
      // delete would make it re-create the session (an untitled ghost chat). Writes already on their way
      // are let through first.
      removing.add(id);
      dropWaitingWrites(id);
      try {
        await writesSettled(id);
        await api.chatSessionDelete(id);
      } catch (e) {
        removing.delete(id);
        throw e;
      }
      if (get().generation !== generation) {
        removing.delete(id);
        return;
      }
      const wasActive = get().activeId === id;
      set((s) => {
        const messagesBySession = { ...s.messagesBySession };
        delete messagesBySession[id];
        const loaded = { ...s.loaded };
        delete loaded[id];
        const loadFailed = { ...s.loadFailed };
        delete loadFailed[id];
        return { sessions: s.sessions.filter((x) => x.id !== id), messagesBySession, loaded, loadFailed, activeId: wasActive ? null : s.activeId };
      });
      // No longer listed: `accepts` refuses it from the list alone.
      removing.delete(id);
      if (!wasActive) {
        useAttachmentsStore.getState().dropSession(id);
        return;
      }
      let next = get().sessions[0]?.id ?? null;
      if (!next) {
        const created = await api.chatSessionCreate(null);
        if (get().generation !== generation) return;
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
  // Called once the backend is on the other workspace: nothing of the old conversations may be written any more.
  dropWaitingWrites();
  useAssistantStore.setState((s) => ({ workspace: null, generation: s.generation + 1, hydrating: false, sessions: [], activeId: null, messagesBySession: {}, loaded: {}, loadFailed: {}, pendingUiActions: {} }));
}
