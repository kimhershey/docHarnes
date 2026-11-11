import { create } from "zustand";

import { api } from "@/lib/ipc";
import { defaultDraftRequest, type Draft, type DraftRequest, type DraftSummary, type Language } from "@/lib/types";

/**
 * 문서 생성 screen state (design §3.13) that survives navigation: the request
 * form, the current draft and the job producing it, per workspace. The
 * assistant's result card and the jobs page hand a finished draft over with
 * `openDraft`, which the page picks up when it mounts.
 *
 * The draft on the screen is one copy of a draft that also lives in the draft store
 * (`.doculens/drafts/<id>.json`), where a draft job, the assistant's `revise_draft` and the autosave
 * write. `dirty` / `syncedAt` say how the two relate, so that the copy with the newer content wins:
 * unsaved edits are written (never replaced), and a stored draft that changed elsewhere is read back
 * (`syncDraftWithStore`, GEN-4 / GEN-7).
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
  /** The draft has edits the draft store has not received (autosave pending or failed): it is written, never replaced. */
  dirty: boolean;
  /** `updated_at` of the stored version the draft on the screen is known to contain (null = no draft). */
  syncedAt: string | null;
  /** Set when the stored draft replaced the one on the screen: the page says so once. */
  reloaded: boolean;
  bind: (workspace: string | null, language: Language) => void;
  setRequest: (patch: Partial<DraftRequest> | ((req: DraftRequest) => DraftRequest)) => void;
  /** A draft as the backend delivered it (job result, saved file, cleared): nothing to write. */
  setDraft: (draft: Draft | null) => void;
  /** A draft changed on the screen (an edit, a validation run): to be written to the draft store. */
  editDraft: (draft: Draft) => void;
  updateDraft: (f: (draft: Draft) => Draft) => void;
  setJobId: (jobId: string | null) => void;
  /**
   * Opens a draft produced elsewhere (assistant plan, jobs page, saved-draft list). A result card or a job
   * keeps the draft as it was when the job finished: unless `fresh` (just read from the draft store), the
   * stored draft is read by id and used when it is newer (GEN-4).
   */
  openDraft: (draft: Draft, opts?: { fresh?: boolean }) => void;
  consumeHandoff: () => void;
  consumeReloaded: () => void;
  /** "새로 만들기": clears the draft and the job, keeps the form. */
  reset: (language: Language) => void;
}

const CLEAN = { dirty: false, syncedAt: null, reloaded: false } as const;

export const useGenerateStore = create<GenerateState>((set, get) => ({
  workspace: null,
  request: defaultDraftRequest(),
  draft: null,
  jobId: null,
  handoff: false,
  ...CLEAN,

  bind: (workspace, language) => {
    const s = get();
    if (s.workspace === workspace) return;
    // First mount (or after `resetGenerate`): adopt the workspace and keep a draft handed over before the
    // page was ever shown; a real switch to another workspace starts over.
    if (s.workspace === null) {
      set({ workspace, request: s.request.content || s.draft ? s.request : defaultDraftRequest(language) });
      return;
    }
    set({ workspace, request: defaultDraftRequest(language), draft: null, jobId: null, handoff: false, ...CLEAN });
  },

  setRequest: (patch) => set((s) => ({ request: typeof patch === "function" ? patch(s.request) : { ...s.request, ...patch } })),

  setDraft: (draft) => set({ draft, dirty: false, syncedAt: draft?.updated_at ?? null }),

  editDraft: (draft) => set((s) => ({ draft, dirty: true, syncedAt: s.draft?.id === draft.id ? s.syncedAt : draft.updated_at })),

  updateDraft: (f) => set((s) => (s.draft ? { draft: f(s.draft), dirty: true } : s)),

  setJobId: (jobId) => set({ jobId }),

  openDraft: (draft, opts) => {
    set((s) => ({
      draft,
      jobId: null,
      handoff: true,
      dirty: false,
      syncedAt: draft.updated_at,
      reloaded: false,
      request: draft.request ? { ...draft.request, previous: null, feedback: null, revise: null } : { ...s.request, content: s.request.content || draft.title },
    }));
    if (!opts?.fresh) void syncDraftWithStore();
  },

  consumeHandoff: () => set({ handoff: false }),

  consumeReloaded: () => set({ reloaded: false }),

  reset: (language) => set((s) => ({ draft: null, jobId: null, handoff: false, ...CLEAN, request: { ...defaultDraftRequest(language), form_ref: s.request.form_ref, output: s.request.output, tone: s.request.tone, length: s.request.length, use_knowledge: s.request.use_knowledge } })),
}));

