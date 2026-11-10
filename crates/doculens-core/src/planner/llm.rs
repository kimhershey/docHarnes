//! Planning entry point: pre-router → phrase rules → model with one re-query.

use std::sync::atomic::AtomicBool;

use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::knowledge::answer::ChatBackend;
use crate::pipeline::prompts::Stage;
use crate::pipeline::vlm::Vlm;
use crate::prerouter::{self, PreRoutedOp};
use crate::settings::{Language, Profile};

use super::rules::is_compound;
use super::{
    has_errors, prompts, rules, schema, validate, Op, OpenTarget, Plan, PlanContext, PlanIssue, PlanMode, PlanResult,
    PlanSource, MIN_CONFIDENCE,
};

/// Model access for the planner (`None` in `plan` = rules only).
pub struct PlannerDeps<'a> {
    pub chat: &'a dyn ChatBackend,
    pub model: String,
    pub profile: Profile,
    /// Context size of the loaded instance (compact prompt below 6K).
    pub ctx_size: Option<u32>,
    pub cancel: Option<&'a AtomicBool>,
}

/// Reply budget of a plan call (`Stage::Plan` preset).
pub const REPLY_TOKENS: u32 = 1_024;

fn clarification_text(ko: bool, reason: &str) -> String {
    if ko {
        format!("명령을 해석하지 못했습니다({reason}). 무엇을 할지 조금 더 구체적으로 말씀해 주세요 — 예: '지난달 영수증 합계', '세금계산서를 엑셀로 내보내'.")
    } else {
        format!("I could not interpret the command ({reason}). Please say more precisely what to do — e.g. 'total of last month's receipts', 'export tax invoices to Excel'.")
    }
}

fn prerouted(text: &str, ctx: &PlanContext) -> Option<Plan> {
    let r = prerouter::route(text);
    let ko = ctx.language == Language::Ko;
    let op = r.op?;
    let (op, summary) = match op {
        PreRoutedOp::SetTheme { theme } => (
            Op::SetTheme { theme },
            if ko {
                format!("테마를 {}(으)로 바꿉니다", theme.as_str())
            } else {
                format!("Switch the theme to {}", theme.as_str())
            },
        ),
        PreRoutedOp::SetLanguage { language } => (
            Op::SetLanguage { lang: language },
            if ko {
                format!("UI 언어를 {}(으)로 바꿉니다", language.as_str())
            } else {
                format!("Switch the UI language to {}", language.as_str())
            },
        ),
        PreRoutedOp::Open { screen } => (
            Op::Open { target: OpenTarget::Screen, reference: screen.route().trim_start_matches('/').to_string() },
            if ko { "화면을 엽니다".to_string() } else { "Open the screen".to_string() },
        ),
        PreRoutedOp::Help => (Op::Help {}, if ko { "도움말".to_string() } else { "Help".to_string() }),
    };
    let mut plan = Plan::single(op, r.confidence, summary);
    if let Op::Open { reference, .. } = &mut plan.operations[0] {
        if reference.is_empty() {
            *reference = "home".into();
        }
    }
    Some(plan)
}

