//! Debounced recursive directory watcher shared by the drop-in model folder
//! (`modelstore`) and the workspace `inbox/` (`documents`). Events are
//! filtered to content changes, optionally by path, and coalesced: the
//! callback fires once the folder has been quiet for `settle`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::error::{CoreError, Result};

pub struct DirWatcher {
    _watcher: notify::RecommendedWatcher,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub root: PathBuf,
}

impl std::fmt::Debug for DirWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirWatcher").field("root", &self.root).finish()
    }
}

impl DirWatcher {
    /// Starts watching `root` recursively. `relevant(path)` decides whether a
    /// changed path should wake the callback; `on_change` runs on the watcher
    /// thread after `settle` of quiet (so a batch of copies yields one call).
    pub fn start(
        root: &Path,
        settle: Duration,
        thread_name: &str,
        relevant: impl Fn(&Path) -> bool + Send + 'static,
        on_change: impl Fn() + Send + 'static,
    ) -> Result<DirWatcher> {
        use notify::{RecursiveMode, Watcher};
        let (tx, rx) = mpsc::channel::<()>();
        let mut watcher = notify::recommended_watcher(move |res: std::result::Result<notify::Event, notify::Error>| {
            if let Ok(ev) = res {
                if !is_content_change(&ev.kind) {
                    return; // opens / reads / metadata-only changes (e.g. the engine mmapping a model)
                }
                if ev.paths.iter().any(|p| relevant(p)) {
                    let _ = tx.send(());
                }
            }
        })
        .map_err(|e| CoreError::engine(format!("watcher: {e}")))?;
        std::fs::create_dir_all(root).map_err(|e| CoreError::io(root, e))?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| CoreError::engine(format!("watch {}: {e}", root.display())))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name(thread_name.to_string())
            .spawn(move || {
                let mut pending = false;
                let mut last_event = Instant::now();
                while !stop2.load(Ordering::Relaxed) {
                    match rx.recv_timeout(Duration::from_millis(250)) {
                        Ok(()) => {
                            pending = true;
                            last_event = Instant::now();
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    if pending && last_event.elapsed() >= settle {
                        pending = false;
                        on_change();
                    }
                }
            })
            .map_err(|e| CoreError::engine(format!("watcher thread: {e}")))?;
        Ok(DirWatcher { _watcher: watcher, stop, thread: Some(thread), root: root.to_path_buf() })
    }
}

impl Drop for DirWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Create / data-modify / rename / remove events change what a scan would see;
/// access, open/close and metadata-only events do not (notify's inotify backend
/// subscribes to `OPEN` and `ATTRIB` as well, which the engine triggers while
/// loading a model).
pub fn is_content_change(kind: &notify::EventKind) -> bool {
    use notify::event::{EventKind, ModifyKind};
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any => true,
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_) | ModifyKind::Any) => true,
        EventKind::Modify(ModifyKind::Metadata(_) | ModifyKind::Other) => false,
        EventKind::Access(_) | EventKind::Other => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn coalesces_a_burst_into_one_callback() {
        let dir = tempfile::tempdir().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = hits.clone();
        let w = DirWatcher::start(
            dir.path(),
            Duration::from_millis(300),
            "test-watch",
            |p| p.extension().is_some_and(|e| e == "txt"),
            move || {
                hits2.fetch_add(1, Ordering::SeqCst);
            },
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), b"x").unwrap();
            std::fs::write(dir.path().join(format!("ignored{i}.tmp")), b"x").unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while hits.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(500));
        let n = hits.load(Ordering::SeqCst);
        assert!((1..=2).contains(&n), "expected one coalesced callback, got {n}");
        drop(w);
    }
}
