import { create } from "zustand";

import { subscribe } from "@/lib/ipc";
import { queryClient, queryKeys } from "@/lib/query-client";
import { EVENTS, type DocumentsChanged } from "@/lib/types";

/** Library UI state that survives navigation (view mode, filters, selection). */
interface DocumentsState {
  initialized: boolean;
  view: "grid" | "list";
  folder: string | null;
  query: string;
  init: () => Promise<void>;
  setView: (view: "grid" | "list") => void;
  setFolder: (folder: string | null) => void;
  setQuery: (query: string) => void;
}

let invalidateTimer: ReturnType<typeof setTimeout> | null = null;

/** Coalesces bursts of `documents://changed` (one per imported file) into one refetch. */
function scheduleInvalidate() {
  if (invalidateTimer) return;
  invalidateTimer = setTimeout(() => {
    invalidateTimer = null;
    void queryClient.invalidateQueries({ queryKey: queryKeys.documents });
  }, 150);
}

export const useDocumentsStore = create<DocumentsState>((set, get) => ({
  initialized: false,
  view: "grid",
  folder: null,
  query: "",

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });
    await subscribe<DocumentsChanged>(EVENTS.documentsChanged, () => scheduleInvalidate());
  },

  setView: (view) => set({ view }),
  setFolder: (folder) => set({ folder }),
  setQuery: (query) => set({ query }),
}));

/** Call after a workspace switch: library queries belong to the old folder. */
export function resetDocumentQueries(): void {
  useDocumentsStore.setState({ folder: null, query: "" });
  void queryClient.invalidateQueries({ queryKey: queryKeys.documents });
}
