import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import type { Op, Plan, PlanContext, Scope } from "@/lib/types";

import { docTypeName, opLabel, replaceOp, withScope } from "./describePlan";
import { coerceInteger, coerceIntList, coerceNumber } from "./numeric";
import { OP_SPECS, paramDesc, type ParamSpec } from "./opSpecs";

export { coerceInteger, coerceIntList, coerceNumber };

const ANY = "__any__";

function parseList(value: string): string[] {
  return value
    .split(/[,，\n]/)
    .map((s) => s.trim())
    .filter(Boolean);
}

function parseScalar(value: string): string | number | boolean {
  const v = value.trim();
  if (/^(true|false)$/i.test(v)) return v.toLowerCase() === "true";
  if (/^-?\d+(\.\d+)?$/.test(v)) return Number(v);
  return value;
}

/** Enum values get a translated label when the app has one (themes, languages, profiles …). */
function enumLabel(param: ParamSpec, value: string, t: (k: string, o?: Record<string, unknown>) => string): string {
  switch (param.name) {
    case "theme":
      return t(`theme.${value}`);
    case "lang":
      return t(`language.${value}`);
    case "profile":
      return t(`models.profiles.${value}`);
    case "fn":
      return t(`planner.fn.${value}`);
    case "mode":
      return ["hybrid", "keyword", "semantic"].includes(value) ? t(`search.mode.${value}`) : value;
    case "include":
      return ["evidence", "confidence", "status"].includes(value) ? t(`export.columns.${value}`) : value;
    default:
      return value;
  }
}

