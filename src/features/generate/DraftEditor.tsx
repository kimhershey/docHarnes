import { AlertTriangle, Check, ExternalLink, Plus, Trash2, Wand2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input, Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { Draft, DraftIssue } from "@/lib/types";
import { cn } from "@/lib/utils";

import { addRow, applySuggestion, blockText, blockWithText, fieldKindLabel, issuesFor, removeRow, setBlock, setCell, setFieldValue, setTitle } from "./draft";

function IssueBadges({ issues, onApply, t }: { issues: DraftIssue[]; onApply: (i: DraftIssue) => void; t: (k: string, o?: Record<string, unknown>) => string }) {
  if (issues.length === 0) return null;
  return (
    <div className="flex flex-wrap items-center gap-1">
      {issues.map((i, n) => (
        <span key={n} className="flex items-center gap-1" data-testid={`generate-issue-${i.code}`}>
          <Badge variant={i.blocking ? "danger" : "warn"} title={i.message}>
            {t(`generate.issue_codes.${i.code}`, { defaultValue: i.code })}
          </Badge>
          {i.suggestion != null && (
            <Button size="sm" variant="outline" className="h-6 px-2 text-[11px]" onClick={() => onApply(i)} data-testid="generate-apply-suggestion">
              <Wand2 />
              {t("generate.editor.apply_suggestion")} ({i.suggestion})
            </Button>
          )}
        </span>
      ))}
    </div>
  );
}

/**
 * Right column of 문서 생성: title, the fields grid (required marks, issue badges with "제안 적용"), item
 * tables, body blocks, the 참고 문서 panel (exclude → regenerate) and the validation issue list. `disabled`
 * (a draft job is running) locks every control: the job's result replaces the draft, so edits would be lost.
 */
export function DraftEditor({ draft, onChange, excluded, onToggleExcluded, disabled = false }: { draft: Draft; onChange: (draft: Draft) => void; excluded: string[]; onToggleExcluded: (docId: string) => void; disabled?: boolean }) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const apply = (issue: DraftIssue) => onChange(applySuggestion(draft, issue));
  return (
    <fieldset disabled={disabled} aria-busy={disabled || undefined} className={cn("flex min-w-0 flex-col gap-4 text-sm", disabled && "opacity-70")} data-testid="generate-editor" data-disabled={disabled}>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="gen-title">{t("generate.editor.doc_title")}</Label>
        <Input id="gen-title" value={draft.title} onChange={(e) => onChange(setTitle(draft, e.target.value))} data-testid="generate-title" />
      </div>

      <section className="flex flex-col gap-2" aria-labelledby="gen-fields-title">
        <h3 id="gen-fields-title" className="text-xs font-semibold uppercase tracking-wide text-fg-subtle">
          {t("generate.editor.fields")}
        </h3>
        <div className="flex flex-col gap-2" data-testid="generate-fields">
          {draft.fields.map((f) => {
            const issues = issuesFor(draft, `fields.${f.key}`);
            const id = `gen-field-${f.key}`;
            return (
              <div key={f.key} className="flex flex-col gap-1" data-testid={`generate-field-${f.key}`} data-field-issues={issues.length}>
                <div className="flex items-center gap-2">
                  <Label htmlFor={id} className="flex-1 truncate">
                    {f.label}
                    {f.required && (
                      <span className="text-danger" aria-label={t("schemas.editor.required")}>
                        {" "}
                        *
                      </span>
                    )}
                  </Label>
                  <span className="text-[10px] text-fg-subtle">{fieldKindLabel(f.kind, i18n.language)}</span>
                  {f.user_edited && <Badge variant="info">{t("generate.editor.user_edited")}</Badge>}
                  {f.source && (
                    <button type="button" className="text-[10px] text-accent hover:underline cursor-pointer" onClick={() => f.source?.doc_id && navigate(`/library/${f.source.doc_id}${f.source.page ? `?page=${f.source.page}` : ""}`)} title={f.source.title} data-testid="generate-field-source">
                      {t("generate.editor.source_badge")}
                    </button>
                  )}
                </div>
                <Input id={id} value={f.value} onChange={(e) => onChange(setFieldValue(draft, f.key, e.target.value))} className={cn("h-8", issues.some((i) => i.blocking) && "border-danger")} aria-invalid={issues.some((i) => i.blocking) || undefined} data-testid={`generate-field-input-${f.key}`} />
                <IssueBadges issues={issues} onApply={apply} t={t} />
              </div>
            );
          })}
        </div>
      </section>

      {draft.tables.map((table) => (
        <section key={table.key} className="flex flex-col gap-2" aria-label={table.label} data-testid={`generate-table-${table.key}`}>
          <div className="flex items-center justify-between">
            <h3 className="text-xs font-semibold uppercase tracking-wide text-fg-subtle">
              {t("generate.editor.tables")} · {table.label}
            </h3>
            <Button size="sm" variant="outline" onClick={() => onChange(addRow(draft, table.key))} data-testid={`generate-add-row-${table.key}`}>
              <Plus />
              {t("generate.editor.add_row")}
            </Button>
          </div>
          {table.rows.length === 0 ? (
            <p className="text-xs text-fg-subtle">{t("generate.editor.no_rows")}</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-xs">
                <caption className="sr-only">{table.label}</caption>
                <thead>
                  <tr className="text-left text-fg-subtle">
                    {table.columns.map((c) => (
                      <th key={c.key} scope="col" className="px-1 py-1 font-medium">
                        {c.label}
                      </th>
                    ))}
                    <th scope="col" className="sr-only">
                      {t("generate.editor.remove_row")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {table.rows.map((row, ri) => (
                    <tr key={ri} data-testid={`generate-row-${table.key}-${ri}`}>
                      {table.columns.map((c, ci) => {
                        const issues = issuesFor(draft, `tables.${table.key}[${ri}].${c.key}`);
                        return (
                          <td key={c.key} className="px-1 py-0.5 align-top">
                            <Input value={row[ci] ?? ""} onChange={(e) => onChange(setCell(draft, table.key, ri, ci, e.target.value))} className={cn("h-7 min-w-20 px-1.5 text-xs", (c.kind === "amount" || c.kind === "integer") && "text-right tabular-nums", issues.length > 0 && "border-danger")} aria-label={`${t("generate.editor.row", { index: ri + 1 })} ${c.label}`} data-testid={`generate-cell-${table.key}-${ri}-${c.key}`} />
                            <IssueBadges issues={issues} onApply={apply} t={t} />
                          </td>
                        );
                      })}
                      <td className="px-1 py-0.5 align-top">
                        <Button size="icon-sm" variant="ghost" onClick={() => onChange(removeRow(draft, table.key, ri))} aria-label={`${t("generate.editor.remove_row")} ${ri + 1}`} data-testid={`generate-remove-row-${table.key}-${ri}`}>
                          <Trash2 />
                        </Button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>
      ))}

      <section className="flex flex-col gap-2" aria-labelledby="gen-body-title">
        <h3 id="gen-body-title" className="text-xs font-semibold uppercase tracking-wide text-fg-subtle">
          {t("generate.editor.body")}
        </h3>
        {draft.body.map((b, i) => {
          const issues = issuesFor(draft, `body[${i}]`);
          if (b.kind === "table_ref") {
            return (
              <p key={i} className="rounded-md border border-dashed border-border px-2 py-1 text-xs text-fg-subtle" data-testid={`generate-block-${i}`}>
                {t("generate.editor.table_ref", { key: draft.tables.find((x) => x.key === b.key)?.label ?? b.key })}
              </p>
            );
          }
          const label = b.kind === "heading" ? t("generate.editor.heading", { level: b.level }) : b.kind === "list" ? t("generate.editor.list") : t("generate.editor.paragraph");
          return (
            <div key={i} className="flex flex-col gap-1" data-testid={`generate-block-${i}`}>
              <div className="flex items-center gap-2 text-[11px] text-fg-subtle">
                <span>{label}</span>
                {b.kind === "paragraph" && b.source && (
                  <button type="button" className="text-accent hover:underline cursor-pointer" onClick={() => b.source?.doc_id && navigate(`/library/${b.source.doc_id}${b.source.page ? `?page=${b.source.page}` : ""}`)} title={b.source.title}>
                    {t("generate.editor.source_badge")}
                  </button>
                )}
              </div>
              <Textarea rows={b.kind === "heading" ? 1 : 3} value={blockText(b)} onChange={(e) => onChange(setBlock(draft, i, blockWithText(b, e.target.value)))} className={cn("text-xs", b.kind === "heading" && "font-semibold")} aria-label={label} lang={i18n.language} data-testid={`generate-block-input-${i}`} />
              <IssueBadges issues={issues} onApply={apply} t={t} />
            </div>
          );
        })}
      </section>

      <section className="flex flex-col gap-2" aria-labelledby="gen-refs-title" data-testid="generate-references">
        <h3 id="gen-refs-title" className="text-xs font-semibold uppercase tracking-wide text-fg-subtle">
          {t("generate.editor.references")}
        </h3>
        {draft.references.length === 0 ? (
          <p className="text-xs text-fg-subtle">{t("generate.editor.no_references")}</p>
        ) : (
          <ul className="flex flex-col gap-1">
            {draft.references.map((r, i) => {
              const isExcluded = !!r.doc_id && excluded.includes(r.doc_id);
              return (
                <li key={i} className={cn("flex items-start gap-2 rounded-md border border-border px-2 py-1.5 text-xs", isExcluded && "opacity-60")} data-testid="generate-reference" data-excluded={isExcluded}>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-1 font-medium">
                      <span className="truncate">{r.title}</span>
                      {r.page && <span className="text-fg-subtle">p.{r.page}</span>}
                      <Badge variant="muted" className="text-[10px]">
                        {t(`generate.reference_source.${r.source}`, { defaultValue: r.source })}
                      </Badge>
                    </div>
                    {r.excerpt && <div className="truncate text-fg-muted selectable">{r.excerpt}</div>}
                  </div>
                  {r.doc_id && (
                    <Button size="icon-sm" variant="ghost" onClick={() => navigate(`/library/${r.doc_id}${r.page ? `?page=${r.page}` : ""}`)} aria-label={t("generate.editor.open_source", { title: r.title })}>
                      <ExternalLink />
                    </Button>
                  )}
                  {r.doc_id && (
                    <label className="flex items-center gap-1 text-[11px] text-fg-muted">
                      <input type="checkbox" checked={isExcluded} onChange={() => onToggleExcluded(r.doc_id!)} data-testid="generate-reference-exclude" />
                      {t("generate.editor.exclude")}
                    </label>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section className="flex flex-col gap-1" aria-labelledby="gen-issues-title" data-testid="generate-issues">
        <h3 id="gen-issues-title" className="text-xs font-semibold uppercase tracking-wide text-fg-subtle">
          {t("generate.editor.issues")}
        </h3>
        {draft.issues.length === 0 ? (
          <p className="flex items-center gap-1 text-xs text-ok" data-testid="generate-issues-ok">
            <Check className="size-3.5" aria-hidden="true" />
            {t("generate.editor.issues_ok")}
          </p>
        ) : (
          <ul className="flex flex-col gap-1 text-xs">
            {draft.issues.map((i, n) => (
              <li key={n} className={cn("flex items-start gap-1.5", i.blocking ? "text-danger" : "text-warn")} data-testid="generate-issue-row" data-issue-code={i.code}>
                <AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
                <span className="selectable">
                  {i.message} <span className="text-fg-subtle">({i.path})</span>
                </span>
                {i.suggestion != null && (
                  <button type="button" className="shrink-0 text-accent hover:underline cursor-pointer" onClick={() => apply(i)}>
                    {t("generate.editor.apply_suggestion")}
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>
    </fieldset>
  );
}
