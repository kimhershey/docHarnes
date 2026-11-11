//! `planner` subcommand: every golden command through
//! `doculens_core::planner::llm::plan` — rules + pre-router (deterministic) and,
//! with `--engine`, the product path (fast path → model: routing call + plan call
//! with one re-query) — with accuracy per category / intent / kind of turn, the
//! misses that would have run a change unasked, a miss list grouped by cause and
//! the 0.5 pt regression gate against a baseline file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use doculens_core::planner::llm::{plan, PlannerDeps};
use doculens_core::planner::{PlanContext, PlanMode, PlanResult, PlanSource, SideEffect};
use doculens_core::settings::Profile;

use crate::cli::Args;
use crate::engine::{self, EngineInfo, EngineOpts, RunningEngine};
use crate::golden::{self, Expected, GoldenItem, Mismatch};
use crate::metrics;

pub const RESULTS_SCHEMA_VERSION: u32 = 1;
/// Design §3.11 regression gate: block when accuracy drops by more than 0.5 pt.
pub const MAX_DROP_PT: f64 = 0.5;

const KNOWN_FLAGS: &[&str] = &[
    "golden",
    "context",
    "baseline",
    "out",
    "engine",
    "models-dir",
    "model",
    "backend",
    "profile",
    "ids",
    "kinds",
    "verbose",
    "help",
];

pub const USAGE: &str = "doculens-eval planner --golden <json> [--context <json>] [--baseline <results.json>] [--out <results.json>]
                      [--engine <llama-server> --models-dir <dir> --model <id> [--backend cpu|vulkan|cu121|cu129] [--profile ...]]
                      [--ids ko-94,ko-110,...] [--kinds chat,question,command,setting] [--verbose]";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Bucket {
    pub total: u32,
    pub correct: u32,
    pub accuracy: f64,
}

