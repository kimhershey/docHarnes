//! `llama-server --list-devices` (design §3.3.3): the free VRAM the engine
//! itself sees, which is what the tier table is defined on.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::Result;
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
    let mut cmd = super::process::engine_command(binary);
    cmd.arg("--list-devices");
    let out = super::process::run_with_timeout(cmd, timeout)?;
    Ok(parse_list_devices(&out.stdout))
}

/// CPU rasterisers a Vulkan loader may list (Mesa llvmpipe / lavapipe, SwiftShader, the Windows
/// basic render driver). They are not GPUs: "auto" must not leave the CPU pack for them.
pub fn is_software_device(description: &str) -> bool {
    let d = description.to_lowercase();
    ["llvmpipe", "lavapipe", "swiftshader", "software rasterizer", "basic render driver"].iter().any(|s| d.contains(s))
}

/// GPUs for a machine on which NVML reports none (AMD / Intel cards, PLT-4): what the Vulkan
/// engine pack lists with `--list-devices`, without software rasterisers.
pub fn probe_vulkan_gpus(vulkan_binary: &Path, timeout: Duration) -> Result<Vec<GpuInfo>> {
    let devices: Vec<EngineDevice> =
        list_devices(vulkan_binary, timeout)?.into_iter().filter(|d| !is_software_device(&d.description)).collect();
    Ok(to_gpu_infos(&devices))
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

    /// PLT-4: fake device lists of Vulkan packs on PCs without an NVIDIA card.
    #[test]
    fn vulkan_device_lists_become_gpus_without_software_rasterisers() {
        use crate::hw::{select_backend, Backend};
        use crate::settings::BackendPreference;

        let lite = [Backend::Vulkan, Backend::Cpu];
        let gpus = |text: &str| {
            let devices: Vec<EngineDevice> =
                parse_list_devices(text).into_iter().filter(|d| !is_software_device(&d.description)).collect();
            to_gpu_infos(&devices)
        };
        let amd = gpus("Available devices:\n  Vulkan0: AMD Radeon RX 7600 (8176 MiB, 7900 MiB free)\n");
        assert_eq!(amd.len(), 1);
        assert_eq!((amd[0].vendor, amd[0].free_mib, amd[0].cuda_driver_version), (GpuVendor::Amd, Some(7900), None));
        assert_eq!(select_backend(amd.first(), &lite, BackendPreference::Auto), Backend::Vulkan);
        let intel = gpus("Available devices:\n  Vulkan0: Intel(R) Arc(TM) A770 Graphics (16256 MiB, 15000 MiB free)\n");
        assert_eq!(select_backend(intel.first(), &lite, BackendPreference::Auto), Backend::Vulkan);
        // A PC without a GPU whose Vulkan loader only offers CPU rasterisers stays on the CPU pack.
        let none = gpus(
            "Available devices:\n  Vulkan0: llvmpipe (LLVM 19.1.1, 256 bits) (31957 MiB, 31957 MiB free)\n  Vulkan1: Microsoft Basic Render Driver (16000 MiB, 16000 MiB free)\n",
        );
        assert!(none.is_empty());
        assert_eq!(select_backend(none.first(), &lite, BackendPreference::Auto), Backend::Cpu);
        assert!(is_software_device("SwiftShader Device (Subzero)") && !is_software_device("AMD Radeon(TM) Graphics"));
    }

    /// The probe itself, against a stand-in for the Vulkan pack's `llama-server --list-devices`.
    #[cfg(unix)]
    #[test]
    fn probe_vulkan_gpus_runs_the_pack() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("llama-server");
        std::fs::write(
            &bin,
            "#!/bin/sh\n[ \"$1\" = \"--list-devices\" ] || exit 2\necho 'ggml_vulkan: Found 2 Vulkan devices:' 1>&2\necho 'Available devices:'\necho '  Vulkan0: AMD Radeon RX 7600 (8176 MiB, 7900 MiB free)'\necho '  Vulkan1: llvmpipe (LLVM 19.1.1, 256 bits) (31957 MiB, 31957 MiB free)'\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        // "Text file busy": another test thread may fork while the script is still open for writing.
        let mut attempt = probe_vulkan_gpus(&bin, Duration::from_secs(20));
        for _ in 0..20 {
            if !attempt.as_ref().is_err_and(|e| e.to_string().contains("busy")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
            attempt = probe_vulkan_gpus(&bin, Duration::from_secs(20));
        }
        let gpus = attempt.unwrap();
        assert_eq!(gpus.len(), 1);
        assert_eq!(
            (gpus[0].name.as_str(), gpus[0].total_mib, gpus[0].source),
            ("AMD Radeon RX 7600", 8176, GpuSource::Engine)
        );
        assert!(probe_vulkan_gpus(&tmp.path().join("missing"), Duration::from_secs(5)).is_err());
    }
}
