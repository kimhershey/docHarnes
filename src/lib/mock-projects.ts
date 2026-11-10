// Browser mock of the P6 project commands (design §3.15): projects with
// their documents, templates (built-in + user), natural-language → rule JSON
// (a small phrase parser standing in for the phrase rules + grammar-bound
// model), deterministic consistency checks over the latest extracted fields,
// run history, the relation graph, the Markdown report, the 보완요청서 draft
// job, and the P7 maintenance (retention purge) job. Mirrors
// crates/doculens-core/src/projects; the Rust code is the source of truth.

import {
  currentWorkspacePath,
  currentWorkspaceView,
  emit,
  err,
  hasActiveJob,
  listJobs,
  loadSettings,
  pace,
  submitJob,
  updateJob,
  workerFailure,
  wsDocs,
  type JobControl,
  type MockDoc,
  type WorkerOutcome,
} from "./mock-backend";
import { buildSupplementDraft, submitDraftJob } from "./mock-generate";
import * as pl from "./mock-pipeline";
import {
  EVENTS,
  type CheckRun,
  type ChecklistItem,
  type ClassifyApplyResult,
  type ClassifyAssignment,
  type ClassifyCandidate,
  type ClassifyRequest,
  type ClassifyResult,
  type ClassifySuggestion,
  type Draft,
  type DraftRequest,
  type Evidence,
  type FieldRecord,
  type Job,
  type MaintenanceStatus,
  type MaintenanceSummary,
  type OutputFormat,
  type Project,
  type ProjectDetail,
  type ProjectInput,
  type ProjectStatus,
  type ProjectSummary,
  type ProjectTemplate,
  type ProjectsChanged,
  type RelationEdge,
  type RelationGraph,
  type RelationNode,
  type Rule,
  type RuleDraft,
  type RuleOp,
  type RuleOperand,
  type RuleResult,
  type SchemaRecord,
  type TemplateInput,
  type Verdict,
} from "./types";
import { docTypeAliases } from "./doc-type-aliases";

const KEY = "doculens.mock.projects";
const MAINT_KEY = "doculens.mock.maintenance";

interface PState {
  projects: Project[];
  members: { project_id: string; doc_id: string; added_at: string }[];
  rules: { project_id: string; rule: Rule }[];
  runs: CheckRun[];
  templates: ProjectTemplate[];
}

function now(): string {
  return new Date().toISOString();
}

function newId(): string {
  return crypto.randomUUID().replace(/-/g, "");
}

export function compact(s: string): string {
  return s.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
}

function ko(): boolean {
  return loadSettings().ui.language !== "en";
}

function loadAll(): Record<string, PState> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "{}") as Record<string, PState>;
  } catch {
    return {};
  }
}

function load(): PState {
  const ws = currentWorkspacePath() ?? "(none)";
  return loadAll()[ws] ?? { projects: [], members: [], rules: [], runs: [], templates: [] };
}

function save(s: PState): void {
  const ws = currentWorkspacePath() ?? "(none)";
  const all = loadAll();
  all[ws] = s;
  localStorage.setItem(KEY, JSON.stringify(all));
}

function changed(ids: string[]): void {
  emit(EVENTS.projectsChanged, { project_ids: ids } satisfies ProjectsChanged);
}

// ---------------------------------------------------------------------------
// Built-in templates (projects/repo.rs `builtin_templates`)
// ---------------------------------------------------------------------------

function rule(description: string, left: RuleOperand, op: RuleOp, right: RuleOperand | null, tolerance: number | null = null, required = true): Rule {
  return { id: `builtin-${compact(description).slice(0, 24)}-${op}`, description, left, op, right, tolerance, required, enabled: true };
}

const fieldRef = (doc_type: string, field: string): RuleOperand => ({ doc_type, field, value: null });
const EPOCH = "2026-01-01T00:00:00Z";

const BUILTIN_TEMPLATES: ProjectTemplate[] = [
  {
    id: "builtin-outsourcing",
    name_ko: "외주 개발 사업",
    name_en: "Outsourced development",
    description_ko: "제안요청서·제안서·개발 계약서·과업지시서·요구사항 정의서·WBS·테스트 결과서·검수확인서. 계약금액이 제안 금액과 같고 예산을 넘지 않는지, 계약서·검수확인서가 서명됐는지 검사합니다.",
    description_en: "RFP, proposal, development contract, SOW, requirements, WBS, test report and acceptance certificate; checks amounts against the proposal and the budget and the signatures.",
    required_doc_types: ["rfp", "proposal", "dev_contract", "sow", "srs", "wbs", "test_report", "acceptance"],
    rules: [
      rule("개발 계약서의 계약금액은 제안서의 제안 금액과 같아야 한다", fieldRef("dev_contract", "amount"), "eq", fieldRef("proposal", "price")),
      rule("개발 계약서의 계약금액은 제안요청서의 사업 예산 이하여야 한다", fieldRef("dev_contract", "amount"), "lte", fieldRef("rfp", "budget"), null, false),
      rule("과업지시서의 과업 금액은 계약금액과 5% 이내여야 한다", fieldRef("sow", "amount"), "within_pct", fieldRef("dev_contract", "amount"), 5, false),
      rule("개발 계약서 서명 필수", fieldRef("dev_contract", "signed"), "present", null),
      rule("검수확인서 서명 필수", fieldRef("acceptance", "signed"), "present", null),
    ],
    builtin: true,
    created_at: EPOCH,
    updated_at: EPOCH,
  },
  {
    id: "builtin-maintenance",
    name_ko: "유지보수",
    name_en: "Maintenance",
    description_ko: "개발 계약서·변경요청서·진척 보고서. 계약서 서명, 변경요청서의 승인 상태, 진척 보고서의 전체 진척률이 있는지 검사합니다.",
    description_en: "Development contract, change requests and progress reports; checks the signature, the approval status and the progress figure.",
    required_doc_types: ["dev_contract", "change_request", "progress_report"],
    rules: [
      rule("개발 계약서 서명 필수", fieldRef("dev_contract", "signed"), "present", null),
      rule("변경요청서 승인 상태 필수", fieldRef("change_request", "status"), "present", null),
      rule("진척 보고서 전체 진척률 필수", fieldRef("progress_report", "progress_percent"), "present", null, null, false),
    ],
    builtin: true,
    created_at: EPOCH,
    updated_at: EPOCH,
  },
  {
    id: "builtin-internal_dev",
    name_ko: "내부 개발",
    name_en: "In-house development",
    description_ko: "요구사항 정의서·WBS·테스트 결과서. 요구사항 정의서 버전, WBS와 요구사항 정의서의 사업명 일치, 테스트 통과율이 있는지 검사합니다.",
    description_en: "Requirements specification, WBS and test report; checks the requirements version, matching project names and the pass rate.",
    required_doc_types: ["srs", "wbs", "test_report"],
    rules: [
      rule("요구사항 정의서 버전 필수", fieldRef("srs", "version"), "present", null),
      rule("WBS의 사업명은 요구사항 정의서의 사업명과 같아야 한다", fieldRef("wbs", "project_name"), "eq", fieldRef("srs", "project_name"), null, false),
      rule("테스트 결과서 통과율 필수", fieldRef("test_report", "pass_rate"), "present", null, null, false),
    ],
    builtin: true,
    created_at: EPOCH,
    updated_at: EPOCH,
  },
];

export function templates(s: PState = load()): ProjectTemplate[] {
  return [...BUILTIN_TEMPLATES, ...s.templates];
}

function templateOf(s: PState, id: string | null): ProjectTemplate | null {
  if (!id) return null;
  return templates(s).find((t) => t.id === id) ?? null;
}

export function resolveTemplateRef(ref: string | null | undefined, s: PState = load()): ProjectTemplate | null {
  if (!ref) return null;
  const key = compact(ref);
  return templates(s).find((t) => t.id === ref || compact(t.name_ko) === key || compact(t.name_en) === key || compact(t.id.replace(/^builtin-/, "")) === key) ?? templates(s).find((t) => compact(t.name_ko).includes(key) || compact(t.name_en).includes(key)) ?? null;
}

// ---------------------------------------------------------------------------
// Projects (projects/repo.rs)
// ---------------------------------------------------------------------------

