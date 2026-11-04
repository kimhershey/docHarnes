import { create } from "zustand";

export type ToastKind = "info" | "ok" | "warn" | "danger";

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
  createdAt: number;
}

interface UiState {
  paletteOpen: boolean;
  paletteQuery: string;
  openPalette: (query?: string) => void;
  closePalette: () => void;
  togglePalette: () => void;
  toasts: Toast[];
  toast: (message: string, kind?: ToastKind) => void;
  dismissToast: (id: number) => void;
  /** Text handed to the assistant from the command bar / palette. */
  pendingAssistantInput: string | null;
  setPendingAssistantInput: (text: string | null) => void;
  /** "Open another folder" / "New workspace" dialog (top bar, settings, palette). */
  workspaceDialog: "open" | "create" | null;
  openWorkspaceDialog: (mode: "open" | "create") => void;
  closeWorkspaceDialog: () => void;
  /** Onboarding wizard stays visible after step 1 opened a workspace. */
  onboardingActive: boolean;
  setOnboardingActive: (active: boolean) => void;
  /** Document (and page) the viewer showed last — the assistant's "이 문서" scope (P5). */
  lastDocument: { id: string; page: number | null } | null;
  setLastDocument: (doc: { id: string; page: number | null } | null) => void;
}

let toastSeq = 1;

export const useUiStore = create<UiState>((set, get) => ({
  paletteOpen: false,
  paletteQuery: "",
  openPalette: (query = "") => set({ paletteOpen: true, paletteQuery: query }),
  closePalette: () => set({ paletteOpen: false, paletteQuery: "" }),
  togglePalette: () => (get().paletteOpen ? get().closePalette() : get().openPalette()),
  toasts: [],
  toast: (message, kind = "info") => {
    const id = toastSeq++;
    set((s) => ({ toasts: [...s.toasts.slice(-4), { id, kind, message, createdAt: Date.now() }] }));
    window.setTimeout(() => get().dismissToast(id), kind === "danger" ? 8000 : 4000);
  },
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
  pendingAssistantInput: null,
  setPendingAssistantInput: (text) => set({ pendingAssistantInput: text }),
  workspaceDialog: null,
  openWorkspaceDialog: (mode) => set({ workspaceDialog: mode }),
  closeWorkspaceDialog: () => set({ workspaceDialog: null }),
  onboardingActive: false,
  setOnboardingActive: (active) => set({ onboardingActive: active }),
  lastDocument: null,
  setLastDocument: (doc) => set({ lastDocument: doc }),
}));

export function toast(message: string, kind: ToastKind = "info"): void {
  useUiStore.getState().toast(message, kind);
}
