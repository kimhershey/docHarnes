//! The `summarize` step (0.8.0 plan §1.4): one summary per document, and — for `target = set` or
//! a `group_by` — one combined summary per group built from those document summaries.
//!
//! Until 0.8.0 both `target` values ran the same per-document loop, so "묶음 하나로" on the plan
//! card produced eleven separate summaries, and "프로젝트별로 요약해줘" could not be expressed at all
//! (audit USR-7). Grouping is the executor's job: the model only names the criterion.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::documents::DocumentRecord;
use crate::error::{CoreError, Result};
use crate::knowledge::answer::{self, AnswerDeps};
use crate::knowledge::search::SearchDeps;
use crate::knowledge::{Answer, AnswerKind, AnswerRequest, SummaryMode};
use crate::projects::repo as projects_repo;

use super::super::{Plan, SummarizeTarget, SummaryGroup, SummaryLength};
use super::{count_label, no_model, working_set, ExecDeps, LeaseGuard, StepOutcome};

/// What one combining call may carry, in prompt tokens: `batch` for all the document summaries it
/// quotes (larger groups are combined in rounds), `item` for a single one.
///
/// Until 1.0.1 these were 3,000 and 700 characters whatever the model: a combined summary was built
/// from the first lines of each document summary, and a longer one (user report 2026-11-17, "출력이
/// 너무 짧음") had nothing more to say. They follow the loaded model's context now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CombineRoom {
    pub batch: u32,
    pub item: u32,
}

impl CombineRoom {
    /// `room` = the prompt tokens a text-only request may carry ([`answer::text_room`]).
    pub(super) fn of(room: u32) -> CombineRoom {
        // (the instruction and the labels of the documents take a little of it)
        let batch = room.saturating_sub(300).max(1_200);
        CombineRoom { batch, item: (batch / 3).max(500) }
    }
}

/// Documents of one combined summary (`docs` = indices into the working set).
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
    pub rel_path: &'a str,
    pub doc_type: Option<&'a str>,
}

