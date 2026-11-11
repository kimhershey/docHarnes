//! Project summary report (§3.14.3 "프로젝트 요약") as Markdown and the
//! 보완요청서 draft request (§3.15 "보완 요청 자동 생성"): the selected mismatch /
//! missing findings become `SourceRef::Text` entries in a stable, parseable
//! shape (`title = "finding:<verdict>:<rule_id>"`, `text` = JSON with
//! `rule, verdict, expected, found, doc_name, page, message`) plus one
//! `project` source with the case facts, so `generate::draft` fills the
//! 보완요청서 template deterministically without a model.

use serde_json::json;

use crate::db::Db;
use crate::documents::ReviewStatus;
use crate::error::{CoreError, Result};
use crate::generate::{DraftRequest, Length, OutputFormat, SourceRef, Tone};
use crate::settings::Language;

use super::relations::kind_label;
use super::repo;
use super::{CheckRun, Evidence, ProjectDetail, RuleResult, Verdict};

fn tr<'a>(ko: bool, k: &'a str, e: &'a str) -> &'a str {
    if ko {
        k
    } else {
        e
    }
}

/// Verdict label for tables and cards.
pub fn verdict_label(v: Verdict, ko: bool) -> &'static str {
    match (v, ko) {
        (Verdict::Pass, true) => "통과",
        (Verdict::Pass, false) => "Pass",
        (Verdict::Mismatch, true) => "불일치",
        (Verdict::Mismatch, false) => "Mismatch",
        (Verdict::Undetermined, true) => "판단불가",
        (Verdict::Undetermined, false) => "Undetermined",
        (Verdict::Missing, true) => "누락",
        (Verdict::Missing, false) => "Missing",
    }
}

fn review_label(s: ReviewStatus, ko: bool) -> &'static str {
    match (s, ko) {
        (ReviewStatus::None, _) => "-",
        (ReviewStatus::Pending, true) => "검토 대기",
        (ReviewStatus::Pending, false) => "pending",
        (ReviewStatus::Done, true) => "완료",
        (ReviewStatus::Done, false) => "done",
    }
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn dash(s: Option<&str>) -> String {
    s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| "-".into())
}

/// `doc_name label value (p.N)` for an evidence cell; literals show the value only.
pub fn evidence_text(e: Option<&Evidence>, ko: bool) -> String {
    let Some(e) = e else { return "-".into() };
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = &e.doc_name {
        parts.push(d.clone());
    }
    if let Some(l) = e.label.clone().or(e.field.clone()) {
        parts.push(l);
    }
    match e.normalized.clone().or(e.value.clone()) {
        Some(v) => parts.push(v),
        None if e.doc_id.is_some() || e.doc_type.is_some() => parts.push(tr(ko, "(값 없음)", "(no value)").into()),
        None => {}
    }
    if let Some(p) = e.page {
        parts.push(format!("(p.{p})"));
    }
    match e.status.as_deref() {
        Some("pending") => parts.push(tr(ko, "[검토 대기]", "[pending]").into()),
        Some("rejected") => parts.push(tr(ko, "[거부]", "[rejected]").into()),
        Some("not_extracted") => parts.push(tr(ko, "[추출 전]", "[not extracted]").into()),
        Some("missing_document") => parts.push(tr(ko, "[문서 없음]", "[no document]").into()),
        _ => {}
    }
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join(" ")
    }
}

