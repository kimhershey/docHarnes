// Pure helpers for the assistant's plan / execution / result cards: op labels
// and parameter summaries (design §3.7.6 plan card rows), the impact line,
// the `only_reviewed` toggle, the UI context sent with a command and the
// derivation of result-card actions from a `PlanExecution`.

import type { TFunction } from "i18next";

import type { UiOpResult } from "@/lib/commands";
import { settingSpec } from "@/lib/settingsCatalog";
import {
  FOLDER_TEMPLATES,
  isCheckRun,
  isDraft,
  isProject,
  isProjectDetail,
  isRuleDraft,
  type AggregateRequest,
  type AggregateResult,
  type Answer,
  type CheckRun,
  type Draft,
  type ExportPreview,
  type ExportResult,
  type GroupBy,
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
  type Project,
  type ProjectDetail,
  type RuleDraft,
  type Scope,
  type SearchResponse,
  type StepPreview,
  type StepResult,
  type UiPlanContext,
  type UndoReport,
} from "@/lib/types";

import { DRAFT_OPS, FILE_OPS, UI_OPS } from "./opSpecs";

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

/** "월별" / "거래처별" / "유형별" / "프로젝트별" (null for no grouping). */
function groupLabel(group: GroupBy | null | undefined, t: TFunction): string | null {
  switch (group) {
    case "month":
      return t("planner.params.group_month");
    case "vendor":
      return t("planner.params.group_vendor");
    case "doc_type":
      return t("planner.params.group_doc_type");
    case "project":
      return t("planner.params.group_project");
    default:
      return null;
  }
}

/** A file-name / folder template with its variables named in words: `documents/{{year}}/{{doc_type}}` → `documents/<연도>/<유형>`. */
export function readableTemplate(template: string, t: TFunction): string {
  return template.replace(/\{\{\s*([A-Za-z_]+)\s*\}\}/g, (_match, name: string) => `<${t(`organize.variables.${name}`, { defaultValue: name })}>`);
}

/**
 * Where a file op puts the documents, in words (UI-15: the card showed `documents/{{doc_type}} · group_by=doc_type`):
 * "유형별 폴더(documents/<유형>)" when the folder is the grouping's own, else "폴더 <path>".
 */
function folderLabel(folder: string | null | undefined, group: GroupBy | null | undefined, t: TFunction): string | null {
  if (!folder) return null;
  const path = readableTemplate(folder, t);
  const grouped = group && FOLDER_TEMPLATES[group] === folder ? groupLabel(group, t) : null;
  return grouped ? t("planner.params.folder_grouped", { group: grouped, path }) : t("planner.params.folder_template", { path });
}

