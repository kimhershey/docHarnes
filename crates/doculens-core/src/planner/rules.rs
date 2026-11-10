//! Deterministic phrase rules for the frequent single-intent commands (§3.5
//! "Pre-router … 단순 → 즉시 실행"): totals, exports, inbox processing,
//! organising, processing a selection, search, undo and job control. They are
//! conservative — anything they do not recognise goes to the model.

use crate::generate::{Length, OutputFormat, Tone};
use crate::prerouter::{self, DateRange, PreRoutedOp};
use crate::settings::{Language, Profile};

use super::{
    compact, AggregateFn, AutoPipeline, DocTypeSummary, EngineOp, ExplainScope, ExportFormat, ExportTarget,
    FieldSummary, GroupBy, ImportFrom, JobOp, OcrProfile, Op, OpKind, OpenTarget, Plan, PlanContext, RetentionTarget,
    ReviewOnly, ReviewOp, SchemaMode, SchemaSummary, Scope, SettingKey, SummarizeTarget, SummaryLength,
};

/// Default folder templates per grouping (design §3.7 정리 화면).
pub fn folder_template(group: GroupBy) -> &'static str {
    match group {
        GroupBy::Month => "documents/{{year}}/{{month}}/{{doc_type}}",
        GroupBy::Vendor => "documents/{{vendor}}/{{doc_type}}",
        GroupBy::DocType => "documents/{{doc_type}}",
        GroupBy::None => "documents/{{doc_type}}",
    }
}

/// Default file-name template (`2026-10-14_영수증_스타벅스_12,500.jpg`).
pub const NAME_TEMPLATE: &str = "{{year}}-{{month}}-{{day}}_{{doc_type}}_{{vendor}}_{{total}}";

struct Text<'a> {
    raw: &'a str,
    lower: String,
    compact: String,
    /// Lower-case with every run of punctuation / whitespace collapsed to one space and a
    /// space at both ends, so Latin triggers can be matched on word boundaries.
    norm: String,
    ko: bool,
}

impl<'a> Text<'a> {
    fn new(raw: &'a str, ko: bool) -> Text<'a> {
        let lower = raw.to_lowercase();
        let mut norm = String::from(" ");
        let mut prev_space = true;
        let mut prev_ascii: Option<bool> = None;
        for c in lower.chars() {
            if c.is_alphanumeric() {
                // A Korean particle glued to a Latin word ("csv로", "inbox에") ends the word.
                if !prev_space && prev_ascii.is_some_and(|a| a != c.is_ascii()) {
                    norm.push(' ');
                }
                norm.push(c);
                prev_space = false;
                prev_ascii = Some(c.is_ascii());
            } else if !prev_space {
                norm.push(' ');
                prev_space = true;
                prev_ascii = None;
            }
        }
        if !norm.ends_with(' ') {
            norm.push(' ');
        }
        let hangul = raw.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c));
        Text { raw, lower, compact: compact(raw), norm, ko: ko || hangul }
    }

    /// Trigger matching. Korean (and other non-ASCII) triggers are substrings of the compacted
    /// text; ASCII triggers match whole words (`"move"` does not match `remove`), a trailing `*`
    /// allows a word prefix (`"classif*"`), and ASCII phrases / long glued tokens match the
    /// compacted text (`"how much"`, `"includeunreviewed"`).
    fn has(&self, words: &[&str]) -> bool {
        words.iter().any(|w| self.has_one(w))
    }

    fn has_one(&self, w: &str) -> bool {
        let (key, prefix) = match w.strip_suffix('*') {
            Some(k) => (k, true),
            None => (w, false),
        };
        if !key.is_ascii() {
            return self.compact.contains(&compact(key));
        }
        let key = key.trim().to_lowercase();
        if key.is_empty() {
            return false;
        }
        if key.chars().any(|c| !c.is_alphanumeric() && c != ' ') {
            return self.lower.contains(&key); // ".pdf", "12-3" — punctuation is gone from `norm`
        }
        if key.contains(' ') || key.len() >= 9 {
            return self.compact.contains(&compact(&key));
        }
        let needle = format!(" {key}");
        let mut from = 0;
        while let Some(pos) = self.norm[from..].find(&needle) {
            let end = from + pos + needle.len();
            if prefix || self.norm[end..].starts_with(' ') {
                return true;
            }
            from += pos + 1;
        }
        false
    }

    /// A question about documents (what / how / 알려줘 / …?) rather than a request to act on them;
    /// polite request forms ("…해 줄래?", "can you …") still count as commands.
    fn is_question(&self) -> bool {
        if self.has(&["줄래", "줄수", "해줄", "해줘", "해주", "부탁", "please", "can you", "could you", "would you"])
        {
            return false;
        }
        self.raw.trim_end().ends_with(['?', '？'])
            || self.has(&[
                "알려",
                "뭐야",
                "무엇",
                "어떻게",
                "어디",
                "언제",
                "누구",
                "왜",
                "인가요",
                "있어",
                "있나",
                "되나",
                "가능해",
                "될까",
                "무슨",
                "어느",
                "explain",
                "what",
                "how",
                "when",
                "where",
                "why",
                "which",
                "who",
                "is it",
                "does",
                "do i",
            ])
    }
    fn t<'b>(&self, ko: &'b str, en: &'b str) -> &'b str {
        if self.ko {
            ko
        } else {
            en
        }
    }
}

/// True for sentences that chain several commands ("…하고 …", "… and …").
pub fn is_compound(text: &str) -> bool {
    let t = text.trim();
    t.contains(" and ") || t.contains("그리고") || t.contains("고 ") || t.contains(", ")
}

fn split_clauses(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for piece in text.split(" and ").flat_map(|p| p.split("그리고")).flat_map(|p| p.split(", ")) {
        // Korean "-고 " connective: "바꾸고 모델을…" → "바꾸", "모델을…".
        let mut rest = piece;
        while let Some(idx) = rest.find("고 ") {
            let (head, tail) = rest.split_at(idx);
            if !head.trim().is_empty() {
                parts.push(head.trim().to_string());
            }
            rest = tail["고 ".len()..].trim_start();
        }
        if !rest.trim().is_empty() {
            parts.push(rest.trim().to_string());
        }
    }
    parts
}

const SELECTION_WORDS: &[&str] = &[
    "이문서",
    "이계약서",
    "이영수증",
    "이파일",
    "선택문서",
    "선택한",
    "선택된",
    "현재문서",
    "열린문서",
    "지금문서",
    "선택영역",
    "이페이지",
    "현재페이지",
    "this document",
    "this file",
    "this page",
    "selected",
    "the selection",
    "these documents",
    "current document",
    "this contract",
    "this receipt",
];
const ALL_WORDS: &[&str] = &["전부", "전체", "모두", "모든", "라이브러리전체", "all documents", "everything", "all"];

/// Whether the sentence explicitly targets the whole library ("전부", "모든 문서", "all documents").
pub fn mentions_all(text: &str) -> bool {
    Text::new(text, true).has(ALL_WORDS)
}

fn selection_ids(ctx: &PlanContext) -> Vec<String> {
    if !ctx.selected_doc_ids.is_empty() {
        ctx.selected_doc_ids.clone()
    } else {
        ctx.current_document.iter().cloned().collect()
    }
}

/// Longest document-type label (either language, or the slug) mentioned in the text.
fn doc_type<'a>(t: &Text<'_>, ctx: &'a PlanContext) -> Option<&'a DocTypeSummary> {
    let mut best: Option<(&DocTypeSummary, usize)> = None;
    for d in &ctx.doc_types {
        for label in [d.label_ko.as_str(), d.label_en.as_str(), d.slug.as_str()] {
            let key = compact(label);
            if key.chars().count() >= 2 && t.compact.contains(&key) {
                let len = key.chars().count();
                if best.is_none_or(|(_, l)| len > l) {
                    best = Some((d, len));
                }
            }
        }
    }
    best.map(|(d, _)| d)
}

/// Fields of a schema mentioned in the text, in order of appearance.
fn fields_mentioned<'a>(t: &Text<'_>, schema: &'a SchemaSummary) -> Vec<&'a FieldSummary> {
    let mut found: Vec<(usize, usize, &FieldSummary)> = Vec::new();
    for f in &schema.fields {
        // The label without a trailing generic word still identifies the field:
        // "공급가액 합계" ← "공급가액", "세액 합계" ← "세액", "합계금액" ← "합계".
        let trimmed = ["합계", "금액", "번호", "일자", "총액", "명", "액"]
            .iter()
            .find_map(|g| f.label_ko.trim_end().strip_suffix(g))
            .map(str::trim)
            .filter(|s| s.chars().count() >= 2)
            .unwrap_or("");
        let mut best: Option<(usize, usize)> = None;
        for label in [f.label_ko.as_str(), f.label_en.as_str(), f.key.as_str(), trimmed] {
            let key = compact(label);
            if key.chars().count() < 2 {
                continue;
            }
            if let Some(pos) = t.compact.find(&key) {
                // Prefer the shortest matching form so a generic tail ("합계") is not swallowed.
                let len = key.len();
                if best.is_none_or(|(_, l)| len < l) {
                    best = Some((pos, len));
                }
            }
        }
        if let Some((pos, len)) = best {
            found.push((pos, len, f));
        }
    }
    found.sort_by_key(|(pos, len, _)| (*pos, std::cmp::Reverse(*len)));
    // Drop matches nested inside an earlier one.
    let mut out: Vec<&FieldSummary> = Vec::new();
    let mut covered_until = 0usize;
    for (pos, len, f) in found {
        if pos < covered_until {
            continue;
        }
        covered_until = pos + len;
        out.push(f);
    }
    out
}

fn total_field(schema: &SchemaSummary) -> Option<&FieldSummary> {
    let amounts: Vec<&FieldSummary> = schema.fields.iter().filter(|f| f.kind == "amount").collect();
    amounts
        .iter()
        .find(|f| f.key.contains("total") || f.label_ko.contains("합계") || f.label_ko.contains("총액"))
        .or_else(|| amounts.first())
        .copied()
}

fn date(t: &Text<'_>, ctx: &PlanContext) -> Option<DateRange> {
    prerouter::resolve_relative_date(t.raw, ctx.today)
}

fn apply_date(scope: &mut Scope, d: Option<&DateRange>) {
    if let Some(d) = d {
        scope.filter.date_from = Some(d.from.to_string());
        scope.filter.date_to = Some(d.to.to_string());
    }
}

/// Scope from the sentence: selection words → selected ids; `inbox` → that folder;
/// otherwise the type / date filter (and the current folder for "여기").
/// "이 contract", "이 견적서", "this quotation": the open / selected document of that type.
fn refers_to_open_doc(t: &Text<'_>, ctx: &PlanContext) -> bool {
    if selection_ids(ctx).is_empty() {
        return false;
    }
    ctx.doc_types.iter().any(|d| {
        [d.label_ko.as_str(), d.label_en.as_str(), d.slug.as_str()].iter().any(|l| {
            let k = compact(l);
            k.chars().count() >= 2
                && (t.compact.contains(&format!("이{k}"))
                    || t.compact.contains(&format!("this{k}"))
                    || t.compact.contains(&format!("현재{k}")))
        })
    })
}

