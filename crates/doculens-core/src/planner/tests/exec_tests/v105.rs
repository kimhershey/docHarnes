//! 1.0.5 — the document type as a scope condition against a workspace (user decision B), the
//! question that asks which documents of a type are meant (decision C, `choices::type_overlap`)
//! and the reason a step without documents gives (G).

use super::*;

use crate::planner::choices;
use crate::projects::{repo as projects_repo, ProjectInput};

fn run(w: &World, plan: &Plan) -> exec::PlanExecution {
    exec::run(&w.deps(None), plan, &mut |_| {}).unwrap()
}

fn preview(w: &World, plan: &Plan) -> exec::PlanPreview {
    exec::preview(&w.db, &w.paths, plan, &ProcessingSettings::default(), Language::Ko).unwrap()
}

fn typed(slug: &str, how: Option<TypeMatch>) -> Scope {
    Scope {
        filter: ScopeFilter { doc_type: Some(slug.into()), type_match: how, ..ScopeFilter::default() },
        ..Scope::default()
    }
}

fn plan_of(ops: Vec<Op>, scope: Scope) -> Plan {
    let mut plan = Plan::single(ops[0].clone(), 0.9, "s");
    plan.operations = ops;
    plan.scope = scope;
    plan
}

fn context() -> PlanContext {
    PlanContext {
        doc_types: vec![DocTypeSummary {
            slug: "dev_contract".into(),
            label_ko: "소프트웨어 개발 계약서".into(),
            label_en: "Software development contract".into(),
        }],
        ..PlanContext::default()
    }
}

/// A classified contract, a document that only carries the name in its title, one that carries it in
/// its file name, one that is both and one that is neither.
fn world() -> World {
    let w = World::new();
    for (id, path) in [
        ("classified", "inbox/a.jpg"),
        ("titled", "inbox/b.jpg"),
        ("filed", "inbox/한빛_용역계약서_초안.jpg"),
        ("both", "inbox/유지보수 계약서.jpg"),
        ("neither", "inbox/c.jpg"),
        ("estimate", "inbox/d.jpg"),
    ] {
        w.add_image(id, path);
    }
    w.db.with(|c| {
        docs::set_doc_type(c, "classified", Some("dev_contract"), Some("manual"))?;
        docs::set_doc_type(c, "both", Some("dev_contract"), Some("manual"))?;
        // (a type of its own does not hide a name: the classifier may have got it wrong)
        docs::set_doc_type(c, "estimate", Some("estimate"), Some("manual"))?;
        docs::set_title(c, "titled", Some("새봄 ERP 개발 계약서 (서명본)"))?;
        // "계약" alone — two characters — says nothing about the type.
        docs::set_title(c, "neither", Some("계약 관련 메모"))
    })
    .unwrap();
    w
}

fn resolved(w: &World, scope: &Scope) -> Vec<String> {
    let mut ids: Vec<String> =
        w.db.with(|c| crate::planner::scope::resolve(c, scope)).unwrap().into_iter().map(|d| d.id).collect();
    ids.sort();
    ids
}

#[test]
fn a_type_condition_takes_classified_and_named_documents() {
    let w = world();
    assert_eq!(resolved(&w, &typed("dev_contract", Some(TypeMatch::Classified))), vec!["both", "classified"]);
    assert_eq!(resolved(&w, &typed("dev_contract", Some(TypeMatch::Named))), vec!["both", "filed", "titled"]);
    let union = vec!["both", "classified", "filed", "titled"];
    assert_eq!(resolved(&w, &typed("dev_contract", Some(TypeMatch::Both))), union);
    assert_eq!(resolved(&w, &typed("dev_contract", None)), union, "no choice made: both");
    // The condition intersects with the rest of the scope, as before.
    let mut scope = typed("dev_contract", None);
    scope.doc_ids = vec!["titled".into(), "neither".into()];
    assert_eq!(resolved(&w, &scope), vec!["titled"]);
    // A type nothing is classified as and nothing is named after matches nothing — the condition is
    // never dropped (a delete must not widen to every document).
    assert_eq!(resolved(&w, &typed("wbs", None)), Vec::<String>::new());
    let delete = plan_of(vec![Op::Delete { to_trash: None }], typed("wbs", None));
    let p = preview(&w, &delete);
    assert_eq!(p.document_count, 0, "{p:?}");
    let ex = run(&w, &delete);
    assert_eq!(ex.steps[0].payload["moved"], json!(0), "{:?}", ex.steps[0]);
    for id in ["classified", "titled", "filed", "both", "neither", "estimate"] {
        assert!(w.doc(id).deleted_at.is_none(), "{id}");
    }
    // The names of a type: its registered forms, the built-in labels and the aliases of three or
    // more characters.
    let names = w.db.with(|c| crate::planner::scope::type_names(c, "dev_contract")).unwrap();
    for name in ["소프트웨어개발계약서", "계약서", "용역계약서", "contract"] {
        assert!(names.iter().any(|n| n == name), "{name}: {names:?}");
    }
    assert!(!names.iter().any(|n| n == "계약"), "{names:?}");
}

