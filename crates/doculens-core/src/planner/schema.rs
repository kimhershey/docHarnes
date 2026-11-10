//! Appendix D as data: the parameter table of every op (the model-facing
//! description and the grammar) and the PlanSchema handed to llama-server as
//! `response_format`. The serde types in `super` are the validator's truth;
//! `tests::specs_match_serde` keeps both in step.

use serde_json::{json, Map, Value};

use super::OpKind;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamType {
    Str,
    Bool,
    Int,
    Number,
    Date,
    Enum(&'static [&'static str]),
    StrList,
    IntList,
    EnumList(&'static [&'static str]),
    /// `set_setting.value`: string, number or boolean.
    Scalar,
}

#[derive(Debug, Clone, Copy)]
pub struct ParamSpec {
    pub name: &'static str,
    pub ty: ParamType,
    pub required: bool,
    pub desc_ko: &'static str,
    pub desc_en: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct OpSpec {
    pub kind: OpKind,
    pub params: &'static [ParamSpec],
    pub desc_ko: &'static str,
    pub desc_en: &'static str,
}

const fn p(name: &'static str, ty: ParamType, required: bool, ko: &'static str, en: &'static str) -> ParamSpec {
    ParamSpec { name, ty, required, desc_ko: ko, desc_en: en }
}

pub const THEMES: &[&str] = &["light", "dark", "system", "high-contrast"];
pub const LANGS: &[&str] = &["ko", "en"];
pub const PROFILES: &[&str] = &["accuracy", "balanced", "speed", "fine_print"];
pub const GROUP_BY: &[&str] = &["month", "vendor", "doc_type", "project", "none"];
pub const EXPORT_FORMATS: &[&str] = &["xlsx", "csv", "jsonl", "md", "pdf_searchable"];
pub const DOC_STATUSES: &[&str] = &["new", "rendered", "extracted", "reviewed", "archived"];
pub const DRAFT_OUTPUTS: &[&str] = &["hwpx", "docx", "pdf", "md"];
/// `revise_draft.scope` (0.7.0).
pub const REVISE_SCOPES: &[&str] = &["section", "insert_after", "whole"];
pub const TONES: &[&str] = &["formal", "plain", "concise"];
pub const LENGTHS: &[&str] = &["short", "medium", "long"];
pub const EXPLAIN_SCOPES: &[&str] = &["kb", "reference", "project", "document"];
pub const RETENTION_ITEMS: &[&str] = &["documents", "extractions", "logs", "journal", "chats", "temp"];
/// `set_setting.key`: the settings catalog paths + the 0.1–0.5 short keys (`planner::settings_catalog`).
pub use super::settings_catalog::SETTING_KEYS;
pub const ROLES: &[&str] = &["work", "knowledge", "form"];
pub const REFERENCE_ACTIONS: &[&str] = &["create", "rename", "delete", "enable", "disable", "add", "remove"];
pub const SAVE_DELETE: &[&str] = &["save", "delete"];
pub const SCHEMA_ACTIONS: &[&str] = &["enable", "disable", "set_default", "delete", "rename"];
pub const MODEL_ACTIONS: &[&str] =
    &["set_tier", "set_backend", "import", "download", "delete", "verify", "benchmark", "load", "unload"];
pub const MODEL_TIERS: &[&str] =
    &["auto", "cpu_only", "vram_6gb", "vram_8gb", "vram_12gb", "vram_16gb", "vram_24gb_plus"];
pub const WORKSPACE_ACTIONS: &[&str] = &["open", "create", "close", "forget"];
pub const MAINTENANCE_ACTIONS: &[&str] = &["purge_now", "clear_finished_jobs", "rescan", "inbox_scan"];

pub const OP_SPECS: &[OpSpec] = &[
    OpSpec {
        kind: OpKind::Import,
        params: &[
            p("source", ParamType::Enum(&["inbox", "folder", "files"]), false, "가져올 곳(기본 inbox)", "where from (default inbox)"),
            p("path", ParamType::Str, false, "폴더 경로(source=folder)", "folder path (source=folder)"),
            p("paths", ParamType::StrList, false, "파일 경로들(source=files; 적거나 끌어다 놓은 파일)", "file paths (source=files; typed or dropped)"),
            p("recursive", ParamType::Bool, false, "하위 폴더 포함", "include subfolders"),
            p("auto_pipeline", ParamType::Enum(&["none", "classify", "full"]), false, "가져온 뒤 자동 처리 수준", "processing after import"),
        ],
        desc_ko: "문서 가져오기(inbox/폴더)",
        desc_en: "import documents (inbox / folder)",
    },
    OpSpec {
        kind: OpKind::SetInboxWatch,
        params: &[p("enabled", ParamType::Bool, true, "inbox 감시 켜기/끄기", "inbox watching on/off")],
        desc_ko: "inbox 폴더 감시 설정",
        desc_en: "toggle inbox watching",
    },
    OpSpec {
        kind: OpKind::Classify,
        params: &[
            p("doc_types", ParamType::StrList, false, "후보 유형 슬러그(비우면 전체)", "candidate type slugs (empty = all)"),
            p("force", ParamType::Bool, false, "이미 분류된 문서도 다시", "re-classify classified documents"),
        ],
        desc_ko: "문서 유형 분류(범위의 문서)",
        desc_en: "classify documents in scope",
    },
    OpSpec {
        kind: OpKind::SetDocType,
        params: &[p("doc_type", ParamType::Str, true, "유형 슬러그", "type slug")],
        desc_ko: "유형 수동 지정",
        desc_en: "set the document type manually",
    },
    OpSpec {
        kind: OpKind::Ocr,
        params: &[
            p("pages", ParamType::IntList, false, "쪽 번호(비우면 전체)", "page numbers (empty = all)"),
            p("profile", ParamType::Enum(&["standard", "fine_print"]), false, "표준 / 잔글씨", "standard / fine print"),
            p("force", ParamType::Bool, false, "이미 OCR한 쪽도 다시", "re-OCR pages that have text"),
        ],
        desc_ko: "OCR(텍스트 인식)",
        desc_en: "OCR pages",
    },
    OpSpec {
        kind: OpKind::DefineSchema,
        params: &[
            p("name", ParamType::Str, false, "스키마 이름", "schema name"),
            p("description", ParamType::Str, false, "필드 설명(자연어)", "field description (natural language)"),
            p("mode", ParamType::Enum(&["create", "modify"]), false, "새로 만들기 / 기존 수정", "create / modify"),
            p("base", ParamType::Str, false, "수정할 스키마 이름·유형", "schema to modify"),
        ],
        desc_ko: "추출 스키마 정의/수정(스키마 화면에서 확인)",
        desc_en: "define / modify an extraction schema (confirmed on the schema screen)",
    },
    OpSpec {
        kind: OpKind::Extract,
        params: &[
            p("schema_ref", ParamType::Str, false, "auto 또는 스키마 이름/유형 슬러그", "auto or a schema name / type slug"),
            p("fields", ParamType::StrList, false, "원하는 필드(키 또는 라벨)", "wanted fields (keys or labels)"),
            p("force", ParamType::Bool, false, "이미 추출된 문서도 다시", "re-extract extracted documents"),
        ],
        desc_ko: "필드 추출(KIE)",
        desc_en: "extract fields (KIE)",
    },
    OpSpec { kind: OpKind::Validate, params: &[], desc_ko: "검증 규칙 다시 실행", desc_en: "re-run validation rules" },
    OpSpec {
        kind: OpKind::Review,
        params: &[
            p("action", ParamType::Enum(&["show", "approve", "reject", "correct"]), true, "검토 동작", "review action"),
            p("field", ParamType::Str, false, "필드 키(correct)", "field key (correct)"),
            p("value", ParamType::Str, false, "수정 값(correct)", "new value (correct)"),
            p("only", ParamType::Enum(&["low_confidence", "failed", "all"]), false, "대상 필드", "which fields"),
        ],
        desc_ko: "검토 큐 보기/일괄 승인·거부·수정",
        desc_en: "show the review queue / bulk approve, reject, correct",
    },
    OpSpec {
        kind: OpKind::Reindex,
        params: &[p("scope", ParamType::Enum(&["fts", "vec", "all"]), false, "색인 범위", "index scope")],
        desc_ko: "검색 색인 재생성",
        desc_en: "rebuild the search index",
    },
    OpSpec {
        kind: OpKind::Search,
        params: &[
            p("query", ParamType::Str, true, "검색어", "query"),
            p("mode", ParamType::Enum(&["hybrid", "keyword", "semantic"]), false, "검색 방식", "search mode"),
            p("top_k", ParamType::Int, false, "결과 수", "result count"),
        ],
        desc_ko: "문서 검색",
        desc_en: "search documents",
    },
    OpSpec {
        kind: OpKind::Answer,
        params: &[
            p("question", ParamType::Str, true, "질문", "question"),
            p("cite", ParamType::Bool, false, "출처 인용(기본 true)", "cite sources (default true)"),
            p("knowledge", ParamType::Bool, false, "지식 베이스 검색(기본 true; 사용자가 보낸 글만 다듬을 때 false)", "search the knowledge base (default true; false to work on the user's own text)"),
        ],
        desc_ko: "근거 기반 질의응답(지식 베이스·문서)",
        desc_en: "grounded question answering",
    },
    OpSpec {
        kind: OpKind::Summarize,
        params: &[
            p("target", ParamType::Enum(&["each_doc", "set"]), false, "문서별 / 묶음 하나로", "per document / one summary"),
            p("length", ParamType::Enum(&["short", "long"]), false, "길이", "length"),
        ],
        desc_ko: "요약",
        desc_en: "summarise",
    },
    OpSpec {
        kind: OpKind::Aggregate,
        params: &[
            p("fn", ParamType::Enum(&["sum", "count", "avg", "min", "max"]), true, "집계 함수", "aggregate function"),
            p("field", ParamType::Str, false, "필드 키/라벨(count는 생략 가능)", "field key / label (optional for count)"),
            p("group_by", ParamType::Enum(GROUP_BY), false, "그룹", "grouping"),
            p("include_unreviewed", ParamType::Bool, false, "검토 미완료 포함(기본 false)", "include unreviewed (default false)"),
        ],
        desc_ko: "집계(합계·건수·평균; 검토 미완료는 따로 표시)",
        desc_en: "aggregate (sum / count / avg; unreviewed shown separately)",
    },
    OpSpec {
        kind: OpKind::Organize,
        params: &[
            p("template", ParamType::Str, false, "파일명 템플릿({{year}} {{month}} {{day}} {{doc_type}} {{vendor}} {{total}} {{title}} {{original_name}} {{tags}} {{seq}} {{project}} {{project_code}})", "file-name template"),
            p("folder_template", ParamType::Str, false, "폴더 템플릿(워크스페이스 상대; {{project}} = 문서의 프로젝트)", "folder template (workspace-relative; {{project}} = the document's project)"),
            p("group_by", ParamType::Enum(GROUP_BY), false, "폴더 그룹", "folder grouping"),
            p("only_reviewed", ParamType::Bool, false, "검토 완료 문서만(기본 true)", "reviewed documents only (default true)"),
            p("dry_run", ParamType::Bool, false, "미리보기만", "preview only"),
        ],
        desc_ko: "정리(이름 템플릿 + 폴더 이동, 저널 기록·되돌리기 가능)",
        desc_en: "organise (rename template + folder move, journaled / undoable)",
    },
    OpSpec {
        kind: OpKind::Rename,
        params: &[
            p("template", ParamType::Str, true, "파일명 템플릿", "file-name template"),
            p("only_reviewed", ParamType::Bool, false, "검토 완료 문서만", "reviewed only"),
        ],
        desc_ko: "이름 변경",
        desc_en: "rename files",
    },
    OpSpec {
        kind: OpKind::Move,
        params: &[
            p("folder", ParamType::Str, true, "대상 폴더 템플릿(워크스페이스 상대)", "destination folder template"),
            p("group_by", ParamType::Enum(GROUP_BY), false, "폴더 그룹", "folder grouping"),
            p("only_reviewed", ParamType::Bool, false, "검토 완료 문서만", "reviewed only"),
        ],
        desc_ko: "폴더 이동",
        desc_en: "move files",
    },
    OpSpec {
        kind: OpKind::Tag,
        params: &[
            p("add", ParamType::StrList, false, "추가할 태그", "tags to add"),
            p("remove", ParamType::StrList, false, "제거할 태그", "tags to remove"),
        ],
        desc_ko: "태그 추가/제거",
        desc_en: "add / remove tags",
    },
    OpSpec {
        kind: OpKind::Delete,
        params: &[p("to_trash", ParamType::Bool, false, "휴지통으로(기본 true)", "to the recycle bin (default true)")],
        desc_ko: "문서 삭제(휴지통)",
        desc_en: "delete documents (recycle bin)",
    },
    OpSpec {
        kind: OpKind::Export,
        params: &[
            p("format", ParamType::Enum(EXPORT_FORMATS), true, "형식", "format"),
            p("path", ParamType::Str, false, "저장 경로(워크스페이스 상대, 기본 exports/날짜_유형.확장자)", "output path (workspace-relative)"),
            p("columns", ParamType::StrList, false, "포함할 필드(비우면 전체)", "fields to include (empty = all)"),
            p("layout", ParamType::Enum(&["sheet_per_type", "sheet_per_project", "single"]), false, "XLSX 시트 구성(유형별 / 프로젝트별 / 단일)", "XLSX layout (per type / per project / single)"),
            p("include", ParamType::EnumList(&["evidence", "confidence", "status"]), false, "공통 열", "extra columns"),
            p("target", ParamType::Enum(&["fields", "table", "text"]), false, "필드 / 표 / 본문 텍스트", "fields / tables / page text"),
        ],
        desc_ko: "내보내기(XLSX/CSV/JSONL/MD/검색가능 PDF)",
        desc_en: "export (XLSX / CSV / JSONL / MD / searchable PDF)",
    },
    OpSpec {
        kind: OpKind::DraftDocument,
        params: &[
            p("form_ref", ParamType::Str, false, "등록 양식 이름/유형 또는 내장 템플릿(official_letter|quotation|transaction_statement|contract|report|minutes|supplement_request)", "registered form (name / type) or built-in template name"),
            p("format_description", ParamType::Str, false, "형식을 자연어로 설명(양식이 없을 때)", "layout described in words (no form)"),
            p("content", ParamType::Str, true, "문서에 담을 내용(자연어)", "what the document should say"),
            p("sources", ParamType::StrList, false, "인용할 문서 id, 또는 selection(선택 문서)·aggregate(집계 결과)", "document ids to quote, or selection / aggregate"),
            p("output", ParamType::Enum(DRAFT_OUTPUTS), false, "저장 형식(기본 hwpx)", "output format (default hwpx)"),
            p("tone", ParamType::Enum(TONES), false, "어조", "tone"),
            p("length", ParamType::Enum(LENGTHS), false, "분량", "length"),
        ],
        desc_ko: "양식·템플릿으로 문서 초안 생성(미리보기 후 저장)",
        desc_en: "draft a document from a form / template (preview, then save)",
    },
    OpSpec {
        kind: OpKind::ReviseDraft,
        params: &[
            p("draft_id", ParamType::Str, false, "초안 id(생략 = 문서 작성 화면에 열린 초안)", "draft id (omit = the draft open in the document generator)"),
            p("scope", ParamType::Enum(REVISE_SCOPES), true, "section(절 다시 쓰기) | insert_after(새 절 추가) | whole(전체 다시)", "section (rewrite one) | insert_after (add a section) | whole"),
            p("section", ParamType::Int, false, "절 번호(1부터; insert_after는 그 뒤에, 생략 = 끝에)", "1-based section number (insert_after: after it; omit = at the end)"),
            p("heading", ParamType::Str, false, "새 절 제목(선택)", "title of the new section (optional)"),
            p("instruction", ParamType::Str, true, "수정 지시(자연어)", "revision instruction"),
        ],
        desc_ko: "열린 초안의 절을 다시 쓰거나 새 절을 넣거나 전체를 다시 생성(AI 수정)",
        desc_en: "revise the open draft: rewrite a section, insert one or regenerate the whole",
    },
    OpSpec {
        kind: OpKind::RegisterForm,
        params: &[
            p("doc_ref", ParamType::Str, false, "양식 문서 id 또는 current(열린 문서)", "form document id or current"),
            p("name", ParamType::Str, false, "양식 이름", "form name"),
            p("doc_type_slug", ParamType::Str, false, "새 문서 유형 슬러그(선택)", "new document-type slug (optional)"),
            p("doc_ids", ParamType::StrList, false, "샘플 문서 id 여러 개(선택; 문서 1~N개로 필드를 병합)", "several sample document ids (optional; fields merged over them)"),
        ],
        desc_ko: "열린/지정 문서(1~N개)를 양식으로 등록(필드 도출 → 확인)",
        desc_en: "register the open / given document as a form (field grid confirmation)",
    },
    OpSpec {
        kind: OpKind::Explain,
        params: &[
            p("selection", ParamType::Str, false, "설명할 문장·용어(선택 텍스트)", "text to explain (selection)"),
            p("question", ParamType::Str, false, "질문 형태일 때", "question form"),
            p("scope", ParamType::Enum(EXPLAIN_SCOPES), false, "근거 범위(기본 kb)", "evidence scope (default kb)"),
            p("allow_general_knowledge", ParamType::Bool, false, "근거 없을 때 일반 지식 허용(기본 false)", "allow general knowledge when no evidence"),
        ],
        desc_ko: "지식 베이스 근거로 선택 문장·용어 설명",
        desc_en: "explain a selection / term with knowledge-base evidence",
    },
    OpSpec {
        kind: OpKind::Compare,
        params: &[
            p("doc_ids", ParamType::StrList, true, "비교할 문서 id 2~5개", "2–5 document ids"),
            p("aspects", ParamType::StrList, false, "비교 관점(금액·기간·조항 …)", "aspects to compare"),
        ],
        desc_ko: "문서 2~5건 비교 요약",
        desc_en: "compare 2–5 documents",
    },
    OpSpec {
        kind: OpKind::CreateProject,
        params: &[
            p("name", ParamType::Str, true, "프로젝트 이름", "project name"),
            p("template", ParamType::Str, false, "프로젝트 템플릿 이름/ID", "project template name / id"),
            p("doc_types_required", ParamType::StrList, false, "필수 문서 유형 슬러그", "required document-type slugs"),
            p("start_date", ParamType::Str, false, "시작일(YYYY-MM-DD; 없으면 오늘)", "start date (YYYY-MM-DD; default today)"),
            p("end_date", ParamType::Str, false, "종료일(YYYY-MM-DD; 없으면 진행중)", "end date (YYYY-MM-DD; none = open-ended)"),
        ],
        desc_ko: "프로젝트(케이스) 만들기",
        desc_en: "create a project (case)",
    },
    OpSpec {
        kind: OpKind::AddToProject,
        params: &[
            p("project", ParamType::Str, false, "프로젝트 이름/ID(기본: 열린 프로젝트)", "project name / id (default: open project)"),
            p("doc_ids", ParamType::StrList, false, "추가할 문서 id(기본: 범위의 문서)", "document ids (default: documents in scope)"),
            p("as_reference", ParamType::Bool, false, "참고 자료(지침·규정·기준)로 넣기(기본 false = 업무 문서)", "add as reference material (guidelines / regulations; default false = work documents)"),
        ],
        desc_ko: "문서를 프로젝트에 넣기(업무 문서 또는 참고 자료)",
        desc_en: "put documents in a project (work documents or reference material)",
    },
    OpSpec {
        kind: OpKind::DefineRule,
        params: &[
            p("project", ParamType::Str, false, "프로젝트 이름/ID", "project name / id"),
            p("template", ParamType::Str, false, "또는 템플릿 이름/ID", "or template name / id"),
            p("description", ParamType::Str, true, "규칙을 자연어로(예: 세금계산서 공급자 사업자번호 = 사업자등록증 번호)", "rule in words"),
        ],
        desc_ko: "정합성 규칙 정의(규칙 그리드 확인)",
        desc_en: "define a consistency rule (grid confirmation)",
    },
    OpSpec {
        kind: OpKind::CheckConsistency,
        params: &[
            p("project", ParamType::Str, false, "프로젝트 이름/ID(기본: 열린 프로젝트)", "project name / id (default: open project)"),
            p("rules", ParamType::StrList, false, "규칙 id(비우면 전체)", "rule ids (empty = all)"),
        ],
        desc_ko: "프로젝트 문서 간 정합성 검사",
        desc_en: "run the project's consistency rules",
    },
    OpSpec {
        kind: OpKind::RequestSupplement,
        params: &[
            p("project", ParamType::Str, false, "프로젝트 이름/ID(기본: 열린 프로젝트)", "project name / id (default: open project)"),
            p("findings", ParamType::StrList, false, "포함할 결과 id(비우면 불일치·누락 전체)", "finding ids (empty = every mismatch / missing)"),
            p("output", ParamType::Enum(DRAFT_OUTPUTS), false, "저장 형식(기본 hwpx)", "output format"),
        ],
        desc_ko: "불일치·누락으로 보완요청서 초안 생성",
        desc_en: "draft a supplement request from the findings",
    },
    OpSpec {
        kind: OpKind::SetRetention,
        params: &[
            p("item", ParamType::Enum(RETENTION_ITEMS), true, "보존 항목", "retention item"),
            p("days", ParamType::Int, true, "보존 일수(-1 = 무기한, 0 = 즉시 파기)", "days to keep (-1 = forever, 0 = immediately)"),
        ],
        desc_ko: "보존/파기 기간 설정",
        desc_en: "set a retention period",
    },
    OpSpec {
        kind: OpKind::SetModel,
        params: &[p("id", ParamType::Str, true, "모델 id", "model id")],
        desc_ko: "활성 모델 변경",
        desc_en: "switch the active model",
    },
    OpSpec {
        kind: OpKind::SetProfile,
        params: &[p("profile", ParamType::Enum(PROFILES), true, "추론 프로파일", "inference profile")],
        desc_ko: "추론 프로파일 변경",
        desc_en: "set the inference profile",
    },
    OpSpec {
        kind: OpKind::SetTheme,
        params: &[p("theme", ParamType::Enum(THEMES), true, "테마", "theme")],
        desc_ko: "테마 변경",
        desc_en: "set the theme",
    },
    OpSpec {
        kind: OpKind::SetLanguage,
        params: &[p("lang", ParamType::Enum(LANGS), true, "UI 언어", "UI language")],
        desc_ko: "UI 언어 변경",
        desc_en: "set the UI language",
    },
    OpSpec {
        kind: OpKind::SetSetting,
        params: &[
            p("key", ParamType::Enum(SETTING_KEYS), true, "설정 키", "setting key"),
            p("value", ParamType::Scalar, true, "값", "value"),
        ],
        desc_ko: "설정 값 변경",
        desc_en: "change a setting",
    },
    OpSpec {
        kind: OpKind::Open,
        params: &[
            p("target", ParamType::Enum(&["document", "page", "folder", "screen"]), true, "열 대상", "what to open"),
            p("ref", ParamType::Str, true, "문서 이름/id, 'doc:page', 폴더, 화면 이름", "document name / id, 'doc:page', folder or screen"),
        ],
        desc_ko: "문서/쪽/폴더/화면 열기",
        desc_en: "open a document / page / folder / screen",
    },
    OpSpec {
        kind: OpKind::Job,
        params: &[
            p("action", ParamType::Enum(&["show", "cancel", "retry", "pause", "resume", "prioritize"]), true, "작업 제어", "job control"),
            p("id", ParamType::Str, false, "작업 id 또는 last", "job id or last"),
        ],
        desc_ko: "작업 조회/취소/재시도/일시정지/재개/우선",
        desc_en: "job control",
    },
    OpSpec {
        kind: OpKind::Engine,
        params: &[p("action", ParamType::Enum(&["restart", "sleep", "wake", "start", "stop"]), true, "AI 엔진 제어", "engine control")],
        desc_ko: "AI 엔진 시작/정지/재시작/슬립/깨우기",
        desc_en: "engine start / stop / restart / sleep / wake",
    },
    OpSpec {
        kind: OpKind::Undo,
        params: &[p("target", ParamType::Str, false, "last 또는 배치 id", "last or a batch id")],
        desc_ko: "마지막 파일 작업 되돌리기",
        desc_en: "undo the last file batch",
    },
    OpSpec { kind: OpKind::Redo, params: &[], desc_ko: "되돌린 작업 다시 실행", desc_en: "redo the last undo" },
    OpSpec { kind: OpKind::Help, params: &[], desc_ko: "도움말", desc_en: "help" },
    OpSpec {
        kind: OpKind::Clarify,
        params: &[p("question", ParamType::Str, true, "사용자에게 할 질문", "question to ask the user")],
        desc_ko: "명확화 질문(명령이 모호할 때 — 다른 op 없이 단독)",
        desc_en: "ask a clarification question (alone, when the command is ambiguous)",
    },
    OpSpec { kind: OpKind::Noop, params: &[], desc_ko: "아무 것도 하지 않음", desc_en: "do nothing" },
    // -- 0.6.0: every GUI function as an op (plan §1) ------------------------------------------
    OpSpec {
        kind: OpKind::Rotate,
        params: &[
            p("degrees", ParamType::Int, true, "회전 각도(90·180·270)", "degrees (90, 180 or 270)"),
            p("pages", ParamType::IntList, false, "쪽 번호(비우면 전체)", "page numbers (empty = all)"),
        ],
        desc_ko: "범위 문서의 쪽 이미지 회전(원본 불변)",
        desc_en: "rotate the page images of the documents in scope",
    },
    OpSpec {
        kind: OpKind::Rerender,
        params: &[p("dpi", ParamType::Int, false, "렌더 DPI(72~600)", "render DPI (72–600)")],
        desc_ko: "범위 문서 다시 렌더",
        desc_en: "re-render the documents in scope",
    },
    OpSpec {
        kind: OpKind::Split,
        params: &[
            p("boundaries", ParamType::IntList, false, "새 문서가 시작하는 쪽 번호(2 이상)", "pages that start a new document (≥ 2)"),
            p("every", ParamType::Int, false, "n쪽마다 나누기", "split every n pages"),
        ],
        desc_ko: "문서 1개를 여러 PDF 문서로 분할",
        desc_en: "split one document into several PDF documents",
    },
    OpSpec {
        kind: OpKind::Merge,
        params: &[p("name", ParamType::Str, false, "새 문서 이름", "name of the merged document")],
        desc_ko: "범위 문서(2~20개)를 PDF 하나로 병합",
        desc_en: "merge the documents in scope (2–20) into one PDF",
    },
    OpSpec {
        kind: OpKind::SetTitle,
        params: &[p("title", ParamType::Str, true, "새 제목", "new title")],
        desc_ko: "문서 제목 바꾸기(문서 1개)",
        desc_en: "set the title of the one document in scope",
    },
    OpSpec {
        kind: OpKind::EditText,
        params: &[
            p("find", ParamType::Str, true, "찾을 글자", "text to find"),
            p("replace", ParamType::Str, true, "바꿀 글자", "replacement"),
            p("pages", ParamType::IntList, false, "쪽 번호(비우면 전체)", "page numbers (empty = all)"),
        ],
        desc_ko: "범위 문서의 쪽 글자 찾아 바꾸기(이력에 기록)",
        desc_en: "find / replace in the page text of the documents in scope",
    },
    OpSpec {
        kind: OpKind::SetRole,
        params: &[p("role", ParamType::Enum(ROLES), true, "업무 문서 / 참고 자료 / 양식", "work / knowledge (reference) / form")],
        desc_ko: "문서 역할 표시(업무 문서·참고 자료·양식)",
        desc_en: "mark documents as work documents, reference material or forms",
    },
    OpSpec {
        kind: OpKind::ManageReference,
        params: &[
            p("action", ParamType::Enum(REFERENCE_ACTIONS), true, "만들기·이름 변경·삭제·사용·사용 안 함·문서 추가·문서 제외", "create / rename / delete / enable / disable / add / remove"),
            p("group", ParamType::Str, false, "묶음 이름 또는 ID(공통 참고 자료)", "group name or id (common reference material)"),
            p("name", ParamType::Str, false, "새 이름(create·rename)", "new name (create / rename)"),
            p("project", ParamType::Str, false, "프로젝트 이름/ID(그 프로젝트의 참고 자료 묶음)", "project name / id (its reference group)"),
        ],
        desc_ko: "참고 자료 묶음 관리; add/remove는 범위 문서(add는 참고 자료로 표시)",
        desc_en: "manage reference groups; add / remove take the documents in scope",
    },
    OpSpec {
        kind: OpKind::DefineTerm,
        params: &[
            p("action", ParamType::Enum(SAVE_DELETE), false, "저장(기본)/삭제", "save (default) / delete"),
            p("term", ParamType::Str, true, "용어", "term"),
            p("definition", ParamType::Str, false, "뜻", "definition"),
            p("synonyms", ParamType::StrList, false, "동의어", "synonyms"),
        ],
        desc_ko: "용어집 등록/삭제(검색 질의 확장에 쓰임)",
        desc_en: "save / delete a glossary term (used for query expansion)",
    },
    OpSpec {
        kind: OpKind::SetVersion,
        params: &[
            p("group_key", ParamType::Str, false, "같은 제목(판) 묶음 키", "title group key"),
            p("label", ParamType::Str, false, "판 라벨(예: 2024)", "edition label"),
            p("effective_date", ParamType::Str, false, "시행일 YYYY-MM-DD", "effective date YYYY-MM-DD"),
            p("latest", ParamType::Bool, false, "최신본 여부(기본 true)", "latest edition (default true)"),
            p("clear", ParamType::Bool, false, "버전 표시 해제", "clear the version mark"),
        ],
        desc_ko: "범위 문서를 어떤 제목의 판(신·구판)으로 표시",
        desc_en: "mark the documents in scope as editions of a title",
    },
    OpSpec {
        kind: OpKind::UpdateProject,
        params: &[
            p("project", ParamType::Str, false, "프로젝트 이름/ID(기본: 열린 프로젝트)", "project name / id (default: open project)"),
            p("name", ParamType::Str, false, "새 이름", "new name"),
            p("template", ParamType::Str, false, "템플릿 이름/ID(빈 문자열 = 해제)", "template name / id (empty = none)"),
            p("start_date", ParamType::Str, false, "시작일 YYYY-MM-DD(빈 문자열 = 지움)", "start date (empty clears)"),
            p("end_date", ParamType::Str, false, "종료일 YYYY-MM-DD(빈 문자열 = 지움)", "end date (empty clears)"),
            p("owner", ParamType::Str, false, "담당자", "owner"),
            p("intake_no", ParamType::Str, false, "접수번호", "intake number"),
            p("notes", ParamType::Str, false, "메모", "notes"),
            p("keywords", ParamType::StrList, false, "분류 키워드", "classification keywords"),
            p("inbox_folder", ParamType::Str, false, "inbox 하위 폴더", "inbox subfolder"),
        ],
        desc_ko: "프로젝트 정보 수정(이름·템플릿·기간·담당·메모·키워드)",
        desc_en: "update a project (name, template, dates, owner, notes, keywords)",
    },
    OpSpec {
        kind: OpKind::DeleteProject,
        params: &[p("project", ParamType::Str, false, "프로젝트 이름/ID(기본: 열린 프로젝트)", "project name / id (default: open project)")],
        desc_ko: "프로젝트 삭제(문서는 문서함에 남음)",
        desc_en: "delete a project (documents stay in the library)",
    },
    OpSpec {
        kind: OpKind::RemoveFromProject,
        params: &[
            p("project", ParamType::Str, false, "프로젝트 이름/ID(기본: 열린 프로젝트)", "project name / id (default: open project)"),
            p("doc_ids", ParamType::StrList, false, "뺄 문서 id(기본: 범위의 문서)", "document ids (default: documents in scope)"),
        ],
        desc_ko: "문서를 프로젝트에서 빼기",
        desc_en: "remove documents from a project",
    },
    OpSpec {
        kind: OpKind::ManageTemplate,
        params: &[
            p("action", ParamType::Enum(SAVE_DELETE), false, "저장(기본)/삭제", "save (default) / delete"),
            p("template", ParamType::Str, false, "기존 템플릿 이름/ID", "existing template name / id"),
            p("name", ParamType::Str, false, "(새) 이름", "(new) name"),
            p("doc_types_required", ParamType::StrList, false, "필수 문서 유형 슬러그", "required document-type slugs"),
            p("description", ParamType::Str, false, "설명", "description"),
        ],
        desc_ko: "프로젝트 템플릿 저장/삭제(내장 템플릿 제외)",
        desc_en: "save / delete a project template (built-ins excluded)",
    },
    OpSpec {
        kind: OpKind::ClassifyProjects,
        params: &[
            p("apply", ParamType::Bool, false, "제안을 바로 배정(기본 false = 제안만)", "assign the suggestions (default false = suggest only)"),
            p("use_model", ParamType::Bool, false, "규칙이 못 정한 문서를 모델에 묻기", "ask the model about undecided documents"),
        ],
        desc_ko: "범위 문서(비우면 미배정 전체)를 프로젝트로 자동 분류",
        desc_en: "auto-classify the documents in scope (empty = all unassigned) into projects",
    },
    OpSpec {
        kind: OpKind::ManageSchema,
        params: &[
            p("action", ParamType::Enum(SCHEMA_ACTIONS), true, "사용·사용 안 함·기본값·삭제·이름 변경", "enable / disable / set_default / delete / rename"),
            p("schema", ParamType::Str, true, "양식(추출 항목) 이름·ID·유형", "schema name / id / type"),
            p("name", ParamType::Str, false, "새 이름(rename)", "new name (rename)"),
        ],
        desc_ko: "양식·추출 항목 관리",
        desc_en: "manage extraction schemas (forms)",
    },
    OpSpec {
        kind: OpKind::ManageModel,
        params: &[
            p("action", ParamType::Enum(MODEL_ACTIONS), true, "티어·백엔드·가져오기·다운로드·삭제·해시 검증·벤치마크·로드·언로드", "set_tier / set_backend / import / download / delete / verify / benchmark / load / unload"),
            p("id", ParamType::Str, false, "모델 ID", "model id"),
            p("tier", ParamType::Enum(MODEL_TIERS), false, "티어(set_tier)", "tier (set_tier)"),
            p("backend", ParamType::Enum(super::settings_catalog::BACKENDS), false, "백엔드(set_backend)", "backend (set_backend)"),
            p("path", ParamType::Str, false, "모델 폴더/GGUF 경로(import)", "model folder / GGUF path (import)"),
        ],
        desc_ko: "AI 모델 관리(설정 > AI 모델)",
        desc_en: "manage AI models (settings > AI models)",
    },
    OpSpec {
        kind: OpKind::Workspace,
        params: &[
            p("action", ParamType::Enum(WORKSPACE_ACTIONS), true, "열기·만들기·닫기·목록에서 지우기", "open / create / close / forget"),
            p("path", ParamType::Str, false, "워크스페이스 폴더 경로", "workspace folder path"),
        ],
        desc_ko: "워크스페이스 열기/만들기/닫기",
        desc_en: "open / create / close a workspace",
    },
    OpSpec {
        kind: OpKind::Maintenance,
        params: &[p("action", ParamType::Enum(MAINTENANCE_ACTIONS), true, "지금 파기·끝난 작업 지우기·다시 검사·inbox 검사", "purge now / clear finished jobs / rescan / inbox scan")],
        desc_ko: "유지보수 실행(보존 파기·작업 정리·다시 검사)",
        desc_en: "maintenance (retention purge, clear finished jobs, rescan)",
    },
];

/// Specs of the given op kinds in `OP_SPECS` order (two-stage planning: one family's ops).
pub fn op_specs_for(kinds: &[OpKind]) -> Vec<&'static OpSpec> {
    OP_SPECS.iter().filter(|s| kinds.contains(&s.kind)).collect()
}

