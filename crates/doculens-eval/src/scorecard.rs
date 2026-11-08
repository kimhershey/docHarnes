//! `scorecard` subcommand: merges several results files (one per model /
//! quantisation / engine build) into the quantisation scorecard table (design
//! §3.11 "양자화별 스코어카드").

use std::path::Path;

use chrono::Utc;
use serde_json::Value;

use crate::cli::Args;

const KNOWN_FLAGS: &[&str] = &["inputs", "out", "title", "help"];

pub const USAGE: &str = "doculens-eval scorecard --inputs <results.json>... --out <scorecard.md> [--title <text>]";

#[derive(Debug, Default)]
pub struct Row {
    source: String,
    kind: String,
    model: String,
    quant: String,
    engine: String,
    device: String,
    class_f1: Option<f64>,
    class_acc: Option<f64>,
    field_em: Option<f64>,
    anls: Option<f64>,
    cer: Option<f64>,
    p50_ms: Option<f64>,
    p95_ms: Option<f64>,
    vram_mib: Option<f64>,
    planner_acc: Option<f64>,
    generated_at: String,
}

/// Quantisation label from a model id / file name (`q4_k_m` → `Q4_K_M`).
pub fn quant_from_name(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    let tokens: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').collect();
    // Try the longest well-known spellings first.
    const KNOWN: &[&str] = &[
        "q4_k_m", "q4_k_s", "q5_k_m", "q5_k_s", "q3_k_m", "q3_k_s", "q3_k_l", "q2_k", "q6_k", "q8_0", "q4_0", "q4_1",
        "q5_0", "q5_1", "iq4_xs", "iq4_nl", "iq3_m", "iq2_m", "bf16", "f16", "f32", "q4km", "q5km", "q6k", "q8",
    ];
    for k in KNOWN {
        if tokens.iter().any(|t| t == k) || lower.contains(&format!("-{k}")) || lower.ends_with(k) {
            let pretty = match *k {
                "q4km" => "Q4_K_M",
                "q5km" => "Q5_K_M",
                "q6k" => "Q6_K",
                "q8" => "Q8_0",
                other => return Some(other.to_uppercase()),
            };
            return Some(pretty.to_string());
        }
    }
    None
}

fn f(v: &Value, path: &[&str]) -> Option<f64> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.as_f64()
}

fn s(v: &Value, path: &[&str]) -> Option<String> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.as_str().map(String::from)
}

fn row_of(path: &Path, v: &Value) -> Row {
    let kind = s(v, &["kind"]).unwrap_or_else(|| "?".into());
    let model = s(v, &["model"]).or_else(|| s(v, &["engine", "model"])).unwrap_or_else(|| "–".into());
    let quant = s(v, &["quantization"])
        .or_else(|| s(v, &["engine", "quantization"]))
        .or_else(|| quant_from_name(&model))
        .unwrap_or_else(|| "–".into());
    let engine = match (s(v, &["engine", "backend"]), f(v, &["engine", "build_number"])) {
        (Some(b), Some(n)) => format!("{b} b{}", n as u64),
        (Some(b), None) => b,
        (None, Some(n)) => format!("b{}", n as u64),
        (None, None) => "–".into(),
    };
    // The engine report lists the devices it sees; an empty list is a CPU run, no report = unknown.
    let device = match v.get("engine").and_then(|e| e.get("devices")).and_then(Value::as_array) {
        Some(devices) => match devices.first() {
            Some(d) => format!(
                "{} ({} MiB)",
                d.get("description").and_then(Value::as_str).unwrap_or("?"),
                d.get("total_mib").and_then(Value::as_u64).unwrap_or(0)
            ),
            None => "CPU".into(),
        },
        None => "–".into(),
    };
    let mut row = Row {
        source: path.display().to_string(),
        kind: kind.clone(),
        model,
        quant,
        engine,
        device,
        generated_at: s(v, &["generated_at"]).unwrap_or_default(),
        ..Default::default()
    };
    if kind == "pipeline" {
        row.class_f1 = f(v, &["metrics", "classification", "macro_f1"]);
        row.class_acc = f(v, &["metrics", "classification", "accuracy"]);
        row.field_em = f(v, &["metrics", "fields", "exact_match_rate"]);
        row.anls = f(v, &["metrics", "fields", "anls_star_mean"]);
        row.cer = f(v, &["metrics", "ocr", "mean_cer"]);
        row.p50_ms = f(v, &["metrics", "latency", "p50_ms"]);
        row.p95_ms = f(v, &["metrics", "latency", "p95_ms"]);
        row.vram_mib = f(v, &["metrics", "vram", "peak_used_mib"]).or_else(|| f(v, &["engine", "vram_used_mib"]));
    } else if kind == "planner" {
        row.planner_acc = f(v, &["accuracy"]);
        row.vram_mib = f(v, &["engine", "vram_used_mib"]);
        row.p50_ms = f(v, &["latency", "p50_ms"]);
        row.p95_ms = f(v, &["latency", "p95_ms"]);
    }
    row
}

fn pct(v: Option<f64>) -> String {
    v.map(|x| format!("{:.1}%", x * 100.0)).unwrap_or_else(|| "–".into())
}

fn num(v: Option<f64>, digits: usize) -> String {
    v.map(|x| format!("{x:.digits$}")).unwrap_or_else(|| "–".into())
}

fn ms(v: Option<f64>) -> String {
    v.map(|x| format!("{:.0} ms", x)).unwrap_or_else(|| "–".into())
}

fn mib(v: Option<f64>) -> String {
    v.map(|x| format!("{:.0} MiB", x)).unwrap_or_else(|| "–".into())
}

