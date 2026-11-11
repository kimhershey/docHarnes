import { create } from "zustand";

import { api } from "@/lib/ipc";
import { queryClient, queryKeys } from "@/lib/query-client";
import type { Language, RetentionPolicy, StartupStatus, WorkspaceView } from "@/lib/types";
import { errorMessage } from "@/lib/utils";

import { resetDocumentQueries } from "./documents";
import { resetKnowledgeQueries } from "./knowledge";
import { usePipelineStore } from "./pipeline";
import { useSchemasStore } from "./schemas";
import { useSettingsStore } from "./settings";

interface WorkspaceState {
  current: WorkspaceView | null;
  startup: StartupStatus | null;
  loaded: boolean;
  error: string | null;
  load: () => Promise<void>;
  open: (path: string, forceUnlock?: boolean) => Promise<WorkspaceView>;
  create: (path: string, name?: string | null, locale?: Language | null, forceUnlock?: boolean) => Promise<WorkspaceView>;
  close: () => Promise<void>;
  forget: (path: string) => Promise<void>;
  setRetention: (retention: RetentionPolicy) => Promise<void>;
  setPreferredModel: (modelId: string | null) => Promise<void>;
}

/**
 * Query roots that describe the machine or the installation, not the open workspace. Every other
 * cached query belongs to a workspace: documents, knowledge, pipeline (review queue, forms, runs),
 * projects, drafts, the journal, the purge status, the model folder and its assessment.
 */
const MACHINE_QUERY_ROOTS: readonly unknown[] = [queryKeys.appInfo[0], queryKeys.hardware[0], queryKeys.engineBinary[0], queryKeys.pdfium[0]];

/**
 * Drops every workspace-scoped query result (active queries refetch from the new workspace).
 * Invalidating alone keeps the old rows on screen until the refetch lands — and for queries that
 * are not mounted, until their screen is opened again: review items, forms and journal batches of
 * the previous workspace, whose ids the new one rejects.
 */
export function resetWorkspaceQueries(): void {
  void queryClient.resetQueries({ predicate: (query) => !MACHINE_QUERY_ROOTS.includes(query.queryKey[0]) });
}

function afterWorkspaceChange() {
  resetWorkspaceQueries();
  resetDocumentQueries();
  resetKnowledgeQueries();
  // Ids of the previous workspace held outside the query cache: the review queue's document
  // filter and a form-discovery result waiting for the 양식 screen.
  usePipelineStore.setState({ reviewDocument: null, notice: null });
  useSchemasStore.setState({ pending: null });
  void useSettingsStore.getState().load();
}

export const useWorkspaceStore = create<WorkspaceState>((set) => ({
  current: null,
  startup: null,
  loaded: false,
  error: null,

  load: async () => {
    try {
      const [current, startup] = await Promise.all([api.workspaceCurrent(), api.startupStatus()]);
      set({ current, startup, loaded: true, error: null });
    } catch (e) {
      set({ error: errorMessage(e), loaded: true });
    }
  },

  open: async (path, forceUnlock = false) => {
    const ws = await api.workspaceOpen(path, forceUnlock);
    set({ current: ws, error: null });
    afterWorkspaceChange();
    return ws;
  },

  create: async (path, name = null, locale = null, forceUnlock = false) => {
    const ws = await api.workspaceCreate(path, name, locale, forceUnlock);
    set({ current: ws, error: null });
    afterWorkspaceChange();
    return ws;
  },

  close: async () => {
    await api.workspaceClose();
    set({ current: null });
    afterWorkspaceChange();
  },

  forget: async (path) => {
    await api.workspaceForget(path);
    await useSettingsStore.getState().load();
  },

  setRetention: async (retention) => {
    const ws = await api.workspaceSetRetention(retention);
    set({ current: ws });
  },

  setPreferredModel: async (modelId) => {
    const ws = await api.workspaceSetPreferredModel(modelId);
    set({ current: ws });
  },
}));

export function useWorkspace(): WorkspaceView | null {
  return useWorkspaceStore((s) => s.current);
}
