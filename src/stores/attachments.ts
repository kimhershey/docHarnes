import { create } from "zustand";

import type { DragDoc } from "@/lib/dnd/store";

/**
 * Documents attached to the assistant's input (the scope of the next questions / commands) and
 * the one-shot hand-offs a drop on a sidebar entry leaves for the destination screen.
 * Attachments belong to a chat session (0.7.0 §1.6): `attached` is the active session's list,
 * the other sessions' lists wait in `attachedBySession` until they are selected again.
 */
interface AttachmentsState {
  attached: DragDoc[];
  attachedBySession: Record<string, DragDoc[]>;
  attach: (docs: DragDoc[]) => void;
  detach: (id: string) => void;
  clearAttached: () => void;
  /** Chat session switch: parks the active list under `from`, restores the one kept for `to`. */
  switchSession: (from: string | null, to: string | null) => void;
  /** A deleted session's attachments go with it. */
  dropSession: (id: string) => void;
  /** Drop on the 지식 entry: documents to add to a collection / files to ingest. */
  pendingKnowledgeDocs: DragDoc[] | null;
  pendingKnowledgePaths: string[] | null;
  /** Drop on the 프로젝트 entry: documents / files waiting for a project pick. */
  pendingProjectDocs: DragDoc[] | null;
  pendingProjectPaths: string[] | null;
  /** Drop on the 라이브러리 entry: files for the import dialog. */
  pendingLibraryPaths: string[] | null;
  /** Drop on the 스키마 entry: the document to register as a form. */
  pendingFormDoc: DragDoc | null;
  /** Drop on the 문서 생성 entry: documents to quote as sources. */
  pendingGenerateSources: DragDoc[] | null;
  setPending: (patch: Partial<Pick<AttachmentsState, "pendingKnowledgeDocs" | "pendingKnowledgePaths" | "pendingProjectDocs" | "pendingProjectPaths" | "pendingLibraryPaths" | "pendingFormDoc" | "pendingGenerateSources">>) => void;
}

const EMPTY_PENDING = {
  pendingKnowledgeDocs: null,
  pendingKnowledgePaths: null,
  pendingProjectDocs: null,
  pendingProjectPaths: null,
  pendingLibraryPaths: null,
  pendingFormDoc: null,
  pendingGenerateSources: null,
};

export const useAttachmentsStore = create<AttachmentsState>((set) => ({
  attached: [],
  attachedBySession: {},
  ...EMPTY_PENDING,
  attach: (docs) =>
    set((s) => {
      const seen = new Set(s.attached.map((d) => d.id));
      const added = docs.filter((d) => !seen.has(d.id));
      return added.length === 0 ? s : { attached: [...s.attached, ...added] };
    }),
  detach: (id) => set((s) => ({ attached: s.attached.filter((d) => d.id !== id) })),
  clearAttached: () => set({ attached: [] }),
  switchSession: (from, to) =>
    set((s) => {
      if (from === to) return s;
      const parked = { ...s.attachedBySession };
      if (from) parked[from] = s.attached;
      // Without a `from` (first hydration), what was attached before the sessions were known stays attached.
      const carried = from ? [] : s.attached;
      const kept = to ? (parked[to] ?? []) : [];
      const seen = new Set(kept.map((d) => d.id));
      const next = [...kept, ...carried.filter((d) => !seen.has(d.id))];
      if (to) delete parked[to];
      return { attached: next, attachedBySession: parked };
    }),
  dropSession: (id) =>
    set((s) => {
      if (!(id in s.attachedBySession)) return s;
      const parked = { ...s.attachedBySession };
      delete parked[id];
      return { attachedBySession: parked };
    }),
  setPending: (patch) => set(patch),
}));

/** Workspace switch: attachments and hand-offs belong to the old folder. */
export function resetAttachments(): void {
  useAttachmentsStore.setState({ attached: [], attachedBySession: {}, ...EMPTY_PENDING });
}