function docsOf(s: PState, projectId: string): MockDoc[] {
  const ids = new Set(s.members.filter((m) => m.project_id === projectId).map((m) => m.doc_id));
  return wsDocs().filter((d) => ids.has(d.id));
}

function docTypeLabel(slug: string): { ko: string; en: string } {
  const info = pl.docTypeList().find((d) => d.slug === slug);
  return info ? { ko: info.label_ko, en: info.label_en } : { ko: slug, en: slug };
}

function checklist(s: PState, p: Project): ChecklistItem[] {
  const docs = docsOf(s, p.id);
  const tpl = templateOf(s, p.template_id);
  const required = tpl?.required_doc_types ?? [];
  const present = new Map<string, string[]>();
  for (const d of docs) {
    const t = d.doc_type ?? "other";
    present.set(t, [...(present.get(t) ?? []), d.id]);
  }
  const out: ChecklistItem[] = required.map((slug) => {
    const l = docTypeLabel(slug);
    const ids = present.get(slug) ?? [];
    return { doc_type: slug, label_ko: l.ko, label_en: l.en, required: true, doc_ids: ids, missing: ids.length === 0 };
  });
  for (const [slug, ids] of present) {
    if (required.includes(slug)) continue;
    const l = docTypeLabel(slug);
    out.push({ doc_type: slug, label_ko: l.ko, label_en: l.en, required: false, doc_ids: ids, missing: false });
  }
  return out;
}

/** Recomputes the counters the list / kanban cards show. */
function refresh(s: PState, p: Project): Project {
  const docs = docsOf(s, p.id);
  p.document_count = docs.length;
  p.missing_count = checklist(s, p).filter((c) => c.missing).length;
  const last = lastRun(s, p.id);
  p.last_check_at = last?.finished_at ?? null;
  p.last_pass = last?.pass ?? 0;
  p.last_mismatch = last?.mismatch ?? 0;
  p.last_undetermined = last?.undetermined ?? 0;
  return p;
}

function lastRun(s: PState, projectId: string): CheckRun | null {
  return [...s.runs].filter((r) => r.project_id === projectId).sort((a, b) => b.finished_at.localeCompare(a.finished_at))[0] ?? null;
}

function getProject(s: PState, id: string): Project {
  const p = s.projects.find((x) => x.id === id);
  if (!p) throw err("not_found", "project_not_found");
  return p;
}

export function resolveProjectRef(ref: string | null | undefined, current: string | null, s: PState = load()): Project | null {
  if (ref) {
    const key = compact(ref);
    const p = s.projects.find((x) => x.id === ref) ?? s.projects.find((x) => compact(x.name) === key) ?? s.projects.find((x) => compact(x.name).includes(key) || compact(x.intake_no ?? "") === key) ?? null;
    if (p) return refresh(s, p);
  }
  if (current) {
    const p = s.projects.find((x) => x.id === current);
    if (p) return refresh(s, p);
  }
  return null;
}

export function projectSummaries(): ProjectSummary[] {
  return load().projects.map((p) => ({ id: p.id, name: p.name, status: p.status }));
}

function normalizeKeywords(keywords: string[]): string[] {
  const out: string[] = [];
  for (const k of keywords) {
    const t = k.trim().normalize("NFC");
    if (!t || out.some((x) => x.toLowerCase() === t.toLowerCase())) continue;
    out.push(t);
    if (out.length >= 32) break;
  }
  return out;
}

function upsertProject(s: PState, input: ProjectInput): Project {
  const name = (input.name ?? "").trim();
  if (!name) throw err("invalid", "project_name_empty");
  if (input.template_id && !templateOf(s, input.template_id)) throw err("invalid", "template_not_found");
  const clean = (v: string | null | undefined) => {
    const t = (v ?? "").trim();
    return t ? t : null;
  };
  let p: Project;
  // Same contract as core `save_project`: a null / omitted optional field keeps the stored value, a blank one clears it.
  const merged = (given: string | null | undefined, stored: string | null) => (given == null ? stored : clean(given));
  if (input.id) {
    p = getProject(s, input.id);
    p.name = name;
    p.template_id = merged(input.template_id, p.template_id);
    if (input.status) p.status = input.status;
    p.intake_no = merged(input.intake_no, p.intake_no);
    p.owner = merged(input.owner, p.owner);
    p.due_date = merged(input.due_date, p.due_date);
    p.notes = merged(input.notes, p.notes);
    p.inbox_folder = merged(input.inbox_folder, p.inbox_folder);
    if (input.keywords != null) p.keywords = normalizeKeywords(input.keywords);
    p.updated_at = now();
  } else {
    if (s.projects.some((x) => compact(x.name) === compact(name))) throw err("invalid", "project_name_taken");
    p = {
      id: newId(),
      name,
      template_id: clean(input.template_id),
      status: input.status ?? "intake",
      intake_no: clean(input.intake_no),
      owner: clean(input.owner),
      due_date: clean(input.due_date),
      notes: clean(input.notes),
      inbox_folder: clean(input.inbox_folder),
      keywords: normalizeKeywords(input.keywords ?? []),
      created_at: now(),
      updated_at: now(),
      document_count: 0,
      missing_count: 0,
      last_check_at: null,
      last_pass: 0,
      last_mismatch: 0,
      last_undetermined: 0,
    };
    s.projects.push(p);
    // The template's rules are copied into the project so they can be edited per case.
    const tpl = templateOf(s, p.template_id);
    for (const r of tpl?.rules ?? []) s.rules.push({ project_id: p.id, rule: { ...structuredClone(r), id: newId() } });
  }
  refresh(s, p);
  return p;
}

/** `project_get` as a plain call (plan previews). */
export function projectDetail(id: string): ProjectDetail | null {
  const s = load();
  const p = s.projects.find((x) => x.id === id);
  return p ? detail(s, p) : null;
}

export function createProject(input: ProjectInput): Project {
  const s = load();
  const p = upsertProject(s, { ...input, id: null });
  save(s);
  changed([p.id]);
  return p;
}

export function addDocuments(projectId: string, docIds: string[]): ProjectDetail {
  const s = load();
  const p = getProject(s, projectId);
  const known = new Set(wsDocs().map((d) => d.id));
  for (const id of docIds) {
    if (!known.has(id)) continue;
    if (s.members.some((m) => m.project_id === projectId && m.doc_id === id)) continue;
    s.members.push({ project_id: projectId, doc_id: id, added_at: now() });
  }
  p.updated_at = now();
  refresh(s, p);
  save(s);
  changed([p.id]);
  return detail(s, p);
}

function detail(s: PState, p: Project): ProjectDetail {
  refresh(s, p);
  const docs = docsOf(s, p.id);
  const added = new Map(s.members.filter((m) => m.project_id === p.id).map((m) => [m.doc_id, m.added_at]));
  return {
    project: { ...p },
    template: templateOf(s, p.template_id),
    documents: docs.map((d) => {
      const { workspace: _ws, pages: _pages, ...record } = d;
      return { document: record, added_at: added.get(d.id) ?? d.imported_at };
    }),
    checklist: checklist(s, p),
    rules: s.rules.filter((r) => r.project_id === p.id).map((r) => r.rule),
    last_run: lastRun(s, p.id),
    relations: relations(s, p),
  };
}

// ---------------------------------------------------------------------------
// Natural-language rules (projects/rules.rs)
// ---------------------------------------------------------------------------

interface FieldPath {
  path: string;
  labels: string[];
  kind: string;
}

interface TypeInfo {
  slug: string;
  labels: string[];
  fields: FieldPath[];
}

const FIELD_ALIASES: Record<string, string[]> = {
  사업자번호: ["사업자등록번호", "등록번호"],
  사업자등록번호: ["등록번호", "사업자번호"],
  등록번호: ["사업자등록번호", "registration_no"],
  대표: ["대표자"],
  대표자명: ["대표자"],
  상호명: ["상호"],
  회사명: ["상호", "법인명"],
  법인명: ["상호"],
  성명: ["이름", "신청인 성명", "name"],
  이름: ["성명", "name"],
  신청인: ["신청인 성명"],
  계약금액: ["계약 금액", "금액"],
  합계: ["합계금액", "합계 금액", "total", "grand_total"],
  총액: ["합계금액", "합계"],
  합계금액: ["합계", "grand_total"],
  공급가액: ["공급가액 합계"],
  세액: ["세액 합계"],
  서명: ["서명/날인 여부", "서명 여부", "signed"],
  날인: ["서명/날인 여부"],
  기간: ["계약기간"],
  시작일: ["계약기간 시작"],
  종료일: ["계약기간 종료"],
};

