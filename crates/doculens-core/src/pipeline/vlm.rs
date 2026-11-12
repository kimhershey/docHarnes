//! One place that talks to the vision model for the pipeline: builds the
//! request with the stage preset (thinking off, grammar-constrained JSON),
//! checks the reply (reasoning leak, truncation, repetition loops) and
//! retries with the §3.4 policy (temperature 0.1 → 0.3 → 0.5, ≤ 3 attempts).

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::engine::client::{ChatRequest, ChatResponse};
use crate::error::{CoreError, Result};
use crate::knowledge::answer::ChatBackend;
use crate::settings::Profile;

use super::image::VisionGeometry;
use super::prompts::{self, Stage, StageParams};

pub const MAX_ATTEMPTS: u32 = 3;

pub struct Vlm<'a> {
    pub chat: &'a dyn ChatBackend,
    pub model: String,
    pub profile: Profile,
    pub cancel: Option<&'a AtomicBool>,
    /// Starting temperature override (precision-mode second sample); `None` = stage preset.
    pub temperature: Option<f32>,
    /// Sampling seed override (`None` = the fixed default, reproducible runs).
    pub seed: Option<u64>,
    /// Context size of the loaded instance (`None` = unknown → preset anchors, adaptive retry only).
    pub ctx: Option<u32>,
    /// Vision encoder geometry of the loaded model (page render budgets).
    pub geometry: VisionGeometry,
}

impl<'a> Vlm<'a> {
    pub fn new(chat: &'a dyn ChatBackend, model: String, profile: Profile, cancel: Option<&'a AtomicBool>) -> Vlm<'a> {
        Vlm {
            chat,
            model,
            profile,
            cancel,
            temperature: None,
            seed: None,
            ctx: None,
            geometry: VisionGeometry::DEFAULT,
        }
    }
    /// Page images are sized for this encoder (Gemma 4 by default; Qwen drop-ins differ).
    pub fn with_geometry(mut self, geometry: VisionGeometry) -> Vlm<'a> {
        self.geometry = geometry;
        self
    }
    /// Budgets page-text anchors against the instance's context size.
    pub fn with_context(mut self, ctx: Option<u32>) -> Vlm<'a> {
        self.ctx = ctx;
        self
    }
    /// The same backend with its own sampling (second sample of precision mode).
    pub fn with_sampling(mut self, temperature: Option<f32>, seed: Option<u64>) -> Vlm<'a> {
        self.temperature = temperature;
        self.seed = seed;
        self
    }
}

/// HTTP 4xx from the server: the request itself is wrong (grammar rejected, context
/// overflow) and repeating it cannot help.
fn is_client_error(e: &CoreError) -> bool {
    matches!(e, CoreError::Engine(m) if m.contains("HTTP 4"))
}

/// llama-server's output parser rejected what the model generated ("The model produced
/// output that does not match the expected … format": a tool-call fragment inside a JSON
/// string, an unterminated think tag). Deterministic sampling would reproduce it exactly,
/// so the retry has to sample differently.
fn is_output_format_error(e: &CoreError) -> bool {
    matches!(e, CoreError::Engine(m) if m.contains("does not match the expected"))
}

/// The reason code of a model that cannot take images (1.0.0 §6-8). The screens translate it
/// (`errors.reasons.*` / `jobs.error.*`): the engine's own text was shown as it came, in English.
pub const NO_VISION_PROJECTOR: &str = "no_vision_projector";

/// llama-server's answer to an image sent to a model that was loaded without its vision projector:
/// "image input is not supported - hint: if this is unexpected, you may need to provide the mmproj".
/// Asking again cannot help, and every further page image would get the same answer.
pub fn no_vision(e: &CoreError) -> bool {
    matches!(e, CoreError::Engine(m) if m.contains("image input is not supported"))
}

