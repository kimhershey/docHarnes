import { ChevronDown, ChevronUp, Sigma } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useSchemas } from "@/lib/queries";
import type { AggregateFn, AggregateRequest, AggregateResult, DocumentView } from "@/lib/types";

import { amountFields, formatAggregate, formatGroup } from "./aggregate";

const FUNCTIONS: AggregateFn[] = ["sum", "count", "avg", "min", "max"];
/** Field select value for the document count (`func = count`, no field). */
const COUNT = "__count__";
const DEBOUNCE_MS = 300;

/**
 * 집계 바 (design §3.7 문서 라이브러리): sum / count / average of one amount field over the
 * selected documents (else the type filter), review-pending values kept apart (decision 16).
 * Re-runs 300 ms after the selection, filter or controls change.
 */
export function AggregateBar({
  docType,
  selectedDocs,
  typeLabel,
  open,
  onOpenChange,
}: {
  /** Active library type filter (null = none). */
  docType: string | null;
  /** Library selection in list order (the scope when non-empty). */
  selectedDocs: DocumentView[];
  typeLabel: (slug: string) => string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t, i18n } = useTranslation();
  const { data: schemas } = useSchemas();
  const selectedIds = useMemo(() => selectedDocs.map((d) => d.id), [selectedDocs]);
  const types = useMemo(() => (docType ? [docType] : [...new Set(selectedDocs.flatMap((d) => (d.doc_type ? [d.doc_type] : [])))]), [docType, selectedDocs]);
  const fields = useMemo(() => amountFields(schemas, types), [schemas, types]);
  const [field, setField] = useState<string>(COUNT);
  const fieldTouched = useRef(false);
  const [fn, setFn] = useState<AggregateFn>("sum");
  const [includeUnreviewed, setIncludeUnreviewed] = useState(false);
  const [monthly, setMonthly] = useState(false);
  const [result, setResult] = useState<AggregateResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const seq = useRef(0);

  // Default to the type's total (the first amount field) until the user picks a field; drop a
  // field that the current types do not have.
  useEffect(() => {
    const first = fields[0]?.key;
    if (!fieldTouched.current) setField(first ?? COUNT);
    else if (field !== COUNT && !fields.some((f) => f.key === field)) setField(first ?? COUNT);
  }, [fields, field]);

  const request = useMemo<AggregateRequest>(
    () => ({
      func: field === COUNT ? "count" : fn,
      field: field === COUNT ? null : field,
      group_by: monthly ? "month" : "none",
      include_unreviewed: includeUnreviewed,
      scope: selectedIds.length > 0 ? { doc_ids: selectedIds } : { filter: { doc_type: docType } },
    }),
    [field, fn, monthly, includeUnreviewed, selectedIds, docType],
  );
  const reqKey = JSON.stringify(request);

  const run = useCallback(async () => {
    const mine = ++seq.current;
    setBusy(true);
    try {
      const r = await api.aggregateRun(JSON.parse(reqKey) as AggregateRequest);
      if (mine !== seq.current) return;
      setResult(r);
      setError(null);
    } catch (e) {
      if (mine !== seq.current) return;
      setResult(null);
      setError(describeError(e, t));
    } finally {
      if (mine === seq.current) setBusy(false);
    }
  }, [reqKey, t]);

  useEffect(() => {
    if (!open) return;
    const h = setTimeout(() => void run(), DEBOUNCE_MS);
    return () => clearTimeout(h);
  }, [open, run]);

  const scopeLabel = selectedIds.length > 0 ? t("aggregate.scope_selection", { count: selectedIds.length }) : docType ? t("aggregate.scope_type", { type: typeLabel(docType) }) : "";
  const fieldLabel = (f: { label_ko: string; label_en: string }) => (i18n.language.startsWith("ko") ? f.label_ko : f.label_en);

  if (!open) {
    return (
      <div className="flex items-center gap-2 border-b border-border px-4 py-1 text-xs text-fg-muted" data-testid="aggregate-bar" data-open="false">
        <Sigma className="size-3.5" aria-hidden="true" />
        <span>{t("aggregate.title")}</span>
        <span className="text-fg-subtle">{scopeLabel}</span>
        <Button size="icon-sm" variant="ghost" className="ml-auto h-6 w-6" onClick={() => onOpenChange(true)} aria-label={t("aggregate.expand")} data-testid="aggregate-toggle">
          <ChevronDown />
        </Button>
      </div>
    );
  }

  return (
    <div className="flex flex-wrap items-center gap-2 border-b border-border bg-bg-sunken/40 px-4 py-1.5 text-xs" role="region" aria-label={t("aggregate.title")} data-testid="aggregate-bar" data-open="true">
      <Sigma className="size-3.5 text-fg-muted" aria-hidden="true" />
      <span className="font-medium">{t("aggregate.title")}</span>
      <span className="text-fg-subtle">{scopeLabel}</span>
      <Select
        value={field}
        onValueChange={(v) => {
          fieldTouched.current = true;
          setField(v);
        }}
      >
        <SelectTrigger className="h-7 w-36 text-xs" aria-label={t("aggregate.field")} data-testid="aggregate-field">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={COUNT}>{t("aggregate.count")}</SelectItem>
          {fields.map((f) => (
            <SelectItem key={f.key} value={f.key}>
              {fieldLabel(f)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Select value={field === COUNT ? "count" : fn} onValueChange={(v) => setFn(v as AggregateFn)} disabled={field === COUNT}>
        <SelectTrigger className="h-7 w-24 text-xs" aria-label={t("aggregate.fn_label")} data-testid="aggregate-fn">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {FUNCTIONS.map((f) => (
            <SelectItem key={f} value={f}>
              {t(`aggregate.fn.${f}`)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <label className="flex items-center gap-1.5">
        <Switch checked={includeUnreviewed} onCheckedChange={setIncludeUnreviewed} data-testid="aggregate-include-unreviewed" />
        {t("aggregate.include_unreviewed")}
      </label>
      <label className="flex items-center gap-1.5">
        <Switch checked={monthly} onCheckedChange={setMonthly} data-testid="aggregate-monthly" />
        {t("aggregate.monthly")}
      </label>
      <Button size="sm" variant="secondary" className="h-7" onClick={() => void run()} disabled={busy} data-testid="aggregate-run">
        {busy ? <Spinner /> : <Sigma />}
        {t("aggregate.run")}
      </Button>
      <span className="font-medium tabular-nums" aria-live="polite" data-testid="aggregate-value">
        {error ? <span className="text-danger">{error}</span> : result ? formatAggregate(result, t) : busy ? t("aggregate.running") : ""}
      </span>
      {result && !error && <span className="text-fg-subtle">({t(`aggregate.basis.${result.basis === "imported_at" ? "imported_at" : result.basis === "mixed" ? "mixed" : "doc_date"}`)})</span>}
      {monthly && result && (
        <div className="flex basis-full flex-wrap gap-1" data-testid="aggregate-groups">
          {result.groups.length === 0 ? (
            <span className="text-fg-subtle">{t("aggregate.no_groups")}</span>
          ) : (
            result.groups.map((g) => (
              <span key={g.key} className="rounded-full border border-border bg-bg-elevated px-2 py-0.5 tabular-nums">
                {formatGroup(g, result.func, t)}
              </span>
            ))
          )}
        </div>
      )}
      <Button size="icon-sm" variant="ghost" className="ml-auto h-6 w-6" onClick={() => onOpenChange(false)} aria-label={t("aggregate.collapse")} data-testid="aggregate-toggle">
        <ChevronUp />
      </Button>
    </div>
  );
}
