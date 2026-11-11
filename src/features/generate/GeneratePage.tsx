import { CheckCheck, FilePlus2, FolderOpen, Library, ListTree, Save } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Button } from "@/components/ui/button";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { describeJobError } from "@/features/jobs/describe";
import { describeError } from "@/lib/errors";
import { api, revealPath } from "@/lib/ipc";
import { invalidateDrafts } from "@/lib/queries";
import { isJobFinished, isDraftOutcome, type Draft, type DraftSummary, type Language, type SaveRequest, type SavedDocument } from "@/lib/types";
import { errorCode } from "@/lib/utils";
import { useAttachmentsStore } from "@/stores/attachments";
import { persistDraft, syncDraftWithStore, useGenerateStore } from "@/stores/generate";
import { useJob, useJobsStore } from "@/stores/jobs";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

import { anchorScope, documentSourceIds, followScopeAnchor, NO_SCOPE, pickerToReviseScope, reviseRequest, reviseScopeLabel, scopeSectionOrdinal, toggleExcluded, withDocumentSources, withScopeOrdinal, type ScopeAnchor } from "./draft";
import { DraftAssistantPanel, type ReviseLogEntry } from "./DraftAssistantPanel";
import { DraftEditor } from "./DraftEditor";
import { DraftPreview } from "./DraftPreview";
import { DraftsDrawer } from "./DraftsDrawer";
import { RequestForm } from "./RequestForm";
import { SaveDialog } from "./SaveDialog";

/**
 * 문서 생성 (design §3.13, §3.14.4): request form → `draft` job → preview (left) + editable fields / tables /
 * body / references (right) → validate → save (HWPX / DOCX / PDF / MD, library registration). Drafts handed
 * over by the assistant's `draft_document` result card or the jobs page open here (`useGenerateStore.openDraft`).
 */