/// `true` for the error [`Vlm::call`] makes of [`no_vision`].
pub fn no_vision_projector(e: &CoreError) -> bool {
    matches!(e, CoreError::Invalid(m) if m == NO_VISION_PROJECTOR)
}

/// The request never reached a working server: the engine client failed before any HTTP
/// status arrived (`POST <path>: io: Connection refused …`, a connect timeout) or the
/// connection broke while the reply was streamed (`stream: …`) — the process is gone.
/// Classified by the client's message shape, not by the OS error text (that one is localised).
pub fn transport_failure(e: &CoreError) -> bool {
    match e {
        CoreError::Engine(m) => {
            m.starts_with("POST ")
                || m.starts_with("GET ")
                || (m.starts_with("stream: ") && !m.to_ascii_lowercase().contains("utf-8"))
        }
        _ => false,
    }
}

/// The engine itself is unavailable — stopped, crashed, its port closed, or answering 502 / 503 /
/// 504 for a model instance that is gone — as opposed to a request it answered with an error or
/// a reply that could not be used. Nothing about the document at hand is wrong then, and the
/// next document would fail the same way within seconds (audit PIP-6).
pub fn engine_unavailable(e: &CoreError) -> bool {
    matches!(e, CoreError::EngineNotRunning)
        || transport_failure(e)
        || matches!(e, CoreError::Engine(m) if ["HTTP 502", "HTTP 503", "HTTP 504"].iter().any(|s| m.contains(s)))
}

/// Token usage of one or more calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt: u64,
    pub completion: u64,
    pub calls: u32,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.prompt += other.prompt;
        self.completion += other.completion;
        self.calls += other.calls;
    }
}

#[derive(Debug, Clone, Default)]
pub struct Reply {
    pub content: String,
    pub usage: Usage,
    pub truncated: bool,
    pub attempts: u32,
    /// The reply was cut because prompt + reply filled the context of the loaded instance: a
    /// bigger reply budget cannot help, a shorter prompt can.
    pub context_full: bool,
}

