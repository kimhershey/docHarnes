import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { docTypeLabel } from "@/features/pipeline/labels";
import { useDocTypes } from "@/lib/queries";
import { OUTPUT_FORMATS, type OutputFormat, type ProjectDetail } from "@/lib/types";
import { cn } from "@/lib/utils";

import { verdictVariant } from "./rules";
import { defaultSupplementSelection, selectableSupplementIds, supplementRows } from "./supplement";

/**
 * "보완요청서": pick the findings of the last check and the output format → `project_request_supplement`.
 *
 * The rows are everything the request can ask for (PRJ-2): the required documents the project lacks — each a
 * finding of its own (`missing_doc:<type>`), ticked like the rule findings; they used to be a note under the list
 * and dropped out of the request as soon as any rule finding was ticked — and the rule results that did not pass.
 * A 판단불가 result cannot become a request item (the builder takes 불일치 / 누락 only), so its row is shown but
 * cannot be ticked; with nothing ticked the request is not sent (an empty selection would mean "everything").
 */
export function SupplementDialog({ open, onOpenChange, detail, busy, onGenerate }: { open: boolean; onOpenChange: (o: boolean) => void; detail: ProjectDetail; busy: boolean; onGenerate: (findingIds: string[], output: OutputFormat) => void }) {
  const { t, i18n } = useTranslation();
  const { data: docTypes } = useDocTypes();
  const run = detail.last_run;
  const rows = useMemo(() => supplementRows(run), [run]);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [output, setOutput] = useState<OutputFormat>("hwpx");
  useEffect(() => {
    if (open) setPicked(new Set(defaultSupplementSelection(rows)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, run?.id]);
  const toggle = (id: string) =>
    setPicked((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const selectable = selectableSupplementIds(rows);
  const none = !run || rows.length === 0;
  const hasUndetermined = rows.some((r) => !r.selectable);
  const ko = i18n.language.startsWith("ko");
  /** The checklist's label of a document type (the template's wording), else the type's own label. */
  const typeLabel = (docType: string) => {
    const item = detail.checklist.find((c) => c.doc_type === docType);
    return item ? (ko ? item.label_ko : item.label_en) : docTypeLabel(docType, docTypes, t, i18n.language);
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl" closeLabel={t("common.close")} data-testid="supplement-dialog">
        <DialogHeader>
          <DialogTitle>{t("projects.supplement.title")}</DialogTitle>
          <DialogDescription>{t("projects.supplement.hint")}</DialogDescription>
        </DialogHeader>
        {none ? (
          <p className="text-sm text-fg-muted" data-testid="supplement-none">
            {t("projects.supplement.none")}
          </p>
        ) : (
          <div className="flex flex-col gap-3">
            <div className="flex items-center justify-between">
              <Label>{t("projects.supplement.findings")}</Label>
              <Button size="sm" variant="ghost" onClick={() => setPicked(new Set(selectable))} disabled={selectable.length === 0} data-testid="supplement-select-all">
                {t("projects.supplement.select_all")}
              </Button>
            </div>
            <ul className="max-h-64 divide-y divide-border overflow-y-auto rounded-md border border-border text-sm" data-testid="supplement-findings">
              {rows.map((row) => {
                const title = row.docType ? t("projects.supplement.missing_doc", { type: typeLabel(row.docType) }) : (row.result?.description ?? "");
                const message = row.docType ? t("projects.supplement.missing_doc_message", { type: typeLabel(row.docType) }) : (row.result?.message ?? "");
                return (
                  <li key={row.id} data-testid="supplement-row" data-kind={row.docType ? "missing_doc" : "result"} data-verdict={row.verdict} data-selectable={row.selectable}>
                    <label className={cn("flex items-start gap-2 px-3 py-2", row.selectable ? "cursor-pointer hover:bg-bg-sunken" : "cursor-not-allowed opacity-70")} title={row.selectable ? undefined : t("projects.supplement.undetermined_hint")}>
                      <input type="checkbox" className="mt-1" checked={picked.has(row.id)} disabled={!row.selectable} onChange={() => toggle(row.id)} data-testid={`supplement-finding-${row.id}`} />
                      <span className="min-w-0 flex-1">
                        <span className="flex items-center gap-2">
                          <Badge variant={verdictVariant(row.verdict)}>{t(`projects.results.verdict.${row.verdict}`)}</Badge>
                          <span className="truncate font-medium">{title}</span>
                        </span>
                        <span className="block truncate text-xs text-fg-muted">{message}</span>
                        {row.covered && <span className="block text-xs text-fg-subtle">{t("projects.supplement.covered")}</span>}
                      </span>
                    </label>
                  </li>
                );
              })}
            </ul>
            {hasUndetermined && (
              <p className="text-xs text-fg-muted" data-testid="supplement-undetermined-hint">
                {t("projects.supplement.undetermined_hint")}
              </p>
            )}
            <div className="flex items-center gap-2">
              <Label htmlFor="supplement-output">{t("projects.supplement.output")}</Label>
              <Select value={output} onValueChange={(v) => setOutput(v as OutputFormat)}>
                <SelectTrigger id="supplement-output" className="h-8 w-40" aria-label={t("projects.supplement.output")} data-testid="supplement-output">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {OUTPUT_FORMATS.filter((f) => f !== "md").map((f) => (
                    <SelectItem key={f} value={f}>
                      {t(`generate.output.${f}`)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            {picked.size === 0 && (
              <p className="text-xs text-warn" role="status" data-testid="supplement-pick-one">
                {t("projects.supplement.pick_one")}
              </p>
            )}
          </div>
        )}
        <DialogFooter>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button disabled={none || busy || picked.size === 0} onClick={() => onGenerate([...picked], output)} data-testid="supplement-generate">
            {busy ? t("projects.supplement.generating") : t("projects.supplement.generate")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
