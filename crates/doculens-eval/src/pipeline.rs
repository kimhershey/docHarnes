//! `pipeline` subcommand: the document pipeline (import → classify → OCR →
//! extract) over a golden manifest with a real `llama-server`, scored with the
//! §3.11 metrics — document-type accuracy / macro F1, OCR CER, field exact match
//! after normalisation, ANLS*, per-page latency p50 / p95 and the VRAM the
//! engine reports. `--dry-run` validates the manifest and writes the report
//! skeleton without an engine.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use doculens_core::documents::convert::StubPdfPrinter;
use doculens_core::documents::import::{self, ImportDeps, ImportOptions, ImportSource};
use doculens_core::documents::render::{self, RenderOptions, RenderService};
use doculens_core::documents::repo as docs;
use doculens_core::pipeline::run::{process, PipelineDeps};
use doculens_core::pipeline::{
    repo as pipe_repo, validate, FieldKind, FieldRecord, PipelineLevel, ProcessRequest, SchemaChoice,
};
use doculens_core::settings::{AutoProcessingLevel, ProcessingSettings, Profile};
use doculens_core::workspace::{OpenOptions, Workspace};

use crate::cli::Args;
use crate::engine::{self, EngineInfo, EngineOpts, RunningEngine};
use crate::metrics::{self, Classification};

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const RESULTS_SCHEMA_VERSION: u32 = 1;

const KNOWN_FLAGS: &[&str] = &[
    "manifest",
    "engine",
    "models-dir",
    "model",
    "out",
    "pdfium",
    "dry-run",
    "backend",
    "profile",
    "workspace",
    "limit",
    "precise",
    "verbose",
    "help",
];

pub const USAGE: &str = "doculens-eval pipeline --manifest <json> --out <dir> [--dry-run]
                       [--engine <llama-server> --models-dir <dir> --model <id>] [--pdfium <lib>]
                       [--backend cpu|vulkan|cu121|cu129] [--profile accuracy|balanced|speed] [--workspace <dir>]
                       [--limit <n>] [--precise] [--verbose]";

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    #[serde(default)]
    pub name: Option<String>,
    /// Every produced non-empty field that the manifest does not list counts as a
    /// hallucination (complete labelling). Off = partial labelling.
    #[serde(default)]
    pub strict_keys: bool,
    pub documents: Vec<ManifestDoc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestDoc {
    /// Relative to the manifest file, or absolute.
    pub path: String,
    pub doc_type: String,
    /// Field path → expected value (string / number; `null` = must be empty).
    #[serde(default)]
    pub fields: BTreeMap<String, Value>,
    /// Expected OCR text of the scored pages (optional).
    #[serde(default)]
    pub text: Option<String>,
    /// Pages to OCR / extract / score (empty = all).
    #[serde(default)]
    pub pages: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedDoc {
    pub manifest: ManifestDoc,
    pub abs: PathBuf,
    pub size: u64,
    pub expected_fields: BTreeMap<String, Option<String>>,
}

fn expected_value(key: &str, v: &Value) -> Result<Option<String>, String> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone()).filter(|s| !s.trim().is_empty())),
        Value::Number(n) => Ok(Some(n.to_string())),
        Value::Bool(b) => Ok(Some(b.to_string())),
        other => Err(format!("필드 {key}: 문자열·숫자·null 만 허용 ({other})")),
    }
}

/// Loads, validates and resolves a manifest (paths relative to the manifest file).
pub fn load_manifest(path: &Path) -> Result<(Manifest, Vec<ResolvedDoc>), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|e| format!("{}: JSON 오류: {e}", path.display()))?;
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(format!("schema_version {} (지원: {MANIFEST_SCHEMA_VERSION})", manifest.schema_version));
    }
    if manifest.documents.is_empty() {
        return Err("documents 가 비어 있음".into());
    }
    let base = path.parent().unwrap_or(Path::new("."));
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for (i, d) in manifest.documents.iter().enumerate() {
        if d.path.trim().is_empty() {
            return Err(format!("documents[{i}]: path 가 비어 있음"));
        }
        if d.doc_type.trim().is_empty() {
            return Err(format!("documents[{i}] {}: doc_type 이 비어 있음", d.path));
        }
        if d.pages.contains(&0) {
            return Err(format!("documents[{i}] {}: pages 는 1부터 시작", d.path));
        }
        let abs = {
            let p = Path::new(&d.path);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                base.join(p)
            }
        };
        if !seen.insert(abs.clone()) {
            return Err(format!("documents[{i}] {}: 같은 파일이 두 번 나옴", d.path));
        }
        let meta = std::fs::metadata(&abs).map_err(|e| format!("documents[{i}] {}: {e}", abs.display()))?;
        if !meta.is_file() {
            return Err(format!("documents[{i}] {}: 파일이 아님", abs.display()));
        }
        let mut expected_fields = BTreeMap::new();
        for (k, v) in &d.fields {
            expected_fields.insert(k.clone(), expected_value(k, v)?);
        }
        out.push(ResolvedDoc { manifest: d.clone(), abs, size: meta.len(), expected_fields });
    }
    Ok((manifest, out))
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

