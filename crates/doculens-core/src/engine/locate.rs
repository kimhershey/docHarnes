//! Finding the `llama-server` binary (design §3.12): environment override
//! (development / tests), explicit setting, then the pack of the selected backend
//! from one pool — `<AppData>/engines/<version>/<pack>/` (newest build) and the
//! packs bundled next to the application for the backends AppData does not have.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::hw::{self, Backend, GpuInfo};
use crate::settings::BackendPreference;

/// Path of a `llama-server` binary to use instead of the installed packs.
pub const ENGINE_ENV: &str = "DOCULENS_LLAMA_SERVER";
/// Backend name (`cpu`, `vulkan`, `cu121`, `cu129`) of the binary from [`ENGINE_ENV`].
pub const ENGINE_BACKEND_ENV: &str = "DOCULENS_LLAMA_SERVER_BACKEND";
/// Oldest llama.cpp build the app supports (router presets, `--reasoning`, the mtmd
/// vision fixes; Gemma 4 / EmbeddingGemma 2 need the pinned b12066 or newer).
pub const MIN_ENGINE_BUILD: u32 = 11_747;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineSource {
    Env,
    Override,
    AppData,
    Bundled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineBinary {
    pub path: PathBuf,
    pub backend: Backend,
    /// Version directory name, e.g. `b12066` (unknown for env/override binaries until probed).
    pub version: Option<String>,
    pub build_number: Option<u32>,
    pub source: EngineSource,
}

impl EngineBinary {
    /// True when the build number is known (non-zero: local builds without
    /// tags report 0) and older than [`MIN_ENGINE_BUILD`].
    pub fn is_too_old(&self) -> bool {
        self.build_number.is_some_and(|b| b > 0 && b < MIN_ENGINE_BUILD)
    }
    /// Fills `version` / `build_number` by running `llama-server --version`.
    pub fn with_probed_version(mut self) -> Self {
        if self.build_number.is_none() {
            if let Some((v, n)) = probe_version(&self.path) {
                self.version = Some(v);
                self.build_number = Some(n);
            }
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPack {
    pub backend: Backend,
    pub version: String,
    pub build_number: Option<u32>,
    pub path: PathBuf,
}

pub fn binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

pub fn pack_backend(name: &str) -> Option<Backend> {
    match name.to_ascii_lowercase().as_str() {
        "cpu" => Some(Backend::Cpu),
        "vulkan" => Some(Backend::Vulkan),
        "cu121" => Some(Backend::Cu121),
        "cu129" => Some(Backend::Cu129),
        _ => None,
    }
}

/// `b12066` / `12066` / `build-12066` → 12066.
pub fn parse_build_number(version: &str) -> Option<u32> {
    let v = version.trim();
    let digits: String =
        v.trim_start_matches(|c: char| !c.is_ascii_digit()).chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    // Reject semver-looking strings such as 1.2.3 (ambiguous).
    if v.matches('.').count() >= 2 {
        return None;
    }
    digits.parse().ok()
}

/// Lists `engines/<version>/<pack>/llama-server` packs.
pub fn list_installed(engines_dir: &Path) -> Vec<InstalledPack> {
    let mut out = Vec::new();
    let Ok(versions) = std::fs::read_dir(engines_dir) else { return out };
    for v in versions.flatten() {
        if !v.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let version = v.file_name().to_string_lossy().into_owned();
        let Ok(packs) = std::fs::read_dir(v.path()) else { continue };
        for p in packs.flatten() {
            let Some(backend) = pack_backend(&p.file_name().to_string_lossy()) else { continue };
            let bin = p.path().join(binary_name());
            if bin.is_file() {
                out.push(InstalledPack {
                    backend,
                    build_number: parse_build_number(&version),
                    version: version.clone(),
                    path: bin,
                });
            }
        }
    }
    out.sort_by(|a, b| b.build_number.cmp(&a.build_number).then_with(|| b.version.cmp(&a.version)));
    out
}

pub struct LocateInputs<'a> {
    pub engines_dir: &'a Path,
    /// Directories that contain `<pack>/llama-server` (bundled resources).
    pub bundled_roots: &'a [PathBuf],
    pub override_path: Option<&'a Path>,
    pub gpu: Option<&'a GpuInfo>,
    pub preference: BackendPreference,
}

/// Packs shipped with the application: `<root>/<pack>/llama-server` under the bundled roots
/// (one per backend; the first root that has it wins).
pub fn list_bundled(roots: &[PathBuf]) -> Vec<InstalledPack> {
    let mut out: Vec<InstalledPack> = Vec::new();
    for root in roots {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        for p in rd.flatten() {
            let Some(backend) = pack_backend(&p.file_name().to_string_lossy()) else { continue };
            let bin = p.path().join(binary_name());
            if bin.is_file() && !out.iter().any(|o| o.backend == backend) {
                out.push(InstalledPack { backend, version: "bundled".into(), build_number: None, path: bin });
            }
        }
    }
    out
}

/// Every pack the app can run, one per backend: the newest pack under
/// `<AppData>/engines/<version>/` wins, the bundled packs fill in the backends that have none
/// there. The two sources are one pool — a CUDA pack unpacked into AppData must not hide the
/// CPU / Vulkan packs that came with the installer.
pub fn available_packs(engines_dir: &Path, bundled_roots: &[PathBuf]) -> Vec<(InstalledPack, EngineSource)> {
    let mut out: Vec<(InstalledPack, EngineSource)> = Vec::new();
    let app_data = list_installed(engines_dir).into_iter().map(|p| (p, EngineSource::AppData));
    let bundled = list_bundled(bundled_roots).into_iter().map(|p| (p, EngineSource::Bundled));
    for (pack, source) in app_data.chain(bundled) {
        if !out.iter().any(|(o, _)| o.backend == pack.backend) {
            out.push((pack, source));
        }
    }
    out
}

/// The pack that would run for `backend`, if any (AppData first, then bundled).
pub fn find_pack(engines_dir: &Path, bundled_roots: &[PathBuf], backend: Backend) -> Option<InstalledPack> {
    available_packs(engines_dir, bundled_roots).into_iter().map(|(p, _)| p).find(|p| p.backend == backend)
}

/// Resolves the binary to run (see module docs for the precedence).
pub fn locate(inputs: &LocateInputs) -> Result<EngineBinary> {
    if let Some(p) = std::env::var_os(ENGINE_ENV) {
        let path = PathBuf::from(p);
        if path.is_file() {
            let backend = std::env::var(ENGINE_BACKEND_ENV).ok().and_then(|b| pack_backend(&b)).unwrap_or(Backend::Cpu);
            return Ok(EngineBinary { path, backend, version: None, build_number: None, source: EngineSource::Env });
        }
        log::warn!("{ENGINE_ENV} points to a missing file: {}", path.display());
    }
    if let Some(p) = inputs.override_path {
        if p.is_file() {
            let backend = p
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|n| pack_backend(&n.to_string_lossy()))
                .unwrap_or(Backend::Cpu);
            let version = p
                .parent()
                .and_then(|d| d.parent())
                .and_then(|d| d.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            return Ok(EngineBinary {
                path: p.to_path_buf(),
                backend,
                build_number: version.as_deref().and_then(parse_build_number),
                version,
                source: EngineSource::Override,
            });
        }
        return Err(CoreError::EngineNotFound(p.display().to_string()));
    }
    let packs = available_packs(inputs.engines_dir, inputs.bundled_roots);
    if packs.is_empty() {
        return Err(CoreError::EngineNotFound(inputs.engines_dir.display().to_string()));
    }
    let installed: Vec<Backend> = packs.iter().map(|(p, _)| p.backend).collect();
    let backend = hw::select_backend(inputs.gpu, &installed, inputs.preference);
    // No substitute when the selected backend has no pack: running another one (a CUDA binary
    // although the user chose CPU, CUDA on a driver that is too old) is what must not happen.
    let Some((pack, source)) = packs.iter().find(|(p, _)| p.backend == backend) else {
        return Err(CoreError::EngineNotFound(format!(
            "no {} pack under {} or next to the application",
            backend.as_str(),
            inputs.engines_dir.display()
        )));
    };
    Ok(EngineBinary {
        path: pack.path.clone(),
        backend: pack.backend,
        version: Some(pack.version.clone()),
        build_number: pack.build_number,
        source: *source,
    })
}

/// Runs `llama-server --version` and extracts the build number
/// (`version: 12066 (abc123)` / `version: 0.8.0 (build 12066, commit abc123)` / `build: 12066 (...)` /
/// `b12066`).
pub fn probe_version(path: &Path) -> Option<(String, u32)> {
    let mut cmd = super::process::engine_command(path);
    cmd.arg("--version");
    let out = super::process::run_with_timeout(cmd, std::time::Duration::from_secs(20)).ok()?;
    parse_version_output(&format!("{}\n{}", out.stdout, out.stderr))
}

pub fn parse_version_output(text: &str) -> Option<(String, u32)> {
    for line in text.lines() {
        let l = line.trim();
        for prefix in ["version:", "build:"] {
            if let Some(rest) = l.strip_prefix(prefix) {
                let rest = rest.trim();
                // llama.cpp ≥ 0.8 prints `version: 0.8.0 (build 12066, commit 713319e)`.
                if let Some(after) = rest.split("build ").nth(1) {
                    let digits: String = after.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(n) = digits.parse::<u32>() {
                        // 0 = untagged checkout, 1 = shallow clone (`git rev-list --count` of one commit):
                        // the build is unknown either way.
                        return (n > 1).then(|| (format!("b{n}"), n));
                    }
                }
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = digits.parse::<u32>() {
                    // 0 = built from an untagged checkout: version unknown
                    return (n > 0).then(|| (format!("b{n}"), n));
                }
            }
        }
        for tok in l.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
            if let Some(num) = tok.strip_prefix('b') {
                if num.len() >= 4 && num.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(n) = num.parse::<u32>() {
                        return Some((tok.to_string(), n));
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::{GpuSource, GpuVendor};

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"#!/bin/sh\n").unwrap();
    }

    #[test]
    fn build_numbers_and_version_output() {
        assert_eq!(parse_build_number("b12066"), Some(12066));
        assert_eq!(parse_build_number("12066"), Some(12066));
        assert_eq!(parse_build_number("1.2.3"), None);
        assert_eq!(parse_build_number("bundled"), None);
        assert_eq!(parse_version_output("version: 12066 (620d222)\nbuilt with gcc"), Some(("b12066".into(), 12066)));
        assert_eq!(parse_version_output("llama-server b11747"), Some(("b11747".into(), 11747)));
        assert_eq!(parse_version_output("nothing"), None);
        assert_eq!(parse_version_output("version: 0 (unknown)"), None);
        // llama.cpp ≥ 0.8 format; shallow-clone / untagged builds report build 1 / 0.
        assert_eq!(
            parse_version_output(
                "version: 0.8.0 (build 12066, commit 713319e)\nbuilt with MSVC 19.44 for Windows AMD64"
            ),
            Some(("b12066".into(), 12066))
        );
        // Exact output of the CUDA packs built by `cuda-packs.yml` (full-history clone of tag b12066).
        assert_eq!(
            parse_version_output(
                "version: 0.8.0-dev (build 12066, commit 713319e28)\nbuilt with MSVC 19.44.35207.1 for Windows AMD64"
            ),
            Some(("b12066".to_string(), 12066))
        );
        assert_eq!(parse_version_output("version: 0.8.0-dev (build 1, commit 713319e)"), None);
        assert_eq!(parse_version_output("version: 0.8.0-dev (build 0, commit unknown)"), None);
    }

    #[test]
    fn picks_newest_pack_for_the_selected_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let engines = tmp.path().join("engines");
        touch(&engines.join("b12000").join("cpu").join(binary_name()));
        touch(&engines.join("b12066").join("cpu").join(binary_name()));
        touch(&engines.join("b12066").join("vulkan").join(binary_name()));
        touch(&engines.join("b12050").join("cu121").join(binary_name()));
        let packs = list_installed(&engines);
        assert_eq!(packs.len(), 4);
        assert_eq!(packs[0].version, "b12066");

        let nvidia = GpuInfo {
            index: 0,
            name: "NVIDIA GeForce RTX 4060".into(),
            vendor: GpuVendor::Nvidia,
            total_mib: 8192,
            free_mib: Some(7000),
            driver_version: None,
            cuda_driver_version: Some((12, 6)),
            compute_capability: Some((8, 9)),
            source: GpuSource::Nvml,
        };
        let inputs = LocateInputs {
            engines_dir: &engines,
            bundled_roots: &[],
            override_path: None,
            gpu: Some(&nvidia),
            preference: BackendPreference::Auto,
        };
        let b = locate(&inputs).unwrap();
        assert_eq!(b.backend, Backend::Cu121);
        assert_eq!(b.version.as_deref(), Some("b12050"));
        let inputs = LocateInputs {
            engines_dir: &engines,
            bundled_roots: &[],
            override_path: None,
            gpu: Some(&nvidia),
            preference: BackendPreference::Cpu,
        };
        let b = locate(&inputs).unwrap();
        assert_eq!(b.backend, Backend::Cpu);
        assert_eq!(b.version.as_deref(), Some("b12066"));
        assert!(!b.is_too_old());
        let inputs = LocateInputs {
            engines_dir: &engines,
            bundled_roots: &[],
            override_path: None,
            gpu: None,
            preference: BackendPreference::Auto,
        };
        assert_eq!(locate(&inputs).unwrap().backend, Backend::Cpu);

        let empty = tmp.path().join("none");
        let inputs = LocateInputs {
            engines_dir: &empty,
            bundled_roots: &[],
            override_path: None,
            gpu: None,
            preference: BackendPreference::Auto,
        };
        assert_eq!(locate(&inputs).unwrap_err().code(), "engine_not_found");
        // bundled fallback
        let bundled = tmp.path().join("resources").join("engines");
        touch(&bundled.join("cpu").join(binary_name()));
        let roots = vec![bundled.clone()];
        let inputs = LocateInputs {
            engines_dir: &empty,
            bundled_roots: &roots,
            override_path: None,
            gpu: None,
            preference: BackendPreference::Auto,
        };
        assert_eq!(locate(&inputs).unwrap().source, EngineSource::Bundled);
        // override wins over packs
        let ovr = tmp.path().join("custom").join("vulkan").join(binary_name());
        touch(&ovr);
        let inputs = LocateInputs {
            engines_dir: &engines,
            bundled_roots: &[],
            override_path: Some(&ovr),
            gpu: None,
            preference: BackendPreference::Auto,
        };
        let b = locate(&inputs).unwrap();
        assert_eq!(b.source, EngineSource::Override);
        assert_eq!(b.backend, Backend::Vulkan);
    }

    fn gpu(name: &str, cuda: Option<(u32, u32)>, source: GpuSource) -> GpuInfo {
        GpuInfo {
            index: 0,
            name: name.into(),
            vendor: GpuVendor::guess_from_name(name),
            total_mib: 8192,
            free_mib: Some(7000),
            driver_version: None,
            cuda_driver_version: cuda,
            compute_capability: None,
            source,
        }
    }

    /// PLT-11: the Lite installer bundles the CPU and Vulkan packs and the guide has the user
    /// unpack a CUDA pack into AppData. The AppData pack must not hide the bundled ones.
    #[test]
    fn app_data_and_bundled_packs_are_one_pool() {
        let tmp = tempfile::tempdir().unwrap();
        let engines = tmp.path().join("appdata").join("engines");
        let bundled = tmp.path().join("install").join("engines");
        touch(&engines.join("b12066").join("cu121").join(binary_name()));
        touch(&bundled.join("cpu").join(binary_name()));
        touch(&bundled.join("vulkan").join(binary_name()));
        // The same folder listed twice (resource dir = exe dir) and a missing root are harmless.
        let roots = vec![bundled.clone(), bundled.clone(), tmp.path().join("nowhere")];
        let run = |gpu: Option<&GpuInfo>, preference| {
            locate(&LocateInputs { engines_dir: &engines, bundled_roots: &roots, override_path: None, gpu, preference })
        };
        let pool = available_packs(&engines, &roots);
        assert_eq!(pool.len(), 3, "{pool:?}");
        assert_eq!(
            find_pack(&engines, &roots, Backend::Vulkan).unwrap().path,
            bundled.join("vulkan").join(binary_name())
        );
        assert!(find_pack(&engines, &roots, Backend::Cu129).is_none());

        let nvidia = gpu("NVIDIA GeForce RTX 4060", Some((12, 6)), GpuSource::Nvml);
        let b = run(Some(&nvidia), BackendPreference::Auto).unwrap();
        assert_eq!(
            (b.backend, b.source, b.version.as_deref()),
            (Backend::Cu121, EngineSource::AppData, Some("b12066"))
        );
        // Backend set to CPU / Vulkan: the bundled pack runs, not the CUDA binary from AppData.
        let b = run(Some(&nvidia), BackendPreference::Cpu).unwrap();
        assert_eq!((b.backend, b.source), (Backend::Cpu, EngineSource::Bundled));
        assert_eq!(b.path, bundled.join("cpu").join(binary_name()));
        let b = run(Some(&nvidia), BackendPreference::Vulkan).unwrap();
        assert_eq!((b.backend, b.source), (Backend::Vulkan, EngineSource::Bundled));
        // A driver too old for the CUDA pack: bundled Vulkan instead of a CUDA start that fails.
        let old_driver = gpu("NVIDIA GeForce GTX 1660", Some((11, 8)), GpuSource::Nvml);
        assert_eq!(run(Some(&old_driver), BackendPreference::Auto).unwrap().backend, Backend::Vulkan);
        // An AMD card the Vulkan pack listed (PLT-4), and a PC without any GPU.
        let amd = gpu("AMD Radeon RX 7600", None, GpuSource::Engine);
        assert_eq!(run(Some(&amd), BackendPreference::Auto).unwrap().backend, Backend::Vulkan);
        assert_eq!(run(None, BackendPreference::Auto).unwrap().backend, Backend::Cpu);
        assert_eq!(run(None, BackendPreference::Vulkan).unwrap().backend, Backend::Vulkan);

        // The same backend in both places: the pack the user installed wins.
        touch(&engines.join("b12070").join("vulkan").join(binary_name()));
        let b = run(Some(&amd), BackendPreference::Auto).unwrap();
        assert_eq!((b.source, b.version.as_deref()), (EngineSource::AppData, Some("b12070")));
    }

    /// PLT-11: when the selected backend has no pack anywhere, nothing else is started in its place.
    #[test]
    fn a_missing_pack_is_an_error_not_another_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let engines = tmp.path().join("engines");
        touch(&engines.join("b12066").join("cu121").join(binary_name()));
        let nvidia = gpu("NVIDIA GeForce RTX 4060", Some((12, 6)), GpuSource::Nvml);
        let run = |gpu: Option<&GpuInfo>, preference| {
            locate(&LocateInputs { engines_dir: &engines, bundled_roots: &[], override_path: None, gpu, preference })
        };
        assert_eq!(run(Some(&nvidia), BackendPreference::Auto).unwrap().backend, Backend::Cu121);
        for (gpu, preference) in [
            (Some(&nvidia), BackendPreference::Cpu),
            (None, BackendPreference::Auto),
            (None, BackendPreference::Vulkan),
        ] {
            let err = run(gpu, preference).unwrap_err();
            assert_eq!(err.code(), "engine_not_found");
            assert!(err.to_string().contains("no cpu pack"), "{err}");
        }
    }
}