/** Workspace switch: the draft belongs to the old workspace. */
export function resetGenerate(): void {
  useGenerateStore.setState({ workspace: null, request: defaultDraftRequest(), draft: null, jobId: null, handoff: false, ...CLEAN });
}

// ---------------------------------------------------------------------------
// The draft on the screen ↔ the draft store
// ---------------------------------------------------------------------------

/** The two draft-store calls the sync needs (replaced in tests). */
export interface DraftStoreIo {
  get: (id: string) => Promise<Draft>;
  put: (draft: Draft) => Promise<DraftSummary>;
}

const draftStore: DraftStoreIo = { get: (id) => api.generateDraftGet(id), put: (draft) => api.generateDraftPut(draft) };

/** Writes are serialised; a sync waits for the ones on their way out before it reads. */
let writes: Promise<unknown> = Promise.resolve();

/**
 * Writes a draft to the draft store (the screen's autosave and its flush when the screen is left). When the
 * draft on the screen is still that draft it is clean afterwards. Resolves to whether the write succeeded.
 */
export function persistDraft(draft: Draft, io: DraftStoreIo = draftStore): Promise<boolean> {
  const run = writes.then(async () => {
    try {
      const summary = await io.put(draft);
      useGenerateStore.setState((s) => (s.draft === draft ? { dirty: false, syncedAt: summary.updated_at } : s.draft?.id === draft.id ? { syncedAt: summary.updated_at } : s));
      return true;
    } catch {
      return false;
    }
  });
  writes = run;
  return run;
}

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(value).sort()) {
      const v = (value as Record<string, unknown>)[key];
      // A missing key, null and undefined are the same thing on the two sides of the IPC.
      if (v !== null && v !== undefined) out[key] = canonical(v);
    }
    return out;
  }
  return value;
}

/** Whether two copies of a draft say the same thing (`updated_at` and key order aside). */
export function sameDraftContent(a: Draft, b: Draft): boolean {
  const text = (d: Draft) => JSON.stringify(canonical({ ...d, updated_at: null }));
  return text(a) === text(b);
}

function isNewer(a: string, b: string): boolean {
  const ta = Date.parse(a);
  const tb = Date.parse(b);
  // An unreadable timestamp on the screen's side (a draft without one) counts as old.
  return Number.isFinite(ta) && (!Number.isFinite(tb) || ta > tb);
}

/**
 * What a sync did: `none` (no draft, or a draft job is running: its result replaces the draft), `unchanged`,
 * `written` (unsaved edits went to the store), `adopted` (the store was rewritten with the same content) or
 * `reloaded` (the stored draft was newer and different: it is on the screen now).
 */
export type DraftSync = "none" | "unchanged" | "written" | "adopted" | "reloaded";

let syncRun: { id: string; io: DraftStoreIo; promise: Promise<DraftSync> } | null = null;

/**
 * Brings the draft on the screen and its stored copy together (GEN-4, GEN-7): unsaved edits are written; else
 * a stored draft that is newer than what the screen holds — the assistant revised it, a result card held an
 * older snapshot — is read back. One check per draft at a time: concurrent callers share it.
 */
export function syncDraftWithStore(io: DraftStoreIo = draftStore): Promise<DraftSync> {
  const start = useGenerateStore.getState();
  if (!start.draft || start.jobId !== null) return Promise.resolve("none");
  const id = start.draft.id;
  if (syncRun && syncRun.id === id && syncRun.io === io) return syncRun.promise;
  const promise: Promise<DraftSync> = (async (): Promise<DraftSync> => {
    await writes;
    const before = useGenerateStore.getState();
    if (!before.draft || before.draft.id !== id || before.jobId !== null) return "none";
    if (before.dirty) return (await persistDraft(before.draft, io)) ? "written" : "unchanged";
    let stored: Draft;
    try {
      stored = await io.get(id);
    } catch {
      // Not in the draft store (deleted there) or unreadable: what is on the screen stays.
      return "unchanged";
    }
    const now = useGenerateStore.getState();
    // Edited, replaced or handed to a job while the store answered: the screen is ahead.
    if (!now.draft || now.draft.id !== id || now.dirty || now.jobId !== null) return "unchanged";
    if (!isNewer(stored.updated_at, now.syncedAt ?? now.draft.updated_at)) return "unchanged";
    if (sameDraftContent(stored, now.draft)) {
      useGenerateStore.setState({ syncedAt: stored.updated_at });
      return "adopted";
    }
    useGenerateStore.setState({ draft: stored, dirty: false, syncedAt: stored.updated_at, reloaded: true });
    return "reloaded";
  })().finally(() => {
    if (syncRun?.promise === promise) syncRun = null;
  });
  syncRun = { id, io, promise };
  return promise;
}
