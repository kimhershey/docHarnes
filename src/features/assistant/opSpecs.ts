// TypeScript mirror of `crates/doculens-core/src/planner/schema.rs` (`OP_SPECS`):
// the parameter table of every planner op (appendix D) with the Korean /
// English descriptions the inline plan editor shows. The Rust table is the
// single source of truth for the validator; keep this file in step with it.

import type { Language, OpKind } from "@/lib/types";

export type ParamType =
  | { kind: "string" }
  | { kind: "boolean" }
  | { kind: "integer" }
  | { kind: "number" }
  | { kind: "date" }
  | { kind: "enum"; values: readonly string[] }
  | { kind: "list" }
  | { kind: "int_list" }
  | { kind: "enum_list"; values: readonly string[] }
  /** `set_setting.value`: string, number or boolean. */
  | { kind: "scalar" };

export interface ParamSpec {
  name: string;
  type: ParamType;
  required: boolean;
  desc_ko: string;
  desc_en: string;
}

export interface OpSpec {
  op: OpKind;
  params: ParamSpec[];
  desc_ko: string;
  desc_en: string;
}

export const THEMES = ["light", "dark", "system", "high-contrast"] as const;
export const LANGS = ["ko", "en"] as const;
export const PROFILES = ["accuracy", "balanced", "speed", "fine_print"] as const;
export const GROUP_BY_VALUES = ["month", "vendor", "doc_type", "none"] as const;
export const EXPORT_FORMAT_VALUES = ["xlsx", "csv", "jsonl", "md", "pdf_searchable"] as const;
export const DOC_STATUSES = ["new", "rendered", "extracted", "reviewed", "archived"] as const;
export const SETTING_KEYS = ["render_dpi", "pii_masking", "auto_pipeline", "inbox_watch", "review_threshold", "cache_limit_gb", "journal_days"] as const;
export const OUTPUT_FORMAT_VALUES = ["hwpx", "docx", "pdf", "md"] as const;
export const TONE_VALUES = ["formal", "plain", "concise"] as const;
export const DRAFT_LENGTH_VALUES = ["short", "medium", "long"] as const;
export const EXPLAIN_SCOPES = ["kb", "collection", "project", "document"] as const;
export const RETENTION_ITEMS = ["documents", "extractions", "logs", "journal", "chats", "temp"] as const;

const str: ParamType = { kind: "string" };
const bool: ParamType = { kind: "boolean" };
const int: ParamType = { kind: "integer" };
const list: ParamType = { kind: "list" };
const en = (values: readonly string[]): ParamType => ({ kind: "enum", values });

function p(name: string, type: ParamType, required: boolean, desc_ko: string, desc_en: string): ParamSpec {
  return { name, type, required, desc_ko, desc_en };
}

