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
import { sectionsOf } from "./draft-sections";
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
  type DraftStoreOutcome,
  type DraftRequest,
  type DraftSummary,
  type DraftTable,
  type InputCut,
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
  LENGTH_TOKENS,
  type BudgetInfo,
} from "./types";

/** Saved drafts (`.doculens/drafts/<id>.json`): `{ [workspace]: Draft[] }`. */
const DRAFTS_KEY = "doculens.mock.drafts";
const GENERATOR_MODEL = "gemma4-e4b-q5km";

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
/** 역할·등급·투입 MM·단가·공급가액·비고 — the pipeline's `estimate.items[]` columns (templates.rs `staffing_columns`). */
const STAFFING_COLUMNS: DraftColumn[] = [col("role", "역할·품목"), col("grade", "등급"), col("months", "투입 MM", "amount"), col("unit_price", "단가", "amount"), col("supply_amount", "공급가액", "amount"), col("note", "비고")];
const REQUIREMENT_COLUMNS = (full: boolean): DraftColumn[] => [col("id", "ID"), col("name", "요구사항명"), col("description", "상세 설명"), col("category", "구분"), col("priority", "우선순위"), ...(full ? [col("source", "출처"), col("acceptance", "수용 기준")] : [])];
const DELIVERABLE_COLUMNS = (withResult: boolean): DraftColumn[] => [col("name", "산출물"), col("version", "버전"), col("due_date", "납기", "date"), ...(withResult ? [col("result", "검수 결과")] : []), col("note", "비고")];
const SCHEDULE_COLUMNS: DraftColumn[] = [col("phase", "단계"), col("start_date", "시작", "date"), col("end_date", "종료", "date"), col("note", "비고")];

/** Party block of the software templates (templates.rs `sw_party`): name, registration no., representative, contact, phone. */
function swParty(prefix: string, ko: string, en: string, nameRequired: boolean): TemplateField[] {
  return [
    tf(`${prefix}.name`, `${ko} 명칭`, `${en} name`, "string", nameRequired),
    tf(`${prefix}.brn`, `${ko} 사업자등록번호`, `${en} business reg. no.`, "brn"),
    tf(`${prefix}.representative`, `${ko} 대표자`, `${en} representative`),
    tf(`${prefix}.contact_person`, `${ko} 담당자`, `${en} contact person`),
    tf(`${prefix}.phone`, `${ko} 연락처`, `${en} phone`, "phone"),
  ];
}
/** Contract / estimate party block (templates.rs `party`): 상호·사업자등록번호·대표자·주소·전화. */
function party(prefix: string, ko: string, en: string, nameRequired: boolean): TemplateField[] {
  return [
    tf(`${prefix}.name`, `${ko} 상호`, `${en} name`, "string", nameRequired),
    tf(`${prefix}.brn`, `${ko} 사업자등록번호`, `${en} business reg. no.`, "brn"),
    tf(`${prefix}.representative`, `${ko} 대표자`, `${en} representative`),
    tf(`${prefix}.address`, `${ko} 주소`, `${en} address`),
    tf(`${prefix}.phone`, `${ko} 전화`, `${en} phone`, "phone"),
  ];
}