function typeInfos(): TypeInfo[] {
  const out: TypeInfo[] = [];
  const schemas = pl.listSchemas().filter((s) => s.enabled);
  const byType = new Map<string, SchemaRecord[]>();
  for (const s of schemas) byType.set(s.doc_type, [...(byType.get(s.doc_type) ?? []), s]);
  for (const [slug, list] of byType) {
    const l = docTypeLabel(slug);
    const labels = [slug, l.ko, l.en, ...docTypeAliases(slug), ...list.flatMap((s) => [s.name, s.name_en ?? ""])].filter(Boolean);
    const fields: FieldPath[] = [];
    for (const s of list) {
      for (const f of s.definition.fields) {
        if (f.kind === "table") continue;
        if (f.kind === "group") {
          for (const c of f.fields) fields.push({ path: `${f.key}.${c.key}`, labels: [`${f.label_ko} ${c.label_ko}`, c.label_ko, `${f.label_en} ${c.label_en}`, c.label_en, `${f.key}.${c.key}`, c.key].filter(Boolean), kind: c.kind });
        } else fields.push({ path: f.key, labels: [f.label_ko, f.label_en, f.key].filter(Boolean), kind: f.kind });
      }
    }
    out.push({ slug, labels, fields });
  }
  return out;
}

/** A token plus its aliases ("사업자번호" → "사업자등록번호"), compacted. */
function tokenForms(token: string): string[] {
  return [...new Set([token, ...(FIELD_ALIASES[token] ?? [])].map(compact).filter(Boolean))];
}

/**
 * Resolves a field phrase against a type's fields: an exact label match (aliases included) first,
 * else the field whose labels cover most of the phrase's words, the head noun being mandatory
 * ("공급자 사업자번호" → supplier.brn rather than buyer.brn).
 */
function matchField(type: TypeInfo, text: string): FieldPath | null {
  const phrase = text.trim();
  const key = compact(phrase);
  if (!key) return null;
  const whole = tokenForms(phrase);
  for (const c of whole) {
    const exact = type.fields.find((f) => f.labels.some((l) => compact(l) === c));
    if (exact) return exact;
  }
  // Every word of the phrase (particles stripped, aliases applied) must occur in the field's labels.
  const tokens = phrase
    .split(/\s+/)
    .filter(Boolean)
    .map((tk) => (Array.from(tk).length > 2 ? tk.replace(/(의|은|는|이|가|을|를)$/u, "") : tk));
  const head = tokens[tokens.length - 1] ?? phrase;
  let best: { field: FieldPath; length: number } | null = null;
  for (const f of type.fields) {
    const labels = f.labels.map(compact).filter((l) => l.length >= 2);
    const headHit = tokenForms(head).some((h) => h.length >= 2 && labels.some((l) => l.includes(h) || (h.length >= 3 && h.includes(l))));
    if (!headHit) continue;
    const all = tokens.every((tk) => tokenForms(tk).some((form) => form.length >= 2 && labels.some((l) => l.includes(form))));
    if (!all) continue;
    const length = Math.min(...labels.map((l) => l.length));
    if (!best || length < best.length) best = { field: f, length };
  }
  return best?.field ?? null;
}

interface Mention {
  type: TypeInfo;
  start: number;
  end: number;
}

function findTypeMentions(text: string, types: TypeInfo[]): Mention[] {
  const mentions: Mention[] = [];
  for (const type of types) {
    for (const label of type.labels) {
      if (label.length < 2) continue;
      let from = 0;
      for (;;) {
        const at = text.indexOf(label, from);
        if (at < 0) break;
        from = at + label.length;
        // A short alias glued to more letters is part of another word ("계약" in "계약금액", "견적" in "견적일").
        if (Array.from(label).length <= 2 && /^[\p{L}\p{N}]/u.test(text.slice(from))) continue;
        if (!mentions.some((m) => at < m.end && at + label.length > m.start)) mentions.push({ type, start: at, end: at + label.length });
      }
    }
  }
  // Longer labels win over shorter overlapping ones ("세금계산서" over "계산서").
  mentions.sort((a, b) => a.start - b.start || b.end - a.end);
  const out: Mention[] = [];
  for (const m of mentions) if (!out.some((o) => m.start < o.end && m.end > o.start)) out.push(m);
  return out;
}

// A particle only when a space or comma follows it: "과업지시서의 과업 금액" keeps the 과 of 과업, "…의 이름" keeps 이름.
const PARTICLE = /^(?:(?:의|에서|에|이|가|은|는|을|를|와|과|랑|이랑|하고|도)(?=[\s,])|,|\s)+/u;
/** Where a field phrase ends: a particle at a word boundary, `=`, or a comparison keyword after a space. */
const FIELD_END = /(?:(?:이|가|은|는|을|를|와|과|랑|이랑|하고|도)(?=[\s,]|$)|\s*=|\s+(?:같|일치|동일|달라|다르|필수|있어|존재|비어|이상|이하|이내|오차|형식|패턴|match|equal|must|should|differ|present|required|within|at\s+least|at\s+most)|,|$)/u;

/** The field text that follows a type mention ("세금계산서의 공급자 사업자번호는" → "공급자 사업자번호"). */
function fieldAfter(text: string, m: Mention): string {
  let rest = text.slice(m.end).replace(PARTICLE, "");
  const end = rest.search(FIELD_END);
  if (end >= 0) rest = rest.slice(0, end);
  return rest.trim();
}

function opOf(text: string): { op: RuleOp; tolerance: number | null; literal: string | null } {
  const pct = text.match(/(\d+(?:\.\d+)?)\s*%\s*(?:이내|안|오차|차이|이하|within|tolerance)?/u);
  if (pct && /(이내|오차|차이|within|tolerance|안에)/u.test(text)) return { op: "within_pct", tolerance: Number(pct[1]), literal: null };
  if (/(달라야|다르게|다르고|같지\s*않|상이해야|differ|not\s+equal|!=|≠)/iu.test(text)) return { op: "neq", tolerance: null, literal: null };
  const regex = text.match(/\/(.+)\/|정규식\s*([^\s]+)|패턴\s*([^\s]+)/u);
  if (regex && /(형식|패턴|정규식|match|format)/iu.test(text)) return { op: "matches", tolerance: null, literal: regex[1] ?? regex[2] ?? regex[3] ?? null };
  const num = text.match(/([\d,]+(?:\.\d+)?)\s*(원|%|일|개|건)?\s*(이상|넘어야|크거나|at least|>=|이하|넘지|작거나|at most|<=)/iu);
  if (num) {
    const n = num[1]!.replace(/,/g, "");
    return /(이상|넘어야|크거나|at least|>=)/iu.test(num[3]!) ? { op: "gte", tolerance: null, literal: n } : { op: "lte", tolerance: null, literal: n };
  }
  if (/(필수|있어야|존재해야|비어\s*있으면\s*안|비어있으면\s*안|누락되면\s*안|required|present|must\s+have|must\s+exist)/iu.test(text)) return { op: "present", tolerance: null, literal: null };
  return { op: "eq", tolerance: null, literal: null };
}

/**
 * Natural language → rule JSON (`projects::define_rule`): "세금계산서의 공급자 사업자번호는 사업자등록증 번호와
 * 같아야 해" → left tax_invoice.supplier.brn, eq, right business_registration.registration_no.
 */
