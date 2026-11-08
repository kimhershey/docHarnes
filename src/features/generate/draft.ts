// Pure helpers of the 문서 생성 screen (design §3.13): draft edits (fields,
// table rows, body blocks), applying validator suggestions, the excluded
// reference set and the template label.

import type { TFunction } from "i18next";

import type { Draft, DraftBlock, DraftIssue, DraftRequest, SchemaRecord, SourceRef, TemplateInfo, TemplateRef } from "@/lib/types";

export function setTitle(draft: Draft, title: string): Draft {
  return { ...draft, title };
}

/** Edits a field value; the edit is kept across regenerations (`user_edited`). */
export function setFieldValue(draft: Draft, key: string, value: string): Draft {
  return { ...draft, fields: draft.fields.map((f) => (f.key === key ? { ...f, value, normalized: null, user_edited: true } : f)) };
}

export function setCell(draft: Draft, tableKey: string, row: number, col: number, value: string): Draft {
  return {
    ...draft,
    tables: draft.tables.map((t) => (t.key === tableKey ? { ...t, rows: t.rows.map((r, ri) => (ri === row ? r.map((c, ci) => (ci === col ? value : c)) : r)) } : t)),
  };
}

export function addRow(draft: Draft, tableKey: string): Draft {
  return { ...draft, tables: draft.tables.map((t) => (t.key === tableKey ? { ...t, rows: [...t.rows, t.columns.map(() => "")] } : t)) };
}

export function removeRow(draft: Draft, tableKey: string, row: number): Draft {
  return { ...draft, tables: draft.tables.map((t) => (t.key === tableKey ? { ...t, rows: t.rows.filter((_, ri) => ri !== row) } : t)) };
}

export function setBlock(draft: Draft, index: number, block: DraftBlock): Draft {
  return { ...draft, body: draft.body.map((b, i) => (i === index ? block : b)) };
}

/** Text form of a block for a textarea: list items one per line. */
export function blockText(block: DraftBlock): string {
  switch (block.kind) {
    case "heading":
    case "paragraph":
      return block.text;
    case "list":
      return block.items.join("\n");
    case "table_ref":
      return "";
  }
}

export function blockWithText(block: DraftBlock, text: string): DraftBlock {
  switch (block.kind) {
    case "heading":
      return { ...block, text };
    case "paragraph":
      return { ...block, text };
    case "list":
      return { ...block, items: text.split("\n").map((s) => s.trim()).filter(Boolean) };
    case "table_ref":
      return block;
  }
}

/** Issue path → `fields.<key>` key, `tables.<key>[row].<col>` cell or `body[i]` block. */
export function parseIssuePath(path: string): { kind: "field"; key: string } | { kind: "cell"; table: string; row: number; column: string } | { kind: "body"; index: number } | { kind: "other" } {
  let m = path.match(/^fields\.(.+)$/);
  if (m) return { kind: "field", key: m[1]! };
  m = path.match(/^tables\.([^[]+)\[(\d+)\]\.(.+)$/);
  if (m) return { kind: "cell", table: m[1]!, row: Number(m[2]), column: m[3]! };
  m = path.match(/^body\[(\d+)\]$/);
  if (m) return { kind: "body", index: Number(m[1]) };
  return { kind: "other" };
}

/** Applies the validator's suggestion (computed sum, normalised value) and drops the issue. */
export function applySuggestion(draft: Draft, issue: DraftIssue): Draft {
  if (issue.suggestion == null) return draft;
  const target = parseIssuePath(issue.path);
  let next = draft;
  if (target.kind === "field") next = setFieldValue(draft, target.key, issue.suggestion);
  else if (target.kind === "cell") {
    const table = draft.tables.find((t) => t.key === target.table);
    const col = table?.columns.findIndex((c) => c.key === target.column) ?? -1;
    if (table && col >= 0) next = setCell(draft, target.table, target.row, col, issue.suggestion);
  }
  return { ...next, issues: next.issues.filter((i) => i !== issue && !(i.path === issue.path && i.code === issue.code)) };
}

export function issuesFor(draft: Draft, path: string): DraftIssue[] {
  return draft.issues.filter((i) => i.path === path);
}

export function hasBlockingIssues(draft: Draft): boolean {
  return draft.issues.some((i) => i.blocking);
}

/** The regeneration request: the current draft as `previous`, the feedback and the excluded references. */
export function regenerationRequest(base: DraftRequest, draft: Draft, feedback: string, excluded: string[]): DraftRequest {
  return { ...base, previous: draft, feedback: feedback.trim() || null, excluded_sources: excluded };
}

export function toggleExcluded(excluded: string[], docId: string): string[] {
  return excluded.includes(docId) ? excluded.filter((x) => x !== docId) : [...excluded, docId];
}

export function documentSourceIds(sources: SourceRef[]): string[] {
  return sources.flatMap((s) => (s.kind === "document" ? [s.doc_id] : []));
}

export function withDocumentSources(sources: SourceRef[], ids: string[]): SourceRef[] {
  const others = sources.filter((s) => s.kind !== "document");
  return [...ids.map((doc_id): SourceRef => ({ kind: "document", doc_id })), ...others];
}

/** "form:<schema id>" / "tpl:<kind>" picker value ↔ `form_ref`. */
export function formRefToPicker(formRef: string | null, templates: TemplateInfo[], schemas: SchemaRecord[]): string {
  if (!formRef) return "auto";
  if (templates.some((t) => t.kind === formRef)) return `tpl:${formRef}`;
  const schema = schemas.find((s) => s.id === formRef || s.name === formRef || s.doc_type === formRef);
  return schema ? `form:${schema.id}` : "auto";
}

export function pickerToFormRef(value: string): string | null {
  if (value === "auto") return null;
  if (value.startsWith("tpl:")) return value.slice(4);
  if (value.startsWith("form:")) return value.slice(5);
  return value;
}

export function templateLabel(ref: TemplateRef, schemas: SchemaRecord[] | undefined, t: TFunction): string {
  if (ref.kind === "builtin") return t(`generate.templates.${ref.template}`);
  const schema = schemas?.find((s) => s.id === ref.schema_id);
  return schema ? schema.name : t("schemas.source.form");
}

/** `FieldKind` strings → short badge text; unknown kinds are shown verbatim. */
export const FIELD_KIND_LABELS: Record<string, { ko: string; en: string }> = {
  string: { ko: "문자", en: "text" },
  amount: { ko: "금액", en: "amount" },
  integer: { ko: "정수", en: "integer" },
  date: { ko: "날짜", en: "date" },
  boolean: { ko: "예/아니오", en: "yes/no" },
  enum: { ko: "선택", en: "choice" },
  brn: { ko: "사업자번호", en: "BRN" },
  rrn: { ko: "주민번호", en: "RRN" },
  account: { ko: "계좌", en: "account" },
  phone: { ko: "전화", en: "phone" },
  email: { ko: "이메일", en: "email" },
};

export function fieldKindLabel(kind: string, lang: string): string {
  const l = FIELD_KIND_LABELS[kind];
  return l ? (lang.startsWith("ko") ? l.ko : l.en) : kind;
}
