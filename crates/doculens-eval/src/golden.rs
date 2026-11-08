//! Planner golden set (design §3.11 "플래너 정확도 100건"): file format, loading
//! with validation, and the plan comparison (intent, ordered op kinds, parameter
//! subset containment per op, optional scope subset).

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use doculens_core::planner::{OpKind, Plan, PlanContext};

pub const SCHEMA_VERSION: u32 = 1;
pub const CATEGORIES: &[&str] = &["ko", "en", "mixed"];

#[derive(Debug, Clone, Deserialize)]
pub struct GoldenSet {
    pub schema_version: u32,
    /// Plan-context file, relative to the golden file (default `planner-context.json` beside it).
    #[serde(default)]
    pub context: Option<String>,
    pub items: Vec<GoldenItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenItem {
    pub id: String,
    /// `ko` | `en` | `mixed`
    pub category: String,
    pub text: String,
    pub expected: Expected,
    /// Top-level `PlanContext` keys that replace the shared context for this item only
    /// (e.g. `{"selected_doc_ids": ["d1a2", "d7f3"]}` for a comparison).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Expected {
    pub intent: String,
    /// Ordered op kinds.
    pub ops: Vec<String>,
    /// Per op index: a subset of the serialised op (`null` entry = not checked; a `null`
    /// value inside = the parameter must be absent).
    #[serde(default)]
    pub params: Vec<Option<Value>>,
    /// Subset of the serialised scope (`folder`, `doc_ids`, `filter.*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mismatch {
    Intent { expected: String, produced: String },
    Ops { expected: Vec<String>, produced: Vec<String> },
    Params { index: usize, expected: Value, produced: Value },
    Scope { expected: Value, produced: Value },
}

impl Mismatch {
    pub fn kind(&self) -> &'static str {
        match self {
            Mismatch::Intent { .. } => "intent",
            Mismatch::Ops { .. } => "ops",
            Mismatch::Params { .. } => "params",
            Mismatch::Scope { .. } => "scope",
        }
    }
}

/// Subset containment: objects recursively, arrays element-wise with the same
/// length, scalars by equality; an expected `null` means absent or `null`.
pub fn contains(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (_, Value::Null) => actual.is_null(),
        (Value::Object(a), Value::Object(e)) => e.iter().all(|(k, ev)| match a.get(k) {
            Some(av) => contains(av, ev),
            None => ev.is_null(),
        }),
        (Value::Array(a), Value::Array(e)) => a.len() == e.len() && a.iter().zip(e).all(|(av, ev)| contains(av, ev)),
        (a, e) => a == e,
    }
}

pub fn op_kinds(plan: &Plan) -> Vec<String> {
    plan.operations.iter().map(|o| o.kind().as_str().to_string()).collect()
}

/// Every way the produced plan differs from the expectation (empty = correct).
pub fn compare(expected: &Expected, plan: &Plan) -> Vec<Mismatch> {
    let mut out = Vec::new();
    let produced_intent = plan.intent.as_str().to_string();
    if produced_intent != expected.intent {
        out.push(Mismatch::Intent { expected: expected.intent.clone(), produced: produced_intent });
    }
    let kinds = op_kinds(plan);
    if kinds != expected.ops {
        out.push(Mismatch::Ops { expected: expected.ops.clone(), produced: kinds });
        // Parameters cannot be aligned to the wrong op list.
        return out;
    }
    for (index, params) in expected.params.iter().enumerate() {
        let Some(params) = params else { continue };
        let Some(op) = plan.operations.get(index) else { continue };
        let produced = serde_json::to_value(op).unwrap_or(Value::Null);
        if !contains(&produced, params) {
            out.push(Mismatch::Params { index, expected: params.clone(), produced });
        }
    }
    if let Some(scope) = &expected.scope {
        let produced = serde_json::to_value(&plan.scope).unwrap_or(Value::Null);
        if !contains(&produced, scope) {
            out.push(Mismatch::Scope { expected: scope.clone(), produced });
        }
    }
    out
}

/// Loads and validates a golden set (ids unique, categories / op names known,
/// parameter lists no longer than the op lists).
pub fn load(path: &Path) -> Result<GoldenSet, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let set: GoldenSet = serde_json::from_str(&text).map_err(|e| format!("{}: JSON 오류: {e}", path.display()))?;
    validate(&set).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(set)
}

