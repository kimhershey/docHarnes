//! Hardware probe and tier assessment (design §2.6, §3.3.3).
//!
//! Tier thresholds use the engine-reported *free* VRAM without subtraction;
//! the 1 GiB headroom is applied only in the pre-load memory estimate.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::memory::MIB;
use crate::settings::{ModelSettings, TierMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tier {
    #[serde(rename = "cpu_only")]
    CpuOnly,
    #[serde(rename = "vram_6gb")]
    Vram6gb,
    #[serde(rename = "vram_8gb")]
    Vram8gb,
    #[serde(rename = "vram_12gb")]
    Vram12gb,
    #[serde(rename = "vram_16gb")]
    Vram16gb,
    #[serde(rename = "vram_24gb_plus")]
    Vram24gbPlus,
}

impl Tier {
    pub const ALL: [Tier; 6] =
        [Tier::CpuOnly, Tier::Vram6gb, Tier::Vram8gb, Tier::Vram12gb, Tier::Vram16gb, Tier::Vram24gbPlus];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::CpuOnly => "cpu_only",
            Tier::Vram6gb => "vram_6gb",
            Tier::Vram8gb => "vram_8gb",
            Tier::Vram12gb => "vram_12gb",
            Tier::Vram16gb => "vram_16gb",
            Tier::Vram24gbPlus => "vram_24gb_plus",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        Tier::ALL.iter().copied().find(|t| t.as_str() == s.trim())
    }

    /// Tier from free VRAM in MiB (design §2.6): <3.5 GiB cpu_only; 3.5–5.5 6 GB;
    /// 5.5–9.5 8 GB; 9.5–13.5 12 GB; 13.5–20 16 GB; >20 24 GB+.
    pub fn from_free_vram_mib(free_mib: Option<u64>) -> Tier {
        let Some(free) = free_mib else { return Tier::CpuOnly };
        match free {
            f if f < 3_584 => Tier::CpuOnly,
            f if f < 5_632 => Tier::Vram6gb,
            f if f < 9_728 => Tier::Vram8gb,
            f if f < 13_824 => Tier::Vram12gb,
            f if f <= 20_480 => Tier::Vram16gb,
            _ => Tier::Vram24gbPlus,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Other,
}

impl GpuVendor {
    pub fn guess_from_name(name: &str) -> GpuVendor {
        let n = name.to_lowercase();
        if n.contains("nvidia") || n.contains("geforce") || n.contains("rtx") || n.contains("quadro") {
            GpuVendor::Nvidia
        } else if n.contains("amd") || n.contains("radeon") {
            GpuVendor::Amd
        } else if n.contains("intel") || n.contains("arc") {
            GpuVendor::Intel
        } else if n.contains("apple") {
            GpuVendor::Apple
        } else {
            GpuVendor::Other
        }
    }
}

/// Where a GPU reading came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuSource {
    Nvml,
    /// `llama-server --list-devices` (phase 1).
    Engine,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuInfo {
    pub index: u32,
    pub name: String,
    pub vendor: GpuVendor,
    pub total_mib: u64,
    pub free_mib: Option<u64>,
    pub driver_version: Option<String>,
    /// CUDA version supported by the installed driver (major, minor), from NVML.
    pub cuda_driver_version: Option<(u32, u32)>,
    pub compute_capability: Option<(u32, u32)>,
    pub source: GpuSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareSummary {
    pub os: String,
    pub os_version: String,
    pub hostname: String,
    pub cpu_name: String,
    pub cpu_cores_physical: Option<u32>,
    pub cpu_cores_logical: u32,
    pub ram_total_mib: u64,
    pub ram_available_mib: u64,
    pub gpus: Vec<GpuInfo>,
    /// Whether the NVML library could be loaded (false on non-NVIDIA machines).
    pub nvml_available: bool,
    pub probed_at: DateTime<Utc>,
}

impl HardwareSummary {
    /// GPU with the most free VRAM (falls back to most total VRAM).
    pub fn primary_gpu(&self) -> Option<&GpuInfo> {
        self.gpus.iter().max_by_key(|g| (g.free_mib.unwrap_or(0), g.total_mib))
    }
}

/// Probes CPU/RAM via sysinfo and NVIDIA GPUs via NVML when available.
pub fn probe() -> HardwareSummary {
    use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

    let sys = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()).with_cpu(CpuRefreshKind::everything()),
    );
    let cpu_name = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".to_string());

    let (gpus, nvml_available) = probe_nvml();

    HardwareSummary {
        os: System::name().unwrap_or_else(|| std::env::consts::OS.to_string()),
        os_version: System::long_os_version().or_else(System::os_version).unwrap_or_default(),
        hostname: System::host_name().unwrap_or_else(|| "localhost".to_string()),
        cpu_name,
        cpu_cores_physical: System::physical_core_count().map(|c| c as u32),
        cpu_cores_logical: sys.cpus().len() as u32,
        ram_total_mib: sys.total_memory() / MIB,
        ram_available_mib: sys.available_memory() / MIB,
        gpus,
        nvml_available,
        probed_at: Utc::now(),
    }
}

#[cfg(feature = "nvml")]
fn probe_nvml() -> (Vec<GpuInfo>, bool) {
    let nvml = match nvml_wrapper::Nvml::init() {
        Ok(n) => n,
        Err(e) => {
            log::debug!("NVML unavailable: {e}");
            return (Vec::new(), false);
        }
    };
    let driver_version = nvml.sys_driver_version().ok();
    let cuda_driver_version = nvml.sys_cuda_driver_version().ok().map(|v| {
        let v = v.max(0) as u32;
        (v / 1000, (v % 1000) / 10)
    });
    let count = nvml.device_count().unwrap_or(0);
    let mut gpus = Vec::new();
    for index in 0..count {
        let Ok(dev) = nvml.device_by_index(index) else { continue };
        let name = dev.name().unwrap_or_else(|_| format!("NVIDIA GPU {index}"));
        let mem = dev.memory_info().ok();
        let cc = dev.cuda_compute_capability().ok().map(|c| (c.major as u32, c.minor as u32));
        gpus.push(GpuInfo {
            index,
            name,
            vendor: GpuVendor::Nvidia,
            total_mib: mem.as_ref().map(|m| m.total / MIB).unwrap_or(0),
            free_mib: mem.as_ref().map(|m| m.free / MIB),
            driver_version: driver_version.clone(),
            cuda_driver_version,
            compute_capability: cc,
            source: GpuSource::Nvml,
        });
    }
    (gpus, true)
}

#[cfg(not(feature = "nvml"))]
fn probe_nvml() -> (Vec<GpuInfo>, bool) {
    (Vec::new(), false)
}

/// Inference backend (engine build) to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// llama.cpp built with CUDA 12.9 (Blackwell native; needs driver CUDA ≥ 12.8).
    Cu129,
    /// llama.cpp built with CUDA 12.1 (driver CUDA 12.1–12.7, forward compatible).
    Cu121,
    Vulkan,
    Cpu,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Cu129 => "cu129",
            Backend::Cu121 => "cu121",
            Backend::Vulkan => "vulkan",
            Backend::Cpu => "cpu",
        }
    }
    pub fn is_cuda(self) -> bool {
        matches!(self, Backend::Cu129 | Backend::Cu121)
    }
}