/// Plans one command. Never fails on model trouble: the result carries a
/// clarification plan and the issue instead (cancellation propagates).
pub fn plan(deps: Option<&PlannerDeps<'_>>, text: &str, ctx: &PlanContext, mode: PlanMode) -> Result<PlanResult> {
    let text = text.trim();
    let ko = ctx.language == Language::Ko;
    let date = prerouter::resolve_relative_date(text, ctx.today);
    let mut result = PlanResult {
        text: text.to_string(),
        source: PlanSource::None,
        plan: Plan::clarify(clarification_text(ko, if ko { "빈 명령" } else { "empty" }), 0.0),
        issues: Vec::new(),
        date: date.clone(),
        attempts: 0,
        model: None,
        prompt_tokens: 0,
        completion_tokens: 0,
        raw: None,
    };
    if text.is_empty() {
        return Ok(result);
    }

    let scoped = |plan: Plan| guard_scope(plan, text, ko);
    if mode == PlanMode::ModelFirst {
        // Slash commands stay deterministic; every other sentence is the model's to read.
        if text.starts_with('/') {
            if let Some(mut p) = prerouted(text, ctx) {
                let issues = validate::validate(&mut p, ctx, date.as_ref());
                if !has_errors(&issues) {
                    result.source = PlanSource::Prerouter;
                    result.plan = p;
                    result.issues = issues;
                    return Ok(result);
                }
            }
        }
    } else {
        // 1. deterministic pre-router (settings / navigation / help). Compound sentences
        //    ("다크 테마로 바꾸고 모델을 9B로 올려") go to the rules first so the second clause is kept.
        if is_compound(text) {
            if let Some(mut p) = rules::plan(text, ctx) {
                let issues = validate::validate(&mut p, ctx, date.as_ref());
                if !has_errors(&issues) {
                    result.source = PlanSource::Rules;
                    result.plan = scoped(p);
                    result.issues = issues;
                    return Ok(result);
                }
            }
        }
        if let Some(mut p) = prerouted(text, ctx) {
            let issues = validate::validate(&mut p, ctx, date.as_ref());
            if !has_errors(&issues) {
                result.source = PlanSource::Prerouter;
                result.plan = p;
                result.issues = issues;
                return Ok(result);
            }
        }

        // 2. phrase rules
        if let Some(mut p) = rules::plan(text, ctx) {
            let issues = validate::validate(&mut p, ctx, date.as_ref());
            if !has_errors(&issues) {
                result.source = PlanSource::Rules;
                result.plan = scoped(p);
                result.issues = issues;
                return Ok(result);
            }
            // A rule that produced an invalid plan (e.g. unknown field) still beats a model guess:
            // ask the user with the first error.
            if let Some(first) = issues.iter().find(|i| i.blocking) {
                result.source = PlanSource::Rules;
                result.plan = Plan::clarify(
                    if ko {
                        format!("{} — {}", first.message, "어떻게 할까요?")
                    } else {
                        format!("{} — what should I do?", first.message)
                    },
                    0.5,
                );
                result.issues = issues;
                return Ok(result);
            }
        }
    }
    // 3. the model
    let Some(deps) = deps else {
        result.issues.push(PlanIssue::error("", "no_model", "the planner model is not available"));
        result.plan = Plan::clarify(
            match (mode, ko) {
                (PlanMode::ModelFirst, true) => {
                    "플래너 모델이 로드되지 않아 해석할 수 없습니다. 모델을 로드하거나 해석 방식을 '규칙'으로 바꿔 주세요."
                }
                (PlanMode::ModelFirst, false) => {
                    "The planner model is not loaded, so the sentence cannot be interpreted. Load a model or switch the interpretation to 'rules'."
                }
                (PlanMode::RulesFirst, true) => {
                    "규칙으로 해석할 수 없는 명령이고 모델이 로드되지 않아 플랜을 만들 수 없습니다. 모델을 로드하거나 명령을 바꿔 주세요."
                }
                (PlanMode::RulesFirst, false) => {
                    "The command needs the planner model, which is not loaded. Load a model or rephrase."
                }
            },
            0.0,
        );
        return Ok(result);
    };
    result.model = Some(deps.model.clone());
    let vlm = Vlm::new(deps.chat, deps.model.clone(), deps.profile, deps.cancel).with_context(deps.ctx_size);
    let plan_schema = schema::plan_schema();
    let mut budget = prompts::prompt_budget(deps.ctx_size, REPLY_TOKENS);
    let mut feedback: Option<(String, Vec<PlanIssue>)> = None;
    let mut attempt = 0u32;
    let mut shrinks = 0u32;
    while attempt < 2 {
        attempt += 1;
        result.attempts = attempt;
        let fb = feedback.as_ref().map(|(p, i)| (p.as_str(), i.as_slice()));
        let (messages, tier) = prompts::messages(ctx, text, date.as_ref(), budget, fb);
        let (value, reply) = match vlm.call_json(Stage::Plan, &messages, "plan", &plan_schema, Some(REPLY_TOKENS)) {
            Ok(v) => v,
            Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
            Err(e) if is_context_overflow(&e) && tier > prompts::Tier::Tiny && shrinks < 3 => {
                // (a cut-off after a long reply is the model rambling, not a full slot — see `is_context_overflow`)
                // The server's tokenizer counted more than our estimate: retry one tier smaller. When the
                // server reported its counts, scale our estimate to them so the next tier really fits.
                shrinks += 1;
                attempt -= 1;
                let used = prompts::message_tokens(&messages);
                let calibrated = reported_counts(&e).and_then(|(prompt, completion)| {
                    let slot = prompt + completion;
                    let ratio = prompt as f32 / used.max(1) as f32; // server tokens per estimated token
                    let room = slot.checked_sub(REPLY_TOKENS + 128)?;
                    Some(((room as f32 / ratio.max(0.5)) as u32).max(128))
                });
                let fallback = budget.unwrap_or(used).min(used).saturating_sub(used / 4).max(256);
                budget = Some(calibrated.map(|c| c.min(fallback)).unwrap_or(fallback));
                log::info!("planner: prompt exceeded the context at tier {tier:?}; retrying smaller ({})", e);
                continue;
            }
            Err(e) => {
                result.issues = vec![PlanIssue::error("", "model_error", e.to_string())];
                result.plan = Plan::clarify(clarification_text(ko, if ko { "모델 오류" } else { "model error" }), 0.0);
                return Ok(result);
            }
        };
        result.prompt_tokens += reply.usage.prompt;
        result.completion_tokens += reply.usage.completion;
        result.raw = Some(reply.content.clone());
        let parsed = parse_and_validate(&value, ctx, date.as_ref());
        match parsed {
            Ok((mut plan, mut issues)) => {
                result.source = PlanSource::Model;
                // A sentence that carries its own text is answered from that text, not from the library.
                if let Some(note) = super::guard::prefer_text_only_answer(text, &mut plan, ko) {
                    issues.push(note);
                }
                result.issues = issues;
                result.plan = scoped(finalize(plan, ko));
                return Ok(result);
            }
            Err(issues) => {
                result.issues = issues.clone();
                feedback = Some((reply.content, issues));
            }
        }
    }
    // Two rejected attempts → clarification with the first error.
    let first = result.issues.iter().find(|i| i.blocking).map(|i| i.message.clone()).unwrap_or_default();
    result.source = PlanSource::Model;
    result.plan = Plan::clarify(clarification_text(ko, &first), 0.0);
    Ok(result)
}

