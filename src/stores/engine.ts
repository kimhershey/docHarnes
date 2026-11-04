import { create } from "zustand";

import { api, subscribe } from "@/lib/ipc";
import { queryClient, queryKeys } from "@/lib/query-client";
import { EVENTS, type EngineStatus, type LogLine, type ModelInstanceStatus } from "@/lib/types";

const LOG_LIMIT = 400;

interface EngineState {
  status: EngineStatus | null;
  logs: LogLine[];
  initialized: boolean;
  /** Incremented on every event; snapshots requested before the last event are dropped. */
  eventSeq: number;
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

function mergeLogs(existing: LogLine[], incoming: LogLine[]): LogLine[] {
  const seen = new Set(existing.map((l) => `${l.at}|${l.stream}|${l.text}`));
  const fresh = incoming.filter((l) => !seen.has(`${l.at}|${l.stream}|${l.text}`));
  return [...existing, ...fresh].sort((a, b) => a.at.localeCompare(b.at)).slice(-LOG_LIMIT);
}

export const useEngineStore = create<EngineState>((set, get) => ({
  status: null,
  logs: [],
  initialized: false,
  eventSeq: 0,

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });
    await subscribe<EngineStatus>(EVENTS.engineStatus, (status) => set((s) => ({ status, eventSeq: s.eventSeq + 1 })));
    await subscribe<ModelInstanceStatus>(EVENTS.engineModel, (model) => {
      const s = get().status;
      if (!s) return;
      const models = s.models.some((m) => m.id === model.id) ? s.models.map((m) => (m.id === model.id ? model : m)) : [...s.models, model];
      set((st) => ({ status: { ...s, models }, eventSeq: st.eventSeq + 1 }));
    });
    await subscribe<LogLine>(EVENTS.engineLog, (line) => set((st) => ({ logs: mergeLogs(st.logs, [line]) })));
    await subscribe(EVENTS.modelsChanged, () => invalidateModelQueries());
    await get().refresh();
    // Lines emitted before the webview subscribed (auto-start runs in setup()).
    await get().loadLogs();
  },

  refresh: async () => {
    const before = get().eventSeq;
    try {
      const status = await api.engineStatus();
      // An event delivered meanwhile is newer than this snapshot.
      if (get().eventSeq === before) set({ status });
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
      const lines = await api.engineLogs(LOG_LIMIT);
      set((st) => ({ logs: mergeLogs(st.logs, lines) }));
    } catch {
      /* ignore */
    }
  },
}));

export function useEngineStatus(): EngineStatus | null {
  return useEngineStore((s) => s.status);
}

/** Instances that answer requests (loaded or sleeping). */
export function loadedModels(status: EngineStatus | null): ModelInstanceStatus[] {
  return status?.models.filter((m) => m.status === "loaded" || m.status === "sleeping") ?? [];
}
