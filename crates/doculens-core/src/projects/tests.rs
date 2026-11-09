//! Projects: migration, templates, project CRUD + membership, checklist,
//! phrase rules (ko / en) and the model fallback, deterministic checks with
//! evidence, relations, report, 보완요청서 request and inbox mapping.

use serde_json::{json, Value};

use crate::aggregate::tests::{seed, Scripted, Seed};
use crate::db::{Db, SCHEMA_VERSION};
use crate::documents::{repo as docs, DocumentRole, NewDocument, SourceKind};
use crate::error::CoreError;
use crate::generate::{OutputFormat, SourceRef};
use crate::pipeline::repo as pipeline_repo;
use crate::pipeline::schema::FieldKind::{self, Amount, Boolean, Brn, Date, String as Str};
use crate::pipeline::vlm::testing::StageChat;
use crate::pipeline::vlm::Vlm;
use crate::pipeline::ReviewReason;
use crate::settings::{Language, Profile};

use super::*;

fn db() -> Db {
    let db = Db::open_in_memory().unwrap();
    db.with(pipeline_repo::ensure_builtins).unwrap();
    db
}

fn deps<'a>(db: &'a Db, vlm: Option<&'a Vlm<'a>>) -> ProjectDeps<'a> {
    ProjectDeps { db, vlm, language: Language::Ko }
}

/// Inserts one extracted document; `meta` = (vendor, document date, total amount).
fn seed_doc(db: &Db, id: &str, doc_type: &str, meta: (&str, &str, f64), pending: bool, fields: &[Scripted<'_>]) {
    let name = format!("{id}_{doc_type}.pdf");
    seed(
        db,
        &Seed {
            id,
            name: &name,
            doc_type,
            vendor: meta.0,
            doc_date: meta.1,
            total: meta.2,
            pending,
            fields,
            pages: &[Some("본문")],
        },
    );
}

/// 사업자등록증 한빛상사 123-45-67890.
fn seed_br(db: &Db, id: &str, brn: &str, company: &str) {
    let digits: String = brn.chars().filter(|c| c.is_ascii_digit()).collect();
    seed_doc(
        db,
        id,
        "business_registration",
        (company, "2020-03-02", 0.0),
        false,
        &[
            ("registration_no", brn, Some(digits.as_str()), Brn, None),
            ("company_name", company, Some(company), Str, None),
            ("representative", "김한빛", Some("김한빛"), Str, None),
            ("business_address", "서울 강남구", Some("서울 강남구"), Str, None),
            ("issue_date", "2020.03.02", Some("2020-03-02"), Date, None),
        ],
    );
}

/// 세금계산서 from `supplier` (`tax_pending` puts the tax total in the review queue).
fn seed_invoice(
    db: &Db,
    id: &str,
    supplier_brn: &str,
    supplier_name: &str,
    supply: &str,
    total: &str,
    tax_pending: bool,
) {
    let digits: String = supplier_brn.chars().filter(|c| c.is_ascii_digit()).collect();
    let supply_n = supply.replace(',', "");
    let total_n = total.replace(',', "");
    let tax_reason = tax_pending.then_some(ReviewReason::LowConfidence);
    seed_doc(
        db,
        id,
        "tax_invoice",
        (supplier_name, "2026-09-30", total_n.parse().unwrap()),
        tax_pending,
        &[
            ("issue_date", "2026.09.30", Some("2026-09-30"), Date, None),
            ("supplier.brn", supplier_brn, Some(digits.as_str()), Brn, None),
            ("supplier.name", supplier_name, Some(supplier_name), Str, None),
            ("buyer.brn", "220-81-62517", Some("2208162517"), Brn, None),
            ("buyer.name", "주식회사 도큐", Some("주식회사 도큐"), Str, None),
            ("supply_total", supply, Some(supply_n.as_str()), Amount, None),
            ("tax_total", "100,000", if tax_pending { None } else { Some("100000") }, Amount, tax_reason),
            ("grand_total", total, Some(total_n.as_str()), Amount, None),
        ],
    );
}

/// 계약서 between 한빛상사 (갑) and 도큐 (을).
fn seed_contract(db: &Db, id: &str, party_brn: &str, party_name: &str, amount: &str, signed: Option<&str>) {
    let digits: String = party_brn.chars().filter(|c| c.is_ascii_digit()).collect();
    let amount_n = amount.replace(',', "");
    let mut fields: Vec<Scripted<'_>> = vec![
        ("title", "소프트웨어 공급 계약", Some("소프트웨어 공급 계약"), Str, None),
        ("parties[0].role", "갑", Some("갑"), Str, None),
        ("parties[0].name", party_name, Some(party_name), Str, None),
        ("parties[0].brn", party_brn, Some(digits.as_str()), Brn, None),
        ("parties[1].role", "을", Some("을"), Str, None),
        ("parties[1].name", "㈜도큐", Some("㈜도큐"), Str, None),
        ("parties[1].brn", "220-81-62517", Some("2208162517"), Brn, None),
        ("contract_date", "2026-09-01", Some("2026-09-01"), Date, None),
        ("amount", amount, Some(amount_n.as_str()), Amount, None),
    ];
    if let Some(s) = signed {
        fields.push(("signed", s, Some(if s == "예" { "true" } else { "false" }), Boolean, None));
    }
    seed_doc(db, id, "contract", (party_name, "2026-09-01", amount_n.parse().unwrap()), false, &fields);
}

fn seed_application(db: &Db, id: &str, signed: Option<(&str, bool)>) {
    let mut fields: Vec<Scripted<'_>> = vec![
        ("form_title", "인증 신청서", Some("인증 신청서"), Str, None),
        ("applicant.name", "김한빛", Some("김한빛"), Str, None),
        ("application_date", "2026-10-01", Some("2026-10-01"), Date, None),
    ];
    let mut pending = false;
    if let Some((s, p)) = signed {
        pending = p;
        fields.push(("signed", s, Some("true"), Boolean, p.then_some(ReviewReason::LowConfidence)));
    }
    seed_doc(db, id, "application_form", ("김한빛", "2026-10-01", 0.0), pending, &fields);
}

fn project(db: &Db, name: &str, template: Option<&str>) -> Project {
    db.with(|c| {
        repo::save_project(
            c,
            &ProjectInput {
                name: name.into(),
                template_id: template.map(str::to_string),
                intake_no: Some("2026-0042".into()),
                owner: Some("박담당".into()),
                due_date: Some("2026-11-30".into()),
                ..Default::default()
            },
        )
    })
    .unwrap()
}

fn add(db: &Db, project_id: &str, ids: &[&str]) -> u32 {
    let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    db.with(|c| repo::add_documents(c, project_id, &ids)).unwrap()
}

fn add_rule(
    db: &Db,
    project_id: &str,
    description: &str,
    left: RuleOperand,
    op: RuleOp,
    right: Option<RuleOperand>,
    tolerance: Option<f64>,
) -> Rule {
    let rule = Rule { description: description.into(), left, op, right, tolerance, ..Rule::default() };
    db.with(|c| repo::save_rule(c, project_id, &rule)).unwrap()
}

/// Certification project: BR + invoice + contract (application form missing).
fn certification_world(db: &Db) -> Project {
    seed_br(db, "br1", "123-45-67890", "한빛상사");
    seed_invoice(db, "ti1", "123-45-67890", "㈜한빛상사", "1,000,000", "1,100,000", true);
    seed_contract(db, "c1", "123-45-67890", "한빛상사", "1,000,000", Some("예"));
    let p = project(db, "한빛 인증 심사", Some("tpl_certification_audit"));
    assert_eq!(add(db, &p.id, &["br1", "ti1", "c1"]), 3);
    p
}