export function parseRule(description: string, knownTypes: TypeInfo[] = typeInfos()): RuleDraft {
  const isKo = ko();
  const text = description.replace(/\s+/g, " ").trim();
  if (!text) throw err("invalid", "description_empty");
  const { op, tolerance, literal: lit } = opOf(text);
  const mentions = findTypeMentions(text, knownTypes);
  const unresolved: string[] = [];
  const issues: string[] = [];
  const operand = (m: Mention | undefined, fallbackText: string | null): RuleOperand | null => {
    if (m) {
      const fieldText = fieldAfter(text, m);
      const f = fieldText ? matchField(m.type, fieldText) : null;
      if (!f) {
        if (fieldText) unresolved.push(fieldText);
        else unresolved.push(isKo ? `${m.type.labels[1] ?? m.type.slug}의 필드` : `field of ${m.type.slug}`);
        return { doc_type: m.type.slug, field: fieldText || null, value: null };
      }
      return { doc_type: m.type.slug, field: f.path, value: null };
    }
    if (fallbackText) {
      // No document type named: look the field label up across every schema ("계약금액", "견적 합계").
      const words = fallbackText.replace(/[=:]/g, " ").split(/\s+/).filter(Boolean);
      for (let n = Math.min(3, words.length); n >= 1; n -= 1) {
        for (let i = 0; i + n <= words.length; i += 1) {
          const phrase = words.slice(i, i + n).join(" ");
          for (const type of knownTypes) {
            const f = matchField(type, phrase);
            if (f) return { doc_type: type.slug, field: f.path, value: null };
          }
        }
      }
      if (fallbackText.trim()) unresolved.push(fallbackText.trim());
    }
    return null;
  };
  let left: RuleOperand | null;
  let right: RuleOperand | null = null;
  if (mentions.length >= 2) {
    left = operand(mentions[0], null);
    right = op === "present" ? null : operand(mentions[1], null);
  } else if (mentions.length === 1) {
    left = operand(mentions[0], null);
    if (op !== "present" && op !== "gte" && op !== "lte" && op !== "matches") {
      const m = mentions[0]!;
      const after = text.slice(m.end);
      const second = after.match(/(?:와|과|랑|하고|and|=)\s*([^=]+?)\s*(?:같|일치|동일|달라|다르|이내|오차|equal|match|differ|within|$)/u);
      right = second ? operand(undefined, second[1]!) : null;
    }
  } else {
    // "계약금액 = 견적 합계": two field phrases around a comparison word.
    const parts = text.split(/\s*(?:=|와|과|랑|하고|은|는)\s*/u).filter(Boolean);
    left = operand(undefined, parts[0] ?? text);
    right = op === "present" || op === "gte" || op === "lte" || op === "matches" ? null : operand(undefined, parts.slice(1).join(" ").replace(/(같아야|일치해야|동일해야|달라야).*$/u, "").trim() || null);
  }
  if (!left) {
    left = { doc_type: null, field: null, value: null };
    issues.push(isKo ? "왼쪽 비교 대상(문서 유형·필드)을 찾지 못했습니다." : "Could not resolve the left operand (document type / field).");
  }
  if (lit !== null) right = { doc_type: null, field: null, value: lit };
  if (right === null && op !== "present") issues.push(isKo ? "비교할 오른쪽 대상(다른 문서의 필드 또는 값)을 찾지 못했습니다." : "Could not resolve the right operand (another document's field or a literal).");
  if (op === "within_pct" && tolerance === null) issues.push(isKo ? "허용 오차(%)가 필요합니다." : "A tolerance (%) is required.");
  for (const u of unresolved) issues.push(isKo ? `'${u}'을(를) 알려진 문서 유형·필드에서 찾지 못했습니다.` : `'${u}' is not a known document type / field.`);
  const resolvedLeft = !!left.doc_type && !!left.field && !unresolved.includes(left.field);
  const resolvedRight = op === "present" || (right !== null && (right.value !== null || (!!right.doc_type && !!right.field && !unresolved.includes(right.field ?? ""))));
  const confidence = resolvedLeft && resolvedRight ? (mentions.length >= 1 ? 0.92 : 0.8) : resolvedLeft || resolvedRight ? 0.55 : 0.3;
  return {
    rule: { id: newId(), description: text, left, op, right, tolerance, required: !/(선택|권장|optional|warn)/iu.test(text), enabled: true },
    confidence,
    source: "rules",
    issues,
    unresolved: [...new Set(unresolved)],
  };
}

export function saveRule(projectId: string, input: Rule): Rule {
  const s = load();
  getProject(s, projectId);
  const r: Rule = { ...input, id: input.id || newId(), description: input.description.trim() };
  if (!r.left.doc_type || !r.left.field) throw err("invalid", "rule_left_missing");
  if (r.op !== "present" && (!r.right || (!r.right.value && !(r.right.doc_type && r.right.field)))) throw err("invalid", "rule_right_missing");
  const existing = s.rules.find((x) => x.project_id === projectId && x.rule.id === r.id);
  if (existing) existing.rule = r;
  else s.rules.push({ project_id: projectId, rule: r });
  const p = getProject(s, projectId);
  p.updated_at = now();
  save(s);
  changed([projectId]);
  return r;
}

// ---------------------------------------------------------------------------
// Consistency check (projects/check.rs)
// ---------------------------------------------------------------------------

function fieldOf(doc: MockDoc, path: string): FieldRecord | null {
  const fields = pl.latestFields(doc.id);
  return fields.find((f) => f.path === path) ?? fields.find((f) => f.key === path && f.row === null) ?? fields.find((f) => f.path.endsWith(`.${path}`) && f.row === null) ?? null;
}

function evidence(doc: MockDoc, f: FieldRecord | null, field: string): Evidence {
  const pending = f?.review_reason !== null && f !== null && !["approved", "corrected", "rejected"].includes(f.status);
  return {
    doc_id: doc.id,
    doc_name: doc.title ?? doc.original_name,
    doc_type: doc.doc_type,
    field,
    label: f ? f.label_ko || f.label_en : null,
    value: f?.value ?? null,
    normalized: f?.normalized ?? null,
    page: f?.page ?? null,
    status: f ? (pending ? "pending" : f.status) : null,
  };
}

function numeric(e: Evidence | null): number | null {
  if (!e) return null;
  const v = e.normalized ?? e.value;
  if (v === null) return null;
  const n = Number(String(v).replace(/[^0-9.-]/g, ""));
  return Number.isFinite(n) && /\d/.test(String(v)) ? n : null;
}

function comparable(e: Evidence | null, kind: string | null): string | null {
  if (!e) return null;
  const v = e.normalized ?? e.value;
  if (v === null || v === "") return null;
  if (kind === "brn" || kind === "account" || kind === "phone") return v.replace(/\D/g, "");
  if (kind === "amount" || kind === "integer") {
    const n = numeric(e);
    return n === null ? null : String(n);
  }
  return compact(v);
}

function kindOf(e: Evidence | null, f: FieldRecord | null, fallback: FieldRecord | null): string | null {
  return f?.kind ?? fallback?.kind ?? (e?.value && /^\d{3}-\d{2}-\d{5}$/.test(e.value) ? "brn" : null);
}

