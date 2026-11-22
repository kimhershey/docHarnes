//! The `summarize` step: one summary of the content of all the documents in scope — or, with a
//! `group_by`, one per project / document type / folder.
//!
//! User decision 2026-11-22 (1.0.3): "파일별 요약은 없애고 모든 파일의 내용을 하나의 결과로 요약". Until
//! then the step wrote a summary of every document (`target = each_doc`, the default since the first
//! design) and, for `target = set` or a `group_by`, a combined summary made from those summaries. The
//! per-document summaries are gone: the passages of all the documents of a group are read and
//! summarised as one text with its sources ([`answer::summarize_documents`]), and the result holds
//! that summary only. Grouping is the executor's job: the model only names the criterion.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::documents::DocumentRecord;
use crate::error::{CoreError, Result};
use crate::knowledge::answer::{self, AnswerDeps};
use crate::knowledge::search::SearchDeps;
use crate::knowledge::{AnswerKind, AnswerRequest, SearchScope, SummaryMode};
use crate::projects::repo as projects_repo;

use super::super::{Plan, SummaryGroup, SummaryLength};
use super::{count_label, no_model, working_set, ExecDeps, LeaseGuard, StepOutcome};

/// The documents of one summary (`docs` = indices into the working set).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Group {
    pub key: String,
    pub label: String,
    pub docs: Vec<usize>,
}

/// The parts of a document the grouping looks at.
#[derive(Debug, Clone, Copy)]
pub(super) struct DocKey<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub rel_path: &'a str,
    pub doc_type: Option<&'a str>,
}

