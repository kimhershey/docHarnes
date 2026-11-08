//! Shared-entity relation graph (§3.15 "문서 간 관계"): the documents of a
//! project are linked when they share a normalised entity — business number,
//! contract / serial number, an equal amount, an equal date, a party name or an
//! account number — taken from their latest extracted fields and the document
//! record itself (`vendor`, `doc_date`, `total_amount`). Edges are unique per
//! (pair, kind, value) and ordered by document order, kind and value.

use std::collections::BTreeSet;

use crate::db::Db;
use crate::error::Result;
use crate::pipeline::repo as pipeline_repo;
use crate::pipeline::schema::FieldKind;
use crate::pipeline::validate::{brn_digits, format_number, normalize_amount, normalize_date};
use crate::pipeline::FieldStatus;

use super::check::{normalize_name, DocFields};
use super::repo;
use super::{RelationEdge, RelationGraph, RelationNode};

/// Relation kinds (`RelationEdge::kind`).
pub const KINDS: [&str; 6] = ["brn", "contract_no", "amount", "date", "name", "account"];

/// Human label of a relation kind.
pub fn kind_label(kind: &str, ko: bool) -> &'static str {
    match (kind, ko) {
        ("brn", true) => "사업자번호",
        ("brn", false) => "Business no.",
        ("contract_no", true) => "계약·일련번호",
        ("contract_no", false) => "Contract / serial no.",
        ("amount", true) => "금액",
        ("amount", false) => "Amount",
        ("date", true) => "날짜",
        ("date", false) => "Date",
        ("name", true) => "명칭",
        ("name", false) => "Name",
        ("account", true) => "계좌",
        ("account", false) => "Account",
        (_, true) => "기타",
        (_, false) => "Other",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Entity {
    kind: &'static str,
    value: String,
    field: String,
}

const NUMBER_KEYS: [&str; 5] = ["contract_no", "contract_number", "serial_no", "document_no", "doc_no"];
const TOTAL_KEYS: [&str; 6] = ["grand_total", "total", "total_amount", "amount", "contract_amount", "sum_total"];
const DATE_KEYS: [&str; 6] = ["issue_date", "doc_date", "contract_date", "datetime", "application_date", "date"];
const NAME_PATHS: [&str; 7] =
    ["supplier.name", "buyer.name", "company_name", "store_name", "vendor", "name", "applicant.name"];

fn push(out: &mut Vec<Entity>, kind: &'static str, value: String, field: &str) {
    if value.is_empty() || out.iter().any(|e| e.kind == kind && e.value == value) {
        return;
    }
    out.push(Entity { kind, value, field: field.to_string() });
}

/// Normalised entities of one document (first field of each (kind, value) kept).
fn entities(d: &DocFields) -> Vec<Entity> {
    let mut out = Vec::new();
    for f in &d.fields {
        if f.status == FieldStatus::Rejected {
            continue;
        }
        let Some(value) = f.effective_value().map(str::trim).filter(|v| !v.is_empty()) else { continue };
        let key = f.key.as_str();
        let normalized = f.normalized.as_deref();
        if f.kind == FieldKind::Brn || key.contains("brn") || key == "registration_no" || key == "business_number" {
            if let Some(d10) = brn_digits(normalized.unwrap_or(value)).or_else(|| brn_digits(value)) {
                push(&mut out, "brn", d10, &f.path);
                continue;
            }
        }
        if f.row.is_none() && NUMBER_KEYS.contains(&key) {
            let v: String = value.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_uppercase()).collect();
            if v.chars().count() >= 3 {
                push(&mut out, "contract_no", v, &f.path);
            }
            continue;
        }
        if f.row.is_none() && f.kind == FieldKind::Amount && (key.ends_with("_total") || TOTAL_KEYS.contains(&key)) {
            let n = normalized.and_then(|n| n.trim().parse::<f64>().ok()).or_else(|| normalize_amount(value));
            if let Some(n) = n.filter(|n| n.abs() > 1e-9) {
                push(&mut out, "amount", format_number(n), &f.path);
            }
            continue;
        }
        if f.row.is_none() && DATE_KEYS.contains(&key) {
            let iso = normalized
                .filter(|n| n.len() == 10 && n.as_bytes()[4] == b'-')
                .map(str::to_string)
                .or_else(|| normalize_date(value));
            if let Some(iso) = iso {
                push(&mut out, "date", iso, &f.path);
            }
            continue;
        }
        let party_row = f.row.is_some() && key == "name" && f.path.starts_with("parties[");
        if (f.row.is_none() && NAME_PATHS.contains(&f.path.as_str())) || party_row {
            let n = normalize_name(value);
            if n.chars().count() >= 2 {
                push(&mut out, "name", n, &f.path);
            }
            continue;
        }
        if f.kind == FieldKind::Account || key.contains("account") {
            let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() >= 4 {
                push(&mut out, "account", digits, &f.path);
            }
        }
    }
    if let Some(v) = d.doc.vendor.as_deref() {
        let n = normalize_name(v);
        if n.chars().count() >= 2 {
            push(&mut out, "name", n, "vendor");
        }
    }
    if let Some(iso) = d.doc.doc_date.as_deref().and_then(normalize_date) {
        push(&mut out, "date", iso, "doc_date");
    }
    if let Some(t) = d.doc.total_amount.filter(|t| t.abs() > 1e-9) {
        push(&mut out, "amount", format_number(t), "total_amount");
    }
    out
}

/// Builds the graph from loaded documents (pure; `graph` loads them).
pub fn build(docs: &[DocFields]) -> RelationGraph {
    let nodes: Vec<RelationNode> = docs
        .iter()
        .map(|d| RelationNode { doc_id: d.doc.id.clone(), label: d.name(), doc_type: d.doc.doc_type.clone() })
        .collect();
    let ents: Vec<Vec<Entity>> = docs.iter().map(entities).collect();
    let mut seen: BTreeSet<(usize, usize, &str, String)> = BTreeSet::new();
    let mut edges: Vec<(usize, usize, RelationEdge)> = Vec::new();
    for i in 0..docs.len() {
        for j in i + 1..docs.len() {
            for a in &ents[i] {
                for b in &ents[j] {
                    if a.kind != b.kind || a.value != b.value {
                        continue;
                    }
                    if !seen.insert((i, j, a.kind, a.value.clone())) {
                        continue;
                    }
                    edges.push((
                        i,
                        j,
                        RelationEdge {
                            from: docs[i].doc.id.clone(),
                            to: docs[j].doc.id.clone(),
                            kind: a.kind.to_string(),
                            value: a.value.clone(),
                            from_field: Some(a.field.clone()),
                            to_field: Some(b.field.clone()),
                        },
                    ));
                }
            }
        }
    }
    edges.sort_by(|x, y| {
        (x.0, x.1, KINDS.iter().position(|k| *k == x.2.kind), &x.2.value).cmp(&(
            y.0,
            y.1,
            KINDS.iter().position(|k| *k == y.2.kind),
            &y.2.value,
        ))
    });
    RelationGraph { nodes, edges: edges.into_iter().map(|(_, _, e)| e).collect() }
}

/// Shared entities between the project's documents.
pub fn graph(db: &Db, project_id: &str) -> Result<RelationGraph> {
    let docs = db.with(|c| {
        let mut out = Vec::new();
        for d in repo::documents_of(c, project_id)? {
            let fields = pipeline_repo::latest_fields(c, &d.document.id)?;
            out.push(DocFields { doc: d.document, fields });
        }
        Ok(out)
    })?;
    Ok(build(&docs))
}