pub fn render(rows: &[Row], title: &str) -> String {
    let mut md = String::new();
    md.push_str(&format!("# {title}\n\n"));
    md.push_str(&format!(
        "생성: {} · 입력 {}건 · `doculens-eval scorecard`로 생성됨 (지표 정의: `eval/README.md`)\n\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        rows.len()
    ));
    md.push_str("| 모델 | 양자화 | 엔진 | 장치 | 분류 F1 | 분류 정확도 | 필드 정확일치 | ANLS* | OCR CER | 페이지 p50 | 페이지 p95 | VRAM 피크 | 플래너 정확도 |\n");
    md.push_str("|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for r in rows {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.model,
            r.quant,
            r.engine,
            r.device,
            num(r.class_f1, 3),
            pct(r.class_acc),
            pct(r.field_em),
            num(r.anls, 3),
            pct(r.cer),
            ms(r.p50_ms),
            ms(r.p95_ms),
            mib(r.vram_mib),
            pct(r.planner_acc)
        ));
    }
    md.push_str("\n## 입력 파일\n\n");
    for r in rows {
        md.push_str(&format!("- `{}` — {} ({})\n", r.source, r.kind, r.generated_at));
    }
    md.push_str("\n> 분류 F1은 문서 유형 macro F1, 필드 정확일치는 정규화 후 일치율, ANLS*는 누락·환각 키 패널티를 포함한 문서 평균, OCR CER은 NFC 정규화 문자 오류율(낮을수록 좋음), 페이지 p50/p95는 문서 처리 시간을 페이지 수로 나눈 분포, VRAM 피크는 엔진이 보고한 장치 사용량(전체 − 여유)의 최대값입니다. 플래너 정확도는 `planner` 결과 파일에서만 채워집니다.\n");
    md
}

pub fn run(args: &Args) -> Result<i32, String> {
    let unknown = args.unknown_flags(KNOWN_FLAGS);
    if !unknown.is_empty() {
        return Err(format!("알 수 없는 옵션 {}\n{USAGE}", unknown.join(" ")));
    }
    let inputs = args.opt_many("inputs");
    if inputs.is_empty() {
        return Err(format!("--inputs 에 결과 파일을 하나 이상 주세요\n{USAGE}"));
    }
    let out = Path::new(args.opt("out").ok_or_else(|| format!("--out 이 필요합니다\n{USAGE}"))?);
    let mut rows = Vec::new();
    for input in &inputs {
        let p = Path::new(input);
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("{}: JSON 오류: {e}", p.display()))?;
        match v.get("kind").and_then(Value::as_str) {
            Some("pipeline") | Some("planner") => rows.push(row_of(p, &v)),
            other => return Err(format!("{}: 알 수 없는 결과 종류 {other:?}", p.display())),
        }
    }
    rows.sort_by(|a, b| a.model.cmp(&b.model).then(a.quant.cmp(&b.quant)).then(a.kind.cmp(&b.kind)));
    let md = render(&rows, args.opt("title").unwrap_or("양자화 스코어카드"));
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(out, &md).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("스코어카드 저장: {} ({}행)", out.display(), rows.len());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_quantisation_labels() {
        assert_eq!(quant_from_name("qwen3.5-4b-q6k").as_deref(), Some("Q6_K"));
        assert_eq!(quant_from_name("Qwen3.5-4B-Q4_K_M.gguf").as_deref(), Some("Q4_K_M"));
        assert_eq!(quant_from_name("qwen3.5-9b-q4km").as_deref(), Some("Q4_K_M"));
        assert_eq!(quant_from_name("mmproj-F16").as_deref(), Some("F16"));
        assert_eq!(quant_from_name("tiny-qwen35"), None);
    }

    #[test]
    fn renders_rows_from_both_result_kinds() {
        let pipeline = json!({
            "kind": "pipeline", "model": "qwen3.5-4b-q4km", "generated_at": "2026-11-08T00:00:00Z",
            "engine": { "backend": "cu121", "build_number": 12066, "devices": [{ "description": "NVIDIA GeForce RTX 4060", "total_mib": 8188 }], "vram_used_mib": 4300 },
            "metrics": { "classification": { "macro_f1": 0.9, "accuracy": 0.95 }, "fields": { "exact_match_rate": 0.88, "anls_star_mean": 0.91 },
                         "ocr": { "mean_cer": 0.032 }, "latency": { "p50_ms": 4210.0, "p95_ms": 9800.0 }, "vram": { "peak_used_mib": 5900 } }
        });
        let planner = json!({ "kind": "planner", "accuracy": 0.75, "engine": { "model": "qwen3.5-4b-q6k" }, "generated_at": "x" });
        let rows = vec![row_of(Path::new("a.json"), &pipeline), row_of(Path::new("b.json"), &planner)];
        assert_eq!(rows[0].quant, "Q4_K_M");
        assert_eq!(rows[0].engine, "cu121 b12066");
        assert_eq!(rows[1].model, "qwen3.5-4b-q6k");
        let md = render(&rows, "테스트");
        assert!(md.contains("| qwen3.5-4b-q4km | Q4_K_M | cu121 b12066 | NVIDIA GeForce RTX 4060 (8188 MiB) | 0.900 | 95.0% | 88.0% | 0.910 | 3.2% | 4210 ms | 9800 ms | 5900 MiB | – |"), "{md}");
        assert!(md.contains("| qwen3.5-4b-q6k | Q6_K | – | – | – | – | – | – | – | – | – | – | 75.0% |"), "{md}");
    }
}