function evaluate(r: Rule, docs: MockDoc[], isKo: boolean): RuleResult {
  const base: RuleResult = { id: newId(), run_id: "", rule_id: r.id, description: r.description, verdict: "undetermined", required: r.required, left: null, right: null, message: "", checked_at: now() };
  const typeLabel = (slug: string | null) => (slug ? (isKo ? docTypeLabel(slug).ko : docTypeLabel(slug).en) : "?");
  const side = (o: RuleOperand | null): { ev: Evidence | null; field: FieldRecord | null; verdict: Verdict | null; literal: string | null } => {
    if (!o) return { ev: null, field: null, verdict: null, literal: null };
    if (!o.doc_type && !o.field) return { ev: { doc_id: null, doc_name: null, doc_type: null, field: null, label: null, value: o.value, normalized: o.value, page: null, status: null }, field: null, verdict: null, literal: o.value };
    const candidates = docs.filter((d) => d.doc_type === o.doc_type);
    if (candidates.length === 0) return { ev: { doc_id: null, doc_name: null, doc_type: o.doc_type, field: o.field, label: null, value: null, normalized: null, page: null, status: null }, field: null, verdict: "missing", literal: null };
    const path = o.field ?? "";
    let found: { d: MockDoc; f: FieldRecord | null } | null = null;
    for (const d of candidates) {
      const f = fieldOf(d, path);
      if (f && f.value) {
        found = { d, f };
        break;
      }
      if (!found) found = { d, f };
    }
    const d = found!.d;
    const f = found!.f;
    return { ev: evidence(d, f, path), field: f, verdict: f && f.value ? null : "undetermined", literal: null };
  };
  const L = side(r.left);
  const R = side(r.right);
  base.left = L.ev;
  base.right = R.ev;
  if (L.verdict === "missing" || R.verdict === "missing") {
    base.verdict = "missing";
    const missing = L.verdict === "missing" ? r.left.doc_type : r.right?.doc_type ?? null;
    base.message = isKo ? `${typeLabel(missing)} 문서가 프로젝트에 없습니다.` : `No ${typeLabel(missing)} document in the project.`;
    return base;
  }
  if (r.op === "present") {
    base.verdict = L.ev?.value ? "pass" : "mismatch";
    base.message = base.verdict === "pass" ? (isKo ? `${L.ev?.label ?? r.left.field} 값이 있습니다: ${L.ev?.value}` : `${L.ev?.label ?? r.left.field} is present: ${L.ev?.value}`) : isKo ? `${typeLabel(r.left.doc_type)}의 ${L.ev?.label ?? r.left.field} 값이 비어 있습니다.` : `${typeLabel(r.left.doc_type)} ${L.ev?.label ?? r.left.field} is empty.`;
    return base;
  }
  if (L.verdict === "undetermined" || (R.verdict === "undetermined" && !R.literal)) {
    base.verdict = "undetermined";
    const which = L.verdict === "undetermined" ? `${typeLabel(r.left.doc_type)} ${L.ev?.label ?? r.left.field}` : `${typeLabel(r.right?.doc_type ?? null)} ${R.ev?.label ?? r.right?.field}`;
    base.message = isKo ? `${which} 값이 추출되지 않아 비교할 수 없습니다.` : `${which} has no extracted value to compare.`;
    return base;
  }
  const kind = kindOf(L.ev, L.field, R.field);
  const l = comparable(L.ev, kind);
  const rv = R.literal !== null ? (kind === "amount" || kind === "integer" ? String(Number(R.literal.replace(/[^0-9.-]/g, ""))) : kind === "brn" ? R.literal.replace(/\D/g, "") : compact(R.literal)) : comparable(R.ev, kind);
  const ln = numeric(L.ev);
  const rn = R.literal !== null ? Number(R.literal.replace(/[^0-9.-]/g, "")) : numeric(R.ev);
  const lText = L.ev?.value ?? "";
  const rText = R.literal ?? R.ev?.value ?? "";
  const label = `${L.ev?.label ?? r.left.field} ${lText}`;
  let pass: boolean | null = null;
  switch (r.op) {
    case "eq":
      if (l === null || rv === null) pass = null;
      else if (ln !== null && rn !== null && Number.isFinite(rn) && (kind === "amount" || kind === "integer")) pass = Math.abs(ln - rn) <= (r.tolerance ?? 0);
      else pass = l === rv;
      break;
    case "neq":
      pass = l !== null && rv !== null ? l !== rv : null;
      break;
    case "gte":
      pass = ln !== null && rn !== null ? ln >= rn : null;
      break;
    case "lte":
      pass = ln !== null && rn !== null ? ln <= rn : null;
      break;
    case "within_pct": {
      if (ln === null || rn === null || !Number.isFinite(rn)) pass = null;
      else pass = Math.abs(ln - rn) <= (Math.abs(rn) * (r.tolerance ?? 0)) / 100;
      break;
    }
    case "matches": {
      try {
        pass = lText ? new RegExp(R.literal ?? rText).test(lText) : null;
      } catch {
        pass = null;
      }
      break;
    }
    default:
      pass = null;
  }
  if (pass === null) {
    base.verdict = "undetermined";
    base.message = isKo ? `${label} 값을 비교할 수 없습니다 (${rText || "-"}).` : `${label} cannot be compared with ${rText || "-"}.`;
  } else if (pass) {
    base.verdict = "pass";
    base.message = r.op === "neq" ? (isKo ? `${label} ≠ ${rText}` : `${label} ≠ ${rText}`) : r.op === "within_pct" ? (isKo ? `${label} ≈ ${rText} (허용 ${r.tolerance ?? 0}%)` : `${label} ≈ ${rText} (±${r.tolerance ?? 0}%)`) : `${label} = ${rText}`;
  } else {
    base.verdict = "mismatch";
    const sym = r.op === "neq" ? "=" : r.op === "gte" ? "<" : r.op === "lte" ? ">" : "≠";
    base.message = isKo ? `${typeLabel(r.left.doc_type)} ${label} ${sym} ${R.ev?.doc_type ? `${typeLabel(R.ev.doc_type)} ` : ""}${R.ev?.label ? `${R.ev.label} ` : ""}${rText}` : `${typeLabel(r.left.doc_type)} ${label} ${sym} ${R.ev?.doc_type ? `${typeLabel(R.ev.doc_type)} ` : ""}${rText}`;
  }
  return base;
}

export function check(projectId: string, ruleIds: string[]): CheckRun {
  const s = load();
  const p = getProject(s, projectId);
  const isKo = ko();
  const docs = docsOf(s, p.id);
  const rules = s.rules.filter((r) => r.project_id === p.id && r.rule.enabled && (ruleIds.length === 0 || ruleIds.includes(r.rule.id))).map((r) => r.rule);
  const started = now();
  const run: CheckRun = { id: newId(), project_id: p.id, started_at: started, finished_at: started, pass: 0, mismatch: 0, undetermined: 0, missing: 0, results: [], missing_doc_types: checklist(s, p).filter((c) => c.missing).map((c) => c.doc_type) };
  for (const r of rules) {
    const res = evaluate(r, docs, isKo);
    res.run_id = run.id;
    run.results.push(res);
    run[res.verdict] += 1;
  }
  run.finished_at = now();
  s.runs.push(run);
  if (s.runs.length > 200) s.runs = s.runs.slice(-200);
  p.updated_at = now();
  if (p.status === "intake" || p.status === "extracting") p.status = "validating";
  refresh(s, p);
  save(s);
  changed([p.id]);
  return run;
}

// ---------------------------------------------------------------------------
// Relations (projects/relations.rs)
// ---------------------------------------------------------------------------

const NAME_KEYS = new Set(["name", "company_name", "store_name", "representative", "title", "recipient", "project_name", "supplier", "buyer"]);

function relations(s: PState, p: Project): RelationGraph {
  const docs = docsOf(s, p.id);
  const nodes: RelationNode[] = docs.map((d) => ({ doc_id: d.id, label: d.title ?? d.original_name, doc_type: d.doc_type }));
  const edges: RelationEdge[] = [];
  const scalars = docs.map((d) => ({ d, fields: pl.latestFields(d.id).filter((f) => f.row === null && f.value && f.value.trim()) }));
  for (let i = 0; i < scalars.length; i += 1) {
    for (let j = i + 1; j < scalars.length; j += 1) {
      const a = scalars[i]!;
      const b = scalars[j]!;
      const seen = new Set<string>();
      for (const fa of a.fields) {
        for (const fb of b.fields) {
          const kind = relationKind(fa, fb);
          if (!kind) continue;
          const va = relationValue(fa);
          const vb = relationValue(fb);
          if (!va || va !== vb) continue;
          const dedupe = `${kind}:${va}`;
          if (seen.has(dedupe)) continue;
          seen.add(dedupe);
          edges.push({ from: a.d.id, to: b.d.id, kind, value: fa.value!, from_field: fa.path, to_field: fb.path });
        }
      }
    }
  }
  return { nodes, edges };
}

function relationKind(a: FieldRecord, b: FieldRecord): string | null {
  if (a.kind === "brn" && b.kind === "brn") return "brn";
  if (a.kind === "account" && b.kind === "account") return "account";
  if (a.kind === "amount" && b.kind === "amount") return "amount";
  if (a.kind === "date" && b.kind === "date") return "date";
  if (/_no$|number$/.test(a.key) && /_no$|number$/.test(b.key) && a.kind === "string" && b.kind === "string") return "contract_no";
  if (a.kind === "string" && b.kind === "string" && NAME_KEYS.has(a.key) && NAME_KEYS.has(b.key)) return "name";
  return null;
}

function relationValue(f: FieldRecord): string | null {
  const v = f.normalized ?? f.value;
  if (!v) return null;
  if (f.kind === "brn" || f.kind === "account") return v.replace(/\D/g, "");
  if (f.kind === "amount") {
    const n = Number(String(v).replace(/[^0-9.-]/g, ""));
    return Number.isFinite(n) && n > 0 ? String(n) : null;
  }
  return compact(v) || null;
}

