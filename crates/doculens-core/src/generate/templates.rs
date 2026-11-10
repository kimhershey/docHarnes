//! Built-in templates. Since 0.3.0 the visible set is the software-development
//! set (제안요청서·제안서·과업지시서·요구사항 정의서·개발 계약서·WBS·진척 보고서·
//! 회의록·테스트 결과서·변경요청서·검수확인서·견적서) plus the 보완요청서 of §3.15;
//! the generic 0.1 templates (공문·견적서(품목)·거래명세서·계약서·보고서·회의록) stay
//! defined but hidden. Slugs and field keys equal the pipeline's built-in schemas
//! (`project_name`, `amount`, `supply_total`, `grand_total`, `supplier.name`,
//! `items[]` 역할·등급·투입 MM·단가·공급가액 …) so extracted fields of source
//! documents map onto template fields by key and registered forms fill the same way.

use crate::pipeline::schema::{FieldKind, SchemaRecord};

use super::{DraftColumn, TemplateField, TemplateInfo, TemplateKind, TemplateRef};

fn field(key: &str, ko: &str, en: &str, kind: FieldKind, required: bool) -> TemplateField {
    TemplateField {
        key: key.into(),
        label_ko: ko.into(),
        label_en: en.into(),
        kind: kind.as_str().into(),
        required,
        columns: Vec::new(),
    }
}

/// `en` is looked up through [`column_label_en`] by the UI; `DraftColumn` carries one label.
fn column(key: &str, ko: &str, _en: &str, kind: FieldKind) -> DraftColumn {
    DraftColumn { key: key.into(), label: ko.into(), kind: kind.as_str().into() }
}

/// English label of a column (the UI picks by language; `DraftColumn` carries one label).
pub fn column_label_en(key: &str) -> &'static str {
    match key {
        "item_name" => "Item",
        "spec" => "Spec",
        "quantity" => "Qty",
        "unit_price" => "Unit price",
        "supply_amount" => "Supply amount",
        "tax_amount" => "Tax",
        "note" => "Note",
        "item" => "Item",
        "owner" => "Owner",
        "due_date" => "Due",
        "document" => "Document",
        "issue" => "Finding",
        "evidence" => "Evidence",
        "action" => "Requested action",
        "role" => "Role",
        "grade" => "Grade",
        "months" => "Person-months",
        "id" => "ID",
        "name" => "Name",
        "description" => "Description",
        "category" => "Category",
        "priority" => "Priority",
        "source" => "Source",
        "acceptance" => "Acceptance criteria",
        "version" => "Version",
        "phase" => "Phase",
        "start_date" => "Start",
        "end_date" => "End",
        "points" => "Points",
        "task" => "Task",
        "deliverable" => "Deliverable",
        "ratio" => "Ratio (%)",
        "amount" => "Amount",
        "progress" => "Progress (%)",
        "planned" => "Planned",
        "actual" => "Actual",
        "impact" => "Impact",
        "organization" => "Organization",
        "detail" => "Detail",
        "scenario" => "Scenario",
        "expected" => "Expected",
        "result" => "Result",
        "defect_id" => "Defect id",
        "severity" => "Severity",
        "status" => "Status",
        "date" => "Date",
        _ => "",
    }
}

fn table(key: &str, ko: &str, en: &str, columns: Vec<DraftColumn>, required: bool) -> TemplateField {
    TemplateField {
        key: key.into(),
        label_ko: ko.into(),
        label_en: en.into(),
        kind: FieldKind::Table.as_str().into(),
        required,
        columns,
    }
}

/// 품명·규격·수량·단가·공급가액·세액·비고 — the same keys as the pipeline's `items[]`.
pub fn item_columns() -> Vec<DraftColumn> {
    use FieldKind::*;
    vec![
        column("item_name", "품명", "Item", String),
        column("spec", "규격", "Spec", String),
        column("quantity", "수량", "Qty", Integer),
        column("unit_price", "단가", "Unit price", Amount),
        column("supply_amount", "공급가액", "Supply amount", Amount),
        column("tax_amount", "세액", "Tax", Amount),
        column("note", "비고", "Note", String),
    ]
}

