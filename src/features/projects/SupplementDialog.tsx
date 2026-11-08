import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { OUTPUT_FORMATS, type OutputFormat, type ProjectDetail } from "@/lib/types";

import { verdictVariant } from "./rules";

/** "보완요청서": pick the findings of the last check and the output format → `project_request_supplement`. */
export function SupplementDialog({ open, onOpenChange, detail, busy, onGenerate }: { open: boolean; onOpenChange: (o: boolean) => void; detail: ProjectDetail; busy: boolean; onGenerate: (findingIds: string[], output: OutputFormat) => void }) {
  const { t } = useTranslation();
  const run = detail.last_run;
  const findings = (run?.results ?? []).filter((r) => r.verdict !== "pass");
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [output, setOutput] = useState<OutputFormat>("hwpx");
  useEffect(() => {
    if (open) setPicked(new Set(findings.filter((r) => r.verdict === "mismatch" || r.verdict === "missing").map((r) => r.id)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, run?.id]);
  const toggle = (id: string) =>
    setPicked((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const none = !run || (findings.length === 0 && run.missing_doc_types.length === 0);
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
              <Button size="sm" variant="ghost" onClick={() => setPicked(new Set(findings.map((r) => r.id)))}>
                {t("projects.supplement.select_all")}
              </Button>
            </div>
            <ul className="max-h-64 divide-y divide-border overflow-y-auto rounded-md border border-border text-sm" data-testid="supplement-findings">
              {findings.map((r) => (
                <li key={r.id}>
                  <label className="flex cursor-pointer items-start gap-2 px-3 py-2 hover:bg-bg-sunken">
                    <input type="checkbox" className="mt-1" checked={picked.has(r.id)} onChange={() => toggle(r.id)} data-testid={`supplement-finding-${r.id}`} />
                    <span className="min-w-0 flex-1">
                      <span className="flex items-center gap-2">
                        <Badge variant={verdictVariant(r.verdict)}>{t(`projects.results.verdict.${r.verdict}`)}</Badge>
                        <span className="truncate font-medium">{r.description}</span>
                      </span>
                      <span className="block truncate text-xs text-fg-muted">{r.message}</span>
                    </span>
                  </label>
                </li>
              ))}
              {run.missing_doc_types.length > 0 && <li className="px-3 py-2 text-xs text-fg-muted">{t("projects.results.missing_types", { types: run.missing_doc_types.join(", ") })}</li>}
            </ul>
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
          </div>
        )}
        <DialogFooter>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button disabled={none || busy} onClick={() => onGenerate([...picked], output)} data-testid="supplement-generate">
            {busy ? t("projects.supplement.generating") : t("projects.supplement.generate")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
