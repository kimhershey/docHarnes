import { AlertTriangle, Eye, Play, Undo2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { docTypeLabel } from "@/features/pipeline/labels";
import { api } from "@/lib/ipc";
import { invalidateJournal, useDocTypes, useJournalBatches, useProjects } from "@/lib/queries";
import {
  DEFAULT_NAME_TEMPLATE,
  DEFAULT_PROJECT_FOLDER,
  FOLDER_TEMPLATES,
  TEMPLATE_VARIABLES,
  isJobFinished,
  type ApplyReport,
  type GroupBy,
  type Language,
  type OrganizeItem,
  type OrganizeItemStatus,
  type OrganizePlan,
  type OrganizeRequest,
} from "@/lib/types";
import { cn } from "@/lib/utils";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";

import { describeOrganizeError, knownCode, templateVariablesOf } from "./errors";
import { ScopeSection } from "./ScopeSection";
import { defaultScope, toScope, type ProjectRequest, type ScopeState } from "./scope";
import { EXAMPLE_DOCUMENT, renderTargetPath } from "./template";
import { TemplateEditor } from "./TemplateEditor";
import { useJob } from "./useJob";

/** Diff rows shown before "전체 보기" (design §3.7: 행 제한 + 전체 보기). */
const ROW_LIMIT = 50;
type FolderPreset = GroupBy | "custom";
const FOLDER_PRESETS: FolderPreset[] = ["month", "vendor", "doc_type", "project", "custom"];
/** `{{seq}}` is numbered inside the target folder, so the folder template cannot use it. */
const FOLDER_VARIABLES = TEMPLATE_VARIABLES.filter((v) => v !== "seq");

interface Outcome {
  moved: number;
  failed: number;
  batchId: string | null;
  items: OrganizeItem[];
  undone: boolean;
}

function statusVariant(s: OrganizeItemStatus): "ok" | "warn" | "danger" | "muted" | "info" {
  switch (s) {
    case "move":
      return "info";
    case "done":
      return "ok";
    case "held":
      return "warn";
    case "missing":
    case "failed":
      return "danger";
    default:
      return "muted";
  }
}

function asApplyReport(v: unknown): ApplyReport | null {
  if (!v || typeof v !== "object") return null;
  const r = v as Partial<ApplyReport>;
  return typeof r.moved === "number" && Array.isArray(r.items) ? { ...r, failed: Array.isArray(r.failed) ? r.failed : [], items: r.items, moved: r.moved, skipped: r.skipped ?? 0, cancelled: r.cancelled ?? false, documents: r.documents ?? [], batch_id: r.batch_id ?? null } : null;
}

/** 정리 tab: scope → templates → dry-run diff → confirmed apply as a job → undo of that batch. */
export function OrganizeTab({ selectedIds, selectionRequest, projectRequest = null }: { selectedIds: string[]; selectionRequest: number; projectRequest?: ProjectRequest | null }) {
  const { t, i18n } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const defaultOnlyReviewed = useSettingsStore((s) => s.settings?.processing.organize_reviewed_only ?? true);
  const { data: docTypes } = useDocTypes();
  const { data: projects } = useProjects();
  const [scope, setScope] = useState<ScopeState>(() => defaultScope(selectedIds.length > 0));
  /** With a project scope, the project's own folder / file-name templates fill the editors (plan §6). */
  const [useProjectSettings, setUseProjectSettings] = useState(true);
  const [nameTemplate, setNameTemplate] = useState(DEFAULT_NAME_TEMPLATE);
  const [keepName, setKeepName] = useState(false);
  const [preset, setPreset] = useState<FolderPreset>("month");
  const [folderTemplate, setFolderTemplate] = useState<string>(FOLDER_TEMPLATES.month);
  const [keepFolder, setKeepFolder] = useState(false);
  const [onlyReviewed, setOnlyReviewed] = useState(defaultOnlyReviewed);
  const reviewedTouched = useRef(false);
  const [plan, setPlan] = useState<OrganizePlan | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [templateError, setTemplateError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [jobId, setJobId] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const job = useJob(jobId);
  const handled = useRef<string | null>(null);
  // The batch may be reverted from the history tab / assistant / Ctrl+Z: follow the journal.
  const { data: batches } = useJournalBatches(100, outcome?.batchId != null);
  const batchUndone = !!outcome?.batchId && (outcome.undone || batches?.some((b) => b.id === outcome.batchId && b.undone_at !== null) === true);
  const language: Language = i18n.language.startsWith("ko") ? "ko" : "en";
  const typeLabel = useCallback((slug: string) => docTypeLabel(slug, docTypes, t, i18n.language), [docTypes, t, i18n.language]);

  // The settings load after mount; adopt the stored default until the user flips the switch.
  useEffect(() => {
    if (!reviewedTouched.current) setOnlyReviewed(defaultOnlyReviewed);
  }, [defaultOnlyReviewed]);
  // "/organize?selection=1" from the library bulk bar.
  useEffect(() => {
    if (selectionRequest > 0 && selectedIds.length > 0) setScope((s) => ({ ...s, mode: "selection" }));
  }, [selectionRequest, selectedIds.length]);
  useEffect(() => {
    if (scope.mode === "selection" && selectedIds.length === 0) setScope((s) => ({ ...s, mode: "all" }));
  }, [scope.mode, selectedIds.length]);
  // "/organize?project=<id>" from the project's settings tab.
  useEffect(() => {
    if (!projectRequest) return;
    setScope((s) => ({ ...s, mode: "project", projectId: projectRequest.id }));
    setUseProjectSettings(true);
  }, [projectRequest]);
  const scopeProject = scope.mode === "project" ? projects?.find((p) => p.id === scope.projectId) : undefined;
  const scopeProjectKey = scopeProject ? `${scopeProject.id}:${scopeProject.settings.folder_template ?? ""}:${scopeProject.settings.name_template ?? ""}` : null;
  useEffect(() => {
    if (!scopeProject || !useProjectSettings) return;
    const folder = scopeProject.settings.folder_template?.trim() || DEFAULT_PROJECT_FOLDER;
    setFolderTemplate(folder);
    const match = FOLDER_PRESETS.find((p) => p !== "custom" && FOLDER_TEMPLATES[p as GroupBy] === folder);
    setPreset(match ?? "custom");
    setNameTemplate(scopeProject.settings.name_template?.trim() || DEFAULT_NAME_TEMPLATE);
    setKeepFolder(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scopeProjectKey, useProjectSettings]);

  const effectiveName = keepName ? null : nameTemplate;
  const effectiveFolder = keepFolder ? null : folderTemplate;
  const example = useMemo(() => renderTargetPath(effectiveName, effectiveFolder, EXAMPLE_DOCUMENT, { typeLabel }), [effectiveName, effectiveFolder, typeLabel]);

  const choosePreset = (p: FolderPreset) => {
    setPreset(p);
    if (p !== "custom") setFolderTemplate(FOLDER_TEMPLATES[p]);
  };
  const editFolderTemplate = (v: string) => {
    setFolderTemplate(v);
    const match = FOLDER_PRESETS.find((p) => p !== "custom" && FOLDER_TEMPLATES[p as GroupBy] === v);
    setPreset(match ?? "custom");
  };

  const request = (): OrganizeRequest => ({ scope: toScope(scope, selectedIds), name_template: effectiveName, folder_template: effectiveFolder, only_reviewed: onlyReviewed, language });
  // The dry-run is replayed as-is by the executor, so a plan only stays valid for the inputs it was built
  // from: any change (templates, keep switches, scope, only_reviewed) discards it and its outcome.
  const reqKey = JSON.stringify(request());
  const planKey = useRef<string | null>(null);
  const running = !!job && !isJobFinished(job);
  const stale = !!plan && planKey.current !== reqKey;
  useEffect(() => {
    if (!stale || running) return;
    setPlan(null);
    setOutcome(null);
    planKey.current = null;
  }, [stale, running]);

  const preview = async () => {
    setBusy(true);
    setPlanError(null);
    setTemplateError(null);
    setOutcome(null);
    setJobId(null);
    const req = request();
    const key = JSON.stringify(req);
    try {
      const p = await api.organizePlan(req);
      planKey.current = key;
      setPlan(p);
      setShowAll(false);
    } catch (e) {
      setPlan(null);
      if (knownCode(e, ["template_variable"])) setTemplateError(t("organize.errors.template_variable", { vars: templateVariablesOf(e).join(", ") || "?" }));
      else setPlanError(describeOrganizeError(e, t));
    } finally {
      setBusy(false);
    }
  };

  const run = async () => {
    setConfirmOpen(false);
    if (!plan) return;
    try {
      const id = await api.organizeApply(plan, t("organize.batch_label", { count: plan.moves }));
      handled.current = null;
      setOutcome(null);
      setJobId(id);
      toast(t("organize.started"), "info");
    } catch (e) {
      toast(describeOrganizeError(e, t), "danger");
    }
  };

  // Follow the apply job; its result is the ApplyReport (the mock / older shells may omit it).
  useEffect(() => {
    if (!job || !isJobFinished(job) || handled.current === job.id) return;
    handled.current = job.id;
    const finish = async () => {
      let report = asApplyReport(job.result);
      if (!report) {
        try {
          report = asApplyReport((await api.jobsGet(job.id))?.result);
        } catch {
          /* ignore */
        }
      }
      if (job.state === "done") {
        const moved = report?.moved ?? plan?.moves ?? 0;
        const failed = report?.failed.length ?? 0;
        let batchId = report?.batch_id ?? null;
        if (!report) {
          try {
            batchId = (await api.journalBatches(1))[0]?.id ?? null;
          } catch {
            /* ignore */
          }
        }
        setOutcome({ moved, failed, batchId, items: report?.items ?? [], undone: false });
        toast(t("organize.done", { moved, failed }), failed > 0 ? "warn" : "ok");
      } else if (job.state === "failed") {
        toast(t("organize.failed", { message: job.error ?? "" }), "danger");
      } else {
        toast(t("organize.cancelled"), "warn");
      }
      invalidateJournal();
    };
    void finish();
  }, [job, plan, t, toast]);

  const undo = async (batchId: string) => {
    try {
      const res = await api.journalUndo(batchId);
      if (res.report) toast(t("organize.undo_done", { restored: res.report.restored, skipped: res.report.skipped.length }), "ok");
      else toast(t("organize.undo_started"), "info");
      setOutcome((o) => (o ? { ...o, undone: true } : o));
      setPlan(null);
      invalidateJournal();
    } catch (e) {
      toast(describeOrganizeError(e, t), "danger");
    }
  };

  const rows = outcome && outcome.items.length > 0 ? outcome.items : (plan?.items ?? []);
  const visibleRows = showAll ? rows : rows.slice(0, ROW_LIMIT);
  const canRun = !!plan && !stale && plan.moves > 0 && !running && !busy && (!outcome || batchUndone);
  const reasonText = (reason: string | null | undefined) => (reason ? (i18n.exists(`organize.reason.${reason}`) ? t(`organize.reason.${reason}`) : reason) : null);

  return (
    <div className="flex flex-col gap-4" data-testid="organize-tab">
      <Card>
        <CardHeader>
          <CardTitle>{t("organize.scope.title")}</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <ScopeSection value={scope} onChange={setScope} selectedCount={selectedIds.length} testId="organize-scope" />
          {scopeProject && (
            <label className="flex items-center gap-2 text-sm" data-testid="organize-project-settings-row">
              <Switch checked={useProjectSettings} onCheckedChange={setUseProjectSettings} data-testid="organize-project-settings" />
              {t("organize.scope.project_settings")}
              <span className="text-xs text-fg-subtle">{t("organize.scope.project_settings_hint")}</span>
            </label>
          )}
        </CardContent>
      </Card>

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-3">
              {t("organize.name.title")}
              <label className="ml-auto flex items-center gap-2 text-sm font-normal">
                <Switch checked={keepName} onCheckedChange={setKeepName} data-testid="organize-keep-name" />
                {t("organize.name.keep")}
              </label>
            </CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            <TemplateEditor id="organize-name-template" label={t("organize.name.title")} value={nameTemplate} onChange={(v) => { setNameTemplate(v); setTemplateError(null); }} disabled={keepName} variables={TEMPLATE_VARIABLES} error={templateError} testId="organize-name-template">
              <Button size="sm" variant="ghost" className="self-start" disabled={keepName || nameTemplate === DEFAULT_NAME_TEMPLATE} onClick={() => setNameTemplate(DEFAULT_NAME_TEMPLATE)}>
                {t("organize.name.reset")}
              </Button>
            </TemplateEditor>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-3">
              {t("organize.folder.title")}
              <label className="ml-auto flex items-center gap-2 text-sm font-normal">
                <Switch checked={keepFolder} onCheckedChange={setKeepFolder} data-testid="organize-keep-folder" />
                {t("organize.folder.keep")}
              </label>
            </CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            <div className="flex flex-col gap-1">
              <Label htmlFor="organize-folder-preset">{t("organize.folder.preset")}</Label>
              <Select value={preset} onValueChange={(v) => choosePreset(v as FolderPreset)} disabled={keepFolder}>
                <SelectTrigger id="organize-folder-preset" className="h-8 w-48" data-testid="organize-folder-preset">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {FOLDER_PRESETS.map((p) => (
                    <SelectItem key={p} value={p}>
                      {t(`organize.folder.presets.${p}`)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <TemplateEditor id="organize-folder-template" label={t("organize.folder.title")} value={folderTemplate} onChange={(v) => { editFolderTemplate(v); setTemplateError(null); }} disabled={keepFolder} variables={FOLDER_VARIABLES} testId="organize-folder-template" />
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardContent className="flex flex-col gap-3 pt-5">
          <div className="flex flex-wrap items-center gap-3 text-sm">
            <span className="text-fg-muted">{t("organize.name.example")}:</span>
            <code className="selectable rounded bg-bg-sunken px-2 py-0.5 text-xs" data-testid="organize-example">
              {example.path}
            </code>
            {example.missing.length > 0 && (
              <Badge variant="warn">
                <AlertTriangle className="size-3" aria-hidden="true" />
                {t("organize.unknown_values", { vars: example.missing.map((v) => t(`organize.variables.${v}`)).join(", ") })}
              </Badge>
            )}
          </div>
          <p className="text-xs text-fg-subtle">{t("organize.name.example_doc")}</p>
          <div className="flex flex-wrap items-center gap-3">
            <label className="flex items-center gap-2 text-sm" title={t("organize.only_reviewed_hint")}>
              <Switch
                checked={onlyReviewed}
                onCheckedChange={(v) => {
                  reviewedTouched.current = true;
                  setOnlyReviewed(v);
                }}
                data-testid="organize-only-reviewed"
              />
              {t("organize.only_reviewed")}
            </label>
            <span className="text-xs text-fg-subtle">{t("organize.only_reviewed_hint")}</span>
            <div className="ml-auto flex items-center gap-2">
              <Button variant="secondary" onClick={() => void preview()} disabled={busy || running} data-testid="organize-preview">
                {busy ? <Spinner /> : <Eye />}
                {t("organize.preview")}
              </Button>
              <Button onClick={() => setConfirmOpen(true)} disabled={!canRun} data-testid="organize-run">
                <Play />
                {t("organize.run")}
              </Button>
            </div>
          </div>
          {planError && (
            <p className="text-sm text-danger" role="alert" data-testid="organize-plan-error">
              {planError}
            </p>
          )}
        </CardContent>
      </Card>

      {running && job && (
        <Card>
          <CardContent className="flex flex-col gap-2 pt-5" data-testid="organize-progress">
            <div className="flex items-center gap-2 text-sm">
              <Spinner />
              <span>{t("organize.running")}</span>
              {job.detail && <span className="text-xs text-fg-muted">{job.detail}</span>}
            </div>
            <Progress value={job.progress} label={t("organize.running")} />
          </CardContent>
        </Card>
      )}

      {outcome && (
        <Card>
          <CardContent className="flex flex-wrap items-center gap-3 pt-5" data-testid="organize-result">
            <Badge variant={outcome.failed > 0 ? "warn" : "ok"}>{t("organize.done", { moved: outcome.moved, failed: outcome.failed })}</Badge>
            {batchUndone && <Badge variant="muted">{t("organize.history.undone")}</Badge>}
            {outcome.batchId && !batchUndone && (
              <Button size="sm" variant="secondary" onClick={() => void undo(outcome.batchId!)} data-testid="organize-undo">
                <Undo2 />
                {t("organize.undo")}
              </Button>
            )}
          </CardContent>
        </Card>
      )}

      {plan && (
        <Card>
          <CardHeader>
            <CardTitle>{t("organize.preview")}</CardTitle>
            <CardDescription data-testid="organize-summary">
              {t("organize.summary", { moves: plan.moves, held: plan.held, unchanged: plan.unchanged, needs_review: plan.needs_review })}
              {plan.missing_files > 0 && ` · ${t("organize.summary_missing_files", { count: plan.missing_files })}`}
            </CardDescription>
          </CardHeader>
          <CardContent>
            {rows.length === 0 ? (
              <p className="text-sm text-fg-muted" data-testid="organize-empty">
                {t("organize.empty_plan")}
              </p>
            ) : (
              <div className="overflow-auto rounded-md border border-border">
                <table className="w-full text-xs" data-testid="organize-preview-table">
                  <thead className="bg-bg-sunken">
                    <tr>
                      <th className="px-2 py-1 text-left font-medium">{t("organize.columns.name")}</th>
                      <th className="px-2 py-1 text-left font-medium">{t("organize.columns.from")}</th>
                      <th className="px-2 py-1 text-left font-medium">{t("organize.columns.to")}</th>
                      <th className="px-2 py-1 text-left font-medium">{t("organize.columns.status")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {visibleRows.map((item) => (
                      <tr key={item.doc_id} className={cn("border-t border-border align-top", item.status === "held" && "opacity-70")} data-testid={`organize-row-${item.doc_id}`} data-row-status={item.status}>
                        <td className="max-w-48 truncate px-2 py-1" title={item.name}>
                          {item.name}
                        </td>
                        <td className="selectable px-2 py-1 font-mono">{item.from}</td>
                        <td className="selectable px-2 py-1 font-mono">{item.to ?? "—"}</td>
                        <td className="px-2 py-1">
                          <div className="flex flex-wrap items-center gap-1">
                            <Badge variant={statusVariant(item.status)}>{t(`organize.status.${item.status}`)}</Badge>
                            {(item.status === "missing" || item.status === "failed") && reasonText(item.reason) && <span className="text-fg-muted">{reasonText(item.reason)}</span>}
                            {item.missing && item.missing.length > 0 && (
                              <Badge variant="warn" title={t("organize.unknown_values", { vars: item.missing.join(", ") })}>
                                <AlertTriangle className="size-3" aria-hidden="true" />
                                {t("organize.unknown_values", { vars: item.missing.map((v) => (i18n.exists(`organize.variables.${v}`) ? t(`organize.variables.${v}`) : v)).join(", ") })}
                              </Badge>
                            )}
                          </div>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            {rows.length > ROW_LIMIT && (
              <div className="mt-2 flex items-center gap-2 text-xs text-fg-muted">
                <span>{t("organize.rows_shown", { shown: visibleRows.length, total: rows.length })}</span>
                <Button size="sm" variant="ghost" onClick={() => setShowAll((v) => !v)} data-testid="organize-show-all">
                  {showAll ? t("organize.show_less") : t("organize.show_all", { count: rows.length })}
                </Button>
              </div>
            )}
          </CardContent>
        </Card>
      )}

      <Dialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <DialogContent className="max-w-md" closeLabel={t("common.close")} data-testid="organize-confirm-dialog">
          <DialogHeader>
            <DialogTitle>{t("organize.confirm_title")}</DialogTitle>
            <DialogDescription>{t("organize.confirm_body", { count: plan?.moves ?? 0 })}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="secondary" onClick={() => setConfirmOpen(false)}>
              {t("common.cancel")}
            </Button>
            <Button onClick={() => void run()} data-testid="organize-confirm">
              {t("organize.run")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