/// llama-server rejects prompts longer than the slot context with HTTP 400
/// "request (N tokens) exceeds the available context size"; some builds instead clamp the
/// reply to the tokens left in the slot, which surfaces as a reply "cut off" after a few
/// dozen tokens. Both mean the prompt was too big for the context: retry one tier smaller.
fn is_context_overflow(e: &CoreError) -> bool {
    match e {
        CoreError::Engine(m) if m.contains("exceeds the available context") => true,
        // A reply clamped after a few dozen tokens means the slot was full; a reply cut at the
        // reply budget means the model rambled (grammar-constrained garbage from tiny / broken models).
        CoreError::Engine(m) if m.contains("was cut off") => {
            reported_counts(e).map(|(_, completion)| completion < REPLY_TOKENS / 2).unwrap_or(true)
        }
        _ => false,
    }
}

/// `(prompt tokens, completion tokens)` from a cut-off error ("… cut off (93 tokens; prompt 4003 tokens)").
fn reported_counts(e: &CoreError) -> Option<(u32, u32)> {
    let CoreError::Engine(m) = e else { return None };
    let completion = m.split("cut off (").nth(1)?.split(' ').next()?.parse().ok()?;
    let prompt = m.split("prompt ").nth(1)?.split(' ').next()?.parse().ok()?;
    Some((prompt, completion))
}

fn parse_and_validate(
    value: &Value,
    ctx: &PlanContext,
    date: Option<&prerouter::DateRange>,
) -> std::result::Result<(Plan, Vec<PlanIssue>), Vec<PlanIssue>> {
    let mut plan = validate::parse_plan(value).map_err(|i| vec![i])?;
    let issues = validate::validate(&mut plan, ctx, date);
    if has_errors(&issues) {
        Err(issues)
    } else {
        Ok((plan, issues))
    }
}

/// Document-changing ops without a scope ask which documents are meant, unless the sentence
/// explicitly targeted the whole library.
fn guard_scope(plan: Plan, text: &str, ko: bool) -> Plan {
    if plan.is_clarification() || rules::mentions_all(text) || validate::scope_required(&plan).is_none() {
        return plan;
    }
    let mut c = Plan::clarify(
        if ko {
            "어떤 문서를 대상으로 할까요? 라이브러리에서 선택하거나 폴더·유형·날짜를 말씀해 주세요(전체면 '전부'라고 해 주세요)."
        } else {
            "Which documents? Select them in the library or name a folder / type / date range (say 'all documents' for the whole library)."
        },
        plan.confidence.min(0.5),
    );
    c.summary = plan.summary.clone();
    c
}

/// Low confidence → clarification first (§3.5).
fn finalize(plan: Plan, ko: bool) -> Plan {
    if plan.is_clarification() {
        return plan;
    }
    if plan.confidence < MIN_CONFIDENCE {
        let q = plan.clarification_question.clone().unwrap_or_else(|| {
            let s = plan.summary.clone().unwrap_or_default();
            if ko {
                format!("이렇게 이해했습니다: {s}. 맞나요? 아니면 더 구체적으로 말씀해 주세요.")
            } else {
                format!("I understood: {s}. Is that right? Otherwise please be more specific.")
            }
        });
        let mut c = Plan::clarify(q, plan.confidence);
        c.summary = plan.summary.clone();
        return c;
    }
    plan
}