fn scope_of(t: &Text<'_>, ctx: &PlanContext, dt: Option<&DocTypeSummary>, d: Option<&DateRange>) -> Scope {
    let mut scope = Scope::default();
    if t.has(SELECTION_WORDS) || refers_to_open_doc(t, ctx) {
        scope.doc_ids = selection_ids(ctx);
    }
    if t.has(&["inbox", "인박스", "받은함"]) {
        scope.folder = Some("inbox".into());
    } else if t.has(&["이폴더", "여기있는", "현재폴더", "this folder", "current folder", "here"]) {
        scope.folder = ctx.current_folder.clone();
    }
    if let Some(dt) = dt {
        scope.filter.doc_type = Some(dt.slug.clone());
    }
    apply_date(&mut scope, d);
    scope
}

fn mk(op: Op, scope: Scope, confidence: f32, summary: String) -> Plan {
    let mut p = Plan::single(op, confidence, summary);
    p.scope = scope;
    p
}

fn mk_many(ops: Vec<Op>, scope: Scope, confidence: f32, summary: String) -> Plan {
    let mut p = Plan {
        intent: ops.first().map(Op::kind).unwrap_or(OpKind::Noop),
        scope,
        operations: ops,
        requires_confirmation: false,
        confidence,
        clarification_question: None,
        summary: Some(summary),
    };
    p.requires_confirmation = p.operations.iter().any(Op::needs_confirmation);
    p
}