/// Renders the report from a loaded detail (pure).
pub fn render(d: &ProjectDetail, language: Language) -> String {
    let ko = language == Language::Ko;
    let p = &d.project;
    let mut s = String::new();
    s.push_str(&format!("# {}\n\n", p.name));
    let template = d.template.as_ref().map(|t| if ko { t.name_ko.clone() } else { t.name_en.clone() });
    let phase = if p.overdue {
        format!("{} ({})", p.phase.label(ko), tr(ko, "기한 경과", "overdue"))
    } else {
        p.phase.label(ko).to_string()
    };
    s.push_str(&format!(
        "- {}: {} · {}: {} · {}: {} · {}: {} · {}: {} · {}: {}\n",
        tr(ko, "상태", "Status"),
        phase,
        tr(ko, "템플릿", "Template"),
        dash(template.as_deref()),
        tr(ko, "접수번호", "Intake no."),
        dash(p.intake_no.as_deref()),
        tr(ko, "담당", "Owner"),
        dash(p.owner.as_deref()),
        tr(ko, "시작", "Start"),
        dash(p.start_date.as_deref()),
        tr(ko, "종료", "End"),
        dash(p.end_date.as_deref()),
    ));
    s.push_str(&format!(
        "- {}: {} · {}: {} · {}: {}\n",
        tr(ko, "문서", "Documents"),
        p.document_count,
        tr(ko, "누락", "Missing"),
        p.missing_count,
        tr(ko, "최근 검사", "Last check"),
        p.last_check_at
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| tr(ko, "없음", "none").into()),
    ));
    if let Some(n) = p.notes.as_deref().filter(|n| !n.trim().is_empty()) {
        s.push_str(&format!("- {}: {}\n", tr(ko, "메모", "Notes"), n.trim()));
    }

    // Documents
    s.push_str(&format!("\n## {}\n\n", tr(ko, "문서", "Documents")));
    if d.documents.is_empty() {
        s.push_str(tr(ko, "문서가 없습니다.\n", "No documents.\n"));
    } else {
        s.push_str(&format!(
            "| {} | {} | {} | {} |\n|---|---|---|---|\n",
            tr(ko, "유형", "Type"),
            tr(ko, "문서", "Document"),
            tr(ko, "날짜", "Date"),
            tr(ko, "검토", "Review")
        ));
        for pd in &d.documents {
            let doc = &pd.document;
            let type_label = doc
                .doc_type
                .as_deref()
                .map(|t| {
                    d.checklist
                        .iter()
                        .find(|c| c.doc_type == t)
                        .map(|c| if ko { c.label_ko.clone() } else { c.label_en.clone() })
                        .unwrap_or_else(|| t.to_string())
                })
                .unwrap_or_else(|| "-".into());
            let name = doc.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| doc.original_name.clone());
            s.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                cell(&type_label),
                cell(&name),
                dash(doc.doc_date.as_deref()),
                review_label(doc.review_status, ko)
            ));
        }
    }

    // Checklist
    s.push_str(&format!("\n## {}\n\n", tr(ko, "체크리스트", "Checklist")));
    if d.checklist.is_empty() {
        s.push_str(tr(
            ko,
            "템플릿이 없어 필수 문서 목록이 없습니다.\n",
            "No template, so no required document list.\n",
        ));
    }
    for c in &d.checklist {
        let label = if ko { &c.label_ko } else { &c.label_en };
        let mark = if c.missing { "[ ]" } else { "[x]" };
        let count = c.doc_ids.len();
        let note = if c.missing {
            tr(ko, " — 누락", " — missing").to_string()
        } else if c.required {
            tr(ko, &format!(" ({count}건)"), &format!(" ({count})")).to_string()
        } else {
            tr(ko, &format!(" ({count}건, 추가 문서)"), &format!(" ({count}, extra)")).to_string()
        };
        s.push_str(&format!("- {mark} {label}{note}\n"));
    }

    // Rule results
    s.push_str(&format!("\n## {}\n\n", tr(ko, "정합성 검사", "Consistency check")));
    match &d.last_run {
        None => s.push_str(tr(ko, "검사를 아직 실행하지 않았습니다.\n", "No check has been run yet.\n")),
        Some(run) => {
            s.push_str(&format!(
                "{} {} — {} {} · {} {} · {} {} · {} {}\n\n",
                tr(ko, "최근 검사", "Last check"),
                run.finished_at.format("%Y-%m-%d %H:%M"),
                tr(ko, "통과", "pass"),
                run.pass,
                tr(ko, "불일치", "mismatch"),
                run.mismatch,
                tr(ko, "판단불가", "undetermined"),
                run.undetermined,
                tr(ko, "누락", "missing"),
                run.missing
            ));
            if !run.missing_doc_types.is_empty() {
                let names: Vec<String> = run
                    .missing_doc_types
                    .iter()
                    .map(|t| {
                        d.checklist
                            .iter()
                            .find(|c| &c.doc_type == t)
                            .map(|c| if ko { c.label_ko.clone() } else { c.label_en.clone() })
                            .unwrap_or_else(|| t.clone())
                    })
                    .collect();
                s.push_str(&format!("{}: {}\n\n", tr(ko, "누락 문서", "Missing documents"), names.join(", ")));
            }
            if run.results.is_empty() {
                s.push_str(tr(ko, "실행된 규칙이 없습니다.\n", "No rules were run.\n"));
            } else {
                s.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n|---|---|---|---|---|\n",
                    tr(ko, "규칙", "Rule"),
                    tr(ko, "결과", "Verdict"),
                    tr(ko, "좌변", "Left"),
                    tr(ko, "우변", "Right"),
                    tr(ko, "설명", "Message")
                ));
                for r in &run.results {
                    let mut verdict = verdict_label(r.verdict, ko).to_string();
                    if !r.required && r.verdict != Verdict::Pass {
                        verdict.push_str(tr(ko, " (선택)", " (optional)"));
                    }
                    s.push_str(&format!(
                        "| {} | {} | {} | {} | {} |\n",
                        cell(&r.description),
                        verdict,
                        cell(&evidence_text(r.left.as_ref(), ko)),
                        cell(&evidence_text(r.right.as_ref(), ko)),
                        cell(&r.message)
                    ));
                }
            }
        }
    }

    // Relations
    s.push_str(&format!("\n## {}\n\n", tr(ko, "문서 간 관계", "Relations between documents")));
    if d.relations.edges.is_empty() {
        s.push_str(tr(ko, "공유 엔터티가 없습니다.\n", "No shared entities.\n"));
    } else {
        let label = |id: &str| {
            d.relations.nodes.iter().find(|n| n.doc_id == id).map(|n| n.label.clone()).unwrap_or_else(|| id.to_string())
        };
        for e in &d.relations.edges {
            s.push_str(&format!("- {} ↔ {}: {} {}\n", label(&e.from), label(&e.to), kind_label(&e.kind, ko), e.value));
        }
    }

    // Next steps
    s.push_str(&format!("\n## {}\n\n", tr(ko, "다음 단계", "Next steps")));
    let missing: Vec<String> = d
        .checklist
        .iter()
        .filter(|c| c.missing)
        .map(|c| if ko { c.label_ko.clone() } else { c.label_en.clone() })
        .collect();
    let mut steps: Vec<String> = Vec::new();
    if !missing.is_empty() {
        steps.push(
            tr(
                ko,
                &format!("누락 문서 {}건({}) 보완 요청", missing.len(), missing.join(", ")),
                &format!("Request the {} missing document(s): {}", missing.len(), missing.join(", ")),
            )
            .to_string(),
        );
    }
    match &d.last_run {
        None => steps.push(tr(ko, "정합성 검사 실행", "Run the consistency check").into()),
        Some(run) => {
            if run.mismatch > 0 {
                steps.push(
                    tr(
                        ko,
                        &format!("불일치 {}건 확인 후 보완요청서 생성", run.mismatch),
                        &format!("Review {} mismatch(es) and draft the supplement request", run.mismatch),
                    )
                    .to_string(),
                );
            }
            // Rules that could not be judged because a document has not been extracted yet need the
            // extraction, not the review queue.
            let not_extracted = |e: &Option<Evidence>| e.as_ref().is_some_and(|e| e.status.as_deref() == Some("not_extracted"));
            let pending_extraction = run
                .results
                .iter()
                .filter(|r| r.verdict == Verdict::Undetermined && (not_extracted(&r.left) || not_extracted(&r.right)))
                .count() as u32;
            if pending_extraction > 0 {
                steps.push(
                    tr(
                        ko,
                        &format!("추출 전 문서가 걸린 규칙 {pending_extraction}건: 추출 실행 후 재검사"),
                        &format!("{pending_extraction} rule(s) wait for a document that is not extracted yet: run the extraction and re-run"),
                    )
                    .to_string(),
                );
            }
            let undetermined = run.undetermined.saturating_sub(pending_extraction);
            if undetermined > 0 {
                steps.push(
                    tr(
                        ko,
                        &format!("판단불가 {undetermined}건: 검토 큐 처리 후 재검사"),
                        &format!("{undetermined} undetermined: clear the review queue and re-run"),
                    )
                    .to_string(),
                );
            }
            if run.mismatch == 0 && run.undetermined == 0 && run.missing == 0 && missing.is_empty() {
                steps.push(
                    tr(ko, "모든 규칙 통과 — 완료 처리 가능", "All rules pass — the project can be completed").into(),
                );
            }
        }
    }
    for st in steps {
        s.push_str(&format!("- {st}\n"));
    }
    s
}

