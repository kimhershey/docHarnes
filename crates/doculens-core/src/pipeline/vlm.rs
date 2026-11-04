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

use super::prompts::{self, Stage, StageParams};

pub const MAX_ATTEMPTS: u32 = 3;

pub struct Vlm<'a> {
    pub chat: &'a dyn ChatBackend,
    pub model: String,
    pub profile: Profile,
    pub cancel: Option<&'a AtomicBool>,
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

#[derive(Debug, Clone)]
pub struct Reply {
    pub content: String,
    pub usage: Usage,
    pub truncated: bool,
    pub attempts: u32,
}

impl Vlm<'_> {
    pub fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }

    pub fn params(&self, stage: Stage) -> StageParams {
        prompts::params(stage, self.profile)
    }

    fn request(
        &self,
        stage: Stage,
        messages: &[Value],
        schema: Option<(&str, &Value)>,
        temperature: f32,
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
            seed: Some(7),
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
        let mut temperature = p.temperature;
        let mut budget = max_tokens.unwrap_or(p.max_tokens);
        let mut usage = Usage::default();
        let mut last_err: Option<CoreError> = None;
        let mut last: Option<ChatResponse> = None;
        for attempt in 1..=MAX_ATTEMPTS {
            if self.cancelled() {
                return Err(CoreError::Cancelled);
            }
            let req = self.request(stage, messages, schema, temperature, budget);
            match self.chat.complete(&req) {
                Err(e) => {
                    log::warn!("{} attempt {attempt}: {e}", stage.as_str());
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
                        // Thinking leaked despite `enable_thinking: false` → retry (design §3.4).
                        log::warn!("{} attempt {attempt}: reasoning_content present, retrying", stage.as_str());
                        last_err = Some(CoreError::engine("model produced reasoning_content with thinking disabled"));
                        continue;
                    }
                    let truncated = res.finish_reason.as_deref() == Some("length");
                    if truncated && attempt < MAX_ATTEMPTS && stage != Stage::Ocr {
                        // JSON cut mid-way is unusable; give the model more room once.
                        budget = (budget * 3 / 2).min(8_192);
                        last = Some(res);
                        continue;
                    }
                    if prompts::looks_looped(&res.content) && attempt < MAX_ATTEMPTS {
                        temperature = (temperature + 0.2).min(0.5);
                        last = Some(res);
                        continue;
                    }
                    return Ok(Reply { content: res.content, usage, truncated, attempts: attempt });
                }
            }
        }
        if let Some(res) = last {
            // Exhausted retries with a (possibly looped / truncated) reply: hand it back flagged.
            let truncated = res.finish_reason.as_deref() == Some("length") || prompts::looks_looped(&res.content);
            return Ok(Reply { content: res.content, usage, truncated, attempts: MAX_ATTEMPTS });
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
            None => {
                if reply.truncated {
                    Err(CoreError::engine(format!(
                        "{} reply was cut off ({} tokens)",
                        stage.as_str(),
                        reply.usage.completion
                    )))
                } else {
                    Err(CoreError::engine(format!(
                        "{} reply is not JSON: {}",
                        stage.as_str(),
                        prompts::truncate(&reply.content, 120)
                    )))
                }
            }
        }
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

#[cfg(test)]
pub(crate) mod testing {
    //! Scripted backend keyed by stage (the `json_schema` name of the request).
    use std::sync::Mutex;

    use super::*;

    pub struct StageChat {
        pub replies: Mutex<Vec<(String, Value)>>,
        /// Last consumed reply per stage — repeated when the queue for that stage is empty.
        pub last: Mutex<std::collections::HashMap<String, Value>>,
        pub requests: Mutex<Vec<ChatRequest>>,
        /// Replies for stages without a schema name (OCR), consumed in order (the last one repeats).
        pub ocr: Mutex<Vec<String>>,
        pub finish: Mutex<Option<String>>,
        pub reasoning: Mutex<Option<String>>,
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
            }
        }
        /// Queues a JSON reply for the named stage (consumed first-in first-out; the last consumed one repeats).
        pub fn push(&self, stage: &str, v: Value) {
            self.replies.lock().unwrap().push((stage.into(), v));
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
            let content = match Self::stage_of(req) {
                Some(stage) => {
                    let mut replies = self.replies.lock().unwrap();
                    let idx = replies.iter().position(|(s, _)| *s == stage);
                    match idx {
                        Some(i) => {
                            let v = replies.remove(i).1;
                            self.last.lock().unwrap().insert(stage, v.clone());
                            v.to_string()
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
                finish_reason: Some(self.finish.lock().unwrap().clone().unwrap_or_else(|| "stop".into())),
                usage: Some(crate::engine::client::ChatUsage {
                    prompt_tokens: 100,
                    completion_tokens: 20,
                    total_tokens: 120,
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
        let vlm = Vlm { chat: &chat, model: "m".into(), profile: Profile::Balanced, cancel: None };
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
        let chat = StageChat::new();
        chat.push("probe", json!({ "ok": true }));
        *chat.reasoning.lock().unwrap() = Some("thinking…".into());
        let vlm = Vlm { chat: &chat, model: "m".into(), profile: Profile::Balanced, cancel: None };
        let err = vlm
            .call_json(Stage::Kie, &[crate::engine::client::user_message("hi")], "probe", &json!({}), None)
            .unwrap_err();
        assert!(err.to_string().contains("reasoning_content"), "{err}");
        assert_eq!(chat.calls("probe"), MAX_ATTEMPTS as usize);

        let chat = StageChat::new();
        chat.push("probe", json!({ "ok": true }));
        *chat.finish.lock().unwrap() = Some("length".into());
        let vlm = Vlm { chat: &chat, model: "m".into(), profile: Profile::Balanced, cancel: None };
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

    #[test]
    fn cancellation_stops_before_the_call() {
        let chat = StageChat::new();
        chat.push("probe", json!({}));
        let flag = AtomicBool::new(true);
        let vlm = Vlm { chat: &chat, model: "m".into(), profile: Profile::Balanced, cancel: Some(&flag) };
        assert!(matches!(vlm.call(Stage::Kie, &[], Some(("probe", &json!({}))), None), Err(CoreError::Cancelled)));
    }
}