impl<'a> DocKey<'a> {
    fn of(d: &'a DocumentRecord) -> DocKey<'a> {
        DocKey { id: &d.id, name: &d.original_name, rel_path: &d.rel_path, doc_type: d.doc_type.as_deref() }
    }
}

/// What `group_documents` needs to know about projects and document types.
#[derive(Debug, Default)]
pub(super) struct GroupInfo {
    /// `(project id, name)` in display order.
    pub projects: Vec<(String, String)>,
    /// document id → project ids.
    pub memberships: BTreeMap<String, Vec<String>>,
    /// `(slug, label_ko, label_en)`.
    pub type_labels: Vec<(String, String, String)>,
}

/// Splits the working set into the groups that get a summary each. Without a criterion that is one
/// group of everything — named after the document when there is only one.
pub(super) fn group_documents(
    docs: &[DocKey<'_>],
    group_by: Option<SummaryGroup>,
    info: &GroupInfo,
    ko: bool,
) -> Vec<Group> {
    let t = |k: &'static str, e: &'static str| if ko { k } else { e };
    match group_by.unwrap_or(SummaryGroup::None) {
        SummaryGroup::None => {
            let label = match docs {
                [only] => only.name.to_string(),
                _ => t("전체", "All documents").to_string(),
            };
            vec![Group { key: "all".into(), label, docs: (0..docs.len()).collect() }]
        }
        SummaryGroup::Project => {
            let mut groups: Vec<Group> = info
                .projects
                .iter()
                .map(|(id, name)| Group { key: id.clone(), label: name.clone(), docs: Vec::new() })
                .collect();
            let mut none =
                Group { key: String::new(), label: t("프로젝트 없음", "No project").into(), docs: Vec::new() };
            for (i, d) in docs.iter().enumerate() {
                let mut placed = false;
                for pid in info.memberships.get(d.id).map(Vec::as_slice).unwrap_or(&[]) {
                    if let Some(g) = groups.iter_mut().find(|g| &g.key == pid) {
                        g.docs.push(i);
                        placed = true;
                    }
                }
                if !placed {
                    none.docs.push(i);
                }
            }
            groups.push(none);
            groups.retain(|g| !g.docs.is_empty());
            groups
        }
        SummaryGroup::DocType => {
            let mut groups: Vec<Group> = Vec::new();
            for (i, d) in docs.iter().enumerate() {
                let slug = d.doc_type.unwrap_or_default().to_string();
                match groups.iter_mut().find(|g| g.key == slug) {
                    Some(g) => g.docs.push(i),
                    None => {
                        let label = if slug.is_empty() {
                            t("유형 없음", "No type").to_string()
                        } else {
                            projects_repo::type_label(&info.type_labels, &slug, ko)
                        };
                        groups.push(Group { key: slug, label, docs: vec![i] });
                    }
                }
            }
            groups
        }
        SummaryGroup::Folder => {
            let mut groups: Vec<Group> = Vec::new();
            for (i, d) in docs.iter().enumerate() {
                let folder = d.rel_path.rsplit_once('/').map(|(dir, _)| dir.to_string()).unwrap_or_default();
                match groups.iter_mut().find(|g| g.key == folder) {
                    Some(g) => g.docs.push(i),
                    None => {
                        let label = if folder.is_empty() {
                            t("(최상위)", "(top level)").to_string()
                        } else {
                            folder.clone()
                        };
                        groups.push(Group { key: folder, label, docs: vec![i] });
                    }
                }
            }
            groups
        }
    }
}

/// 1.0.2 — "AA에 대해서 요약해줘": what the documents in scope say about `topic`, as one summary with
/// its sources. The scope works as it does for a search — none is every document, a project also
/// sees its reference material and the common one — and a scope that matches no document is told,
/// not widened.
///
/// User report 2026-11-19: the step summarised every document in scope ("요약만 하고 AA에 대한 내용을
/// 수집하지 않음"); a summary had no way to say what it is about.
pub(super) fn run_topic(
    deps: &ExecDeps<'_>,
    plan: &Plan,
    imported: Option<&[String]>,
    topic: &str,
    length: Option<SummaryLength>,
    progress: &mut dyn FnMut(&str, u32, u32, &str),
) -> Result<StepOutcome> {
    let db = deps.db();
    let chat = deps.chat.ok_or_else(|| no_model(deps))?;
    let _lease = LeaseGuard::acquire(deps.lease);
    // "이 프로젝트의 참고 자료에서 …": the search scope itself adds the project's reference group and
    // the common reference material (`project_id` + `roles`), as for `search`.
    let reference_of_project = super::project_reference_only(plan, imported);
    let narrowed = super::scope_narrows(plan, imported) && !reference_of_project;
    let docs = if (plan.scope.is_empty() && imported.is_none()) || reference_of_project {
        Vec::new()
    } else {
        working_set(deps, plan, imported)?
    };
    if narrowed && docs.is_empty() {
        // (an empty document list would mean "everywhere" to the search)
        return Err(super::no_documents(plan));
    }
    deps.checkpoint()?;
    progress("summarize", 0, 1, topic);
    let mut req = AnswerRequest::question(topic);
    req.kind = AnswerKind::Summary;
    req.summary_mode = match length {
        Some(SummaryLength::Short) => SummaryMode::OneLine,
        Some(SummaryLength::Long) => SummaryMode::OnePage,
        None => SummaryMode::KeyPoints,
    };
    req.context_tokens = deps.knowledge.context_tokens;
    req.top_k = deps.knowledge.top_k;
    req.min_similarity = Some(deps.knowledge.min_vector_similarity);
    req.scope = SearchScope {
        documents: super::ids(&docs),
        project_id: plan.scope.filter.project_id.clone(),
        roles: plan.scope.filter.role.map(|r| r.as_role()).into_iter().collect(),
        latest_only: deps.knowledge.latest_only,
        ..SearchScope::default()
    };
    let cancelled = || deps.cancelled();
    let adeps = AnswerDeps {
        search: SearchDeps { db, analyzer: deps.analyzer, embedder: deps.embedder },
        chat,
        model: deps.model.clone(),
        cancelled: Some(&cancelled),
        context_size: deps.ctx_size,
    };
    let ans = answer::summarize_topic(&adeps, &req, &mut |_| !deps.cancelled())?;
    // The documents the summary draws on, in the order their passages are cited.
    let mut cited: Vec<&str> = Vec::new();
    for s in &ans.sources {
        if !cited.contains(&s.document_id.as_str()) {
            cited.push(&s.document_id);
        }
    }
    let summary = match (ans.no_evidence, deps.ko()) {
        (true, true) => format!("'{topic}'에 관한 내용을 범위의 문서에서 찾지 못했습니다"),
        (true, false) => format!("Nothing about '{topic}' was found in the documents in scope"),
        (false, true) => format!("'{topic}'에 관한 내용 요약 · 문서 {}건의 대목 {}곳", cited.len(), ans.sources.len()),
        (false, false) => {
            format!("Summary about '{topic}' · {} passage(s) of {} document(s)", ans.sources.len(), cited.len())
        }
    };
    let label = if deps.ko() { format!("'{topic}'에 관한 내용") } else { format!("About '{topic}'") };
    let payload = json!({
        "topic": topic,
        "summaries": [],
        "groups": [{ "key": "topic", "label": label, "document_ids": cited, "answer": ans }],
        "group_by": SummaryGroup::None,
    });
    Ok(StepOutcome { summary, payload, imported: None })
}

pub(super) fn run(
    deps: &ExecDeps<'_>,
    plan: &Plan,
    imported: Option<&[String]>,
    length: Option<SummaryLength>,
    group_by: Option<SummaryGroup>,
    progress: &mut dyn FnMut(&str, u32, u32, &str),
) -> Result<StepOutcome> {
    let db = deps.db();
    let chat = deps.chat.ok_or_else(|| no_model(deps))?;
    let _lease = LeaseGuard::acquire(deps.lease);
    let docs = working_set(deps, plan, imported)?;
    if docs.is_empty() {
        // (which kind of document the scope asked for and does not hold, when it named one)
        return Err(super::no_documents(plan));
    }
    let mode = match length {
        Some(SummaryLength::Short) => SummaryMode::OneLine,
        Some(SummaryLength::Long) => SummaryMode::OnePage,
        None => SummaryMode::KeyPoints,
    };
    let info = match group_by {
        Some(SummaryGroup::Project) => {
            let ids: Vec<String> = docs.iter().map(|d| d.id.clone()).collect();
            db.with(|c| {
                Ok(GroupInfo {
                    projects: projects_repo::list_projects(c)?.into_iter().map(|p| (p.id, p.name)).collect(),
                    memberships: projects_repo::memberships_with_reference(c, &ids)?,
                    type_labels: Vec::new(),
                })
            })?
        }
        Some(SummaryGroup::DocType) => {
            GroupInfo { type_labels: db.with(projects_repo::doc_type_labels)?, ..GroupInfo::default() }
        }
        _ => GroupInfo::default(),
    };
    let keys: Vec<DocKey<'_>> = docs.iter().map(DocKey::of).collect();
    let groups = group_documents(&keys, group_by, &info, deps.ko());

    let cancelled = || deps.cancelled();
    let adeps = AnswerDeps {
        search: SearchDeps { db, analyzer: deps.analyzer, embedder: deps.embedder },
        chat,
        model: deps.model.clone(),
        cancelled: Some(&cancelled),
        context_size: deps.ctx_size,
    };
    // One summary per group, made from the passages of all its documents.
    //
    // A document without text (not processed yet, an image without OCR) has nothing to summarise: it
    // is left out and named in the result. A group the model could not summarise does not throw away
    // the summaries of the groups before it (1.0.1): its documents are listed under `failed`. What
    // stops every group alike — no engine, no model, a cancelled run — still ends the step.
    let mut group_values: Vec<Value> = Vec::new();
    let mut covered: Vec<usize> = Vec::new();
    let mut without_text: Vec<usize> = Vec::new();
    let mut failures: Vec<(usize, String)> = Vec::new();
    let mut first_error: Option<CoreError> = None;
    // Progress counts model calls: the ones made, and the ones still expected (a group not started
    // counts as one until its passages are laid out).
    let mut calls = 0u32;
    for (n, g) in groups.iter().enumerate() {
        deps.checkpoint()?;
        let waiting = (groups.len() - n - 1) as u32;
        progress("summarize", calls, calls + 1 + waiting, &g.label);
        let ids: Vec<String> = g.docs.iter().map(|&i| docs[i].id.clone()).collect();
        let mut req = AnswerRequest::question("");
        req.kind = AnswerKind::Summary;
        req.summary_mode = mode;
        req.context_tokens = deps.knowledge.context_tokens;
        let mut made_here = 0u32;
        let result =
            answer::summarize_documents(&adeps, &req, &ids, &mut |_| !deps.cancelled(), &mut |made, expected| {
                made_here = made;
                progress("summarize", calls + made, calls + expected.max(made) + waiting, &g.label);
            });
        calls += made_here.max(1);
        match result {
            Ok((answer, no_text)) => {
                let mut members: Vec<&str> = Vec::new();
                for &i in &g.docs {
                    if no_text.contains(&docs[i].id) {
                        if !without_text.contains(&i) {
                            without_text.push(i);
                        }
                        continue;
                    }
                    members.push(docs[i].id.as_str());
                    if !covered.contains(&i) {
                        covered.push(i);
                    }
                }
                group_values.push(json!({ "key": g.key, "label": g.label, "document_ids": members, "answer": answer }));
            }
            Err(CoreError::NotFound(reason)) if reason == "no_text" => {
                for &i in &g.docs {
                    if !without_text.contains(&i) {
                        without_text.push(i);
                    }
                }
            }
            Err(
                e @ (CoreError::Cancelled
                | CoreError::EngineNotFound(_)
                | CoreError::UnknownModel(_)
                | CoreError::NoWorkspace
                | CoreError::Db(_)),
            ) => return Err(e),
            // (the engine is gone: the next group would fail the same way within seconds)
            Err(e) if crate::pipeline::vlm::engine_unavailable(&e) => return Err(e),
            Err(e) => {
                log::warn!("summary of '{}' failed: {e}", g.label);
                for &i in &g.docs {
                    if !failures.iter().any(|(f, _)| *f == i) {
                        failures.push((i, e.to_string()));
                    }
                }
                first_error.get_or_insert(e);
            }
        }
    }
    if group_values.is_empty() {
        // Nothing was summarised: the reason of the first group that failed, else "no text".
        return Err(first_error.unwrap_or_else(|| CoreError::NotFound("no_text".into())));
    }
    without_text.sort_unstable();
    let skipped: Vec<Value> = without_text
        .iter()
        .map(|&i| json!({ "document_id": docs[i].id, "name": docs[i].original_name, "reason": "no_text" }))
        .collect();
    // (a document that another group's summary covers is not one that could not be summarised)
    failures.retain(|(i, _)| !covered.contains(i));
    failures.sort_by_key(|(i, _)| *i);
    let failed: Vec<Value> = failures
        .iter()
        .map(|(i, e)| json!({ "document_id": docs[*i].id, "name": docs[*i].original_name, "error": e }))
        .collect();

    let criterion = match group_by.unwrap_or(SummaryGroup::None) {
        SummaryGroup::Project => Some(deps.t("프로젝트별", "by project")),
        SummaryGroup::DocType => Some(deps.t("유형별", "by type")),
        SummaryGroup::Folder => Some(deps.t("폴더별", "by folder")),
        SummaryGroup::None => None,
    };
    let made = covered.len();
    let made_groups = group_values.len();
    let mut summary = match criterion {
        Some(by) if deps.ko() => format!("{} 요약 · {by} 묶음 {made_groups}개", count_label(deps, made)),
        Some(by) => format!("Summarised {} · {made_groups} group(s) {by}", count_label(deps, made)),
        None if made == 1 && deps.ko() => format!("{} 요약", count_label(deps, made)),
        None if made == 1 => format!("Summarised {}", count_label(deps, made)),
        None if deps.ko() => format!("{}의 내용을 하나로 요약", count_label(deps, made)),
        None => format!("One summary of {}", count_label(deps, made)),
    };
    if !skipped.is_empty() {
        summary.push_str(&if deps.ko() {
            format!(" · 본문이 없는 문서 {}건은 건너뜀", skipped.len())
        } else {
            format!(" · {} document(s) without text skipped", skipped.len())
        });
    }
    if !failed.is_empty() {
        summary.push_str(&if deps.ko() {
            format!(" · 요약하지 못한 문서 {}건", failed.len())
        } else {
            format!(" · {} document(s) could not be summarised", failed.len())
        });
    }
    // (`summaries` — the per-document summaries of the versions before 1.0.3 — stays as an empty
    // list: it is how a reader of the payload tells a summary step's result.)
    let mut payload = json!({
        "summaries": [],
        "groups": group_values,
        "group_by": group_by.unwrap_or(SummaryGroup::None),
    });
    if !skipped.is_empty() {
        payload["skipped"] = Value::Array(skipped);
    }
    if !failed.is_empty() {
        payload["failed"] = Value::Array(failed);
    }
    Ok(StepOutcome { summary, payload, imported: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs() -> Vec<DocKey<'static>> {
        vec![
            DocKey {
                id: "a",
                name: "계약서.pdf",
                rel_path: "documents/한빛/계약서.pdf",
                doc_type: Some("dev_contract"),
            },
            DocKey {
                id: "b", name: "제안서.pdf", rel_path: "documents/한빛/제안서.pdf", doc_type: Some("proposal")
            },
            DocKey {
                id: "c",
                name: "계약서.pdf",
                rel_path: "documents/새한/계약서.pdf",
                doc_type: Some("dev_contract"),
            },
            DocKey { id: "d", name: "메모.txt", rel_path: "메모.txt", doc_type: None },
        ]
    }

    /// 1.0.3: without a criterion every document goes into the one summary — there is no summary
    /// per document any more.
    #[test]
    fn everything_in_scope_is_one_group() {
        for group_by in [None, Some(SummaryGroup::None)] {
            let g = group_documents(&docs(), group_by, &GroupInfo::default(), true);
            assert_eq!(g.len(), 1);
            assert_eq!(g[0].docs, vec![0, 1, 2, 3]);
            assert_eq!((g[0].key.as_str(), g[0].label.as_str()), ("all", "전체"));
        }
        assert_eq!(group_documents(&docs(), None, &GroupInfo::default(), false)[0].label, "All documents");
        // A single document's summary is named after it.
        let one = group_documents(&docs()[1..2], None, &GroupInfo::default(), true);
        assert_eq!((one.len(), one[0].label.as_str(), one[0].docs.as_slice()), (1, "제안서.pdf", &[0][..]));
    }

    #[test]
    fn documents_are_grouped_by_project_with_a_rest_group() {
        let info = GroupInfo {
            projects: vec![
                ("p1".into(), "한빛 포털".into()),
                ("p2".into(), "새한 ERP".into()),
                ("p3".into(), "빈 프로젝트".into()),
            ],
            memberships: BTreeMap::from([
                ("a".to_string(), vec!["p1".to_string()]),
                ("b".to_string(), vec!["p1".to_string(), "p2".to_string()]),
                ("c".to_string(), vec!["p2".to_string()]),
            ]),
            type_labels: Vec::new(),
        };
        let g = group_documents(&docs(), Some(SummaryGroup::Project), &info, true);
        let view: Vec<(&str, &[usize])> = g.iter().map(|x| (x.label.as_str(), x.docs.as_slice())).collect();
        assert_eq!(view, vec![("한빛 포털", &[0, 1][..]), ("새한 ERP", &[1, 2][..]), ("프로젝트 없음", &[3][..])]);
    }

    #[test]
    fn documents_are_grouped_by_type_and_folder() {
        let info = GroupInfo {
            type_labels: vec![("dev_contract".into(), "개발 계약서".into(), "Development contract".into())],
            ..GroupInfo::default()
        };
        let g = group_documents(&docs(), Some(SummaryGroup::DocType), &info, true);
        let labels: Vec<&str> = g.iter().map(|x| x.label.as_str()).collect();
        // Registered name → built-in table → slug.
        assert_eq!(labels, vec!["개발 계약서", "제안서", "유형 없음"]);
        assert_eq!(g[0].docs, vec![0, 2]);

        let g = group_documents(&docs(), Some(SummaryGroup::Folder), &info, false);
        let labels: Vec<&str> = g.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(labels, vec!["documents/한빛", "documents/새한", "(top level)"]);
    }
}
