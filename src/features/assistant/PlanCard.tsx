import { AlertTriangle, CheckSquare, Pencil, Play, RotateCcw, X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { RuleGrid } from "@/features/projects/RuleGrid";
import { retentionShortens } from "@/lib/commands";
import { api } from "@/lib/ipc";
import { DEFAULT_NAME_TEMPLATE, FOLDER_TEMPLATES, isRuleDraft, type Op, type OrganizeItem, type OrganizePlan, type Plan, type PlanIssue, type PlanResponse, type StepPreview } from "@/lib/types";
import { cn } from "@/lib/utils";
import type { PlanStatus } from "@/stores/assistant";
import { useSettingsStore, useShortcut } from "@/stores/settings";
import { useWorkspaceStore } from "@/stores/workspace";

import { describeOrganizeError } from "@/features/organize/errors";

import { exportPreview, impactLine, impactOf, onlyReviewedValue, opLabel, organizePreview, requiresConfirmation, sourceLabel, summarizeOp, warningIssues, withOnlyReviewed } from "./describePlan";
import { FILE_OPS } from "./opSpecs";
import { PlanEditor } from "./PlanEditor";

export type { PlanStatus };

const PREVIEW_ROWS = 3;

/** Organise diff preview: first rows of `payload.items` (from → to), held rows marked, "전체 보기" expands. */
export function OrganizeDiff({ items, index }: { items: OrganizeItem[]; index: number }) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  if (items.length === 0) return null;
  const shown = expanded ? items : items.slice(0, PREVIEW_ROWS);
  return (
    <div className="mt-1 flex flex-col gap-1 text-xs" data-testid={`plan-preview-${index}`}>
      <div className="flex items-center gap-2 text-fg-subtle">
        <span>{t("planner.card.preview_count", { shown: shown.length, total: items.length })}</span>
        {items.length > PREVIEW_ROWS && (
          <button type="button" className="text-accent hover:underline cursor-pointer" onClick={() => setExpanded((v) => !v)} aria-expanded={expanded} data-testid="plan-preview-expand">
            {expanded ? t("planner.card.show_less") : t("planner.card.show_all")}
          </button>
        )}
      </div>
      <table className="w-full">
        <caption className="sr-only">{t("planner.card.preview_caption")}</caption>
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
              <td className="truncate py-0.5 pr-2 font-mono text-[11px] selectable" title={item.from}>
                {item.from}
              </td>
              <td className="py-0.5 pr-2 text-fg-subtle">→</td>
              <td className={cn("truncate py-0.5 font-mono text-[11px] selectable", item.status === "held" && "text-warn")} title={item.to ?? ""}>
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

function StepPreviewView({ step, override, index }: { step: StepPreview | undefined; override: OrganizePlan | undefined; index: number }) {
  const { t } = useTranslation();
  if (!step && !override) return null;
  const organise = override ? { items: override.items.filter((i) => i.status === "move" || i.status === "held"), moves: override.moves, held: override.held, unchanged: override.unchanged } : organizePreview(step);
  const exp = exportPreview(step);
  // `define_rule`: the rule grid is the confirmation (design §3.15); the payload is the parsed RuleDraft.
  const ruleDraft = step && isRuleDraft(step.payload) ? step.payload : null;
  return (
    <div className="flex flex-col">
      {step?.summary && !organise && !exp && <span className="text-xs text-fg-muted selectable">{step.summary}</span>}
      {ruleDraft && (
        <div className="mt-1" data-testid={`plan-preview-${index}`}>
          <RuleGrid rule={ruleDraft.rule} readOnly unresolvedNames={ruleDraft.unresolved} testId={`plan-rule-${index}`} />
          {ruleDraft.issues.length > 0 && (
            <ul className="mt-1 text-xs text-warn" data-testid="plan-rule-issues">
              {ruleDraft.issues.map((i, n) => (
                <li key={n}>⚠ {i}</li>
              ))}
            </ul>
          )}
        </div>
      )}
      {organise && (
        <>
          <span className="text-xs text-fg-muted">{t("planner.card.organize_summary", { moves: organise.moves, held: organise.held, unchanged: organise.unchanged })}</span>
          <OrganizeDiff items={organise.items} index={index} />
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
}) {
  const { t, i18n } = useTranslation();
  const executeChord = useShortcut("execute_plan") || "Ctrl+Enter";
  const closeChord = useShortcut("close") || "Escape";
  const reviewedDefault = useSettingsStore((s) => s.settings?.processing.organize_reviewed_only ?? true);
  const retention = useWorkspaceStore((s) => s.current?.manifest.retention ?? null);
  const [overrides, setOverrides] = useState<Record<number, OrganizePlan>>({});
  const [previewError, setPreviewError] = useState<string | null>(null);
  const { result, preview, context } = response;
  const confirm = requiresConfirmation(response);
  const onlyReviewed = onlyReviewedValue(plan, reviewedDefault);
  const warnings = warningIssues(result.issues);
  const impact = impactOf(plan, preview);
  const pending = status === "pending";

  /**
   * Re-previews the file operations with the toggled `only_reviewed` (the executor re-plans anyway);
   * a failed re-preview (`template_variable`, `path_outside` …) is shown next to the switch.
   */
  const toggleOnlyReviewed = (value: boolean) => {
    const next = withOnlyReviewed(plan, value);
    onChange(next);
    setPreviewError(null);
    next.operations.forEach((op, index) => {
      if (!FILE_OPS.has(op.op)) return;
      const o = op as Extract<Op, { op: "organize" | "rename" | "move" }>;
      const req = {
        scope: next.scope,
        name_template: o.op === "move" ? null : o.op === "rename" ? o.template : (o.template ?? DEFAULT_NAME_TEMPLATE),
        folder_template: o.op === "rename" ? null : o.op === "move" ? o.folder : (o.folder_template ?? FOLDER_TEMPLATES[o.group_by ?? "doc_type"]),
        only_reviewed: value,
        language: i18n.language.startsWith("ko") ? ("ko" as const) : ("en" as const),
      };
      void api
        .organizePlan(req)
        .then((p) => setOverrides((prev) => ({ ...prev, [index]: p })))
        .catch((e: unknown) => setPreviewError(describeOrganizeError(e, t)));
    });
  };

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
                  {params && <span className="truncate text-fg-muted selectable">{params}</span>}
                </div>
                {op.op === "set_retention" && retention && retentionShortens(retention, op.item, op.days) && (
                  <span className="text-xs text-warn" data-testid={`plan-retention-shorter-${i}`}>
                    <AlertTriangle className="mr-1 inline size-3" aria-hidden="true" />
                    {t("planner.card.retention_shorter")}
                  </span>
                )}
                <StepPreviewView step={step} override={overrides[i]} index={i} />
              </div>
            </li>
          );
        })}
      </ol>
      <p className="text-xs text-fg-muted" data-testid="plan-impact">
        {impactLine(impact, t)}
      </p>
      {onlyReviewed !== null && (
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <label className="flex items-center gap-2">
            <Switch checked={onlyReviewed} disabled={!pending} onCheckedChange={toggleOnlyReviewed} aria-label={t("planner.card.only_reviewed")} data-testid="plan-only-reviewed" />
            {t("planner.card.only_reviewed")}
          </label>
          {previewError && (
            <span className="text-danger" role="alert" data-testid="plan-only-reviewed-error">
              {t("planner.card.preview_error", { message: previewError })}
            </span>
          )}
        </div>
      )}
      {warnings.length > 0 && (
        <ul className="flex flex-col gap-0.5 text-xs text-fg-muted" aria-label={t("planner.card.warnings")} data-testid="plan-warnings">
          {warnings.map((w, i) => (
            <li key={i}>
              ⚠ {w.message}
              {w.path && <span className="text-fg-subtle"> ({w.path})</span>}
            </li>
          ))}
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
    </div>
  );
}
