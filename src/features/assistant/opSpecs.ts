// TypeScript mirror of `crates/doculens-core/src/planner/schema.rs` (`OP_SPECS`):
// the parameter table of every planner op (appendix D) with the Korean /
// English descriptions the inline plan editor shows. The Rust table is the
// single source of truth for the validator; keep this file in step with it —
// `opSpecs.test.ts` reads schema.rs and fails when an op, a parameter, an enum
// value or a `required` flag differs.

import { SETTING_KEYS_ALL } from "@/lib/settingsCatalog";
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
export const GROUP_BY_VALUES = ["month", "vendor", "doc_type", "project", "none"] as const;
export const EXPORT_FORMAT_VALUES = ["xlsx", "csv", "jsonl", "md", "pdf_searchable"] as const;
export const EXPORT_LAYOUT_VALUES = ["sheet_per_type", "sheet_per_project", "single"] as const;
export const DOC_STATUSES = ["new", "rendered", "extracted", "reviewed", "archived"] as const;
/** `set_setting.key`: the settings-catalog paths (0.6.0) plus the pre-0.6.0 aliases. */
export const SETTING_KEYS: readonly string[] = SETTING_KEYS_ALL;
export const ROLE_VALUES = ["work", "knowledge", "form"] as const;
export const REFERENCE_ACTIONS = ["create", "rename", "delete", "enable", "disable", "add", "remove"] as const;
export const TERM_ACTIONS = ["save", "delete"] as const;
export const TEMPLATE_ACTIONS = ["save", "delete"] as const;
export const SCHEMA_ACTIONS = ["enable", "disable", "set_default", "delete", "rename"] as const;
export const MODEL_ACTIONS = ["set_tier", "set_backend", "import", "download", "delete", "verify", "benchmark", "load", "unload"] as const;
export const TIER_CHOICES = ["auto", "cpu_only", "vram_6gb", "vram_8gb", "vram_12gb", "vram_16gb", "vram_24gb_plus"] as const;
export const BACKEND_VALUES = ["auto", "cuda", "vulkan", "cpu"] as const;
export const WORKSPACE_ACTIONS = ["open", "create", "close", "forget"] as const;
export const MAINTENANCE_ACTIONS = ["purge_now", "clear_finished_jobs", "rescan", "inbox_scan"] as const;
export const ENGINE_ACTIONS = ["start", "stop", "restart", "sleep", "wake"] as const;
export const OUTPUT_FORMAT_VALUES = ["hwpx", "docx", "pdf", "md"] as const;
export const TONE_VALUES = ["formal", "plain", "concise"] as const;
export const DRAFT_LENGTH_VALUES = ["short", "medium", "long"] as const;
export const EXPLAIN_SCOPES = ["kb", "reference", "project", "document"] as const;
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
      p("paths", list, false, "파일 절대 경로(source=files; 끌어다 놓거나 적은 경로)", "absolute file paths (source=files; dropped or typed)"),
      p("recursive", bool, false, "하위 폴더 포함", "include subfolders"),
      p("auto_pipeline", en(["none", "classify", "full"]), false, "가져온 뒤 자동 처리 수준", "processing after import"),
    ],
    desc_ko: "문서 가져오기(inbox/폴더/파일 경로)",
    desc_en: "import documents (inbox / folder / file paths)",
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
      p("layout", en(EXPORT_LAYOUT_VALUES), false, "XLSX 시트 구성(유형별 / 프로젝트별 / 단일)", "XLSX layout (per type / per project / single)"),
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
  // 0.7.0 (plan §1.5): the 초안 어시스턴트 as an op — revise the draft open on the 문서 작성 screen.
  {
    op: "revise_draft",
    params: [
      p("draft_id", str, false, "초안 id(없으면 문서 작성 화면에 열린 초안)", "draft id (default: the draft open on the generate screen)"),
      p("scope", en(["section", "insert_after", "whole"]), true, "범위: section(절 다시 쓰기) / insert_after(절 뒤에 새 절) / whole(전체)", "scope: section (rewrite) / insert_after (new section) / whole"),
      p("section", int, false, "절 번호(1부터; insert_after에서 비우면 끝에 추가)", "section ordinal (1-based; empty = append for insert_after)"),
      p("heading", str, false, "새 절 제목", "heading of the new section"),
      p("instruction", str, true, "수정 지시", "revision instruction"),
    ],
    desc_ko: "열린 초안의 절 수정·새 절 추가·전체 수정(문서 작성 화면)",
    desc_en: "revise a section, add a section or rewrite the open draft (generate screen)",
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
    params: [
      p("name", str, true, "프로젝트 이름", "project name"),
      p("template", str, false, "프로젝트 템플릿", "project template"),
      p("doc_types_required", list, false, "필수 문서 유형", "required document types"),
      p("start_date", { kind: "date" }, false, "시작일(비우면 오늘)", "start date (default: today)"),
      p("end_date", { kind: "date" }, false, "종료일(비우면 진행중)", "end date (none = open-ended)"),
    ],
    desc_ko: "프로젝트 만들기",
    desc_en: "create a project",
  },
  {
    op: "add_to_project",
    params: [
      p("project", str, false, "프로젝트(비우면 열린 프로젝트)", "project (default: the open one)"),
      p("doc_ids", list, false, "문서 id(비우면 범위)", "document ids (default: the scope)"),
      p("as_reference", bool, false, "참고 자료로 넣기(기본: 업무 문서)", "add as reference material (default: work documents)"),
    ],
    desc_ko: "프로젝트에 문서 추가(업무 문서 또는 참고 자료)",
    desc_en: "add documents to a project (work documents or reference material)",
  },
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
  { op: "set_setting", params: [p("key", en(SETTING_KEYS), true, "설정 키(섹션.항목, 예: processing.render_dpi)", "setting key (section.field, e.g. processing.render_dpi)"), p("value", { kind: "scalar" }, true, "값", "value")], desc_ko: "설정 값 변경(설정 카탈로그의 모든 항목)", desc_en: "change a setting (any catalog item)" },
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
  { op: "engine", params: [p("action", en(ENGINE_ACTIONS), true, "엔진 제어", "engine control")], desc_ko: "AI 엔진 시작/정지/재시작/슬립/깨우기", desc_en: "AI engine start / stop / restart / sleep / wake" },
  { op: "undo", params: [p("target", str, false, "last 또는 배치 id", "last or a batch id")], desc_ko: "마지막 파일 작업 되돌리기", desc_en: "undo the last file batch" },
  { op: "redo", params: [], desc_ko: "되돌린 작업 다시 실행", desc_en: "redo the last undo" },
  { op: "help", params: [], desc_ko: "도움말", desc_en: "help" },
  { op: "clarify", params: [p("question", str, true, "사용자에게 할 질문", "question to ask the user")], desc_ko: "명확화 질문(명령이 모호할 때 — 다른 op 없이 단독)", desc_en: "ask a clarification question (alone, when the command is ambiguous)" },
  { op: "noop", params: [], desc_ko: "아무 것도 하지 않음", desc_en: "do nothing" },
  // -- 0.6.0 (plan §1): documents --
  { op: "rotate", params: [p("degrees", int, true, "회전 각도(90·180·270)", "degrees (90 / 180 / 270)"), p("pages", { kind: "int_list" }, false, "쪽 번호(비우면 전체)", "page numbers (empty = all)")], desc_ko: "쪽 이미지 회전(원본 불변)", desc_en: "rotate page images (originals untouched)" },
  { op: "rerender", params: [p("dpi", int, false, "렌더 DPI(72~600; 비우면 설정값)", "render DPI (72–600; default from settings)")], desc_ko: "페이지 이미지 다시 렌더", desc_en: "re-render page images" },
  { op: "split", params: [p("boundaries", { kind: "int_list" }, false, "새 문서가 시작하는 쪽 번호(2 이상)", "pages where a new document starts (≥ 2)"), p("every", int, false, "n쪽마다 나누기", "split every n pages")], desc_ko: "문서를 쪽 단위로 나누기(새 PDF 문서)", desc_en: "split a document at pages (new PDF documents)" },
  { op: "merge", params: [p("name", str, false, "합친 문서 이름", "name of the merged document")], desc_ko: "범위 문서 2~20개를 하나로 합치기", desc_en: "merge 2–20 documents into one" },
  { op: "set_title", params: [p("title", str, true, "새 제목", "new title")], desc_ko: "문서 제목 바꾸기", desc_en: "set the document title" },
  { op: "edit_text", params: [p("find", str, true, "찾을 글자", "text to find"), p("replace", str, true, "바꿀 글자", "replacement"), p("pages", { kind: "int_list" }, false, "쪽 번호(비우면 전체)", "page numbers (empty = all)")], desc_ko: "쪽 글자 찾아 바꾸기(이력·재색인)", desc_en: "find / replace page text (history, re-index)" },
  { op: "set_role", params: [p("role", en(ROLE_VALUES), true, "업무 문서 / 참고 자료 / 양식", "work / reference material / form")], desc_ko: "문서 역할 표시", desc_en: "set the document role" },
  // -- reference material --
  {
    op: "manage_reference",
    params: [
      p("action", en(REFERENCE_ACTIONS), true, "묶음 동작", "group action"),
      p("group", str, false, "묶음 이름/ID(비우고 project를 주면 그 프로젝트 묶음)", "group name / id (empty + project = that project's group)"),
      p("name", str, false, "새 이름(create·rename)", "new name (create / rename)"),
      p("project", str, false, "프로젝트 묶음일 때 프로젝트", "project whose group it is"),
    ],
    desc_ko: "참고 자료 묶음 만들기/이름/삭제/사용/문서 넣기·빼기",
    desc_en: "reference groups: create / rename / delete / enable / disable / add / remove documents",
  },
  { op: "define_term", params: [p("action", en(TERM_ACTIONS), false, "저장(기본)/삭제", "save (default) / delete"), p("term", str, true, "용어", "term"), p("definition", str, false, "정의", "definition"), p("synonyms", list, false, "동의어", "synonyms")], desc_ko: "용어집 항목 저장/삭제", desc_en: "save / delete a glossary term" },
  {
    op: "set_version",
    params: [
      p("group_key", str, false, "같은 제목의 판본을 묶는 키", "version group key"),
      p("label", str, false, "판 이름(2024년판 …)", "version label"),
      p("effective_date", { kind: "date" }, false, "시행일", "effective date"),
      p("latest", bool, false, "최신본 여부", "is the latest edition"),
      p("clear", bool, false, "판본 표시 지우기", "clear the version"),
    ],
    desc_ko: "문서 판본(신·구판) 표시",
    desc_en: "mark a document's edition (version group)",
  },
  // -- projects --
  {
    op: "update_project",
    params: [
      p("project", str, false, "프로젝트(비우면 열린 프로젝트)", "project (default: the open one)"),
      p("name", str, false, "새 이름", "new name"),
      p("template", str, false, "프로젝트 템플릿", "project template"),
      p("start_date", { kind: "date" }, false, "시작일", "start date"),
      p("end_date", { kind: "date" }, false, "종료일", "end date"),
      p("owner", str, false, "담당", "owner"),
      p("intake_no", str, false, "접수번호", "intake number"),
      p("notes", str, false, "메모", "notes"),
      p("keywords", list, false, "분류 키워드", "classification keywords"),
      p("inbox_folder", str, false, "inbox 하위 폴더", "inbox subfolder"),
    ],
    desc_ko: "프로젝트 정보 수정(이름·기간·담당·키워드)",
    desc_en: "edit a project (name, dates, owner, keywords)",
  },
  { op: "delete_project", params: [p("project", str, false, "프로젝트(비우면 열린 프로젝트)", "project (default: the open one)")], desc_ko: "프로젝트 삭제(문서는 문서함에 남음)", desc_en: "delete a project (documents stay)" },
  { op: "remove_from_project", params: [p("project", str, false, "프로젝트(비우면 열린 프로젝트)", "project (default: the open one)"), p("doc_ids", list, false, "문서 id(비우면 범위)", "document ids (default: the scope)")], desc_ko: "프로젝트에서 문서 빼기", desc_en: "remove documents from a project" },
  {
    op: "manage_template",
    params: [
      p("action", en(TEMPLATE_ACTIONS), false, "저장(기본)/삭제", "save (default) / delete"),
      p("template", str, false, "템플릿 이름/ID(수정·삭제)", "template name / id (edit / delete)"),
      p("name", str, false, "템플릿 이름(새로 만들 때)", "template name (new)"),
      p("doc_types_required", list, false, "필수 문서 유형", "required document types"),
      p("description", str, false, "설명", "description"),
    ],
    desc_ko: "프로젝트 템플릿 저장/삭제(내장 제외)",
    desc_en: "save / delete a project template (not built-ins)",
  },
  { op: "classify_projects", params: [p("apply", bool, false, "제안을 바로 적용(기본 미리보기)", "apply the suggestions (default: preview)"), p("use_model", bool, false, "규칙이 못 정한 문서를 모델에게", "ask the model for undecided documents")], desc_ko: "문서를 프로젝트로 자동 분류(범위 비우면 미배정 전체)", desc_en: "classify documents into projects (empty scope = every unassigned)" },
  // -- forms --
  { op: "manage_schema", params: [p("action", en(SCHEMA_ACTIONS), true, "동작", "action"), p("schema", str, true, "양식/추출 항목 이름·ID", "schema name / id"), p("name", str, false, "새 이름(rename)", "new name (rename)")], desc_ko: "양식·추출 항목 켜기/끄기/기본/삭제/이름", desc_en: "forms / extraction schemas: enable / disable / default / delete / rename" },
  // -- system (UI actions) --
  {
    op: "manage_model",
    params: [
      p("action", en(MODEL_ACTIONS), true, "동작", "action"),
      p("id", str, false, "모델 id(download·delete·verify·load·unload·benchmark)", "model id"),
      p("tier", en(TIER_CHOICES), false, "티어(set_tier)", "tier (set_tier)"),
      p("backend", en(BACKEND_VALUES), false, "백엔드(set_backend)", "backend (set_backend)"),
      p("path", str, false, "GGUF 파일/폴더 경로(import)", "GGUF file / folder path (import)"),
    ],
    desc_ko: "AI 모델 관리: 티어·백엔드·가져오기·다운로드·삭제·검증·테스트·로드",
    desc_en: "AI models: tier, backend, import, download, delete, verify, benchmark, load / unload",
  },
  { op: "workspace", params: [p("action", en(WORKSPACE_ACTIONS), true, "동작", "action"), p("path", str, false, "폴더 경로(open·create·forget)", "folder path (open / create / forget)")], desc_ko: "워크스페이스 열기/만들기/닫기/목록에서 제거", desc_en: "workspace open / create / close / forget" },
  { op: "maintenance", params: [p("action", en(MAINTENANCE_ACTIONS), true, "동작", "action")], desc_ko: "파기 실행 · 끝난 작업 지우기 · 문서함 재검사 · inbox 검사", desc_en: "purge now · clear finished jobs · rescan the library · scan the inbox" },
];

export const OP_SPECS: Readonly<Record<OpKind, OpSpec>> = Object.fromEntries(SPECS.map((s) => [s.op, s])) as Record<OpKind, OpSpec>;

export const OP_KINDS: readonly OpKind[] = SPECS.map((s) => s.op);

/** Ops the UI / shell performs itself (`OpKind::runs_in_ui`). */
export const UI_OPS: ReadonlySet<OpKind> = new Set<OpKind>(["define_schema", "set_model", "set_profile", "set_theme", "set_language", "set_setting", "open", "job", "engine", "help", "clarify", "noop", "reindex", "set_inbox_watch", "register_form", "set_retention", "manage_model", "workspace", "maintenance"]);

/** Ops that need the vision / chat model (`OpKind::needs_model`). */
export const MODEL_OPS: ReadonlySet<OpKind> = new Set<OpKind>(["classify", "ocr", "extract", "answer", "summarize", "import", "draft_document", "revise_draft", "explain", "compare", "define_rule", "request_supplement"]);

/** Ops whose step payload is a `Draft` the 문서 생성 screen opens. */
export const DRAFT_OPS: ReadonlySet<OpKind> = new Set<OpKind>(["draft_document", "revise_draft", "request_supplement"]);

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
