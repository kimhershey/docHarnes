//! Real `llama-server` bootstrap for the engine-backed runs, mirroring
//! `crates/doculens-core/tests/real_engine.rs`: scan the drop-in models folder,
//! write `presets.ini`, start the router, load the model and probe the devices
//! the engine sees (`--list-devices`) before and after the load.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use doculens_core::engine::{
    devices, locate, presets, EngineBinary, EngineConfig, EngineDevice, EngineManager, EngineState, PresetContext,
};
use doculens_core::hw::{Backend, Tier};
use doculens_core::modelstore::{self, scan_models_dir, ModelHealth, ScanOptions};
use doculens_core::registry::Registry;
use doculens_core::settings::{ModelSettings, Profile};

#[derive(Debug, Clone)]
pub struct EngineOpts {
    pub binary: PathBuf,
    pub models_dir: PathBuf,
    pub model: String,
    /// `None` = decide from `--list-devices` (CUDA → cu121 preset rules, other GPU → Vulkan, none → CPU).
    pub backend: Option<Backend>,
    pub profile: Profile,
    /// Working directory for `presets.ini` and the router process (created when missing).
    pub work_dir: PathBuf,
    pub load_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub description: String,
    pub total_mib: u64,
    pub free_before_mib: u64,
    pub free_after_mib: u64,
}

/// What the results files record about the engine run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EngineInfo {
    pub binary: String,
    pub version: Option<String>,
    pub build_number: Option<u32>,
    pub backend: String,
    pub tier: String,
    pub profile: String,
    pub model: String,
    pub model_display: Option<String>,
    pub quantization: Option<String>,
    pub architecture: Option<String>,
    pub has_vision: bool,
    pub ctx: Option<u32>,
    /// Router `memory.effective` device bytes + mmproj size, when reported.
    pub model_memory_bytes: Option<u64>,
    pub mmproj_bytes: u64,
    pub devices: Vec<DeviceInfo>,
    /// Σ(free before − free after the load) over the devices the engine reports.
    pub vram_used_mib: Option<u64>,
    pub load_ms: u128,
}

pub struct RunningEngine {
    pub manager: EngineManager,
    pub info: EngineInfo,
    binary: PathBuf,
}

pub fn parse_backend(s: &str) -> Option<Backend> {
    match s.trim().to_lowercase().as_str() {
        "cpu" => Some(Backend::Cpu),
        "vulkan" => Some(Backend::Vulkan),
        "cu121" | "cuda" => Some(Backend::Cu121),
        "cu129" => Some(Backend::Cu129),
        _ => None,
    }
}

pub fn parse_profile(s: &str) -> Option<Profile> {
    match s.trim().to_lowercase().as_str() {
        "accuracy" => Some(Profile::Accuracy),
        "balanced" => Some(Profile::Balanced),
        "speed" => Some(Profile::Speed),
        "fine_print" | "fine-print" => Some(Profile::FinePrint),
        _ => None,
    }
}

pub fn profile_name(p: Profile) -> &'static str {
    doculens_core::pipeline::run::profile_name(p)
}

/// VRAM in use on the reported devices (total − free), summed.
pub fn used_mib(devs: &[EngineDevice]) -> u64 {
    devs.iter().map(|d| d.total_mib.saturating_sub(d.free_mib)).sum()
}