/// Backend selection rule (design §3.3.1 / §3.12): NVIDIA + CUDA pack
/// installed → cu129 if driver CUDA ≥ 12.8, cu121 if ≥ 12.1; any other GPU
/// → Vulkan (when a Vulkan engine is installed); otherwise CPU.
pub fn select_backend(
    gpu: Option<&GpuInfo>,
    installed: &[Backend],
    preference: crate::settings::BackendPreference,
) -> Backend {
    use crate::settings::BackendPreference as P;
    let has = |b: Backend| installed.contains(&b);
    let cuda_choice =
        gpu.filter(|g| g.vendor == GpuVendor::Nvidia).and_then(|g| g.cuda_driver_version).and_then(|(maj, min)| {
            if (maj, min) >= (12, 8) && has(Backend::Cu129) {
                Some(Backend::Cu129)
            } else if (maj, min) >= (12, 1) && has(Backend::Cu121) {
                Some(Backend::Cu121)
            } else {
                None
            }
        });
    let vulkan_choice = if gpu.is_some() && has(Backend::Vulkan) { Some(Backend::Vulkan) } else { None };

    match preference {
        P::Cpu => Backend::Cpu,
        P::Cuda => cuda_choice.or(vulkan_choice).unwrap_or(Backend::Cpu),
        P::Vulkan => vulkan_choice.or(cuda_choice).unwrap_or(Backend::Cpu),
        P::Auto => cuda_choice.or(vulkan_choice).unwrap_or(Backend::Cpu),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierBasis {
    NvmlFreeVram,
    EngineFreeVram,
    NoGpu,
    ManualOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierAssessment {
    pub tier: Tier,
    pub basis: TierBasis,
    pub free_vram_mib: Option<u64>,
    pub total_vram_mib: Option<u64>,
    pub gpu_name: Option<String>,
    pub warnings: Vec<String>,
}

/// Computes the tier from the probe and the user's auto/manual setting.
/// Warning strings are stable i18n keys (`hw.warn.*`), resolved by the UI.
pub fn assess(hw: &HardwareSummary, models: &ModelSettings) -> TierAssessment {
    let gpu = hw.primary_gpu();
    let free = gpu.and_then(|g| g.free_mib);
    let mut warnings = Vec::new();

    let (tier, basis) = match (models.tier_mode, models.manual_tier) {
        (TierMode::Manual, Some(t)) => (t, TierBasis::ManualOverride),
        _ => match gpu {
            None => (Tier::CpuOnly, TierBasis::NoGpu),
            Some(g) => (
                Tier::from_free_vram_mib(g.free_mib.or(Some(g.total_mib))),
                match g.source {
                    GpuSource::Engine => TierBasis::EngineFreeVram,
                    _ => TierBasis::NvmlFreeVram,
                },
            ),
        },
    };

    if tier == Tier::CpuOnly {
        warnings.push("hw.warn.cpu_only_slow".to_string());
    }
    if gpu.is_none() && !hw.nvml_available {
        warnings.push("hw.warn.no_nvml".to_string());
    }
    if hw.ram_total_mib < 8 * 1024 {
        warnings.push("hw.warn.low_ram".to_string());
    }
    if let Some(g) = gpu {
        if g.vendor == GpuVendor::Nvidia {
            if let Some((maj, min)) = g.cuda_driver_version {
                if (maj, min) < (12, 1) {
                    warnings.push("hw.warn.cuda_driver_too_old".to_string());
                }
            }
            if let Some((cc_major, _)) = g.compute_capability {
                if cc_major < 7 {
                    warnings.push("hw.warn.old_gpu_arch".to_string());
                }
            }
        }
    }

    TierAssessment {
        tier,
        basis,
        free_vram_mib: free,
        total_vram_mib: gpu.map(|g| g.total_mib),
        gpu_name: gpu.map(|g| g.name.clone()),
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::BackendPreference;

    fn gpu(name: &str, total: u64, free: Option<u64>, cuda: Option<(u32, u32)>) -> GpuInfo {
        GpuInfo {
            index: 0,
            name: name.into(),
            vendor: GpuVendor::guess_from_name(name),
            total_mib: total,
            free_mib: free,
            driver_version: None,
            cuda_driver_version: cuda,
            compute_capability: None,
            source: GpuSource::Nvml,
        }
    }

    #[test]
    fn tier_thresholds_match_design() {
        assert_eq!(Tier::from_free_vram_mib(None), Tier::CpuOnly);
        assert_eq!(Tier::from_free_vram_mib(Some(3_000)), Tier::CpuOnly);
        assert_eq!(Tier::from_free_vram_mib(Some(3_584)), Tier::Vram6gb);
        assert_eq!(Tier::from_free_vram_mib(Some(5_631)), Tier::Vram6gb);
        assert_eq!(Tier::from_free_vram_mib(Some(5_632)), Tier::Vram8gb);
        // RTX 3060 Ti 8 GB with 7126 MiB free (design example) -> 8 GB tier
        assert_eq!(Tier::from_free_vram_mib(Some(7_126)), Tier::Vram8gb);
        assert_eq!(Tier::from_free_vram_mib(Some(9_728)), Tier::Vram12gb);
        assert_eq!(Tier::from_free_vram_mib(Some(11_000)), Tier::Vram12gb);
        assert_eq!(Tier::from_free_vram_mib(Some(13_824)), Tier::Vram16gb);
        assert_eq!(Tier::from_free_vram_mib(Some(20_480)), Tier::Vram16gb);
        assert_eq!(Tier::from_free_vram_mib(Some(20_481)), Tier::Vram24gbPlus);
        assert_eq!(Tier::from_free_vram_mib(Some(23_000)), Tier::Vram24gbPlus);
        assert_eq!(Tier::parse("vram_8gb"), Some(Tier::Vram8gb));
        assert_eq!(serde_json::to_string(&Tier::Vram24gbPlus).unwrap(), "\"vram_24gb_plus\"");
    }

    #[test]
    fn backend_selection_follows_driver_cuda_version() {
        let all = [Backend::Cu129, Backend::Cu121, Backend::Vulkan, Backend::Cpu];
        let g = gpu("NVIDIA GeForce RTX 4060", 8192, Some(7000), Some((12, 8)));
        assert_eq!(select_backend(Some(&g), &all, BackendPreference::Auto), Backend::Cu129);
        let g = gpu("NVIDIA GeForce RTX 3060 Ti", 8192, Some(7000), Some((12, 4)));
        assert_eq!(select_backend(Some(&g), &all, BackendPreference::Auto), Backend::Cu121);
        // cu129 pack missing -> cu121 still works on newer drivers (forward compatible)
        assert_eq!(
            select_backend(
                Some(&gpu("RTX 5080", 16384, None, Some((12, 9)))),
                &[Backend::Cu121, Backend::Vulkan],
                BackendPreference::Auto
            ),
            Backend::Cu121
        );
        // driver too old -> Vulkan
        let g = gpu("NVIDIA GeForce GTX 1660", 6144, Some(5000), Some((11, 8)));
        assert_eq!(select_backend(Some(&g), &all, BackendPreference::Auto), Backend::Vulkan);
        // AMD -> Vulkan; no Vulkan pack -> CPU
        let g = gpu("AMD Radeon RX 7600", 8192, None, None);
        assert_eq!(select_backend(Some(&g), &all, BackendPreference::Auto), Backend::Vulkan);
        assert_eq!(select_backend(Some(&g), &[Backend::Cpu], BackendPreference::Auto), Backend::Cpu);
        // no GPU -> CPU; explicit CPU preference always wins
        assert_eq!(select_backend(None, &all, BackendPreference::Auto), Backend::Cpu);
        let g = gpu("NVIDIA RTX 4090", 24576, Some(20000), Some((12, 9)));
        assert_eq!(select_backend(Some(&g), &all, BackendPreference::Cpu), Backend::Cpu);
        assert_eq!(select_backend(Some(&g), &all, BackendPreference::Vulkan), Backend::Vulkan);
    }

    #[test]
    fn assessment_manual_override_and_warnings() {
        let hw = HardwareSummary {
            os: "Windows".into(),
            os_version: "11".into(),
            hostname: "pc".into(),
            cpu_name: "cpu".into(),
            cpu_cores_physical: Some(8),
            cpu_cores_logical: 16,
            ram_total_mib: 4096,
            ram_available_mib: 2048,
            gpus: vec![gpu("NVIDIA GeForce RTX 4060", 8192, Some(7000), Some((12, 8)))],
            nvml_available: true,
            probed_at: Utc::now(),
        };
        let auto = assess(&hw, &ModelSettings::default());
        assert_eq!(auto.tier, Tier::Vram8gb);
        assert_eq!(auto.basis, TierBasis::NvmlFreeVram);
        assert!(auto.warnings.contains(&"hw.warn.low_ram".to_string()));

        let manual =
            ModelSettings { tier_mode: TierMode::Manual, manual_tier: Some(Tier::Vram6gb), ..Default::default() };
        let m = assess(&hw, &manual);
        assert_eq!(m.tier, Tier::Vram6gb);
        assert_eq!(m.basis, TierBasis::ManualOverride);

        let no_gpu = HardwareSummary { gpus: vec![], nvml_available: false, ram_total_mib: 16384, ..hw.clone() };
        let a = assess(&no_gpu, &ModelSettings::default());
        assert_eq!(a.tier, Tier::CpuOnly);
        assert_eq!(a.basis, TierBasis::NoGpu);
        assert!(a.warnings.contains(&"hw.warn.cpu_only_slow".to_string()));
        assert!(a.warnings.contains(&"hw.warn.no_nvml".to_string()));
    }

    #[test]
    fn probe_runs_on_any_machine() {
        let hw = probe();
        assert!(hw.cpu_cores_logical >= 1);
        assert!(hw.ram_total_mib > 0);
    }
}
