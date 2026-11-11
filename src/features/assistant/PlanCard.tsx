import { AlertTriangle, CheckSquare, CornerDownLeft, Pencil, Play, RotateCcw, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { RuleGrid } from "@/features/projects/RuleGrid";
import { retentionShortens } from "@/lib/commands";
import { api } from "@/lib/ipc";
import type { OrganizeItem, Plan, PlanIssue, PlanPreview, PlanResponse, StepPreview } from "@/lib/types";
import { cn } from "@/lib/utils";
import type { Clipped, PlanStatus } from "@/stores/assistant";
import { useSettingsStore, useShortcut } from "@/stores/settings";
import { useWorkspaceStore } from "@/stores/workspace";

import { describeExportError, describeOrganizeError } from "@/features/organize/errors";

import { ClippedNote } from "./ClippedNote";
import { exportPreview, impactLine, impactOf, onlyReviewedValue, opLabel, organizePreview, requiresConfirmation, ruleDraftPreview, sourceLabel, summarizeOp, warningIssues, withOnlyReviewed } from "./describePlan";
import { PlanEditor } from "./PlanEditor";
import { planKey, previewEditedPlan, previewFailure } from "./repreview";

export type { PlanStatus };

const PREVIEW_ROWS = 3;
/** An edited plan is previewed this long after the last edit (the editor patches the plan per keystroke). */
const REPREVIEW_DELAY_MS = 350;

/**
 * Organise diff preview: first rows of `payload.items` (from → to), held rows marked, "전체 보기" expands.
 * `total`: how many rows the plan has when `items` is only the first of them (a card read back after it was
 * stored smaller, CON-16) — the count line then reads "200 / 6000".
 */
export function OrganizeDiff({ items, index, total }: { items: OrganizeItem[]; index: number; total?: number }) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  if (items.length === 0) return null;
  const shown = expanded ? items : items.slice(0, PREVIEW_ROWS);
  return (
    <div className="mt-1 flex flex-col gap-1 text-xs" data-testid={`plan-preview-${index}`}>
      <div className="flex items-center gap-2 text-fg-subtle">
        <span data-testid="plan-preview-count">{t("planner.card.preview_count", { shown: shown.length, total: Math.max(items.length, total ?? 0) })}</span>
        {items.length > PREVIEW_ROWS && (
          <button type="button" className="text-accent hover:underline cursor-pointer" onClick={() => setExpanded((v) => !v)} aria-expanded={expanded} data-testid="plan-preview-expand">
            {expanded ? t("planner.card.show_less") : t("planner.card.show_all")}
          </button>
        )}
      </div>
      {/* Fixed layout: the two path columns share the card's width and a long path wraps inside its cell (the
          full path is also the cell's title). An auto-layout table is as wide as its longest path (UI-2). */}
      <table className="w-full table-fixed">
        <caption className="sr-only">{t("planner.card.preview_caption")}</caption>
        <colgroup>
          <col />
          <col className="w-6" />
          <col />
        </colgroup>
        <thead className="sr-only">
          <tr>
            <th scope="col">{t("planner.card.columns.from")}</th>
            <th scope="col" aria-hidden="true" />
            <th scope="col">{t("planner.card.columns.to")}</th>
          </tr>
        </thead>
        <tbody>
          {shown.map((item) => (
            <tr key={`${item.doc_id}-${item.from}`} className="align-top" data-testid="plan-preview-row" data-item-status={item.status}>
              <td className="break-all py-0.5 pr-2 font-mono text-[0.6875rem] selectable" title={item.from}>
                {item.from}
              </td>
              <td className="py-0.5 pr-2 text-fg-subtle">→</td>
              <td className={cn("break-all py-0.5 font-mono text-[0.6875rem] selectable", item.status === "held" && "text-warn")} title={item.to ?? ""}>
                {item.status === "held" ? (
                  <>
                    <AlertTriangle className="mr-1 inline size-3" aria-hidden="true" />
                    {t("planner.card.held")}
                    {item.reason && item.reason !== "review_pending" && <span className="ml-1 text-fg-subtle">({item.reason})</span>}
                  </>
                ) : (
                  <>
                    {item.to}
                    {item.missing && item.missing.length > 0 && <span className="ml-1 text-warn">({t("planner.card.missing_values", { vars: item.missing.join(", ") })})</span>}
                  </>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function StepPreviewView({ step, index }: { step: StepPreview | undefined; index: number }) {
  const { t } = useTranslation();
  if (!step) return null;
  const organise = organizePreview(step);
  const exp = exportPreview(step);
  // `define_rule`: the rule grid is the confirmation (design §3.15). `exec::preview` sends
  // `{ description, draft: RuleDraft | null }`; without a parsed draft only the sentence is shown.
  const rule = ruleDraftPreview(step);
  // The preview of an edited step failed (unknown template variable, path outside the workspace …).
  const failed = previewFailure(step);
  return (
    <div className="flex flex-col">
      {failed && (
        <span className="text-xs text-danger selectable" role="alert" data-testid={`plan-preview-error-${index}`}>
          {t("planner.card.preview_error", { message: failed })}
        </span>
      )}
      {step.summary && !organise && !exp && <span className="text-xs text-fg-muted selectable">{step.summary}</span>}
      {rule?.draft && (
        <div className="mt-1" data-testid={`plan-preview-${index}`}>
          <RuleGrid rule={rule.draft.rule} readOnly unresolvedNames={rule.draft.unresolved} testId={`plan-rule-${index}`} />
          {rule.draft.issues.length > 0 && (
            <ul className="mt-1 text-xs text-warn" data-testid="plan-rule-issues">
              {rule.draft.issues.map((i, n) => (
                <li key={n}>⚠ {i}</li>
              ))}
            </ul>
          )}
        </div>
      )}
      {rule && !rule.draft && rule.description && (
        <span className="mt-1 text-xs text-fg-muted selectable" data-testid={`plan-preview-${index}`}>
          {t("planner.card.rule_description", { description: rule.description })}
        </span>
      )}
      {organise && (
        <>
          <span className="text-xs text-fg-muted">{t("planner.card.organize_summary", { moves: organise.moves, held: organise.held, unchanged: organise.unchanged })}</span>
          <OrganizeDiff items={organise.items} total={organise.total} index={index} />
        </>
      )}
      {exp && (
        <span className="text-xs text-fg-muted selectable" data-testid={`plan-preview-${index}`}>
          {t("planner.card.export_path", { count: exp.documents, path: exp.path })}
          {exp.exists && <span className="ml-1 text-warn">{t("planner.card.overwrite")}</span>}
        </span>
      )}
    </div>
  );
}

/** Notes of `planner::guard::contract_checks`: each of them makes the plan one the user confirms. */
const CONTRACT_WARNINGS: ReadonlySet<string> = new Set(["unmet_request", "grouping_dropped", "ungrounded_change"]);

/**
 * Plan card (design §3.7.6): header with confidence / source, the
 * interpretation, one checklist row per op with its parameter summary and
 * preview (organise diff, export path, delete count), the impact line, the
 * `only_reviewed` toggle, validator warnings, and the approve / edit /
 * rephrase / cancel actions. Ctrl+Enter and Esc are bound by the page.
 */
export function PlanCard({
  response,
  plan,
  status,
  errors,
  editing,
  active,
  onChange,
  onExecute,
  onCancel,
  onRephrase,
  onToggleEdit,
  onAskBack,
  editedPreview,
  onPreview,
  clipped,
}: {
  response: PlanResponse;
  plan: Plan;
  status: PlanStatus;
  errors: PlanIssue[] | null;
  editing: boolean;
  active: boolean;
  onChange: (plan: Plan) => void;
  onExecute: () => void;
  onCancel: () => void;
  onRephrase: () => void;
  onToggleEdit: () => void;
  /** 되묻기: the user's reply to this plan goes back to the model with the plan as context. */
  onAskBack?: (reply: string) => void;
  /** The preview of `plan` as edited (absent while `plan` is as interpreted, or while a fresh preview is on its way). */
  editedPreview?: PlanPreview | null;
  /** The preview of the edited plan arrived: the owner keeps it with the plan (and drops it at the next edit). */
  onPreview?: (preview: PlanPreview) => void;
  /** The card was read back from a copy stored smaller than it was (CON-16): it says so. */
  clipped?: Clipped;
}) {
  const { t, i18n } = useTranslation();
  const [reply, setReply] = useState("");
  const executeChord = useShortcut("execute_plan") || "Ctrl+Enter";
  const closeChord = useShortcut("close") || "Escape";
  const reviewedDefault = useSettingsStore((s) => s.settings?.processing.organize_reviewed_only ?? true);
  const retention = useWorkspaceStore((s) => s.current?.manifest.retention ?? null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const { result, context } = response;
  const onlyReviewed = onlyReviewedValue(plan, reviewedDefault);
  const warnings = warningIssues(result.issues);
  const pending = status === "pending";
  // The numbers on the card belong to the plan it shows (PLN-14): the planner's preview while the plan is as
  // interpreted; once the scope or a parameter was edited, the preview of the edited plan — and none while
  // that one is on its way (the card used to keep "대상 문서 3건" of the plan before the edit).
  // (Keyed once per plan object: the page renders its cards on every keystroke in the composer, and a scope
  // can name thousands of documents.)
  const key = useMemo(() => planKey(plan), [plan]);
  const interpretedKey = useMemo(() => planKey(result.plan), [result.plan]);
  const edited = key !== interpretedKey;
  const preview = edited ? (editedPreview ?? null) : response.preview;
  const refreshing = edited && !editedPreview && pending && previewError === null;
  const impact = impactOf(plan, preview);
  /** Whose numbers the card shows: the planner's, the edited plan's, or none (on their way / could not be made). */
  const previewOf = !edited ? "planned" : preview ? "edited" : refreshing ? "refreshing" : "none";
  // An edit can make a plan one that is confirmed first (an export onto an existing file, a dry run switched off).
  const confirm = requiresConfirmation(response) || (edited && preview?.requires_confirmation === true);
  const language = i18n.language.startsWith("ko") ? ("ko" as const) : ("en" as const);

  useEffect(() => {
    setPreviewError(null);
    if (!edited || editedPreview || !pending || !onPreview) return;
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void previewEditedPlan(plan, result.plan, response.preview, api, {
        language,
        onlyReviewedDefault: reviewedDefault,
        countLabel: (count) => t("planner.card.step_documents", { count }),
        deleteLabel: (count) => t("planner.card.step_delete", { count }),
        describeError: (e, kind) => (kind === "export" ? describeExportError(e, t) : describeOrganizeError(e, t)),
      })
        .then((fresh) => {
          if (!cancelled) onPreview(fresh);
        })
        .catch((e: unknown) => {
          if (!cancelled) setPreviewError(describeOrganizeError(e, t));
        });
    }, REPREVIEW_DELAY_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
    // The plan's key stands for the plan; the callbacks and the response are those of the same card.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, edited, editedPreview, pending, language, reviewedDefault]);

  /** `only_reviewed` is a parameter of the file ops: toggling it edits the plan, which is previewed again like any edit. */
  const toggleOnlyReviewed = (value: boolean) => onChange(withOnlyReviewed(plan, value));

  return (
    <div
      className={cn("flex w-full flex-col gap-2 rounded-lg border bg-bg-elevated p-3 text-sm", active && pending ? "border-accent" : "border-border")}
      role="group"
      aria-label={t("planner.card.aria", { summary: plan.summary ?? result.text })}
      data-testid="assistant-card-plan"
      data-plan-status={status}
      data-plan-active={active && pending}
    >
      <div className="flex flex-wrap items-center gap-2 text-xs text-fg-subtle">
        <Badge variant={confirm ? "warn" : "info"}>{t("planner.card.title")}</Badge>
        <span>{t("assistant.confidence", { value: result.plan.confidence.toFixed(2) })}</span>
        <span>· {t("planner.card.source", { source: sourceLabel(result.source, t) })}</span>
        {result.source === "model" && result.model && <span>· {result.model}</span>}
        {status === "executing" && (
          <span className="ml-auto flex items-center gap-1" role="status">
            <Spinner /> {t("planner.card.executing")}
          </span>
        )}
        {status === "done" && (
          <Badge variant="ok" className="ml-auto">
            {t("planner.card.executed")}
          </Badge>
        )}
        {status === "cancelled" && (
          <Badge variant="muted" className="ml-auto">
            {t("planner.card.cancelled")}
          </Badge>
        )}
      </div>
      <p className="font-medium selectable" data-testid="plan-summary">
        {t("planner.card.interpretation", { summary: plan.summary ?? result.text })}
      </p>
      {result.date && <p className="text-xs text-fg-muted">{t("assistant.date_resolved", { from: result.date.from, to: result.date.to, label: result.date.label })}</p>}
      <ol className="flex flex-col gap-1" aria-label={t("planner.card.steps")}>
        {plan.operations.map((op, i) => {
          const step = preview?.steps.find((s) => s.index === i);
          const params = summarizeOp(op, t, { lang: i18n.language, context });
          return (
            <li key={i} className="flex items-start gap-2" data-testid={`plan-step-${i}`} data-op={op.op}>
              <CheckSquare className="mt-0.5 size-4 shrink-0 text-ok" aria-hidden="true" />
              <div className="flex min-w-0 flex-1 flex-col">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="tabular-nums text-fg-subtle">{i + 1}</span>
                  <span className="font-medium">{opLabel(op.op, t)}</span>
                  {step?.confirm && <AlertTriangle className="size-3.5 text-warn" role="img" aria-label={t("planner.card.needs_confirmation")} />}
                  {params && (
                    <span className="truncate text-fg-muted selectable" title={params} data-testid={`plan-step-params-${i}`}>
                      {params}
                    </span>
                  )}
                </div>
                {op.op === "set_retention" && retention && retentionShortens(retention, op.item, op.days) && (
                  <span className="text-xs text-warn" data-testid={`plan-retention-shorter-${i}`}>
                    <AlertTriangle className="mr-1 inline size-3" aria-hidden="true" />
                    {t("planner.card.retention_shorter")}
                  </span>
                )}
                <StepPreviewView step={step} index={i} />
              </div>
            </li>
          );
        })}
      </ol>
      {/* `data-preview`: whose numbers these are — the planner's (`planned`), the edited plan's (`edited`) or none
          (`refreshing`; `none` = the preview failed, or the edited plan ran before its preview arrived). */}
      {(previewOf !== "none" || previewError) && (
        <p className="text-xs text-fg-muted" data-testid="plan-impact" data-preview={previewOf}>
          {previewOf === "planned" || previewOf === "edited" ? (
            impactLine(impact, t, preview?.uses_scope !== false)
          ) : previewOf === "refreshing" ? (
            <span className="flex items-center gap-1" role="status">
              <Spinner /> {t("planner.card.preview_refreshing")}
            </span>
          ) : (
            <span className="text-danger" role="alert" data-testid="plan-preview-error">
              {t("planner.card.preview_error", { message: previewError })}
            </span>
          )}
        </p>
      )}
      <ClippedNote clipped={clipped} />
      {onlyReviewed !== null && (
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <label className="flex items-center gap-2">
            <Switch checked={onlyReviewed} disabled={!pending} onCheckedChange={toggleOnlyReviewed} aria-label={t("planner.card.only_reviewed")} data-testid="plan-only-reviewed" />
            {t("planner.card.only_reviewed")}
          </label>
        </div>
      )}
      {warnings.length > 0 && (
        <ul className="flex flex-col gap-0.5 text-xs text-fg-muted" aria-label={t("planner.card.warnings")} data-testid="plan-warnings">
          {warnings.map((w, i) => {
            // The planner's checks against the sentence (0.8.0) are why this card waits for approval: they
            // stand out, and their internal path says nothing to the user.
            const contract = CONTRACT_WARNINGS.has(w.code);
            return (
              <li key={i} className={contract ? "font-medium text-warn" : undefined} data-warning-code={w.code}>
                ⚠ {w.message}
                {w.path && !contract && <span className="text-fg-subtle"> ({w.path})</span>}
              </li>
            );
          })}
        </ul>
      )}
      {editing && pending && <PlanEditor plan={plan} context={context} onChange={onChange} />}
      {errors && errors.length > 0 && (
        <ul className="flex flex-col gap-0.5 rounded-md bg-danger-soft px-2 py-1.5 text-xs text-danger" role="alert" data-testid="plan-errors">
          <li className="font-medium">{t("planner.card.invalid")}</li>
          {errors.map((e, i) => (
            <li key={i}>
              {e.message}
              {e.path && <span className="opacity-70"> ({e.path})</span>}
            </li>
          ))}
        </ul>
      )}
      {pending && (
        <div className="flex flex-wrap items-center gap-2">
          <Button size="sm" onClick={onExecute} data-testid="plan-execute">
            <Play />
            {confirm ? t("planner.card.execute") : t("planner.card.run")}
            <Kbd chord={executeChord} />
          </Button>
          <Button size="sm" variant={editing ? "secondary" : "outline"} aria-pressed={editing} onClick={onToggleEdit} data-testid="plan-edit">
            <Pencil />
            {editing ? t("planner.card.edit_done") : t("planner.card.edit")}
          </Button>
          <Button size="sm" variant="ghost" onClick={onRephrase} data-testid="plan-rephrase">
            <RotateCcw />
            {t("planner.card.rephrase")}
          </Button>
          <Button size="sm" variant="ghost" onClick={onCancel} data-testid="plan-cancel">
            <X />
            {t("planner.card.cancel")}
            <Kbd chord={closeChord} />
          </Button>
        </div>
      )}
      {pending && onAskBack && (
        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            const r = reply.trim();
            if (!r) return;
            onAskBack(r);
            setReply("");
          }}
        >
          <Input value={reply} onChange={(e) => setReply(e.target.value)} placeholder={t("planner.card.ask_back_placeholder")} aria-label={t("planner.card.ask_back")} className="h-8 min-w-56 flex-1" data-testid="plan-ask-back-input" />
          <Button type="submit" size="sm" variant="outline" disabled={!reply.trim()} data-testid="plan-ask-back">
            <CornerDownLeft />
            {t("planner.card.ask_back")}
          </Button>
        </form>
      )}
    </div>
  );
}
