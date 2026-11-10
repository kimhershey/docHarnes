import { ExternalLink, FileText, Play } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useProjectRuns } from "@/lib/queries";
import type { CheckRun, Evidence, ProjectDetail } from "@/lib/types";
import { formatDate } from "@/lib/utils";

import { RelationsSection } from "./RelationsTab";
import { ReportSection } from "./ReportTab";
import { verdictVariant } from "./rules";

function EvidenceLink({ e, onOpen, t }: { e: Evidence | null; onOpen: (e: Evidence) => void; t: (k: string, o?: Record<string, unknown>) => string }) {
  if (!e || !e.doc_name) return <span className="text-fg-subtle">{e?.value ?? "—"}</span>;
  return (
    <button type="button" className="inline-flex max-w-[16rem] items-center gap-1 truncate text-accent hover:underline cursor-pointer" onClick={() => onOpen(e)} title={e.value ?? undefined} aria-label={t("projects.results.evidence_open", { name: e.doc_name })} data-testid="result-evidence">
      <ExternalLink className="size-3 shrink-0" aria-hidden="true" />
      <span className="truncate">
        {e.doc_name}
        {e.page ? ` p.${e.page}` : ""}
        {e.label ? ` · ${e.label}` : ""}
        {e.value ? ` = ${e.value}` : ""}
      </span>
    </button>
  );
}

/**
 * 결과 tab (0.5.0 plan §3: one tab instead of 결과·관계·리포트): run the check, the verdict chips, the results
 * table with evidence links and the run history on top, the relation graph below and the report behind a button.
 */
export function ResultsTab({ detail, onCheck, checking }: { detail: ProjectDetail; onCheck: () => void; checking: boolean }) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const { data: runs } = useProjectRuns(detail.project.id);
  const [selected, setSelected] = useState<string | null>(null);
  const [reportOpen, setReportOpen] = useState(false);
  // A new check run replaces a history selection (otherwise the tab keeps showing the older run).
  const lastRunId = detail.last_run?.id ?? null;
  useEffect(() => setSelected(null), [lastRunId]);
  const run: CheckRun | null = (selected ? runs?.find((r) => r.id === selected) : null) ?? detail.last_run;
  const open = (e: Evidence) => {
    if (!e.doc_id) return;
    navigate(`/library/${e.doc_id}${e.page ? `?page=${e.page}` : ""}`);
  };
  return (
    <div className="flex flex-col gap-5" data-testid="project-results">
      <section className="flex flex-col gap-3" aria-labelledby="results-check-title">
      <h3 id="results-check-title" className="sr-only">
        {t("projects.results.run")}
      </h3>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" onClick={onCheck} disabled={checking} data-testid="project-check">
          <Play />
          {checking ? t("projects.results.running") : t("projects.results.run")}
        </Button>
        {runs && runs.length > 0 && (
          <Select value={run?.id ?? ""} onValueChange={setSelected}>
            <SelectTrigger className="h-8 w-72" aria-label={t("projects.results.history")} data-testid="project-run-select">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {runs.map((r) => (
                <SelectItem key={r.id} value={r.id}>
                  {formatDate(r.finished_at, i18n.language)} · {t("projects.results.summary", { pass: r.pass, mismatch: r.mismatch, undetermined: r.undetermined, missing: r.missing })}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </div>
      {!run ? (
        <p className="text-sm text-fg-muted" data-testid="project-results-none">
          {t("projects.results.none")}
        </p>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-2" aria-live="polite" data-testid="project-verdicts">
            <Badge variant="ok" data-testid="verdict-pass">
              {t("projects.results.verdict.pass")} {run.pass}
            </Badge>
            <Badge variant="danger" data-testid="verdict-mismatch">
              {t("projects.results.verdict.mismatch")} {run.mismatch}
            </Badge>
            <Badge variant="warn" data-testid="verdict-undetermined">
              {t("projects.results.verdict.undetermined")} {run.undetermined}
            </Badge>
            <Badge variant="muted" data-testid="verdict-missing">
              {t("projects.results.verdict.missing")} {run.missing}
            </Badge>
            <span className="text-xs text-fg-subtle">{formatDate(run.finished_at, i18n.language)}</span>
          </div>
          {run.missing_doc_types.length > 0 && (
            <p className="text-xs text-danger" data-testid="project-missing-types">
              {t("projects.results.missing_types", { types: run.missing_doc_types.join(", ") })}
            </p>
          )}
          <table className="w-full text-sm" data-testid="project-results-table">
            <thead className="text-left text-xs uppercase tracking-wide text-fg-subtle">
              <tr>
                <th className="py-2 pr-3">{t("projects.results.columns.rule")}</th>
                <th className="py-2 pr-3">{t("projects.results.columns.verdict")}</th>
                <th className="py-2 pr-3">{t("projects.results.columns.message")}</th>
                <th className="py-2">{t("projects.results.columns.evidence")}</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {run.results.map((r) => (
                <tr key={r.id} className="align-top" data-testid={`result-${r.rule_id}`} data-verdict={r.verdict}>
                  <td className="py-2 pr-3">
                    <div className="font-medium">{r.description}</div>
                    {r.required && <span className="text-[0.6875rem] text-fg-subtle">{t("projects.results.required")}</span>}
                  </td>
                  <td className="py-2 pr-3">
                    <Badge variant={verdictVariant(r.verdict)}>{t(`projects.results.verdict.${r.verdict}`)}</Badge>
                  </td>
                  <td className="py-2 pr-3 text-fg-muted selectable">{r.message}</td>
                  <td className="py-2">
                    <div className="flex flex-col gap-0.5 text-xs">
                      <EvidenceLink e={r.left} onOpen={open} t={t} />
                      {r.right && r.right.doc_name && <EvidenceLink e={r.right} onOpen={open} t={t} />}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
      </section>
      <section className="flex flex-col gap-2" aria-labelledby="results-relations-title">
        <h3 id="results-relations-title" className="text-sm font-semibold">
          {t("projects.results.relations_title")}
        </h3>
        <RelationsSection graph={detail.relations} />
      </section>
      <section className="flex flex-col gap-2" aria-labelledby="results-report-title">
        <div className="flex flex-wrap items-center gap-2">
          <h3 id="results-report-title" className="text-sm font-semibold">
            {t("projects.results.report_title")}
          </h3>
          <Button size="sm" variant="outline" onClick={() => setReportOpen((v) => !v)} aria-expanded={reportOpen} data-testid="project-report-toggle">
            <FileText />
            {reportOpen ? t("projects.results.report_hide") : t("projects.results.report_show")}
          </Button>
        </div>
        {reportOpen && <ReportSection projectId={detail.project.id} active={reportOpen} />}
      </section>
    </div>
  );
}
