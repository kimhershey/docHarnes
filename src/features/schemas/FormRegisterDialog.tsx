import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { Spinner } from "@/components/ui/spinner";
import { describeJobDetail, describeJobError } from "@/features/jobs/describe";
import { DropHint, importDroppedFiles, useDocumentDropTarget } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useDocuments } from "@/lib/queries";
import { isJobFinished, isSchemaDiscoveryOutcome, type SchemaDiscoveryOutcome } from "@/lib/types";
import { useJob, useJobsStore } from "@/stores/jobs";
import { useUiStore } from "@/stores/ui";

/**
 * "양식 등록" step 1 (design §3.4.0, 0.3.0 §2): pick 1..N sample documents + a name → a
 * `schema_discovery` job finds the fields of every sample and merges them; the draft then opens
 * in the editor unsaved for confirmation (step 2). The dialog follows the job (progress, cancel);
 * closing it leaves the job running — its result stays reachable from the 작업 screen.
 */
export function FormRegisterDialog({
  open,
  onOpenChange,
  documentIds,
  initialName,
  onDraft,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  documentIds?: string[] | null;
  initialName?: string | null;
  onDraft: (outcome: SchemaDiscoveryOutcome) => void;
}) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: docs } = useDocuments({ limit: 500, sort: "imported_desc" }, open);
  const [selected, setSelected] = useState<string[]>([]);
  const [name, setName] = useState("");
  const [jobId, setJobId] = useState<string | null>(null);
  const job = useJob(jobId);
  /** Job whose terminal state was already handled (the store keeps emitting the finished job). */
  const handled = useRef<string | null>(null);
  const busy = jobId !== null;

  const add = useCallback((ids: string[]) => setSelected((cur) => [...cur, ...ids.filter((id) => !cur.includes(id))]), []);
  const toggle = (id: string) => setSelected((cur) => (cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id]));
  // Documents dropped on the dialog join the samples; files are imported first.
  const drop = useDocumentDropTarget({
    disabled: !open || busy,
    onDocuments: (dropped) => add(dropped.map((d) => d.id)),
    onFiles: (paths) => void importDroppedFiles(paths, {}, t).then((ids) => add(ids)),
  });
  useEffect(() => {
    if (open) {
      setSelected(documentIds ?? []);
      setName(initialName ?? "");
      setJobId(null);
      handled.current = null;
    }
  }, [open, documentIds, initialName]);

  // Follow the job: its result (fetched with jobs_get — the broadcast omits it) opens the editor.
  useEffect(() => {
    if (!open || !job || !isJobFinished(job) || handled.current === job.id) return;
    handled.current = job.id;
    void (async () => {
      let result: unknown = job.result;
      if (!isSchemaDiscoveryOutcome(result)) {
        const full = await useJobsStore.getState().fetch(job.id);
        result = full?.result;
      }
      setJobId(null);
      if (job.state === "done" && isSchemaDiscoveryOutcome(result)) {
        onDraft(result);
        onOpenChange(false);
        return;
      }
      if (job.state === "done") toast(t("schemas.form.job_result_missing"), "warn");
      else if (job.state === "cancelled") toast(t("schemas.form.job_cancelled"), "warn");
      else toast(t("schemas.form.job_failed", { message: describeJobError(job, t) ?? "" }), "danger");
    })();
  }, [open, job, onDraft, onOpenChange, toast, t]);

  const close = () => onOpenChange(false);
  const cancelJob = async () => {
    if (!jobId) return;
    try {
      await api.jobsCancel(jobId);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const derive = async () => {
    if (selected.length === 0) return;
    try {
      handled.current = null;
      const id = await api.pipelineSchemaDiscover(selected, name.trim() || null);
      setJobId(id);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const selectedDocs = useMemo(
    () => selected.map((id) => docs?.find((d) => d.id === id)).filter((d): d is NonNullable<typeof d> => !!d),
    [selected, docs],
  );
  const detail = job ? describeJobDetail(job, t) : null;

  return (
    <Dialog open={open} onOpenChange={(o) => (o ? onOpenChange(true) : close())}>
      <DialogContent closeLabel={t("common.close")} data-testid="form-register-dialog" {...drop.props}>
        <DropHint active={drop.isActive} over={drop.isOver} testId="form-register-drop-hint">
          {t("schemas.form.drop_hint")}
        </DropHint>
        <DialogHeader>
          <DialogTitle>{t("schemas.form.title")}</DialogTitle>
          <DialogDescription>{t("schemas.form.hint")}</DialogDescription>
        </DialogHeader>
        <div className="flex min-w-0 flex-col gap-3">
          <div className="flex min-w-0 flex-col gap-1">
            <Label>{t("schemas.form.document")}</Label>
            <p className="text-xs text-fg-subtle">{t("schemas.form.documents_hint")}</p>
            {docs && docs.length === 0 ? (
              <p className="text-sm text-fg-muted">{t("schemas.form.no_docs")}</p>
            ) : (
              <div className="max-h-56 overflow-y-auto rounded-md border border-border" data-testid="form-register-docs">
                {(docs ?? []).map((d) => (
                  <label key={d.id} className="flex min-w-0 cursor-pointer items-center gap-2 px-2 py-1 text-sm hover:bg-bg-sunken">
                    <input type="checkbox" className="size-4 shrink-0" checked={selected.includes(d.id)} onChange={() => toggle(d.id)} disabled={busy} data-testid={`form-register-doc-${d.id}`} />
                    <span className="min-w-0 flex-1 truncate" title={d.original_name}>
                      {d.title ?? d.original_name}
                    </span>
                    <span className="ml-auto shrink-0 text-xs uppercase text-fg-subtle">{d.source_kind}</span>
                  </label>
                ))}
              </div>
            )}
            <p className="text-xs text-fg-muted" data-testid="form-register-doc">
              {selectedDocs.length > 0 ? `${t("schemas.form.selected_count", { count: selectedDocs.length })}: ${selectedDocs.map((d) => d.title ?? d.original_name).join(", ")}` : t("schemas.form.none_selected")}
            </p>
          </div>
          <div>
            <Label htmlFor="form-name">{t("schemas.form.name")}</Label>
            <Input id="form-name" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("schemas.form.name_hint")} disabled={busy} data-testid="form-register-name" />
          </div>
          {busy && (
            <div className="flex flex-col gap-1.5 rounded-md border border-border bg-bg-sunken p-2 text-sm" role="status" data-testid="form-register-pending">
              <span className="flex items-center gap-2">
                <Spinner />
                {t("schemas.form.deriving")}
              </span>
              <Progress value={job?.progress ?? null} label={t("schemas.form.deriving")} />
              <span className="text-xs text-fg-subtle" data-testid="form-register-pending-hint">
                {job?.state === "queued" ? t("schemas.form.waiting_hint") : (detail ?? t("schemas.pending_hint"))}
              </span>
            </div>
          )}
        </div>
        <DialogFooter className="items-center">
          {busy && (
            <Button variant="ghost" className="mr-auto" onClick={() => void cancelJob()} data-testid="form-register-cancel-job">
              {t("schemas.form.cancel_job")}
            </Button>
          )}
          <Button variant="secondary" onClick={close} data-testid="form-register-cancel">
            {t("common.close")}
          </Button>
          <Button onClick={() => void derive()} disabled={busy || selected.length === 0} data-testid="form-register-derive">
            {t("schemas.form.derive")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
