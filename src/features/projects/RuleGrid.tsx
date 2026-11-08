import { AlertTriangle } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { useDocTypes, useSchemas } from "@/lib/queries";
import { RULE_OPS, type DocTypeInfo, type Rule, type RuleOp, type RuleOperand, type SchemaRecord } from "@/lib/types";
import { cn } from "@/lib/utils";

import { docTypeLabelOf, fieldOptions, isLiteral, operandLabel, ruleSummary, schemaForType, unresolvedOperands } from "./rules";

const NONE = "__none__";

function OperandEditor({ id, operand, onChange, docTypes, schemas, unresolved, readOnly, allowLiteral, literalPlaceholder }: { id: string; operand: RuleOperand; onChange: (o: RuleOperand) => void; docTypes: DocTypeInfo[]; schemas: SchemaRecord[]; unresolved: boolean; readOnly: boolean; allowLiteral: boolean; literalPlaceholder: string }) {
  const { t, i18n } = useTranslation();
  const literal = allowLiteral && isLiteral(operand);
  const fields = fieldOptions(schemaForType(schemas, operand.doc_type));
  const fieldKnown = !operand.field || fields.some((f) => f.path === operand.field);
  return (
    <div className={cn("flex flex-col gap-1 rounded-md border p-2", unresolved ? "border-danger bg-danger-soft/40" : "border-border")} data-testid={`${id}-operand`} data-unresolved={unresolved}>
      {allowLiteral && (
        <Select value={literal ? "literal" : "field"} onValueChange={(v) => onChange(v === "literal" ? { doc_type: null, field: null, value: operand.value ?? "" } : { doc_type: null, field: null, value: null })} disabled={readOnly}>
          <SelectTrigger className="h-7 text-xs" aria-label={t("projects.rules.right_mode")} data-testid={`${id}-mode`}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="field">{t("projects.rules.right_field")}</SelectItem>
            <SelectItem value="literal">{t("projects.rules.right_literal")}</SelectItem>
          </SelectContent>
        </Select>
      )}
      {literal ? (
        <Input className="h-7 text-xs" value={operand.value ?? ""} onChange={(e) => onChange({ doc_type: null, field: null, value: e.target.value })} placeholder={literalPlaceholder} aria-label={t("projects.rules.right_literal")} readOnly={readOnly} data-testid={`${id}-literal`} />
      ) : (
        <>
          <Select value={operand.doc_type ?? NONE} onValueChange={(v) => onChange({ doc_type: v === NONE ? null : v, field: null, value: null })} disabled={readOnly}>
            <SelectTrigger className="h-7 text-xs" aria-label={t("projects.rules.type")} data-testid={`${id}-type`}>
              <SelectValue placeholder={t("projects.rules.type_any")} />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE}>{t("projects.rules.type_any")}</SelectItem>
              {docTypes.map((d) => (
                <SelectItem key={d.slug} value={d.slug}>
                  {docTypeLabelOf(docTypes, d.slug, i18n.language)}
                </SelectItem>
              ))}
              {operand.doc_type && !docTypes.some((d) => d.slug === operand.doc_type) && <SelectItem value={operand.doc_type}>{operand.doc_type}</SelectItem>}
            </SelectContent>
          </Select>
          <Select value={operand.field ?? NONE} onValueChange={(v) => onChange({ ...operand, field: v === NONE ? null : v })} disabled={readOnly || !operand.doc_type}>
            <SelectTrigger className={cn("h-7 text-xs", !fieldKnown && "border-danger")} aria-label={t("projects.rules.field")} data-testid={`${id}-field`}>
              <SelectValue placeholder={t("projects.rules.field_any")} />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE}>{t("projects.rules.field_any")}</SelectItem>
              {fields.map((f) => (
                <SelectItem key={f.path} value={f.path}>
                  {i18n.language.startsWith("ko") ? f.label_ko : f.label_en}
                </SelectItem>
              ))}
              {operand.field && !fieldKnown && <SelectItem value={operand.field}>{operand.field}</SelectItem>}
            </SelectContent>
          </Select>
        </>
      )}
    </div>
  );
}

/**
 * The rule grid (design §3.15 "자연어 입력 → 규칙 그리드"): left type / field, operator, right type / field or
 * literal, tolerance and the required flag. Operands the schemas cannot resolve are highlighted.
 */
