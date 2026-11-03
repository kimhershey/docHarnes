//! `llama-server --list-devices` (design §3.3.3): the free VRAM the engine
//! itself sees, which is what the tier table is defined on.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::hw::{GpuInfo, GpuSource, GpuVendor};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineDevice {
    /// Backend device name, e.g. `CUDA0`, `Vulkan0`.
    pub name: String,
    pub description: String,
    pub total_mib: u64,
    pub free_mib: u64,
}

/// Parses lines such as `  CUDA0: NVIDIA GeForce RTX 4060 (8188 MiB, 7126 MiB free)`.
pub fn parse_list_devices(text: &str) -> Vec<EngineDevice> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("Available devices") || line == "(none)" {
            continue;
        }
        let Some((name, rest)) = line.split_once(": ") else { continue };
        let Some((description, tail)) = rest.rsplit_once(" (") else { continue };
        let tail = tail.trim_end_matches(')');
        let mut nums = tail.split(',').map(|s| s.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()));
        let (Some(Some(total)), Some(Some(free))) = (nums.next(), nums.next()) else { continue };
        if !tail.contains("MiB") {
            continue;
        }
        out.push(EngineDevice {
            name: name.trim().to_string(),
            description: description.trim().to_string(),
            total_mib: total,
            free_mib: free,
        });
    }
    out
}

/// Runs the binary with `--list-devices` (bounded by `timeout`).
pub fn list_devices(binary: &Path, timeout: Duration) -> Result<Vec<EngineDevice>> {
    let mut child = Command::new(binary)
        .arg("--list-devices")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CoreError::engine(format!("cannot run {}: {e}", binary.display())))?;
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CoreError::Timeout("llama-server --list-devices".into()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(CoreError::engine(format!("--list-devices: {e}"))),
        }
    }
    let text = reader.join().unwrap_or_default();
    Ok(parse_list_devices(&text))
}

/// Engine devices as [`GpuInfo`] rows (`source = engine`).
pub fn to_gpu_infos(devices: &[EngineDevice]) -> Vec<GpuInfo> {
    devices
        .iter()
        .enumerate()
        .map(|(i, d)| GpuInfo {
            index: i as u32,
            name: d.description.clone(),
            vendor: if d.name.starts_with("CUDA") {
                GpuVendor::Nvidia
            } else {
                GpuVendor::guess_from_name(&d.description)
            },
            total_mib: d.total_mib,
            free_mib: Some(d.free_mib),
            driver_version: None,
            cuda_driver_version: None,
            compute_capability: None,
            source: GpuSource::Engine,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_engine_output() {
        let text = "ggml_cuda_init: found 1 CUDA devices:\nAvailable devices:\n  CUDA0: NVIDIA GeForce RTX 4060 (8188 MiB, 7126 MiB free)\n  Vulkan0: Intel(R) Arc(TM) A770 Graphics (16256 MiB, 15000 MiB free)\n";
        let d = parse_list_devices(text);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].name, "CUDA0");
        assert_eq!(d[0].description, "NVIDIA GeForce RTX 4060");
        assert_eq!(d[0].total_mib, 8188);
        assert_eq!(d[0].free_mib, 7126);
        assert_eq!(d[1].description, "Intel(R) Arc(TM) A770 Graphics");
        assert_eq!(parse_list_devices("Available devices:\n  (none)\n"), vec![]);
        let g = to_gpu_infos(&d);
        assert_eq!(g[0].vendor, GpuVendor::Nvidia);
        assert_eq!(g[1].vendor, GpuVendor::Intel);
        assert_eq!(g[0].source, GpuSource::Engine);
    }
}