impl Bucket {
    fn add(&mut self, correct: bool) {
        self.total += 1;
        self.correct += u32::from(correct);
        self.accuracy = if self.total == 0 { 0.0 } else { f64::from(self.correct) / f64::from(self.total) };
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemResult {
    pub id: String,
    pub category: String,
    pub text: String,
    pub correct: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    pub source: String,
    pub intent: String,
    pub ops: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub confidence: f32,
    pub latency_ms: f64,
    /// `chat` | `question` | `command` | `setting` (0.8.0).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default)]
    pub requires_confirmation: bool,
    /// What the plan says it leaves out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmet: Option<String>,
    /// `chat` / `general` when the routing call answered instead of planning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// Codes of the notes the planner attached (`ungrounded_change`, `grouping_dropped`, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// A wrong plan that changes something and would have run without asking.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unasked_change: bool,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub mismatches: Vec<Mismatch>,
    #[serde(default)]
    pub operations: Value,
    #[serde(default)]
    pub scope: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Latency {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub mean_ms: f64,
}

/// One pass over the golden set (`rules` = rules + pre-router only, `model` = the full path).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Pass {
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub total: u32,
    pub correct: u32,
    pub accuracy: f64,
    pub by_category: BTreeMap<String, Bucket>,
    pub by_intent: BTreeMap<String, Bucket>,
    /// Accuracy per kind of turn (chat / question / command / setting; audit PLN-21).
    #[serde(default)]
    pub by_kind: BTreeMap<String, Bucket>,
    pub by_source: BTreeMap<String, u32>,
    pub by_cause: BTreeMap<String, u32>,
    /// Wrong plans that change something and would have run without confirmation — the
    /// failure the user reported ("UI 배율 120%" → the profile was changed).
    #[serde(default)]
    pub unasked_changes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<Latency>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub latency_by_kind: BTreeMap<String, Latency>,
    pub items: Vec<ItemResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Results {
    pub schema_version: u32,
    pub kind: String,
    pub generated_at: String,
    pub golden: String,
    pub golden_count: u32,
    pub context: String,
    pub today: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineInfo>,
    #[serde(flatten)]
    pub primary: Pass,
    /// The rules-only pass when the primary pass used the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_only: Option<Pass>,
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

fn source_name(s: PlanSource) -> &'static str {
    match s {
        PlanSource::Prerouter => "prerouter",
        PlanSource::Rules => "rules",
        PlanSource::Model => "model",
        PlanSource::Fast => "fast",
        PlanSource::None => "none",
    }
}

/// Stable cause code of a miss (grouping for the report).
pub fn cause(expected: &Expected, r: &PlanResult, mismatches: &[Mismatch]) -> Option<&'static str> {
    if mismatches.is_empty() {
        return None;
    }
    let produced_clarify = r.plan.is_clarification();
    let expected_passive = expected.intent == "clarify" || expected.intent == "noop";
    if mismatches.iter().all(|m| m.kind() == "plan") {
        return Some("confirmation");
    }
    if r.source == PlanSource::None || r.issues.iter().any(|i| i.code == "no_model") {
        return Some("no_rule");
    }
    if produced_clarify && !expected_passive {
        return Some("rule_clarified");
    }
    if expected_passive && !produced_clarify {
        return Some("over_interpretation");
    }
    Some(match mismatches[0].kind() {
        "intent" | "ops" => "wrong_ops",
        "params" => "params",
        _ => "scope",
    })
}

pub fn cause_label(code: &str) -> &'static str {
    match code {
        "no_rule" => "규칙 없음(모델 필요)",
        "rule_clarified" => "규칙이 되묻기로 후퇴",
        "over_interpretation" => "과잉 해석(되묻기·무시가 정답)",
        "wrong_ops" => "의도·연산 오판",
        "params" => "파라미터 누락·오류",
        "scope" => "범위(scope) 오류",
        "confirmation" => "확인 여부·'반영하지 못한 부분' 표시가 다름",
        _ => "기타",
    }
}

fn evaluate(items: &[GoldenItem], ctx: &PlanContext, deps: Option<&PlannerDeps<'_>>, verbose: bool) -> Pass {
    let mut pass = Pass { mode: if deps.is_some() { "model" } else { "rules" }.to_string(), ..Default::default() };
    pass.model = deps.map(|d| d.model.clone());
    let mut latencies = Vec::with_capacity(items.len());
    let mut kind_latencies: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (n, item) in items.iter().enumerate() {
        let item_ctx = match golden::item_context(ctx, item) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                continue;
            }
        };
        let t = Instant::now();
        let r = match plan(
            deps,
            &item.text,
            &item_ctx,
            if deps.is_some() { PlanMode::ModelFirst } else { PlanMode::RulesFirst },
        ) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{}: 플래너 오류 {e}", item.id);
                continue;
            }
        };
        let latency_ms = t.elapsed().as_secs_f64() * 1000.0;
        latencies.push(latency_ms);
        let kind = item.kind().to_string();
        kind_latencies.entry(kind.clone()).or_default().push(latency_ms);
        let mismatches = golden::compare(&item.expected, &r.plan);
        let correct = mismatches.is_empty();
        let cause = cause(&item.expected, &r, &mismatches).map(String::from);
        let ops = golden::op_kinds(&r.plan);
        let unasked_change = !correct
            && !r.plan.is_clarification()
            && !r.plan.requires_confirmation
            && r.plan.operations.iter().any(|op| op.side_effect() != SideEffect::None);
        if verbose {
            eprintln!(
                "[{}/{}] {} {} {} → {}[{}] ({}) {:.0} ms",
                n + 1,
                items.len(),
                if correct { "OK  " } else { "MISS" },
                item.id,
                item.text,
                r.plan.intent.as_str(),
                ops.join(","),
                source_name(r.source),
                latency_ms
            );
        }
        pass.by_category.entry(item.category.clone()).or_default().add(correct);
        pass.by_intent.entry(item.expected.intent.clone()).or_default().add(correct);
        pass.by_kind.entry(kind.clone()).or_default().add(correct);
        pass.unasked_changes += u32::from(unasked_change);
        *pass.by_source.entry(source_name(r.source).to_string()).or_default() += 1;
        if let Some(c) = &cause {
            *pass.by_cause.entry(c.clone()).or_default() += 1;
        }
        pass.total += 1;
        pass.correct += u32::from(correct);
        pass.items.push(ItemResult {
            id: item.id.clone(),
            category: item.category.clone(),
            text: item.text.clone(),
            correct,
            cause,
            source: source_name(r.source).to_string(),
            intent: r.plan.intent.as_str().to_string(),
            ops,
            summary: r.plan.summary.clone().or(r.plan.clarification_question.clone()),
            confidence: r.plan.confidence,
            latency_ms,
            kind,
            requires_confirmation: r.plan.requires_confirmation,
            unmet: r.plan.unmet.clone(),
            route: r.route.clone(),
            notes: r.issues.iter().map(|i| i.code.clone()).collect(),
            unasked_change,
            attempts: r.attempts,
            prompt_tokens: r.prompt_tokens,
            completion_tokens: r.completion_tokens,
            mismatches,
            operations: serde_json::to_value(&r.plan.operations).unwrap_or(Value::Null),
            scope: serde_json::to_value(&r.plan.scope).unwrap_or(Value::Null),
            raw: if deps.is_some() { r.raw.clone() } else { None },
        });
    }
    pass.accuracy = if pass.total == 0 { 0.0 } else { f64::from(pass.correct) / f64::from(pass.total) };
    if deps.is_some() {
        let latency = |values: &[f64]| Latency {
            p50_ms: metrics::percentile(values, 50.0).unwrap_or(0.0),
            p95_ms: metrics::percentile(values, 95.0).unwrap_or(0.0),
            mean_ms: metrics::mean(values).unwrap_or(0.0),
        };
        pass.latency = Some(latency(&latencies));
        pass.latency_by_kind = kind_latencies.iter().map(|(k, v)| (k.clone(), latency(v))).collect();
    }
    pass
}