/// The family-selection reply of the two-stage planner: one or two families.
pub fn family_schema() -> Value {
    let families: Vec<Value> = super::OpFamily::ALL.iter().map(|f| Value::String(f.as_str().into())).collect();
    json!({
        "type": "object",
        "properties": {
            "families": { "type": "array", "items": { "enum": families }, "minItems": 1, "maxItems": 2 }
        },
        "required": ["families"],
        "additionalProperties": false
    })
}

pub fn spec(kind: OpKind) -> &'static OpSpec {
    OP_SPECS.iter().find(|s| s.kind == kind).expect("every op kind has a spec")
}

fn type_schema(ty: ParamType) -> Value {
    match ty {
        ParamType::Str => json!({ "type": "string" }),
        ParamType::Bool => json!({ "type": "boolean" }),
        ParamType::Int => json!({ "type": "integer" }),
        ParamType::Number => json!({ "type": "number" }),
        ParamType::Date => json!({ "type": "string", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$" }),
        ParamType::Enum(values) => json!({ "enum": values }),
        ParamType::StrList => json!({ "type": "array", "items": { "type": "string" }, "maxItems": 40 }),
        ParamType::IntList => json!({ "type": "array", "items": { "type": "integer" }, "maxItems": 200 }),
        ParamType::EnumList(values) => {
            json!({ "type": "array", "items": { "enum": values }, "maxItems": values.len() })
        }
        ParamType::Scalar => json!({ "anyOf": [{ "type": "string" }, { "type": "number" }, { "type": "boolean" }] }),
    }
}

/// Grammar-safe schema of one op object (`op` const + its parameters).
pub fn op_schema(spec: &OpSpec) -> Value {
    let mut props = Map::new();
    props.insert("op".into(), json!({ "const": spec.kind.as_str() }));
    let mut required = vec![Value::String("op".into())];
    for param in spec.params {
        props.insert(param.name.into(), type_schema(param.ty));
        if param.required {
            required.push(Value::String(param.name.into()));
        }
    }
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

pub fn scope_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "folder": { "type": "string" },
            "doc_ids": { "type": "array", "items": { "type": "string" }, "maxItems": 200 },
            "filter": {
                "type": "object",
                "properties": {
                    "doc_type": { "type": "string" },
                    "date_from": type_schema(ParamType::Date),
                    "date_to": type_schema(ParamType::Date),
                    "keyword": { "type": "string" },
                    "vendor": { "type": "string" },
                    "status": { "enum": DOC_STATUSES },
                    "tags": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
                    "min_confidence": { "type": "number" },
                    "review_status": { "enum": ["none", "pending", "done"] }
                },
                "additionalProperties": false
            }
        },
        "additionalProperties": false
    })
}

