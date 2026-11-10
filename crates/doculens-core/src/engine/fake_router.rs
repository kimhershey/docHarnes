//! In-process stand-in for `llama-server` router mode. Implements the subset
//! of the API the app uses (`/health`, `/models`, `/models/load|unload`,
//! `/models/sse`, `/v1/chat/completions`, `/v1/embeddings`, `/props`,
//! `/metrics`) over plain `std::net`, so the lifecycle code can be tested on
//! CI without llama.cpp and the UI can be demoed without a model.
//!
//! [`run_from_args`] turns any binary into a fake `llama-server` (used by the
//! process-level tests: the test executable re-executes itself).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct FakeRouterConfig {
    /// 0 = any free port.
    pub port: u16,
    pub host: String,
    pub models: Vec<String>,
    pub load_delay: Duration,
    pub api_key: Option<String>,
    /// Models whose load fails (status `unloaded`, `failed = true`).
    pub fail_load: Vec<String>,
    pub reply: String,
    /// Re-read on `GET /models?reload=1` when set.
    pub preset_path: Option<std::path::PathBuf>,
}

impl Default for FakeRouterConfig {
    fn default() -> Self {
        Self {
            port: 0,
            host: "127.0.0.1".into(),
            models: Vec::new(),
            load_delay: Duration::from_millis(10),
            api_key: None,
            fail_load: Vec::new(),
            reply: "Hello from the fake router.".into(),
            preset_path: None,
        }
    }
}

#[derive(Debug, Clone)]
struct ModelState {
    status: String,
    failed: bool,
    exit_code: Option<i64>,
    progress: Option<Value>,
}

impl ModelState {
    fn unloaded() -> Self {
        Self { status: "unloaded".into(), failed: false, exit_code: None, progress: None }
    }
}

struct State {
    cfg: FakeRouterConfig,
    models: Vec<(String, ModelState)>,
    sse: Vec<TcpStream>,
}

impl State {
    fn model_json(&self, id: &str, m: &ModelState) -> Value {
        let mut status = json!({ "value": m.status });
        if m.failed {
            status["failed"] = json!(true);
            status["exit_code"] = json!(m.exit_code.unwrap_or(1));
        }
        if let Some(p) = &m.progress {
            status["progress"] = p.clone();
        }
        let mut v = json!({
            "id": id,
            "object": "model",
            "path": format!("/fake/{id}.gguf"),
            "tags": ["local"],
            "status": status,
            "architecture": { "input_modalities": if id.contains("embed") { json!(["text"]) } else { json!(["text", "image"]) }, "output_modalities": ["text"] },
        });
        if m.status == "loaded" {
            v["memory"] = json!({
                "status": "measured",
                "min": { "n_ctx": 4096, "devices": [ { "kind": "device", "name": "CUDA0", "total": 8589934592u64, "free": 7000000000u64, "model": 3000000000u64, "context": 200000000u64, "compute": 300000000u64 } ] },
                "max": { "n_ctx": 32768, "devices": [] },
                "effective": { "n_ctx": 32768, "devices": [ { "kind": "device", "name": "CUDA0", "total": 8589934592u64, "free": 3000000000u64, "model": 3000000000u64, "context": 900000000u64, "compute": 400000000u64 }, { "kind": "host", "name": "Host", "model": 100, "context": 0, "compute": 0 } ] }
            });
        } else {
            v["memory"] = json!({ "status": "measured", "min": { "n_ctx": 4096, "devices": [] }, "max": { "n_ctx": 32768, "devices": [] }, "effective": null });
        }
        v
    }

    fn broadcast(&mut self, event: &str, model: &str, data: Value) {
        let mut payload = json!({ "model": model, "event": event });
        if !data.is_null() {
            payload["data"] = data;
        }
        let line = format!("data: {payload}\n\n");
        self.sse.retain_mut(|s| s.write_all(line.as_bytes()).and_then(|_| s.flush()).is_ok());
    }
}

/// Section names of a preset file (everything but `[*]`).
pub fn models_from_preset(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with('[') && l.ends_with(']'))
        .map(|l| l[1..l.len() - 1].trim().to_string())
        .filter(|s| s != "*" && !s.is_empty())
        .collect()
}

