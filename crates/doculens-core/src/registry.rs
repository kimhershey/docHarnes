//! Model registry (`models/registry.json`, design §3.3.4 / §2.6).
//!
//! The built-in registry is embedded at compile time; a workspace can add
//! drop-in models (phase 1) which are merged on top. `tier_defaults` maps a
//! hardware tier to a model id or to `auto:<family-size>` which resolves via
//! the adaptive quantisation rules (decision 5).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::hw::Tier;

pub const BUILTIN_REGISTRY_JSON: &str = include_str!("../../../models/registry.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    Vlm,
    Llm,
    Embedding,
    Reranker,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelFile {
    pub name: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub hf: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

impl ModelFile {
    pub fn is_mmproj(&self) -> bool {
        self.name.to_lowercase().starts_with("mmproj")
    }
    /// Public download URL (Hugging Face `resolve` endpoint) when known.
    pub fn download_url(&self) -> Option<String> {
        if let Some(u) = &self.url {
            return Some(u.clone());
        }
        self.hf.as_ref().map(|repo| format!("https://huggingface.co/{repo}/resolve/main/{}", self.name))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModelRequirements {
    pub min_free_vram_mib: u64,
    pub min_ram_mib: u64,
    pub tiers: Vec<Tier>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub family: String,
    pub role: ModelRole,
    pub display: String,
    pub license: String,
    #[serde(default)]
    pub license_url: Option<String>,
    #[serde(default)]
    pub source_repo: Option<String>,
    pub arch: String,
    #[serde(default)]
    pub modalities: Vec<String>,
    #[serde(default)]
    pub ctx_max: u64,
    #[serde(default)]
    pub files: Vec<ModelFile>,
    #[serde(default)]
    pub requirements: ModelRequirements,
    /// Engine defaults (ctx, kv, image tokens, sampling…) — opaque to this crate.
    #[serde(default)]
    pub defaults: serde_json::Value,
    #[serde(default)]
    pub capabilities: serde_json::Value,
    #[serde(default)]
    pub prompt_profile: Option<String>,
    /// False until sizes/hashes were pinned against the real repository.
    #[serde(default)]
    pub verified: bool,
    /// Set for drop-in models discovered in a workspace (phase 1).
    #[serde(default)]
    pub user_added: bool,
}

impl ModelEntry {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
    pub fn has_vision(&self) -> bool {
        self.modalities.iter().any(|m| m == "image")
    }
    pub fn weights_file(&self) -> Option<&ModelFile> {
        self.files.iter().find(|f| !f.is_mmproj())
    }
    pub fn mmproj_file(&self) -> Option<&ModelFile> {
        self.files.iter().find(|f| f.is_mmproj())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdaptiveRule {
    pub min_free_vram_mib: u64,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    pub schema_version: u32,
    pub models: Vec<ModelEntry>,
    #[serde(default)]
    pub tier_defaults: BTreeMap<Tier, String>,
    #[serde(default)]
    pub adaptive: BTreeMap<String, Vec<AdaptiveRule>>,
    #[serde(default)]
    pub default_embedding: Option<String>,
    #[serde(default)]
    pub fallback_family: Option<String>,
}

impl Registry {
    pub fn builtin() -> Registry {
        serde_json::from_str(BUILTIN_REGISTRY_JSON).expect("embedded registry.json is valid")
    }

    pub fn load(path: &Path) -> Result<Registry> {
        let bytes = std::fs::read(path).map_err(|e| CoreError::io(path, e))?;
        serde_json::from_slice(&bytes).map_err(|e| CoreError::json(path, e))
    }

    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn require(&self, id: &str) -> Result<&ModelEntry> {
        self.get(id).ok_or_else(|| CoreError::UnknownModel(id.to_string()))
    }

    /// Resolves the default model for a tier. `auto:<key>` entries use the
    /// adaptive rules with the given free VRAM (largest matching rule wins;
    /// without a VRAM reading the smallest variant is chosen).
    pub fn default_model_for(&self, tier: Tier, free_vram_mib: Option<u64>) -> Option<&ModelEntry> {
        let target = self.tier_defaults.get(&tier)?;
        self.resolve(target, free_vram_mib)
    }

    /// Resolves a model id or `auto:<key>` to a concrete entry.
    pub fn resolve(&self, id_or_auto: &str, free_vram_mib: Option<u64>) -> Option<&ModelEntry> {
        if let Some(key) = id_or_auto.strip_prefix("auto:") {
            let rules = self.adaptive.get(key)?;
            let free = free_vram_mib.unwrap_or(0);
            let chosen = rules
                .iter()
                .filter(|r| free >= r.min_free_vram_mib)
                .max_by_key(|r| r.min_free_vram_mib)
                .or_else(|| rules.iter().min_by_key(|r| r.min_free_vram_mib))?;
            self.get(&chosen.model)
        } else {
            self.get(id_or_auto)
        }
    }

    /// Models usable on a tier (by `requirements.tiers`), VLMs first.
    pub fn models_for_tier(&self, tier: Tier) -> Vec<&ModelEntry> {
        let mut v: Vec<&ModelEntry> = self.models.iter().filter(|m| m.requirements.tiers.contains(&tier)).collect();
        v.sort_by_key(|m| (m.role != ModelRole::Vlm, m.id.clone()));
        v
    }

    /// Checks `models/registry.json` consistency (unique ids, resolvable
    /// defaults, every model has a weights file). Returns problems as strings.
    pub fn lint(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for m in &self.models {
            if !seen.insert(m.id.as_str()) {
                problems.push(format!("duplicate model id: {}", m.id));
            }
            if m.weights_file().is_none() {
                problems.push(format!("{}: no weights file", m.id));
            }
            if m.has_vision() && m.mmproj_file().is_none() {
                problems.push(format!("{}: vision model without mmproj file", m.id));
            }
        }
        for (tier, target) in &self.tier_defaults {
            if self.resolve(target, Some(u64::MAX)).is_none() || self.resolve(target, Some(0)).is_none() {
                problems.push(format!("tier_defaults[{}] -> {target} does not resolve", tier.as_str()));
            }
        }
        for (key, rules) in &self.adaptive {
            for r in rules {
                if self.get(&r.model).is_none() {
                    problems.push(format!("adaptive[{key}] -> unknown model {}", r.model));
                }
            }
        }
        if let Some(e) = &self.default_embedding {
            if self.get(e).is_none() {
                problems.push(format!("default_embedding -> unknown model {e}"));
            }
        }
        problems
    }
}

/// Installation state of a model inside a workspace models directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallState {
    pub installed: bool,
    /// Folder that holds the files (drop-in folders may have any name).
    pub folder: Option<PathBuf>,
    pub missing_files: Vec<String>,
}

/// Phase-0 check: a model counts as installed when every registry file name
/// exists in `<models_dir>/<model id>/` or in any immediate sub-folder.
/// Phase 1 replaces name matching with GGUF-header + hash verification.
pub fn install_state(models_dir: &Path, model: &ModelEntry) -> InstallState {
    let mut candidates = vec![models_dir.join(&model.id)];
    if let Ok(rd) = std::fs::read_dir(models_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && !candidates.contains(&p) {
                candidates.push(p);
            }
        }
    }
    let mut best: Option<InstallState> = None;
    for dir in candidates {
        let missing: Vec<String> =
            model.files.iter().filter(|f| !dir.join(&f.name).is_file()).map(|f| f.name.clone()).collect();
        let state = InstallState { installed: missing.is_empty(), folder: Some(dir), missing_files: missing };
        if state.installed {
            return state;
        }
        if best.as_ref().map(|b| state.missing_files.len() < b.missing_files.len()).unwrap_or(true) {
            best = Some(state);
        }
    }
    best.unwrap_or(InstallState {
        installed: false,
        folder: None,
        missing_files: model.files.iter().map(|f| f.name.clone()).collect(),
    })
}

/// Text for "필요 파일 목록 내보내기" (design §3.3.4): file name, size, SHA-256 and URL
/// so the files can be fetched on another PC and carried over by USB.
pub fn required_files_text(model: &ModelEntry, target_folder: &Path) -> String {
    let mut out = String::new();
    out.push_str(&format!("DocuLens - required model files for {} ({})\n", model.display, model.id));
    out.push_str(&format!(
        "License: {}{}\n",
        model.license,
        model.license_url.as_deref().map(|u| format!(" ({u})")).unwrap_or_default()
    ));
    out.push_str(&format!("Copy the files into: {}\n\n", target_folder.display()));
    for f in &model.files {
        out.push_str(&format!(
            "- {}\n  size: {} bytes (~{:.2} GiB)\n  sha256: {}\n  url: {}\n",
            f.name,
            f.size,
            f.size as f64 / (1024.0 * 1024.0 * 1024.0),
            f.sha256.as_deref().unwrap_or("(not pinned yet - verify after download)"),
            f.download_url().unwrap_or_else(|| "(no public URL)".to_string())
        ));
    }
    out.push_str("\nAlso copy LICENSE / NOTICE files from the source repository into the same folder.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_parses_and_lints_clean() {
        let r = Registry::builtin();
        assert_eq!(r.schema_version, 1);
        assert!(r.models.len() >= 8);
        assert!(r.lint().is_empty(), "{:?}", r.lint());
        assert!(r.get("qwen3.5-4b-q6k").unwrap().has_vision());
        assert_eq!(r.get("embed-bge-m3-q8").unwrap().role, ModelRole::Embedding);
    }

    #[test]
    fn adaptive_rule_for_8gb_tier() {
        let r = Registry::builtin();
        assert_eq!(r.default_model_for(Tier::Vram8gb, Some(7_126)).unwrap().id, "qwen3.5-4b-q6k");
        assert_eq!(r.default_model_for(Tier::Vram8gb, Some(6_200)).unwrap().id, "qwen3.5-4b-q5km");
        assert_eq!(r.default_model_for(Tier::Vram8gb, Some(5_700)).unwrap().id, "qwen3.5-4b-q4km");
        assert_eq!(r.default_model_for(Tier::Vram8gb, None).unwrap().id, "qwen3.5-4b-q4km");
        assert_eq!(r.default_model_for(Tier::CpuOnly, None).unwrap().id, "qwen3.5-2b-q8");
        assert_eq!(r.default_model_for(Tier::Vram24gbPlus, Some(23_000)).unwrap().id, "qwen3.5-27b-q4km");
        assert_eq!(r.resolve("qwen3-vl-4b-q4km", None).unwrap().family, "qwen3-vl");
        assert!(r.resolve("auto:nope", None).is_none());
    }

    #[test]
    fn install_state_matches_files_in_any_folder() {
        let r = Registry::builtin();
        let m = r.get("qwen3.5-4b-q4km").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let s = install_state(dir.path(), m);
        assert!(!s.installed);
        assert_eq!(s.missing_files.len(), 2);

        let custom = dir.path().join("my-custom-folder");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(custom.join("Qwen3.5-4B-Q4_K_M.gguf"), b"x").unwrap();
        let s = install_state(dir.path(), m);
        assert!(!s.installed);
        assert_eq!(s.missing_files, vec!["mmproj-F16.gguf".to_string()]);
        std::fs::write(custom.join("mmproj-F16.gguf"), b"x").unwrap();
        let s = install_state(dir.path(), m);
        assert!(s.installed);
        assert_eq!(s.folder, Some(custom));
    }

    #[test]
    fn required_files_text_lists_every_file() {
        let r = Registry::builtin();
        let m = r.get("qwen3.5-4b-q6k").unwrap();
        let t = required_files_text(m, Path::new("C:\\Work\\.doculens\\models\\qwen3.5-4b-q6k"));
        assert!(t.contains("Qwen3.5-4B-Q6_K.gguf"));
        assert!(t.contains("mmproj-F16.gguf"));
        assert!(t.contains("https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/main/mmproj-F16.gguf"));
    }
}