// ---------------------------------------------------------------------------
// Report (projects/report.rs)
// ---------------------------------------------------------------------------

export function report(projectId: string): string {
  const s = load();
  const p = getProject(s, projectId);
  const d = detail(s, p);
  const isKo = ko();
  const status = isKo ? { intake: "접수", extracting: "추출", validating: "검증", supplement: "보완요청", done: "완료" }[p.status] : p.status;
  const out: string[] = [`# ${isKo ? "프로젝트 리포트" : "Project report"} — ${p.name}`, ""];
  out.push(`| ${isKo ? "항목" : "Item"} | ${isKo ? "값" : "Value"} |`, "| --- | --- |");
  out.push(`| ${isKo ? "상태" : "Status"} | ${status} |`);
  if (p.intake_no) out.push(`| ${isKo ? "접수번호" : "Intake no."} | ${p.intake_no} |`);
  if (p.owner) out.push(`| ${isKo ? "담당" : "Owner"} | ${p.owner} |`);
  if (p.due_date) out.push(`| ${isKo ? "기한" : "Due"} | ${p.due_date} |`);
  out.push(`| ${isKo ? "문서 수" : "Documents"} | ${d.documents.length} |`);
  if (d.template) out.push(`| ${isKo ? "템플릿" : "Template"} | ${isKo ? d.template.name_ko : d.template.name_en} |`);
  out.push("", `## ${isKo ? "문서 체크리스트" : "Document checklist"}`, "");
  for (const c of d.checklist) out.push(`- ${c.missing ? "✖" : "✔"} ${isKo ? c.label_ko : c.label_en}${c.required ? "" : isKo ? " (추가)" : " (extra)"}${c.doc_ids.length > 0 ? ` — ${c.doc_ids.length}${isKo ? "건" : ""}` : isKo ? " — 누락" : " — missing"}`);
  out.push("", `## ${isKo ? "문서" : "Documents"}`, "");
  out.push(`| ${isKo ? "문서" : "Document"} | ${isKo ? "유형" : "Type"} | ${isKo ? "상태" : "Status"} | ${isKo ? "검토" : "Review"} |`, "| --- | --- | --- | --- |");
  for (const pd of d.documents) out.push(`| ${pd.document.title ?? pd.document.original_name} | ${pd.document.doc_type ? (isKo ? docTypeLabel(pd.document.doc_type).ko : docTypeLabel(pd.document.doc_type).en) : "-"} | ${pd.document.status} | ${pd.document.review_status} |`);
  out.push("", `## ${isKo ? "정합성 검사" : "Consistency check"}`, "");
  if (!d.last_run) out.push(isKo ? "아직 검사를 실행하지 않았습니다." : "No check has run yet.");
  else {
    const r = d.last_run;
    out.push(isKo ? `마지막 실행 ${r.finished_at.slice(0, 16).replace("T", " ")} — 통과 ${r.pass} · 불일치 ${r.mismatch} · 판단불가 ${r.undetermined} · 누락 ${r.missing}` : `Last run ${r.finished_at.slice(0, 16).replace("T", " ")} — pass ${r.pass} · mismatch ${r.mismatch} · undetermined ${r.undetermined} · missing ${r.missing}`, "");
    out.push(`| ${isKo ? "규칙" : "Rule"} | ${isKo ? "판정" : "Verdict"} | ${isKo ? "근거" : "Evidence"} |`, "| --- | --- | --- |");
    for (const x of r.results) {
      const ev = [x.left, x.right].filter((e): e is Evidence => !!e && !!e.doc_name).map((e) => `${e.doc_name}${e.page ? ` p.${e.page}` : ""}`).join(", ");
      out.push(`| ${x.description} | ${isKo ? { pass: "통과", mismatch: "불일치", undetermined: "판단불가", missing: "누락" }[x.verdict] : x.verdict} | ${x.message}${ev ? ` (${ev})` : ""} |`);
    }
  }
  out.push("", `## ${isKo ? "문서 간 관계" : "Relations"}`, "");
  if (d.relations.edges.length === 0) out.push(isKo ? "공통 엔터티가 없습니다." : "No shared entities.");
  for (const e of d.relations.edges) {
    const from = d.relations.nodes.find((n) => n.doc_id === e.from)?.label ?? e.from;
    const to = d.relations.nodes.find((n) => n.doc_id === e.to)?.label ?? e.to;
    out.push(`- ${from} ↔ ${to}: ${e.kind} = ${e.value}`);
  }
  return out.join("\n");
}

// ---------------------------------------------------------------------------
// 보완요청서 (projects/report.rs `supplement_request` + generate)
// ---------------------------------------------------------------------------

export function supplementRequest(projectId: string, findingIds: string[], output: OutputFormat | null): { req: DraftRequest; draft: Draft } {
  const s = load();
  const p = getProject(s, projectId);
  const run = lastRun(s, p.id);
  if (!run) throw err("invalid", "no_check_run");
  const findings = run.results.filter((r) => (findingIds.length === 0 ? r.verdict === "mismatch" || r.verdict === "missing" : findingIds.includes(r.id)));
  const isKo = ko();
  const missingTypes = findingIds.length === 0 ? run.missing_doc_types : [];
  if (findings.length === 0 && missingTypes.length === 0) throw err("invalid", "no_findings");
  const docs = docsOf(s, p.id);
  const party = docs.flatMap((d) => pl.latestFields(d.id)).find((f) => ["contractor.name", "proposer.name", "supplier.name", "company_name", "store_name", "applicant.name"].includes(f.path) && f.value)?.value ?? null;
  const d = buildSupplementDraft({
    projectId: p.id,
    projectName: p.name,
    intakeNo: p.intake_no,
    recipient: party ?? (isKo ? `${p.name} 담당자` : `${p.name} contact`),
    sender: p.owner ?? (isKo ? "DocuLens 심사팀" : "DocuLens review team"),
    language: isKo ? "ko" : "en",
    output: output ?? "hwpx",
    findings: findings.map((f) => ({
      id: f.id,
      item: f.description,
      finding: f.message,
      evidence: [f.left, f.right].filter((e): e is Evidence => !!e && !!e.doc_name).map((e) => `${e.doc_name}${e.page ? ` p.${e.page}` : ""}`).join(", "),
      verdict: f.verdict,
    })),
    missingDocTypes: missingTypes.map((slug) => (isKo ? docTypeLabel(slug).ko : docTypeLabel(slug).en)),
  });
  if (p.status !== "done") {
    p.status = "supplement";
    p.updated_at = now();
    save(s);
    changed([p.id]);
  }
  return d;
}

// ---------------------------------------------------------------------------
// Maintenance (P7 retention job)
// ---------------------------------------------------------------------------

interface MaintState {
  last_run_at: string | null;
  last_summary: MaintenanceSummary | null;
}

function loadMaint(): MaintState {
  try {
    const all = JSON.parse(localStorage.getItem(MAINT_KEY) ?? "{}") as Record<string, MaintState>;
    return all[currentWorkspacePath() ?? "(none)"] ?? { last_run_at: null, last_summary: null };
  } catch {
    return { last_run_at: null, last_summary: null };
  }
}

function saveMaint(m: MaintState): void {
  let all: Record<string, MaintState> = {};
  try {
    all = JSON.parse(localStorage.getItem(MAINT_KEY) ?? "{}") as Record<string, MaintState>;
  } catch {
    all = {};
  }
  all[currentWorkspacePath() ?? "(none)"] = m;
  localStorage.setItem(MAINT_KEY, JSON.stringify(all));
}

export function maintenanceStatus(): MaintenanceStatus {
  const m = loadMaint();
  const ws = currentWorkspaceView();
  const retention = ws?.manifest.retention;
  let next: string | null = null;
  if (retention?.purge_enabled) {
    const d = new Date();
    d.setHours(retention.purge_hour_local, 0, 0, 0);
    if (d.getTime() <= Date.now()) d.setDate(d.getDate() + 1);
    next = d.toISOString();
  }
  return { enabled: !!retention?.purge_enabled, purge_hour_local: retention?.purge_hour_local ?? 3, last_run_at: m.last_run_at, next_run_at: next, running: hasActiveJob("maintenance"), last_summary: m.last_summary };
}

