//! Human review (§3.4 검토 큐 / §3.7 검토 화면): approve / correct / hold /
//! reject fields, bulk approval, re-validation of the latest run, snapshot
//! restore from the field history, and the document promotion rule (every
//! required field resolved with a value → `reviewed`).

use std::collections::BTreeSet;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::documents::ReviewStatus;
use crate::error::Result;

use super::repo::{self, ReviewFilter};
use super::schema::{FieldDef, SchemaDef};
use super::validate::{self, ConfidenceInput};
use super::{FieldRecord, FieldStatus, ReviewReason, ValidationIssue};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewAction {
    Approve,
    Reject,
    Hold,
    /// Back to `auto` (undo of a review decision).
    Reset,
    Correct {
        value: String,
    },
}

/// The schema field behind a stored row (options, regex hint, mask), falling back
/// to what the row itself records when the schema is gone.
fn field_def_for(f: &FieldRecord, def: Option<&SchemaDef>) -> FieldDef {
    if let Some(def) = def {
        if let Some(t) = f.path.split('[').next().filter(|_| f.row.is_some()) {
            if let Some(col) = def.table(t).and_then(|t| t.fields.iter().find(|c| c.key == f.key)) {
                return col.clone();
            }
        } else if let Some(d) = def.field(&f.path) {
            return d.clone();
        }
    }
    FieldDef {
        key: f.key.clone(),
        label_ko: f.label_ko.clone(),
        label_en: f.label_en.clone(),
        kind: f.kind,
        required: f.required,
        ..Default::default()
    }
}

/// Schema definition of a run (`None` when the schema was deleted).
fn schema_of_run(conn: &Connection, run_id: &str) -> rusqlite::Result<Option<SchemaDef>> {
    let Some(run) = repo::get_run(conn, run_id)? else { return Ok(None) };
    let Some(id) = run.schema_id else { return Ok(None) };
    Ok(repo::get_schema(conn, &id)?.map(|s| s.definition))
}

/// Applies one action to several fields (one transaction) and recomputes the
/// review status of every touched document.
pub fn apply(db: &Db, field_ids: &[i64], action: &ReviewAction, note: Option<&str>) -> Result<Vec<FieldRecord>> {
    let ids = field_ids.to_vec();
    let action = action.clone();
    let note = note.map(str::to_string);
    db.with_tx(move |tx| {
        let mut out = Vec::new();
        let mut touched: BTreeSet<String> = BTreeSet::new();
        let mut defs: std::collections::HashMap<String, Option<SchemaDef>> = std::collections::HashMap::new();
        for id in &ids {
            let Some(current) = repo::get_field(tx, *id)? else { return Err(repo::typed("field_not_found")) };
            if !defs.contains_key(&current.run_id) {
                let d = schema_of_run(tx, &current.run_id)?;
                defs.insert(current.run_id.clone(), d);
            }
            let schema_def = defs.get(&current.run_id).and_then(|d| d.as_ref());
            let updated = match &action {
                ReviewAction::Approve => {
                    repo::update_field_status(tx, *id, FieldStatus::Approved, None, None, note.as_deref())?
                }
                ReviewAction::Reject => {
                    repo::update_field_status(tx, *id, FieldStatus::Rejected, None, None, note.as_deref())?
                }
                ReviewAction::Hold => {
                    repo::update_field_status(tx, *id, FieldStatus::Held, None, None, note.as_deref())?
                }
                ReviewAction::Reset => {
                    repo::update_field_status(tx, *id, FieldStatus::Auto, None, None, note.as_deref())?
                }
                ReviewAction::Correct { value } => {
                    let v = value.trim();
                    if v.is_empty() {
                        return Err(repo::typed("field_value_empty"));
                    }
                    let def = field_def_for(&current, schema_def);
                    let (normalized, mut issues) = validate::validate_value(&def, v);
                    let stored = validate::mask_value(&def, v);
                    let updated = repo::update_field_status(
                        tx,
                        *id,
                        FieldStatus::Corrected,
                        Some(&stored),
                        normalized.as_deref(),
                        note.as_deref(),
                    )?;
                    // The corrected value is still checked (enum options, regex, checksum) so a
                    // typo in the correction shows up; the field stays resolved either way.
                    issues.retain(|i| i.level != "ok");
                    repo::update_field_validation(tx, *id, updated.confidence, updated.review_reason, &issues)?;
                    repo::get_field(tx, *id)?.unwrap_or(updated)
                }
            };
            touched.insert(updated.document_id.clone());
            out.push(updated);
        }
        for d in touched {
            recompute_document(tx, &d)?;
        }
        Ok(out)
    })
}

/// Derives `documents.review_status` / `status` from the latest run's fields.
pub fn recompute_document(conn: &Connection, doc_id: &str) -> rusqlite::Result<ReviewStatus> {
    let fields = repo::latest_fields(conn, doc_id)?;
    if fields.is_empty() {
        return Ok(ReviewStatus::None);
    }
    let pending = fields.iter().any(FieldRecord::needs_review);
    let flagged = fields.iter().any(|f| f.review_reason.is_some());
    let status = if pending {
        ReviewStatus::Pending
    } else if flagged {
        ReviewStatus::Done
    } else {
        ReviewStatus::None
    };
    let required_ok = fields.iter().filter(|f| f.required).all(|f| f.effective_value().is_some());
    repo::set_review_status(conn, doc_id, status, !pending && required_ok)?;
    if pending {
        conn.execute("UPDATE documents SET status = 'extracted' WHERE id = ?1 AND status = 'reviewed'", [doc_id])?;
    }
    Ok(status)
}

