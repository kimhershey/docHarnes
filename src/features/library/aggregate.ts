// Formatting helpers for the library aggregate bar (design §3.7 "집계 바", decision 16:
// review-pending values are kept apart from the headline number unless included).

import type { AggregateFn, AggregateGroup, AggregateResult, SchemaRecord } from "@/lib/types";

/** `t`-shaped function (react-i18next's `t` or a test stub). */
export type TranslateFn = (key: string, vars?: Record<string, unknown>) => string;

/** Thousands separators, at most `maxFraction` decimals (`1234567.891` → `1,234,567.89`). */
export function formatAmount(value: number, maxFraction = 2): string {
  if (!Number.isFinite(value)) return "–";
  return new Intl.NumberFormat("en-US", { maximumFractionDigits: maxFraction }).format(value);
}

export interface AmountField {
  key: string;
  label_ko: string;
  label_en: string;
}

/** The schema that applies to a document type: the enabled default, else the first enabled one. */
export function schemaForType(schemas: SchemaRecord[] | undefined, docType: string): SchemaRecord | undefined {
  const candidates = (schemas ?? []).filter((s) => s.doc_type === docType && s.enabled);
  return candidates.find((s) => s.is_default) ?? candidates[0];
}

/** Top-level amount fields of the schemas of `docTypes` (the backend resolves the field per type by key / label). */
export function amountFields(schemas: SchemaRecord[] | undefined, docTypes: string[]): AmountField[] {
  const out: AmountField[] = [];
  const seen = new Set<string>();
  for (const type of docTypes) {
    const schema = schemaForType(schemas, type);
    if (!schema) continue;
    for (const f of schema.definition.fields) {
      if (f.kind !== "amount" || seen.has(f.key)) continue;
      seen.add(f.key);
      out.push({ key: f.key, label_ko: f.label_ko || f.label_en || f.key, label_en: f.label_en || f.label_ko || f.key });
    }
  }
  return out;
}

/**
 * "합계 1,234,500 = 검토 완료 33건 1,180,000 + 대기 4건 54,500 · 값 없음 2건". With the pending part
 * excluded (the default) the headline only covers reviewed values, so the pending part is shown as
 * "제외"; for count the parts carry counts only, for avg / min / max only the counts are listed.
 */
export function formatAggregate(r: AggregateResult, t: TranslateFn): string {
  const fn = t(`aggregate.fn.${r.func}`);
  const included = r.include_unreviewed;
  let text: string;
  if (r.func === "count") {
    const headline = `${fn} ${t("aggregate.count_value", { count: formatAmount(r.value, 0) })}`;
    const reviewed = t("aggregate.reviewed_count", { count: r.reviewed.count });
    const pending = t(included ? "aggregate.pending_count" : "aggregate.pending_count_excluded", { count: r.unreviewed.count });
    text = included ? `${headline} = ${reviewed} + ${pending}` : `${headline} = ${reviewed} · ${pending}`;
  } else if (r.func === "sum") {
    const headline = `${fn} ${formatAmount(r.value)}`;
    const reviewed = t("aggregate.reviewed", { count: r.reviewed.count, value: formatAmount(r.reviewed.value) });
    const pending = t(included ? "aggregate.pending" : "aggregate.pending_excluded", { count: r.unreviewed.count, value: formatAmount(r.unreviewed.value) });
    text = included ? `${headline} = ${reviewed} + ${pending}` : `${headline} = ${reviewed} · ${pending}`;
  } else {
    const headline = `${fn} ${formatAmount(r.value)}`;
    const reviewed = t("aggregate.reviewed_count", { count: r.reviewed.count });
    const pending = t(included ? "aggregate.pending_count" : "aggregate.pending_count_excluded", { count: r.unreviewed.count });
    text = `${headline} · ${reviewed} · ${pending}`;
  }
  if (r.missing > 0) text += ` · ${t("aggregate.missing", { count: r.missing })}`;
  return text;
}

/** One group chip: `2026-10: 1,234,500 (3건)`. */
export function formatGroup(g: AggregateGroup, func: AggregateFn, t: TranslateFn): string {
  return t("aggregate.group", { key: g.key, value: func === "count" ? formatAmount(g.value, 0) : formatAmount(g.value), count: g.count });
}