impl Vlm<'_> {
    pub fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }

    pub fn params(&self, stage: Stage) -> StageParams {
        prompts::params(stage, self.profile)
    }

    #[allow(clippy::too_many_arguments)]
    fn request(
        &self,
        stage: Stage,
        messages: &[Value],
        schema: Option<(&str, &Value)>,
        temperature: f32,
        seed: u64,
        max_tokens: u32,
    ) -> ChatRequest {
        let p = self.params(stage);
        let mut req = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            max_tokens: Some(max_tokens),
            temperature: Some(temperature),
            top_p: Some(p.top_p),
            top_k: Some(p.top_k),
            chat_template_kwargs: Some(prompts::template_kwargs()),
            seed: Some(seed),
            cache_prompt: Some(true),
            ..Default::default()
        };
        if let Some((name, s)) = schema {
            req = req.json_schema(name, s.clone());
        }
        req
    }

    /// Runs one stage call with retries. `schema` = `(name, json_schema)` for
    /// JSON stages, `None` for OCR text. `max_tokens` overrides the preset.
    pub fn call(
        &self,
        stage: Stage,
        messages: &[Value],
        schema: Option<(&str, &Value)>,
        max_tokens: Option<u32>,
    ) -> Result<Reply> {
        let p = self.params(stage);
        let mut temperature = self.temperature.unwrap_or(p.temperature);
        let mut seed = self.seed.unwrap_or(7);
        let mut budget = max_tokens.unwrap_or(p.max_tokens);
        let mut usage = Usage::default();
        let mut last_err: Option<CoreError> = None;
        let mut last: Option<ChatResponse> = None;
        let cancelled = || self.cancelled();
        for attempt in 1..=MAX_ATTEMPTS {
            if self.cancelled() {
                return Err(CoreError::Cancelled);
            }
            let req = self.request(stage, messages, schema, temperature, seed, budget);
            match self.chat.complete_cancellable(&req, &cancelled) {
                Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
                Err(e) if no_vision(&e) => {
                    log::warn!("{}: the model takes no images ({e})", stage.as_str());
                    return Err(CoreError::Invalid(NO_VISION_PROJECTOR.into()));
                }
                Err(e) if is_client_error(&e) => return Err(e),
                Err(e) => {
                    log::warn!("{} attempt {attempt}: {e}", stage.as_str());
                    if is_output_format_error(&e) {
                        seed += 1;
                        temperature = (temperature + 0.1).min(0.5);
                    }
                    last_err = Some(e);
                    continue;
                }
                Ok(res) => {
                    if let Some(u) = &res.usage {
                        usage.add(Usage { prompt: u.prompt_tokens, completion: u.completion_tokens, calls: 1 });
                    } else {
                        usage.calls += 1;
                    }
                    if res.reasoning.is_some() {
                        // Thinking leaked despite `enable_thinking: false` (design §3.4). A usable
                        // answer is kept (logged); an empty one is retried with other sampling.
                        if res.content.trim().is_empty() && attempt < MAX_ATTEMPTS {
                            log::warn!(
                                "{} attempt {attempt}: reasoning_content without content, retrying",
                                stage.as_str()
                            );
                            last_err =
                                Some(CoreError::engine("model produced reasoning_content with thinking disabled"));
                            seed += 1;
                            temperature = (temperature + 0.1).min(0.5);
                            continue;
                        }
                        log::warn!(
                            "{} attempt {attempt}: reasoning_content present, keeping the answer",
                            stage.as_str()
                        );
                    }
                    let truncated = res.finish_reason.as_deref() == Some("length");
                    // A reply that already fills the context cannot grow: a bigger budget would
                    // only repeat the (slow) generation and be cut at the same place.
                    let context_full = match (self.ctx, res.usage.as_ref()) {
                        (Some(ctx), Some(u)) => u.prompt_tokens + u.completion_tokens + 16 >= ctx as u64,
                        _ => false,
                    };
                    if truncated && context_full {
                        log::warn!(
                            "{}: reply cut by the context size ({:?} tokens), not retrying",
                            stage.as_str(),
                            self.ctx
                        );
                        return Ok(Reply {
                            content: res.content,
                            usage,
                            truncated,
                            attempts: attempt,
                            context_full: true,
                        });
                    }
                    if truncated && attempt < MAX_ATTEMPTS {
                        // JSON cut mid-way is unusable: ×1.5 up to three tries. OCR gets one ×2 pass
                        // (a dense Korean page with tables routinely needs more than 2K tokens).
                        if stage == Stage::Ocr {
                            if attempt == 1 {
                                budget = (budget * 2).min(6_144);
                                last = Some(res);
                                continue;
                            }
                        } else {
                            budget = (budget * 3 / 2).min(8_192);
                            last = Some(res);
                            continue;
                        }
                    }
                    if !truncated && prompts::looks_looped(&res.content) && attempt < MAX_ATTEMPTS {
                        temperature = (temperature + 0.2).min(0.5);
                        seed += 1;
                        last = Some(res);
                        continue;
                    }
                    return Ok(Reply {
                        content: res.content,
                        usage,
                        truncated,
                        attempts: attempt,
                        context_full: false,
                    });
                }
            }
        }
        if let Some(res) = last {
            // Exhausted retries with a (possibly looped / truncated) reply: hand it back flagged.
            let truncated = res.finish_reason.as_deref() == Some("length") || prompts::looks_looped(&res.content);
            if truncated {
                log::warn!("{}: reply still truncated after {MAX_ATTEMPTS} attempts", stage.as_str());
            }
            return Ok(Reply { content: res.content, usage, truncated, attempts: MAX_ATTEMPTS, context_full: false });
        }
        Err(last_err.unwrap_or_else(|| CoreError::engine("no reply")))
    }

    /// JSON stage: parses the reply (tolerating code fences / leading text).
    pub fn call_json(
        &self,
        stage: Stage,
        messages: &[Value],
        name: &str,
        schema: &Value,
        max_tokens: Option<u32>,
    ) -> Result<(Value, Reply)> {
        let reply = self.call(stage, messages, Some((name, schema)), max_tokens)?;
        match parse_json(&reply.content) {
            Some(v) => Ok((v, reply)),
            None => Err(unusable_json(stage, &reply)),
        }
    }
}

