import { create } from "zustand";

import type { Answer, AnswerRequest, Op, Plan, PlanExecution, PlanIssue, PlanResponse, SearchMode, SearchResponse, UiPlanContext } from "@/lib/types";

/** Plan card lifecycle (design §3.7.6). */
export type PlanStatus = "pending" | "executing" | "done" | "cancelled";
/** Clarification card lifecycle: waiting for the user's reply, replied to (되묻기), or cancelled. */
export type ClarifyStatus = "pending" | "answered" | "cancelled";

export type Message =
  | { id: number; role: "user"; text: string; attachments?: { id: string; name: string }[] }
  | { id: number; role: "assistant"; kind: "executed"; text: string; source: string; confidence: number }
  | { id: number; role: "assistant"; kind: "answer"; requestId: string; request: AnswerRequest; answer: Answer | null; error: unknown; pending: boolean }
  | { id: number; role: "assistant"; kind: "help" }
  | { id: number; role: "assistant"; kind: "plan"; text: string; response: PlanResponse; plan: Plan; context: UiPlanContext; status: PlanStatus; errors: PlanIssue[] | null; editing: boolean }
  | { id: number; role: "assistant"; kind: "execution"; text: string; response: PlanResponse; plan: Plan; jobId: string }
  | { id: number; role: "assistant"; kind: "result"; text: string; response: PlanResponse; plan: Plan; execution: PlanExecution }
  | { id: number; role: "assistant"; kind: "clarify"; text: string; response: PlanResponse; status: ClarifyStatus }
  /** "근거만 찾기" (0.5.0): the hits of a knowledge search, no model call. */
  | { id: number; role: "assistant"; kind: "evidence"; query: string; mode: SearchMode; response: SearchResponse | null; error: unknown; pending: boolean }
  | { id: number; role: "assistant"; kind: "error"; text: string };

export type PlanMessage = Extract<Message, { kind: "plan" }>;
type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;
export type NewMessage = DistributiveOmit<Message, "id">;

/**
 * Assistant conversation that survives navigation (review M7): the message
 * list is kept per workspace path, and the `ui_actions` owed by job-backed
 * plans are tracked by job id so the assistant page can run them exactly once
 * when it observes the finished job — on mount or on a later update — even if
 * the execution card was not mounted when the job finished.
 */
interface AssistantState {
  /** Workspace the conversation belongs to (null = none yet). */
  workspace: string | null;
  messages: Message[];
  /** Job id → UI ops the plan contained (replaced by the execution's `ui_actions` once known) that have not run yet. */
  pendingUiActions: Record<string, Op[]>;
  nextId: number;
  /** Binds the conversation to a workspace; a different path starts a fresh one. */
  bind: (workspace: string | null) => void;
  push: (message: NewMessage) => number;
  patch: (id: number, f: (m: Message) => Message) => void;
  /** "새 대화": clears the messages (owed `ui_actions` of running jobs stay tracked). */
  clear: () => void;
  trackJob: (jobId: string, ops: Op[]) => void;
  /**
   * Claims the `ui_actions` of a finished job: returns them (the execution's list when given,
   * else the tracked plan ops) the first time and null afterwards or for unknown jobs.
   */
  claimUiActions: (jobId: string, executed?: Op[] | null) => Op[] | null;
}

export const useAssistantStore = create<AssistantState>((set, get) => ({
  workspace: null,
  messages: [],
  pendingUiActions: {},
  nextId: 1,

  bind: (workspace) => {
    if (get().workspace === workspace) return;
    set({ workspace, messages: [], pendingUiActions: {} });
  },

  push: (message) => {
    const id = get().nextId;
    set((s) => ({ nextId: id + 1, messages: [...s.messages, { ...message, id } as Message] }));
    return id;
  },

  patch: (id, f) => set((s) => ({ messages: s.messages.map((m) => (m.id === id ? f(m) : m)) })),

  clear: () => set({ messages: [] }),

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
}));

/** Workspace switch: the conversation and the owed UI actions belong to the old workspace. */
export function resetAssistant(): void {
  useAssistantStore.setState({ workspace: null, messages: [], pendingUiActions: {} });
}