function ParamField({ index, param, value, onChange }: { index: number; param: ParamSpec; value: unknown; onChange: (value: unknown) => void }) {
  const { t, i18n } = useTranslation();
  const id = `plan-param-${index}-${param.name}`;
  const label = paramDesc(param, i18n.language);
  const testId = `plan-editor-param-${index}-${param.name}`;
  const type = param.type;
  let control: ReactNode;
  switch (type.kind) {
    case "boolean":
      control = param.required ? (
        <Switch id={id} checked={value === true} onCheckedChange={(c) => onChange(c)} aria-label={label} data-testid={testId} />
      ) : (
        <Select value={value === true ? "true" : value === false ? "false" : ANY} onValueChange={(v) => onChange(v === ANY ? undefined : v === "true")}>
          <SelectTrigger id={id} className="h-8" aria-label={label} data-testid={testId}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ANY}>{t("planner.editor.param_default")}</SelectItem>
            <SelectItem value="true">{t("common.yes")}</SelectItem>
            <SelectItem value="false">{t("common.no")}</SelectItem>
          </SelectContent>
        </Select>
      );
      break;
    case "enum":
      control = (
        <Select value={typeof value === "string" ? value : ANY} onValueChange={(v) => onChange(v === ANY ? undefined : v)}>
          <SelectTrigger id={id} className="h-8" aria-label={label} data-testid={testId}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {!param.required && <SelectItem value={ANY}>{t("planner.editor.param_default")}</SelectItem>}
            {type.values.map((v) => (
              <SelectItem key={v} value={v}>
                {enumLabel(param, v, t)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      );
      break;
    case "enum_list": {
      const selected = Array.isArray(value) ? (value as string[]) : [];
      control = (
        <div className="flex flex-wrap gap-3" role="group" aria-labelledby={`${id}-label`} data-testid={testId}>
          {type.values.map((v) => (
            <label key={v} className="flex items-center gap-1 text-xs">
              <input
                type="checkbox"
                checked={selected.includes(v)}
                onChange={(e) => {
                  const next = e.target.checked ? [...selected, v] : selected.filter((x) => x !== v);
                  onChange(next.length > 0 ? next : undefined);
                }}
              />
              {enumLabel(param, v, t)}
            </label>
          ))}
        </div>
      );
      break;
    }
    case "integer":
    case "number":
      // Rust `u32` fields (top_k, pages): fractions are truncated, NaN / negatives rejected (review M4).
      control = (
        <Input
          id={id}
          type="number"
          step={type.kind === "integer" ? 1 : "any"}
          min={type.kind === "integer" ? 0 : undefined}
          className="h-8"
          value={typeof value === "number" ? String(value) : ""}
          onChange={(e) => {
            const v = type.kind === "integer" ? coerceInteger(e.target.value) : coerceNumber(e.target.value);
            if (v !== null) onChange(v);
          }}
          aria-label={label}
          data-testid={testId}
        />
      );
      break;
    case "date":
      control = <Input id={id} type="date" className="h-8" value={typeof value === "string" ? value : ""} onChange={(e) => onChange(e.target.value || undefined)} aria-label={label} data-testid={testId} />;
      break;
    case "list":
    case "int_list":
      control = (
        <Input
          id={id}
          className="h-8"
          value={Array.isArray(value) ? value.join(", ") : ""}
          placeholder={t("planner.editor.list_hint")}
          onChange={(e) => {
            if (type.kind === "int_list") return onChange(coerceIntList(e.target.value));
            const items = parseList(e.target.value);
            onChange(items.length > 0 ? items : undefined);
          }}
          aria-label={label}
          data-testid={testId}
        />
      );
      break;
    case "scalar":
      control = <Input id={id} className="h-8" value={value === undefined || value === null ? "" : String(value)} placeholder={t("planner.editor.scalar_hint")} onChange={(e) => onChange(e.target.value === "" ? undefined : parseScalar(e.target.value))} aria-label={label} data-testid={testId} />;
      break;
    case "string":
    default:
      control = <Input id={id} className="h-8" value={typeof value === "string" ? value : ""} onChange={(e) => onChange(e.target.value === "" ? (param.required ? "" : undefined) : e.target.value)} aria-label={label} data-testid={testId} />;
      break;
  }
  // An enum list is a group of checkboxes, not a single control: label it by id instead of `htmlFor`.
  const LabelTag = type.kind === "enum_list" ? "span" : "label";
  // (`minmax(0,1fr)`: the control column may be narrower than a long select label, which is then cut inside the control.)
  return (
    <div className="grid grid-cols-[minmax(8rem,14rem)_minmax(0,1fr)] items-center gap-2 text-xs">
      <LabelTag id={`${id}-label`} htmlFor={type.kind === "enum_list" ? undefined : id} className="text-fg-muted">
        {label}
        {param.required && <span className="text-danger"> *</span>}
      </LabelTag>
      {control}
    </div>
  );
}

/**
 * Generic inline editor of a plan (design §3.5 "각 단계 인라인 편집"): one
 * field per parameter of every op, driven by the `OP_SPECS` mirror, plus the
 * scope (type, dates, folder). Edits stay local; execution re-validates.
 */
export function PlanEditor({ plan, context, onChange }: { plan: Plan; context: PlanContext; onChange: (plan: Plan) => void }) {
  const { t, i18n } = useTranslation();
  const filter = plan.scope.filter ?? {};
  const setFilter = (patch: Record<string, unknown>) => {
    const next: Scope = { ...plan.scope, filter: { ...filter, ...patch } };
    for (const [k, v] of Object.entries(next.filter ?? {})) if (v === undefined || v === null || v === "") delete (next.filter as Record<string, unknown>)[k];
    onChange(withScope(plan, next));
  };
  const setFolder = (folder: string | null) => {
    const next: Scope = { ...plan.scope };
    if (folder) next.folder = folder;
    else delete next.folder;
    onChange(withScope(plan, next));
  };
  const setParam = (index: number, name: string, value: unknown) => {
    const op = { ...(plan.operations[index] as unknown as Record<string, unknown>) };
    if (value === undefined) delete op[name];
    else op[name] = value;
    onChange(replaceOp(plan, index, op as unknown as Op));
  };
  const folders = [...context.folders];
  if (plan.scope.folder && !folders.includes(plan.scope.folder)) folders.unshift(plan.scope.folder);

  return (
    <div className="flex flex-col gap-3 rounded-md border border-border bg-bg p-3" data-testid="plan-editor">
      <fieldset className="flex flex-col gap-2">
        <legend className="text-xs font-medium text-fg-muted">{t("planner.scope.title")}</legend>
        <div className="grid grid-cols-[minmax(8rem,14rem)_minmax(0,1fr)] items-center gap-2 text-xs">
          <label htmlFor="plan-scope-doc-type" className="text-fg-muted">
            {t("planner.scope.doc_type")}
          </label>
          <Select value={filter.doc_type ?? ANY} onValueChange={(v) => setFilter({ doc_type: v === ANY ? undefined : v })}>
            <SelectTrigger id="plan-scope-doc-type" className="h-8" aria-label={t("planner.scope.doc_type")} data-testid="plan-editor-scope-doc-type">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ANY}>{t("planner.scope.any")}</SelectItem>
              {context.doc_types.map((d) => (
                <SelectItem key={d.slug} value={d.slug}>
                  {docTypeName(d.slug, context, i18n.language)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <label htmlFor="plan-scope-from" className="text-fg-muted">
            {t("planner.scope.date_from")}
          </label>
          <Input id="plan-scope-from" type="date" className="h-8" value={filter.date_from ?? ""} onChange={(e) => setFilter({ date_from: e.target.value || undefined })} data-testid="plan-editor-scope-from" />
          <label htmlFor="plan-scope-to" className="text-fg-muted">
            {t("planner.scope.date_to")}
          </label>
          <Input id="plan-scope-to" type="date" className="h-8" value={filter.date_to ?? ""} onChange={(e) => setFilter({ date_to: e.target.value || undefined })} data-testid="plan-editor-scope-to" />
          <label htmlFor="plan-scope-folder" className="text-fg-muted">
            {t("planner.scope.folder")}
          </label>
          <Select value={plan.scope.folder ?? ANY} onValueChange={(v) => setFolder(v === ANY ? null : v)}>
            <SelectTrigger id="plan-scope-folder" className="h-8" aria-label={t("planner.scope.folder")} data-testid="plan-editor-scope-folder">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ANY}>{t("planner.scope.any")}</SelectItem>
              {folders.map((f) => (
                <SelectItem key={f} value={f}>
                  {f}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {plan.scope.doc_ids && plan.scope.doc_ids.length > 0 && (
            <>
              <span className="text-fg-muted">{t("planner.scope.documents")}</span>
              <span>{t("planner.scope.doc_ids", { count: plan.scope.doc_ids.length })}</span>
            </>
          )}
        </div>
      </fieldset>
      {plan.operations.map((op, i) => {
        const spec = OP_SPECS[op.op];
        if (!spec || spec.params.length === 0) return null;
        const record = op as unknown as Record<string, unknown>;
        return (
          <fieldset key={i} className="flex flex-col gap-2 border-t border-border pt-2" data-testid={`plan-editor-op-${i}`}>
            <legend className="text-xs font-medium">{t("planner.editor.step", { index: i + 1, op: opLabel(op.op, t) })}</legend>
            {spec.params.map((param) => (
              <ParamField key={param.name} index={i} param={param} value={record[param.name]} onChange={(v) => setParam(i, param.name, v)} />
            ))}
          </fieldset>
        );
      })}
    </div>
  );
}