fn print_pass(pass: &Pass, title: &str) {
    println!("== {title} ==");
    println!("정확도: {}/{} = {}", pass.correct, pass.total, pct(pass.accuracy));
    let cats: Vec<String> = golden::CATEGORIES
        .iter()
        .filter_map(|c| {
            pass.by_category.get(*c).map(|b| format!("{c} {}/{} ({})", b.correct, b.total, pct(b.accuracy)))
        })
        .collect();
    println!("  카테고리별: {}", cats.join(" · "));
    let intents: Vec<String> = pass
        .by_intent
        .iter()
        .map(|(k, b)| format!("{k} {}/{}{}", b.correct, b.total, if b.correct < b.total { "*" } else { "" }))
        .collect();
    println!("  의도별(미달 *): {}", intents.join(", "));
    let kinds: Vec<String> = golden::KINDS
        .iter()
        .filter_map(|k| {
            pass.by_kind.get(*k).map(|b| format!("{} {}/{} ({})", kind_label(k), b.correct, b.total, pct(b.accuracy)))
        })
        .collect();
    println!("  종류별: {}", kinds.join(" · "));
    let sources: Vec<String> = pass.by_source.iter().map(|(k, n)| format!("{k} {n}")).collect();
    println!("  플랜 출처: {}", sources.join(" · "));
    println!("  확인 없이 실행됐을 잘못된 변경: {}건", pass.unasked_changes);
    if let Some(l) = &pass.latency {
        println!("  응답 시간: p50 {:.0} ms · p95 {:.0} ms · 평균 {:.0} ms", l.p50_ms, l.p95_ms, l.mean_ms);
        let per_kind: Vec<String> = golden::KINDS
            .iter()
            .filter_map(|k| pass.latency_by_kind.get(*k).map(|l| format!("{} p50 {:.0} ms", kind_label(k), l.p50_ms)))
            .collect();
        if !per_kind.is_empty() {
            println!("  응답 시간(종류별): {}", per_kind.join(" · "));
        }
    }
    let misses: Vec<&ItemResult> = pass.items.iter().filter(|i| !i.correct).collect();
    println!("불일치 {}건 (원인별):", misses.len());
    let mut causes: Vec<(&String, &u32)> = pass.by_cause.iter().collect();
    causes.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (code, n) in causes {
        println!("  [{} — {}] {n}건", code, cause_label(code));
        for m in misses.iter().filter(|m| m.cause.as_deref() == Some(code)) {
            println!(
                "    {} \"{}\"\n      기대 {}[{}] / 결과 {}[{}] (출처 {}{}{}) — {}",
                m.id,
                m.text,
                expected_of(m).0,
                expected_of(m).1,
                m.intent,
                m.ops.join(","),
                m.source,
                if m.confidence > 0.0 { format!(", 신뢰도 {:.2}", m.confidence) } else { String::new() },
                if m.unasked_change { ", 확인 없이 실행" } else { "" },
                m.summary.as_deref().unwrap_or("-")
            );
            for mm in &m.mismatches {
                match mm {
                    Mismatch::Params { index, expected, produced } => {
                        println!("      params[{index}] 기대 {expected} / 결과 {produced}");
                    }
                    Mismatch::Scope { expected, produced } => {
                        println!("      scope 기대 {expected} / 결과 {produced}");
                    }
                    Mismatch::Plan { field, expected, produced } => {
                        println!("      {field} 기대 {expected} / 결과 {produced}");
                    }
                    _ => {}
                }
            }
        }
    }
    println!();
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "chat" => "대화",
        "question" => "질문",
        "setting" => "설정",
        _ => "명령",
    }
}

