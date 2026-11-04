import { create } from "zustand";

import { subscribe } from "@/lib/ipc";
import { queryClient, queryKeys } from "@/lib/query-client";
import { EVENTS, type AnswerDelta, type KnowledgeChanged, type SearchMode } from "@/lib/types";

/** Knowledge/search UI state that survives navigation plus the event wiring. */
interface KnowledgeState {
  initialized: boolean;
  /** Search page: last query / mode / scope (collection id or "all" / "knowledge"). */
  searchQuery: string;
  searchMode: SearchMode;
  searchScope: string;
  /** Streaming answer text per request id (cleared when the answer resolves). */
  streams: Record<string, string>;
  init: () => Promise<void>;
  setSearch: (patch: Partial<Pick<KnowledgeState, "searchQuery" | "searchMode" | "searchScope">>) => void;
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
          invalidateSoon("glossary", [queryKeys.glossary, queryKeys.knowledgeStatus]);
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
        if (ev.done) return s;
        return { streams: { ...s.streams, [ev.request_id]: (s.streams[ev.request_id] ?? "") + ev.delta } };
      });
    });
  },

  setSearch: (patch) => set(patch),
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
  useKnowledgeStore.setState({ searchQuery: "", searchScope: "all", streams: {} });
  void queryClient.invalidateQueries({ queryKey: queryKeys.knowledge });
}

export function newRequestId(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}
