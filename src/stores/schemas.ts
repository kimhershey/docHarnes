import { create } from "zustand";

import type { SchemaDiscoveryOutcome } from "@/lib/types";

/**
 * Hand-over of a finished `schema_discovery` job to the 양식 screen (0.3.0 §2): the jobs page's
 * "편집기에서 열기" and the assistant store the outcome here; `SchemasPage` takes it when it mounts
 * (or at once when it is open) and opens the merged draft unsaved.
 */
interface SchemasState {
  pending: SchemaDiscoveryOutcome | null;
  openDiscovery: (outcome: SchemaDiscoveryOutcome) => void;
  take: () => SchemaDiscoveryOutcome | null;
}

export const useSchemasStore = create<SchemasState>((set, get) => ({
  pending: null,
  openDiscovery: (outcome) => set({ pending: outcome }),
  take: () => {
    const p = get().pending;
    if (p) set({ pending: null });
    return p;
  },
}));