/// `(intent, ops)` the item expected — recovered from the mismatch list when present,
/// else the produced values (a correct item).
fn expected_of(m: &ItemResult) -> (String, String) {
    let mut intent = m.intent.clone();
    let mut ops = m.ops.join(",");
    for mm in &m.mismatches {
        match mm {
            Mismatch::Intent { expected, .. } => intent = expected.clone(),
            Mismatch::Ops { expected, .. } => ops = expected.join(","),
            _ => {}
        }
    }
    (intent, ops)
}

fn load_context(args: &Args, golden_path: &Path, set: &golden::GoldenSet) -> Result<(PlanContext, String), String> {
    let dir = golden_path.parent().unwrap_or(Path::new("."));
    let path: Option<PathBuf> = match (args.opt("context"), &set.context) {
        (Some(p), _) => Some(PathBuf::from(p)),
        (None, Some(rel)) => Some(dir.join(rel)),
        (None, None) => {
            let sibling = dir.join("planner-context.json");
            sibling.is_file().then_some(sibling)
        }
    };
    match path {
        Some(p) => {
            let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let ctx: PlanContext =
                serde_json::from_str(&text).map_err(|e| format!("{}: 컨텍스트 JSON 오류: {e}", p.display()))?;
            Ok((ctx, p.display().to_string()))
        }
        None => {
            eprintln!("경고: 플랜 컨텍스트 파일이 없어 기본 컨텍스트(오늘 날짜, 빈 워크스페이스)를 사용합니다");
            Ok((PlanContext::default(), "(default)".into()))
        }
    }
}