export function GeneratePage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const workspace = useWorkspaceStore((s) => s.current);
  const language = useSettingsStore((s) => s.settings?.ui.language ?? "ko") as Language;
  const request = useGenerateStore((s) => s.request);
  const draft = useGenerateStore((s) => s.draft);
  const jobId = useGenerateStore((s) => s.jobId);
  const handoff = useGenerateStore((s) => s.handoff);
  const reloaded = useGenerateStore((s) => s.reloaded);
  const setRequest = useGenerateStore((s) => s.setRequest);
  const setDraft = useGenerateStore((s) => s.setDraft);
  const editDraft = useGenerateStore((s) => s.editDraft);
  const setJobId = useGenerateStore((s) => s.setJobId);
  const job = useJob(jobId);
  // 초안 어시스턴트 (0.7.0 §1.5): scope picker value, the new section's heading, the instruction and the log of
  // finished revisions; `pendingRevise` remembers what the running job was asked so its outcome can be logged.
  const [reviseScope, setReviseScope] = useState("whole");
  const [reviseHeading, setReviseHeading] = useState("");
  const [instruction, setInstruction] = useState("");
  const [reviseLog, setReviseLog] = useState<ReviseLogEntry[]>([]);
  const pendingRevise = useRef<{ instruction: string; scope: string } | null>(null);
  // The section the scope names (followed across edits) and the draft the scope belongs to.
  const scopeAnchor = useRef<ScopeAnchor | null>(null);
  const scopeDraft = useRef<string | null>(null);
  const logSeq = useRef(0);
  const instructionRef = useRef<HTMLTextAreaElement>(null);
  const [excluded, setExcluded] = useState<string[]>([]);
  const [saveOpen, setSaveOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saved, setSaved] = useState<SavedDocument | null>(null);
  const [draftsOpen, setDraftsOpen] = useState(false);
  const [validating, setValidating] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const handled = useRef<string | null>(null);
  // The autosave: the timer of the pause after the last edit and the draft it will write.
  const putTimer = useRef<number | null>(null);
  const pendingPut = useRef<Draft | null>(null);
  // True until the draft on the screen has been checked against the draft store once (the editor is locked meanwhile).
  const [arriving, setArriving] = useState(true);

  const workspacePath = workspace?.path ?? null;
  useEffect(() => {
    useGenerateStore.getState().bind(workspacePath, language);
  }, [workspacePath, language]);
  // Documents dropped on the sidebar's 문서 생성 entry become sources once the store is bound.
  const pendingSources = useAttachmentsStore((s) => s.pendingGenerateSources);
  useEffect(() => {
    if (!pendingSources || !workspacePath) return;
    useAttachmentsStore.getState().setPending({ pendingGenerateSources: null });
    const ids = pendingSources.map((d) => d.id);
    useGenerateStore.getState().setRequest((req) => ({ ...req, sources: withDocumentSources(req.sources, [...new Set([...documentSourceIds(req.sources), ...ids])]) }));
    toast(t("generate.form.sources_added", { count: ids.length }), "ok");
  }, [pendingSources, workspacePath, toast, t]);
  const cancelPersist = useCallback(() => {
    if (putTimer.current) window.clearTimeout(putTimer.current);
    putTimer.current = null;
    pendingPut.current = null;
  }, []);
  // Leaving the screen (GEN-7): an edit still waiting for the autosave pause is written at once, not dropped —
  // the assistant's `revise_draft` and the draft list work on the stored draft.
  const flushPersist = useCallback(() => {
    const waiting = pendingPut.current;
    cancelPersist();
    if (waiting && useGenerateStore.getState().jobId === null) void persistDraft(waiting).then((ok) => ok && invalidateDrafts());
  }, [cancelPersist]);
  useEffect(() => flushPersist, [flushPersist]);
  // Edits are persisted to the draft store a moment after the last keystroke (the saved-draft list shows them).
  // Nothing is persisted while a draft job runs: its result replaces the draft.
  const persist = useCallback(
    (d: Draft) => {
      cancelPersist();
      pendingPut.current = d;
      putTimer.current = window.setTimeout(() => {
        putTimer.current = null;
        pendingPut.current = null;
        if (useGenerateStore.getState().jobId !== null) return;
        void persistDraft(d).then((ok) => ok && invalidateDrafts());
      }, 1200);
    },
    [cancelPersist],
  );

  // A draft handed over from the assistant / jobs page: announce it once.
  useEffect(() => {
    if (!handoff) return;
    useGenerateStore.getState().consumeHandoff();
    setExcluded(draft?.request?.excluded_sources ?? []);
    setSaved(null);
    toast(t("generate.handoff"), "info");
  }, [handoff, draft, toast, t]);

  // The draft on the screen against its stored copy (GEN-4 / GEN-7): when the screen is entered, when another
  // draft becomes current and whenever a plan / draft job finishes elsewhere — the assistant's `revise_draft`
  // writes to the draft store, a result card hands over the draft as it was when its job finished.
  const draftId = draft?.id ?? null;
  const otherJobs = useJobsStore((s) => {
    const done = s.jobs.filter((j) => (j.kind === "plan" || j.kind === "draft") && isJobFinished(j));
    return `${done.length}:${done[done.length - 1]?.id ?? ""}`;
  });
  useEffect(() => {
    void syncDraftWithStore().finally(() => setArriving(false));
  }, [draftId, otherJobs]);
  useEffect(() => {
    if (!reloaded) return;
    useGenerateStore.getState().consumeReloaded();
    setSaved(null);
    toast(t("generate.reloaded"), "info");
  }, [reloaded, toast, t]);

  // The job is "running" from the moment `generate_draft` answers until the job book reports a terminal state
  // (the `jobs://update` for the new job may arrive after the response: `job` is null for a tick).
  const running = jobId !== null && (!job || !isJobFinished(job));

  // Follow the draft job: the finished job's result is a DraftOutcome (fetched with jobs_get, the broadcast omits it).
  useEffect(() => {
    if (!job || !isJobFinished(job) || handled.current === job.id) return;
    handled.current = job.id;
    // The finished draft replaces the editor's state: drop a pending edit persist of the previous draft.
    cancelPersist();
    void (async () => {
      let result: unknown = job.result;
      if (!isDraftOutcome(result)) {
        const full = await useJobsStore.getState().fetch(job.id);
        result = full?.result;
      }
      const asked = pendingRevise.current;
      pendingRevise.current = null;
      if (job.state === "done") {
        if (isDraftOutcome(result)) {
          setDraft(result.draft);
          setSaved(null);
          setInstruction("");
          if (asked) {
            // The core / mock records the revision as the draft's last note.
            const note = result.draft.notes[result.draft.notes.length - 1] ?? null;
            setReviseLog((log) => [...log, { id: ++logSeq.current, instruction: asked.instruction, scope: asked.scope, note }]);
          }
          invalidateDrafts();
        } else {
          // Job book cleared / result not delivered: the draft store still has it.
          toast(t("generate.form.job_result_missing"), "warn", { label: t("generate.drafts"), onClick: () => setDraftsOpen(true) });
        }
      } else {
        // The job delivered no draft, so the one on the screen stays. A revision cancelled between sections has
        // written its partial result over the stored copy (GEN-3): the screen's draft is written back, so the
        // stored copy is the complete draft again and the next check against the store does not read the partial one.
        const onScreen = useGenerateStore.getState().draft;
        if (asked && onScreen) {
          editDraft(onScreen);
          persist(onScreen);
        }
        if (job.state === "failed") toast(t("generate.form.job_failed", { message: describeJobError(job, t) ?? "" }), "danger");
        else if (job.state === "cancelled") toast(t("generate.form.job_cancelled"), "warn");
        else if (job.state === "interrupted") toast(t("generate.form.job_interrupted", { reason: t("jobs.error.interrupted") }), "warn");
      }
      setJobId(null);
    })();
  }, [job, cancelPersist, persist, setDraft, editDraft, setJobId, toast, t]);

  // The scope names a section by its ordinal, and edits shift ordinals (GEN-10). The picked section is followed
  // by its heading; when it is gone — its heading block removed, the draft rewritten — the scope is cleared and
  // the user is told. It never quietly becomes "전체 다시 쓰기" or another section.
  const body = draft?.body;
  useEffect(() => {
    if (scopeDraft.current !== draftId) {
      // Another draft (new, opened, handed over): the revision log and the scope belong to the old one.
      scopeDraft.current = draftId;
      scopeAnchor.current = null;
      setReviseLog([]);
      setReviseScope("whole");
      setReviseHeading("");
      return;
    }
    if (!body) return;
    const anchor = scopeAnchor.current;
    if (!anchor) {
      scopeAnchor.current = anchorScope(reviseScope, body);
      if (scopeSectionOrdinal(reviseScope) !== null && !scopeAnchor.current) setReviseScope(NO_SCOPE);
      return;
    }
    const ordinal = followScopeAnchor(anchor, body);
    if (ordinal === null) {
      scopeAnchor.current = null;
      setReviseScope(NO_SCOPE);
      toast(t("generate.revise.scope_lost"), "warn");
      return;
    }
    const next = withScopeOrdinal(reviseScope, ordinal);
    scopeAnchor.current = anchorScope(next, body);
    if (next !== reviseScope) setReviseScope(next);
  }, [draftId, body, reviseScope, toast, t]);
  /** The scope picked in the panel or by a section's "AI 수정" button. */
  const pickScope = (value: string) => {
    scopeAnchor.current = draft ? anchorScope(value, draft.body) : null;
    setReviseScope(value);
  };

  const start = useCallback(
    async (req: typeof request) => {
      // An edit persisted after the job finishes would overwrite the regenerated draft (stale `generate_draft_put`).
      cancelPersist();
      try {
        const { job_id } = await api.generateDraft(req);
        handled.current = null;
        setJobId(job_id);
      } catch (e) {
        // A revision that never started must not be logged against the next generation.
        pendingRevise.current = null;
        toast(describeError(e, t), "danger");
      }
    },
    [cancelPersist, setJobId, toast, t],
  );

  const generate = () => void start({ ...request, previous: null, feedback: null, excluded_sources: [], revise: null, language });
  /** 초안 어시스턴트: the current draft, the picked scope and the instruction go through the ordinary draft job. */
  const revise = () => {
    if (!draft || !instruction.trim() || reviseScope === NO_SCOPE) return;
    const scope = pickerToReviseScope(reviseScope, reviseHeading);
    pendingRevise.current = { instruction: instruction.trim(), scope: reviseScopeLabel(scope, draft.body, language !== "en") };
    void start(reviseRequest({ ...request, language }, draft, instruction, scope, excluded));
  };
  /** The editor's per-section "AI 수정" button: pick that section and go to the instruction box. */
  const reviseSection = (ordinal: number) => {
    pickScope(`section:${ordinal}`);
    instructionRef.current?.focus();
  };
  const cancel = () => {
    if (jobId) void useJobsStore.getState().cancel(jobId);
  };

  const changeDraft = (d: Draft) => {
    editDraft(d);
    setSaved(null);
    persist(d);
  };

  const validate = async () => {
    if (!draft) return;
    setValidating(true);
    try {
      const next = await api.generateValidate(draft);
      editDraft(next);
      persist(next);
      toast(t("generate.editor.validated", { count: next.issues.length }), next.issues.some((i) => i.blocking) ? "warn" : "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setValidating(false);
    }
  };

  const save = async (req: SaveRequest) => {
    setSaving(true);
    setSaveError(null);
    // The backend stores the saved draft itself: drop a pending edit persist so it cannot overwrite the saved path.
    cancelPersist();
    try {
      const res = await api.generateSave(req);
      // The backend validated the draft and stored it with the saved path: the screen shows that stored copy
      // (what was sent plus the path when it cannot be read back).
      const stored = await api.generateDraftGet(req.draft.id).catch(() => null);
      setDraft(stored ?? { ...req.draft, saved_path: res.rel_path, saved_doc_id: res.doc_id });
      setSaved(res);
      setSaveOpen(false);
      invalidateDrafts();
      // The format really written can differ from the request (PDF printer unavailable → MD, registered form → its format).
      const message = res.format !== req.output ? t("generate.save.saved_as", { path: res.rel_path, format: t(`generate.output.${res.format}`) }) : t("generate.save.saved", { path: res.rel_path });
      toast(message, "ok", res.doc_id ? { label: t("generate.save.open_library"), onClick: () => navigate(`/library/${res.doc_id}`) } : { label: t("generate.save.open_folder"), onClick: () => void revealPath(res.absolute) });
      // `warnings` are localised sentences from the core (fallbacks, fill / registration problems): shown as they are.
      for (const w of res.warnings) toast(w, "warn");
    } catch (e) {
      const code = errorCode(e);
      const message = String((e as { message?: unknown }).message ?? "");
      if (code === "invalid" && (message.includes("export_exists") || message.includes("file_exists"))) setSaveError(t("generate.save.exists"));
      else setSaveError(describeError(e, t));
    } finally {
      setSaving(false);
    }
  };

  const openSaved = async (d: DraftSummary) => {
    try {
      const full = await api.generateDraftGet(d.id);
      cancelPersist();
      useGenerateStore.getState().openDraft(full, { fresh: true });
      useGenerateStore.getState().consumeHandoff();
      setExcluded(full.request?.excluded_sources ?? []);
      setSaved(null);
      setDraftsOpen(false);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const deleteSaved = (d: DraftSummary) => {
    setConfirm({
      title: t("generate.drafts_list.delete"),
      description: t("generate.drafts_list.delete_confirm", { title: d.title }),
      confirmLabel: t("generate.drafts_list.delete"),
      danger: true,
      onConfirm: async () => {
        try {
          // A pending edit persist would re-create the draft right after the delete.
          if (draft?.id === d.id) cancelPersist();
          await api.generateDraftDelete(d.id);
          invalidateDrafts();
          if (draft?.id === d.id) setDraft(null);
          toast(t("generate.drafts_list.deleted"), "ok");
        } catch (e) {
          toast(describeError(e, t), "danger");
        }
      },
    });
  };

  const reset = () => {
    cancelPersist();
    useGenerateStore.getState().reset(language);
    setInstruction("");
    setReviseLog([]);
    pickScope("whole");
    setReviseHeading("");
    setExcluded([]);
    setSaved(null);
  };

  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="page-generate">
      <div className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-2">
        <h1 className="text-base font-semibold">{t("generate.title")}</h1>
        <Button variant="ghost" size="sm" onClick={reset} data-testid="generate-new">
          <FilePlus2 />
          {t("generate.new")}
        </Button>
        <Button variant="ghost" size="sm" onClick={() => setDraftsOpen(true)} data-testid="generate-drafts-open">
          <ListTree />
          {t("generate.drafts")}
        </Button>
        {draft && (
          <div className="ml-auto flex flex-wrap items-center gap-2">
            {saved && (
              <span className="flex items-center gap-1 text-xs text-fg-muted" data-testid="generate-saved">
                <span className="font-mono selectable">{saved.rel_path}</span>
                <Button size="sm" variant="ghost" onClick={() => void revealPath(saved.absolute)} data-testid="generate-saved-folder">
                  <FolderOpen />
                  {t("generate.save.open_folder")}
                </Button>
                {saved.doc_id && (
                  <Button size="sm" variant="ghost" onClick={() => navigate(`/library/${saved.doc_id}`)} data-testid="generate-saved-library">
                    <Library />
                    {t("generate.save.open_library")}
                  </Button>
                )}
              </span>
            )}
            <Button size="sm" variant="outline" disabled={validating || running} onClick={() => void validate()} data-testid="generate-validate">
              <CheckCheck />
              {t("generate.editor.validate")}
            </Button>
            <Button size="sm" disabled={running} onClick={() => setSaveOpen(true)} data-testid="generate-save">
              <Save />
              {t("generate.editor.save")}
            </Button>
          </div>
        )}
      </div>
      {/* Layout by the width available to the screen in rem (a container query, so the UI 배율 counts like a narrower
          window): three columns from 64 rem; below that the editor takes a second row across the full width instead of
          being squeezed under the form. Every cell scrolls on its own (0.7.0 §1.3): the columns clip sideways and the
          preview clips both ways, so the page / `main` never grows a scrollbar. */}
      <div className="@container flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <div className="grid min-h-0 min-w-0 flex-1 grid-cols-[16rem_minmax(0,1fr)] grid-rows-[minmax(0,3fr)_minmax(0,2fr)] overflow-hidden @5xl:grid-cols-[18rem_minmax(0,1fr)_24rem] @5xl:grid-rows-1 @7xl:grid-cols-[20rem_minmax(0,1fr)_26rem]" data-testid="generate-layout">
          <aside className="flex min-h-0 min-w-0 flex-col gap-3 overflow-y-auto overflow-x-hidden border-r border-border p-4">
            <RequestForm request={request} onChange={(patch) => setRequest(patch)} onGenerate={generate} job={job} running={running} onCancel={cancel} />
            {draft && <DraftAssistantPanel ref={instructionRef} draft={draft} scope={reviseScope} onScope={pickScope} heading={reviseHeading} onHeading={setReviseHeading} instruction={instruction} onInstruction={setInstruction} onRun={revise} running={running} excludedCount={excluded.length} log={reviseLog} />}
          </aside>
          <section className="min-h-0 min-w-0 overflow-hidden" aria-label={t("generate.preview.title")}>
            <DraftPreview draft={draft} />
          </section>
          <aside className="col-span-2 min-h-0 min-w-0 overflow-y-auto overflow-x-hidden border-t border-border p-4 @5xl:col-span-1 @5xl:border-l @5xl:border-t-0" aria-label={t("generate.editor.title")}>
            {draft ? <DraftEditor draft={draft} onChange={changeDraft} excluded={excluded} onToggleExcluded={(id) => setExcluded((x) => toggleExcluded(x, id))} disabled={running || arriving} onRevise={reviseSection} /> : <p className="text-sm text-fg-subtle">{t("generate.editor.empty")}</p>}
          </aside>
        </div>
      </div>
      {draft && <SaveDialog open={saveOpen} onOpenChange={(o) => { setSaveOpen(o); if (!o) setSaveError(null); }} draft={draft} defaultOutput={request.output} busy={saving} error={saveError} onSave={(req) => void save(req)} />}
      <DraftsDrawer open={draftsOpen} onOpenChange={setDraftsOpen} onOpen={(d) => void openSaved(d)} onDelete={deleteSaved} currentId={draft?.id ?? null} />
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