/// Markdown summary report of a project.
pub fn markdown(db: &Db, project_id: &str, language: Language) -> Result<String> {
    let d = repo::detail(db, project_id, language)?;
    Ok(render(&d, language))
}

// ---------------------------------------------------------------------------
// 보완요청서 request
// ---------------------------------------------------------------------------

/// One finding of the request (a mismatch / missing rule result or a missing document type).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Finding {
    pub id: String,
    pub rule: String,
    pub verdict: String,
    pub expected: String,
    pub found: String,
    pub doc_name: Option<String>,
    pub page: Option<u32>,
    pub message: String,
}

impl Finding {
    /// `SourceRef::Text` title: `finding:<verdict>:<rule_id>`.
    pub fn title(&self) -> String {
        format!("finding:{}:{}", self.verdict, self.id)
    }

    pub fn source(&self) -> SourceRef {
        let text = json!({
            "rule": self.rule,
            "verdict": self.verdict,
            "expected": self.expected,
            "found": self.found,
            "doc_name": self.doc_name,
            "page": self.page,
            "message": self.message,
        })
        .to_string();
        SourceRef::Text { title: Some(self.title()), text }
    }
}

fn summary(e: Option<&Evidence>, ko: bool) -> String {
    let Some(e) = e else { return tr(ko, "없음", "none").into() };
    let value = e.normalized.clone().or(e.value.clone());
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = &e.doc_name {
        parts.push(d.clone());
    }
    if let Some(l) = e.label.clone().or(e.field.clone()) {
        parts.push(l);
    }
    match value {
        Some(v) => parts.push(v),
        None => parts.push(tr(ko, "없음", "none").into()),
    }
    parts.join(" ")
}