fn type_label<'a>(dt: Option<&'a DocTypeSummary>, ko: bool, fallback: &'a str) -> &'a str {
    match dt {
        Some(d) => {
            if ko {
                &d.label_ko
            } else {
                &d.label_en
            }
        }
        None => fallback,
    }
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

fn undo_redo(t: &Text<'_>) -> Option<Plan> {
    if t.has(&["되돌려", "되돌리", "되돌림", "원래대로", "undo", "revert"]) {
        return Some(mk(
            Op::Undo { target: None },
            Scope::default(),
            0.95,
            t.t("마지막 파일 작업을 되돌립니다", "Undo the last file batch").into(),
        ));
    }
    if t.has(&["다시실행", "redo"]) && !t.has(&["추출", "ocr", "분류", "처리", "extract*", "classify", "process*"])
    {
        return Some(mk(
            Op::Redo {},
            Scope::default(),
            0.95,
            t.t("되돌린 작업을 다시 실행합니다", "Redo the last undone batch").into(),
        ));
    }
    None
}

fn job_control(t: &Text<'_>) -> Option<Plan> {
    let job_word = t.has(&["작업", "job", "배치", "batch", "처리를", "큐"]);
    // "pause the job" / "작업 일시정지" — the bare Korean nouns are unambiguous, the English verbs
    // ("hold", "resume", "continue") are everyday words and need the job word.
    let action = if t.has(&["일시정지", "일시중지", "잠깐멈"]) || (job_word && t.has(&["멈춰", "pause", "hold"]))
    {
        JobOp::Pause
    } else if t.has(&["재개"]) || (job_word && t.has(&["계속해", "이어서", "resume", "continue"])) {
        JobOp::Resume
    } else if t.has(&["재시도", "다시시도"]) || (job_word && t.has(&["retry"])) {
        JobOp::Retry
    } else if job_word && t.has(&["우선", "먼저해", "prioriti*", "first please", "bump up"]) {
        JobOp::Prioritize
    } else if job_word && t.has(&["취소", "중단", "cancel", "stop", "abort"]) {
        JobOp::Cancel
    } else if job_word && t.has(&["보여", "목록", "현황", "상태", "show", "list", "status"]) {
        return Some(mk(
            Op::Open { target: OpenTarget::Screen, reference: "jobs".into() },
            Scope::default(),
            0.9,
            t.t("작업 화면을 엽니다", "Open the jobs screen").into(),
        ));
    } else {
        return None;
    };
    let what = match action {
        JobOp::Pause => t.t("실행 중인 작업을 일시정지합니다", "Pause the running job"),
        JobOp::Resume => t.t("일시정지한 작업을 재개합니다", "Resume the paused job"),
        JobOp::Retry => t.t("실패한 작업을 재시도합니다", "Retry the failed job"),
        JobOp::Prioritize => t.t("대기 중인 작업을 앞으로 당깁니다", "Move the queued job to the front"),
        JobOp::Cancel => t.t("실행 중인 작업을 취소합니다", "Cancel the running job"),
        JobOp::Show => "",
    };
    Some(mk(Op::Job { action, id: Some("last".into()) }, Scope::default(), 0.92, what.into()))
}

fn engine(t: &Text<'_>) -> Option<Plan> {
    if !t.has(&["엔진", "engine", "llama"]) {
        return None;
    }
    let action = if t.has(&["재시작", "다시시작", "restart", "reboot"]) {
        EngineOp::Restart
    } else if t.has(&["슬립", "재워", "sleep", "잠재"]) {
        EngineOp::Sleep
    } else if t.has(&["깨워", "wake"]) {
        EngineOp::Wake
    } else {
        return None;
    };
    Some(mk(Op::Engine { action }, Scope::default(), 0.9, t.t("엔진을 제어합니다", "Control the engine").into()))
}

fn model_id(t: &Text<'_>, ctx: &PlanContext) -> Option<String> {
    if !t.has(&["모델", "model"]) || ctx.model_ids.is_empty() {
        return None;
    }
    // "9B", "4b", "q4" or a chunk of the id itself.
    let mut best: Option<(&String, usize)> = None;
    for id in &ctx.model_ids {
        let key = compact(id);
        if t.compact.contains(&key) {
            return Some(id.clone());
        }
        for part in id.split(['-', '_', '.']).map(compact).filter(|p| p.len() >= 2) {
            if part.chars().any(|c| c.is_ascii_digit())
                && t.compact.contains(&part)
                && best.is_none_or(|(_, l)| part.len() > l)
            {
                best = Some((id, part.len()));
            }
        }
    }
    best.map(|(id, _)| id.clone())
}

/// Several settings in one sentence ("다크 테마로 바꾸고 모델을 9B로 올려").
fn settings_combo(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    let clauses = split_clauses(t.raw);
    let mut ops = Vec::new();
    for clause in &clauses {
        let ct = Text::new(clause, t.ko);
        if let Some(id) = model_id(&ct, ctx) {
            ops.push(Op::SetModel { id });
            continue;
        }
        match prerouter::route(clause).op {
            Some(PreRoutedOp::SetTheme { theme }) => ops.push(Op::SetTheme { theme }),
            Some(PreRoutedOp::SetLanguage { language }) => ops.push(Op::SetLanguage { lang: language }),
            Some(PreRoutedOp::Open { screen }) => ops.push(Op::Open {
                target: OpenTarget::Screen,
                reference: screen.route().trim_start_matches('/').to_string(),
            }),
            Some(PreRoutedOp::Help) => ops.push(Op::Help {}),
            None => return None,
        }
    }
    if ops.is_empty() || (ops.len() == 1 && !matches!(ops[0], Op::SetModel { .. })) {
        return None;
    }
    let summary = ops
        .iter()
        .map(|o| match o {
            Op::SetModel { id } => t.t(&format!("모델을 {id}(으)로"), &format!("model → {id}")).to_string(),
            Op::SetTheme { theme } => {
                t.t(&format!("테마를 {}(으)로", theme.as_str()), &format!("theme → {}", theme.as_str())).to_string()
            }
            Op::SetLanguage { lang } => {
                t.t(&format!("언어를 {}(으)로", lang.as_str()), &format!("language → {}", lang.as_str())).to_string()
            }
            Op::Open { reference, .. } => {
                t.t(&format!("{reference} 화면 열기"), &format!("open {reference}")).to_string()
            }
            _ => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    Some(mk_many(ops, Scope::default(), 0.9, summary))
}

fn define_schema(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    if !t.has(&["스키마", "schema", "양식필드", "추출필드"]) {
        return None;
    }
    if !t.has(&[
        "추가", "만들", "생성", "수정", "바꿔", "변경", "정의", "빼", "제거", "삭제", "add", "create", "modify",
        "change", "define", "new", "remove", "delete", "drop",
    ]) {
        return None;
    }
    let dt = doc_type(t, ctx);
    let schema_ref =
        dt.and_then(|d| ctx.schema_for_type(&d.slug)).map(|s| s.name.clone()).or_else(|| dt.map(|d| d.slug.clone()));
    let modify = schema_ref.is_some()
        && t.has(&[
            "추가", "수정", "바꿔", "변경", "빼", "제거", "삭제", "add", "modify", "change", "remove", "delete", "drop",
        ]);
    let mode = if modify { SchemaMode::Modify } else { SchemaMode::Create };
    let op = Op::DefineSchema {
        name: None,
        description: Some(t.raw.trim().to_string()),
        mode: Some(mode),
        base: if modify { schema_ref.clone() } else { None },
    };
    let summary = match (&mode, &schema_ref) {
        (SchemaMode::Modify, Some(s)) => t
            .t(
                &format!("{s} 스키마를 수정합니다(스키마 화면에서 확인)"),
                &format!("Modify the {s} schema (confirm on the schema screen)"),
            )
            .to_string(),
        _ => t
            .t(
                "새 추출 스키마를 만듭니다(스키마 화면에서 확인)",
                "Create a new extraction schema (confirm on the schema screen)",
            )
            .to_string(),
    };
    Some(mk(op, Scope::default(), 0.85, summary))
}

fn export_format(t: &Text<'_>) -> Option<ExportFormat> {
    if t.has(&["검색가능pdf", "검색가능한pdf", "searchablepdf", "텍스트레이어", "textlayer", "검색되는pdf"])
    {
        Some(ExportFormat::PdfSearchable)
    } else if t.has(&["엑셀", "excel", "xlsx", "스프레드시트", "spreadsheet"]) {
        Some(ExportFormat::Xlsx)
    } else if t.has(&["csv"]) {
        Some(ExportFormat::Csv)
    } else if t.has(&["jsonl", "json"]) {
        Some(ExportFormat::Jsonl)
    } else if t.has(&["마크다운", "markdown", "md파일", "md로", ".md"]) {
        Some(ExportFormat::Md)
    } else {
        None
    }
}

fn export(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    let format = export_format(t)?;
    if t.is_question() {
        return None;
    }
    // A verb, an arrow ("세금계산서 → 엑셀") or a terse "<유형> 엑셀로" (≤ 4 words).
    let words = t.raw.split_whitespace().count();
    let verb = t.has(&["내보내", "저장", "뽑아", "추출해서", "만들어", "출력", "export*", "save", "dump", "write"])
        || t.raw.contains("→")
        || t.raw.contains("->")
        || words <= 4;
    if !verb {
        return None;
    }
    let dt = doc_type(t, ctx);
    let d = date(t, ctx);
    let mut scope = scope_of(t, ctx, dt, d.as_ref());
    if scope.folder.is_none() && scope.doc_ids.is_empty() && dt.is_none() && d.is_none() && !t.has(ALL_WORDS) {
        scope.folder = ctx.current_folder.clone();
    }
    let target = if t.has(&["표를", "표만", "테이블", "table*"]) {
        ExportTarget::Table
    } else if t.has(&["텍스트", "본문", "ocr결과", "text"]) || format == ExportFormat::PdfSearchable {
        ExportTarget::Text
    } else {
        ExportTarget::Fields
    };
    let schema = dt.and_then(|d| ctx.schema_for_type(&d.slug));
    let columns: Vec<String> = match (schema, target) {
        (Some(s), ExportTarget::Fields) if !t.has(ALL_WORDS) => {
            fields_mentioned(t, s).into_iter().map(|f| f.key.clone()).collect()
        }
        _ => Vec::new(),
    };
    let mut ops = Vec::new();
    if !columns.is_empty() {
        ops.push(Op::Extract { schema_ref: dt.map(|d| d.slug.clone()), fields: columns.clone(), force: None });
    }
    ops.push(Op::Export {
        format,
        path: None,
        columns: columns.clone(),
        layout: None,
        include: Vec::new(),
        target: Some(target),
    });
    let label = type_label(dt, t.ko, t.t("문서", "documents"));
    let summary = if t.ko {
        match columns.len() {
            0 => format!("{label}을(를) {}(으)로 내보냅니다", format.as_str().to_uppercase()),
            n => format!("{label}에서 {n}개 필드를 추출해 {}(으)로 내보냅니다", format.as_str().to_uppercase()),
        }
    } else {
        match columns.len() {
            0 => format!("Export {label} as {}", format.as_str().to_uppercase()),
            n => format!("Extract {n} fields of {label} and export them as {}", format.as_str().to_uppercase()),
        }
    };
    Some(mk_many(ops, scope, 0.88, summary))
}

fn aggregate(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    const TRIGGERS: &[&str] = &[
        "합계",
        "총합",
        "총액",
        "총금액",
        "얼마",
        "집계",
        "평균",
        "최대",
        "최소",
        "건수",
        "몇건",
        "몇개",
        "개수",
        "count",
        "total",
        "sum",
        "average",
        "how much",
        "how many",
        "highest",
        "lowest",
        "largest",
        "smallest",
    ];
    if !t.has(TRIGGERS) {
        return None;
    }
    // "합계금액 추출해" asks for a field; "이 영수증 합계 금액이 얼마야?" asks about the open document.
    if t.has(&["추출", "뽑아", "extract*"]) || (t.has(SELECTION_WORDS) && t.is_question()) {
        return None;
    }
    // "합계 금액은?" on its own is a question about a document, not a library-wide total:
    // aggregate only when the sentence names a type, a period, a grouping or the whole library.
    let dt_hint = doc_type(t, ctx).is_some();
    let scoped = dt_hint
        || date(t, ctx).is_some()
        || t.has(&[
            "월별",
            "거래처별",
            "유형별",
            "monthly",
            "by vendor",
            "by type",
            "집계",
            "총합",
            "모든",
            "모두",
            "전부",
            "전체",
            "across",
            "all",
        ])
        || !selection_ids(ctx).is_empty() && t.has(SELECTION_WORDS);
    if !scoped {
        return None;
    }
    let func = if t.has(&["평균", "average", "avg", "mean"]) {
        AggregateFn::Avg
    } else if t.has(&["최대", "가장큰", "가장비싼", "max", "highest", "largest"]) {
        AggregateFn::Max
    } else if t.has(&["최소", "가장작은", "가장싼", "min", "lowest", "smallest"]) {
        AggregateFn::Min
    } else if t.has(&["건수", "몇건", "몇개", "개수", "count", "how many", "number of"]) {
        AggregateFn::Count
    } else {
        AggregateFn::Sum
    };
    let dt = doc_type(t, ctx);
    let d = date(t, ctx);
    let scope = scope_of(t, ctx, dt, d.as_ref());
    let schema = dt.and_then(|d| ctx.schema_for_type(&d.slug));
    let field = if func == AggregateFn::Count {
        None
    } else {
        match schema {
            Some(s) => {
                let mentioned: Vec<&FieldSummary> =
                    fields_mentioned(t, s).into_iter().filter(|f| f.kind == "amount" || f.kind == "integer").collect();
                mentioned.first().or(total_field(s).as_ref()).map(|f| f.key.clone())
            }
            None => None,
        }
    };
    let group_by = if t.has(&["월별", "달별", "monthly", "by month", "per month"]) {
        Some(GroupBy::Month)
    } else if t.has(&["거래처별", "업체별", "by vendor", "per vendor"]) {
        Some(GroupBy::Vendor)
    } else if t.has(&["유형별", "종류별", "by type", "per type"]) {
        Some(GroupBy::DocType)
    } else {
        None
    };
    let include_unreviewed = t
        .has(&[
            "검토대기포함",
            "검토미완료포함",
            "미검토포함",
            "대기포함",
            "include unreviewed",
            "including unreviewed",
        ])
        .then_some(true);
    let label = type_label(dt, t.ko, t.t("문서", "documents"));
    let fname = field.as_deref().and_then(|k| schema.and_then(|s| s.fields.iter().find(|f| f.key == k))).map(|f| {
        if t.ko {
            f.label_ko.clone()
        } else {
            f.label_en.clone()
        }
    });
    let when = d.as_ref().map(|d| format!("{} ~ {}", d.from, d.to));
    let summary = if t.ko {
        format!(
            "{}{label}의 {}{}",
            when.map(|w| format!("{w} ")).unwrap_or_default(),
            fname.clone().unwrap_or_else(|| "건수".into()),
            match func {
                AggregateFn::Sum => " 합계",
                AggregateFn::Count => "",
                AggregateFn::Avg => " 평균",
                AggregateFn::Min => " 최소값",
                AggregateFn::Max => " 최대값",
            }
        )
    } else {
        format!(
            "{} of {}{}{}",
            func.as_str(),
            fname.map(|f| format!("{f} of ")).unwrap_or_default(),
            label,
            when.map(|w| format!(" ({w})")).unwrap_or_default()
        )
    };
    Some(mk(Op::Aggregate { func, field, group_by, include_unreviewed }, scope, 0.9, summary))
}

fn inbox(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    if !t.has(&["inbox", "인박스", "받은함", "받은문서"]) || t.is_question() {
        return None;
    }
    if !t.has(&["처리", "가져", "불러", "import*", "process*", "정리", "분류", "추출", "ocr", "등록"]) {
        return None;
    }
    let dt = doc_type(t, ctx);
    let mut scope = Scope { folder: Some("inbox".into()), ..Scope::default() };
    if let Some(d) = dt {
        scope.filter.doc_type = Some(d.slug.clone());
    }
    let mut ops = vec![Op::Import {
        source: ImportFrom::Inbox,
        path: None,
        recursive: None,
        auto_pipeline: Some(AutoPipeline::None),
    }];
    let only_classify = t.has(&["분류만", "classify only"])
        || (t.has(&["분류", "classif*"])
            && !t.has(&["처리", "process*", "추출", "extract*", "ocr", "전부", "모두", "전체", "all"]));
    let only_ocr = t.has(&["ocr만", "ocr only"]);
    let wants_processing = t.has(&["처리", "process*", "추출", "extract*", "전부", "모두", "ocr", "분류", "classif*"]);
    if wants_processing || dt.is_some() {
        ops.push(Op::Classify { doc_types: Vec::new(), force: None });
        if !only_classify {
            ops.push(Op::Ocr { pages: Vec::new(), profile: None, force: None });
            if !only_ocr {
                ops.push(Op::Extract { schema_ref: dt.map(|d| d.slug.clone()), fields: Vec::new(), force: None });
            }
        }
    }
    let group = if t.has(&["월별", "달별", "monthly", "by month"]) {
        Some(GroupBy::Month)
    } else if t.has(&["거래처별", "업체별", "by vendor"]) {
        Some(GroupBy::Vendor)
    } else if t.has(&[
        "유형별",
        "종류별",
        "by type",
        "정리",
        "폴더로",
        "옮겨",
        "이동해",
        "이동시",
        "organi*",
        "move",
        "sort into",
    ]) {
        Some(GroupBy::DocType)
    } else {
        None
    };
    if let Some(g) = group {
        ops.push(Op::Move { folder: folder_template(g).into(), group_by: Some(g), only_reviewed: None });
    }
    let label = type_label(dt, t.ko, t.t("문서", "documents"));
    let summary = if t.ko {
        format!(
            "inbox의 {label}을(를) 가져와 {}{}",
            if ops.len() > 1 { "분류·OCR·추출한 뒤 " } else { "등록하고 " },
            match group {
                Some(GroupBy::Month) => "월별 폴더로 이동합니다",
                Some(GroupBy::Vendor) => "거래처별 폴더로 이동합니다",
                Some(_) => "유형별 폴더로 이동합니다",
                None => "마칩니다",
            }
        )
    } else {
        format!(
            "Import {label} from inbox{}{}",
            if ops.len() > 1 { ", classify, OCR and extract them" } else { "" },
            match group {
                Some(GroupBy::Month) => ", then move them into monthly folders",
                Some(GroupBy::Vendor) => ", then move them into vendor folders",
                Some(_) => ", then move them into type folders",
                None => "",
            }
        )
    };
    Some(mk_many(ops, scope, 0.88, summary))
}

fn organize(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    if t.is_question() || t.has(&["알려", "설명", "요약", "summar*", "tell", "explain"]) {
        return None; // "정리해서 알려줘" asks for a summary, not for file moves
    }
    let rename = t.has(&["이름바꿔", "이름을바꿔", "이름변경", "파일명", "rename*", "이름을"]);
    let organize_word = t.has(&[
        "정리",
        "organi*",
        "옮겨",
        "옮기",
        "이동해",
        "이동시",
        "이동하",
        "로이동",
        "move",
        "폴더로",
        "sort into",
        "tidy",
    ]);
    if !rename && !organize_word {
        return None;
    }
    if t.has(&["작업", "job", "스키마", "schema", "태그", "tag"])
        && !t.has(&[
            "문서",
            "파일",
            "document",
            "file",
            "영수증",
            "receipt",
            "견적서",
            "계약서",
            "회의록",
            "보고서",
            "estimate",
            "contract",
        ])
    {
        return None;
    }
    let dt = doc_type(t, ctx);
    let d = date(t, ctx);
    let scope = scope_of(t, ctx, dt, d.as_ref());
    let only_reviewed = if t.has(&["검토완료", "검토된", "reviewed only", "only reviewed", "reviewed"]) {
        Some(true)
    } else {
        None
    };
    let group = if t.has(&["월별", "달별", "monthly", "by month"]) {
        Some(GroupBy::Month)
    } else if t.has(&["거래처별", "업체별", "by vendor"]) {
        Some(GroupBy::Vendor)
    } else if t.has(&["유형별", "종류별", "by type"]) {
        Some(GroupBy::DocType)
    } else {
        None
    };
    // "X 폴더로 옮겨": an explicit folder name.
    let explicit = t.raw.split_whitespace().collect::<Vec<_>>().windows(2).find_map(|w| {
        let name = w[0].trim_matches(|c: char| "\"'“”‘’".contains(c));
        if compact(w[1]).starts_with("폴더")
            && !name.is_empty()
            && !t.has(&["월별", "거래처별", "유형별"])
            && doc_type_label_matches(ctx, name).is_none()
        {
            Some(name.to_string())
        } else {
            None
        }
    });
    let label = type_label(dt, t.ko, t.t("문서", "documents"));
    let (op, summary) = if rename && group.is_none() && explicit.is_none() {
        (
            Op::Rename { template: NAME_TEMPLATE.into(), only_reviewed },
            t.t(
                &format!("{label}의 파일명을 템플릿에 맞춰 바꿉니다"),
                &format!("Rename {label} with the name template"),
            )
            .to_string(),
        )
    } else if let Some(folder) = explicit {
        let folder = if folder.contains('/') { folder } else { format!("documents/{folder}") };
        (
            Op::Move { folder: folder.clone(), group_by: None, only_reviewed },
            t.t(&format!("{label}을(를) {folder}/ 로 이동합니다"), &format!("Move {label} to {folder}/")).to_string(),
        )
    } else if rename {
        let g = group.unwrap_or(GroupBy::DocType);
        (
            Op::Organize {
                template: Some(NAME_TEMPLATE.into()),
                folder_template: Some(folder_template(g).into()),
                group_by: Some(g),
                only_reviewed,
                dry_run: None,
            },
            t.t(
                &format!("{label}의 이름을 바꾸고 {} 폴더로 정리합니다", group_word(g, true)),
                &format!("Rename {label} and sort them into {} folders", group_word(g, false)),
            )
            .to_string(),
        )
    } else {
        let g = group.unwrap_or(GroupBy::DocType);
        (
            Op::Move { folder: folder_template(g).into(), group_by: Some(g), only_reviewed },
            t.t(
                &format!("{label}을(를) {} 폴더로 이동합니다", group_word(g, true)),
                &format!("Move {label} into {} folders", group_word(g, false)),
            )
            .to_string(),
        )
    };
    Some(mk(op, scope, 0.86, summary))
}

fn doc_type_label_matches<'a>(ctx: &'a PlanContext, name: &str) -> Option<&'a DocTypeSummary> {
    ctx.doc_type(name)
}

fn group_word(g: GroupBy, ko: bool) -> &'static str {
    match (g, ko) {
        (GroupBy::Month, true) => "월별",
        (GroupBy::Month, false) => "monthly",
        (GroupBy::Vendor, true) => "거래처별",
        (GroupBy::Vendor, false) => "vendor",
        (GroupBy::DocType | GroupBy::None, true) => "유형별",
        (GroupBy::DocType | GroupBy::None, false) => "type",
    }
}

