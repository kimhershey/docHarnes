// Browser mock of the P5 commands: the natural-language planner (pre-router →
// phrase rules → simulated model), plan previews and execution, organise /
// rename / move with the file-operation journal and undo / redo, aggregation,
// exports, and the workers of the persistent jobs (pipeline, plan, export,
// organise, undo, redo). Mirrors crates/doculens-core/src/{planner,organize,
// aggregate,export}.rs and src-tauri/src/planner_bridge.rs; the Rust code is
// the source of truth, this file only has to behave the same way for the UI.

import {
  cancelledError,
  currentWorkspacePath,
  docView,
  emit,
  err,
  gateInteractive,
  getJob,
  isoLocal,
  listModelIds,
  loadDocs,
  loadSettings,
  mockRoute,
  pace,
  pauseJob,
  planNeedsModel,
  prioritizeJob,
  relDir,
  resolveRelativeDate,
  resumeJob,
  retryJob,
  saveDocs,
  sleep,
  submitJob,
  tick,
  updateJob,
  upsertDoc,
  workerFailure,
  wsDocs,
  type JobControl,
  type JobSpec,
  type MockDoc,
  type WorkerOutcome,
} from "./mock-backend";
import { mockKnowledge } from "./mock-knowledge";
import * as pl from "./mock-pipeline";
import {
  DEFAULT_NAME_TEMPLATE,
  EVENTS,
  FOLDER_TEMPLATES,
  SCREEN_ROUTES,
  TEMPLATE_VARIABLES,
  type AggregateDocument,
  type AggregateFn,
  type AggregateGroup,
  type AggregateRequest,
  type AggregateResult,
  type Answer,
  type ApplyReport,
  type DateRange,
  type DocTypeInfo,
  type DocTypeSummary,
  type DocumentsChanged,
  type ExecuteResponse,
  type ExportFormat,
  type ExportPreview,
  type ExportRequest,
  type ExportResult,
  type ExportTarget,
  type FieldDef,
  type FieldRecord,
  type FieldSummary,
  type GroupBy,
  type Job,
  type JournalBatch,
  type JournalEntry,
  type JournalEntryKind,
  type Language,
  type Op,
  type OpKind,
  type OrganizeItem,
  type OrganizePlan,
  type OrganizeRequest,
  type PipelineChanged,
  type Plan,
  type PlanContext,
  type PlanExecution,
  type PlanIssue,
  type PlanPreview,
  type PlanResponse,
  type PlanResult,
  type ProcessRequest,
  type Scope,
  type SchemaRecord,
  type SchemaSummary,
  type Screen,
  type SearchResponse,
  type StepPreview,
  type StepResult,
  type Totals,
  type UiPlanContext,
  type UndoReport,
  type UndoResponse,
} from "./types";

/** File-operation journal (`{ batches, entries }`, batches tagged with their workspace). */
const JOURNAL_KEY = "doculens.mock.journal";
/** Export registry (`{ [workspace-relative path]: { format, documents, rows, created_at, workspace } }`). */
const EXPORTS_KEY = "doculens.mock.exports";
/** `on` = unrecognised commands are planned by the (simulated) model instead of asking back. */
const PLANNER_MODEL_KEY = "doculens.mock.planner_model";
/** `1` = deletions go to the OS recycle bin (journal entries without `trash_path`, undo skips them with `os_trash`). */
const OS_TRASH_KEY = "doculens.mock.os_trash";
const PLANNER_MODEL = "qwen3.5-4b-q6k";
/** Hard cap on operations per plan (§3.5 validator). */
const MAX_OPS = 12;
/** Plans below this confidence ask a clarification question instead of running (§3.5). */
const MIN_CONFIDENCE = 0.6;
/** Placeholder for a missing template value (design: "null은 `미상` + 검토 필요 표시"). */
export const UNKNOWN = "미상";
const MAX_PATH_CHARS = 240;
const MAX_COMPONENT_CHARS = 120;
/** Undo batches up to this size run inline (Ctrl+Z feel); larger ones become a job. */
const INLINE_UNDO_ENTRIES = 50;

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

function flag(key: string, value: string): boolean {
  try {
    return localStorage.getItem(key) === value;
  } catch {
    return false;
  }
}

function read<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

function write(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* ignore */
  }
}

function now(): string {
  return new Date().toISOString();
}

function todayIso(): string {
  return isoLocal(new Date());
}

function newId(): string {
  return crypto.randomUUID().replace(/-/g, "");
}

/** Lower-case, whitespace / punctuation stripped comparison key (`planner::compact`). */
export function compact(s: string): string {
  let out = "";
  for (const c of s.toLowerCase()) if (/[\p{L}\p{N}]/u.test(c)) out += c;
  return out;
}

function tr(ko: boolean, k: string, e: string): string {
  return ko ? k : e;
}

function countLabel(n: number, ko: boolean): string {
  return ko ? `문서 ${n}건` : `${n} document${n === 1 ? "" : "s"}`;
}

function basename(path: string): string {
  const i = path.lastIndexOf("/");
  return i >= 0 ? path.slice(i + 1) : path;
}

function stemOf(name: string): string {
  const i = name.lastIndexOf(".");
  return i > 0 ? name.slice(0, i) : name;
}

function isCancellation(e: unknown): boolean {
  const x = e as { code?: unknown; message?: unknown } | null;
  return x?.code === "cancelled" || x?.message === "cancelled";
}

function messageOf(e: unknown): string {
  const x = e as { message?: unknown } | null;
  return typeof x?.message === "string" ? x.message : String(e);
}

// ---------------------------------------------------------------------------
// Plans: op helpers (planner/mod.rs)
// ---------------------------------------------------------------------------

const UI_OPS: ReadonlySet<OpKind> = new Set<OpKind>(["define_schema", "set_model", "set_profile", "set_theme", "set_language", "set_setting", "open", "job", "engine", "help", "clarify", "noop", "reindex", "set_inbox_watch"]);

/** Ops the UI / shell performs itself (`OpKind::runs_in_ui`). */
export function runsInUi(kind: OpKind): boolean {
  return UI_OPS.has(kind);
}

/** Whether the op needs the confirmation card (`Op::needs_confirmation`). */
export function needsConfirmation(op: Op): boolean {
  switch (op.op) {
    case "organize":
      return op.dry_run !== true;
    case "rename":
    case "move":
    case "delete":
    case "undo":
    case "redo":
    case "define_schema":
      return true;
    case "job":
      return op.action === "cancel";
    case "engine":
      return op.action === "restart";
    case "review":
      return op.action === "approve" || op.action === "correct" || op.action === "reject";
    default:
      return false;
  }
}

export function emptyScope(): Scope {
  return { folder: null, doc_ids: [], filter: {} };
}

function filterIsEmpty(f: Scope["filter"] | undefined): boolean {
  if (!f) return true;
  return (
    f.doc_type == null &&
    f.date_from == null &&
    f.date_to == null &&
    f.keyword == null &&
    f.vendor == null &&
    f.status == null &&
    (f.tags ?? []).length === 0 &&
    f.min_confidence == null &&
    f.review_status == null
  );
}

export function scopeIsEmpty(scope: Scope): boolean {
  return scope.folder == null && (scope.doc_ids ?? []).length === 0 && filterIsEmpty(scope.filter);
}

export function isClarification(plan: Plan): boolean {
  return plan.intent === "clarify" || plan.operations.every((o) => o.op === "clarify") || (plan.operations.length === 0 && !!plan.clarification_question);
}

function single(op: Op, confidence: number, summary: string, scope: Scope = emptyScope()): Plan {
  return { intent: op.op, scope, operations: [op], requires_confirmation: needsConfirmation(op), confidence, clarification_question: null, summary };
}

function many(ops: Op[], scope: Scope, confidence: number, summary: string): Plan {
  return { intent: ops[0]?.op ?? "noop", scope, operations: ops, requires_confirmation: ops.some(needsConfirmation), confidence, clarification_question: null, summary };
}

function clarify(question: string, confidence: number): Plan {
  return { intent: "clarify", scope: emptyScope(), operations: [{ op: "clarify", question }], requires_confirmation: false, confidence, clarification_question: question, summary: null };
}

// ---------------------------------------------------------------------------
// Templates (organize/template.rs)
// ---------------------------------------------------------------------------

export type TemplateToken = { kind: "lit"; text: string } | { kind: "var"; name: string };

/** Splits a template into literal runs and `{{var}}` tokens (unknown names stay tokens). */
export function parseTemplate(template: string): TemplateToken[] {
  const out: TemplateToken[] = [];
  let lit = "";
  let rest = template;
  for (;;) {
    const start = rest.indexOf("{{");
    if (start < 0) break;
    lit += rest.slice(0, start);
    const after = rest.slice(start + 2);
    const end = after.indexOf("}}");
    if (end < 0) {
      lit += "{{";
      rest = after;
      continue;
    }
    if (lit) {
      out.push({ kind: "lit", text: lit });
      lit = "";
    }
    out.push({ kind: "var", name: after.slice(0, end).trim() });
    rest = after.slice(end + 2);
  }
  lit += rest;
  if (lit) out.push({ kind: "lit", text: lit });
  return out;
}

/** Variable names in the template that are not in `TEMPLATE_VARIABLES`. */
export function unknownVariables(template: string): string[] {
  const known = new Set<string>(TEMPLATE_VARIABLES);
  return [...new Set(parseTemplate(template).flatMap((t) => (t.kind === "var" && !known.has(t.name) ? [t.name] : [])))].sort();
}

const RESERVED = new Set(["con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9"]);