/// The error of a JSON stage whose reply cannot be parsed.
pub fn unusable_json(stage: Stage, reply: &Reply) -> CoreError {
    if reply.truncated {
        // The prompt count lets callers tell "the model rambled" from "the slot was full".
        CoreError::engine(format!(
            "{} reply was cut off ({} tokens; prompt {} tokens)",
            stage.as_str(),
            reply.usage.completion,
            reply.usage.prompt
        ))
    } else {
        CoreError::engine(format!("{} reply is not JSON: {}", stage.as_str(), prompts::truncate(&reply.content, 120)))
    }
}

/// Extracts the first JSON object from a reply (fences, prose or whitespace around it).
pub fn parse_json(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some(v);
    }
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Value>(&t[start..=end]).ok()
}

/// Salvages a JSON object whose end was cut off by the reply length limit: everything up to the
/// last complete member / element is kept and the containers still open there are closed.
/// `{"total": {…}, "items": [{"name": "A"}, {"name": "B", "am` → `{"total": {…}, "items":
/// [{"name": "A"}]}` — an object inside an array (a table row) is only kept whole, a row with
/// half its cells would read like a complete one. `None` when not even one complete member arrived.
pub fn repair_truncated_json(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let s = &text[start..];
    let mut stack: Vec<u8> = Vec::new();
    let (mut in_string, mut escaped) = (false, false);
    // The last place the text can be cut at: just before a `,` or just after a closing bracket
    // (both follow a complete value), with the containers that are open there.
    let mut cut: Option<(usize, Vec<u8>)> = None;
    let inside_row = |stack: &[u8]| matches!(stack, [.., b']', b'}']);
    for (i, c) in s.char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => stack.push(b'}'),
            '[' => stack.push(b']'),
            '}' | ']' => {
                if stack.pop() != Some(c as u8) {
                    return None; // not the JSON we know how to close
                }
                if stack.is_empty() {
                    // Complete after all (trailing text follows): the ordinary parser's case.
                    return serde_json::from_str(&s[..=i]).ok();
                }
                if !inside_row(&stack) {
                    cut = Some((i + 1, stack.clone()));
                }
            }
            ',' if !inside_row(&stack) => cut = Some((i, stack.clone())),
            _ => {}
        }
    }
    let (end, open) = cut?;
    let mut repaired = s[..end].to_string();
    repaired.extend(open.iter().rev().map(|b| *b as char));
    serde_json::from_str(&repaired).ok()
}

#[cfg(test)]
pub(crate) mod testing {
    //! Scripted backend keyed by stage (the `json_schema` name of the request).
    use std::sync::Mutex;

    use super::*;

    /// Members of the queue entry `push_raw` stores (raw reply text, its finish reason).
    const RAW_REPLY: &str = "__raw_reply";
    const RAW_FINISH: &str = "__raw_finish";

    pub struct StageChat {
        pub replies: Mutex<Vec<(String, Value)>>,
        /// Last consumed reply per stage — repeated when the queue for that stage is empty.
        pub last: Mutex<std::collections::HashMap<String, Value>>,
        pub requests: Mutex<Vec<ChatRequest>>,
        /// Replies for stages without a schema name (OCR), consumed in order (the last one repeats).
        pub ocr: Mutex<Vec<String>>,
        pub finish: Mutex<Option<String>>,
        pub reasoning: Mutex<Option<String>>,
        /// Error returned by every call while set (transport / HTTP simulation).
        pub fail_with: Mutex<Option<String>>,
        /// Errors returned once each, in order, before replies are served.
        pub fail_queue: Mutex<Vec<String>>,
        /// `(prompt_tokens, completion_tokens)` reported with every reply.
        pub usage: Mutex<Option<(u64, u64)>>,
    }

