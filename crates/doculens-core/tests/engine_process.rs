//! Process-level sidecar tests (design §3.3.1 #5/#6) without llama.cpp: the
//! test executable re-executes itself as a fake `llama-server`
//! (`DOCULENS_FAKE_ROUTER=1`), so start/health/load/SSE/chat/stop, crash
//! recovery with back-off and the crash-loop "degraded" state are exercised
//! with real child processes. `harness = false`: see `Cargo.toml`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use doculens_core::engine::client::RouterClient;
use doculens_core::engine::fake_router;
use doculens_core::engine::{
    ChatRequest, EngineBinary, EngineConfig, EngineEvent, EngineManager, EngineSource, EngineState,
};
use doculens_core::hw::Backend;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if std::env::var_os("DOCULENS_FAKE_ROUTER").is_some() {
        std::process::exit(fake_router::run_from_args(&args[1..]));
    }
    // Children inherit this and become fake routers.
    std::env::set_var("DOCULENS_FAKE_ROUTER", "1");
    std::env::set_var("DOCULENS_FAKE_LOAD_DELAY_MS", "150");

    let exe = std::env::current_exe().expect("current exe");
    type TestFn = fn(&Path) -> Result<(), String>;
    let tests: Vec<(&str, TestFn)> = vec![
        ("lifecycle", lifecycle),
        ("load_on_startup_and_cancel", load_on_startup_and_cancel),
        ("crash_restart", crash_restart),
        ("crash_loop_degraded", crash_loop_degraded),
        ("relaunch_failure_degrades", relaunch_failure_degrades),
    ];
    let filter = args.iter().skip(1).find(|a| !a.starts_with("--")).cloned();
    let mut failed = 0;
    for (name, f) in tests {
        if filter.as_ref().is_some_and(|flt| !name.contains(flt.as_str())) {
            continue;
        }
        std::env::remove_var("DOCULENS_FAKE_CRASH_AFTER_MS");
        std::env::remove_var("DOCULENS_FAKE_CRASH_ONCE_MARKER");
        std::env::remove_var("DOCULENS_FAKE_FAIL_START_AFTER_MARKER");
        let start = Instant::now();
        match f(&exe) {
            Ok(()) => println!("test engine_process::{name} ... ok ({} ms)", start.elapsed().as_millis()),
            Err(e) => {
                failed += 1;
                println!("test engine_process::{name} ... FAILED: {e}");
            }
        }
    }
    if failed > 0 {
        println!("engine_process: {failed} failed");
        std::process::exit(1);
    }
    println!("engine_process: all passed");
}

fn check(cond: bool, msg: impl Into<String>) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(msg.into())
    }
}

fn config(exe: &Path, dir: &Path, models: &[&str]) -> EngineConfig {
    config_with_startup(exe, dir, models, None)
}

fn config_with_startup(exe: &Path, dir: &Path, models: &[&str], load_on_startup: Option<&str>) -> EngineConfig {
    let presets = dir.join("presets.ini");
    let mut ini = String::from("version = 1\n[*]\njinja = true\n");
    for m in models {
        ini.push_str(&format!("\n[{m}]\nmodel = {}\n", dir.join(format!("{m}.gguf")).display()));
        if load_on_startup == Some(m) {
            ini.push_str("load-on-startup = true\n");
        }
    }
    std::fs::write(&presets, ini).unwrap();
    let binary = EngineBinary {
        path: exe.to_path_buf(),
        backend: Backend::Cpu,
        version: None,
        build_number: None,
        source: EngineSource::Env,
    };
    let mut cfg = EngineConfig::new(binary, presets, dir.to_path_buf());
    cfg.health_timeout = Duration::from_secs(15);
    cfg.restart_backoff = Duration::from_millis(100);
    cfg.max_restarts = 3;
    cfg.sleep_idle_seconds = 0;
    cfg
}

fn wait_for(deadline: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    pred()
}

#[cfg(target_os = "linux")]
fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}
#[cfg(not(target_os = "linux"))]
fn pid_alive(_pid: u32) -> bool {
    false
}