/// Section names with `load-on-startup = true`.
pub fn load_on_startup_from_preset(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            let name = line[1..line.len() - 1].trim().to_string();
            current = (name != "*").then_some(name);
        } else if let (Some(sec), Some((k, v))) = (&current, line.split_once('=')) {
            if k.trim() == "load-on-startup" && v.trim() == "true" {
                out.push(sec.clone());
            }
        }
    }
    out
}

pub struct FakeRouter {
    pub port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeRouter {
    pub fn start(cfg: FakeRouterConfig) -> std::io::Result<FakeRouter> {
        let listener = TcpListener::bind((cfg.host.as_str(), cfg.port))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread =
            std::thread::Builder::new().name("fake-router".into()).spawn(move || serve(listener, cfg, stop2))?;
        Ok(FakeRouter { port, stop, thread: Some(thread) })
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for FakeRouter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Blocks serving until `stop` is set.
pub fn serve(listener: TcpListener, cfg: FakeRouterConfig, stop: Arc<AtomicBool>) {
    listener.set_nonblocking(true).expect("nonblocking");
    let mut models: Vec<(String, ModelState)> =
        cfg.models.iter().map(|m| (m.clone(), ModelState::unloaded())).collect();
    if let Some(p) = &cfg.preset_path {
        for m in models_from_preset(p) {
            if !models.iter().any(|(id, _)| *id == m) {
                models.push((m, ModelState::unloaded()));
            }
        }
    }
    let startup: Vec<String> = cfg.preset_path.as_ref().map(|p| load_on_startup_from_preset(p)).unwrap_or_default();
    let load_delay = cfg.load_delay;
    let state = Arc::new(Mutex::new(State { cfg, models, sse: Vec::new() }));
    // `load-on-startup = true` sections start loading as soon as the router is up.
    for id in startup {
        let st = state.clone();
        {
            let mut s = st.lock().unwrap();
            if let Some((_, m)) = s.models.iter_mut().find(|(m, _)| *m == id) {
                m.status = "loading".into();
            }
            s.broadcast("model_status", &id, json!({ "status": "loading" }));
        }
        std::thread::spawn(move || {
            std::thread::sleep(load_delay);
            let mut s = st.lock().unwrap();
            if let Some((_, m)) = s.models.iter_mut().find(|(m, _)| *m == id) {
                if m.status == "loading" {
                    m.status = "loaded".into();
                    s.broadcast("model_status", &id, json!({ "status": "loaded", "info": {} }));
                }
            }
        });
    }
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let st = state.clone();
                let _ = stream.set_nonblocking(false);
                std::thread::spawn(move || handle(stream, st));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
}

struct Request {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = BTreeMap::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    let (path, query_str) =
        target.split_once('?').map(|(p, q)| (p.to_string(), q.to_string())).unwrap_or((target.clone(), String::new()));
    let mut query = BTreeMap::new();
    for pair in query_str.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        query.insert(k.to_string(), url_decode(v));
    }
    Some(Request { method, path, query, headers, body })
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() + 1 && i + 2 <= bytes.len() - 1 + 1 {
            if let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..(i + 3).min(bytes.len())]).unwrap_or("zz"), 16)
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn respond_json(stream: &mut TcpStream, status: u16, v: Value) {
    respond(stream, status, "application/json", v.to_string().as_bytes());
}

fn error_json(code: u16, message: &str, ty: &str) -> Value {
    json!({ "error": { "code": code, "message": message, "type": ty } })
}

fn handle(mut stream: TcpStream, state: Arc<Mutex<State>>) {
    let Some(req) = read_request(&mut stream) else { return };
    let (api_key, reply, load_delay, fail_load) = {
        let s = state.lock().unwrap();
        (s.cfg.api_key.clone(), s.cfg.reply.clone(), s.cfg.load_delay, s.cfg.fail_load.clone())
    };
    if req.path == "/health" || req.path == "/v1/health" {
        respond_json(&mut stream, 200, json!({ "status": "ok" }));
        return;
    }
    if let Some(key) = &api_key {
        let ok = req.headers.get("authorization").map(|v| v == &format!("Bearer {key}")).unwrap_or(false);
        if !ok {
            respond_json(&mut stream, 401, error_json(401, "Invalid API Key", "authentication_error"));
            return;
        }
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/models") | ("GET", "/v1/models") => {
            let mut s = state.lock().unwrap();
            if req.query.contains_key("reload") {
                if let Some(p) = s.cfg.preset_path.clone() {
                    let wanted = models_from_preset(&p);
                    s.models.retain(|(id, _)| wanted.contains(id));
                    for m in wanted {
                        if !s.models.iter().any(|(id, _)| *id == m) {
                            s.models.push((m, ModelState::unloaded()));
                        }
                    }
                    s.broadcast("models_reload", "*", Value::Null);
                }
            }
            let data: Vec<Value> = s.models.iter().map(|(id, m)| s.model_json(id, m)).collect();
            respond_json(&mut stream, 200, json!({ "object": "list", "data": data }));
        }
        ("POST", "/models/load") => {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let Some(id) = body.get("model").and_then(Value::as_str).map(str::to_string) else {
                respond_json(
                    &mut stream,
                    400,
                    error_json(400, "model must be a non-empty string", "invalid_request_error"),
                );
                return;
            };
            let running = {
                let s = state.lock().unwrap();
                s.models
                    .iter()
                    .find(|(m, _)| *m == id)
                    .map(|(_, m)| ["loading", "loaded", "sleeping"].contains(&m.status.as_str()))
            };
            if running == Some(true) {
                // llama.cpp router: HTTP 400 while the instance is loading / loaded / sleeping
                respond_json(&mut stream, 400, error_json(400, "model is already running", "invalid_request_error"));
                return;
            }
            let known = running.is_some();
            if !known {
                respond_json(
                    &mut stream,
                    400,
                    error_json(400, &format!("model '{id}' not found"), "invalid_request_error"),
                );
                return;
            }
            {
                let mut s = state.lock().unwrap();
                if let Some((_, m)) = s.models.iter_mut().find(|(m, _)| *m == id) {
                    m.status = "loading".into();
                    m.failed = false;
                    m.progress = Some(
                        json!({ "stages": ["text_model", "mmproj_model"], "current": "text_model", "value": 0.0 }),
                    );
                }
                s.broadcast("model_status", &id, json!({ "status": "loading", "progress": { "stages": ["text_model", "mmproj_model"], "current": "text_model", "value": 0.0 } }));
            }
            respond_json(&mut stream, 200, json!({ "success": true }));
            let st = state.clone();
            let fails = fail_load.contains(&id);
            std::thread::spawn(move || {
                std::thread::sleep(load_delay / 2);
                {
                    let mut s = st.lock().unwrap();
                    if let Some((_, m)) = s.models.iter_mut().find(|(m, _)| *m == id) {
                        if m.status != "loading" {
                            return;
                        }
                        m.progress = Some(
                            json!({ "stages": ["text_model", "mmproj_model"], "current": "mmproj_model", "value": 0.5 }),
                        );
                    }
                    s.broadcast("model_status", &id, json!({ "status": "loading", "progress": { "stages": ["text_model", "mmproj_model"], "current": "mmproj_model", "value": 0.5 } }));
                }
                std::thread::sleep(load_delay / 2);
                let mut s = st.lock().unwrap();
                if let Some((_, m)) = s.models.iter_mut().find(|(m, _)| *m == id) {
                    if m.status != "loading" {
                        return;
                    }
                    m.progress = None;
                    if fails {
                        m.status = "unloaded".into();
                        m.failed = true;
                        m.exit_code = Some(1);
                    } else {
                        m.status = "loaded".into();
                    }
                }
                if fails {
                    s.broadcast("model_status", &id, json!({ "status": "unloaded", "failed": true, "exit_code": 1 }));
                } else {
                    s.broadcast("model_status", &id, json!({ "status": "loaded", "info": {} }));
                    s.broadcast("model_memory", &id, json!({ "status": "measured", "effective": { "n_ctx": 32768, "devices": [ { "kind": "device", "name": "CUDA0", "model": 3000000000u64, "context": 900000000u64, "compute": 400000000u64 } ] } }));
                }
            });
        }
        ("POST", "/models/unload") => {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let id = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
            let mut s = state.lock().unwrap();
            let Some((_, m)) = s.models.iter_mut().find(|(m, _)| *m == id) else {
                respond_json(&mut stream, 400, error_json(400, "model not found", "invalid_request_error"));
                return;
            };
            if m.status == "unloaded" {
                respond_json(&mut stream, 400, error_json(400, "model is not running", "invalid_request_error"));
                return;
            }
            m.status = "unloaded".into();
            m.progress = None;
            s.broadcast("model_status", &id, json!({ "status": "unloaded" }));
            respond_json(&mut stream, 200, json!({ "success": true }));
        }
        ("GET", "/models/sse") => {
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n";
            if stream.write_all(head.as_bytes()).and_then(|_| stream.flush()).is_ok() {
                state.lock().unwrap().sse.push(stream);
            }
        }
        ("POST", "/v1/chat/completions") => {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let model = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
            {
                let mut s = state.lock().unwrap();
                let Some((_, m)) = s.models.iter_mut().find(|(m, _)| *m == model) else {
                    respond_json(
                        &mut stream,
                        400,
                        error_json(400, &format!("model '{model}' not found"), "invalid_request_error"),
                    );
                    return;
                };
                if m.status != "loaded" {
                    // router default: autoload on request
                    m.status = "loaded".into();
                    m.failed = false;
                    s.broadcast("model_status", &model, json!({ "status": "loaded" }));
                }
            }
            let has_image = body
                .get("messages")
                .and_then(Value::as_array)
                .map(|ms| {
                    ms.iter().any(|m| {
                        m.get("content").and_then(Value::as_array).is_some_and(|parts| {
                            parts.iter().any(|p| p.get("type").and_then(Value::as_str) == Some("image_url"))
                        })
                    })
                })
                .unwrap_or(false);
            let json_mode = body.get("response_format").is_some();
            let content = if json_mode {
                json!({ "answer": reply, "image": has_image }).to_string()
            } else if has_image {
                format!("{reply} (image received)")
            } else {
                reply.clone()
            };
            let usage = json!({ "prompt_tokens": 12, "completion_tokens": 7, "total_tokens": 19 });
            let timings = json!({ "prompt_ms": 10.0, "predicted_ms": 20.0, "predicted_per_second": 350.0 });
            let stream_mode = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
            if stream_mode {
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(head.as_bytes());
                let words: Vec<&str> = content.split_inclusive(' ').collect();
                for (i, w) in words.iter().enumerate() {
                    let chunk = json!({ "id": "chatcmpl-fake", "object": "chat.completion.chunk", "model": model, "choices": [ { "index": 0, "delta": { "content": w }, "finish_reason": if i + 1 == words.len() { json!("stop") } else { Value::Null } } ] });
                    let _ = stream.write_all(format!("data: {chunk}\n\n").as_bytes());
                    let _ = stream.flush();
                    std::thread::sleep(Duration::from_millis(2));
                }
                let last = json!({ "id": "chatcmpl-fake", "object": "chat.completion.chunk", "model": model, "choices": [], "usage": usage, "timings": timings });
                let _ = stream.write_all(format!("data: {last}\n\ndata: [DONE]\n\n").as_bytes());
                let _ = stream.flush();
            } else {
                respond_json(
                    &mut stream,
                    200,
                    json!({
                        "id": "chatcmpl-fake", "object": "chat.completion", "model": model,
                        "choices": [ { "index": 0, "message": { "role": "assistant", "content": content }, "finish_reason": "stop" } ],
                        "usage": usage, "timings": timings
                    }),
                );
            }
        }
        ("POST", "/v1/embeddings") => {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let inputs: Vec<String> = match body.get("input") {
                Some(Value::String(s)) => vec![s.clone()],
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                _ => Vec::new(),
            };
            let data: Vec<Value> = inputs
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let v: Vec<f32> = (0..8).map(|k| ((s.len() + k) as f32 * 0.01) % 1.0).collect();
                    json!({ "object": "embedding", "index": i, "embedding": v })
                })
                .collect();
            respond_json(
                &mut stream,
                200,
                json!({ "object": "list", "data": data, "model": body.get("model").cloned().unwrap_or(Value::Null) }),
            );
        }
        ("GET", "/props") => {
            respond_json(
                &mut stream,
                200,
                json!({ "sleeping": false, "model_alias": req.query.get("model").cloned().unwrap_or_default(), "fake": true }),
            );
        }
        ("GET", "/metrics") => {
            respond(
                &mut stream,
                200,
                "text/plain; version=0.0.4",
                b"# HELP llamacpp:prompt_tokens_total fake\nllamacpp:prompt_tokens_total 12\n",
            );
        }
        _ => respond_json(&mut stream, 404, error_json(404, "not found", "not_found_error")),
    }
}

