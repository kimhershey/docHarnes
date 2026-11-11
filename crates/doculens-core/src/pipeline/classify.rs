//! Page classification (§3.4 "VLM 1차"): document type from the dynamic
//! schema enum, page rotation, blank-page and language detection; plus the
//! per-document vote.

use serde_json::Value;

use crate::engine::client::{user_message, user_message_with_image};
use crate::error::Result;

use super::prompts::{self, Stage};
use super::schema;
use super::vlm::{Reply, Vlm};
use super::{DocTypeInfo, PageClass};

/// Classifies one page from its image (preferred) or its text.
pub fn classify_page(
    vlm: &Vlm<'_>,
    doc_types: &[DocTypeInfo],
    image_url: Option<&str>,
    text: Option<&str>,
) -> Result<(PageClass, Reply)> {
    let slugs: Vec<String> = doc_types.iter().map(|d| d.slug.clone()).collect();
    let schema = schema::classify_schema(&slugs);
    let messages = match (image_url, text) {
        (Some(url), _) => vec![
            serde_json::json!({ "role": "system", "content": prompts::SYSTEM_JSON }),
            user_message_with_image(&prompts::classify(doc_types), url),
        ],
        (None, Some(t)) => {
            let p = vlm.params(Stage::Classify);
            let fixed = prompts::classify(doc_types).chars().count() + prompts::SYSTEM_JSON.chars().count();
            let budget = prompts::text_budget(vlm.ctx, 0, p.max_tokens, fixed, prompts::CLASSIFY_TEXT_CHARS);
            let shortened = prompts::truncate(t, budget.max(200));
            vec![
                serde_json::json!({ "role": "system", "content": prompts::SYSTEM_JSON }),
                user_message(&prompts::classify_text(doc_types, &shortened)),
            ]
        }
        (None, None) => {
            return Ok((
                PageClass { doc_type: "other".into(), rotation: 0, is_blank: true, language: "ko".into() },
                Reply::default(),
            ))
        }
    };
    let (v, reply) = vlm.call_json(Stage::Classify, &messages, "classify", &schema, None)?;
    Ok((parse_class(&v, &slugs), reply))
}

/// Tolerant parse of the classifier JSON (unknown types → `other`).
pub fn parse_class(v: &Value, allowed: &[String]) -> PageClass {
    let doc_type = v
        .get("doc_type")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| allowed.iter().any(|a| a == s) || s == "other")
        .unwrap_or_else(|| "other".into());
    let rotation = match v.get("rotation") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    };
    let rotation = match rotation.rem_euclid(360) {
        90 => 90,
        180 => 180,
        270 => 270,
        _ => 0,
    };
    let language = v
        .get("language")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| matches!(s.as_str(), "ko" | "en" | "mixed"))
        .unwrap_or_else(|| "ko".into());
    PageClass { doc_type, rotation, is_blank: v.get("is_blank").and_then(Value::as_bool).unwrap_or(false), language }
}

/// Document-level type and language from the page classes: majority vote over
/// non-blank pages, the first page breaking ties (and winning outright when the
/// rest is `other`).
pub fn vote(classes: &[(u32, PageClass)]) -> (String, String) {
    let mut sorted: Vec<&(u32, PageClass)> = classes.iter().filter(|(_, c)| !c.is_blank).collect();
    sorted.sort_by_key(|(n, _)| *n);
    if sorted.is_empty() {
        return ("other".into(), "ko".into());
    }
    let mut counts: Vec<(String, usize)> = Vec::new();
    for (_, c) in &sorted {
        match counts.iter_mut().find(|(t, _)| *t == c.doc_type) {
            Some(e) => e.1 += 1,
            None => counts.push((c.doc_type.clone(), 1)),
        }
    }
    let first = &sorted[0].1;
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| (a.0 != first.doc_type).cmp(&(b.0 != first.doc_type))));
    let mut doc_type = counts[0].0.clone();
    if doc_type == "other" {
        if let Some((t, _)) = counts.iter().find(|(t, _)| t != "other") {
            // A typed page beats a majority of untyped pages (cover / attachment sheets).
            doc_type = t.clone();
        }
    }
    let mut langs: Vec<(String, usize)> = Vec::new();
    for (_, c) in &sorted {
        match langs.iter_mut().find(|(t, _)| *t == c.language) {
            Some(e) => e.1 += 1,
            None => langs.push((c.language.clone(), 1)),
        }
    }
    langs.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let language = if langs.len() > 1 && langs[0].1 == langs[1].1 { "mixed".to_string() } else { langs[0].0.clone() };
    (doc_type, language)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_and_vote() {
        let allowed = vec!["receipt".to_string(), "contract".to_string()];
        let c = parse_class(
            &json!({"doc_type": "Receipt", "rotation": "90", "is_blank": false, "language": "KO"}),
            &allowed,
        );
        assert_eq!((c.doc_type.as_str(), c.rotation, c.language.as_str()), ("receipt", 90, "ko"));
        let c = parse_class(&json!({"doc_type": "memo", "rotation": 45}), &allowed);
        assert_eq!((c.doc_type.as_str(), c.rotation), ("other", 0));
        let pc = |t: &str, blank: bool, lang: &str| PageClass {
            doc_type: t.into(),
            rotation: 0,
            is_blank: blank,
            language: lang.into(),
        };
        assert_eq!(
            vote(&[(1, pc("contract", false, "ko")), (2, pc("other", false, "ko")), (3, pc("other", false, "en"))]),
            ("contract".into(), "ko".into())
        );
        assert_eq!(
            vote(&[(1, pc("receipt", false, "ko")), (2, pc("contract", false, "en"))]),
            ("receipt".into(), "mixed".into())
        );
        assert_eq!(vote(&[(1, pc("other", true, "ko"))]), ("other".into(), "ko".into()));
        assert_eq!(
            vote(&[
                (2, pc("contract", false, "en")),
                (1, pc("receipt", false, "en")),
                (3, pc("contract", false, "en"))
            ])
            .0,
            "contract"
        );
    }
}
