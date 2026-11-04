import { create } from "zustand";

import { subscribe } from "@/lib/ipc";
import { queryClient, queryKeys } from "@/lib/query-client";
import { EVENTS, type AnswerDelta, type KnowledgeChanged, type SearchMode, type SearchResponse } from "@/lib/types";

/** Knowledge/search UI state that survives navigation plus the event wiring. */
interface KnowledgeState {
  initialized: boolean;
  /** Search page: last query / mode / scope (collection id or "all" / "knowledge"). */
  searchQuery: string;
  searchMode: SearchMode;
  searchScope: string;
  /** Knowledge screen "question test" result (kept across tab switches). */
  testQuestion: string;
  testResult: SearchResponse | null;
  /** Streaming answer text per request id, registered by `beginStream` before the call. */
  streams: Record<string, string>;
  init: () => Promise<void>;
  setSearch: (patch: Partial<Pick<KnowledgeState, "searchQuery" | "searchMode" | "searchScope">>) => void;
  setTest: (question: string, result: SearchResponse | null) => void;
  beginStream: (requestId: string) => void;
  clearStream: (requestId: string) => void;
}

const timers = new Map<string, ReturnType<typeof setTimeout>>();

function invalidateSoon(key: string, keys: readonly (readonly unknown[])[]) {
  if (timers.has(key)) return;
  timers.set(
    key,
    setTimeout(() => {
      timers.delete(key);
      for (const k of keys) void queryClient.invalidateQueries({ queryKey: k as unknown[] });
    }, 150),
  );
}

export const useKnowledgeStore = create<KnowledgeState>((set, get) => ({
  initialized: false,
  searchQuery: "",
  searchMode: "hybrid",
  searchScope: "all",
  testQuestion: "",
  testResult: null,
  streams: {},

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });
    await subscribe<KnowledgeChanged>(EVENTS.knowledgeChanged, (ev) => {
      switch (ev.kind) {
        case "collections":
          invalidateSoon("collections", [queryKeys.collections, queryKeys.knowledgeStatus, ["knowledge", "doc-collections"], ["knowledge", "search"]]);
          break;
        case "glossary":
          invalidateSoon("glossary", [queryKeys.glossary, queryKeys.knowledgeStatus, ["knowledge", "search"]]);
          break;
        case "versions":
          invalidateSoon("versions", [queryKeys.versions, ["knowledge", "search"]]);
          break;
        case "answers":
          invalidateSoon("answers", [["knowledge", "answers"], queryKeys.knowledgeStatus]);
          break;
        default:
          invalidateSoon("index", [queryKeys.knowledgeStatus, ["knowledge", "chunks"], ["knowledge", "search"], queryKeys.collections]);
      }
    });
    await subscribe<AnswerDelta>(EVENTS.knowledgeAnswer, (ev) => {
      set((s) => {
        // Only requests this UI started are tracked; `done` releases the entry.
        if (!(ev.request_id in s.streams)) return s;
        const next = { ...s.streams };
        if (ev.done) delete next[ev.request_id];
        else next[ev.request_id] = (next[ev.request_id] ?? "") + ev.delta;
        return { streams: next };
      });
    });
  },

  setSearch: (patch) => set(patch),
  setTest: (question, result) => set({ testQuestion: question, testResult: result }),
  beginStream: (requestId) => set((s) => ({ streams: { ...s.streams, [requestId]: "" } })),
  clearStream: (requestId) =>
    set((s) => {
      if (!(requestId in s.streams)) return s;
      const next = { ...s.streams };
      delete next[requestId];
      return { streams: next };
    }),
}));

/** Call after a workspace switch: knowledge queries belong to the old folder. */
export function resetKnowledgeQueries(): void {
  useKnowledgeStore.setState({ searchQuery: "", searchScope: "all", testQuestion: "", testResult: null });
  void queryClient.invalidateQueries({ queryKey: queryKeys.knowledge });
}

export function newRequestId(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}