/// Finding of a mismatch / missing rule result.
pub fn finding_from_result(r: &RuleResult, labels: &[(String, String, String)], ko: bool) -> Finding {
    let (expected, found) = match r.verdict {
        Verdict::Missing => {
            let slug = r.left.as_ref().and_then(|e| e.doc_type.clone()).unwrap_or_default();
            let label = repo::type_label(labels, &slug, ko);
            (
                tr(ko, &format!("{label} 문서 제출"), &format!("{label} document")).to_string(),
                tr(ko, "없음", "missing").to_string(),
            )
        }
        _ => match &r.right {
            Some(right) if right.doc_id.is_none() && right.doc_type.is_none() => {
                // Literal / pattern on the right.
                let v = right.normalized.clone().or(right.value.clone()).unwrap_or_default();
                (v, summary(r.left.as_ref(), ko))
            }
            Some(right) => (summary(Some(right), ko), summary(r.left.as_ref(), ko)),
            None => {
                let label = r.left.as_ref().and_then(|e| e.label.clone().or(e.field.clone())).unwrap_or_default();
                (
                    tr(ko, &format!("{label} 기재"), &format!("{label} filled in")).to_string(),
                    tr(ko, "없음", "missing").to_string(),
                )
            }
        },
    };
    let doc = r.left.as_ref().filter(|e| e.doc_id.is_some()).or(r.right.as_ref().filter(|e| e.doc_id.is_some()));
    Finding {
        id: r.id.clone(),
        rule: r.description.clone(),
        verdict: r.verdict.as_str().to_string(),
        expected,
        found,
        doc_name: doc.and_then(|e| e.doc_name.clone()),
        page: doc.and_then(|e| e.page),
        message: r.message.clone(),
    }
}

/// Finding id of a missing document type (`finding_ids` entry).
pub fn missing_doc_finding_id(doc_type: &str) -> String {
    format!("missing_doc:{doc_type}")
}

