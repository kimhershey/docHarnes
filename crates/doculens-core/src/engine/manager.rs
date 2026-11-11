//! Sidecar lifecycle (design §3.3.1): start → wait for `/health` → router
//! running; model load/unload through the router API with status tracking
//! from `/models/sse`; crash detection with exponential back-off restarts
//! (3 attempts, each attempt may itself fail to come up) and a "degraded"
//! terminal state; graceful stop. `start`/`stop`/`restart` and the monitor's
//! relaunch are serialised by a lifecycle mutex.

use std::collections::{BTreeMap, VecDeque};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

use super::client::{ChatRequest, ChatResponse, RouterClient, RouterEvent, RouterModel};
use super::locate::EngineBinary;
use super::process::{self, EngineChild, LogLine, SpawnSpec};

const LOG_RING_CAPACITY: usize = 1_000;
/// A crash after this much uptime resets the restart counter.
const STABLE_UPTIME: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineState {
    Stopped,
    Starting,
    Running,
    Restarting {
        attempt: u32,
        delay_ms: u64,
    },
    /// Gave up restarting; the UI offers "restart engine" / a smaller profile (§3.3.1 #5).
    Degraded {
        reason: String,
    },
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModelInstanceStatus {
    pub id: String,
    /// `unloaded | loading | loaded | sleeping | …` (router vocabulary).
    pub status: String,
    pub failed: bool,
    pub exit_code: Option<i64>,
    /// 0..1 while loading.
    pub progress: Option<f32>,
    pub stage: Option<String>,
    /// Router `memory.effective` device bytes plus the mmproj file size.
    pub memory_effective_bytes: Option<u64>,
    pub memory_min_bytes: Option<u64>,
    pub mmproj_bytes: u64,
    pub input_modalities: Vec<String>,
    pub loaded_at: Option<DateTime<Utc>>,
    pub warmed_up: bool,
    /// Context size (tokens) the router reports for the instance; prompts are budgeted against it.
    pub ctx: Option<u32>,
}

impl ModelInstanceStatus {
    fn from_router(m: &RouterModel, mmproj_bytes: u64, prev: Option<&ModelInstanceStatus>) -> ModelInstanceStatus {
        let usable = is_usable_status(&m.status);
        ModelInstanceStatus {
            id: m.id.clone(),
            status: m.status.clone(),
            failed: m.failed,
            exit_code: m.exit_code,
            progress: m.progress_value(),
            stage: m.progress_stage(),
            memory_effective_bytes: m
                .effective_device_bytes()
                .map(|b| b + mmproj_bytes)
                .or_else(|| prev.and_then(|p| p.memory_effective_bytes).filter(|_| usable)),
            memory_min_bytes: m.min_device_bytes().map(|b| b + mmproj_bytes),
            ctx: m.context_size().or_else(|| prev.and_then(|p| p.ctx).filter(|_| usable)),
            mmproj_bytes,
            input_modalities: m.input_modalities.clone(),
            loaded_at: if usable { prev.and_then(|p| p.loaded_at).or_else(|| Some(Utc::now())) } else { None },
            warmed_up: usable && prev.is_some_and(|p| p.warmed_up),
        }
    }
    pub fn is_loaded(&self) -> bool {
        self.status == "loaded"
    }
    /// Loaded or sleeping: the router answers requests (waking the instance if needed).
    pub fn is_usable(&self) -> bool {
        is_usable_status(&self.status)
    }
    /// Loading, loaded or sleeping — the router rejects `/models/load` for these.
    pub fn is_running(&self) -> bool {
        is_running_status(&self.status)
    }
}

pub fn is_usable_status(status: &str) -> bool {
    status == "loaded" || status == "sleeping"
}

pub fn is_running_status(status: &str) -> bool {
    status == "loaded" || status == "sleeping" || status == "loading"
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineStatus {
    pub state: EngineState,
    pub binary: Option<EngineBinary>,
    pub pid: Option<u32>,
    pub host: String,
    pub port: Option<u16>,
    pub started_at: Option<DateTime<Utc>>,
    pub restarts: u32,
    pub last_error: Option<String>,
    pub last_exit_code: Option<i32>,
    pub models: Vec<ModelInstanceStatus>,
    pub presets_path: Option<PathBuf>,
    /// Model most recently loaded through [`EngineManager::load_model`].
    pub active_model: Option<String>,
    pub sleep_idle_seconds: u32,
}

impl Default for EngineStatus {
    fn default() -> Self {
        Self {
            state: EngineState::Stopped,
            binary: None,
            pid: None,
            host: "127.0.0.1".into(),
            port: None,
            started_at: None,
            restarts: 0,
            last_error: None,
            last_exit_code: None,
            models: Vec::new(),
            presets_path: None,
            active_model: None,
            sleep_idle_seconds: 0,
        }
    }
}

impl EngineStatus {
    pub fn is_running(&self) -> bool {
        self.state == EngineState::Running
    }
    pub fn model(&self, id: &str) -> Option<&ModelInstanceStatus> {
        self.models.iter().find(|m| m.id == id)
    }
    /// Instances that answer requests (loaded or sleeping).
    pub fn loaded_models(&self) -> impl Iterator<Item = &ModelInstanceStatus> {
        self.models.iter().filter(|m| m.is_usable())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineEvent {
    Status { status: EngineStatus },
    Log { line: LogLine },
    Model { model: ModelInstanceStatus },
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub binary: EngineBinary,
    pub presets_path: PathBuf,
    /// Working directory of the sidecar (`<AppData>/DocuLens/`).
    pub cwd: PathBuf,
    pub host: String,
    /// `None` → a free loopback port is picked at every start.
    pub port: Option<u16>,
    /// `None` → random per start (sent via `LLAMA_API_KEY`, never on the command line).
    pub api_key: Option<String>,
    pub sleep_idle_seconds: u32,
    pub models_max: u32,
    pub memory_margin_mib: u32,
    /// Expose `/slots` (prompts!) — debug builds only.
    pub expose_slots: bool,
    pub extra_args: Vec<String>,
    /// mmproj bytes per model id (the router's figures exclude them).
    pub mmproj_sizes: BTreeMap<String, u64>,
    pub health_timeout: Duration,
    pub max_restarts: u32,
    pub restart_backoff: Duration,
}

impl EngineConfig {
    pub fn new(binary: EngineBinary, presets_path: PathBuf, cwd: PathBuf) -> EngineConfig {
        EngineConfig {
            binary,
            presets_path,
            cwd,
            host: "127.0.0.1".into(),
            port: None,
            api_key: None,
            sleep_idle_seconds: 900,
            models_max: 0,
            memory_margin_mib: 1024,
            expose_slots: false,
            extra_args: Vec::new(),
            mmproj_sizes: BTreeMap::new(),
            health_timeout: Duration::from_secs(90),
            max_restarts: 3,
            restart_backoff: Duration::from_secs(1),
        }
    }

    /// Router command line (design §3.3.1 #2; the API key goes through the environment).
    pub fn args(&self, port: u16) -> Vec<String> {
        let mut a = vec![
            "--host".to_string(),
            self.host.clone(),
            "--port".to_string(),
            port.to_string(),
            "--models-preset".to_string(),
            self.presets_path.to_string_lossy().into_owned(),
            "--models-max".to_string(),
            self.models_max.to_string(),
            "--models-memory-margin".to_string(),
            self.memory_margin_mib.to_string(),
            "--sleep-idle-seconds".to_string(),
            if self.sleep_idle_seconds == 0 { "-1".to_string() } else { self.sleep_idle_seconds.to_string() },
            "--jinja".to_string(),
            "--metrics".to_string(),
        ];
        if !self.expose_slots {
            a.push("--no-slots".to_string());
        }
        a.extend(self.extra_args.iter().cloned());
        a
    }
}

struct Inner {
    cfg: Mutex<Option<EngineConfig>>,
    status: Mutex<EngineStatus>,
    child: Mutex<Option<EngineChild>>,
    client: Mutex<Option<RouterClient>>,
    subscribers: Mutex<Vec<mpsc::Sender<EngineEvent>>>,
    logs: Mutex<VecDeque<LogLine>>,
    /// Serialises start / stop / restart and the monitor's relaunch.
    lifecycle: Mutex<()>,
    generation: AtomicU64,
    stopping: AtomicBool,
    restart_attempts: AtomicU32,
}

#[derive(Clone)]
pub struct EngineManager {
    inner: Arc<Inner>,
}

impl Default for EngineManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Inner {
    fn emit(&self, ev: EngineEvent) {
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.retain(|s| s.send(ev.clone()).is_ok());
        }
    }

    fn status(&self) -> EngineStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn update_status(&self, f: impl FnOnce(&mut EngineStatus)) -> EngineStatus {
        let snapshot = {
            let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut s);
            s.clone()
        };
        self.emit(EngineEvent::Status { status: snapshot.clone() });
        snapshot
    }

    fn log(&self, line: LogLine) {
        match line.stream {
            super::process::LogStream::App => log::info!(target: "doculens_core::engine", "{}", line.text),
            _ => log::trace!(target: "doculens_core::engine::server", "{}", line.text),
        }
        if let Ok(mut ring) = self.logs.lock() {
            if ring.len() >= LOG_RING_CAPACITY {
                ring.pop_front();
            }
            ring.push_back(line.clone());
        }
        self.emit(EngineEvent::Log { line });
    }

    fn client(&self) -> Result<RouterClient> {
        self.client.lock().ok().and_then(|c| c.clone()).ok_or(CoreError::EngineNotRunning)
    }

    fn mmproj_size(&self, id: &str) -> u64 {
        self.cfg.lock().ok().and_then(|c| c.as_ref().and_then(|c| c.mmproj_sizes.get(id).copied())).unwrap_or(0)
    }

    fn apply_router_models(&self, models: &[RouterModel]) -> Vec<ModelInstanceStatus> {
        let mmproj = self.cfg.lock().ok().and_then(|c| c.as_ref().map(|c| c.mmproj_sizes.clone())).unwrap_or_default();
        self.update_status(|s| {
            let prev = std::mem::take(&mut s.models);
            s.models = models
                .iter()
                .map(|m| {
                    ModelInstanceStatus::from_router(
                        m,
                        *mmproj.get(&m.id).unwrap_or(&0),
                        prev.iter().find(|p| p.id == m.id),
                    )
                })
                .collect();
        })
        .models
    }

    fn apply_event(&self, ev: RouterEvent) {
        match ev {
            RouterEvent::ModelStatus { model, status, progress, failed } => {
                let mmproj = self.mmproj_size(&model);
                let snapshot = self.update_status(|s| {
                    let entry = match s.models.iter_mut().find(|m| m.id == model) {
                        Some(m) => m,
                        None => {
                            s.models.push(ModelInstanceStatus {
                                id: model.clone(),
                                mmproj_bytes: mmproj,
                                ..Default::default()
                            });
                            s.models.last_mut().expect("pushed")
                        }
                    };
                    entry.status = status.clone();
                    entry.failed = failed;
                    entry.progress =
                        progress.as_ref().and_then(|p| p.get("value")).and_then(|v| v.as_f64()).map(|v| v as f32);
                    entry.stage =
                        progress.as_ref().and_then(|p| p.get("current")).and_then(|v| v.as_str()).map(str::to_string);
                    if status == "loaded" {
                        entry.loaded_at.get_or_insert_with(Utc::now);
                        entry.progress = None;
                        entry.stage = None;
                    } else if status != "sleeping" {
                        entry.loaded_at = None;
                        entry.warmed_up = false;
                        if status == "unloaded" {
                            entry.memory_effective_bytes = None;
                        }
                    }
                });
                if let Some(m) = snapshot.model(&model) {
                    self.emit(EngineEvent::Model { model: m.clone() });
                }
            }
            RouterEvent::ModelMemory { model, data } => {
                let mmproj = self.mmproj_size(&model);
                let effective = data.get("effective").and_then(super::client::sum_device_bytes).map(|b| b + mmproj);
                let min = data.get("min").and_then(super::client::sum_device_bytes).map(|b| b + mmproj);
                self.update_status(|s| {
                    if let Some(m) = s.models.iter_mut().find(|m| m.id == model) {
                        if effective.is_some() {
                            m.memory_effective_bytes = effective;
                        }
                        if min.is_some() {
                            m.memory_min_bytes = min;
                        }
                    }
                });
            }
            RouterEvent::ModelRemove { model } => {
                self.update_status(|s| s.models.retain(|m| m.id != model));
            }
            RouterEvent::ModelsReload => {
                if let Ok(c) = self.client() {
                    if let Ok(models) = c.models() {
                        self.apply_router_models(&models);
                    }
                }
            }
            RouterEvent::DownloadProgress { .. } | RouterEvent::Other { .. } => {}
        }
    }

    /// Kills the child (if any) and resets the runtime state. Caller holds `lifecycle`.
    fn stop_locked(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        let child = self.child.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut c) = child {
            self.update_status(|s| s.state = EngineState::Stopping);
            let status = c.terminate(Duration::from_secs(5));
            self.log(LogLine::app(format!("engine stopped (pid {}, exit {:?})", c.pid, status.and_then(|s| s.code()))));
        }
        *self.client.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.update_status(|s| {
            s.state = EngineState::Stopped;
            s.pid = None;
            s.port = None;
            s.models.clear();
            s.active_model = None;
        });
    }
}

fn free_port(host: &str) -> Result<u16> {
    let l = TcpListener::bind((host, 0)).map_err(|e| CoreError::engine(format!("no free loopback port: {e}")))?;
    Ok(l.local_addr().map_err(|e| CoreError::engine(e.to_string()))?.port())
}

impl EngineManager {
    pub fn new() -> EngineManager {
        EngineManager {
            inner: Arc::new(Inner {
                cfg: Mutex::new(None),
                status: Mutex::new(EngineStatus::default()),
                child: Mutex::new(None),
                client: Mutex::new(None),
                subscribers: Mutex::new(Vec::new()),
                logs: Mutex::new(VecDeque::new()),
                lifecycle: Mutex::new(()),
                generation: AtomicU64::new(0),
                stopping: AtomicBool::new(false),
                restart_attempts: AtomicU32::new(0),
            }),
        }
    }

    pub fn subscribe(&self) -> mpsc::Receiver<EngineEvent> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut subs) = self.inner.subscribers.lock() {
            subs.push(tx);
        }
        rx
    }

    pub fn status(&self) -> EngineStatus {
        self.inner.status()
    }

    pub fn is_running(&self) -> bool {
        self.inner.status().is_running()
    }

    /// True while a start is in flight (another thread is waiting for `/health`).
    pub fn is_starting(&self) -> bool {
        matches!(self.inner.status().state, EngineState::Starting)
    }

    pub fn recent_logs(&self, n: usize) -> Vec<LogLine> {
        self.inner
            .logs
            .lock()
            .map(|r| r.iter().rev().take(n).cloned().collect::<Vec<_>>().into_iter().rev().collect())
            .unwrap_or_default()
    }

    pub fn client(&self) -> Result<RouterClient> {
        self.inner.client()
    }

    pub fn config(&self) -> Option<EngineConfig> {
        self.inner.cfg.lock().ok().and_then(|c| c.clone())
    }

    /// Starts the router and blocks until `/health` answers (or the health
    /// timeout elapses). Idempotent: when the engine is already running (or
    /// another start just finished while we waited for the lock) the current
    /// status is returned without restarting.
    pub fn start(&self, cfg: EngineConfig) -> Result<EngineStatus> {
        let _guard = self.inner.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        if self.inner.status().is_running() && self.inner.child.lock().map(|c| c.is_some()).unwrap_or(false) {
            return Ok(self.inner.status());
        }
        self.start_locked(cfg)
    }

    fn start_locked(&self, cfg: EngineConfig) -> Result<EngineStatus> {
        if self.inner.child.lock().map(|c| c.is_some()).unwrap_or(false) {
            self.inner.stop_locked();
        }
        self.inner.stopping.store(false, Ordering::SeqCst);
        self.inner.restart_attempts.store(0, Ordering::SeqCst);
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.inner.cfg.lock().map_err(|_| CoreError::engine("state poisoned"))? = Some(cfg.clone());
        let restarts = self.inner.status().restarts;
        self.inner.update_status(|s| {
            *s = EngineStatus {
                state: EngineState::Starting,
                binary: Some(cfg.binary.clone()),
                host: cfg.host.clone(),
                presets_path: Some(cfg.presets_path.clone()),
                sleep_idle_seconds: cfg.sleep_idle_seconds,
                restarts,
                ..EngineStatus::default()
            };
        });
        if let Err(e) = launch(&self.inner, &cfg, generation) {
            // `launch` records the failures it sees once the process exists (early exit, no
            // `/health`). When the process could not be created at all (binary blocked or missing,
            // no free port) nothing else leaves "starting", and the start button stays disabled.
            let message = e.to_string();
            self.inner.log(LogLine::app(format!("engine start failed: {message}")));
            self.inner.update_status(|s| {
                if s.state == EngineState::Starting {
                    s.state = EngineState::Stopped;
                    s.pid = None;
                    s.port = None;
                    s.last_error = Some(message.clone());
                }
            });
            return Err(e);
        }
        spawn_monitor(&self.inner, generation);
        spawn_sse(&self.inner, generation);
        Ok(self.inner.status())
    }

    /// Stops the sidecar (process group / job object) and clears the state.
    pub fn stop(&self) -> Result<()> {
        let _guard = self.inner.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        self.inner.stop_locked();
        Ok(())
    }

    /// Stop + start with the previous configuration (restart counter kept).
    pub fn restart(&self) -> Result<EngineStatus> {
        let cfg = self.config().ok_or_else(|| CoreError::engine("engine was never started"))?;
        self.restart_with(cfg)
    }

    /// Stop + start with a fresh configuration (e.g. regenerated presets), under one lock.
    pub fn restart_with(&self, cfg: EngineConfig) -> Result<EngineStatus> {
        let _guard = self.inner.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        self.inner.stop_locked();
        self.start_locked(cfg)
    }

    /// `GET /models` (or `?reload=1`) → updated instance list.
    pub fn refresh_models(&self, reload: bool) -> Result<Vec<ModelInstanceStatus>> {
        let c = self.inner.client()?;
        let models = if reload { c.reload_models()? } else { c.models()? };
        Ok(self.inner.apply_router_models(&models))
    }

    /// Loads a model and waits until the router reports it usable (loaded or
    /// sleeping), a failure, or the timeout. Optionally sends a one-token
    /// warm-up request (which also wakes a sleeping instance).
    pub fn load_model(&self, id: &str, timeout: Duration, warmup: bool) -> Result<ModelInstanceStatus> {
        self.load_model_cancellable(id, timeout, warmup, None)
    }

    /// Like [`load_model`], but aborts (unloading the half-loaded instance)
    /// when `cancel` becomes true.
    pub fn load_model_cancellable(
        &self,
        id: &str,
        timeout: Duration,
        warmup: bool,
        cancel: Option<&AtomicBool>,
    ) -> Result<ModelInstanceStatus> {
        let c = self.inner.client()?;
        let known = c.models()?;
        let Some(current) = known.iter().find(|m| m.id == id) else {
            return Err(CoreError::UnknownModel(id.to_string()));
        };
        // The router rejects `/models/load` while the model is loading, loaded or
        // sleeping ("model is already running"); e.g. a `load-on-startup` model.
        if !is_running_status(&current.status) {
            c.load(id)?;
        }
        self.inner.log(LogLine::app(format!("loading model {id}")));
        let start = Instant::now();
        loop {
            let models = c.models()?;
            self.inner.apply_router_models(&models);
            let Some(m) = models.iter().find(|m| m.id == id) else {
                return Err(CoreError::UnknownModel(id.to_string()));
            };
            if is_usable_status(&m.status) {
                break;
            }
            if m.failed && m.status == "unloaded" {
                return Err(CoreError::engine(format!("model {id} failed to load (exit code {:?})", m.exit_code)));
            }
            if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
                let _ = c.unload(id);
                self.inner.log(LogLine::app(format!("load of {id} cancelled")));
                return Err(CoreError::Cancelled);
            }
            if start.elapsed() > timeout {
                return Err(CoreError::Timeout(format!("loading {id}")));
            }
            if self.inner.stopping.load(Ordering::SeqCst) {
                return Err(CoreError::EngineNotRunning);
            }
            std::thread::sleep(Duration::from_millis(400));
        }
        if warmup {
            match c.chat(&ChatRequest::text(id, "ping").max_tokens(1)) {
                Ok(_) => {
                    self.inner.log(LogLine::app(format!("warm-up done for {id} in {} ms", start.elapsed().as_millis())))
                }
                Err(e) => self.inner.log(LogLine::app(format!("warm-up failed for {id}: {e}"))),
            }
        }
        let snapshot = self.inner.update_status(|s| {
            s.active_model = Some(id.to_string());
            if let Some(m) = s.models.iter_mut().find(|m| m.id == id) {
                m.warmed_up = warmup;
                m.loaded_at.get_or_insert_with(Utc::now);
            }
        });
        snapshot.model(id).cloned().ok_or_else(|| CoreError::engine("model vanished"))
    }

    /// Unloads a model and waits (up to 60 s) until the router no longer
    /// reports it as running — the router stops the instance asynchronously
    /// (`stop-timeout`, default 10 s, then SIGKILL).
    pub fn unload_model(&self, id: &str) -> Result<()> {
        let c = self.inner.client()?;
        c.unload(id)?;
        let start = Instant::now();
        let models = loop {
            let models = c.models()?;
            let still = models.iter().find(|m| m.id == id).is_some_and(|m| is_running_status(&m.status));
            if !still || start.elapsed() > Duration::from_secs(60) {
                break models;
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        self.inner.log(LogLine::app(format!("unloaded model {id} in {} ms", start.elapsed().as_millis())));
        self.inner.apply_router_models(&models);
        self.inner.update_status(|s| {
            if s.active_model.as_deref() == Some(id) {
                s.active_model = None;
            }
        });
        Ok(())
    }

    pub fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        self.inner.client()?.chat(req)
    }

    pub fn chat_stream(&self, req: &ChatRequest, on_delta: &mut dyn FnMut(&str) -> bool) -> Result<ChatResponse> {
        self.inner.client()?.chat_stream(req, on_delta)
    }
}

