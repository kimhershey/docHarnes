// Browser mock of the P6 document-generation commands (design §3.13, §3.14.4):
// built-in templates, the `draft` job that turns a request into a structured
// draft (fields, item tables, body blocks, references), the reverse
// validation (sums, checksums, dates, required fields), the HTML / Markdown
// preview, saving (file path + library registration) and the saved-draft
// store. Mirrors crates/doculens-core/src/generate; the Rust code is the
// source of truth, this file only has to behave the same way for the UI.

import {
  currentWorkspacePath,
  emit,
  err,
  loadDocs,
  loadSettings,
  pace,
  submitJob,
  updateJob,
  upsertDoc,
  workerFailure,
  wsDocs,
  type JobControl,
  type MockDoc,
  type WorkerOutcome,
} from "./mock-backend";
import * as pl from "./mock-pipeline";
import {
  EVENTS,
  type DocumentsChanged,
  type Draft,
  type DraftBlock,
  type DraftCitation,
  type DraftColumn,
  type DraftField,
  type DraftIssue,
  type DraftOutcome,
  type DraftRequest,
  type DraftSummary,
  type DraftTable,
  type Job,
  type Language,
  type OutputFormat,
  type SaveRequest,
  type SavedDocument,
  type SearchResponse,
  type SourceKind,
  type TemplateField,
  type TemplateInfo,
  type TemplateKind,
  type TemplateRef,
} from "./types";

/** Saved drafts (`.doculens/drafts/<id>.json`): `{ [workspace]: Draft[] }`. */
const DRAFTS_KEY = "doculens.mock.drafts";
const GENERATOR_MODEL = "qwen3.5-4b-q6k";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function now(): string {
  return new Date().toISOString();
}