fn tag(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    if !t.has(&["태그", "tag*"]) || t.is_question() {
        return None;
    }
    let remove = t.has(&["제거", "빼", "삭제", "지워", "remove", "untag", "delete"]);
    let add = t.has(&["추가", "붙여", "달아", "넣어", "add", "tag as", "tag with", "apply"])
        || (t.has(&["tag*"]) && t.lower.contains(" as "));
    if !remove && !add {
        return None;
    }
    // Tag names: the words between 태그/tag and the verb, quotes stripped.
    let raw = t.raw;
    let lower = t.lower.as_str();
    let start = lower.find("태그").map(|i| i + "태그".len()).or_else(|| lower.find("tag").map(|i| i + 3))?;
    let mut rest = &raw[start..];
    for stop in ["추가", "붙여", "달아", "넣어", "제거", "빼", "삭제", "지워", " add", " remove", " to ", " from "]
    {
        if let Some(i) = rest.find(stop) {
            rest = &rest[..i];
        }
    }
    // "tag these documents as urgent": the name is what follows " as ".
    if let Some(i) = rest.to_lowercase().find(" as ") {
        rest = &rest[i + 4..];
    }
    let clean = |text: &str| -> Vec<String> {
        text.split([',', '·', '/'])
            .map(|s| s.trim().trim_matches(|c: char| "\"'“”‘’()[]".contains(c)).trim())
            .map(|s| s.trim_end_matches(['을', '를', '로']).trim_start_matches(['를', '을']).trim().to_string())
            .filter(|s| !s.is_empty() && s.chars().count() <= 64)
            .collect()
    };
    let mut names: Vec<String> = clean(rest);
    if names.is_empty() {
        // "tag these documents as urgent" / "중요 태그 빼줘": the name follows " as " or precedes 태그.
        if let Some(i) = lower.find(" as ") {
            names = clean(&raw[i + 4..]);
        } else if let Some(i) = lower.find("태그") {
            let before: Vec<&str> = raw[..i].split_whitespace().collect();
            if let Some(w) = before.last() {
                let skip = ["문서에", "문서에서", "파일에", "전부", "모두", "선택한", "이", "to", "the", "from"];
                if !skip.contains(&compact(w).as_str()) && !w.ends_with("에서") && !w.ends_with("에") {
                    names = clean(w);
                }
            }
        }
    }
    if names.is_empty() {
        return None;
    }
    let dt = doc_type(t, ctx);
    let scope = scope_of(t, ctx, dt, None);
    if scope.is_empty() && !t.has(ALL_WORDS) {
        return Some(Plan::clarify(
            t.t(
                "어떤 문서에 태그를 적용할까요? 라이브러리에서 선택하거나 유형·날짜를 말씀해 주세요.",
                "Which documents should be tagged? Select them in the library or name a type / date range.",
            ),
            0.5,
        ));
    }
    let op = if remove {
        Op::Tag { add: Vec::new(), remove: names.clone() }
    } else {
        Op::Tag { add: names.clone(), remove: Vec::new() }
    };
    let summary = t
        .t(
            &format!("태그 {}을(를) {}", names.join(", "), if remove { "제거합니다" } else { "추가합니다" }),
            &format!("{} tag(s) {}", if remove { "Remove" } else { "Add" }, names.join(", ")),
        )
        .to_string();
    Some(mk(op, scope, 0.85, summary))
}

fn delete(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    if !t.has(&["삭제", "지워", "지우", "delete", "remove", "trash", "휴지통"]) || t.is_question() {
        return None;
    }
    if t.has(&[
        "스키마",
        "schema",
        "태그",
        "tag",
        "작업",
        "job",
        "컬렉션",
        "collection",
        "모델",
        "model",
        "용어",
        "glossary",
    ]) {
        return None;
    }
    let dt = doc_type(t, ctx);
    let d = date(t, ctx);
    let scope = scope_of(t, ctx, dt, d.as_ref());
    if scope.is_empty() {
        return Some(Plan::clarify(
            t.t(
                "어떤 문서를 삭제할까요? 라이브러리에서 선택하거나 유형·날짜를 말씀해 주세요.",
                "Which documents should be deleted? Select them in the library or name a type / date range.",
            ),
            0.5,
        ));
    }
    let label = type_label(dt, t.ko, t.t("선택 문서", "the selected documents"));
    Some(mk(
        Op::Delete { to_trash: Some(true) },
        scope,
        0.85,
        t.t(&format!("{label}을(를) 휴지통으로 보냅니다"), &format!("Move {label} to the recycle bin")).to_string(),
    ))
}

fn process(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    if t.is_question() {
        return None;
    }
    let classify = t.has(&["분류", "classif*", "유형판정", "유형을정"]);
    let ocr = t.has(&["ocr", "문자인식", "텍스트인식", "글자인식", "텍스트추출"]);
    let extract = t.has(&["추출", "extract*", "필드뽑", "값뽑", "kie"]) && !t.has(&["텍스트추출"]);
    let validate = t.has(&["검증", "validat*", "검사해"]);
    let summarize = t.has(&["요약", "summar*"]);
    let generic =
        t.has(&["처리", "process*", "돌려", "실행해"]) && !classify && !ocr && !extract && !summarize && !validate;
    if !(classify || ocr || extract || validate || summarize || generic) {
        return None;
    }
    let force = t.has(&["다시", "재처리", "재추출", "재분류", "재실행", "again", "redo", "force"]);
    let dt = doc_type(t, ctx);
    let d = date(t, ctx);
    let mut scope = scope_of(t, ctx, dt, d.as_ref());
    if scope.is_empty() && !t.has(ALL_WORDS) {
        if !selection_ids(ctx).is_empty() && (t.has(&["이", "선택", "this", "selected"]) || summarize || ocr) {
            scope.doc_ids = selection_ids(ctx);
        } else if ctx.current_folder.is_some() && t.has(&["여기", "here"]) {
            scope.folder = ctx.current_folder.clone();
        } else if !selection_ids(ctx).is_empty() {
            scope.doc_ids = selection_ids(ctx);
        } else {
            return Some(Plan::clarify(
                t.t(
                    "어떤 문서를 처리할까요? 라이브러리에서 선택하거나 폴더·유형·날짜를 말씀해 주세요.",
                    "Which documents? Select them in the library or name a folder / type / date range.",
                ),
                0.5,
            ));
        }
    }
    let mut ops = Vec::new();
    if summarize {
        let target = if t.has(&["하나로", "합쳐", "묶어", "전체요약", "together", "one summary", "combined"])
        {
            SummarizeTarget::Set
        } else {
            SummarizeTarget::EachDoc
        };
        let length = if t.has(&["짧게", "한줄", "간단", "short", "brief*"]) {
            Some(SummaryLength::Short)
        } else if t.has(&["길게", "자세", "상세", "long", "detailed"]) {
            Some(SummaryLength::Long)
        } else {
            None
        };
        ops.push(Op::Summarize { target: Some(target), length });
    }
    if classify || generic {
        ops.push(Op::Classify {
            doc_types: dt.map(|d| vec![d.slug.clone()]).unwrap_or_default(),
            force: force.then_some(true),
        });
    }
    if ocr || generic {
        let profile = if t.has(&["잔글씨", "작은글씨", "fine print", "영역", "region", "잘안읽"]) {
            Some(OcrProfile::FinePrint)
        } else {
            None
        };
        let pages = if t.has(&["이페이지", "현재페이지", "선택영역", "this page", "selection"]) {
            ctx.current_page.into_iter().collect()
        } else {
            Vec::new()
        };
        ops.push(Op::Ocr { pages, profile, force: (force || ocr).then_some(true) });
    }
    if extract || generic {
        let fields = dt
            .and_then(|d| ctx.schema_for_type(&d.slug))
            .map(|s| fields_mentioned(t, s).iter().map(|f| f.key.clone()).collect())
            .unwrap_or_default();
        ops.push(Op::Extract { schema_ref: dt.map(|d| d.slug.clone()), fields, force: force.then_some(true) });
    }
    if validate {
        ops.push(Op::Validate {});
    }
    if ops.is_empty() {
        return None;
    }
    let label = if !scope.doc_ids.is_empty() {
        t.t("선택 문서", "the selected documents")
    } else {
        type_label(dt, t.ko, t.t("문서", "documents"))
    };
    let what: Vec<&str> = ops
        .iter()
        .map(|o| match o {
            Op::Summarize { .. } => t.t("요약", "summarise"),
            Op::Classify { .. } => t.t("분류", "classify"),
            Op::Ocr { .. } => "OCR",
            Op::Extract { .. } => t.t("추출", "extract*"),
            Op::Validate {} => t.t("검증", "validat*"),
            _ => "",
        })
        .collect();
    let summary = if t.ko {
        format!("{label}을(를) {}합니다{}", what.join("·"), if force { "(다시 실행)" } else { "" })
    } else {
        format!("{} {label}{}", what.join(", "), if force { " (re-run)" } else { "" })
    };
    Some(mk_many(ops, scope, 0.87, summary))
}

