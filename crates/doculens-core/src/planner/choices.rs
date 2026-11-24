//! 1.0.5 (user decision C) — ask back with choices, without a second model call.
//!
//! Some sentences can be read two ways, and no rule can know which one the user means:
//! - "AAA에 대해서 요약해줘" when a project is called AAA: a summary of that project's documents,
//!   what every document says about AAA, or what the project's documents say about it;
//! - "견적서에 대해 요약해줘": a summary of the estimates, or what the documents say about the word;
//! - "계약서 전부 내보내줘" when some documents are classified as contracts and others only carry
//!   "계약서" in their title or file name: which of them.
//!
//! Instead of picking one reading (and saying so in a note the user reads after the fact), the
//! assistant asks before anything runs. Every reading is a ready-made plan ([`PlanChoice`]): the one
//! the user picks is executed as it is — nothing is planned again, so nothing is asked again.

use rusqlite::Connection;

use crate::josa;

use super::{
    compact, exec, guard, rules, scope, validate, DocTypeSummary, Op, OpKind, Plan, PlanChoice, PlanContext, TypeMatch,
    MIN_CONFIDENCE,
};

/// The reading that is a summary of everything in a place: no subject, no split.
fn whole(mut plan: Plan, length: Option<super::SummaryLength>, summary: String) -> Plan {
    plan.operations = vec![Op::Summarize { length, group_by: None, topic: None }];
    plan.intent = OpKind::Summarize;
    plan.summary = Some(summary);
    plan
}

/// The reading that is about the word: the passages about it, summarised — or, for a sentence that
/// asks to be told or explained without naming a place, an answer from the documents (decision A).
fn about_the_word(
    mut plan: Plan,
    text: &str,
    subject: &str,
    length: Option<super::SummaryLength>,
    answer: bool,
    place: Option<&str>,
    ko: bool,
) -> Plan {
    if answer {
        let question = rules::user_words(text).trim().to_string();
        plan.operations = vec![Op::Answer { question, cite: None, knowledge: None, mode: None, reply: None }];
        plan.intent = OpKind::Answer;
        plan.summary = Some(if ko { "문서에서 찾아 답합니다" } else { "Answers from the documents" }.to_string());
        return plan;
    }
    plan.operations = vec![Op::Summarize { length, group_by: None, topic: Some(subject.to_string()) }];
    plan.intent = OpKind::Summarize;
    plan.summary = Some(match (place, ko) {
        (Some(p), true) => format!("'{p}' 프로젝트에서 '{subject}'에 관한 내용을 찾아 요약합니다"),
        (Some(p), false) => format!("Summarise what the documents of the project '{p}' say about '{subject}'"),
        (None, true) => format!("'{subject}'에 관한 내용을 문서에서 찾아 요약합니다"),
        (None, false) => format!("Summarise what the documents say about '{subject}'"),
    });
    plan
}

/// The choices that pass the validator (each normalised as a planned plan is), as buttons.
fn checked(readings: Vec<(&str, String, Plan)>, ctx: &PlanContext) -> Vec<PlanChoice> {
    readings
        .into_iter()
        .filter_map(|(key, label, mut plan)| {
            let issues = validate::validate(&mut plan, ctx, None);
            (!super::has_errors(&issues) && !plan.is_clarification()).then(|| PlanChoice {
                key: key.to_string(),
                label,
                plan,
            })
        })
        .collect()
}

