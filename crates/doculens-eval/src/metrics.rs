//! Metric definitions (design §3.11): character error rate on NFC-normalised
//! text, ANLS* with missing / hallucinated-key penalties, document-type macro
//! F1 / accuracy and nearest-rank percentiles for latencies.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// ANLS similarity below this counts as zero (ANLS / ANLS* convention).
pub const ANLS_THRESHOLD: f64 = 0.5;

/// NFC normalisation plus whitespace collapsing (runs → one space, trimmed).
pub fn normalize_text(s: &str) -> String {
    let nfc = doculens_core::documents::nfc(s);
    nfc.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Character-level Levenshtein distance (two-row dynamic programme).
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Character error rate: edit distance over the reference length, both sides
/// NFC-normalised with whitespace collapsed. An empty reference scores 0 when
/// the hypothesis is empty too, else 1.
pub fn cer(expected: &str, produced: &str) -> f64 {
    let e = normalize_text(expected);
    let p = normalize_text(produced);
    let n = e.chars().count();
    if n == 0 {
        return if p.is_empty() { 0.0 } else { 1.0 };
    }
    levenshtein(&e, &p) as f64 / n as f64
}

/// Normalised Levenshtein similarity `1 − d / max(len)` (1 for two empty strings).
pub fn similarity(a: &str, b: &str) -> f64 {
    let m = a.chars().count().max(b.chars().count());
    if m == 0 {
        return 1.0;
    }
    1.0 - levenshtein(a, b) as f64 / m as f64
}

/// ANLS* over flattened keys for one document. Each expected key contributes the
/// thresholded similarity of its (pre-normalised) value; a key expected empty
/// (`None`) scores 1 only when the prediction is empty too; a missing key scores
/// 0. With `strict_keys` every produced non-empty key that is not expected is a
/// hallucination and enlarges the denominator (full labelling); without it,
/// unlisted produced keys are ignored (partial labelling). `None` when nothing is
/// expected.
pub fn anls_star(
    expected: &BTreeMap<String, Option<String>>,
    produced: &BTreeMap<String, String>,
    strict_keys: bool,
) -> Option<f64> {
    if expected.is_empty() {
        return None;
    }
    let mut total = 0.0;
    for (key, exp) in expected {
        let got = produced.get(key).map(|s| s.trim()).filter(|s| !s.is_empty());
        let exp = exp.as_deref().map(str::trim).filter(|s| !s.is_empty());
        total += match (exp, got) {
            (None, None) => 1.0,
            (None, Some(_)) | (Some(_), None) => 0.0,
            (Some(e), Some(g)) => {
                let s = similarity(e, g);
                if s >= ANLS_THRESHOLD {
                    s
                } else {
                    0.0
                }
            }
        };
    }
    let hallucinated = if strict_keys {
        produced.iter().filter(|(k, v)| !expected.contains_key(*k) && !v.trim().is_empty()).count()
    } else {
        0
    };
    Some(total / (expected.len() + hallucinated) as f64)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelScore {
    pub label: String,
    pub tp: u32,
    pub fp: u32,
    #[serde(rename = "fn")]
    pub fn_: u32,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub total: u32,
    pub correct: u32,
    pub accuracy: f64,
    /// Unweighted mean F1 over every label that occurs in the gold or the prediction.
    pub macro_f1: f64,
    pub labels: Vec<LabelScore>,
}

/// Accuracy and macro F1 of `(expected, predicted)` label pairs.
pub fn classification(pairs: &[(String, String)]) -> Option<Classification> {
    if pairs.is_empty() {
        return None;
    }
    let mut labels: Vec<&str> = pairs.iter().flat_map(|(e, p)| [e.as_str(), p.as_str()]).collect();
    labels.sort_unstable();
    labels.dedup();
    let mut scores = Vec::new();
    for label in labels {
        let tp = pairs.iter().filter(|(e, p)| e == label && p == label).count() as u32;
        let fp = pairs.iter().filter(|(e, p)| e != label && p == label).count() as u32;
        let fn_ = pairs.iter().filter(|(e, p)| e == label && p != label).count() as u32;
        let precision = if tp + fp == 0 { 0.0 } else { f64::from(tp) / f64::from(tp + fp) };
        let recall = if tp + fn_ == 0 { 0.0 } else { f64::from(tp) / f64::from(tp + fn_) };
        let f1 = if precision + recall == 0.0 { 0.0 } else { 2.0 * precision * recall / (precision + recall) };
        scores.push(LabelScore { label: label.to_string(), tp, fp, fn_, precision, recall, f1 });
    }
    let correct = pairs.iter().filter(|(e, p)| e == p).count() as u32;
    let macro_f1 = scores.iter().map(|s| s.f1).sum::<f64>() / scores.len() as f64;
    Some(Classification {
        total: pairs.len() as u32,
        correct,
        accuracy: f64::from(correct) / pairs.len() as f64,
        macro_f1,
        labels: scores,
    })
}

/// Nearest-rank percentile (`p` in 0..=100) of the samples; `None` when empty.
pub fn percentile(samples: &[f64], p: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted: Vec<f64> = samples.iter().copied().filter(|x| x.is_finite()).collect();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let rank = ((p.clamp(0.0, 100.0) / 100.0) * n as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, n) - 1])
}

