import { CheckCheck, FilePlus2, FolderOpen, Library, ListTree, Save } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Button } from "@/components/ui/button";
import { describeJobError } from "@/features/jobs/describe";
import { describeError } from "@/lib/errors";
import { api, revealPath } from "@/lib/ipc";
import { invalidateDrafts } from "@/lib/queries";
import { isJobFinished, isDraftOutcome, type Draft, type DraftSummary, type Language, type SaveRequest, type SavedDocument } from "@/lib/types";
import { errorCode } from "@/lib/utils";
import { useGenerateStore } from "@/stores/generate";
import { useJob, useJobsStore } from "@/stores/jobs";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

import { regenerationRequest, toggleExcluded } from "./draft";
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
  const setRequest = useGenerateStore((s) => s.setRequest);
  const setDraft = useGenerateStore((s) => s.setDraft);
  const setJobId = useGenerateStore((s) => s.setJobId);
  const job = useJob(jobId);
  const [feedback, setFeedback] = useState("");
  const [excluded, setExcluded] = useState<string[]>([]);
  const [saveOpen, setSaveOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saved, setSaved] = useState<SavedDocument | null>(null);
  const [draftsOpen, setDraftsOpen] = useState(false);
  const [validating, setValidating] = useState(false);
  const handled = useRef<string | null>(null);
  const putTimer = useRef<number | null>(null);

  const workspacePath = workspace?.path ?? null;
  useEffect(() => {
    useGenerateStore.getState().bind(workspacePath, language);
  }, [workspacePath, language]);
  useEffect(
    () => () => {
      if (putTimer.current) window.clearTimeout(putTimer.current);
    },
    [],
  );

  // A draft handed over from the assistant / jobs page: announce it once.
  useEffect(() => {
    if (!handoff) return;
    useGenerateStore.getState().consumeHandoff();
    setExcluded(draft?.request?.excluded_sources ?? []);
    setSaved(null);
    toast(t("generate.handoff"), "info");
  }, [handoff, draft, toast, t]);

  const running = !!job && !isJobFinished(job);

  // Follow the draft job: the finished job's result is a DraftOutcome (fetched with jobs_get, the broadcast omits it).
  useEffect(() => {
    if (!job || !isJobFinished(job) || handled.current === job.id) return;
    handled.current = job.id;
    void (async () => {
      let result: unknown = job.result;
      if (!isDraftOutcome(result)) {
        const full = await useJobsStore.getState().fetch(job.id);
        result = full?.result;
      }
      if (job.state === "done" && isDraftOutcome(result)) {
        setDraft(result.draft);
        setSaved(null);
        setFeedback("");
        invalidateDrafts();
      } else if (job.state === "failed") {
        toast(t("generate.form.job_failed", { message: describeJobError(job, t) ?? "" }), "danger");
      } else if (job.state === "cancelled") {
        toast(t("generate.form.job_cancelled"), "warn");
      }
      setJobId(null);
    })();
  }, [job, setDraft, setJobId, toast, t]);

  const start = useCallback(
    async (req: typeof request) => {
      try {
        const { job_id } = await api.generateDraft(req);
        handled.current = null;
        setJobId(job_id);
      } catch (e) {
        toast(describeError(e, t), "danger");
      }
    },
    [setJobId, toast, t],
  );

  const generate = () => void start({ ...request, previous: null, feedback: null, excluded_sources: [], language });
  const regenerate = () => {
    if (!draft) return;
    void start(regenerationRequest({ ...request, language }, draft, feedback, excluded));
  };
  const cancel = () => {
    if (jobId) void useJobsStore.getState().cancel(jobId);
  };

  // Edits are persisted to the draft store a moment after the last keystroke (the saved-draft list shows them).
  const persist = useCallback((d: Draft) => {
    if (putTimer.current) window.clearTimeout(putTimer.current);
    putTimer.current = window.setTimeout(() => {
      putTimer.current = null;
      void api
        .generateDraftPut(d)
        .then(() => invalidateDrafts())
        .catch(() => undefined);
    }, 1200);
  }, []);
  const changeDraft = (d: Draft) => {
    setDraft(d);
    setSaved(null);
    persist(d);
  };

  const validate = async () => {
    if (!draft) return;
    setValidating(true);
    try {
      const next = await api.generateValidate(draft);
      setDraft(next);
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
    if (putTimer.current) {
      window.clearTimeout(putTimer.current);
      putTimer.current = null;
    }
    try {
      const res = await api.generateSave(req);
      const next: Draft = { ...req.draft, saved_path: res.rel_path, saved_doc_id: res.doc_id };
      setDraft(next);
      setSaved(res);
      setSaveOpen(false);
      invalidateDrafts();
      toast(
        t("generate.save.saved", { path: res.rel_path }),
        "ok",
        res.doc_id ? { label: t("generate.save.open_library"), onClick: () => navigate(`/library/${res.doc_id}`) } : { label: t("generate.save.open_folder"), onClick: () => void revealPath(res.absolute) },
      );
      for (const w of res.warnings) if (w === "markdown_not_registered") toast(t("generate.save.warning_markdown"), "info");
    } catch (e) {
      const code = errorCode(e);
      const message = String((e as { message?: unknown }).message ?? "");
      if (code === "invalid" && message.includes("file_exists")) setSaveError(t("generate.save.exists"));
      else setSaveError(describeError(e, t));
    } finally {
      setSaving(false);
    }
  };

  const openSaved = async (d: DraftSummary) => {
    try {
      const full = await api.generateDraftGet(d.id);
      useGenerateStore.getState().openDraft(full);
      useGenerateStore.getState().consumeHandoff();
      setExcluded(full.request?.excluded_sources ?? []);
      setSaved(null);
      setDraftsOpen(false);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const deleteSaved = async (d: DraftSummary) => {
    try {
      await api.generateDraftDelete(d.id);
      invalidateDrafts();
      if (draft?.id === d.id) setDraft(null);
      toast(t("generate.drafts_list.deleted"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const reset = () => {
    useGenerateStore.getState().reset(language);
    setFeedback("");
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
      <div className="grid min-h-0 flex-1 grid-cols-[20rem_minmax(0,1fr)] xl:grid-cols-[20rem_minmax(0,1fr)_26rem]">
        <aside className="min-h-0 overflow-y-auto border-r border-border p-4">
          <RequestForm request={request} onChange={(patch) => setRequest(patch)} onGenerate={generate} job={job} running={running} hasDraft={!!draft} feedback={feedback} onFeedback={setFeedback} onRegenerate={regenerate} excludedCount={excluded.length} onCancel={cancel} />
        </aside>
        <section className="min-h-0 min-w-0" aria-label={t("generate.preview.title")}>
          <DraftPreview draft={draft} />
        </section>
        <aside className="min-h-0 overflow-y-auto border-l border-border p-4 xl:block" aria-label={t("generate.editor.title")}>
          {draft ? <DraftEditor draft={draft} onChange={changeDraft} excluded={excluded} onToggleExcluded={(id) => setExcluded((x) => toggleExcluded(x, id))} /> : <p className="text-sm text-fg-subtle">{t("generate.editor.empty")}</p>}
        </aside>
      </div>
      {draft && <SaveDialog open={saveOpen} onOpenChange={(o) => { setSaveOpen(o); if (!o) setSaveError(null); }} draft={draft} defaultOutput={request.output} busy={saving} error={saveError} onSave={(req) => void save(req)} />}
      <DraftsDrawer open={draftsOpen} onOpenChange={setDraftsOpen} onOpen={(d) => void openSaved(d)} onDelete={(d) => void deleteSaved(d)} currentId={draft?.id ?? null} />
    </div>
  );
}