/// Turns the current process into a fake `llama-server`. Understands
/// `--host`, `--port`, `--models-preset`, `--version`, `--list-devices`, the
/// `LLAMA_API_KEY` environment variable and, for tests:
/// `DOCULENS_FAKE_LOAD_DELAY_MS`, `DOCULENS_FAKE_CRASH_AFTER_MS` (exit with
/// code 3 after the delay), `DOCULENS_FAKE_CRASH_ONCE_MARKER` (crash only
/// when the marker file does not exist yet; it is created on the first run)
/// and `DOCULENS_FAKE_FAIL_START_AFTER_MARKER` (every run after the first
/// exits with code 2 before listening).
/// Returns the process exit code.
pub fn run_from_args(args: &[String]) -> i32 {
    let mut cfg = FakeRouterConfig::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--version" => {
                println!("version: 99999 (fake)\nbuilt with fake-cc for test");
                return 0;
            }
            "--list-devices" => {
                println!("Available devices:\n  CUDA0: Fake GPU 8 GB (8192 MiB, 7126 MiB free)");
                return 0;
            }
            "--host" => {
                cfg.host = args.get(i + 1).cloned().unwrap_or_else(|| "127.0.0.1".into());
                i += 1;
            }
            "--port" => {
                cfg.port = args.get(i + 1).and_then(|p| p.parse().ok()).unwrap_or(0);
                i += 1;
            }
            "--models-preset" => {
                cfg.preset_path = args.get(i + 1).map(std::path::PathBuf::from);
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    cfg.api_key = std::env::var("LLAMA_API_KEY").ok().filter(|k| !k.is_empty());
    if let Some(ms) = std::env::var("DOCULENS_FAKE_LOAD_DELAY_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
        cfg.load_delay = Duration::from_millis(ms);
    }
    if let Ok(marker) = std::env::var("DOCULENS_FAKE_FAIL_START_AFTER_MARKER") {
        // First run: create the marker and behave normally; later runs exit before binding.
        if Path::new(&marker).exists() {
            eprintln!("fake router: simulated start failure");
            return 2;
        }
        let _ = std::fs::write(&marker, b"started once");
    }
    let crash_after = std::env::var("DOCULENS_FAKE_CRASH_AFTER_MS").ok().and_then(|v| v.parse::<u64>().ok());
    let crash_once_marker = std::env::var("DOCULENS_FAKE_CRASH_ONCE_MARKER").ok();
    let should_crash = match (&crash_after, &crash_once_marker) {
        (Some(_), Some(marker)) => {
            if Path::new(marker).exists() {
                false
            } else {
                let _ = std::fs::write(marker, b"crashed once");
                true
            }
        }
        (Some(_), None) => true,
        _ => false,
    };
    let listener = match TcpListener::bind((cfg.host.as_str(), cfg.port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("fake router: cannot bind {}:{}: {e}", cfg.host, cfg.port);
            return 2;
        }
    };
    eprintln!("fake router listening on {}", listener.local_addr().map(|a| a.to_string()).unwrap_or_default());
    let stop = Arc::new(AtomicBool::new(false));
    if let (true, Some(ms)) = (should_crash, crash_after) {
        let stop2 = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            eprintln!("fake router: simulated crash");
            stop2.store(true, Ordering::Relaxed);
            std::process::exit(3);
        });
    }
    serve(listener, cfg, stop);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_sections_and_url_decode() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("p.ini");
        std::fs::write(&p, "version = 1\n[*]\nx = 1\n[gemma4-e4b-q6k]\nmodel = /a\n[embed]\nmodel = /b\n").unwrap();
        assert_eq!(models_from_preset(&p), vec!["gemma4-e4b-q6k".to_string(), "embed".to_string()]);
        assert_eq!(url_decode("a%2Fb+c"), "a/b c");
        assert_eq!(url_decode("plain"), "plain");
    }
}