/// Builds the 보완요청서 draft request from a check run (`finding_ids` empty = every mismatch
/// / missing finding). Fails with `no_findings` when nothing is selected.
pub fn supplement_request(
    db: &Db,
    project_id: &str,
    run: &CheckRun,
    finding_ids: &[String],
    output: OutputFormat,
    language: Language,
) -> Result<DraftRequest> {
    let ko = language == Language::Ko;
    let loaded = db.with(|c| {
        let Some(project) = repo::get_project(c, project_id)? else { return Ok(None) };
        let template = match project.template_id.as_deref() {
            Some(t) => repo::get_template(c, t)?,
            None => None,
        };
        Ok(Some((project, template, repo::documents_of(c, project_id)?, repo::doc_type_labels(c)?)))
    })?;
    let Some((project, template, documents, labels)) = loaded else {
        return Err(CoreError::NotFound(format!("project {project_id}")));
    };
    let want = |id: &str| finding_ids.is_empty() || finding_ids.iter().any(|f| f == id);
    let mut findings: Vec<Finding> = Vec::new();
    for t in &run.missing_doc_types {
        let id = missing_doc_finding_id(t);
        if !want(&id) {
            continue;
        }
        let label = repo::type_label(&labels, t, ko);
        findings.push(Finding {
            id,
            rule: tr(ko, &format!("{label} 문서 필수"), &format!("{label} document required")).to_string(),
            verdict: Verdict::Missing.as_str().to_string(),
            expected: tr(ko, &format!("{label} 문서 제출"), &format!("{label} document")).to_string(),
            found: tr(ko, "없음", "missing").to_string(),
            doc_name: None,
            page: None,
            message: tr(
                ko,
                &format!("프로젝트에 {label} 문서가 없습니다."),
                &format!("No {label} document in the project."),
            )
            .to_string(),
        });
    }
    for r in &run.results {
        if !matches!(r.verdict, Verdict::Mismatch | Verdict::Missing) || !want(&r.id) {
            continue;
        }
        // A rule that only failed because a required document is absent is covered by the
        // missing-document finding when the selection is "everything".
        if finding_ids.is_empty() && r.verdict == Verdict::Missing {
            let slug = r.left.as_ref().and_then(|e| e.doc_type.clone()).unwrap_or_default();
            if run.missing_doc_types.contains(&slug) {
                continue;
            }
        }
        findings.push(finding_from_result(r, &labels, ko));
    }
    if findings.is_empty() {
        return Err(CoreError::Invalid("no_findings".into()));
    }
    let mismatches = findings.iter().filter(|f| f.verdict == "mismatch").count();
    let missing = findings.len() - mismatches;

    let mut sources: Vec<SourceRef> = findings.iter().map(Finding::source).collect();
    let docs: Vec<serde_json::Value> = documents
        .iter()
        .map(|d| {
            let doc = &d.document;
            json!({
                "id": doc.id,
                "name": doc.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| doc.original_name.clone()),
                "doc_type": doc.doc_type,
                "doc_type_label": doc.doc_type.as_deref().map(|t| repo::type_label(&labels, t, ko)),
                "date": doc.doc_date,
                "review_status": doc.review_status.as_str(),
            })
        })
        .collect();
    sources.push(SourceRef::Text {
        title: Some("project".into()),
        text: json!({
            "id": project.id,
            "name": project.name,
            "phase": project.phase.as_str(),
            "phase_label": project.phase.label(ko),
            "overdue": project.overdue,
            "intake_no": project.intake_no,
            "owner": project.owner,
            "start_date": project.start_date,
            "end_date": project.end_date,
            "template": template.as_ref().map(|t| if ko { t.name_ko.clone() } else { t.name_en.clone() }),
            "document_count": documents.len(),
            "documents": docs,
            "checked_at": run.finished_at.to_rfc3339(),
            "counts": { "pass": run.pass, "mismatch": run.mismatch, "undetermined": run.undetermined, "missing": run.missing },
            "findings": { "mismatch": mismatches, "missing": missing },
        })
        .to_string(),
    });

    let intake = project
        .intake_no
        .as_deref()
        .map(|n| tr(ko, &format!("(접수번호 {n}) "), &format!("(intake no. {n}) ")).to_string())
        .unwrap_or_default();
    let due = project
        .end_date
        .as_deref()
        .map(|d| tr(ko, &format!(" 프로젝트 종료: {d}."), &format!(" Project end: {d}.")).to_string())
        .unwrap_or_default();
    let owner = project
        .owner
        .as_deref()
        .map(|o| tr(ko, &format!(" 담당: {o}."), &format!(" Owner: {o}.")).to_string())
        .unwrap_or_default();
    let content = if ko {
        format!(
            "프로젝트 '{}' {intake}정합성 검사 결과 불일치 {mismatches}건, 누락 {missing}건에 대한 보완을 요청하는 보완요청서. 각 항목의 요구 사항과 확인된 내용, 근거 문서를 표로 정리하고 제출 방법을 안내합니다.{due}{owner}",
            project.name
        )
    } else {
        format!(
            "Supplement request for project '{}' {intake}after the consistency check: {mismatches} mismatch(es) and {missing} missing item(s). List each item with the requirement, what was found and the evidence document, and explain how to submit.{due}{owner}",
            project.name
        )
    };
    Ok(DraftRequest {
        form_ref: Some("supplement_request".into()),
        content,
        sources,
        output,
        tone: Tone::Formal,
        length: Length::Medium,
        language,
        use_knowledge: false,
        ..Default::default()
    })
}