/// Approves every pending field of a document (optionally only one reason).
pub fn approve_document(db: &Db, doc_id: &str, only: Option<ReviewReason>) -> Result<usize> {
    let filter = ReviewFilter { document_id: Some(doc_id.to_string()), reason: only, ..Default::default() };
    let pending = db.with(|c| repo::review_queue(c, &filter))?;
    let ids: Vec<i64> = pending.iter().map(|f| f.id).collect();
    if ids.is_empty() {
        db.with(|c| recompute_document(c, doc_id))?;
        return Ok(0);
    }
    apply(db, &ids, &ReviewAction::Approve, Some("bulk"))?;
    Ok(ids.len())
}

/// Restores the value recorded in a history entry (viewer "이 값으로 복원").
pub fn restore(db: &Db, change_id: i64) -> Result<FieldRecord> {
    let change = db.with(|c| repo::get_change(c, change_id))?.ok_or_else(|| repo::typed("change_not_found"))?;
    let action = match &change.old_value {
        Some(v) if !v.trim().is_empty() => ReviewAction::Correct { value: v.clone() },
        _ => ReviewAction::Reject,
    };
    let mut out = apply(db, &[change.field_id], &action, Some("restored"))?;
    out.pop().ok_or_else(|| repo::typed("field_not_found").into())
}

/// Re-runs value validation and the schema's cross-field rules over the
/// latest run with the current (possibly corrected) values; approved and
/// corrected fields keep their status and confidence.
pub fn revalidate(db: &Db, doc_id: &str, threshold: f64, audit_percent: u8) -> Result<Vec<FieldRecord>> {
    let fields = db.with(|c| repo::latest_fields(c, doc_id))?;
    if fields.is_empty() {
        return Ok(fields);
    }
    let run = db.with(|c| repo::get_run(c, &fields[0].run_id))?;
    let schema = match run.as_ref().and_then(|r| r.schema_id.clone()) {
        Some(id) => db.with(|c| repo::get_schema(c, &id))?,
        None => None,
    };
    let mut scalars: Vec<(String, Option<String>)> = Vec::new();
    let mut rows: Vec<(String, u32, String, Option<String>)> = Vec::new();
    let mut per_field: Vec<(i64, Option<String>, Vec<ValidationIssue>, bool)> = Vec::new();
    for f in &fields {
        let def = field_def_for(f, schema.as_ref().map(|s| &s.definition));
        let (normalized, issues) = match f.effective_value() {
            Some(v) => validate::validate_value(&def, v),
            None => (None, Vec::new()),
        };
        match f.row {
            Some(r) => {
                let table = f.path.split('[').next().unwrap_or("").to_string();
                rows.push((table, r, f.key.clone(), normalized.clone()));
            }
            None => scalars.push((f.path.clone(), normalized.clone())),
        }
        per_field.push((
            f.id,
            normalized,
            issues.into_iter().filter(|i| i.level != "ok").collect(),
            f.effective_value().is_some(),
        ));
    }
    let values = validate::values_from(&scalars, &rows);
    let mut rule_issues: Vec<(String, ValidationIssue)> = Vec::new();
    if let Some(s) = &schema {
        rule_issues =
            validate::document_rules(&s.definition, &values).into_iter().filter(|(_, i)| i.level != "ok").collect();
    }
    let doc_id_s = doc_id.to_string();
    db.with_tx(move |tx| {
        for (f, (id, _normalized, mut issues, has_value)) in fields.iter().zip(per_field) {
            for (p, i) in rule_issues.iter().filter(|(p, _)| *p == f.path) {
                let _ = p;
                issues.push(i.clone());
            }
            let has_fail = issues.iter().any(|i| i.level == "fail");
            let has_warn = issues.iter().any(|i| i.level == "warn");
            let had_fail = f.validation.iter().any(|i| i.level == "fail");
            let had_warn = f.validation.iter().any(|i| i.level == "warn");
            // Idempotent: the stored score stands unless the validation outcome changed
            // (the evidence / value signals of the original run are not re-derivable here).
            let base = if f.status.is_resolved() {
                1.0
            } else if has_fail == had_fail && has_warn == had_warn {
                f.confidence
            } else {
                validate::confidence(ConfidenceInput {
                    model: f.model_confidence,
                    evidence_found: None,
                    value_found: None,
                    agreement: None,
                    has_fail,
                    has_warn,
                })
            };
            let reason = if !has_value {
                f.required.then_some(ReviewReason::RequiredMissing)
            } else if has_fail {
                Some(ReviewReason::ValidationFailed)
            } else if base < threshold {
                Some(ReviewReason::LowConfidence)
            } else if f.review_reason == Some(ReviewReason::Audit) && audit_percent > 0 {
                Some(ReviewReason::Audit)
            } else {
                None
            };
            repo::update_field_validation(tx, id, base, reason, &issues)?;
        }
        recompute_document(tx, &doc_id_s)?;
        Ok(())
    })?;
    db.with(|c| repo::latest_fields(c, doc_id))
}

/// Corrects a field with a value typed in the viewer (same as `Correct`, single field).
pub fn correct(db: &Db, field_id: i64, value: &str) -> Result<FieldRecord> {
    let mut out = apply(db, &[field_id], &ReviewAction::Correct { value: value.to_string() }, None)?;
    out.pop().ok_or_else(|| repo::typed("field_not_found").into())
}