    impl StageChat {
        pub fn new() -> StageChat {
            StageChat {
                replies: Mutex::new(Vec::new()),
                last: Mutex::new(std::collections::HashMap::new()),
                requests: Mutex::new(Vec::new()),
                ocr: Mutex::new(Vec::new()),
                finish: Mutex::new(None),
                reasoning: Mutex::new(None),
                fail_with: Mutex::new(None),
                fail_queue: Mutex::new(Vec::new()),
                usage: Mutex::new(None),
            }
        }
        /// Queues a JSON reply for the named stage (consumed first-in first-out; the last consumed one repeats).
        pub fn push(&self, stage: &str, v: Value) {
            self.replies.lock().unwrap().push((stage.into(), v));
        }
        /// Queues a reply for the named stage as raw text with its own finish reason (a cut-off
        /// reply: `Some("length")`). It takes its turn in the stage's queue like a JSON reply but is
        /// served once only — it never becomes the repeating last reply.
        pub fn push_raw(&self, stage: &str, content: &str, finish: Option<&str>) {
            let marker = serde_json::json!({ RAW_REPLY: content, RAW_FINISH: finish });
            self.replies.lock().unwrap().push((stage.into(), marker));
        }
        pub fn push_ocr(&self, text: &str) {
            self.ocr.lock().unwrap().push(text.into());
        }
        pub fn stage_of(req: &ChatRequest) -> Option<String> {
            req.response_format.as_ref()?.get("json_schema")?.get("name")?.as_str().map(str::to_string)
        }
        pub fn calls(&self, stage: &str) -> usize {
            self.requests.lock().unwrap().iter().filter(|r| Self::stage_of(r).as_deref() == Some(stage)).count()
        }
        pub fn image_calls(&self) -> usize {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| {
                    r.messages
                        .iter()
                        .any(|m| m["content"].as_array().is_some_and(|p| p.iter().any(|x| x["type"] == "image_url")))
                })
                .count()
        }
    }

    impl ChatBackend for StageChat {
        fn model_id(&self) -> Option<String> {
            Some("scripted".into())
        }
        fn chat_stream(&self, req: &ChatRequest, on_delta: &mut dyn FnMut(&str) -> bool) -> Result<ChatResponse> {
            let res = self.complete(req)?;
            on_delta(&res.content);
            Ok(res)
        }
        fn complete(&self, req: &ChatRequest) -> Result<ChatResponse> {
            self.requests.lock().unwrap().push(req.clone());
            if let Some(e) = self.fail_with.lock().unwrap().clone() {
                return Err(CoreError::engine(e));
            }
            {
                let mut q = self.fail_queue.lock().unwrap();
                if !q.is_empty() {
                    return Err(CoreError::engine(q.remove(0)));
                }
            }
            let mut raw_finish: Option<String> = None;
            let content = match Self::stage_of(req) {
                Some(stage) => {
                    let mut replies = self.replies.lock().unwrap();
                    let idx = replies.iter().position(|(s, _)| *s == stage);
                    match idx {
                        Some(i) => {
                            let v = replies.remove(i).1;
                            match v.get(RAW_REPLY).and_then(Value::as_str) {
                                Some(raw) => {
                                    raw_finish = v.get(RAW_FINISH).and_then(Value::as_str).map(str::to_string);
                                    raw.to_string()
                                }
                                None => {
                                    self.last.lock().unwrap().insert(stage, v.clone());
                                    v.to_string()
                                }
                            }
                        }
                        None => match self.last.lock().unwrap().get(&stage) {
                            Some(v) => v.to_string(),
                            None => return Err(CoreError::engine(format!("no scripted reply for stage {stage}"))),
                        },
                    }
                }
                None => {
                    let mut ocr = self.ocr.lock().unwrap();
                    if ocr.is_empty() {
                        "null".to_string()
                    } else if ocr.len() == 1 {
                        ocr[0].clone()
                    } else {
                        ocr.remove(0)
                    }
                }
            };
            Ok(ChatResponse {
                content,
                finish_reason: Some(
                    raw_finish.or_else(|| self.finish.lock().unwrap().clone()).unwrap_or_else(|| "stop".into()),
                ),
                usage: Some({
                    let (p, c) = self.usage.lock().unwrap().unwrap_or((100, 20));
                    crate::engine::client::ChatUsage { prompt_tokens: p, completion_tokens: c, total_tokens: p + c }
                }),
                timings: None,
                model: Some("scripted".into()),
                reasoning: self.reasoning.lock().unwrap().clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::StageChat;
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_disable_thinking_and_carry_the_preset() {
        let chat = StageChat::new();
        chat.push("probe", json!({ "ok": true }));
        let vlm = Vlm::new(&chat, "m".into(), Profile::Balanced, None);
        let (v, reply) = vlm
            .call_json(
                Stage::Classify,
                &[crate::engine::client::user_message("hi")],
                "probe",
                &json!({"type": "object"}),
                None,
            )
            .unwrap();
        assert_eq!(v, json!({ "ok": true }));
        assert_eq!(reply.attempts, 1);
        let req = &chat.requests.lock().unwrap()[0];
        assert_eq!(req.chat_template_kwargs, Some(json!({ "enable_thinking": false })));
        assert_eq!(req.temperature, Some(0.0));
        assert_eq!(req.top_k, Some(20));
        assert_eq!(req.response_format.as_ref().unwrap()["json_schema"]["name"], "probe");
    }

    #[test]
    fn reasoning_leak_and_truncation_retry() {
        // A leaked reasoning block with a usable answer is kept (one call, logged).
        let chat = StageChat::new();
        chat.push("probe", json!({ "ok": true }));
        *chat.reasoning.lock().unwrap() = Some("thinking…".into());
        let vlm = Vlm::new(&chat, "m".into(), Profile::Balanced, None);
        let (v, reply) =
            vlm.call_json(Stage::Kie, &[crate::engine::client::user_message("hi")], "probe", &json!({}), None).unwrap();
        assert_eq!((v, reply.attempts), (json!({ "ok": true }), 1));
        assert_eq!(chat.calls("probe"), 1);

        // HTTP 4xx is not retried: the request itself is wrong.
        let chat = StageChat::new();
        *chat.fail_with.lock().unwrap() = Some("/v1/chat/completions: HTTP 400: grammar".into());
        let vlm = Vlm::new(&chat, "m".into(), Profile::Balanced, None);
        let err = vlm
            .call(Stage::Kie, &[crate::engine::client::user_message("hi")], Some(("probe", &json!({}))), None)
            .unwrap_err();
        assert!(err.to_string().contains("HTTP 400"), "{err}");
        assert_eq!(chat.requests.lock().unwrap().len(), 1);

        // OCR truncation gets exactly one doubled budget.
        let chat = StageChat::new();
        chat.push_ocr("partial page text");
        *chat.finish.lock().unwrap() = Some("length".into());
        let vlm = Vlm::new(&chat, "m".into(), Profile::Balanced, None);
        let reply = vlm.call(Stage::Ocr, &[crate::engine::client::user_message("ocr")], None, None).unwrap();
        assert!(reply.truncated && reply.attempts == 2);
        let budgets: Vec<Option<u32>> = chat.requests.lock().unwrap().iter().map(|r| r.max_tokens).collect();
        assert_eq!(budgets, vec![Some(2_048), Some(4_096)]);

        // The server's output-format 500 is retried with other sampling (T=0 would repeat it).
        let chat = StageChat::new();
        let fmt_err = "stream error: {\"code\":500,\"message\":\"The model produced output that does not match the expected peg-native format\"}";
        chat.fail_queue.lock().unwrap().extend([fmt_err.to_string(), fmt_err.to_string()]);
        chat.push("probe", json!({ "ok": true }));
        let vlm = Vlm::new(&chat, "m".into(), Profile::Balanced, None);
        let (_, reply) =
            vlm.call_json(Stage::Kie, &[crate::engine::client::user_message("hi")], "probe", &json!({}), None).unwrap();
        assert_eq!(reply.attempts, 3);
        let sampling: Vec<(Option<f32>, Option<u64>)> =
            chat.requests.lock().unwrap().iter().map(|r| (r.temperature, r.seed)).collect();
        assert_eq!(sampling, vec![(Some(0.0), Some(7)), (Some(0.1), Some(8)), (Some(0.2), Some(9))]);

        // A reply cut by the context itself is not retried with a bigger budget.
        let chat = StageChat::new();
        chat.push("probe", json!({ "ok": true }));
        *chat.finish.lock().unwrap() = Some("length".into());
        *chat.usage.lock().unwrap() = Some((3_000, 1_090));
        let vlm = Vlm::new(&chat, "m".into(), Profile::Balanced, None).with_context(Some(4_096));
        let reply = vlm
            .call(Stage::Kie, &[crate::engine::client::user_message("hi")], Some(("probe", &json!({}))), None)
            .unwrap();
        assert!(reply.truncated && reply.attempts == 1);
        assert_eq!(chat.requests.lock().unwrap().len(), 1);

        // The precision-mode sample carries its own temperature and seed.
        let chat = StageChat::new();
        chat.push("probe", json!({ "ok": true }));
        let vlm = Vlm::new(&chat, "m".into(), Profile::Balanced, None).with_sampling(Some(0.3), Some(11));
        vlm.call_json(Stage::Kie, &[crate::engine::client::user_message("hi")], "probe", &json!({}), None).unwrap();
        let req = chat.requests.lock().unwrap()[0].clone();
        assert_eq!((req.temperature, req.seed), (Some(0.3), Some(11)));

        let chat = StageChat::new();
        chat.push("probe", json!({ "ok": true }));
        *chat.finish.lock().unwrap() = Some("length".into());
        let vlm = Vlm::new(&chat, "m".into(), Profile::Balanced, None);
        let (_, reply) = vlm
            .call_json(Stage::Kie, &[crate::engine::client::user_message("hi")], "probe", &json!({}), Some(100))
            .unwrap();
        assert!(reply.truncated && reply.attempts == MAX_ATTEMPTS);
        let budgets: Vec<Option<u32>> = chat.requests.lock().unwrap().iter().map(|r| r.max_tokens).collect();
        assert_eq!(budgets, vec![Some(100), Some(150), Some(225)]);
    }

    #[test]
    fn parse_json_tolerates_fences() {
        assert_eq!(parse_json("```json\n{\"a\": 1}\n```"), Some(json!({"a": 1})));
        assert_eq!(parse_json("Sure: {\"a\": [1, 2]} done"), Some(json!({"a": [1, 2]})));
        assert_eq!(parse_json("nothing"), None);
    }

    /// Audit PIP-8: a reply cut by the length limit is salvaged up to its last complete member.
    #[test]
    fn truncated_json_is_salvaged_up_to_the_last_complete_member() {
        let total = json!({ "value": "1,000", "evidence": "합계 1,000", "confidence": 90 });
        // Cut inside the second table row: complete rows only.
        let cut = r#"{"total": {"value": "1,000", "evidence": "합계 1,000", "confidence": 90}, "items": [{"name": "A, B", "amount": "500"}, {"name": "C", "amo"#;
        assert_eq!(
            repair_truncated_json(cut),
            Some(json!({ "total": total, "items": [{ "name": "A, B", "amount": "500" }] }))
        );
        // Cut inside a field object: the members that arrived are kept (brackets and escaped
        // quotes inside strings do not count).
        let cut = "```json\n{\"a\": {\"value\": \"x\", \"evidence\": \"e\", \"confidence\": 9}, \"b\": {\"value\": \"y \\\"q\\\" }\", \"evid";
        assert_eq!(
            repair_truncated_json(cut),
            Some(json!({ "a": { "value": "x", "evidence": "e", "confidence": 9 }, "b": { "value": "y \"q\" }" } }))
        );
        // A number may be cut in the middle: the member before it is the last complete one.
        assert_eq!(repair_truncated_json(r#"{"a": 1, "b": 12"#), Some(json!({ "a": 1 })));
        // Nothing complete, no JSON, mismatched brackets: no salvage.
        assert_eq!(repair_truncated_json(r#"{"a": {"value": "x"#), None);
        assert_eq!(repair_truncated_json("no json here"), None);
        assert_eq!(repair_truncated_json(r#"{"a": [1, 2}, "b": 3"#), None);
        // A complete object passes through.
        assert_eq!(repair_truncated_json(r#"{"a": [1, 2]} trailing"#), Some(json!({ "a": [1, 2] })));
    }

    /// Audit PIP-6: only a missing engine stops a batch; a request the engine answered does not.
    #[test]
    fn engine_level_failures_are_told_from_request_failures() {
        let engine = |m: &str| CoreError::engine(m);
        assert!(engine_unavailable(&CoreError::EngineNotRunning));
        assert!(engine_unavailable(&engine("POST /v1/chat/completions: io: Connection refused (os error 111)")));
        assert!(engine_unavailable(&engine(
            "POST /v1/chat/completions: io: 대상 컴퓨터에서 연결을 거부했으므로 연결하지 못했습니다. (os error 10061)"
        )));
        assert!(engine_unavailable(&engine("POST /v1/chat/completions: timeout: connect")));
        assert!(engine_unavailable(&engine("stream: unexpected end of file")));
        assert!(engine_unavailable(&engine("/v1/chat/completions: HTTP 503: Loading model")));
        for answered in [
            "/v1/chat/completions: HTTP 400: grammar",
            "/v1/chat/completions: HTTP 500: The model produced output that does not match the expected peg-native format",
            "stream error: {\"code\":500,\"message\":\"x\"}",
            "stream: stream did not contain valid UTF-8",
            "kie reply is not JSON: …",
            "no scripted reply for stage classify",
        ] {
            assert!(!engine_unavailable(&engine(answered)), "{answered}");
        }
        assert!(!engine_unavailable(&CoreError::Cancelled));
        // A model without its projector is neither: the reason is named, and it is not a lost engine.
        let no_projector = engine(
            "/v1/chat/completions: HTTP 500: image input is not supported - hint: if this is unexpected, you may need to provide the mmproj",
        );
        assert!(no_vision(&no_projector) && !engine_unavailable(&no_projector));
        assert!(!no_vision(&engine("/v1/chat/completions: HTTP 400: grammar")));
        assert!(no_vision_projector(&CoreError::Invalid(NO_VISION_PROJECTOR.into())));
        assert!(
            transport_failure(&engine("GET /health: io: Connection refused"))
                && !transport_failure(&CoreError::EngineNotRunning)
        );
    }

    #[test]
    fn cancellation_stops_before_the_call() {
        let chat = StageChat::new();
        chat.push("probe", json!({}));
        let flag = AtomicBool::new(true);
        let vlm = Vlm::new(&chat, "m".into(), Profile::Balanced, Some(&flag));
        assert!(matches!(vlm.call(Stage::Kie, &[], Some(("probe", &json!({}))), None), Err(CoreError::Cancelled)));
    }
}