#[test]
fn migration_adds_project_tables() {
    let db = db();
    // Schema 5 added the project tables; schema 6 (page layout) only adds a column to `pages`.
    assert_eq!(SCHEMA_VERSION, 6);
    assert_eq!(db.user_version().unwrap(), 6);
    let tables: Vec<String> = db
        .with(|c| {
            let mut s = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?;
            let rows = s.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect()
        })
        .unwrap();
    for t in
        ["projects", "project_templates", "project_documents", "project_rules", "project_runs", "project_rule_results"]
    {
        assert!(tables.iter().any(|n| n == t), "missing table {t}: {tables:?}");
    }
}

#[test]
fn builtin_templates_and_user_template_crud() {
    let db = db();
    let pure = builtin_templates();
    assert_eq!(pure.len(), 3);
    assert!(pure.iter().all(|t| t.builtin && !t.rules.is_empty() && !t.required_doc_types.is_empty()));
    let audit = pure.iter().find(|t| t.id == "tpl_certification_audit").unwrap();
    assert_eq!(audit.required_doc_types, vec!["business_registration", "application_form", "contract"]);
    let signed = audit.rules.iter().find(|r| r.op == RuleOp::Present).unwrap();
    assert_eq!(signed.left, RuleOperand::field("application_form", "signed"));
    let settlement = pure.iter().find(|t| t.id == "tpl_purchase_settlement").unwrap();
    assert!(settlement.rules.iter().any(|r| r.op == RuleOp::WithinPct && r.tolerance == Some(5.0)));
    // Every built-in rule refers to a field the resolution context knows.
    let ctx = db.with(|c| rules::context(c, None, Some("tpl_purchase_settlement"))).unwrap();
    for t in &pure {
        for r in &t.rules {
            let ctx = if t.id == "tpl_purchase_settlement" {
                ctx.clone()
            } else {
                db.with(|c| rules::context(c, None, Some(&t.id))).unwrap()
            };
            let l = &r.left;
            assert!(
                ctx.field(l.doc_type.as_deref().unwrap(), l.field.as_deref().unwrap()).is_some(),
                "{} {:?}",
                t.id,
                l
            );
            if let Some(rr) = &r.right {
                assert!(
                    ctx.field(rr.doc_type.as_deref().unwrap(), rr.field.as_deref().unwrap()).is_some(),
                    "{} {:?}",
                    t.id,
                    rr
                );
            }
        }
    }

    let listed = db.with(repo::list_templates).unwrap();
    assert_eq!(listed.len(), 3);
    assert!(listed.iter().all(|t| t.builtin));
    // Listing again does not duplicate.
    assert_eq!(db.with(repo::list_templates).unwrap().len(), 3);
    // `ensure_builtins` refreshes an untouched built-in from its definition without rewriting
    // `created_at` (and leaves it refreshable: `updated_at` stays equal to `created_at`).
    let before = db.with(|c| repo::get_template(c, "tpl_certification_audit")).unwrap().unwrap();
    db.with(|c| c.execute("UPDATE project_templates SET name_en = 'stale' WHERE id = 'tpl_certification_audit'", []))
        .unwrap();
    db.with(repo::ensure_builtins).unwrap();
    let after = db.with(|c| repo::get_template(c, "tpl_certification_audit")).unwrap().unwrap();
    assert_eq!((after.created_at, after.updated_at), (before.created_at, before.updated_at));
    assert_eq!(after.name_en, before.name_en, "refreshed from the built-in definition");

    let user = db
        .with(|c| {
            repo::save_template(
                c,
                &TemplateInput {
                    name_ko: "임대 계약 검토".into(),
                    required_doc_types: vec!["contract".into(), "business_registration".into(), "contract".into()],
                    rules: vec![Rule {
                        id: String::new(),
                        description: "계약서 서명 필수".into(),
                        left: RuleOperand::field("contract", "signed"),
                        op: RuleOp::Present,
                        ..Rule::default()
                    }],
                    ..Default::default()
                },
            )
        })
        .unwrap();
    assert!(!user.builtin && user.id.starts_with("tpl_"));
    assert_eq!(user.name_en, "임대 계약 검토", "name_en falls back to the Korean name");
    assert_eq!(user.required_doc_types, vec!["contract", "business_registration"], "deduplicated");
    assert!(!user.rules[0].id.is_empty(), "rule ids are filled in");
    assert_eq!(db.with(repo::list_templates).unwrap().len(), 4);

    let updated = db
        .with(|c| {
            repo::save_template(
                c,
                &TemplateInput {
                    id: Some(user.id.clone()),
                    name_ko: "임대 계약".into(),
                    name_en: Some("Lease".into()),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    assert_eq!((updated.name_ko.as_str(), updated.name_en.as_str()), ("임대 계약", "Lease"));
    assert!(updated.rules.is_empty());

    let err = db.with(|c| repo::delete_template(c, "tpl_certification_audit")).unwrap_err();
    assert!(err.to_string().contains("template_builtin"), "{err}");
    assert!(db.with(|c| repo::delete_template(c, &user.id)).unwrap());
    assert!(!db.with(|c| repo::delete_template(c, &user.id)).unwrap());
    assert_eq!(db.with(repo::list_templates).unwrap().len(), 3);

    // Editing a built-in keeps the edit across `ensure_builtins`.
    let edited = db
        .with(|c| {
            repo::save_template(
                c,
                &TemplateInput {
                    id: Some("tpl_vendor_registration".into()),
                    name_ko: "거래처 등록(수정)".into(),
                    required_doc_types: vec!["business_registration".into()],
                    ..Default::default()
                },
            )
        })
        .unwrap();
    assert!(edited.builtin);
    db.with(repo::ensure_builtins).unwrap();
    let again = db.with(|c| repo::get_template(c, "tpl_vendor_registration")).unwrap().unwrap();
    assert_eq!(again.name_ko, "거래처 등록(수정)");
    let empty =
        db.with(|c| repo::save_template(c, &TemplateInput { name_ko: "  ".into(), ..Default::default() })).unwrap_err();
    assert!(empty.to_string().contains("template_name_empty"));
}

#[test]
fn project_crud_inherits_template_rules() {
    let db = db();
    let p = project(&db, "  한빛 인증  심사 ", Some("tpl_certification_audit"));
    assert_eq!(p.name, "한빛 인증  심사", "trimmed, inner spacing kept");
    assert_eq!(p.status, ProjectStatus::Intake);
    assert_eq!((p.document_count, p.missing_count), (0, 3));
    assert!(p.last_check_at.is_none());
    let rules = db.with(|c| repo::rules_of(c, &p.id)).unwrap();
    assert_eq!(rules.len(), 3);
    let template = builtin_templates().into_iter().find(|t| t.id == "tpl_certification_audit").unwrap();
    assert!(rules
        .iter()
        .zip(&template.rules)
        .all(|(a, b)| a.id != b.id && a.description == b.description && a.left == b.left));
    // Two projects from the same template get independent rule rows.
    let p2 = project(&db, "두번째", Some("tpl_certification_audit"));
    assert_eq!(db.with(|c| repo::rules_of(c, &p2.id)).unwrap().len(), 3);
    assert_eq!(db.with(repo::list_projects).unwrap().len(), 2);

    let by_name = db.with(|c| repo::project_by_name(c, "한빛인증심사")).unwrap().unwrap();
    assert_eq!(by_name.id, p.id);
    assert!(db.with(|c| repo::project_by_name(c, "없는 프로젝트")).unwrap().is_none());

    let updated = db
        .with(|c| {
            repo::save_project(
                c,
                &ProjectInput {
                    id: Some(p.id.clone()),
                    name: "한빛 인증 심사 2026".into(),
                    template_id: Some("tpl_certification_audit".into()),
                    status: Some(ProjectStatus::Validating),
                    notes: Some("재심".into()),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    assert_eq!((updated.name.as_str(), updated.status), ("한빛 인증 심사 2026", ProjectStatus::Validating));
    assert_eq!(updated.notes.as_deref(), Some("재심"));
    assert_eq!(updated.intake_no.as_deref(), Some("2026-0042"), "a `None` field keeps the stored value");
    assert_eq!(updated.owner.as_deref(), Some("박담당"));
    let cleared = db
        .with(|c| {
            repo::save_project(
                c,
                &ProjectInput {
                    id: Some(p.id.clone()),
                    name: "한빛 인증 심사 2026".into(),
                    intake_no: Some("  ".into()),
                    owner: Some(String::new()),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    assert_eq!((cleared.intake_no, cleared.owner), (None, None), "a blank `Some` clears the field");
    assert_eq!(cleared.notes.as_deref(), Some("재심"));
    assert_eq!(cleared.template_id.as_deref(), Some("tpl_certification_audit"), "the template is kept too");
    assert_eq!(cleared.status, ProjectStatus::Validating);
    db.with(|c| repo::set_status(c, &p.id, ProjectStatus::Done)).unwrap();
    assert_eq!(db.with(|c| repo::get_project(c, &p.id)).unwrap().unwrap().status, ProjectStatus::Done);

    // Rule CRUD.
    let r = add_rule(
        &db,
        &p.id,
        "계약금액 = 세금계산서 합계",
        RuleOperand::field("contract", "amount"),
        RuleOp::Eq,
        Some(RuleOperand::field("tax_invoice", "grand_total")),
        None,
    );
    assert_eq!(db.with(|c| repo::rules_of(c, &p.id)).unwrap().len(), 4);
    let mut edited = r.clone();
    edited.enabled = false;
    edited.tolerance = Some(100.0);
    db.with(|c| repo::save_rule(c, &p.id, &edited)).unwrap();
    let stored = db.with(|c| repo::rules_of(c, &p.id)).unwrap();
    let last = stored.iter().find(|x| x.id == r.id).unwrap();
    assert!(!last.enabled && last.tolerance == Some(100.0));
    assert_eq!(stored.last().unwrap().id, r.id, "appended at the end");
    assert!(db.with(|c| repo::delete_rule(c, &p.id, &r.id)).unwrap());
    assert_eq!(db.with(|c| repo::rules_of(c, &p.id)).unwrap().len(), 3);
    let foreign = db.with(|c| repo::save_rule(c, &p2.id, &stored[0])).unwrap_err();
    assert!(foreign.to_string().contains("rule_not_found"));

    let missing_template = db
        .with(|c| {
            repo::save_project(
                c,
                &ProjectInput { name: "x".into(), template_id: Some("tpl_nope".into()), ..Default::default() },
            )
        })
        .unwrap_err();
    assert!(missing_template.to_string().contains("template_not_found"));
    let empty =
        db.with(|c| repo::save_project(c, &ProjectInput { name: " ".into(), ..Default::default() })).unwrap_err();
    assert!(empty.to_string().contains("project_name_empty"));

    assert!(db.with(|c| repo::delete_project(c, &p.id)).unwrap());
    assert!(db.with(|c| repo::get_project(c, &p.id)).unwrap().is_none());
    assert!(db.with(|c| repo::rules_of(c, &p.id)).unwrap().is_empty(), "cascade");
    assert!(!db.with(|c| repo::delete_project(c, &p.id)).unwrap());
}

#[test]
fn membership_and_checklist() {
    let db = db();
    seed_br(&db, "br1", "123-45-67890", "한빛상사");
    seed_invoice(&db, "ti1", "123-45-67890", "한빛상사", "1,000,000", "1,100,000", false);
    let p = project(&db, "거래처 A", Some("tpl_certification_audit"));
    assert_eq!(add(&db, &p.id, &["br1", "ti1", "nope", "br1"]), 2, "unknown and duplicate ids are skipped");
    assert_eq!(add(&db, &p.id, &["br1"]), 0);
    let docs = db.with(|c| repo::documents_of(c, &p.id)).unwrap();
    assert_eq!(docs.iter().map(|d| d.document.id.as_str()).collect::<Vec<_>>(), vec!["br1", "ti1"]);
    let p = db.with(|c| repo::get_project(c, &p.id)).unwrap().unwrap();
    assert_eq!((p.document_count, p.missing_count), (2, 2));

    let list = db.with(|c| repo::checklist(c, &p.id, Language::Ko)).unwrap();
    let rows: Vec<(&str, bool, bool, usize)> =
        list.iter().map(|c| (c.doc_type.as_str(), c.required, c.missing, c.doc_ids.len())).collect();
    assert_eq!(
        rows,
        vec![
            ("business_registration", true, false, 1),
            ("application_form", true, true, 0),
            ("contract", true, true, 0),
            ("tax_invoice", false, false, 1),
        ]
    );
    assert_eq!(list[0].label_ko, "사업자등록증");
    assert_eq!(list[1].label_en, "Application form");
    assert_eq!(db.with(|c| repo::projects_of_document(c, "br1")).unwrap().len(), 1);

    assert!(db.with(|c| repo::remove_document(c, &p.id, "ti1")).unwrap());
    assert!(!db.with(|c| repo::remove_document(c, &p.id, "ti1")).unwrap());
    assert_eq!(db.with(|c| repo::get_project(c, &p.id)).unwrap().unwrap().document_count, 1);
    // A deleted library document disappears from the project view (membership row kept).
    db.with(|c| docs::delete_document(c, "br1")).unwrap();
    assert!(db.with(|c| repo::documents_of(c, &p.id)).unwrap().is_empty());
    assert_eq!(db.with(|c| repo::get_project(c, &p.id)).unwrap().unwrap().document_count, 0);

    let detail = detail(&db, &p.id, Language::Ko).unwrap();
    assert_eq!(detail.template.unwrap().id, "tpl_certification_audit");
    assert!(detail.last_run.is_none() && detail.rules.len() == 3);
    assert!(matches!(detail_err(&db), CoreError::NotFound(_)));
}

fn detail_err(db: &Db) -> CoreError {
    detail(db, "nope", Language::Ko).unwrap_err()
}

fn draft(db: &Db, project_id: &str, text: &str) -> RuleDraft {
    define_rule(&deps(db, None), Some(project_id), None, text).unwrap()
}

fn expect_rule(d: &RuleDraft, left: (&str, &str), op: RuleOp, right: Option<(&str, &str)>) {
    assert_eq!(d.rule.op, op, "{}: {:?}", d.rule.description, d);
    assert_eq!(d.rule.left, RuleOperand::field(left.0, left.1), "{}: {:?}", d.rule.description, d);
    match right {
        Some(r) => assert_eq!(d.rule.right, Some(RuleOperand::field(r.0, r.1)), "{}: {:?}", d.rule.description, d),
        None => assert!(d.rule.right.is_none(), "{}: {:?}", d.rule.description, d),
    }
    assert_eq!(d.source, "rules", "{}", d.rule.description);
    assert!(d.confidence >= 0.5 && d.unresolved.is_empty(), "{}: {:?}", d.rule.description, d);
}

#[test]
fn phrase_rules_korean_and_english() {
    let db = db();
    let audit = certification_world(&db);
    let settlement = project(&db, "구매 정산 10월", Some("tpl_purchase_settlement"));

    let d = draft(&db, &audit.id, "세금계산서의 공급자 사업자번호는 사업자등록증 번호와 같아야 해");
    expect_rule(&d, ("tax_invoice", "supplier.brn"), RuleOp::Eq, Some(("business_registration", "registration_no")));
    assert!(d.rule.required && d.rule.tolerance.is_none());

    let d = draft(&db, &settlement.id, "계약금액 = 견적 합계");
    expect_rule(&d, ("contract", "amount"), RuleOp::Eq, Some(("quotation", "grand_total")));
    assert!(
        d.issues.iter().any(|i| i.contains("템플릿에 없는")),
        "contract is outside the settlement template: {:?}",
        d.issues
    );

    let d = draft(&db, &settlement.id, "견적 합계가 세금계산서 공급가액보다 크거나 같아야 해");
    expect_rule(&d, ("quotation", "grand_total"), RuleOp::Gte, Some(("tax_invoice", "supply_total")));

    let d = draft(&db, &settlement.id, "견적 합계와 세금계산서 공급가액의 차이는 5% 이내여야 해");
    expect_rule(&d, ("quotation", "grand_total"), RuleOp::WithinPct, Some(("tax_invoice", "supply_total")));
    assert_eq!(d.rule.tolerance, Some(5.0));

    let d = draft(&db, &settlement.id, "세금계산서 합계금액은 거래명세서 합계와 같아야 한다");
    expect_rule(&d, ("tax_invoice", "grand_total"), RuleOp::Eq, Some(("transaction_statement", "grand_total")));

    let d = draft(&db, &settlement.id, "세금계산서 합계금액은 거래명세서 합계와 1,000원 이내로 같아야 한다");
    expect_rule(&d, ("tax_invoice", "grand_total"), RuleOp::Eq, Some(("transaction_statement", "grand_total")));
    assert_eq!(d.rule.tolerance, Some(1000.0));

    let d = draft(&db, &audit.id, "신청서 서명 필수");
    expect_rule(&d, ("application_form", "signed"), RuleOp::Present, None);

    let d = draft(&db, &audit.id, "신청서에 서명이 있어야 해 (선택)");
    expect_rule(&d, ("application_form", "signed"), RuleOp::Present, None);
    assert!(!d.rule.required, "선택 → optional");

    let d = draft(&db, &audit.id, "사업자등록증 등록번호는 숫자 10자리여야 해");
    assert_eq!(d.rule.op, RuleOp::Matches);
    assert_eq!(d.rule.left, RuleOperand::field("business_registration", "registration_no"));
    assert_eq!(d.rule.right, Some(RuleOperand::literal("^[0-9]{10}$")));
    assert!(d.confidence >= 0.5, "{d:?}");

    let d = draft(&db, &audit.id, "계약서 계약일과 신청서 신청일은 달라야 해");
    expect_rule(&d, ("contract", "contract_date"), RuleOp::Neq, Some(("application_form", "application_date")));

    let d = draft(&db, &audit.id, "계약서 당사자 사업자번호 = 사업자등록증 등록번호");
    expect_rule(&d, ("contract", "parties.brn"), RuleOp::Eq, Some(("business_registration", "registration_no")));

    let d = draft(&db, &audit.id, "세금계산서 공급자 상호는 사업자등록증 상호와 같아야 해");
    expect_rule(&d, ("tax_invoice", "supplier.name"), RuleOp::Eq, Some(("business_registration", "company_name")));

    let d = draft(&db, &audit.id, "세금계산서 공급가액은 0보다 커야 한다");
    assert_eq!((d.rule.op, d.rule.left.clone()), (RuleOp::Gte, RuleOperand::field("tax_invoice", "supply_total")));
    assert_eq!(d.rule.right, Some(RuleOperand::literal("0")));

    // English.
    let d = draft(&db, &audit.id, "The supplier business number on the tax invoice must equal the registration number on the business registration");
    expect_rule(&d, ("tax_invoice", "supplier.brn"), RuleOp::Eq, Some(("business_registration", "registration_no")));

    let d = draft(&db, &audit.id, "The application form must be signed");
    expect_rule(&d, ("application_form", "signed"), RuleOp::Present, None);

    let d = draft(&db, &settlement.id, "Quoted total must be within 5% of the invoiced supply amount");
    expect_rule(&d, ("quotation", "grand_total"), RuleOp::WithinPct, Some(("tax_invoice", "supply_total")));
    assert_eq!(d.rule.tolerance, Some(5.0));

    let d = draft(&db, &settlement.id, "Quotation grand total must be at least the tax invoice supply total");
    expect_rule(&d, ("quotation", "grand_total"), RuleOp::Gte, Some(("tax_invoice", "supply_total")));

    let d = draft(&db, &audit.id, "Contract date and application date must differ");
    expect_rule(&d, ("contract", "contract_date"), RuleOp::Neq, Some(("application_form", "application_date")));

    // Unresolved names stay unresolved (no model): low confidence, issues, names listed.
    let d = draft(&db, &audit.id, "세금계산서 담당자 전화번호는 사업자등록증 팩스번호와 같아야 해");
    assert!(d.confidence < 0.5, "{d:?}");
    assert_eq!(d.rule.op, RuleOp::Eq);
    assert!(!d.unresolved.is_empty() && !d.issues.is_empty(), "{d:?}");
    assert!(d.unresolved.iter().any(|u| u.contains("전화번호")), "{:?}", d.unresolved);
    assert!(d.issues.iter().any(|i| i.contains("모델이 없어")), "{:?}", d.issues);

    let d = draft(&db, &audit.id, "서류를 잘 살펴봐 줘");
    assert!(d.confidence < 0.5 && d.issues.iter().any(|i| i.contains("비교 연산")), "{d:?}");

    let err = define_rule(&deps(&db, None), Some(&audit.id), None, "   ").unwrap_err();
    assert!(matches!(err, CoreError::Invalid(ref m) if m == "description_empty"));

    // Template-only scope (no project) and no scope at all resolve too.
    let d =
        define_rule(&deps(&db, None), None, Some("tpl_purchase_settlement"), "거래명세서 합계 = 세금계산서 합계금액")
            .unwrap();
    expect_rule(&d, ("transaction_statement", "grand_total"), RuleOp::Eq, Some(("tax_invoice", "grand_total")));
    let d = define_rule(&deps(&db, None), None, None, "영수증 합계는 10,000원 이하여야 한다").unwrap();
    assert_eq!((d.rule.op, d.rule.left.clone()), (RuleOp::Lte, RuleOperand::field("receipt", "total")));
    assert_eq!(d.rule.right, Some(RuleOperand::literal("10000")));
}

#[test]
fn model_fallback_with_scripted_chat() {
    let db = db();
    let p = certification_world(&db);
    let chat = StageChat::new();
    chat.push(
        "project_rule",
        json!({
            "left": { "doc_type": "tax_invoice", "field": "supplier.name" },
            "op": "eq",
            "right": { "doc_type": "business_registration", "field": "company_name", "value": null },
            "tolerance": null,
            "required": false
        }),
    );
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None);
    let d = define_rule(&deps(&db, Some(&vlm)), Some(&p.id), None, "두 서류의 거래 상대가 맞는지 봐줘").unwrap();
    assert_eq!(d.source, "model");
    assert_eq!(d.rule.left, RuleOperand::field("tax_invoice", "supplier.name"));
    assert_eq!(d.rule.right, Some(RuleOperand::field("business_registration", "company_name")));
    assert!(!d.rule.required && d.confidence >= 0.7 && d.unresolved.is_empty(), "{d:?}");
    assert_eq!(chat.calls("project_rule"), 1);
    // The grammar restricts types / fields to the project's scope and the operator enum.
    let req = chat.requests.lock().unwrap().last().cloned().unwrap();
    let schema = req.response_format.as_ref().unwrap()["json_schema"]["schema"].clone();
    let types = schema["properties"]["left"]["properties"]["doc_type"]["enum"].as_array().unwrap().clone();
    assert!(types.contains(&json!("tax_invoice")) && types.contains(&json!("application_form")));
    assert!(!types.contains(&json!("receipt")), "{types:?}");
    assert_eq!(schema["properties"]["op"]["enum"].as_array().unwrap().len(), 7);
    let prompt = req.messages[1]["content"].as_str().unwrap();
    assert!(prompt.contains("supplier.brn") && prompt.contains("두 서류의 거래 상대"));

    // A phrase the rules handle never reaches the model.
    let d = define_rule(&deps(&db, Some(&vlm)), Some(&p.id), None, "신청서 서명 필수").unwrap();
    assert_eq!((d.source.as_str(), chat.calls("project_rule")), ("rules", 1));

    // Unknown field in the reply → unresolved, low confidence.
    chat.push(
        "project_rule",
        json!({
            "left": { "doc_type": "tax_invoice", "field": "supplier.fax" },
            "op": "present",
            "right": { "doc_type": "", "field": "", "value": null },
            "tolerance": null,
            "required": true
        }),
    );
    let d = define_rule(&deps(&db, Some(&vlm)), Some(&p.id), None, "팩스 확인").unwrap();
    assert_eq!(d.source, "model");
    assert!(d.confidence < 0.5 && d.unresolved.contains(&"tax_invoice.supplier.fax".to_string()), "{d:?}");

    // A literal right operand and a within_pct without tolerance.
    chat.push(
        "project_rule",
        json!({
            "left": { "doc_type": "tax_invoice", "field": "grand_total" },
            "op": "within_pct",
            "right": { "doc_type": "", "field": "", "value": "1100000" },
            "tolerance": null,
            "required": true
        }),
    );
    let d = define_rule(&deps(&db, Some(&vlm)), Some(&p.id), None, "합계가 백십만원 근처인지").unwrap();
    assert_eq!(d.rule.right, Some(RuleOperand::literal("1100000")));
    assert_eq!(d.rule.tolerance, Some(5.0));
    assert!(d.issues.iter().any(|i| i.contains("5%")));

    // Model failure falls back to the phrase result with an issue.
    *chat.fail_with.lock().unwrap() = Some("boom".into());
    let d = define_rule(&deps(&db, Some(&vlm)), Some(&p.id), None, "서류를 잘 살펴봐 줘").unwrap();
    assert_eq!(d.source, "rules");
    assert!(d.confidence < 0.5 && d.issues.iter().any(|i| i.contains("모델 변환에 실패")), "{d:?}");
}

#[test]
fn check_run_verdicts_with_evidence() {
    let db = db();
    let p = certification_world(&db);
    // Template rules: contract BRN (pass), invoice BRN (pass), application signed (missing).
    add_rule(
        &db,
        &p.id,
        "계약금액 = 세금계산서 합계금액",
        RuleOperand::field("contract", "amount"),
        RuleOp::Eq,
        Some(RuleOperand::field("tax_invoice", "grand_total")),
        None,
    );
    add_rule(
        &db,
        &p.id,
        "계약금액 ≈ 세금계산서 합계 (10%)",
        RuleOperand::field("tax_invoice", "grand_total"),
        RuleOp::WithinPct,
        Some(RuleOperand::field("contract", "amount")),
        Some(10.0),
    );
    add_rule(
        &db,
        &p.id,
        "계약금액 ≈ 세금계산서 합계 (5%)",
        RuleOperand::field("tax_invoice", "grand_total"),
        RuleOp::WithinPct,
        Some(RuleOperand::field("contract", "amount")),
        Some(5.0),
    );
    add_rule(
        &db,
        &p.id,
        "등록번호 10자리",
        RuleOperand::field("business_registration", "registration_no"),
        RuleOp::Matches,
        Some(RuleOperand::literal("^[0-9]{10}$")),
        None,
    );
    add_rule(
        &db,
        &p.id,
        "세액 = 100,000",
        RuleOperand::field("tax_invoice", "tax_total"),
        RuleOp::Eq,
        Some(RuleOperand::literal("100,000")),
        None,
    );
    add_rule(
        &db,
        &p.id,
        "공급가액 = 1,000,000",
        RuleOperand::field("tax_invoice", "supply_total"),
        RuleOp::Eq,
        Some(RuleOperand::literal("1,000,000원")),
        None,
    );
    add_rule(
        &db,
        &p.id,
        "공급자 상호 = 사업자등록증 상호",
        RuleOperand::field("tax_invoice", "supplier.name"),
        RuleOp::Eq,
        Some(RuleOperand::field("business_registration", "company_name")),
        None,
    );
    add_rule(&db, &p.id, "계약서 서명 필수", RuleOperand::field("contract", "signed"), RuleOp::Present, None, None);
    add_rule(
        &db,
        &p.id,
        "계약일 ≤ 세금계산서 작성일",
        RuleOperand::field("contract", "contract_date"),
        RuleOp::Lte,
        Some(RuleOperand::field("tax_invoice", "issue_date")),
        None,
    );
    add_rule(
        &db,
        &p.id,
        "세금계산서 비고 필수",
        RuleOperand::field("tax_invoice", "remarks"),
        RuleOp::Present,
        None,
        None,
    );
    let disabled =
        add_rule(&db, &p.id, "비활성", RuleOperand::field("tax_invoice", "remarks"), RuleOp::Present, None, None);
    let mut off = disabled.clone();
    off.enabled = false;
    db.with(|c| repo::save_rule(c, &p.id, &off)).unwrap();

    let run = check(&deps(&db, None), &p.id, &[]).unwrap();
    let by = |desc: &str| {
        run.results.iter().find(|r| r.description == desc).unwrap_or_else(|| panic!("{desc}: {:?}", run.results))
    };

    let contract_brn = by("계약서 당사자 사업자번호 = 사업자등록증 등록번호");
    assert_eq!(contract_brn.verdict, Verdict::Pass, "{}", contract_brn.message);
    assert_eq!(
        contract_brn.left.as_ref().unwrap().field.as_deref(),
        Some("parties[0].brn"),
        "the matching row is the evidence"
    );
    assert_eq!(contract_brn.left.as_ref().unwrap().doc_id.as_deref(), Some("c1"));
    assert_eq!(contract_brn.right.as_ref().unwrap().normalized.as_deref(), Some("123-45-67890"));
    assert_eq!(by("세금계산서 공급자 사업자번호 = 사업자등록증 등록번호").verdict, Verdict::Pass);

    let signed = by("신청서 서명 필수");
    assert_eq!(signed.verdict, Verdict::Missing);
    assert!(signed.message.contains("신청서"), "{}", signed.message);
    assert_eq!(signed.left.as_ref().unwrap().status.as_deref(), Some("missing_document"));

    let amount = by("계약금액 = 세금계산서 합계금액");
    assert_eq!(amount.verdict, Verdict::Mismatch);
    let l = amount.left.as_ref().unwrap();
    let r = amount.right.as_ref().unwrap();
    assert_eq!(
        (l.doc_id.as_deref(), l.field.as_deref(), l.normalized.as_deref(), l.page),
        (Some("c1"), Some("amount"), Some("1000000"), Some(1))
    );
    assert_eq!(
        (r.doc_id.as_deref(), r.field.as_deref(), r.normalized.as_deref()),
        (Some("ti1"), Some("grand_total"), Some("1100000"))
    );
    assert!(amount.message.contains('≠'), "{}", amount.message);
    assert_eq!(l.label.as_deref(), Some("계약금액"));

    assert_eq!(by("계약금액 ≈ 세금계산서 합계 (10%)").verdict, Verdict::Pass);
    let pct = by("계약금액 ≈ 세금계산서 합계 (5%)");
    assert_eq!(pct.verdict, Verdict::Mismatch);
    assert!(pct.message.contains("10.0%"), "{}", pct.message);

    let m = by("등록번호 10자리");
    assert_eq!(m.verdict, Verdict::Pass, "{}", m.message);
    assert_eq!(m.right.as_ref().unwrap().value.as_deref(), Some("^[0-9]{10}$"));

    let pending = by("세액 = 100,000");
    assert_eq!(pending.verdict, Verdict::Undetermined, "{}", pending.message);
    assert_eq!(pending.left.as_ref().unwrap().status.as_deref(), Some("pending"));
    assert!(pending.message.contains("검토 대기"), "{}", pending.message);

    let literal = by("공급가액 = 1,000,000");
    assert_eq!(literal.verdict, Verdict::Pass, "{}", literal.message);
    assert_eq!(literal.right.as_ref().unwrap().normalized.as_deref(), Some("1000000"));

    let name = by("공급자 상호 = 사업자등록증 상호");
    assert_eq!(name.verdict, Verdict::Pass, "㈜ is ignored: {}", name.message);

    assert_eq!(by("계약서 서명 필수").verdict, Verdict::Pass);
    assert_eq!(by("계약일 ≤ 세금계산서 작성일").verdict, Verdict::Pass);
    let remarks = by("세금계산서 비고 필수");
    assert_eq!(remarks.verdict, Verdict::Mismatch, "an absent field fails `present`: {}", remarks.message);
    assert_eq!(remarks.left.as_ref().unwrap().status.as_deref(), Some("absent"));
    assert!(run.results.iter().all(|r| r.description != "비활성"), "disabled rules are skipped");

    assert_eq!((run.pass, run.mismatch, run.undetermined, run.missing), (8, 3, 1, 1));
    assert_eq!(run.results.len(), 13);
    assert_eq!(run.missing_doc_types, vec!["application_form"]);
    assert!(run.results.iter().all(|r| r.run_id == run.id && !r.id.is_empty()));

    // Persisted and reflected on the project.
    let stored = db.with(|c| repo::last_run(c, &p.id)).unwrap().unwrap();
    assert_eq!(stored.id, run.id);
    assert_eq!(stored.results.len(), run.results.len());
    assert_eq!(stored.results[0].left, run.results[0].left);
    assert_eq!(stored.missing_doc_types, run.missing_doc_types);
    let p2 = db.with(|c| repo::get_project(c, &p.id)).unwrap().unwrap();
    assert!(p2.last_check_at.is_some());
    assert_eq!((p2.last_pass, p2.last_mismatch, p2.last_undetermined), (8, 3, 1));

    // Subset of rules, second run becomes the latest.
    let subset = check(&deps(&db, None), &p.id, std::slice::from_ref(&contract_brn.rule_id)).unwrap();
    assert_eq!(subset.results.len(), 1);
    assert_eq!(db.with(|c| repo::list_runs(c, &p.id, 10)).unwrap().len(), 2);
    assert_eq!(db.with(|c| repo::last_run(c, &p.id)).unwrap().unwrap().id, subset.id);
    assert!(matches!(check(&deps(&db, None), "nope", &[]).unwrap_err(), CoreError::NotFound(_)));
}

#[test]
fn several_documents_of_one_type_report_the_worst_pair() {
    let db = db();
    seed_br(&db, "br1", "123-45-67890", "한빛상사");
    seed_invoice(&db, "ti1", "123-45-67890", "한빛상사", "1,000,000", "1,100,000", false);
    seed_invoice(&db, "ti2", "999-88-77777", "다른상사", "500,000", "550,000", false);
    let p = project(&db, "다중", None);
    add(&db, &p.id, &["br1", "ti1", "ti2"]);
    add_rule(
        &db,
        &p.id,
        "공급자 BRN",
        RuleOperand::field("tax_invoice", "supplier.brn"),
        RuleOp::Eq,
        Some(RuleOperand::field("business_registration", "registration_no")),
        None,
    );
    let run = check(&deps(&db, None), &p.id, &[]).unwrap();
    let r = &run.results[0];
    assert_eq!(r.verdict, Verdict::Mismatch);
    assert_eq!(r.left.as_ref().unwrap().doc_id.as_deref(), Some("ti2"), "the offending invoice is the evidence");
    assert_eq!(r.left.as_ref().unwrap().normalized.as_deref(), Some("999-88-77777"));
    assert!(run.missing_doc_types.is_empty(), "no template → nothing required");

    // Without a model, differing company names stay undetermined; the model decides variants.
    seed_invoice(&db, "ti3", "123-45-67890", "Hanbit Corp", "1,000,000", "1,100,000", false);
    let q = project(&db, "명칭", None);
    add(&db, &q.id, &["br1", "ti3"]);
    add_rule(
        &db,
        &q.id,
        "공급자 상호",
        RuleOperand::field("tax_invoice", "supplier.name"),
        RuleOp::Eq,
        Some(RuleOperand::field("business_registration", "company_name")),
        None,
    );
    let open = check(&deps(&db, None), &q.id, &[]).unwrap();
    assert_eq!(open.results[0].verdict, Verdict::Undetermined, "{}", open.results[0].message);
    assert!(open.results[0].message.contains("모델 없음"));

    let chat = StageChat::new();
    chat.push("name_match", json!({ "same": true }));
    let vlm = Vlm::new(&chat, "scripted".into(), Profile::Balanced, None);
    let judged = check(&deps(&db, Some(&vlm)), &q.id, &[]).unwrap();
    assert_eq!(judged.results[0].verdict, Verdict::Pass, "{}", judged.results[0].message);
    assert!(judged.results[0].message.contains("모델 판정"));
    assert_eq!(chat.calls("name_match"), 1);
    let prompt = chat.requests.lock().unwrap()[0].messages[1]["content"].as_str().unwrap().to_string();
    assert!(prompt.contains("Hanbit Corp") && prompt.contains("한빛상사"));
    chat.push("name_match", json!({ "same": false }));
    let judged = check(&deps(&db, Some(&vlm)), &q.id, &[]).unwrap();
    assert_eq!(judged.results[0].verdict, Verdict::Mismatch);
    // Equal names never ask the model.
    add_rule(
        &db,
        &q.id,
        "등록번호",
        RuleOperand::field("business_registration", "registration_no"),
        RuleOp::Eq,
        Some(RuleOperand::field("tax_invoice", "supplier.brn")),
        None,
    );
    let calls = chat.calls("name_match");
    check(&deps(&db, Some(&vlm)), &q.id, &[]).unwrap();
    assert_eq!(chat.calls("name_match"), calls + 1, "only the name rule asks");
}

#[test]
fn same_type_rules_compare_each_document_with_itself() {
    let db = db();
    // ti1 is a normal invoice; ti2 is self-billed (supplier = buyer) with smaller amounts.
    seed_invoice(&db, "ti1", "123-45-67890", "한빛상사", "1,000,000", "1,100,000", false);
    seed_invoice(&db, "ti2", "220-81-62517", "주식회사 도큐", "500,000", "550,000", false);
    let p = project(&db, "동일 유형", None);
    add(&db, &p.id, &["ti1", "ti2"]);
    // Cross-document pairing would compare ti1's supply total with ti2's smaller grand total.
    add_rule(
        &db,
        &p.id,
        "공급가액 ≤ 합계",
        RuleOperand::field("tax_invoice", "supply_total"),
        RuleOp::Lte,
        Some(RuleOperand::field("tax_invoice", "grand_total")),
        None,
    );
    add_rule(
        &db,
        &p.id,
        "공급자 ≠ 공급받는자",
        RuleOperand::field("tax_invoice", "supplier.brn"),
        RuleOp::Neq,
        Some(RuleOperand::field("tax_invoice", "buyer.brn")),
        None,
    );
    let run = check(&deps(&db, None), &p.id, &[]).unwrap();
    assert_eq!(run.results[0].verdict, Verdict::Pass, "{}", run.results[0].message);
    let self_billed = &run.results[1];
    assert_eq!(self_billed.verdict, Verdict::Mismatch, "{}", self_billed.message);
    assert_eq!(self_billed.left.as_ref().unwrap().doc_id.as_deref(), Some("ti2"));
    assert_eq!(self_billed.right.as_ref().unwrap().doc_id.as_deref(), Some("ti2"), "same document on both sides");
    assert_eq!((run.pass, run.mismatch), (1, 1));
}

#[test]
fn present_on_pending_boolean_is_undetermined_and_false_fails() {
    let db = db();
    seed_application(&db, "ap1", Some(("예", true)));
    seed_contract(&db, "c2", "123-45-67890", "한빛상사", "1,000,000", Some("아니오"));
    let p = project(&db, "서명", None);
    add(&db, &p.id, &["ap1", "c2"]);
    add_rule(&db, &p.id, "신청서 서명", RuleOperand::field("application_form", "signed"), RuleOp::Present, None, None);
    add_rule(&db, &p.id, "계약서 서명", RuleOperand::field("contract", "signed"), RuleOp::Present, None, None);
    let run = check(&deps(&db, None), &p.id, &[]).unwrap();
    assert_eq!(run.results[0].verdict, Verdict::Undetermined, "{}", run.results[0].message);
    assert_eq!(run.results[1].verdict, Verdict::Mismatch, "{}", run.results[1].message);
    assert!(run.results[1].message.contains("아니오"));
}

#[test]
fn relations_link_shared_entities() {
    let db = db();
    let p = certification_world(&db);
    seed_application(&db, "ap1", None);
    add(&db, &p.id, &["ap1"]);
    let g = relations(&db, &p.id).unwrap();
    assert_eq!(g.nodes.iter().map(|n| n.doc_id.as_str()).collect::<Vec<_>>(), vec!["br1", "ti1", "c1", "ap1"]);
    assert_eq!(g.nodes[0].label, "br1_business_registration.pdf");
    let edges: Vec<(&str, &str, &str, &str)> =
        g.edges.iter().map(|e| (e.from.as_str(), e.to.as_str(), e.kind.as_str(), e.value.as_str())).collect();
    assert!(edges.contains(&("br1", "ti1", "brn", "1234567890")), "{edges:?}");
    assert!(edges.contains(&("br1", "c1", "brn", "1234567890")), "{edges:?}");
    assert!(edges.contains(&("ti1", "c1", "brn", "1234567890")), "{edges:?}");
    assert!(edges.contains(&("br1", "ti1", "name", "한빛상사")), "㈜ stripped: {edges:?}");
    assert!(edges.contains(&("br1", "c1", "name", "한빛상사")), "party row name: {edges:?}");
    assert!(edges.contains(&("ti1", "c1", "amount", "1000000")), "supply total = contract amount: {edges:?}");
    assert!(!edges.iter().any(|e| e.3 == "1100000"), "the grand total matches nothing: {edges:?}");
    assert!(
        edges.contains(&("ti1", "c1", "brn", "2208162517")) && edges.contains(&("ti1", "c1", "name", "도큐")),
        "buyer ↔ party 을: {edges:?}"
    );
    assert!(!edges.iter().any(|e| e.0 == "ap1" || e.3 == "ap1"), "application shares nothing: {edges:?}");
    // Unique and ordered.
    let mut sorted = edges.clone();
    sorted.dedup();
    assert_eq!(sorted.len(), edges.len());
    let order: Vec<(usize, usize)> = g
        .edges
        .iter()
        .map(|e| {
            let idx = |id: &str| g.nodes.iter().position(|n| n.doc_id == id).unwrap();
            (idx(&e.from), idx(&e.to))
        })
        .collect();
    let mut expected = order.clone();
    expected.sort();
    assert_eq!(order, expected);
    let brn_edge = g.edges.iter().find(|e| e.from == "ti1" && e.to == "c1" && e.kind == "brn").unwrap();
    assert_eq!(
        (brn_edge.from_field.as_deref(), brn_edge.to_field.as_deref()),
        (Some("supplier.brn"), Some("parties[0].brn"))
    );

    // Amount / date edges through document-level values and equal totals.
    seed_invoice(&db, "ti9", "555-55-55555", "다른상사", "1,000,000", "1,100,000", false);
    let q = project(&db, "금액", None);
    add(&db, &q.id, &["ti1", "ti9"]);
    let g = relations(&db, &q.id).unwrap();
    let kinds: Vec<&str> = g.edges.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.contains(&"amount") && kinds.contains(&"date"), "{:?}", g.edges);
    assert_eq!(g.edges.iter().filter(|e| e.kind == "amount").count(), 3, "supply, tax and grand totals: {:?}", g.edges);
    assert_eq!(relations(&db, "nope").unwrap(), RelationGraph::default());
}

#[test]
fn report_markdown_lists_documents_checklist_and_verdicts() {
    let db = db();
    let p = certification_world(&db);
    add_rule(
        &db,
        &p.id,
        "계약금액 = 세금계산서 합계금액",
        RuleOperand::field("contract", "amount"),
        RuleOp::Eq,
        Some(RuleOperand::field("tax_invoice", "grand_total")),
        None,
    );
    let before = report(&db, &p.id, Language::Ko).unwrap();
    assert!(before.contains("검사를 아직 실행하지 않았습니다"));
    assert!(before.contains("- [ ] 신청서 — 누락") && before.contains("- [x] 사업자등록증 (1건)"));
    assert!(before.contains("정합성 검사 실행"));
    check(&deps(&db, None), &p.id, &[]).unwrap();
    let md = report(&db, &p.id, Language::Ko).unwrap();
    assert!(md.starts_with("# 한빛 인증 심사\n"));
    assert!(md.contains("접수번호: 2026-0042") && md.contains("담당: 박담당") && md.contains("기한: 2026-11-30"));
    assert!(md.contains("| 세금계산서 | ti1_tax_invoice.pdf | 2026-09-30 | 검토 대기 |"), "{md}");
    assert!(md.contains("| 계약금액 = 세금계산서 합계금액 | 불일치 |"), "{md}");
    assert!(md.contains("| 통과 |") && md.contains("| 누락 (") || md.contains("| 누락 |"), "{md}");
    assert!(md.contains("c1_contract.pdf 계약금액 1000000 (p.1)"), "{md}");
    assert!(md.contains("## 문서 간 관계") && md.contains("↔") && md.contains("사업자번호 1234567890"), "{md}");
    assert!(md.contains("불일치 1건 확인 후 보완요청서 생성"), "{md}");
    assert!(md.contains("누락 문서 1건(신청서) 보완 요청"), "{md}");
    let en = report(&db, &p.id, Language::En).unwrap();
    assert!(en.contains("## Consistency check") && en.contains("| Mismatch |") && en.contains("Business registration"));
    assert!(matches!(report(&db, "nope", Language::Ko).unwrap_err(), CoreError::NotFound(_)));
}

#[test]
fn supplement_request_lists_findings_as_text_sources() {
    let db = db();
    let p = certification_world(&db);
    add_rule(
        &db,
        &p.id,
        "계약금액 = 세금계산서 합계금액",
        RuleOperand::field("contract", "amount"),
        RuleOp::Eq,
        Some(RuleOperand::field("tax_invoice", "grand_total")),
        None,
    );
    add_rule(
        &db,
        &p.id,
        "세금계산서 비고 필수",
        RuleOperand::field("tax_invoice", "remarks"),
        RuleOp::Present,
        None,
        None,
    );
    let run = check(&deps(&db, None), &p.id, &[]).unwrap();
    let req = supplement_request(&db, &p.id, &run, &[], OutputFormat::Hwpx, Language::Ko).unwrap();
    assert_eq!(req.form_ref.as_deref(), Some("supplement_request"));
    assert_eq!(req.output, OutputFormat::Hwpx);
    assert!(!req.use_knowledge && req.language == Language::Ko);
    assert!(
        req.content.contains("한빛 인증 심사")
            && req.content.contains("불일치 2건")
            && req.content.contains("누락 1건"),
        "{}",
        req.content
    );
    let texts: Vec<(String, Value)> = req
        .sources
        .iter()
        .map(|s| match s {
            SourceRef::Text { title, text } => (title.clone().unwrap(), serde_json::from_str::<Value>(text).unwrap()),
            other => panic!("unexpected source {other:?}"),
        })
        .collect();
    assert_eq!(texts.len(), 4, "{texts:?}");
    assert_eq!(texts[0].0, "finding:missing:missing_doc:application_form");
    assert_eq!(texts[0].1["expected"], "신청서 문서 제출");
    let mismatch = texts.iter().find(|(t, _)| t.starts_with("finding:mismatch:")).unwrap();
    let f = &mismatch.1;
    for key in ["rule", "verdict", "expected", "found", "doc_name", "page", "message"] {
        assert!(f.get(key).is_some(), "{key} missing in {f}");
    }
    assert_eq!(f["verdict"], "mismatch");
    assert_eq!(f["rule"], "계약금액 = 세금계산서 합계금액");
    assert_eq!(f["expected"], "ti1_tax_invoice.pdf 합계금액 1100000");
    assert_eq!(f["found"], "c1_contract.pdf 계약금액 1000000");
    assert_eq!((f["doc_name"].as_str(), f["page"].as_u64()), (Some("c1_contract.pdf"), Some(1)));
    let present = texts.iter().find(|(_, v)| v["rule"] == "세금계산서 비고 필수").unwrap();
    assert_eq!(present.1["expected"], "비고 기재");
    assert_eq!(present.1["found"], "없음");
    let project = texts.iter().find(|(t, _)| t == "project").unwrap();
    assert_eq!(project.1["name"], "한빛 인증 심사");
    assert_eq!(project.1["intake_no"], "2026-0042");
    assert_eq!(project.1["documents"].as_array().unwrap().len(), 3);
    assert_eq!(project.1["findings"]["mismatch"], 2);
    // The template's `missing` rule result is folded into the missing-document finding.
    assert!(!texts.iter().any(|(t, _)| t.starts_with("finding:missing:") && !t.contains("missing_doc:")), "{texts:?}");

    // Selecting one finding.
    let id = mismatch.0.trim_start_matches("finding:mismatch:").to_string();
    let one = supplement_request(&db, &p.id, &run, &[id], OutputFormat::Pdf, Language::En).unwrap();
    assert_eq!(one.sources.len(), 2);
    assert!(one.content.starts_with("Supplement request for project"));
    let missing_only = supplement_request(
        &db,
        &p.id,
        &run,
        &["missing_doc:application_form".into()],
        OutputFormat::Docx,
        Language::Ko,
    )
    .unwrap();
    assert_eq!(missing_only.sources.len(), 2);
    let none = supplement_request(&db, &p.id, &run, &["nope".into()], OutputFormat::Docx, Language::Ko).unwrap_err();
    assert!(matches!(none, CoreError::Invalid(ref m) if m == "no_findings"));
}

fn insert_inbox_doc(db: &Db, id: &str, rel_path: &str) {
    db.with(|c| {
        docs::insert_document(
            c,
            &NewDocument {
                id: id.into(),
                rel_path: rel_path.into(),
                original_name: rel_path.rsplit('/').next().unwrap().into(),
                source_kind: SourceKind::Pdf,
                content_hash: format!("hash-{id}"),
                size_bytes: 1,
                file_modified_at: None,
                role: DocumentRole::Work,
                import_batch_id: None,
            },
        )
    })
    .unwrap();
}

#[test]
fn inbox_subfolders_map_to_projects() {
    assert_eq!(repo::inbox_folder_of("inbox/한빛/a.pdf").as_deref(), Some("한빛"));
    assert_eq!(repo::inbox_folder_of("inbox/한빛/2026/a.pdf").as_deref(), Some("한빛"));
    assert_eq!(repo::inbox_folder_of("inbox/a.pdf"), None);
    assert_eq!(repo::inbox_folder_of("documents/한빛/a.pdf"), None);
    assert_eq!(repo::inbox_folder_of("inbox/\u{1112}\u{1161}\u{11ab}/a.pdf").as_deref(), Some("한"), "NFC");

    let db = db();
    insert_inbox_doc(&db, "ib1", "inbox/한빛/a.pdf");
    insert_inbox_doc(&db, "ib2", "inbox/한빛/b.pdf");
    insert_inbox_doc(&db, "ib3", "inbox/c.pdf");
    insert_inbox_doc(&db, "ib4", "inbox/새폴더/d.pdf");
    let pid = assign_inbox_document(&db, "ib1", "inbox/한빛/a.pdf", true).unwrap().unwrap();
    let p = db.with(|c| repo::project_by_name(c, "한빛")).unwrap().unwrap();
    assert_eq!((p.id.as_str(), p.inbox_folder.as_deref(), p.document_count), (pid.as_str(), Some("한빛"), 1));
    assert_eq!(assign_inbox_document(&db, "ib2", "inbox/한빛/b.pdf", false).unwrap().as_deref(), Some(pid.as_str()));
    assert_eq!(db.with(|c| repo::get_project(c, &pid)).unwrap().unwrap().document_count, 2);
    assert_eq!(
        assign_inbox_document(&db, "ib3", "inbox/c.pdf", true).unwrap(),
        None,
        "files directly in inbox/ are not mapped"
    );
    assert_eq!(assign_inbox_document(&db, "ib4", "inbox/새폴더/d.pdf", false).unwrap(), None);
    assert_eq!(db.with(repo::list_projects).unwrap().len(), 1, "no project created without `create`");
    // A project already named like the folder is mapped and reused instead of a duplicate.
    let named = project(&db, "새 폴더", None);
    assert_eq!(named.inbox_folder, None);
    assert_eq!(
        assign_inbox_document(&db, "ib4", "inbox/새폴더/d.pdf", true).unwrap().as_deref(),
        Some(named.id.as_str())
    );
    assert_eq!(db.with(repo::list_projects).unwrap().len(), 2);
    let named = db.with(|c| repo::get_project(c, &named.id)).unwrap().unwrap();
    assert_eq!((named.inbox_folder.as_deref(), named.document_count), (Some("새폴더"), 1));
    assert_eq!(
        assign_inbox_document(&db, "unknown", "inbox/한빛/z.pdf", true).unwrap(),
        None,
        "unknown documents are not members"
    );
}

#[test]
fn normalisation_helpers() {
    use check::{normalize, normalize_name, Norm};
    assert_eq!(normalize_name("㈜한빛 상사"), "한빛상사");
    assert_eq!(normalize_name("(주) 한빛상사"), "한빛상사");
    assert_eq!(normalize_name("주식회사 한빛상사"), "한빛상사");
    assert_eq!(normalize_name("Hanbit Co., Ltd."), "hanbit");
    assert_eq!(normalize(FieldKind::Brn, "123-45-67890", None), Some(Norm::Digits("1234567890".into())));
    assert_eq!(normalize(FieldKind::Amount, "1,250,000원", None), Some(Norm::Number(1_250_000.0)));
    assert_eq!(normalize(FieldKind::Amount, "x", Some("12")), Some(Norm::Number(12.0)));
    assert_eq!(normalize(FieldKind::Date, "2026년 1월 3일", None), Some(Norm::Date("2026-01-03".into())));
    assert_eq!(normalize(FieldKind::Boolean, "예", None), Some(Norm::Bool(true)));
    assert_eq!(normalize(FieldKind::Boolean, "서명 있음", None), Some(Norm::Text("서명있음".into())));
    assert_eq!(normalize(FieldKind::String, "  ", None), None);
    assert_eq!(Norm::Digits("1234567890".into()).display(), "123-45-67890");
}
