//! Spawning the sidecar so that nothing survives the app (design §3.3.1 #6):
//! Windows → job object with `KILL_ON_JOB_CLOSE` (the router's model child
//! processes inherit it); Unix → own process group and `killpg` on stop /
//! drop / app exit. stdout/stderr are pumped line by line to a sink.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
    App,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub at: DateTime<Utc>,
    pub stream: LogStream,
    pub text: String,
}

impl LogLine {
    pub fn app(text: impl Into<String>) -> LogLine {
        LogLine { at: Utc::now(), stream: LogStream::App, text: text.into() }
    }
}

pub type LogSink = Arc<dyn Fn(LogLine) + Send + Sync>;

/// `Command` for an engine binary with the platform defaults applied: no
/// console window on Windows (the app is a GUI subsystem binary), no stdin.
pub fn engine_command(binary: &std::path::Path) -> Command {
    let mut cmd = Command::new(binary);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    cmd
}

/// Output of a short-lived engine invocation (`--version`, `--list-devices`).
#[derive(Debug, Default)]
pub struct CapturedOutput {
    pub status: Option<ExitStatus>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a short-lived command, draining both pipes (backend init logs can
/// exceed the pipe buffer), and kills it after `timeout`.
pub fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<CapturedOutput> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CoreError::engine(format!("cannot run {:?}: {e}", cmd.get_program())))?;
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut p) = pipe {
                let mut buf = Vec::new();
                let _ = p.read_to_end(&mut buf);
                s = String::from_utf8_lossy(&buf).into_owned();
            }
            s
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(CoreError::engine(format!("wait: {e}"))),
        }
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if status.is_none() {
        return Err(CoreError::Timeout(format!("{:?} did not exit within {} s", cmd.get_program(), timeout.as_secs())));
    }
    Ok(CapturedOutput { status, stdout, stderr })
}

#[derive(Clone)]
pub struct SpawnSpec {
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    pub log_sink: Option<LogSink>,
}

pub struct EngineChild {
    child: Child,
    pub pid: u32,
    pub started_at: Instant,
    #[cfg(windows)]
    job: Option<job::JobHandle>,
    readers: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for EngineChild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineChild").field("pid", &self.pid).finish()
    }
}

/// Spawns the process with piped output and the platform containment.
pub fn spawn(spec: &SpawnSpec) -> Result<EngineChild> {
    let mut cmd = engine_command(&spec.binary);
    cmd.args(&spec.args).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    #[cfg(unix)]
    {
        // Own process group so the whole tree (router + model instances) can be
        // signalled with killpg. PR_SET_PDEATHSIG is deliberately NOT used: on
        // Linux it fires when the spawning *thread* exits, and the engine is
        // started from short-lived worker threads.
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    let job = job::JobHandle::kill_on_close().map_err(|e| CoreError::engine(format!("job object: {e}"))).ok();

    let mut child =
        cmd.spawn().map_err(|e| CoreError::engine(format!("cannot start {}: {e}", spec.binary.display())))?;
    let pid = child.id();
    #[cfg(windows)]
    if let Some(j) = &job {
        if let Err(e) = j.assign(&child) {
            log::warn!("cannot assign llama-server to the job object: {e}");
        }
    }

    let mut readers = Vec::new();
    if let Some(sink) = &spec.log_sink {
        if let Some(out) = child.stdout.take() {
            readers.push(pump(out, LogStream::Stdout, sink.clone()));
        }
        if let Some(err) = child.stderr.take() {
            readers.push(pump(err, LogStream::Stderr, sink.clone()));
        }
    } else {
        // Drain to avoid blocking the child on a full pipe.
        if let Some(out) = child.stdout.take() {
            readers.push(pump(out, LogStream::Stdout, Arc::new(|_| {})));
        }
        if let Some(err) = child.stderr.take() {
            readers.push(pump(err, LogStream::Stderr, Arc::new(|_| {})));
        }
    }
    Ok(EngineChild {
        child,
        pid,
        started_at: Instant::now(),
        #[cfg(windows)]
        job,
        readers,
    })
}

fn pump<R: std::io::Read + Send + 'static>(reader: R, stream: LogStream, sink: LogSink) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("doculens-engine-{stream:?}").to_lowercase())
        .spawn(move || {
            let buf = BufReader::new(reader);
            for line in buf.split(b'\n') {
                match line {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).trim_end_matches('\r').to_string();
                        sink(LogLine { at: Utc::now(), stream, text });
                    }
                    Err(_) => break,
                }
            }
        })
        .expect("spawn log pump")
}

impl EngineChild {
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    pub fn wait_timeout(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let start = Instant::now();
        loop {
            if let Some(s) = self.try_wait() {
                return Some(s);
            }
            if start.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Graceful stop: SIGTERM to the process group (Unix) / `TerminateJobObject`
    /// after the grace period (Windows), then SIGKILL.
    pub fn terminate(&mut self, grace: Duration) -> Option<ExitStatus> {
        if let Some(s) = self.try_wait() {
            return Some(s);
        }
        #[cfg(unix)]
        {
            unsafe {
                libc::killpg(self.pid as i32, libc::SIGTERM);
            }
            if let Some(s) = self.wait_timeout(grace) {
                return Some(s);
            }
            unsafe {
                libc::killpg(self.pid as i32, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            // llama-server has no graceful signal on Windows; the router stops its
            // children when it dies and the job object catches the rest.
            let _ = grace;
            let _ = self.child.kill();
            if let Some(j) = &self.job {
                j.terminate();
            }
        }
        let _ = self.child.kill();
        let status = self.child.wait().ok();
        for r in self.readers.drain(..) {
            let _ = r.join();
        }
        status
    }

    pub fn is_running(&mut self) -> bool {
        self.try_wait().is_none()
    }
}

impl Drop for EngineChild {
    fn drop(&mut self) {
        if self.is_running() {
            let _ = self.terminate(Duration::from_secs(3));
        }
    }
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct JobHandle(HANDLE);
    unsafe impl Send for JobHandle {}
    unsafe impl Sync for JobHandle {}

    impl JobHandle {
        pub fn kill_on_close() -> std::io::Result<JobHandle> {
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return Err(std::io::Error::last_os_error());
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    let e = std::io::Error::last_os_error();
                    CloseHandle(h);
                    return Err(e);
                }
                Ok(JobHandle(h))
            }
        }

        pub fn assign(&self, child: &Child) -> std::io::Result<()> {
            let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for JobHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn spawns_pumps_logs_and_terminates_the_group() {
        let lines: Arc<Mutex<Vec<LogLine>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_lines = lines.clone();
        let spec = SpawnSpec {
            binary: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), "echo hello; echo oops 1>&2; sleep 30".into()],
            env: vec![("DOCULENS_TEST".into(), "1".into())],
            cwd: None,
            log_sink: Some(Arc::new(move |l| sink_lines.lock().unwrap().push(l))),
        };
        let mut child = spawn(&spec).unwrap();
        assert!(child.is_running());
        std::thread::sleep(Duration::from_millis(300));
        let status = child.terminate(Duration::from_secs(2)).unwrap();
        assert!(!status.success());
        let got = lines.lock().unwrap();
        assert!(got.iter().any(|l| l.stream == LogStream::Stdout && l.text == "hello"));
        assert!(got.iter().any(|l| l.stream == LogStream::Stderr && l.text == "oops"));
    }
}
