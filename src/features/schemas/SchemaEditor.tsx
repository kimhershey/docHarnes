import { ArrowDown, ArrowUp, Plus, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input, Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { api } from "@/lib/ipc";
import { FIELD_KINDS, SCALAR_FIELD_KINDS, newFieldDef, type FieldDef, type FieldKind, type LintIssue, type SchemaDraft } from "@/lib/types";

/** Key suggestion while the user types a Korean label (mirrors pipeline::schema::key_from_label loosely). */
function suggestKey(label: string, fallback: string): string {
  const ascii = label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");
  if (/^[a-z][a-z0-9_]{0,39}$/.test(ascii)) return ascii;
  return fallback;
}

function FieldRow({ field, index, count, nested, onChange, onRemove, onMove }: { field: FieldDef; index: number; count: number; nested: boolean; onChange: (f: FieldDef) => void; onRemove: () => void; onMove: (delta: number) => void }) {
  const { t } = useTranslation();
  const kinds = nested ? SCALAR_FIELD_KINDS : FIELD_KINDS;
  const set = (patch: Partial<FieldDef>) => onChange({ ...field, ...patch });
  const setChild = (i: number, c: FieldDef) => set({ fields: field.fields.map((x, k) => (k === i ? c : x)) });
  return (
    <div className="rounded-md border border-border bg-bg p-2" data-testid={nested ? "schema-column" : "schema-field"} data-field-key={field.key}>
      <div className="grid grid-cols-[1fr_1fr_1fr_120px_auto] items-end gap-2">
        <div>
          <Label className="text-xs">{t("schemas.editor.key")}</Label>
          <Input value={field.key} onChange={(e) => set({ key: e.target.value })} className="h-8 font-mono text-xs" aria-label={t("schemas.editor.key")} data-testid="schema-field-key" />
        </div>
        <div>
          <Label className="text-xs">{t("schemas.editor.label_ko")}</Label>
          <Input
            value={field.label_ko}
            onChange={(e) => set({ label_ko: e.target.value, key: field.key || suggestKey(e.target.value, "") })}
            className="h-8"
            aria-label={t("schemas.editor.label_ko")}
            data-testid="schema-field-label-ko"
          />
        </div>
        <div>
          <Label className="text-xs">{t("schemas.editor.label_en")}</Label>
          <Input value={field.label_en} onChange={(e) => set({ label_en: e.target.value, key: field.key || suggestKey(e.target.value, "") })} className="h-8" aria-label={t("schemas.editor.label_en")} />
        </div>
        <div>
          <Label className="text-xs">{t("schemas.editor.kind")}</Label>
          <Select value={field.kind} onValueChange={(v) => set({ kind: v as FieldKind, fields: v === "table" || v === "group" ? (field.fields.length ? field.fields : [newFieldDef("value")]) : [] })}>
            <SelectTrigger className="h-8" aria-label={t("schemas.editor.kind")} data-testid="schema-field-kind">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {kinds.map((k) => (
                <SelectItem key={k} value={k}>
                  {t(`pipeline.kinds.${k}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="flex items-center gap-1 pb-1">
          <label className="flex items-center gap-1 text-xs">
            <Switch checked={field.required} onCheckedChange={(v) => set({ required: v })} aria-label={t("schemas.editor.required")} />
            {t("schemas.editor.required")}
          </label>
          <Button size="icon-sm" variant="ghost" onClick={() => onMove(-1)} disabled={index === 0} aria-label={t("schemas.editor.up")}>
            <ArrowUp />
          </Button>
          <Button size="icon-sm" variant="ghost" onClick={() => onMove(1)} disabled={index >= count - 1} aria-label={t("schemas.editor.down")}>
            <ArrowDown />
          </Button>
          <Button size="icon-sm" variant="ghost" className="text-danger" onClick={onRemove} aria-label={t("schemas.editor.remove")} data-testid="schema-field-remove">
            <Trash2 />
          </Button>
        </div>
      </div>
      {field.kind !== "table" && field.kind !== "group" && (
        <div className="mt-2 grid grid-cols-[1fr_1fr_1fr_auto] items-end gap-2">
          <div>
            <Label className="text-xs">{t("schemas.editor.regex")}</Label>
            <Input value={field.regex_hint ?? ""} onChange={(e) => set({ regex_hint: e.target.value || null })} className="h-7 font-mono text-xs" aria-label={t("schemas.editor.regex")} />
          </div>
          <div>
            <Label className="text-xs">{t("schemas.editor.example")}</Label>
            <Input value={field.example ?? ""} onChange={(e) => set({ example: e.target.value || null })} className="h-7 text-xs" aria-label={t("schemas.editor.example")} />
          </div>
          <div>
            <Label className="text-xs">{t("schemas.editor.options")}</Label>
            <Input
              value={field.options.join(", ")}
              onChange={(e) => set({ options: e.target.value.split(/[,，]/).map((s) => s.trim()).filter(Boolean) })}
              disabled={field.kind !== "enum"}
              className="h-7 text-xs"
              aria-label={t("schemas.editor.options")}
            />
          </div>
          <label className="flex items-center gap-1 pb-1 text-xs">
            <Switch checked={field.mask} onCheckedChange={(v) => set({ mask: v })} aria-label={t("schemas.editor.mask")} />
            {t("schemas.editor.mask")}
          </label>
        </div>
      )}
      {(field.kind === "table" || field.kind === "group") && (
        <div className="mt-2 flex flex-col gap-1 border-l-2 border-accent pl-2">
          <div className="flex items-center justify-between">
            <span className="text-xs font-medium text-fg-muted">{t("schemas.editor.columns")}</span>
            <Button size="sm" variant="ghost" onClick={() => set({ fields: [...field.fields, newFieldDef(`col_${field.fields.length + 1}`)] })} data-testid="schema-add-column">
              <Plus />
              {t("schemas.editor.add_column")}
            </Button>
          </div>
          {field.fields.map((c, i) => (
            <FieldRow
              key={i}
              field={c}
              index={i}
              count={field.fields.length}
              nested
              onChange={(nc) => setChild(i, nc)}
              onRemove={() => set({ fields: field.fields.filter((_, k) => k !== i) })}
              onMove={(d) => {
                const j = i + d;
                if (j < 0 || j >= field.fields.length) return;
                const next = [...field.fields];
                [next[i], next[j]] = [next[j]!, next[i]!];
                set({ fields: next });
              }}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * Field grid of a schema draft (design §3.7.2 "필드 그리드: key/라벨/타입/필수/정규식/예시")
 * with the grammar lint shown live (debounced through the backend's lint command).
 */
export function SchemaEditor({ draft, onChange, lintTick = 400 }: { draft: SchemaDraft; onChange: (d: SchemaDraft) => void; lintTick?: number }) {
  const { t } = useTranslation();
  const [issues, setIssues] = useState<LintIssue[]>([]);
  const def = draft.definition;
  const set = (patch: Partial<SchemaDraft>) => onChange({ ...draft, ...patch });
  const setFields = (fields: FieldDef[]) => set({ definition: { ...def, fields } });

  useEffect(() => {
    const h = setTimeout(() => {
      void api
        .pipelineSchemaLint(def)
        .then(setIssues)
        .catch(() => setIssues([]));
    }, lintTick);
    return () => clearTimeout(h);
  }, [def, lintTick]);

  const errors = useMemo(() => issues.filter((i) => i.level === "error"), [issues]);
  const warnings = useMemo(() => issues.filter((i) => i.level === "warn"), [issues]);

  return (
    <div className="flex flex-col gap-3" data-testid="schema-editor">
      <div className="grid grid-cols-2 gap-2 md:grid-cols-3">
        <div>
          <Label htmlFor="schema-name">{t("schemas.editor.name")}</Label>
          <Input id="schema-name" value={draft.name} onChange={(e) => set({ name: e.target.value })} data-testid="schema-name" />
        </div>
        <div>
          <Label htmlFor="schema-name-en">{t("schemas.editor.name_en")}</Label>
          <Input id="schema-name-en" value={draft.name_en ?? ""} onChange={(e) => set({ name_en: e.target.value || null })} />
        </div>
        <div>
          <Label htmlFor="schema-doc-type">{t("schemas.editor.doc_type")}</Label>
          <Input id="schema-doc-type" value={draft.doc_type} onChange={(e) => set({ doc_type: e.target.value.trim() })} className="font-mono" data-testid="schema-doc-type" />
          <p className="text-xs text-fg-subtle">{t("schemas.editor.doc_type_hint")}</p>
        </div>
      </div>
      <div>
        <Label htmlFor="schema-desc">{t("schemas.editor.description")}</Label>
        <Textarea id="schema-desc" rows={2} value={draft.description_ko ?? ""} onChange={(e) => set({ description_ko: e.target.value || null })} />
      </div>
      <div className="flex items-center justify-between">
        <span className="font-medium">{t("schemas.editor.fields")}</span>
        <Button size="sm" variant="secondary" onClick={() => setFields([...def.fields, newFieldDef(`field_${def.fields.length + 1}`)])} data-testid="schema-add-field">
          <Plus />
          {t("schemas.editor.add_field")}
        </Button>
      </div>
      <div className="flex flex-col gap-2" data-testid="schema-fields">
        {def.fields.map((f, i) => (
          <FieldRow
            key={i}
            field={f}
            index={i}
            count={def.fields.length}
            nested={false}
            onChange={(nf) => setFields(def.fields.map((x, k) => (k === i ? nf : x)))}
            onRemove={() => setFields(def.fields.filter((_, k) => k !== i))}
            onMove={(d) => {
              const j = i + d;
              if (j < 0 || j >= def.fields.length) return;
              const next = [...def.fields];
              [next[i], next[j]] = [next[j]!, next[i]!];
              setFields(next);
            }}
          />
        ))}
      </div>
      <div className="flex flex-wrap items-center gap-2 text-xs" role="status" data-testid="schema-lint">
        {errors.length === 0 && warnings.length === 0 && <Badge variant="ok">{t("schemas.editor.lint_ok")}</Badge>}
        {errors.length > 0 && <Badge variant="danger">{t("schemas.editor.lint_errors", { count: errors.length })}</Badge>}
        {warnings.length > 0 && <Badge variant="warn">{t("schemas.editor.lint_warnings", { count: warnings.length })}</Badge>}
        {issues.slice(0, 6).map((i, k) => (
          <span key={k} className={i.level === "error" ? "text-danger" : "text-warn"}>
            {i.path ? `${i.path}: ` : ""}
            {i.message}
          </span>
        ))}
      </div>
    </div>
  );
}