fn review(t: &Text<'_>) -> Option<Plan> {
    let field_action = t.has(&["승인", "approve", "accept", "거부", "reject"])
        && t.has(&["필드", "field*", "저신뢰", "low confidence", "실패", "failed", "대기"]);
    if !t.has(&["검토", "review"]) && !field_action {
        return None;
    }
    let only = if t.has(&["저신뢰", "신뢰도낮", "low confidence"]) {
        Some(ReviewOnly::LowConfidence)
    } else if t.has(&["검증실패", "실패한", "failed"]) {
        Some(ReviewOnly::Failed)
    } else if t.has(ALL_WORDS) {
        Some(ReviewOnly::All)
    } else {
        None
    };
    if t.has(&["승인", "approve", "accept"]) {
        return Some(mk(
            Op::Review { action: ReviewOp::Approve, field: None, value: None, only },
            Scope::default(),
            0.85,
            t.t("검토 대기 필드를 일괄 승인합니다", "Approve the pending review fields").into(),
        ));
    }
    if t.has(&["거부", "reject"]) {
        return Some(mk(
            Op::Review { action: ReviewOp::Reject, field: None, value: None, only },
            Scope::default(),
            0.85,
            t.t("검토 대기 필드를 일괄 거부합니다", "Reject the pending review fields").into(),
        ));
    }
    if t.has(&["보여", "열어", "큐", "목록", "show", "open", "queue", "list"]) {
        return Some(mk(
            Op::Open { target: OpenTarget::Screen, reference: "review".into() },
            Scope::default(),
            0.9,
            t.t("검토 큐를 엽니다", "Open the review queue").into(),
        ));
    }
    None
}

fn search(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    if !t.has(&["찾아", "찾기", "검색", "find", "search", "look for", "where is", "locate"]) {
        return None;
    }
    // "거래처명이 X인 문서" / "find documents from X": strip the frame words.
    let mut q = t.raw.trim().to_string();
    for frame in [
        "거래처명이",
        "거래처가",
        "업체명이",
        "상호가",
        "제목이",
        "문서명이",
        "파일명이",
        "관련된",
        "관련",
        "포함된",
        "포함한",
        "포함하는",
        "들어있는",
        "들어간",
        "언급된",
        "문서들",
        "문서를",
        "문서",
        "파일을",
        "파일",
        "자료",
        "전부",
        "모두",
        "찾아줘",
        "찾아주세요",
        "찾아봐",
        "찾아",
        "찾기",
        "검색해줘",
        "검색해",
        "검색",
        "해줘",
        "주세요",
        "좀",
        "please",
        "documents",
        "document",
        "files",
        "file",
        "find",
        "search",
        "look for",
        "mentioning",
        "containing",
        "about",
        "from",
        "with",
        "for",
        "all",
        "the",
        "me",
    ] {
        q = q.replace(frame, " ");
    }
    let q = q
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| "\"'“”‘’".contains(c)))
        .map(|w| {
            if w.chars().count() > 2 {
                w.trim_end_matches(['인', '의', '을', '를', '이', '가', '은', '는'])
            } else {
                w
            }
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut q = q.trim().trim_end_matches(['?', '？', '.']).trim().to_string();
    // The type label moved into the scope filter ("'위약금' 들어간 계약서" → query 위약금, type contract).
    if let Some(d) = doc_type(t, ctx) {
        for label in [d.label_ko.as_str(), d.label_en.as_str()] {
            if label.chars().count() >= 2 {
                let stripped: String = q
                    .split_whitespace()
                    .filter(|w| compact(w) != compact(label) && !compact(w).ends_with(&compact(label)))
                    .collect::<Vec<_>>()
                    .join(" ");
                if !stripped.trim().is_empty() {
                    q = stripped;
                }
            }
        }
    }
    if q.is_empty() {
        return Some(mk(
            Op::Open { target: OpenTarget::Screen, reference: "search".into() },
            Scope::default(),
            0.8,
            t.t("검색 화면을 엽니다", "Open the search screen").into(),
        ));
    }
    let d = date(t, ctx);
    let dt = doc_type(t, ctx);
    let scope = scope_of(t, ctx, dt, d.as_ref());
    Some(mk(
        Op::Search { query: q.clone(), mode: None, top_k: None },
        scope,
        0.85,
        t.t(&format!("'{q}'을(를) 검색합니다"), &format!("Search for '{q}'")).to_string(),
    ))
}

fn open_document(t: &Text<'_>) -> Option<Plan> {
    if !t.has(&["열어", "열기", "open", "띄워"])
        || !t.has(&["문서", "파일", "document", "file", ".pdf", ".jpg", ".png", ".hwpx", ".docx"])
    {
        return None;
    }
    let name = t
        .raw
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| "\"'“”‘’".contains(c)))
        .find(|w| w.contains('.') && w.len() > 3)
        .map(str::to_string)?;
    Some(mk(
        Op::Open { target: OpenTarget::Document, reference: name.clone() },
        Scope::default(),
        0.85,
        t.t(&format!("{name} 문서를 엽니다"), &format!("Open {name}")).to_string(),
    ))
}

fn answer(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    let question = t.raw.trim().ends_with(['?', '？'])
        || t.has(&[
            "알려줘",
            "알려주",
            "알려",
            "뭐야",
            "무엇",
            "어떻게",
            "어디",
            "언제",
            "누구",
            "왜",
            "인가요",
            "인지",
            "나요",
            "까요",
            "가요",
            "설명해",
            "explain",
            "what",
            "how",
            "when",
            "where",
            "why",
            "which",
            "who",
        ]);
    if !question {
        return None;
    }
    let mut scope = Scope::default();
    if t.has(SELECTION_WORDS) {
        scope.doc_ids = selection_ids(ctx);
    }
    Some(mk(
        Op::Answer { question: t.raw.trim().to_string(), cite: Some(true), knowledge: None },
        scope,
        0.8,
        t.t("지식 베이스·문서에서 근거를 찾아 답합니다", "Answer with evidence from the knowledge base / documents")
            .into(),
    ))
}

const COMMAND_MARKERS: &[&str] = &[
    "해줘",
    "해주세요",
    "하세요",
    "해라",
    "하자",
    "시켜",
    "바꿔",
    "바꾸",
    "옮겨",
    "지워",
    "삭제",
    "정리",
    "추출",
    "분류",
    "내보내",
    "변환",
    "만들어",
    "실행",
    "열어",
    "보여",
    "찾아",
    "검색",
    "이동",
    "설정",
    "켜줘",
    "꺼줘",
    "처리",
    "돌려",
    "저장",
    "export*",
    "delete",
    "move",
    "rename*",
    "run",
    "open",
    "show",
    "make",
    "create",
    "set",
    "turn",
    "switch",
    "process*",
    "extract*",
    "classify",
    "organi*",
    "import*",
];

/// A short topic with no command verb ("xyzzy 플루토늄 원자로", "계약 해지 조건") is a
/// knowledge-base lookup: answer it rather than asking what to do.
fn topic(t: &Text<'_>) -> Option<Plan> {
    let words = t.raw.split_whitespace().count();
    if words == 0 || words > 8 || t.compact.chars().count() < 2 || t.has(COMMAND_MARKERS) {
        return None;
    }
    // Greetings / thanks are not knowledge-base lookups.
    if words <= 4
        && t.has(&[
            "고마워",
            "고맙",
            "감사",
            "수고",
            "안녕",
            "잘가",
            "thanks",
            "thank you",
            "hello",
            "hi",
            "bye",
            "good job",
        ])
    {
        return Some(mk(Op::Noop {}, Scope::default(), 0.9, t.t("별도 작업 없음", "Nothing to do").into()));
    }
    Some(mk(
        Op::Answer { question: t.raw.trim().to_string(), cite: Some(true), knowledge: None },
        Scope::default(),
        0.7,
        t.t("지식 베이스·문서에서 근거를 찾아 답합니다", "Answer with evidence from the knowledge base / documents")
            .into(),
    ))
}

// ---------------------------------------------------------------------------
// P6 rules: drafting, projects / consistency, explain / compare, retention, forms
// ---------------------------------------------------------------------------

const DRAFT_VERBS: &[&str] = &[
    "만들어",
    "작성해",
    "작성 해",
    "작성하",
    "써줘",
    "써 줘",
    "써주",
    "써 주",
    "초안",
    "생성해",
    "뽑아줘 문서",
    "draft",
    "write",
    "compose",
    "generate a",
    "create a",
    "prepare a",
    "make a",
];

/// Built-in template keywords (kept here so the rules do not depend on the template catalogue).
const TEMPLATE_WORDS: &[(crate::generate::TemplateKind, &str, &str, &[&str])] = &[
    (
        crate::generate::TemplateKind::SupplementRequest,
        "보완요청서",
        "supplement request",
        &["보완요청서", "보완 요청서", "supplement request", "supplement letter"],
    ),
    (
        crate::generate::TemplateKind::OfficialLetter,
        "공문",
        "official letter",
        &["공문", "official letter", "formal letter"],
    ),
    (crate::generate::TemplateKind::Quotation, "견적서", "quotation", &["견적서", "견적", "quotation", "quote"]),
    (
        crate::generate::TemplateKind::TransactionStatement,
        "거래명세서",
        "transaction statement",
        &["거래명세서", "거래명세표", "거래 명세서", "transaction statement", "statement of account"],
    ),
    (crate::generate::TemplateKind::Contract, "계약서", "contract", &["계약서", "contract", "agreement"]),
    (
        crate::generate::TemplateKind::Minutes,
        "회의록",
        "minutes",
        &["회의록", "meeting minutes", "minutes", "meeting notes"],
    ),
    (crate::generate::TemplateKind::Report, "보고서", "report", &["보고서", "리포트", "정산서", "report"]),
];