/**
 * One-line parameter summary of a plan step ("스키마 영수증 · 필드 11개",
 * "월별 폴더(documents/<연도>/<월>/<유형>)").
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
        op.auto_pipeline ? pt("auto_pipeline", { level: t(`settings.processing.auto_levels.${op.auto_pipeline === "classify" ? "classify_only" : op.auto_pipeline}`) }) : null,
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
      return op.scope ? pt(`reindex_${op.scope}`) : "";
    case "search":
      return joinParts([`'${op.query}'`, op.mode ? t(`search.mode.${op.mode}`) : null, op.top_k ? pt("top_k", { count: op.top_k }) : null]);
    case "answer":
      return joinParts([truncate(op.question, 80), op.knowledge === false ? pt("knowledge_text_only") : null]);
    case "summarize":
      return joinParts([op.target ? pt(op.target) : null, op.length ? pt(op.length) : null]);
    case "aggregate":
      return joinParts([t(`planner.fn.${op.fn}`), op.field ?? null, groupLabel(op.group_by, t), op.include_unreviewed ? pt("include_unreviewed") : null]);
    case "organize": {
      const folder = op.folder_template ?? (op.group_by ? FOLDER_TEMPLATES[op.group_by] : null);
      return joinParts([op.template ? pt("name_template", { template: readableTemplate(op.template, t) }) : null, folderLabel(folder, op.group_by, t), op.only_reviewed === false ? pt("with_unreviewed") : null, op.dry_run ? pt("dry_run") : null]);
    }
    case "rename":
      return joinParts([pt("name_template", { template: readableTemplate(op.template, t) }), op.only_reviewed === false ? pt("with_unreviewed") : null]);
    case "move":
      return joinParts([folderLabel(op.folder, op.group_by, t), op.only_reviewed === false ? pt("with_unreviewed") : null]);
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
        op.include && op.include.length > 0 ? pt("include", { list: op.include.map((column) => t(`export.columns.${column}`)).join(", ") }) : null,
        op.target && op.target !== "fields" ? pt(`target_${op.target}`) : null,
      ]);
    case "draft_document": {
      const form = op.form_ref ? (TEMPLATE_KEYS.includes(op.form_ref) ? t(`generate.templates.${op.form_ref}`) : op.form_ref) : null;
      return joinParts([form ? pt("form", { name: form }) : op.format_description ? truncate(op.format_description, 40) : pt("form_auto"), truncate(op.content, 60), op.sources && op.sources.length > 0 ? pt("sources", { count: op.sources.length }) : null, op.output ? op.output.toUpperCase() : null, op.tone ? t(`generate.tone.${op.tone}`) : null, op.length ? t(`generate.length.${op.length}`) : null]);
    }
    case "revise_draft": {
      const scope = op.scope === "whole" ? pt("revise_whole") : op.scope === "section" ? pt("revise_section", { index: op.section ?? 1 }) : op.section == null ? pt("revise_append") : pt("revise_insert_after", { index: op.section });
      return joinParts([scope, op.heading ? `'${truncate(op.heading, 30)}'` : null, truncate(op.instruction, 60)]);
    }
    case "register_form":
      return joinParts([op.doc_ids && op.doc_ids.length >= 2 ? pt("sample_docs", { count: op.doc_ids.length }) : op.doc_ref && op.doc_ref !== "current" ? op.doc_ref : pt("current_document"), op.name ?? null, op.doc_type_slug ? docTypeName(op.doc_type_slug, ctx.context, lang) : null]);
    case "explain":
      return joinParts([truncate(op.selection ?? op.question ?? "", 80), op.scope ? pt(`explain_scope_${op.scope}`) : null, op.allow_general_knowledge ? pt("allow_general") : null]);
    case "compare":
      return joinParts([pt("compare_docs", { count: op.doc_ids.length }), op.aspects && op.aspects.length > 0 ? op.aspects.join(", ") : null]);
    case "create_project":
      return joinParts([`'${op.name}'`, op.template ? pt("project_template", { name: projectTemplateName(op.template, t) }) : null, op.doc_types_required && op.doc_types_required.length > 0 ? pt("required_types", { count: op.doc_types_required.length }) : null]);
    case "add_to_project":
      return joinParts([projectName(op.project, ctx.context, t), op.doc_ids && op.doc_ids.length > 0 ? pt("documents_count", { count: op.doc_ids.length }) : pt("scope_documents")]);
    case "define_rule":
      return joinParts([op.template ? pt("project_template", { name: projectTemplateName(op.template, t) }) : projectName(op.project, ctx.context, t), truncate(op.description, 80)]);
    case "check_consistency":
      return joinParts([projectName(op.project, ctx.context, t), op.rules && op.rules.length > 0 ? pt("rules_count", { count: op.rules.length }) : pt("all_rules")]);
    case "request_supplement":
      return joinParts([projectName(op.project, ctx.context, t), op.findings && op.findings.length > 0 ? pt("findings_count", { count: op.findings.length }) : pt("all_findings"), op.output ? op.output.toUpperCase() : null]);
    case "set_retention":
      return joinParts([t(`planner.params.retention_${op.item}`), op.days < 0 ? t("common.forever") : op.days === 0 ? t("settings.privacy.immediate") : pt("days", { count: op.days })]);
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
    // -- 0.6.0 (plan §1) --
    case "rotate":
      return joinParts([pt("degrees", { value: op.degrees }), op.pages && op.pages.length > 0 ? pt("pages", { count: op.pages.length }) : pt("all_pages")]);
    case "rerender":
      return op.dpi ? `${op.dpi} DPI` : pt("default_dpi");
    case "split":
      return op.every ? pt("split_every", { count: op.every }) : op.boundaries && op.boundaries.length > 0 ? pt("split_at", { pages: op.boundaries.join(", ") }) : pt("split_auto");
    case "merge":
      return op.name ? `'${op.name}'` : pt("merge_default_name");
    case "set_title":
      return `'${truncate(op.title, 60)}'`;
    case "edit_text":
      return joinParts([`'${truncate(op.find, 30)}' → '${truncate(op.replace, 30)}'`, op.pages && op.pages.length > 0 ? pt("pages", { count: op.pages.length }) : null]);
    case "set_role":
      return pt(`role_${op.role}`);
    case "manage_reference":
      return joinParts([pt(`reference_${op.action}`), op.group ?? (op.project ? pt("project_group", { name: projectName(op.project, ctx.context, t) }) : null), op.name ? `→ '${op.name}'` : null]);
    case "define_term":
      return joinParts([pt(`term_${op.action ?? "save"}`), `'${op.term}'`, op.definition ? truncate(op.definition, 40) : null, op.synonyms && op.synonyms.length > 0 ? pt("synonyms", { count: op.synonyms.length }) : null]);
    case "set_version":
      return op.clear ? pt("version_clear") : joinParts([op.group_key ? `'${op.group_key}'` : null, op.label ?? null, op.effective_date ?? null, op.latest === false ? pt("version_old") : op.latest ? pt("version_latest") : null]);
    case "update_project": {
      const fields = [op.name ? pt("new_name", { name: op.name }) : null, op.start_date ? `${pt("start_date")} ${op.start_date}` : null, op.end_date ? `${pt("end_date")} ${op.end_date}` : null, op.owner ? `${pt("owner")} ${op.owner}` : null, op.intake_no ? `${pt("intake_no")} ${op.intake_no}` : null, op.keywords && op.keywords.length > 0 ? pt("keywords", { count: op.keywords.length }) : null, op.template ? pt("project_template", { name: projectTemplateName(op.template, t) }) : null, op.inbox_folder ? `inbox/${op.inbox_folder}` : null, op.notes ? truncate(op.notes, 40) : null];
      return joinParts([projectName(op.project, ctx.context, t), ...fields]);
    }
    case "delete_project":
      return projectName(op.project, ctx.context, t);
    case "remove_from_project":
      return joinParts([projectName(op.project, ctx.context, t), op.doc_ids && op.doc_ids.length > 0 ? pt("documents_count", { count: op.doc_ids.length }) : pt("scope_documents")]);
    case "manage_template":
      return joinParts([pt(`template_${op.action ?? "save"}`), op.template ? projectTemplateName(op.template, t) : op.name ? `'${op.name}'` : null, op.doc_types_required && op.doc_types_required.length > 0 ? pt("required_types", { count: op.doc_types_required.length }) : null]);
    case "classify_projects":
      return joinParts([op.apply ? pt("classify_apply") : pt("classify_preview"), op.use_model ? pt("with_model") : null]);
    case "manage_schema":
      return joinParts([pt(`schema_${op.action}`), schemaName(op.schema, ctx.context, lang) ?? op.schema, op.name ? `→ '${op.name}'` : null]);
    case "manage_model":
      return joinParts([pt(`model_${op.action}`), op.id ?? null, op.tier ? (op.tier === "auto" ? t("models.tier_auto") : t(`hw.tier.${op.tier}`)) : null, op.backend ? t(`models.backends.${op.backend}`) : null, op.path ?? null]);
    case "workspace":
      return joinParts([pt(`workspace_${op.action}`), op.path ?? null]);
    case "maintenance":
      return pt(`maintenance_${op.action}`);
    default:
      return "";
  }
}

const TEMPLATE_KEYS = ["rfp", "proposal", "sow", "srs", "dev_contract", "wbs", "progress_report", "meeting_minutes", "test_report", "change_request", "acceptance", "estimate", "supplement_request", "official_letter", "quotation", "transaction_statement", "contract", "report", "minutes"];
const PROJECT_TEMPLATE_KEYS: Record<string, string> = {
  "builtin-outsourcing": "outsourcing",
  "builtin-maintenance": "maintenance",
  "builtin-internal_dev": "internal_dev",
  "builtin-certification_review": "certification_review",
  "builtin-vendor_onboarding": "vendor_onboarding",
  "builtin-purchase_settlement": "purchase_settlement",
};

/** Built-in project templates get their i18n name; user templates are shown by id / name. */
export function projectTemplateName(ref: string, t: TFunction): string {
  const key = PROJECT_TEMPLATE_KEYS[ref] ?? (["certification_review", "vendor_onboarding", "purchase_settlement"].includes(ref) ? ref : null);
  return key ? t(`projects.templates.builtin.${key}`) : ref;
}