pub fn validate(set: &GoldenSet) -> Result<(), String> {
    if set.schema_version != SCHEMA_VERSION {
        return Err(format!("schema_version {} (지원: {SCHEMA_VERSION})", set.schema_version));
    }
    if set.items.is_empty() {
        return Err("items 가 비어 있음".into());
    }
    let mut ids = BTreeSet::new();
    for item in &set.items {
        let id = item.id.trim();
        if id.is_empty() {
            return Err(format!("id 없는 항목: {:?}", item.text));
        }
        if !ids.insert(id.to_string()) {
            return Err(format!("{id}: id 중복"));
        }
        if !CATEGORIES.contains(&item.category.as_str()) {
            return Err(format!("{id}: category {:?} (허용: ko|en|mixed)", item.category));
        }
        if item.text.trim().is_empty() {
            return Err(format!("{id}: text 가 비어 있음"));
        }
        if OpKind::parse(&item.expected.intent).is_none() {
            return Err(format!("{id}: 알 수 없는 intent {:?}", item.expected.intent));
        }
        if item.expected.ops.is_empty() {
            return Err(format!("{id}: ops 가 비어 있음"));
        }
        for op in &item.expected.ops {
            if OpKind::parse(op).is_none() {
                return Err(format!("{id}: 알 수 없는 op {op:?}"));
            }
        }
        if item.expected.params.len() > item.expected.ops.len() {
            return Err(format!(
                "{id}: params({})가 ops({})보다 김",
                item.expected.params.len(),
                item.expected.ops.len()
            ));
        }
        for (i, p) in item.expected.params.iter().enumerate() {
            if let Some(p) = p {
                if !p.is_object() {
                    return Err(format!("{id}: params[{i}] 는 객체 또는 null 이어야 함"));
                }
            }
        }
        if let Some(s) = &item.expected.scope {
            if !s.is_object() {
                return Err(format!("{id}: scope 는 객체여야 함"));
            }
        }
        if let Some(c) = &item.context {
            if !c.is_object() {
                return Err(format!("{id}: context 는 객체여야 함"));
            }
        }
    }
    Ok(())
}