/// Template keyword → built-in template name (the registered forms are matched through the context).
fn draft_template(t: &Text<'_>, ctx: &PlanContext) -> Option<(String, String)> {
    // A document type with a registered schema mentioned in the sentence (data source or form).
    let registered = doc_type(t, ctx).filter(|d| ctx.schema_for_type(&d.slug).is_some());
    // Only the curated keyword table here: the template catalogue's broader keywords ("회의") would
    // turn everyday sentences ("내일 회의 자료 만들어줘") into drafts.
    let builtin = TEMPLATE_WORDS
        .iter()
        .find(|(_, _, _, words)| t.has(words))
        .map(|(kind, ko, en, _)| (kind.as_str().to_string(), t.t(ko, en).to_string()));
    match (registered, builtin) {
        // "영수증 집계 결과로 보고서 초안": the type is the data, the template word is the layout —
        // unless the sentence asks for that type's registered form ("휴가신청서 양식으로").
        (Some(d), Some(b)) => {
            let form_words = [
                format!("{}양식", d.label_ko),
                format!("{} 양식", d.label_ko),
                format!("{}서식", d.label_ko),
                format!("{} form", d.label_en),
                format!("{} template", d.label_en),
            ];
            let asks_form = form_words.iter().any(|w| t.has(&[w.as_str()]));
            if asks_form || b.0 == d.slug {
                Some((d.slug.clone(), type_label(Some(d), t.ko, &d.slug).to_string()))
            } else {
                Some(b)
            }
        }
        // A registered type alone ("영수증 합계 써줘") is data, not a layout — unless its form is asked for.
        (Some(d), None) => {
            let form_words = [
                format!("{}양식", d.label_ko),
                format!("{} 양식", d.label_ko),
                format!("{}서식", d.label_ko),
                format!("{} form", d.label_en),
                format!("{} template", d.label_en),
            ];
            form_words
                .iter()
                .any(|w| t.has(&[w.as_str()]))
                .then(|| (d.slug.clone(), type_label(Some(d), t.ko, &d.slug).to_string()))
        }
        (None, b) => b,
    }
}

fn draft_document(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    if t.is_question() || !t.has(DRAFT_VERBS) {
        return None;
    }
    // "작성일자", organising / exporting / searching sentences are not drafting requests.
    if t.has(&[
        "작성일",
        "정리해",
        "이동",
        "폴더로",
        "폴더",
        "folder",
        "is empty",
        "비어",
        "찾아",
        "검색",
        "엑셀",
        "export",
        "스키마",
        "schema",
        "프로젝트",
        "project",
    ]) {
        return None;
    }
    let template = draft_template(t, ctx);
    if template.is_none()
        && !t.has(&[
            "초안",
            "문서 만들",
            "문서를 만들",
            "문서 작성",
            "문서를 작성",
            "문서로 만들",
            "draft a",
            "draft the",
            "write a",
            "compose a",
            "generate a document",
            "create a document",
        ])
    {
        return None;
    }
    let mut sources: Vec<String> = Vec::new();
    if t.has(SELECTION_WORDS) || t.has(&["이 내용으로", "이것으로", "참고해서", "based on this", "from this"])
    {
        sources.extend(selection_ids(ctx));
        if sources.is_empty() {
            sources.push("selection".into());
        }
    }
    if t.has(&["집계결과", "집계 결과", "합계결과", "aggregate", "totals"]) {
        sources.push("aggregate".into());
    }
    let output = if t.has(&["pdf*", "피디에프"]) {
        Some(OutputFormat::Pdf)
    } else if t.has(&["docx*", "워드", "word*"]) {
        Some(OutputFormat::Docx)
    } else if t.has(&["마크다운", "markdown", "md"]) {
        Some(OutputFormat::Md)
    } else if t.has(&["hwpx*", "한글파일", "한글 파일", "한컴"]) {
        Some(OutputFormat::Hwpx)
    } else {
        None
    };
    let tone = if t.has(&["간결", "개조식", "짧게", "concise", "brief"]) {
        Some(Tone::Concise)
    } else if t.has(&["쉽게", "평이", "plain"]) {
        Some(Tone::Plain)
    } else {
        None
    };
    let length = if t.has(&["짧게", "한 쪽", "한쪽", "short", "one page"]) {
        Some(Length::Short)
    } else if t.has(&["자세히", "상세", "길게", "detailed", "long"]) {
        Some(Length::Long)
    } else {
        None
    };
    let (form_ref, label) = match template {
        Some((slug, label)) => (Some(slug), label),
        None => (None, t.t("문서", "document").to_string()),
    };
    // "지난달 영수증 집계 결과로 경비 보고서 초안" → aggregate the type first, then draft from the card.
    let aggregate_first = sources.iter().any(|s| s == "aggregate")
        && doc_type(t, ctx).is_some_and(|d| !TEMPLATE_WORDS.iter().any(|(k, ..)| k.as_str() == d.slug));
    let op = Op::DraftDocument {
        form_ref,
        format_description: None,
        content: t.raw.trim().to_string(),
        sources,
        output,
        tone,
        length,
    };
    let summary = t
        .t(
            &format!("{label} 초안을 만들어 문서 생성 화면에서 보여 줍니다"),
            &format!("Draft a {label} and open it in the document generator"),
        )
        .to_string();
    if aggregate_first {
        let dt = doc_type(t, ctx);
        let scope = scope_of(t, ctx, dt, date(t, ctx).as_ref());
        let agg = Op::Aggregate { func: AggregateFn::Sum, field: None, group_by: None, include_unreviewed: None };
        let summary = t
            .t(
                &format!("{}을(를) 집계한 뒤 그 결과로 {label} 초안을 만듭니다", type_label(dt, true, "문서")),
                &format!(
                    "Aggregate the {} first, then draft a {label} from the totals",
                    type_label(dt, false, "documents")
                ),
            )
            .to_string();
        return Some(mk_many(vec![agg, op], scope, 0.8, summary));
    }
    Some(mk(op, Scope::default(), 0.82, summary))
}

/// Quoted text ("…", '…', “…”) or `None`.
fn quoted(raw: &str) -> Option<String> {
    let chars: Vec<char> = raw.chars().collect();
    let opens = ['"', '\'', '“', '‘', '「', '『'];
    let closes = ['"', '\'', '”', '’', '」', '』'];
    let start = chars.iter().position(|c| opens.contains(c))?;
    let end = chars.iter().skip(start + 1).position(|c| closes.contains(c))? + start + 1;
    let inner: String = chars[start + 1..end].iter().collect();
    let inner = inner.trim();
    (!inner.is_empty()).then(|| inner.to_string())
}

/// Project name from "<name> 프로젝트", "프로젝트 <name>", "project <name>" or quotes.
fn project_name(t: &Text<'_>) -> Option<String> {
    if let Some(q) = quoted(t.raw) {
        return Some(q);
    }
    let stop_words = [
        "프로젝트",
        "케이스",
        "project",
        "case",
        "만들어",
        "만들",
        "생성",
        "새",
        "추가",
        "넣어",
        "create",
        "new",
        "add",
        "to",
        "the",
        "a",
        "an",
        "에",
        "을",
        "를",
        "해줘",
        "해",
        "줘",
        "please",
        "로",
        "으로",
        "이름",
        "named",
        "called",
        "정합성",
        "검사",
        "check",
        "consistency",
        "run",
        "실행",
        "해주세요",
        "하고",
        "선택한",
        "선택",
        "문서",
        "파일",
        "documents",
        "document",
        "selected",
        "these",
        "this",
        "files",
        "file",
    ];
    let words: Vec<&str> = t.raw.split_whitespace().collect();
    let idx = words.iter().position(|w| {
        let c = compact(w);
        c.starts_with("프로젝트") || c.starts_with("project") || c.starts_with("케이스")
    })?;
    // Korean: words before "프로젝트"; English: words after "project".
    let before: Vec<&str> = words[..idx].to_vec();
    let after: Vec<&str> = words[idx + 1..].to_vec();
    let clean = |ws: &[&str]| -> Vec<String> {
        ws.iter()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
            .map(|w| {
                // Strip a trailing particle glued to the name ("한빛을" → "한빛").
                let mut w = w;
                for p in ["이라는", "라는", "으로", "에서", "에는", "을", "를", "은", "는", "로", "에", "의"]
                {
                    if w.chars().count() > p.chars().count() + 1 {
                        if let Some(stripped) = w.strip_suffix(p) {
                            w = stripped.to_string();
                            break;
                        }
                    }
                }
                w
            })
            .filter(|w| !w.is_empty() && !stop_words.contains(&compact(w).as_str()))
            .collect()
    };
    let mut name = clean(&before);
    if name.is_empty() {
        // "create project Acme renewal": the words after `project` — but "a project plan for Q1" is not a name.
        let generic =
            ["for", "about", "with", "on", "of", "plan", "meeting", "notes", "kickoff", "manager", "review", "in"];
        if after.len() <= 4 && !after.iter().any(|w| generic.contains(&compact(w).as_str())) {
            name = clean(&after);
        }
    }
    let name = name.join(" ");
    (!name.is_empty() && name.chars().count() <= 60).then_some(name)
}

/// The known project whose name is mentioned in the sentence (longest match), as its id.
fn project_ref(t: &Text<'_>, ctx: &PlanContext) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for p in &ctx.projects {
        let key = compact(&p.name);
        if key.chars().count() >= 2 && t.compact.contains(&key) && best.is_none_or(|(l, _)| key.len() > l) {
            best = Some((key.len(), p.id.as_str()));
        }
    }
    best.map(|(_, id)| id.to_string())
}