/// The PlanSchema (§3.5) for `response_format`. `summary` comes first so the
/// model states its interpretation before committing to operations.
pub fn plan_schema() -> Value {
    plan_schema_for(&OpKind::ALL)
}

/// The PlanSchema restricted to `kinds` (the chosen families' ops; 0.6.0 two-stage planning).
pub fn plan_schema_for(kinds: &[OpKind]) -> Value {
    let intents: Vec<Value> = kinds.iter().map(|k| Value::String(k.as_str().into())).collect();
    let ops: Vec<Value> = op_specs_for(kinds).into_iter().map(op_schema).collect();
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "intent": { "enum": intents },
            "scope": scope_schema(),
            "operations": { "type": "array", "items": { "anyOf": ops }, "minItems": 1, "maxItems": super::MAX_OPS },
            "requires_confirmation": { "type": "boolean" },
            "confidence": { "type": "number" },
            "clarification_question": { "type": "string" }
        },
        "required": ["summary", "intent", "scope", "operations", "requires_confirmation", "confidence"],
        "additionalProperties": false
    })
}

fn type_label(ty: ParamType) -> String {
    match ty {
        ParamType::Str => "string".into(),
        ParamType::Bool => "boolean".into(),
        ParamType::Int => "integer".into(),
        ParamType::Number => "number".into(),
        ParamType::Date => "YYYY-MM-DD".into(),
        ParamType::Enum(v) => v.join("|"),
        ParamType::StrList => "string[]".into(),
        ParamType::IntList => "integer[]".into(),
        ParamType::EnumList(v) => format!("({})[]", v.join("|")),
        ParamType::Scalar => "string|number|boolean".into(),
    }
}