/// Spawns the process, waits for `/health` and marks the engine running.
fn launch(inner: &Arc<Inner>, cfg: &EngineConfig, generation: u64) -> Result<()> {
    let port = match cfg.port {
        Some(p) => p,
        None => free_port(&cfg.host)?,
    };
    let api_key = cfg.api_key.clone().unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    let weak = Arc::downgrade(inner);
    let sink: process::LogSink = Arc::new(move |line: LogLine| {
        if let Some(inner) = weak.upgrade() {
            inner.log(line);
        }
    });
    // The API key travels through the environment only (§3.3.1 #2); the router
    // propagates its environment to the model instances it spawns.
    let env = vec![("LLAMA_API_KEY".to_string(), api_key.clone())];
    let spec = SpawnSpec {
        binary: cfg.binary.path.clone(),
        args: cfg.args(port),
        env,
        cwd: Some(cfg.cwd.clone()),
        log_sink: Some(sink),
    };
    inner.log(LogLine::app(format!("starting {} {}", spec.binary.display(), spec.args.join(" "))));
    let mut child = process::spawn(&spec)?;
    let pid = child.pid;
    inner.update_status(|s| {
        s.state = EngineState::Starting;
        s.pid = Some(pid);
        s.port = Some(port);
        s.started_at = Some(Utc::now());
        s.last_error = None;
    });
    let client = RouterClient::new(&cfg.host, port, Some(api_key));
    let deadline = Instant::now() + cfg.health_timeout;
    loop {
        if let Some(status) = child.try_wait() {
            let code = status.code();
            let msg = format!("llama-server exited during startup (code {code:?})");
            inner.update_status(|s| {
                s.state = EngineState::Stopped;
                s.last_error = Some(msg.clone());
                s.last_exit_code = code;
                s.pid = None;
            });
            return Err(CoreError::engine(msg));
        }
        if inner.generation.load(Ordering::SeqCst) != generation || inner.stopping.load(Ordering::SeqCst) {
            let _ = child.terminate(Duration::from_secs(2));
            return Err(CoreError::EngineNotRunning);
        }
        if let Ok(true) = client.health() {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.terminate(Duration::from_secs(2));
            let msg = format!("llama-server did not answer /health within {} s", cfg.health_timeout.as_secs());
            inner.update_status(|s| {
                s.state = EngineState::Stopped;
                s.last_error = Some(msg.clone());
                s.pid = None;
            });
            return Err(CoreError::Timeout(msg));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    *inner.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
    *inner.client.lock().unwrap_or_else(|e| e.into_inner()) = Some(client.clone());
    inner.update_status(|s| s.state = EngineState::Running);
    inner.log(LogLine::app(format!("engine ready on {}:{port} (pid {pid})", cfg.host)));
    if let Ok(models) = client.models() {
        inner.apply_router_models(&models);
    }
    Ok(())
}

/// Watches the child; after a crash, relaunches with exponential back-off.
/// Every attempt — including one whose relaunch fails to come up — counts,
/// and `max_restarts` exhausted attempts end in `Degraded`.
fn spawn_monitor(inner: &Arc<Inner>, generation: u64) {
    let weak = Arc::downgrade(inner);
    let _ = std::thread::Builder::new().name("doculens-engine-monitor".into()).spawn(move || 'monitor: loop {
        std::thread::sleep(Duration::from_millis(300));
        let Some(inner) = weak.upgrade() else { break };
        if inner.stopping.load(Ordering::SeqCst) || inner.generation.load(Ordering::SeqCst) != generation {
            break;
        }
        let exited = {
            let mut guard = inner.child.lock().unwrap_or_else(|e| e.into_inner());
            guard.as_mut().and_then(|c| c.try_wait().map(|st| (st.code(), c.started_at.elapsed())))
        };
        let Some((code, uptime)) = exited else { continue };
        let Some(cfg) = inner.cfg.lock().ok().and_then(|c| c.clone()) else { break };
        inner.child.lock().unwrap_or_else(|e| e.into_inner()).take();
        *inner.client.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if uptime >= STABLE_UPTIME {
            inner.restart_attempts.store(0, Ordering::SeqCst);
        }
        inner.log(LogLine::app(format!("engine exited unexpectedly (code {code:?}) after {} s", uptime.as_secs())));
        let mut last_code = code;
        loop {
            let attempt = inner.restart_attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt > cfg.max_restarts {
                inner.update_status(|s| {
                    s.state = EngineState::Degraded {
                        reason: format!("crashed {} times; last exit code {last_code:?}", attempt - 1),
                    };
                    s.pid = None;
                    s.last_exit_code = last_code;
                    s.last_error = Some("engine_crash_loop".into());
                    s.models.clear();
                });
                break 'monitor;
            }
            let delay = cfg.restart_backoff * 2u32.pow(attempt - 1);
            inner.update_status(|s| {
                s.state = EngineState::Restarting { attempt, delay_ms: delay.as_millis() as u64 };
                s.pid = None;
                s.last_exit_code = last_code;
                s.models.clear();
            });
            std::thread::sleep(delay);
            if inner.stopping.load(Ordering::SeqCst) || inner.generation.load(Ordering::SeqCst) != generation {
                break 'monitor;
            }
            let result = {
                let _guard = inner.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
                if inner.stopping.load(Ordering::SeqCst) || inner.generation.load(Ordering::SeqCst) != generation {
                    break 'monitor;
                }
                launch(&inner, &cfg, generation)
            };
            match result {
                Ok(()) => {
                    let active = inner.status().active_model;
                    inner.update_status(|s| s.restarts += 1);
                    // Re-load the model that was active before the crash.
                    if let Some(id) = active {
                        if let Ok(c) = inner.client() {
                            let _ = c.load(&id);
                        }
                    }
                    continue 'monitor;
                }
                Err(e) => {
                    inner.log(LogLine::app(format!("restart attempt {attempt} failed: {e}")));
                    last_code = inner.status().last_exit_code;
                }
            }
        }
    });
}

/// Keeps a `/models/sse` subscription for the lifetime of this generation.
fn spawn_sse(inner: &Arc<Inner>, generation: u64) {
    let weak = Arc::downgrade(inner);
    let _ = std::thread::Builder::new().name("doculens-engine-sse".into()).spawn(move || loop {
        let Some(inner) = weak.upgrade() else { break };
        if inner.stopping.load(Ordering::SeqCst) || inner.generation.load(Ordering::SeqCst) != generation {
            break;
        }
        let client = inner.client.lock().ok().and_then(|c| c.clone());
        if let Some(c) = &client {
            // Events emitted while the subscription was down (crash → relaunch →
            // active model re-load) are reconciled from `/models` before listening.
            if let Ok(models) = c.models() {
                inner.apply_router_models(&models);
            }
        }
        drop(inner);
        if let Some(c) = client {
            let weak2 = weak.clone();
            let _ = c.sse(&mut |ev| {
                let Some(inner) = weak2.upgrade() else { return false };
                if inner.stopping.load(Ordering::SeqCst) || inner.generation.load(Ordering::SeqCst) != generation {
                    return false;
                }
                inner.apply_event(ev);
                true
            });
        }
        std::thread::sleep(Duration::from_millis(500));
    });
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(mut c) = self.child.lock() {
            if let Some(mut child) = c.take() {
                let _ = child.terminate(Duration::from_secs(3));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::client::RouterModel;
    use crate::engine::locate::EngineSource;
    use crate::hw::Backend;

    #[test]
    fn args_follow_the_design() {
        let b = EngineBinary {
            path: "/x/llama-server".into(),
            backend: Backend::Cpu,
            version: None,
            build_number: None,
            source: EngineSource::Env,
        };
        let mut cfg = EngineConfig::new(b, "/app/presets.ini".into(), "/app".into());
        cfg.sleep_idle_seconds = 900;
        let a = cfg.args(4242).join(" ");
        assert!(a.contains("--host 127.0.0.1 --port 4242 --models-preset /app/presets.ini --models-max 0 --models-memory-margin 1024 --sleep-idle-seconds 900 --jinja --metrics --no-slots"));
        assert!(!a.contains("api-key"));
        cfg.expose_slots = true;
        cfg.sleep_idle_seconds = 0;
        let a = cfg.args(1).join(" ");
        assert!(!a.contains("--no-slots"));
        assert!(a.contains("--sleep-idle-seconds -1"));
    }

    #[test]
    fn status_mapping_adds_mmproj_and_keeps_loaded_at() {
        let v = serde_json::json!({ "id": "q", "status": { "value": "loaded" }, "memory": { "effective": { "devices": [ { "kind": "device", "model": 100, "context": 10, "compute": 1 } ] } } });
        let rm = RouterModel::from_json(&v).unwrap();
        let first = ModelInstanceStatus::from_router(&rm, 50, None);
        assert_eq!(first.memory_effective_bytes, Some(161));
        assert!(first.loaded_at.is_some());
        let again = ModelInstanceStatus::from_router(&rm, 50, Some(&first));
        assert_eq!(again.loaded_at, first.loaded_at);
        let sleeping =
            RouterModel::from_json(&serde_json::json!({ "id": "q", "status": { "value": "sleeping" } })).unwrap();
        let s = ModelInstanceStatus::from_router(&sleeping, 50, Some(&first));
        assert!(s.is_usable() && !s.is_loaded() && s.is_running());
        assert_eq!(s.loaded_at, first.loaded_at);
        let unloaded =
            RouterModel::from_json(&serde_json::json!({ "id": "q", "status": { "value": "unloaded" } })).unwrap();
        let u = ModelInstanceStatus::from_router(&unloaded, 50, Some(&first));
        assert!(u.loaded_at.is_none());
        assert_eq!(u.memory_effective_bytes, None);
        assert!(is_running_status("loading") && !is_usable_status("loading"));
    }

    #[test]
    fn stopped_manager_reports_not_running() {
        let m = EngineManager::new();
        assert_eq!(m.status().state, EngineState::Stopped);
        assert_eq!(m.client().unwrap_err().code(), "engine_not_running");
        assert!(m.restart().is_err());
        assert!(m.recent_logs(10).is_empty());
        // stop on a stopped manager is a no-op
        m.stop().unwrap();
        assert_eq!(m.status().state, EngineState::Stopped);
    }

    /// PLT-14: when the sidecar cannot even be created (binary blocked by antivirus or missing, no
    /// free port) the engine must not stay "starting" — the start button is disabled in that state.
    #[test]
    fn failed_launch_leaves_the_engine_stopped_with_the_error() {
        let tmp = tempfile::tempdir().unwrap();
        let binary = EngineBinary {
            path: tmp.path().join("missing-llama-server"),
            backend: Backend::Cpu,
            version: None,
            build_number: None,
            source: EngineSource::Env,
        };
        let cfg = EngineConfig::new(binary, tmp.path().join("presets.ini"), tmp.path().to_path_buf());
        let m = EngineManager::new();
        let events = m.subscribe();
        let err = m.start(cfg.clone()).unwrap_err();
        assert!(err.to_string().contains("cannot start"), "{err}");
        let st = m.status();
        assert_eq!(st.state, EngineState::Stopped);
        assert!(!m.is_starting());
        assert!(st.last_error.as_deref().is_some_and(|e| e.contains("cannot start")), "{:?}", st.last_error);
        assert_eq!((st.pid, st.port), (None, None));
        // The webview hears about the final state, and the reason is in the engine log.
        let states: Vec<EngineState> = events
            .try_iter()
            .filter_map(|e| match e {
                EngineEvent::Status { status } => Some(status.state),
                _ => None,
            })
            .collect();
        assert_eq!(states.first(), Some(&EngineState::Starting));
        assert_eq!(states.last(), Some(&EngineState::Stopped));
        assert!(m.recent_logs(10).iter().any(|l| l.text.contains("engine start failed")));
        // Starting again is possible and fails the same clean way.
        assert!(m.start(cfg).is_err());
        assert_eq!(m.status().state, EngineState::Stopped);
    }
}
