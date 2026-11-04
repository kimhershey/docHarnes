import { create } from "zustand";

import { subscribe } from "@/lib/ipc";
import { queryClient, queryKeys } from "@/lib/query-client";
import { EVENTS, type PipelineChanged, type ReviewReason } from "@/lib/types";

/** Pipeline UI state that survives navigation plus the `pipeline://changed` wiring. */
interface PipelineState {
  initialized: boolean;
  /** Review screen filters. */
  reviewDocType: string;
  reviewReason: ReviewReason | "all";
  reviewDocument: string | null;
  /** Last notice from the auto pipeline (engine off / no model), shown once in the library. */
  notice: PipelineChanged["kind"] | null;
  init: () => Promise<void>;
  setReviewFilter: (patch: Partial<Pick<PipelineState, "reviewDocType" | "reviewReason" | "reviewDocument">>) => void;
  /**
   * Narrow the review queue to one document (the type / reason filters are reset so the whole
   * document shows). Sets the filter only; callers navigate to `/review` themselves, e.g.
   * `usePipelineStore.getState().openReviewForDocument(id); navigate("/review")`.
   */
  openReviewForDocument: (id: string) => void;
  clearNotice: () => void;
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

export const usePipelineStore = create<PipelineState>((set, get) => ({
  initialized: false,
  reviewDocType: "all",
  reviewReason: "all",
  reviewDocument: null,
  notice: null,

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });
    await subscribe<PipelineChanged>(EVENTS.pipelineChanged, (ev) => {
      switch (ev.kind) {
        case "schemas":
          invalidateSoon("schemas", [queryKeys.schemas, queryKeys.docTypes, queryKeys.pipelineStatus]);
          break;
        case "engine_off":
        case "no_model":
          set({ notice: ev.kind });
          invalidateSoon("status", [queryKeys.pipelineStatus]);
          break;
        default:
          invalidateSoon("pipeline", [queryKeys.pipelineStatus, ["pipeline", "fields"], ["pipeline", "runs"], ["pipeline", "history"], ["pipeline", "review"], ["pipeline", "tables"], queryKeys.documents]);
      }
    });
  },

  setReviewFilter: (patch) => set(patch),
  openReviewForDocument: (id) => set({ reviewDocument: id, reviewDocType: "all", reviewReason: "all" }),
  clearNotice: () => set({ notice: null }),
}));