const SPECS: OpSpec[] = [
  {
    op: "import",
    params: [
      p("source", en(["inbox", "folder", "files"]), false, "가져올 곳(기본 inbox)", "where from (default inbox)"),
      p("path", str, false, "폴더 경로(source=folder)", "folder path (source=folder)"),
      p("recursive", bool, false, "하위 폴더 포함", "include subfolders"),
      p("auto_pipeline", en(["none", "classify", "full"]), false, "가져온 뒤 자동 처리 수준", "processing after import"),
    ],
    desc_ko: "문서 가져오기(inbox/폴더)",
    desc_en: "import documents (inbox / folder)",
  },
  { op: "set_inbox_watch", params: [p("enabled", bool, true, "inbox 감시 켜기/끄기", "inbox watching on/off")], desc_ko: "inbox 폴더 감시 설정", desc_en: "toggle inbox watching" },
  {
    op: "classify",
    params: [p("doc_types", list, false, "후보 유형 슬러그(비우면 전체)", "candidate type slugs (empty = all)"), p("force", bool, false, "이미 분류된 문서도 다시", "re-classify classified documents")],
    desc_ko: "문서 유형 분류(범위의 문서)",
    desc_en: "classify documents in scope",
  },
  { op: "set_doc_type", params: [p("doc_type", str, true, "유형 슬러그", "type slug")], desc_ko: "유형 수동 지정", desc_en: "set the document type manually" },
  {
    op: "ocr",
    params: [
      p("pages", { kind: "int_list" }, false, "쪽 번호(비우면 전체)", "page numbers (empty = all)"),
      p("profile", en(["standard", "fine_print"]), false, "표준 / 잔글씨", "standard / fine print"),
      p("force", bool, false, "이미 OCR한 쪽도 다시", "re-OCR pages that have text"),
    ],
    desc_ko: "OCR(텍스트 인식)",
    desc_en: "OCR pages",
  },
  {
    op: "define_schema",
    params: [
      p("name", str, false, "스키마 이름", "schema name"),
      p("description", str, false, "필드 설명(자연어)", "field description (natural language)"),
      p("mode", en(["create", "modify"]), false, "새로 만들기 / 기존 수정", "create / modify"),
      p("base", str, false, "수정할 스키마 이름·유형", "schema to modify"),
    ],
    desc_ko: "추출 스키마 정의/수정(스키마 화면에서 확인)",
    desc_en: "define / modify an extraction schema (confirmed on the schema screen)",
  },
  {
    op: "extract",
    params: [
      p("schema_ref", str, false, "auto 또는 스키마 이름/유형 슬러그", "auto or a schema name / type slug"),
      p("fields", list, false, "원하는 필드(키 또는 라벨)", "wanted fields (keys or labels)"),
      p("force", bool, false, "이미 추출된 문서도 다시", "re-extract extracted documents"),
    ],
    desc_ko: "필드 추출(KIE)",
    desc_en: "extract fields (KIE)",
  },
  { op: "validate", params: [], desc_ko: "검증 규칙 다시 실행", desc_en: "re-run validation rules" },
  {
    op: "review",
    params: [
      p("action", en(["show", "approve", "reject", "correct"]), true, "검토 동작", "review action"),
      p("field", str, false, "필드 키(correct)", "field key (correct)"),
      p("value", str, false, "수정 값(correct)", "new value (correct)"),
      p("only", en(["low_confidence", "failed", "all"]), false, "대상 필드", "which fields"),
    ],
    desc_ko: "검토 큐 보기/일괄 승인·거부·수정",
    desc_en: "show the review queue / bulk approve, reject, correct",
  },
  { op: "reindex", params: [p("scope", en(["fts", "vec", "all"]), false, "색인 범위", "index scope")], desc_ko: "검색 색인 재생성", desc_en: "rebuild the search index" },
  {
    op: "search",
    params: [p("query", str, true, "검색어", "query"), p("mode", en(["hybrid", "keyword", "semantic"]), false, "검색 방식", "search mode"), p("top_k", int, false, "결과 수", "result count")],
    desc_ko: "문서 검색",
    desc_en: "search documents",
  },
  { op: "answer", params: [p("question", str, true, "질문", "question"), p("cite", bool, false, "출처 인용(기본 true)", "cite sources (default true)"), p("knowledge", bool, false, "지식 베이스 검색(기본 true; 보낸 글만 다듬을 때 false)", "search the knowledge base (default true; false for the user's own text)")], desc_ko: "근거 기반 질의응답(지식 베이스·문서)", desc_en: "grounded question answering" },
  {
    op: "summarize",
    params: [p("target", en(["each_doc", "set"]), false, "문서별 / 묶음 하나로", "per document / one summary"), p("length", en(["short", "long"]), false, "길이", "length")],
    desc_ko: "요약",
    desc_en: "summarise",
  },
  {
    op: "aggregate",
    params: [
      p("fn", en(["sum", "count", "avg", "min", "max"]), true, "집계 함수", "aggregate function"),
      p("field", str, false, "필드 키/라벨(count는 생략 가능)", "field key / label (optional for count)"),
      p("group_by", en(GROUP_BY_VALUES), false, "그룹", "grouping"),
      p("include_unreviewed", bool, false, "검토 미완료 포함(기본 false)", "include unreviewed (default false)"),
    ],
    desc_ko: "집계(합계·건수·평균; 검토 미완료는 따로 표시)",
    desc_en: "aggregate (sum / count / avg; unreviewed shown separately)",
  },
  {
    op: "organize",
    params: [
      p("template", str, false, "파일명 템플릿({{year}} {{month}} {{day}} {{doc_type}} {{vendor}} {{total}} {{title}} {{original_name}} {{tags}} {{seq}})", "file-name template"),
      p("folder_template", str, false, "폴더 템플릿(워크스페이스 상대)", "folder template (workspace-relative)"),
      p("group_by", en(GROUP_BY_VALUES), false, "폴더 그룹", "folder grouping"),
      p("only_reviewed", bool, false, "검토 완료 문서만(기본 true)", "reviewed documents only (default true)"),
      p("dry_run", bool, false, "미리보기만", "preview only"),
    ],
    desc_ko: "정리(이름 템플릿 + 폴더 이동, 저널 기록·되돌리기 가능)",
    desc_en: "organise (rename template + folder move, journaled / undoable)",
  },
  { op: "rename", params: [p("template", str, true, "파일명 템플릿", "file-name template"), p("only_reviewed", bool, false, "검토 완료 문서만", "reviewed only")], desc_ko: "이름 변경", desc_en: "rename files" },
  {
    op: "move",
    params: [p("folder", str, true, "대상 폴더 템플릿(워크스페이스 상대)", "destination folder template"), p("group_by", en(GROUP_BY_VALUES), false, "폴더 그룹", "folder grouping"), p("only_reviewed", bool, false, "검토 완료 문서만", "reviewed only")],
    desc_ko: "폴더 이동",
    desc_en: "move files",
  },
  { op: "tag", params: [p("add", list, false, "추가할 태그", "tags to add"), p("remove", list, false, "제거할 태그", "tags to remove")], desc_ko: "태그 추가/제거", desc_en: "add / remove tags" },
  { op: "delete", params: [p("to_trash", bool, false, "휴지통으로(기본 true)", "to the recycle bin (default true)")], desc_ko: "문서 삭제(휴지통)", desc_en: "delete documents (recycle bin)" },
  {
    op: "export",
    params: [
      p("format", en(EXPORT_FORMAT_VALUES), true, "형식", "format"),
      p("path", str, false, "저장 경로(워크스페이스 상대, 기본 exports/날짜_유형.확장자)", "output path (workspace-relative)"),
      p("columns", list, false, "포함할 필드(비우면 전체)", "fields to include (empty = all)"),
      p("layout", en(["sheet_per_type", "single"]), false, "XLSX 시트 구성", "XLSX layout"),
      p("include", { kind: "enum_list", values: ["evidence", "confidence", "status"] }, false, "공통 열", "extra columns"),
      p("target", en(["fields", "table", "text"]), false, "필드 / 표 / 본문 텍스트", "fields / tables / page text"),
    ],
    desc_ko: "내보내기(XLSX/CSV/JSONL/MD/검색가능 PDF)",
    desc_en: "export (XLSX / CSV / JSONL / MD / searchable PDF)",
  },
  // -- P6 (§3.13 document generation, §3.14 explain / compare, §3.15 projects, §3.8 retention) --
  {
    op: "draft_document",
    params: [
      p("form_ref", str, false, "등록 양식(이름·유형) 또는 내장 템플릿(quotation, official_letter, contract, report, minutes, transaction_statement)", "registered form (name / type) or a built-in template"),
      p("format_description", str, false, "형식 자연어 설명", "layout description (natural language)"),
      p("content", str, true, "문서 내용(자연어)", "what the document should say"),
      p("sources", list, false, "인용할 문서 id, selection, aggregate", "document ids to quote, selection, aggregate"),
      p("output", en(OUTPUT_FORMAT_VALUES), false, "출력 형식(기본 hwpx)", "output format (default hwpx)"),
      p("tone", en(TONE_VALUES), false, "어조", "tone"),
      p("length", en(DRAFT_LENGTH_VALUES), false, "길이", "length"),
    ],
    desc_ko: "문서 초안 생성(미리보기 후 문서 생성 화면에서 저장)",
    desc_en: "draft a document (previewed, saved on the generate screen)",
  },
  {
    op: "register_form",
    params: [p("doc_ref", str, false, "문서 id·이름 또는 current", "document id / name or current"), p("name", str, false, "양식 이름", "form name"), p("doc_type_slug", str, false, "유형 슬러그", "type slug"), p("doc_ids", list, false, "샘플 문서 id 여러 개(필드 병합)", "several sample document ids (merged fields)")],
    desc_ko: "문서를 양식으로 등록(필드 그리드 확인)",
    desc_en: "register a document as a form (field grid confirmation)",
  },
  {
    op: "explain",
    params: [
      p("selection", str, false, "설명할 선택 텍스트", "selected text to explain"),
      p("question", str, false, "질문", "question"),
      p("scope", en(EXPLAIN_SCOPES), false, "근거 범위", "evidence scope"),
      p("allow_general_knowledge", bool, false, "근거 없을 때 일반 지식 허용", "allow general knowledge without evidence"),
    ],
    desc_ko: "근거 기반 설명",
    desc_en: "grounded explanation",
  },
  { op: "compare", params: [p("doc_ids", list, true, "비교할 문서 id 2~5개", "2–5 document ids"), p("aspects", list, false, "비교 관점(금액, 기간 …)", "aspects (amount, period …)")], desc_ko: "문서 비교 요약", desc_en: "compare documents" },
  {
    op: "create_project",
    params: [p("name", str, true, "프로젝트 이름", "project name"), p("template", str, false, "프로젝트 템플릿", "project template"), p("doc_types_required", list, false, "필수 문서 유형", "required document types")],
    desc_ko: "프로젝트 만들기",
    desc_en: "create a project",
  },
  { op: "add_to_project", params: [p("project", str, false, "프로젝트(비우면 열린 프로젝트)", "project (default: the open one)"), p("doc_ids", list, false, "문서 id(비우면 범위)", "document ids (default: the scope)")], desc_ko: "프로젝트에 문서 추가", desc_en: "add documents to a project" },
  {
    op: "define_rule",
    params: [p("project", str, false, "프로젝트", "project"), p("template", str, false, "프로젝트 템플릿", "project template"), p("description", str, true, "규칙 설명(자연어)", "rule description (natural language)")],
    desc_ko: "정합성 규칙 정의(규칙 그리드 확인)",
    desc_en: "define a consistency rule (rule grid confirmation)",
  },
  { op: "check_consistency", params: [p("project", str, false, "프로젝트", "project"), p("rules", list, false, "규칙 id(비우면 전체)", "rule ids (empty = all)")], desc_ko: "정합성 검사 실행", desc_en: "run the consistency check" },
  {
    op: "request_supplement",
    params: [p("project", str, false, "프로젝트", "project"), p("findings", list, false, "지적 사항 id(비우면 불일치·누락 전체)", "finding ids (empty = every mismatch / missing)"), p("output", en(OUTPUT_FORMAT_VALUES), false, "출력 형식", "output format")],
    desc_ko: "보완요청서 초안 생성(미리보기)",
    desc_en: "draft a supplement request (preview)",
  },
  { op: "set_retention", params: [p("item", en(RETENTION_ITEMS), true, "보존 항목", "retention item"), p("days", int, true, "보존 일수(-1 무기한, 0 즉시)", "days (-1 forever, 0 immediately)")], desc_ko: "보존/파기 기간 설정", desc_en: "set a retention period" },
  { op: "set_model", params: [p("id", str, true, "모델 id", "model id")], desc_ko: "활성 모델 변경", desc_en: "switch the active model" },
  { op: "set_profile", params: [p("profile", en(PROFILES), true, "추론 프로파일", "inference profile")], desc_ko: "추론 프로파일 변경", desc_en: "set the inference profile" },
  { op: "set_theme", params: [p("theme", en(THEMES), true, "테마", "theme")], desc_ko: "테마 변경", desc_en: "set the theme" },
  { op: "set_language", params: [p("lang", en(LANGS), true, "UI 언어", "UI language")], desc_ko: "UI 언어 변경", desc_en: "set the UI language" },
  { op: "set_setting", params: [p("key", en(SETTING_KEYS), true, "설정 키", "setting key"), p("value", { kind: "scalar" }, true, "값", "value")], desc_ko: "설정 값 변경", desc_en: "change a setting" },
  {
    op: "open",
    params: [p("target", en(["document", "page", "folder", "screen"]), true, "열 대상", "what to open"), p("ref", str, true, "문서 이름/id, 'doc:page', 폴더, 화면 이름", "document name / id, 'doc:page', folder or screen")],
    desc_ko: "문서/쪽/폴더/화면 열기",
    desc_en: "open a document / page / folder / screen",
  },
  {
    op: "job",
    params: [p("action", en(["show", "cancel", "retry", "pause", "resume", "prioritize"]), true, "작업 제어", "job control"), p("id", str, false, "작업 id 또는 last", "job id or last")],
    desc_ko: "작업 조회/취소/재시도/일시정지/재개/우선",
    desc_en: "job control",
  },
  { op: "engine", params: [p("action", en(["restart", "sleep", "wake"]), true, "엔진 제어", "engine control")], desc_ko: "엔진 재시작/슬립/깨우기", desc_en: "engine restart / sleep / wake" },
  { op: "undo", params: [p("target", str, false, "last 또는 배치 id", "last or a batch id")], desc_ko: "마지막 파일 작업 되돌리기", desc_en: "undo the last file batch" },
  { op: "redo", params: [], desc_ko: "되돌린 작업 다시 실행", desc_en: "redo the last undo" },
  { op: "help", params: [], desc_ko: "도움말", desc_en: "help" },
  { op: "clarify", params: [p("question", str, true, "사용자에게 할 질문", "question to ask the user")], desc_ko: "명확화 질문(명령이 모호할 때 — 다른 op 없이 단독)", desc_en: "ask a clarification question (alone, when the command is ambiguous)" },
  { op: "noop", params: [], desc_ko: "아무 것도 하지 않음", desc_en: "do nothing" },
];

