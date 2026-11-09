import { create } from "zustand";

import { subscribe } from "@/lib/ipc";
import { queryClient, queryKeys } from "@/lib/query-client";
import { EVENTS, type DocumentsChanged } from "@/lib/types";

import { resetAssistant } from "./assistant";
import { resetAttachments } from "./attachments";
import { resetGenerate } from "./generate";
import { resetProjects } from "./projects";
import { useUiStore } from "./ui";

/** Library UI state that survives navigation (view mode, filters, selection). */
interface DocumentsState {
  initialized: boolean;
  view: "grid" | "list";
  folder: string | null;
  query: string;
  /** Library selection (document ids), shared with the assistant's "선택 문서" scope and the aggregate bar (P5). */
  selectedIds: string[];
  init: () => Promise<void>;
  setView: (view: "grid" | "list") => void;
  setFolder: (folder: string | null) => void;
  setQuery: (query: string) => void;
  setSelectedIds: (ids: string[]) => void;
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
  selectedIds: [],

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });
    await subscribe<DocumentsChanged>(EVENTS.documentsChanged, () => scheduleInvalidate());
  },

  setView: (view) => set({ view }),
  setFolder: (folder) => set({ folder }),
  setQuery: (query) => set({ query }),
  setSelectedIds: (ids) => set({ selectedIds: ids }),
}));

/**
 * Call after a workspace switch: library queries, the selection, the viewer's last document
 * (the assistant's "이 문서" scope) and the assistant conversation belong to the old folder.
 */
export function resetDocumentQueries(): void {
  useDocumentsStore.setState({ folder: null, query: "", selectedIds: [] });
  useUiStore.getState().setLastDocument(null);
  resetAssistant();
  resetAttachments();
  resetGenerate();
  resetProjects();
  void queryClient.invalidateQueries({ queryKey: queryKeys.documents });
}
