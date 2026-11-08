// Pure helpers for the assistant's plan / execution / result cards: op labels
// and parameter summaries (design §3.7.6 plan card rows), the impact line,
// the `only_reviewed` toggle, the UI context sent with a command and the
// derivation of result-card actions from a `PlanExecution`.

import type { TFunction } from "i18next";

import {
  FOLDER_TEMPLATES,
  type AggregateRequest,
  type AggregateResult,
  type Answer,
  type ExportPreview,
  type ExportResult,
  type Op,
  type OpKind,
  type OrganizeItem,
  type OrganizePlan,
  type Plan,
  type PlanContext,
  type PlanExecution,
  type PlanIssue,
  type PlanPreview,
  type PlanResponse,
  type PlanSource,
  type Scope,
  type SearchResponse,
  type StepPreview,
  type StepResult,
  type UiPlanContext,
  type UndoReport,
} from "@/lib/types";

import { FILE_OPS, UI_OPS } from "./opSpecs";

type Dict = Record<string, unknown>;

export function isRecord(v: unknown): v is Dict {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function num(v: unknown): number {
  return typeof v === "number" && Number.isFinite(v) ? v : 0;
}

function str(v: unknown): string | null {
  return typeof v === "string" ? v : null;
}

/** Lower-case alphanumeric comparison key (mirrors `planner::compact`). */
export function compact(s: string): string {
  return s.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
}

export function truncate(s: string, max: number): string {
  const chars = Array.from(s);
  return chars.length > max ? `${chars.slice(0, max).join("")}…` : s;
}

export function opLabel(op: OpKind, t: TFunction): string {
  return t(`planner.ops.${op}`);
}

export function sourceLabel(source: PlanSource, t: TFunction): string {
  return t(`planner.source.${source}`);
}

export function docTypeName(slug: string | null | undefined, context: PlanContext | null | undefined, lang: string): string {
  if (!slug) return "";
  const d = context?.doc_types.find((x) => x.slug === slug || compact(x.label_ko) === compact(slug) || compact(x.label_en) === compact(slug));
  if (!d) return slug;
  return lang.startsWith("ko") ? d.label_ko || d.label_en || slug : d.label_en || d.label_ko || slug;
}

function schemaName(ref: string | null | undefined, context: PlanContext | null | undefined, lang: string): string | null {
  if (!ref || ref.toLowerCase() === "auto") return null;
  const key = compact(ref);
  const s = context?.schemas.find((x) => x.id === ref || compact(x.name) === key || compact(x.name_en) === key || compact(x.doc_type) === key);
  if (!s) return ref;
  return lang.startsWith("ko") ? s.name || s.name_en : s.name_en || s.name;
}

function joinParts(parts: (string | null | undefined | false)[]): string {
  return parts.filter((x): x is string => typeof x === "string" && x.length > 0).join(" · ");
}

export interface SummaryContext {
  lang: string;
  context?: PlanContext | null;
}

/**
 * One-line parameter summary of a plan step ("스키마 영수증 · 필드 11개",
 * "documents/{{year}}/{{month}}/{{doc_type}} · group_by=month").
 */
export function summarizeOp(op: Op, t: TFunction, ctx: SummaryContext = { lang: "ko" }): string {
  const lang = ctx.lang;
  const pt = (key: string, vars?: Record<string, unknown>) => t(`planner.params.${key}`, vars);
  switch (op.op) {
    case "import": {
      const source = op.source ?? "inbox";
      return joinParts([
        source === "folder" ? pt("source_folder", { path: op.path ?? "" }) : source === "files" ? pt("source_files") : pt("source_inbox"),
        op.recursive ? pt("recursive") : null,
        op.auto_pipeline ? pt("auto_pipeline", { level: op.auto_pipeline }) : null,
      ]);
    }
    case "set_inbox_watch":
      return op.enabled ? pt("on") : pt("off");
    case "classify":
      return joinParts([op.doc_types && op.doc_types.length > 0 ? pt("candidate_types", { count: op.doc_types.length }) : pt("all_types"), op.force ? pt("force") : null]);
    case "set_doc_type":
      return docTypeName(op.doc_type, ctx.context, lang);
    case "ocr":
      return joinParts([
        op.pages && op.pages.length > 0 ? pt("pages", { count: op.pages.length }) : pt("all_pages"),
        op.profile === "fine_print" ? pt("profile_fine_print") : op.profile === "standard" ? pt("profile_standard") : null,
        op.force ? pt("force") : null,
      ]);
    case "define_schema":
      return joinParts([op.mode === "modify" ? pt("mode_modify", { base: op.base ?? "" }) : pt("mode_create"), op.name ?? null, op.description ? truncate(op.description, 60) : null]);
    case "extract": {
      const name = schemaName(op.schema_ref, ctx.context, lang);
      return joinParts([name ? pt("schema", { name }) : pt("schema_auto"), op.fields && op.fields.length > 0 ? pt("fields", { count: op.fields.length }) : pt("all_fields"), op.force ? pt("force") : null]);
    }
    case "validate":
      return "";
    case "review":
      return joinParts([
        pt(`review_${op.action}`),
        op.only ? pt(`only_${op.only}`) : null,
        op.action === "correct" && op.field ? `${op.field} = ${op.value ?? ""}` : null,
      ]);
    case "reindex":
      return op.scope ? pt("scope", { scope: op.scope }) : "";
    case "search":
      return joinParts([`'${op.query}'`, op.mode ? t(`search.mode.${op.mode}`) : null, op.top_k ? pt("top_k", { count: op.top_k }) : null]);
    case "answer":
      return truncate(op.question, 80);
    case "summarize":
      return joinParts([op.target ? pt(op.target) : null, op.length ? pt(op.length) : null]);
    case "aggregate":
      return joinParts([t(`planner.fn.${op.fn}`), op.field ?? null, op.group_by && op.group_by !== "none" ? pt("group_by", { value: op.group_by }) : null, op.include_unreviewed ? pt("include_unreviewed") : null]);
    case "organize": {
      const folder = op.folder_template ?? (op.group_by ? FOLDER_TEMPLATES[op.group_by] : null);
      return joinParts([op.template ?? null, folder, op.group_by ? pt("group_by", { value: op.group_by }) : null, op.only_reviewed === false ? pt("with_unreviewed") : null, op.dry_run ? pt("dry_run") : null]);
    }
    case "rename":
      return joinParts([op.template, op.only_reviewed === false ? pt("with_unreviewed") : null]);
    case "move":
      return joinParts([op.folder, op.group_by ? pt("group_by", { value: op.group_by }) : null, op.only_reviewed === false ? pt("with_unreviewed") : null]);
    case "tag":
      return joinParts([op.add && op.add.length > 0 ? `+${op.add.join(", ")}` : null, op.remove && op.remove.length > 0 ? `−${op.remove.join(", ")}` : null]);
    case "delete":
      return op.to_trash === false ? pt("permanent") : pt("to_trash");
    case "export":
      return joinParts([
        op.format.toUpperCase(),
        op.path ?? pt("default_path"),
        op.columns && op.columns.length > 0 ? pt("columns", { count: op.columns.length }) : null,
        op.layout ? pt(`layout_${op.layout}`) : null,
        op.include && op.include.length > 0 ? pt("include", { list: op.include.join(", ") }) : null,
        op.target && op.target !== "fields" ? pt(`target_${op.target}`) : null,
      ]);
    case "set_model":
      return op.id;
    case "set_profile":
      return t(`models.profiles.${op.profile}`);
    case "set_theme":
      return t(`theme.${op.theme}`);
    case "set_language":
      return t(`language.${op.lang}`);
    case "set_setting":
      return `${settingLabel(op.key, t)} = ${String(op.value)}`;
    case "open":
      return joinParts([pt(`open_${op.target}`), op.target === "screen" ? screenLabel(op.ref, t) : op.ref]);
    case "job":
      return joinParts([pt(`job_${op.action}`), !op.id || op.id === "last" ? pt("last_job") : op.id]);
    case "engine":
      return pt(`engine_${op.action}`);
    case "undo":
      return !op.target || op.target === "last" ? pt("undo_last") : pt("undo_batch", { id: op.target });
    case "redo":
    case "help":
    case "noop":
      return "";
    case "clarify":
      return op.question;
    default:
      return "";
  }
}

const SETTING_LABEL_KEYS: Record<string, string> = {
  render_dpi: "settings.processing.render_dpi",
  pii_masking: "settings.privacy.pii_masking",
  auto_pipeline: "settings.processing.auto_level",
  inbox_watch: "settings.processing.inbox_watch",
  review_threshold: "settings.processing.review_threshold",
  cache_limit_gb: "planner.params.cache_limit_gb",
  journal_days: "settings.privacy.items.journal_days",
};

export function settingLabel(key: string, t: TFunction): string {
  const k = SETTING_LABEL_KEYS[key];
  return k ? t(k) : key;
}

const SCREEN_KEYS = ["home", "library", "search", "assistant", "review", "schemas", "knowledge", "generate", "projects", "organize", "jobs", "models", "settings"];

export function screenLabel(ref: string, t: TFunction): string {
  return SCREEN_KEYS.includes(ref) ? t(`nav.${ref}`) : ref;
}

// ---------------------------------------------------------------------------
// Plan classification / editing
// ---------------------------------------------------------------------------

export type PlanKind = "clarify" | "help" | "answer" | "ui" | "work";

export function isClarification(plan: Plan): boolean {
  return plan.intent === "clarify" || (plan.operations.length > 0 && plan.operations.every((o) => o.op === "clarify")) || (plan.operations.length === 0 && !!plan.clarification_question);
}

export function clarificationQuestion(plan: Plan): string {
  const op = plan.operations.find((o): o is Extract<Op, { op: "clarify" }> => o.op === "clarify");
  return plan.clarification_question ?? op?.question ?? "";
}

/** What the assistant does with a plan before any confirmation logic. */
export function planKind(plan: Plan): PlanKind {
  if (isClarification(plan)) return "clarify";
  const real = plan.operations.filter((o) => o.op !== "noop");
  if (real.length === 1 && real[0]!.op === "help") return "help";
  if (real.length === 1 && real[0]!.op === "answer") return "answer";
  if (real.length > 0 && real.every((o) => UI_OPS.has(o.op))) return "ui";
  return "work";
}

export function requiresConfirmation(response: PlanResponse): boolean {
  return response.result.plan.requires_confirmation || response.preview?.requires_confirmation === true;
}

/** The `only_reviewed` value of the plan's file operations (null when it has none). */
export function onlyReviewedValue(plan: Plan, fallback: boolean): boolean | null {
  const ops = plan.operations.filter((o) => FILE_OPS.has(o.op)) as Extract<Op, { op: "organize" | "rename" | "move" }>[];
  if (ops.length === 0) return null;
  return ops.every((o) => (o.only_reviewed ?? fallback) !== false);
}

export function withOnlyReviewed(plan: Plan, value: boolean): Plan {
  return {
    ...plan,
    operations: plan.operations.map((o) => (FILE_OPS.has(o.op) ? ({ ...o, only_reviewed: value } as Op) : o)),
  };
}

export function replaceOp(plan: Plan, index: number, op: Op): Plan {
  return { ...plan, operations: plan.operations.map((o, i) => (i === index ? op : o)) };
}

export function withScope(plan: Plan, scope: Scope): Plan {
  return { ...plan, scope };
}

export function blockingIssues(issues: PlanIssue[]): PlanIssue[] {
  return issues.filter((i) => i.blocking);
}

export function warningIssues(issues: PlanIssue[]): PlanIssue[] {
  return issues.filter((i) => !i.blocking);
}

// ---------------------------------------------------------------------------
// UI context (default scope of a command)
// ---------------------------------------------------------------------------

export type AssistantScope = "all" | "folder" | "selection" | "knowledge" | `col:${string}`;

export function uiContextFor(
  scope: string,
  folder: string | null,
  selectedIds: string[],
  lastDocument: { id: string; page: number | null } | null,
): UiPlanContext {
  const ctx: UiPlanContext = {};
  if (scope === "folder" && folder) ctx.folder = folder;
  if (scope === "selection" && selectedIds.length > 0) ctx.selected_doc_ids = selectedIds;
  if (lastDocument) {
    ctx.current_document = lastDocument.id;
    if (lastDocument.page != null) ctx.current_page = lastDocument.page;
  }
  return ctx;
}

// ---------------------------------------------------------------------------
// Preview / impact
// ---------------------------------------------------------------------------

export interface OrganizePreviewRows {
  items: OrganizeItem[];
  moves: number;
  held: number;
  unchanged: number;
}

export function organizePreview(step: StepPreview | undefined): OrganizePreviewRows | null {
  if (!step || !isRecord(step.payload) || !Array.isArray(step.payload.items)) return null;
  const p = step.payload as unknown as OrganizePlan;
  return { items: p.items.filter((i) => i.status === "move" || i.status === "held"), moves: num(p.moves), held: num(p.held), unchanged: num(p.unchanged) };
}

export function exportPreview(step: StepPreview | undefined): ExportPreview | null {
  if (!step || !isRecord(step.payload) || typeof step.payload.path !== "string" || !("exists" in step.payload)) return null;
  return step.payload as unknown as ExportPreview;
}

export interface Impact {
  moves: number;
  deletes: number;
  exports: number;
  documents: number;
}

export function impactOf(plan: Plan, preview: PlanPreview | null): Impact {
  const impact: Impact = { moves: 0, deletes: 0, exports: 0, documents: preview?.document_count ?? 0 };
  plan.operations.forEach((op, i) => {
    const step = preview?.steps.find((s) => s.index === i);
    if (FILE_OPS.has(op.op)) {
      const rows = organizePreview(step);
      impact.moves += rows ? rows.moves : impact.documents;
    } else if (op.op === "delete") {
      impact.deletes += step && isRecord(step.payload) ? num(step.payload.documents) : impact.documents;
    } else if (op.op === "export") {
      impact.exports += 1;
    }
  });
  return impact;
}

export function impactLine(impact: Impact, t: TFunction): string {
  const effects = [
    impact.moves > 0 ? t("planner.card.impact_moves", { count: impact.moves }) : null,
    impact.deletes > 0 ? t("planner.card.impact_delete", { count: impact.deletes }) : null,
    impact.exports > 0 ? t("planner.card.impact_export", { count: impact.exports }) : null,
  ].filter((x): x is string => x !== null);
  return t("planner.card.impact", { effects: effects.length > 0 ? effects.join(" · ") : t("planner.card.impact_none"), count: impact.documents });
}

// ---------------------------------------------------------------------------
// Aggregate helpers
// ---------------------------------------------------------------------------

export function aggregateRequestFor(op: Extract<Op, { op: "aggregate" }>, scope: Scope, includeUnreviewed: boolean): AggregateRequest {
  return { func: op.fn, field: op.field ?? null, group_by: op.group_by ?? "none", include_unreviewed: includeUnreviewed, scope };
}

export function isAggregateResult(v: unknown): v is AggregateResult {
  return isRecord(v) && typeof v.func === "string" && typeof v.value === "number" && Array.isArray(v.groups) && Array.isArray(v.documents);
}

/** Thousands separators, at most one decimal (averages). */
export function formatAmount(value: number): string {
  return new Intl.NumberFormat("ko-KR", { maximumFractionDigits: Number.isInteger(value) ? 0 : 1 }).format(value);
}

/** Whether the aggregated field is a money amount (schema field kind, else a name heuristic). */
export function aggregateIsAmount(result: AggregateResult, context: PlanContext | null | undefined): boolean {
  if (result.func === "count") return false;
  const schema = context?.schemas.find((s) => s.doc_type === result.doc_type) ?? context?.schemas.find((s) => s.fields.some((f) => f.key === result.field));
  const field = schema?.fields.find((f) => f.key === result.field);
  if (field) return field.kind === "amount";
  return /amount|total|price|sum|tax|fee|cost|금액|합계|가액|세액|단가/i.test(result.field ?? "") || result.field === null;
}

export function formatAggregateValue(value: number, result: AggregateResult, amount: boolean, t: TFunction): string {
  if (result.func === "count") return t("planner.aggregate.unit_count", { value: formatAmount(value) });
  return amount ? t("planner.aggregate.won", { value: formatAmount(value) }) : formatAmount(value);
}

// ---------------------------------------------------------------------------
// Result actions
// ---------------------------------------------------------------------------

export interface OrganizeOutcome {
  index: number;
  batchId: string | null;
  moved: number;
  held: number;
  failed: number;
  items: OrganizeItem[];
}

export interface ResultActions {
  reviewPending: number;
  reviewDocuments: string[];
  organise: OrganizeOutcome[];
  exports: { index: number; result: ExportResult }[];
  searches: { index: number; query: string; response: SearchResponse }[];
  answers: { index: number; answer: Answer }[];
  summaries: { index: number; items: { document_id: string; name: string; answer: Answer }[] }[];
  aggregates: { index: number; result: AggregateResult; op: Extract<Op, { op: "aggregate" }> | null }[];
  undos: { index: number; report: UndoReport }[];
  batches: string[];
  failed: StepResult | null;
}

function isAnswer(v: unknown): v is Answer {
  return isRecord(v) && typeof v.text === "string" && Array.isArray(v.sources) && Array.isArray(v.sentences);
}

/** Derives the action buttons / sections of the result card from the step payloads (`exec.rs`). */
export function deriveResultActions(execution: PlanExecution, plan: Plan): ResultActions {
  const out: ResultActions = {
    reviewPending: 0,
    reviewDocuments: [],
    organise: [],
    exports: [],
    searches: [],
    answers: [],
    summaries: [],
    aggregates: [],
    undos: [],
    batches: [...execution.batches],
    failed: execution.steps.find((s) => s.status === "failed") ?? null,
  };
  for (const step of execution.steps) {
    if (step.status !== "done") continue;
    const payload = step.payload;
    const op = plan.operations[step.index] ?? null;
    switch (step.op) {
      case "extract":
      case "validate":
        if (isRecord(payload)) {
          out.reviewPending += num(payload.review_pending) + num(payload.review_fields);
          const docs = Array.isArray(payload.processed) ? payload.processed : Array.isArray(payload.documents) ? payload.documents : [];
          out.reviewDocuments.push(...docs.filter((d): d is string => typeof d === "string"));
        }
        break;
      case "organize":
      case "rename":
      case "move":
      case "delete":
        if (isRecord(payload) && Array.isArray(payload.items)) {
          out.organise.push({
            index: step.index,
            batchId: str(payload.batch_id),
            moved: num(payload.moved),
            held: num(payload.held),
            failed: Array.isArray(payload.failed) ? payload.failed.length : num(payload.failed),
            items: (payload.items as unknown[]).filter(isRecord) as unknown as OrganizeItem[],
          });
        }
        break;
      case "export":
        if (isRecord(payload) && typeof payload.path === "string" && typeof payload.absolute === "string") out.exports.push({ index: step.index, result: payload as unknown as ExportResult });
        break;
      case "search":
        if (isRecord(payload) && Array.isArray(payload.hits)) out.searches.push({ index: step.index, query: op && op.op === "search" ? op.query : "", response: payload as unknown as SearchResponse });
        break;
      case "answer":
        if (isAnswer(payload)) out.answers.push({ index: step.index, answer: payload });
        break;
      case "summarize":
        if (isRecord(payload) && Array.isArray(payload.summaries)) {
          const items = (payload.summaries as unknown[])
            .filter(isRecord)
            .filter((s) => isAnswer(s.answer))
            .map((s) => ({ document_id: String(s.document_id ?? ""), name: String(s.name ?? ""), answer: s.answer as Answer }));
          out.summaries.push({ index: step.index, items });
        }
        break;
      case "aggregate":
        if (isAggregateResult(payload)) out.aggregates.push({ index: step.index, result: payload, op: op && op.op === "aggregate" ? op : null });
        break;
      case "undo":
      case "redo":
        if (isRecord(payload) && typeof payload.batch_id === "string" && typeof payload.restored === "number") out.undos.push({ index: step.index, report: payload as unknown as UndoReport });
        break;
      default:
        break;
    }
  }
  out.reviewDocuments = [...new Set(out.reviewDocuments)];
  return out;
}

export function isPlanExecution(v: unknown): v is PlanExecution {
  return isRecord(v) && Array.isArray(v.steps) && Array.isArray(v.ui_actions) && Array.isArray(v.batches);
}
