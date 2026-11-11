//! Order-preserving JSON output for grammar-bound model requests (0.8.0 plan §1.1).
//!
//! `serde_json::Value` keeps object keys sorted, so a JSON Schema built with `json!` is
//! serialised with its `properties` in alphabetical order — and llama.cpp's schema-to-grammar
//! converter turns that order into the order the model MUST write the keys in. A plan then starts
//! with `confidence` and ends with `summary`, and `set_setting` has to begin with `"key"` instead
//! of `"op"` (audit PLN-1).
//!
//! The schemas of this crate list their properties in `required` in the order the model should
//! write them (`summary` first, `op` first, `title → fields → tables → body → notes`). This module
//! writes JSON so that order survives:
//!
//! * [`to_string`]: a value where every object that has both `properties` (object) and `required`
//!   (array of strings) gets its `properties` written in `required` order, the rest after it.
//!   Everything else is written as `serde_json` would.
//! * [`instance_to_string`]: an instance of such a schema (a few-shot reply) with its keys in the
//!   same order the grammar will force, so the examples and the grammar agree.

use serde_json::{Map, Value};

/// Serialises `value`, writing JSON-Schema `properties` in `required` order.
pub fn to_string(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Object(map) => write_object(out, map),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn write_key(out: &mut String, key: &str) {
    out.push_str(&Value::String(key.to_string()).to_string());
    out.push(':');
}

fn write_object(out: &mut String, map: &Map<String, Value>) {
    let order = schema_order(map);
    out.push('{');
    let mut first = true;
    for (key, value) in map {
        if !first {
            out.push(',');
        }
        first = false;
        write_key(out, key);
        match (key.as_str(), value, &order) {
            ("properties", Value::Object(props), Some(order)) => write_properties(out, props, order),
            _ => write_value(out, value),
        }
    }
    out.push('}');
}

/// The `required` list of a schema object that also has `properties`.
fn schema_order(map: &Map<String, Value>) -> Option<Vec<&str>> {
    if !map.get("properties").is_some_and(Value::is_object) {
        return None;
    }
    let required = map.get("required")?.as_array()?;
    Some(required.iter().filter_map(Value::as_str).collect())
}

fn write_properties(out: &mut String, props: &Map<String, Value>, order: &[&str]) {
    out.push('{');
    let mut first = true;
    for key in ordered_keys(props, order) {
        if !first {
            out.push(',');
        }
        first = false;
        write_key(out, key);
        write_value(out, &props[key]);
    }
    out.push('}');
}

/// `order` first (keys that exist), then the remaining keys as the map yields them.
fn ordered_keys<'a>(map: &'a Map<String, Value>, order: &[&str]) -> Vec<&'a str> {
    let mut keys: Vec<&str> = Vec::with_capacity(map.len());
    for k in order {
        if let Some((key, _)) = map.get_key_value(*k) {
            if !keys.contains(&key.as_str()) {
                keys.push(key.as_str());
            }
        }
    }
    for key in map.keys() {
        if !keys.contains(&key.as_str()) {
            keys.push(key.as_str());
        }
    }
    keys
}

/// Serialises `instance` with the key order a grammar built from `schema` forces: `required`
/// order first, then the other keys. Arrays follow `items`; an `anyOf` picks the alternative whose
/// `const` properties match the instance (the `op` of a plan operation).
pub fn instance_to_string(instance: &Value, schema: &Value) -> String {
    let mut out = String::new();
    write_instance(&mut out, instance, Some(schema));
    out
}

