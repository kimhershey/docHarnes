import { Check, CircleSlash, FolderOpen, ListChecks, MinusCircle, Undo2, X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { AnswerCard } from "@/features/knowledge/AnswerCard";
import { openCitation } from "@/features/knowledge/AnswersPanel";
import { hitHref } from "@/features/search/SearchPage";
import { runJournal } from "@/lib/commands";
import { revealPath } from "@/lib/ipc";
import { SCREEN_ROUTES, type OrganizeItem, type Plan, type PlanExecution, type PlanResponse, type StepStatus, type UndoReport } from "@/lib/types";
import { cn } from "@/lib/utils";
import { usePipelineStore } from "@/stores/pipeline";

import { AggregateCard } from "./AggregateCard";
import { aggregateRequestFor, deriveResultActions, opLabel } from "./describePlan";

function StatusIcon({ status }: { status: StepStatus }) {
  switch (status) {
    case "done":
    case "ui":
      return <Check className="size-4 text-ok" aria-hidden="true" />;
    case "failed":
      return <X className="size-4 text-danger" aria-hidden="true" />;
    case "cancelled":
      return <CircleSlash className="size-4 text-fg-muted" aria-hidden="true" />;
    default:
      return <MinusCircle className="size-4 text-fg-subtle" aria-hidden="true" />;
  }
}

function MovesTable({ items }: { items: OrganizeItem[] }) {
  const { t } = useTranslation();
  return (
    <table className="w-full text-xs" data-testid="result-moves">
      <caption className="sr-only">{t("planner.result.moves_caption")}</caption>
      <thead className="sr-only">
        <tr>
          <th scope="col">{t("planner.card.columns.from")}</th>
          <th scope="col" aria-hidden="true" />
          <th scope="col">{t("planner.card.columns.to")}</th>
          <th scope="col">{t("planner.card.columns.status")}</th>
        </tr>
      </thead>
      <tbody>
        {items.map((item, i) => (
          <tr key={`${item.doc_id}-${i}`} className="border-t border-border align-top" data-item-status={item.status}>
            <td className="truncate py-0.5 pr-2 font-mono text-[11px] selectable" title={item.from}>
              {item.from}
            </td>
            <td className="py-0.5 pr-2 text-fg-subtle">→</td>
            <td className="truncate py-0.5 pr-2 font-mono text-[11px] selectable" title={item.to ?? ""}>
              {item.to ?? "—"}
            </td>
            <td className="py-0.5">
              <Badge variant={item.status === "done" || item.status === "move" ? "ok" : item.status === "failed" || item.status === "missing" ? "danger" : "warn"}>{t(`planner.result.item_status.${item.status}`)}</Badge>
              {item.reason && item.status !== "done" && <span className="ml-1 text-fg-subtle">{item.reason}</span>}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/**
 * Result card (design §3.7.6): ✔/✖ per step with the executor's summaries,
 * then the actions the payloads allow — review queue, move log, undo, export
 * folder, search hits, answers / summaries and aggregate results.
 */
export function ResultCard({ execution, plan, response, error }: { execution: PlanExecution; plan: Plan; response: PlanResponse | null; error?: string | null }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [showMoves, setShowMoves] = useState(false);
  const [undone, setUndone] = useState<UndoReport | null>(null);
  const [undoing, setUndoing] = useState(false);
  const actions = deriveResultActions(execution, plan);
  const outcome = execution.cancelled ? "cancelled" : actions.failed || error ? "failed" : "done";
  const undoBatch = actions.batches[actions.batches.length - 1] ?? null;
  const moveItems = actions.organise.flatMap((o) => o.items);
  const context = response?.context ?? null;

  const undo = async () => {
    if (!undoBatch) return;
    setUndoing(true);
    try {
      const report = await runJournal(t, { batchId: undoBatch });
      if (report) setUndone(report);
    } finally {
      setUndoing(false);
    }
  };

  const openReview = () => {
    if (actions.reviewDocuments.length === 1) usePipelineStore.getState().openReviewForDocument(actions.reviewDocuments[0]!);
    navigate(SCREEN_ROUTES.review);
  };

  return (
    <div className="flex w-full flex-col gap-2 rounded-lg border border-border bg-bg-elevated p-3 text-sm" role="group" aria-label={t("planner.result.title")} aria-live="polite" data-testid="assistant-card-result" data-result-status={outcome}>
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant={outcome === "done" ? "ok" : outcome === "failed" ? "danger" : "muted"}>{t(`planner.result.${outcome}`)}</Badge>
        <span className="text-xs text-fg-subtle">{t("planner.result.title")}</span>
      </div>
      <ol className="flex flex-col gap-1" aria-label={t("planner.card.steps")}>
        {execution.steps.map((step) => (
          <li key={step.index} className="flex items-start gap-2" data-testid={`result-step-${step.index}`} data-step-status={step.status}>
            <span className="mt-0.5 shrink-0">
              <StatusIcon status={step.status} />
            </span>
            <div className="flex min-w-0 flex-1 flex-col">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-medium">{opLabel(step.op, t)}</span>
                {step.summary && <span className="text-fg-muted selectable">{step.summary}</span>}
                {(step.status === "skipped" || step.status === "cancelled" || step.status === "ui") && <span className="text-xs text-fg-subtle">{t(`planner.result.status.${step.status}`)}</span>}
              </div>
              {step.error && (
                <span className="text-xs text-danger selectable" role="alert">
                  {t("planner.result.step_failed", { error: step.error })}
                </span>
              )}
            </div>
          </li>
        ))}
      </ol>
      {error && !actions.failed && (
        <p className="text-xs text-danger selectable" role="alert">
          {error}
        </p>
      )}
      {(actions.reviewPending > 0 || moveItems.length > 0 || undoBatch || actions.exports.length > 0) && (
        <div className="flex flex-wrap items-center gap-2">
          {actions.reviewPending > 0 && (
            <Button size="sm" variant="secondary" onClick={openReview} data-testid="result-open-review">
              <ListChecks />
              {t("planner.result.open_review", { count: actions.reviewPending })}
            </Button>
          )}
          {moveItems.length > 0 && (
            <Button size="sm" variant="outline" aria-pressed={showMoves} onClick={() => setShowMoves((v) => !v)} data-testid="result-toggle-moves">
              {t("planner.result.moves")} ({moveItems.length})
            </Button>
          )}
          {undoBatch && (
            <Button size="sm" variant="outline" disabled={undoing || undone !== null} onClick={() => void undo()} data-testid="result-undo">
              <Undo2 />
              {undone ? t("planner.result.undone", { restored: undone.restored, skipped: undone.skipped.length }) : actions.batches.length > 1 ? t("planner.result.undo_last_batch") : t("planner.result.undo")}
            </Button>
          )}
          {actions.exports.map((e) => (
            <span key={e.index} className="flex flex-wrap items-center gap-1 text-xs" data-testid="result-export">
              <span className="font-mono selectable" data-testid="result-export-path">
                {e.result.path}
              </span>
              {e.result.overwritten && <span className="text-warn">{t("planner.card.overwrite")}</span>}
              <Button size="sm" variant="ghost" onClick={() => void revealPath(e.result.absolute)} data-testid="result-open-folder">
                <FolderOpen />
                {t("common.open_folder")}
              </Button>
            </span>
          ))}
        </div>
      )}
      {showMoves && moveItems.length > 0 && <MovesTable items={moveItems} />}
      {actions.searches.map((s) => (
        <div key={s.index} className="flex flex-col gap-1" data-testid="result-search">
          <span className="text-xs text-fg-muted">{t("planner.result.hits", { count: s.response.hits.length })}</span>
          <div className="flex flex-wrap gap-1">
            {s.response.hits.map((h) => (
              <button
                key={h.chunk_id}
                type="button"
                className={cn("max-w-full truncate rounded-full bg-accent-soft px-2 py-0.5 text-xs text-accent hover:bg-accent hover:text-accent-fg cursor-pointer")}
                title={h.snippet}
                onClick={() => navigate(hitHref(h, s.query))}
                data-testid="result-hit"
              >
                {h.title} · {t("answer.citation_page", { page: h.page_start })}
              </button>
            ))}
          </div>
        </div>
      ))}
      {actions.answers.map((a) => (
        <AnswerCard key={a.index} answer={a.answer} compact onCitation={(c) => openCitation(navigate, c)} />
      ))}
      {actions.summaries.map((s) => (
        <div key={s.index} className="flex flex-col gap-2" data-testid="result-summaries">
          {s.items.map((item) => (
            <div key={item.document_id} className="flex flex-col gap-1">
              <button type="button" className="self-start text-xs font-medium text-accent hover:underline cursor-pointer" onClick={() => navigate(`${SCREEN_ROUTES.library}/${item.document_id}`)}>
                {item.name}
              </button>
              <AnswerCard answer={item.answer} compact onCitation={(c) => openCitation(navigate, c)} />
            </div>
          ))}
        </div>
      ))}
      {actions.aggregates.map((a) => (
        <AggregateCard key={a.index} result={a.result} request={a.op ? aggregateRequestFor(a.op, plan.scope, a.result.include_unreviewed) : null} summary={plan.summary ?? execution.steps[a.index]?.summary ?? ""} context={context} />
      ))}
    </div>
  );
}
