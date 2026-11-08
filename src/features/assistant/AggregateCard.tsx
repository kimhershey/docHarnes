import { Sigma } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { SCREEN_ROUTES, type AggregateRequest, type AggregateResult, type PlanContext } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

import { aggregateIsAmount, docTypeName, formatAggregateValue, formatAmount } from "./describePlan";

/**
 * Aggregate result card (design §3.7.6 bottom right): date range + basis,
 * the headline value split into reviewed / unreviewed, a switch that re-runs
 * the aggregate including unreviewed documents, the per-group table and the
 * document list with links into the viewer.
 */
export function AggregateCard({
  result: initial,
  request,
  summary,
  context,
}: {
  result: AggregateResult;
  /** The request that produced `result` (re-run with `include_unreviewed` toggled). */
  request: AggregateRequest | null;
  summary: string;
  context: PlanContext | null;
}) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const [result, setResult] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [showGroups, setShowGroups] = useState(true);
  const [showDocs, setShowDocs] = useState(false);
  const amount = aggregateIsAmount(result, context);
  const fmt = (v: number) => formatAggregateValue(v, result, amount, t);
  const typeLabel = result.doc_type ? docTypeName(result.doc_type, context, i18n.language) : t("planner.aggregate.all_types");
  const fnLabel = t(`planner.fn.${result.func}`);
  const basisKey = result.basis === "imported_at" ? "imported_at" : result.basis === "mixed" ? "mixed" : "doc_date";

  const toggleUnreviewed = async (include: boolean) => {
    if (!request) return;
    setBusy(true);
    try {
      setResult(await api.aggregateRun({ ...request, include_unreviewed: include }));
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex w-full flex-col gap-2 rounded-lg border border-border bg-bg-elevated p-3 text-sm" role="group" aria-label={t("planner.aggregate.title", { summary })} data-testid="assistant-card-aggregate" data-include-unreviewed={result.include_unreviewed}>
      <div className="flex flex-wrap items-center gap-2 font-medium">
        <Sigma className="size-4 text-accent" aria-hidden="true" />
        <span>{t("planner.aggregate.title", { summary })}</span>
      </div>
      <div className="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
        {result.date_from || result.date_to ? <span>{t("planner.aggregate.range", { from: result.date_from ?? "…", to: result.date_to ?? "…" })}</span> : null}
        <span>({t(`planner.aggregate.basis.${basisKey}`)})</span>
        <span>· {t("planner.aggregate.count", { type: typeLabel, count: result.count })}</span>
        {result.missing > 0 && <span>· {t("planner.aggregate.missing", { count: result.missing })}</span>}
      </div>
      <p className="text-base font-semibold tabular-nums" data-testid="aggregate-value">
        {result.func === "count" ? fmt(result.value) : `${result.field_label ?? result.field ?? ""} ${fnLabel} ${fmt(result.value)}`}
      </p>
      {result.func !== "count" && (result.reviewed.count > 0 || result.unreviewed.count > 0) && (
        <p className="text-xs text-fg-muted tabular-nums" data-testid="aggregate-split">
          {t("planner.aggregate.split", {
            reviewed_count: result.reviewed.count,
            reviewed: fmt(result.reviewed.value),
            unreviewed_count: result.unreviewed.count,
            unreviewed: fmt(result.unreviewed.value),
          })}
        </p>
      )}
      <div className="flex flex-wrap items-center gap-3 text-xs">
        <label className="flex items-center gap-1">
          <Switch checked={result.include_unreviewed} disabled={busy || !request} onCheckedChange={(c) => void toggleUnreviewed(c)} aria-label={t("planner.aggregate.include_unreviewed")} data-testid="aggregate-include-unreviewed" />
          {t("planner.aggregate.include_unreviewed")}
        </label>
        {result.groups.length > 0 && (
          <Button size="sm" variant={showGroups ? "secondary" : "ghost"} aria-pressed={showGroups} onClick={() => setShowGroups((v) => !v)} data-testid="aggregate-toggle-groups">
            {t("planner.aggregate.groups")}
          </Button>
        )}
        <Button size="sm" variant={showDocs ? "secondary" : "ghost"} aria-pressed={showDocs} onClick={() => setShowDocs((v) => !v)} disabled={result.documents.length === 0} data-testid="aggregate-toggle-documents">
          {t("planner.aggregate.documents")} ({result.documents.length})
        </Button>
      </div>
      {showGroups && result.groups.length > 0 && (
        <table className="w-full text-xs" data-testid="aggregate-groups">
          <thead>
            <tr className="text-left text-fg-muted">
              <th className="py-1 pr-2 font-medium">{t("planner.aggregate.columns.group")}</th>
              <th className="py-1 pr-2 text-right font-medium">{t("planner.aggregate.columns.value")}</th>
              <th className="py-1 pr-2 text-right font-medium">{t("planner.aggregate.columns.count")}</th>
              <th className="py-1 pr-2 text-right font-medium">{t("planner.aggregate.columns.reviewed")}</th>
              <th className="py-1 text-right font-medium">{t("planner.aggregate.columns.unreviewed")}</th>
            </tr>
          </thead>
          <tbody className="tabular-nums">
            {result.groups.map((g) => (
              <tr key={g.key} className="border-t border-border">
                <td className="py-1 pr-2">{request?.group_by === "doc_type" ? docTypeName(g.key, context, i18n.language) : g.key}</td>
                <td className="py-1 pr-2 text-right">{fmt(g.value)}</td>
                <td className="py-1 pr-2 text-right">{g.count}</td>
                <td className="py-1 pr-2 text-right">{formatAmount(g.reviewed.value)}</td>
                <td className="py-1 text-right">{formatAmount(g.unreviewed.value)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {showDocs && result.documents.length > 0 && (
        <ul className="flex max-h-64 flex-col gap-1 overflow-y-auto text-xs" data-testid="aggregate-documents">
          {result.documents.map((d) => (
            <li key={d.id} className="flex flex-wrap items-center gap-2 border-t border-border py-1">
              <button type="button" className="truncate font-medium text-accent hover:underline cursor-pointer" onClick={() => navigate(`${SCREEN_ROUTES.library}/${d.id}`)} title={t("planner.aggregate.open_doc")}>
                {d.name}
              </button>
              <span className="text-fg-muted">{d.date ?? "—"}</span>
              <span className="tabular-nums">{d.value == null ? "—" : fmt(d.value)}</span>
              <Badge variant={d.reviewed ? "ok" : "warn"}>{d.reviewed ? t("pipeline.review_status.done") : t("pipeline.review_status.pending")}</Badge>
            </li>
          ))}
        </ul>
      )}
      {result.field_label && <p className="text-xs text-fg-subtle">{t("planner.aggregate.evidence", { field: result.field_label })}</p>}
    </div>
  );
}
