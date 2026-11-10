import type { TFunction } from "i18next";

import type { DocTypeInfo, FieldRecord, FieldStatus, ReviewReason } from "@/lib/types";

/** Built-in doc type slugs that have their own i18n labels. */
const BUILTIN_TYPES = [
  "rfp",
  "proposal",
  "sow",
  "srs",
  "dev_contract",
  "wbs",
  "progress_report",
  "meeting_minutes",
  "test_report",
  "change_request",
  "acceptance",
  "estimate",
  "tax_invoice",
  "invoice_exempt",
  "receipt",
  "business_registration",
  "contract",
  "application_form",
  "general",
  "other",
];

/** Label of a document type slug: i18n for the built-in types, the schema's name otherwise. */
export function docTypeLabel(slug: string | null | undefined, types: DocTypeInfo[] | undefined, t: TFunction, lang: string): string {
  if (!slug) return t("viewer.no_doc_type");
  const info = types?.find((d) => d.slug === slug);
  if (info && !info.builtin) return lang.startsWith("ko") ? info.label_ko : info.label_en;
  if (BUILTIN_TYPES.includes(slug)) return t(`documents.doc_types.${slug}`);
  return info ? (lang.startsWith("ko") ? info.label_ko : info.label_en) : slug;
}

export function fieldLabel(f: { label_ko: string; label_en: string; key: string }, lang: string): string {
  const l = lang.startsWith("ko") ? f.label_ko || f.label_en : f.label_en || f.label_ko;
  return l || f.key;
}

/** Fallback for `processing.review_threshold` when the live value is not at hand (mirrors the settings default). */
export const DEFAULT_REVIEW_THRESHOLD = 0.8;

export type ConfidenceTone = "ok" | "warn" | "danger" | "muted";

/** Static text classes per tone: Tailwind only generates classes it can see as literals (`text-${tone}` is not one). */
export const CONFIDENCE_TEXT_CLASS: Record<ConfidenceTone, string> = { ok: "text-ok", warn: "text-warn", danger: "text-danger", muted: "text-fg-muted" };

/**
 * Colour + icon label for a confidence, derived from the review threshold so colour, glyph and the
 * "low confidence" flag agree (design §3.7.4): `ok` at or above the threshold, `warn` within 0.2
 * below it, `danger` further down. Colour-blind safe: the badge text carries the meaning too.
 */
export function confidenceTone(c: number | null | undefined, threshold = DEFAULT_REVIEW_THRESHOLD): ConfidenceTone {
  if (c == null || Number.isNaN(c)) return "muted";
  const v = c + 1e-9; // 0.6 must count as "warn" at 0.8 although 0.8 - 0.2 > 0.6 in floating point
  if (v >= threshold) return "ok";
  if (v >= threshold - 0.2) return "warn";
  return "danger";
}

export function confidenceGlyph(c: number | null | undefined, threshold = DEFAULT_REVIEW_THRESHOLD): string {
  const tone = confidenceTone(c, threshold);
  return tone === "ok" ? "✔" : tone === "warn" ? "●" : tone === "danger" ? "▲" : "–";
}

export function fieldStatusVariant(s: FieldStatus): "ok" | "warn" | "danger" | "muted" | "info" {
  switch (s) {
    case "approved":
    case "corrected":
      return "ok";
    case "rejected":
      return "danger";
    case "held":
      return "warn";
    default:
      return "muted";
  }
}

export function reasonVariant(r: ReviewReason | null): "ok" | "warn" | "danger" | "muted" | "info" {
  switch (r) {
    case "validation_failed":
    case "required_missing":
      return "danger";
    case "low_confidence":
      return "warn";
    case "audit":
      return "info";
    default:
      return "muted";
  }
}

/** Human path: `supplier.brn` → "공급자 사업자등록번호", `items[2].amount` → "품목 3행 · 금액". */
export function fieldTitle(f: FieldRecord, t: TFunction, lang: string): string {
  const base = fieldLabel(f, lang);
  const m = f.path.match(/^([a-z0-9_]+)\[(\d+)\]/);
  if (m) return `${t("viewer.fields.table_row", { table: m[1], row: Number(m[2]) + 1 })} · ${base}`;
  return base;
}

export function formatConfidence(c: number | null | undefined): string {
  return c == null ? "–" : `${Math.round(c * 100)}%`;
}