fn project_ops(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    let project_word = t.has(&["프로젝트", "project"]);
    // -- 보완요청서 ---------------------------------------------------------
    if t.has(&["보완요청서", "보완 요청서", "보완요청", "보완 요청", "supplement request", "supplement letter"])
        && !t.is_question()
        && t.has(DRAFT_VERBS)
    {
        let output = if t.has(&["pdf*"]) {
            Some(OutputFormat::Pdf)
        } else if t.has(&["docx*", "워드", "word*"]) {
            Some(OutputFormat::Docx)
        } else {
            None
        };
        let project = project_ref(t, ctx);
        return Some(mk(
            Op::RequestSupplement { project, findings: Vec::new(), output },
            Scope::default(),
            0.85,
            t.t(
                "불일치·누락 항목으로 보완요청서 초안을 만듭니다",
                "Draft a supplement request from the mismatches / missing documents",
            )
            .to_string(),
        ));
    }
    // -- 정합성 검사 ---------------------------------------------------------
    if t.has(&[
        "정합성",
        "일치하는지",
        "일치 여부",
        "일치여부",
        "교차검증",
        "교차 검증",
        "consistency",
        "cross-check",
        "cross check",
        "reconcile",
    ]) && t.has(&["검사", "검증", "확인", "점검", "돌려", "실행", "check", "run", "verify", "validate"])
        && !t.is_question()
    {
        let project = project_ref(t, ctx);
        return Some(mk(
            Op::CheckConsistency { project, rules: Vec::new() },
            Scope::default(),
            0.85,
            t.t("프로젝트 문서 간 정합성 규칙을 실행합니다", "Run the project's consistency rules").to_string(),
        ));
    }
    // -- 규칙 정의 -------------------------------------------------------------
    let rule_phrase = t.has(&[
        "같아야",
        "일치해야",
        "동일해야",
        "달라야",
        "이내여야",
        "이상이어야",
        "이하여야",
        "넘지 않아야",
        "필수",
        "must equal",
        "must match",
        "must be equal",
        "must be present",
        "must not",
        "within",
        "should equal",
        "has to match",
    ]);
    if (t.has(&["규칙", "rule"]) && t.has(&["추가", "정의", "만들", "등록", "add", "define", "create", "new"]))
        || (rule_phrase
            && (project_word || t.has(&["규칙", "rule"]) || !ctx.doc_types.is_empty() && doc_type(t, ctx).is_some()))
    {
        if t.is_question() {
            return None; // "필수 항목이 뭐야?", "… 이내여야 해?" ask about rules, they do not define one
        }
        // The description is the sentence minus the "add a rule" frame.
        let mut description = t.raw.trim().to_string();
        for frame in [
            "규칙으로 추가해줘",
            "규칙 추가해줘",
            "규칙으로 추가",
            "규칙 추가",
            "규칙을 추가",
            "규칙으로 등록",
            "규칙 정의",
            "as a rule",
            "add a rule",
            "add rule",
            "define a rule",
            "define rule",
        ] {
            if let Some(i) = description.to_lowercase().find(frame) {
                let (a, b) = description.split_at(i);
                let b = &b[frame.len().min(b.len())..];
                description = format!("{} {}", a.trim(), b.trim()).trim().trim_matches(':').trim().to_string();
            }
        }
        let project = project_ref(t, ctx);
        return Some(mk(
            Op::DefineRule { project, template: None, description },
            Scope::default(),
            0.8,
            t.t("정합성 규칙을 만들어 규칙 그리드에서 확인합니다", "Define a consistency rule (confirm in the grid)")
                .to_string(),
        ));
    }
    if !project_word {
        return None;
    }
    // -- 프로젝트에 추가 ---------------------------------------------------------
    if t.has(&[
        "프로젝트에",
        "프로젝트로",
        "into the project",
        "to the project",
        "to project",
        "add to",
        "add this",
        "add these",
    ]) && t.has(&["추가", "넣어", "보내", "포함", "add", "put", "move"])
    {
        let project = project_ref(t, ctx);
        let dt = doc_type(t, ctx);
        let mut scope = scope_of(t, ctx, dt, date(t, ctx).as_ref());
        if scope.is_empty() {
            scope.doc_ids = selection_ids(ctx);
        }
        if scope.is_empty() {
            return Some(Plan::clarify(
                t.t(
                    "어떤 문서를 프로젝트에 추가할까요? 라이브러리에서 선택하거나 유형·날짜를 말씀해 주세요.",
                    "Which documents should join the project? Select them or name a type / date range.",
                ),
                0.5,
            ));
        }
        return Some(mk(
            Op::AddToProject { project, doc_ids: Vec::new() },
            scope,
            0.82,
            t.t("선택한 문서를 프로젝트에 추가합니다", "Add the documents to the project").to_string(),
        ));
    }
    // -- 프로젝트 만들기 ----------------------------------------------------------
    // Only explicit creation commands; "새 프로젝트 매니저가 왔어" is a statement.
    if t.has(&[
        "만들어",
        "만들자",
        "만들고",
        "만들어서",
        "생성해",
        "생성하",
        "create",
        "new project",
        "set up a project",
    ]) && !t.is_question()
    {
        let Some(name) = project_name(t) else {
            return Some(Plan::clarify(
                t.t("새 프로젝트의 이름을 알려주세요.", "What should the new project be called?"),
                0.5,
            ));
        };
        let template = [
            "인증 심사",
            "인증심사",
            "거래처 등록",
            "거래처등록",
            "구매 정산",
            "구매정산",
            "audit",
            "vendor",
            "purchase",
        ]
        .iter()
        .find(|k| t.has(&[k]))
        .map(|k| k.to_string());
        let doc_types_required: Vec<String> = ctx
            .doc_types
            .iter()
            .filter(|d| t.has(&[d.label_ko.as_str()]) || t.has(&[d.label_en.as_str()]))
            .map(|d| d.slug.clone())
            .collect();
        let summary =
            t.t(&format!("'{name}' 프로젝트를 만듭니다"), &format!("Create the project '{name}'")).to_string();
        return Some(mk(Op::CreateProject { name, template, doc_types_required }, Scope::default(), 0.82, summary));
    }
    None
}

fn explain_compare(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    if t.has(&["비교해", "비교 해", "비교하", "비교", "compare", "차이점", "differences between"])
        && !t.has(&["요약", "summar"])
    {
        let ids = selection_ids(ctx);
        if ids.len() >= 2 {
            let aspects: Vec<String> =
                ["금액", "기간", "조항", "날짜", "당사자", "amount", "period", "clause", "date", "party"]
                    .iter()
                    .filter(|a| t.has(&[a]))
                    .map(|a| a.to_string())
                    .collect();
            return Some(mk(
                Op::Compare { doc_ids: ids.clone(), aspects },
                Scope { doc_ids: ids, ..Scope::default() },
                0.85,
                t.t("선택한 문서를 비교 요약합니다", "Compare the selected documents").to_string(),
            ));
        }
        if t.has(SELECTION_WORDS) || t.has(&["이 두", "두 문서", "these", "both"]) {
            return Some(Plan::clarify(
                t.t(
                    "비교할 문서를 라이브러리에서 2건 이상 선택해 주세요.",
                    "Select two or more documents in the library to compare.",
                ),
                0.5,
            ));
        }
    }
    // Explanations of a selection / term; "이어서 설명해줘"-style follow-ups stay ordinary questions.
    if t.has(&["설명해", "설명 해", "설명좀", "풀이해", "뜻이 뭐", "무슨 뜻", "explain", "meaning of"])
        && (t.has(SELECTION_WORDS)
            || t.has(&[
                "이 문장",
                "이문장",
                "이 용어",
                "이용어",
                "이 조항",
                "이조항",
                "이 부분",
                "이 표",
                "이 단어",
                "뜻",
                "의미",
                "용어",
                "this clause",
                "this sentence",
                "this term",
                "this paragraph",
                "meaning",
            ]))
        && !t.has(&["say about", "says about", "말하", "내용"])
    {
        let scope =
            if ctx.current_document.is_some() && t.has(SELECTION_WORDS) { Some(ExplainScope::Document) } else { None };
        return Some(mk(
            Op::Explain {
                selection: None,
                question: Some(t.raw.trim().to_string()),
                scope,
                allow_general_knowledge: None,
            },
            Scope { doc_ids: if scope.is_some() { selection_ids(ctx) } else { Vec::new() }, ..Scope::default() },
            0.8,
            t.t("지식 베이스 근거로 설명합니다", "Explain with knowledge-base evidence").to_string(),
        ));
    }
    None
}

fn set_retention(t: &Text<'_>) -> Option<Plan> {
    if t.is_question()
        || !t.has(&[
            "보관",
            "보존",
            "파기",
            "삭제돼",
            "지워지",
            "retention",
            "keep",
            "purge",
            "delete after",
            "retain*",
        ])
    {
        return None;
    }
    // "5년 보존이 법적 의무야", "6개월 보관이 기본이지" describe rules; only commands change the policy.
    if t.has(&[
        "의무",
        "법적",
        "법에",
        "법으로",
        "규정",
        "기본이",
        "해야 한다",
        "해야한다",
        "해야 하",
        "안 돼",
        "안돼",
        "안 되",
        "안되",
        "요구",
        "by law",
        "is the default",
        "required to",
        "should be kept",
        "supposed to",
    ]) {
        return None;
    }
    if !t.has(&[
        "보관해",
        "보존해",
        "로 설정",
        "으로 설정",
        "바꿔",
        "변경",
        "해줘",
        "해 줘",
        "해주",
        "만 보관",
        "만 보존",
        "무기한",
        "영구",
        "즉시",
        "keep",
        "retain*",
        "set",
        "purge",
        "delete after",
        "지워",
        "파기해",
    ]) {
        return None;
    }
    let item = if t.has(&["저널", "journal", "되돌리기 기록", "undo history"]) {
        RetentionTarget::Journal
    } else if t.has(&["로그", "log"]) {
        RetentionTarget::Logs
    } else if t.has(&["대화", "채팅", "답변 기록", "chat", "conversation"]) {
        RetentionTarget::Chats
    } else if t.has(&["추출", "extraction", "필드"]) {
        RetentionTarget::Extractions
    } else if t.has(&["임시", "temp"]) {
        RetentionTarget::Temp
    } else if t.has(&["문서", "원본", "document"]) {
        RetentionTarget::Documents
    } else {
        return None;
    };
    let days: i64 = if t.has(&["무기한", "영구", "계속", "forever", "indefinitely", "never"]) {
        -1
    } else if t.has(&["즉시", "바로", "immediately", "right away"]) {
        0
    } else {
        // "90일", "3개월", "1년", "30 days", "6 months", "2 years"
        let mut num: Option<i64> = None;
        let mut unit_mult = 1i64;
        let norm = &t.norm;
        let tokens: Vec<&str> = norm.split_whitespace().collect();
        for (i, tok) in tokens.iter().enumerate() {
            let digits: String = tok.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                continue;
            }
            let rest = &tok[digits.len()..];
            let next = tokens.get(i + 1).copied().unwrap_or("");
            let unit = if rest.is_empty() { next } else { rest };
            if unit.starts_with("일") || unit.starts_with("day") {
                unit_mult = 1;
            } else if unit.starts_with("주") || unit.starts_with("week") {
                unit_mult = 7;
            } else if unit.starts_with("개월") || unit.starts_with("달") || unit.starts_with("month") {
                unit_mult = 30;
            } else if unit.starts_with("년") || unit.starts_with("year") {
                unit_mult = 365;
            } else {
                continue;
            }
            num = digits.parse().ok();
            break;
        }
        num? * unit_mult
    };
    let label = match item {
        RetentionTarget::Documents => t.t("원본 문서", "documents"),
        RetentionTarget::Extractions => t.t("추출 결과", "extractions"),
        RetentionTarget::Logs => t.t("로그", "logs"),
        RetentionTarget::Journal => t.t("파일 조작 저널", "journal"),
        RetentionTarget::Chats => t.t("대화 기록", "conversations"),
        RetentionTarget::Temp => t.t("임시 데이터", "temporary data"),
    };
    let period = match days {
        -1 => t.t("무기한", "forever").to_string(),
        0 => t.t("즉시 파기", "immediately").to_string(),
        d => {
            if t.ko {
                format!("{d}일")
            } else {
                format!("{d} days")
            }
        }
    };
    Some(mk(
        Op::SetRetention { item, days },
        Scope::default(),
        0.85,
        t.t(&format!("{label} 보존 기간을 {period}로 설정합니다"), &format!("Keep {label} for {period}")).to_string(),
    ))
}

