import { create } from "zustand";

import { api, subscribe } from "@/lib/ipc";
import { queryClient, queryKeys } from "@/lib/query-client";
import { EVENTS, type EngineStatus, type LogLine, type ModelInstanceStatus } from "@/lib/types";

const LOG_LIMIT = 400;

interface EngineState {
  status: EngineStatus | null;
  logs: LogLine[];
  initialized: boolean;
  init: () => Promise<void>;
  refresh: () => Promise<void>;
  start: () => Promise<EngineStatus>;
  stop: () => Promise<void>;
  restart: () => Promise<EngineStatus>;
  loadModel: (modelId: string, warmup?: boolean | null) => Promise<string>;
  unloadModel: (modelId: string) => Promise<void>;
  loadLogs: () => Promise<void>;
}

function invalidateModelQueries() {
  void queryClient.invalidateQueries({ queryKey: queryKeys.assessment });
  void queryClient.invalidateQueries({ queryKey: queryKeys.registry });
  void queryClient.invalidateQueries({ queryKey: queryKeys.modelIndex });
}

export const useEngineStore = create<EngineState>((set, get) => ({
  status: null,
  logs: [],
  initialized: false,

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });
    await subscribe<EngineStatus>(EVENTS.engineStatus, (status) => set({ status }));
    await subscribe<ModelInstanceStatus>(EVENTS.engineModel, (model) => {
      const s = get().status;
      if (!s) return;
      const models = s.models.some((m) => m.id === model.id) ? s.models.map((m) => (m.id === model.id ? model : m)) : [...s.models, model];
      set({ status: { ...s, models } });
    });
    await subscribe<LogLine>(EVENTS.engineLog, (line) => set((st) => ({ logs: [...st.logs.slice(-(LOG_LIMIT - 1)), line] })));
    await subscribe(EVENTS.modelsChanged, () => invalidateModelQueries());
    await get().refresh();
  },

  refresh: async () => {
    try {
      set({ status: await api.engineStatus() });
    } catch {
      /* status is optional in the browser preview */
    }
  },

  start: async () => {
    const status = await api.engineStart();
    set({ status });
    return status;
  },

  stop: async () => {
    await api.engineStop();
    await get().refresh();
  },

  restart: async () => {
    const status = await api.engineRestart();
    set({ status });
    return status;
  },

  loadModel: async (modelId, warmup = null) => {
    const job = await api.engineLoadModel(modelId, warmup);
    await get().refresh();
    return job;
  },

  unloadModel: async (modelId) => {
    await api.engineUnloadModel(modelId);
    await get().refresh();
  },

  loadLogs: async () => {
    try {
      set({ logs: await api.engineLogs(LOG_LIMIT) });
    } catch {
      /* ignore */
    }
  },
}));

export function useEngineStatus(): EngineStatus | null {
  return useEngineStore((s) => s.status);
}

/** Loaded instance with the largest memory footprint (status bar). */
export function loadedModels(status: EngineStatus | null): ModelInstanceStatus[] {
  return status?.models.filter((m) => m.status === "loaded" || m.status === "sleeping") ?? [];
}