fn lifecycle(exe: &Path) -> Result<(), String> {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(exe, tmp.path(), &["alpha", "beta"]);
    let m = EngineManager::new();
    let events = m.subscribe();
    let st = m.start(cfg).map_err(|e| format!("start: {e}"))?;
    check(st.state == EngineState::Running, format!("state after start: {:?}", st.state))?;
    let pid = st.pid.ok_or("no pid")?;
    check(st.port.is_some(), "no port")?;
    check(
        st.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>() == vec!["alpha", "beta"],
        format!("models: {:?}", st.models),
    )?;
    check(
        m.recent_logs(50).iter().any(|l| l.text.contains("fake router listening")),
        "stderr of the child is not pumped into the log ring",
    )?;

    // API key enforced: an unauthenticated client is rejected.
    let anon = RouterClient::new("127.0.0.1", st.port.unwrap(), None);
    check(anon.models().is_err(), "router accepted a request without the API key")?;

    let loaded = m.load_model("alpha", Duration::from_secs(10), true).map_err(|e| format!("load: {e}"))?;
    check(loaded.is_loaded() && loaded.warmed_up, format!("loaded status: {loaded:?}"))?;
    check(loaded.memory_effective_bytes.is_some(), "no memory figure after load")?;
    check(m.status().active_model.as_deref() == Some("alpha"), "active model not recorded")?;

    let r = m.chat(&ChatRequest::text("alpha", "hello").max_tokens(4)).map_err(|e| format!("chat: {e}"))?;
    check(r.content.contains("fake"), format!("chat content: {}", r.content))?;
    let mut n = 0;
    let s = m
        .chat_stream(&ChatRequest::text("alpha", "stream"), &mut |_| {
            n += 1;
            true
        })
        .map_err(|e| format!("stream: {e}"))?;
    check(n >= 2 && !s.content.is_empty(), "streaming produced no deltas")?;

    // SSE events reached the manager.
    let mut saw_loaded = false;
    while let Ok(ev) = events.try_recv() {
        if let EngineEvent::Model { model } = ev {
            if model.id == "alpha" && model.is_loaded() {
                saw_loaded = true;
            }
        }
    }
    check(saw_loaded, "no model_status event observed through SSE")?;

    // Preset reload picks up a new section.
    let presets = m.config().unwrap().presets_path;
    let mut ini = std::fs::read_to_string(&presets).unwrap();
    ini.push_str(&format!("\n[gamma]\nmodel = {}\n", tmp.path().join("gamma.gguf").display()));
    std::fs::write(&presets, ini).unwrap();
    let models = m.refresh_models(true).map_err(|e| format!("reload: {e}"))?;
    check(models.iter().any(|m| m.id == "gamma"), "reload did not add gamma")?;

    m.unload_model("alpha").map_err(|e| format!("unload: {e}"))?;
    check(m.status().model("alpha").map(|m| m.status.as_str()) == Some("unloaded"), "alpha still loaded")?;
    check(m.status().active_model.is_none(), "active model should clear after unload")?;

    // Unknown model → clean error, not a hang.
    check(m.load_model("nope", Duration::from_secs(2), false).is_err(), "loading an unknown model succeeded")?;

    m.stop().map_err(|e| format!("stop: {e}"))?;
    check(m.status().state == EngineState::Stopped, "not stopped")?;
    check(wait_for(Duration::from_secs(3), || !pid_alive(pid)), format!("child {pid} still alive after stop"))?;
    check(m.client().is_err(), "client available after stop")?;
    Ok(())
}

fn load_on_startup_and_cancel(exe: &Path) -> Result<(), String> {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("DOCULENS_FAKE_LOAD_DELAY_MS", "600");
    let cfg = config_with_startup(exe, tmp.path(), &["alpha", "beta"], Some("alpha"));
    let m = EngineManager::new();
    m.start(cfg).map_err(|e| format!("start: {e}"))?;
    // alpha is already loading (load-on-startup): load_model must not POST (400) and must wait for it.
    let st =
        m.load_model("alpha", Duration::from_secs(10), true).map_err(|e| format!("load during startup load: {e}"))?;
    check(st.is_loaded() && st.warmed_up, format!("alpha not loaded: {st:?}"))?;
    // Loading again while loaded is a no-op success.
    m.load_model("alpha", Duration::from_secs(5), false).map_err(|e| format!("re-load: {e}"))?;
    // Cancelling a load unloads the half-loaded instance and reports Cancelled.
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        c2.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let err = m.load_model_cancellable("beta", Duration::from_secs(10), false, Some(&cancel)).unwrap_err();
    check(err.code() == "cancelled", format!("expected cancelled, got {err}"))?;
    let beta = wait_for(Duration::from_secs(5), || m.status().model("beta").is_some_and(|b| b.status == "unloaded"));
    check(beta, format!("beta still running after cancel: {:?}", m.status().model("beta")))?;
    // Starting an already running engine is idempotent (no second router).
    let pid = m.status().pid;
    let again = m.start(m.config().unwrap()).map_err(|e| format!("second start: {e}"))?;
    check(again.pid == pid && again.state == EngineState::Running, "start() restarted a running engine")?;
    m.stop().map_err(|e| format!("stop: {e}"))?;
    std::env::set_var("DOCULENS_FAKE_LOAD_DELAY_MS", "150");
    Ok(())
}