/**
 * Project reference of a plan step: the named project, else the one open in the UI, else "(현재 프로젝트)".
 * No "only project" fallback: the validator is strict (`project_required`) and the card must not pretend otherwise.
 */
export function projectName(ref: string | null | undefined, context: PlanContext | null | undefined, t: TFunction): string {
  const id = ref ?? context?.current_project ?? null;
  if (!id) return t("planner.params.current_project");
  const p = context?.projects.find((x) => x.id === id || compact(x.name) === compact(id));
  return p ? p.name : ref ?? t("planner.params.current_project");
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

/** Label of a `set_setting` key: the catalog label (0.6.0) or the legacy alias' label. */
export function settingLabel(key: string, t: TFunction): string {
  const k = SETTING_LABEL_KEYS[key];
  if (k) return t(k);
  const spec = settingSpec(key);
  return spec ? t(spec.label) : key;
}

const SCREEN_KEYS = ["home", "library", "search", "assistant", "review", "schemas", "knowledge", "generate", "projects", "organize", "jobs", "models", "settings"];

export function screenLabel(ref: string, t: TFunction): string {
  return SCREEN_KEYS.includes(ref) ? t(`nav.${ref}`) : ref;
}

/** Compact detail line of a 0.6.0 agent step from its executor payload (plan §1 payload shapes). */
export interface AgentStepDetail {
  text: string;
  /** Document to open (merged / first split part). */
  openDocument?: string | null;
  /** Project to open. */
  openProject?: string | null;
}

export function agentStepDetail(op: OpKind, payload: unknown, t: TFunction): AgentStepDetail | null {
  if (!isRecord(payload)) return null;
  const docs = Array.isArray(payload.documents) ? payload.documents.length : null;
  const dt = (key: string, vars?: Record<string, unknown>) => t(`planner.result.detail.${key}`, vars);
  switch (op) {
    case "rotate":
      return { text: dt("rotate", { documents: docs ?? 0, degrees: num(payload.degrees) }) };
    case "rerender":
      return { text: dt("rerender", { documents: docs ?? 0 }) };
    case "split": {
      const created = Array.isArray(payload.created) ? (payload.created as unknown[]).map(String) : [];
      return { text: dt("split", { count: created.length }), openDocument: created[0] ?? null };
    }
    case "merge":
      return { text: dt("merge", { count: Array.isArray(payload.sources) ? payload.sources.length : 0 }), openDocument: str(payload.document_id) };
    case "set_title":
      return { text: dt("set_title", { documents: docs ?? 0 }) };
    case "edit_text":
      return { text: dt("edit_text", { replaced: num(payload.replaced), documents: docs ?? 0 }) };
    case "set_role":
      return { text: dt("set_role", { documents: docs ?? 0, role: str(payload.role) ? t(`planner.params.role_${String(payload.role)}`) : "" }) };
    case "manage_reference": {
      const group = isRecord(payload.group) ? str(payload.group.name) : null;
      return { text: dt("manage_reference", { action: t(`planner.params.reference_${String(payload.action ?? "")}`), group: group ?? "", documents: docs ?? 0 }) };
    }
    case "define_term": {
      // `exec.rs` sends the glossary entry (`term: { id, term, definition, synonyms }`); a bare string is accepted too.
      const term = isRecord(payload.term) ? str(payload.term.term) : str(payload.term);
      return { text: dt("define_term", { action: t(`planner.params.term_${String(payload.action ?? "save")}`), term: term ?? "" }) };
    }
    case "set_version":
      return { text: dt("set_version", { documents: docs ?? 0, group: str(payload.group_key) ?? "" }) };
    case "update_project": {
      const project = isRecord(payload.project) ? payload.project : null;
      return { text: dt("update_project", { name: project ? (str(project.name) ?? "") : "" }), openProject: project ? str(project.id) : null };
    }
    case "delete_project":
      return { text: dt(payload.deleted === false ? "delete_project_missing" : "delete_project") };
    case "remove_from_project":
      return { text: dt("remove_from_project", { removed: num(payload.removed) }), openProject: str(payload.project_id) };
    case "manage_template": {
      const tpl = isRecord(payload.template) ? payload.template : null;
      return { text: dt("manage_template", { action: t(`planner.params.template_${String(payload.action ?? "save")}`), name: tpl ? (str(tpl.name_ko) ?? "") : "" }) };
    }
    case "classify_projects": {
      // `exec.rs`: `applied` is always a number (0 for a run that only suggests); a run that assigned carries
      // its `apply_result`. Without that key (older payloads) a null `applied` meant "suggestions only".
      const assigned = "apply_result" in payload ? payload.apply_result != null : payload.applied != null;
      return { text: dt(assigned ? "classify_applied" : "classify_preview", { suggested: num(payload.suggested), applied: num(payload.applied) }) };
    }
    case "manage_schema": {
      const schema = isRecord(payload.schema) ? payload.schema : null;
      return { text: dt("manage_schema", { action: t(`planner.params.schema_${String(payload.action ?? "")}`), name: schema ? (str(schema.name) ?? "") : "" }) };
    }
    default:
      return null;
  }
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

export type AssistantScope = "all" | "folder" | "selection" | "document" | "knowledge" | `col:${string}`;

/**
 * The UI context sent with a command: the chosen scope (folder / selection / the last viewed
 * document) plus the viewer's current document and page, which the rules use as the "이 문서 /
 * 이 쪽" fallback when the sentence names no scope.
 */
export function uiContextFor(
  scope: string,
  folder: string | null,
  selectedIds: string[],
  lastDocument: { id: string; page: number | null } | null,
  currentProject: string | null = null,
): UiPlanContext {
  const ctx: UiPlanContext = {};
  if (currentProject) ctx.current_project = currentProject;
  if (scope === "folder" && folder) ctx.folder = folder;
  if (scope === "selection" && selectedIds.length > 0) ctx.selected_doc_ids = selectedIds;
  if (scope === "document" && lastDocument) ctx.selected_doc_ids = [lastDocument.id];
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

export interface RuleDraftPreview {
  /** The parsed rule (the grid is the confirmation); null when the phrase rules could not parse the sentence. */
  draft: RuleDraft | null;
  description: string | null;
}

/**
 * `define_rule` preview payload: `exec::preview` sends `{ description, draft: RuleDraft | null }`; a bare
 * `RuleDraft` (older shells, the browser mock) is accepted too.
 */
export function ruleDraftPreview(step: StepPreview | undefined): RuleDraftPreview | null {
  if (!step) return null;
  const payload = step.payload;
  if (isRuleDraft(payload)) return { draft: payload, description: payload.rule.description || null };
  if (!isRecord(payload) || !("draft" in payload || typeof payload.description === "string")) return null;
  const draft = isRuleDraft(payload.draft) ? payload.draft : null;
  return { draft, description: str(payload.description) ?? draft?.rule.description ?? null };
}

export interface Impact {
  moves: number;
  deletes: number;
  exports: number;
  /** Document drafts opened in 문서 생성 (saved there, not by the plan). */
  drafts: number;
  /** Project / rule database writes. */
  projects: number;
  documents: number;
}

export function impactOf(plan: Plan, preview: PlanPreview | null): Impact {
  const impact: Impact = { moves: 0, deletes: 0, exports: 0, drafts: 0, projects: 0, documents: preview?.document_count ?? 0 };
  plan.operations.forEach((op, i) => {
    const step = preview?.steps.find((s) => s.index === i);
    if (FILE_OPS.has(op.op)) {
      const rows = organizePreview(step);
      impact.moves += rows ? rows.moves : impact.documents;
    } else if (op.op === "delete") {
      impact.deletes += step && isRecord(step.payload) ? num(step.payload.documents) : impact.documents;
    } else if (op.op === "export") {
      impact.exports += 1;
    } else if (DRAFT_OPS.has(op.op)) {
      impact.drafts += 1;
    } else if (op.op === "create_project" || op.op === "add_to_project" || op.op === "define_rule") {
      impact.projects += 1;
    }
  });
  return impact;
}

export function impactLine(impact: Impact, t: TFunction): string {
  const effects = [
    impact.moves > 0 ? t("planner.card.impact_moves", { count: impact.moves }) : null,
    impact.deletes > 0 ? t("planner.card.impact_delete", { count: impact.deletes }) : null,
    impact.exports > 0 ? t("planner.card.impact_export", { count: impact.exports }) : null,
    impact.drafts > 0 ? t("planner.card.impact_draft", { count: impact.drafts }) : null,
    impact.projects > 0 ? t("planner.card.impact_project", { count: impact.projects }) : null,
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
  /** `draft_document` / `request_supplement`: drafts the 문서 생성 screen opens (P6); the supplement's project when known. */
  drafts: { index: number; draft: Draft; projectId: string | null }[];
  /**
   * Project ops: the project each payload refers to (link to `/projects/<id>`). `project` is null when the
   * shell could not load it any more (deleted meanwhile): the card then falls back to the id / context name.
   */
  projects: { index: number; projectId: string | null; project: Project | null; detail: ProjectDetail | null }[];
  /** `define_rule`: the parsed rule, its project and whether the executor saved it (`saved` badge). */
  ruleDrafts: { index: number; ruleDraft: RuleDraft; projectId: string | null; saved: boolean }[];
  checks: { index: number; run: CheckRun }[];
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
    drafts: [],
    projects: [],
    ruleDrafts: [],
    checks: [],
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
      case "explain":
      case "compare":
        if (isAnswer(payload)) out.answers.push({ index: step.index, answer: payload });
        break;
      case "draft_document":
      case "revise_draft":
      case "request_supplement": {
        // `exec.rs` sends the `DraftOutcome` (+ `project_id` for the supplement); a bare `Draft` is accepted too.
        const draft = isDraft(payload) ? payload : isRecord(payload) && isDraft(payload.draft) ? payload.draft : null;
        if (draft) out.drafts.push({ index: step.index, draft, projectId: (isRecord(payload) ? str(payload.project_id) : null) ?? (op && op.op === "request_supplement" ? (op.project ?? null) : null) });
        break;
      }
      case "create_project":
        if (isProject(payload)) out.projects.push({ index: step.index, projectId: payload.id, project: payload, detail: null });
        break;
      case "add_to_project": {
        // `exec.rs`: `{ project_id, added, project: Project | null }`; a `ProjectDetail` (browser mock) is accepted too.
        if (isProjectDetail(payload)) out.projects.push({ index: step.index, projectId: payload.project.id, project: payload.project, detail: payload });
        else if (isRecord(payload) && ("project_id" in payload || "project" in payload)) {
          const inner = payload.project;
          const detail = isProjectDetail(inner) ? inner : null;
          const project = detail ? detail.project : isProject(inner) ? inner : null;
          const projectId = str(payload.project_id) ?? project?.id ?? (op && op.op === "add_to_project" ? (op.project ?? null) : null);
          if (projectId || project) out.projects.push({ index: step.index, projectId, project, detail });
        }
        break;
      }
      case "define_rule": {
        // `exec.rs`: `{ project_id, saved, draft: RuleDraft }`; a bare `RuleDraft` is accepted too.
        const opProject = op && op.op === "define_rule" ? (op.project ?? null) : null;
        if (isRuleDraft(payload)) out.ruleDrafts.push({ index: step.index, ruleDraft: payload, projectId: opProject, saved: false });
        else if (isRecord(payload) && isRuleDraft(payload.draft)) out.ruleDrafts.push({ index: step.index, ruleDraft: payload.draft, projectId: str(payload.project_id) ?? opProject, saved: payload.saved === true });
        break;
      }
      case "check_consistency":
        if (isCheckRun(payload)) out.checks.push({ index: step.index, run: payload });
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

/**
 * The outcome of each step's UI action, by step index. The executor leaves UI ops to the UI: it lists
 * them in `ui_actions` in step order and marks their steps `ui` (`skipped` for clarify / noop), so the
 * k-th result belongs to the k-th such step.
 */
export function uiResultsByStep(execution: PlanExecution, results: UiOpResult[] | null | undefined): Map<number, UiOpResult> {
  const out = new Map<number, UiOpResult>();
  if (!results || results.length === 0) return out;
  let k = 0;
  for (const step of execution.steps) {
    const result = results[k];
    if (!result) break;
    const handedOver = step.status === "ui" || (step.status === "skipped" && (step.op === "clarify" || step.op === "noop"));
    if (!handedOver || result.op !== step.op) continue;
    out.set(step.index, result);
    k += 1;
  }
  return out;
}
