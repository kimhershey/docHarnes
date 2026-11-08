// Browser mock of the pipeline commands (P4): classification by file name,
// canned OCR text, template-based extraction with confidences / validation,
// the review queue, schemas (built-in samples + user), form derivation,
// natural-language schemas, split / merge and the pipeline status — all
// publishing the same events as the Rust shell so the UI can be driven by
// Playwright without the desktop runtime.

import {
  createJob,
  currentWorkspacePath,
  emit,
  err,
  finishJob,
  gateStatus,
  hasActiveJob,
  loadDocs,
  loadSettings,
  pace,
  persistentQueued,
  simulate,
  submitJob,
  updateJob,
  upsertDoc,
  workerFailure,
  wsDocs,
  type JobControl,
  type MockDoc,
  type MockPage,
  type WorkerOutcome,
} from "./mock-backend";
import {
  EVENTS,
  type AutoProcessingLevel,
  type DocTypeInfo,
  type DocumentsChanged,
  type ExtractedTable,
  type ExtractionRun,
  type FieldChange,
  type FieldDef,
  type FieldKind,
  type FieldRecord,
  type FieldStatus,
  type ImportSource,
  type Job,
  type LintIssue,
  type PipelineChanged,
  type PipelineLevel,
  type PipelineStatus,
  type ProcessRequest,
  type ReviewAction,
  type ReviewFilter,
  type ReviewItem,
  type ReviewReason,
  type SchemaDef,
  type SchemaDraft,
  type SchemaRecord,
  type SchemaSource,
  type ValidationIssue,
  type VisionModelInfo,
} from "./types";

const KEY = "doculens.mock.pipeline";

/**
 * `localStorage.doculens.mock.vision` = ready (default) | not_loaded | none | engine_off — set by
 * Playwright (and by hand in the browser) to exercise the no-model / engine-off paths of the UI.
 */
const VISION_KEY = "doculens.mock.vision";
type VisionMode = "ready" | "not_loaded" | "none" | "engine_off";
const VISION_MODEL = "qwen3.5-4b-q6k";

function visionMode(): VisionMode {
  try {
    const v = localStorage.getItem(VISION_KEY);
    return v === "not_loaded" || v === "none" || v === "engine_off" ? v : "ready";
  } catch {
    return "ready";
  }
}

function visionInfo(): VisionModelInfo {
  const mode = visionMode();
  return { model: mode === "none" ? null : VISION_MODEL, engine_running: mode !== "engine_off", loaded: mode === "ready" };
}

/** `ready` | `not_loaded` | `none` | `engine_off` — the planner checks this before model-bound steps. */
export function visionAvailability(): VisionMode {
  return visionMode();
}

/** The shell loads the vision model on first use; from then on the status reports it as loaded. */
export function markVisionLoaded(): void {
  if (visionMode() !== "not_loaded") return;
  localStorage.setItem(VISION_KEY, "ready");
  changed("status");
}

interface PState {
  schemas: SchemaRecord[];
  runs: ExtractionRun[];
  fields: FieldRecord[];
  history: FieldChange[];
  seq: number;
}

function now(): string {
  return new Date().toISOString();
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
  const all = loadAll();
  const s = all[ws] ?? { schemas: [], runs: [], fields: [], history: [], seq: 1 };
  if (seedBuiltins(s)) {
    all[ws] = s;
    localStorage.setItem(KEY, JSON.stringify(all));
  }
  return s;
}

function save(s: PState): void {
  const ws = currentWorkspacePath() ?? "(none)";
  const all = loadAll();
  all[ws] = s;
  localStorage.setItem(KEY, JSON.stringify(all));
}

function changed(kind: PipelineChanged["kind"], ids: string[] = []): void {
  emit(EVENTS.pipelineChanged, { kind, ids } satisfies PipelineChanged);
}

function docsChanged(ids: string[]): void {
  if (ids.length > 0) emit(EVENTS.documentsChanged, { kind: "updated", ids } satisfies DocumentsChanged);
}

// ---------------------------------------------------------------------------
// Built-in sample forms (subset of the Rust definitions, same keys)
// ---------------------------------------------------------------------------

function f(key: string, label_ko: string, label_en: string, kind: FieldKind = "string", extra: Partial<FieldDef> = {}): FieldDef {
  return { key, label_ko, label_en, kind, required: false, regex_hint: null, example: null, description: null, options: [], fields: [], mask: false, ...extra };
}

const PARTY = (ko: string, en: string, email: boolean): FieldDef[] => [
  f("brn", `${ko} 사업자등록번호`, `${en} business reg. no.`, "brn", { required: true, example: "123-45-67890" }),
  f("name", `${ko} 상호`, `${en} name`, "string", { required: true }),
  f("representative", "대표자", "Representative"),
  f("address", "주소", "Address"),
  f("business_type", "업태", "Business type"),
  f("business_item", "종목", "Business item"),
  ...(email ? [f("email1", "이메일1", "Email 1", "email")] : []),
];

const INVOICE_ITEMS = (tax: boolean): FieldDef[] => [
  f("month_day", "월일", "Date (MM/DD)"),
  f("item", "품목", "Item"),
  f("spec", "규격", "Spec"),
  f("quantity", "수량", "Qty", "integer"),
  f("unit_price", "단가", "Unit price", "amount"),
  f("supply_amount", "공급가액", "Supply amount", "amount"),
  ...(tax ? [f("tax_amount", "세액", "Tax", "amount")] : []),
  f("note", "비고", "Note"),
];

interface BuiltinDef {
  key: string;
  name_ko: string;
  name_en: string;
  desc_ko: string;
  desc_en: string;
  fields: FieldDef[];
  rules: string[];
}

