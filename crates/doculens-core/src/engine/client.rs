//! Blocking HTTP client for the llama-server router API (design §3.3.1):
//! `/health`, `/models`, `/models/load|unload`, `/models/sse`,
//! `/v1/chat/completions` (with and without streaming), `/v1/embeddings`,
//! `/props`, `/metrics`. Loopback only; the API key travels in the
//! `Authorization` header.

use std::io::{BufRead, BufReader, Read};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};

#[derive(Debug, Clone)]
pub struct RouterClient {
    base: String,
    api_key: Option<String>,
    /// Control endpoints: short timeouts.
    control: ureq::Agent,
    /// Chat / embeddings / SSE: no global timeout (model loads and long prompts).
    long: ureq::Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RouterModel {
    pub id: String,
    pub path: Option<String>,
    /// `unloaded | loading | loaded | sleeping | downloading | incomplete`
    pub status: String,
    pub failed: bool,
    pub exit_code: Option<i64>,
    /// `{"stages": [...], "current": "...", "value": 0.5}` while loading.
    pub progress: Option<Value>,
    pub memory: Option<Value>,
    pub input_modalities: Vec<String>,
    pub raw: Value,
}

impl RouterModel {
    pub fn from_json(v: &Value) -> Option<RouterModel> {
        let id = v.get("id")?.as_str()?.to_string();
        let status = v.get("status").cloned().unwrap_or(Value::Null);
        Some(RouterModel {
            id,
            path: v.get("path").and_then(Value::as_str).map(str::to_string),
            status: status.get("value").and_then(Value::as_str).unwrap_or("unknown").to_string(),
            failed: status.get("failed").and_then(Value::as_bool).unwrap_or(false),
            exit_code: status.get("exit_code").and_then(Value::as_i64),
            progress: status.get("progress").cloned(),
            memory: v.get("memory").cloned().filter(|m| !m.is_null()),
            input_modalities: v
                .get("architecture")
                .and_then(|a| a.get("input_modalities"))
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
            raw: v.clone(),
        })
    }
    pub fn is_loaded(&self) -> bool {
        self.status == "loaded"
    }
    /// Bytes of device memory actually allocated (`memory.effective`), when measured.
    pub fn effective_device_bytes(&self) -> Option<u64> {
        sum_device_bytes(self.memory.as_ref()?.get("effective")?)
    }
    pub fn min_device_bytes(&self) -> Option<u64> {
        sum_device_bytes(self.memory.as_ref()?.get("min")?)
    }
    /// Context size of the running instance (`memory.effective.n_ctx`, else `min.n_ctx`).
    pub fn context_size(&self) -> Option<u32> {
        let m = self.memory.as_ref()?;
        ["effective", "min"]
            .iter()
            .find_map(|k| m.get(k)?.get("n_ctx")?.as_u64())
            .map(|n| n.min(u32::MAX as u64) as u32)
    }
    /// Load progress 0..1 from the `progress` object.
    pub fn progress_value(&self) -> Option<f32> {
        self.progress.as_ref()?.get("value")?.as_f64().map(|v| v as f32)
    }
    pub fn progress_stage(&self) -> Option<String> {
        self.progress.as_ref()?.get("current")?.as_str().map(str::to_string)
    }
}

/// Sum of `model + context + compute` over `devices[kind == "device"]`.
pub fn sum_device_bytes(measurement: &Value) -> Option<u64> {
    let devices = measurement.get("devices")?.as_array()?;
    let mut total = 0u64;
    let mut any = false;
    for d in devices {
        if d.get("kind").and_then(Value::as_str) != Some("device") {
            continue;
        }
        any = true;
        for k in ["model", "context", "compute"] {
            total += d.get(k).and_then(Value::as_u64).unwrap_or(0);
        }
    }
    any.then_some(total)
}

/// One event from `GET /models/sse`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouterEvent {
    ModelStatus { model: String, status: String, progress: Option<Value>, failed: bool },
    ModelMemory { model: String, data: Value },
    ModelRemove { model: String },
    ModelsReload,
    DownloadProgress { model: String, data: Value },
    Other { model: String, event: String, data: Value },
}