#[test]
fn documents_of_a_type_are_asked_about_when_classified_and_named_differ() {
    let w = world();
    let c = context();
    let ask = |plan: &Plan| w.db.with(|conn| choices::type_overlap(conn, plan, &c, true)).unwrap();
    let summarize = || Op::Summarize { length: None, group_by: None, topic: None };
    let plan = plan_of(vec![summarize()], typed("dev_contract", None));
    let (question, readings) = ask(&plan).expect("a question");
    assert!(question.contains("'소프트웨어 개발 계약서'") && question.contains("2건"), "{question}");
    assert_eq!(
        readings.iter().map(|r| (r.key.as_str(), r.plan.scope.filter.type_match)).collect::<Vec<_>>(),
        vec![
            ("classified", Some(TypeMatch::Classified)),
            ("named", Some(TypeMatch::Named)),
            ("both", Some(TypeMatch::Both))
        ]
    );
    // The counts on the buttons are the documents each plan covers.
    for (reading, count) in readings.iter().zip([2u64, 3, 4]) {
        assert!(reading.label.contains(&format!("{count}건")), "{reading:?}");
        assert_eq!(preview(&w, &reading.plan).document_count, count, "{reading:?}");
        // A plan that came from a choice is not asked about again.
        assert!(ask(&reading.plan).is_none());
        assert_eq!(reading.plan.operations, plan.operations);
    }
    assert_eq!(readings[0].label, "'소프트웨어 개발 계약서'로 분류된 문서 2건");
    // The rest of the scope decides which documents are counted.
    let mut narrowed = plan.clone();
    narrowed.scope.doc_ids = vec!["classified".into(), "neither".into()];
    assert!(ask(&narrowed).is_none(), "nothing is only named among these");
    // One kind only: nothing to ask — the condition takes what there is.
    w.db.with(|c| {
        docs::set_doc_type(c, "titled", Some("dev_contract"), Some("manual"))?;
        docs::set_doc_type(c, "filed", Some("dev_contract"), Some("manual"))
    })
    .unwrap();
    assert!(ask(&plan).is_none());
    assert!(ask(&plan_of(vec![summarize()], typed("wbs", None))).is_none());
    // No type in the scope, a question, a plan that reads no documents: no question.
    assert!(ask(&plan_of(vec![summarize()], Scope::default())).is_none());
    assert!(ask(&Plan::clarify("?", 0.5)).is_none());
    assert!(ask(&plan_of(vec![Op::Help {}], typed("dev_contract", None))).is_none());
}

/// G — "이 범위에는 참고 자료가 없습니다" was said whenever the scope named reference material, even
/// when another condition was what matched nothing.
#[test]
fn the_reason_names_reference_material_only_when_it_is_what_is_missing() {
    let w = world();
    let project =
        w.db.with(|c| projects_repo::save_project(c, &ProjectInput { name: "AAA".into(), ..Default::default() }))
            .unwrap();
    w.db.with(|c| {
        let group = crate::knowledge::repo::project_reference_group(c, &project.id, &project.name)?;
        crate::knowledge::repo::add_docs_to_collection(c, &group.id, &["neither".to_string()])?;
        docs::set_role(c, "neither", DocumentRole::Knowledge)
    })
    .unwrap();
    let reference = |filter: ScopeFilter| Scope {
        filter: ScopeFilter { project_id: Some(project.id.clone()), role: Some(RoleParam::Knowledge), ..filter },
        ..Scope::default()
    };
    let failure = |scope: Scope| {
        let plan = plan_of(vec![Op::Summarize { length: None, group_by: None, topic: None }], scope);
        let ex = run(&w, &plan);
        assert_eq!(ex.steps[0].status, StepStatus::Failed, "{:?}", ex.steps[0]);
        ex.steps[0].error.clone().unwrap_or_default()
    };
    // The project has reference material; a tag no document carries empties the scope.
    assert_eq!(resolved(&w, &reference(ScopeFilter::default())), vec!["neither"]);
    let error = failure(reference(ScopeFilter { tags: vec!["AAA".into()], ..ScopeFilter::default() }));
    assert!(error.contains("no_documents") && !error.contains("no_reference"), "{error}");
    let error = failure(reference(ScopeFilter { doc_type: Some("wbs".into()), ..ScopeFilter::default() }));
    assert!(error.contains("no_documents") && !error.contains("no_reference"), "{error}");
    // Reference material is what is missing: said so, as before.
    let empty = w
        .db
        .with(|c| projects_repo::save_project(c, &ProjectInput { name: "빈 프로젝트".into(), ..Default::default() }))
        .unwrap();
    let error = failure(Scope {
        filter: ScopeFilter {
            project_id: Some(empty.id.clone()),
            role: Some(RoleParam::Knowledge),
            ..ScopeFilter::default()
        },
        ..Scope::default()
    });
    assert!(error.contains("no_reference_in_project"), "{error}");
}