const BUILTIN: BuiltinDef[] = [
  {
    key: "tax_invoice",
    name_ko: "세금계산서",
    name_en: "Tax invoice",
    desc_ko: "과세·영세율 세금계산서(전자·종이). 면세 거래는 '계산서'를 사용.",
    desc_en: "Taxable / zero-rated tax invoice.",
    fields: [
      f("issue_date", "작성일자", "Issue date", "date", { required: true, example: "2026-10-14" }),
      f("tax_type", "과세유형", "Tax type", "enum", { required: true, options: ["과세", "영세율"] }),
      f("supplier", "공급자", "Supplier", "group", { fields: PARTY("공급자", "Supplier", false) }),
      f("buyer", "공급받는자", "Buyer", "group", { fields: PARTY("공급받는자", "Buyer", true) }),
      f("supply_total", "공급가액 합계", "Supply total", "amount", { required: true }),
      f("tax_total", "세액 합계", "Tax total", "amount", { required: true }),
      f("grand_total", "합계금액", "Grand total", "amount", { required: true }),
      f("nts_approval_no", "국세청 승인번호", "NTS approval no."),
      f("remarks", "비고", "Remarks"),
      f("items", "품목", "Line items", "table", { fields: INVOICE_ITEMS(true) }),
    ],
    rules: ["supply_plus_tax_equals_total", "items_sum_equals_supply", "tax_is_ten_percent", "zero_rate_tax_is_zero"],
  },
  {
    key: "invoice_exempt",
    name_ko: "계산서(면세)",
    name_en: "Invoice (tax-exempt)",
    desc_ko: "면세 거래의 계산서 — 세액 항목이 없음.",
    desc_en: "Invoice for tax-exempt transactions.",
    fields: [
      f("issue_date", "작성일자", "Issue date", "date", { required: true }),
      f("supplier", "공급자", "Supplier", "group", { fields: PARTY("공급자", "Supplier", false) }),
      f("buyer", "공급받는자", "Buyer", "group", { fields: PARTY("공급받는자", "Buyer", true) }),
      f("supply_total", "공급가액 합계", "Supply total", "amount", { required: true }),
      f("grand_total", "합계금액", "Grand total", "amount", { required: true }),
      f("items", "품목", "Line items", "table", { fields: INVOICE_ITEMS(false) }),
    ],
    rules: ["items_sum_equals_supply", "exempt_total_equals_supply"],
  },
  {
    key: "receipt",
    name_ko: "영수증",
    name_en: "Receipt",
    desc_ko: "카드·현금 영수증, 간이영수증.",
    desc_en: "Card / cash receipts.",
    fields: [
      f("store_name", "상호", "Store", "string", { required: true }),
      f("brn", "사업자등록번호", "Business reg. no.", "brn"),
      f("merchant_no", "가맹점번호", "Merchant no."),
      f("address", "주소", "Address"),
      f("phone", "전화", "Phone", "phone"),
      f("datetime", "일시", "Date / time", "date", { required: true }),
      f("items", "품목", "Items", "table", { fields: [f("name", "품명", "Item"), f("quantity", "수량", "Qty", "integer"), f("unit_price", "단가", "Unit price", "amount"), f("amount", "금액", "Amount", "amount")] }),
      f("supply_amount", "공급가액", "Supply amount", "amount"),
      f("vat", "부가세", "VAT", "amount"),
      f("service_charge", "봉사료", "Service charge", "amount"),
      f("total", "합계", "Total", "amount", { required: true }),
      f("payment_method", "결제수단", "Payment method", "enum", { options: ["카드", "현금", "현금영수증", "간편결제", "기타"] }),
      f("card_last4", "카드 끝 4자리", "Card last 4", "string", { regex_hint: "^[0-9]{4}$" }),
      f("approval_no", "승인번호", "Approval no."),
      f("cash_receipt_id", "현금영수증 식별번호", "Cash receipt id", "account"),
    ],
    rules: ["receipt_parts_equal_total", "items_sum_equals_total"],
  },
  {
    key: "business_registration",
    name_ko: "사업자등록증",
    name_en: "Business registration",
    desc_ko: "일반·간이·면세·법인 사업자등록증.",
    desc_en: "Business registration certificate.",
    fields: [
      f("tax_header", "과세유형", "Taxpayer type", "enum", { options: ["일반과세자", "간이과세자", "면세사업자", "법인사업자"] }),
      f("registration_no", "등록번호", "Registration no.", "brn", { required: true }),
      f("company_name", "상호(법인명)", "Company name", "string", { required: true }),
      f("representative", "대표자", "Representative", "string", { required: true }),
      f("birth_date", "생년월일", "Date of birth", "date", { mask: true }),
      f("opening_date", "개업연월일", "Opening date", "date"),
      f("business_address", "사업장 소재지", "Business address", "string", { required: true }),
      f("business_type", "업태", "Business type"),
      f("business_item", "종목", "Business item"),
      f("issue_date", "발급일", "Issue date", "date"),
      f("issuer", "발급기관", "Issuer"),
    ],
    rules: [],
  },
  {
    key: "contract",
    name_ko: "계약서",
    name_en: "Contract",
    desc_ko: "공급·용역·임대 등 계약서.",
    desc_en: "Supply / service / lease contracts.",
    fields: [
      f("title", "계약명", "Title", "string", { required: true }),
      f("parties", "당사자", "Parties", "table", { fields: [f("role", "구분", "Role"), f("name", "명칭", "Name"), f("representative", "대표자", "Representative"), f("brn", "사업자번호", "Business reg. no.", "brn")] }),
      f("contract_date", "계약일", "Contract date", "date"),
      f("period_start", "계약기간 시작", "Period start", "date"),
      f("period_end", "계약기간 종료", "Period end", "date"),
      f("amount", "계약금액", "Amount", "amount"),
      f("payment_terms", "지급조건", "Payment terms"),
      f("signed", "서명/날인 여부", "Signed / sealed", "boolean"),
    ],
    rules: ["period_start_before_end"],
  },
  {
    key: "application_form",
    name_ko: "신청서",
    name_en: "Application form",
    desc_ko: "각종 신청서·접수 서식.",
    desc_en: "Application / request forms.",
    fields: [
      f("form_title", "서식명", "Form title", "string", { required: true }),
      f("applicant", "신청인", "Applicant", "group", { fields: [f("name", "성명", "Name", "string", { required: true }), f("birth_date", "생년월일", "Date of birth", "date", { mask: true }), f("phone", "연락처", "Phone", "phone"), f("address", "주소", "Address")] }),
      f("application_date", "신청일", "Application date", "date"),
      f("items", "신청 항목", "Items", "table", { fields: [f("item", "항목", "Item"), f("value", "값", "Value")] }),
      f("signed", "서명/날인 여부", "Signed / sealed", "boolean"),
    ],
    rules: [],
  },
  {
    key: "general",
    name_ko: "일반(표/차트)",
    name_en: "General (table / chart)",
    desc_ko: "유형이 없는 문서 — 제목과 핵심 수치.",
    desc_en: "Untyped documents — title and key figures.",
    fields: [f("title", "제목", "Title"), f("date", "날짜", "Date", "date"), f("key_figures", "핵심 수치", "Key figures", "table", { fields: [f("label", "항목", "Label"), f("value", "값", "Value"), f("unit", "단위", "Unit")] })],
    rules: [],
  },
];

function seedBuiltins(s: PState): boolean {
  let added = false;
  for (const b of BUILTIN) {
    if (s.schemas.some((x) => x.builtin_key === b.key)) continue;
    const taken = s.schemas.some((x) => x.doc_type === b.key && x.is_default);
    s.schemas.push({
      id: `builtin-${b.key}`,
      name: b.name_ko,
      name_en: b.name_en,
      doc_type: b.key,
      source: "builtin",
      version: 1,
      description_ko: b.desc_ko,
      description_en: b.desc_en,
      definition: { fields: b.fields, rules: b.rules },
      form_document_id: null,
      builtin_key: b.key,
      enabled: true,
      is_default: !taken,
      use_count: 0,
      created_at: now(),
      updated_at: now(),
    });
    added = true;
  }
  return added;
}

// ---------------------------------------------------------------------------
// Keys / lint (mirrors pipeline::schema)
// ---------------------------------------------------------------------------

const KNOWN_KEYS: Record<string, string> = {
  상호: "company_name",
  사업자등록번호: "brn",
  대표자: "representative",
  성명: "name",
  이름: "name",
  주소: "address",
  연락처: "phone",
  전화번호: "phone",
  이메일: "email",
  날짜: "date",
  일자: "date",
  작성일자: "issue_date",
  금액: "amount",
  합계: "total",
  합계금액: "total",
  공급가액: "supply_amount",
  세액: "tax_amount",
  수량: "quantity",
  단가: "unit_price",
  품명: "item_name",
  품목: "item_name",
  규격: "spec",
  비고: "note",
  수신: "recipient",
  담당자: "contact_person",
  유효기간: "valid_until",
  계약명: "contract_title",
  부서: "department",
  사유: "reason",
  기간: "period",
};

export function isKey(s: string): boolean {
  return /^[a-z][a-z0-9_]{0,39}$/.test(s);
}

export function keyFromLabel(label: string, fallback: string): string {
  const l = label.trim().replace(/[:：]$/, "").replace(/\s/g, "");
  if (KNOWN_KEYS[l]) return KNOWN_KEYS[l]!;
  for (const [suf, k] of [["일자", "date"], ["날짜", "date"], ["금액", "amount"], ["번호", "number"], ["기간", "period"], ["명", "name"], ["사유", "reason"]] as const) {
    if (l.endsWith(suf) && !/^[\x00-\x7f]*$/.test(l)) return k;
  }
  const ascii = label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");
  const key = /^[0-9]/.test(ascii) ? `f_${ascii}` : ascii;
  return isKey(key) ? key : fallback;
}

export function kindFromLabel(label: string): FieldKind {
  const l = label.replace(/\s/g, "");
  if (l.includes("주민")) return "rrn";
  if (l.includes("사업자등록번호") || l.includes("사업자번호")) return "brn";
  if (l.includes("계좌")) return "account";
  if (l.includes("이메일") || /mail/i.test(l)) return "email";
  if (l.includes("전화") || l.includes("연락처")) return "phone";
  if (l.includes("수량") || l.includes("인원")) return "integer";
  if (/(금액|가액|합계|단가|세액|총액|부가세|비용)/.test(l)) return "amount";
  if (/(일자|날짜|년월일|연월일|기간|일시)/.test(l) || (l.endsWith("일") && l.length <= 6)) return "date";
  if (l.includes("여부")) return "boolean";
  return "string";
}