impl<'a> DocKey<'a> {
    fn of(d: &'a DocumentRecord) -> DocKey<'a> {
        DocKey { id: &d.id, rel_path: &d.rel_path, doc_type: d.doc_type.as_deref() }
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

/// Splits the working set into the groups of a combined summary. `None` when no combined
/// summary is asked for (per-document summaries only).
pub(super) fn group_documents(
    docs: &[DocKey<'_>],
    target: Option<SummarizeTarget>,
    group_by: Option<SummaryGroup>,
    info: &GroupInfo,
    ko: bool,
) -> Option<Vec<Group>> {
    let t = |k: &'static str, e: &'static str| if ko { k } else { e };
    match group_by.unwrap_or(SummaryGroup::None) {
        SummaryGroup::None => {
            if target != Some(SummarizeTarget::Set) {
                return None;
            }
            Some(vec![Group {
                key: "all".into(),
                label: t("전체", "All documents").into(),
                docs: (0..docs.len()).collect(),
            }])
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
            Some(groups)
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
            Some(groups)
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
            Some(groups)
        }
    }
}

fn clip(text: &str, tokens: u32) -> String {
    let t = text.trim();
    if answer::text_cost(t) <= tokens {
        return t.to_string();
    }
    let mut out = answer::clip_to_tokens(t, tokens);
    out.push('…');
    out
}

/// Batches `(name, summary)` items so that each batch fits one combining call.
pub(super) fn batches(items: &[(String, String)], room: CombineRoom) -> Vec<Vec<(String, String)>> {
    let mut out: Vec<Vec<(String, String)>> = Vec::new();
    let mut used = 0u32;
    for (name, text) in items {
        let text = clip(text, room.item);
        let size = answer::text_cost(name) + answer::text_cost(&text);
        if out.is_empty() || (used + size > room.batch && !out.last().is_some_and(Vec::is_empty)) {
            out.push(Vec::new());
            used = 0;
        }
        used += size;
        out.last_mut().expect("a batch exists").push((name.clone(), text));
    }
    out
}

/// `detailed`: the reply may be long (the user raised the evidence budget), so the group summary is
/// asked to keep what each document says instead of a few lines for all of them.
fn combine_prompt(label: &str, items: &[(String, String)], ko: bool, detailed: bool) -> String {
    let mut s = if ko {
        format!(
            "다음은 '{label}' 묶음에 속한 문서 {}건의 문서별 요약입니다. 이 요약들만 재료로 묶음 전체를 하나로 요약하세요. \
             여러 문서에 공통된 내용과 문서마다 다른 점을 구분하고, 요약에 없는 사실은 더하지 마세요.{}\n",
            items.len(),
            if detailed {
                " 문서마다 핵심 내용(금액·기한·조건 포함)을 빠뜨리지 말고, 소제목과 항목으로 나눠 자세히 쓰세요."
            } else {
                ""
            }
        )
    } else {
        format!(
            "Below are the per-document summaries of the {} documents in the group '{label}'. Write one summary of the whole group \
             from these summaries only: say what the documents share and where they differ, and add no facts that are not in them.{}\n",
            items.len(),
            if detailed {
                " Keep the key points of every document (amounts, deadlines, conditions) and write in detail, with headings and bullets."
            } else {
                ""
            }
        )
    };
    for (name, text) in items {
        s.push_str(&format!("\n[{name}]\n{text}\n"));
    }
    s
}

/// One combined summary of `items`, in rounds when they do not fit one call.
fn combine(
    adeps: &AnswerDeps<'_>,
    deps: &ExecDeps<'_>,
    label: &str,
    items: &[(String, String)],
    context_tokens: u32,
) -> Result<Answer> {
    let mut items: Vec<(String, String)> = items.to_vec();
    let room = CombineRoom::of(answer::text_room(adeps));
    // (a summary's default reply limit is 1,500 tokens: beyond half as much again it is asked in detail)
    let detailed = answer::summary_reply_tokens(adeps, context_tokens) > 2_250;
    loop {
        let rounds = batches(&items, room);
        let last_round = rounds.len() <= 1;
        let mut next: Vec<(String, String)> = Vec::new();
        let mut answer_of_round: Option<Answer> = None;
        for (i, batch) in rounds.iter().enumerate() {
            deps.checkpoint()?;
            let mut req = AnswerRequest::question(&combine_prompt(label, batch, deps.ko(), detailed));
            req.retrieve = false;
            req.save = false;
            req.context_tokens = context_tokens;
            let ans = answer::answer(adeps, &req, &mut |_| !deps.cancelled())?;
            next.push((format!("{label} {}", i + 1), ans.text.clone()));
            answer_of_round = Some(ans);
        }
        if last_round {
            // `rounds` is never empty: `combine` is only called with items.
            return Ok(answer_of_round.unwrap_or_default());
        }
        items = next;
    }
}

pub(super) fn run(
    deps: &ExecDeps<'_>,
    plan: &Plan,
    imported: Option<&[String]>,
    target: Option<SummarizeTarget>,
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
    let groups = group_documents(&keys, target, group_by, &info, deps.ko());
    // A group of one document needs no combining call: its summary is the document's.
    let combining = groups.as_ref().map(|g| g.iter().filter(|x| x.docs.len() > 1).count()).unwrap_or(0);
    let total = (docs.len() + combining) as u32;

    let cancelled = || deps.cancelled();
    let adeps = AnswerDeps {
        search: SearchDeps { db, analyzer: deps.analyzer, embedder: deps.embedder },
        chat,
        model: deps.model.clone(),
        cancelled: Some(&cancelled),
        context_size: deps.ctx_size,
    };
    // A document without text (not processed yet, an image without OCR) has nothing to summarise.
    // It is left out and named in the result; it does not throw away the summaries made so far.
    // Neither does a document the model could not summarise (1.0.1): one refused request used to end
    // the whole step, and the summaries of the documents before it were lost with it. What stops
    // every document alike — no engine, no model, a cancelled run — still ends the step.
    let mut answers: Vec<Option<Answer>> = Vec::with_capacity(docs.len());
    let mut failures: Vec<(usize, CoreError)> = Vec::new();
    for (i, d) in docs.iter().enumerate() {
        deps.checkpoint()?;
        progress("summarize", i as u32, total, &d.original_name);
        let mut req = AnswerRequest::question("");
        req.kind = AnswerKind::Summary;
        req.document_id = Some(d.id.clone());
        req.summary_mode = mode;
        req.context_tokens = deps.knowledge.context_tokens;
        match answer::summarize(&adeps, &req, &mut |_| !deps.cancelled()) {
            Ok(a) => answers.push(Some(a)),
            Err(CoreError::NotFound(reason)) if reason == "no_text" => answers.push(None),
            Err(
                e @ (CoreError::Cancelled
                | CoreError::EngineNotFound(_)
                | CoreError::UnknownModel(_)
                | CoreError::NoWorkspace
                | CoreError::Db(_)),
            ) => return Err(e),
            // (the engine is gone: the next document would fail the same way within seconds)
            Err(e) if crate::pipeline::vlm::engine_unavailable(&e) => return Err(e),
            Err(e) => {
                log::warn!("summary of {} failed: {e}", d.id);
                failures.push((i, e));
                answers.push(None);
            }
        }
    }
    if answers.iter().all(Option::is_none) {
        // Nothing was summarised: the reason of the first document that failed, else "no text".
        return Err(match failures.into_iter().next() {
            Some((_, e)) => e,
            None => CoreError::NotFound("no_text".into()),
        });
    }
    let summaries: Vec<Value> = docs
        .iter()
        .zip(&answers)
        .filter_map(|(d, a)| Some(json!({ "document_id": d.id, "name": d.original_name, "answer": a.as_ref()? })))
        .collect();
    let skipped: Vec<Value> = docs
        .iter()
        .enumerate()
        .zip(&answers)
        .filter(|((i, _), a)| a.is_none() && !failures.iter().any(|(f, _)| f == i))
        .map(|((_, d), _)| json!({ "document_id": d.id, "name": d.original_name, "reason": "no_text" }))
        .collect();
    let failed: Vec<Value> = failures
        .iter()
        .map(|(i, e)| json!({ "document_id": docs[*i].id, "name": docs[*i].original_name, "error": e.to_string() }))
        .collect();

    let mut group_values: Vec<Value> = Vec::new();
    if let Some(groups) = &groups {
        let mut done = docs.len() as u32;
        for g in groups {
            // Only the documents that have a summary make up the group's summary.
            let members: Vec<(usize, &Answer)> =
                g.docs.iter().filter_map(|&i| answers[i].as_ref().map(|a| (i, a))).collect();
            let ids: Vec<&str> = members.iter().map(|(i, _)| docs[*i].id.as_str()).collect();
            let answer = match members.as_slice() {
                [] => continue,
                [(_, only)] => (*only).clone(),
                many => {
                    deps.checkpoint()?;
                    progress("summarize", done.min(total.saturating_sub(1)), total, &g.label);
                    done += 1;
                    let items: Vec<(String, String)> =
                        many.iter().map(|(i, a)| (docs[*i].original_name.clone(), a.text.clone())).collect();
                    combine(&adeps, deps, &g.label, &items, deps.knowledge.context_tokens)?
                }
            };
            group_values.push(json!({ "key": g.key, "label": g.label, "document_ids": ids, "answer": answer }));
        }
    }

    let criterion = match group_by.unwrap_or(SummaryGroup::None) {
        SummaryGroup::Project => Some(deps.t("프로젝트별", "by project")),
        SummaryGroup::DocType => Some(deps.t("유형별", "by type")),
        SummaryGroup::Folder => Some(deps.t("폴더별", "by folder")),
        SummaryGroup::None => None,
    };
    let made = summaries.len();
    let made_groups = group_values.len();
    let mut summary = match (&groups, criterion) {
        (Some(_), Some(by)) if deps.ko() => format!("{} 요약 · {by} 묶음 {made_groups}개", count_label(deps, made)),
        (Some(_), Some(by)) => format!("Summarised {} · {made_groups} group(s) {by}", count_label(deps, made)),
        (Some(_), None) if deps.ko() => format!("{} 요약 · 묶음 하나로", count_label(deps, made)),
        (Some(_), None) => format!("Summarised {} as one set", count_label(deps, made)),
        (None, _) if deps.ko() => format!("{} 요약", count_label(deps, made)),
        (None, _) => format!("Summarised {}", count_label(deps, made)),
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
    let mut payload = json!({ "summaries": summaries });
    if !skipped.is_empty() {
        payload["skipped"] = Value::Array(skipped);
    }
    if !failed.is_empty() {
        payload["failed"] = Value::Array(failed);
    }
    if groups.is_some() {
        payload["groups"] = Value::Array(group_values);
        payload["group_by"] = json!(group_by.unwrap_or(SummaryGroup::None));
    }
    Ok(StepOutcome { summary, payload, imported: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs() -> Vec<DocKey<'static>> {
        vec![
            DocKey { id: "a", rel_path: "documents/한빛/계약서.pdf", doc_type: Some("dev_contract") },
            DocKey { id: "b", rel_path: "documents/한빛/제안서.pdf", doc_type: Some("proposal") },
            DocKey { id: "c", rel_path: "documents/새한/계약서.pdf", doc_type: Some("dev_contract") },
            DocKey { id: "d", rel_path: "메모.txt", doc_type: None },
        ]
    }

    #[test]
    fn per_document_summaries_have_no_groups() {
        let info = GroupInfo::default();
        assert!(group_documents(&docs(), None, None, &info, true).is_none());
        assert!(
            group_documents(&docs(), Some(SummarizeTarget::EachDoc), Some(SummaryGroup::None), &info, true).is_none()
        );
    }

    #[test]
    fn a_set_is_one_group_of_everything() {
        let g = group_documents(&docs(), Some(SummarizeTarget::Set), None, &GroupInfo::default(), true).unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].docs, vec![0, 1, 2, 3]);
        assert_eq!(g[0].label, "전체");
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
        // `group_by` wins over the target: one combined summary per project either way.
        let g = group_documents(&docs(), None, Some(SummaryGroup::Project), &info, true).unwrap();
        let view: Vec<(&str, &[usize])> = g.iter().map(|x| (x.label.as_str(), x.docs.as_slice())).collect();
        assert_eq!(view, vec![("한빛 포털", &[0, 1][..]), ("새한 ERP", &[1, 2][..]), ("프로젝트 없음", &[3][..])]);
    }

    #[test]
    fn documents_are_grouped_by_type_and_folder() {
        let info = GroupInfo {
            type_labels: vec![("dev_contract".into(), "개발 계약서".into(), "Development contract".into())],
            ..GroupInfo::default()
        };
        let g = group_documents(&docs(), None, Some(SummaryGroup::DocType), &info, true).unwrap();
        let labels: Vec<&str> = g.iter().map(|x| x.label.as_str()).collect();
        // Registered name → built-in table → slug.
        assert_eq!(labels, vec!["개발 계약서", "제안서", "유형 없음"]);
        assert_eq!(g[0].docs, vec![0, 2]);

        let g = group_documents(&docs(), None, Some(SummaryGroup::Folder), &info, false).unwrap();
        let labels: Vec<&str> = g.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(labels, vec!["documents/한빛", "documents/새한", "(top level)"]);
    }

    #[test]
    fn long_groups_are_combined_in_rounds() {
        // The room of the smallest chat context (8,192 tokens): a batch of about 3,400 prompt tokens.
        let room = CombineRoom::of(3_746);
        assert_eq!((room.batch, room.item), (3_446, 1_148));
        let items: Vec<(String, String)> = (0..12).map(|i| (format!("문서{i}.pdf"), "가".repeat(3_000))).collect();
        let b = batches(&items, room);
        let small = b.len();
        assert!(b.len() > 1, "{} batch(es)", b.len());
        assert_eq!(b.iter().map(Vec::len).sum::<usize>(), 12);
        for batch in &b {
            let size: u32 = batch.iter().map(|(n, t)| answer::text_cost(n) + answer::text_cost(t)).sum();
            assert!(size <= room.batch + room.item, "{size}");
            // Each quoted summary is clipped to its share, and says so.
            assert!(batch.iter().all(|(_, t)| answer::text_cost(t) <= room.item + 8 && t.ends_with('…')), "{batch:?}");
        }
        // A summary of ordinary length is quoted whole: 700 characters used to be the limit.
        let whole = vec![("a.pdf".to_string(), "나".repeat(900)), ("b.pdf".to_string(), "다".repeat(900))];
        let b = batches(&whole, room);
        assert_eq!(b.len(), 1);
        assert!(b[0].iter().all(|(_, t)| t.chars().count() == 900), "{}", answer::text_cost(&whole[0].1));
        // A larger context carries more in one call, and quotes each summary whole.
        let large = batches(&items, CombineRoom::of(16_034));
        assert!(large.len() < small, "{} of {small}", large.len());
        assert!(large.iter().flatten().all(|(_, t)| t.chars().count() == 3_000));
        let prompt = combine_prompt("한빛 포털", &whole, true, false);
        assert!(
            prompt.contains("'한빛 포털' 묶음") && prompt.contains("[a.pdf]") && !prompt.contains("자세히"),
            "{prompt}"
        );
        assert!(combine_prompt("한빛 포털", &whole, true, true).contains("소제목과 항목으로 나눠 자세히"));
    }
}
