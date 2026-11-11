//! Planner golden set (design §3.11 "플래너 정확도 100건"): file format, loading
//! with validation, and the plan comparison (intent, ordered op kinds, parameter
//! subset containment per op, optional scope subset; 0.8.0: whether the plan must be
//! confirmed / must name what it leaves out, alternative expectations, and the kind
//! of turn — chat, question, command, setting — the accuracy is reported by).

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use doculens_core::planner::{OpKind, Plan, PlanContext};

pub const SCHEMA_VERSION: u32 = 1;
pub const CATEGORIES: &[&str] = &["ko", "en", "mixed"];
/// Kinds of turn (audit PLN-21): the planner contract differs per kind, so does the accuracy.
pub const KINDS: &[&str] = &["chat", "question", "command", "setting"];

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
    /// `chat` | `question` | `command` | `setting`; derived from the expectation when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl GoldenItem {
    /// The kind of turn: the item's own `kind`, else what the expected plan says.
    pub fn kind(&self) -> &str {
        if let Some(k) = self.kind.as_deref() {
            return k;
        }
        let chat = self.expected.params.first().and_then(|p| p.as_ref()).and_then(|p| p.get("mode"))
            == Some(&Value::String("chat".into()));
        match self.expected.intent.as_str() {
            "noop" => "chat",
            "answer" if chat => "chat",
            "answer" | "explain" | "summarize" | "compare" | "search" => "question",
            "set_setting" | "set_theme" | "set_language" | "set_profile" | "set_model" | "set_inbox_watch"
            | "set_retention" => "setting",
            _ => "command",
        }
    }
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
    /// Whether the plan has to wait for the user's confirmation (`true`) or must run without
    /// one (`false`). Not checked when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<bool>,
    /// `true`: the plan must say what it leaves out (`unmet`); `false`: it must not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmet: Option<bool>,
    /// Other plans that are just as right ("메일로 보내줘" may be answered with a question
    /// instead of a partial plan). The item is correct when this or any alternative matches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alt: Vec<Expected>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mismatch {
    Intent {
        expected: String,
        produced: String,
    },
    Ops {
        expected: Vec<String>,
        produced: Vec<String>,
    },
    Params {
        index: usize,
        expected: Value,
        produced: Value,
    },
    Scope {
        expected: Value,
        produced: Value,
    },
    /// `requires_confirmation` or `unmet` differs.
    Plan {
        field: String,
        expected: Value,
        produced: Value,
    },
}