function today(): string {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

function newId(): string {
  return crypto.randomUUID().replace(/-/g, "");
}

function compact(s: string): string {
  return s.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
}

export function formatAmount(n: number): string {
  return new Intl.NumberFormat("ko-KR", { maximumFractionDigits: 0 }).format(Math.round(n));
}

export function parseAmount(s: string | null | undefined): number | null {
  if (!s) return null;
  const digits = s.replace(/[^0-9.-]/g, "");
  if (!digits || !/\d/.test(digits)) return null;
  const n = Number(digits);
  return Number.isFinite(n) ? n : null;
}

function brnValid(value: string): boolean {
  const d = value.replace(/\D/g, "");
  if (d.length !== 10) return false;
  const w = [1, 3, 7, 1, 3, 7, 1, 3, 5];
  let sum = 0;
  for (let i = 0; i < 9; i += 1) sum += Number(d[i]) * w[i]!;
  sum += Math.floor((Number(d[8]) * 5) / 10);
  return (10 - (sum % 10)) % 10 === Number(d[9]);
}

function isoDate(v: string): string | null {
  const m = v.match(/(20\d{2}|19\d{2})\D{0,3}(\d{1,2})\D{0,3}(\d{1,2})/);
  if (!m) return null;
  return `${m[1]}-${m[2]!.padStart(2, "0")}-${m[3]!.padStart(2, "0")}`;
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

// ---------------------------------------------------------------------------
// Built-in templates (generate/templates.rs)
// ---------------------------------------------------------------------------

function tf(key: string, label_ko: string, label_en: string, kind = "string", required = false, columns?: DraftColumn[]): TemplateField {
  const f: TemplateField = { key, label_ko, label_en, kind, required };
  if (columns) f.columns = columns;
  return f;
}

const col = (key: string, label: string, kind = "string"): DraftColumn => ({ key, label, kind });

const ITEM_COLUMNS: DraftColumn[] = [col("item_name", "품명"), col("spec", "규격"), col("quantity", "수량", "integer"), col("unit_price", "단가", "amount"), col("amount", "금액", "amount")];

const TEMPLATES: TemplateInfo[] = [
  {
    kind: "official_letter",
    name_ko: "공문",
    name_en: "Official letter",
    description_ko: "수신·참조·제목·본문·붙임으로 구성된 A4 공문.",
    description_en: "A4 official letter: recipient, cc, subject, body, attachments.",
    fields: [tf("doc_no", "문서번호", "Document no."), tf("date", "시행일자", "Date", "date", true), tf("recipient", "수신", "Recipient", "string", true), tf("reference", "참조", "Cc"), tf("title", "제목", "Subject", "string", true), tf("sender", "발신", "Sender", "string", true), tf("attachments", "붙임", "Attachments")],
    sections_ko: ["본문", "붙임"],
    sections_en: ["Body", "Attachments"],
    keywords: ["공문", "협조문", "안내문", "official letter", "letter"],
  },
  {
    kind: "quotation",
    name_ko: "견적서",
    name_en: "Quotation",
    description_ko: "수신처·공급자·품목(품명·규격·수량·단가·금액)·합계·유효기간.",
    description_en: "Recipient, supplier, line items, total and validity.",
    fields: [
      tf("recipient", "수신", "Recipient", "string", true),
      tf("date", "견적일자", "Date", "date", true),
      tf("supplier", "공급자", "Supplier", "string", true),
      tf("supplier_brn", "공급자 사업자등록번호", "Supplier business reg. no.", "brn"),
      tf("valid_until", "유효기간", "Valid until", "date"),
      tf("total", "합계금액", "Total", "amount", true),
      tf("note", "비고", "Note"),
      tf("items", "견적 품목", "Items", "table", true, ITEM_COLUMNS),
    ],
    sections_ko: ["견적 조건"],
    sections_en: ["Terms"],
    keywords: ["견적", "견적서", "quotation", "quote"],
  },
  {
    kind: "transaction_statement",
    name_ko: "거래명세서",
    name_en: "Transaction statement",
    description_ko: "공급자·공급받는자·거래일·품목·공급가액·세액·합계.",
    description_en: "Supplier, buyer, date, items, supply / tax / grand total.",
    fields: [
      tf("date", "거래일자", "Date", "date", true),
      tf("supplier", "공급자", "Supplier", "string", true),
      tf("supplier_brn", "공급자 사업자등록번호", "Supplier business reg. no.", "brn"),
      tf("buyer", "공급받는자", "Buyer", "string", true),
      tf("supply_total", "공급가액 합계", "Supply total", "amount"),
      tf("tax_total", "세액 합계", "Tax total", "amount"),
      tf("total", "합계금액", "Grand total", "amount", true),
      tf("items", "거래 품목", "Items", "table", true, [col("item_name", "품명"), col("spec", "규격"), col("quantity", "수량", "integer"), col("unit_price", "단가", "amount"), col("amount", "공급가액", "amount"), col("tax", "세액", "amount")]),
    ],
    sections_ko: ["비고"],
    sections_en: ["Notes"],
    keywords: ["거래명세서", "명세서", "transaction statement", "statement"],
  },
  {
    kind: "contract",
    name_ko: "계약서",
    name_en: "Contract",
    description_ko: "계약명·갑·을·계약기간·계약금액·지급조건과 조항 본문.",
    description_en: "Title, parties, period, amount, payment terms and clauses.",
    fields: [tf("title", "계약명", "Title", "string", true), tf("party_a", "갑", "Party A", "string", true), tf("party_b", "을", "Party B", "string", true), tf("contract_date", "계약일", "Contract date", "date", true), tf("period_start", "계약기간 시작", "Period start", "date"), tf("period_end", "계약기간 종료", "Period end", "date"), tf("amount", "계약금액", "Amount", "amount"), tf("payment_terms", "지급조건", "Payment terms")],
    sections_ko: ["제1조 (목적)", "제2조 (계약 기간)", "제3조 (대금 및 지급)", "제4조 (기타)"],
    sections_en: ["Article 1 (Purpose)", "Article 2 (Term)", "Article 3 (Payment)", "Article 4 (Miscellaneous)"],
    keywords: ["계약", "계약서", "contract", "agreement"],
  },
  {
    kind: "report",
    name_ko: "보고서",
    name_en: "Report",
    description_ko: "개요·현황·분석·결론의 업무 보고서.",
    description_en: "Overview, status, analysis, conclusion.",
    fields: [tf("title", "제목", "Title", "string", true), tf("date", "작성일", "Date", "date", true), tf("author", "작성자", "Author"), tf("department", "부서", "Department"), tf("summary", "요약", "Summary")],
    sections_ko: ["개요", "현황", "분석", "결론 및 제언"],
    sections_en: ["Overview", "Status", "Analysis", "Conclusion"],
    keywords: ["보고서", "보고", "report", "정산서", "결과보고"],
  },
  {
    kind: "minutes",
    name_ko: "회의록",
    name_en: "Minutes",
    description_ko: "회의명·일시·장소·참석자·안건·논의·결정 사항·후속 조치.",
    description_en: "Meeting, date, place, attendees, agenda, discussion, decisions, actions.",
    fields: [tf("title", "회의명", "Meeting", "string", true), tf("date", "일시", "Date", "date", true), tf("place", "장소", "Place"), tf("attendees", "참석자", "Attendees"), tf("recorder", "작성자", "Recorder")],
    sections_ko: ["안건", "논의 내용", "결정 사항", "후속 조치"],
    sections_en: ["Agenda", "Discussion", "Decisions", "Actions"],
    keywords: ["회의록", "회의", "minutes", "meeting"],
  },
  {
    kind: "supplement_request",
    name_ko: "보완요청서",
    name_en: "Supplement request",
    description_ko: "정합성 검사의 불일치·누락 항목을 근거와 함께 보완 요청하는 문서(§3.15).",
    description_en: "Requests corrections for the mismatches / missing documents of a consistency check (§3.15).",
    fields: [
      tf("date", "요청일", "Date", "date", true),
      tf("recipient", "수신", "Recipient", "string", true),
      tf("project_name", "건명", "Case", "string", true),
      tf("intake_no", "접수번호", "Intake no."),
      tf("due_date", "보완 기한", "Due date", "date"),
      tf("sender", "발신", "Sender", "string", true),
      tf("findings", "보완 항목", "Findings", "table", true, [col("no", "번호", "integer"), col("item", "항목"), col("finding", "지적 사항"), col("evidence", "근거"), col("action", "조치")]),
    ],
    sections_ko: ["보완 요청 사유", "보완 방법"],
    sections_en: ["Reason", "How to supplement"],
    keywords: ["보완요청", "보완 요청서", "보완요청서", "supplement", "supplement request"],
  },
];

export function listTemplates(): TemplateInfo[] {
  return TEMPLATES;
}

export function templateInfo(kind: TemplateKind): TemplateInfo {
  return TEMPLATES.find((t) => t.kind === kind) ?? TEMPLATES[4]!;
}

// ---------------------------------------------------------------------------
// Template resolution (generate/draft.rs `resolve_template`)
// ---------------------------------------------------------------------------

/** Registered form / schema reference (id, name, doc-type slug) → `form` template, else a built-in template. */
export function resolveTemplate(req: DraftRequest): TemplateRef {
  const ref = (req.form_ref ?? "").trim();
  if (ref) {
    const key = compact(ref);
    const builtin = TEMPLATES.find((t) => t.kind === ref || compact(t.name_ko) === key || compact(t.name_en) === key);
    const schema = pl.listSchemas().find((s) => s.id === ref || compact(s.name) === key || compact(s.name_en ?? "") === key || compact(s.doc_type) === key);
    // A registered form wins over a built-in template of the same name (the user registered it on purpose).
    if (schema && schema.source === "form") return { kind: "form", schema_id: schema.id, doc_id: schema.form_document_id };
    if (builtin) return { kind: "builtin", template: builtin.kind };
    if (schema) return { kind: "form", schema_id: schema.id, doc_id: schema.form_document_id };
  }
  const text = compact(`${req.format_description ?? ""} ${req.content}`);
  for (const t of TEMPLATES) if (t.keywords.some((k) => text.includes(compact(k)))) return { kind: "builtin", template: t.kind };
  return { kind: "builtin", template: "report" };
}

/** Fields the layout wants: a registered form's scalar fields + tables, or the built-in template's. */
function layoutFields(template: TemplateRef): { fields: TemplateField[]; sections: string[]; name: string; docType: string } {
  if (template.kind === "form") {
    const schema = pl.getSchema(template.schema_id);
    if (schema) {
      const fields: TemplateField[] = [];
      for (const f of schema.definition.fields) {
        if (f.kind === "group") for (const c of f.fields) fields.push(tf(`${f.key}.${c.key}`, `${f.label_ko} ${c.label_ko}`.trim(), `${f.label_en} ${c.label_en}`.trim(), c.kind, c.required));
        else if (f.kind === "table") fields.push(tf(f.key, f.label_ko, f.label_en, "table", f.required, f.fields.map((c) => col(c.key, c.label_ko || c.label_en || c.key, c.kind))));
        else fields.push(tf(f.key, f.label_ko, f.label_en, f.kind, f.required));
      }
      return { fields, sections: ["본문"], name: schema.name, docType: schema.doc_type };
    }
  }
  const kind = template.kind === "builtin" ? template.template : "report";
  const info = templateInfo(kind);
  return { fields: info.fields, sections: info.sections_ko, name: info.name_ko, docType: kind };
}

// ---------------------------------------------------------------------------
// The simulated model (generate/draft.rs `draft`)
// ---------------------------------------------------------------------------

interface ContentHints {
  recipient: string | null;
  items: { name: string; quantity: number; unitPrice: number | null }[];
  amount: number | null;
  validityDays: number | null;
  title: string | null;
}

const RECIPIENT_RE = /((?:\(주\)|㈜|주식회사\s*)?[가-힣A-Za-z0-9·&]+(?:\(주\))?)\s*(?:에게|에|앞|측|귀하|귀중)(?=[\s,.]|$)/u;

/** Pulls the facts the sentence states (party, items, unit price, validity) out of the request text. */
export function contentHints(content: string): ContentHints {
  const text = content.replace(/\s+/g, " ").trim();
  const recipient = text.match(RECIPIENT_RE)?.[1]?.replace(/^(에|에게)$/, "") ?? null;
  const items: ContentHints["items"] = [];
  const itemRe = /([가-힣A-Za-z][가-힣A-Za-z0-9·\-]*)\s*(\d+)\s*(대|개|EA|ea|set|세트|건|식|권|박스|box)/gu;
  let m: RegExpExecArray | null;
  while ((m = itemRe.exec(text))) {
    const name = m[1]!;
    if (/^(유효기간|기한|합계|총|대당|개당|단가)$/.test(name)) continue;
    items.push({ name, quantity: Number(m[2]), unitPrice: null });
  }
  const unit = text.match(/(?:대당|개당|단가|unit price)\s*([\d,]+)\s*원?/iu);
  const unitPrice = unit ? parseAmount(unit[1]) : null;
  if (unitPrice !== null && items.length > 0) for (const it of items) if (it.unitPrice === null) it.unitPrice = unitPrice;
  const amountMatch = text.match(/(?:총액|합계|총|금액|계약금액|계약 금액)\s*([\d,]{4,})\s*원?/u) ?? text.match(/([\d,]{5,})\s*원/u);
  const amount = amountMatch ? parseAmount(amountMatch[1]) : null;
  const validity = text.match(/유효(?:기간)?\s*(\d+)\s*일/u);
  const quoted = text.match(/[“"']([^”"']{2,60})[”"']/u);
  return { recipient, items, amount, validityDays: validity ? Number(validity[1]) : null, title: quoted?.[1] ?? null };
}

function addDays(iso: string, days: number): string {
  const d = new Date(`${iso}T12:00:00`);
  d.setDate(d.getDate() + days);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

function docTitle(d: MockDoc): string {
  return d.title ?? d.original_name;
}

/** Latest extracted scalar fields of a library document, as "label: value" lines + citations. */
function documentFacts(doc: MockDoc): { lines: string[]; citation: DraftCitation; values: Record<string, string> } {
  const fields = pl.latestFields(doc.id).filter((f) => f.row === null && f.value);
  const values: Record<string, string> = {};
  for (const f of fields) values[f.path] = f.value!;
  const lines = fields.slice(0, 8).map((f) => `${f.label_ko || f.label_en}: ${f.value}`);
  const page = fields[0]?.page ?? 1;
  return { lines, values, citation: { doc_id: doc.id, title: docTitle(doc), page, chunk_id: null, source: "document", excerpt: lines.slice(0, 3).join(" · ") || null } };
}

/** Knowledge-base retrieval for grounding (§3.14.4): the mock hybrid search over indexed chunks. */
export async function knowledgeCitations(query: string, excluded: string[]): Promise<DraftCitation[]> {
  let res: SearchResponse | null = null;
  try {
    const kb = await import("./mock-knowledge");
    res = (await kb.mockKnowledge("knowledge_search", { req: { query, mode: "hybrid", limit: 6, scope: { roles: ["knowledge"], latest_only: true } } })) as SearchResponse;
  } catch {
    res = null;
  }
  if (!res) return [];
  return res.hits
    .filter((h) => !excluded.includes(h.document_id))
    .slice(0, 3)
    .map((h) => ({ doc_id: h.document_id, title: h.title, page: h.page_start, chunk_id: h.chunk_id, source: "knowledge", excerpt: h.snippet }));
}

function sentenceFor(section: string, content: string, ko: boolean, tone: DraftRequest["tone"], citation: DraftCitation | null): string {
  const ending = tone === "plain" ? (ko ? "입니다." : ".") : tone === "concise" ? (ko ? "임." : ".") : ko ? "합니다." : ".";
  const subject = content.replace(/\s+/g, " ").trim().replace(/[.。!?]+$/, "");
  const cite = citation ? ` [${citation.title}${citation.page ? ` p.${citation.page}` : ""}]` : "";
  if (!ko) return `${section}: this section covers "${subject}"${cite}.`;
  switch (section) {
    case "본문":
      return `위와 관련하여 ${subject} 건을 아래와 같이 요청드리오니 협조하여 주시기 바랍${ending.replace("합니다.", "니다.")}${cite}`;
    case "개요":
      return `본 보고서는 ${subject}에 대한 내용을 정리한 것${ending === "임." ? "임." : "입니다."}${cite}`;
    case "현황":
      return `현재 ${subject} 관련 현황은 다음과 같${ending === "임." ? "음." : "습니다."}${cite}`;
    case "분석":
      return `주요 수치와 추이를 검토한 결과 특이 사항은 없었으며, 세부 내역은 첨부 표를 참고하시기 바랍니다.${cite}`;
    case "결론 및 제언":
      return `위 내용을 바탕으로 ${subject}을(를) 계획대로 진행할 것을 제언${ending}`;
    case "견적 조건":
      return `본 견적은 부가가치세 별도이며, 납품 조건과 유효기간은 상기와 같${ending === "임." ? "음." : "습니다."}`;
    case "안건":
      return `${subject}`;
    case "논의 내용":
      return `${subject}에 대해 참석자 의견을 교환하였${ending === "임." ? "음." : "습니다."}`;
    case "결정 사항":
      return `논의 결과를 반영하여 다음 회의까지 세부안을 확정하기로 하였${ending === "임." ? "음." : "습니다."}`;
    case "후속 조치":
      return `담당자는 결정 사항을 정리하여 공유${ending}`;
    case "보완 요청 사유":
      return `정합성 검사 결과 아래 항목에서 불일치 또는 누락이 확인되어 보완을 요청${ending}`;
    case "보완 방법":
      return `해당 서류를 수정·보완하여 보완 기한 내에 제출하여 주시기 바랍니다.`;
    case "제1조 (목적)":
      return `본 계약은 ${subject}에 관하여 갑과 을의 권리·의무를 정함을 목적으로 ${ending === "임." ? "함." : "합니다."}`;
    case "제2조 (계약 기간)":
      return `계약 기간은 계약서에 명시한 시작일부터 종료일까지로 ${ending === "임." ? "함." : "합니다."}`;
    case "제3조 (대금 및 지급)":
      return `계약금액은 상기 금액으로 하며, 지급 조건에 따라 지급${ending}`;
    case "제4조 (기타)":
      return `본 계약에 명시되지 않은 사항은 상호 협의하여 정${ending === "임." ? "함." : "합니다."}`;
    case "붙임":
      return `붙임 1. 관련 서류 1부.  끝.`;
    default:
      return `${section}: ${subject}${cite}`;
  }
}

function keepEdited(previous: Draft | null, key: string): DraftField | null {
  const f = previous?.fields.find((x) => x.key === key);
  return f && f.user_edited ? f : null;
}

/**
 * Produces the draft the way the model would (deterministically): the layout's fields are filled from
 * the request text and the quoted sources, item rows come from "노트북 10대 대당 1,200,000원", body
 * paragraphs follow the template sections, and references list the documents / knowledge used.
 */
export function buildDraft(req: DraftRequest, knowledge: DraftCitation[] = []): Draft {
  const ko = req.language !== "en";
  const template = resolveTemplate(req);
  const layout = layoutFields(template);
  const hints = contentHints(req.content);
  const previous = req.previous;
  const feedback = (req.feedback ?? "").trim();
  const docs = wsDocs();
  const sourceDocs = req.sources.flatMap((s) => (s.kind === "document" ? docs.filter((d) => d.id === s.doc_id) : []));
  const facts = sourceDocs.map(documentFacts);
  const textSources = req.sources.filter((s): s is Extract<DraftRequest["sources"][number], { kind: "text" }> => s.kind === "text");
  const aggregate = req.sources.find((s) => s.kind === "aggregate");
  const references: DraftCitation[] = facts.map((f) => f.citation);
  for (const ts of textSources) references.push({ doc_id: null, title: ts.title ?? (ko ? "입력 텍스트" : "Pasted text"), page: null, chunk_id: null, source: "text", excerpt: ts.text.slice(0, 120) });
  if (aggregate) references.push({ doc_id: null, title: ko ? "집계 결과" : "Aggregate result", page: null, chunk_id: null, source: "aggregate", excerpt: JSON.stringify(aggregate.kind === "aggregate" ? aggregate.result : null).slice(0, 120) });
  if (req.use_knowledge) references.push(...knowledge);
  const excluded = new Set(req.excluded_sources);
  const refs = references.filter((r) => !r.doc_id || !excluded.has(r.doc_id));
  const firstFact = facts[0] ?? null;
  const factValue = (keys: string[]): string | null => {
    for (const f of facts) for (const k of keys) if (f.values[k]) return f.values[k]!;
    return null;
  };
  const feedbackAmount = feedback.match(/(?:금액|합계|총액)[^\d]{0,6}([\d,]{4,})\s*원?/u);
  const length = /짧게|간단히|shorter/i.test(feedback) ? "short" : /길게|자세히|longer/i.test(feedback) ? "long" : req.length;
  const tone = /격식|formal/i.test(feedback) ? "formal" : /쉽게|평이|plain/i.test(feedback) ? "plain" : /간결|개조식|concise/i.test(feedback) ? "concise" : req.tone;

  const title =
    keepEdited(previous, "title")?.value ??
    hints.title ??
    (template.kind === "builtin" && template.template === "quotation" && hints.recipient ? `${hints.recipient} ${layout.name}` : null) ??
    (req.content.length <= 40 ? req.content.replace(/\s*(만들어|작성|써)[^ ]*$/u, "").trim() || layout.name : `${layout.name} — ${today()}`);

  // -- tables first (totals are derived from the item rows), then the scalar fields --------
  const fields: DraftField[] = [];
  const tables: DraftTable[] = [];
  const ordered = [...layout.fields.filter((f) => f.kind === "table"), ...layout.fields.filter((f) => f.kind !== "table")];
  for (const f of ordered) {
    if (f.kind === "table") {
      const columns = f.columns ?? ITEM_COLUMNS;
      let rows: string[][] = [];
      const prevTable = previous?.tables.find((t) => t.key === f.key);
      if (prevTable && prevTable.rows.length > 0 && !/품목|항목|items?/i.test(feedback)) rows = prevTable.rows;
      else if (hints.items.length > 0) {
        rows = hints.items.map((it) =>
          columns.map((c) => {
            switch (c.key) {
              case "item_name":
              case "item":
              case "name":
                return it.name;
              case "quantity":
                return String(it.quantity);
              case "unit_price":
                return it.unitPrice !== null ? formatAmount(it.unitPrice) : "";
              case "amount":
              case "supply_amount":
                return it.unitPrice !== null ? formatAmount(it.unitPrice * it.quantity) : "";
              case "tax":
              case "tax_amount":
                return it.unitPrice !== null ? formatAmount(it.unitPrice * it.quantity * 0.1) : "";
              default:
                return "";
            }
          }),
        );
      } else if (f.required) {
        rows = [columns.map((c) => (c.key === "no" ? "1" : c.kind === "integer" ? "1" : c.kind === "amount" ? "" : ko ? `(${c.label})` : `(${c.label})`))];
      }
      tables.push({ key: f.key, label: ko ? f.label_ko : f.label_en, columns, rows });
      continue;
    }
    const edited = keepEdited(previous, f.key);
    if (edited) {
      fields.push({ ...edited });
      continue;
    }
    let value = "";
    let source: DraftCitation | null = null;
    switch (f.key) {
      case "recipient":
        value = hints.recipient ?? factValue(["buyer.name", "store_name", "company_name"]) ?? "";
        if (!hints.recipient && value && firstFact) source = firstFact.citation;
        break;
      case "supplier":
      case "sender":
        value = factValue(["supplier.name"]) ?? (ko ? "DocuLens" : "DocuLens");
        if (factValue(["supplier.name"]) && firstFact) source = firstFact.citation;
        break;
      case "supplier_brn":
        value = factValue(["supplier.brn", "registration_no", "brn"]) ?? "";
        if (value && firstFact) source = firstFact.citation;
        break;
      case "buyer":
        value = hints.recipient ?? factValue(["buyer.name"]) ?? "";
        break;
      case "party_a":
        value = hints.recipient ?? factValue(["supplier.name", "company_name"]) ?? "";
        break;
      case "party_b":
        value = "DocuLens";
        break;
      case "date":
      case "contract_date":
        value = today();
        break;
      case "valid_until":
        value = hints.validityDays !== null ? addDays(today(), hints.validityDays) : addDays(today(), 30);
        break;
      case "due_date":
        value = addDays(today(), 7);
        break;
      case "period_start":
        value = factValue(["period_start"]) ?? today();
        break;
      case "period_end":
        value = factValue(["period_end"]) ?? addDays(today(), 365);
        break;
      case "title":
      case "project_name":
        value = title;
        break;
      case "total":
      case "amount":
      case "grand_total": {
        const items = tables.find((t) => t.key === "items");
        const sum = items ? sumOfColumn(items, ["amount", "supply_amount"]) : null;
        if (feedbackAmount) value = formatAmount(parseAmount(feedbackAmount[1]) ?? 0);
        else if (sum !== null && sum > 0) value = formatAmount(Math.round(sum * 1.1)); // the model adds VAT to the total — the validator points the mismatch out
        else if (hints.amount !== null) value = formatAmount(hints.amount);
        else value = factValue(["grand_total", "total", "amount"]) ?? "";
        break;
      }
      case "supply_total": {
        const items = tables.find((t) => t.key === "items");
        const sum = items ? sumOfColumn(items, ["amount", "supply_amount"]) : null;
        value = sum !== null && sum > 0 ? formatAmount(sum) : (factValue(["supply_total"]) ?? "");
        break;
      }
      case "tax_total": {
        const items = tables.find((t) => t.key === "items");
        const sum = items ? sumOfColumn(items, ["tax", "tax_amount"]) : null;
        value = sum !== null && sum > 0 ? formatAmount(sum) : (factValue(["tax_total"]) ?? "");
        break;
      }
      case "payment_terms":
        value = factValue(["payment_terms"]) ?? (ko ? "검수 후 30일 이내 현금 지급" : "Cash within 30 days of acceptance");
        break;
      case "author":
      case "recorder":
        value = ko ? "담당자" : "Author";
        break;
      case "department":
        value = ko ? "경영지원팀" : "Operations";
        break;
      case "summary":
        value = req.content.replace(/\s+/g, " ").trim().slice(0, 80);
        break;
      case "doc_no":
        value = `DL-${today().replace(/-/g, "")}-01`;
        break;
      case "attendees":
        value = ko ? "담당자 외 3명" : "Team (4)";
        break;
      case "place":
        value = ko ? "회의실 A" : "Room A";
        break;
      case "note":
      case "reference":
      case "attachments":
      case "intake_no":
        value = "";
        break;
      default:
        // Registered-form fields: use the quoted document's value of the same key when it has one.
        value = factValue([f.key]) ?? "";
        if (value && firstFact) source = firstFact.citation;
    }
    fields.push({ key: f.key, label: ko ? f.label_ko || f.label_en : f.label_en || f.label_ko, kind: f.kind, required: f.required, value, normalized: null, source, user_edited: false });
  }
  // Items table in the body for templates that have one, otherwise plain sections.
  const body: DraftBlock[] = [];
  const sectionCount = length === "short" ? Math.min(2, layout.sections.length) : layout.sections.length;
  const sections = layout.sections.slice(0, sectionCount);
  const mainTable = tables.find((t) => t.rows.length > 0);
  for (const [i, section] of sections.entries()) {
    body.push({ kind: "heading", level: 2, text: section });
    const cite = i === 0 && refs.length > 0 ? refs[0]! : null;
    body.push({ kind: "paragraph", text: sentenceFor(section, req.content, ko, tone, cite), source: cite });
    if (length === "long" && i === 0) body.push({ kind: "list", items: ko ? ["요청 배경 및 목적", "주요 내용", "기대 효과"] : ["Background", "Main points", "Expected outcome"], ordered: false });
    if (i === 0 && mainTable) body.push({ kind: "table_ref", key: mainTable.key });
  }
  for (const f of facts) {
    if (f.lines.length === 0) continue;
    body.push({ kind: "paragraph", text: ko ? `참고: ${f.citation.title}의 추출 결과 — ${f.lines.slice(0, 3).join(", ")}.` : `Reference: ${f.citation.title} — ${f.lines.slice(0, 3).join(", ")}.`, source: f.citation });
  }
  const draft: Draft = {
    id: previous?.id ?? newId(),
    title,
    template,
    doc_type: layout.docType,
    fields,
    tables,
    body,
    references: refs,
    issues: [],
    language: req.language,
    tone,
    length,
    created_at: previous?.created_at ?? now(),
    updated_at: now(),
    model: GENERATOR_MODEL,
    request: { ...req, previous: null },
    notes: [ko ? "검토 전 초안입니다. 금액·날짜·사업자번호는 검증 결과를 확인하세요." : "Unreviewed draft: check the validation results for amounts, dates and registration numbers."],
    saved_path: previous?.saved_path ?? null,
    saved_doc_id: previous?.saved_doc_id ?? null,
  };
  if (feedback) draft.notes.push(ko ? `재생성 반영: ${feedback}` : `Regenerated with: ${feedback}`);
  validateDraft(draft);
  return draft;
}

function sumOfColumn(table: DraftTable, keys: string[]): number | null {
  const idx = table.columns.findIndex((c) => keys.includes(c.key));
  if (idx < 0) return null;
  let sum = 0;
  let any = false;
  for (const row of table.rows) {
    const n = parseAmount(row[idx]);
    if (n !== null) {
      sum += n;
      any = true;
    }
  }
  return any ? sum : null;
}

// ---------------------------------------------------------------------------
// Reverse validation (generate/validate.rs)
// ---------------------------------------------------------------------------

/** Fills `normalized` values and `issues` in place; computed sums fill empty total fields. */
export function validateDraft(draft: Draft): void {
  const issues: DraftIssue[] = [];
  const ko = draft.language !== "en";
  for (const f of draft.fields) {
    const v = f.value.trim();
    f.normalized = null;
    if (!v) {
      if (f.required) issues.push({ path: `fields.${f.key}`, code: "required_missing", message: ko ? `필수 항목 '${f.label}'이(가) 비어 있습니다.` : `Required field '${f.label}' is empty.`, blocking: true });
      continue;
    }
    switch (f.kind) {
      case "amount":
      case "integer": {
        const n = parseAmount(v);
        if (n === null) issues.push({ path: `fields.${f.key}`, code: "amount_invalid", message: ko ? `'${f.label}' 값을 숫자로 읽을 수 없습니다: ${v}` : `'${f.label}' is not a number: ${v}`, blocking: true });
        else f.normalized = String(n);
        break;
      }
      case "date": {
        const d = isoDate(v);
        if (!d) issues.push({ path: `fields.${f.key}`, code: "date_invalid", message: ko ? `'${f.label}' 날짜 형식을 인식할 수 없습니다: ${v}` : `'${f.label}' is not a date: ${v}`, blocking: true });
        else f.normalized = d;
        break;
      }
      case "brn": {
        const d = v.replace(/\D/g, "");
        if (!brnValid(d)) issues.push({ path: `fields.${f.key}`, code: "brn_invalid", message: ko ? `'${f.label}' 사업자등록번호 검증에 실패했습니다: ${v}` : `'${f.label}' failed the registration-number checksum: ${v}`, blocking: false });
        else f.normalized = `${d.slice(0, 3)}-${d.slice(3, 5)}-${d.slice(5)}`;
        break;
      }
      default:
        f.normalized = v;
    }
  }
  // Table row amounts and the sum rules (quantity × unit price = amount; Σ amount = total).
  for (const table of draft.tables) {
    const qi = table.columns.findIndex((c) => c.key === "quantity");
    const ui = table.columns.findIndex((c) => c.key === "unit_price");
    const ai = table.columns.findIndex((c) => ["amount", "supply_amount"].includes(c.key));
    table.rows.forEach((row, ri) => {
      if (qi >= 0 && ui >= 0 && ai >= 0) {
        const q = parseAmount(row[qi]);
        const u = parseAmount(row[ui]);
        const a = parseAmount(row[ai]);
        if (q !== null && u !== null && a !== null && Math.abs(q * u - a) > 0.5) {
          issues.push({ path: `tables.${table.key}[${ri}].${table.columns[ai]!.key}`, code: "sum_mismatch", message: ko ? `${ri + 1}행 금액 ${row[ai]}이(가) 수량×단가(${formatAmount(q * u)})와 다릅니다.` : `Row ${ri + 1} amount ${row[ai]} differs from quantity × unit price (${formatAmount(q * u)}).`, blocking: true, suggestion: formatAmount(q * u) });
        }
      }
    });
    const sum = sumOfColumn(table, ["amount", "supply_amount"]);
    if (sum === null) continue;
    const totalKey = table.key === "items" ? ["total", "grand_total", "amount"] : [];
    const totalField = draft.fields.find((f) => totalKey.includes(f.key) && f.kind === "amount");
    if (!totalField) continue;
    const supply = draft.fields.find((f) => f.key === "supply_total");
    const tax = draft.fields.find((f) => f.key === "tax_total");
    const expected = supply && tax ? (parseAmount(supply.value) ?? sum) + (parseAmount(tax.value) ?? 0) : sum;
    if (!totalField.value.trim()) {
      totalField.value = formatAmount(expected);
      totalField.normalized = String(expected);
      continue;
    }
    const total = parseAmount(totalField.value);
    if (total !== null && Math.abs(total - expected) > 0.5) {
      issues.push({ path: `fields.${totalField.key}`, code: "sum_mismatch", message: ko ? `'${totalField.label}' ${totalField.value}이(가) 품목 합계 ${formatAmount(expected)}와 다릅니다.` : `'${totalField.label}' ${totalField.value} differs from the item sum ${formatAmount(expected)}.`, blocking: true, suggestion: formatAmount(expected) });
    }
  }
  // Body claims that quote a source keep their citation; a paragraph that states a number without one is flagged (warning).
  draft.body.forEach((b, i) => {
    if (b.kind === "paragraph" && !b.source && /\d{1,3}(,\d{3}){2,}\s*원/.test(b.text)) issues.push({ path: `body[${i}]`, code: "uncited_claim", message: ko ? `${i + 1}번째 문단의 금액에 출처가 없습니다.` : `Paragraph ${i + 1} states an amount without a source.`, blocking: false });
  });
  draft.issues = issues;
  draft.updated_at = now();
}

// ---------------------------------------------------------------------------
// Rendering (generate/render.rs)
// ---------------------------------------------------------------------------

function tableHtml(t: DraftTable): string {
  const head = t.columns.map((c) => `<th>${escapeHtml(c.label)}</th>`).join("");
  const rows = t.rows.map((r) => `<tr>${t.columns.map((c, i) => `<td class="${c.kind === "amount" || c.kind === "integer" ? "num" : ""}">${escapeHtml(r[i] ?? "")}</td>`).join("")}</tr>`).join("");
  return `<table class="items"><caption>${escapeHtml(t.label)}</caption><thead><tr>${head}</tr></thead><tbody>${rows}</tbody></table>`;
}

export function toHtml(draft: Draft): string {
  const ko = draft.language !== "en";
  const fields = draft.fields.filter((f) => f.value.trim()).map((f) => `<tr><th>${escapeHtml(f.label)}</th><td>${escapeHtml(f.value)}</td></tr>`).join("");
  const body = draft.body
    .map((b) => {
      switch (b.kind) {
        case "heading":
          return `<h${Math.min(6, Math.max(1, b.level))}>${escapeHtml(b.text)}</h${Math.min(6, Math.max(1, b.level))}>`;
        case "paragraph":
          return `<p>${escapeHtml(b.text)}</p>`;
        case "list":
          return `<${b.ordered ? "ol" : "ul"}>${b.items.map((i) => `<li>${escapeHtml(i)}</li>`).join("")}</${b.ordered ? "ol" : "ul"}>`;
        case "table_ref": {
          const t = draft.tables.find((x) => x.key === b.key);
          return t ? tableHtml(t) : "";
        }
      }
    })
    .join("\n");
  const referenced = new Set(draft.body.flatMap((b) => (b.kind === "table_ref" ? [b.key] : [])));
  const extraTables = draft.tables.filter((t) => !referenced.has(t.key) && t.rows.length > 0).map(tableHtml).join("\n");
  const refs = draft.references.length > 0 ? `<section class="refs"><h3>${ko ? "참고 문서" : "References"}</h3><ol>${draft.references.map((r) => `<li>${escapeHtml(r.title)}${r.page ? ` p.${r.page}` : ""}${r.excerpt ? ` — <span class="excerpt">${escapeHtml(r.excerpt)}</span>` : ""}</li>`).join("")}</ol></section>` : "";
  const css = `body{font-family:"Malgun Gothic","Apple SD Gothic Neo","Noto Sans KR",sans-serif;margin:32px;color:#222;background:#fff;font-size:12pt;line-height:1.6}h1{text-align:center;font-size:20pt;letter-spacing:.2em;margin:0 0 24px}table.fields{border-collapse:collapse;margin:0 0 20px;width:100%}table.fields th{text-align:left;width:9em;padding:4px 8px;border:1px solid #999;background:#f3f3f3}table.fields td{padding:4px 8px;border:1px solid #999}table.items{border-collapse:collapse;width:100%;margin:12px 0}table.items caption{text-align:left;font-weight:600;margin-bottom:4px}table.items th,table.items td{border:1px solid #999;padding:4px 6px}table.items td.num{text-align:right}h2{font-size:13pt;margin:18px 0 6px}section.refs{margin-top:28px;font-size:10pt;color:#555}section.refs .excerpt{color:#777}`;
  return `<!doctype html><html lang="${ko ? "ko" : "en"}"><head><meta charset="utf-8"><title>${escapeHtml(draft.title)}</title><style>${css}</style></head><body><h1>${escapeHtml(draft.title)}</h1><table class="fields"><tbody>${fields}</tbody></table>${body}${extraTables}${refs}</body></html>`;
}

export function toMarkdown(draft: Draft): string {
  const ko = draft.language !== "en";
  const out: string[] = [`# ${draft.title}`, ""];
  for (const f of draft.fields) if (f.value.trim()) out.push(`- **${f.label}**: ${f.value}`);
  out.push("");
  const table = (t: DraftTable) => {
    out.push(`**${t.label}**`, "");
    out.push(`| ${t.columns.map((c) => c.label).join(" | ")} |`);
    out.push(`| ${t.columns.map(() => "---").join(" | ")} |`);
    for (const r of t.rows) out.push(`| ${t.columns.map((_, i) => r[i] ?? "").join(" | ")} |`);
    out.push("");
  };
  const referenced = new Set<string>();
  for (const b of draft.body) {
    switch (b.kind) {
      case "heading":
        out.push(`${"#".repeat(Math.min(6, b.level + 1))} ${b.text}`, "");
        break;
      case "paragraph":
        out.push(b.text, "");
        break;
      case "list":
        b.items.forEach((item, i) => out.push(`${b.ordered ? `${i + 1}.` : "-"} ${item}`));
        out.push("");
        break;
      case "table_ref": {
        const t = draft.tables.find((x) => x.key === b.key);
        if (t) {
          referenced.add(t.key);
          table(t);
        }
      }
    }
  }
  for (const t of draft.tables) if (!referenced.has(t.key) && t.rows.length > 0) table(t);
  if (draft.references.length > 0) {
    out.push(`## ${ko ? "참고 문서" : "References"}`, "");
    draft.references.forEach((r, i) => out.push(`${i + 1}. ${r.title}${r.page ? ` p.${r.page}` : ""}${r.excerpt ? ` — ${r.excerpt}` : ""}`));
  }
  return out.join("\n");
}

// ---------------------------------------------------------------------------
// Draft store (generate/store.rs)
// ---------------------------------------------------------------------------

function loadAllDrafts(): Record<string, Draft[]> {
  try {
    return JSON.parse(localStorage.getItem(DRAFTS_KEY) ?? "{}") as Record<string, Draft[]>;
  } catch {
    return {};
  }
}

function wsDrafts(): Draft[] {
  const ws = currentWorkspacePath();
  if (!ws) return [];
  return loadAllDrafts()[ws] ?? [];
}

function saveWsDrafts(list: Draft[]): void {
  const ws = currentWorkspacePath();
  if (!ws) return;
  const all = loadAllDrafts();
  all[ws] = list;
  localStorage.setItem(DRAFTS_KEY, JSON.stringify(all));
}

function summary(d: Draft): DraftSummary {
  return { id: d.id, title: d.title, template: d.template, updated_at: d.updated_at, issues: d.issues.length, saved_path: d.saved_path };
}

export function putDraft(draft: Draft): DraftSummary {
  const list = wsDrafts().filter((d) => d.id !== draft.id);
  draft.updated_at = now();
  list.push(draft);
  saveWsDrafts(list);
  return summary(draft);
}

export function listDrafts(): DraftSummary[] {
  return wsDrafts()
    .map(summary)
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at));
}

export function getDraft(id: string): Draft {
  const d = wsDrafts().find((x) => x.id === id);
  if (!d) throw err("not_found", "draft_not_found");
  return d;
}

export function deleteDraft(id: string): boolean {
  const list = wsDrafts();
  const next = list.filter((d) => d.id !== id);
  saveWsDrafts(next);
  return next.length < list.length;
}

// ---------------------------------------------------------------------------
// Saving (generate/render.rs `save`)
// ---------------------------------------------------------------------------

function sanitizeName(s: string): string {
  const cleaned = s
    .replace(/[\\/:*?"<>|\u0000-\u001f]/g, "_")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/[. ]+$/, "");
  return Array.from(cleaned).slice(0, 80).join("") || "document";
}

export function defaultFileName(draft: Draft): string {
  return sanitizeName(`${today()}_${draft.title || "document"}`);
}

function sourceKindOf(format: OutputFormat): SourceKind | null {
  switch (format) {
    case "hwpx":
      return "hwpx";
    case "docx":
      return "docx";
    case "pdf":
      return "pdf";
    case "md":
      // Markdown is an input format too (text family): generated drafts register like the core.
      return "text";
    default:
      return null;
  }
}

export function saveDraft(req: SaveRequest): SavedDocument {
  const ws = currentWorkspacePath();
  if (!ws) throw err("no_workspace", "no workspace is open");
  const draft = structuredClone(req.draft);
  validateDraft(draft);
  const folder = (req.folder ?? "documents/generated").replace(/\\/g, "/").replace(/^\/+|\/+$/g, "");
  if (!folder || folder.split("/").some((c) => c === "..") || folder.toLowerCase().startsWith(".doculens")) throw err("invalid", "path_outside");
  const base = sanitizeName(req.file_name?.trim() || defaultFileName(draft));
  const rel = `${folder}/${base}.${req.output}`;
  const ko = defaultLanguage() !== "en";
  // `warnings` are localised sentences (`generate::render::save`), shown by the UI as they are.
  const warnings: string[] = [];
  if (draft.issues.some((i) => i.blocking)) warnings.push(ko ? "검증에서 확인이 필요한 항목이 있는 상태로 저장했습니다 (검토 전)." : "Saved with unresolved validation issues (not yet reviewed).");
  const docs = loadDocs().filter((d) => d.workspace === ws && !d.deleted_at);
  const existing = docs.find((d) => d.rel_path.toLowerCase() === rel.toLowerCase()) ?? null;
  const taken = existing !== null || wsDrafts().some((d) => d.id !== draft.id && d.saved_path?.toLowerCase() === rel.toLowerCase());
  // Like the core: an existing file is an error unless `overwrite`, which replaces it (same path, same library document).
  if (taken && !req.overwrite) throw err("invalid", "export_exists");
  const kind = sourceKindOf(req.output);
  let docId: string | null = null;
  if (req.register && kind) {
    const markdown = toMarkdown(draft);
    const stamp = now();
    const doc: MockDoc = {
      workspace: ws,
      pages: [{ page_no: 1, width_px: 1654, height_px: 2339, rotation: 0, text: markdown, is_blank: false, rendered_at: stamp, text_source: "converted" }],
      id: existing?.id ?? newId(),
      rel_path: rel,
      original_name: `${base}.${req.output}`,
      source_kind: kind,
      content_hash: `generated-${draft.id}-${stamp}`,
      size_bytes: markdown.length * 2,
      file_modified_at: stamp,
      imported_at: existing?.imported_at ?? stamp,
      updated_at: stamp,
      status: "rendered",
      role: "generated",
      title: draft.title,
      doc_type: draft.doc_type,
      vendor: null,
      doc_date: today(),
      total_amount: parseAmount(draft.fields.find((f) => ["total", "grand_total", "amount"].includes(f.key))?.value) ?? null,
      language: draft.language,
      page_count: 1,
      has_text_layer: true,
      converted: kind !== "pdf",
      conversion_note: null,
      render_dpi: loadSettings().processing.render_dpi,
      error: null,
      import_batch_id: null,
      deleted_at: null,
      tags: existing ? [...new Set([...existing.tags, "생성"])] : ["생성"],
      doc_type_source: "form",
      classified_at: stamp,
      ocr_at: null,
      extracted_at: null,
      last_run_id: null,
      schema_id: null,
      review_status: "none",
      confidence: null,
      pipeline_error: null,
    };
    upsertDoc(doc);
    emit(EVENTS.documentsChanged, { kind: existing ? "updated" : "imported", ids: [doc.id] } satisfies DocumentsChanged);
    docId = doc.id;
  } else if (req.register && !kind) {
    warnings.push(ko ? "Markdown 파일은 라이브러리에 등록되지 않습니다(입력 형식이 아님)." : "Markdown files are not registered in the library (not an input format).");
  }
  draft.saved_path = rel;
  draft.saved_doc_id = docId;
  putDraft(draft);
  const sep = ws.includes("\\") ? "\\" : "/";
  return { rel_path: rel, absolute: `${ws}${sep}${rel.split("/").join(sep)}`, format: req.output, doc_id: docId, filled_form: draft.template.kind === "form" && (req.output === "hwpx" || req.output === "docx"), warnings };
}

// ---------------------------------------------------------------------------
// The `draft` job (generate/draft.rs through planner_bridge)
// ---------------------------------------------------------------------------

export interface DraftJobSpec {
  kind: "draft";
  req: DraftRequest;
  /** A rules-filled draft (보완요청서) that needs no model call. */
  prepared?: Draft | null;
  label: string;
  /** `JobSpec::Draft { project_id, source }`: the project of a 보완요청서 and where the job came from. */
  project_id?: string | null;
  source?: string;
}

function templateName(req: DraftRequest, ko: boolean): string {
  const t = resolveTemplate(req);
  if (t.kind === "form") return pl.getSchema(t.schema_id)?.name ?? "form";
  const info = templateInfo(t.template);
  return ko ? info.name_ko : info.name_en;
}

/** Queues the draft job (`generate_draft`); the finished job's `result` is a `DraftOutcome` (+ `project_id`, `job_source`). */
export function submitDraftJob(req: DraftRequest, prepared: Draft | null = null, origin: { project_id?: string | null; source?: string } = {}): string {
  const ko = req.language !== "en";
  const label = prepared?.title ?? req.content.replace(/\s+/g, " ").trim().slice(0, 60) ?? "";
  return submitJob({ kind: "draft", req, prepared, label, project_id: origin.project_id ?? null, source: origin.source ?? "generate" } satisfies DraftJobSpec, templateName(req, ko), "normal");
}

/**
 * Runs one draft (the executor's `draft_document` step and the draft job share it). The stage words are the
 * shell's (`generate_bridge::run_draft`): "loading model" (model drafts only), then "drafting".
 */
export async function draftNow(req: DraftRequest, ctl: JobControl | null, progress: (stage: string, frac: number) => void, prepared: Draft | null = null): Promise<DraftOutcome> {
  const started = performance.now();
  if (prepared) {
    progress("drafting", 0.2);
    await pace(ctl, 200);
    const draft = structuredClone(prepared);
    validateDraft(draft);
    putDraft(draft);
    return { draft, prompt_tokens: 0, completion_tokens: 0, elapsed_ms: Math.round(performance.now() - started), source: "rules" };
  }
  if (!req.content.trim()) throw err("invalid", "content_empty");
  progress("loading model", 0.05);
  const mode = pl.visionAvailability();
  if (mode === "engine_off") throw err("engine_not_running", "engine_not_running");
  if (mode === "none") throw err("invalid", "no_model");
  await pace(ctl, 240);
  pl.markVisionLoaded();
  progress("drafting", 0.2);
  const knowledge = req.use_knowledge ? await knowledgeCitations(req.content.slice(0, 60), req.excluded_sources) : [];
  await pace(ctl, 400);
  progress("drafting", 0.6);
  await pace(ctl, 400);
  const draft = buildDraft(req, knowledge);
  progress("drafting", 0.9);
  await pace(ctl, 150);
  putDraft(draft);
  const chars = Array.from(req.content).length;
  return { draft, prompt_tokens: 2400 + chars, completion_tokens: 350 + draft.body.length * 40, elapsed_ms: Math.round(performance.now() - started), source: "model" };
}

export async function draftWorker(job: Job, spec: DraftJobSpec, ctl: JobControl): Promise<WorkerOutcome> {
  try {
    const outcome = await draftNow(
      spec.req,
      ctl,
      (stage, frac) => {
        updateJob(job.id, { progress: frac, detail: stage });
      },
      spec.prepared ?? null,
    );
    const d = outcome.draft;
    // Finishing detail and payload like `run_draft`: localised summary; `source` stays the draft's own origin.
    const detail = spec.req.language === "en" ? `${d.title} · ${d.fields.length} fields · ${d.issues.length} issues` : `${d.title} · 필드 ${d.fields.length} · 검토 항목 ${d.issues.length}`;
    return { detail, result: { ...outcome, project_id: spec.project_id ?? null, job_source: spec.source ?? "generate" } satisfies DraftOutcome };
  } catch (e) {
    const message = typeof e === "object" && e !== null && "message" in e ? String((e as { message: unknown }).message) : String(e);
    throw workerFailure(message);
  }
}

// ---------------------------------------------------------------------------
// Commands (src-tauri/src/commands/generate.rs)
// ---------------------------------------------------------------------------

export function defaultLanguage(): Language {
  return loadSettings().ui.language;
}

export async function mockGenerate(cmd: string, a: Record<string, any>): Promise<unknown> { // eslint-disable-line @typescript-eslint/no-explicit-any
  switch (cmd) {
    case "generate_templates":
      return listTemplates();
    case "generate_draft": {
      const req = a.req as DraftRequest | undefined;
      if (!req || !req.content?.trim()) throw err("invalid", "content_empty");
      return { job_id: submitDraftJob({ ...req, language: req.language ?? defaultLanguage() }) };
    }
    case "generate_preview": {
      const draft = a.draft as Draft;
      return { html: toHtml(draft), markdown: toMarkdown(draft) };
    }
    case "generate_validate": {
      const draft = structuredClone(a.draft as Draft);
      validateDraft(draft);
      return draft;
    }
    case "generate_save":
      return saveDraft(a.req as SaveRequest);
    case "generate_drafts":
      return listDrafts();
    case "generate_draft_get":
      return getDraft(String(a.id));
    case "generate_draft_put":
      return putDraft(structuredClone(a.draft as Draft));
    case "generate_draft_delete":
      return deleteDraft(String(a.id));
    default:
      throw err("internal", `mock generate: unknown command ${cmd}`);
  }
}

// ---------------------------------------------------------------------------
// 보완요청서 from check findings (projects/report.rs `supplement_request` + generate/draft.rs rules path)
// ---------------------------------------------------------------------------

export interface SupplementInput {
  projectId: string;
  projectName: string;
  intakeNo: string | null;
  recipient: string;
  sender: string;
  language: Language;
  output: OutputFormat;
  findings: { id: string; item: string; finding: string; evidence: string; verdict: string }[];
  missingDocTypes: string[];
}

/** A rules-filled 보완요청서 draft (no model call): the request the planner would send plus the prepared draft. */
export function buildSupplementDraft(input: SupplementInput): { req: DraftRequest; draft: Draft } {
  const ko = input.language !== "en";
  const info = templateInfo("supplement_request");
  const lines = [...input.findings.map((f) => `- ${f.item}: ${f.finding}`), ...input.missingDocTypes.map((t) => (ko ? `- ${t} 누락` : `- ${t} missing`))];
  const content = ko ? `${input.projectName} 건 보완요청서. 보완 항목:\n${lines.join("\n")}` : `Supplement request for ${input.projectName}. Findings:\n${lines.join("\n")}`;
  const req: DraftRequest = {
    form_ref: "supplement_request",
    format_description: null,
    content,
    sources: [{ kind: "text", title: ko ? "정합성 검사 결과" : "Consistency check findings", text: lines.join("\n") }],
    output: input.output,
    tone: "formal",
    length: "medium",
    language: input.language,
    use_knowledge: false,
    feedback: null,
    previous: null,
    excluded_sources: [],
  };
  const rows: string[][] = [];
  input.findings.forEach((f, i) => rows.push([String(i + 1), f.item, f.finding, f.evidence, ko ? (f.verdict === "missing" ? "서류 제출" : "수정본 제출") : f.verdict === "missing" ? "Submit document" : "Submit corrected copy"]));
  input.missingDocTypes.forEach((t, i) => rows.push([String(input.findings.length + i + 1), t, ko ? `${t} 서류가 접수되지 않았습니다.` : `${t} was not received.`, "-", ko ? "서류 제출" : "Submit document"]));
  const fieldValue: Record<string, string> = {
    date: today(),
    recipient: input.recipient,
    project_name: input.projectName,
    intake_no: input.intakeNo ?? "",
    due_date: addDays(today(), 7),
    sender: input.sender,
  };
  const fields: DraftField[] = info.fields
    .filter((f) => f.kind !== "table")
    .map((f) => ({ key: f.key, label: ko ? f.label_ko : f.label_en, kind: f.kind, required: f.required, value: fieldValue[f.key] ?? "", normalized: null, source: null, user_edited: false }));
  const tableDef = info.fields.find((f) => f.kind === "table")!;
  const tables: DraftTable[] = [{ key: tableDef.key, label: ko ? tableDef.label_ko : tableDef.label_en, columns: tableDef.columns ?? [], rows }];
  const sections = ko ? info.sections_ko : info.sections_en;
  const body: DraftBlock[] = [
    { kind: "heading", level: 2, text: sections[0]! },
    { kind: "paragraph", text: sentenceFor(sections[0]!, content, ko, "formal", null) },
    { kind: "table_ref", key: tableDef.key },
    { kind: "heading", level: 2, text: sections[1]! },
    { kind: "paragraph", text: sentenceFor(sections[1]!, content, ko, "formal", null) },
  ];
  const draft: Draft = {
    id: newId(),
    title: ko ? `${input.projectName} 보완요청서` : `${input.projectName} supplement request`,
    template: { kind: "builtin", template: "supplement_request" },
    doc_type: "supplement_request",
    fields,
    tables,
    body,
    references: [{ doc_id: null, title: ko ? `정합성 검사 결과 (${input.projectName})` : `Consistency check (${input.projectName})`, page: null, chunk_id: null, source: "text", excerpt: lines.slice(0, 2).join(" · ") || null }],
    issues: [],
    language: input.language,
    tone: "formal",
    length: "medium",
    created_at: now(),
    updated_at: now(),
    model: null,
    request: req,
    notes: [ko ? "정합성 검사 결과에서 규칙으로 채운 초안입니다(모델 호출 없음)." : "Filled from the check findings by rules (no model call)."],
    saved_path: null,
    saved_doc_id: null,
  };
  validateDraft(draft);
  return { req, draft };
}
