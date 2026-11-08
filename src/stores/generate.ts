import { create } from "zustand";

import { defaultDraftRequest, type Draft, type DraftRequest, type Language } from "@/lib/types";

/**
 * 문서 생성 screen state (design §3.13) that survives navigation: the request
 * form, the current draft and the job producing it, per workspace. The
 * assistant's result card and the jobs page hand a finished draft over with
 * `openDraft`, which the page picks up when it mounts.
 */
interface GenerateState {
  /** Workspace the state belongs to (null = none yet). */
  workspace: string | null;
  request: DraftRequest;
  draft: Draft | null;
  /** The `draft` job the screen follows (null when idle). */
  jobId: string | null;
  /** Set by `openDraft`: the page announces the handed-over draft once. */
  handoff: boolean;
  bind: (workspace: string | null, language: Language) => void;
  setRequest: (patch: Partial<DraftRequest> | ((req: DraftRequest) => DraftRequest)) => void;
  setDraft: (draft: Draft | null) => void;
  updateDraft: (f: (draft: Draft) => Draft) => void;
  setJobId: (jobId: string | null) => void;
  /** Opens a draft produced elsewhere (assistant plan, jobs page, saved-draft list). */
  openDraft: (draft: Draft) => void;
  consumeHandoff: () => void;
  /** "새로 만들기": clears the draft and the job, keeps the form. */
  reset: (language: Language) => void;
}

export const useGenerateStore = create<GenerateState>((set, get) => ({
  workspace: null,
  request: defaultDraftRequest(),
  draft: null,
  jobId: null,
  handoff: false,

  bind: (workspace, language) => {
    const s = get();
    if (s.workspace === workspace) return;
    // First mount (or after `resetGenerate`): adopt the workspace and keep a draft handed over before the
    // page was ever shown; a real switch to another workspace starts over.
    if (s.workspace === null) {
      set({ workspace, request: s.request.content || s.draft ? s.request : defaultDraftRequest(language) });
      return;
    }
    set({ workspace, request: defaultDraftRequest(language), draft: null, jobId: null, handoff: false });
  },

  setRequest: (patch) => set((s) => ({ request: typeof patch === "function" ? patch(s.request) : { ...s.request, ...patch } })),

  setDraft: (draft) => set({ draft }),

  updateDraft: (f) => set((s) => (s.draft ? { draft: f(s.draft) } : s)),

  setJobId: (jobId) => set({ jobId }),

  openDraft: (draft) =>
    set((s) => ({
      draft,
      jobId: null,
      handoff: true,
      request: draft.request ? { ...draft.request, previous: null, feedback: null } : { ...s.request, content: s.request.content || draft.title },
    })),

  consumeHandoff: () => set({ handoff: false }),

  reset: (language) => set((s) => ({ draft: null, jobId: null, handoff: false, request: { ...defaultDraftRequest(language), form_ref: s.request.form_ref, output: s.request.output, tone: s.request.tone, length: s.request.length, use_knowledge: s.request.use_knowledge } })),
}));

/** Workspace switch: the draft belongs to the old workspace. */
export function resetGenerate(): void {
  useGenerateStore.setState({ workspace: null, request: defaultDraftRequest(), draft: null, jobId: null, handoff: false });
}