fn register_form(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    if t.is_question() || t.has(&["화면", "어디", "메뉴", "screen", "where", "menu"]) {
        return None;
    }
    if !t.has(&[
        "양식으로등록",
        "양식등록",
        "양식으로 등록",
        "양식 등록",
        "서식으로 등록",
        "register as a form",
        "register form",
        "register this form",
        "register the form",
    ]) {
        return None;
    }
    let doc_ref = if ctx.current_document.is_some() || !ctx.selected_doc_ids.is_empty() {
        Some(selection_ids(ctx).first().cloned().unwrap_or_else(|| "current".into()))
    } else {
        None
    };
    if doc_ref.is_none() {
        return Some(Plan::clarify(
            t.t(
                "양식으로 등록할 문서를 먼저 열거나 선택해 주세요.",
                "Open or select the document to register as a form first.",
            ),
            0.5,
        ));
    }
    let name = quoted(t.raw);
    Some(mk(
        Op::RegisterForm { doc_ref, name, doc_type_slug: None },
        Scope::default(),
        0.85,
        t.t("열린 문서를 양식으로 등록합니다(필드 그리드 확인)", "Register the open document as a form (field grid)")
            .to_string(),
    ))
}

fn profile_name(p: Profile) -> &'static str {
    match p {
        Profile::Accuracy => "accuracy",
        Profile::Balanced => "balanced",
        Profile::Speed => "speed",
        Profile::FinePrint => "fine_print",
    }
}

/// Small settings / control sentences the pre-router does not cover: inbox watching,
/// re-indexing, the prompt profile, DPI / review-threshold settings and manual document types.
fn quick_settings(t: &Text<'_>, ctx: &PlanContext) -> Option<Plan> {
    if t.is_question() {
        return None;
    }
    // inbox watching on / off
    if t.has(&["inbox", "인박스", "받은함"]) && t.has(&["감시", "watch*", "모니터"]) {
        let off = t.has(&["꺼", "끄", "중지", "해제", "off", "stop", "disable"]);
        let on = t.has(&["켜", "시작", "on", "enable", "start"]);
        if off || on {
            return Some(mk(
                Op::SetInboxWatch { enabled: !off },
                Scope::default(),
                0.9,
                t.t(
                    if off { "inbox 감시를 끕니다" } else { "inbox 감시를 켭니다" },
                    if off { "Turn inbox watching off" } else { "Turn inbox watching on" },
                )
                .into(),
            ));
        }
    }
    // re-index (not "인덱스 펀드 다시 알아봐줘": the search index must be named and rebuilt)
    if t.has(&["색인", "reindex", "re-index", "search index", "검색 인덱스", "검색인덱스"])
        && t.has(&[
            "다시 만들",
            "다시만들",
            "재구축",
            "재생성",
            "다시 색인",
            "재색인",
            "rebuild",
            "refresh",
            "reindex",
            "re-index",
        ])
    {
        let scope = if t.has(&["벡터", "임베딩", "vector", "embedding"]) {
            Some(super::ReindexScope::Vec)
        } else if t.has(&["fts", "전문", "키워드", "keyword"]) {
            Some(super::ReindexScope::Fts)
        } else {
            Some(super::ReindexScope::All)
        };
        return Some(mk(
            Op::Reindex { scope },
            Scope::default(),
            0.88,
            t.t("검색 색인을 다시 만듭니다", "Rebuild the search index").into(),
        ));
    }
    // prompt profile (not when a processing request merely names a profile: "re-OCR … fine print")
    if (t.has(&["프로파일", "프로필", "profile", "모드로"])
        || t.has(&["정확도 우선", "속도 우선", "균형 모드", "잔글씨 모드"]))
        && !t.has(&["ocr", "re-ocr", "추출", "분류", "처리", "다시", "again", "extract*", "classif*", "process*"])
    {
        let profile = if t.has(&["정확", "accura*"]) {
            Some(Profile::Accuracy)
        } else if t.has(&["속도", "빠른", "speed", "fast"]) {
            Some(Profile::Speed)
        } else if t.has(&["잔글씨", "작은 글씨", "fine print", "fine_print"]) {
            Some(Profile::FinePrint)
        } else if t.has(&["균형", "balanc*"]) {
            Some(Profile::Balanced)
        } else {
            None
        };
        if let Some(profile) = profile {
            return Some(mk(
                Op::SetProfile { profile },
                Scope::default(),
                0.88,
                t.t(
                    &format!("프로파일을 {}(으)로", profile_name(profile)),
                    &format!("Profile → {}", profile_name(profile)),
                )
                .to_string(),
            ));
        }
    }
    // numeric settings: render DPI, review threshold — commands only ("300으로 스캔했어" is history)
    let number = t.norm.split_whitespace().filter_map(|w| w.trim_end_matches('%').parse::<f64>().ok()).next();
    let setting_verb = t.has(&[
        "로 설정",
        "으로 설정",
        "로 바꿔",
        "으로 바꿔",
        "바꿔",
        "변경",
        "해줘",
        "해 줘",
        "로 해",
        "올려",
        "내려",
        "set",
        "change",
        "use",
    ]) && !t.has(&["했어", "했다", "했습니다", "scanned", "did"]);
    if let (Some(n), true) = (number, setting_verb) {
        if t.has(&["dpi", "해상도", "resolution"]) && [150.0, 200.0, 300.0].contains(&n) {
            return Some(mk(
                Op::SetSetting { key: SettingKey::RenderDpi, value: serde_json::json!(n as u32) },
                Scope::default(),
                0.88,
                t.t(&format!("렌더 DPI를 {n}로"), &format!("Render DPI → {n}")).to_string(),
            ));
        }
        if t.has(&["임계", "threshold", "기준값", "신뢰도 기준", "confidence threshold"]) {
            let v = if n > 1.0 { n / 100.0 } else { n };
            return Some(mk(
                Op::SetSetting { key: SettingKey::ReviewThreshold, value: serde_json::json!(v) },
                Scope::default(),
                0.85,
                t.t(&format!("검토 임계값을 {v}로"), &format!("Review threshold → {v}")).to_string(),
            ));
        }
    }
    // manual document type: "유형을 견적서로 바꿔", "mark this document as a business registration"
    if (t.has(&["유형", "종류", "type"]) && t.has(&["바꿔", "변경", "설정", "지정", "set", "change", "to"]))
        || t.has(&["mark this", "mark these", "mark the", "로 표시", "로 지정"])
    {
        if let Some(dt) = doc_type(t, ctx) {
            let mut scope = scope_of(t, ctx, None, None);
            if scope.is_empty() && !t.has(ALL_WORDS) {
                // "유형을 견적서로 바꿔" applies to the open document; "한빛과 거래한 서류들 유형을 …" carries a
                // qualifier the rules cannot express — leave that to the model.
                let frame = [
                    "유형",
                    "종류",
                    "type",
                    "바꿔",
                    "변경",
                    "설정",
                    "지정",
                    "set",
                    "change",
                    "to",
                    "mark",
                    "this",
                    "these",
                    "the",
                    "document",
                    "documents",
                    "as",
                    "a",
                    "an",
                    "해줘",
                    "해",
                    "줘",
                    "문서",
                    "문서의",
                    "로",
                    "으로",
                ];
                let leftover = t
                    .raw
                    .split_whitespace()
                    .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
                    .map(|w| w.trim_end_matches(['을', '를', '은', '는', '이', '가', '로', '의']))
                    .filter(|w| !w.is_empty())
                    .filter(|w| !frame.contains(&compact(w).as_str()))
                    .filter(|w| {
                        compact(w) != compact(&dt.label_ko)
                            && compact(w) != compact(&dt.label_en)
                            && compact(w) != compact(&dt.slug)
                    })
                    .filter(|w| !compact(&dt.label_ko).is_empty() && !compact(w).starts_with(&compact(&dt.label_ko)))
                    .count();
                if leftover > 0 || selection_ids(ctx).is_empty() {
                    return None;
                }
                scope.doc_ids = selection_ids(ctx);
            }
            return Some(mk(
                Op::SetDocType { doc_type: dt.slug.clone() },
                scope,
                0.87,
                t.t(
                    &format!("문서 유형을 {}(으)로 설정합니다", dt.label_ko),
                    &format!("Set the document type to {}", dt.label_en),
                )
                .to_string(),
            ));
        }
    }
    None
}

/// Tries the rules in order; `None` leaves the command to the model.
pub fn plan(text: &str, ctx: &PlanContext) -> Option<Plan> {
    let raw = text.trim();
    if raw.is_empty() {
        return None;
    }
    let t = Text::new(raw, ctx.language == Language::Ko);
    undo_redo(&t)
        .or_else(|| job_control(&t))
        .or_else(|| engine(&t))
        .or_else(|| set_retention(&t))
        .or_else(|| settings_combo(&t, ctx))
        .or_else(|| quick_settings(&t, ctx))
        .or_else(|| register_form(&t, ctx))
        .or_else(|| project_ops(&t, ctx))
        .or_else(|| define_schema(&t, ctx))
        .or_else(|| draft_document(&t, ctx))
        .or_else(|| export(&t, ctx))
        .or_else(|| aggregate(&t, ctx))
        .or_else(|| inbox(&t, ctx))
        .or_else(|| tag(&t, ctx))
        .or_else(|| delete(&t, ctx))
        .or_else(|| organize(&t, ctx))
        .or_else(|| process(&t, ctx))
        .or_else(|| review(&t))
        .or_else(|| explain_compare(&t, ctx))
        .or_else(|| search(&t, ctx))
        .or_else(|| open_document(&t))
        .or_else(|| answer(&t, ctx))
        .or_else(|| topic(&t))
}