/// Compares against a baseline results file; `Ok(true)` when the gate passes.
fn gate(current: &Pass, baseline_path: &Path) -> Result<bool, String> {
    let text = std::fs::read_to_string(baseline_path).map_err(|e| format!("{}: {e}", baseline_path.display()))?;
    let baseline: Results =
        serde_json::from_str(&text).map_err(|e| format!("{}: 기준선 JSON 오류: {e}", baseline_path.display()))?;
    let base = &baseline.primary;
    let delta_pt = (current.accuracy - base.accuracy) * 100.0;
    println!("== 회귀 게이트 (허용 하락 {MAX_DROP_PT} pt) ==");
    if base.mode != current.mode {
        println!("경고: 기준선 모드 {} ≠ 현재 모드 {} — 같은 모드끼리 비교하세요", base.mode, current.mode);
    }
    println!(
        "기준선 {} ({}, {}): {} → 현재 {} ({:+.1} pt)",
        baseline_path.display(),
        baseline.generated_at,
        base.mode,
        pct(base.accuracy),
        pct(current.accuracy),
        delta_pt
    );
    let base_ok: BTreeMap<&str, bool> = base.items.iter().map(|i| (i.id.as_str(), i.correct)).collect();
    let regressions: Vec<&ItemResult> =
        current.items.iter().filter(|i| !i.correct && base_ok.get(i.id.as_str()) == Some(&true)).collect();
    let fixed: Vec<&ItemResult> =
        current.items.iter().filter(|i| i.correct && base_ok.get(i.id.as_str()) == Some(&false)).collect();
    let new_items = current.items.iter().filter(|i| !base_ok.contains_key(i.id.as_str())).count();
    if !regressions.is_empty() {
        println!("  새로 실패 {}건:", regressions.len());
        for r in &regressions {
            println!("    {} \"{}\" → {}[{}] ({})", r.id, r.text, r.intent, r.ops.join(","), r.source);
        }
    }
    if !fixed.is_empty() {
        println!(
            "  새로 통과 {}건: {}",
            fixed.len(),
            fixed.iter().map(|f| f.id.as_str()).collect::<Vec<_>>().join(", ")
        );
    }
    if new_items > 0 {
        println!("  기준선에 없는 항목 {new_items}건");
    }
    let pass = delta_pt >= -MAX_DROP_PT;
    println!(
        "{}",
        if pass {
            "결과: 통과"
        } else {
            "결과: 실패 — 정확도가 허용치보다 많이 떨어졌습니다"
        }
    );
    Ok(pass)
}