export function RuleGrid({ rule, onChange, readOnly = false, unresolvedNames = [], testId = "rule-grid" }: { rule: Rule; onChange?: (rule: Rule) => void; readOnly?: boolean; unresolvedNames?: string[]; testId?: string }) {
  const { t } = useTranslation();
  const { data: docTypes } = useDocTypes();
  const { data: schemas } = useSchemas();
  const unresolved = unresolvedOperands(rule, schemas, docTypes);
  const change = (patch: Partial<Rule>) => onChange?.({ ...rule, ...patch });
  const needsRight = rule.op !== "present";
  return (
    <div className="flex flex-col gap-2 rounded-md border border-border bg-bg p-3 text-sm" data-testid={testId}>
      <div className="flex flex-col gap-1">
        <label htmlFor={`${testId}-description`} className="text-xs text-fg-muted">
          {t("projects.rules.description")}
        </label>
        <Input id={`${testId}-description`} className="h-8" value={rule.description} onChange={(e) => change({ description: e.target.value })} readOnly={readOnly} data-testid={`${testId}-description`} />
      </div>
      <div className="grid gap-2 md:grid-cols-[1fr_auto_1fr]">
        <div className="flex flex-col gap-1">
          <span className="text-xs text-fg-muted">{t("projects.rules.left")}</span>
          <OperandEditor id={`${testId}-left`} operand={rule.left} onChange={(left) => change({ left })} docTypes={docTypes ?? []} schemas={schemas ?? []} unresolved={unresolved.includes("left")} readOnly={readOnly} allowLiteral={false} literalPlaceholder="" />
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={`${testId}-op`} className="text-xs text-fg-muted">
            {t("projects.rules.op")}
          </label>
          <Select value={rule.op} onValueChange={(v) => change({ op: v as RuleOp, right: v === "present" ? null : (rule.right ?? { doc_type: null, field: null, value: null }) })} disabled={readOnly}>
            <SelectTrigger id={`${testId}-op`} className="h-8 w-40 text-xs" aria-label={t("projects.rules.op")} data-testid={`${testId}-op`}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {RULE_OPS.map((op) => (
                <SelectItem key={op} value={op}>
                  {t(`projects.rules.ops.${op}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {(rule.op === "within_pct" || rule.op === "eq") && (
            <>
              <label htmlFor={`${testId}-tolerance`} className="text-xs text-fg-muted">
                {rule.op === "within_pct" ? t("projects.rules.tolerance_pct") : t("projects.rules.tolerance")}
              </label>
              <Input id={`${testId}-tolerance`} type="number" min={0} step="any" className="h-8 w-40 text-xs" value={rule.tolerance ?? ""} onChange={(e) => change({ tolerance: e.target.value === "" ? null : Number(e.target.value) })} readOnly={readOnly} data-testid={`${testId}-tolerance`} />
            </>
          )}
        </div>
        <div className="flex flex-col gap-1">
          <span className="text-xs text-fg-muted">{t("projects.rules.right")}</span>
          {needsRight ? (
            <OperandEditor id={`${testId}-right`} operand={rule.right ?? { doc_type: null, field: null, value: null }} onChange={(right) => change({ right })} docTypes={docTypes ?? []} schemas={schemas ?? []} unresolved={unresolved.includes("right")} readOnly={readOnly} allowLiteral literalPlaceholder={t("projects.rules.literal_placeholder")} />
          ) : (
            <span className="rounded-md border border-dashed border-border p-2 text-xs text-fg-subtle">—</span>
          )}
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-3 text-xs">
        <label className="flex items-center gap-2">
          <Switch checked={rule.required} onCheckedChange={(v) => change({ required: v })} disabled={readOnly} aria-label={t("projects.rules.required")} data-testid={`${testId}-required`} />
          {rule.required ? t("projects.rules.required") : t("projects.rules.optional")}
        </label>
        {unresolvedNames.length > 0 && (
          <span className="flex items-center gap-1 text-danger" data-testid={`${testId}-unresolved`}>
            <AlertTriangle className="size-3.5" aria-hidden="true" />
            {t("projects.rules.unresolved", { names: unresolvedNames.join(", ") })}
          </span>
        )}
      </div>
    </div>
  );
}

/** One-line rendering of a rule (lists, plan cards). */
export function RuleSummary({ rule, className }: { rule: Rule; className?: string }) {
  const { t, i18n } = useTranslation();
  const { data: docTypes } = useDocTypes();
  const { data: schemas } = useSchemas();
  const unresolved = unresolvedOperands(rule, schemas, docTypes);
  return (
    <span className={cn("flex flex-wrap items-center gap-1 text-xs", className)} data-testid="rule-summary">
      <span className={cn("font-mono", unresolved.length > 0 && "text-danger")}>{ruleSummary(rule, docTypes, schemas, i18n.language, t)}</span>
      {rule.right && isLiteral(rule.right) && rule.op !== "present" && <span className="sr-only">{operandLabel(rule.right, docTypes, schemas, i18n.language)}</span>}
      {!rule.required && <Badge variant="muted">{t("projects.rules.optional")}</Badge>}
      {unresolved.length > 0 && <Badge variant="danger">{t("projects.rules.issues")}</Badge>}
    </span>
  );
}