fn relaunch_failure_degrades(exe: &Path) -> Result<(), String> {
    let tmp = tempfile::tempdir().unwrap();
    let marker: PathBuf = tmp.path().join("started-once");
    std::env::set_var("DOCULENS_FAKE_FAIL_START_AFTER_MARKER", &marker);
    std::env::set_var("DOCULENS_FAKE_CRASH_AFTER_MS", "500");
    let mut cfg = config(exe, tmp.path(), &["alpha"]);
    cfg.max_restarts = 2;
    cfg.health_timeout = Duration::from_secs(5);
    let m = EngineManager::new();
    m.start(cfg).map_err(|e| format!("start: {e}"))?;
    // First instance crashes after 500 ms; every relaunch exits before listening →
    // each failed attempt counts and the manager ends in Degraded.
    let degraded = wait_for(Duration::from_secs(20), || matches!(m.status().state, EngineState::Degraded { .. }));
    let s = m.status();
    check(degraded, format!("expected degraded after failed relaunches, got {:?}", s.state))?;
    check(s.restarts == 0, format!("no relaunch succeeded, restarts must be 0 (got {})", s.restarts))?;
    check(
        m.recent_logs(100).iter().filter(|l| l.text.contains("restart attempt")).count() >= 2,
        "attempts not logged",
    )?;
    m.stop().map_err(|e| format!("stop: {e}"))?;
    Ok(())
}

fn crash_restart(exe: &Path) -> Result<(), String> {
    let tmp = tempfile::tempdir().unwrap();
    let marker: PathBuf = tmp.path().join("crashed-once");
    std::env::set_var("DOCULENS_FAKE_CRASH_AFTER_MS", "700");
    std::env::set_var("DOCULENS_FAKE_CRASH_ONCE_MARKER", &marker);
    let cfg = config(exe, tmp.path(), &["alpha"]);
    let m = EngineManager::new();
    let st = m.start(cfg).map_err(|e| format!("start: {e}"))?;
    let first_pid = st.pid.ok_or("no pid")?;
    m.load_model("alpha", Duration::from_secs(10), false).map_err(|e| format!("load: {e}"))?;

    // The first instance exits with code 3 after ~700 ms; the monitor restarts it.
    let ok = wait_for(Duration::from_secs(10), || {
        let s = m.status();
        s.state == EngineState::Running && s.restarts == 1 && s.pid.is_some_and(|p| p != first_pid)
    });
    let s = m.status();
    check(ok, format!("no restart observed: state {:?} restarts {} pid {:?}", s.state, s.restarts, s.pid))?;
    check(s.last_exit_code == Some(3), format!("exit code not recorded: {:?}", s.last_exit_code))?;
    check(m.recent_logs(200).iter().any(|l| l.text.contains("exited unexpectedly")), "crash not logged")?;
    // The model that was active before the crash is loaded again.
    let reloaded = wait_for(Duration::from_secs(10), || m.status().model("alpha").is_some_and(|m| m.is_loaded()));
    check(reloaded, format!("active model not re-loaded after restart: {:?}", m.status().models))?;
    // Still usable.
    m.chat(&ChatRequest::text("alpha", "after restart")).map_err(|e| format!("chat after restart: {e}"))?;
    m.stop().map_err(|e| format!("stop: {e}"))?;
    Ok(())
}

fn crash_loop_degraded(exe: &Path) -> Result<(), String> {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("DOCULENS_FAKE_CRASH_AFTER_MS", "300");
    let mut cfg = config(exe, tmp.path(), &["alpha"]);
    cfg.max_restarts = 2;
    let m = EngineManager::new();
    m.start(cfg).map_err(|e| format!("start: {e}"))?;
    let degraded = wait_for(Duration::from_secs(20), || matches!(m.status().state, EngineState::Degraded { .. }));
    let s = m.status();
    check(degraded, format!("expected degraded, got {:?} (restarts {})", s.state, s.restarts))?;
    check(s.restarts == 2, format!("expected 2 successful restarts before giving up, got {}", s.restarts))?;
    check(s.pid.is_none() && s.models.is_empty(), "degraded state still reports a pid / models")?;
    // Manual restart recovers when the binary behaves again.
    std::env::remove_var("DOCULENS_FAKE_CRASH_AFTER_MS");
    let st = m.restart().map_err(|e| format!("manual restart: {e}"))?;
    check(st.state == EngineState::Running, format!("manual restart state {:?}", st.state))?;
    m.stop().map_err(|e| format!("stop: {e}"))?;
    Ok(())
}