fn format_amount(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v}")
    }
}

/// Value normalisation by field kind (the validator's normalisers for amounts,
/// dates, BRNs and phone numbers; NFC + whitespace collapse otherwise).
pub fn normalize_by_kind(kind: Option<FieldKind>, raw: &str) -> String {
    let text = metrics::normalize_text(raw);
    match kind {
        Some(FieldKind::Amount | FieldKind::Integer) => {
            validate::normalize_amount(raw).map(format_amount).unwrap_or(text)
        }
        Some(FieldKind::Date) => validate::normalize_date(raw).unwrap_or(text),
        Some(FieldKind::Brn) => validate::brn_digits(raw).unwrap_or(text),
        Some(FieldKind::Phone) => validate::normalize_phone(raw).unwrap_or(text),
        _ => text,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDetail {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub expected: Option<String>,
    pub produced: Option<String>,
    pub expected_norm: Option<String>,
    pub produced_norm: Option<String>,
    pub exact: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FieldScore {
    pub expected: u32,
    pub matched: u32,
    pub missing: Vec<String>,
    pub hallucinated: u32,
    pub exact_match_rate: Option<f64>,
    pub anls_star: Option<f64>,
    pub details: Vec<FieldDetail>,
}

/// Scores one document's fields: exact match after kind-aware normalisation and ANLS*.
pub fn score_fields(expected: &BTreeMap<String, Option<String>>, produced: &[FieldRecord], strict: bool) -> FieldScore {
    let produced_map: BTreeMap<&str, &FieldRecord> = produced
        .iter()
        .filter(|f| f.kind.is_scalar())
        .filter(|f| f.effective_value().is_some_and(|v| !v.trim().is_empty()))
        .map(|f| (f.path.as_str(), f))
        .collect();
    let mut score = FieldScore { expected: expected.len() as u32, ..Default::default() };
    let mut exp_norm: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut prod_norm: BTreeMap<String, String> = BTreeMap::new();
    for (key, exp) in expected {
        let rec = produced_map.get(key.as_str()).copied();
        let kind = rec.map(|r| r.kind);
        let produced_value = rec.and_then(|r| r.effective_value()).map(String::from);
        let e_norm = exp.as_deref().map(|e| normalize_by_kind(kind, e)).filter(|s| !s.is_empty());
        let p_norm = produced_value.as_deref().map(|p| normalize_by_kind(kind, p)).filter(|s| !s.is_empty());
        let exact = e_norm == p_norm;
        let similarity = match (&e_norm, &p_norm) {
            (Some(e), Some(p)) => Some(metrics::similarity(e, p)),
            _ => None,
        };
        if exact {
            score.matched += 1;
        } else if p_norm.is_none() && e_norm.is_some() {
            score.missing.push(key.clone());
        }
        exp_norm.insert(key.clone(), e_norm.clone());
        if let Some(p) = &p_norm {
            prod_norm.insert(key.clone(), p.clone());
        }
        score.details.push(FieldDetail {
            key: key.clone(),
            kind: kind.map(|k| k.as_str().to_string()),
            expected: exp.clone(),
            produced: produced_value,
            expected_norm: e_norm,
            produced_norm: p_norm,
            exact,
            similarity,
        });
    }
    for (path, rec) in &produced_map {
        if !expected.contains_key(*path) {
            score.hallucinated += 1;
            if strict {
                prod_norm.insert(
                    (*path).to_string(),
                    normalize_by_kind(Some(rec.kind), rec.effective_value().unwrap_or("")),
                );
            }
        }
    }
    if score.expected > 0 {
        score.exact_match_rate = Some(f64::from(score.matched) / f64::from(score.expected));
    }
    score.anls_star = metrics::anls_star(&exp_norm, &prod_norm, strict);
    score
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocResult {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub expected_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicted_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_correct: Option<bool>,
    pub pages: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub page_filter: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_page_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cer: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_sources: Vec<String>,
    #[serde(default)]
    pub fields: FieldScore,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrMetrics {
    pub documents: u32,
    pub mean_cer: f64,
    pub max_cer: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FieldMetrics {
    pub documents: u32,
    pub expected: u32,
    pub matched: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_match_rate: Option<f64>,
    pub hallucinated: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anls_star_mean: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyMetrics {
    pub documents: u32,
    pub pages: u32,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub mean_ms: f64,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VramMetrics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_after_load_mib: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_used_mib: Option<u64>,
    pub samples: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Classification>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr: Option<OcrMetrics>,
    pub fields: FieldMetrics,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<LatencyMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram: Option<VramMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineResults {
    pub schema_version: u32,
    pub kind: String,
    /// `done` | `dry_run`
    pub status: String,
    pub generated_at: String,
    pub manifest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_name: Option<String>,
    pub strict_keys: bool,
    pub document_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantization: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub precise: bool,
    pub pii_masking: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pdfium: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    pub documents: Vec<DocResult>,
}

pub fn aggregate_metrics(docs: &[DocResult], vram: Option<VramMetrics>) -> Metrics {
    let pairs: Vec<(String, String)> =
        docs.iter().filter_map(|d| d.predicted_type.as_ref().map(|p| (d.expected_type.clone(), p.clone()))).collect();
    let cers: Vec<f64> = docs.iter().filter_map(|d| d.cer).collect();
    let ocr = metrics::mean(&cers).map(|m| OcrMetrics {
        documents: cers.len() as u32,
        mean_cer: m,
        max_cer: cers.iter().cloned().fold(0.0, f64::max),
    });
    let scored: Vec<&DocResult> = docs.iter().filter(|d| d.error.is_none() && d.fields.expected > 0).collect();
    let expected: u32 = scored.iter().map(|d| d.fields.expected).sum();
    let matched: u32 = scored.iter().map(|d| d.fields.matched).sum();
    let anls: Vec<f64> = scored.iter().filter_map(|d| d.fields.anls_star).collect();
    let fields = FieldMetrics {
        documents: scored.len() as u32,
        expected,
        matched,
        exact_match_rate: (expected > 0).then(|| f64::from(matched) / f64::from(expected)),
        hallucinated: scored.iter().map(|d| d.fields.hallucinated).sum(),
        anls_star_mean: metrics::mean(&anls),
    };
    let mut page_samples = Vec::new();
    let mut total_ms = 0u64;
    let mut pages = 0u32;
    let mut timed = 0u32;
    for d in docs.iter().filter(|d| d.error.is_none()) {
        if let (Some(ms), Some(per_page)) = (d.duration_ms, d.per_page_ms) {
            timed += 1;
            total_ms += ms;
            pages += d.pages.max(1);
            page_samples.extend(std::iter::repeat_n(per_page, d.pages.max(1) as usize));
        }
    }
    let latency = (!page_samples.is_empty()).then(|| LatencyMetrics {
        documents: timed,
        pages,
        p50_ms: metrics::percentile(&page_samples, 50.0).unwrap_or(0.0),
        p95_ms: metrics::percentile(&page_samples, 95.0).unwrap_or(0.0),
        mean_ms: metrics::mean(&page_samples).unwrap_or(0.0),
        total_ms,
    });
    Metrics { classification: metrics::classification(&pairs), ocr, fields, latency, vram }
}

fn pct(v: Option<f64>) -> String {
    v.map(|x| format!("{:.1}%", x * 100.0)).unwrap_or_else(|| "–".into())
}

fn num(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.3}")).unwrap_or_else(|| "–".into())
}

/// Korean Markdown scorecard for one run.
pub fn scorecard_md(r: &PipelineResults) -> String {
    let mut md = String::new();
    md.push_str("# 파이프라인 스코어카드\n\n");
    md.push_str(&format!(
        "- 생성: {} · 상태: {}\n- 매니페스트: `{}`{} ({}건, strict_keys={})\n",
        r.generated_at,
        if r.status == "dry_run" { "미실행(--dry-run)" } else { "완료" },
        r.manifest,
        r.manifest_name.as_deref().map(|n| format!(" — {n}")).unwrap_or_default(),
        r.document_count,
        r.strict_keys
    ));
    match &r.engine {
        Some(e) => {
            md.push_str(&format!(
                "- 모델: `{}` (양자화 {}, 컨텍스트 {}) · 엔진: {} {} · 티어 {} · 프로파일 {}{}\n",
                e.model,
                e.quantization.as_deref().unwrap_or("?"),
                e.ctx.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                e.backend,
                e.build_number.map(|b| format!("b{b}")).unwrap_or_else(|| "(빌드 불명)".into()),
                e.tier,
                e.profile,
                if r.precise { " · 정밀 모드" } else { "" }
            ));
            if e.devices.is_empty() {
                md.push_str("- 장치: 엔진이 보고한 GPU 없음(CPU)\n");
            } else {
                for d in &e.devices {
                    md.push_str(&format!(
                        "- 장치: {} — {} ({} MiB, 로드 전 여유 {} MiB → 로드 후 {} MiB)\n",
                        d.name, d.description, d.total_mib, d.free_before_mib, d.free_after_mib
                    ));
                }
            }
        }
        None => md.push_str("- 모델/엔진: 미실행\n"),
    }
    if let Some(ws) = &r.workspace {
        md.push_str(&format!(
            "- 평가 워크스페이스: `{ws}` (추출 미러 `.doculens/extractions/<doc>/<run>/result.json`)\n"
        ));
    }
    md.push_str("\n## 지표\n\n");
    match &r.metrics {
        None => md.push_str(
            "**미실행** — `--dry-run`은 매니페스트 검증과 파일 확인만 수행합니다. 엔진을 지정해 다시 실행하세요.\n",
        ),
        Some(m) => {
            md.push_str("| 지표 | 값 | 비고 |\n|---|---:|---|\n");
            match &m.classification {
                Some(c) => {
                    md.push_str(&format!(
                        "| 문서 유형 분류 정확도 | {} | {}/{} |\n",
                        pct(Some(c.accuracy)),
                        c.correct,
                        c.total
                    ));
                    md.push_str(&format!(
                        "| 문서 유형 분류 macro F1 | {} | 라벨 {}종 |\n",
                        num(Some(c.macro_f1)),
                        c.labels.len()
                    ));
                }
                None => md.push_str("| 문서 유형 분류 | – | 분류 결과 없음 |\n"),
            }
            match &m.ocr {
                Some(o) => md.push_str(&format!(
                    "| OCR CER(평균) | {} | {}건, 최대 {} |\n",
                    pct(Some(o.mean_cer)),
                    o.documents,
                    pct(Some(o.max_cer))
                )),
                None => md.push_str("| OCR CER | – | 매니페스트에 기대 텍스트 없음 |\n"),
            }
            md.push_str(&format!(
                "| 필드 정확일치율(정규화 후) | {} | {}/{} 필드, {}건, 환각 {} |\n",
                pct(m.fields.exact_match_rate),
                m.fields.matched,
                m.fields.expected,
                m.fields.documents,
                m.fields.hallucinated
            ));
            md.push_str(&format!(
                "| ANLS* (문서 평균) | {} | 임계 0.5, 누락·환각 키 패널티 |\n",
                num(m.fields.anls_star_mean)
            ));
            match &m.latency {
                Some(l) => md.push_str(&format!(
                    "| 페이지 처리 시간 p50 / p95 | {:.0} ms / {:.0} ms | {}건 {}쪽, 평균 {:.0} ms, 총 {:.1} s |\n",
                    l.p50_ms,
                    l.p95_ms,
                    l.documents,
                    l.pages,
                    l.mean_ms,
                    l.total_ms as f64 / 1000.0
                )),
                None => md.push_str("| 페이지 처리 시간 | – | |\n"),
            }
            match &m.vram {
                Some(v) => md.push_str(&format!(
                    "| VRAM 사용(로드 후) / 피크 | {} / {} | 엔진 보고(전체 − 여유), 표본 {} |\n",
                    v.used_after_load_mib.map(|x| format!("{x} MiB")).unwrap_or_else(|| "–".into()),
                    v.peak_used_mib.map(|x| format!("{x} MiB")).unwrap_or_else(|| "–".into()),
                    v.samples
                )),
                None => md.push_str("| VRAM | – | 엔진이 장치를 보고하지 않음(CPU) |\n"),
            }
            if let Some(c) = &m.classification {
                md.push_str("\n### 유형별 분류\n\n| 라벨 | TP | FP | FN | 정밀도 | 재현율 | F1 |\n|---|---:|---:|---:|---:|---:|---:|\n");
                for l in &c.labels {
                    md.push_str(&format!(
                        "| {} | {} | {} | {} | {} | {} | {} |\n",
                        l.label,
                        l.tp,
                        l.fp,
                        l.fn_,
                        num(Some(l.precision)),
                        num(Some(l.recall)),
                        num(Some(l.f1))
                    ));
                }
            }
        }
    }
    md.push_str("\n## 문서별\n\n| 문서 | 기대 유형 | 판정 유형 | CER | 필드 일치 | ANLS* | 쪽 | 처리 시간 | 비고 |\n|---|---|---|---:|---:|---:|---:|---:|---|\n");
    for d in &r.documents {
        md.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            d.path,
            d.expected_type,
            d.predicted_type
                .as_deref()
                .map(|p| if d.type_correct == Some(true) { p.to_string() } else { format!("**{p}**") })
                .unwrap_or_else(|| "–".into()),
            pct(d.cer),
            if d.fields.expected > 0 { format!("{}/{}", d.fields.matched, d.fields.expected) } else { "–".into() },
            num(d.fields.anls_star),
            if d.pages == 0 && d.page_filter.is_empty() { "전체".to_string() } else { d.pages.to_string() },
            d.duration_ms.map(|ms| format!("{:.1} s", ms as f64 / 1000.0)).unwrap_or_else(|| "–".into()),
            d.error.as_deref().map(|e| format!("오류: {e}")).unwrap_or_else(|| {
                if d.fields.missing.is_empty() {
                    String::new()
                } else {
                    format!("누락 {}", d.fields.missing.join(", "))
                }
            })
        ));
    }
    let mismatched: Vec<&FieldDetail> =
        r.documents.iter().flat_map(|d| d.fields.details.iter().filter(|f| !f.exact)).collect();
    if !mismatched.is_empty() {
        md.push_str(
            "\n## 불일치 필드\n\n| 필드 | 종류 | 기대(정규화) | 결과(정규화) | 유사도 |\n|---|---|---|---|---:|\n",
        );
        for f in mismatched.iter().take(200) {
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                f.key,
                f.kind.as_deref().unwrap_or("–"),
                f.expected_norm.as_deref().unwrap_or("(비어 있음)"),
                f.produced_norm.as_deref().unwrap_or("(없음)"),
                num(f.similarity)
            ));
        }
    }
    md
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

fn write_outputs(out: &Path, results: &PipelineResults) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let json = serde_json::to_string_pretty(results).map_err(|e| e.to_string())?;
    std::fs::write(out.join("results.json"), json + "\n").map_err(|e| format!("results.json: {e}"))?;
    std::fs::write(out.join("scorecard.md"), scorecard_md(results)).map_err(|e| format!("scorecard.md: {e}"))?;
    println!("저장: {} / {}", out.join("results.json").display(), out.join("scorecard.md").display());
    Ok(())
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn run(args: &Args) -> Result<i32, String> {
    let unknown = args.unknown_flags(KNOWN_FLAGS);
    if !unknown.is_empty() {
        return Err(format!("알 수 없는 옵션 {}\n{USAGE}", unknown.join(" ")));
    }
    let manifest_path =
        PathBuf::from(args.opt("manifest").ok_or_else(|| format!("--manifest 가 필요합니다\n{USAGE}"))?);
    let out = PathBuf::from(args.opt("out").ok_or_else(|| format!("--out <dir> 이 필요합니다\n{USAGE}"))?);
    let (manifest, mut docs_in) = load_manifest(&manifest_path)?;
    if let Some(limit) = args.opt("limit") {
        let n: usize = limit.parse().map_err(|_| format!("--limit 값이 숫자가 아닙니다: {limit}"))?;
        docs_in.truncate(n);
    }
    let verbose = args.flag("verbose");
    println!(
        "매니페스트 {} — {}건{} · 기대 필드 {}개 · OCR 텍스트 {}건",
        manifest_path.display(),
        docs_in.len(),
        manifest.name.as_deref().map(|n| format!(" ({n})")).unwrap_or_default(),
        docs_in.iter().map(|d| d.expected_fields.len()).sum::<usize>(),
        docs_in.iter().filter(|d| d.manifest.text.is_some()).count()
    );

    let mut results = PipelineResults {
        schema_version: RESULTS_SCHEMA_VERSION,
        kind: "pipeline".into(),
        status: "dry_run".into(),
        generated_at: now(),
        manifest: manifest_path.display().to_string(),
        manifest_name: manifest.name.clone(),
        strict_keys: manifest.strict_keys,
        document_count: docs_in.len() as u32,
        model: None,
        quantization: None,
        engine: None,
        profile: None,
        precise: args.flag("precise"),
        pii_masking: false,
        workspace: None,
        pdfium: None,
        metrics: None,
        documents: docs_in
            .iter()
            .map(|d| DocResult {
                path: d.manifest.path.clone(),
                expected_type: d.manifest.doc_type.clone(),
                pages: d.manifest.pages.len() as u32,
                page_filter: d.manifest.pages.clone(),
                fields: FieldScore { expected: d.expected_fields.len() as u32, ..Default::default() },
                ..Default::default()
            })
            .collect(),
    };

    if args.flag("dry-run") {
        for d in &docs_in {
            println!(
                "  확인: {} ({} bytes) 유형 {} 필드 {}",
                d.abs.display(),
                d.size,
                d.manifest.doc_type,
                d.expected_fields.len()
            );
        }
        write_outputs(&out, &results)?;
        println!("미실행(dry-run): 매니페스트와 파일을 확인했습니다. 엔진을 지정하면 지표를 계산합니다.");
        return Ok(0);
    }

    let bin = args.opt("engine").ok_or_else(|| format!("--engine 이 필요합니다(또는 --dry-run)\n{USAGE}"))?;
    let models_dir = args.opt("models-dir").ok_or("--engine 에는 --models-dir 가 필요합니다")?;
    let model = args.opt("model").ok_or("--engine 에는 --model <id> 가 필요합니다")?;
    let backend = match args.opt("backend") {
        Some(b) => Some(engine::parse_backend(b).ok_or_else(|| format!("알 수 없는 백엔드 {b}"))?),
        None => None,
    };
    let profile = match args.opt("profile") {
        Some(p) => engine::parse_profile(p).ok_or_else(|| format!("알 수 없는 프로파일 {p}"))?,
        None => Profile::Accuracy,
    };

    // 1. workspace
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let ws_root = args.opt("workspace").map(PathBuf::from).unwrap_or_else(|| out.join("workspace"));
    let ws = Workspace::open(
        &ws_root,
        OpenOptions { create: true, name: Some("doculens-eval".into()), locale: None, force_unlock: true },
    )
    .map_err(|e| format!("워크스페이스 {}: {e}", ws_root.display()))?;
    ws.db.with(pipe_repo::ensure_builtins).map_err(|e| e.to_string())?;
    results.workspace = Some(ws_root.display().to_string());

    // 2. PDFium (PDF rendering); images render without it.
    let candidates = match args.opt("pdfium") {
        Some(p) => vec![PathBuf::from(p)],
        None => render::default_pdfium_candidates(),
    };
    let render_svc: Option<&'static RenderService> = match render::locate_pdfium(&candidates) {
        Some(lib) => match RenderService::global(&lib) {
            Ok(s) => {
                results.pdfium = Some(lib.display().to_string());
                Some(s)
            }
            Err(e) => {
                eprintln!("경고: PDFium 로드 실패({}): {e} — PDF 문서는 렌더되지 않습니다", lib.display());
                None
            }
        },
        None => {
            eprintln!("경고: PDFium 을 찾지 못했습니다(--pdfium 또는 DOCULENS_PDFIUM) — PDF 문서는 렌더되지 않습니다");
            None
        }
    };
    let printer = StubPdfPrinter;
    let render_options = RenderOptions { thumbnails: false, ..Default::default() };
    let import_deps = ImportDeps {
        db: &ws.db,
        paths: &ws.paths,
        render: render_svc,
        printer: &printer,
        render_options: render_options.clone(),
    };

    // 3. import (copy + render into the evaluation workspace)
    let sources: Vec<PathBuf> = docs_in.iter().map(|d| d.abs.clone()).collect();
    let options = ImportOptions { render: true, auto_level: Some(AutoProcessingLevel::None), ..Default::default() };
    let plan = import::plan(&ws.db, &ws.paths, &sources, &options, ImportSource::Drop).map_err(|e| e.to_string())?;
    let t_import = Instant::now();
    let report = import::run(
        &import_deps,
        &plan,
        None,
        &mut |p| {
            if verbose {
                eprintln!("  가져오기 {}/{} {}", p.done, p.total, p.current);
            }
        },
        &mut |_, _| {},
    )
    .map_err(|e| format!("가져오기 실패: {e}"))?;
    println!(
        "가져오기: 신규 {} · 재연결 {} · 실패 {} · 건너뜀 {} ({:.1} s)",
        report.imported.len(),
        report.relinked.len(),
        report.failed.len(),
        report.skipped,
        t_import.elapsed().as_secs_f64()
    );
    // Manifest document → workspace id by content hash (works for re-runs on the same workspace).
    let mut ids: Vec<Option<String>> = Vec::with_capacity(docs_in.len());
    for (i, d) in docs_in.iter().enumerate() {
        let hash = import::blake3_file(&d.abs).map_err(|e| e.to_string())?;
        let found = ws.db.with(|c| docs::find_by_hash(c, &hash)).map_err(|e| e.to_string())?;
        match found.first() {
            Some(doc) => {
                results.documents[i].id = Some(doc.id.clone());
                ids.push(Some(doc.id.clone()));
            }
            None => {
                let reason = plan
                    .items
                    .iter()
                    .find(|it| it.path == d.abs)
                    .map(|it| serde_json::to_string(&it.status).unwrap_or_default())
                    .unwrap_or_else(|| "not in the import plan".into());
                let err = report
                    .failed
                    .iter()
                    .find(|(name, _)| d.abs.ends_with(name) || name.contains(&d.manifest.path))
                    .map(|(_, e)| e.clone())
                    .unwrap_or(reason);
                results.documents[i].error = Some(format!("import: {err}"));
                ids.push(None);
            }
        }
    }

    // 4. engine
    let opts = EngineOpts {
        binary: PathBuf::from(bin),
        models_dir: PathBuf::from(models_dir),
        model: model.to_string(),
        backend,
        profile,
        work_dir: out.join("engine"),
        load_timeout: Duration::from_secs(900),
    };
    eprintln!("엔진 시작: {} · 모델 {}", opts.binary.display(), opts.model);
    let running = RunningEngine::start(&opts)?;
    eprintln!(
        "엔진 준비: 빌드 {:?} 백엔드 {} 컨텍스트 {:?} 로드 {} ms VRAM {:?} MiB",
        running.info.build_number,
        running.info.backend,
        running.info.ctx,
        running.info.load_ms,
        running.info.vram_used_mib
    );
    if !running.info.has_vision {
        eprintln!("경고: 모델에 비전 프로젝터(mmproj)가 없습니다 — 이미지 페이지 분류·OCR이 동작하지 않습니다");
    }
    results.model = Some(running.info.model.clone());
    results.quantization = running.info.quantization.clone();
    results.profile = Some(engine::profile_name(profile).to_string());
    let mut vram = VramMetrics::default();
    let after_load = running.sample_devices();
    if !after_load.is_empty() {
        vram.used_after_load_mib = Some(engine::used_mib(&after_load));
        vram.peak_used_mib = vram.used_after_load_mib;
        vram.samples = 1;
    }

    let settings = ProcessingSettings { precise_mode: args.flag("precise"), ..Default::default() };
    let deps = PipelineDeps {
        import: ImportDeps { db: &ws.db, paths: &ws.paths, render: render_svc, printer: &printer, render_options },
        chat: &running.manager,
        model: running.info.model.clone(),
        settings,
        profile,
        pii_masking: false,
        cancel: None,
        ctx: running.info.ctx,
    };

    // 5. process + score each document
    for (i, d) in docs_in.iter().enumerate() {
        let Some(id) = ids[i].clone() else { continue };
        let page_count = ws.db.with(|c| docs::list_pages(c, &id, false)).map_err(|e| e.to_string())?.len() as u32;
        let pages_scored = if d.manifest.pages.is_empty() { page_count } else { d.manifest.pages.len() as u32 };
        results.documents[i].pages = pages_scored;
        let req = ProcessRequest {
            document_ids: vec![id.clone()],
            level: PipelineLevel::Extract,
            schema: SchemaChoice::Auto,
            force: true,
            profile: None,
            pages: d.manifest.pages.clone(),
            precise: None,
            // The golden run measures the model: hand-corrected page text is read again too.
            reocr_manual: true,
            ..Default::default()
        };
        eprintln!("[{}/{}] {} ({}쪽)", i + 1, docs_in.len(), d.manifest.path, page_count);
        let t = Instant::now();
        let outcome = process(&deps, &req, &mut |p| {
            if verbose {
                eprintln!("    {} {}", p.stage, p.page.map(|(a, b)| format!("{a}/{b}")).unwrap_or_default());
            }
        });
        let duration_ms = t.elapsed().as_millis() as u64;
        let dr = &mut results.documents[i];
        match outcome {
            Ok(rep) => {
                if let Some((_, e)) = rep.failed.first() {
                    dr.error = Some(e.clone());
                } else if rep.cancelled {
                    dr.error = Some("cancelled".into());
                }
            }
            Err(e) => dr.error = Some(e.to_string()),
        }
        dr.duration_ms = Some(duration_ms);
        dr.per_page_ms = Some(duration_ms as f64 / f64::from(pages_scored.max(1)));

        // classification
        let doc = ws.db.with(|c| docs::get_document(c, &id)).map_err(|e| e.to_string())?;
        if let Some(doc) = &doc {
            dr.predicted_type = doc.doc_type.clone();
            dr.type_correct = doc.doc_type.as_ref().map(|p| *p == d.manifest.doc_type);
        }
        // OCR text
        let pages = ws.db.with(|c| docs::list_pages(c, &id, true)).map_err(|e| e.to_string())?;
        let scored: Vec<_> =
            pages.iter().filter(|p| d.manifest.pages.is_empty() || d.manifest.pages.contains(&p.page_no)).collect();
        let mut sources: BTreeSet<String> = BTreeSet::new();
        let text: Vec<&str> = scored
            .iter()
            .filter_map(|p| {
                if let Some(s) = &p.text_source {
                    sources.insert(s.clone());
                }
                p.text.as_deref()
            })
            .collect();
        dr.text_sources = sources.into_iter().collect();
        if let Some(expected_text) = &d.manifest.text {
            dr.cer = Some(metrics::cer(expected_text, &text.join("\n")));
        }
        // fields
        let fields = ws.db.with(|c| pipe_repo::latest_fields(c, &id)).map_err(|e| e.to_string())?;
        dr.fields = score_fields(&d.expected_fields, &fields, manifest.strict_keys);
        dr.mirror = Some(ws.paths.extractions_dir().join(&id).display().to_string());
        println!(
            "  유형 {} → {} · CER {} · 필드 {}/{} · ANLS* {} · {:.1} s{}",
            d.manifest.doc_type,
            dr.predicted_type.as_deref().unwrap_or("–"),
            pct(dr.cer),
            dr.fields.matched,
            dr.fields.expected,
            num(dr.fields.anls_star),
            duration_ms as f64 / 1000.0,
            dr.error.as_deref().map(|e| format!(" · 오류: {e}")).unwrap_or_default()
        );
        let sample = running.sample_devices();
        if !sample.is_empty() {
            let used = engine::used_mib(&sample);
            vram.peak_used_mib = Some(vram.peak_used_mib.map_or(used, |p| p.max(used)));
            vram.samples += 1;
        }
    }

    results.engine = Some(running.info.clone());
    results.metrics = Some(aggregate_metrics(&results.documents, (vram.samples > 0).then_some(vram)));
    results.status = "done".into();
    results.generated_at = now();
    if let Some(m) = &results.metrics {
        println!(
            "== 요약 == 분류 정확도 {} · macro F1 {} · 필드 정확일치 {} · ANLS* {} · CER {} · 페이지 p50/p95 {} / {}",
            pct(m.classification.as_ref().map(|c| c.accuracy)),
            num(m.classification.as_ref().map(|c| c.macro_f1)),
            pct(m.fields.exact_match_rate),
            num(m.fields.anls_star_mean),
            pct(m.ocr.as_ref().map(|o| o.mean_cer)),
            m.latency.as_ref().map(|l| format!("{:.0} ms", l.p50_ms)).unwrap_or_else(|| "–".into()),
            m.latency.as_ref().map(|l| format!("{:.0} ms", l.p95_ms)).unwrap_or_else(|| "–".into())
        );
    }
    write_outputs(&out, &results)?;
    drop(deps);
    drop(running);
    ws.close();
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use doculens_core::pipeline::FieldStatus;

    fn field(path: &str, kind: FieldKind, value: &str) -> FieldRecord {
        FieldRecord {
            id: 0,
            run_id: "r".into(),
            document_id: "d".into(),
            path: path.into(),
            key: path.rsplit('.').next().unwrap_or(path).into(),
            row: None,
            label_ko: path.into(),
            label_en: path.into(),
            kind,
            required: false,
            value: Some(value.into()),
            normalized: None,
            original_value: None,
            page: Some(1),
            bbox: None,
            evidence: None,
            confidence: 0.9,
            model_confidence: None,
            status: FieldStatus::Auto,
            review_reason: None,
            validation: Vec::new(),
            reviewed_at: None,
        }
    }

    #[test]
    fn kind_aware_normalisation() {
        assert_eq!(normalize_by_kind(Some(FieldKind::Amount), "1,000,000원"), "1000000");
        assert_eq!(normalize_by_kind(Some(FieldKind::Amount), "₩12,500"), "12500");
        assert_eq!(normalize_by_kind(Some(FieldKind::Date), "2026년 10월 14일"), "2026-10-14");
        assert_eq!(normalize_by_kind(Some(FieldKind::Date), "2026.10.14"), "2026-10-14");
        assert_eq!(normalize_by_kind(Some(FieldKind::Brn), "123-45-67890"), "1234567890");
        assert_eq!(normalize_by_kind(Some(FieldKind::String), "  ㈜한빛  시스템 "), "㈜한빛 시스템");
        assert_eq!(normalize_by_kind(None, "x"), "x");
    }

    #[test]
    fn field_scoring_exact_match_and_anls() {
        let expected: BTreeMap<String, Option<String>> = [
            ("supply_total", Some("1000000")),
            ("issue_date", Some("2026-10-14")),
            ("supplier.brn", Some("1234567890")),
            ("remarks", None),
            ("missing_key", Some("abc")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.map(String::from)))
        .collect();
        let produced = vec![
            field("supply_total", FieldKind::Amount, "1,000,000원"),
            field("issue_date", FieldKind::Date, "2026.10.14"),
            field("supplier.brn", FieldKind::Brn, "123-45-67899"),
            field("extra", FieldKind::String, "hallucinated"),
        ];
        let s = score_fields(&expected, &produced, false);
        assert_eq!(s.expected, 5);
        // amount + date exact; brn differs in one digit; remarks empty both sides counts as exact.
        assert_eq!(s.matched, 3, "{s:?}");
        assert_eq!(s.missing, vec!["missing_key".to_string()]);
        assert_eq!(s.hallucinated, 1);
        assert!((s.exact_match_rate.unwrap() - 0.6).abs() < 1e-9);
        // ANLS*: 1 + 1 + 0.9 + 1 + 0 over 5 expected keys (lenient: hallucination ignored).
        assert!((s.anls_star.unwrap() - 3.9 / 5.0).abs() < 1e-9, "{:?}", s.anls_star);
        let strict = score_fields(&expected, &produced, true);
        assert!((strict.anls_star.unwrap() - 3.9 / 6.0).abs() < 1e-9, "{:?}", strict.anls_star);
    }

    #[test]
    fn aggregates_and_renders_a_scorecard() {
        let mut docs = vec![
            DocResult {
                path: "a.jpg".into(),
                expected_type: "receipt".into(),
                predicted_type: Some("receipt".into()),
                type_correct: Some(true),
                pages: 2,
                duration_ms: Some(4000),
                per_page_ms: Some(2000.0),
                cer: Some(0.05),
                fields: FieldScore {
                    expected: 4,
                    matched: 3,
                    anls_star: Some(0.8),
                    exact_match_rate: Some(0.75),
                    ..Default::default()
                },
                ..Default::default()
            },
            DocResult {
                path: "b.pdf".into(),
                expected_type: "tax_invoice".into(),
                predicted_type: Some("receipt".into()),
                type_correct: Some(false),
                pages: 1,
                duration_ms: Some(6000),
                per_page_ms: Some(6000.0),
                fields: FieldScore {
                    expected: 2,
                    matched: 2,
                    anls_star: Some(1.0),
                    exact_match_rate: Some(1.0),
                    ..Default::default()
                },
                ..Default::default()
            },
        ];
        let m = aggregate_metrics(&docs, None);
        let c = m.classification.unwrap();
        assert_eq!(c.correct, 1);
        assert!((m.fields.exact_match_rate.unwrap() - 5.0 / 6.0).abs() < 1e-9);
        assert!((m.fields.anls_star_mean.unwrap() - 0.9).abs() < 1e-9);
        let l = m.latency.unwrap();
        assert_eq!(l.pages, 3);
        assert_eq!(l.p50_ms, 2000.0);
        assert_eq!(l.p95_ms, 6000.0);
        assert!((m.ocr.unwrap().mean_cer - 0.05).abs() < 1e-9);
        docs[1].error = Some("boom".into());
        let m2 = aggregate_metrics(&docs, None);
        assert_eq!(m2.fields.documents, 1);
        let r = PipelineResults {
            schema_version: 1,
            kind: "pipeline".into(),
            status: "done".into(),
            generated_at: "t".into(),
            manifest: "m.json".into(),
            manifest_name: None,
            strict_keys: false,
            document_count: 2,
            model: Some("gemma4".into()),
            quantization: None,
            engine: None,
            profile: None,
            precise: false,
            pii_masking: false,
            workspace: None,
            pdfium: None,
            metrics: Some(m2),
            documents: docs,
        };
        let md = scorecard_md(&r);
        assert!(md.contains("# 파이프라인 스코어카드"));
        assert!(md.contains("| 문서 유형 분류 정확도 |"));
        assert!(md.contains("오류: boom"));
        let dry = PipelineResults { status: "dry_run".into(), metrics: None, ..r };
        assert!(scorecard_md(&dry).contains("**미실행**"));
    }
}
