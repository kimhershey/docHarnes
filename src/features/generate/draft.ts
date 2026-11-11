// Pure helpers of the 문서 생성 screen (design §3.13): draft edits (fields,
// table rows, body blocks), applying validator suggestions, the excluded
// reference set and the template label.

import type { TFunction } from "i18next";

import type { Draft, DraftBlock, DraftIssue, DraftRequest, ReviseScope, SchemaRecord, SourceRef, TemplateInfo, TemplateRef } from "@/lib/types";

export { isSectionHeading, reviseScopeLabel, sectionAt, sectionsOf, type DraftSection } from "@/lib/draft-sections";

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

// -- body blocks: add / remove / move (0.7.0 plan §1.2) ------------------------------------------------
// The validator's `body[i]` issue paths are positional, so every structural edit re-indexes them: an issue
// of a removed block is dropped, the others follow their block.

export type NewBlockKind = "paragraph" | "heading" | "list";

export function newBlock(kind: NewBlockKind, level = 2): DraftBlock {
  switch (kind) {
    case "heading":
      return { kind: "heading", level, text: "" };
    case "list":
      return { kind: "list", items: [], ordered: false };
    default:
      return { kind: "paragraph", text: "", source: null };
  }
}

/** Re-maps `body[i]` issue paths through `f` (null drops the issue); other paths are kept. */
function remapBodyIssues(issues: DraftIssue[], f: (index: number) => number | null): DraftIssue[] {
  const out: DraftIssue[] = [];
  for (const issue of issues) {
    const target = parseIssuePath(issue.path);
    if (target.kind !== "body") {
      out.push(issue);
      continue;
    }
    const next = f(target.index);
    if (next === null) continue;
    out.push(next === target.index ? issue : { ...issue, path: `body[${next}]` });
  }
  return out;
}

/** Inserts `block` so that it becomes `body[at]` (`at` = body length appends). */
export function insertBlock(draft: Draft, at: number, block: DraftBlock): Draft {
  const index = Math.max(0, Math.min(draft.body.length, at));
  const body = [...draft.body.slice(0, index), block, ...draft.body.slice(index)];
  return { ...draft, body, issues: remapBodyIssues(draft.issues, (i) => (i >= index ? i + 1 : i)) };
}

export function removeBlock(draft: Draft, at: number): Draft {
  if (at < 0 || at >= draft.body.length) return draft;
  const body = draft.body.filter((_, i) => i !== at);
  return { ...draft, body, issues: remapBodyIssues(draft.issues, (i) => (i === at ? null : i > at ? i - 1 : i)) };
}

/** Moves `body[from]` to position `to` (the other blocks keep their order). */
export function moveBlock(draft: Draft, from: number, to: number): Draft {
  const n = draft.body.length;
  if (from < 0 || from >= n || to < 0 || to >= n || from === to) return draft;
  const body = [...draft.body];
  const [moved] = body.splice(from, 1);
  body.splice(to, 0, moved!);
  const remap = (i: number): number => {
    if (i === from) return to;
    if (from < to) return i > from && i <= to ? i - 1 : i;
    return i >= to && i < from ? i + 1 : i;
  };
  return { ...draft, body, issues: remapBodyIssues(draft.issues, remap) };
}

/** Heading level of a heading block (2 = section heading, 3 = sub-heading); other blocks are returned as they are. */
export function setBlockLevel(draft: Draft, at: number, level: number): Draft {
  const b = draft.body[at];
  if (!b || b.kind !== "heading") return draft;
  return setBlock(draft, at, { ...b, level: Math.max(1, Math.min(4, level)) });
}

/** The revision request of the 초안 어시스턴트 (plan §1.5): the current draft as `previous`, the instruction as `feedback`. */
export function reviseRequest(base: DraftRequest, draft: Draft, instruction: string, scope: ReviseScope, excluded: string[]): DraftRequest {
  return { ...base, previous: draft, feedback: instruction.trim(), excluded_sources: excluded, revise: scope };
}

/** `"whole"` / `"section:3"` / `"insert_after:3"` / `"insert_after:end"` — the scope select's values. */
export function reviseScopeToPicker(scope: ReviseScope): string {
  switch (scope.scope) {
    case "whole":
      return "whole";
    case "section":
      return `section:${scope.section}`;
    case "insert_after":
      return scope.section == null ? "insert_after:end" : `insert_after:${scope.section}`;
  }
}

export function pickerToReviseScope(value: string, heading: string | null = null): ReviseScope {
  if (value.startsWith("section:")) return { scope: "section", section: Math.max(1, Number(value.slice(8)) || 1) };
  if (value.startsWith("insert_after:")) {
    const rest = value.slice(13);
    return { scope: "insert_after", section: rest === "end" ? null : Math.max(1, Number(rest) || 1), heading: heading?.trim() || null };
  }
  return { scope: "whole" };
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