/// The question and the readings of a sentence whose subject is also the name of a project or of
/// a document type — when it asks for that subject to be summarised, told or explained. `plan` is
/// the plan the sentence was given (after every guard): the readings keep what it says besides
/// (the period, the kind of document, the length).
///
/// Nothing for a sentence that is already an answer to a question (an ask-back text), for a plan
/// that is a question itself, for a plan that does something else than read, and when the sentence
/// settles the reading itself: it says "프로젝트", or names where to look.
pub fn overlap(text: &str, plan: &Plan, ctx: &PlanContext, ko: bool) -> Option<(String, Vec<PlanChoice>)> {
    if plan.is_clarification() || rules::user_words(text) != text {
        return None;
    }
    let [op] = plan.operations.as_slice() else { return None };
    if !matches!(op, Op::Summarize { .. } | Op::Answer { reply: None, .. } | Op::Search { .. } | Op::Explain { .. }) {
        return None;
    }
    // (an answer over a text the user brought or over the last reply looks nothing up: there is
    // no place to choose — 1.0.6)
    if matches!(op, Op::Answer { knowledge: Some(false), mode: None, .. }) {
        return None;
    }
    let a = guard::subject_of(text, ctx)?;
    let summarised = guard::asks_summary(&a.rest);
    let told = !summarised && guard::asks_explanation(&a.rest);
    if !summarised && !told {
        return None;
    }
    let subject = a.subject.as_str();
    let length = rules::length_said(&a.rest);
    let mut base = plan.clone();
    base.requires_confirmation = false;
    base.clarification_question = None;
    base.unmet = None;
    base.confidence = base.confidence.max(MIN_CONFIDENCE);
    let names_documents = !plan.scope.doc_ids.is_empty() || plan.scope.folder.is_some();
    let says_where = guard::says_where(&a.rest, ctx);

    // 1. The subject is the name of a project, and the sentence does not say which it means.
    if let Some(project) = guard::project_of(subject, ctx) {
        let said = compact(text);
        let says_project = said.contains("프로젝트") || rules::says(text, &["project", "projects"]);
        if says_project || names_documents || says_where {
            return None;
        }
        let name = project.name.as_str();
        // ① the project: one summary of its documents.
        let mut of_project = whole(
            base.clone(),
            length,
            if ko {
                format!("'{name}' 프로젝트의 문서를 요약합니다")
            } else {
                format!("Summarise the documents of the project '{name}'")
            },
        );
        of_project.scope.filter.project_id = Some(project.id.clone());
        // ② the word, wherever it stands (inside the "현재 프로젝트" scope when one is set: the
        //    validator fills it in as for any plan that names no place).
        let mut anywhere = about_the_word(base.clone(), text, subject, length, told, None, ko);
        anywhere.scope.filter.project_id = None;
        // ③ the word, inside the project.
        let mut inside = about_the_word(base.clone(), text, subject, length, false, Some(name), ko);
        inside.scope.filter.project_id = Some(project.id.clone());
        let scoped_elsewhere = ctx.scope_project.as_deref().is_some_and(|p| !p.trim().is_empty());
        let everywhere = match (scoped_elsewhere, told, ko) {
            (false, false, true) => format!("모든 문서에서 '{subject}'에 관한 내용을 찾아 요약"),
            (false, true, true) => format!("모든 문서에서 '{subject}'에 관해 찾아 답하기"),
            (true, false, true) => format!("현재 프로젝트 범위에서 '{subject}'에 관한 내용을 찾아 요약"),
            (true, true, true) => format!("현재 프로젝트 범위에서 '{subject}'에 관해 찾아 답하기"),
            (false, false, false) => format!("Summarise what all documents say about '{subject}'"),
            (false, true, false) => format!("Answer from all documents about '{subject}'"),
            (true, false, false) => format!("Summarise what the current project scope says about '{subject}'"),
            (true, true, false) => format!("Answer from the current project scope about '{subject}'"),
        };
        let mut readings = vec![(
            "project",
            if ko {
                format!("'{name}' 프로젝트의 문서 전체를 요약")
            } else {
                format!("Summarise all documents of the project '{name}'")
            },
            of_project,
        )];
        // (with that project as the "현재 프로젝트" scope, "everywhere" is the project again)
        if ctx.scope_project.as_deref() != Some(project.id.as_str()) {
            readings.push(("topic", everywhere, anywhere));
        }
        readings.push((
            "project_topic",
            if ko {
                format!("'{name}' 프로젝트의 문서에서 '{subject}'에 관한 내용을 찾아 요약")
            } else {
                format!("Summarise what the documents of the project '{name}' say about '{subject}'")
            },
            inside,
        ));
        let choices = checked(readings, ctx);
        return (choices.len() >= 2).then(|| {
            (
                if ko {
                    format!("'{name}' 프로젝트가 있습니다. 어느 쪽을 말씀하신 건가요?")
                } else {
                    format!("There is a project called '{name}'. Which did you mean?")
                },
                choices,
            )
        });
    }

    // 2. The subject is the name of a document type: the documents of that type, or the word.
    let doc_type: &DocTypeSummary = guard::type_named(subject, ctx)?;
    if !plan.scope.doc_ids.is_empty() {
        return None;
    }
    let mut of_type = whole(
        base.clone(),
        length,
        if ko {
            format!("{} 요약합니다", josa::eul_reul(&format!("'{}' 문서", doc_type.label_ko)))
        } else {
            format!("Summarise the '{}' documents", doc_type.label_en)
        },
    );
    of_type.scope.filter.doc_type = Some(doc_type.slug.clone());
    of_type.scope.filter.type_match = None;
    let place = plan.scope.filter.project_id.as_deref().and_then(|id| ctx.projects.iter().find(|p| p.id == id));
    let mut word =
        about_the_word(base.clone(), text, subject, length, told && !says_where, place.map(|p| p.name.as_str()), ko);
    word.scope.filter.doc_type = None;
    word.scope.filter.type_match = None;
    let answers = matches!(word.operations.as_slice(), [Op::Answer { .. }]);
    let label = if ko { doc_type.label_ko.as_str() } else { doc_type.label_en.as_str() };
    let readings = vec![
        (
            "type",
            if ko {
                format!("유형이 '{label}'인 문서를 요약")
            } else {
                format!("Summarise the documents of the type '{label}'")
            },
            of_type,
        ),
        (
            "topic",
            match (answers, ko) {
                (false, true) => format!("문서에서 '{subject}'에 관한 내용을 찾아 요약"),
                (true, true) => format!("문서에서 '{subject}'에 관해 찾아 답하기"),
                (false, false) => format!("Summarise what the documents say about '{subject}'"),
                (true, false) => format!("Answer from the documents about '{subject}'"),
            },
            word,
        ),
    ];
    let choices = checked(readings, ctx);
    (choices.len() >= 2).then(|| {
        (
            if ko {
                format!(
                    "{} 문서 유형의 이름이기도 합니다. 어느 쪽을 말씀하신 건가요?",
                    josa::eun_neun(&format!("'{subject}'"))
                )
            } else {
                format!("'{subject}' is also the name of a document type. Which did you mean?")
            },
            choices,
        )
    })
}

