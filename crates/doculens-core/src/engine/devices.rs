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

/// Whether the Vulkan loader the pack links against can be found. On Windows a program whose
/// DLL is missing does not just fail: the system shows a "vulkan-1.dll was not found" dialog — a
/// background probe must never cause that on a PC without Vulkan (VM, basic display driver).
pub fn vulkan_loader_present(vulkan_binary: &Path) -> bool {
    if cfg!(windows) {
        const LOADER: &str = "vulkan-1.dll";
        let beside = vulkan_binary.parent().is_some_and(|dir| dir.join(LOADER).is_file());
        let system = ["SystemRoot", "windir"]
            .iter()
            .filter_map(std::env::var_os)
            .any(|root| Path::new(&root).join("System32").join(LOADER).is_file());
        beside || system
    } else {
        // A missing libvulkan only makes the process exit with an error.
        true
    }
}

/// Whether a Vulkan device is integrated graphics (Intel UHD / Iris, AMD APU graphics): it shares
/// the system memory and is no reason for "auto" to leave the CPU pack — most office PCs have
/// nothing else. Taken from the backend's own flag when its init log carries one
/// (`ggml_vulkan: 0 = Intel(R) UHD Graphics 770 (Intel Corporation) | uma: 1 | fp16: 1 | …`),
/// else from the usual product names.
pub fn is_integrated_gpu(device: &EngineDevice, init_log: &str) -> bool {
    let uma_of = |line: &str| line.split("| uma:").nth(1).and_then(|rest| rest.trim_start().chars().next());
    let lines = || init_log.lines().filter(|l| l.contains("ggml_vulkan:") && l.contains("| uma:"));
    let by_name = format!("= {} (", device.description);
    let by_index = device.name.strip_prefix("Vulkan").map(|i| format!("ggml_vulkan: {i} ="));
    let flag = lines()
        .find(|l| l.contains(&by_name))
        .or_else(|| lines().find(|l| by_index.as_ref().is_some_and(|prefix| l.contains(prefix.as_str()))))
        .and_then(uma_of);
    match flag {
        Some(uma) => uma == '1',
        None => {
            let d = device.description.to_lowercase();
            [
                "uhd graphics",
                "hd graphics",
                "iris",
                "radeon(tm) graphics",
                "radeon graphics",
                "radeon vega",
                "radeon(tm) vega",
            ]
            .iter()
            .any(|s| d.contains(s))
        }
    }
}

/// The discrete GPUs in the output of a Vulkan pack's `--list-devices` (`stdout` = the device
/// list, `init_log` = what the backend logged while starting): software rasterisers and
/// integrated graphics are left out.
pub fn discrete_vulkan_gpus(stdout: &str, init_log: &str) -> Vec<GpuInfo> {
    let devices: Vec<EngineDevice> = parse_list_devices(stdout)
        .into_iter()
        .filter(|d| !is_software_device(&d.description) && !is_integrated_gpu(d, init_log))
        .collect();
    to_gpu_infos(&devices)
}