export function maintenanceRunNow(): string {
  const active = listJobs().find((j) => j.kind === "maintenance" && (j.state === "queued" || j.state === "running" || j.state === "paused"));
  if (active) return active.id;
  return submitJob({ kind: "maintenance" }, "workspace", "low");
}

/**
 * The purge job (`maintenance::run`): progress only (no stage words), steps whose policy is "keep forever"
 * are listed in `skipped`, the finishing detail is the localised summary line the shell writes.
 */
export async function maintenanceWorker(job: Job, ctl: JobControl): Promise<WorkerOutcome> {
  const started = now();
  const t0 = performance.now();
  const stages = ["journal", "jobs", "answers", "cache", "trash", "logs", "temp", "documents", "extractions"] as const;
  const purged: MaintenanceSummary["purged"] = { journal: 0, jobs: 0, answers: 0, cache_bytes: 0, trash: 0, logs: 0, temp: 0, documents: 0, extractions: 0 };
  const retention = currentWorkspaceView()?.manifest.retention;
  const skipped: string[] = [];
  if (retention && !retention.purge_enabled) skipped.push("disabled");
  const policyOf: Partial<Record<(typeof stages)[number], number | null | undefined>> = { journal: retention?.journal_days, jobs: retention?.job_logs_days, answers: retention?.chats_days, logs: retention?.app_logs_days, temp: retention?.temp_days, documents: retention?.documents_days, extractions: retention?.extractions_days };
  let cancelled = false;
  for (const [i, stage] of stages.entries()) {
    if (ctl.cancelled()) {
      cancelled = true;
      break;
    }
    updateJob(job.id, { progress: i / stages.length });
    await pace(ctl, 120);
    if (stage in policyOf && policyOf[stage] == null) {
      // Rust reports the step by its policy name (`chats` for answers).
      skipped.push(stage === "answers" ? "chats" : stage);
      continue;
    }
    if (stage === "jobs" && retention?.job_logs_days != null) {
      const cutoff = Date.now() - retention.job_logs_days * 86_400_000;
      purged.jobs = listJobs().filter((j) => j.finished_at && new Date(j.finished_at).getTime() < cutoff).length;
    }
  }
  const summary: MaintenanceSummary = { started_at: started, elapsed_ms: Math.round(performance.now() - t0), purged, skipped, cancelled };
  saveMaint({ last_run_at: now(), last_summary: summary });
  if (cancelled) throw workerFailure("cancelled", summary);
  const ko = loadSettings().ui.language !== "en";
  const mb = Math.round(purged.cache_bytes / (1024 * 1024));
  const detail = ko ? `저널 ${purged.journal} · 작업 ${purged.jobs} · 대화 ${purged.answers} · 추출 ${purged.extractions} · 문서 ${purged.documents} · 캐시 ${mb} MB` : `journal ${purged.journal} · jobs ${purged.jobs} · chats ${purged.answers} · runs ${purged.extractions} · documents ${purged.documents} · cache ${mb} MB`;
  return { detail, result: summary };
}

// ---------------------------------------------------------------------------
// 자동 분류 (projects::classify) — the same deterministic scoring as the core
// ---------------------------------------------------------------------------

const GENERIC = new Set(["프로젝트", "등록", "심사", "정산", "계약", "공사", "사업", "건", "관련", "업무", "신규", "처리", "문서", "서류", "거래처", "구매", "project", "case", "new", "docs", "documents", "vendor", "onboarding"]);
const STOP = new Set(["문서", "파일", "사본", "최종", "수정", "스캔", "scan", "copy", "final", "pdf", "page", "쪽", "년", "월", "일", "및", "the", "and", "of", "untitled"]);

function words(text: string): string[] {
  return text
    .split(/[^\p{L}\p{N}]+/u)
    .map((w) => w.toLowerCase())
    .filter((w) => [...w].length >= 2 && !STOP.has(w));
}