/// The question and the readings of a plan whose scope names a document type, when the documents
/// classified as that type and the ones that only carry its name are not the same: the user says
/// which are meant (`scope.filter.type_match`). Asked only when both kinds exist — with one of
/// them the condition takes what there is ([`scope::resolve`]: the union).
///
/// Needs the workspace: the shell calls it after the default scope is applied and before the
/// preview. A plan that came from a choice carries its `type_match` and is not asked about again.
pub fn type_overlap(
    conn: &Connection,
    plan: &Plan,
    ctx: &PlanContext,
    ko: bool,
) -> rusqlite::Result<Option<(String, Vec<PlanChoice>)>> {
    let Some(slug) = plan.scope.filter.doc_type.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if plan.is_clarification() || plan.scope.filter.type_match.is_some() || !validate::uses_scope(plan) {
        return Ok(None);
    }
    // The rest of the scope decides which documents are looked at; the type splits them.
    let mut rest = exec::plan_scope(plan);
    rest.filter.doc_type = None;
    rest.filter.type_match = None;
    let docs = scope::resolve(conn, &rest)?;
    let names = scope::type_names(conn, slug)?;
    let is_classified = |d: &&crate::documents::DocumentRecord| d.doc_type.as_deref() == Some(slug);
    let classified = docs.iter().filter(is_classified).count();
    let named = docs.iter().filter(|d| scope::named_as(d, &names)).count();
    let named_only = docs.iter().filter(|d| !is_classified(d) && scope::named_as(d, &names)).count();
    if classified == 0 || named_only == 0 {
        return Ok(None);
    }
    let label = ctx
        .doc_types
        .iter()
        .find(|d| d.slug == slug)
        .map(|d| if ko { d.label_ko.clone() } else { d.label_en.clone() })
        .unwrap_or_else(|| slug.to_string());
    let with = |how: TypeMatch| {
        let mut p = plan.clone();
        p.scope.filter.type_match = Some(how);
        p
    };
    let count = |n: usize| if ko { format!("{n}건") } else { format!("{n}") };
    let quoted = format!("'{label}'");
    let choices = vec![
        PlanChoice {
            key: "classified".into(),
            label: if ko {
                format!("{} 분류된 문서 {}", josa::euro_ro(&quoted), count(classified))
            } else {
                format!("Documents classified as {quoted} ({})", count(classified))
            },
            plan: with(TypeMatch::Classified),
        },
        PlanChoice {
            key: "named".into(),
            label: if ko {
                format!("제목이나 파일 이름에 유형 이름이 들어 있는 문서 {}", count(named))
            } else {
                format!("Documents with the type's name in the title or file name ({})", count(named))
            },
            plan: with(TypeMatch::Named),
        },
        PlanChoice {
            key: "both".into(),
            label: if ko {
                format!("둘 다 {}", count(classified + named_only))
            } else {
                format!("Both ({})", count(classified + named_only))
            },
            plan: with(TypeMatch::Both),
        },
    ];
    let question = if ko {
        format!(
            "{quoted} 문서를 어떻게 고를까요? {} 분류된 문서가 {classified}건 있고, 그렇게 분류되지는 않았지만 제목이나 파일 이름에 유형 이름이 들어 있는 문서가 {named_only}건 있습니다.",
            josa::euro_ro(&quoted)
        )
    } else {
        format!(
            "Which {quoted} documents? {classified} are classified as that type, and {named_only} more only carry its name in their title or file name."
        )
    };
    Ok(Some((question, choices)))
}

/// The clarification that stands in for a plan the user has to choose a reading of: every existing
/// consumer treats the result as a question; the choices travel next to it
/// ([`super::PlanResult::choices`]).
pub fn question_plan(question: String, plan: &Plan) -> Plan {
    let mut asked = Plan::clarify(question, plan.confidence);
    asked.summary = plan.summary.clone();
    asked
}