/// The shared context with an item's `context` keys replaced (top-level merge).
pub fn item_context(base: &PlanContext, item: &GoldenItem) -> Result<PlanContext, String> {
    let Some(Value::Object(over)) = &item.context else { return Ok(base.clone()) };
    let mut v = serde_json::to_value(base).map_err(|e| e.to_string())?;
    let Some(obj) = v.as_object_mut() else { return Ok(base.clone()) };
    for (k, val) in over {
        obj.insert(k.clone(), val.clone());
    }
    serde_json::from_value(v).map_err(|e| format!("{}: context 병합 오류: {e}", item.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use doculens_core::planner::{AggregateFn, GroupBy, Op, Scope};
    use serde_json::json;

    #[test]
    fn subset_containment() {
        let actual =
            json!({ "op": "aggregate", "fn": "sum", "field": "total", "group_by": "month", "tags": ["a", "b"] });
        assert!(contains(&actual, &json!({ "fn": "sum" })));
        assert!(contains(&actual, &json!({ "fn": "sum", "field": "total" })));
        assert!(contains(&actual, &json!({ "tags": ["a", "b"] })));
        assert!(!contains(&actual, &json!({ "tags": ["a"] })), "arrays compare whole");
        assert!(!contains(&actual, &json!({ "fn": "count" })));
        assert!(!contains(&actual, &json!({ "missing": "x" })));
        // `null` = the key must be absent (skip_serializing_if) or null.
        assert!(contains(&actual, &json!({ "include_unreviewed": null })));
        assert!(!contains(&actual, &json!({ "field": null })));
        // nested objects
        let scope = json!({ "folder": "inbox", "filter": { "doc_type": "receipt", "date_from": "2026-10-01" } });
        assert!(contains(&scope, &json!({ "filter": { "doc_type": "receipt" } })));
        assert!(!contains(&scope, &json!({ "filter": { "doc_type": "contract" } })));
        assert!(!contains(&json!(1), &json!("1")));
    }

    #[test]
    fn compare_reports_each_difference() {
        let mut plan = Plan::single(
            Op::Aggregate {
                func: AggregateFn::Sum,
                field: Some("total".into()),
                group_by: None,
                include_unreviewed: None,
            },
            0.9,
            "s",
        );
        plan.scope = Scope::default();
        plan.scope.filter.doc_type = Some("receipt".into());
        let ok = Expected {
            intent: "aggregate".into(),
            ops: vec!["aggregate".into()],
            params: vec![Some(json!({ "fn": "sum", "field": "total", "group_by": null }))],
            scope: Some(json!({ "filter": { "doc_type": "receipt" } })),
        };
        assert!(compare(&ok, &plan).is_empty());

        let wrong_params = Expected { params: vec![Some(json!({ "fn": "count" }))], ..ok.clone() };
        let m = compare(&wrong_params, &plan);
        assert_eq!(m.len(), 1);
        assert!(matches!(&m[0], Mismatch::Params { index: 0, .. }));

        let wrong_scope = Expected { scope: Some(json!({ "doc_ids": ["d1"] })), ..ok.clone() };
        assert!(matches!(compare(&wrong_scope, &plan).as_slice(), [Mismatch::Scope { .. }]));

        // Different ops: intent + ops reported, params skipped.
        let wrong_ops = Expected {
            intent: "export".into(),
            ops: vec!["extract".into(), "export".into()],
            params: vec![Some(json!({ "fields": ["x"] })), None],
            scope: None,
        };
        let m = compare(&wrong_ops, &plan);
        assert_eq!(m.iter().map(Mismatch::kind).collect::<Vec<_>>(), vec!["intent", "ops"]);

        // Params shorter than ops is fine; group_by present when expected absent fails.
        plan.operations[0] = Op::Aggregate {
            func: AggregateFn::Sum,
            field: Some("total".into()),
            group_by: Some(GroupBy::Month),
            include_unreviewed: None,
        };
        assert!(matches!(compare(&ok, &plan).as_slice(), [Mismatch::Params { .. }]));
    }

    #[test]
    fn item_context_replaces_top_level_keys() {
        let base = PlanContext { current_document: Some("d1a2".into()), document_count: 42, ..Default::default() };
        let item: GoldenItem = serde_json::from_value(json!({
            "id": "x", "category": "ko", "text": "t",
            "expected": { "intent": "compare", "ops": ["compare"] },
            "context": { "selected_doc_ids": ["d1a2", "d7f3"] }
        }))
        .unwrap();
        let merged = item_context(&base, &item).unwrap();
        assert_eq!(merged.selected_doc_ids, vec!["d1a2".to_string(), "d7f3".to_string()]);
        assert_eq!(merged.current_document.as_deref(), Some("d1a2"));
        assert_eq!(merged.document_count, 42);
        let plain: GoldenItem = serde_json::from_value(json!({
            "id": "y", "category": "ko", "text": "t", "expected": { "intent": "help", "ops": ["help"] }
        }))
        .unwrap();
        assert_eq!(item_context(&base, &plain).unwrap(), base);
    }

    #[test]
    fn validation_rejects_bad_sets() {
        let good = |items: Value| -> Result<(), String> {
            let set: GoldenSet =
                serde_json::from_value(json!({ "schema_version": 1, "items": items })).map_err(|e| e.to_string())?;
            validate(&set)
        };
        let item =
            json!({ "id": "a", "category": "ko", "text": "x", "expected": { "intent": "search", "ops": ["search"] } });
        assert!(good(json!([item])).is_ok());
        assert!(good(json!([item, item])).unwrap_err().contains("중복"));
        let mut bad = item.clone();
        bad["category"] = json!("jp");
        assert!(good(json!([bad])).unwrap_err().contains("category"));
        let mut bad = item.clone();
        bad["expected"]["ops"] = json!(["teleport"]);
        assert!(good(json!([bad])).unwrap_err().contains("teleport"));
        let mut bad = item.clone();
        bad["expected"]["params"] = json!([{ "a": 1 }, { "b": 2 }]);
        assert!(good(json!([bad])).unwrap_err().contains("params"));
        assert!(good(json!([])).is_err());
    }
}
