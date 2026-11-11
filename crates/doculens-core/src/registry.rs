//! Model registry (`models/registry.json`, design §3.3.4 / §2.6).
//!
//! The built-in registry is embedded at compile time; drop-in models found in
//! a workspace (`modelstore`) are merged on top with `user_added = true`. `tier_defaults` maps a
//! hardware tier to a model id or to `auto:<family-size>` which resolves via
//! the adaptive quantisation rules (decision 5).

use std::collections::BTreeMap;
use std::path::Path;

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

    /// The model "자동" runs. The tier default ([`Self::default_model_for`]) when it is installed;
    /// otherwise the closest installed model, so a small change of the free VRAM between two
    /// starts (Q6_K on disk, Q5_K_M recommended today) never selects a model that is not there.
    /// Preference among the installed ones: another quantisation of the default's adaptive group,
    /// the same family on this tier, the same family, any other vision model, a user-added text
    /// model; inside a group the largest one that fits the free VRAM, else the smallest. With no
    /// usable model installed the tier default is returned, so the UI can offer its download.
    pub fn auto_model(
        &self,
        tier: Tier,
        free_vram_mib: Option<u64>,
        installed: &dyn Fn(&ModelEntry) -> bool,
    ) -> Option<&ModelEntry> {
        let recommended = self.default_model_for(tier, free_vram_mib);
        if recommended.is_some_and(installed) {
            return recommended;
        }
        // Quantisations that stand in for each other: the adaptive group behind the tier default.
        let group: Vec<&str> = match self.tier_defaults.get(&tier).and_then(|t| t.strip_prefix("auto:")) {
            Some(key) => self.adaptive.get(key).map(|rules| rules.iter().map(|r| r.model.as_str()).collect()),
            None => recommended.and_then(|rec| {
                self.adaptive
                    .values()
                    .find(|rules| rules.iter().any(|r| r.model == rec.id))
                    .map(|rules| rules.iter().map(|r| r.model.as_str()).collect())
            }),
        }
        .unwrap_or_default();
        let family = recommended.map(|m| m.family.as_str());
        let free = free_vram_mib.unwrap_or(0);
        let distance = |m: &ModelEntry| -> u8 {
            if m.role != ModelRole::Vlm {
                4
            } else if group.contains(&m.id.as_str()) {
                0
            } else if family == Some(m.family.as_str()) && m.requirements.tiers.contains(&tier) {
                1
            } else if family == Some(m.family.as_str()) {
                2
            } else {
                3
            }
        };
        self.models
            .iter()
            .filter(|m| (m.role == ModelRole::Vlm || (m.user_added && m.role == ModelRole::Llm)) && installed(m))
            .min_by_key(|m| {
                let need = m.requirements.min_free_vram_mib;
                let fits = need <= free;
                (distance(m), !fits, if fits { u64::MAX - need } else { need }, m.id.as_str())
            })
            .or(recommended)
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
        assert!(r.models.len() >= 7);
        assert!(r.lint().is_empty(), "{:?}", r.lint());
        assert!(r.get("gemma4-e4b-q6k").unwrap().has_vision());
        assert_eq!(r.get("embed-gemma2-q8").unwrap().role, ModelRole::Embedding);
        // 0.4.0: no Chinese-origin models, every file ≤ 8 GB, Apache-2.0 only (user decision 2026-11-10)
        for m in &r.models {
            assert!(!m.id.contains("qwen") && !m.id.contains("bge"), "{}", m.id);
            assert_eq!(m.license, "Apache-2.0", "{}", m.id);
            assert!(m.files.iter().all(|f| f.size <= 8_000_000_000), "{}", m.id);
        }
    }

    #[test]
    fn adaptive_rule_for_8gb_tier() {
        let r = Registry::builtin();
        assert_eq!(r.default_model_for(Tier::Vram8gb, Some(7_126)).unwrap().id, "gemma4-e4b-q6k");
        assert_eq!(r.default_model_for(Tier::Vram8gb, Some(6_200)).unwrap().id, "gemma4-e4b-q5km");
        assert_eq!(r.default_model_for(Tier::Vram8gb, Some(5_700)).unwrap().id, "gemma4-e4b-q4km");
        assert_eq!(r.default_model_for(Tier::Vram8gb, None).unwrap().id, "gemma4-e4b-q4km");
        assert_eq!(r.default_model_for(Tier::CpuOnly, None).unwrap().id, "gemma4-e2b-q4km");
        assert_eq!(r.default_model_for(Tier::Vram6gb, Some(5_000)).unwrap().id, "gemma4-e2b-q8");
        assert_eq!(r.default_model_for(Tier::Vram24gbPlus, Some(23_000)).unwrap().id, "gemma4-12b-q4km");
        assert_eq!(r.resolve("embed-gemma2-q8", None).unwrap().family, "embeddinggemma-2");
        assert!(r.resolve("auto:nope", None).is_none());
    }

    /// PLT-3: "자동" on fake inventories of installed models.
    #[test]
    fn auto_prefers_an_installed_model_over_an_uninstalled_recommendation() {
        let r = Registry::builtin();
        let auto = |tier: Tier, free: Option<u64>, on_disk: &[&str]| {
            r.auto_model(tier, free, &|m: &ModelEntry| on_disk.contains(&m.id.as_str())).map(|m| m.id.clone())
        };
        let some = |id: &str| Some(id.to_string());
        // The recommendation itself when it is installed, and when nothing is (→ download offer).
        assert_eq!(auto(Tier::Vram8gb, Some(7_126), &["gemma4-e4b-q6k"]), some("gemma4-e4b-q6k"));
        assert_eq!(auto(Tier::Vram8gb, Some(6_200), &[]), some("gemma4-e4b-q5km"));
        // Q6_K was downloaded at 7,126 MiB free; today 7,000 MiB are free and Q5_K_M is recommended.
        assert_eq!(r.default_model_for(Tier::Vram8gb, Some(7_000)).unwrap().id, "gemma4-e4b-q5km");
        assert_eq!(auto(Tier::Vram8gb, Some(7_000), &["gemma4-e4b-q6k"]), some("gemma4-e4b-q6k"));
        // Two quantisations of the group on disk: the largest that fits the free VRAM.
        let both = ["gemma4-e4b-q4km", "gemma4-e4b-q6k"];
        assert_eq!(auto(Tier::Vram8gb, Some(7_000), &both), some("gemma4-e4b-q4km"));
        assert_eq!(auto(Tier::Vram8gb, Some(6_200), &["gemma4-e4b-q4km", "gemma4-e2b-q8"]), some("gemma4-e4b-q4km"));
        // Nothing of the group: the same family on this tier, then the same family at all.
        assert_eq!(auto(Tier::Vram8gb, Some(7_126), &["gemma4-e2b-q4km", "gemma4-12b-q4km"]), some("gemma4-e2b-q4km"));
        assert_eq!(auto(Tier::Vram8gb, Some(7_126), &["gemma4-12b-q4km"]), some("gemma4-12b-q4km"));
        // A reading that says "CPU only" (no GPU known) still runs what is installed.
        assert_eq!(r.default_model_for(Tier::CpuOnly, None).unwrap().id, "gemma4-e2b-q4km");
        assert_eq!(auto(Tier::CpuOnly, None, &["gemma4-e4b-q6k"]), some("gemma4-e4b-q6k"));
        // The embedding model is never a chat model.
        assert_eq!(auto(Tier::Vram8gb, Some(7_126), &["embed-gemma2-q8"]), some("gemma4-e4b-q6k"));

        // Drop-in models: a vision model before a text-only one, both only when no built-in one is there.
        let mut merged = r.clone();
        let mut vlm = r.get("gemma4-e2b-q4km").unwrap().clone();
        vlm.id = "my-vlm".into();
        vlm.family = "qwen35".into();
        vlm.user_added = true;
        let mut llm = vlm.clone();
        llm.id = "my-llm".into();
        llm.role = ModelRole::Llm;
        merged.models.extend([llm, vlm]);
        let auto_merged = |on_disk: &[&str]| {
            merged
                .auto_model(Tier::Vram8gb, Some(7_126), &|m: &ModelEntry| on_disk.contains(&m.id.as_str()))
                .map(|m| m.id.clone())
        };
        assert_eq!(auto_merged(&["my-llm", "my-vlm"]), some("my-vlm"));
        assert_eq!(auto_merged(&["my-llm"]), some("my-llm"));
        assert_eq!(auto_merged(&["my-llm", "my-vlm", "gemma4-12b-q4km"]), some("gemma4-12b-q4km"));
    }

    #[test]
    fn required_files_text_lists_every_file() {
        let r = Registry::builtin();
        let m = r.get("gemma4-e4b-q6k").unwrap();
        let t = required_files_text(m, Path::new("C:\\Work\\.doculens\\models\\gemma4-e4b-q6k"));
        assert!(t.contains("gemma-4-E4B-it-Q6_K.gguf"));
        assert!(t.contains("mmproj-F16.gguf"));
        // PKG-6: the address names a repository commit, never the moving `main` branch
        assert!(t.contains(
            "https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF/resolve/bfc15c382204943c3a8fff0c750b94ae2364d7a3/mmproj-F16.gguf"
        ));
        assert!(!t.contains("/resolve/main/"));
        assert!(!t.contains("not pinned yet"));
    }

    /// PKG-6: every built-in file carries an exact size, a lowercase SHA-256 (the form `gguf::sha256_file`
    /// produces — `modelstore::match_registry` compares it case-sensitively) and a commit address.
    #[test]
    fn builtin_files_are_pinned_to_a_commit_and_a_hash() {
        let r = Registry::builtin();
        for m in &r.models {
            assert!(m.verified, "{}", m.id);
            for f in &m.files {
                let sha = f.sha256.as_deref().unwrap_or_else(|| panic!("{}: {} has no sha256", m.id, f.name));
                assert!(
                    sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                    "{}: {sha}",
                    m.id
                );
                assert!(f.size > 0, "{}: {}", m.id, f.name);
                let url = f.download_url().expect("download url");
                let repo = f.hf.as_deref().expect("hf repository");
                let rest = url
                    .strip_prefix(&format!("https://huggingface.co/{repo}/resolve/"))
                    .unwrap_or_else(|| panic!("{}: unexpected address {url}", m.id));
                let (revision, name) = rest.split_once('/').expect("revision/name");
                assert!(
                    revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()),
                    "{}: {url} is not pinned to a commit",
                    m.id
                );
                assert_eq!(name, f.name, "{}", m.id);
            }
        }
    }
}
