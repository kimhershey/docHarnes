import type { TFunction } from "i18next";

import type { DocTypeInfo, FieldRecord, FieldStatus, ReviewReason } from "@/lib/types";

/** Built-in doc type slugs that have their own i18n labels. */
const BUILTIN_TYPES = ["tax_invoice", "invoice_exempt", "receipt", "business_registration", "contract", "application_form", "general", "other"];

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

/** Colour + icon label for a confidence (colour-blind safe: badge text carries the meaning too). */
export function confidenceTone(c: number | null | undefined): "ok" | "warn" | "danger" | "muted" {
  if (c == null) return "muted";
  if (c >= 0.8) return "ok";
  if (c >= 0.6) return "warn";
  return "danger";
}

export function confidenceGlyph(c: number | null | undefined): string {
  const tone = confidenceTone(c);
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