pub fn run(args: &Args) -> Result<i32, String> {
    let unknown = args.unknown_flags(KNOWN_FLAGS);
    if !unknown.is_empty() {
        return Err(format!("알 수 없는 옵션 {}\n{USAGE}", unknown.join(" ")));
    }
    let golden_path = PathBuf::from(args.opt("golden").ok_or_else(|| format!("--golden 이 필요합니다\n{USAGE}"))?);
    let mut set = golden::load(&golden_path)?;
    // A part of the set (a model on a CPU needs minutes per item): by id and / or by kind of turn.
    let list = |name: &str| -> Vec<String> {
        args.opt(name)
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default()
    };
    let (ids, kinds) = (list("ids"), list("kinds"));
    if let Some(unknown) = kinds.iter().find(|k| !golden::KINDS.contains(&k.as_str())) {
        return Err(format!("알 수 없는 종류 {unknown} (허용: {})", golden::KINDS.join("|")));
    }
    if let Some(unknown) = ids.iter().find(|id| !set.items.iter().any(|i| &i.id == *id)) {
        return Err(format!("골든셋에 없는 id {unknown}"));
    }
    let partial = !ids.is_empty() || !kinds.is_empty();
    if partial {
        if args.opt("baseline").is_some() {
            return Err("--ids / --kinds 로 일부만 돌릴 때는 --baseline(회귀 게이트)을 쓸 수 없습니다".into());
        }
        let total = set.items.len();
        set.items.retain(|i| {
            (ids.is_empty() || ids.contains(&i.id)) && (kinds.is_empty() || kinds.iter().any(|k| k == i.kind()))
        });
        if set.items.is_empty() {
            return Err("--ids / --kinds 에 맞는 항목이 없습니다".into());
        }
        println!("부분 실행: {total}건 중 {}건", set.items.len());
    }
    let (ctx, context_name) = load_context(args, &golden_path, &set)?;
    let verbose = args.flag("verbose");
    println!(
        "골든셋 {} ({}건: {}; {}) · 컨텍스트 {} · 기준일 {}",
        golden_path.display(),
        set.items.len(),
        golden::CATEGORIES
            .iter()
            .map(|c| format!("{c} {}", set.items.iter().filter(|i| i.category == *c).count()))
            .collect::<Vec<_>>()
            .join(" / "),
        golden::KINDS
            .iter()
            .map(|k| format!("{} {}", kind_label(k), set.items.iter().filter(|i| i.kind() == *k).count()))
            .collect::<Vec<_>>()
            .join(" / "),
        context_name,
        ctx.today
    );

    let rules_pass = evaluate(&set.items, &ctx, None, verbose);
    print_pass(&rules_pass, "플래너 정확도 — 규칙 + 프리라우터(결정적)");

    let mut engine_info = None;
    let mut model_pass = None;
    if let Some(bin) = args.opt("engine") {
        let models_dir = args.opt("models-dir").ok_or("--engine 에는 --models-dir 가 필요합니다")?;
        let model = args.opt("model").ok_or("--engine 에는 --model <id> 가 필요합니다")?;
        let backend = match args.opt("backend") {
            Some(b) => Some(engine::parse_backend(b).ok_or_else(|| format!("알 수 없는 백엔드 {b}"))?),
            None => None,
        };
        let profile = match args.opt("profile") {
            Some(p) => engine::parse_profile(p).ok_or_else(|| format!("알 수 없는 프로파일 {p}"))?,
            None => Profile::Balanced,
        };
        let work_dir = std::env::temp_dir().join(format!("doculens-eval-{}", std::process::id()));
        let opts = EngineOpts {
            binary: PathBuf::from(bin),
            models_dir: PathBuf::from(models_dir),
            model: model.to_string(),
            backend,
            profile,
            work_dir,
            load_timeout: Duration::from_secs(600),
        };
        eprintln!("엔진 시작: {} · 모델 {}", opts.binary.display(), opts.model);
        let running = RunningEngine::start(&opts)?;
        eprintln!(
            "엔진 준비: 빌드 {:?} 백엔드 {} 컨텍스트 {:?} 로드 {} ms",
            running.info.build_number, running.info.backend, running.info.ctx, running.info.load_ms
        );
        let deps = PlannerDeps {
            chat: &running.manager,
            model: opts.model.clone(),
            profile,
            ctx_size: running.info.ctx,
            cancel: None,
        };
        let pass = evaluate(&set.items, &ctx, Some(&deps), verbose);
        print_pass(&pass, &format!("플래너 정확도 — 빠른 경로 + 모델 {} (제품 경로)", opts.model));
        engine_info = Some(running.info.clone());
        model_pass = Some(pass);
    }

    let (primary, rules_only) = match model_pass {
        Some(m) => (m, Some(rules_pass)),
        None => (rules_pass, None),
    };
    let results = Results {
        schema_version: RESULTS_SCHEMA_VERSION,
        kind: "planner".into(),
        generated_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        golden: golden_path.display().to_string(),
        golden_count: set.items.len() as u32,
        context: context_name,
        today: ctx.today.to_string(),
        engine: engine_info,
        primary,
        rules_only,
    };

    let mut code = 0;
    if let Some(b) = args.opt("baseline") {
        if !gate(&results.primary, Path::new(b))? {
            code = 1;
        }
    }
    if let Some(out) = args.opt("out") {
        let out = Path::new(out);
        if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&results).map_err(|e| e.to_string())?;
        std::fs::write(out, text + "\n").map_err(|e| format!("{}: {e}", out.display()))?;
        println!("결과 저장: {}", out.display());
    }
    Ok(code)
}