/// GPUs for a machine on which NVML reports none (AMD / Intel cards, PLT-4): the discrete GPUs
/// the Vulkan engine pack lists with `--list-devices`. An explicit "Vulkan" backend still runs
/// on integrated graphics; only the automatic choice ignores them. No Vulkan loader on the PC
/// means no Vulkan GPU, and the pack is not started at all.
pub fn probe_vulkan_gpus(vulkan_binary: &Path, timeout: Duration) -> Result<Vec<GpuInfo>> {
    if !vulkan_loader_present(vulkan_binary) {
        return Ok(Vec::new());
    }
    let mut cmd = super::process::engine_command(vulkan_binary);
    cmd.arg("--list-devices");
    let out = super::process::run_with_timeout(cmd, timeout)?;
    Ok(discrete_vulkan_gpus(&out.stdout, &format!("{}\n{}", out.stderr, out.stdout)))
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
    fn vulkan_device_lists_become_discrete_gpus() {
        use crate::hw::{select_backend, Backend};
        use crate::settings::BackendPreference;

        let lite = [Backend::Vulkan, Backend::Cpu];
        let auto = |gpus: &[GpuInfo]| select_backend(gpus.first(), &lite, BackendPreference::Auto);
        let amd = discrete_vulkan_gpus(
            "Available devices:\n  Vulkan0: AMD Radeon RX 7600 (8176 MiB, 7900 MiB free)\n",
            "ggml_vulkan: Found 1 Vulkan devices:\nggml_vulkan: 0 = AMD Radeon RX 7600 (AMD proprietary driver) | uma: 0 | fp16: 1 | bf16: 0 | warp size: 64 | shared memory: 32768 | int dot: 1 | matrix cores: none\n",
        );
        assert_eq!(amd.len(), 1);
        assert_eq!((amd[0].vendor, amd[0].free_mib, amd[0].cuda_driver_version), (GpuVendor::Amd, Some(7900), None));
        assert_eq!(auto(&amd), Backend::Vulkan);
        // No init log at all (another llama.cpp build): the product name decides.
        let arc = discrete_vulkan_gpus(
            "Available devices:\n  Vulkan0: Intel(R) Arc(TM) A770 Graphics (16256 MiB, 15000 MiB free)\n",
            "",
        );
        assert_eq!(auto(&arc), Backend::Vulkan);

        // A PC without a GPU whose Vulkan loader only offers CPU rasterisers stays on the CPU pack.
        let none = discrete_vulkan_gpus(
            "Available devices:\n  Vulkan0: llvmpipe (LLVM 19.1.1, 256 bits) (31957 MiB, 31957 MiB free)\n  Vulkan1: Microsoft Basic Render Driver (16000 MiB, 16000 MiB free)\n",
            "",
        );
        assert!(none.is_empty());
        assert_eq!(auto(&none), Backend::Cpu);
        assert!(is_software_device("SwiftShader Device (Subzero)") && !is_software_device("AMD Radeon(TM) Graphics"));

        // The usual office PC: integrated graphics only. "Auto" keeps the CPU pack (an explicit
        // "Vulkan" still runs on it — `select_backend` honours that without a detected GPU).
        let uhd_list = "Available devices:\n  Vulkan0: Intel(R) UHD Graphics 770 (16203 MiB, 15400 MiB free)\n";
        let uhd_log = "ggml_vulkan: 0 = Intel(R) UHD Graphics 770 (Intel Corporation) | uma: 1 | fp16: 1 | bf16: 0 | warp size: 32 | shared memory: 4096 | int dot: 1 | matrix cores: none\n";
        for log in [uhd_log, ""] {
            let igpu = discrete_vulkan_gpus(uhd_list, log);
            assert!(igpu.is_empty(), "{igpu:?}");
            assert_eq!(auto(&igpu), Backend::Cpu);
            assert_eq!(select_backend(igpu.first(), &lite, BackendPreference::Vulkan), Backend::Vulkan);
        }
        // A laptop with both: only the discrete card counts, whatever the order.
        let both = discrete_vulkan_gpus(
            "Available devices:\n  Vulkan0: AMD Radeon(TM) Graphics (15000 MiB, 14000 MiB free)\n  Vulkan1: AMD Radeon RX 7600M XT (8176 MiB, 7900 MiB free)\n",
            "ggml_vulkan: 0 = AMD Radeon(TM) Graphics (AMD proprietary driver) | uma: 1 | fp16: 1\nggml_vulkan: 1 = AMD Radeon RX 7600M XT (AMD proprietary driver) | uma: 0 | fp16: 1\n",
        );
        assert_eq!(both.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["AMD Radeon RX 7600M XT"]);

        // The backend's own flag wins over the name; the index is used when the names differ.
        let device = |name: &str, description: &str| EngineDevice {
            name: name.into(),
            description: description.into(),
            total_mib: 8000,
            free_mib: 7000,
        };
        assert!(!is_integrated_gpu(
            &device("Vulkan0", "Intel(R) Iris(R) Xe MAX Graphics"),
            "ggml_vulkan: 0 = Intel(R) Iris(R) Xe MAX Graphics (Intel Corporation) | uma: 0 | fp16: 1"
        ));
        assert!(is_integrated_gpu(
            &device("Vulkan1", "Some GPU"),
            "ggml_vulkan: 0 = A (x) | uma: 0 |\nggml_vulkan: 1 = Renamed (x) | uma: 1 |"
        ));
        for name in [
            "Intel(R) Iris(R) Xe Graphics",
            "Intel(R) HD Graphics 630",
            "AMD Radeon(TM) Vega 8 Graphics",
            "AMD Radeon Graphics (RADV RENOIR)",
        ] {
            assert!(is_integrated_gpu(&device("Vulkan0", name), ""), "{name}");
        }
        for name in [
            "AMD Radeon RX 580 Series",
            "Radeon RX Vega 64",
            "NVIDIA GeForce GTX 1060",
            "Intel(R) Arc(TM) B580 Graphics",
        ] {
            assert!(!is_integrated_gpu(&device("Vulkan0", name), ""), "{name}");
        }
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
        // Only Windows needs the loader check (a missing DLL raises a system dialog there).
        assert!(vulkan_loader_present(&bin));
    }
}
