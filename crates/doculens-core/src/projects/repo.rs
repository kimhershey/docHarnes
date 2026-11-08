//! Project storage (`0005_projects.sql`): templates, projects, membership,
//! rules and check runs. Functions take a connection so the shell composes
//! them inside `db.with(...)` / `db.with_tx(...)`; expected failures surface as
//! `rusqlite::Error::InvalidParameterName(code)` (typed UI errors), the same
//! convention as `pipeline::repo`. Counters shown on the board
//! (`document_count`, `missing_count`, `last_*`) are computed from the
//! membership / run tables, never stored.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::db::Db;
use crate::documents::{self, repo as docs};
use crate::error::{CoreError, Result};
use crate::pipeline::repo::{self as pipeline_repo, typed};
use crate::settings::Language;

use super::{
    relations, CheckRun, ChecklistItem, Evidence, Project, ProjectDetail, ProjectDocument, ProjectInput, ProjectStatus,
    ProjectTemplate, Rule, RuleOp, RuleOperand, RuleResult, TemplateInput, Verdict,
};

fn ts(d: &DateTime<Utc>) -> String {
    d.to_rfc3339()
}

fn parse_ts(s: Option<String>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc)))
}

fn to_json<T: Serialize>(v: &T) -> rusqlite::Result<String> {
    serde_json::to_string(v).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn from_json<T: DeserializeOwned + Default>(s: Option<String>) -> T {
    s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn blank(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Document-type labels
// ---------------------------------------------------------------------------

/// Korean / English labels of document types that §3.15 templates refer to before a
/// schema with that slug is registered (the registered schema's names win once it exists).
pub fn known_type_label(slug: &str) -> Option<(&'static str, &'static str)> {
    const TABLE: &[(&str, &str, &str)] = &[
        ("tax_invoice", "세금계산서", "Tax invoice"),
        ("invoice_exempt", "계산서(면세)", "Invoice (tax-exempt)"),
        ("receipt", "영수증", "Receipt"),
        ("business_registration", "사업자등록증", "Business registration"),
        ("contract", "계약서", "Contract"),
        ("application_form", "신청서", "Application form"),
        ("general", "일반", "General"),
        ("quotation", "견적서", "Quotation"),
        ("transaction_statement", "거래명세서", "Transaction statement"),
        ("financial_statement", "재무제표", "Financial statement"),
        ("purchase_order", "발주서", "Purchase order"),
        ("official_letter", "공문", "Official letter"),
        ("report", "보고서", "Report"),
        ("minutes", "회의록", "Minutes"),
        ("supplement_request", "보완요청서", "Supplement request"),
    ];
    TABLE.iter().find(|(s, _, _)| *s == slug).map(|(_, ko, en)| (*ko, *en))
}

/// `(slug, label_ko, label_en)` of every registered document type (enabled schemas).
pub fn doc_type_labels(conn: &Connection) -> rusqlite::Result<Vec<(String, String, String)>> {
    Ok(pipeline_repo::doc_types(conn)?.into_iter().map(|d| (d.slug, d.label_ko, d.label_en)).collect())
}

/// Label of a document type in one language: registered schema name → built-in table → slug.
pub fn type_label(labels: &[(String, String, String)], slug: &str, ko: bool) -> String {
    if let Some((_, l_ko, l_en)) = labels.iter().find(|(s, _, _)| s == slug) {
        return if ko { l_ko.clone() } else { l_en.clone() };
    }
    match known_type_label(slug) {
        Some((l_ko, l_en)) => (if ko { l_ko } else { l_en }).to_string(),
        None => slug.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Built-in templates (§3.15 examples)
// ---------------------------------------------------------------------------

fn rule(
    id: &str,
    description: &str,
    left: RuleOperand,
    op: RuleOp,
    right: Option<RuleOperand>,
    tolerance: Option<f64>,
    required: bool,
) -> Rule {
    Rule { id: id.into(), description: description.into(), left, op, right, tolerance, required, enabled: true }
}

fn template(
    id: &str,
    names: (&str, &str),
    descriptions: (&str, &str),
    required_doc_types: &[&str],
    rules: Vec<Rule>,
) -> ProjectTemplate {
    let epoch = DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default();
    ProjectTemplate {
        id: id.into(),
        name_ko: names.0.into(),
        name_en: names.1.into(),
        description_ko: descriptions.0.into(),
        description_en: descriptions.1.into(),
        required_doc_types: required_doc_types.iter().map(|s| s.to_string()).collect(),
        rules,
        builtin: true,
        created_at: epoch,
        updated_at: epoch,
    }
}

/// The bundled templates: 인증 심사 · 거래처 등록 · 구매 정산. Field keys follow the
/// built-in extraction schemas (`pipeline::schema::builtins`); 견적서 / 거래명세서 use the
/// same `supplier.*` / `*_total` vocabulary and resolve through key aliases at check time.
pub fn builtin_templates() -> Vec<ProjectTemplate> {
    use RuleOperand as O;
    vec![
        template(
            "tpl_certification_audit",
            ("인증 심사", "Certification audit"),
            (
                "사업자등록증·신청서·계약서 필수(재무제표 선택). 계약서·세금계산서의 사업자번호가 사업자등록증과 일치하는지, 신청서에 서명이 있는지 검사합니다.",
                "Business registration, application form and contract required (financial statement optional). Checks that the contract / tax-invoice business numbers match the registration and that the application is signed.",
            ),
            &["business_registration", "application_form", "contract"],
            vec![
                rule(
                    "tpl_certification_audit.contract_brn",
                    "계약서 당사자 사업자번호 = 사업자등록증 등록번호",
                    O::field("contract", "parties.brn"),
                    RuleOp::Eq,
                    Some(O::field("business_registration", "registration_no")),
                    None,
                    true,
                ),
                rule(
                    "tpl_certification_audit.invoice_brn",
                    "세금계산서 공급자 사업자번호 = 사업자등록증 등록번호",
                    O::field("tax_invoice", "supplier.brn"),
                    RuleOp::Eq,
                    Some(O::field("business_registration", "registration_no")),
                    None,
                    false,
                ),
                rule(
                    "tpl_certification_audit.application_signed",
                    "신청서 서명 필수",
                    O::field("application_form", "signed"),
                    RuleOp::Present,
                    None,
                    None,
                    true,
                ),
            ],
        ),
        template(
            "tpl_vendor_registration",
            ("거래처 등록", "Vendor registration"),
            (
                "사업자등록증·견적서 필수. 견적서의 공급자 사업자번호·상호가 사업자등록증과 일치하는지 검사합니다.",
                "Business registration and quotation required. Checks that the quotation's supplier business number and name match the registration.",
            ),
            &["business_registration", "quotation"],
            vec![
                rule(
                    "tpl_vendor_registration.registration_no",
                    "사업자등록증 등록번호 필수",
                    O::field("business_registration", "registration_no"),
                    RuleOp::Present,
                    None,
                    None,
                    true,
                ),
                rule(
                    "tpl_vendor_registration.quotation_brn",
                    "견적서 공급자 사업자번호 = 사업자등록증 등록번호",
                    O::field("quotation", "supplier.brn"),
                    RuleOp::Eq,
                    Some(O::field("business_registration", "registration_no")),
                    None,
                    true,
                ),
                rule(
                    "tpl_vendor_registration.quotation_name",
                    "견적서 공급자 상호 = 사업자등록증 상호",
                    O::field("quotation", "supplier.name"),
                    RuleOp::Eq,
                    Some(O::field("business_registration", "company_name")),
                    None,
                    false,
                ),
            ],
        ),
        template(
            "tpl_purchase_settlement",
            ("구매 정산", "Purchase settlement"),
            (
                "견적서·거래명세서·세금계산서 필수. 세금계산서 합계가 거래명세서 합계와 같고, 견적 합계가 세금계산서 공급가액 이상(5% 이내)인지 검사합니다.",
                "Quotation, transaction statement and tax invoice required. Checks that the tax-invoice total equals the statement total and that the quoted total is at least the invoiced supply amount (within 5%).",
            ),
            &["quotation", "transaction_statement", "tax_invoice"],
            vec![
                rule(
                    "tpl_purchase_settlement.invoice_statement_total",
                    "세금계산서 합계금액 = 거래명세서 합계",
                    O::field("tax_invoice", "grand_total"),
                    RuleOp::Eq,
                    Some(O::field("transaction_statement", "grand_total")),
                    None,
                    true,
                ),
                rule(
                    "tpl_purchase_settlement.quote_gte_supply",
                    "견적 합계 ≥ 세금계산서 공급가액",
                    O::field("quotation", "grand_total"),
                    RuleOp::Gte,
                    Some(O::field("tax_invoice", "supply_total")),
                    None,
                    true,
                ),
                rule(
                    "tpl_purchase_settlement.quote_within_5pct",
                    "견적 합계와 세금계산서 공급가액 차이 5% 이내",
                    O::field("quotation", "grand_total"),
                    RuleOp::WithinPct,
                    Some(O::field("tax_invoice", "supply_total")),
                    Some(5.0),
                    false,
                ),
            ],
        ),
    ]
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

const TEMPLATE_COLUMNS: &str =
    "id, name_ko, name_en, description_ko, description_en, required_doc_types, rules, builtin, created_at, updated_at";

fn row_to_template(r: &Row<'_>) -> rusqlite::Result<ProjectTemplate> {
    Ok(ProjectTemplate {
        id: r.get(0)?,
        name_ko: r.get(1)?,
        name_en: r.get(2)?,
        description_ko: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        description_en: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        required_doc_types: from_json(r.get(5)?),
        rules: from_json(r.get(6)?),
        builtin: r.get::<_, i64>(7)? != 0,
        created_at: parse_ts(r.get(8)?).unwrap_or_else(Utc::now),
        updated_at: parse_ts(r.get(9)?).unwrap_or_else(Utc::now),
    })
}

/// Inserts the built-in templates that are missing and refreshes the ones the user never
/// edited (`updated_at == created_at`). Called when a workspace opens / templates are listed.
pub fn ensure_builtins(conn: &Connection) -> rusqlite::Result<usize> {
    let now = ts(&Utc::now());
    let mut added = 0;
    for t in builtin_templates() {
        let existing: Option<(String, String)> = conn
            .query_row("SELECT created_at, updated_at FROM project_templates WHERE id = ?1", [&t.id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let types = to_json(&t.required_doc_types)?;
        let rules = to_json(&t.rules)?;
        match existing {
            None => {
                conn.execute(
                    "INSERT INTO project_templates (id, name_ko, name_en, description_ko, description_en, \
                     required_doc_types, rules, builtin, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?8)",
                    params![t.id, t.name_ko, t.name_en, t.description_ko, t.description_en, types, rules, now],
                )?;
                added += 1;
            }
            Some((created, updated)) if created == updated => {
                conn.execute(
                    "UPDATE project_templates SET name_ko = ?2, name_en = ?3, description_ko = ?4, description_en = ?5, \
                     required_doc_types = ?6, rules = ?7, builtin = 1, created_at = ?8, updated_at = ?8 WHERE id = ?1",
                    params![t.id, t.name_ko, t.name_en, t.description_ko, t.description_en, types, rules, now],
                )?;
            }
            Some(_) => {}
        }
    }
    Ok(added)
}

/// Every template, built-ins first (ensures the built-ins exist).
pub fn list_templates(conn: &Connection) -> rusqlite::Result<Vec<ProjectTemplate>> {
    ensure_builtins(conn)?;
    let mut st = conn.prepare_cached(&format!(
        "SELECT {TEMPLATE_COLUMNS} FROM project_templates ORDER BY builtin DESC, created_at, name_ko"
    ))?;
    let rows = st.query_map([], row_to_template)?;
    rows.collect()
}

pub fn get_template(conn: &Connection, id: &str) -> rusqlite::Result<Option<ProjectTemplate>> {
    let found = conn
        .query_row(&format!("SELECT {TEMPLATE_COLUMNS} FROM project_templates WHERE id = ?1"), [id], row_to_template)
        .optional()?;
    if found.is_none() && builtin_templates().iter().any(|t| t.id == id) {
        // A built-in asked for before the first listing of this workspace.
        ensure_builtins(conn)?;
        return conn
            .query_row(
                &format!("SELECT {TEMPLATE_COLUMNS} FROM project_templates WHERE id = ?1"),
                [id],
                row_to_template,
            )
            .optional();
    }
    Ok(found)
}

fn clean_doc_types(types: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in types {
        let t = t.trim().to_string();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// Rules with non-empty, unique ids (the grid may hand back rules without one).
fn clean_rules(rules: &[Rule]) -> Vec<Rule> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    rules
        .iter()
        .map(|r| {
            let mut r = r.clone();
            if r.id.trim().is_empty() || !seen.insert(r.id.clone()) {
                r.id = documents::new_id();
                seen.insert(r.id.clone());
            }
            r.description = r.description.trim().to_string();
            r
        })
        .collect()
}

/// Creates (`id = None`) or updates a template. Editing a built-in keeps `builtin = 1` but
/// stops the automatic refresh.
pub fn save_template(conn: &Connection, input: &TemplateInput) -> rusqlite::Result<ProjectTemplate> {
    let name_ko = input.name_ko.trim();
    if name_ko.is_empty() {
        return Err(typed("template_name_empty"));
    }
    let name_en = blank(input.name_en.clone()).unwrap_or_else(|| name_ko.to_string());
    let types = to_json(&clean_doc_types(&input.required_doc_types))?;
    let rules = to_json(&clean_rules(&input.rules))?;
    let now = ts(&Utc::now());
    let id = match &input.id {
        Some(id) if !id.trim().is_empty() => {
            let n = conn.execute(
                "UPDATE project_templates SET name_ko = ?2, name_en = ?3, description_ko = ?4, description_en = ?5, \
                 required_doc_types = ?6, rules = ?7, updated_at = ?8 WHERE id = ?1",
                params![
                    id,
                    name_ko,
                    name_en,
                    blank(input.description_ko.clone()),
                    blank(input.description_en.clone()),
                    types,
                    rules,
                    now
                ],
            )?;
            if n == 0 {
                return Err(typed("template_not_found"));
            }
            id.clone()
        }
        _ => {
            let id = format!("tpl_{}", documents::new_id());
            conn.execute(
                "INSERT INTO project_templates (id, name_ko, name_en, description_ko, description_en, \
                 required_doc_types, rules, builtin, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?8)",
                params![
                    id,
                    name_ko,
                    name_en,
                    blank(input.description_ko.clone()),
                    blank(input.description_en.clone()),
                    types,
                    rules,
                    now
                ],
            )?;
            id
        }
    };
    get_template(conn, &id)?.ok_or_else(|| typed("template_not_found"))
}

/// Deletes a user template (built-ins are refused with `template_builtin`); projects made from
/// it keep their rules and lose only the checklist reference.
pub fn delete_template(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let Some(t) = get_template(conn, id)? else { return Ok(false) };
    if t.builtin {
        return Err(typed("template_builtin"));
    }
    conn.execute("UPDATE projects SET template_id = NULL WHERE template_id = ?1", [id])?;
    Ok(conn.execute("DELETE FROM project_templates WHERE id = ?1", [id])? > 0)
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

const PROJECT_COLUMNS: &str = "id, name, template_id, status, intake_no, owner, due_date, notes, inbox_folder, \
created_at, updated_at, last_check_at";

fn row_to_project(r: &Row<'_>) -> rusqlite::Result<Project> {
    let status: String = r.get(3)?;
    Ok(Project {
        id: r.get(0)?,
        name: r.get(1)?,
        template_id: r.get(2)?,
        status: ProjectStatus::parse(&status).unwrap_or_default(),
        intake_no: r.get(4)?,
        owner: r.get(5)?,
        due_date: r.get(6)?,
        notes: r.get(7)?,
        inbox_folder: r.get(8)?,
        created_at: parse_ts(r.get(9)?).unwrap_or_else(Utc::now),
        updated_at: parse_ts(r.get(10)?).unwrap_or_else(Utc::now),
        document_count: 0,
        missing_count: 0,
        last_check_at: parse_ts(r.get(11)?),
        last_pass: 0,
        last_mismatch: 0,
        last_undetermined: 0,
    })
}

/// Document types present per project (existing, non-deleted documents only).
fn types_by_project(conn: &Connection, project_id: Option<&str>) -> rusqlite::Result<BTreeMap<String, Vec<String>>> {
    let mut sql = String::from(
        "SELECT pd.project_id, d.doc_type FROM project_documents pd JOIN documents d ON d.id = pd.document_id \
         WHERE d.deleted_at IS NULL",
    );
    if project_id.is_some() {
        sql.push_str(" AND pd.project_id = ?1");
    }
    let mut st = conn.prepare(&sql)?;
    let map = |r: &Row<'_>| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?));
    let rows: Vec<(String, Option<String>)> = match project_id {
        Some(id) => st.query_map([id], map)?.collect::<rusqlite::Result<_>>()?,
        None => st.query_map([], map)?.collect::<rusqlite::Result<_>>()?,
    };
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (pid, t) in rows {
        out.entry(pid).or_default().push(t.unwrap_or_default());
    }
    Ok(out)
}

/// Fills the computed counters of the given projects.
fn fill_counters(conn: &Connection, projects: &mut [Project], only: Option<&str>) -> rusqlite::Result<()> {
    if projects.is_empty() {
        return Ok(());
    }
    let types = types_by_project(conn, only)?;
    let template_ids: BTreeSet<String> = projects.iter().filter_map(|p| p.template_id.clone()).collect();
    let mut required: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in template_ids {
        if let Some(t) = get_template(conn, &id)? {
            required.insert(id, t.required_doc_types);
        }
    }
    let sql = match only {
        Some(_) => {
            "SELECT project_id, pass, mismatch, undetermined, started_at FROM project_runs WHERE project_id = ?1 \
             ORDER BY started_at DESC, id"
        }
        None => {
            "SELECT project_id, pass, mismatch, undetermined, started_at FROM project_runs ORDER BY started_at DESC, id"
        }
    };
    let mut st = conn.prepare(sql)?;
    let map = |r: &Row<'_>| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
        ))
    };
    let runs: Vec<(String, i64, i64, i64, String)> = match only {
        Some(id) => st.query_map([id], map)?.collect::<rusqlite::Result<_>>()?,
        None => st.query_map([], map)?.collect::<rusqlite::Result<_>>()?,
    };
    for p in projects.iter_mut() {
        let present = types.get(&p.id).cloned().unwrap_or_default();
        p.document_count = present.len() as u32;
        p.missing_count = p
            .template_id
            .as_ref()
            .and_then(|t| required.get(t))
            .map(|req| req.iter().filter(|t| !present.iter().any(|x| x == *t)).count() as u32)
            .unwrap_or(0);
        if let Some((_, pass, mismatch, undetermined, started)) = runs.iter().find(|r| r.0 == p.id) {
            p.last_pass = (*pass).max(0) as u32;
            p.last_mismatch = (*mismatch).max(0) as u32;
            p.last_undetermined = (*undetermined).max(0) as u32;
            if p.last_check_at.is_none() {
                p.last_check_at = parse_ts(Some(started.clone()));
            }
        }
    }
    Ok(())
}

/// Every project, most recently updated first, with the board counters.
pub fn list_projects(conn: &Connection) -> rusqlite::Result<Vec<Project>> {
    let mut st =
        conn.prepare_cached(&format!("SELECT {PROJECT_COLUMNS} FROM projects ORDER BY updated_at DESC, name"))?;
    let mut out: Vec<Project> = st.query_map([], row_to_project)?.collect::<rusqlite::Result<_>>()?;
    fill_counters(conn, &mut out, None)?;
    Ok(out)
}

pub fn get_project(conn: &Connection, id: &str) -> rusqlite::Result<Option<Project>> {
    let p = conn
        .query_row(&format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?1"), [id], row_to_project)
        .optional()?;
    match p {
        Some(p) => {
            let mut v = vec![p];
            fill_counters(conn, &mut v, Some(id))?;
            Ok(v.pop())
        }
        None => Ok(None),
    }
}

/// Project whose name matches `name` ignoring case, spacing and punctuation
/// (`planner::compact`), most recently updated first.
pub fn project_by_name(conn: &Connection, name: &str) -> rusqlite::Result<Option<Project>> {
    let key = crate::planner::compact(name);
    if key.is_empty() {
        return Ok(None);
    }
    let mut st = conn.prepare_cached("SELECT id, name FROM projects ORDER BY updated_at DESC, name")?;
    let rows: Vec<(String, String)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    match rows.into_iter().find(|(_, n)| crate::planner::compact(n) == key) {
        Some((id, _)) => get_project(conn, &id),
        None => Ok(None),
    }
}

/// Inserts rules for a project after the existing ones (fresh ids so several projects can
/// inherit the same template).
fn insert_rules(conn: &Connection, project_id: &str, rules: &[Rule]) -> rusqlite::Result<()> {
    let mut position: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) FROM project_rules WHERE project_id = ?1",
        [project_id],
        |r| r.get(0),
    )?;
    let now = ts(&Utc::now());
    for r in rules {
        position += 1;
        let mut r = r.clone();
        r.id = documents::new_id();
        conn.execute(
            "INSERT INTO project_rules (id, project_id, rule, position, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![r.id, project_id, to_json(&r)?, position, now],
        )?;
    }
    Ok(())
}

/// Creates (`id = None`) or updates a project. New projects inherit the template's rules; a
/// project that changes template while it has no rules yet inherits the new template's rules.
pub fn save_project(conn: &Connection, input: &ProjectInput) -> rusqlite::Result<Project> {
    let name = documents::nfc(input.name.trim());
    if name.is_empty() {
        return Err(typed("project_name_empty"));
    }
    let template_id = blank(input.template_id.clone());
    if let Some(t) = &template_id {
        if get_template(conn, t)?.is_none() {
            return Err(typed("template_not_found"));
        }
    }
    let now = ts(&Utc::now());
    let id = match &input.id {
        Some(id) if !id.trim().is_empty() => {
            let before = get_project(conn, id)?.ok_or_else(|| typed("project_not_found"))?;
            let status = input.status.unwrap_or(before.status);
            conn.execute(
                "UPDATE projects SET name = ?2, template_id = ?3, status = ?4, intake_no = ?5, owner = ?6, due_date = ?7, \
                 notes = ?8, inbox_folder = ?9, updated_at = ?10 WHERE id = ?1",
                params![
                    id,
                    name,
                    template_id,
                    status.as_str(),
                    blank(input.intake_no.clone()),
                    blank(input.owner.clone()),
                    blank(input.due_date.clone()),
                    blank(input.notes.clone()),
                    blank(input.inbox_folder.clone()).map(|f| documents::nfc(&f)),
                    now
                ],
            )?;
            if template_id.is_some() && template_id != before.template_id && rules_of(conn, id)?.is_empty() {
                if let Some(t) = template_of(conn, template_id.as_deref())? {
                    insert_rules(conn, id, &t.rules)?;
                }
            }
            id.clone()
        }
        _ => {
            let id = documents::new_id();
            conn.execute(
                "INSERT INTO projects (id, name, template_id, status, intake_no, owner, due_date, notes, inbox_folder, \
                 created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    id,
                    name,
                    template_id,
                    input.status.unwrap_or_default().as_str(),
                    blank(input.intake_no.clone()),
                    blank(input.owner.clone()),
                    blank(input.due_date.clone()),
                    blank(input.notes.clone()),
                    blank(input.inbox_folder.clone()).map(|f| documents::nfc(&f)),
                    now
                ],
            )?;
            if let Some(t) = template_of(conn, template_id.as_deref())? {
                insert_rules(conn, &id, &t.rules)?;
            }
            id
        }
    };
    get_project(conn, &id)?.ok_or_else(|| typed("project_not_found"))
}

fn template_of(conn: &Connection, id: Option<&str>) -> rusqlite::Result<Option<ProjectTemplate>> {
    match id {
        Some(id) => get_template(conn, id),
        None => Ok(None),
    }
}

/// Deletes a project with its membership, rules and runs (documents stay in the library).
pub fn delete_project(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM projects WHERE id = ?1", [id])? > 0)
}

pub fn set_status(conn: &Connection, id: &str, status: ProjectStatus) -> rusqlite::Result<()> {
    let n = conn.execute(
        "UPDATE projects SET status = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, status.as_str(), ts(&Utc::now())],
    )?;
    if n == 0 {
        return Err(typed("project_not_found"));
    }
    Ok(())
}

fn touch(conn: &Connection, project_id: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE projects SET updated_at = ?2 WHERE id = ?1", params![project_id, ts(&Utc::now())])?;
    Ok(())
}

/// Adds library documents to a project; unknown ids and existing members are skipped.
/// Returns the number of documents actually added.
pub fn add_documents(conn: &Connection, project_id: &str, doc_ids: &[String]) -> rusqlite::Result<u32> {
    if get_project(conn, project_id)?.is_none() {
        return Err(typed("project_not_found"));
    }
    let now = ts(&Utc::now());
    let mut added = 0u32;
    let mut st = conn.prepare_cached(
        "INSERT OR IGNORE INTO project_documents (project_id, document_id, added_at) \
         SELECT ?1, id, ?3 FROM documents WHERE id = ?2 AND deleted_at IS NULL",
    )?;
    for id in doc_ids {
        added += st.execute(params![project_id, id, now])? as u32;
    }
    if added > 0 {
        touch(conn, project_id)?;
    }
    Ok(added)
}

pub fn remove_document(conn: &Connection, project_id: &str, doc_id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM project_documents WHERE project_id = ?1 AND document_id = ?2",
        params![project_id, doc_id],
    )?;
    if n > 0 {
        touch(conn, project_id)?;
    }
    Ok(n > 0)
}

/// Member documents that still exist (deleted / purged documents are skipped), in the order
/// they were added.
pub fn documents_of(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<ProjectDocument>> {
    let mut st = conn.prepare_cached(
        "SELECT pd.document_id, pd.added_at FROM project_documents pd JOIN documents d ON d.id = pd.document_id \
         WHERE pd.project_id = ?1 AND d.deleted_at IS NULL ORDER BY pd.added_at, pd.rowid",
    )?;
    let rows: Vec<(String, Option<String>)> =
        st.query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for (id, added) in rows {
        if let Some(document) = docs::get_document(conn, &id)? {
            out.push(ProjectDocument { document, added_at: parse_ts(added).unwrap_or_else(Utc::now) });
        }
    }
    Ok(out)
}

/// Projects a document belongs to (viewer badge / deletion hooks).
pub fn projects_of_document(conn: &Connection, doc_id: &str) -> rusqlite::Result<Vec<Project>> {
    let mut st = conn.prepare_cached("SELECT project_id FROM project_documents WHERE document_id = ?1")?;
    let ids: Vec<String> = st.query_map([doc_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for id in ids {
        if let Some(p) = get_project(conn, &id)? {
            out.push(p);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

pub fn rules_of(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<Rule>> {
    let mut st = conn
        .prepare_cached("SELECT id, rule FROM project_rules WHERE project_id = ?1 ORDER BY position, created_at, id")?;
    let rows: Vec<(String, String)> =
        st.query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .map(|(id, json)| {
            let mut r: Rule = serde_json::from_str(&json).unwrap_or_default();
            r.id = id;
            r
        })
        .collect())
}

/// Inserts or replaces one rule of a project (new id when the rule has none).
pub fn save_rule(conn: &Connection, project_id: &str, rule: &Rule) -> rusqlite::Result<Rule> {
    if get_project(conn, project_id)?.is_none() {
        return Err(typed("project_not_found"));
    }
    let mut rule = rule.clone();
    rule.description = rule.description.trim().to_string();
    if rule.id.trim().is_empty() {
        rule.id = documents::new_id();
    }
    let now = ts(&Utc::now());
    let existing: Option<String> = conn
        .query_row("SELECT project_id FROM project_rules WHERE id = ?1", [&rule.id], |r| r.get::<_, String>(0))
        .optional()?;
    match existing {
        Some(owner) if owner == project_id => {
            conn.execute(
                "UPDATE project_rules SET rule = ?2, updated_at = ?3 WHERE id = ?1",
                params![rule.id, to_json(&rule)?, now],
            )?;
        }
        Some(_) => return Err(typed("rule_not_found")),
        None => {
            let position: i64 = conn.query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM project_rules WHERE project_id = ?1",
                [project_id],
                |r| r.get(0),
            )?;
            conn.execute(
                "INSERT INTO project_rules (id, project_id, rule, position, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![rule.id, project_id, to_json(&rule)?, position, now],
            )?;
        }
    }
    touch(conn, project_id)?;
    Ok(rule)
}

pub fn delete_rule(conn: &Connection, project_id: &str, rule_id: &str) -> rusqlite::Result<bool> {
    let n =
        conn.execute("DELETE FROM project_rules WHERE id = ?1 AND project_id = ?2", params![rule_id, project_id])?;
    if n > 0 {
        touch(conn, project_id)?;
    }
    Ok(n > 0)
}

// ---------------------------------------------------------------------------
// Check runs
// ---------------------------------------------------------------------------

/// Stores a finished check run with its results and stamps the project's `last_check_at`.
pub fn insert_run(conn: &Connection, run: &CheckRun) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO project_runs (id, project_id, started_at, finished_at, pass, mismatch, undetermined, missing, \
         missing_doc_types) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            run.id,
            run.project_id,
            ts(&run.started_at),
            ts(&run.finished_at),
            run.pass as i64,
            run.mismatch as i64,
            run.undetermined as i64,
            run.missing as i64,
            to_json(&run.missing_doc_types)?
        ],
    )?;
    let mut st = conn.prepare_cached(
        "INSERT INTO project_rule_results (id, run_id, rule_id, description, verdict, required, left_json, right_json, \
         message, checked_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    for r in &run.results {
        st.execute(params![
            r.id,
            run.id,
            r.rule_id,
            r.description,
            r.verdict.as_str(),
            r.required as i64,
            r.left.as_ref().map(to_json).transpose()?,
            r.right.as_ref().map(to_json).transpose()?,
            r.message,
            ts(&r.checked_at)
        ])?;
    }
    conn.execute(
        "UPDATE projects SET last_check_at = ?2, updated_at = ?2 WHERE id = ?1",
        params![run.project_id, ts(&run.finished_at)],
    )?;
    Ok(())
}

fn results_of(conn: &Connection, run_id: &str) -> rusqlite::Result<Vec<RuleResult>> {
    let mut st = conn.prepare_cached(
        "SELECT id, rule_id, description, verdict, required, left_json, right_json, message, checked_at \
         FROM project_rule_results WHERE run_id = ?1 ORDER BY rowid",
    )?;
    let rows = st.query_map([run_id], |r| {
        let verdict: String = r.get(3)?;
        Ok(RuleResult {
            id: r.get(0)?,
            run_id: run_id.to_string(),
            rule_id: r.get(1)?,
            description: r.get(2)?,
            verdict: Verdict::parse(&verdict).unwrap_or(Verdict::Undetermined),
            required: r.get::<_, i64>(4)? != 0,
            left: r.get::<_, Option<String>>(5)?.and_then(|s| serde_json::from_str::<Evidence>(&s).ok()),
            right: r.get::<_, Option<String>>(6)?.and_then(|s| serde_json::from_str::<Evidence>(&s).ok()),
            message: r.get(7)?,
            checked_at: parse_ts(r.get(8)?).unwrap_or_else(Utc::now),
        })
    })?;
    rows.collect()
}

fn row_to_run(conn: &Connection, r: &Row<'_>) -> rusqlite::Result<CheckRun> {
    let id: String = r.get(0)?;
    Ok(CheckRun {
        results: results_of(conn, &id)?,
        id,
        project_id: r.get(1)?,
        started_at: parse_ts(r.get(2)?).unwrap_or_else(Utc::now),
        finished_at: parse_ts(r.get(3)?).unwrap_or_else(Utc::now),
        pass: r.get::<_, i64>(4)?.max(0) as u32,
        mismatch: r.get::<_, i64>(5)?.max(0) as u32,
        undetermined: r.get::<_, i64>(6)?.max(0) as u32,
        missing: r.get::<_, i64>(7)?.max(0) as u32,
        missing_doc_types: from_json(r.get(8)?),
    })
}

/// The most recent runs of a project (newest first) with their results.
pub fn list_runs(conn: &Connection, project_id: &str, limit: u32) -> rusqlite::Result<Vec<CheckRun>> {
    let mut st = conn.prepare_cached(
        "SELECT id, project_id, started_at, finished_at, pass, mismatch, undetermined, missing, missing_doc_types \
         FROM project_runs WHERE project_id = ?1 ORDER BY started_at DESC, id DESC LIMIT ?2",
    )?;
    let rows = st.query_map(params![project_id, limit.max(1) as i64], |r| row_to_run(conn, r))?;
    rows.collect()
}

pub fn last_run(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<CheckRun>> {
    Ok(list_runs(conn, project_id, 1)?.pop())
}

// ---------------------------------------------------------------------------
// Checklist, detail, inbox mapping
// ---------------------------------------------------------------------------

/// Required document types of the project's template (in template order) followed by the
/// other types present among its documents.
pub fn checklist(conn: &Connection, project_id: &str, language: Language) -> rusqlite::Result<Vec<ChecklistItem>> {
    let project = get_project(conn, project_id)?.ok_or_else(|| typed("project_not_found"))?;
    let required = template_of(conn, project.template_id.as_deref())?.map(|t| t.required_doc_types).unwrap_or_default();
    let docs = documents_of(conn, project_id)?;
    let labels = doc_type_labels(conn)?;
    let _ = language;
    let mut out: Vec<ChecklistItem> = Vec::new();
    let item = |doc_type: &str, is_required: bool| {
        let ids: Vec<String> = docs
            .iter()
            .filter(|d| d.document.doc_type.as_deref() == Some(doc_type))
            .map(|d| d.document.id.clone())
            .collect();
        ChecklistItem {
            doc_type: doc_type.to_string(),
            label_ko: type_label(&labels, doc_type, true),
            label_en: type_label(&labels, doc_type, false),
            required: is_required,
            missing: is_required && ids.is_empty(),
            doc_ids: ids,
        }
    };
    for t in &required {
        out.push(item(t, true));
    }
    let mut extra: Vec<String> = docs
        .iter()
        .map(|d| d.document.doc_type.clone().unwrap_or_default())
        .filter(|t| !required.contains(t))
        .collect();
    extra.sort();
    extra.dedup();
    for t in extra {
        out.push(item(&t, false));
    }
    Ok(out)
}

/// Everything the project screen shows: project, template, documents, checklist, rules,
/// last run and the relation graph.
pub fn detail(db: &Db, project_id: &str, language: Language) -> Result<ProjectDetail> {
    let loaded = db.with(|c| {
        let Some(project) = get_project(c, project_id)? else { return Ok(None) };
        let template = template_of(c, project.template_id.as_deref())?;
        Ok(Some((
            project,
            template,
            documents_of(c, project_id)?,
            checklist(c, project_id, language)?,
            rules_of(c, project_id)?,
            last_run(c, project_id)?,
        )))
    })?;
    let Some((project, template, documents, checklist, rules, last_run)) = loaded else {
        return Err(CoreError::NotFound(format!("project {project_id}")));
    };
    let relations = relations::graph(db, project_id)?;
    Ok(ProjectDetail { project, template, documents, checklist, rules, last_run, relations })
}

/// `inbox/<folder>/…` → `<folder>` (NFC). Files directly in `inbox/` and paths outside it
/// give `None`.
pub fn inbox_folder_of(rel_path: &str) -> Option<String> {
    let rest = rel_path.trim_start_matches('/').strip_prefix("inbox/")?;
    let (folder, file) = rest.split_once('/')?;
    let folder = documents::nfc(folder.trim());
    if folder.is_empty() || file.trim().is_empty() {
        return None;
    }
    Some(folder)
}

/// Maps a document imported from an inbox subfolder to the project with that
/// `inbox_folder` (created with the folder name when `create` is set). Returns the project id
/// when the document is a member afterwards.
pub fn assign_inbox_document(db: &Db, doc_id: &str, rel_path: &str, create: bool) -> Result<Option<String>> {
    let Some(folder) = inbox_folder_of(rel_path) else { return Ok(None) };
    db.with(|c| {
        let existing: Option<String> = c
            .query_row(
                "SELECT id FROM projects WHERE inbox_folder = ?1 ORDER BY updated_at DESC LIMIT 1",
                [&folder],
                |r| r.get(0),
            )
            .optional()?;
        let project_id = match existing {
            Some(id) => id,
            None if create => {
                save_project(
                    c,
                    &ProjectInput { name: folder.clone(), inbox_folder: Some(folder.clone()), ..Default::default() },
                )?
                .id
            }
            None => return Ok(None),
        };
        add_documents(c, &project_id, &[doc_id.to_string()])?;
        let member: i64 = c.query_row(
            "SELECT COUNT(*) FROM project_documents WHERE project_id = ?1 AND document_id = ?2",
            params![project_id, doc_id],
            |r| r.get(0),
        )?;
        Ok((member > 0).then_some(project_id))
    })
}