/// The smallest op table: names with their required parameters (minimal prompts).
pub fn op_names() -> String {
    op_names_for(&OpKind::ALL)
}

pub fn op_names_for(kinds: &[OpKind]) -> String {
    let mut out = String::new();
    for s in op_specs_for(kinds) {
        let req: Vec<String> =
            s.params.iter().filter(|p| p.required).map(|p| format!("{}:{}", p.name, type_label(p.ty))).collect();
        let opt: Vec<&str> = s.params.iter().filter(|p| !p.required).map(|p| p.name).collect();
        out.push_str(&format!(
            "- {}{}{}\n",
            s.kind.as_str(),
            if req.is_empty() { String::new() } else { format!(" [{}]", req.join(", ")) },
            if opt.is_empty() { String::new() } else { format!(" (?{})", opt.join(" ")) }
        ));
    }
    out
}

/// Tool description table for the planner prompt (one line per op).
/// `compact` drops the parameter descriptions (small-context presets).
pub fn op_table(ko: bool, compact: bool) -> String {
    op_table_for(&OpKind::ALL, ko, compact)
}

pub fn op_table_for(kinds: &[OpKind], ko: bool, compact: bool) -> String {
    let mut out = String::new();
    for s in op_specs_for(kinds) {
        let params: Vec<String> = s
            .params
            .iter()
            .map(|p| {
                let req = if p.required { "" } else { "?" };
                if compact {
                    format!("{}{req}:{}", p.name, type_label(p.ty))
                } else {
                    format!("{}{req}:{} — {}", p.name, type_label(p.ty), if ko { p.desc_ko } else { p.desc_en })
                }
            })
            .collect();
        let desc = if ko { s.desc_ko } else { s.desc_en };
        if params.is_empty() {
            out.push_str(&format!("- {}: {desc}\n", s.kind.as_str()));
        } else {
            out.push_str(&format!("- {}: {desc} [{}]\n", s.kind.as_str(), params.join("; ")));
        }
    }
    out
}
