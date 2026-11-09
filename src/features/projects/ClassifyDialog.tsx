import { Sparkles } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { docTypeLabel } from "@/features/pipeline/labels";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects, useDocTypes } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import type { ClassifyResult, ClassifySuggestion, Project } from "@/lib/types";
import { useEngineStore } from "@/stores/engine";
import { useUiStore } from "@/stores/ui";

const NONE = "__none__";

/**
 * 자동 분류 (design §3.15 addendum, 2026-11-09): suggests a project for library / knowledge-base
 * documents from their title, file name, vendor and text (deterministic scores, optionally the
 * loaded model for the undecided ones). The user reviews the preview — every row's project can be
 * changed or left out — and applies it; nothing is assigned before that.
 */
export function ClassifyDialog({ open, onOpenChange, docIds, projects }: { open: boolean; onOpenChange: (o: boolean) => void; docIds: string[]; projects: Project[] }) {
  const { t, i18n } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: docTypes } = useDocTypes(open);
  const modelLoaded = useEngineStore((s) => (s.status?.models ?? []).some((m) => m.status === "loaded" || m.status === "sleeping"));
  const [useModel, setUseModel] = useState(false);
  const [busy, setBusy] = useState<"preview" | "apply" | null>(null);
  const [result, setResult] = useState<ClassifyResult | null>(null);
  // Per-document choice: the suggestion to start with, editable (NONE = leave the document out).
  const [choice, setChoice] = useState<Record<string, string>>({});
  useEffect(() => {
    if (!open) return;
    setResult(null);
    setChoice({});
    setUseModel(false);
  }, [open, docIds]);
  const run = async () => {
    setBusy("preview");
    try {
      const r = await api.projectClassifyPreview({ doc_ids: docIds, use_model: useModel && modelLoaded });
      setResult(r);
      const next: Record<string, string> = {};
      for (const s of r.suggestions) next[s.doc_id] = s.project_id ?? NONE;
      setChoice(next);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(null);
    }
  };
  const assignments = useMemo(
    () =>
      (result?.suggestions ?? [])
        .map((s) => ({ doc_id: s.doc_id, project_id: choice[s.doc_id] ?? NONE }))
        .filter((a) => a.project_id !== NONE),
    [result, choice],
  );
  const apply = async () => {
    if (assignments.length === 0) return;
    setBusy("apply");
    try {
      const r = await api.projectClassifyApply(assignments);
      invalidateProjects(r.project_ids);
      void queryClient.invalidateQueries({ queryKey: queryKeys.documents });
      toast(t("projects.classify.applied", { count: r.added, projects: r.project_ids.length }), "ok");
      onOpenChange(false);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(null);
    }
  };
  const pct = (v: number) => `${Math.round(v * 100)}%`;
  const confidenceVariant = (s: ClassifySuggestion): "ok" | "warn" | "muted" => (s.project_id === null ? "muted" : s.confidence >= 0.65 ? "ok" : "warn");
  const scope = docIds.length > 0 ? t("projects.classify.scope_selected", { count: docIds.length }) : t("projects.classify.scope_unassigned");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-4xl" closeLabel={t("common.close")} data-testid="classify-dialog">
        <DialogHeader>
          <DialogTitle>{t("projects.classify.title")}</DialogTitle>
          <DialogDescription>{t("projects.classify.hint")}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-wrap items-center gap-3 text-sm">
          <Badge variant="outline" data-testid="classify-scope">
            {scope}
          </Badge>
          <label className="flex items-center gap-2 text-xs text-fg-muted" title={modelLoaded ? t("projects.classify.model_hint") : t("projects.classify.model_unavailable")}>
            <Switch checked={useModel && modelLoaded} onCheckedChange={setUseModel} disabled={!modelLoaded} aria-label={t("projects.classify.use_model")} data-testid="classify-use-model" />
            {t("projects.classify.use_model")}
          </label>
          <Button size="sm" className="ml-auto" onClick={() => void run()} disabled={busy !== null || projects.length === 0} data-testid="classify-run">
            {busy === "preview" ? <Spinner className="size-4" /> : <Sparkles />}
            {result ? t("projects.classify.rerun") : t("projects.classify.run")}
          </Button>
        </div>
        {projects.length === 0 && <p className="text-sm text-fg-muted">{t("projects.classify.no_projects")}</p>}
        {result && (
          <>
            <p className="text-xs text-fg-muted" data-testid="classify-summary">
              {t("projects.classify.summary", { total: result.suggestions.length, suggested: result.suggested, undecided: result.undecided })}
              {result.model_calls > 0 && ` · ${t("projects.classify.model_calls", { count: result.model_calls })}`}
            </p>
            {result.notes.map((n) => (
              <p key={n} className="text-xs text-warn">
                {n}
              </p>
            ))}
            <div className="max-h-[50vh] overflow-auto rounded-md border border-border">
              {result.suggestions.length === 0 ? (
                <p className="p-3 text-sm text-fg-muted">{t("projects.classify.empty")}</p>
              ) : (
                <table className="w-full text-sm" data-testid="classify-table">
                  <thead className="sticky top-0 bg-bg-elevated text-left text-xs uppercase tracking-wide text-fg-subtle">
                    <tr>
                      <th className="px-3 py-2">{t("projects.classify.columns.document")}</th>
                      <th className="px-3 py-2">{t("projects.classify.columns.project")}</th>
                      <th className="px-3 py-2">{t("projects.classify.columns.confidence")}</th>
                      <th className="px-3 py-2">{t("projects.classify.columns.reasons")}</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-border">
                    {result.suggestions.map((s) => (
                      <tr key={s.doc_id} data-testid={`classify-row-${s.doc_id}`} data-suggested={s.project_id ?? ""} data-source={s.source}>
                        <td className="px-3 py-2 align-top">
                          <div className="font-medium">{s.title}</div>
                          <div className="text-xs text-fg-subtle">
                            {s.doc_type ? docTypeLabel(s.doc_type, docTypes, t, i18n.language) : t("projects.classify.untyped")}
                            {s.already_in.length > 0 && ` · ${t("projects.classify.already_in", { names: s.already_in.join(", ") })}`}
                          </div>
                        </td>
                        <td className="px-3 py-2 align-top">
                          <Select value={choice[s.doc_id] ?? NONE} onValueChange={(v) => setChoice((c) => ({ ...c, [s.doc_id]: v }))}>
                            <SelectTrigger className="h-8 w-56" aria-label={t("projects.classify.columns.project")} data-testid={`classify-project-${s.doc_id}`}>
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectItem value={NONE}>{t("projects.classify.leave_out")}</SelectItem>
                              {projects.map((p) => (
                                <SelectItem key={p.id} value={p.id} data-testid={`classify-project-${s.doc_id}-${p.id}`}>
                                  {p.name}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                        </td>
                        <td className="px-3 py-2 align-top">
                          <Badge variant={confidenceVariant(s)} data-testid={`classify-confidence-${s.doc_id}`}>
                            {s.project_id === null ? t("projects.classify.undecided") : pct(s.confidence)}
                          </Badge>
                          {s.source === "model" && <div className="mt-1 text-[0.6875rem] text-fg-subtle">{t("projects.classify.by_model")}</div>}
                        </td>
                        <td className="px-3 py-2 align-top text-xs text-fg-muted">
                          {s.reasons.join(" · ")}
                          {s.alternatives.length > 0 && (
                            <div className="mt-1 text-fg-subtle">
                              {t("projects.classify.alternatives", { names: s.alternatives.map((a) => `${a.project_name} (${a.score.toFixed(1)})`).join(", ") })}
                            </div>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          </>
        )}
        <DialogFooter>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button onClick={() => void apply()} disabled={busy !== null || assignments.length === 0} data-testid="classify-apply">
            {busy === "apply" ? <Spinner className="size-4" /> : null}
            {t("projects.classify.apply", { count: assignments.length })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