/** One path component made safe for Windows / NTFS and ext4: NFC, forbidden characters → `_`, no trailing dots / spaces, reserved names prefixed, length-capped. */
export function sanitizeComponent(s: string): string {
  let out = "";
  for (const c of s.normalize("NFC")) out += /[<>:"/\\|?*]/.test(c) || /\p{Cc}/u.test(c) ? "_" : c;
  out = out.trim().replace(/[. ]+$/, "");
  if (!out) out = "_";
  const stem = (out.split(".")[0] ?? "").toLowerCase();
  if (RESERVED.has(stem)) out = `_${out}`;
  const chars = Array.from(out);
  if (chars.length > MAX_COMPONENT_CHARS) out = chars.slice(0, MAX_COMPONENT_CHARS).join("");
  return out;
}

/** Sanitises every component of a relative folder path, dropping empty, `.` and `..` segments. */
export function sanitizeFolder(path: string): string {
  return path
    .replace(/\\/g, "/")
    .split("/")
    .map((c) => c.trim())
    .filter((c) => c && c !== "." && c !== "..")
    .map(sanitizeComponent)
    .join("/");
}

/** Renders a template; `lookup` returns a variable's value (`null` = unknown → `미상`). Also returns the missing variables. */
export function renderTemplate(template: string, lookup: (name: string) => string | null): { text: string; missing: string[] } {
  let text = "";
  const missing: string[] = [];
  for (const t of parseTemplate(template)) {
    if (t.kind === "lit") {
      text += t.text;
      continue;
    }
    const v = lookup(t.name);
    if (v !== null && v.trim() !== "") text += sanitizeComponent(v);
    else {
      text += UNKNOWN;
      missing.push(t.name);
    }
  }
  return { text, missing };
}

/** Four-digit year, two-digit month / day from an ISO date (`2026-10-14`; the day may be empty). */
export function dateParts(iso: string): { year: string; month: string; day: string } | null {
  const [y, m, d] = iso.split("-", 3).map((p) => p.trim());
  if (!y || !m || y.length !== 4 || !/^\d{4}$/.test(y)) return null;
  const mm = Number.parseInt(m, 10);
  if (!Number.isFinite(mm)) return null;
  const dd = Number.parseInt((d ?? "").slice(0, 2), 10) || 0;
  return { year: y, month: String(mm).padStart(2, "0"), day: dd === 0 ? "" : String(dd).padStart(2, "0") };
}

/** Amount formatting for file names: integer with thousands separators (`12,500`). */
export function formatTotal(amount: number): string {
  const neg = amount < 0;
  const s = String(Math.round(Math.abs(amount)));
  let out = "";
  for (let i = 0; i < s.length; i += 1) {
    if (i > 0 && (s.length - i) % 3 === 0) out += ",";
    out += s[i];
  }
  return neg ? `-${out}` : out;
}

/** Number formatting of the aggregate card (`aggregate::format_value`). */
export function formatValue(func: AggregateFn, value: number): string {
  if (!Number.isFinite(value)) return "0";
  const sign = value < 0 ? "-" : "";
  const abs = Math.abs(value);
  if (Math.abs(abs - Math.round(abs)) < 1e-9) return `${sign}${formatTotal(Math.round(abs))}`;
  const text = func === "avg" ? abs.toFixed(2) : String(abs);
  const [int, frac] = text.split(".");
  return frac ? `${sign}${formatTotal(Number(int))}.${frac}` : `${sign}${text}`;
}

// ---------------------------------------------------------------------------
// Phrase rules (planner/rules.rs): deterministic plans for the frequent commands
// ---------------------------------------------------------------------------

interface Text {
  raw: string;
  lower: string;
  compact: string;
  ko: boolean;
}

function mkText(raw: string, ko: boolean): Text {
  return { raw, lower: raw.toLowerCase(), compact: compact(raw), ko };
}

function has(t: Text, words: readonly string[]): boolean {
  return words.some((w) => t.compact.includes(compact(w)));
}

/** True for sentences that chain several commands ("…하고 …", "… and …"). */
export function isCompound(text: string): boolean {
  const t = text.trim();
  return t.includes(" and ") || t.includes("그리고") || t.includes("고 ") || t.includes(", ");
}

function splitClauses(text: string): string[] {
  const parts: string[] = [];
  for (const piece of text.split(" and ").flatMap((p) => p.split("그리고")).flatMap((p) => p.split(", "))) {
    // Korean "-고 " connective: "바꾸고 모델을…" → "바꾸", "모델을…".
    let rest = piece;
    for (;;) {
      const idx = rest.indexOf("고 ");
      if (idx < 0) break;
      const head = rest.slice(0, idx);
      if (head.trim()) parts.push(head.trim());
      rest = rest.slice(idx + 2).trimStart();
    }
    if (rest.trim()) parts.push(rest.trim());
  }
  return parts;
}

const SELECTION_WORDS = ["이문서", "이계약서", "이영수증", "이파일", "선택문서", "선택한", "선택된", "현재문서", "열린문서", "지금문서", "선택영역", "이페이지", "현재페이지", "thisdocument", "thisfile", "thispage", "selected", "theselection", "thesedocuments", "currentdocument", "thiscontract", "thisreceipt"];
const ALL_WORDS = ["전부", "전체", "모두", "모든", "다 ", "라이브러리전체", "alldocuments", "everything", "all "];

function selectionIds(ctx: PlanContext): string[] {
  if (ctx.selected_doc_ids.length > 0) return [...ctx.selected_doc_ids];
  return ctx.current_document ? [ctx.current_document] : [];
}

/** Longest document-type label (either language, or the slug) mentioned in the text. */
function docTypeOf(t: Text, ctx: PlanContext): DocTypeSummary | null {
  let best: { d: DocTypeSummary; len: number } | null = null;
  for (const d of ctx.doc_types) {
    for (const label of [d.label_ko, d.label_en, d.slug]) {
      const key = compact(label);
      const len = Array.from(key).length;
      if (len >= 2 && t.compact.includes(key) && (best === null || len > best.len)) best = { d, len };
    }
  }
  return best?.d ?? null;
}

/** Fields of a schema mentioned in the text, in order of appearance. */
function fieldsMentioned(t: Text, schema: SchemaSummary): FieldSummary[] {
  const found: { pos: number; len: number; f: FieldSummary }[] = [];
  for (const f of schema.fields) {
    // The label without a trailing generic word still identifies the field: "공급가액 합계" ← "공급가액".
    let trimmed = "";
    const labelKo = f.label_ko.trimEnd();
    for (const g of ["합계", "금액", "번호", "일자", "총액", "명", "액"]) {
      if (labelKo.endsWith(g)) {
        const s = labelKo.slice(0, labelKo.length - g.length).trim();
        trimmed = Array.from(s).length >= 2 ? s : "";
        break;
      }
    }
    let best: { pos: number; len: number } | null = null;
    for (const label of [f.label_ko, f.label_en, f.key, trimmed]) {
      const key = compact(label);
      if (Array.from(key).length < 2) continue;
      const pos = t.compact.indexOf(key);
      if (pos < 0) continue;
      // Prefer the shortest matching form so a generic tail ("합계") is not swallowed.
      if (best === null || key.length < best.len) best = { pos, len: key.length };
    }
    if (best) found.push({ ...best, f });
  }
  found.sort((a, b) => a.pos - b.pos || b.len - a.len);
  const out: FieldSummary[] = [];
  let coveredUntil = 0;
  for (const { pos, len, f } of found) {
    if (pos < coveredUntil) continue;
    coveredUntil = pos + len;
    out.push(f);
  }
  return out;
}

function totalField(schema: SchemaSummary): FieldSummary | null {
  const amounts = schema.fields.filter((f) => f.kind === "amount");
  return amounts.find((f) => f.key.includes("total") || f.label_ko.includes("합계") || f.label_ko.includes("총액")) ?? amounts[0] ?? null;
}

function dateOf(t: Text, ctx: PlanContext): DateRange | null {
  return resolveRelativeDate(t.raw, new Date(`${ctx.today}T12:00:00`));
}

function applyDate(scope: Scope, d: DateRange | null): void {
  if (!d) return;
  scope.filter = { ...(scope.filter ?? {}), date_from: d.from, date_to: d.to };
}

/** Scope from the sentence: selection words → selected ids; `inbox` → that folder; otherwise the type / date filter. */
function scopeOf(t: Text, ctx: PlanContext, dt: DocTypeSummary | null, d: DateRange | null): Scope {
  const scope = emptyScope();
  if (has(t, SELECTION_WORDS)) scope.doc_ids = selectionIds(ctx);
  if (has(t, ["inbox", "인박스", "받은함"])) scope.folder = "inbox";
  else if (has(t, ["이폴더", "여기있는", "현재폴더", "thisfolder", "currentfolder", "here"])) scope.folder = ctx.current_folder;
  if (dt) scope.filter = { ...(scope.filter ?? {}), doc_type: dt.slug };
  applyDate(scope, d);
  return scope;
}

function typeLabel(dt: DocTypeSummary | null, ko: boolean, fallback: string): string {
  return dt ? (ko ? dt.label_ko : dt.label_en) : fallback;
}

function screenRef(screen: Screen): string {
  const ref = SCREEN_ROUTES[screen].replace(/^\//, "");
  return ref || "home";
}

function ruleUndoRedo(t: Text): Plan | null {
  if (has(t, ["되돌려", "되돌리", "되돌림", "원래대로", "undo", "revert"])) return single({ op: "undo", target: null }, 0.95, tr(t.ko, "마지막 파일 작업을 되돌립니다", "Undo the last file batch"));
  if (has(t, ["다시실행", "redo"]) && !has(t, ["추출", "ocr", "분류", "처리", "extract", "classify", "process"])) return single({ op: "redo" }, 0.95, tr(t.ko, "되돌린 작업을 다시 실행합니다", "Redo the last undone batch"));
  return null;
}

function ruleJobControl(t: Text): Plan | null {
  const jobWord = has(t, ["작업", "job", "배치", "batch", "처리를", "큐"]);
  let action: "pause" | "resume" | "retry" | "prioritize" | "cancel";
  if (has(t, ["일시정지", "일시중지", "멈춰", "잠깐멈", "pause", "hold"])) action = "pause";
  else if (has(t, ["재개", "계속해", "이어서", "resume", "continue"])) action = "resume";
  else if (has(t, ["재시도", "다시시도", "retry"])) action = "retry";
  else if (has(t, ["우선", "먼저해", "prioriti", "firstplease", "bumpup"])) action = "prioritize";
  else if (jobWord && has(t, ["취소", "중단", "cancel", "stop", "abort"])) action = "cancel";
  else if (jobWord && has(t, ["보여", "목록", "현황", "상태", "show", "list", "status"])) return single({ op: "open", target: "screen", ref: "jobs" }, 0.9, tr(t.ko, "작업 화면을 엽니다", "Open the jobs screen"));
  else return null;
  if (!jobWord && action !== "pause" && action !== "resume") return null;
  const what = {
    pause: tr(t.ko, "실행 중인 작업을 일시정지합니다", "Pause the running job"),
    resume: tr(t.ko, "일시정지한 작업을 재개합니다", "Resume the paused job"),
    retry: tr(t.ko, "실패한 작업을 재시도합니다", "Retry the failed job"),
    prioritize: tr(t.ko, "대기 중인 작업을 앞으로 당깁니다", "Move the queued job to the front"),
    cancel: tr(t.ko, "실행 중인 작업을 취소합니다", "Cancel the running job"),
  }[action];
  return single({ op: "job", action, id: "last" }, 0.92, what);
}

function ruleEngine(t: Text): Plan | null {
  if (!has(t, ["엔진", "engine", "llama"])) return null;
  let action: "restart" | "sleep" | "wake";
  if (has(t, ["재시작", "다시시작", "restart", "reboot"])) action = "restart";
  else if (has(t, ["슬립", "재워", "sleep", "잠재"])) action = "sleep";
  else if (has(t, ["깨워", "wake"])) action = "wake";
  else return null;
  return single({ op: "engine", action }, 0.9, tr(t.ko, "엔진을 제어합니다", "Control the engine"));
}

function modelId(t: Text, ctx: PlanContext): string | null {
  if (!has(t, ["모델", "model"]) || ctx.model_ids.length === 0) return null;
  // "9B", "4b", "q4" or a chunk of the id itself.
  let best: { id: string; len: number } | null = null;
  for (const id of ctx.model_ids) {
    if (t.compact.includes(compact(id))) return id;
    for (const part of id.split(/[-_.]/).map(compact).filter((p) => p.length >= 2)) {
      if (/\d/.test(part) && t.compact.includes(part) && (best === null || part.length > best.len)) best = { id, len: part.length };
    }
  }
  return best?.id ?? null;
}

/** Several settings in one sentence ("다크 테마로 바꾸고 모델을 9B로 올려"). */
function ruleSettingsCombo(t: Text, ctx: PlanContext): Plan | null {
  const ops: Op[] = [];
  for (const clause of splitClauses(t.raw)) {
    const ct = mkText(clause, t.ko);
    const id = modelId(ct, ctx);
    if (id) {
      ops.push({ op: "set_model", id });
      continue;
    }
    const r = mockRoute(clause);
    if (r.op === "set_theme") ops.push({ op: "set_theme", theme: r.theme });
    else if (r.op === "set_language") ops.push({ op: "set_language", lang: r.language });
    else if (r.op === "open") ops.push({ op: "open", target: "screen", ref: screenRef(r.screen) });
    else if (r.op === "help") ops.push({ op: "help" });
    else return null;
  }
  if (ops.length === 0 || (ops.length === 1 && ops[0]?.op !== "set_model")) return null;
  const summary = ops
    .map((o) => {
      switch (o.op) {
        case "set_model":
          return tr(t.ko, `모델을 ${o.id}(으)로`, `model → ${o.id}`);
        case "set_theme":
          return tr(t.ko, `테마를 ${o.theme}(으)로`, `theme → ${o.theme}`);
        case "set_language":
          return tr(t.ko, `언어를 ${o.lang}(으)로`, `language → ${o.lang}`);
        case "open":
          return tr(t.ko, `${o.ref} 화면 열기`, `open ${o.ref}`);
        default:
          return "";
      }
    })
    .filter(Boolean)
    .join(", ");
  return many(ops, emptyScope(), 0.9, summary);
}

function schemaForType(ctx: PlanContext, slug: string): SchemaSummary | null {
  return ctx.schemas.find((s) => s.doc_type === slug) ?? null;
}

function ruleDefineSchema(t: Text, ctx: PlanContext): Plan | null {
  if (!has(t, ["스키마", "schema", "양식필드", "추출필드"])) return null;
  if (!has(t, ["추가", "만들", "생성", "수정", "바꿔", "변경", "정의", "add", "create", "modify", "change", "define", "new"])) return null;
  const dt = docTypeOf(t, ctx);
  const schemaRef = (dt && schemaForType(ctx, dt.slug)?.name) ?? dt?.slug ?? null;
  const modify = schemaRef !== null && has(t, ["추가", "수정", "바꿔", "변경", "add", "modify", "change"]);
  const op: Op = { op: "define_schema", name: null, description: t.raw.trim(), mode: modify ? "modify" : "create", base: modify ? schemaRef : null };
  const summary =
    modify && schemaRef
      ? tr(t.ko, `${schemaRef} 스키마를 수정합니다(스키마 화면에서 확인)`, `Modify the ${schemaRef} schema (confirm on the schema screen)`)
      : tr(t.ko, "새 추출 스키마를 만듭니다(스키마 화면에서 확인)", "Create a new extraction schema (confirm on the schema screen)");
  return single(op, 0.85, summary);
}

function exportFormatOf(t: Text): ExportFormat | null {
  if (has(t, ["검색가능pdf", "검색가능한pdf", "searchablepdf", "텍스트레이어", "textlayer", "검색되는pdf"])) return "pdf_searchable";
  if (has(t, ["엑셀", "excel", "xlsx", "스프레드시트", "spreadsheet"])) return "xlsx";
  if (has(t, ["csv"])) return "csv";
  if (has(t, ["jsonl", "json"])) return "jsonl";
  if (has(t, ["마크다운", "markdown", "md파일", "md로", ".md"])) return "md";
  return null;
}

function ruleExport(t: Text, ctx: PlanContext): Plan | null {
  const format = exportFormatOf(t);
  if (!format) return null;
  const verb = has(t, ["내보내", "저장", "뽑아", "추출해서", "만들어", "출력", "export", "save", "dump", "write"]) || has(t, ["로", "으로", "→", "->", "to", "into", "as"]);
  if (!verb) return null;
  const dt = docTypeOf(t, ctx);
  const d = dateOf(t, ctx);
  const scope = scopeOf(t, ctx, dt, d);
  if (scope.folder == null && (scope.doc_ids ?? []).length === 0 && !dt && !d && !has(t, ALL_WORDS)) scope.folder = ctx.current_folder;
  const target: ExportTarget = has(t, ["표를", "표만", "테이블", "table"]) ? "table" : has(t, ["텍스트", "본문", "ocr결과", "text"]) || format === "pdf_searchable" ? "text" : "fields";
  const schema = dt ? schemaForType(ctx, dt.slug) : null;
  const columns = schema && target === "fields" && !has(t, ALL_WORDS) ? fieldsMentioned(t, schema).map((f) => f.key) : [];
  const ops: Op[] = [];
  if (columns.length > 0) ops.push({ op: "extract", schema_ref: dt?.slug ?? null, fields: [...columns], force: null });
  ops.push({ op: "export", format, path: null, columns: [...columns], layout: null, include: [], target });
  const label = typeLabel(dt, t.ko, tr(t.ko, "문서", "documents"));
  const fmt = format.toUpperCase();
  const summary = t.ko
    ? columns.length === 0
      ? `${label}을(를) ${fmt}(으)로 내보냅니다`
      : `${label}에서 ${columns.length}개 필드를 추출해 ${fmt}(으)로 내보냅니다`
    : columns.length === 0
      ? `Export ${label} as ${fmt}`
      : `Extract ${columns.length} fields of ${label} and export them as ${fmt}`;
  return many(ops, scope, 0.88, summary);
}

const AGGREGATE_TRIGGERS = ["합계", "총합", "총액", "총금액", "얼마", "집계", "평균", "최대", "최소", "건수", "몇건", "몇개", "개수", "count", "total", "sum", "average", "howmuch", "howmany", "highest", "lowest", "largest", "smallest"];

function ruleAggregate(t: Text, ctx: PlanContext): Plan | null {
  if (!has(t, AGGREGATE_TRIGGERS)) return null;
  // "합계 금액은?" on its own is a question about a document, not a library-wide total (mirrors rules.rs):
  // aggregate only when the sentence names a type, a period, a grouping or the whole library.
  const scoped =
    docTypeOf(t, ctx) !== null ||
    dateOf(t, ctx) !== null ||
    has(t, ["월별", "거래처별", "유형별", "monthly", "byvendor", "bytype", "집계", "총합", "모든", "모두", "전부", "전체", "across", "all "]) ||
    (selectionIds(ctx).length > 0 && has(t, SELECTION_WORDS));
  if (!scoped) return null;
  const func: AggregateFn = has(t, ["평균", "average", "avg", "mean"])
    ? "avg"
    : has(t, ["최대", "가장큰", "가장비싼", "max", "highest", "largest"])
      ? "max"
      : has(t, ["최소", "가장작은", "가장싼", "min", "lowest", "smallest"])
        ? "min"
        : has(t, ["건수", "몇건", "몇개", "개수", "count", "howmany", "number of"])
          ? "count"
          : "sum";
  const dt = docTypeOf(t, ctx);
  const d = dateOf(t, ctx);
  const scope = scopeOf(t, ctx, dt, d);
  const schema = dt ? schemaForType(ctx, dt.slug) : null;
  let field: string | null = null;
  if (func !== "count" && schema) {
    const mentioned = fieldsMentioned(t, schema).filter((f) => f.kind === "amount" || f.kind === "integer");
    field = (mentioned[0] ?? totalField(schema))?.key ?? null;
  }
  const groupBy: GroupBy | null = has(t, ["월별", "달별", "monthly", "bymonth", "permonth"]) ? "month" : has(t, ["거래처별", "업체별", "byvendor", "pervendor"]) ? "vendor" : has(t, ["유형별", "종류별", "bytype", "pertype"]) ? "doc_type" : null;
  const includeUnreviewed = has(t, ["검토대기포함", "검토미완료포함", "미검토포함", "대기포함", "includeunreviewed", "includingunreviewed"]) ? true : null;
  const label = typeLabel(dt, t.ko, tr(t.ko, "문서", "documents"));
  const fieldDef = field && schema ? schema.fields.find((f) => f.key === field) : undefined;
  const fname = fieldDef ? (t.ko ? fieldDef.label_ko : fieldDef.label_en) : null;
  const when = d ? `${d.from} ~ ${d.to}` : null;
  const summary = t.ko
    ? `${when ? `${when} ` : ""}${label}의 ${fname ?? "건수"}${{ sum: " 합계", count: "", avg: " 평균", min: " 최소값", max: " 최대값" }[func]}`
    : `${func} of ${fname ? `${fname} of ` : ""}${label}${when ? ` (${when})` : ""}`;
  return single({ op: "aggregate", fn: func, field, group_by: groupBy, include_unreviewed: includeUnreviewed }, 0.9, summary, scope);
}

function ruleInbox(t: Text, ctx: PlanContext): Plan | null {
  if (!has(t, ["inbox", "인박스", "받은함", "받은문서"])) return null;
  if (!has(t, ["처리", "가져", "불러", "import", "process", "정리", "분류", "추출", "ocr", "읽어", "등록", "run", "handle", "전부", "모두"])) return null;
  const dt = docTypeOf(t, ctx);
  const scope = emptyScope();
  scope.folder = "inbox";
  if (dt) scope.filter = { doc_type: dt.slug };
  const ops: Op[] = [{ op: "import", source: "inbox", path: null, recursive: null, auto_pipeline: "none" }];
  const onlyClassify = has(t, ["분류만", "classifyonly"]);
  const onlyOcr = has(t, ["ocr만", "ocronly"]);
  const wantsProcessing = has(t, ["처리", "process", "추출", "extract", "전부", "모두", "ocr", "분류", "classif", "read", "읽어"]);
  if (wantsProcessing || dt) {
    ops.push({ op: "classify", doc_types: [], force: null });
    if (!onlyClassify) {
      ops.push({ op: "ocr", pages: [], profile: null, force: null });
      if (!onlyOcr) ops.push({ op: "extract", schema_ref: dt?.slug ?? null, fields: [], force: null });
    }
  }
  const group: GroupBy | null = has(t, ["월별", "달별", "monthly", "bymonth"])
    ? "month"
    : has(t, ["거래처별", "업체별", "byvendor"])
      ? "vendor"
      : has(t, ["유형별", "종류별", "bytype", "정리", "폴더로", "옮겨", "이동", "organi", "move", "sort into"])
        ? "doc_type"
        : null;
  if (group) ops.push({ op: "move", folder: FOLDER_TEMPLATES[group], group_by: group, only_reviewed: null });
  const label = typeLabel(dt, t.ko, tr(t.ko, "문서", "documents"));
  const summary = t.ko
    ? `inbox의 ${label}을(를) 가져와 ${ops.length > 1 ? "분류·OCR·추출한 뒤 " : "등록하고 "}${group === "month" ? "월별 폴더로 이동합니다" : group === "vendor" ? "거래처별 폴더로 이동합니다" : group ? "유형별 폴더로 이동합니다" : "마칩니다"}`
    : `Import ${label} from inbox${ops.length > 1 ? ", classify, OCR and extract them" : ""}${group === "month" ? ", then move them into monthly folders" : group === "vendor" ? ", then move them into vendor folders" : group ? ", then move them into type folders" : ""}`;
  return many(ops, scope, 0.88, summary);
}

function groupWord(g: GroupBy, ko: boolean): string {
  if (g === "month") return ko ? "월별" : "monthly";
  if (g === "vendor") return ko ? "거래처별" : "vendor";
  return ko ? "유형별" : "type";
}

function ruleOrganize(t: Text, ctx: PlanContext): Plan | null {
  const rename = has(t, ["이름바꿔", "이름을바꿔", "이름변경", "파일명", "rename", "이름을"]);
  const organizeWord = has(t, ["정리", "organi", "옮겨", "옮기", "이동", "move", "폴더로", "sortinto", "tidy"]);
  if (!rename && !organizeWord) return null;
  if (has(t, ["작업", "job", "스키마", "schema", "태그", "tag"]) && !has(t, ["문서", "파일", "document", "file", "영수증", "receipt"])) return null;
  const dt = docTypeOf(t, ctx);
  const d = dateOf(t, ctx);
  const scope = scopeOf(t, ctx, dt, d);
  const onlyReviewed = has(t, ["검토완료", "검토된", "reviewedonly", "onlyreviewed"]) ? true : null;
  const group: GroupBy | null = has(t, ["월별", "달별", "monthly", "bymonth"]) ? "month" : has(t, ["거래처별", "업체별", "byvendor"]) ? "vendor" : has(t, ["유형별", "종류별", "bytype"]) ? "doc_type" : null;
  // "X 폴더로 옮겨": an explicit folder name.
  const words = t.raw.split(/\s+/).filter(Boolean);
  let explicit: string | null = null;
  for (let i = 0; i + 1 < words.length; i += 1) {
    const name = (words[i] ?? "").replace(/^["'“”‘’]+|["'“”‘’]+$/g, "");
    if (compact(words[i + 1] ?? "").startsWith("폴더") && name && !has(t, ["월별", "거래처별", "유형별"]) && !ctxDocType(ctx, name)) {
      explicit = name;
      break;
    }
  }
  const label = typeLabel(dt, t.ko, tr(t.ko, "문서", "documents"));
  let op: Op;
  let summary: string;
  if (rename && !group && explicit === null) {
    op = { op: "rename", template: DEFAULT_NAME_TEMPLATE, only_reviewed: onlyReviewed };
    summary = tr(t.ko, `${label}의 파일명을 템플릿에 맞춰 바꿉니다`, `Rename ${label} with the name template`);
  } else if (explicit !== null) {
    const folder = explicit.includes("/") ? explicit : `documents/${explicit}`;
    op = { op: "move", folder, group_by: null, only_reviewed: onlyReviewed };
    summary = tr(t.ko, `${label}을(를) ${folder}/ 로 이동합니다`, `Move ${label} to ${folder}/`);
  } else if (rename) {
    const g = group ?? "doc_type";
    op = { op: "organize", template: DEFAULT_NAME_TEMPLATE, folder_template: FOLDER_TEMPLATES[g], group_by: g, only_reviewed: onlyReviewed, dry_run: null };
    summary = tr(t.ko, `${label}의 이름을 바꾸고 ${groupWord(g, true)} 폴더로 정리합니다`, `Rename ${label} and sort them into ${groupWord(g, false)} folders`);
  } else {
    const g = group ?? "doc_type";
    op = { op: "move", folder: FOLDER_TEMPLATES[g], group_by: g, only_reviewed: onlyReviewed };
    summary = tr(t.ko, `${label}을(를) ${groupWord(g, true)} 폴더로 이동합니다`, `Move ${label} into ${groupWord(g, false)} folders`);
  }
  return single(op, 0.86, summary, scope);
}

/** Resolves a document type mentioned by slug or label (`PlanContext::doc_type`). */
function ctxDocType(ctx: PlanContext, text: string): DocTypeSummary | null {
  const key = compact(text);
  if (!key) return null;
  return ctx.doc_types.find((d) => compact(d.slug) === key || compact(d.label_ko) === key || compact(d.label_en) === key) ?? null;
}

/** Resolves a schema by id, name (either language) or doc-type slug (`PlanContext::schema`). */
function ctxSchema(ctx: PlanContext, reference: string): SchemaSummary | null {
  const key = compact(reference);
  return ctx.schemas.find((s) => s.id === reference || compact(s.name) === key || compact(s.name_en) === key || compact(s.doc_type) === key) ?? null;
}

/** Resolves a field of a schema by key or label (`PlanContext::field`). */
function ctxField(schema: SchemaSummary, text: string): FieldSummary | null {
  const key = compact(text);
  return schema.fields.find((f) => compact(f.key) === key || compact(f.label_ko) === key || compact(f.label_en) === key) ?? null;
}

function ruleTag(t: Text, ctx: PlanContext): Plan | null {
  if (!has(t, ["태그", "tag"])) return null;
  const remove = has(t, ["제거", "빼", "삭제", "지워", "remove", "untag", "delete"]);
  const add = has(t, ["추가", "붙여", "달아", "넣어", "add", "tagas", "tagwith", "apply"]);
  if (!remove && !add) return null;
  // Tag names: the words between 태그/tag and the verb, quotes stripped.
  const ko = t.lower.indexOf("태그");
  const en = t.lower.indexOf("tag");
  const start = ko >= 0 ? ko + 2 : en >= 0 ? en + 3 : -1;
  if (start < 0) return null;
  let rest = t.raw.slice(start);
  for (const stop of ["추가", "붙여", "달아", "넣어", "제거", "빼", "삭제", "지워", " add", " remove", " to ", " from "]) {
    const i = rest.indexOf(stop);
    if (i >= 0) rest = rest.slice(0, i);
  }
  const names = rest
    .split(/[,·/]/)
    .map((s) => s.trim().replace(/^["'“”‘’()[\]]+|["'“”‘’()[\]]+$/g, "").trim())
    .map((s) => s.replace(/[을를로]+$/, "").replace(/^[를을]+/, "").trim())
    .filter((s) => s && Array.from(s).length <= 64);
  if (names.length === 0) return null;
  const dt = docTypeOf(t, ctx);
  const scope = scopeOf(t, ctx, dt, null);
  const op: Op = remove ? { op: "tag", add: [], remove: names } : { op: "tag", add: names, remove: [] };
  const summary = tr(t.ko, `태그 ${names.join(", ")}을(를) ${remove ? "제거합니다" : "추가합니다"}`, `${remove ? "Remove" : "Add"} tag(s) ${names.join(", ")}`);
  return single(op, 0.85, summary, scope);
}

function ruleDelete(t: Text, ctx: PlanContext): Plan | null {
  if (!has(t, ["삭제", "지워", "지우", "delete", "remove", "trash", "휴지통"])) return null;
  if (has(t, ["스키마", "schema", "태그", "tag", "작업", "job", "컬렉션", "collection", "모델", "model", "용어", "glossary"])) return null;
  const dt = docTypeOf(t, ctx);
  const d = dateOf(t, ctx);
  const scope = scopeOf(t, ctx, dt, d);
  if (scopeIsEmpty(scope)) return clarify(tr(t.ko, "어떤 문서를 삭제할까요? 라이브러리에서 선택하거나 유형·날짜를 말씀해 주세요.", "Which documents should be deleted? Select them in the library or name a type / date range."), 0.5);
  const label = typeLabel(dt, t.ko, tr(t.ko, "선택 문서", "the selected documents"));
  return single({ op: "delete", to_trash: true }, 0.85, tr(t.ko, `${label}을(를) 휴지통으로 보냅니다`, `Move ${label} to the recycle bin`), scope);
}

function ruleProcess(t: Text, ctx: PlanContext): Plan | null {
  const classify = has(t, ["분류", "classif", "유형판정", "유형을정"]);
  const ocr = has(t, ["ocr", "문자인식", "텍스트인식", "글자인식", "텍스트추출", "읽어"]);
  const extract = has(t, ["추출", "extract", "필드뽑", "값뽑", "kie"]) && !has(t, ["텍스트추출"]);
  const validate = has(t, ["검증", "validate", "검사해"]);
  const summarize = has(t, ["요약", "summar"]);
  const generic = has(t, ["처리", "process", "돌려", "실행해"]) && !classify && !ocr && !extract && !summarize;
  if (!(classify || ocr || extract || validate || summarize || generic)) return null;
  const force = has(t, ["다시", "재", "again", "re-", "redo", "force"]);
  const dt = docTypeOf(t, ctx);
  const d = dateOf(t, ctx);
  const scope = scopeOf(t, ctx, dt, d);
  if (scopeIsEmpty(scope) && !has(t, ALL_WORDS)) {
    const sel = selectionIds(ctx);
    if (sel.length > 0 && (has(t, ["이", "선택", "this", "selected"]) || summarize || ocr)) scope.doc_ids = sel;
    else if (ctx.current_folder && has(t, ["여기", "here"])) scope.folder = ctx.current_folder;
    else if (sel.length > 0) scope.doc_ids = sel;
    else return clarify(tr(t.ko, "어떤 문서를 처리할까요? 라이브러리에서 선택하거나 폴더·유형·날짜를 말씀해 주세요.", "Which documents? Select them in the library or name a folder / type / date range."), 0.5);
  }
  const ops: Op[] = [];
  if (summarize) {
    const target = has(t, ["하나로", "합쳐", "묶어", "전체요약", "together", "onesummary", "combined"]) ? "set" : "each_doc";
    const length = has(t, ["짧게", "한줄", "간단", "short", "brief"]) ? "short" : has(t, ["길게", "자세", "상세", "long", "detailed"]) ? "long" : null;
    ops.push({ op: "summarize", target, length });
  }
  if (classify || generic) ops.push({ op: "classify", doc_types: dt ? [dt.slug] : [], force: force ? true : null });
  if (ocr || generic) {
    const profile = has(t, ["잔글씨", "작은글씨", "fineprint", "영역", "region", "잘안읽"]) ? "fine_print" : null;
    const pages = has(t, ["이페이지", "현재페이지", "선택영역", "thispage", "selection"]) && ctx.current_page != null ? [ctx.current_page] : [];
    ops.push({ op: "ocr", pages, profile, force: force || ocr ? true : null });
  }
  if (extract || generic) ops.push({ op: "extract", schema_ref: dt?.slug ?? null, fields: [], force: force ? true : null });
  if (validate) ops.push({ op: "validate" });
  if (ops.length === 0) return null;
  const label = (scope.doc_ids ?? []).length > 0 ? tr(t.ko, "선택 문서", "the selected documents") : typeLabel(dt, t.ko, tr(t.ko, "문서", "documents"));
  const what = ops.map((o) => ({ summarize: tr(t.ko, "요약", "summarise"), classify: tr(t.ko, "분류", "classify"), ocr: "OCR", extract: tr(t.ko, "추출", "extract"), validate: tr(t.ko, "검증", "validate") })[o.op as "summarize" | "classify" | "ocr" | "extract" | "validate"] ?? "");
  const summary = t.ko ? `${label}을(를) ${what.join("·")}합니다${force ? "(다시 실행)" : ""}` : `${what.join(", ")} ${label}${force ? " (re-run)" : ""}`;
  return many(ops, scope, 0.87, summary);
}

function ruleReview(t: Text): Plan | null {
  if (!has(t, ["검토", "review"])) return null;
  const only = has(t, ["저신뢰", "신뢰도낮", "lowconfidence"]) ? "low_confidence" : has(t, ["검증실패", "실패한", "failed"]) ? "failed" : has(t, ALL_WORDS) ? "all" : null;
  if (has(t, ["승인", "approve", "accept"])) return single({ op: "review", action: "approve", field: null, value: null, only }, 0.85, tr(t.ko, "검토 대기 필드를 일괄 승인합니다", "Approve the pending review fields"));
  if (has(t, ["거부", "reject"])) return single({ op: "review", action: "reject", field: null, value: null, only }, 0.85, tr(t.ko, "검토 대기 필드를 일괄 거부합니다", "Reject the pending review fields"));
  if (has(t, ["보여", "열어", "큐", "목록", "show", "open", "queue", "list"])) return single({ op: "open", target: "screen", ref: "review" }, 0.9, tr(t.ko, "검토 큐를 엽니다", "Open the review queue"));
  return null;
}

const SEARCH_FRAMES = ["거래처명이", "거래처가", "업체명이", "상호가", "제목이", "문서명이", "파일명이", "관련된", "관련", "포함된", "포함한", "포함하는", "들어있는", "들어간", "언급된", "문서들", "문서를", "문서", "파일을", "파일", "자료", "전부", "모두", "찾아줘", "찾아주세요", "찾아봐", "찾아", "찾기", "검색해줘", "검색해", "검색", "해줘", "주세요", "좀", "please", "documents", "document", "files", "file", "find", "search", "look for", "mentioning", "containing", "about", "from", "with", "for", "all", "the", "me"];

function ruleSearch(t: Text, ctx: PlanContext): Plan | null {
  if (!has(t, ["찾아", "찾기", "검색", "find", "search", "lookfor", "whereis", "locate"])) return null;
  // "거래처명이 X인 문서" / "find documents from X": strip the frame words.
  let q = t.raw.trim();
  for (const frame of SEARCH_FRAMES) q = q.split(frame).join(" ");
  q = q
    .split(/\s+/)
    .map((w) => w.replace(/^["'“”‘’]+|["'“”‘’]+$/g, ""))
    .map((w) => (Array.from(w).length > 2 ? w.replace(/[인의을를이가은는]+$/, "") : w))
    .filter(Boolean)
    .join(" ")
    .trim()
    .replace(/[?？.]+$/, "")
    .trim();
  if (!q) return single({ op: "open", target: "screen", ref: "search" }, 0.8, tr(t.ko, "검색 화면을 엽니다", "Open the search screen"));
  const d = dateOf(t, ctx);
  const dt = docTypeOf(t, ctx);
  const scope = scopeOf(t, ctx, dt, d);
  return single({ op: "search", query: q, mode: null, top_k: null }, 0.85, tr(t.ko, `'${q}'을(를) 검색합니다`, `Search for '${q}'`), scope);
}

function ruleOpenDocument(t: Text): Plan | null {
  if (!has(t, ["열어", "열기", "open", "띄워"]) || !has(t, ["문서", "파일", "document", "file", ".pdf", ".jpg", ".png", ".hwpx", ".docx"])) return null;
  const name = t.raw
    .split(/\s+/)
    .map((w) => w.replace(/^["'“”‘’]+|["'“”‘’]+$/g, ""))
    .find((w) => w.includes(".") && w.length > 3);
  if (!name) return null;
  return single({ op: "open", target: "document", ref: name }, 0.85, tr(t.ko, `${name} 문서를 엽니다`, `Open ${name}`));
}

const QUESTION_WORDS = ["알려줘", "알려주", "알려", "뭐야", "무엇", "어떻게", "어디", "언제", "누구", "왜", "인가요", "인지", "나요", "까요", "가요", "설명해", "explain", "what", "how", "when", "where", "why", "which", "who"];

function ruleAnswer(t: Text, ctx: PlanContext): Plan | null {
  const question = /[?？]$/.test(t.raw.trim()) || has(t, QUESTION_WORDS);
  if (!question) return null;
  const scope = emptyScope();
  if (has(t, SELECTION_WORDS)) scope.doc_ids = selectionIds(ctx);
  return single({ op: "answer", question: t.raw.trim(), cite: true }, 0.8, tr(t.ko, "지식 베이스·문서에서 근거를 찾아 답합니다", "Answer with evidence from the knowledge base / documents"), scope);
}

const COMMAND_MARKERS = ["해줘", "해주세요", "하세요", "해라", "하자", "시켜", "바꿔", "바꾸", "옮겨", "지워", "삭제", "정리", "추출", "분류", "내보내", "변환", "만들어", "실행", "열어", "보여", "찾아", "검색", "이동", "설정", "켜줘", "꺼줘", "처리", "돌려", "저장", "export", "delete", "move", "rename", "run ", "open", "show", "make", "create", "set ", "turn", "switch", "process", "extract", "classify", "organi", "import"];

/** A short topic with no command verb is a knowledge-base lookup (mirrors `rules::topic`). */
function ruleTopic(t: Text): Plan | null {
  const words = t.raw.split(/\s+/).filter(Boolean).length;
  if (words === 0 || words > 8 || Array.from(t.compact).length < 2 || has(t, COMMAND_MARKERS)) return null;
  return single({ op: "answer", question: t.raw.trim(), cite: true }, 0.7, tr(t.ko, "지식 베이스·문서에서 근거를 찾아 답합니다", "Answer with evidence from the knowledge base / documents"));
}

/** Tries the rules in order (`rules::plan`); `null` leaves the command to the model. */
export function rulesPlan(text: string, ctx: PlanContext): Plan | null {
  const raw = text.trim();
  if (!raw) return null;
  const t = mkText(raw, ctx.language === "ko" || /[가-힣]/.test(raw));
  return (
    ruleUndoRedo(t) ??
    ruleJobControl(t) ??
    ruleEngine(t) ??
    ruleSettingsCombo(t, ctx) ??
    ruleDefineSchema(t, ctx) ??
    ruleExport(t, ctx) ??
    ruleAggregate(t, ctx) ??
    ruleInbox(t, ctx) ??
    ruleTag(t, ctx) ??
    ruleOrganize(t, ctx) ??
    ruleDelete(t, ctx) ??
    ruleProcess(t, ctx) ??
    ruleReview(t) ??
    ruleSearch(t, ctx) ??
    ruleOpenDocument(t) ??
    ruleAnswer(t, ctx) ??
    ruleTopic(t)
  );
}

// ---------------------------------------------------------------------------
// Validator (planner/validate.rs): normalises a plan in place, reports issues
// ---------------------------------------------------------------------------

const DOCUMENT_STATUSES = new Set(["importing", "new", "rendered", "extracted", "reviewed", "archived", "missing", "error"]);
const SCREENS = new Set<string>(Object.keys(SCREEN_ROUTES));

function issue(path: string, code: string, message: string, blocking = true): PlanIssue {
  return { path, code, message, blocking };
}

/** A workspace-relative path: forward slashes, no drive / root / `..` / `.doculens`, ≤ 240 characters. */
export function workspaceRelative(path: string): string | null {
  const p = path.trim().replace(/\\/g, "/");
  if (!p || p.startsWith("/") || p.charAt(1) === ":") return null;
  const parts: string[] = [];
  for (const c0 of p.split("/")) {
    const c = c0.trim();
    if (!c || c === ".") continue;
    if (c === "..") return null;
    parts.push(c);
  }
  if (parts.length === 0 || (parts[0] ?? "").toLowerCase() === ".doculens") return null;
  const out = parts.join("/");
  return Array.from(out).length > MAX_PATH_CHARS ? null : out;
}

function isoDate(s: string): boolean {
  const m = s.trim().match(/^(\d{4})-(\d{2})-(\d{2})$/);
  if (!m) return false;
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  return d.getMonth() === Number(m[2]) - 1 && d.getDate() === Number(m[3]);
}

function knownTypes(ctx: PlanContext): string {
  return ctx.doc_types.map((d) => `${d.slug}(${d.label_ko})`).join(", ");
}

function knownSchemas(ctx: PlanContext): string {
  return ctx.schemas.map((s) => `${s.name}(${s.doc_type})`).join(", ");
}

function checkTemplate(tpl: string | null | undefined, path: string, issues: PlanIssue[]): void {
  if (tpl == null) return;
  if (!tpl.trim()) {
    issues.push(issue(path, "template_empty", "template is empty"));
    return;
  }
  const unknown = unknownVariables(tpl);
  if (unknown.length > 0) issues.push(issue(path, "template_variable", `unknown template variable(s) ${unknown.join(", ")}; allowed: ${TEMPLATE_VARIABLES.join(", ")}`));
}

function checkFolderTemplate(t: string, path: string, issues: PlanIssue[]): void {
  // Variables are substituted later; the literal skeleton must still be a workspace-relative path.
  const skeleton = parseTemplate(t)
    .map((tok) => (tok.kind === "lit" ? tok.text : "x"))
    .join("");
  if (workspaceRelative(skeleton) === null) issues.push(issue(path, "path_outside", `'${t}' must be a workspace-relative folder (documents/…)`));
}

function checkSetting(op: Extract<Op, { op: "set_setting" }>, path: string, issues: PlanIssue[]): void {
  const bad = (msg: string) => issues.push(issue(path, "setting_value", msg));
  const v = op.value;
  const num = typeof v === "number" ? v : typeof v === "string" && v.trim() !== "" ? Number(v.trim().replace(/%$/, "")) : Number.NaN;
  switch (op.key) {
    case "render_dpi":
      if ([150, 200, 300].includes(num)) op.value = num;
      else bad("render_dpi must be 150, 200 or 300");
      break;
    case "pii_masking":
    case "inbox_watch": {
      if (typeof v === "boolean") break;
      const s = String(v).trim().toLowerCase();
      if (["true", "on", "켜기", "yes"].includes(s)) op.value = true;
      else if (["false", "off", "끄기", "no"].includes(s)) op.value = false;
      else bad("value must be true or false");
      break;
    }
    case "auto_pipeline": {
      const s = String(v).trim();
      if (["none", "classify_only", "classify_and_ocr", "full"].includes(s)) op.value = s;
      else if (s === "classify") op.value = "classify_only";
      else if (s === "ocr") op.value = "classify_and_ocr";
      else bad("auto_pipeline must be none, classify_only, classify_and_ocr or full");
      break;
    }
    case "review_threshold":
      if (num >= 0 && num <= 1) op.value = num;
      else if (num > 1 && num <= 100) op.value = num / 100;
      else bad("review_threshold must be between 0 and 1");
      break;
    case "cache_limit_gb":
      if (num >= 0.5 && num <= 1000) op.value = num;
      else bad("cache_limit_gb must be between 0.5 and 1000");
      break;
    case "journal_days":
      if (num >= 0 && num <= 36_500) op.value = Math.trunc(num);
      else bad("journal_days must be between 0 and 36500");
      break;
  }
}

/** Validates and normalises a plan in place (`validate::validate`). Blocking issues mean the plan must not run. */
export function validatePlan(plan: Plan, ctx: PlanContext, date: DateRange | null): PlanIssue[] {
  const issues: PlanIssue[] = [];
  plan.confidence = Number.isFinite(plan.confidence) ? Math.min(1, Math.max(0, plan.confidence)) : 0;
  if (plan.summary != null) {
    plan.summary = Array.from(plan.summary.trim()).slice(0, 300).join("");
    if (!plan.summary) plan.summary = null;
  }
  plan.scope = plan.scope ?? emptyScope();
  plan.scope.filter = plan.scope.filter ?? {};
  plan.scope.doc_ids = (plan.scope.doc_ids ?? []).map((d) => d.trim()).filter(Boolean).filter((d, i, all) => all.indexOf(d) === i);
  const f = plan.scope.filter;

  // -- scope ----------------------------------------------------------------
  if (plan.scope.folder != null) {
    const p = workspaceRelative(plan.scope.folder);
    if (p === null) issues.push(issue("scope.folder", "path_invalid", `'${plan.scope.folder}' is not a workspace-relative folder`));
    plan.scope.folder = p;
  }
  if (f.doc_type != null) {
    const d = ctxDocType(ctx, f.doc_type);
    if (d) f.doc_type = d.slug;
    else if (ctx.doc_types.length > 0) issues.push(issue("scope.filter.doc_type", "doc_type_unknown", `unknown document type '${f.doc_type}'; known: ${knownTypes(ctx)}`));
  }
  for (const [path, value] of [
    ["scope.filter.date_from", f.date_from],
    ["scope.filter.date_to", f.date_to],
  ] as const) {
    if (value != null && !isoDate(value)) issues.push(issue(path, "date_format", `'${value}' is not an ISO-8601 date (YYYY-MM-DD)`));
  }
  if (f.date_from != null && f.date_to != null && isoDate(f.date_from) && isoDate(f.date_to) && f.date_to < f.date_from) issues.push(issue("scope.filter.date_to", "date_order", "date_to must not precede date_from"));
  // Relative-date phrase resolved by the pre-router: deterministic override of a missing range.
  if (date && f.date_from == null && f.date_to == null) {
    f.date_from = date.from;
    f.date_to = date.to;
  }
  if (f.status != null && !DOCUMENT_STATUSES.has(f.status)) issues.push(issue("scope.filter.status", "status_unknown", `unknown status '${f.status}'`));
  if (f.min_confidence != null && !(f.min_confidence >= 0 && f.min_confidence <= 1)) issues.push(issue("scope.filter.min_confidence", "range", "min_confidence must be between 0 and 1"));
  if (f.tags) f.tags = f.tags.filter((t) => t.trim() !== "");

  // -- operations -----------------------------------------------------------
  if (plan.operations.length === 0) {
    if (plan.clarification_question) plan.operations.push({ op: "clarify", question: plan.clarification_question });
    else issues.push(issue("operations", "ops_empty", "the plan has no operations"));
  }
  if (plan.operations.length > MAX_OPS) {
    issues.push(issue("operations", "ops_many", `more than ${MAX_OPS} operations`));
    plan.operations = plan.operations.slice(0, MAX_OPS);
  }
  if (plan.operations.some((o) => o.op === "clarify") && plan.operations.length > 1) issues.push(issue("operations", "clarify_mixed", "clarify must be the only operation"));
  const scopeDocType = f.doc_type ?? null;
  plan.operations.forEach((op, i) => {
    const at = (field: string) => `operations[${i}].${field}`;
    switch (op.op) {
      case "import":
        if (op.source === "files") issues.push(issue(at("source"), "import_files_gui", "file selection needs the import dialog; use source=inbox or folder"));
        if (op.source === "folder" && !(op.path ?? "").trim()) issues.push(issue(at("path"), "path_required", "source=folder needs a path"));
        if (op.path != null) op.path = Array.from(op.path.trim()).slice(0, 1000).join("");
        break;
      case "classify": {
        const resolved: string[] = [];
        for (const t of op.doc_types ?? []) {
          const d = ctxDocType(ctx, t);
          if (d) resolved.push(d.slug);
          else if (ctx.doc_types.length === 0) resolved.push(t);
          else issues.push(issue(at("doc_types"), "doc_type_unknown", `unknown type '${t}' ignored`, false));
        }
        op.doc_types = resolved;
        break;
      }
      case "set_doc_type": {
        const d = ctxDocType(ctx, op.doc_type);
        if (d) op.doc_type = d.slug;
        else if (ctx.doc_types.length > 0) issues.push(issue(at("doc_type"), "doc_type_unknown", `unknown document type '${op.doc_type}'; known: ${knownTypes(ctx)}`));
        break;
      }
      case "ocr":
        op.pages = [...new Set((op.pages ?? []).filter((p) => p >= 1 && p <= 5000))].sort((a, b) => a - b);
        break;
      case "define_schema":
        if (op.base != null && ctx.schemas.length > 0 && !ctxSchema(ctx, op.base) && !ctxDocType(ctx, op.base)) issues.push(issue(at("base"), "schema_unknown", `unknown schema '${op.base}'; known: ${knownSchemas(ctx)}`));
        if (op.mode == null) op.mode = op.base != null ? "modify" : "create";
        if (!(op.description ?? "").trim() && !(op.name ?? "").trim()) issues.push(issue(at("description"), "description_required", "define_schema needs a description or a name"));
        break;
      case "extract": {
        let schema: SchemaSummary | null = null;
        if (op.schema_ref != null) {
          const r = op.schema_ref;
          if (r.toLowerCase() === "auto" || r.trim() === "") op.schema_ref = null;
          else {
            const s = ctxSchema(ctx, r);
            if (s) {
              op.schema_ref = s.id;
              schema = s;
            } else if (ctx.schemas.length > 0) issues.push(issue(at("schema_ref"), "schema_unknown", `unknown schema '${r}'; known: ${knownSchemas(ctx)}`));
          }
        }
        if (!schema && scopeDocType) schema = schemaForType(ctx, scopeDocType);
        if (schema) {
          const keep: string[] = [];
          for (const fld of op.fields ?? []) {
            const fd = ctxField(schema, fld);
            if (fd) keep.push(fd.key);
            else issues.push(issue(at("fields"), "field_unknown", `'${fld}' is not a field of ${schema.name}; ignored`, false));
          }
          op.fields = keep;
        }
        break;
      }
      case "review":
        if (op.action === "correct" && (!(op.field ?? "").trim() || op.value == null)) issues.push(issue(at("field"), "correct_needs_field", "review correct needs field and value"));
        break;
      case "search":
        op.query = Array.from(op.query.trim()).slice(0, 500).join("");
        if (!op.query) issues.push(issue(at("query"), "query_empty", "search needs a query"));
        if (op.top_k != null) op.top_k = Math.min(200, Math.max(1, op.top_k));
        break;
      case "answer":
        op.question = Array.from(op.question.trim()).slice(0, 4000).join("");
        if (!op.question) issues.push(issue(at("question"), "question_empty", "answer needs a question"));
        break;
      case "aggregate": {
        const text = (op.field ?? "").trim();
        if (!text) {
          if (op.fn !== "count") issues.push(issue(at("field"), "field_default", "no field given; the type's total field is used", false));
        } else if (scopeDocType) {
          const s = schemaForType(ctx, scopeDocType);
          if (s) {
            const fd = ctxField(s, text);
            if (fd) op.field = fd.key;
            else issues.push(issue(at("field"), "field_unknown", `'${text}' is not a field of ${s.name}; fields: ${s.fields.map((x) => `${x.key}(${x.label_ko})`).join(", ")}`));
          }
        }
        break;
      }
      case "organize":
        checkTemplate(op.template, at("template"), issues);
        if (op.folder_template != null) {
          checkTemplate(op.folder_template, at("folder_template"), issues);
          checkFolderTemplate(op.folder_template, at("folder_template"), issues);
        }
        break;
      case "rename":
        checkTemplate(op.template, at("template"), issues);
        break;
      case "move":
        checkTemplate(op.folder, at("folder"), issues);
        checkFolderTemplate(op.folder, at("folder"), issues);
        break;
      case "tag": {
        const clean = (v: string[] | undefined) => [...new Set((v ?? []).map((t) => Array.from(t.trim()).slice(0, 64).join("")).filter(Boolean))];
        op.add = clean(op.add);
        op.remove = clean(op.remove);
        if (op.add.length === 0 && op.remove.length === 0) issues.push(issue(at("add"), "tags_empty", "tag needs tags to add or remove"));
        break;
      }
      case "export": {
        if (op.path != null) {
          const rel = workspaceRelative(op.path);
          if (rel === null) issues.push(issue(at("path"), "path_outside", `'${op.path}' must be a workspace-relative path (exports/…)`));
          else {
            const ext = `.${extensionOf(op.format)}`;
            op.path = rel.toLowerCase().endsWith(ext) ? rel : `${rel}${ext}`;
          }
        }
        if (scopeDocType) {
          const s = schemaForType(ctx, scopeDocType);
          if (s) {
            const keep: string[] = [];
            for (const c of op.columns ?? []) {
              const fd = ctxField(s, c);
              if (fd) keep.push(fd.key);
              else issues.push(issue(at("columns"), "field_unknown", `'${c}' is not a field of ${s.name}; ignored`, false));
            }
            op.columns = keep;
          }
        }
        break;
      }
      case "set_model":
        if (ctx.model_ids.length > 0) {
          const key = compact(op.id);
          const m = ctx.model_ids.find((x) => compact(x) === key || compact(x).includes(key));
          if (m) op.id = m;
          else issues.push(issue(at("id"), "model_unknown", `unknown model '${op.id}'; available: ${ctx.model_ids.join(", ")}`));
        }
        break;
      case "set_setting":
        checkSetting(op, at("value"), issues);
        break;
      case "open":
        op.ref = op.ref.trim();
        if (op.target === "screen") {
          if (!SCREENS.has(op.ref)) issues.push(issue(at("ref"), "screen_unknown", `unknown screen '${op.ref}'`));
        } else if (op.target === "folder") {
          const p = workspaceRelative(op.ref);
          if (p === null) issues.push(issue(at("ref"), "path_invalid", `'${op.ref}' is not a workspace folder`));
          else op.ref = p;
        } else if (!op.ref) issues.push(issue(at("ref"), "ref_empty", "open needs a reference"));
        break;
      case "job":
        if (op.id != null) {
          op.id = op.id.trim();
          if (!op.id) op.id = null;
        }
        break;
      case "undo":
        if (op.target != null) {
          op.target = op.target.trim();
          if (!op.target || op.target.toLowerCase() === "last") op.target = null;
        }
        break;
      case "clarify":
        op.question = Array.from(op.question.trim()).slice(0, 300).join("");
        if (!op.question) issues.push(issue(at("question"), "question_empty", "clarify needs a question"));
        break;
      default:
        break;
    }
  });

  // -- derived flags ---------------------------------------------------------
  const clarifyOp = plan.operations.find((o) => o.op === "clarify");
  if (clarifyOp && clarifyOp.op === "clarify") {
    plan.intent = "clarify";
    if (!plan.clarification_question) plan.clarification_question = clarifyOp.question;
  } else if (!plan.operations.some((o) => o.op === plan.intent)) {
    const first = plan.operations.find((o) => o.op !== "noop");
    if (first) plan.intent = first.op;
  }
  plan.requires_confirmation = plan.requires_confirmation || plan.operations.some(needsConfirmation);
  return issues;
}

export function hasErrors(issues: PlanIssue[]): boolean {
  return issues.some((i) => i.blocking);
}

export function extensionOf(format: ExportFormat): string {
  return format === "pdf_searchable" ? "pdf" : format;
}

// ---------------------------------------------------------------------------
// Context + planning entry point (planner_bridge::build_context, llm::plan)
// ---------------------------------------------------------------------------

/** Workspace facts the planner grounds its plans in (types, schemas, folders, tags, models + what the UI shows). */
export function buildContext(ui: UiPlanContext = {}): PlanContext {
  const settings = loadSettings();
  const docs = wsDocs();
  const folders = [...new Set(docs.map((d) => relDir(d.rel_path)).filter(Boolean))].sort();
  const tags = [...new Set(docs.flatMap((d) => d.tags))].sort((a, b) => a.localeCompare(b));
  const schemas: SchemaSummary[] = pl
    .listSchemas()
    .filter((s) => s.enabled)
    .map((s) => ({
      id: s.id,
      name: s.name,
      name_en: s.name_en ?? "",
      doc_type: s.doc_type,
      fields: s.definition.fields.map((f) => ({ key: f.key, label_ko: f.label_ko, label_en: f.label_en, kind: f.kind })),
    }));
  return {
    today: todayIso(),
    language: settings.ui.language,
    folders,
    doc_types: pl.docTypeList().map((d) => ({ slug: d.slug, label_ko: d.label_ko, label_en: d.label_en })),
    schemas,
    tags,
    current_folder: ui.folder ?? null,
    selected_doc_ids: [...(ui.selected_doc_ids ?? [])],
    current_document: ui.current_document ?? null,
    current_page: ui.current_page ?? null,
    document_count: docs.length,
    model_ids: listModelIds(),
  };
}

function clarificationText(ko: boolean, reason: string): string {
  return ko
    ? `명령을 해석하지 못했습니다(${reason}). 무엇을 할지 조금 더 구체적으로 말씀해 주세요 — 예: '지난달 영수증 합계', '세금계산서를 엑셀로 내보내'.`
    : `I could not interpret the command (${reason}). Please say more precisely what to do — e.g. 'total of last month's receipts', 'export tax invoices to Excel'.`;
}

/** Pre-routed settings / navigation / help commands as single-op plans (`llm::prerouted`). */
function prerouted(text: string, ctx: PlanContext): { plan: Plan; confidence: number } | null {
  const r = mockRoute(text);
  const ko = ctx.language === "ko";
  let op: Op;
  let summary: string;
  if (r.op === "set_theme") {
    op = { op: "set_theme", theme: r.theme };
    summary = ko ? `테마를 ${r.theme}(으)로 바꿉니다` : `Switch the theme to ${r.theme}`;
  } else if (r.op === "set_language") {
    op = { op: "set_language", lang: r.language };
    summary = ko ? `UI 언어를 ${r.language}(으)로 바꿉니다` : `Switch the UI language to ${r.language}`;
  } else if (r.op === "open") {
    op = { op: "open", target: "screen", ref: screenRef(r.screen) };
    summary = ko ? "화면을 엽니다" : "Open the screen";
  } else if (r.op === "help") {
    op = { op: "help" };
    summary = ko ? "도움말" : "Help";
  } else return null;
  return { plan: single(op, r.confidence, summary), confidence: r.confidence };
}

/** Low confidence → clarification first (`llm::finalize`). */
function finalize(plan: Plan, ko: boolean): Plan {
  if (isClarification(plan) || plan.confidence >= MIN_CONFIDENCE) return plan;
  const q = plan.clarification_question ?? (ko ? `이렇게 이해했습니다: ${plan.summary ?? ""}. 맞나요? 아니면 더 구체적으로 말씀해 주세요.` : `I understood: ${plan.summary ?? ""}. Is that right? Otherwise please be more specific.`);
  const c = clarify(q, plan.confidence);
  c.summary = plan.summary ?? null;
  return c;
}

/** The simulated planner model: a search over the sentence (so the UI can show the model path). */
function modelPlan(text: string, ctx: PlanContext, date: DateRange | null): Plan {
  const ko = ctx.language === "ko" || /[가-힣]/.test(text);
  const query = text.replace(/[?？.!]+$/, "").trim();
  const plan = single({ op: "search", query, mode: null, top_k: null }, 0.72, ko ? `'${query}'을(를) 검색합니다 (모델 해석)` : `Search for '${query}' (model interpretation)`);
  applyDate(plan.scope, date);
  return finalize(plan, ko);
}

export function plannerModelEnabled(): boolean {
  return flag(PLANNER_MODEL_KEY, "on");
}

/**
 * Plans one command: pre-router → phrase rules → (simulated) model (`llm::plan`). Never throws
 * on model trouble: without a model the result carries a clarification plan and a `no_model` issue.
 */
export function planText(text: string, ctx: PlanContext, opts: { model?: boolean } = {}): PlanResult {
  const t = text.trim();
  const ko = ctx.language === "ko";
  const date = resolveRelativeDate(t, new Date(`${ctx.today}T12:00:00`));
  const result: PlanResult = {
    text: t,
    source: "none",
    plan: clarify(clarificationText(ko, ko ? "빈 명령" : "empty"), 0),
    issues: [],
    date,
    attempts: 0,
    model: null,
    prompt_tokens: 0,
    completion_tokens: 0,
    raw: null,
  };
  if (!t) return result;
  const accept = (p: Plan, source: PlanResult["source"]): boolean => {
    const issues = validatePlan(p, ctx, date);
    if (hasErrors(issues)) return false;
    result.source = source;
    result.plan = p;
    result.issues = issues;
    return true;
  };
  // 1. compound sentences go to the rules first so the second clause is kept
  if (isCompound(t)) {
    const p = rulesPlan(t, ctx);
    if (p && accept(p, "rules")) return result;
  }
  const pre = prerouted(t, ctx);
  if (pre && accept(pre.plan, "prerouter")) return result;
  // 2. phrase rules
  const p = rulesPlan(t, ctx);
  if (p) {
    if (accept(p, "rules")) return result;
    // A rule that produced an invalid plan still beats a model guess: ask the user with the first error.
    const issues = validatePlan(structuredClone(p), ctx, date);
    const first = issues.find((i) => i.blocking);
    if (first) {
      result.source = "rules";
      result.plan = clarify(ko ? `${first.message} — 어떻게 할까요?` : `${first.message} — what should I do?`, 0.5);
      result.issues = issues;
      return result;
    }
  }
  // 3. the model
  if (!(opts.model ?? plannerModelEnabled())) {
    result.issues.push(issue("", "no_model", "the planner model is not available"));
    result.plan = clarify(ko ? "규칙으로 해석할 수 없는 명령이고 모델이 로드되지 않아 플랜을 만들 수 없습니다. 모델을 로드하거나 명령을 바꿔 주세요." : "The command needs the planner model, which is not loaded. Load a model or rephrase.", 0);
    return result;
  }
  result.model = PLANNER_MODEL;
  result.attempts = 1;
  result.prompt_tokens = 1800 + Array.from(t).length;
  result.completion_tokens = 60;
  const guess = modelPlan(t, ctx, date);
  result.raw = JSON.stringify(guess);
  const issues = validatePlan(guess, ctx, date);
  result.source = "model";
  result.issues = issues;
  result.plan = hasErrors(issues) ? clarify(clarificationText(ko, issues.find((i) => i.blocking)?.message ?? ""), 0) : finalize(guess, ko);
  return result;
}

// ---------------------------------------------------------------------------
// Scope resolution (planner/scope.rs)
// ---------------------------------------------------------------------------

/** Date a document is filed under: its document date, else the import date. Returns `[YYYY-MM-DD, basis]`. */
export function basisDate(doc: MockDoc): [string | null, "doc_date" | "imported_at"] {
  const d = (doc.doc_date ?? "").trim();
  if (d.length >= 10) return [d.slice(0, 10), "doc_date"];
  return [doc.imported_at.slice(0, 10), "imported_at"];
}

/** Documents in scope, sorted by path. Deleted / importing / missing / error documents are never in scope. */
export function resolveScope(scope: Scope | undefined): MockDoc[] {
  const s = scope ?? emptyScope();
  const f = s.filter ?? {};
  let docs = wsDocs().filter((d) => !["importing", "missing", "error"].includes(d.status));
  if (s.folder != null) {
    const folder = s.folder;
    docs = docs.filter((d) => {
      const dir = relDir(d.rel_path);
      return folder === "" ? dir === "" : dir === folder || dir.startsWith(`${folder}/`);
    });
  }
  if (f.doc_type) docs = docs.filter((d) => d.doc_type === f.doc_type);
  const tags = f.tags ?? [];
  if (tags.length > 0) docs = docs.filter((d) => tags.every((t) => d.tags.some((x) => x.toLowerCase() === t.toLowerCase())));
  if (f.review_status) docs = docs.filter((d) => d.review_status === f.review_status);
  if (f.keyword && f.keyword.trim()) {
    const q = f.keyword.trim().toLowerCase();
    docs = docs.filter((d) => d.original_name.toLowerCase().includes(q) || (d.title ?? "").toLowerCase().includes(q) || d.rel_path.toLowerCase().includes(q));
  }
  if (f.status) docs = docs.filter((d) => d.status === f.status);
  const ids = s.doc_ids ?? [];
  if (ids.length > 0) docs = docs.filter((d) => ids.includes(d.id));
  if (f.date_from != null || f.date_to != null) {
    const from = f.date_from ?? "1900-01-01";
    const to = f.date_to ?? "9999-12-31";
    docs = docs.filter((d) => {
      const [date] = basisDate(d);
      return date !== null && date >= from && date <= to;
    });
  }
  const vendor = f.vendor ? compact(f.vendor) : "";
  if (vendor) docs = docs.filter((d) => compact(d.vendor ?? "").includes(vendor) || compact(d.title ?? "").includes(vendor) || compact(d.original_name).includes(vendor));
  if (f.min_confidence != null) {
    const min = f.min_confidence;
    docs = docs.filter((d) => d.confidence !== null && d.confidence >= min);
  }
  return docs.sort((a, b) => (a.rel_path < b.rel_path ? -1 : a.rel_path > b.rel_path ? 1 : 0));
}

// ---------------------------------------------------------------------------
// Journal (organize/journal.rs): batches + entries in localStorage
// ---------------------------------------------------------------------------

interface StoredBatch extends JournalBatch {
  workspace: string;
  plan_json?: string | null;
}

interface JournalStore {
  batches: StoredBatch[];
  entries: JournalEntry[];
}

function loadJournal(): JournalStore {
  const s = read<Partial<JournalStore>>(JOURNAL_KEY, {});
  return { batches: Array.isArray(s.batches) ? s.batches : [], entries: Array.isArray(s.entries) ? s.entries : [] };
}

function saveJournal(s: JournalStore): void {
  write(JOURNAL_KEY, s);
}

function publicBatch(b: StoredBatch): JournalBatch {
  return { id: b.id, kind: b.kind, label: b.label, source: b.source, created_at: b.created_at, entries: b.entries, undone_at: b.undone_at, undone_by: b.undone_by, redo_of: b.redo_of };
}

function workspaceBatches(s: JournalStore): StoredBatch[] {
  const ws = currentWorkspacePath() ?? "";
  // Newest first (created_at desc, then insertion order desc).
  return s.batches
    .map((b, i) => ({ b, i }))
    .filter(({ b }) => b.workspace === ws)
    .sort((x, y) => y.b.created_at.localeCompare(x.b.created_at) || y.i - x.i)
    .map(({ b }) => b);
}

function beginBatch(s: JournalStore, kind: string, label: string | null, source: string | null, planJson: string | null): StoredBatch {
  const b: StoredBatch = { id: newId(), kind, label, source, created_at: now(), entries: 0, undone_at: null, undone_by: null, redo_of: null, workspace: currentWorkspacePath() ?? "", plan_json: planJson };
  s.batches.push(b);
  return b;
}

interface NewEntry {
  kind: JournalEntryKind;
  document_id: string | null;
  from_path: string;
  to_path: string | null;
  trash_path: string | null;
  content_hash: string | null;
  size_bytes: number | null;
}

function appendEntry(s: JournalStore, batch: StoredBatch, seq: number, e: NewEntry): JournalEntry {
  const entry: JournalEntry = { id: s.entries.reduce((m, x) => Math.max(m, x.id), 0) + 1, batch_id: batch.id, seq, ...e, created_at: now(), undone_at: null, undo_error: null };
  s.entries.push(entry);
  batch.entries += 1;
  return entry;
}

function dropEmptyBatch(s: JournalStore, id: string): void {
  s.batches = s.batches.filter((b) => !(b.id === id && b.entries === 0));
}

export function listBatches(limit: number): JournalBatch[] {
  return workspaceBatches(loadJournal()).slice(0, Math.max(1, limit)).map(publicBatch);
}

export function batchEntries(batchId: string): JournalEntry[] {
  return loadJournal()
    .entries.filter((e) => e.batch_id === batchId)
    .sort((a, b) => a.seq - b.seq);
}

function getBatch(s: JournalStore, id: string): StoredBatch | null {
  return s.batches.find((b) => b.id === id) ?? null;
}

/** A batch that can still be reverted (not an undo itself, not yet undone, has entries). */
function undoable(b: JournalBatch): boolean {
  return b.kind !== "undo" && b.undone_at === null && b.entries > 0;
}

export function lastUndoable(): JournalBatch | null {
  return workspaceBatches(loadJournal()).find(undoable) ?? null;
}

export function lastRedoable(): JournalBatch | null {
  return workspaceBatches(loadJournal()).find((b) => b.kind === "undo" && b.undone_at === null && b.entries > 0) ?? null;
}

function docsChanged(kind: DocumentsChanged["kind"], ids: string[]): void {
  if (ids.length > 0) emit(EVENTS.documentsChanged, { kind, ids } satisfies DocumentsChanged);
}

function pipelineChanged(kind: PipelineChanged["kind"], ids: string[] = []): void {
  emit(EVENTS.pipelineChanged, { kind, ids } satisfies PipelineChanged);
}

/**
 * Reverts one batch LIFO (`journal::undo_batch`). Entries whose file is no longer where the
 * batch left it (`missing`), whose content changed (`changed`), whose original location is
 * occupied again (`target_occupied`) or that went to the OS recycle bin (`os_trash`) are
 * skipped with that reason. The reversal is journaled as a new batch of kind `undo` (or
 * `redo` when the batch is itself an undo), so it can be undone again.
 */
export function undoBatch(batchId: string): UndoReport {
  const s = loadJournal();
  const batch = getBatch(s, batchId);
  if (!batch) throw err("not_found", `journal batch ${batchId}`);
  if (batch.undone_at !== null) throw err("batch_already_undone", "batch_already_undone");
  const newKind = batch.kind === "undo" ? "redo" : "undo";
  const prefix = newKind === "undo" ? "되돌리기" : "다시 실행";
  const nb = beginBatch(s, newKind, batch.label ? `${prefix}: ${batch.label}` : prefix, batch.source, null);
  const report: UndoReport = { source_batch: batch.id, source_kind: batch.kind, batch_id: nb.id, restored: 0, skipped: [], restored_paths: [], documents: [] };
  const all = loadDocs();
  const live = (rel: string) => all.find((d) => !d.deleted_at && d.rel_path === rel);
  const entries = s.entries.filter((e) => e.batch_id === batch.id).sort((a, b) => b.seq - a.seq);
  let seq = 0;
  let touched = false;
  for (const e of entries) {
    if (e.undone_at !== null) continue;
    let outcome: NewEntry | string;
    if (e.kind === "move" || e.kind === "rename") {
      const toRel = e.to_path;
      if (toRel === null) continue;
      const doc = live(toRel);
      if (!doc) outcome = "missing";
      else if (e.content_hash !== null && doc.content_hash !== e.content_hash) outcome = "changed";
      else if (live(e.from_path)) outcome = "target_occupied";
      else {
        doc.rel_path = e.from_path;
        doc.original_name = basename(e.from_path);
        doc.updated_at = now();
        touched = true;
        if (e.document_id !== null && doc.id === e.document_id) report.documents.push(doc.id);
        outcome = { kind: e.kind, document_id: e.document_id, from_path: toRel, to_path: e.from_path, trash_path: null, content_hash: e.content_hash, size_bytes: e.size_bytes };
      }
    } else if (e.trash_path === null) outcome = "os_trash";
    else {
      const doc = e.document_id ? all.find((d) => d.id === e.document_id && d.deleted_at) : undefined;
      if (!doc) outcome = "missing";
      else if (live(e.from_path)) outcome = "target_occupied";
      else {
        doc.deleted_at = null;
        doc.rel_path = e.from_path;
        doc.original_name = basename(e.from_path);
        doc.updated_at = now();
        touched = true;
        report.restored_paths.push(e.from_path);
        outcome = { kind: "move", document_id: null, from_path: e.trash_path, to_path: e.from_path, trash_path: null, content_hash: e.content_hash, size_bytes: e.size_bytes };
      }
    }
    if (typeof outcome === "string") {
      e.undo_error = outcome;
      report.skipped.push([e.to_path ?? e.from_path, outcome]);
    } else {
      appendEntry(s, nb, seq, outcome);
      seq += 1;
      e.undone_at = now();
      e.undo_error = null;
      report.restored += 1;
    }
  }
  batch.undone_at = now();
  batch.undone_by = nb.id;
  nb.redo_of = batch.id;
  saveJournal(s);
  if (touched) saveDocs(all);
  return report;
}

// ---------------------------------------------------------------------------
// Organise (organize/mod.rs): dry-run diff, apply with journal, delete
// ---------------------------------------------------------------------------

function splitName(name: string): [string, string] {
  const i = name.lastIndexOf(".");
  if (i > 0) {
    const stem = name.slice(0, i);
    const ext = name.slice(i + 1);
    if (stem && ext && ext.length <= 8) return [stem, ext];
  }
  return [name, ""];
}

function joinPath(folder: string, name: string, ext: string): string {
  const file = ext ? `${name}.${ext}` : name;
  return folder ? `${folder}/${file}` : file;
}

function variableOf(doc: MockDoc, labels: DocTypeInfo[], lang: Language, v: string, stem: string, ext: string, seq: number | null): string | null {
  const [date] = basisDate(doc);
  const parts = date ? dateParts(date) : null;
  switch (v) {
    case "year":
      return parts?.year ?? null;
    case "month":
      return parts?.month ?? null;
    case "day":
      return parts?.day || null;
    case "doc_type": {
      const slug = doc.doc_type;
      if (!slug) return null;
      const t = labels.find((l) => l.slug === slug);
      return t ? (lang === "ko" ? t.label_ko : t.label_en) : slug;
    }
    case "vendor":
      return doc.vendor && doc.vendor.trim() ? doc.vendor : null;
    case "total":
      return doc.total_amount != null ? formatTotal(doc.total_amount) : null;
    case "title":
      return doc.title && doc.title.trim() ? doc.title : stem;
    case "original_name":
    case "name":
      return stem;
    case "ext":
      return ext;
    case "tags":
      return doc.tags.length > 0 ? doc.tags.join("-") : null;
    case "seq":
      return seq != null ? String(seq).padStart(2, "0") : null;
    default:
      return null;
  }
}

/** Renders the templates for the documents in scope (the diff table the user confirms). */
export function organizePlan(req: OrganizeRequest): OrganizePlan {
  for (const t of [req.name_template, req.folder_template]) {
    if (t == null) continue;
    const unknown = unknownVariables(t);
    if (unknown.length > 0) throw err("template_variable", `template_variable: ${unknown.join(", ")}`);
  }
  const onlyReviewed = req.only_reviewed ?? true;
  const lang: Language = req.language ?? "ko";
  const labels = pl.docTypeList();
  const docs = resolveScope(req.scope);
  const ws = currentWorkspacePath();
  const others = loadDocs().filter((d) => d.workspace === ws && !d.deleted_at);
  const out: OrganizePlan = { items: [], moves: 0, unchanged: 0, held: 0, missing_files: 0, needs_review: 0, name_template: req.name_template ?? null, folder_template: req.folder_template ?? null, only_reviewed: onlyReviewed };
  const taken = new Set<string>();
  const seqByFolder = new Map<string, number>();
  for (const doc of docs) {
    const display = doc.title ?? doc.original_name;
    const from = doc.rel_path;
    if (onlyReviewed && doc.review_status === "pending") {
      out.held += 1;
      out.items.push({ doc_id: doc.id, name: display, from, to: null, status: "held", reason: "review_pending", missing: [] });
      continue;
    }
    const [nameStem, nameExt] = splitName(basename(doc.original_name));
    const [diskStem, diskExt] = splitName(basename(from));
    const ext = diskExt || nameExt;
    const stem = diskStem || nameStem;
    const missing: string[] = [];
    // Folder first (it must not use {{seq}}), then the running number inside that folder.
    let folder: string;
    if (req.folder_template != null) {
      const r = renderTemplate(req.folder_template, (v) => variableOf(doc, labels, lang, v, stem, ext, null));
      missing.push(...r.missing);
      folder = sanitizeFolder(r.text);
    } else folder = relDir(from);
    const seq = (seqByFolder.get(folder) ?? 0) + 1;
    seqByFolder.set(folder, seq);
    let name: string;
    if (req.name_template != null) {
      const r = renderTemplate(req.name_template, (v) => variableOf(doc, labels, lang, v, stem, ext, seq));
      missing.push(...r.missing);
      name = sanitizeComponent(r.text);
    } else name = stem;
    const miss = [...new Set(missing)].sort();
    let candidate = joinPath(folder, name, ext);
    // Keep the whole relative path within the Windows-safe budget.
    if (Array.from(candidate).length > MAX_PATH_CHARS) {
      const overflow = Array.from(candidate).length - MAX_PATH_CHARS;
      const chars = Array.from(name);
      candidate = joinPath(folder, chars.slice(0, Math.max(1, chars.length - overflow)).join(""), ext);
    }
    if (candidate === from) {
      out.unchanged += 1;
      taken.add(candidate);
      out.items.push({ doc_id: doc.id, name: display, from, to: null, status: "unchanged", reason: null, missing: [] });
      continue;
    }
    // Collisions inside the batch or with another document's file → `_2`, `_3` …
    const occupied = (p: string) => taken.has(p) || others.some((d) => d.id !== doc.id && d.rel_path === p);
    const base = name;
    let n = 2;
    while (occupied(candidate) && n <= 10_000) {
      candidate = joinPath(folder, `${base}_${n}`, ext);
      n += 1;
    }
    taken.add(candidate);
    if (miss.length > 0) out.needs_review += 1;
    out.moves += 1;
    out.items.push({ doc_id: doc.id, name: display, from, to: candidate, status: "move", reason: null, missing: miss });
  }
  return out;
}

/** Replays a plan (`organize::apply`): every `move` item updates the document's path and is journaled. */
export async function applyOrganize(plan: OrganizePlan, kind: string, label: string, source: string, ctl: JobControl | null, progress: (done: number, total: number, name: string) => void): Promise<ApplyReport> {
  const items: OrganizeItem[] = plan.items.map((i) => ({ ...i, missing: [...(i.missing ?? [])] }));
  const total = items.filter((i) => i.status === "move").length;
  const report: ApplyReport = { batch_id: null, moved: 0, skipped: 0, failed: [], cancelled: false, items, documents: [] };
  if (total === 0) return report;
  const s = loadJournal();
  const batch = beginBatch(s, kind, label, source, JSON.stringify({ name_template: plan.name_template, folder_template: plan.folder_template, only_reviewed: plan.only_reviewed, moves: plan.moves }));
  report.batch_id = batch.id;
  saveJournal(s);
  let done = 0;
  let seq = 0;
  for (const item of items) {
    if (item.status !== "move" || item.to === null) continue;
    if (ctl?.cancelled()) {
      report.cancelled = true;
      break;
    }
    progress(done, total, item.name);
    try {
      await pace(ctl, 80);
    } catch (e) {
      if (!isCancellation(e)) throw e;
      report.cancelled = true;
      break;
    }
    const doc = loadDocs().find((d) => d.id === item.doc_id);
    if (!doc) {
      item.status = "failed";
      item.reason = "document_missing";
      report.failed.push([item.doc_id, "document_missing"]);
      continue;
    }
    // The document's current path must still be what the plan saw (a rescan may have moved it).
    if (doc.rel_path !== item.from || doc.deleted_at) {
      item.status = "failed";
      item.reason = "source_changed";
      report.failed.push([item.doc_id, "source_changed"]);
      continue;
    }
    const to = item.to;
    if (wsDocs().some((d) => d.id !== doc.id && d.rel_path === to)) {
      item.status = "failed";
      item.reason = `target exists: ${to}`;
      report.failed.push([item.doc_id, item.reason]);
      continue;
    }
    const fromDir = relDir(item.from);
    doc.rel_path = to;
    doc.original_name = basename(to);
    doc.updated_at = now();
    upsertDoc(doc);
    appendEntry(s, batch, seq, { kind: fromDir === relDir(to) ? "rename" : "move", document_id: doc.id, from_path: item.from, to_path: to, trash_path: null, content_hash: doc.content_hash, size_bytes: doc.size_bytes });
    saveJournal(s);
    seq += 1;
    item.status = "done";
    report.moved += 1;
    report.documents.push(doc.id);
    done += 1;
  }
  report.skipped = items.length - total;
  if (report.moved === 0) {
    // Nothing happened: drop the empty batch so undo does not offer it.
    dropEmptyBatch(s, batch.id);
    saveJournal(s);
    if (!getBatch(s, batch.id)) report.batch_id = null;
  }
  return report;
}

/** Deletes documents to the recycle bin (`organize::delete_documents`): soft-deleted here, journaled so undo can restore them. */
export async function deleteDocuments(ids: string[], label: string, source: string, ctl: JobControl | null): Promise<ApplyReport> {
  const report: ApplyReport = { batch_id: null, moved: 0, skipped: 0, failed: [], cancelled: false, items: [], documents: [] };
  if (ids.length === 0) return report;
  const osTrash = flag(OS_TRASH_KEY, "1");
  const s = loadJournal();
  const batch = beginBatch(s, "delete", label, source, null);
  report.batch_id = batch.id;
  let seq = 0;
  for (const id of ids) {
    if (ctl?.cancelled()) {
      report.cancelled = true;
      break;
    }
    const doc = wsDocs().find((d) => d.id === id);
    if (!doc) {
      report.failed.push([id, "document_missing"]);
      continue;
    }
    await pace(ctl, 40);
    doc.deleted_at = now();
    doc.updated_at = doc.deleted_at;
    upsertDoc(doc);
    appendEntry(s, batch, seq, { kind: "delete", document_id: doc.id, from_path: doc.rel_path, to_path: null, trash_path: osTrash ? null : `.doculens/trash/${basename(doc.rel_path)}`, content_hash: doc.content_hash, size_bytes: doc.size_bytes });
    seq += 1;
    report.moved += 1;
    report.documents.push(doc.id);
    report.items.push({ doc_id: doc.id, name: doc.original_name, from: doc.rel_path, to: null, status: "done", reason: null, missing: [] });
  }
  if (report.moved === 0) {
    dropEmptyBatch(s, batch.id);
    report.batch_id = null;
  }
  saveJournal(s);
  return report;
}

// ---------------------------------------------------------------------------
// Aggregate (aggregate.rs)
// ---------------------------------------------------------------------------

function isScalar(kind: string): boolean {
  return kind !== "table" && kind !== "group";
}

function asksForTotal(key: string): boolean {
  return ["합계", "총액", "총합", "총금액", "total", "sum", "amount", "금액"].some((w) => key.includes(w));
}

function defaultTotal(fields: FieldDef[]): FieldDef | null {
  const amounts = fields.filter((f) => f.kind === "amount");
  const named = amounts.find((f) => ["grand_total", "total", "total_amount"].includes(f.key));
  if (named) return named;
  return [...amounts].reverse().find((f) => f.key.includes("total") || f.label_ko.includes("합계") || f.label_ko.includes("총액")) ?? amounts[0] ?? null;
}

/**
 * Resolves the field a request refers to within one schema (`aggregate::resolve_field`): by key or
 * label, else the type's total amount; a text that matches nothing falls back to the total only when
 * it asks for one. Tables / groups never qualify.
 */
export function resolveField(fields: FieldDef[], text: string | null | undefined): FieldDef | null {
  const key = compact(text ?? "");
  if (key) {
    const hit = fields.filter((f) => isScalar(f.kind)).find((f) => compact(f.key) === key || compact(f.label_ko) === key || compact(f.label_en) === key);
    if (hit) return hit;
    if (!asksForTotal(key)) return null;
  }
  return defaultTotal(fields);
}

function needsReview(f: FieldRecord): boolean {
  return f.review_reason !== null && !["approved", "corrected", "rejected"].includes(f.status);
}

function strictNumber(s: string | null | undefined): number | null {
  if (s == null) return null;
  const t = s.trim().replace(/,/g, "");
  if (!t || !/^[-+]?\d*\.?\d+$/.test(t)) return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
}

function normalizeAmount(s: string): number | null {
  const digits = s.replace(/[^0-9.-]/g, "");
  if (!/\d/.test(digits)) return null;
  const n = Number(digits);
  return Number.isFinite(n) ? n : null;
}

/** Numeric value of a document's field row (`fields` of the latest run); rejected fields have no value. */
function fieldValue(fields: FieldRecord[], def: FieldDef): [number, boolean] | null {
  const row = fields.find((f) => f.row === null && f.path === def.key) ?? fields.find((f) => f.row === null && f.key === def.key);
  if (!row || row.status === "rejected" || row.value === null) return null;
  const printed = row.value;
  let value = strictNumber(row.normalized);
  if (value === null) value = def.kind === "amount" || def.kind === "integer" ? normalizeAmount(printed) : strictNumber(printed);
  if (value === null) return null;
  return [value, needsReview(row)];
}

class Acc {
  included: number[] = [];
  reviewed: Totals = { value: 0, count: 0 };
  unreviewed: Totals = { value: 0, count: 0 };
  add(value: number, unreviewed: boolean, includeUnreviewed: boolean): void {
    const t = unreviewed ? this.unreviewed : this.reviewed;
    t.value += value;
    t.count += 1;
    if (!unreviewed || includeUnreviewed) this.included.push(value);
  }
  headline(func: AggregateFn): [number, number] {
    const n = this.included.length;
    const sum = this.included.reduce((a, b) => a + b, 0);
    let value: number;
    switch (func) {
      case "count":
        value = n;
        break;
      case "sum":
        value = sum;
        break;
      case "avg":
        value = n > 0 ? sum / n : 0;
        break;
      case "min":
        value = Math.min(...this.included);
        break;
      case "max":
        value = Math.max(...this.included);
        break;
    }
    return [Number.isFinite(value) ? value : 0, n];
  }
}

function groupKey(groupBy: GroupBy, doc: MockDoc, date: string | null): string | null {
  switch (groupBy) {
    case "none":
      return null;
    case "month":
      return date ? date.slice(0, 7) : UNKNOWN;
    case "vendor":
      return doc.vendor && doc.vendor.trim() ? doc.vendor.trim() : UNKNOWN;
    case "doc_type":
      return doc.doc_type && doc.doc_type.trim() ? doc.doc_type.trim() : UNKNOWN;
  }
}

/** Runs an aggregation over the documents of the request's scope (`aggregate::aggregate`). */
export function aggregate(req: AggregateRequest): AggregateResult {
  const func = req.func;
  const groupBy: GroupBy = req.group_by ?? "none";
  const countOnly = func === "count" && req.field == null;
  // A plain document count covers every document (the two review classes stay reported separately).
  const includeUnreviewed = (req.include_unreviewed ?? false) || countOnly;
  const docs = resolveScope(req.scope);
  const schemas = new Map<string, SchemaRecord | null>();
  const total = new Acc();
  const groups = new Map<string, Acc>();
  const entries: { doc: AggregateDocument; group: string | null }[] = [];
  let missing = 0;
  let resolved: [string, string] | null = null;
  let sawDocDate = false;
  let sawImported = false;
  for (const doc of docs) {
    const [date, basis] = basisDate(doc);
    sawDocDate ||= basis === "doc_date";
    sawImported ||= basis === "imported_at";
    const docPending = doc.review_status === "pending";
    let value: number | null;
    let fieldKey: string | null = null;
    let unreviewed = docPending;
    if (countOnly) value = 1;
    else {
      let def: FieldDef | null = null;
      if (doc.doc_type) {
        if (!schemas.has(doc.doc_type)) schemas.set(doc.doc_type, pl.schemaForDocType(doc.doc_type));
        const schema = schemas.get(doc.doc_type) ?? null;
        def = schema ? resolveField(schema.definition.fields, req.field) : null;
      }
      if (def) {
        const found = doc.last_run_id ? fieldValue(pl.latestFields(doc.id), def) : null;
        if (!resolved && found) resolved = [def.key, def.label_ko];
        value = found ? found[0] : null;
        fieldKey = def.key;
        unreviewed = docPending || (found?.[1] ?? false);
      } else value = null;
    }
    const group = groupKey(groupBy, doc, date);
    if (value !== null) {
      total.add(value, unreviewed, includeUnreviewed);
      if (group !== null) {
        const acc = groups.get(group) ?? new Acc();
        acc.add(value, unreviewed, includeUnreviewed);
        groups.set(group, acc);
      }
    } else missing += 1;
    entries.push({
      doc: { id: doc.id, name: doc.title ?? doc.original_name, rel_path: doc.rel_path, doc_type: doc.doc_type, date, basis, vendor: doc.vendor, value: countOnly ? null : value, field_key: fieldKey, reviewed: !unreviewed },
      group,
    });
  }
  // Groups that only hold missing documents still show up (with zero values).
  if (groupBy !== "none") for (const e of entries) if (e.group !== null && !groups.has(e.group)) groups.set(e.group, new Acc());
  const [value, count] = total.headline(func);
  const groupList: AggregateGroup[] = [...groups.entries()]
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([key, acc]) => {
      const [v, c] = acc.headline(func);
      return { key, value: v, count: c, reviewed: acc.reviewed, unreviewed: acc.unreviewed };
    });
  const documents = entries
    .map((e) => e.doc)
    .sort((a, b) => (a.date ?? "").localeCompare(b.date ?? "") || a.name.localeCompare(b.name) || a.id.localeCompare(b.id))
    .slice(0, 500);
  return {
    func,
    field: resolved?.[0] ?? null,
    field_label: resolved?.[1] ?? null,
    doc_type: req.scope?.filter?.doc_type ?? null,
    basis: sawDocDate && sawImported ? "mixed" : sawImported ? "imported_at" : "doc_date",
    date_from: req.scope?.filter?.date_from ?? null,
    date_to: req.scope?.filter?.date_to ?? null,
    include_unreviewed: req.include_unreviewed ?? false,
    value,
    count,
    reviewed: total.reviewed,
    unreviewed: total.unreviewed,
    missing,
    groups: groupList,
    documents,
  };
}

// ---------------------------------------------------------------------------
// Export (export.rs): preview + a simulated writer backed by an export registry
// ---------------------------------------------------------------------------

interface ExportEntry {
  format: ExportFormat;
  documents: number;
  rows: number;
  created_at: string;
  workspace: string;
}

function loadExports(): Record<string, ExportEntry> {
  return read<Record<string, ExportEntry>>(EXPORTS_KEY, {});
}

function exportExists(rel: string): boolean {
  const e = loadExports()[rel];
  return !!e && e.workspace === (currentWorkspacePath() ?? "");
}

interface ExportDoc {
  rec: MockDoc;
  date: string | null;
  fields: FieldRecord[];
}

interface Prepared {
  rel: string;
  absolute: string;
  exists: boolean;
  docs: ExportDoc[];
  skipped: [string, string][];
  schemas: Map<string, SchemaRecord>;
  folder: boolean;
}

/** Default target: `exports/<today>_<type label>.<ext>` (`문서` without a single type, `_표` / `_텍스트` for the table / text targets, `검색가능` for searchable PDFs). */
export function defaultExportPath(format: ExportFormat, target: ExportTarget, docTypeLabel: string | null, today: string): string {
  let name: string;
  if (format === "pdf_searchable") name = "검색가능";
  else {
    const label = docTypeLabel && docTypeLabel.trim() ? sanitizeComponent(docTypeLabel.trim()) : "문서";
    name = `${label}${target === "table" ? "_표" : target === "text" ? "_텍스트" : ""}`;
  }
  return `exports/${today}_${name}.${extensionOf(format)}`;
}

/** Workspace-relative, `/`-separated, sanitised path; absolute paths, `..` and `.doculens` are rejected (`path_outside`). */
function normalizeRelPath(input: string): string {
  const outside = () => err("path_outside", "path_outside");
  const s = input.trim().replace(/\\/g, "/");
  if (s.startsWith("/") || /^[A-Za-z]:/.test(s)) throw outside();
  const parts: string[] = [];
  for (const seg of s.split("/").map((x) => x.trim())) {
    if (!seg || seg === ".") continue;
    if (seg === ".." || seg.toLowerCase() === ".doculens") throw outside();
    parts.push(sanitizeComponent(seg));
  }
  if (parts.length === 0) throw outside();
  return parts.join("/");
}

function ensureExtension(rel: string, ext: string): string {
  return basename(rel).toLowerCase().endsWith(`.${ext}`) ? rel : `${rel}.${ext}`;
}

function stripExtension(rel: string): string {
  const i = rel.lastIndexOf("/");
  const dir = i >= 0 ? rel.slice(0, i + 1) : "";
  return `${dir}${stemOf(rel.slice(i + 1))}`;
}

function schemaDisplayName(s: SchemaRecord, ko: boolean): string {
  return ko ? s.name : s.name_en || s.name;
}

function scopeTypeLabel(req: ExportRequest, docs: ExportDoc[], schemas: Map<string, SchemaRecord>, ko: boolean): string | null {
  const labelOf = (slug: string) => {
    const s = schemas.get(slug);
    return s ? schemaDisplayName(s, ko) : slug;
  };
  const scoped = req.scope?.filter?.doc_type?.trim();
  if (scoped) return labelOf(scoped);
  const first = docs[0]?.rec.doc_type;
  if (!first) return null;
  return docs.every((d) => d.rec.doc_type === first) ? labelOf(first) : null;
}

function prepareExport(req: ExportRequest): Prepared {
  const ko = (req.language ?? "ko") === "ko";
  const target: ExportTarget = req.target ?? "fields";
  const wantFields = req.format !== "pdf_searchable" && target === "fields";
  const docs: ExportDoc[] = [];
  const skipped: [string, string][] = [];
  const schemas = new Map<string, SchemaRecord>();
  for (const rec of resolveScope(req.scope)) {
    if (req.only_reviewed && rec.review_status === "pending") {
      skipped.push([rec.id, "review_pending"]);
      continue;
    }
    if (rec.doc_type && !schemas.has(rec.doc_type)) {
      const s = pl.schemaForDocType(rec.doc_type);
      if (s) schemas.set(rec.doc_type, s);
    }
    docs.push({ rec, date: basisDate(rec)[0], fields: wantFields && rec.last_run_id ? pl.latestFields(rec.id) : [] });
  }
  const label = scopeTypeLabel(req, docs, schemas, ko);
  let rel = req.path && req.path.trim() ? normalizeRelPath(req.path) : defaultExportPath(req.format, target, label, todayIso());
  rel = ensureExtension(rel, extensionOf(req.format));
  const folder = req.format === "pdf_searchable" && docs.length > 1;
  if (folder) rel = stripExtension(rel);
  const ws = currentWorkspacePath() ?? "";
  return { rel, absolute: `${ws}/${rel}`, exists: exportExists(rel), docs, skipped, schemas, folder };
}

interface Column {
  path: string;
  key: string;
  label_ko: string;
  label_en: string;
}

function columnLabel(c: Column, ko: boolean): string {
  return ko || !c.label_en ? c.label_ko : c.label_en;
}

/** Scalar columns of one document: its type's schema (group members with the group label prefixed), or, without a schema, the scalar rows it has; `req.columns` restricts them. */
function docColumns(doc: ExportDoc, schemas: Map<string, SchemaRecord>, req: ExportRequest): Column[] {
  const schema = doc.rec.doc_type ? schemas.get(doc.rec.doc_type) : undefined;
  let out: Column[] = [];
  if (schema) {
    for (const f of schema.definition.fields) {
      if (f.kind === "table") continue;
      if (f.kind === "group") {
        const join = (g: string, m: string) => (m.startsWith(g) ? m : `${g} ${m}`);
        for (const c of f.fields.filter((x) => isScalar(x.kind))) out.push({ path: `${f.key}.${c.key}`, key: c.key, label_ko: join(f.label_ko, c.label_ko), label_en: join(f.label_en, c.label_en) });
      } else out.push({ path: f.key, key: f.key, label_ko: f.label_ko, label_en: f.label_en });
    }
  } else {
    for (const f of doc.fields.filter((x) => x.row === null)) out.push({ path: f.path, key: f.key, label_ko: f.label_ko || f.path, label_en: f.label_en || f.path });
  }
  const wanted = (req.columns ?? []).map(compact).filter(Boolean);
  if (wanted.length > 0) out = out.filter((c) => wanted.some((w) => w === compact(c.path) || w === compact(c.key) || w === compact(c.label_ko) || w === compact(c.label_en)));
  return out;
}

function unionColumns(docs: ExportDoc[], schemas: Map<string, SchemaRecord>, req: ExportRequest): Column[] {
  const out: Column[] = [];
  for (const doc of docs) for (const c of docColumns(doc, schemas, req)) if (!out.some((o) => o.path === c.path)) out.push(c);
  return out;
}

function fieldsHeader(cols: Column[], req: ExportRequest): string[] {
  const ko = (req.language ?? "ko") === "ko";
  const include = req.include ?? [];
  const h = [tr(ko, "파일", "File"), tr(ko, "유형", "Type"), tr(ko, "날짜", "Date"), tr(ko, "검토 상태", "Review")];
  if (include.includes("confidence")) h.push(tr(ko, "신뢰도", "Confidence"));
  for (const c of cols) {
    const label = columnLabel(c, ko);
    h.push(label);
    if (include.includes("status")) h.push(`${label} ${tr(ko, "(상태)", "(status)")}`);
    if (include.includes("confidence")) h.push(`${label} ${tr(ko, "(신뢰도)", "(confidence)")}`);
    if (include.includes("evidence")) h.push(`${label} ${tr(ko, "(근거)", "(evidence)")}`);
  }
  return h;
}

function typeLabelOf(doc: ExportDoc, schemas: Map<string, SchemaRecord>, ko: boolean): string {
  const slug = doc.rec.doc_type;
  if (!slug) return tr(ko, UNKNOWN, "unknown");
  const s = schemas.get(slug);
  return s ? schemaDisplayName(s, ko) : slug;
}

function fitUtf16(s: string, max: number): string {
  let out = "";
  let units = 0;
  for (const c of s) {
    units += c.length;
    if (units > max) break;
    out += c;
  }
  return out;
}

/** Excel-safe worksheet names (no `[ ] : * ? / \`, ≤ 31 UTF-16 units), made unique with `_2`, `_3` …. */
function sheetNames(raw: string[]): string[] {
  const used: string[] = [];
  for (const r of raw) {
    let base = "";
    for (const c of r) base += /[[\]:*?/\\]/.test(c) || /\p{Cc}/u.test(c) ? "_" : c;
    base = base.trim().replace(/^'+|'+$/g, "").trim() || "Sheet";
    const taken = (name: string) => used.some((u) => u.toLowerCase() === name.toLowerCase());
    let name = fitUtf16(base, 31);
    let n = 2;
    while (taken(name)) {
      const suffix = `_${n}`;
      name = `${fitUtf16(base, 31 - suffix.length)}${suffix}`;
      n += 1;
    }
    used.push(name);
  }
  return used;
}

/** Grids of the fields target: one per type (or a single one) plus the long-form items grid. */
function fieldsSheetNames(p: Prepared, req: ExportRequest): string[] {
  const ko = (req.language ?? "ko") === "ko";
  const names: string[] = [];
  if ((req.layout ?? "sheet_per_type") === "single") names.push(tr(ko, "필드", "Fields"));
  else {
    const seen = new Set<string>();
    for (const d of p.docs) {
      const slug = d.rec.doc_type ?? "";
      if (seen.has(slug)) continue;
      seen.add(slug);
      names.push(typeLabelOf(d, p.schemas, ko));
    }
  }
  names.push(tr(ko, "품목", "Items"));
  return sheetNames(names);
}

function tableSheetName(doc: ExportDoc, page: number, index: number): string {
  return `${stemOf(doc.rec.original_name)}_p${page}_${index + 1}`;
}

/** Describes the export without writing anything (`export::preview`). */
export function exportPreview(req: ExportRequest): ExportPreview {
  const p = prepareExport(req);
  const ko = (req.language ?? "ko") === "ko";
  const target: ExportTarget = req.target ?? "fields";
  let columns: string[] = [];
  let sheets: string[] = [];
  if (req.format === "pdf_searchable") {
    /* no columns / sheets */
  } else if (target === "fields") {
    columns = fieldsHeader(unionColumns(p.docs, p.schemas, req), req);
    if (req.format === "xlsx") sheets = fieldsSheetNames(p, req);
  } else if (target === "table") {
    if (req.format === "xlsx") sheets = sheetNames(p.docs.flatMap((d) => pl.documentTables(d.rec.id).map((t) => tableSheetName(d, t.page, t.index))));
  } else {
    columns = [tr(ko, "파일", "File"), tr(ko, "페이지", "Page"), tr(ko, "텍스트", "Text")];
    if (req.format === "xlsx") sheets = [tr(ko, "텍스트", "Text")];
  }
  return { path: p.rel, exists: p.exists, documents: p.docs.length, skipped: p.skipped.length, columns, sheets };
}

/** "Writes" the export into the registry (`export::export`); `cancel` is checked between documents. */
export async function runExport(req: ExportRequest, ctl: JobControl | null): Promise<ExportResult> {
  const p = prepareExport(req);
  if (p.exists && !req.overwrite) throw err("export_exists", "export_exists");
  if (p.docs.length === 0) throw err("no_documents", "no_documents");
  const target: ExportTarget = req.target ?? "fields";
  const ko = (req.language ?? "ko") === "ko";
  const skipped: [string, string][] = [...p.skipped];
  const extra: string[] = [];
  let sheets: string[] = [];
  let rows = 0;
  let documents = p.docs.length;
  if (req.format === "pdf_searchable") {
    const used = new Set<string>();
    const written: ExportDoc[] = [];
    for (const d of p.docs) {
      await pace(ctl, 60);
      if (d.rec.pages.length === 0) {
        skipped.push([d.rec.id, "no_pages"]);
        continue;
      }
      rows += d.rec.pages.length;
      written.push(d);
      if (p.folder) {
        let name = sanitizeComponent(stemOf(d.rec.original_name));
        let n = 2;
        while (used.has(name.toLowerCase())) name = `${sanitizeComponent(stemOf(d.rec.original_name))}_${n++}`;
        used.add(name.toLowerCase());
        extra.push(`${p.rel}/${name}.pdf`);
      }
    }
    documents = written.length;
    if (documents === 0) throw err("no_pages", "no_pages");
  } else if (target === "fields") {
    for (let i = 0; i < p.docs.length; i += 1) {
      await pace(ctl, 40);
      rows += 1;
    }
    const items = p.docs.reduce((n, d) => n + d.fields.filter((f) => f.row !== null).length, 0);
    rows += items;
    if (req.format === "xlsx") sheets = fieldsSheetNames(p, req);
    else if (req.format === "csv" && items > 0) extra.push(`${stripExtension(p.rel)}_items.csv`);
  } else if (target === "table") {
    const names: string[] = [];
    let found = 0;
    for (const d of p.docs) {
      await pace(ctl, 40);
      const tables = pl.documentTables(d.rec.id);
      if (tables.length === 0) {
        skipped.push([d.rec.id, "no_tables"]);
        continue;
      }
      found += 1;
      for (const t of tables) {
        rows += t.rows.length;
        names.push(tableSheetName(d, t.page, t.index));
      }
    }
    if (found === 0) throw err("no_tables", "no_tables");
    documents = found;
    if (req.format === "xlsx") sheets = sheetNames(names);
  } else {
    let found = 0;
    for (const d of p.docs) {
      await pace(ctl, 40);
      const pages = d.rec.pages.filter((pg) => (pg.text ?? "").trim() !== "");
      if (pages.length === 0) {
        skipped.push([d.rec.id, "no_text"]);
        continue;
      }
      found += 1;
      rows += pages.length;
    }
    if (found === 0) throw err("no_text", "no_text");
    documents = found;
    if (req.format === "xlsx") sheets = [tr(ko, "텍스트", "Text")];
  }
  if (ctl?.cancelled()) throw cancelledError();
  const all = loadExports();
  all[p.rel] = { format: req.format, documents, rows, created_at: now(), workspace: currentWorkspacePath() ?? "" };
  write(EXPORTS_KEY, all);
  return { path: p.rel, absolute: p.absolute, format: req.format, target, documents, rows, sheets, overwritten: p.exists, skipped, extra_files: extra };
}

// ---------------------------------------------------------------------------
// Preview (planner/exec.rs `preview`): the confirmation card's side effects
// ---------------------------------------------------------------------------

function organizePreview(req: OrganizeRequest, ko: boolean, confirm: boolean): [string, unknown, boolean] {
  try {
    const p = organizePlan(req);
    const s = ko
      ? `이동 ${p.moves}건 · 보류 ${p.held}건 · 변경 없음 ${p.unchanged}건${p.needs_review > 0 ? ` · 값 미상 ${p.needs_review}건` : ""}`
      : `${p.moves} moves · ${p.held} held · ${p.unchanged} unchanged${p.needs_review > 0 ? ` · ${p.needs_review} with unknown values` : ""}`;
    return [s, { ...p, items: p.items.slice(0, 200) }, confirm && p.moves > 0];
  } catch (e) {
    return [messageOf(e), null, false];
  }
}

/** Previews the side effects of a plan for the confirmation card (`exec::preview`). */
export function planPreview(plan: Plan, ctx: PlanContext): PlanPreview {
  const ko = ctx.language === "ko";
  const settings = loadSettings();
  const docs = resolveScope(plan.scope);
  const out: PlanPreview = { document_count: docs.length, document_ids: docs.map((d) => d.id), steps: [], requires_confirmation: plan.requires_confirmation };
  plan.operations.forEach((op, index) => {
    let summary = "";
    let payload: unknown = null;
    let confirm = false;
    switch (op.op) {
      case "organize":
        [summary, payload, confirm] = organizePreview(
          { scope: plan.scope, name_template: op.template ?? DEFAULT_NAME_TEMPLATE, folder_template: op.folder_template ?? FOLDER_TEMPLATES[op.group_by ?? "doc_type"], only_reviewed: op.only_reviewed ?? settings.processing.organize_reviewed_only, language: ctx.language },
          ko,
          op.dry_run !== true,
        );
        break;
      case "rename":
        [summary, payload, confirm] = organizePreview({ scope: plan.scope, name_template: op.template, folder_template: null, only_reviewed: op.only_reviewed ?? settings.processing.organize_reviewed_only, language: ctx.language }, ko, true);
        break;
      case "move":
        [summary, payload, confirm] = organizePreview({ scope: plan.scope, name_template: null, folder_template: op.folder, only_reviewed: op.only_reviewed ?? settings.processing.organize_reviewed_only, language: ctx.language }, ko, true);
        break;
      case "export": {
        try {
          const p = exportPreview({ format: op.format, target: op.target ?? "fields", path: op.path ?? null, columns: op.columns ?? [], layout: op.layout ?? "sheet_per_type", include: op.include ?? [], scope: plan.scope, only_reviewed: false, overwrite: false, language: ctx.language });
          summary = `${countLabel(p.documents, ko)} → ${p.path}${p.exists ? tr(ko, " (기존 파일 덮어씀)", " (overwrites)") : ""}`;
          payload = p;
          confirm = p.exists;
        } catch (e) {
          summary = messageOf(e);
        }
        break;
      }
      case "extract": {
        const name = (op.schema_ref ? pl.getSchema(op.schema_ref)?.name : null) ?? tr(ko, "자동(유형별 스키마)", "auto");
        const n = (op.fields ?? []).length;
        summary = ko ? `스키마 ${name} · ${n === 0 ? "전체 필드" : `필드 ${n}개`}` : `schema ${name} · ${n === 0 ? "all fields" : `${n} fields`}`;
        payload = { schema: name, fields: op.fields ?? [] };
        break;
      }
      case "delete":
        summary = ko ? `${countLabel(docs.length, ko)} 휴지통으로` : `${countLabel(docs.length, ko)} to the recycle bin`;
        payload = { documents: docs.length };
        confirm = true;
        break;
      case "undo": {
        const b = op.target ? (listBatches(500).find((x) => x.id === op.target) ?? null) : lastUndoable();
        if (b) {
          summary = ko ? `되돌리기: ${b.label ?? b.kind} (${b.entries}건)` : `Undo: ${b.label ?? b.kind} (${b.entries} entries)`;
          payload = b;
          confirm = true;
        } else summary = tr(ko, "되돌릴 작업이 없습니다", "Nothing to undo");
        break;
      }
      case "redo": {
        const b = lastRedoable();
        if (b) {
          summary = ko ? `다시 실행: ${b.entries}건` : `Redo: ${b.entries} entries`;
          payload = b;
          confirm = true;
        } else summary = tr(ko, "다시 실행할 작업이 없습니다", "Nothing to redo");
        break;
      }
      case "classify":
      case "ocr":
      case "validate":
      case "summarize":
      case "tag":
      case "set_doc_type":
        summary = countLabel(docs.length, ko);
        payload = { documents: docs.length };
        break;
      case "import":
        // The browser mock has no inbox files to register (`documents_inbox_scan` is a no-op).
        summary = ko ? "신규 0개" : "0 new files";
        payload = { new: 0 };
        break;
      default:
        confirm = needsConfirmation(op);
        break;
    }
    if (confirm) out.requires_confirmation = true;
    out.steps.push({ index, op: op.op, summary, payload, confirm } satisfies StepPreview);
  });
  return out;
}

// ---------------------------------------------------------------------------
// Executor (planner/exec.rs `run`)
// ---------------------------------------------------------------------------

export interface ExecProgress {
  step: number;
  steps: number;
  op: OpKind | null;
  stage: string;
  done: number;
  total: number;
  current: string;
}

interface StepOutcome {
  summary: string;
  payload: unknown;
  imported?: string[] | null;
}

type StepProgress = (stage: string, done: number, total: number, current: string) => void;

function idsOf(docs: MockDoc[]): string[] {
  return docs.map((d) => d.id);
}

/** Documents the plan works on right now: the scope, narrowed to what an earlier import step registered. */
function workingSet(plan: Plan, imported: string[] | null): MockDoc[] {
  const docs = resolveScope(plan.scope);
  return imported ? docs.filter((d) => imported.includes(d.id)) : docs;
}

function requireModel(): void {
  const mode = pl.visionAvailability();
  if (mode === "none") throw err("invalid", "no_model");
  if (mode === "engine_off") throw err("invalid", "engine_not_running");
}

async function pipelineStep(plan: Plan, imported: string[] | null, ctx: PlanContext, ctl: JobControl | null, req: Omit<ProcessRequest, "document_ids">, progress: StepProgress, out: PlanExecution): Promise<StepOutcome> {
  const ko = ctx.language === "ko";
  requireModel();
  const docs = workingSet(plan, imported);
  if (docs.length === 0) return { summary: tr(ko, "대상 문서 없음", "No documents in scope"), payload: { documents: [] } };
  const report = await pl.runProcess({ ...req, document_ids: idsOf(docs) }, ctl, (p) => progress(req.level, p.done, p.total, p.current));
  out.documents.push(...report.processed, ...report.failed.map(([id]) => id));
  if (report.cancelled) throw cancelledError();
  const failedTail = report.failed.length > 0 ? tr(ko, ` · 실패 ${report.failed.length}건`, ` · ${report.failed.length} failed`) : "";
  const summary =
    req.level === "classify"
      ? tr(ko, `${report.classified}건 분류`, `${report.classified} classified`)
      : req.level === "ocr"
        ? tr(ko, `OCR ${report.ocr_pages}쪽`, `OCR ${report.ocr_pages} pages`)
        : tr(ko, `${report.extracted}건 추출 · 검토 대기 ${report.review_pending}건${failedTail}`, `${report.extracted} extracted · ${report.review_pending} need review${failedTail}`);
  if (report.failed.length > 0 && report.processed.length === 0) throw err("invalid", report.failed.map(([id, e]) => `${id}: ${e}`).join("; "));
  return { summary, payload: report };
}

async function organizeStep(plan: Plan, imported: string[] | null, ctx: PlanContext, ctl: JobControl | null, nameTemplate: string | null, folderTemplate: string | null, onlyReviewed: boolean, dryRun: boolean, kind: string, progress: StepProgress, out: PlanExecution): Promise<StepOutcome> {
  const ko = ctx.language === "ko";
  const scope: Scope = imported ? { ...plan.scope, doc_ids: imported } : plan.scope;
  const oplan = organizePlan({ scope, name_template: nameTemplate, folder_template: folderTemplate, only_reviewed: onlyReviewed, language: ctx.language });
  if (dryRun) return { summary: ko ? `미리보기: 이동 ${oplan.moves}건 · 보류 ${oplan.held}건 · 변경 없음 ${oplan.unchanged}건` : `Preview: ${oplan.moves} moves · ${oplan.held} held · ${oplan.unchanged} unchanged`, payload: oplan };
  const label = ko ? `${kind === "rename" ? "이름 변경" : kind === "move" ? "이동" : "정리"} ${oplan.moves}건` : `${kind} ${oplan.moves} files`;
  const report = await applyOrganize(oplan, kind, label, "planner", ctl, (done, total, name) => progress(kind, done, total, name));
  out.documents.push(...report.documents);
  if (report.batch_id) out.batches.push(report.batch_id);
  if (report.cancelled) throw cancelledError();
  return {
    summary: ko ? `이동 ${report.moved}건 · 보류 ${oplan.held}건 · 실패 ${report.failed.length}건` : `${report.moved} moved · ${oplan.held} held · ${report.failed.length} failed`,
    payload: { batch_id: report.batch_id, moved: report.moved, held: oplan.held, failed: report.failed, items: report.items },
  };
}

async function runStep(plan: Plan, op: Op, imported: string[] | null, ctx: PlanContext, ctl: JobControl | null, progress: StepProgress, out: PlanExecution): Promise<StepOutcome> {
  const ko = ctx.language === "ko";
  const settings = loadSettings();
  switch (op.op) {
    case "import": {
      if (op.source === "files") throw err("invalid", "import_files_gui");
      if (op.source === "folder" && !(op.path ?? "").trim()) throw err("invalid", "path_required");
      // The browser mock has no unregistered inbox files; documents already registered from the inbox stay in scope.
      await pace(ctl, 120);
      const existing = idsOf(resolveScope(plan.scope));
      return {
        summary: ko ? "가져오기 0건(신규 0, 다시 연결 0, 건너뜀 0, 실패 0)" : "Imported 0 (new 0, relinked 0, skipped 0, failed 0)",
        payload: { imported: [], relinked: [], failed: [], skipped: 0 },
        imported: existing,
      };
    }
    case "classify":
      return pipelineStep(plan, imported, ctx, ctl, { level: "classify", schema: { kind: "auto" }, force: op.force ?? false, profile: null, pages: [], precise: null }, progress, out);
    case "ocr":
      return pipelineStep(plan, imported, ctx, ctl, { level: "ocr", schema: { kind: "auto" }, force: op.force ?? false, profile: op.profile === "fine_print" ? "fine_print" : op.profile === "standard" ? settings.models.profile : null, pages: op.pages ?? [], precise: null }, progress, out);
    case "extract": {
      let schema: ProcessRequest["schema"] = { kind: "auto" };
      if (op.schema_ref && op.schema_ref.toLowerCase() !== "auto") {
        const s = pl.getSchema(op.schema_ref) ?? pl.schemaForDocType(op.schema_ref);
        if (!s) throw err("invalid", `schema_not_found: ${op.schema_ref}`);
        schema = { kind: "id", id: s.id };
      }
      return pipelineStep(plan, imported, ctx, ctl, { level: "extract", schema, force: op.force ?? false, profile: null, pages: [], precise: null }, progress, out);
    }
    case "set_doc_type": {
      const docs = workingSet(plan, imported);
      for (const d of docs) {
        d.doc_type = op.doc_type;
        d.doc_type_source = "manual";
        d.updated_at = now();
        upsertDoc(d);
        out.documents.push(d.id);
      }
      return { summary: `${countLabel(docs.length, ko)} → ${op.doc_type}`, payload: { documents: idsOf(docs) } };
    }
    case "validate": {
      const docs = workingSet(plan, imported);
      let flagged = 0;
      for (const [i, d] of docs.entries()) {
        if (ctl) await ctl.checkpoint();
        progress("validate", i, docs.length, d.original_name);
        flagged += pl.revalidateDocument(d.id).filter(needsReview).length;
        out.documents.push(d.id);
      }
      return { summary: ko ? `${countLabel(docs.length, ko)} 검증 · 검토 대기 필드 ${flagged}개` : `Validated ${countLabel(docs.length, ko)} · ${flagged} fields need review`, payload: { documents: idsOf(docs), review_fields: flagged } };
    }
    case "review": {
      const docs = workingSet(plan, imported);
      const reason = op.only === "low_confidence" ? "low_confidence" : op.only === "failed" ? "validation_failed" : null;
      let changed = 0;
      for (const d of docs) {
        if (ctl) await ctl.checkpoint();
        if (op.action === "approve") changed += pl.approveDocument(d.id, reason);
        else if (op.action === "reject") changed += pl.rejectPending(d.id, reason);
        out.documents.push(d.id);
      }
      return { summary: ko ? `필드 ${changed}개 ${op.action === "approve" ? "승인" : "거부"}` : `${changed} fields ${op.action === "approve" ? "approved" : "rejected"}`, payload: { changed, documents: idsOf(docs) } };
    }
    case "search": {
      const docs = scopeIsEmpty(plan.scope) ? [] : workingSet(plan, imported);
      const resp = (await mockKnowledge("knowledge_search", { req: { query: op.query, mode: op.mode ?? "hybrid", limit: Math.min(200, Math.max(1, op.top_k ?? 20)), scope: { documents: idsOf(docs), latest_only: settings.knowledge.latest_only } } })) as SearchResponse;
      return { summary: ko ? `'${op.query}' 검색 결과 ${resp.hits.length}건` : `${resp.hits.length} hits for '${op.query}'`, payload: { ...resp, total: resp.total_candidates } };
    }
    case "answer": {
      requireModel();
      const docs = scopeIsEmpty(plan.scope) ? [] : workingSet(plan, imported);
      const ans = (await mockKnowledge("knowledge_answer", {
        req: { question: op.question, kind: "answer", scope: { documents: idsOf(docs), latest_only: settings.knowledge.latest_only }, allow_general: settings.knowledge.allow_general_knowledge, top_k: settings.knowledge.top_k, context_tokens: settings.knowledge.context_tokens, min_similarity: settings.knowledge.min_vector_similarity },
        requestId: `plan-${newId()}`,
      })) as Answer;
      const text = Array.from(ans.text);
      return { summary: ans.no_evidence ? tr(ko, "근거를 찾지 못했습니다", "No evidence found") : text.length > 200 ? `${text.slice(0, 200).join("")}…` : ans.text, payload: ans };
    }
    case "summarize": {
      requireModel();
      const docs = workingSet(plan, imported);
      if (docs.length === 0) throw err("no_documents", "no_documents");
      const mode = op.length === "short" ? "one_line" : op.length === "long" ? "one_page" : "key_points";
      const summaries: unknown[] = [];
      for (const [i, d] of docs.entries()) {
        if (ctl) await ctl.checkpoint();
        progress("summarize", i, docs.length, d.original_name);
        const ans = await mockKnowledge("knowledge_answer", { req: { question: "", kind: "summary", document_id: d.id, summary_mode: mode, context_tokens: settings.knowledge.context_tokens }, requestId: `plan-${newId()}` });
        summaries.push({ document_id: d.id, name: d.original_name, answer: ans });
      }
      return { summary: ko ? `${countLabel(docs.length, ko)} 요약` : `Summarised ${countLabel(docs.length, ko)}`, payload: { summaries } };
    }
    case "aggregate": {
      const res = aggregate({ func: op.fn, field: op.field ?? null, group_by: op.group_by ?? "none", include_unreviewed: op.include_unreviewed ?? false, scope: plan.scope });
      const value = formatValue(res.func, res.value);
      const summary = ko
        ? op.fn === "count"
          ? `${value} 건`
          : `${res.field_label ?? ""} ${{ sum: "합계", avg: "평균", min: "최소", max: "최대" }[op.fn]}: ${value}${res.unreviewed.count > 0 && !(op.include_unreviewed ?? false) ? ` (검토 대기 ${res.unreviewed.count}건 제외)` : ""}`
        : `${op.fn} of ${res.field_label ?? "documents"}: ${value}`;
      return { summary, payload: res };
    }
    case "organize":
      return organizeStep(plan, imported, ctx, ctl, op.template ?? DEFAULT_NAME_TEMPLATE, op.folder_template ?? FOLDER_TEMPLATES[op.group_by ?? "doc_type"], op.only_reviewed ?? settings.processing.organize_reviewed_only, op.dry_run ?? false, "organize", progress, out);
    case "rename":
      return organizeStep(plan, imported, ctx, ctl, op.template, null, op.only_reviewed ?? settings.processing.organize_reviewed_only, false, "rename", progress, out);
    case "move":
      return organizeStep(plan, imported, ctx, ctl, null, op.folder, op.only_reviewed ?? settings.processing.organize_reviewed_only, false, "move", progress, out);
    case "tag": {
      const docs = workingSet(plan, imported);
      const add = op.add ?? [];
      const remove = op.remove ?? [];
      for (const d of docs) {
        const tags = d.tags.filter((t) => !remove.some((r) => r.toLowerCase() === t.toLowerCase()));
        for (const a of add) if (!tags.some((t) => t.toLowerCase() === a.toLowerCase())) tags.push(a);
        d.tags = tags;
        d.updated_at = now();
        upsertDoc(d);
        out.documents.push(d.id);
      }
      return { summary: ko ? `${countLabel(docs.length, ko)}에 태그 적용(+${add.length} −${remove.length})` : `Tagged ${countLabel(docs.length, ko)} (+${add.length} −${remove.length})`, payload: { documents: idsOf(docs) } };
    }
    case "delete": {
      const docs = workingSet(plan, imported);
      const label = ko ? `${countLabel(docs.length, ko)} 삭제` : `Delete ${countLabel(docs.length, ko)}`;
      const report = await deleteDocuments(idsOf(docs), label, "planner", ctl);
      out.documents.push(...report.documents);
      if (report.batch_id) out.batches.push(report.batch_id);
      if (report.cancelled) throw cancelledError();
      return { summary: ko ? `${report.moved}건 휴지통으로 이동` : `${report.moved} moved to the recycle bin`, payload: report };
    }
    case "export": {
      const res = await runExport({ format: op.format, target: op.target ?? "fields", path: op.path ?? null, columns: op.columns ?? [], layout: op.layout ?? "sheet_per_type", include: op.include ?? [], scope: imported ? { ...plan.scope, doc_ids: imported } : plan.scope, only_reviewed: false, overwrite: true, language: ctx.language }, ctl);
      return { summary: `${countLabel(res.documents, ko)} → ${res.path}`, payload: res };
    }
    case "undo": {
      const target = op.target ?? lastUndoable()?.id ?? null;
      if (!target) return { summary: tr(ko, "되돌릴 작업이 없습니다", "Nothing to undo"), payload: null };
      const r = undoBatch(target);
      out.documents.push(...r.documents);
      out.restored_paths = [...(out.restored_paths ?? []), ...r.restored_paths];
      out.batches.push(r.batch_id);
      return { summary: ko ? `${r.restored}건 되돌림${r.skipped.length > 0 ? ` · ${r.skipped.length}건 불가` : ""}` : `${r.restored} restored${r.skipped.length > 0 ? ` · ${r.skipped.length} skipped` : ""}`, payload: r };
    }
    case "redo": {
      const b = lastRedoable();
      if (!b) return { summary: tr(ko, "다시 실행할 작업이 없습니다", "Nothing to redo"), payload: null };
      const r = undoBatch(b.id);
      out.documents.push(...r.documents);
      out.batches.push(r.batch_id);
      return { summary: ko ? `${r.restored}건 다시 실행` : `${r.restored} redone`, payload: r };
    }
    default:
      // UI-side ops never reach here (`runsInUi`).
      return { summary: "", payload: null };
  }
}

/** Runs the plan step by step (`exec::run`): the first failure stops it (later steps are `skipped`), cancellation marks it cancelled. */
export async function runPlan(plan: Plan, ctx: PlanContext, ctl: JobControl | null, progress: (p: ExecProgress) => void): Promise<PlanExecution> {
  const out: PlanExecution = { steps: [], cancelled: false, documents: [], ui_actions: [], batches: [], restored_paths: [] };
  const steps = plan.operations.length;
  let imported: string[] | null = null;
  let stop = false;
  for (const [index, op] of plan.operations.entries()) {
    const kind = op.op;
    const cancelledNow = ctl?.cancelled() ?? false;
    if (stop || cancelledNow) {
      out.steps.push({ index, op: kind, status: cancelledNow ? "cancelled" : "skipped", summary: "", payload: null, error: null, elapsed_ms: 0 });
      if (cancelledNow) out.cancelled = true;
      continue;
    }
    if (runsInUi(kind)) {
      out.ui_actions.push(op);
      out.steps.push({ index, op: kind, status: kind === "clarify" || kind === "noop" ? "skipped" : "ui", summary: "", payload: null, error: null, elapsed_ms: 0 });
      continue;
    }
    const started = performance.now();
    progress({ step: index, steps, op: kind, stage: kind, done: 0, total: 0, current: "" });
    const report: StepProgress = (stage, done, total, current) => progress({ step: index, steps, op: kind, stage, done, total, current });
    let result: StepResult;
    try {
      const o = await runStep(plan, op, imported, ctx, ctl, report, out);
      if (o.imported) imported = o.imported;
      result = { index, op: kind, status: "done", summary: o.summary, payload: o.payload, error: null, elapsed_ms: Math.round(performance.now() - started) };
    } catch (e) {
      const elapsed = Math.round(performance.now() - started);
      if (isCancellation(e)) {
        out.cancelled = true;
        result = { index, op: kind, status: "cancelled", summary: "", payload: null, error: null, elapsed_ms: elapsed };
      } else result = { index, op: kind, status: "failed", summary: "", payload: null, error: messageOf(e), elapsed_ms: elapsed };
      stop = true;
    }
    out.steps.push(result);
  }
  out.documents = [...new Set(out.documents)].sort();
  return out;
}

/** Ops that run inline (quick, no model, no files) — `planner_bridge::inline_only`. */
export function inlineOnly(plan: Plan): boolean {
  return plan.operations.every((op) => runsInUi(op.op) || ["search", "aggregate", "tag", "set_doc_type", "validate"].includes(op.op) || (op.op === "review" && (op.action === "approve" || op.action === "reject")));
}

/** Emits change events for what an execution touched (`planner_bridge::after_execution`). */
function afterExecution(ex: PlanExecution): void {
  if (ex.documents.length > 0) {
    docsChanged("updated", ex.documents);
    pipelineChanged("processed", ex.documents);
  }
  if (ex.batches.length > 0) pipelineChanged("journal", []);
  reimportRestored(ex.restored_paths ?? []);
}

/** Files an undo brought back from the trash: the mock restored their records in place, so it only announces them. */
function reimportRestored(paths: string[]): void {
  if (paths.length === 0) return;
  const ids = wsDocs()
    .filter((d) => paths.includes(d.rel_path))
    .map((d) => d.id);
  docsChanged("imported", ids);
}

// ---------------------------------------------------------------------------
// Persistent-job workers (planner_bridge::run_spec)
// ---------------------------------------------------------------------------

async function planWorker(job: Job, plan: Plan, context: PlanContext, ctl: JobControl): Promise<WorkerOutcome> {
  if (planNeedsModel(plan)) {
    updateJob(job.id, { detail: "loading model" });
    const mode = pl.visionAvailability();
    if (mode === "engine_off") throw workerFailure("engine_not_running");
    if (mode === "none") throw workerFailure("no_model");
    await pace(ctl, 240);
    pl.markVisionLoaded();
  }
  const steps = plan.operations.length;
  const execution = await runPlan(plan, context, ctl, (p) => {
    const inner = p.total === 0 ? 0 : p.done / p.total;
    const frac = steps === 0 ? null : Math.min(1, (p.step + inner * 0.9) / steps);
    const detail = p.total > 0 ? `${p.step + 1}/${steps} · ${p.stage} · ${p.current} (${p.done}/${p.total})` : `${p.step + 1}/${steps} · ${p.stage}`;
    updateJob(job.id, { progress: frac, bytes_done: p.step, bytes_total: steps, detail });
  });
  afterExecution(execution);
  if (execution.cancelled) throw workerFailure("cancelled", execution);
  const failed = execution.steps.find((s) => s.status === "failed");
  if (failed) throw workerFailure(`${failed.op}: ${failed.error ?? ""}`, execution);
  const done = execution.steps.filter((s) => s.status === "done").length;
  const last = [...execution.steps].reverse().find((s) => s.status === "done")?.summary ?? "";
  return { detail: `plan ${done}/${execution.steps.length} · ${last}`, result: execution };
}

async function exportWorker(req: ExportRequest, ctl: JobControl): Promise<WorkerOutcome> {
  try {
    const res = await runExport(req, ctl);
    return { detail: `exported ${res.documents} documents → ${res.path}`, result: res };
  } catch (e) {
    throw workerFailure(messageOf(e));
  }
}

async function organizeWorker(job: Job, spec: Extract<JobSpec, { kind: "organize" }>, ctl: JobControl): Promise<WorkerOutcome> {
  const report = await applyOrganize(spec.plan, spec.batch_kind, spec.label, spec.source, ctl, (done, total, name) => {
    updateJob(job.id, { progress: total === 0 ? null : done / total, bytes_done: done, bytes_total: total, detail: name });
  });
  docsChanged("updated", report.documents);
  pipelineChanged("journal", []);
  if (report.cancelled) throw workerFailure("cancelled", report);
  return { detail: `moved ${report.moved} / failed ${report.failed.length} / skipped ${report.skipped}`, result: report };
}

function undoWorker(batchId: string | null, redo: boolean): WorkerOutcome {
  const batch = redo ? lastRedoable() : batchId ? (listBatches(500).find((b) => b.id === batchId) ?? null) : lastUndoable();
  if (!batch) return { detail: redo ? "nothing to redo" : "nothing to undo" };
  const report = undoBatch(batch.id);
  docsChanged("updated", report.documents);
  pipelineChanged("journal", []);
  reimportRestored(report.restored_paths);
  return { detail: `restored ${report.restored} / skipped ${report.skipped.length}`, result: report };
}

/** Runs a persistent job's spec (installed on the mock job book by `startJob`). */
export async function runJobSpec(job: Job, spec: JobSpec, ctl: JobControl): Promise<WorkerOutcome> {
  switch (spec.kind) {
    case "pipeline":
      return pl.pipelineWorker(job, spec.req, ctl);
    case "plan": {
      const out = await planWorker(job, spec.plan, spec.context, ctl);
      const kb = await import("./mock-knowledge");
      kb.autoIndex();
      return out;
    }
    case "export":
      return exportWorker(spec.req, ctl);
    case "organize":
      return organizeWorker(job, spec, ctl);
    case "undo":
      return undoWorker(spec.batch_id, false);
    case "redo":
      return undoWorker(null, true);
  }
}

// ---------------------------------------------------------------------------
// Commands (src-tauri/src/commands/planner.rs)
// ---------------------------------------------------------------------------

/** Small batches are reverted inline (Ctrl+Z feel); large ones run as a job (`planner_bridge::undo`). */
function undoCommand(batchId: string | null, redo: boolean): UndoResponse {
  const batch = redo ? lastRedoable() : batchId ? (listBatches(500).find((b) => b.id === batchId) ?? null) : lastUndoable();
  if (!batch) throw err("invalid", redo ? "nothing_to_redo" : "nothing_to_undo");
  if (batch.entries <= INLINE_UNDO_ENTRIES) {
    const out = undoWorker(batch.id, false);
    return { job_id: null, report: (out.result as UndoReport | undefined) ?? null };
  }
  const id = submitJob(redo ? { kind: "redo" } : { kind: "undo", batch_id: batch.id }, `${batch.entries} entries`, "high");
  return { job_id: id, report: null };
}

export async function mockPlanner(cmd: string, a: Record<string, any>): Promise<unknown> { // eslint-disable-line @typescript-eslint/no-explicit-any
  switch (cmd) {
    case "planner_plan": {
      const text = String(a.text ?? "");
      if (!text.trim()) throw err("invalid", "text_empty");
      const ctx = buildContext((a.context as UiPlanContext | null) ?? {});
      const result = planText(text, ctx);
      // Model calls go through the engine gate: a running batch yields at its next page boundary.
      if (result.source === "model") await gateInteractive(() => sleep(tick(400)));
      const preview = isClarification(result.plan) ? null : planPreview(result.plan, ctx);
      return { result, preview, context: ctx } satisfies PlanResponse;
    }
    case "planner_execute": {
      const args = (a.args ?? {}) as { plan?: Plan; text?: string; context?: UiPlanContext | null };
      if (!args.plan) throw err("plan_invalid", "the plan is missing", { issues: [] });
      const plan = structuredClone(args.plan);
      const text = args.text ?? "";
      const ctx = buildContext(args.context ?? {});
      const issues = validatePlan(plan, ctx, resolveRelativeDate(text, new Date(`${ctx.today}T12:00:00`)));
      if (hasErrors(issues)) throw err("plan_invalid", issues.find((i) => i.blocking)?.message ?? "", { issues });
      if (isClarification(plan)) throw err("invalid", "plan_clarify");
      if (inlineOnly(plan)) {
        const execution = await runPlan(plan, ctx, null, () => undefined);
        afterExecution(execution);
        return { job_id: null, execution } satisfies ExecuteResponse;
      }
      const id = submitJob({ kind: "plan", plan, text, context: ctx }, plan.summary ?? text, "normal");
      return { job_id: id, execution: null } satisfies ExecuteResponse;
    }
    case "organize_plan":
      return organizePlan((a.req ?? {}) as OrganizeRequest);
    case "organize_apply": {
      const plan = a.plan as OrganizePlan;
      if (!plan || plan.moves === 0) throw err("invalid", "organize_nothing");
      const label = (a.label as string | null) ?? `정리 ${plan.moves}건`;
      const batchKind = plan.name_template != null && plan.folder_template == null ? "rename" : plan.name_template == null && plan.folder_template != null ? "move" : "organize";
      return submitJob({ kind: "organize", plan, label, source: "organize", batch_kind: batchKind }, `${plan.moves} files`, "high");
    }
    case "journal_batches":
      return listBatches(Math.min(500, Math.max(1, Number(a.limit ?? 50) || 50)));
    case "journal_entries":
      return batchEntries(String(a.batchId));
    case "journal_undo":
      return undoCommand((a.batchId as string | null) ?? null, false);
    case "journal_redo":
      return undoCommand(null, true);
    case "aggregate_run":
      return aggregate((a.req ?? { func: "count" }) as AggregateRequest);
    case "export_preview":
      return exportPreview(a.req as ExportRequest);
    case "export_run": {
      const req = a.req as ExportRequest;
      const preview = exportPreview(req);
      if (preview.exists && !req.overwrite) throw err("invalid", "export_exists");
      return submitJob({ kind: "export", req }, preview.path, "high");
    }
    case "jobs_get":
      return getJob(String(a.jobId));
    case "jobs_pause":
      return pauseJob(String(a.jobId));
    case "jobs_resume":
      return resumeJob(String(a.jobId));
    case "jobs_retry":
      return retryJob(String(a.jobId));
    case "jobs_prioritize":
      return prioritizeJob(String(a.jobId));
    default:
      throw err("internal", `mock planner: unknown command ${cmd}`);
  }
}

/** `docView` re-export for callers that render the documents of a result (keeps the import used in every build). */
export { docView };