/** The software-development templates (0.3.0) + 보완요청서, in UI order; slugs equal the pipeline's document types. */
const TEMPLATES: TemplateInfo[] = [
  {
    kind: "rfp",
    name_ko: "제안요청서(RFP)",
    name_en: "Request for proposal (RFP)",
    description_ko: "발주기관의 소프트웨어 개발 제안요청서 — 사업 개요·예산·기간·요구사항·산출물·일정·평가 기준.",
    description_en: "RFP for software development: overview, budget, period, requirements, deliverables, schedule, evaluation.",
    fields: [
      tf("project_name", "사업명", "Project name", "string", true),
      ...swParty("issuer", "발주기관", "Issuer", true),
      tf("issue_date", "공고일", "Issue date", "date", true),
      tf("budget", "사업 예산", "Budget", "amount"),
      tf("period_start", "사업 기간 시작", "Period start", "date"),
      tf("period_end", "사업 기간 종료", "Period end", "date"),
      tf("submission_deadline", "제출 마감", "Submission deadline", "date"),
      tf("summary", "사업 개요", "Overview"),
      tf("requirements", "요구사항", "Requirements", "table", true, REQUIREMENT_COLUMNS(false)),
      tf("deliverables", "산출물", "Deliverables", "table", false, DELIVERABLE_COLUMNS(false)),
      tf("schedule", "일정", "Schedule", "table", false, SCHEDULE_COLUMNS),
      tf("evaluation", "평가 기준", "Evaluation criteria", "table", false, [col("item", "항목"), col("points", "배점", "amount")]),
    ],
    sections_ko: ["사업 개요", "추진 배경 및 목적", "사업 범위", "요구사항", "산출물", "추진 일정", "제안서 작성 및 평가 기준", "계약 조건"],
    sections_en: ["Overview", "Background and objectives", "Scope", "Requirements", "Deliverables", "Schedule", "Proposal and evaluation criteria", "Contract terms"],
    keywords: ["제안요청서", "제안 요청서", "rfp", "request for proposal"],
  },
  {
    kind: "proposal",
    name_ko: "제안서",
    name_en: "Proposal",
    description_ko: "RFP에 대한 제안서 — 제안사·제안 금액·수행 기간·접근 방법·투입 인력·일정·산출물.",
    description_en: "Proposal: proposer, price, period, approach, staffing, schedule and deliverables.",
    fields: [
      tf("project_name", "사업명", "Project name", "string", true),
      ...swParty("proposer", "제안사", "Proposer", true),
      tf("client_name", "발주기관", "Client"),
      tf("issue_date", "제출일", "Submission date", "date", true),
      tf("price", "제안 금액", "Proposed price", "amount", true),
      tf("period_start", "수행 기간 시작", "Period start", "date"),
      tf("period_end", "수행 기간 종료", "Period end", "date"),
      tf("duration_months", "수행 기간(개월)", "Duration (months)", "integer"),
      tf("approach", "수행 방법론 요약", "Approach"),
      tf("team", "투입 인력", "Team", "table", false, [col("role", "역할"), col("name", "성명"), col("grade", "등급"), col("months", "투입 MM", "amount")]),
      tf("schedule", "일정", "Schedule", "table", false, SCHEDULE_COLUMNS),
      tf("deliverables", "산출물", "Deliverables", "table", false, DELIVERABLE_COLUMNS(false)),
    ],
    sections_ko: ["제안 개요", "사업 이해 및 접근 방법", "수행 범위 및 방법론", "투입 인력", "추진 일정", "품질·보안 관리", "제안 가격"],
    sections_en: ["Overview", "Understanding and approach", "Scope and methodology", "Staffing", "Schedule", "Quality and security", "Price"],
    keywords: ["제안서", "proposal"],
  },
  {
    kind: "sow",
    name_ko: "과업지시서",
    name_en: "Statement of work",
    description_ko: "과업지시서(과업내용서) — 발주자·수행자, 과업 범위, 세부 과업, 산출물, 검수 기준, 단계별 납기·대금.",
    description_en: "Statement of work: parties, scope, tasks, deliverables, acceptance criteria, milestones and payments.",
    fields: [
      tf("project_name", "과업명", "Project name", "string", true),
      ...swParty("client", "발주자", "Client", true),
      ...swParty("contractor", "수행자", "Contractor", false),
      tf("issue_date", "작성일", "Issue date", "date", true),
      tf("period_start", "과업 기간 시작", "Period start", "date"),
      tf("period_end", "과업 기간 종료", "Period end", "date"),
      tf("amount", "과업 금액", "Amount", "amount"),
      tf("scope", "과업 범위", "Scope"),
      tf("acceptance_criteria", "검수 기준", "Acceptance criteria"),
      tf("tasks", "세부 과업", "Tasks", "table", true, [col("id", "번호"), col("task", "과업"), col("description", "내용"), col("deliverable", "산출물")]),
      tf("deliverables", "산출물", "Deliverables", "table", false, DELIVERABLE_COLUMNS(false)),
      tf("milestones", "단계별 납기·대금", "Milestones", "table", false, [col("phase", "단계"), col("due_date", "납기", "date"), col("ratio", "대금 비율(%)", "amount"), col("amount", "금액", "amount")]),
    ],
    sections_ko: ["과업 개요", "과업 범위", "세부 과업", "산출물 및 납품", "검수 기준", "일정 및 대금", "기타 조건"],
    sections_en: ["Overview", "Scope", "Tasks", "Deliverables", "Acceptance criteria", "Schedule and payment", "Other terms"],
    keywords: ["과업지시서", "과업 지시서", "과업내용서", "sow", "statement of work"],
  },
  {
    kind: "srs",
    name_ko: "요구사항 정의서",
    name_en: "Requirements specification",
    description_ko: "요구사항 정의서(SRS) — 버전·작성자·승인자, 기능·비기능·인터페이스 요구사항 표, 제약·가정, 수용 기준.",
    description_en: "Software requirements specification: version, author, approver, requirements table, constraints, acceptance.",
    fields: [
      tf("project_name", "사업명", "Project name", "string", true),
      tf("version", "버전", "Version", "string", true),
      tf("issue_date", "작성일", "Issue date", "date", true),
      tf("author", "작성자", "Author"),
      tf("approver", "승인자", "Approver"),
      tf("summary", "시스템 개요", "System overview"),
      tf("requirements", "요구사항", "Requirements", "table", true, REQUIREMENT_COLUMNS(true)),
    ],
    sections_ko: ["개요", "시스템 범위", "기능 요구사항", "비기능 요구사항", "인터페이스 요구사항", "제약 사항 및 가정", "수용 기준"],
    sections_en: ["Overview", "System scope", "Functional requirements", "Non-functional requirements", "Interface requirements", "Constraints and assumptions", "Acceptance criteria"],
    keywords: ["요구사항 정의서", "요구사항정의서", "요구사항 명세서", "요구사항명세서", "srs", "requirements specification", "requirements"],
  },
  {
    kind: "dev_contract",
    name_ko: "소프트웨어 개발 계약서",
    name_en: "Software development contract",
    description_ko: "소프트웨어 개발(외주·용역) 계약서 — 갑·을, 계약일·기간·금액, 대금 지급 단계, 지식재산권·하자보수·비밀유지·검수 조항.",
    description_en: "Software development contract: parties, date, period, amount, payment milestones, IP, warranty, confidentiality, acceptance.",
    fields: [
      tf("title", "계약명", "Title", "string", true),
      tf("contract_date", "계약일", "Contract date", "date", true),
      ...party("client", "발주자(갑)", "Client (party A)", true),
      ...party("contractor", "수행사(을)", "Contractor (party B)", true),
      tf("period_start", "계약기간 시작", "Period start", "date"),
      tf("period_end", "계약기간 종료", "Period end", "date"),
      tf("amount", "계약금액", "Contract amount", "amount", true),
      tf("vat_included", "부가세 포함 여부", "VAT included", "boolean"),
      tf("payment_terms", "지급 조건", "Payment terms"),
      tf("warranty_months", "하자보수 기간(개월)", "Warranty (months)", "integer"),
      tf("ip_owner", "지식재산권 귀속", "IP owner"),
      tf("confidentiality", "비밀유지 조항", "Confidentiality clause", "boolean"),
      tf("payments", "대금 지급 단계", "Payment milestones", "table", false, [col("phase", "단계"), col("ratio", "비율(%)", "amount"), col("amount", "금액", "amount"), col("due_date", "지급 시기", "date")]),
    ],
    sections_ko: ["제1조 (목적)", "제2조 (과업의 범위)", "제3조 (계약기간)", "제4조 (계약금액 및 대금 지급)", "제5조 (산출물 및 검수)", "제6조 (지식재산권)", "제7조 (하자보수)", "제8조 (비밀유지)", "제9조 (계약의 변경 및 해지)", "제10조 (분쟁 해결)"],
    sections_en: ["Article 1 (Purpose)", "Article 2 (Scope of work)", "Article 3 (Term)", "Article 4 (Contract amount and payment)", "Article 5 (Deliverables and acceptance)", "Article 6 (Intellectual property)", "Article 7 (Warranty)", "Article 8 (Confidentiality)", "Article 9 (Changes and termination)", "Article 10 (Disputes)"],
    keywords: ["개발 계약서", "개발계약서", "용역 계약서", "용역계약서", "계약서", "계약", "contract", "agreement"],
  },
  {
    kind: "wbs",
    name_ko: "WBS·일정표",
    name_en: "WBS / schedule",
    description_ko: "작업 분류 체계와 일정표 — 작업별 담당·시작·종료·진척률·산출물, 마일스톤.",
    description_en: "Work breakdown structure and schedule: tasks with owner, dates, progress and deliverable; milestones.",
    fields: [
      tf("project_name", "사업명", "Project name", "string", true),
      tf("version", "버전", "Version"),
      tf("issue_date", "작성일", "Issue date", "date", true),
      tf("period_start", "전체 시작", "Period start", "date"),
      tf("period_end", "전체 종료", "Period end", "date"),
      tf("tasks", "작업", "Tasks", "table", true, [col("id", "WBS 번호"), col("task", "작업"), col("owner", "담당"), col("start_date", "시작", "date"), col("end_date", "종료", "date"), col("progress", "진척률(%)", "amount"), col("deliverable", "산출물")]),
      tf("milestones", "마일스톤", "Milestones", "table", false, [col("name", "마일스톤"), col("date", "일자", "date")]),
    ],
    sections_ko: ["개요", "작업 분류 체계", "마일스톤"],
    sections_en: ["Overview", "Work breakdown", "Milestones"],
    keywords: ["wbs", "일정표", "작업분류체계", "작업 분류 체계", "work breakdown"],
  },
  {
    kind: "progress_report",
    name_ko: "진척 보고서",
    name_en: "Progress report",
    description_ko: "주간·월간·단계 진척 보고서 — 보고 기간, 전체·계획 진척률, 작업별 실적, 이슈·리스크, 차기 계획.",
    description_en: "Weekly / monthly progress report: period, overall vs planned progress, task results, issues, next plan.",
    fields: [
      tf("project_name", "사업명", "Project name", "string", true),
      tf("report_type", "보고 구분", "Report type"),
      tf("period_start", "보고 기간 시작", "Period start", "date"),
      tf("period_end", "보고 기간 종료", "Period end", "date"),
      tf("report_date", "보고일", "Report date", "date", true),
      tf("author", "작성자", "Author"),
      tf("progress_percent", "전체 진척률(%)", "Overall progress (%)", "amount"),
      tf("plan_percent", "계획 진척률(%)", "Planned progress (%)", "amount"),
      tf("summary", "주요 실적", "Summary"),
      tf("next_plan", "차기 계획", "Next plan"),
      tf("tasks", "작업별 실적", "Task results", "table", false, [col("task", "작업"), col("owner", "담당"), col("planned", "계획"), col("actual", "실적"), col("progress", "진척률(%)", "amount")]),
      tf("issues", "이슈·리스크", "Issues / risks", "table", false, [col("issue", "이슈"), col("impact", "영향도"), col("action", "조치"), col("owner", "담당"), col("due_date", "기한", "date")]),
    ],
    sections_ko: ["보고 개요", "금주 실적", "차주 계획", "이슈 및 리스크", "지원 요청 사항"],
    sections_en: ["Overview", "Results this period", "Plan for next period", "Issues and risks", "Support needed"],
    keywords: ["진척 보고서", "진척보고서", "진척 보고", "주간 보고서", "주간보고서", "주간 보고", "월간 보고서", "월간보고서", "월간 보고", "진도 보고", "progress report", "weekly report", "status report"],
  },
  {
    kind: "meeting_minutes",
    name_ko: "회의록",
    name_en: "Meeting minutes",
    description_ko: "회의록 — 일시·장소·참석자 표, 안건·논의 내용, 결정 사항 표, 실행 항목(담당·기한), 차기 회의.",
    description_en: "Meeting minutes: date, place, attendees, agenda, discussion, decisions, action items, next meeting.",
    fields: [
      tf("title", "회의명", "Title", "string", true),
      tf("project_name", "사업명", "Project name"),
      tf("meeting_date", "일시", "Date", "date", true),
      tf("location", "장소", "Location"),
      tf("author", "작성자", "Author"),
      tf("next_meeting", "차기 회의", "Next meeting", "date"),
      tf("attendees", "참석자", "Attendees", "table", true, [col("name", "성명"), col("organization", "소속"), col("role", "역할")]),
      tf("decisions", "결정 사항", "Decisions", "table", false, [col("item", "항목"), col("detail", "내용")]),
      tf("action_items", "실행 항목", "Action items", "table", false, [col("item", "항목"), col("owner", "담당"), col("due_date", "기한", "date")]),
    ],
    sections_ko: ["안건", "논의 내용", "결정 사항", "실행 항목"],
    sections_en: ["Agenda", "Discussion", "Decisions", "Action items"],
    keywords: ["회의록", "회의 록", "meeting minutes", "minutes"],
  },
  {
    kind: "test_report",
    name_ko: "테스트 결과서",
    name_en: "Test report",
    description_ko: "테스트 결과서 — 대상 버전·일자·구분·환경, 케이스 집계(총·통과·실패·보류·통과율), 케이스별 결과, 결함 목록.",
    description_en: "Test report: version, date, type, environment, case totals and pass rate, case results, defect list.",
    fields: [
      tf("project_name", "사업명", "Project name", "string", true),
      tf("version", "대상 버전", "Version under test"),
      tf("test_date", "테스트 일자", "Test date", "date", true),
      tf("tester", "테스터", "Tester"),
      tf("test_type", "테스트 구분", "Test type"),
      tf("environment", "테스트 환경", "Environment"),
      tf("total_cases", "총 케이스", "Total cases", "integer"),
      tf("passed", "통과", "Passed", "integer"),
      tf("failed", "실패", "Failed", "integer"),
      tf("blocked", "보류", "Blocked", "integer"),
      tf("pass_rate", "통과율(%)", "Pass rate (%)", "amount"),
      tf("cases", "케이스", "Cases", "table", true, [col("id", "ID"), col("scenario", "시나리오"), col("expected", "기대 결과"), col("result", "결과"), col("defect_id", "결함 ID")]),
      tf("defects", "결함", "Defects", "table", false, [col("id", "결함 ID"), col("severity", "심각도"), col("description", "내용"), col("status", "상태")]),
    ],
    sections_ko: ["테스트 개요", "테스트 범위 및 환경", "결과 요약", "결함 목록", "결론 및 권고"],
    sections_en: ["Overview", "Scope and environment", "Summary of results", "Defects", "Conclusion"],
    keywords: ["테스트 결과서", "테스트결과서", "시험 결과서", "시험결과서", "테스트 보고서", "test report", "test results"],
  },
  {
    kind: "change_request",
    name_ko: "변경요청서",
    name_en: "Change request",
    description_ko: "변경요청서(CR) — 번호·요청자·일자, 변경 내용·사유, 일정·비용 영향, 우선순위, 승인 상태·승인자.",
    description_en: "Change request: number, requester, date, change and reason, schedule / cost impact, priority, approval.",
    fields: [
      tf("cr_no", "CR 번호", "CR no.", "string", true),
      tf("project_name", "사업명", "Project name"),
      tf("request_date", "요청일", "Request date", "date", true),
      tf("requester", "요청자", "Requester"),
      tf("title", "변경 제목", "Title", "string", true),
      tf("description", "변경 내용", "Description"),
      tf("reason", "변경 사유", "Reason"),
      tf("impact_schedule", "일정 영향", "Schedule impact"),
      tf("impact_cost", "비용 영향", "Cost impact", "amount"),
      tf("priority", "우선순위", "Priority"),
      tf("status", "승인 상태", "Status"),
      tf("approver", "승인자", "Approver"),
      tf("approval_date", "승인일", "Approval date", "date"),
    ],
    sections_ko: ["변경 개요", "변경 사유", "영향 분석", "승인"],
    sections_en: ["Change overview", "Reason", "Impact analysis", "Approval"],
    keywords: ["변경요청서", "변경 요청서", "변경요청", "change request"],
  },
  {
    kind: "acceptance",
    name_ko: "검수확인서",
    name_en: "Acceptance certificate",
    description_ko: "검수확인서·완료보고서 — 발주자·수행사, 검수일, 산출물별 검수 결과, 검수 금액, 최종 판정, 확인.",
    description_en: "Acceptance certificate: parties, acceptance date, per-deliverable results, amount, verdict, confirmation.",
    fields: [
      tf("project_name", "사업명", "Project name", "string", true),
      ...swParty("client", "발주자", "Client", true),
      ...swParty("contractor", "수행사", "Contractor", true),
      tf("acceptance_date", "검수일", "Acceptance date", "date", true),
      tf("period_start", "사업 기간 시작", "Period start", "date"),
      tf("period_end", "사업 기간 종료", "Period end", "date"),
      tf("amount", "검수 금액", "Amount", "amount"),
      tf("result", "최종 판정", "Verdict"),
      tf("conditions", "조건·특이사항", "Conditions / remarks"),
      tf("deliverables", "산출물", "Deliverables", "table", true, DELIVERABLE_COLUMNS(true)),
    ],
    sections_ko: ["사업 개요", "산출물 검수 결과", "특이사항", "확인"],
    sections_en: ["Overview", "Deliverable inspection", "Remarks", "Confirmation"],
    keywords: ["검수확인서", "검수 확인서", "검수조서", "완료보고서", "완료 보고서", "검수", "acceptance certificate", "acceptance"],
  },
  {
    kind: "estimate",
    name_ko: "견적서",
    name_en: "Estimate",
    description_ko: "소프트웨어 개발 견적서(인력·MM 기준) — 공급자·수신·유효기간, 역할별 등급·투입 MM·단가·공급가액, 합계·세액.",
    description_en: "Software development estimate (person-month based): supplier, recipient, validity, per-role staffing lines, totals.",
    fields: [
      tf("document_no", "견적번호", "Estimate no."),
      tf("issue_date", "견적일", "Issue date", "date", true),
      tf("project_name", "사업명", "Project name"),
      tf("recipient", "수신", "Recipient", "string", true),
      tf("valid_until", "유효기간", "Valid until", "date"),
      ...party("supplier", "공급자", "Supplier", true),
      tf("contact_person", "담당자", "Contact person"),
      tf("supply_total", "공급가액 합계", "Supply total", "amount"),
      tf("tax_total", "세액 합계", "Tax total", "amount"),
      tf("grand_total", "합계금액", "Grand total", "amount", true),
      tf("payment_terms", "결제조건", "Payment terms"),
      tf("note", "비고", "Note"),
      tf("items", "견적 내역", "Items", "table", true, STAFFING_COLUMNS),
    ],
    sections_ko: ["견적 조건", "비고"],
    sections_en: ["Terms", "Notes"],
    keywords: ["견적서", "견적", "estimate", "quotation", "quote"],
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

/** The generic templates of 0.1 (hidden since 0.3.0): not listed, not keyword-matched from free text; explicit refs and old drafts still work. */
const LEGACY_TEMPLATES: TemplateInfo[] = [
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
];
const ALL_TEMPLATES: TemplateInfo[] = [...TEMPLATES, ...LEGACY_TEMPLATES];

export function listTemplates(): TemplateInfo[] {
  return TEMPLATES;
}

/** Budgets above this are drafted section by section (generate/prompts.rs). */
const SPLIT_THRESHOLD_TOKENS = 3000;
/** Reply token budget of a request (explicit, else the length preset). */
function replyBudget(req: DraftRequest): number {
  return req.max_tokens && req.max_tokens > 0 ? req.max_tokens : LENGTH_TOKENS[req.length];
}

export function templateInfo(kind: TemplateKind): TemplateInfo {
  return ALL_TEMPLATES.find((t) => t.kind === kind) ?? ALL_TEMPLATES.find((t) => t.kind === "report")!;
}

/** Template whose keyword occurs in the compacted text; the longest keyword wins ("개발 계약서" over "계약서"). */
function matchKeywords(text: string, pool: TemplateInfo[]): TemplateKind | null {
  let best: { len: number; kind: TemplateKind } | null = null;
  for (const t of pool) {
    for (const k of t.keywords) {
      const kc = compact(k);
      if (kc && text.includes(kc) && (!best || kc.length > best.len)) best = { len: kc.length, kind: t.kind };
    }
  }
  return best?.kind ?? null;
}

// ---------------------------------------------------------------------------
// Template resolution (generate/draft.rs `resolve_template`)
// ---------------------------------------------------------------------------

/** Registered form / schema reference (id, name, doc-type slug) → `form` template, else a built-in template. */
export function resolveTemplate(req: DraftRequest): TemplateRef {
  const ref = (req.form_ref ?? "").trim();
  if (ref) {
    const key = compact(ref);
    const builtin = ALL_TEMPLATES.find((t) => t.kind === ref || compact(t.name_ko) === key || compact(t.name_en) === key) ?? null;
    const schema = pl.listSchemas().find((s) => s.id === ref || compact(s.name) === key || compact(s.name_en ?? "") === key || compact(s.doc_type) === key);
    // A registered form wins over a built-in template of the same name (the user registered it on purpose).
    if (schema && schema.source === "form") return { kind: "form", schema_id: schema.id, doc_id: schema.form_document_id };
    if (builtin) return { kind: "builtin", template: builtin.kind };
    if (schema) return { kind: "form", schema_id: schema.id, doc_id: schema.form_document_id };
    // "A4 공문 형식": an explicit format reference may still name a hidden generic template.
    const byKeyword = matchKeywords(key, ALL_TEMPLATES);
    if (byKeyword) return { kind: "builtin", template: byKeyword };
  }
  const text = compact(`${req.format_description ?? ""} ${req.content}`);
  const matched = matchKeywords(text, TEMPLATES);
  if (matched) return { kind: "builtin", template: matched };
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
  items: { name: string; grade: string | null; quantity: number; unitPrice: number | null }[];
  amount: number | null;
  validityDays: number | null;
  title: string | null;
  /** "한빛 포털 구축 사업" — the project / business name a software document is about. */
  projectName: string | null;
}

const RECIPIENT_RE = /((?:\(주\)|㈜|주식회사\s*)?[가-힣A-Za-z0-9·&]+(?:\(주\))?)\s*(?:에게|에|앞|측|귀하|귀중)(?=[\s,.]|$)/u;

/** Pulls the facts the sentence states (party, items, unit price, validity) out of the request text. */
export function contentHints(content: string): ContentHints {
  const text = content.replace(/\s+/g, " ").trim();
  const recipient = text.match(RECIPIENT_RE)?.[1]?.replace(/^(에|에게)$/, "") ?? null;
  const items: ContentHints["items"] = [];
  // Goods ("노트북 10대") or staffing lines of a software estimate ("백엔드개발 고급 3MM").
  const itemRe = /([가-힣A-Za-z][가-힣A-Za-z0-9·\-]*)\s*(?:(특급|고급|중급|초급)\s*)?(\d+(?:\.\d+)?)\s*(대|개|EA|ea|set|세트|건|식|권|박스|box|MM|mm|M\/M|인월|명)(?![가-힣A-Za-z])/gu;
  let m: RegExpExecArray | null;
  while ((m = itemRe.exec(text))) {
    const name = m[1]!;
    if (/^(유효기간|기한|합계|총|대당|개당|단가)$/.test(name)) continue;
    items.push({ name, grade: m[2] ?? null, quantity: Number(m[3]), unitPrice: null });
  }
  const unit = text.match(/(?:대당|개당|단가|unit price)\s*([\d,]+)\s*원?/iu);
  const unitPrice = unit ? parseAmount(unit[1]) : null;
  if (unitPrice !== null && items.length > 0) for (const it of items) if (it.unitPrice === null) it.unitPrice = unitPrice;
  const amountMatch = text.match(/(?:총액|합계|총|금액|계약금액|계약 금액)\s*([\d,]{4,})\s*원?/u) ?? text.match(/([\d,]{5,})\s*원/u);
  const amount = amountMatch ? parseAmount(amountMatch[1]) : null;
  const validity = text.match(/유효(?:기간)?\s*(\d+)\s*일/u);
  const quoted = text.match(/[“"']([^”"']{2,60})[”"']/u);
  const project = text.match(/((?:[가-힣A-Za-z0-9·]+\s){0,3}[가-힣A-Za-z0-9·]*(?:구축|고도화|사업|프로젝트|시스템))(?![가-힣])/u)?.[1]?.trim() ?? null;
  return { recipient, items, amount, validityDays: validity ? Number(validity[1]) : null, title: quoted?.[1] ?? null, projectName: project && Array.from(project).length >= 4 ? project : null };
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
      return `본 견적은 부가가치세 별도이며, 투입 인력·기간과 유효기간은 상기와 같${ending === "임." ? "음." : "습니다."}`;
    case "사업 개요":
    case "과업 개요":
    case "제안 개요":
    case "보고 개요":
    case "테스트 개요":
    case "변경 개요":
      return `본 문서는 ${subject}에 관한 ${section}를 정리한 것${ending === "임." ? "임." : "입니다."}${cite}`;
    case "실행 항목":
      return `담당자별 실행 항목과 기한은 아래 표와 같${ending === "임." ? "음." : "습니다."}`;
    case "비고":
      return `특이 사항 없${ending === "임." ? "음." : "습니다."}`;
    case "결론 및 권고":
      return `테스트 결과를 바탕으로 잔여 결함을 수정한 뒤 배포할 것을 권고${ending}`;
    case "승인":
      return `위 변경 요청을 검토한 결과 승인${ending}`;
    case "확인":
      return `상기 산출물을 검수하였음을 확인${ending}`;
    case "제3조 (계약기간)":
      return `계약기간은 상기 시작일부터 종료일까지로 ${ending === "임." ? "함." : "합니다."}`;
    case "제4조 (계약금액 및 대금 지급)":
      return `계약금액은 상기 금액으로 하며 대금 지급 단계에 따라 지급${ending}`;
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
    (template.kind === "builtin" && (template.template === "quotation" || template.template === "estimate") && hints.recipient ? `${hints.recipient} ${layout.name}` : null) ??
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
              case "role":
                return it.name;
              case "grade":
                return it.grade ?? "";
              case "quantity":
              case "months":
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
      case "supplier.name":
      case "contractor.name":
      case "proposer.name":
        value = factValue(["supplier.name", "contractor.name", "proposer.name"]) ?? (ko ? "DocuLens" : "DocuLens");
        if (factValue(["supplier.name", "contractor.name", "proposer.name"]) && firstFact) source = firstFact.citation;
        break;
      case "supplier.brn":
      case "contractor.brn":
      case "proposer.brn":
        value = factValue(["supplier.brn", "contractor.brn", "proposer.brn", "registration_no", "brn"]) ?? "";
        if (value && firstFact) source = firstFact.citation;
        break;
      case "client.name":
      case "client_name":
      case "issuer.name":
        value = hints.recipient ?? factValue(["client.name", "issuer.name", "buyer.name"]) ?? (f.key === "issuer.name" ? "DocuLens" : "");
        if (!hints.recipient && value && firstFact && factValue(["client.name", "issuer.name", "buyer.name"])) source = firstFact.citation;
        break;
      case "version":
        value = factValue(["version"]) ?? "1.0";
        break;
      case "cr_no":
        value = `CR-${today().replace(/-/g, "")}-01`;
        break;
      case "report_type":
        value = ko ? "주간" : "Weekly";
        break;
      case "requester":
      case "tester":
      case "approver":
        value = ko ? "담당자" : "Owner";
        break;
      case "location":
        value = ko ? "회의실 A" : "Room A";
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
      case "issue_date":
      case "report_date":
      case "test_date":
      case "acceptance_date":
      case "request_date":
      case "meeting_date":
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
        value = title;
        break;
      case "project_name":
        value = hints.projectName ?? factValue(["project_name"]) ?? title;
        break;
      case "total":
      case "amount":
      case "price":
      case "budget":
      case "grand_total": {
        const items = tables.find((t) => t.key === "items");
        const sum = items ? sumOfColumn(items, ["amount", "supply_amount"]) : null;
        if (feedbackAmount) value = formatAmount(parseAmount(feedbackAmount[1]) ?? 0);
        else if (sum !== null && sum > 0) {
          // The simulated model slips on the total: with separate 공급가액/세액 fields it forgets the VAT,
          // otherwise it adds VAT to a VAT-exclusive total — the validator points either mismatch out.
          const separateTax = layout.fields.some((x) => x.key === "tax_total");
          value = formatAmount(separateTax ? sum : Math.round(sum * 1.1));
        } else if (hints.amount !== null) value = formatAmount(hints.amount);
        else value = factValue(["grand_total", "total", "amount", "price", "budget"]) ?? "";
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
        const supply = items ? sumOfColumn(items, ["amount", "supply_amount"]) : null;
        // No tax column (staffing lines): 10 % of the supply sum, as the core's validator fills it.
        value = sum !== null && sum > 0 ? formatAmount(sum) : supply !== null && supply > 0 ? formatAmount(Math.round(supply * 0.1)) : (factValue(["tax_total"]) ?? "");
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
  // A large token budget (0.3.0) is produced section by section: every template section, more text per section.
  const budget = replyBudget(req);
  const split = budget > SPLIT_THRESHOLD_TOKENS;
  const sectionCount = split ? layout.sections.length : length === "short" ? Math.min(2, layout.sections.length) : layout.sections.length;
  const sections = layout.sections.slice(0, sectionCount);
  const mainTable = tables.find((t) => t.rows.length > 0);
  for (const [i, section] of sections.entries()) {
    body.push({ kind: "heading", level: 2, text: section });
    const cite = i === 0 && refs.length > 0 ? refs[0]! : null;
    body.push({ kind: "paragraph", text: sentenceFor(section, req.content, ko, tone, cite), source: cite });
    if ((length === "long" || split) && i === 0) body.push({ kind: "list", items: ko ? ["요청 배경 및 목적", "주요 내용", "기대 효과"] : ["Background", "Main points", "Expected outcome"], ordered: false });
    if (split && budget >= 12000) body.push({ kind: "paragraph", text: ko ? `${section}의 세부 사항은 관련 자료와 협의 결과를 반영해 정리${tone === "concise" ? "함." : "합니다."}` : `${section}: details follow the sources and the agreed scope.`, source: null });
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
  if (split) draft.notes.push(ko ? `토큰 예산 ${budget.toLocaleString("ko-KR")}로 ${sections.length}개 절을 나눠 생성했습니다.` : `Generated in ${sections.length} sections with a budget of ${budget.toLocaleString("en-US")} tokens.`);
  validateDraft(draft);
  return draft;
}

/**
 * 0.7.0 초안 어시스턴트 (`generate::revise`): `req.revise` names a section to rewrite, a place for a new
 * section or the whole draft; `req.previous` is the draft, `req.feedback` the instruction. Deterministic
 * like `buildDraft`: a rewritten section keeps its heading and tables and gets paragraphs that quote the
 * instruction; a new section is a level-2 heading plus one paragraph; the whole-draft scope is the
 * ordinary regeneration. The validator re-runs, so the positional `body[i]` issues are fresh.
 */
export function reviseDraft(req: DraftRequest): Draft {
  const previous = req.previous;
  const scope = req.revise;
  if (!previous || !scope) throw err("invalid", "revise_previous_missing");
  const ko = req.language !== "en";
  const instruction = (req.feedback ?? "").trim();
  if (scope.scope === "whole") {
    const base: DraftRequest = { ...req, content: req.content.trim() || previous.request?.content || previous.title, revise: null };
    return buildDraft(base, []);
  }
  const draft = structuredClone(previous);
  const sections = sectionsOf(draft.body);
  const ending = draft.tone === "concise" ? "함." : "합니다.";
  const quote = instruction ? (ko ? ` (지시 반영: ${instruction})` : ` (applied: ${instruction})`) : "";
  let note: string;
  const target = scope.scope === "section" ? (sections.find((s) => s.ordinal === scope.section) ?? null) : null;
  if (scope.scope === "section" && target) {
    const heading = target.heading;
    const subject = heading ?? draft.title;
    const blocks: DraftBlock[] = [];
    if (heading !== null) blocks.push(draft.body[target.start]!);
    blocks.push({ kind: "paragraph", text: ko ? `${subject}에 대한 내용을 지시에 따라 다시 정리${ending}${quote}` : `${subject}, rewritten as instructed${quote}.`, source: null });
    if (/자세히|구체적|detail|길게|longer|expand/iu.test(instruction)) {
      blocks.push({ kind: "paragraph", text: ko ? `세부 사항: 배경과 범위, 일정, 기대 효과를 항목별로 설명${ending}` : "Details: background and scope, schedule and the expected outcome.", source: null });
      blocks.push({ kind: "list", items: ko ? ["배경·범위", "일정", "기대 효과"] : ["Background and scope", "Schedule", "Expected outcome"], ordered: false });
    }
    // Tables placed in the section stay where they were.
    for (const b of draft.body.slice(target.start, target.end)) if (b.kind === "table_ref") blocks.push(b);
    draft.body.splice(target.start, target.end - target.start, ...blocks);
    note = ko ? `AI 수정(${target.ordinal}절 '${heading ?? "제목 없음"}'): ${instruction}` : `AI revision (section ${target.ordinal} '${heading ?? "untitled"}'): ${instruction}`;
  } else {
    // A new section after the named one (or at the end); a `section` scope without that section appends too.
    const afterOrdinal = scope.scope === "insert_after" ? scope.section : scope.section;
    const after = afterOrdinal == null ? null : (sections.find((s) => s.ordinal === afterOrdinal) ?? null);
    const at = after ? after.end : draft.body.length;
    const heading = (scope.scope === "insert_after" ? (scope.heading ?? "") : "").trim() || (ko ? "새 절" : "New section");
    draft.body.splice(at, 0, { kind: "heading", level: 2, text: heading }, { kind: "paragraph", text: ko ? `${heading}: ${instruction || "내용을 정리"}${instruction ? "" : ending}${instruction ? `에 따라 작성${ending}` : ""}` : `${heading}: ${instruction || "content"}${instruction ? ", written as instructed." : "."}`, source: null });
    note = ko ? `AI 수정(새 절 '${heading}'${after ? ` — ${after.ordinal}절 뒤` : " — 끝"}): ${instruction}` : `AI revision (new section '${heading}'${after ? ` after section ${after.ordinal}` : " at the end"}): ${instruction}`;
  }
  draft.notes = [...draft.notes, note];
  draft.model = GENERATOR_MODEL;
  draft.request = { ...req, previous: null, revise: null, feedback: null };
  draft.updated_at = now();
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
    const qi = table.columns.findIndex((c) => c.key === "quantity" || c.key === "months");
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
  const d = findDraft(id);
  // `store::load`: `CoreError::NotFound("draft <id>")`.
  if (!d) throw err("not_found", `not found: draft ${id}`);
  return d;
}

/** The stored draft or null (plan previews of `revise_draft`). */
export function findDraft(id: string): Draft | null {
  return wsDrafts().find((x) => x.id === id) ?? null;
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
  const filled_form = draft.template.kind === "form" && (req.output === "hwpx" || req.output === "docx");
  // `render::save`: the body of a draft does not go into a filled form; `fill::body_blocks` counts what was left out.
  const omitted_body_blocks = filled_form ? draft.body.filter((b) => (b.kind === "heading" || b.kind === "paragraph" ? b.text.trim() !== "" : b.kind === "list" ? b.items.some((i) => i.trim() !== "") : false)).length : 0;
  if (omitted_body_blocks > 0) {
    warnings.push(
      ko
        ? `등록 양식에는 필드와 표만 채워집니다. 본문 ${omitted_body_blocks}개 블록은 저장한 파일에 들어가지 않았습니다. 본문까지 담으려면 PDF나 Markdown으로 저장하세요.`
        : `A registered form takes fields and tables only: ${omitted_body_blocks} body block(s) were left out of the saved file. Save as PDF or Markdown to keep the body.`,
    );
  }
  return { rel_path: rel, absolute: `${ws}${sep}${rel.split("/").join(sep)}`, format: req.output, doc_id: docId, filled_form, warnings, omitted_body_blocks };
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
  /** `store::JobInput.existed`: the draft the job starts from was in the draft store when the job was created. */
  existed?: boolean;
}

/** `prompts::MAX_CONTENT_CHARS` / `MAX_FEEDBACK_CHARS`: what the model is given of the content / the instruction at most. */
const MAX_CONTENT_CHARS = 6_000;
const MAX_FEEDBACK_CHARS = 2_000;

/** `draft::InputBudget::report`: the request's own text the model saw only in part, with a note on the draft. */
function reportInputCuts(req: DraftRequest, draft: Draft): InputCut[] {
  const ko = req.language !== "en";
  const cuts: InputCut[] = [];
  const check = (field: InputCut["field"], text: string | null | undefined, max: number, labelKo: string, labelEn: string) => {
    const chars = Array.from(text ?? "").length;
    if (chars <= max) return;
    cuts.push({ field, chars, passed: max });
    draft.notes.push(ko ? `${labelKo} ${chars.toLocaleString("ko-KR")}자 중 앞 ${max.toLocaleString("ko-KR")}자만 모델에 전달했습니다.` : `Only the first ${max.toLocaleString("en-US")} of the ${chars.toLocaleString("en-US")} characters of the ${labelEn} were passed to the model.`);
  };
  check("content", req.content, MAX_CONTENT_CHARS, "내용", "content");
  check("feedback", req.feedback, MAX_FEEDBACK_CHARS, "지시문", "instruction");
  return cuts;
}

/** `store::same_content`: title, field values, table rows and body — timestamps, notes and issues do not count. */
function sameDraftContent(a: Draft, b: Draft): boolean {
  const view = (d: Draft) => JSON.stringify([d.title, d.body, d.fields.map((f) => [f.key, f.value]), d.tables.map((t) => [t.key, t.rows])]);
  return view(a) === view(b);
}

/** What the job captured when it was created (`store::JobInput`). */
interface JobInput {
  captured_at: string;
  existed: boolean;
}

/**
 * `store::save_job_result`: the result of a draft job replaces the stored draft it started from only when that
 * loses nothing. When the stored draft was changed after the job took its input the result becomes a new draft
 * (new id, a note, no export path); when it was deleted meanwhile nothing is stored. (The mock has no partial
 * result of a cancelled job, so the `cancelled` copy does not occur here.)
 */
function storeJobResult(draft: Draft, previous: Draft | null, input: JobInput | null, ko: boolean): DraftStoreOutcome {
  const started = previous && previous.id === draft.id ? previous : null;
  if (started && input) {
    const stored = findDraft(started.id);
    if (input.existed && !stored) return { status: "not_saved", reason: "deleted" };
    if (stored && stored.updated_at > input.captured_at && !sameDraftContent(stored, started)) {
      const original = draft.id;
      draft.id = newId();
      draft.created_at = now();
      draft.saved_path = null;
      draft.saved_doc_id = null;
      draft.notes.unshift(ko ? "작업이 시작된 뒤 원래 초안이 수정되어, 덮어쓰지 않고 새 초안으로 저장했습니다." : "The original draft was edited after this job started, so the result was stored as a new draft.");
      putDraft(draft);
      return { status: "copy", original, reason: "newer" };
    }
  }
  putDraft(draft);
  return { status: "saved" };
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
  // `generate_bridge::draft`: the job's project and the request's are bound to each other (`DraftRequest::bind_project`).
  const project_id = origin.project_id?.trim() || req.project_id?.trim() || null;
  const bound: DraftRequest = { ...req, project_id: req.project_id?.trim() || project_id };
  const existed = !!req.previous && !!findDraft(req.previous.id);
  return submitJob({ kind: "draft", req: bound, prepared, label, project_id, source: origin.source ?? "generate", existed } satisfies DraftJobSpec, templateName(req, ko), "normal");
}

/**
 * Runs one draft (the executor's `draft_document` step and the draft job share it). The stage words are the
 * shell's (`generate_bridge::run_draft`): "loading model" (model drafts only), then "drafting".
 */
export async function draftNow(req: DraftRequest, ctl: JobControl | null, progress: (stage: string, frac: number) => void, prepared: Draft | null = null, input: JobInput | null = null): Promise<DraftOutcome> {
  const ko = req.language !== "en";
  const started = performance.now();
  if (prepared) {
    progress("drafting", 0.2);
    await pace(ctl, 200);
    const draft = structuredClone(prepared);
    // The job's project is bound to the request the draft keeps, so later revisions stay inside it.
    if (draft.request && req.project_id) draft.request.project_id = req.project_id;
    validateDraft(draft);
    putDraft(draft);
    return { draft, prompt_tokens: 0, completion_tokens: 0, elapsed_ms: Math.round(performance.now() - started), source: "rules" };
  }
  if (!req.content.trim() && !req.previous) throw err("invalid", "content_empty");
  progress("loading model", 0.05);
  const mode = pl.visionAvailability();
  if (mode === "engine_off") throw err("engine_not_running", "engine_not_running");
  if (mode === "none") throw err("invalid", "no_model");
  await pace(ctl, 240);
  pl.markVisionLoaded();
  // 0.7.0 초안 어시스턴트: one model call on the section (or the whole draft), progress stage "revising".
  if (req.revise && req.previous) {
    progress("revising", 0.3);
    await pace(ctl, 500);
    const draft = reviseDraft(req);
    progress("revising", 0.95);
    await pace(ctl, 150);
    const input_cuts = reportInputCuts(req, draft);
    const draft_store = storeJobResult(draft, req.previous, input, ko);
    const chars = Array.from(req.feedback ?? "").length;
    return { draft, prompt_tokens: 1800 + chars, completion_tokens: 120 + chars, elapsed_ms: Math.round(performance.now() - started), source: "model", sections: 1, ...(input_cuts.length > 0 ? { input_cuts } : {}), ...(input ? { draft_store } : {}) };
  }
  progress("drafting", 0.2);
  const knowledge = req.use_knowledge ? await knowledgeCitations(req.content.slice(0, 60), req.excluded_sources) : [];
  await pace(ctl, 400);
  // Split drafts (budget above the threshold) report section progress like `generate::draft_split`.
  const split = replyBudget(req) > SPLIT_THRESHOLD_TOKENS;
  let sections = 1;
  if (split) {
    sections = Math.max(2, layoutFields(resolveTemplate(req)).sections.length || 5);
    for (let i = 1; i <= sections; i += 1) {
      progress(`section ${i}/${sections}`, 0.25 + (0.7 * (i - 1)) / sections);
      await pace(ctl, 200);
    }
  } else {
    progress("drafting", 0.6);
    await pace(ctl, 400);
  }
  const draft = buildDraft(req, knowledge);
  progress(split ? "finishing" : "drafting", 0.95);
  await pace(ctl, 150);
  const input_cuts = reportInputCuts(req, draft);
  const draft_store = storeJobResult(draft, req.previous, input, ko);
  const chars = Array.from(req.content).length;
  return { draft, prompt_tokens: (2400 + chars) * sections, completion_tokens: 350 + draft.body.length * 40, elapsed_ms: Math.round(performance.now() - started), source: "model", sections, ...(input_cuts.length > 0 ? { input_cuts } : {}), ...(input ? { draft_store } : {}) };
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
      { captured_at: job.created_at, existed: spec.existed ?? false },
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
    case "generate_budget_info":
      // The mock engine is a GPU build with an 8K context; the cap follows the default settings.
      return { ctx: 8192, gpu: true, cap: 32768, split_threshold: SPLIT_THRESHOLD_TOKENS, content_max_chars: MAX_CONTENT_CHARS, instruction_max_chars: MAX_FEEDBACK_CHARS } satisfies BudgetInfo;
    case "generate_draft": {
      const req = a.req as DraftRequest | undefined;
      // Like `generate_bridge::draft`: an empty content is fine when a previous draft is revised / regenerated.
      if (!req || (!req.content?.trim() && !req.previous)) throw err("invalid", "content_empty");
      if (req.revise && !req.previous) throw err("invalid", "revise_previous_missing");
      if (req.revise && !(req.feedback ?? "").trim()) throw err("invalid", "revise_instruction_empty");
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