export function lint(def: SchemaDef): LintIssue[] {
  const out: LintIssue[] = [];
  const seen = new Set<string>();
  if (def.fields.length === 0) out.push({ level: "error", path: "", code: "fields_empty", message: "a schema needs at least one field" });
  const check = (fd: FieldDef, path: string, nested: boolean) => {
    if (!isKey(fd.key)) out.push({ level: "error", path, code: "key_format", message: `'${fd.key}' must be ASCII snake_case` });
    if (!fd.label_ko.trim() && !fd.label_en.trim()) out.push({ level: "error", path, code: "label_missing", message: "label required" });
    if (fd.regex_hint && !(fd.regex_hint.startsWith("^") && fd.regex_hint.endsWith("$"))) out.push({ level: "warn", path, code: "regex_anchor", message: "regex hints must be anchored ^…$" });
    if (fd.kind === "enum" && fd.options.length === 0) out.push({ level: "error", path, code: "enum_empty", message: "a choice field needs options" });
    if ((fd.kind === "table" || fd.kind === "group") && nested) out.push({ level: "error", path, code: "nesting", message: "tables / groups cannot be nested" });
    if ((fd.kind === "table" || fd.kind === "group") && fd.fields.length === 0) out.push({ level: "error", path, code: "children_empty", message: "needs at least one column" });
    const cseen = new Set<string>();
    for (const c of fd.fields) {
      if (cseen.has(c.key)) out.push({ level: "error", path: `${path}.${c.key}`, code: "key_duplicate", message: "duplicate column key" });
      cseen.add(c.key);
      check(c, `${path}.${c.key}`, true);
    }
  };
  for (const fd of def.fields) {
    if (seen.has(fd.key)) out.push({ level: "error", path: fd.key, code: "key_duplicate", message: "duplicate field key" });
    seen.add(fd.key);
    if (["value", "evidence", "confidence"].includes(fd.key)) out.push({ level: "error", path: fd.key, code: "key_reserved", message: "reserved key" });
    check(fd, fd.key, false);
  }
  return out;
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

function normalizeValue(kind: FieldKind, v: string): { normalized: string | null; issues: ValidationIssue[] } {
  const issues: ValidationIssue[] = [];
  switch (kind) {
    case "amount":
    case "integer": {
      const n = Number(v.replace(/[^0-9.-]/g, ""));
      if (!Number.isFinite(n) || v.replace(/[^0-9]/g, "") === "") {
        issues.push({ rule: `${kind}_parse`, level: "fail", message: "숫자로 읽을 수 없음 / not a number" });
        return { normalized: null, issues };
      }
      return { normalized: String(n), issues };
    }
    case "date": {
      const m = v.match(/(20\d{2}|19\d{2})\D{0,3}(\d{1,2})\D{0,3}(\d{1,2})/);
      if (!m) {
        issues.push({ rule: "date_parse", level: "fail", message: "날짜 형식을 인식할 수 없음 / unrecognised date" });
        return { normalized: null, issues };
      }
      return { normalized: `${m[1]}-${m[2]!.padStart(2, "0")}-${m[3]!.padStart(2, "0")}`, issues };
    }
    case "brn": {
      const d = v.replace(/\D/g, "");
      if (d.length !== 10) issues.push({ rule: "brn_format", level: "fail", message: "사업자등록번호는 10자리 / must be 10 digits" });
      else if (!brnValid(d)) issues.push({ rule: "brn_checksum", level: "fail", message: "사업자등록번호 검증자리 불일치 / checksum mismatch" });
      return { normalized: d.length === 10 ? `${d.slice(0, 3)}-${d.slice(3, 5)}-${d.slice(5)}` : null, issues };
    }
    case "rrn":
      return { normalized: v.replace(/(\d{6})-?(\d)\d{6}/, "$1-$2******"), issues };
    case "account": {
      const digits = v.replace(/\D/g, "").length;
      let seen = 0;
      return { normalized: Array.from(v).map((c) => (/\d/.test(c) ? (++seen > digits - 4 ? c : "*") : c)).join(""), issues };
    }
    case "boolean":
      return { normalized: /^(true|yes|예|☑|y|1)$/i.test(v) ? "true" : "false", issues };
    default:
      return { normalized: v, issues };
  }
}

// ---------------------------------------------------------------------------
// Classification / OCR / extraction templates
// ---------------------------------------------------------------------------

function classifyName(name: string, s: PState): string {
  const n = name.toLowerCase();
  const has = (t: string) => s.schemas.some((x) => x.doc_type === t && x.enabled);
  if (/(영수증|receipt)/.test(n)) return "receipt";
  if (/(세금계산서|tax|invoice)/.test(n)) return "tax_invoice";
  if (/(계산서)/.test(n)) return "invoice_exempt";
  if (/(계약|contract|agreement)/.test(n)) return "contract";
  if (/(사업자|registration)/.test(n)) return "business_registration";
  if (/(신청|application)/.test(n)) return "application_form";
  if (/(견적|quot)/.test(n) && has("quotation")) return "quotation";
  if (/(발주|purchase)/.test(n) && has("purchase_order")) return "purchase_order";
  return "other";
}

function ocrText(doc: MockDoc, type: string, pageNo: number): string {
  const head = `${doc.original_name} ${pageNo}쪽 (OCR)`;
  switch (type) {
    case "receipt":
      return `${head}\n# 스타벅스 역삼점\n사업자등록번호 220-81-62517\n2026-10-14 12:31\n<table><tr><th>품명</th><th>수량</th><th>단가</th><th>금액</th></tr><tr><td>아메리카노</td><td>2</td><td>4,500</td><td>9,000</td></tr><tr><td>쿠키</td><td>1</td><td>3,500</td><td>3,500</td></tr></table>\n공급가액 11,364\n부가세 1,136\n합계 12,500\n카드 승인 12345678`;
    case "tax_invoice":
      return `${head}\n세금계산서 (과세) 작성일자 2026-03-02\n공급자 한빛상사 220-81-62517 대표 김한빛\n공급받는자 DocuLens 123-45-67890\n<table><tr><th>월일</th><th>품목</th><th>수량</th><th>단가</th><th>공급가액</th><th>세액</th></tr><tr><td>03/02</td><td>노트북</td><td>2</td><td>500,000</td><td>1,000,000</td><td>100,000</td></tr></table>\n공급가액 1,000,000 세액 100,000 합계 1,100,000`;
    case "contract":
      return `${head}\n물품 공급 계약서\n갑: ㈜한빛 (대표 김한빛, 220-81-62517)\n을: DocuLens\n계약일 2026-01-15 계약기간 2026-02-01 ~ 2027-01-31\n계약금액 12,000,000원\n지급조건: 검수 후 30일 이내\n<signature/> 서명 날인`;
    case "business_registration":
      return `${head}\n사업자등록증 (일반과세자)\n등록번호: 220-81-62517\n상호: 한빛상사 대표자: 김한빛\n개업연월일: 2015-03-02\n사업장 소재지: 서울특별시 강남구 역삼로 12\n업태: 도소매 종목: 전자기기\n2026-01-05 강남세무서장 <stamp>세무서장인</stamp>`;
    case "application_form":
      return `${head}\n휴가 신청서\n성명: 홍길동 연락처: 010-1234-5678\n신청일: 2026-05-01\n<table><tr><th>항목</th><th>값</th></tr><tr><td>휴가 종류</td><td>연차</td></tr><tr><td>기간</td><td>2026-05-10 ~ 05-12</td></tr></table>\n☑ 확인 <signature/>`;
    default:
      return `${head}\n${doc.original_name}\n<table><tr><th>항목</th><th>값</th></tr><tr><td>합계</td><td>12,500</td></tr></table>`;
  }
}

interface TplScalar {
  v: string;
  c: number;
}
type Tpl = Record<string, TplScalar | Record<string, string>[]>;
const sv = (v: string, c: number): TplScalar => ({ v, c });

function template(type: string, doc: MockDoc): Tpl {
  switch (type) {
    case "receipt":
      return {
        store_name: sv("스타벅스 역삼점", 96),
        brn: sv("220-81-62517", 95),
        datetime: sv("2026-10-14 12:31", 92),
        total: sv("12,500", 94),
        supply_amount: sv("11,364", 88),
        vat: sv("1,136", 72),
        payment_method: sv("카드", 68),
        approval_no: sv("12345678", 52),
        items: [
          { name: "아메리카노", quantity: "2", unit_price: "4,500", amount: "9,000" },
          { name: "쿠키", quantity: "1", unit_price: "3,500", amount: "3,500" },
        ],
      };
    case "tax_invoice":
    case "invoice_exempt":
      return {
        issue_date: sv("2026-03-02", 93),
        tax_type: sv("과세", 90),
        "supplier.brn": sv("220-81-62517", 95),
        "supplier.name": sv("한빛상사", 96),
        "supplier.representative": sv("김한빛", 85),
        "buyer.brn": sv("123-45-67890", 90),
        "buyer.name": sv("DocuLens", 90),
        supply_total: sv("1,000,000", 92),
        tax_total: sv("100,000", 91),
        grand_total: sv("1,100,000", 93),
        items: [{ month_day: "03/02", item: "노트북", spec: "15형", quantity: "2", unit_price: "500,000", supply_amount: "1,000,000", tax_amount: "100,000" }],
      };
    case "contract":
      return {
        title: sv("물품 공급 계약서", 95),
        parties: [
          { role: "갑", name: "㈜한빛", representative: "김한빛", brn: "220-81-62517" },
          { role: "을", name: "DocuLens", representative: "", brn: "" },
        ],
        contract_date: sv("2026-01-15", 90),
        period_start: sv("2026-02-01", 88),
        period_end: sv("2027-01-31", 88),
        amount: sv("12,000,000", 86),
        payment_terms: sv("검수 후 30일 이내", 70),
        signed: sv("true", 80),
      };
    case "business_registration":
      return {
        tax_header: sv("일반과세자", 90),
        registration_no: sv("220-81-62517", 95),
        company_name: sv("한빛상사", 95),
        representative: sv("김한빛", 92),
        opening_date: sv("2015-03-02", 85),
        business_address: sv("서울특별시 강남구 역삼로 12", 88),
        business_type: sv("도소매", 80),
        business_item: sv("전자기기", 80),
        issue_date: sv("2026-01-05", 84),
        issuer: sv("강남세무서장", 82),
      };
    case "application_form":
      return {
        form_title: sv("휴가 신청서", 90),
        "applicant.name": sv("홍길동", 92),
        "applicant.phone": sv("010-1234-5678", 85),
        application_date: sv("2026-05-01", 88),
        items: [
          { item: "휴가 종류", value: "연차" },
          { item: "기간", value: "2026-05-10 ~ 05-12" },
        ],
        signed: sv("true", 75),
      };
    default:
      return { title: sv(doc.title ?? doc.original_name, 80), key_figures: [{ label: "합계", value: "12,500", unit: "원" }] };
  }
}

function sampleFor(kind: FieldKind, label: string): string {
  switch (kind) {
    case "amount":
      return "1,250,000";
    case "integer":
      return "3";
    case "date":
      return "2026-10-14";
    case "brn":
      return "220-81-62517";
    case "phone":
      return "010-1234-5678";
    case "email":
      return "docs@example.com";
    case "boolean":
      return "true";
    case "rrn":
      return "900101-1******";
    default:
      return `(샘플) ${label}`;
  }
}

function buildFields(s: PState, run: ExtractionRun, doc: MockDoc, schema: SchemaRecord, tpl: Tpl, threshold: number): FieldRecord[] {
  const out: FieldRecord[] = [];
  const scalar = (fd: FieldDef, path: string) => {
    const t = tpl[path];
    const custom = !(path in tpl);
    const scalarT = t && !Array.isArray(t) ? t : undefined;
    const raw = scalarT ? scalarT.v : custom && schema.source !== "builtin" ? sampleFor(fd.kind, fd.label_ko) : null;
    const modelConf = scalarT ? scalarT.c : custom ? 66 : null;
    const field: FieldRecord = {
      id: s.seq++,
      run_id: run.id,
      document_id: doc.id,
      path,
      key: fd.key,
      row: null,
      label_ko: fd.label_ko,
      label_en: fd.label_en,
      kind: fd.kind,
      required: fd.required,
      value: null,
      normalized: null,
      original_value: null,
      page: 1,
      bbox: null,
      evidence: null,
      confidence: 0,
      model_confidence: modelConf,
      status: "auto",
      review_reason: null,
      validation: [],
      reviewed_at: null,
    };
    if (raw === null) {
      field.review_reason = fd.required ? "required_missing" : null;
      out.push(field);
      return;
    }
    const { normalized, issues } = normalizeValue(fd.kind, raw);
    const fail = issues.some((i) => i.level === "fail");
    let conf = (modelConf ?? 70) / 100 + 0.05;
    if (fail) conf = Math.min(conf, 0.4);
    conf = Math.min(1, Math.round(conf * 1000) / 1000);
    field.value = fd.kind === "rrn" || fd.kind === "account" ? normalized : raw;
    field.original_value = field.value;
    field.normalized = normalized;
    field.evidence = raw;
    field.confidence = conf;
    field.validation = issues;
    field.review_reason = fail ? "validation_failed" : conf < threshold ? "low_confidence" : null;
    out.push(field);
  };
  for (const fd of schema.definition.fields) {
    if (fd.kind === "table") {
      const rows = (tpl[fd.key] as Record<string, string>[] | undefined) ?? (schema.source !== "builtin" ? [Object.fromEntries(fd.fields.map((c) => [c.key, sampleFor(c.kind, c.label_ko)]))] : []);
      rows.forEach((row, ri) => {
        for (const c of fd.fields) {
          const raw = row[c.key] ?? "";
          const { normalized, issues } = normalizeValue(c.kind, raw);
          const has = raw.trim() !== "";
          out.push({
            id: s.seq++,
            run_id: run.id,
            document_id: doc.id,
            path: `${fd.key}[${ri}].${c.key}`,
            key: c.key,
            row: ri,
            label_ko: c.label_ko,
            label_en: c.label_en,
            kind: c.kind,
            required: false,
            value: has ? raw : null,
            normalized: has ? normalized : null,
            original_value: has ? raw : null,
            page: 1,
            bbox: null,
            evidence: has ? Object.values(row).join(" | ") : null,
            confidence: has ? (issues.some((i) => i.level === "fail") ? 0.4 : 0.9) : 0,
            model_confidence: has ? 85 : null,
            status: "auto",
            review_reason: has && issues.some((i) => i.level === "fail") ? "validation_failed" : null,
            validation: has ? issues : [],
            reviewed_at: null,
          });
        }
      });
    } else if (fd.kind === "group") {
      for (const c of fd.fields) scalar(c, `${fd.key}.${c.key}`);
    } else {
      scalar(fd, fd.key);
    }
  }
  return out;
}

function schemaFor(s: PState, doc: MockDoc, choice: ProcessRequest["schema"]): SchemaRecord {
  if (choice && choice.kind === "id") {
    const sc = s.schemas.find((x) => x.id === choice.id);
    if (!sc) throw err("invalid", "schema_not_found");
    return sc;
  }
  if (doc.schema_id) {
    const sc = s.schemas.find((x) => x.id === doc.schema_id);
    if (sc) return sc;
  }
  const byType = (t: string) => s.schemas.filter((x) => x.doc_type === t && x.enabled).sort((a, b) => Number(b.is_default) - Number(a.is_default) || Number(a.source === "builtin") - Number(b.source === "builtin"))[0];
  return (doc.doc_type && doc.doc_type !== "other" ? byType(doc.doc_type) : undefined) ?? byType("general")!;
}

function recomputeDoc(s: PState, doc: MockDoc): void {
  const fields = doc.last_run_id ? s.fields.filter((x) => x.run_id === doc.last_run_id) : [];
  if (fields.length === 0) return;
  const needs = (x: FieldRecord) => x.review_reason !== null && !["approved", "corrected", "rejected"].includes(x.status);
  const pending = fields.some(needs);
  const flagged = fields.some((x) => x.review_reason !== null);
  doc.review_status = pending ? "pending" : flagged ? "done" : "none";
  const requiredOk = fields.filter((x) => x.required).every((x) => x.status !== "rejected" && x.value !== null);
  if (!pending && requiredOk && ["extracted", "rendered", "new"].includes(doc.status)) doc.status = "reviewed";
  if (pending && doc.status === "reviewed") doc.status = "extracted";
  doc.updated_at = now();
}

/** Mirror of `pipeline::run::ProcessReport` (the payload of a pipeline job / plan step). */
export interface MockProcessReport {
  processed: string[];
  failed: [string, string][];
  classified: number;
  ocr_pages: number;
  extracted: number;
  review_pending: number;
  cancelled: boolean;
}

/** Mirror of `pipeline::ProcessProgress` (`page` = (done, total) inside the current document). */
export interface ProcessProgress {
  stage: string;
  current: string;
  done: number;
  total: number;
  page: [number, number] | null;
}

/** Shell `progress_detail`: `name · ocr (2/4)`, `name · classify`, or the stage alone. */
export function progressDetail(p: ProcessProgress): string {
  if (p.page && p.page[1] > 0) return `${p.current} · ${p.stage} (${p.page[0]}/${p.page[1]})`;
  if (!p.current) return p.stage;
  return `${p.current} · ${p.stage}`;
}

function isCancellation(e: unknown): boolean {
  const x = e as { code?: unknown; message?: unknown } | null;
  return x?.code === "cancelled" || x?.message === "cancelled";
}

/**
 * Runs one `ProcessRequest` over the mock documents (classification by file name, canned OCR
 * text, template extraction with confidences). Shared by the persistent pipeline job and the
 * planner's classify / ocr / extract steps; `ctl` honours pause / cancel between documents and
 * pages, a cancelled run returns `cancelled: true` with what was done so far.
 */
export async function runProcess(req: ProcessRequest, ctl: JobControl | null, progress: (p: ProcessProgress) => void): Promise<MockProcessReport> {
  const settings = loadSettings();
  const s = load();
  const docs = wsDocs().filter((d) => req.document_ids.includes(d.id) && !["importing", "missing", "error"].includes(d.status));
  const report: MockProcessReport = { processed: [], failed: [], classified: 0, ocr_pages: 0, extracted: 0, review_pending: 0, cancelled: false };
  const wantedPages = req.pages && req.pages.length > 0 ? new Set(req.pages) : null;
  try {
    for (const [i, d0] of docs.entries()) {
      const doc = loadDocs().find((x) => x.id === d0.id);
      if (!doc) continue;
      const name = doc.title ?? doc.original_name;
      try {
        if (doc.pages.length === 0) throw new Error("no pages (render the document first)");
        progress({ stage: "classify", current: name, done: i, total: docs.length, page: null });
        await pace(ctl, 120);
        if (req.force || !doc.classified_at) {
          const type = classifyName(doc.original_name, s);
          if (doc.doc_type_source !== "manual") {
            doc.doc_type = type;
            doc.doc_type_source = "auto";
          }
          doc.language = "ko";
          doc.classified_at = now();
          for (const p of doc.pages) {
            p.page_doc_type = type;
            p.classified_at = now();
          }
          report.classified += 1;
        }
        doc.pipeline_error = null;
        if (req.level !== "classify") {
          const type = doc.doc_type ?? "other";
          let ocrPages = 0;
          for (const [pi, p] of doc.pages.entries()) {
            if (wantedPages && !wantedPages.has(p.page_no)) continue;
            const ownText = !!p.text && p.text_source !== "ocr";
            const doOcr = p.text_source === "ocr" ? !!req.force : ownText ? settings.processing.ocr_digital_pdf : true;
            if (!doOcr || p.width_px === 0) continue;
            progress({ stage: "ocr", current: name, done: i, total: docs.length, page: [pi + 1, doc.pages.length] });
            await pace(ctl, 120);
            p.text = ocrText(doc, type, p.page_no);
            p.text_source = "ocr";
            p.ocr_at = now();
            ocrPages += 1;
          }
          if (ocrPages > 0) doc.ocr_at = now();
          report.ocr_pages += ocrPages;
        }
        if (req.level === "extract") {
          progress({ stage: "extract", current: name, done: i, total: docs.length, page: null });
          await pace(ctl, 160);
          const schema = schemaFor(s, doc, req.schema);
          const run: ExtractionRun = {
            id: crypto.randomUUID().replace(/-/g, ""),
            document_id: doc.id,
            schema_id: schema.id,
            schema_version: schema.version,
            doc_type: doc.doc_type,
            model: "qwen3.5-4b-q6k (mock)",
            prompt_version: "p4.1",
            profile: req.profile ?? settings.models.profile,
            precise: req.precise ?? settings.processing.precise_mode,
            status: "done",
            error: null,
            started_at: now(),
            finished_at: now(),
            duration_ms: 1234,
            prompt_tokens: 3800,
            completion_tokens: 640,
            pages_done: Math.min(doc.pages.length, settings.processing.kie_max_pages),
            confidence: null,
            field_count: 0,
            review_count: 0,
          };
          const fields = buildFields(s, run, doc, schema, template(schema.doc_type, doc), settings.processing.review_threshold);
          run.field_count = fields.length;
          run.review_count = fields.filter((x) => x.review_reason !== null).length;
          const withValue = fields.filter((x) => x.value !== null);
          run.confidence = withValue.length > 0 ? Math.min(...withValue.map((x) => x.confidence)) : null;
          s.runs.unshift(run);
          s.fields.push(...fields);
          schema.use_count += 1;
          doc.last_run_id = run.id;
          doc.extracted_at = now();
          doc.confidence = run.confidence;
          doc.schema_id = doc.schema_id ?? null;
          const v = (p: string) => fields.find((x) => x.path === p)?.value ?? null;
          doc.vendor = v("supplier.name") ?? v("store_name") ?? v("company_name") ?? doc.vendor;
          doc.doc_date = fields.find((x) => ["issue_date", "datetime", "contract_date", "application_date", "date"].includes(x.path))?.normalized ?? doc.doc_date;
          const total = fields.find((x) => ["grand_total", "total", "amount"].includes(x.path))?.normalized;
          doc.total_amount = total ? Number(total) : doc.total_amount;
          doc.status = run.review_count > 0 ? "extracted" : "reviewed";
          doc.review_status = run.review_count > 0 ? "pending" : "none";
          report.extracted += 1;
          if (run.review_count > 0) report.review_pending += 1;
        }
        doc.updated_at = now();
        upsertDoc(doc);
        report.processed.push(doc.id);
      } catch (e) {
        if (isCancellation(e)) throw e;
        doc.pipeline_error = e instanceof Error ? e.message : String(e);
        upsertDoc(doc);
        report.failed.push([doc.id, doc.pipeline_error]);
      }
      docsChanged([doc.id]);
      save(s);
    }
  } catch (e) {
    if (!isCancellation(e)) throw e;
    report.cancelled = true;
  }
  save(s);
  changed("processed", [...report.processed, ...report.failed.map(([id]) => id)]);
  return report;
}

/** Worker of a persistent `pipeline` job (shell `run_pipeline`): loads the model, runs the request, reports like the shell. */
export async function pipelineWorker(job: Job, req: ProcessRequest, ctl: JobControl): Promise<WorkerOutcome> {
  const mode = visionMode();
  if (mode === "engine_off") throw workerFailure("engine_not_running");
  if (mode === "none") throw workerFailure("no_model");
  updateJob(job.id, { detail: "loading model" });
  await pace(ctl, 240);
  markVisionLoaded();
  const total = req.document_ids.length;
  const report = await runProcess(req, ctl, (p) => {
    const pagePart = p.page && p.page[1] > 0 ? p.page[0] / p.page[1] : 0;
    const frac = p.total === 0 ? null : Math.min(1, (p.done + pagePart * 0.9) / p.total);
    updateJob(job.id, { progress: frac, bytes_done: p.done, bytes_total: total, detail: progressDetail(p) });
  });
  const kb = await import("./mock-knowledge");
  kb.autoIndex();
  changed("status");
  if (report.cancelled) throw workerFailure("cancelled", report);
  if (report.failed.length > 0 && report.processed.length === 0) throw workerFailure(report.failed.map(([id, e]) => `${id}: ${e}`).join("; "), report);
  return {
    detail: `processed ${report.processed.length} / classified ${report.classified} / ocr pages ${report.ocr_pages} / extracted ${report.extracted} / review ${report.review_pending} / failed ${report.failed.length}`,
    result: report,
  };
}

/** `pipeline_process`: queues a persistent engine-lane job (one runs at a time, like the shell) and returns its id. */
function processJob(req: ProcessRequest): string {
  if (req.document_ids.length === 0) throw err("invalid", "no documents");
  const mode = visionMode();
  if (mode === "engine_off") throw err("invalid", "engine_not_running");
  if (mode === "none") throw err("invalid", "no_model");
  const id = submitJob({ kind: "pipeline", req }, `${req.document_ids.length} documents`, "normal");
  changed("status");
  return id;
}

// ---------------------------------------------------------------------------
// Helpers shared with the planner mock (fields, schemas, review, tables)
// ---------------------------------------------------------------------------

/** Fields of the document's latest run (`pipeline::repo::latest_fields`). */
export function latestFields(documentId: string): FieldRecord[] {
  const s = load();
  const doc = loadDocs().find((d) => d.id === documentId);
  return doc?.last_run_id ? s.fields.filter((f) => f.run_id === doc.last_run_id) : [];
}

/** Default enabled schema of a document type (`pipeline::repo::schema_for_doc_type`). */
export function schemaForDocType(slug: string): SchemaRecord | null {
  const s = load();
  return (
    s.schemas
      .filter((x) => x.doc_type === slug && x.enabled)
      .sort((a, b) => Number(b.is_default) - Number(a.is_default) || Number(a.source === "builtin") - Number(b.source === "builtin") || b.use_count - a.use_count || b.updated_at.localeCompare(a.updated_at))[0] ?? null
  );
}

export function getSchema(id: string): SchemaRecord | null {
  return load().schemas.find((x) => x.id === id) ?? null;
}

export function listSchemas(): SchemaRecord[] {
  return load().schemas;
}

export function docTypeList(): DocTypeInfo[] {
  return docTypes(load());
}

/** Approves every pending field of the document (`review::approve_document`); returns how many. */
export function approveDocument(documentId: string, reason: ReviewReason | null): number {
  const s = load();
  const ids = reviewQueue(s, { document_id: documentId, reason }).map((x) => x.id);
  if (ids.length > 0) applyReview(s, ids, { kind: "approve" }, "bulk");
  else {
    const doc = loadDocs().find((d) => d.id === documentId);
    if (doc) {
      recomputeDoc(s, doc);
      upsertDoc(doc);
      docsChanged([doc.id]);
    }
  }
  save(s);
  changed("review", [documentId]);
  return ids.length;
}

/** Rejects every pending field of the document; returns how many. */
export function rejectPending(documentId: string, reason: ReviewReason | null): number {
  const s = load();
  const ids = reviewQueue(s, { document_id: documentId, reason }).map((x) => x.id);
  if (ids.length === 0) return 0;
  applyReview(s, ids, { kind: "reject" }, "planner");
  save(s);
  changed("review", [documentId]);
  return ids.length;
}

/** Re-runs the validators over the latest fields (`review::revalidate`); returns the fields. */
export function revalidateDocument(documentId: string): FieldRecord[] {
  const s = load();
  const doc = wsDocs().find((d) => d.id === documentId);
  const threshold = loadSettings().processing.review_threshold;
  const fields = doc?.last_run_id ? s.fields.filter((x) => x.run_id === doc.last_run_id) : [];
  for (const fld of fields) {
    if (["approved", "corrected"].includes(fld.status)) continue;
    if (fld.value === null) {
      fld.review_reason = fld.required ? "required_missing" : null;
      continue;
    }
    const { issues } = normalizeValue(fld.kind, fld.value);
    fld.validation = issues;
    const fail = issues.some((i) => i.level === "fail");
    fld.review_reason = fail ? "validation_failed" : fld.confidence < threshold ? "low_confidence" : null;
  }
  if (doc) {
    recomputeDoc(s, doc);
    upsertDoc(doc);
    docsChanged([doc.id]);
  }
  save(s);
  changed("fields", doc ? [doc.id] : []);
  return fields;
}

/** Tables found in the document's page texts (`pipeline::run::document_tables`). */
export function documentTables(documentId: string): ExtractedTable[] {
  const doc = wsDocs().find((d) => d.id === documentId);
  return doc ? doc.pages.flatMap((p) => parseTables(p.text ?? "", p.page_no)) : [];
}

/** Called after imports: the configured level for the source (or the dialog's choice). */
export function autoProcess(ids: string[], level: AutoProcessingLevel | null, source: ImportSource): void {
  if (ids.length === 0) return;
  const settings = loadSettings();
  const lvl = level ?? (source === "inbox" ? settings.processing.inbox_auto_level : settings.processing.auto_level);
  const map: Record<AutoProcessingLevel, PipelineLevel | null> = { none: null, classify_only: "classify", classify_and_ocr: "ocr", full: "extract" };
  const target = map[lvl];
  if (!target) return;
  const mode = visionMode();
  if (mode === "engine_off" || mode === "none") {
    // The shell skips the auto pipeline and notifies the library instead of failing the import.
    changed(mode === "engine_off" ? "engine_off" : "no_model");
    return;
  }
  processJob({ document_ids: ids, level: target });
}

// ---------------------------------------------------------------------------
// Schemas / forms / NL
// ---------------------------------------------------------------------------

function docTypes(s: PState): DocTypeInfo[] {
  const out: DocTypeInfo[] = [];
  for (const sc of s.schemas.filter((x) => x.enabled).sort((a, b) => b.use_count - a.use_count)) {
    const e = out.find((d) => d.slug === sc.doc_type);
    if (e) {
      e.use_count += sc.use_count;
      continue;
    }
    out.push({ slug: sc.doc_type, label_ko: sc.name, label_en: sc.name_en ?? sc.name, schema_id: sc.id, builtin: sc.source === "builtin", use_count: sc.use_count });
  }
  return out.sort((a, b) => Number(a.builtin) - Number(b.builtin) || b.use_count - a.use_count);
}

function saveSchema(s: PState, draft: SchemaDraft): SchemaRecord {
  const name = draft.name.trim();
  if (!name) throw err("invalid", "schema_name_empty");
  if (!isKey(draft.doc_type.trim())) throw err("invalid", "schema_doc_type_invalid");
  if (lint(draft.definition).some((i) => i.level === "error")) throw err("invalid", "schema_invalid");
  if (draft.id) {
    const cur = s.schemas.find((x) => x.id === draft.id);
    if (!cur) throw err("invalid", "schema_not_found");
    pushVersion(cur.id, { version: cur.version, definition: structuredClone(cur.definition), note: draft.note, created_at: now() });
    cur.name = name;
    cur.name_en = draft.name_en?.trim() || null;
    cur.doc_type = draft.doc_type.trim();
    cur.description_ko = draft.description_ko;
    cur.description_en = draft.description_en;
    cur.definition = structuredClone(draft.definition);
    cur.version += 1;
    cur.form_document_id = draft.form_document_id ?? cur.form_document_id;
    cur.updated_at = now();
    return cur;
  }
  const doc_type = draft.doc_type.trim();
  const hasUserDefault = s.schemas.some((x) => x.doc_type === doc_type && x.is_default && x.source !== "builtin");
  if (!hasUserDefault) for (const x of s.schemas) if (x.doc_type === doc_type) x.is_default = false;
  const rec: SchemaRecord = {
    id: crypto.randomUUID().replace(/-/g, ""),
    name,
    name_en: draft.name_en?.trim() || null,
    doc_type,
    source: draft.source,
    version: 1,
    description_ko: draft.description_ko,
    description_en: draft.description_en,
    definition: structuredClone(draft.definition),
    form_document_id: draft.form_document_id,
    builtin_key: null,
    enabled: true,
    is_default: !hasUserDefault,
    use_count: 0,
    created_at: now(),
    updated_at: now(),
  };
  s.schemas.push(rec);
  return rec;
}

const VERSIONS_KEY = "doculens.mock.schema-versions";

interface SchemaVersion {
  version: number;
  definition: SchemaDef;
  note: string | null;
  created_at: string;
}

function loadVersions(): Record<string, SchemaVersion[]> {
  try {
    return JSON.parse(localStorage.getItem(VERSIONS_KEY) ?? "{}") as Record<string, SchemaVersion[]>;
  } catch {
    return {};
  }
}

/** Records the schema's current definition as a version before it is overwritten (persisted right away). */
function pushVersion(id: string, entry: SchemaVersion): void {
  const all = loadVersions();
  (all[id] ??= []).unshift(entry);
  localStorage.setItem(VERSIONS_KEY, JSON.stringify(all));
}

function versionsOf(id: string): SchemaVersion[] {
  return loadVersions()[id] ?? [];
}

const SLUGS: Record<string, string> = { 견적서: "quotation", 발주서: "purchase_order", 거래명세서: "transaction_statement", 공문: "official_letter", 보고서: "report", 회의록: "minutes", 청구서: "bill", 휴가신청서: "leave_request", 출장신청서: "trip_request", 계약서: "contract", 신청서: "application_form", 영수증: "receipt", 세금계산서: "tax_invoice" };

export function slugForName(name: string): string {
  const n = name.replace(/\(.*?\)|\s/g, "");
  for (const [k, v] of Object.entries(SLUGS)) if (n === k || n.endsWith(k) || n.startsWith(k)) return v;
  const ascii = keyFromLabel(name, "");
  if (isKey(ascii) && ascii.length >= 3) return ascii;
  let h = 0;
  for (const c of n) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return `form_${h.toString(16).slice(0, 6)}`;
}

function deriveForm(doc: MockDoc, name: string | null): SchemaDraft {
  const formName = name?.trim() || doc.title || doc.original_name.replace(/\.[^.]+$/, "");
  const slug = slugForName(formName);
  let fields: FieldDef[];
  if (slug === "quotation") {
    fields = [
      f("recipient", "수신", "Recipient"),
      f("date", "견적일자", "Date", "date"),
      f("brn", "사업자등록번호", "Business reg. no.", "brn"),
      f("contact_person", "담당자", "Contact person"),
      f("total", "합계금액", "Total", "amount", { required: true }),
      f("items", "견적 내역", "Items", "table", { fields: [f("item_name", "품명", "Item"), f("spec", "규격", "Spec"), f("quantity", "수량", "Qty", "integer"), f("unit_price", "단가", "Unit price", "amount"), f("amount", "금액", "Amount", "amount")] }),
      f("valid_until", "유효기간", "Valid until", "date"),
      f("note", "비고", "Note"),
    ];
  } else if (slug === "leave_request" || slug === "application_form") {
    fields = [f("name", "성명", "Name", "string", { required: true }), f("department", "부서", "Department"), f("period", "휴가기간", "Period", "date"), f("reason", "사유", "Reason"), f("signed", "서명 여부", "Signed", "boolean")];
  } else {
    fields = [f("title", "제목", "Title"), f("name", "성명", "Name"), f("date", "날짜", "Date", "date"), f("amount", "금액", "Amount", "amount")];
  }
  return { id: null, name: formName, name_en: null, doc_type: slug, source: "form", description_ko: `${formName} 양식에서 도출`, description_en: null, definition: { fields, rules: [] }, form_document_id: doc.id, note: null };
}

function nlSchema(s: PState, description: string, baseId: string | null): SchemaDraft {
  const d = description.trim();
  if (!d) throw err("invalid", "description_empty");
  const base = baseId ? s.schemas.find((x) => x.id === baseId) : undefined;
  if (baseId && !base) throw err("invalid", "schema_not_found");
  const parts = d
    .replace(/(스키마|양식|필드|문서|만들어|추가|수정|포함|있는|해줘|줘|로|를|을|과|와|및)/g, " ")
    .split(/[,，·、\/\n:：.。]|\s{2,}|\s+/)
    .map((x) => x.trim())
    .filter((x) => x.length >= 2 && x.length <= 16);
  const fields: FieldDef[] = base ? structuredClone(base.definition.fields) : [];
  const seen = new Set(fields.map((x) => x.key));
  let n = fields.length;
  let name = base?.name ?? "";
  for (const p of parts) {
    if (!name && /(서|증|표)$/.test(p)) {
      name = p;
      continue;
    }
    if (/^(품목|내역)/.test(p)) {
      const key = seen.has("items") ? `items_${n}` : "items";
      seen.add(key);
      fields.push(f(key, p, "Items", "table", { fields: [f("name", "품명", "Item"), f("quantity", "수량", "Qty", "integer"), f("amount", "금액", "Amount", "amount")] }));
      n += 1;
      continue;
    }
    const key0 = keyFromLabel(p, `field_${n + 1}`);
    let key = key0;
    let k = 2;
    while (seen.has(key)) key = `${key0}_${k++}`;
    seen.add(key);
    fields.push(f(key, p, key.replace(/_/g, " "), kindFromLabel(p), { required: /합계|성명|상호/.test(p) }));
    n += 1;
  }
  if (!name) name = "새 스키마";
  return {
    id: base?.id ?? null,
    name,
    name_en: base?.name_en ?? null,
    doc_type: base?.doc_type ?? slugForName(name),
    source: base ? base.source : "nl",
    description_ko: d.slice(0, 80),
    description_en: null,
    definition: { fields, rules: base?.definition.rules ?? [] },
    form_document_id: base?.form_document_id ?? null,
    note: base ? `NL: ${d.slice(0, 60)}` : null,
  };
}

// ---------------------------------------------------------------------------
// Review
// ---------------------------------------------------------------------------

function applyReview(s: PState, ids: number[], action: ReviewAction, note: string | null): FieldRecord[] {
  const out: FieldRecord[] = [];
  const touched = new Set<string>();
  for (const id of ids) {
    const fld = s.fields.find((x) => x.id === id);
    if (!fld) throw err("invalid", "field_not_found");
    const old = { value: fld.value, status: fld.status };
    let status: FieldStatus = "auto";
    switch (action.kind) {
      case "approve":
        status = "approved";
        fld.confidence = 1;
        break;
      case "reject":
        status = "rejected";
        break;
      case "hold":
        status = "held";
        break;
      case "reset":
        status = "auto";
        break;
      case "correct": {
        const v = action.value.trim();
        if (!v) throw err("invalid", "field_value_empty");
        status = "corrected";
        const { normalized } = normalizeValue(fld.kind, v);
        fld.value = fld.kind === "rrn" || fld.kind === "account" ? normalized : v;
        fld.normalized = normalized;
        fld.confidence = 1;
        break;
      }
    }
    fld.status = status;
    fld.reviewed_at = ["approved", "corrected", "rejected"].includes(status) ? now() : null;
    s.history.unshift({ id: s.seq++, field_id: fld.id, document_id: fld.document_id, path: fld.path, old_value: old.value, new_value: fld.value, old_status: old.status, new_status: status, changed_at: now(), note });
    touched.add(fld.document_id);
    out.push({ ...fld });
  }
  for (const id of touched) {
    const doc = loadDocs().find((x) => x.id === id);
    if (doc) {
      recomputeDoc(s, doc);
      upsertDoc(doc);
    }
  }
  docsChanged([...touched]);
  return out;
}

function reviewQueue(s: PState, filter: ReviewFilter): ReviewItem[] {
  const docs = wsDocs();
  const out: ReviewItem[] = [];
  for (const d of docs) {
    if (!d.last_run_id) continue;
    if (filter.document_id && d.id !== filter.document_id) continue;
    if (filter.doc_type && d.doc_type !== filter.doc_type) continue;
    for (const fld of s.fields.filter((x) => x.run_id === d.last_run_id && x.review_reason !== null)) {
      if (!filter.include_resolved && !["auto", "held"].includes(fld.status)) continue;
      if (filter.reason && fld.review_reason !== filter.reason) continue;
      const page = d.pages.find((p) => p.page_no === (fld.page ?? 1)) ?? d.pages[0];
      out.push({ ...fld, document_name: d.title ?? d.original_name, doc_type: d.doc_type, page_image_url: page && page.width_px > 0 ? `data:image/svg+xml;utf8,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="${page.width_px}" height="${page.height_px}"><rect width="100%" height="100%" fill="#fff"/><text x="40" y="80" font-size="40">${d.original_name} p.${page.page_no}</text></svg>`)}` : null, page_count: d.page_count });
    }
  }
  return out.slice(0, filter.limit ?? 500);
}

function parseTables(text: string, page: number): ExtractedTable[] {
  const out: ExtractedTable[] = [];
  const re = /<table>([\s\S]*?)<\/table>/gi;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    const t: ExtractedTable = { page, index: out.length, header: [], rows: [] };
    for (const row of m[1]!.split(/<\/tr>/i)) {
      const cells = Array.from(row.matchAll(/<t([hd])[^>]*>([\s\S]*?)<\/t[hd]>/gi)).map((c) => [c[1]!.toLowerCase(), c[2]!.replace(/<[^>]+>/g, "").trim()] as const);
      if (cells.length === 0) continue;
      if (cells.some(([k]) => k === "h") && t.header.length === 0 && t.rows.length === 0) t.header = cells.map(([, v]) => v);
      else t.rows.push(cells.map(([, v]) => v));
    }
    if (t.header.length > 0 || t.rows.length > 0) out.push(t);
  }
  return out;
}

function tableCsv(t: ExtractedTable): string {
  const q = (c: string) => (/[",\n]/.test(c) ? `"${c.replace(/"/g, '""')}"` : c);
  const lines = [t.header.length ? t.header.map(q).join(",") : null, ...t.rows.map((r) => r.map(q).join(","))].filter((x): x is string => x !== null);
  return `﻿${lines.join("\r\n")}\r\n`;
}

function status(s: PState): PipelineStatus {
  const docs = wsDocs();
  const settings = loadSettings();
  const pending = docs.flatMap((d) => (d.last_run_id ? s.fields.filter((x) => x.run_id === d.last_run_id && x.review_reason !== null && ["auto", "held"].includes(x.status)) : []));
  return {
    documents: docs.length,
    classified: docs.filter((d) => d.classified_at).length,
    ocr_done: docs.filter((d) => d.ocr_at).length,
    extracted: docs.filter((d) => d.extracted_at).length,
    review_pending: docs.filter((d) => d.review_status === "pending").length,
    reviewed: docs.filter((d) => d.status === "reviewed").length,
    review_fields: pending.length,
    failed: docs.filter((d) => d.pipeline_error).length,
    running: hasActiveJob("pipeline") || hasActiveJob("plan"),
    queued: persistentQueued(),
    model: visionInfo(),
    review_threshold: settings.processing.review_threshold,
    gate: gateStatus(),
  };
}

async function splitJob(docId: string, boundaries: number[]): Promise<string> {
  const doc = wsDocs().find((d) => d.id === docId);
  if (!doc) throw err("not_found", docId);
  const b = Array.from(new Set(boundaries.filter((p) => p > 1 && p <= doc.page_count))).sort((x, y) => x - y);
  if (b.length === 0) throw err("invalid", "no split boundary inside the document");
  const job = createJob("document_split", doc.original_name);
  void (async () => {
    await simulate(job.id, 2, 150, () => undefined);
    const ranges: [number, number][] = [];
    let start = 1;
    for (const p of b) {
      ranges.push([start, p - 1]);
      start = p;
    }
    ranges.push([start, doc.page_count]);
    const stem = doc.original_name.replace(/\.[^.]+$/, "");
    const ids: string[] = [];
    ranges.forEach(([a, z], i) => {
      const pages: MockPage[] = doc.pages.filter((p) => p.page_no >= a && p.page_no <= z).map((p, j) => ({ ...p, page_no: j + 1 }));
      const part: MockDoc = { ...doc, id: crypto.randomUUID().replace(/-/g, ""), rel_path: `${doc.rel_path.replace(/[^/]+$/, "")}${stem}_part${i + 1}.pdf`, original_name: `${stem}_part${i + 1}.pdf`, source_kind: "pdf", pages, page_count: pages.length, status: "rendered", imported_at: now(), updated_at: now(), classified_at: null, ocr_at: null, extracted_at: null, last_run_id: null, review_status: "none", confidence: null, pipeline_error: null };
      upsertDoc(part);
      ids.push(part.id);
    });
    doc.status = "archived";
    doc.updated_at = now();
    upsertDoc(doc);
    emit(EVENTS.documentsChanged, { kind: "imported", ids } satisfies DocumentsChanged);
    docsChanged([doc.id]);
    finishJob(job.id, { ok: true, detail: `split into ${ids.length}` });
    autoProcess(ids, null, "drop");
  })();
  return job.id;
}

async function mergeJob(ids: string[], name: string | null): Promise<string> {
  const docs = ids.map((id) => wsDocs().find((d) => d.id === id)).filter((d): d is MockDoc => !!d);
  if (docs.length < 2) throw err("invalid", "merge needs at least two documents");
  const job = createJob("document_merge", `${docs.length} documents`);
  void (async () => {
    await simulate(job.id, 2, 150, () => undefined);
    const stem = name?.trim().replace(/\.pdf$/i, "") || `${docs[0]!.original_name.replace(/\.[^.]+$/, "")}_merged`;
    const pages: MockPage[] = docs.flatMap((d) => d.pages).map((p, i) => ({ ...p, page_no: i + 1 }));
    const merged: MockDoc = { ...docs[0]!, id: crypto.randomUUID().replace(/-/g, ""), rel_path: `documents/${stem}.pdf`, original_name: `${stem}.pdf`, source_kind: "pdf", pages, page_count: pages.length, status: "rendered", imported_at: now(), updated_at: now(), classified_at: null, ocr_at: null, extracted_at: null, last_run_id: null, review_status: "none", confidence: null, pipeline_error: null, title: null };
    upsertDoc(merged);
    emit(EVENTS.documentsChanged, { kind: "imported", ids: [merged.id] } satisfies DocumentsChanged);
    finishJob(job.id, { ok: true, detail: `merged ${docs.length}` });
    autoProcess([merged.id], null, "drop");
  })();
  return job.id;
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

export async function mockPipeline(cmd: string, a: Record<string, any>): Promise<unknown> { // eslint-disable-line @typescript-eslint/no-explicit-any
  const s = load();
  switch (cmd) {
    case "pipeline_status":
      return status(s);
    case "pipeline_process":
      return processJob(a.req as ProcessRequest);
    case "pipeline_doc_types":
      return docTypes(s);
    case "pipeline_schemas":
      return [...s.schemas].sort((x, y) => y.use_count - x.use_count || Number(x.source === "builtin") - Number(y.source === "builtin") || x.created_at.localeCompare(y.created_at));
    case "pipeline_schema_get": {
      const sc = s.schemas.find((x) => x.id === a.id);
      if (!sc) throw err("invalid", "schema_not_found");
      return sc;
    }
    case "pipeline_schema_versions":
      return versionsOf(String(a.id));
    case "pipeline_schema_save": {
      const saved = saveSchema(s, a.draft as SchemaDraft);
      save(s);
      changed("schemas", [saved.id]);
      return saved;
    }
    case "pipeline_schema_delete": {
      const sc = s.schemas.find((x) => x.id === a.id);
      if (!sc) return false;
      if (sc.source === "builtin") throw err("invalid", "schema_builtin");
      s.schemas = s.schemas.filter((x) => x.id !== sc.id);
      if (sc.is_default) {
        const next = s.schemas.filter((x) => x.doc_type === sc.doc_type).sort((x, y) => Number(x.source === "builtin") - Number(y.source === "builtin"))[0];
        if (next) next.is_default = true;
      }
      for (const d of loadDocs()) if (d.schema_id === sc.id) { d.schema_id = null; upsertDoc(d); }
      save(s);
      changed("schemas", [sc.id]);
      return true;
    }
    case "pipeline_schema_set_default": {
      const sc = s.schemas.find((x) => x.id === a.id);
      if (!sc) throw err("invalid", "schema_not_found");
      for (const x of s.schemas) if (x.doc_type === sc.doc_type) x.is_default = x.id === sc.id;
      save(s);
      changed("schemas", [sc.id]);
      return undefined;
    }
    case "pipeline_schema_set_enabled": {
      const sc = s.schemas.find((x) => x.id === a.id);
      if (!sc) throw err("invalid", "schema_not_found");
      sc.enabled = !!a.enabled;
      save(s);
      changed("schemas", [sc.id]);
      return undefined;
    }
    case "pipeline_schema_lint":
      return lint(a.definition as SchemaDef);
    case "pipeline_schema_from_nl":
      return nlSchema(s, String(a.description ?? ""), (a.baseId as string | null) ?? null);
    case "pipeline_form_derive": {
      const doc = wsDocs().find((d) => d.id === a.documentId);
      if (!doc) throw err("not_found", String(a.documentId));
      // Scanned forms need the vision model; HWPX / DOCX / HWP forms are derived from their structure.
      if (visionMode() === "none" && !["hwpx", "docx", "hwp"].includes(doc.source_kind)) throw err("invalid", "no_model");
      await new Promise((res) => setTimeout(res, 150));
      return deriveForm(doc, (a.name as string | null) ?? null);
    }
    case "pipeline_form_register": {
      const draft = { ...(a.draft as SchemaDraft), source: "form" as SchemaSource };
      if (!isKey(draft.doc_type)) draft.doc_type = slugForName(draft.name);
      const saved = saveSchema(s, draft);
      if (draft.form_document_id) {
        const doc = loadDocs().find((d) => d.id === draft.form_document_id);
        if (doc) {
          doc.role = "form";
          doc.doc_type = saved.doc_type;
          doc.doc_type_source = "form";
          doc.updated_at = now();
          upsertDoc(doc);
          docsChanged([doc.id]);
        }
      }
      save(s);
      changed("schemas", [saved.id]);
      return saved;
    }
    case "pipeline_document_schema": {
      const doc = wsDocs().find((d) => d.id === a.documentId);
      if (!doc) throw err("invalid", "document_not_found");
      if (a.schemaId && !s.schemas.some((x) => x.id === a.schemaId)) throw err("invalid", "schema_not_found");
      doc.schema_id = (a.schemaId as string | null) ?? null;
      doc.updated_at = now();
      upsertDoc(doc);
      docsChanged([doc.id]);
      return undefined;
    }
    case "pipeline_runs":
      return s.runs.filter((r) => r.document_id === a.documentId);
    case "pipeline_fields": {
      const runId = (a.runId as string | null) ?? wsDocs().find((d) => d.id === a.documentId)?.last_run_id ?? null;
      return runId ? s.fields.filter((x) => x.run_id === runId) : [];
    }
    case "pipeline_field_history":
      return s.history.filter((h) => h.document_id === a.documentId);
    case "pipeline_restore": {
      const ch = s.history.find((h) => h.id === a.changeId);
      if (!ch) throw err("invalid", "change_not_found");
      const out = applyReview(s, [ch.field_id], ch.old_value ? { kind: "correct", value: ch.old_value } : { kind: "reject" }, "restored");
      save(s);
      changed("fields", [ch.document_id]);
      return out[0];
    }
    case "pipeline_revalidate":
      return revalidateDocument(String(a.documentId));
    case "pipeline_review_queue":
      return reviewQueue(s, (a.filter as ReviewFilter | null) ?? {});
    case "pipeline_review_apply": {
      const args = a.args as { field_ids: number[]; action: ReviewAction; note: string | null };
      if (!args.field_ids.length) throw err("invalid", "no fields");
      const out = applyReview(s, args.field_ids, args.action, args.note ?? null);
      save(s);
      changed("review", [...new Set(out.map((x) => x.document_id))]);
      return out;
    }
    case "pipeline_review_approve_document":
      return approveDocument(String(a.documentId), (a.reason as ReviewReason | null) ?? null);
    case "pipeline_tables":
      return documentTables(String(a.documentId));
    case "pipeline_table_csv": {
      const t = documentTables(String(a.documentId)).find((x) => x.page === a.page && x.index === a.index);
      if (!t) throw err("invalid", "table_not_found");
      return tableCsv(t);
    }
    case "pipeline_table_export":
      return undefined;
    case "pipeline_split":
      return splitJob(String(a.documentId), (a.boundaries as number[]) ?? []);
    case "pipeline_merge":
      return mergeJob((a.ids as string[]) ?? [], (a.name as string | null) ?? null);
    default:
      throw err("internal", `mock pipeline: unknown command ${cmd}`);
  }
}
