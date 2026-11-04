//! Deterministic phrase rules for the frequent single-intent commands (§3.5
//! "Pre-router … 단순 → 즉시 실행"): totals, exports, inbox processing,
//! organising, processing a selection, search, undo and job control. They are
//! conservative — anything they do not recognise goes to the model.

use crate::prerouter::{self, DateRange, PreRoutedOp};
use crate::settings::Language;

use super::{
    compact, AggregateFn, AutoPipeline, DocTypeSummary, EngineOp, ExportFormat, ExportTarget, FieldSummary, GroupBy,
    ImportFrom, JobOp, OcrProfile, Op, OpKind, OpenTarget, Plan, PlanContext, ReviewOnly, ReviewOp, SchemaMode,
    SchemaSummary, Scope, SummarizeTarget, SummaryLength,
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
    ko: bool,
}

impl Text<'_> {
    fn has(&self, words: &[&str]) -> bool {
        words.iter().any(|w| self.compact.contains(&compact(w)))
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
    "thisdocument",
    "thisfile",
    "thispage",
    "selected",
    "theselection",
    "thesedocuments",
    "currentdocument",
    "thiscontract",
    "thisreceipt",
];
const ALL_WORDS: &[&str] =
    &["전부", "전체", "모두", "모든", "다 ", "라이브러리전체", "alldocuments", "everything", "all "];

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
fn scope_of(t: &Text<'_>, ctx: &PlanContext, dt: Option<&DocTypeSummary>, d: Option<&DateRange>) -> Scope {
    let mut scope = Scope::default();
    if t.has(SELECTION_WORDS) {
        scope.doc_ids = selection_ids(ctx);
    }
    if t.has(&["inbox", "인박스", "받은함"]) {
        scope.folder = Some("inbox".into());
    } else if t.has(&["이폴더", "여기있는", "현재폴더", "thisfolder", "currentfolder", "here"]) {
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
    if t.has(&["다시실행", "redo"]) && !t.has(&["추출", "ocr", "분류", "처리", "extract", "classify", "process"])
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
    let action = if t.has(&["일시정지", "일시중지", "멈춰", "잠깐멈", "pause", "hold"]) {
        JobOp::Pause
    } else if t.has(&["재개", "계속해", "이어서", "resume", "continue"]) {
        JobOp::Resume
    } else if t.has(&["재시도", "다시시도", "retry"]) {
        JobOp::Retry
    } else if t.has(&["우선", "먼저해", "prioriti", "firstplease", "bumpup"]) {
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
    if !job_word && !matches!(action, JobOp::Pause | JobOp::Resume) {
        return None;
    }
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
        let ct = Text { raw: clause, lower: clause.to_lowercase(), compact: compact(clause), ko: t.ko };
        let _ = &ct.lower;
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
        "추가", "만들", "생성", "수정", "바꿔", "변경", "정의", "add", "create", "modify", "change", "define", "new",
    ]) {
        return None;
    }
    let dt = doc_type(t, ctx);
    let schema_ref =
        dt.and_then(|d| ctx.schema_for_type(&d.slug)).map(|s| s.name.clone()).or_else(|| dt.map(|d| d.slug.clone()));
    let modify = schema_ref.is_some() && t.has(&["추가", "수정", "바꿔", "변경", "add", "modify", "change"]);
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
    let verb = t.has(&["내보내", "저장", "뽑아", "추출해서", "만들어", "출력", "export", "save", "dump", "write"])
        || t.has(&["로", "으로", "→", "->", "to", "into", "as"]);
    if !verb {
        return None;
    }
    let dt = doc_type(t, ctx);
    let d = date(t, ctx);
    let mut scope = scope_of(t, ctx, dt, d.as_ref());
    if scope.folder.is_none() && scope.doc_ids.is_empty() && dt.is_none() && d.is_none() && !t.has(ALL_WORDS) {
        scope.folder = ctx.current_folder.clone();
    }
    let target = if t.has(&["표를", "표만", "테이블", "table"]) {
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
        "howmuch",
        "howmany",
        "highest",
        "lowest",
        "largest",
        "smallest",
    ];
    if !t.has(TRIGGERS) {
        return None;
    }
    let func = if t.has(&["평균", "average", "avg", "mean"]) {
        AggregateFn::Avg
    } else if t.has(&["최대", "가장큰", "가장비싼", "max", "highest", "largest"]) {
        AggregateFn::Max
    } else if t.has(&["최소", "가장작은", "가장싼", "min", "lowest", "smallest"]) {
        AggregateFn::Min
    } else if t.has(&["건수", "몇건", "몇개", "개수", "count", "howmany", "number of"]) {
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
    let group_by = if t.has(&["월별", "달별", "monthly", "bymonth", "permonth"]) {
        Some(GroupBy::Month)
    } else if t.has(&["거래처별", "업체별", "byvendor", "pervendor"]) {
        Some(GroupBy::Vendor)
    } else if t.has(&["유형별", "종류별", "bytype", "pertype"]) {
        Some(GroupBy::DocType)
    } else {
        None
    };
    let include_unreviewed = t
        .has(&["검토대기포함", "검토미완료포함", "미검토포함", "대기포함", "includeunreviewed", "includingunreviewed"])
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
    if !t.has(&["inbox", "인박스", "받은함", "받은문서"]) {
        return None;
    }
    if !t.has(&[
        "처리", "가져", "불러", "import", "process", "정리", "분류", "추출", "ocr", "읽어", "등록", "run", "handle",
        "전부", "모두",
    ]) {
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
    let only_classify = t.has(&["분류만", "classifyonly"]);
    let only_ocr = t.has(&["ocr만", "ocronly"]);
    let wants_processing =
        t.has(&["처리", "process", "추출", "extract", "전부", "모두", "ocr", "분류", "classif", "read", "읽어"]);
    if wants_processing || dt.is_some() {
        ops.push(Op::Classify { doc_types: Vec::new(), force: None });
        if !only_classify {
            ops.push(Op::Ocr { pages: Vec::new(), profile: None, force: None });
            if !only_ocr {
                ops.push(Op::Extract { schema_ref: dt.map(|d| d.slug.clone()), fields: Vec::new(), force: None });
            }
        }
    }
    let group = if t.has(&["월별", "달별", "monthly", "bymonth"]) {
        Some(GroupBy::Month)
    } else if t.has(&["거래처별", "업체별", "byvendor"]) {
        Some(GroupBy::Vendor)
    } else if t.has(&["유형별", "종류별", "bytype", "정리", "폴더로", "옮겨", "이동", "organi", "move", "sort into"])
    {
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
    let rename = t.has(&["이름바꿔", "이름을바꿔", "이름변경", "파일명", "rename", "이름을"]);
    let organize_word = t.has(&["정리", "organi", "옮겨", "옮기", "이동", "move", "폴더로", "sortinto", "tidy"]);
    if !rename && !organize_word {
        return None;
    }
    if t.has(&["작업", "job", "스키마", "schema", "태그", "tag"])
        && !t.has(&["문서", "파일", "document", "file", "영수증", "receipt"])
    {
        return None;
    }
    let dt = doc_type(t, ctx);
    let d = date(t, ctx);
    let scope = scope_of(t, ctx, dt, d.as_ref());
    let only_reviewed =
        if t.has(&["검토완료", "검토된", "reviewedonly", "onlyreviewed"]) { Some(true) } else { None };
    let group = if t.has(&["월별", "달별", "monthly", "bymonth"]) {
        Some(GroupBy::Month)
    } else if t.has(&["거래처별", "업체별", "byvendor"]) {
        Some(GroupBy::Vendor)
    } else if t.has(&["유형별", "종류별", "bytype"]) {
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
    if !t.has(&["태그", "tag"]) {
        return None;
    }
    let remove = t.has(&["제거", "빼", "삭제", "지워", "remove", "untag", "delete"]);
    let add = t.has(&["추가", "붙여", "달아", "넣어", "add", "tagas", "tagwith", "apply"]);
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
    let names: Vec<String> = rest
        .split([',', '·', '/'])
        .map(|s| s.trim().trim_matches(|c: char| "\"'“”‘’()[]".contains(c)).trim())
        .map(|s| s.trim_end_matches(['을', '를', '로']).trim_start_matches(['를', '을']).trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().count() <= 64)
        .collect();
    if names.is_empty() {
        return None;
    }
    let dt = doc_type(t, ctx);
    let scope = scope_of(t, ctx, dt, None);
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
    if !t.has(&["삭제", "지워", "지우", "delete", "remove", "trash", "휴지통"]) {
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
    let classify = t.has(&["분류", "classif", "유형판정", "유형을정"]);
    let ocr = t.has(&["ocr", "문자인식", "텍스트인식", "글자인식", "텍스트추출", "읽어"]);
    let extract = t.has(&["추출", "extract", "필드뽑", "값뽑", "kie"]) && !t.has(&["텍스트추출"]);
    let validate = t.has(&["검증", "validate", "검사해"]);
    let summarize = t.has(&["요약", "summar"]);
    let generic = t.has(&["처리", "process", "돌려", "실행해"]) && !classify && !ocr && !extract && !summarize;
    if !(classify || ocr || extract || validate || summarize || generic) {
        return None;
    }
    let force = t.has(&["다시", "재", "again", "re-", "redo", "force"]);
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
        let target = if t.has(&["하나로", "합쳐", "묶어", "전체요약", "together", "onesummary", "combined"])
        {
            SummarizeTarget::Set
        } else {
            SummarizeTarget::EachDoc
        };
        let length = if t.has(&["짧게", "한줄", "간단", "short", "brief"]) {
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
        let profile = if t.has(&["잔글씨", "작은글씨", "fineprint", "영역", "region", "잘안읽"]) {
            Some(OcrProfile::FinePrint)
        } else {
            None
        };
        let pages = if t.has(&["이페이지", "현재페이지", "선택영역", "thispage", "selection"]) {
            ctx.current_page.into_iter().collect()
        } else {
            Vec::new()
        };
        ops.push(Op::Ocr { pages, profile, force: (force || ocr).then_some(true) });
    }
    if extract || generic {
        ops.push(Op::Extract {
            schema_ref: dt.map(|d| d.slug.clone()),
            fields: Vec::new(),
            force: force.then_some(true),
        });
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
            Op::Extract { .. } => t.t("추출", "extract"),
            Op::Validate {} => t.t("검증", "validate"),
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
    if !t.has(&["검토", "review"]) {
        return None;
    }
    let only = if t.has(&["저신뢰", "신뢰도낮", "lowconfidence"]) {
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
    if !t.has(&["찾아", "찾기", "검색", "find", "search", "lookfor", "whereis", "locate"]) {
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
    let q = q.trim().trim_end_matches(['?', '？', '.']).trim().to_string();
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
        Op::Answer { question: t.raw.trim().to_string(), cite: Some(true) },
        scope,
        0.8,
        t.t("지식 베이스·문서에서 근거를 찾아 답합니다", "Answer with evidence from the knowledge base / documents")
            .into(),
    ))
}

/// Tries the rules in order; `None` leaves the command to the model.
pub fn plan(text: &str, ctx: &PlanContext) -> Option<Plan> {
    let raw = text.trim();
    if raw.is_empty() {
        return None;
    }
    let t = Text {
        raw,
        lower: raw.to_lowercase(),
        compact: compact(raw),
        ko: ctx.language == Language::Ko || raw.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c)),
    };
    undo_redo(&t)
        .or_else(|| job_control(&t))
        .or_else(|| engine(&t))
        .or_else(|| settings_combo(&t, ctx))
        .or_else(|| define_schema(&t, ctx))
        .or_else(|| export(&t, ctx))
        .or_else(|| aggregate(&t, ctx))
        .or_else(|| inbox(&t, ctx))
        .or_else(|| tag(&t, ctx))
        .or_else(|| organize(&t, ctx))
        .or_else(|| delete(&t, ctx))
        .or_else(|| process(&t, ctx))
        .or_else(|| review(&t))
        .or_else(|| search(&t, ctx))
        .or_else(|| open_document(&t))
        .or_else(|| answer(&t, ctx))
}