impl RunningEngine {
    pub fn start(opts: &EngineOpts) -> Result<RunningEngine, String> {
        if !opts.binary.is_file() {
            return Err(format!("llama-server 실행 파일이 없습니다: {}", opts.binary.display()));
        }
        if !opts.models_dir.is_dir() {
            return Err(format!("모델 폴더가 없습니다: {}", opts.models_dir.display()));
        }
        std::fs::create_dir_all(&opts.work_dir).map_err(|e| format!("{}: {e}", opts.work_dir.display()))?;

        let registry = Registry::builtin();
        let mut discovered = scan_models_dir(&opts.models_dir, &registry, ScanOptions::default());
        // Freshly checked-out / copied fixtures look like a copy in progress; they are complete.
        let copying: Vec<PathBuf> = discovered
            .iter()
            .find(|d| d.id == opts.model && d.health == ModelHealth::Copying)
            .map(|d| d.all_files().map(|f| f.path.clone()).collect())
            .unwrap_or_default();
        if !copying.is_empty() {
            for path in &copying {
                modelstore::mark_settled(path);
            }
            discovered = scan_models_dir(&opts.models_dir, &registry, ScanOptions::default());
        }
        let model = discovered.iter().find(|d| d.id == opts.model).ok_or_else(|| {
            format!(
                "모델 {}을(를) {}에서 찾지 못했습니다. 발견된 모델: {}",
                opts.model,
                opts.models_dir.display(),
                discovered.iter().map(|d| d.id.as_str()).collect::<Vec<_>>().join(", ")
            )
        })?;
        if !model.health.is_ready() {
            return Err(format!("모델 {}은(는) 로드할 수 없는 상태입니다: {:?}", model.id, model.health));
        }

        let devs_before = devices::list_devices(&opts.binary, Duration::from_secs(60)).unwrap_or_default();
        let backend = opts.backend.unwrap_or_else(|| {
            if devs_before.is_empty() {
                Backend::Cpu
            } else if devs_before.iter().any(|d| d.name.starts_with("CUDA")) {
                Backend::Cu121
            } else {
                Backend::Vulkan
            }
        });
        let tier = if backend == Backend::Cpu {
            Tier::CpuOnly
        } else {
            Tier::from_free_vram_mib(devs_before.iter().map(|d| d.free_mib).max())
        };
        let binary = EngineBinary {
            path: opts.binary.clone(),
            backend,
            version: None,
            build_number: None,
            source: locate::EngineSource::Env,
        }
        .with_probed_version();
        if binary.is_too_old() {
            return Err(format!("엔진 빌드 {:?}가 지원 최소 빌드보다 오래됐습니다", binary.build_number));
        }

        let settings = ModelSettings::default();
        let pctx = PresetContext {
            backend,
            tier,
            profile: opts.profile,
            settings: &settings,
            active_model: None,
            debug: false,
        };
        let build = presets::build_presets(&registry, &discovered, &pctx);
        if let Some((_, reason)) = build.skipped.iter().find(|(id, _)| *id == model.id) {
            return Err(format!("모델 {}의 프리셋을 만들 수 없습니다: {reason}", model.id));
        }
        let presets_path = opts.work_dir.join("presets.ini");
        build.file.write(&presets_path).map_err(|e| e.to_string())?;

        let mut cfg = EngineConfig::new(binary.clone(), presets_path, opts.work_dir.clone());
        cfg.sleep_idle_seconds = 0;
        cfg.health_timeout = Duration::from_secs(120);
        cfg.mmproj_sizes = build.mmproj_sizes.clone();
        let manager = EngineManager::new();
        let st = manager.start(cfg).map_err(|e| format!("엔진 시작 실패: {e}"))?;
        if st.state != EngineState::Running {
            return Err(format!("엔진 상태가 Running 이 아닙니다: {:?}", st.state));
        }
        let t0 = Instant::now();
        let loaded = manager.load_model(&model.id, opts.load_timeout, true).map_err(|e| {
            let tail: Vec<String> = manager.recent_logs(40).iter().map(|l| format!("  {}", l.text)).collect();
            let _ = manager.stop();
            format!("모델 로드 실패: {e}\n엔진 로그:\n{}", tail.join("\n"))
        })?;
        let load_ms = t0.elapsed().as_millis();
        let devs_after = devices::list_devices(&opts.binary, Duration::from_secs(60)).unwrap_or_default();
        let device_infos: Vec<DeviceInfo> = devs_before
            .iter()
            .map(|d| DeviceInfo {
                name: d.name.clone(),
                description: d.description.clone(),
                total_mib: d.total_mib,
                free_before_mib: d.free_mib,
                free_after_mib: devs_after.iter().find(|a| a.name == d.name).map(|a| a.free_mib).unwrap_or(d.free_mib),
            })
            .collect();
        let vram_used_mib = (!device_infos.is_empty())
            .then(|| device_infos.iter().map(|d| d.free_before_mib.saturating_sub(d.free_after_mib)).sum());
        let info = EngineInfo {
            binary: opts.binary.display().to_string(),
            version: binary.version.clone(),
            build_number: binary.build_number,
            backend: backend.as_str().to_string(),
            tier: tier.as_str().to_string(),
            profile: profile_name(opts.profile).to_string(),
            model: model.id.clone(),
            model_display: Some(model.display.clone()),
            quantization: model.quantization.clone(),
            architecture: model.architecture.clone(),
            has_vision: model.has_vision,
            ctx: loaded.ctx,
            model_memory_bytes: loaded.memory_effective_bytes,
            mmproj_bytes: loaded.mmproj_bytes,
            devices: device_infos,
            vram_used_mib,
            load_ms,
        };
        Ok(RunningEngine { manager, info, binary: opts.binary.clone() })
    }

    /// Current device report (VRAM peak sampling between documents).
    pub fn sample_devices(&self) -> Vec<EngineDevice> {
        devices::list_devices(&self.binary, Duration::from_secs(60)).unwrap_or_default()
    }
}

impl Drop for RunningEngine {
    fn drop(&mut self) {
        let _ = self.manager.stop();
    }
}