function scoreDoc(s: PState, doc: MockDoc, p: Project, isKo: boolean): { score: number; reasons: string[] } {
  const members = docsOf(s, p.id);
  const vendors = new Set(members.map((d) => compact(d.vendor ?? "")).filter((v) => [...v].length >= 2));
  const titleWords = new Set(members.flatMap((d) => words(d.title ?? d.original_name)).filter((w) => !GENERIC.has(w)));
  const present = new Set(members.map((d) => d.doc_type).filter((t): t is string => !!t));
  const missing = (templateOf(s, p.template_id)?.required_doc_types ?? []).filter((t) => !present.has(t));
  const nameTokens = words(p.name).filter((w) => !GENERIC.has(w));
  const keywords = p.keywords.map((k) => k.trim().toLowerCase()).filter(Boolean);
  const title = doc.title?.trim() || doc.original_name;
  const head = `${title} ${doc.original_name} ${doc.rel_path} ${doc.vendor ?? ""}`.toLowerCase();
  const text = doc.pages
    .map((pg) => pg.text ?? "")
    .join("\n")
    .slice(0, 4000)
    .toLowerCase();
  const vendor = compact(doc.vendor ?? "");
  let score = 0;
  const reasons: string[] = [];
  const t = (a: string, b: string) => (isKo ? a : b);
  const folder = doc.rel_path.match(/^inbox\/([^/]+)\//)?.[1];
  if (p.inbox_folder && folder && compact(p.inbox_folder) === compact(folder)) {
    score += 4;
    reasons.push(`${t("폴더", "folder")} inbox/${folder}`);
  }
  const intake = compact(p.intake_no ?? "");
  if ([...intake].length >= 3) {
    if (compact(head).includes(intake)) {
      score += 3;
      reasons.push(`${t("접수번호", "intake no.")} ${p.intake_no}`);
    } else if (compact(text).includes(intake)) {
      score += 2.5;
      reasons.push(`${t("본문에 접수번호", "intake no. in the text")} ${p.intake_no}`);
    }
  }
  let hits = 0;
  for (const k of keywords) {
    if (head.includes(k)) {
      score += 2;
      reasons.push(`${t("키워드", "keyword")} '${k}'`);
      hits += 1;
    } else if (text.includes(k)) {
      score += 1.2;
      reasons.push(`${t("본문에 키워드", "keyword in the text")} '${k}'`);
      hits += 1;
    }
    if (hits >= 3) break;
  }
  hits = 0;
  for (const w of nameTokens) {
    if (keywords.includes(w)) continue;
    if (head.includes(w)) {
      score += 1.5;
      reasons.push(`${t("프로젝트 이름의", "project name word")} '${w}'`);
      hits += 1;
    } else if (text.includes(w)) {
      score += 0.8;
      reasons.push(`${t("본문에 프로젝트 이름의", "project name word in the text")} '${w}'`);
      hits += 1;
    }
    if (hits >= 3) break;
  }
  if ([...vendor].length >= 2) {
    if (vendors.has(vendor)) {
      score += 2;
      reasons.push(t("프로젝트 문서와 같은 거래처", "same vendor as the project's documents"));
    } else if (compact(p.name).includes(vendor) || nameTokens.some((w) => vendor.includes(compact(w)))) {
      score += 1.5;
      reasons.push(t("거래처가 프로젝트 이름에 있음", "vendor named in the project"));
    }
  }
  let shared = 0;
  for (const w of words(head).filter((x) => !GENERIC.has(x))) {
    if (!titleWords.has(w) || keywords.includes(w) || nameTokens.includes(w)) continue;
    score += 0.5;
    reasons.push(`${t("비슷한 제목", "similar title")} '${w}'`);
    shared += 1;
    if (shared >= 2) break;
  }
  if (doc.doc_type && missing.includes(doc.doc_type)) {
    score += 0.7;
    reasons.push(t("체크리스트에 없는 필수 서류", "required document type still missing"));
  }
  return { score, reasons };
}

export function classifySuggest(req: ClassifyRequest): ClassifyResult {
  const s = load();
  const isKo = ko();
  const projects = req.project_ids?.length ? s.projects.filter((p) => req.project_ids!.includes(p.id)) : s.projects;
  const all = wsDocs();
  const assigned = new Set(s.members.map((m) => m.doc_id));
  const docs = req.doc_ids?.length ? all.filter((d) => req.doc_ids!.includes(d.id)) : req.include_assigned ? all : all.filter((d) => !assigned.has(d.id)).sort((a, b) => b.imported_at.localeCompare(a.imported_at));
  const result: ClassifyResult = { suggestions: [], suggested: 0, undecided: 0, model_calls: 0, notes: [] };
  if (projects.length === 0) result.notes.push(isKo ? "프로젝트가 없습니다. 먼저 프로젝트를 만드세요." : "No projects yet: create one first.");
  if (req.use_model) result.notes.push(isKo ? "모델이 없어 규칙 점수만 사용했습니다." : "No model: rule scores only.");
  for (const d of docs) {
    const candidates: ClassifyCandidate[] = projects
      .map((p) => {
        const { score, reasons } = scoreDoc(s, d, p, isKo);
        return { project_id: p.id, project_name: p.name, score, reasons };
      })
      .filter((c) => c.score > 0)
      .sort((a, b) => b.score - a.score);
    const best = candidates[0]?.score ?? 0;
    const second = candidates[1]?.score ?? 0;
    let confidence = best > 0 ? best / (best + 2) : 0;
    if (best > 0 && second >= 0.6 * best) confidence *= 0.6;
    confidence = Math.min(0.98, confidence);
    const suggestion: ClassifySuggestion = {
      doc_id: d.id,
      title: d.title?.trim() || d.original_name,
      doc_type: d.doc_type,
      project_id: null,
      project_name: null,
      confidence,
      reasons: [],
      source: "none",
      alternatives: [],
      already_in: s.members.filter((m) => m.doc_id === d.id).map((m) => s.projects.find((p) => p.id === m.project_id)?.name ?? "").filter(Boolean),
    };
    if (confidence >= 0.45) {
      const chosen = candidates.shift()!;
      suggestion.project_id = chosen.project_id;
      suggestion.project_name = chosen.project_name;
      suggestion.reasons = chosen.reasons;
      suggestion.source = "rules";
    } else if (candidates[0]) {
      suggestion.reasons.push(`${isKo ? "신호가 약함 — 가장 가까운 프로젝트:" : "weak signals — closest project:"} ${candidates[0].project_name} (${candidates[0].score.toFixed(1)})`);
    } else {
      suggestion.reasons.push(isKo ? "일치하는 신호 없음" : "no matching signal");
    }
    suggestion.alternatives = candidates.slice(0, 3);
    result.suggestions.push(suggestion);
  }
  result.suggested = result.suggestions.filter((x) => x.project_id).length;
  result.undecided = result.suggestions.length - result.suggested;
  return result;
}

export function classifyApply(assignments: ClassifyAssignment[]): ClassifyApplyResult {
  const byProject = new Map<string, string[]>();
  for (const a of assignments) byProject.set(a.project_id, [...(byProject.get(a.project_id) ?? []), a.doc_id]);
  let added = 0;
  for (const [pid, ids] of byProject) {
    const before = load().members.filter((m) => m.project_id === pid).length;
    addDocuments(pid, ids);
    added += load().members.filter((m) => m.project_id === pid).length - before;
  }
  return { added, project_ids: [...byProject.keys()] };
}

// ---------------------------------------------------------------------------
// Commands (src-tauri/src/commands/projects.rs)
// ---------------------------------------------------------------------------

export async function mockProjects(cmd: string, a: Record<string, any>): Promise<unknown> { // eslint-disable-line @typescript-eslint/no-explicit-any
  const s = load();
  switch (cmd) {
    case "projects_list":
      return s.projects.map((p) => ({ ...refresh(s, p) })).sort((x, y) => y.updated_at.localeCompare(x.updated_at));
    case "project_create":
      return createProject(a.input as ProjectInput);
    case "project_update": {
      const input = a.input as ProjectInput;
      if (!input.id) throw err("invalid", "project_not_found");
      const p = upsertProject(s, input);
      save(s);
      changed([p.id]);
      return { ...p };
    }
    case "project_delete": {
      const id = String(a.id);
      const before = s.projects.length;
      s.projects = s.projects.filter((p) => p.id !== id);
      s.members = s.members.filter((m) => m.project_id !== id);
      s.rules = s.rules.filter((r) => r.project_id !== id);
      s.runs = s.runs.filter((r) => r.project_id !== id);
      save(s);
      changed([id]);
      return s.projects.length < before;
    }
    case "project_get":
      return detail(s, getProject(s, String(a.id)));
    case "project_add_documents":
      return addDocuments(String(a.id), (a.docIds as string[]) ?? []);
    case "project_remove_document": {
      const p = getProject(s, String(a.id));
      s.members = s.members.filter((m) => !(m.project_id === p.id && m.doc_id === a.docId));
      p.updated_at = now();
      refresh(s, p);
      save(s);
      changed([p.id]);
      return detail(s, p);
    }
    case "project_set_status": {
      const p = getProject(s, String(a.id));
      const status = a.status as ProjectStatus;
      if (!["intake", "extracting", "validating", "supplement", "done"].includes(status)) throw err("invalid", "status_invalid");
      p.status = status;
      p.updated_at = now();
      refresh(s, p);
      save(s);
      changed([p.id]);
      return { ...p };
    }
    case "project_templates":
      return templates(s);
    case "project_template_save": {
      const input = a.input as TemplateInput;
      const name = (input.name_ko ?? "").trim();
      if (!name) throw err("invalid", "template_name_empty");
      const existing = input.id ? s.templates.find((t) => t.id === input.id) : undefined;
      if (input.id && !existing) {
        if (BUILTIN_TEMPLATES.some((t) => t.id === input.id)) throw err("invalid", "template_builtin");
        throw err("invalid", "template_not_found");
      }
      const tpl: ProjectTemplate = {
        id: existing?.id ?? newId(),
        name_ko: name,
        name_en: (input.name_en ?? "").trim() || name,
        description_ko: (input.description_ko ?? "").trim(),
        description_en: (input.description_en ?? "").trim(),
        required_doc_types: [...new Set((input.required_doc_types ?? []).map((x) => x.trim()).filter(Boolean))],
        rules: (input.rules ?? []).map((r) => ({ ...r, id: r.id || newId() })),
        builtin: false,
        created_at: existing?.created_at ?? now(),
        updated_at: now(),
      };
      s.templates = [...s.templates.filter((t) => t.id !== tpl.id), tpl];
      save(s);
      changed(s.projects.filter((p) => p.template_id === tpl.id).map((p) => p.id));
      return tpl;
    }
    case "project_template_delete": {
      const id = String(a.id);
      if (BUILTIN_TEMPLATES.some((t) => t.id === id)) throw err("invalid", "template_builtin");
      const before = s.templates.length;
      s.templates = s.templates.filter((t) => t.id !== id);
      for (const p of s.projects) if (p.template_id === id) p.template_id = null;
      save(s);
      return s.templates.length < before;
    }
    case "project_define_rule": {
      getProject(s, String(a.id));
      await new Promise((r) => setTimeout(r, 120));
      return parseRule(String(a.description ?? ""));
    }
    case "project_save_rule":
      return saveRule(String(a.id), a.rule as Rule);
    case "project_delete_rule": {
      const p = getProject(s, String(a.id));
      const before = s.rules.length;
      s.rules = s.rules.filter((r) => !(r.project_id === p.id && r.rule.id === a.ruleId));
      save(s);
      changed([p.id]);
      return s.rules.length < before;
    }
    case "project_check":
      return check(String(a.id), (a.ruleIds as string[] | null) ?? []);
    case "project_runs":
      return s.runs.filter((r) => r.project_id === a.id).sort((x, y) => y.finished_at.localeCompare(x.finished_at));
    case "project_report":
      return { markdown: report(String(a.id)) };
    case "project_request_supplement": {
      const { req, draft } = supplementRequest(String(a.id), (a.findingIds as string[] | null) ?? [], (a.output as OutputFormat | null) ?? null);
      return { job_id: submitDraftJob(req, draft, { project_id: String(a.id), source: "project" }) };
    }
    case "project_classify_preview":
      return classifySuggest((a.req ?? {}) as ClassifyRequest);
    case "project_classify_apply":
      return classifyApply((a.assignments ?? []) as ClassifyAssignment[]);
    case "maintenance_status":
      return maintenanceStatus();
    case "maintenance_run_now":
      return { job_id: maintenanceRunNow() };
    default:
      throw err("internal", `mock projects: unknown command ${cmd}`);
  }
}
