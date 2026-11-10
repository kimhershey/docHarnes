//! Grammar-safe JSON schema of the model's draft reply (same conventions as
//! `pipeline::schema::json_schema`: `additionalProperties: false` and
//! `required` on every object, enum consts, no `$ref`). The field keys and
//! table keys of the template are enumerated so the model cannot invent
//! fields; the body is capped at [`MAX_BODY_BLOCKS`] blocks so a reply fits a
//! 4K–8K context next to the prompt.
//!
//! ```json
//! { "title": "…",
//!   "fields": [{ "key": "<enum>", "value": "…", "source_index": 1 | null }],
//!   "tables": [{ "key": "<enum>", "rows": [["…", "…"]] }],
//!   "body":   [{ "kind": "heading|paragraph|list", "text": "…", "items": ["…"], "source_index": null }],
//!   "notes":  ["…"] }
//! ```

use serde_json::{json, Value};

use super::templates::TemplateSpec;

/// Name of the `response_format` schema (the scripted test backend keys replies by it).
pub const SCHEMA_NAME: &str = "draft_document";
/// Outline call of a split draft (0.3.0 item 4): title + section plan.
pub const OUTLINE_SCHEMA_NAME: &str = "draft_outline";
/// One section's body blocks of a split draft.
pub const SECTION_SCHEMA_NAME: &str = "draft_section";
/// A revised or inserted section (0.7.0 `DraftRequest.revise`): the same `{ blocks }` shape as a section.
pub const REVISE_SCHEMA_NAME: &str = "draft_revise";
pub const MAX_OUTLINE_SECTIONS: usize = 12;
pub const MAX_BODY_BLOCKS: usize = 24;
pub const MAX_LIST_ITEMS: usize = 12;
pub const MAX_TABLE_ROWS: usize = 40;
pub const MAX_NOTES: usize = 6;

fn enum_of(keys: &[String]) -> Value {
    json!({ "enum": keys.iter().map(|k| Value::String(k.clone())).collect::<Vec<_>>() })
}

/// Body block list (`heading | paragraph | list` with an optional source index).
fn body_schema() -> Value {
    json!({
        "type": "array",
        "maxItems": MAX_BODY_BLOCKS,
        "items": {
            "type": "object",
            "properties": {
                "kind": { "enum": ["heading", "paragraph", "list"] },
                "text": { "type": "string" },
                "items": { "type": "array", "maxItems": MAX_LIST_ITEMS, "items": { "type": "string" } },
                "source_index": { "type": ["integer", "null"] }
            },
            "required": ["kind", "text", "items", "source_index"],
            "additionalProperties": false
        }
    })
}

/// JSON Schema of the outline reply: `{ "title", "sections": [{ "heading", "weight", "notes" }] }`.
pub fn outline_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "sections": {
                "type": "array",
                "maxItems": MAX_OUTLINE_SECTIONS,
                "items": {
                    "type": "object",
                    "properties": {
                        "heading": { "type": "string" },
                        "weight": { "type": "integer" },
                        "notes": { "type": "string" }
                    },
                    "required": ["heading", "weight", "notes"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["title", "sections"],
        "additionalProperties": false
    })
}

/// JSON Schema of one section reply: `{ "blocks": [...] }` (the section heading is added by the assembler).
pub fn section_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "blocks": body_schema() },
        "required": ["blocks"],
        "additionalProperties": false
    })
}

/// JSON Schema for the draft reply of `spec`.
pub fn json_schema(spec: &TemplateSpec) -> Value {
    let field_keys: Vec<String> = spec.scalars().map(|f| f.key.clone()).collect();
    let table_keys: Vec<String> = spec.tables().map(|f| f.key.clone()).collect();
    let fields = if field_keys.is_empty() {
        json!({ "type": "array", "maxItems": 0, "items": { "type": "string" } })
    } else {
        json!({
            "type": "array",
            "maxItems": field_keys.len(),
            "items": {
                "type": "object",
                "properties": {
                    "key": enum_of(&field_keys),
                    "value": { "type": "string" },
                    "source_index": { "type": ["integer", "null"] }
                },
                "required": ["key", "value", "source_index"],
                "additionalProperties": false
            }
        })
    };
    let tables = if table_keys.is_empty() {
        json!({ "type": "array", "maxItems": 0, "items": { "type": "string" } })
    } else {
        json!({
            "type": "array",
            "maxItems": table_keys.len(),
            "items": {
                "type": "object",
                "properties": {
                    "key": enum_of(&table_keys),
                    "rows": {
                        "type": "array",
                        "maxItems": MAX_TABLE_ROWS,
                        "items": { "type": "array", "items": { "type": "string" } }
                    }
                },
                "required": ["key", "rows"],
                "additionalProperties": false
            }
        })
    };
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "fields": fields,
            "tables": tables,
            "body": body_schema(),
            "notes": { "type": "array", "maxItems": MAX_NOTES, "items": { "type": "string" } }
        },
        "required": ["title", "fields", "tables", "body", "notes"],
        "additionalProperties": false
    })
}

/// `true` when every object in the schema closes `additionalProperties` and lists
/// all of its properties as required, and nothing uses `$ref` / `oneOf` (the
/// llama.cpp grammar restrictions of §3.4.1).
pub fn is_grammar_safe(schema: &Value) -> bool {
    fn walk(v: &Value) -> bool {
        match v {
            Value::Object(m) => {
                if m.contains_key("$ref")
                    || m.contains_key("$defs")
                    || m.contains_key("oneOf")
                    || m.contains_key("anyOf")
                {
                    return false;
                }
                if m.get("type") == Some(&Value::String("object".into())) {
                    if m.get("additionalProperties") != Some(&Value::Bool(false)) {
                        return false;
                    }
                    let props: Vec<&String> =
                        m.get("properties").and_then(Value::as_object).map(|p| p.keys().collect()).unwrap_or_default();
                    let required: Vec<&str> = m
                        .get("required")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    if props.iter().any(|p| !required.contains(&p.as_str())) {
                        return false;
                    }
                }
                m.values().all(walk)
            }
            Value::Array(a) => a.iter().all(walk),
            _ => true,
        }
    }
    walk(schema)
}
