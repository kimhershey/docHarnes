//! Sleep prevention during long batches (design §3.9: `SetThreadExecutionState`,
//! opt-in through `notifications.prevent_sleep_during_jobs`).
//!
//! `ES_CONTINUOUS` requirements are per thread, so a dedicated thread owns the
//! state: it polls the job book every few seconds and asserts / releases the
//! requirement when engine-bound persistent jobs start / stop. On non-Windows
//! targets the thread only tracks the flag (no-op).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::jobs::{JobKind, JobState};
use crate::state::AppState;

static BUSY: AtomicBool = AtomicBool::new(false);
const POLL: Duration = Duration::from_secs(5);

/// Starts the watcher thread (once, at setup).
pub fn install(app: &AppHandle) {
    let app = app.clone();
    std::thread::Builder::new()
        .name("doculens-power".into())
        .spawn(move || loop {
            std::thread::sleep(POLL);
            let state = app.state::<AppState>();
            let enabled = state.settings.lock().map(|s| s.notifications.prevent_sleep_during_jobs).unwrap_or(false);
            let busy = enabled
                && state.jobs.list().iter().any(|j| {
                    j.state == JobState::Running
                        && matches!(
                            j.kind,
                            JobKind::Pipeline
                                | JobKind::Plan
                                | JobKind::Draft
                                | JobKind::KnowledgeEmbed
                                | JobKind::ModelDownload
                        )
                });
            if busy != BUSY.swap(busy, Ordering::Relaxed) {
                apply(busy);
            }
        })
        .ok();
}

// No explicit release at exit: the `ES_CONTINUOUS` state is per thread, so a call from the
// main thread could not clear what the watcher thread asserted; the system drops it with the
// process.

#[cfg(windows)]
fn apply(busy: bool) {
    use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
    // SAFETY: plain Win32 call with constant flags; no pointers involved.
    let flags = if busy { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
    let prev = unsafe { SetThreadExecutionState(flags) };
    if prev == 0 {
        log::warn!("power: SetThreadExecutionState failed");
    } else {
        log::info!("power: sleep prevention {}", if busy { "on" } else { "off" });
    }
}

#[cfg(not(windows))]
fn apply(busy: bool) {
    log::info!("power: sleep prevention {} (no-op on this platform)", if busy { "on" } else { "off" });
}