pub fn mean(samples: &[f64]) -> Option<f64> {
    if samples.is_empty() {
        None
    } else {
        Some(samples.iter().sum::<f64>() / samples.len() as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map<const N: usize>(pairs: [(&str, Option<&str>); N]) -> BTreeMap<String, Option<String>> {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v.map(String::from))).collect()
    }

    fn got<const N: usize>(pairs: [(&str, &str); N]) -> BTreeMap<String, String> {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn levenshtein_and_cer() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("세금계산서", "세금계산서"), 0);
        assert_eq!(levenshtein("세금계산서", "세금 계산서"), 1);
        assert_eq!(cer("공급가액 1,000,000", "공급가액 1,000,000"), 0.0);
        // NFC: decomposed Hangul (NFD) equals its precomposed form.
        assert_eq!(cer("\u{1112}\u{1161}\u{11AB}\u{1100}\u{1173}\u{11AF}", "한글"), 0.0);
        // Whitespace runs collapse; one substituted character out of ten → 0.1.
        assert!((cer("abcd  fghij", "abcd fghiX") - 0.1).abs() < 1e-9);
        assert_eq!(cer("", ""), 0.0);
        assert_eq!(cer("", "x"), 1.0);
        assert_eq!(cer("abc", ""), 1.0);
        assert!((similarity("abcd", "abxx") - 0.5).abs() < 1e-9);
        assert_eq!(similarity("", ""), 1.0);
    }

    #[test]
    fn anls_star_penalises_missing_and_hallucinated_keys() {
        let exp = map([("a", Some("1000000")), ("b", Some("2026-10-14")), ("c", None)]);
        // Perfect: expected-empty key is empty too.
        assert_eq!(anls_star(&exp, &got([("a", "1000000"), ("b", "2026-10-14")]), true), Some(1.0));
        // Missing key b → 0 for b: (1 + 0 + 1) / 3.
        let s = anls_star(&exp, &got([("a", "1000000")]), true).unwrap();
        assert!((s - 2.0 / 3.0).abs() < 1e-9, "{s}");
        // Below the threshold counts as zero, at/above keeps the similarity.
        let s = anls_star(&map([("a", Some("abcd"))]), &got([("a", "abxx")]), true).unwrap();
        assert!((s - 0.5).abs() < 1e-9);
        assert_eq!(anls_star(&map([("a", Some("abcd"))]), &got([("a", "xxxx")]), true), Some(0.0));
        // Hallucinated key: strict → denominator 2, lenient → ignored.
        let one = map([("a", Some("x"))]);
        assert_eq!(anls_star(&one, &got([("a", "x"), ("z", "junk")]), true), Some(0.5));
        assert_eq!(anls_star(&one, &got([("a", "x"), ("z", "junk")]), false), Some(1.0));
        // An expected-empty key that is produced non-empty scores zero.
        assert_eq!(anls_star(&map([("c", None)]), &got([("c", "oops")]), false), Some(0.0));
        assert_eq!(anls_star(&BTreeMap::new(), &got([("c", "oops")]), true), None);
    }

    #[test]
    fn macro_f1_and_accuracy() {
        let pairs: Vec<(String, String)> = [
            ("receipt", "receipt"),
            ("receipt", "receipt"),
            ("receipt", "tax_invoice"),
            ("tax_invoice", "tax_invoice"),
            ("contract", "receipt"),
        ]
        .into_iter()
        .map(|(e, p)| (e.to_string(), p.to_string()))
        .collect();
        let c = classification(&pairs).unwrap();
        assert_eq!(c.total, 5);
        assert_eq!(c.correct, 3);
        assert!((c.accuracy - 0.6).abs() < 1e-9);
        // receipt: tp 2, fp 1, fn 1 → p 2/3 r 2/3 f1 2/3; tax_invoice: tp 1, fp 1, fn 0 → f1 2/3; contract: f1 0.
        let expected = (2.0 / 3.0 + 2.0 / 3.0 + 0.0) / 3.0;
        assert!((c.macro_f1 - expected).abs() < 1e-9, "{}", c.macro_f1);
        let contract = c.labels.iter().find(|l| l.label == "contract").unwrap();
        assert_eq!((contract.tp, contract.fp, contract.fn_), (0, 0, 1));
        assert!(classification(&[]).is_none());
    }

    #[test]
    fn nearest_rank_percentiles() {
        let s: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(percentile(&s, 50.0), Some(5.0));
        assert_eq!(percentile(&s, 95.0), Some(10.0));
        assert_eq!(percentile(&s, 0.0), Some(1.0));
        assert_eq!(percentile(&s, 100.0), Some(10.0));
        assert_eq!(percentile(&[7.0], 95.0), Some(7.0));
        assert_eq!(percentile(&[], 50.0), None);
        assert_eq!(mean(&[1.0, 2.0, 6.0]), Some(3.0));
    }
}
