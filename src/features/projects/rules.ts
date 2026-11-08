// Pure helpers of the project rules grid (design §3.15): field options per
// document type, operand / verdict labels, completeness and unresolved-name
// checks. Shared by the projects screen and the assistant's rule preview.

import type { TFunction } from "i18next";

import type { DocTypeInfo, FieldDef, Project, Rule, RuleOperand, SchemaRecord, Verdict } from "@/lib/types";

export interface FieldOption {
  path: string;
  label_ko: string;
  label_en: string;
  kind: string;
}

/** Scalar field paths of a schema (group children as `group.child`; tables are skipped). */
export function fieldOptions(schema: SchemaRecord | undefined | null): FieldOption[] {
  if (!schema) return [];
  const out: FieldOption[] = [];
  const push = (f: FieldDef, path: string, prefixKo = "", prefixEn = "") => out.push({ path, label_ko: `${prefixKo}${f.label_ko || f.label_en || f.key}`.trim(), label_en: `${prefixEn}${f.label_en || f.label_ko || f.key}`.trim(), kind: f.kind });
  for (const f of schema.definition.fields) {
    if (f.kind === "table") continue;
    if (f.kind === "group") for (const c of f.fields) push(c, `${f.key}.${c.key}`, `${f.label_ko} `, `${f.label_en} `);
    else push(f, f.key);
  }
  return out;
}

/** The default enabled schema of a document type (same preference as the backend's `schema_for_doc_type`). */
export function schemaForType(schemas: SchemaRecord[] | undefined, slug: string | null | undefined): SchemaRecord | null {
  if (!schemas || !slug) return null;
  return [...schemas.filter((s) => s.doc_type === slug && s.enabled)].sort((a, b) => Number(b.is_default) - Number(a.is_default) || Number(a.source === "builtin") - Number(b.source === "builtin"))[0] ?? null;
}

export function fieldLabelOf(schemas: SchemaRecord[] | undefined, slug: string | null | undefined, path: string | null | undefined, lang: string): string {
  if (!path) return "";
  const opt = fieldOptions(schemaForType(schemas, slug)).find((o) => o.path === path);
  if (!opt) return path;
  return lang.startsWith("ko") ? opt.label_ko : opt.label_en;
}

export function docTypeLabelOf(types: DocTypeInfo[] | undefined, slug: string | null | undefined, lang: string): string {
  if (!slug) return "";
  const info = types?.find((d) => d.slug === slug);
  return info ? (lang.startsWith("ko") ? info.label_ko : info.label_en) : slug;
}

export function isLiteral(o: RuleOperand | null | undefined): boolean {
  return !!o && !o.doc_type && !o.field;
}

/** "세금계산서 · 공급자 사업자등록번호" / "'1,000,000'" */
export function operandLabel(o: RuleOperand | null | undefined, types: DocTypeInfo[] | undefined, schemas: SchemaRecord[] | undefined, lang: string): string {
  if (!o) return "";
  if (isLiteral(o)) return o.value != null ? `'${o.value}'` : "";
  const type = docTypeLabelOf(types, o.doc_type, lang);
  const field = fieldLabelOf(schemas, o.doc_type, o.field, lang);
  return [type, field].filter(Boolean).join(" · ");
}

/** Tolerance input → a finite, non-negative number (empty / invalid → null; negatives are clamped to 0). */
export function parseTolerance(raw: string): number | null {
  if (raw.trim() === "") return null;
  const n = Number(raw);
  return Number.isFinite(n) ? Math.max(0, n) : null;
}

export function ruleSummary(rule: Rule, types: DocTypeInfo[] | undefined, schemas: SchemaRecord[] | undefined, lang: string, t: TFunction): string {
  const left = operandLabel(rule.left, types, schemas, lang);
  const op = t(`projects.rules.ops.${rule.op}`);
  if (rule.op === "present") return `${left} ${op}`;
  const right = operandLabel(rule.right, types, schemas, lang);
  const tol = rule.op === "within_pct" ? ` ${rule.tolerance ?? 0}%` : rule.tolerance ? ` ±${rule.tolerance}` : "";
  return `${left} ${op} ${right}${tol}`.trim();
}

/** Whether the rule can be saved: a resolved left field and, except for `present`, a right field or literal. */
export function ruleComplete(rule: Rule): boolean {
  if (!rule.left.doc_type || !rule.left.field) return false;
  if (rule.op === "present") return true;
  if (!rule.right) return false;
  if (isLiteral(rule.right)) return rule.right.value != null && rule.right.value.trim() !== "";
  if (rule.op === "within_pct" && (rule.tolerance == null || !Number.isFinite(rule.tolerance) || rule.tolerance < 0)) return false;
  if (rule.tolerance != null && (!Number.isFinite(rule.tolerance) || rule.tolerance < 0)) return false;
  return !!rule.right.doc_type && !!rule.right.field;
}

/** Field references the known schemas cannot resolve (highlighted in the grid). */
export function unresolvedOperands(rule: Rule, schemas: SchemaRecord[] | undefined, types: DocTypeInfo[] | undefined): ("left" | "right")[] {
  const out: ("left" | "right")[] = [];
  const check = (o: RuleOperand | null, side: "left" | "right") => {
    if (!o || isLiteral(o)) return;
    const typeKnown = !!types?.some((d) => d.slug === o.doc_type) || !!schemas?.some((s) => s.doc_type === o.doc_type);
    const fieldKnown = fieldOptions(schemaForType(schemas, o.doc_type)).some((f) => f.path === o.field);
    if (!typeKnown || !fieldKnown) out.push(side);
  };
  check(rule.left, "left");
  if (rule.op !== "present") check(rule.right, "right");
  return out;
}

export function verdictVariant(v: Verdict): "ok" | "warn" | "danger" | "muted" {
  switch (v) {
    case "pass":
      return "ok";
    case "mismatch":
      return "danger";
    case "undetermined":
      return "warn";
    case "missing":
      return "muted";
  }
}

export function newRule(description = ""): Rule {
  return { id: crypto.randomUUID().replace(/-/g, ""), description, left: { doc_type: null, field: null, value: null }, op: "eq", right: { doc_type: null, field: null, value: null }, tolerance: null, required: true, enabled: true };
}

/**
 * Why moving a project to 완료 needs a confirmation: it was never checked, or the last check left
 * mismatches / missing documents. Null when the move needs no confirmation.
 */
export function doneMoveWarning(project: Pick<Project, "last_check_at" | "last_mismatch" | "missing_count">): "unchecked" | "findings" | null {
  if (!project.last_check_at) return "unchecked";
  if (project.last_mismatch > 0 || project.missing_count > 0) return "findings";
  return null;
}

/** Kanban column order (§3.15 접수 → 추출 → 검증 → 보완요청 → 완료). */
export const STATUS_ORDER = ["intake", "extracting", "validating", "supplement", "done"] as const;

export function nextStatus(status: (typeof STATUS_ORDER)[number], direction: 1 | -1): (typeof STATUS_ORDER)[number] | null {
  const i = STATUS_ORDER.indexOf(status);
  const n = i + direction;
  return n >= 0 && n < STATUS_ORDER.length ? STATUS_ORDER[n]! : null;
}