impl Mismatch {
    pub fn kind(&self) -> &'static str {
        match self {
            Mismatch::Intent { .. } => "intent",
            Mismatch::Ops { .. } => "ops",
            Mismatch::Params { .. } => "params",
            Mismatch::Scope { .. } => "scope",
            Mismatch::Plan { .. } => "plan",
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

/// Every way the produced plan differs from the expectation (empty = correct). With
/// alternatives, the differences from the closest one.
pub fn compare(expected: &Expected, plan: &Plan) -> Vec<Mismatch> {
    let mut best = compare_one(expected, plan);
    for alt in &expected.alt {
        if best.is_empty() {
            break;
        }
        let other = compare_one(alt, plan);
        if other.len() < best.len() {
            best = other;
        }
    }
    best
}

fn compare_one(expected: &Expected, plan: &Plan) -> Vec<Mismatch> {
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
    if let Some(confirm) = expected.confirm {
        if plan.requires_confirmation != confirm {
            out.push(Mismatch::Plan {
                field: "requires_confirmation".into(),
                expected: Value::Bool(confirm),
                produced: Value::Bool(plan.requires_confirmation),
            });
        }
    }
    if let Some(unmet) = expected.unmet {
        let produced = plan.unmet.as_deref().map(str::trim).unwrap_or_default();
        if produced.is_empty() == unmet {
            out.push(Mismatch::Plan {
                field: "unmet".into(),
                expected: Value::Bool(unmet),
                produced: Value::String(produced.to_string()),
            });
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
        if let Some(kind) = &item.kind {
            if !KINDS.contains(&kind.as_str()) {
                return Err(format!("{id}: kind {kind:?} (허용: chat|question|command|setting)"));
            }
        }
        if item.text.trim().is_empty() {
            return Err(format!("{id}: text 가 비어 있음"));
        }
        validate_expected(id, &item.expected)?;
        for (n, alt) in item.expected.alt.iter().enumerate() {
            if !alt.alt.is_empty() {
                return Err(format!("{id}: alt[{n}] 안에 alt 를 다시 둘 수 없음"));
            }
            validate_expected(&format!("{id} alt[{n}]"), alt)?;
        }
        if let Some(c) = &item.context {
            if !c.is_object() {
                return Err(format!("{id}: context 는 객체여야 함"));
            }
        }
    }
    Ok(())
}

fn validate_expected(id: &str, expected: &Expected) -> Result<(), String> {
    if OpKind::parse(&expected.intent).is_none() {
        return Err(format!("{id}: 알 수 없는 intent {:?}", expected.intent));
    }
    if expected.ops.is_empty() {
        return Err(format!("{id}: ops 가 비어 있음"));
    }
    for op in &expected.ops {
        if OpKind::parse(op).is_none() {
            return Err(format!("{id}: 알 수 없는 op {op:?}"));
        }
    }
    if expected.params.len() > expected.ops.len() {
        return Err(format!("{id}: params({})가 ops({})보다 김", expected.params.len(), expected.ops.len()));
    }
    for (i, p) in expected.params.iter().enumerate() {
        if let Some(p) = p {
            if !p.is_object() {
                return Err(format!("{id}: params[{i}] 는 객체 또는 null 이어야 함"));
            }
        }
    }
    if let Some(s) = &expected.scope {
        if !s.is_object() {
            return Err(format!("{id}: scope 는 객체여야 함"));
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
            confirm: None,
            unmet: None,
            alt: Vec::new(),
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
            confirm: None,
            unmet: None,
            alt: Vec::new(),
        };
        let m = compare(&wrong_ops, &plan);
        assert_eq!(m.iter().map(Mismatch::kind).collect::<Vec<_>>(), vec!["intent", "ops"]);
        // … unless an alternative expectation matches.
        let either = Expected { alt: vec![ok.clone()], ..wrong_ops.clone() };
        assert!(compare(&either, &plan).is_empty());

        // Plan-level expectations: confirmation and what the plan leaves out.
        let confirmed = Expected { confirm: Some(true), ..ok.clone() };
        assert!(
            matches!(compare(&confirmed, &plan).as_slice(), [Mismatch::Plan { field, .. }] if field == "requires_confirmation")
        );
        let names_the_gap = Expected { unmet: Some(true), confirm: Some(true), ..ok.clone() };
        assert_eq!(compare(&names_the_gap, &plan).len(), 2);
        let mut partial = plan.clone();
        partial.unmet = Some("메일로 보내기".into());
        partial.requires_confirmation = true;
        assert!(compare(&names_the_gap, &partial).is_empty());
        assert!(matches!(
            compare(&Expected { unmet: Some(false), ..ok.clone() }, &partial).as_slice(),
            [Mismatch::Plan { field, .. }] if field == "unmet"
        ));

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
    fn kinds_of_turn_come_from_the_expectation_unless_given() {
        let item = |expected: Value, kind: Option<&str>| -> GoldenItem {
            let mut v = json!({ "id": "k", "category": "ko", "text": "t", "expected": expected });
            if let Some(k) = kind {
                v["kind"] = json!(k);
            }
            serde_json::from_value(v).unwrap()
        };
        let kind_of = |expected: Value| item(expected, None).kind().to_string();
        assert_eq!(kind_of(json!({ "intent": "answer", "ops": ["answer"], "params": [{ "mode": "chat" }] })), "chat");
        assert_eq!(kind_of(json!({ "intent": "noop", "ops": ["noop"] })), "chat");
        assert_eq!(
            kind_of(json!({ "intent": "answer", "ops": ["answer"], "params": [{ "mode": "general" }] })),
            "question"
        );
        assert_eq!(kind_of(json!({ "intent": "summarize", "ops": ["summarize"] })), "question");
        assert_eq!(kind_of(json!({ "intent": "set_setting", "ops": ["set_setting"] })), "setting");
        assert_eq!(kind_of(json!({ "intent": "set_theme", "ops": ["set_theme", "set_model"] })), "setting");
        assert_eq!(kind_of(json!({ "intent": "import", "ops": ["import", "move"] })), "command");
        // A counter-example keeps the kind it is about: "배율이 뭐야?" is a setting-shaped question.
        assert_eq!(item(json!({ "intent": "answer", "ops": ["answer"] }), Some("setting")).kind(), "setting");
        let set: GoldenSet = serde_json::from_value(json!({ "schema_version": 1, "items": [
            { "id": "a", "category": "ko", "kind": "poem", "text": "x", "expected": { "intent": "search", "ops": ["search"] } }
        ] }))
        .unwrap();
        assert!(validate(&set).unwrap_err().contains("kind"));
        let set: GoldenSet = serde_json::from_value(json!({ "schema_version": 1, "items": [
            { "id": "a", "category": "ko", "text": "x", "expected": { "intent": "search", "ops": ["search"],
              "alt": [{ "intent": "teleport", "ops": ["teleport"] }] } }
        ] }))
        .unwrap();
        assert!(validate(&set).unwrap_err().contains("alt[0]"));
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