fn write_instance(out: &mut String, instance: &Value, schema: Option<&Value>) {
    let schema = schema.map(|s| pick_alternative(instance, s));
    match instance {
        Value::Object(map) => {
            let props = schema.and_then(|s| s.get("properties")).and_then(Value::as_object);
            let order: Vec<&str> = schema
                .and_then(|s| s.get("required"))
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            out.push('{');
            let mut first = true;
            for key in ordered_keys(map, &order) {
                if !first {
                    out.push(',');
                }
                first = false;
                write_key(out, key);
                write_instance(out, &map[key], props.and_then(|p| p.get(key)));
            }
            out.push('}');
        }
        Value::Array(items) => {
            let item_schema = schema.and_then(|s| s.get("items"));
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_instance(out, item, item_schema);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// For `{"anyOf": [...]}` the alternative whose `const` properties all match `instance`
/// (first object alternative otherwise); any other schema is returned as is.
fn pick_alternative<'a>(instance: &Value, schema: &'a Value) -> &'a Value {
    let Some(alternatives) = schema.get("anyOf").and_then(Value::as_array) else {
        return schema;
    };
    let Some(map) = instance.as_object() else {
        return schema;
    };
    let matches = |alt: &Value| {
        let Some(props) = alt.get("properties").and_then(Value::as_object) else { return false };
        let mut any_const = false;
        for (key, prop) in props {
            if let Some(expected) = prop.get("const") {
                any_const = true;
                if map.get(key) != Some(expected) {
                    return false;
                }
            }
        }
        any_const
    };
    alternatives.iter().find(|alt| matches(alt)).unwrap_or(schema)
}

/// Keys of the top-level object of `json` in the order they are written (tests, diagnostics).
pub fn top_level_keys(json: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let bytes = json.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    let mut expect_key = false;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i + 1;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if depth == 1 && expect_key {
                    if let Ok(key) = serde_json::from_str::<String>(&json[start - 1..=i.min(bytes.len() - 1)]) {
                        keys.push(key);
                    }
                    expect_key = false;
                }
            }
            b'{' | b'[' => {
                depth += 1;
                if depth == 1 && bytes[i] == b'{' {
                    expect_key = true;
                }
            }
            b'}' | b']' => depth -= 1,
            b',' if depth == 1 => expect_key = true,
            _ => {}
        }
        i += 1;
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn properties_follow_required_order() {
        let schema = json!({
            "type": "object",
            "properties": { "alpha": { "type": "string" }, "op": { "const": "x" }, "zeta": { "type": "integer" }, "extra": { "type": "boolean" } },
            "required": ["op", "zeta", "alpha"],
            "additionalProperties": false
        });
        let text = to_string(&schema);
        let props = text.split("\"properties\":").nth(1).unwrap();
        assert_eq!(
            top_level_keys(&props[..props.find("},\"required\"").unwrap() + 1]),
            ["op", "zeta", "alpha", "extra"]
        );
        // The value is unchanged: only the order differs.
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), schema);
    }

    #[test]
    fn nested_schemas_and_plain_objects() {
        let schema = json!({
            "type": "object",
            "properties": {
                "items": { "type": "array", "items": { "anyOf": [
                    { "type": "object", "properties": { "key": { "type": "string" }, "op": { "const": "set" } }, "required": ["op", "key"] }
                ] } }
            },
            "required": ["items"]
        });
        let text = to_string(&schema);
        assert!(text.contains(r#""properties":{"op":{"const":"set"},"key":{"type":"string"}}"#), "{text}");
        // An object without `required` keeps serde's order; strings with quotes / unicode round-trip.
        let plain = json!({ "b": "따옴표 \" 와 \\ 역슬래시", "a": [1, 2.5, null, true] });
        assert_eq!(serde_json::from_str::<Value>(&to_string(&plain)).unwrap(), plain);
        assert_eq!(to_string(&plain), plain.to_string());
    }

    #[test]
    fn instances_follow_their_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "confidence": { "type": "number" },
                "operations": { "type": "array", "items": { "anyOf": [
                    { "type": "object", "properties": { "op": { "const": "set_setting" }, "key": { "type": "string" }, "value": {} }, "required": ["op", "key", "value"] },
                    { "type": "object", "properties": { "op": { "const": "noop" } }, "required": ["op"] }
                ] } },
                "summary": { "type": "string" }
            },
            "required": ["summary", "operations", "confidence"]
        });
        let plan = json!({ "confidence": 0.9, "operations": [{ "key": "ui.scale_percent", "op": "set_setting", "value": 120 }], "summary": "배율 120" });
        let text = instance_to_string(&plan, &schema);
        assert_eq!(
            text,
            r#"{"summary":"배율 120","operations":[{"op":"set_setting","key":"ui.scale_percent","value":120}],"confidence":0.9}"#
        );
        assert_eq!(top_level_keys(&text), ["summary", "operations", "confidence"]);
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), plan);
    }
}