impl RouterEvent {
    pub fn from_json(v: &Value) -> Option<RouterEvent> {
        let model = v.get("model").and_then(Value::as_str).unwrap_or("").to_string();
        let event = v.get("event").and_then(Value::as_str)?.to_string();
        let data = v.get("data").cloned().unwrap_or(Value::Null);
        Some(match event.as_str() {
            "model_status" => RouterEvent::ModelStatus {
                model,
                status: data.get("status").and_then(Value::as_str).unwrap_or("unknown").to_string(),
                progress: data.get("progress").cloned(),
                failed: data.get("failed").and_then(Value::as_bool).unwrap_or(false),
            },
            "model_memory" => RouterEvent::ModelMemory { model, data },
            "model_remove" => RouterEvent::ModelRemove { model },
            "models_reload" => RouterEvent::ModelsReload,
            "download_progress" => RouterEvent::DownloadProgress { model, data },
            _ => RouterEvent::Other { model, event, data },
        })
    }
}

// ---------------------------------------------------------------------------
// Chat types (OpenAI-compatible subset)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// `{"type":"json_schema","json_schema":{"name":..,"schema":{..}}}` or `{"type":"json_object"}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

impl ChatRequest {
    /// Single user text message.
    pub fn text(model: &str, text: &str) -> ChatRequest {
        ChatRequest { model: model.to_string(), messages: vec![user_message(text)], ..Default::default() }
    }
    pub fn with_system(mut self, system: &str) -> ChatRequest {
        self.messages.insert(0, serde_json::json!({ "role": "system", "content": system }));
        self
    }
    pub fn max_tokens(mut self, n: u32) -> ChatRequest {
        self.max_tokens = Some(n);
        self
    }
    pub fn json_schema(mut self, name: &str, schema: Value) -> ChatRequest {
        self.response_format = Some(
            serde_json::json!({ "type": "json_schema", "json_schema": { "name": name, "schema": schema, "strict": true } }),
        );
        self
    }
}

pub fn user_message(text: &str) -> Value {
    serde_json::json!({ "role": "user", "content": text })
}

/// User message with an image (data URL or http URL) and a text part.
pub fn user_message_with_image(text: &str, image_url: &str) -> Value {
    serde_json::json!({
        "role": "user",
        "content": [
            { "type": "image_url", "image_url": { "url": image_url } },
            { "type": "text", "text": text }
        ]
    })
}

/// `data:image/png;base64,...` from raw bytes.
pub fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64_encode(bytes))
}

fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ChatUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ChatResponse {
    pub content: String,
    pub finish_reason: Option<String>,
    pub usage: Option<ChatUsage>,
    /// llama.cpp `timings` (prompt_ms, predicted_per_second …) when present.
    pub timings: Option<Value>,
    pub model: Option<String>,
    /// `message.reasoning_content` when the model thought despite `enable_thinking: false`
    /// (pipeline callers treat that as a failed call, design §3.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

impl ChatResponse {
    fn from_json(v: &Value) -> ChatResponse {
        let choice = v.get("choices").and_then(Value::as_array).and_then(|a| a.first());
        let content = choice
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let reasoning = choice
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("reasoning_content"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string);
        ChatResponse {
            content,
            finish_reason: choice.and_then(|c| c.get("finish_reason")).and_then(Value::as_str).map(str::to_string),
            usage: v.get("usage").and_then(|u| serde_json::from_value(u.clone()).ok()),
            timings: v.get("timings").cloned(),
            model: v.get("model").and_then(Value::as_str).map(str::to_string),
            reasoning,
        }
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

fn agent(connect: Duration, global: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(connect))
        .timeout_global(global)
        .no_delay(true)
        .build()
        .new_agent()
}

impl RouterClient {
    pub fn new(host: &str, port: u16, api_key: Option<String>) -> RouterClient {
        RouterClient {
            base: format!("http://{host}:{port}"),
            api_key,
            control: agent(Duration::from_secs(5), Some(Duration::from_secs(30))),
            long: agent(Duration::from_secs(5), None),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn auth(&self) -> Option<String> {
        self.api_key.as_ref().map(|k| format!("Bearer {k}"))
    }

    fn get(&self, agent: &ureq::Agent, path: &str) -> Result<ureq::http::Response<ureq::Body>> {
        let mut req = agent.get(format!("{}{path}", self.base));
        if let Some(a) = self.auth() {
            req = req.header("Authorization", &a);
        }
        req.call().map_err(|e| CoreError::engine(format!("GET {path}: {e}")))
    }

    fn post_json(&self, agent: &ureq::Agent, path: &str, body: &Value) -> Result<ureq::http::Response<ureq::Body>> {
        let mut req = agent.post(format!("{}{path}", self.base));
        if let Some(a) = self.auth() {
            req = req.header("Authorization", &a);
        }
        req.send_json(body).map_err(|e| CoreError::engine(format!("POST {path}: {e}")))
    }

    fn read_json(path: &str, mut resp: ureq::http::Response<ureq::Body>) -> Result<Value> {
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| CoreError::engine(format!("{path}: reading body: {e}")))?;
        if !(200..300).contains(&status) {
            return Err(CoreError::engine(format!("{path}: HTTP {status}: {}", error_message(&text))));
        }
        serde_json::from_str(&text).map_err(|e| {
            CoreError::engine(format!("{path}: invalid JSON ({e}): {}", text.chars().take(200).collect::<String>()))
        })
    }

    /// `true` when the router answers 200 on `/health`; `false` on 503 (still
    /// starting); `Err` when the port does not answer at all.
    pub fn health(&self) -> Result<bool> {
        let resp = self.get(&self.control, "/health")?;
        Ok(resp.status().as_u16() == 200)
    }

    pub fn models(&self) -> Result<Vec<RouterModel>> {
        self.models_inner(false)
    }

    /// `GET /models?reload=1` — re-reads the preset file (new models appear,
    /// removed models are unloaded; running unchanged ones are kept).
    pub fn reload_models(&self) -> Result<Vec<RouterModel>> {
        self.models_inner(true)
    }

    fn models_inner(&self, reload: bool) -> Result<Vec<RouterModel>> {
        let path = if reload { "/models?reload=1" } else { "/models" };
        let v = Self::read_json(path, self.get(&self.control, path)?)?;
        Ok(v.get("data")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(RouterModel::from_json).collect())
            .unwrap_or_default())
    }

    pub fn load(&self, model: &str) -> Result<()> {
        let v = Self::read_json(
            "/models/load",
            self.post_json(&self.control, "/models/load", &serde_json::json!({ "model": model }))?,
        )?;
        if v.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(CoreError::engine(format!("load {model}: {}", error_message(&v.to_string()))));
        }
        Ok(())
    }

    pub fn unload(&self, model: &str) -> Result<()> {
        Self::read_json(
            "/models/unload",
            self.post_json(&self.control, "/models/unload", &serde_json::json!({ "model": model }))?,
        )?;
        Ok(())
    }

    pub fn props(&self, model: Option<&str>) -> Result<Value> {
        let path = match model {
            Some(m) => format!("/props?model={}", url_encode(m)),
            None => "/props".to_string(),
        };
        Self::read_json(&path, self.get(&self.control, &path)?)
    }

    pub fn metrics(&self, model: Option<&str>) -> Result<String> {
        let path = match model {
            Some(m) => format!("/metrics?model={}", url_encode(m)),
            None => "/metrics".to_string(),
        };
        let mut resp = self.get(&self.control, &path)?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().map_err(|e| CoreError::engine(format!("{path}: {e}")))?;
        if status != 200 {
            return Err(CoreError::engine(format!("{path}: HTTP {status}")));
        }
        Ok(text)
    }

    /// Non-streaming chat completion.
    pub fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let mut body = serde_json::to_value(req).map_err(|e| CoreError::engine(e.to_string()))?;
        body["stream"] = Value::Bool(false);
        let v = Self::read_json("/v1/chat/completions", self.post_json(&self.long, "/v1/chat/completions", &body)?)?;
        Ok(ChatResponse::from_json(&v))
    }

    /// Streaming chat completion. `on_delta` receives each content fragment;
    /// returning `false` stops reading (the request is abandoned).
    pub fn chat_stream(&self, req: &ChatRequest, on_delta: &mut dyn FnMut(&str) -> bool) -> Result<ChatResponse> {
        let mut body = serde_json::to_value(req).map_err(|e| CoreError::engine(e.to_string()))?;
        body["stream"] = Value::Bool(true);
        body["stream_options"] = serde_json::json!({ "include_usage": true });
        let mut resp = self.post_json(&self.long, "/v1/chat/completions", &body)?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(CoreError::engine(format!("/v1/chat/completions: HTTP {status}: {}", error_message(&text))));
        }
        let mut out = ChatResponse::default();
        let reader = BufReader::new(resp.body_mut().as_reader());
        let mut data = String::new();
        for line in reader.lines() {
            let line = line.map_err(|e| CoreError::engine(format!("stream: {e}")))?;
            if let Some(rest) = line.strip_prefix("data:") {
                data.push_str(rest.trim_start());
                continue;
            }
            if !line.is_empty() {
                continue;
            }
            if data.is_empty() {
                continue;
            }
            let payload = std::mem::take(&mut data);
            if payload.trim() == "[DONE]" {
                break;
            }
            let Ok(v) = serde_json::from_str::<Value>(&payload) else { continue };
            if let Some(err) = v.get("error") {
                return Err(CoreError::engine(format!("stream error: {err}")));
            }
            if let Some(choice) = v.get("choices").and_then(Value::as_array).and_then(|a| a.first()) {
                if let Some(delta) = choice.get("delta").and_then(|d| d.get("content")).and_then(Value::as_str) {
                    out.content.push_str(delta);
                    if !on_delta(delta) {
                        return Err(CoreError::Cancelled);
                    }
                }
                if let Some(r) = choice.get("delta").and_then(|d| d.get("reasoning_content")).and_then(Value::as_str) {
                    if !r.is_empty() {
                        out.reasoning.get_or_insert_with(String::new).push_str(r);
                    }
                }
                if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
                    out.finish_reason = Some(fr.to_string());
                }
            }
            if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                out.usage = serde_json::from_value(u.clone()).ok();
            }
            if let Some(t) = v.get("timings") {
                out.timings = Some(t.clone());
            }
            if out.model.is_none() {
                out.model = v.get("model").and_then(Value::as_str).map(str::to_string);
            }
        }
        Ok(out)
    }

    /// `POST /v1/embeddings` for one or more inputs.
    pub fn embeddings(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let body = serde_json::json!({ "model": model, "input": inputs });
        let v = Self::read_json("/v1/embeddings", self.post_json(&self.long, "/v1/embeddings", &body)?)?;
        let mut out = Vec::new();
        for item in v.get("data").and_then(Value::as_array).into_iter().flatten() {
            let emb: Vec<f32> = item
                .get("embedding")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_f64).map(|f| f as f32).collect())
                .unwrap_or_default();
            out.push(emb);
        }
        Ok(out)
    }

    /// Subscribes to `GET /models/sse` and blocks, calling `on_event` for every
    /// event until it returns `false` or the connection closes.
    pub fn sse(&self, on_event: &mut dyn FnMut(RouterEvent) -> bool) -> Result<()> {
        let mut resp = self.get(&self.long, "/models/sse")?;
        let status = resp.status().as_u16();
        if status != 200 {
            return Err(CoreError::engine(format!("/models/sse: HTTP {status}")));
        }
        let reader = BufReader::new(resp.body_mut().as_reader());
        let mut data = String::new();
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => return Err(CoreError::engine(format!("sse: {e}"))),
            };
            if let Some(rest) = line.strip_prefix("data:") {
                data.push_str(rest.trim_start());
                continue;
            }
            if line.starts_with(':') {
                continue; // keep-alive comment
            }
            if !line.is_empty() || data.is_empty() {
                continue;
            }
            let payload = std::mem::take(&mut data);
            if let Ok(v) = serde_json::from_str::<Value>(&payload) {
                if let Some(ev) = RouterEvent::from_json(&v) {
                    if !on_event(ev) {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    /// Reads the raw body of an arbitrary GET (diagnostics).
    pub fn get_text(&self, path: &str) -> Result<(u16, String)> {
        let mut resp = self.get(&self.control, path)?;
        let status = resp.status().as_u16();
        let mut text = String::new();
        resp.body_mut()
            .as_reader()
            .take(1 << 20)
            .read_to_string(&mut text)
            .map_err(|e| CoreError::engine(e.to_string()))?;
        Ok((status, text))
    }
}

/// Extracts `error.message` from an OpenAI-style error body, else the text.
pub fn error_message(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.get("message")).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| text.chars().take(300).collect())
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::fake_router::{FakeRouter, FakeRouterConfig};

    #[test]
    fn parses_models_and_events() {
        let v = serde_json::json!({
            "id": "qwen", "path": "/x.gguf",
            "status": { "value": "unloaded", "failed": true, "exit_code": 1 },
            "memory": { "status": "measured", "effective": { "n_ctx": 4096, "devices": [
                { "kind": "device", "name": "CUDA0", "model": 100, "context": 20, "compute": 5 },
                { "kind": "host", "name": "Host", "model": 999 } ] } },
            "architecture": { "input_modalities": ["text", "image"] }
        });
        let m = RouterModel::from_json(&v).unwrap();
        assert_eq!(m.status, "unloaded");
        assert!(m.failed);
        assert_eq!(m.exit_code, Some(1));
        assert_eq!(m.effective_device_bytes(), Some(125));
        assert_eq!(m.input_modalities, vec!["text", "image"]);
        let e = RouterEvent::from_json(&serde_json::json!({ "model": "qwen", "event": "model_status", "data": { "status": "loading", "progress": { "value": 0.5 } } })).unwrap();
        assert!(matches!(e, RouterEvent::ModelStatus { ref status, .. } if status == "loading"));
        assert_eq!(
            RouterEvent::from_json(&serde_json::json!({ "model": "*", "event": "models_reload" })),
            Some(RouterEvent::ModelsReload)
        );
        assert_eq!(error_message(r#"{"error":{"code":401,"message":"Invalid API Key"}}"#), "Invalid API Key");
        assert_eq!(url_encode("a/b:c"), "a%2Fb%3Ac");
        assert_eq!(base64_encode(b"hi"), "aGk=");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn talks_to_the_fake_router() {
        let router = FakeRouter::start(FakeRouterConfig {
            models: vec!["qwen3.5-4b-q6k".into(), "embed".into()],
            load_delay: Duration::from_millis(50),
            api_key: Some("secret".into()),
            ..Default::default()
        })
        .unwrap();
        let bad = RouterClient::new("127.0.0.1", router.port, Some("wrong".into()));
        assert!(bad.health().unwrap()); // health is public
        assert!(bad.models().unwrap_err().to_string().contains("401"));

        let c = RouterClient::new("127.0.0.1", router.port, Some("secret".into()));
        let models = c.models().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].status, "unloaded");

        // SSE subscriber sees the load transition
        let c2 = c.clone();
        let events = std::thread::spawn(move || {
            let mut seen = Vec::new();
            c2.sse(&mut |ev| {
                if let RouterEvent::ModelStatus { status, .. } = &ev {
                    seen.push(status.clone());
                    return status != "loaded";
                }
                true
            })
            .unwrap();
            seen
        });
        std::thread::sleep(Duration::from_millis(50));
        c.load("qwen3.5-4b-q6k").unwrap();
        let seen = events.join().unwrap();
        assert_eq!(seen.first().map(String::as_str), Some("loading"));
        assert_eq!(seen.last().map(String::as_str), Some("loaded"));
        let m = c.models().unwrap();
        assert!(m[0].is_loaded());
        assert!(m[0].effective_device_bytes().is_some());

        let r = c.chat(&ChatRequest::text("qwen3.5-4b-q6k", "hello").max_tokens(8)).unwrap();
        assert!(r.content.contains("fake"));
        assert_eq!(r.finish_reason.as_deref(), Some("stop"));
        assert!(r.usage.is_some());

        let mut deltas = Vec::new();
        let r = c
            .chat_stream(&ChatRequest::text("qwen3.5-4b-q6k", "stream please"), &mut |d| {
                deltas.push(d.to_string());
                true
            })
            .unwrap();
        assert!(deltas.len() >= 2);
        assert_eq!(deltas.concat(), r.content);

        let j = c
            .chat(&ChatRequest::text("qwen3.5-4b-q6k", "json").json_schema("t", serde_json::json!({"type":"object"})))
            .unwrap();
        assert!(serde_json::from_str::<Value>(&j.content).is_ok());

        let e = c.embeddings("embed", &["a".into(), "b".into()]).unwrap();
        assert_eq!(e.len(), 2);
        assert!(!e[0].is_empty());

        c.unload("qwen3.5-4b-q6k").unwrap();
        assert_eq!(c.models().unwrap()[0].status, "unloaded");
        assert!(c.load("nope").is_err());
        assert!(c.props(Some("embed")).is_ok());
        assert!(c.metrics(None).is_ok());
        router.stop();
    }
}
