import { create } from "zustand";

import { api } from "@/lib/ipc";
import { queryClient, queryKeys } from "@/lib/query-client";
import type { Language, RetentionPolicy, StartupStatus, WorkspaceView } from "@/lib/types";
import { errorMessage } from "@/lib/utils";

import { resetDocumentQueries } from "./documents";
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

function afterWorkspaceChange() {
  resetDocumentQueries();
  void useSettingsStore.getState().load();
  void queryClient.invalidateQueries({ queryKey: queryKeys.assessment });
  void queryClient.invalidateQueries({ queryKey: queryKeys.registry });
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