export const OP_SPECS: Readonly<Record<OpKind, OpSpec>> = Object.fromEntries(SPECS.map((s) => [s.op, s])) as Record<OpKind, OpSpec>;

export const OP_KINDS: readonly OpKind[] = SPECS.map((s) => s.op);

/** Ops the UI / shell performs itself (`OpKind::runs_in_ui`). */
export const UI_OPS: ReadonlySet<OpKind> = new Set<OpKind>(["define_schema", "set_model", "set_profile", "set_theme", "set_language", "set_setting", "open", "job", "engine", "help", "clarify", "noop", "reindex", "set_inbox_watch", "register_form", "set_retention"]);

/** Ops that need the vision / chat model (`OpKind::needs_model`). */
export const MODEL_OPS: ReadonlySet<OpKind> = new Set<OpKind>(["classify", "ocr", "extract", "answer", "summarize", "import", "draft_document", "explain", "compare", "define_rule", "request_supplement"]);

/** Ops whose step payload is a `Draft` the 문서 생성 screen opens. */
export const DRAFT_OPS: ReadonlySet<OpKind> = new Set<OpKind>(["draft_document", "request_supplement"]);

/** Journaled file operations that honour `only_reviewed`. */
export const FILE_OPS: ReadonlySet<OpKind> = new Set<OpKind>(["organize", "rename", "move"]);

export function opSpec(op: OpKind): OpSpec {
  return OP_SPECS[op];
}

export function paramDesc(param: ParamSpec, language: Language | string): string {
  return language.startsWith("ko") ? param.desc_ko : param.desc_en;
}

export function opDesc(op: OpKind, language: Language | string): string {
  const spec = OP_SPECS[op];
  return language.startsWith("ko") ? spec.desc_ko : spec.desc_en;
}