fn party(prefix: &str, ko: &str, en: &str, name_required: bool) -> Vec<TemplateField> {
    use FieldKind::*;
    vec![
        field(&format!("{prefix}.name"), &format!("{ko} 상호"), &format!("{en} name"), String, name_required),
        field(
            &format!("{prefix}.brn"),
            &format!("{ko} 사업자등록번호"),
            &format!("{en} business reg. no."),
            Brn,
            false,
        ),
        field(
            &format!("{prefix}.representative"),
            &format!("{ko} 대표자"),
            &format!("{en} representative"),
            String,
            false,
        ),
        field(&format!("{prefix}.address"), &format!("{ko} 주소"), &format!("{en} address"), String, false),
        field(&format!("{prefix}.phone"), &format!("{ko} 전화"), &format!("{en} phone"), Phone, false),
    ]
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Software-development party block (mirrors the pipeline's `sw_party`): name, registration
/// number, representative, contact person, phone.
fn sw_party(prefix: &str, ko: &str, en: &str, name_required: bool) -> Vec<TemplateField> {
    use FieldKind::*;
    vec![
        field(&format!("{prefix}.name"), &format!("{ko} 명칭"), &format!("{en} name"), String, name_required),
        field(
            &format!("{prefix}.brn"),
            &format!("{ko} 사업자등록번호"),
            &format!("{en} business reg. no."),
            Brn,
            false,
        ),
        field(
            &format!("{prefix}.representative"),
            &format!("{ko} 대표자"),
            &format!("{en} representative"),
            String,
            false,
        ),
        field(
            &format!("{prefix}.contact_person"),
            &format!("{ko} 담당자"),
            &format!("{en} contact person"),
            String,
            false,
        ),
        field(&format!("{prefix}.phone"), &format!("{ko} 연락처"), &format!("{en} phone"), Phone, false),
    ]
}

/// 역할·등급·투입 MM·단가·공급가액·비고 — the pipeline's `estimate.items[]` columns.
pub fn staffing_columns() -> Vec<DraftColumn> {
    use FieldKind::*;
    vec![
        column("role", "역할·품목", "Role / item", String),
        column("grade", "등급", "Grade", String),
        column("months", "투입 MM", "Person-months", Amount),
        column("unit_price", "단가", "Unit price", Amount),
        column("supply_amount", "공급가액", "Supply amount", Amount),
        column("note", "비고", "Note", String),
    ]
}

fn requirements_columns(full: bool) -> Vec<DraftColumn> {
    use FieldKind::*;
    let mut cols = vec![
        column("id", "ID", "ID", String),
        column("name", "요구사항명", "Name", String),
        column("description", "상세 설명", "Description", String),
        column("category", "구분", "Category", String),
        column("priority", "우선순위", "Priority", String),
    ];
    if full {
        cols.push(column("source", "출처", "Source", String));
        cols.push(column("acceptance", "수용 기준", "Acceptance criteria", String));
    }
    cols
}

fn deliverables_columns(with_result: bool) -> Vec<DraftColumn> {
    use FieldKind::*;
    let mut cols = vec![
        column("name", "산출물", "Deliverable", String),
        column("version", "버전", "Version", String),
        column("due_date", "납기", "Due date", Date),
    ];
    if with_result {
        cols.push(column("result", "검수 결과", "Result", String));
    }
    cols.push(column("note", "비고", "Note", String));
    cols
}

fn schedule_columns() -> Vec<DraftColumn> {
    use FieldKind::*;
    vec![
        column("phase", "단계", "Phase", String),
        column("start_date", "시작", "Start", Date),
        column("end_date", "종료", "End", Date),
        column("note", "비고", "Note", String),
    ]
}

/// The software-development templates in UI order (slugs = pipeline document types).
fn software_list() -> Vec<TemplateInfo> {
    use FieldKind::*;
    vec![
        TemplateInfo {
            kind: TemplateKind::Rfp,
            name_ko: "제안요청서(RFP)".into(),
            name_en: "Request for proposal (RFP)".into(),
            description_ko: "발주기관의 소프트웨어 개발 제안요청서 — 사업 개요·예산·기간·요구사항·산출물·일정·평가 기준.".into(),
            description_en: "RFP for software development: overview, budget, period, requirements, deliverables, schedule, evaluation."
                .into(),
            fields: {
                let mut f = vec![field("project_name", "사업명", "Project name", String, true)];
                f.extend(sw_party("issuer", "발주기관", "Issuer", true));
                f.extend(vec![
                    field("issue_date", "공고일", "Issue date", Date, true),
                    field("budget", "사업 예산", "Budget", Amount, false),
                    field("period_start", "사업 기간 시작", "Period start", Date, false),
                    field("period_end", "사업 기간 종료", "Period end", Date, false),
                    field("submission_deadline", "제출 마감", "Submission deadline", Date, false),
                    field("summary", "사업 개요", "Overview", String, false),
                    table("requirements", "요구사항", "Requirements", requirements_columns(false), true),
                    table("deliverables", "산출물", "Deliverables", deliverables_columns(false), false),
                    table("schedule", "일정", "Schedule", schedule_columns(), false),
                    table(
                        "evaluation",
                        "평가 기준",
                        "Evaluation criteria",
                        vec![column("item", "항목", "Item", String), column("points", "배점", "Points", Amount)],
                        false,
                    ),
                ]);
                f
            },
            sections_ko: strings(&[
                "사업 개요",
                "추진 배경 및 목적",
                "사업 범위",
                "요구사항",
                "산출물",
                "추진 일정",
                "제안서 작성 및 평가 기준",
                "계약 조건",
            ]),
            sections_en: strings(&[
                "Overview",
                "Background and objectives",
                "Scope",
                "Requirements",
                "Deliverables",
                "Schedule",
                "Proposal and evaluation criteria",
                "Contract terms",
            ]),
            keywords: strings(&["제안요청서", "제안 요청서", "rfp", "request for proposal"]),
        },
        TemplateInfo {
            kind: TemplateKind::Proposal,
            name_ko: "제안서".into(),
            name_en: "Proposal".into(),
            description_ko: "RFP에 대한 제안서 — 제안사·제안 금액·수행 기간·접근 방법·투입 인력·일정·산출물.".into(),
            description_en: "Proposal: proposer, price, period, approach, staffing, schedule and deliverables.".into(),
            fields: {
                let mut f = vec![field("project_name", "사업명", "Project name", String, true)];
                f.extend(sw_party("proposer", "제안사", "Proposer", true));
                f.extend(vec![
                    field("client_name", "발주기관", "Client", String, false),
                    field("issue_date", "제출일", "Submission date", Date, true),
                    field("price", "제안 금액", "Proposed price", Amount, true),
                    field("period_start", "수행 기간 시작", "Period start", Date, false),
                    field("period_end", "수행 기간 종료", "Period end", Date, false),
                    field("duration_months", "수행 기간(개월)", "Duration (months)", Integer, false),
                    field("approach", "수행 방법론 요약", "Approach", String, false),
                    table(
                        "team",
                        "투입 인력",
                        "Team",
                        vec![
                            column("role", "역할", "Role", String),
                            column("name", "성명", "Name", String),
                            column("grade", "등급", "Grade", String),
                            column("months", "투입 MM", "Person-months", Amount),
                        ],
                        false,
                    ),
                    table("schedule", "일정", "Schedule", schedule_columns(), false),
                    table("deliverables", "산출물", "Deliverables", deliverables_columns(false), false),
                ]);
                f
            },
            sections_ko: strings(&[
                "제안 개요",
                "사업 이해 및 접근 방법",
                "수행 범위 및 방법론",
                "투입 인력",
                "추진 일정",
                "품질·보안 관리",
                "제안 가격",
            ]),
            sections_en: strings(&[
                "Overview",
                "Understanding and approach",
                "Scope and methodology",
                "Staffing",
                "Schedule",
                "Quality and security",
                "Price",
            ]),
            keywords: strings(&["제안서", "proposal"]),
        },
        TemplateInfo {
            kind: TemplateKind::Sow,
            name_ko: "과업지시서".into(),
            name_en: "Statement of work".into(),
            description_ko: "과업지시서(과업내용서) — 발주자·수행자, 과업 범위, 세부 과업, 산출물, 검수 기준, 단계별 납기·대금.".into(),
            description_en: "Statement of work: parties, scope, tasks, deliverables, acceptance criteria, milestones and payments."
                .into(),
            fields: {
                let mut f = vec![field("project_name", "과업명", "Project name", String, true)];
                f.extend(sw_party("client", "발주자", "Client", true));
                f.extend(sw_party("contractor", "수행자", "Contractor", false));
                f.extend(vec![
                    field("issue_date", "작성일", "Issue date", Date, true),
                    field("period_start", "과업 기간 시작", "Period start", Date, false),
                    field("period_end", "과업 기간 종료", "Period end", Date, false),
                    field("amount", "과업 금액", "Amount", Amount, false),
                    field("scope", "과업 범위", "Scope", String, false),
                    field("acceptance_criteria", "검수 기준", "Acceptance criteria", String, false),
                    table(
                        "tasks",
                        "세부 과업",
                        "Tasks",
                        vec![
                            column("id", "번호", "No.", String),
                            column("task", "과업", "Task", String),
                            column("description", "내용", "Description", String),
                            column("deliverable", "산출물", "Deliverable", String),
                        ],
                        true,
                    ),
                    table("deliverables", "산출물", "Deliverables", deliverables_columns(false), false),
                    table(
                        "milestones",
                        "단계별 납기·대금",
                        "Milestones",
                        vec![
                            column("phase", "단계", "Phase", String),
                            column("due_date", "납기", "Due date", Date),
                            column("ratio", "대금 비율(%)", "Payment ratio (%)", Amount),
                            column("amount", "금액", "Amount", Amount),
                        ],
                        false,
                    ),
                ]);
                f
            },
            sections_ko: strings(&["과업 개요", "과업 범위", "세부 과업", "산출물 및 납품", "검수 기준", "일정 및 대금", "기타 조건"]),
            sections_en: strings(&[
                "Overview",
                "Scope",
                "Tasks",
                "Deliverables",
                "Acceptance criteria",
                "Schedule and payment",
                "Other terms",
            ]),
            keywords: strings(&["과업지시서", "과업 지시서", "과업내용서", "sow", "statement of work"]),
        },
        TemplateInfo {
            kind: TemplateKind::Srs,
            name_ko: "요구사항 정의서".into(),
            name_en: "Requirements specification".into(),
            description_ko: "요구사항 정의서(SRS) — 버전·작성자·승인자, 기능·비기능·인터페이스 요구사항 표, 제약·가정, 수용 기준.".into(),
            description_en: "Software requirements specification: version, author, approver, requirements table, constraints, acceptance."
                .into(),
            fields: vec![
                field("project_name", "사업명", "Project name", String, true),
                field("version", "버전", "Version", String, true),
                field("issue_date", "작성일", "Issue date", Date, true),
                field("author", "작성자", "Author", String, false),
                field("approver", "승인자", "Approver", String, false),
                field("summary", "시스템 개요", "System overview", String, false),
                table("requirements", "요구사항", "Requirements", requirements_columns(true), true),
            ],
            sections_ko: strings(&[
                "개요",
                "시스템 범위",
                "기능 요구사항",
                "비기능 요구사항",
                "인터페이스 요구사항",
                "제약 사항 및 가정",
                "수용 기준",
            ]),
            sections_en: strings(&[
                "Overview",
                "System scope",
                "Functional requirements",
                "Non-functional requirements",
                "Interface requirements",
                "Constraints and assumptions",
                "Acceptance criteria",
            ]),
            keywords: strings(&[
                "요구사항 정의서",
                "요구사항정의서",
                "요구사항 명세서",
                "요구사항명세서",
                "srs",
                "requirements specification",
                "requirements",
            ]),
        },
        TemplateInfo {
            kind: TemplateKind::DevContract,
            name_ko: "소프트웨어 개발 계약서".into(),
            name_en: "Software development contract".into(),
            description_ko: "소프트웨어 개발(외주·용역) 계약서 — 갑·을, 계약일·기간·금액, 대금 지급 단계, 지식재산권·하자보수·비밀유지·검수 조항.".into(),
            description_en: "Software development contract: parties, date, period, amount, payment milestones, IP, warranty, confidentiality, acceptance."
                .into(),
            fields: {
                let mut f = vec![
                    field("title", "계약명", "Title", String, true),
                    field("contract_date", "계약일", "Contract date", Date, true),
                ];
                f.extend(party("client", "발주자(갑)", "Client (party A)", true));
                f.extend(party("contractor", "수행사(을)", "Contractor (party B)", true));
                f.extend(vec![
                    field("period_start", "계약기간 시작", "Period start", Date, false),
                    field("period_end", "계약기간 종료", "Period end", Date, false),
                    field("amount", "계약금액", "Contract amount", Amount, true),
                    field("vat_included", "부가세 포함 여부", "VAT included", Boolean, false),
                    field("payment_terms", "지급 조건", "Payment terms", String, false),
                    field("warranty_months", "하자보수 기간(개월)", "Warranty (months)", Integer, false),
                    field("ip_owner", "지식재산권 귀속", "IP owner", String, false),
                    field("confidentiality", "비밀유지 조항", "Confidentiality clause", Boolean, false),
                    table(
                        "payments",
                        "대금 지급 단계",
                        "Payment milestones",
                        vec![
                            column("phase", "단계", "Phase", String),
                            column("ratio", "비율(%)", "Ratio (%)", Amount),
                            column("amount", "금액", "Amount", Amount),
                            column("due_date", "지급 시기", "Due date", Date),
                        ],
                        false,
                    ),
                ]);
                f
            },
            sections_ko: strings(&[
                "제1조 (목적)",
                "제2조 (과업의 범위)",
                "제3조 (계약기간)",
                "제4조 (계약금액 및 대금 지급)",
                "제5조 (산출물 및 검수)",
                "제6조 (지식재산권)",
                "제7조 (하자보수)",
                "제8조 (비밀유지)",
                "제9조 (계약의 변경 및 해지)",
                "제10조 (분쟁 해결)",
            ]),
            sections_en: strings(&[
                "Article 1 (Purpose)",
                "Article 2 (Scope of work)",
                "Article 3 (Term)",
                "Article 4 (Contract amount and payment)",
                "Article 5 (Deliverables and acceptance)",
                "Article 6 (Intellectual property)",
                "Article 7 (Warranty)",
                "Article 8 (Confidentiality)",
                "Article 9 (Changes and termination)",
                "Article 10 (Disputes)",
            ]),
            keywords: strings(&["개발 계약서", "개발계약서", "용역 계약서", "용역계약서", "계약서", "계약", "contract", "agreement"]),
        },
        TemplateInfo {
            kind: TemplateKind::Wbs,
            name_ko: "WBS·일정표".into(),
            name_en: "WBS / schedule".into(),
            description_ko: "작업 분류 체계와 일정표 — 작업별 담당·시작·종료·진척률·산출물, 마일스톤.".into(),
            description_en: "Work breakdown structure and schedule: tasks with owner, dates, progress and deliverable; milestones.".into(),
            fields: vec![
                field("project_name", "사업명", "Project name", String, true),
                field("version", "버전", "Version", String, false),
                field("issue_date", "작성일", "Issue date", Date, true),
                field("period_start", "전체 시작", "Period start", Date, false),
                field("period_end", "전체 종료", "Period end", Date, false),
                table(
                    "tasks",
                    "작업",
                    "Tasks",
                    vec![
                        column("id", "WBS 번호", "WBS id", String),
                        column("task", "작업", "Task", String),
                        column("owner", "담당", "Owner", String),
                        column("start_date", "시작", "Start", Date),
                        column("end_date", "종료", "End", Date),
                        column("progress", "진척률(%)", "Progress (%)", Amount),
                        column("deliverable", "산출물", "Deliverable", String),
                    ],
                    true,
                ),
                table(
                    "milestones",
                    "마일스톤",
                    "Milestones",
                    vec![column("name", "마일스톤", "Milestone", String), column("date", "일자", "Date", Date)],
                    false,
                ),
            ],
            sections_ko: strings(&["개요", "작업 분류 체계", "마일스톤"]),
            sections_en: strings(&["Overview", "Work breakdown", "Milestones"]),
            keywords: strings(&["wbs", "일정표", "작업분류체계", "작업 분류 체계", "work breakdown"]),
        },
        TemplateInfo {
            kind: TemplateKind::ProgressReport,
            name_ko: "진척 보고서".into(),
            name_en: "Progress report".into(),
            description_ko: "주간·월간·단계 진척 보고서 — 보고 기간, 전체·계획 진척률, 작업별 실적, 이슈·리스크, 차기 계획.".into(),
            description_en: "Weekly / monthly progress report: period, overall vs planned progress, task results, issues, next plan."
                .into(),
            fields: vec![
                field("project_name", "사업명", "Project name", String, true),
                field("report_type", "보고 구분", "Report type", String, false),
                field("period_start", "보고 기간 시작", "Period start", Date, false),
                field("period_end", "보고 기간 종료", "Period end", Date, false),
                field("report_date", "보고일", "Report date", Date, true),
                field("author", "작성자", "Author", String, false),
                field("progress_percent", "전체 진척률(%)", "Overall progress (%)", Amount, false),
                field("plan_percent", "계획 진척률(%)", "Planned progress (%)", Amount, false),
                field("summary", "주요 실적", "Summary", String, false),
                field("next_plan", "차기 계획", "Next plan", String, false),
                table(
                    "tasks",
                    "작업별 실적",
                    "Task results",
                    vec![
                        column("task", "작업", "Task", String),
                        column("owner", "담당", "Owner", String),
                        column("planned", "계획", "Planned", String),
                        column("actual", "실적", "Actual", String),
                        column("progress", "진척률(%)", "Progress (%)", Amount),
                    ],
                    false,
                ),
                table(
                    "issues",
                    "이슈·리스크",
                    "Issues / risks",
                    vec![
                        column("issue", "이슈", "Issue", String),
                        column("impact", "영향도", "Impact", String),
                        column("action", "조치", "Action", String),
                        column("owner", "담당", "Owner", String),
                        column("due_date", "기한", "Due date", Date),
                    ],
                    false,
                ),
            ],
            sections_ko: strings(&["보고 개요", "금주 실적", "차주 계획", "이슈 및 리스크", "지원 요청 사항"]),
            sections_en: strings(&["Overview", "Results this period", "Plan for next period", "Issues and risks", "Support needed"]),
            keywords: strings(&[
                "진척 보고서",
                "진척보고서",
                "진척 보고",
                "주간 보고서",
                "주간보고서",
                "주간 보고",
                "월간 보고서",
                "월간보고서",
                "월간 보고",
                "진도 보고",
                "progress report",
                "weekly report",
                "status report",
            ]),
        },
        TemplateInfo {
            kind: TemplateKind::MeetingMinutes,
            name_ko: "회의록".into(),
            name_en: "Meeting minutes".into(),
            description_ko: "회의록 — 일시·장소·참석자 표, 안건·논의 내용, 결정 사항 표, 실행 항목(담당·기한), 차기 회의.".into(),
            description_en: "Meeting minutes: date, place, attendees, agenda, discussion, decisions, action items, next meeting.".into(),
            fields: vec![
                field("title", "회의명", "Title", String, true),
                field("project_name", "사업명", "Project name", String, false),
                field("meeting_date", "일시", "Date", Date, true),
                field("location", "장소", "Location", String, false),
                field("author", "작성자", "Author", String, false),
                field("next_meeting", "차기 회의", "Next meeting", Date, false),
                table(
                    "attendees",
                    "참석자",
                    "Attendees",
                    vec![
                        column("name", "성명", "Name", String),
                        column("organization", "소속", "Organization", String),
                        column("role", "역할", "Role", String),
                    ],
                    true,
                ),
                table(
                    "decisions",
                    "결정 사항",
                    "Decisions",
                    vec![column("item", "항목", "Item", String), column("detail", "내용", "Detail", String)],
                    false,
                ),
                table(
                    "action_items",
                    "실행 항목",
                    "Action items",
                    vec![
                        column("item", "항목", "Item", String),
                        column("owner", "담당", "Owner", String),
                        column("due_date", "기한", "Due date", Date),
                    ],
                    false,
                ),
            ],
            sections_ko: strings(&["안건", "논의 내용", "결정 사항", "실행 항목"]),
            sections_en: strings(&["Agenda", "Discussion", "Decisions", "Action items"]),
            keywords: strings(&["회의록", "회의 록", "meeting minutes", "minutes"]),
        },
        TemplateInfo {
            kind: TemplateKind::TestReport,
            name_ko: "테스트 결과서".into(),
            name_en: "Test report".into(),
            description_ko: "테스트 결과서 — 대상 버전·일자·구분·환경, 케이스 집계(총·통과·실패·보류·통과율), 케이스별 결과, 결함 목록.".into(),
            description_en: "Test report: version, date, type, environment, case totals and pass rate, case results, defect list.".into(),
            fields: vec![
                field("project_name", "사업명", "Project name", String, true),
                field("version", "대상 버전", "Version under test", String, false),
                field("test_date", "테스트 일자", "Test date", Date, true),
                field("tester", "테스터", "Tester", String, false),
                field("test_type", "테스트 구분", "Test type", String, false),
                field("environment", "테스트 환경", "Environment", String, false),
                field("total_cases", "총 케이스", "Total cases", Integer, false),
                field("passed", "통과", "Passed", Integer, false),
                field("failed", "실패", "Failed", Integer, false),
                field("blocked", "보류", "Blocked", Integer, false),
                field("pass_rate", "통과율(%)", "Pass rate (%)", Amount, false),
                table(
                    "cases",
                    "케이스",
                    "Cases",
                    vec![
                        column("id", "ID", "ID", String),
                        column("scenario", "시나리오", "Scenario", String),
                        column("expected", "기대 결과", "Expected", String),
                        column("result", "결과", "Result", String),
                        column("defect_id", "결함 ID", "Defect id", String),
                    ],
                    true,
                ),
                table(
                    "defects",
                    "결함",
                    "Defects",
                    vec![
                        column("id", "결함 ID", "Defect id", String),
                        column("severity", "심각도", "Severity", String),
                        column("description", "내용", "Description", String),
                        column("status", "상태", "Status", String),
                    ],
                    false,
                ),
            ],
            sections_ko: strings(&["테스트 개요", "테스트 범위 및 환경", "결과 요약", "결함 목록", "결론 및 권고"]),
            sections_en: strings(&["Overview", "Scope and environment", "Summary of results", "Defects", "Conclusion"]),
            keywords: strings(&[
                "테스트 결과서",
                "테스트결과서",
                "시험 결과서",
                "시험결과서",
                "테스트 보고서",
                "test report",
                "test results",
            ]),
        },
        TemplateInfo {
            kind: TemplateKind::ChangeRequest,
            name_ko: "변경요청서".into(),
            name_en: "Change request".into(),
            description_ko: "변경요청서(CR) — 번호·요청자·일자, 변경 내용·사유, 일정·비용 영향, 우선순위, 승인 상태·승인자.".into(),
            description_en: "Change request: number, requester, date, change and reason, schedule / cost impact, priority, approval."
                .into(),
            fields: vec![
                field("cr_no", "CR 번호", "CR no.", String, true),
                field("project_name", "사업명", "Project name", String, false),
                field("request_date", "요청일", "Request date", Date, true),
                field("requester", "요청자", "Requester", String, false),
                field("title", "변경 제목", "Title", String, true),
                field("description", "변경 내용", "Description", String, false),
                field("reason", "변경 사유", "Reason", String, false),
                field("impact_schedule", "일정 영향", "Schedule impact", String, false),
                field("impact_cost", "비용 영향", "Cost impact", Amount, false),
                field("priority", "우선순위", "Priority", String, false),
                field("status", "승인 상태", "Status", String, false),
                field("approver", "승인자", "Approver", String, false),
                field("approval_date", "승인일", "Approval date", Date, false),
            ],
            sections_ko: strings(&["변경 개요", "변경 사유", "영향 분석", "승인"]),
            sections_en: strings(&["Change overview", "Reason", "Impact analysis", "Approval"]),
            keywords: strings(&["변경요청서", "변경 요청서", "변경요청", "change request"]),
        },
        TemplateInfo {
            kind: TemplateKind::Acceptance,
            name_ko: "검수확인서".into(),
            name_en: "Acceptance certificate".into(),
            description_ko: "검수확인서·완료보고서 — 발주자·수행사, 검수일, 산출물별 검수 결과, 검수 금액, 최종 판정, 확인.".into(),
            description_en: "Acceptance certificate: parties, acceptance date, per-deliverable results, amount, verdict, confirmation."
                .into(),
            fields: {
                let mut f = vec![field("project_name", "사업명", "Project name", String, true)];
                f.extend(sw_party("client", "발주자", "Client", true));
                f.extend(sw_party("contractor", "수행사", "Contractor", true));
                f.extend(vec![
                    field("acceptance_date", "검수일", "Acceptance date", Date, true),
                    field("period_start", "사업 기간 시작", "Period start", Date, false),
                    field("period_end", "사업 기간 종료", "Period end", Date, false),
                    field("amount", "검수 금액", "Amount", Amount, false),
                    field("result", "최종 판정", "Verdict", String, false),
                    field("conditions", "조건·특이사항", "Conditions / remarks", String, false),
                    table("deliverables", "산출물", "Deliverables", deliverables_columns(true), true),
                ]);
                f
            },
            sections_ko: strings(&["사업 개요", "산출물 검수 결과", "특이사항", "확인"]),
            sections_en: strings(&["Overview", "Deliverable inspection", "Remarks", "Confirmation"]),
            keywords: strings(&[
                "검수확인서",
                "검수 확인서",
                "검수조서",
                "완료보고서",
                "완료 보고서",
                "검수",
                "acceptance certificate",
                "acceptance",
            ]),
        },
        TemplateInfo {
            kind: TemplateKind::Estimate,
            name_ko: "견적서".into(),
            name_en: "Estimate".into(),
            description_ko: "소프트웨어 개발 견적서(인력·MM 기준) — 공급자·수신·유효기간, 역할별 등급·투입 MM·단가·공급가액, 합계·세액.".into(),
            description_en: "Software development estimate (person-month based): supplier, recipient, validity, per-role staffing lines, totals."
                .into(),
            fields: {
                let mut f = vec![
                    field("document_no", "견적번호", "Estimate no.", String, false),
                    field("issue_date", "견적일", "Issue date", Date, true),
                    field("project_name", "사업명", "Project name", String, false),
                    field("recipient", "수신", "Recipient", String, true),
                    field("valid_until", "유효기간", "Valid until", Date, false),
                ];
                f.extend(party("supplier", "공급자", "Supplier", true));
                f.extend(vec![
                    field("contact_person", "담당자", "Contact person", String, false),
                    field("supply_total", "공급가액 합계", "Supply total", Amount, false),
                    field("tax_total", "세액 합계", "Tax total", Amount, false),
                    field("grand_total", "합계금액", "Grand total", Amount, true),
                    field("payment_terms", "결제조건", "Payment terms", String, false),
                    field("note", "비고", "Note", String, false),
                    table("items", "견적 내역", "Items", staffing_columns(), true),
                ]);
                f
            },
            sections_ko: strings(&["견적 조건", "비고"]),
            sections_en: strings(&["Terms", "Notes"]),
            keywords: strings(&["견적서", "견적", "estimate", "quotation", "quote"]),
        },
    ]
}

/// The 보완요청서 of §3.15 (rule-composed, kept visible).
fn supplement_request() -> TemplateInfo {
    use FieldKind::*;
    TemplateInfo {
        kind: TemplateKind::SupplementRequest,
        name_ko: "보완요청서".into(),
        name_en: "Supplement request".into(),
        description_ko: "수신·제목·근거·보완 항목 표·기한·서명으로 구성된 보완 요청 공문(§3.15).".into(),
        description_en:
            "Request for supplementary documents: recipient, subject, basis, item table, deadline, signature.".into(),
        fields: vec![
            field("document_no", "문서번호", "Document no.", String, false),
            field("issue_date", "발송일", "Date", Date, true),
            field("recipient", "수신", "Recipient", String, true),
            field("sender", "발신", "Sender", String, false),
            field("title", "제목", "Subject", String, true),
            field("subject", "대상 건명", "Case", String, false),
            field("due_date", "보완 기한", "Deadline", Date, true),
            field("contact_person", "담당자", "Contact person", String, false),
            field("phone", "연락처", "Phone", Phone, false),
            table(
                "items",
                "보완 항목",
                "Items",
                vec![
                    column("document", "대상 문서", "Document", String),
                    column("issue", "지적 사항", "Finding", String),
                    column("evidence", "근거", "Evidence", String),
                    column("action", "조치 요청", "Requested action", String),
                ],
                true,
            ),
        ],
        sections_ko: strings(&["근거", "보완 요청 사항", "안내"]),
        sections_en: strings(&["Basis", "Items to supplement", "Notice"]),
        keywords: strings(&[
            "보완요청",
            "보완 요청",
            "보완요청서",
            "보완",
            "supplement request",
            "supplement",
            "deficiency",
        ]),
    }
}

/// The built-in templates the UI offers, in UI order: the software set + 보완요청서.
pub fn list() -> Vec<TemplateInfo> {
    let mut all = software_list();
    all.push(supplement_request());
    all
}

/// Every built-in template, the hidden generic ones of 0.1 included.
pub fn list_all() -> Vec<TemplateInfo> {
    let mut all = list();
    all.extend(legacy_list());
    all
}

/// The generic templates of 0.1 (hidden since 0.3.0).
fn legacy_list() -> Vec<TemplateInfo> {
    use FieldKind::*;
    vec![
        TemplateInfo {
            kind: TemplateKind::OfficialLetter,
            name_ko: "공문".into(),
            name_en: "Official letter".into(),
            description_ko: "수신·참조·제목·본문·붙임·발신 기관으로 구성된 공문(시행문).".into(),
            description_en: "Formal letter with recipient, cc, subject, body, attachments and sender.".into(),
            fields: vec![
                field("document_no", "문서번호", "Document no.", String, false),
                field("issue_date", "시행일자", "Date", Date, true),
                field("sender", "발신", "Sender", String, true),
                field("recipient", "수신", "Recipient", String, true),
                field("cc", "참조", "Cc", String, false),
                field("title", "제목", "Subject", String, true),
                field("contact_person", "담당자", "Contact person", String, false),
                field("phone", "연락처", "Phone", Phone, false),
                field("attachments", "붙임", "Attachments", String, false),
            ],
            sections_ko: strings(&["본문"]),
            sections_en: strings(&["Body"]),
            keywords: strings(&["공문", "공문서", "시행문", "협조문", "official letter", "formal letter", "letter"]),
        },
        TemplateInfo {
            kind: TemplateKind::Quotation,
            name_ko: "견적서".into(),
            name_en: "Quotation".into(),
            description_ko: "공급자·수신처·품목 표(수량×단가=공급가액)·합계·견적 조건.".into(),
            description_en: "Supplier, recipient, item table (qty × unit price), totals and terms.".into(),
            fields: {
                let mut f = vec![
                    field("document_no", "견적번호", "Quotation no.", String, false),
                    field("issue_date", "견적일자", "Quotation date", Date, true),
                    field("recipient", "수신", "Recipient", String, true),
                    field("valid_until", "유효기간", "Valid until", String, false),
                ];
                f.extend(party("supplier", "공급자", "Supplier", true));
                f.extend(vec![
                    field("contact_person", "담당자", "Contact person", String, false),
                    field("supply_total", "공급가액 합계", "Supply total", Amount, false),
                    field("tax_total", "세액 합계", "Tax total", Amount, false),
                    field("grand_total", "합계금액", "Grand total", Amount, true),
                    field("payment_terms", "결제조건", "Payment terms", String, false),
                    field("delivery_date", "납기", "Delivery", String, false),
                    field("remarks", "비고", "Remarks", String, false),
                    table("items", "품목", "Items", item_columns(), true),
                ]);
                f
            },
            sections_ko: strings(&["견적 조건", "비고"]),
            sections_en: strings(&["Terms", "Notes"]),
            keywords: strings(&["견적", "견적서", "quotation", "quote", "estimate"]),
        },
        TemplateInfo {
            kind: TemplateKind::TransactionStatement,
            name_ko: "거래명세서".into(),
            name_en: "Transaction statement".into(),
            description_ko: "공급자·공급받는자·품목 표·공급가액/세액/합계.".into(),
            description_en: "Supplier, buyer, item table and supply / tax / grand totals.".into(),
            fields: {
                let mut f = vec![
                    field("document_no", "명세서 번호", "Statement no.", String, false),
                    field("issue_date", "거래일자", "Transaction date", Date, true),
                ];
                f.extend(party("supplier", "공급자", "Supplier", true));
                f.extend(party("buyer", "공급받는자", "Buyer", true));
                f.extend(vec![
                    field("supply_total", "공급가액 합계", "Supply total", Amount, false),
                    field("tax_total", "세액 합계", "Tax total", Amount, false),
                    field("grand_total", "합계금액", "Grand total", Amount, true),
                    field("remarks", "비고", "Remarks", String, false),
                    table("items", "품목", "Items", item_columns(), true),
                ]);
                f
            },
            sections_ko: strings(&["비고"]),
            sections_en: strings(&["Notes"]),
            keywords: strings(&[
                "거래명세",
                "거래명세서",
                "거래명세표",
                "명세서",
                "transaction statement",
                "statement",
            ]),
        },
        TemplateInfo {
            kind: TemplateKind::Contract,
            name_ko: "계약서".into(),
            name_en: "Contract".into(),
            description_ko: "갑·을 당사자, 계약일·기간·금액·지급조건과 조항(제1조…).".into(),
            description_en: "Parties, date, period, amount, payment terms and numbered articles.".into(),
            fields: {
                let mut f = vec![
                    field("title", "계약명", "Title", String, true),
                    field("contract_date", "계약일", "Contract date", Date, true),
                ];
                f.extend(party("party_a", "갑", "Party A", true));
                f.extend(party("party_b", "을", "Party B", true));
                f.extend(vec![
                    field("period_start", "계약기간 시작", "Period start", Date, false),
                    field("period_end", "계약기간 종료", "Period end", Date, false),
                    field("amount", "계약금액", "Contract amount", Amount, false),
                    field("payment_terms", "지급조건", "Payment terms", String, false),
                ]);
                f
            },
            sections_ko: strings(&[
                "제1조 (목적)",
                "제2조 (계약금액)",
                "제3조 (계약기간)",
                "제4조 (대금 지급)",
                "제5조 (당사자의 의무)",
                "제6조 (계약의 해지)",
                "제7조 (분쟁 해결)",
            ]),
            sections_en: strings(&[
                "Article 1 (Purpose)",
                "Article 2 (Contract amount)",
                "Article 3 (Term)",
                "Article 4 (Payment)",
                "Article 5 (Obligations)",
                "Article 6 (Termination)",
                "Article 7 (Disputes)",
            ]),
            keywords: strings(&["계약", "계약서", "contract", "agreement"]),
        },
        TemplateInfo {
            kind: TemplateKind::Report,
            name_ko: "보고서".into(),
            name_en: "Report".into(),
            description_ko: "작성일·부서·작성자와 개요·현황·주요 내용·결론 구성의 업무 보고서.".into(),
            description_en: "Business report: overview, status, details and conclusion.".into(),
            fields: vec![
                field("title", "제목", "Title", String, true),
                field("issue_date", "작성일", "Date", Date, true),
                field("department", "부서", "Department", String, false),
                field("author", "작성자", "Author", String, false),
                field("recipient", "보고 대상", "Addressed to", String, false),
                field("period", "대상 기간", "Period", String, false),
                field("summary", "요약", "Summary", String, false),
            ],
            sections_ko: strings(&["개요", "현황", "주요 내용", "결론 및 건의"]),
            sections_en: strings(&["Overview", "Status", "Details", "Conclusion"]),
            keywords: strings(&["보고서", "보고", "정산서", "결과 보고", "report", "summary report"]),
        },
        TemplateInfo {
            kind: TemplateKind::Minutes,
            name_ko: "회의록".into(),
            name_en: "Meeting minutes".into(),
            description_ko: "일시·장소·참석자와 안건·논의 내용·결정 사항·조치 사항 표.".into(),
            description_en: "Date, place, attendees, agenda, discussion, decisions and action items.".into(),
            fields: vec![
                field("title", "회의명", "Meeting", String, true),
                field("meeting_date", "일시", "Date", Date, true),
                field("location", "장소", "Place", String, false),
                field("attendees", "참석자", "Attendees", String, true),
                field("recorder", "작성자", "Recorded by", String, false),
                table(
                    "action_items",
                    "조치 사항",
                    "Action items",
                    vec![
                        column("item", "항목", "Item", String),
                        column("owner", "담당자", "Owner", String),
                        column("due_date", "기한", "Due", Date),
                    ],
                    false,
                ),
            ],
            sections_ko: strings(&["안건", "논의 내용", "결정 사항", "향후 일정"]),
            sections_en: strings(&["Agenda", "Discussion", "Decisions", "Next steps"]),
            keywords: strings(&["회의록", "회의", "minutes", "meeting minutes", "meeting"]),
        },
    ]
}

/// One built-in template (hidden kinds included).
pub fn get(kind: TemplateKind) -> TemplateInfo {
    list_all().into_iter().find(|t| t.kind == kind).expect("every TemplateKind has a template")
}

/// Lower-cased text without whitespace / punctuation, for keyword and label matching.
pub fn compact(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_whitespace() && !matches!(c, '·' | ',' | '.' | ':' | '：' | '(' | ')' | '/' | '-' | '_' | '*' | '※')
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Visible built-in template whose keyword occurs in `text` (longest keyword wins, so
/// "개발 계약서" beats "계약서"); the hidden generic templates are not matched from free text.
pub fn match_keywords(text: &str) -> Option<TemplateKind> {
    match_in(&list(), text)
}

/// Like [`match_keywords`] but over every template: an explicit format reference ("A4 공문 형식")
/// may still name a hidden generic template.
pub fn match_keywords_any(text: &str) -> Option<TemplateKind> {
    match_in(&list_all(), text)
}

fn match_in(templates: &[TemplateInfo], text: &str) -> Option<TemplateKind> {
    let hay = compact(text);
    if hay.is_empty() {
        return None;
    }
    let mut best: Option<(usize, TemplateKind)> = None;
    for t in templates {
        for k in &t.keywords {
            let kc = compact(k);
            if kc.is_empty() || !hay.contains(&kc) {
                continue;
            }
            if best.is_none_or(|(len, _)| kc.chars().count() > len) {
                best = Some((kc.chars().count(), t.kind));
            }
        }
    }
    best.map(|(_, k)| k)
}

impl TemplateInfo {
    pub fn name(&self, ko: bool) -> &str {
        if ko {
            &self.name_ko
        } else {
            &self.name_en
        }
    }
    pub fn sections(&self, ko: bool) -> &[String] {
        if ko {
            &self.sections_ko
        } else {
            &self.sections_en
        }
    }
    pub fn field(&self, key: &str) -> Option<&TemplateField> {
        self.fields.iter().find(|f| f.key == key)
    }
    pub fn scalars(&self) -> impl Iterator<Item = &TemplateField> {
        self.fields.iter().filter(|f| f.kind != "table")
    }
    pub fn tables(&self) -> impl Iterator<Item = &TemplateField> {
        self.fields.iter().filter(|f| f.kind == "table")
    }
}

impl TemplateField {
    pub fn label(&self, ko: bool) -> &str {
        if ko || self.label_en.is_empty() {
            &self.label_ko
        } else {
            &self.label_en
        }
    }
    pub fn is_table(&self) -> bool {
        self.kind == "table"
    }
}

/// Unified view of the layout source: a built-in template or a registered
/// form / pipeline schema (`SchemaDef` groups become `group.key` paths, tables
/// keep their columns).
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateSpec {
    pub template: TemplateRef,
    pub name_ko: String,
    pub name_en: String,
    /// Document type slug the generated file is registered with.
    pub doc_type: String,
    pub fields: Vec<TemplateField>,
    pub sections_ko: Vec<String>,
    pub sections_en: Vec<String>,
}

impl TemplateSpec {
    pub fn builtin(kind: TemplateKind) -> TemplateSpec {
        let t = get(kind);
        TemplateSpec {
            template: TemplateRef::Builtin { template: kind },
            name_ko: t.name_ko,
            name_en: t.name_en,
            doc_type: kind.as_str().to_string(),
            fields: t.fields,
            sections_ko: t.sections_ko,
            sections_en: t.sections_en,
        }
    }

    /// A registered form (or any pipeline schema) as a template.
    pub fn from_schema(rec: &SchemaRecord) -> TemplateSpec {
        let mut fields = Vec::new();
        for f in &rec.definition.fields {
            match f.kind {
                FieldKind::Table => {
                    let columns = f
                        .fields
                        .iter()
                        .filter(|c| c.kind.is_scalar())
                        .map(|c| DraftColumn {
                            key: c.key.clone(),
                            label: c.label_ko.clone(),
                            kind: c.kind.as_str().into(),
                        })
                        .collect();
                    fields.push(table(&f.key, &f.label_ko, &f.label_en, columns, f.required));
                }
                FieldKind::Group => {
                    for c in f.fields.iter().filter(|c| c.kind.is_scalar()) {
                        let ko = if c.label_ko.contains(&f.label_ko) {
                            c.label_ko.clone()
                        } else {
                            format!("{} {}", f.label_ko, c.label_ko)
                        };
                        let en = if c.label_en.is_empty() {
                            String::new()
                        } else {
                            format!("{} {}", f.label_en, c.label_en)
                        };
                        fields.push(field(&format!("{}.{}", f.key, c.key), &ko, &en, c.kind, c.required));
                    }
                }
                _ => fields.push(field(&f.key, &f.label_ko, &f.label_en, f.kind, f.required)),
            }
        }
        TemplateSpec {
            template: TemplateRef::Form { schema_id: rec.id.clone(), doc_id: rec.form_document_id.clone() },
            name_ko: rec.name.clone(),
            name_en: rec.name_en.clone().unwrap_or_else(|| rec.name.clone()),
            doc_type: rec.doc_type.clone(),
            fields,
            sections_ko: Vec::new(),
            sections_en: Vec::new(),
        }
    }

    pub fn builtin_kind(&self) -> Option<TemplateKind> {
        match &self.template {
            TemplateRef::Builtin { template } => Some(*template),
            TemplateRef::Form { .. } => None,
        }
    }
    pub fn name(&self, ko: bool) -> &str {
        if ko || self.name_en.is_empty() {
            &self.name_ko
        } else {
            &self.name_en
        }
    }
    pub fn sections(&self, ko: bool) -> &[String] {
        if ko {
            &self.sections_ko
        } else {
            &self.sections_en
        }
    }
    pub fn field(&self, key: &str) -> Option<&TemplateField> {
        self.fields.iter().find(|f| f.key == key)
    }
    pub fn scalars(&self) -> impl Iterator<Item = &TemplateField> {
        self.fields.iter().filter(|f| !f.is_table())
    }
    pub fn tables(&self) -> impl Iterator<Item = &TemplateField> {
        self.fields.iter().filter(|f| f.is_table())
    }
}
