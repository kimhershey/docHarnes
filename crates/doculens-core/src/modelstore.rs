//! Drop-in model folders (design §3.3.5), GGUF verification (§3.3.4),
//! registry matching, offline import, resumable download and the folder
//! watcher. Everything here is synchronous; the shell runs long operations
//! (hashing, copying, downloading) on worker threads and forwards progress.
//!
//! Folder rule: `<models_dir>/<any name>/` contains `*.gguf` (one weights
//! file or a `-0000N-of-0000M` split set), optionally `mmproj*.gguf`,
//! optionally `model.json` and LICENSE / NOTICE files.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::gguf::{self, FileHashes, GgufKind, ModelMeta};
use crate::hw::Tier;
use crate::memory::MIB;
use crate::registry::{ModelEntry, ModelFile, ModelRequirements, ModelRole, Registry};

pub const MODEL_JSON: &str = "model.json";
pub const HASH_CACHE_FILE: &str = ".doculens-hashes.json";
/// Files modified more recently than this are treated as still being copied.
pub const COPY_SETTLE_SECONDS: u64 = 5;

/// `general.architecture` → prompt profile for architectures the pipeline
/// knows how to drive. Anything else is registered as "unsupported".
pub const SUPPORTED_ARCHS: [(&str, &str); 12] = [
    ("qwen35", "qwen3.5-doc-v1"),
    ("qwen35moe", "qwen3.5-doc-v1"),
    ("qwen3vl", "qwen3-vl-doc-v1"),
    ("qwen3vlmoe", "qwen3-vl-doc-v1"),
    ("qwen2vl", "qwen3-vl-doc-v1"),
    ("qwen3", "qwen-text-v1"),
    ("qwen3moe", "qwen-text-v1"),
    ("qwen2", "qwen-text-v1"),
    ("bert", "embedding"),
    ("nomic-bert", "embedding"),
    ("jina-bert-v2", "embedding"),
    ("xlm-roberta", "embedding"),
];

pub fn prompt_profile_for_arch(arch: &str) -> Option<&'static str> {
    SUPPORTED_ARCHS.iter().find(|(a, _)| *a == arch).map(|(_, p)| *p)
}

pub fn is_embedding_arch(arch: &str) -> bool {
    prompt_profile_for_arch(arch) == Some("embedding")
}

// ---------------------------------------------------------------------------
// model.json (optional manual overrides in a drop-in folder)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModelJson {
    pub id: Option<String>,
    pub display: Option<String>,
    pub family: Option<String>,
    pub role: Option<ModelRole>,
    pub prompt_profile: Option<String>,
    pub license: Option<String>,
    pub license_url: Option<String>,
    pub min_free_vram_mib: Option<u64>,
    /// Engine defaults (ctx, image tokens, sampling …) merged over the computed ones.
    pub defaults: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Scan results
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
    /// False while the file is still being written (recent mtime).
    pub stable: bool,
    pub meta: Option<ModelMeta>,
    pub hashes: Option<FileHashes>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelHealth {
    Ready,
    /// Files are still changing (copy in progress).
    Copying,
    Unsupported {
        reason: String,
    },
    Incomplete {
        missing: Vec<String>,
    },
    Invalid {
        reason: String,
    },
}

impl ModelHealth {
    pub fn is_ready(&self) -> bool {
        matches!(self, ModelHealth::Ready)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchBy {
    Hash,
    Name,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryMatch {
    pub id: String,
    pub by: MatchBy,
    /// `Some(true)` when every pinned hash matched, `Some(false)` on a
    /// mismatch, `None` when nothing is pinned or the files are not hashed yet.
    pub verified: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub folder: PathBuf,
    pub folder_name: String,
    /// Registry id when matched, else `model.json.id`, else the folder name.
    pub id: String,
    pub display: String,
    /// Split-ordered weights files.
    pub weights: Vec<ScannedFile>,
    pub mmproj: Option<ScannedFile>,
    /// Other `.gguf` files in the folder (draft / MTP sidecars …), ignored.
    pub extra_ggufs: Vec<String>,
    pub model_json: Option<ModelJson>,
    pub license_files: Vec<String>,
    pub architecture: Option<String>,
    pub quantization: Option<String>,
    pub has_chat_template: bool,
    pub has_vision: bool,
    pub role: ModelRole,
    pub total_bytes: u64,
    pub health: ModelHealth,
    /// i18n keys (`models.warn.*`).
    pub warnings: Vec<String>,
    pub registry_match: Option<RegistryMatch>,
    pub hashed: bool,
    pub scanned_at: DateTime<Utc>,
}

impl DiscoveredModel {
    pub fn weights_path(&self) -> Option<&Path> {
        self.weights.first().map(|f| f.path.as_path())
    }
    pub fn mmproj_path(&self) -> Option<&Path> {
        self.mmproj.as_ref().map(|f| f.path.as_path())
    }
    pub fn all_files(&self) -> impl Iterator<Item = &ScannedFile> {
        self.weights.iter().chain(self.mmproj.iter())
    }
    pub fn primary_meta(&self) -> Option<&ModelMeta> {
        self.weights.first().and_then(|f| f.meta.as_ref())
    }
}

// ---------------------------------------------------------------------------
// Hash cache
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HashCacheEntry {
    pub size: u64,
    pub fingerprint: String,
    pub sha256: String,
    pub blake3: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HashCache {
    pub version: u32,
    /// Key: `<folder name>/<file name>`.
    pub files: BTreeMap<String, HashCacheEntry>,
}

impl HashCache {
    pub fn load(models_dir: &Path) -> HashCache {
        let p = models_dir.join(HASH_CACHE_FILE);
        match fs::read(&p) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => HashCache::default(),
        }
    }
    pub fn save(&self, models_dir: &Path) -> Result<()> {
        let p = models_dir.join(HASH_CACHE_FILE);
        let tmp = models_dir.join(format!("{HASH_CACHE_FILE}.tmp"));
        let mut me = self.clone();
        me.version = 1;
        let json = serde_json::to_vec_pretty(&me).map_err(|e| CoreError::json(&p, e))?;
        fs::write(&tmp, json).map_err(|e| CoreError::io(&tmp, e))?;
        fs::rename(&tmp, &p).map_err(|e| CoreError::io(&p, e))?;
        Ok(())
    }
    fn key(folder_name: &str, file_name: &str) -> String {
        format!("{folder_name}/{file_name}")
    }
    /// Returns cached hashes when size and fingerprint still match.
    pub fn lookup(&self, folder_name: &str, file: &ScannedFile, fingerprint: &str) -> Option<FileHashes> {
        let e = self.files.get(&Self::key(folder_name, &file.name))?;
        (e.size == file.size && e.fingerprint == fingerprint).then(|| FileHashes {
            size: e.size,
            sha256: e.sha256.clone(),
            blake3: e.blake3.clone(),
        })
    }
    pub fn insert(&mut self, folder_name: &str, file_name: &str, fingerprint: &str, h: &FileHashes) {
        self.files.insert(
            Self::key(folder_name, file_name),
            HashCacheEntry {
                size: h.size,
                fingerprint: fingerprint.to_string(),
                sha256: h.sha256.clone(),
                blake3: h.blake3.clone(),
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct ScanOptions {
    /// Parse GGUF headers (cheap; off only for a bare directory listing).
    pub read_headers: bool,
    /// Reuse cached SHA-256/BLAKE3 values when the fingerprint matches.
    pub use_hash_cache: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { read_headers: true, use_hash_cache: true }
    }
}

fn is_license_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("license") || n.starts_with("notice") || n.starts_with("copying")
}

fn scan_file(path: &Path, read_headers: bool) -> ScannedFile {
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let md = fs::metadata(path);
    let (size, modified) = match &md {
        Ok(m) => (m.len(), m.modified().ok().map(DateTime::<Utc>::from)),
        Err(_) => (0, None),
    };
    let stable = modified.is_none_or(|m| {
        let age = Utc::now().signed_duration_since(m);
        age.num_seconds() >= COPY_SETTLE_SECONDS as i64 || age.num_seconds() < -60
    });
    let (meta, error) = if read_headers {
        match gguf::inspect(path) {
            Ok(m) => (Some(m), None),
            Err(e) => (None, Some(e.to_string())),
        }
    } else {
        (None, None)
    };
    ScannedFile { path: path.to_path_buf(), name, size, modified, stable, meta, hashes: None, error }
}

/// Scans one drop-in folder. Returns `None` for folders without any GGUF.
pub fn scan_model_folder(
    folder: &Path,
    registry: &Registry,
    cache: Option<&HashCache>,
    opts: ScanOptions,
) -> Option<DiscoveredModel> {
    let folder_name = folder.file_name()?.to_string_lossy().into_owned();
    let mut ggufs: Vec<PathBuf> = Vec::new();
    let mut license_files = Vec::new();
    let mut model_json = None;
    let rd = fs::read_dir(folder).ok()?;
    for entry in rd.flatten() {
        let p = entry.path();
        let Some(name) = p.file_name().map(|s| s.to_string_lossy().into_owned()) else { continue };
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if name.to_ascii_lowercase().ends_with(".gguf") {
            ggufs.push(p);
        } else if name == MODEL_JSON {
            model_json = fs::read(&p).ok().and_then(|b| serde_json::from_slice::<ModelJson>(&b).ok());
        } else if is_license_file(&name) {
            license_files.push(name);
        }
    }
    if ggufs.is_empty() {
        return None;
    }
    ggufs.sort();

    let mut weights: Vec<(Option<(u16, u16)>, ScannedFile)> = Vec::new();
    let mut mmproj: Option<ScannedFile> = None;
    let mut extra = Vec::new();
    for p in &ggufs {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let f = scan_file(p, opts.read_headers);
        let kind = f.meta.as_ref().map(|m| m.kind);
        if gguf::is_mmproj_name(&name) || kind == Some(GgufKind::Mmproj) {
            if mmproj.is_none() {
                mmproj = Some(f);
            } else {
                extra.push(name);
            }
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if lower.contains("-mtp") || lower.contains("draft") || lower.contains("eagle") {
            extra.push(name);
            continue;
        }
        weights.push((gguf::split_index_from_name(&name), f));
    }

    // Group splits: prefer a complete split set; otherwise the single largest file.
    let mut missing: Vec<String> = Vec::new();
    let split_total = weights.iter().filter_map(|(s, _)| s.map(|(_, t)| t)).max();
    let mut ordered: Vec<ScannedFile> = if let Some(total) = split_total {
        let mut parts: Vec<(u16, ScannedFile)> = weights
            .iter()
            .filter_map(|(s, f)| s.map(|(i, t)| (i, t, f.clone())))
            .filter(|(_, t, _)| *t == total)
            .map(|(i, _, f)| (i, f))
            .collect();
        parts.sort_by_key(|(i, _)| *i);
        let present: BTreeSet<u16> = parts.iter().map(|(i, _)| *i).collect();
        if let Some((first_idx, first)) = parts.first() {
            let suffix = format!("-{first_idx:05}-of-{total:05}.gguf");
            let stem = first.name.strip_suffix(&suffix).unwrap_or(&first.name).to_string();
            for i in 1..=total {
                if !present.contains(&i) {
                    missing.push(format!("{stem}-{i:05}-of-{total:05}.gguf"));
                }
            }
        }
        for (s, f) in &weights {
            if s.is_none() {
                extra.push(f.name.clone());
            }
        }
        parts.into_iter().map(|(_, f)| f).collect()
    } else {
        weights.sort_by_key(|(_, f)| std::cmp::Reverse(f.size));
        let mut it = weights.into_iter().map(|(_, f)| f);
        let first: Vec<ScannedFile> = it.next().into_iter().collect();
        extra.extend(it.map(|f| f.name));
        first
    };

    // Hash cache
    if let Some(cache) = cache.filter(|_| opts.use_hash_cache) {
        for f in ordered.iter_mut().chain(mmproj.iter_mut()) {
            if let Ok(fp) = gguf::quick_fingerprint(&f.path) {
                f.hashes = cache.lookup(&folder_name, f, &fp);
            }
        }
    }

    let primary = ordered.first().and_then(|f| f.meta.clone());
    let architecture = primary.as_ref().map(|m| m.architecture.clone()).filter(|a| !a.is_empty());
    let quantization = primary.as_ref().and_then(|m| m.quantization.clone());
    let has_chat_template = primary.as_ref().is_some_and(|m| m.has_chat_template);
    let has_vision = mmproj.is_some();
    let is_embedding = architecture.as_deref().is_some_and(is_embedding_arch)
        || model_json.as_ref().and_then(|j| j.role) == Some(ModelRole::Embedding);
    let role = model_json.as_ref().and_then(|j| j.role).unwrap_or(if is_embedding {
        ModelRole::Embedding
    } else if has_vision {
        ModelRole::Vlm
    } else {
        ModelRole::Llm
    });
    let total_bytes: u64 = ordered.iter().map(|f| f.size).sum::<u64>() + mmproj.as_ref().map_or(0, |f| f.size);
    let hashed = !ordered.is_empty() && ordered.iter().chain(mmproj.iter()).all(|f| f.hashes.is_some());

    // Registry matching (hash first, then names).
    let registry_match = match_registry(registry, &ordered, mmproj.as_ref());

    let mut warnings = Vec::new();
    let health = if ordered.is_empty() {
        ModelHealth::Incomplete { missing: vec!["weights".into()] }
    } else if let Some(err) = ordered.iter().chain(mmproj.iter()).find_map(|f| f.error.clone()) {
        if ordered.iter().chain(mmproj.iter()).any(|f| !f.stable) {
            ModelHealth::Copying
        } else {
            ModelHealth::Invalid { reason: err }
        }
    } else if ordered.iter().chain(mmproj.iter()).any(|f| !f.stable) {
        ModelHealth::Copying
    } else if !missing.is_empty() {
        ModelHealth::Incomplete { missing: missing.clone() }
    } else if registry_match.as_ref().is_some_and(|m| m.verified == Some(false)) {
        ModelHealth::Invalid { reason: "hash_mismatch".into() }
    } else if primary.as_ref().is_some_and(|m| m.kind != GgufKind::Llm) {
        ModelHealth::Invalid { reason: "not_a_language_model".into() }
    } else if architecture.as_deref().and_then(prompt_profile_for_arch).is_none() {
        ModelHealth::Unsupported { reason: format!("architecture:{}", architecture.clone().unwrap_or_default()) }
    } else {
        ModelHealth::Ready
    };
    if opts.read_headers && !is_embedding && !has_chat_template && health.is_ready() {
        warnings.push("models.warn.no_chat_template".into());
    }
    if primary.as_ref().is_some_and(|m| !m.gdn_low_precision.is_empty()) {
        warnings.push("models.warn.gdn_low_precision".into());
    }
    if license_files.is_empty() {
        warnings.push("models.warn.no_license_file".into());
    }
    if registry_match.as_ref().is_some_and(|m| m.by == MatchBy::Name && m.verified.is_none()) {
        warnings.push("models.warn.unverified_hash".into());
    }
    if has_vision && is_embedding {
        warnings.push("models.warn.mmproj_ignored".into());
    }

    let id = registry_match
        .as_ref()
        .map(|m| m.id.clone())
        .or_else(|| model_json.as_ref().and_then(|j| j.id.clone()).map(|s| sanitize_id(&s)))
        .unwrap_or_else(|| sanitize_id(&folder_name));
    let display = model_json
        .as_ref()
        .and_then(|j| j.display.clone())
        .or_else(|| registry_match.as_ref().and_then(|m| registry.get(&m.id)).map(|e| e.display.clone()))
        .or_else(|| {
            primary.as_ref().and_then(|m| m.name.clone()).map(|n| match &quantization {
                Some(q) if !n.contains(q) => format!("{n} ({q})"),
                _ => n,
            })
        })
        .unwrap_or_else(|| folder_name.clone());

    Some(DiscoveredModel {
        folder: folder.to_path_buf(),
        folder_name,
        id,
        display,
        weights: ordered,
        mmproj,
        extra_ggufs: extra,
        model_json,
        license_files,
        architecture,
        quantization,
        has_chat_template,
        has_vision,
        role,
        total_bytes,
        health,
        warnings,
        registry_match,
        hashed,
        scanned_at: Utc::now(),
    })
}

fn match_registry(registry: &Registry, weights: &[ScannedFile], mmproj: Option<&ScannedFile>) -> Option<RegistryMatch> {
    let files: Vec<&ScannedFile> = weights.iter().chain(mmproj).collect();
    fn hash_of(sf: &ScannedFile) -> Option<&str> {
        sf.hashes.as_ref().map(|h| h.sha256.as_str())
    }
    // By hash: only *weights* pins identify a model (several registry entries
    // share one mmproj file); the entry with the most weights hits wins.
    let mut best: Option<(usize, &crate::registry::ModelEntry)> = None;
    for entry in &registry.models {
        let weight_pins: Vec<&ModelFile> =
            entry.files.iter().filter(|f| f.sha256.is_some() && !f.is_mmproj()).collect();
        if weight_pins.is_empty() {
            continue;
        }
        let hits = weight_pins.iter().filter(|pf| weights.iter().any(|sf| hash_of(sf) == pf.sha256.as_deref())).count();
        if hits > 0 && best.is_none_or(|(h, _)| hits > h) {
            best = Some((hits, entry));
        }
    }
    if let Some((_, entry)) = best {
        let all_pins_match = entry.files.iter().all(|pf| match &pf.sha256 {
            Some(pin) => files.iter().any(|sf| hash_of(sf) == Some(pin.as_str())),
            None => true,
        });
        let all_hashed = files.iter().all(|f| f.hashes.is_some());
        return Some(RegistryMatch {
            id: entry.id.clone(),
            by: MatchBy::Hash,
            verified: Some(all_pins_match && all_hashed),
        });
    }
    // By name: every registry file name present.
    for entry in &registry.models {
        if entry.files.is_empty() {
            continue;
        }
        let all_present = entry.files.iter().all(|rf| files.iter().any(|sf| sf.name.eq_ignore_ascii_case(&rf.name)));
        if !all_present {
            continue;
        }
        // Name match: a computed hash that contradicts a *weights* pin marks the
        // folder invalid; a differing mmproj (other precision) only leaves it unverified.
        let mut verified = None;
        for rf in &entry.files {
            let Some(pin) = &rf.sha256 else { continue };
            let Some(sf) = files.iter().find(|sf| sf.name.eq_ignore_ascii_case(&rf.name)) else { continue };
            match &sf.hashes {
                Some(h) if &h.sha256 == pin => verified = verified.or(Some(true)),
                Some(_) if !rf.is_mmproj() => {
                    return Some(RegistryMatch { id: entry.id.clone(), by: MatchBy::Name, verified: Some(false) })
                }
                Some(_) => verified = None,
                None => {}
            }
        }
        return Some(RegistryMatch { id: entry.id.clone(), by: MatchBy::Name, verified });
    }
    None
}

/// Lower-cases and replaces anything outside `[a-z0-9._-]` with `-`.
pub fn sanitize_id(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_dash = false;
    for ch in raw.trim().chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "model".to_string()
    } else {
        trimmed
    }
}

/// Scans every sub-folder of `models_dir` (non-recursive beyond one level).
pub fn scan_models_dir(models_dir: &Path, registry: &Registry, opts: ScanOptions) -> Vec<DiscoveredModel> {
    let cache = if opts.use_hash_cache { Some(HashCache::load(models_dir)) } else { None };
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(models_dir) else { return out };
    let mut folders: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| !p.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(true))
        .collect();
    folders.sort();
    for folder in folders {
        if let Some(m) = scan_model_folder(&folder, registry, cache.as_ref(), opts) {
            out.push(m);
        }
    }
    out
}

/// Scans several roots (workspace models dir + optional global store).
pub fn scan_all(roots: &[PathBuf], registry: &Registry, opts: ScanOptions) -> Vec<DiscoveredModel> {
    let mut out = Vec::new();
    for r in roots {
        out.extend(scan_models_dir(r, registry, opts));
    }
    out
}

/// Computes full hashes for every file of a discovered model and updates the
/// cache in `models_dir`. `progress(file_name, done, total)`; returning
/// `false` cancels.
pub fn hash_model(
    models_dir: &Path,
    model: &DiscoveredModel,
    progress: &mut dyn FnMut(&str, u64, u64) -> bool,
) -> Result<Vec<(String, FileHashes)>> {
    let mut cache = HashCache::load(models_dir);
    let mut out = Vec::new();
    for f in model.all_files() {
        let fp = gguf::quick_fingerprint(&f.path)?;
        let hashes = match cache.lookup(&model.folder_name, f, &fp) {
            Some(h) => h,
            None => {
                let name = f.name.clone();
                match gguf::hash_file(&f.path, &mut |d, t| progress(&name, d, t))? {
                    Some(h) => h,
                    None => return Err(CoreError::Cancelled),
                }
            }
        };
        cache.insert(&model.folder_name, &f.name, &fp, &hashes);
        out.push((f.name.clone(), hashes));
    }
    cache.save(models_dir)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Install state (replaces the phase-0 name check)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstallState {
    pub installed: bool,
    /// Folder holding the files, or the install target when not installed.
    pub folder: Option<PathBuf>,
    pub missing_files: Vec<String>,
    pub verified: Option<bool>,
    pub health: Option<ModelHealth>,
    pub matched_by: Option<MatchBy>,
    pub warnings: Vec<String>,
}

/// Installation state of a registry model given the scan results.
pub fn install_state(models_dir: &Path, model: &ModelEntry, discovered: &[DiscoveredModel]) -> InstallState {
    // Built-in entries match through `registry_match`; user-added entries
    // (synthesised from a folder) match by their id.
    let candidates: Vec<&DiscoveredModel> = discovered
        .iter()
        .filter(|d| match &d.registry_match {
            Some(m) => m.id == model.id,
            None => model.user_added && d.id == model.id,
        })
        .collect();
    // Prefer a ready, hash-verified folder.
    let best = candidates
        .iter()
        .max_by_key(|d| {
            (d.health.is_ready(), d.registry_match.as_ref().and_then(|m| m.verified) == Some(true), d.hashed)
        })
        .copied();
    match best {
        Some(d) => InstallState {
            installed: d.health.is_ready(),
            folder: Some(d.folder.clone()),
            missing_files: match &d.health {
                ModelHealth::Incomplete { missing } => missing.clone(),
                _ => Vec::new(),
            },
            verified: d.registry_match.as_ref().and_then(|m| m.verified),
            health: Some(d.health.clone()),
            matched_by: d.registry_match.as_ref().map(|m| m.by),
            warnings: d.warnings.clone(),
        },
        None => InstallState {
            installed: false,
            folder: Some(models_dir.join(&model.id)),
            missing_files: model.files.iter().map(|f| f.name.clone()).collect(),
            verified: None,
            health: None,
            matched_by: None,
            warnings: Vec::new(),
        },
    }
}

// ---------------------------------------------------------------------------
// Registry merge
// ---------------------------------------------------------------------------

/// Tiers whose free-VRAM band can hold `required_mib` (plus CPU for embeddings).
fn tiers_for_requirement(required_mib: u64, role: ModelRole) -> Vec<Tier> {
    let mut tiers: Vec<Tier> = Tier::ALL
        .iter()
        .copied()
        .filter(|t| match t {
            Tier::CpuOnly => required_mib == 0,
            Tier::Vram6gb => required_mib <= 5_632,
            Tier::Vram8gb => required_mib <= 9_728,
            Tier::Vram12gb => required_mib <= 13_824,
            Tier::Vram16gb => required_mib <= 20_480,
            Tier::Vram24gbPlus => true,
        })
        .collect();
    if (role == ModelRole::Embedding || required_mib <= 3_000) && !tiers.contains(&Tier::CpuOnly) {
        tiers.insert(0, Tier::CpuOnly);
    }
    tiers
}

/// Registry entry for a drop-in model that matches nothing built in.
pub fn entry_from_discovered(d: &DiscoveredModel) -> ModelEntry {
    let meta = d.primary_meta();
    let mj = d.model_json.as_ref();
    let arch = d.architecture.clone().unwrap_or_else(|| "unknown".into());
    let ctx_max = meta.and_then(|m| m.context_length).map(u64::from).unwrap_or(8192);
    let required_mib = mj.and_then(|j| j.min_free_vram_mib).unwrap_or_else(|| {
        if d.role == ModelRole::Embedding {
            0
        } else {
            // weights + mmproj + ~1.5 GiB for KV/compute at a modest context
            d.total_bytes / MIB + 1_536
        }
    });
    let mut files: Vec<ModelFile> = d
        .weights
        .iter()
        .map(|f| ModelFile {
            name: f.name.clone(),
            sha256: f.hashes.as_ref().map(|h| h.sha256.clone()),
            size: f.size,
            hf: None,
            url: None,
        })
        .collect();
    if let Some(m) = &d.mmproj {
        files.push(ModelFile {
            name: m.name.clone(),
            sha256: m.hashes.as_ref().map(|h| h.sha256.clone()),
            size: m.size,
            hf: None,
            url: None,
        });
    }
    let mut modalities = vec!["text".to_string()];
    if d.has_vision && d.role != ModelRole::Embedding {
        modalities.push("image".to_string());
    }
    let mut defaults = serde_json::json!({
        "ctx": ctx_max.min(32_768),
        "kv": "q8_0",
        "temperature": 0.1, "top_p": 0.9, "top_k": 20, "thinking": false
    });
    if d.role == ModelRole::Embedding {
        defaults = serde_json::json!({ "ctx": ctx_max.min(2048), "kv": "f16", "embeddings": true, "pooling": "cls", "n_gpu_layers": 0 });
    } else if d.has_vision {
        defaults["image_min_tokens"] = serde_json::json!(1024);
        defaults["image_max_tokens"] = serde_json::json!(2048);
    }
    if let Some(serde_json::Value::Object(extra)) = mj.map(|j| &j.defaults) {
        if let serde_json::Value::Object(base) = &mut defaults {
            for (k, v) in extra {
                base.insert(k.clone(), v.clone());
            }
        }
    }
    ModelEntry {
        id: d.id.clone(),
        family: mj.and_then(|j| j.family.clone()).unwrap_or_else(|| arch.clone()),
        role: d.role,
        display: d.display.clone(),
        license: mj
            .and_then(|j| j.license.clone())
            .or_else(|| meta.and_then(|m| m.license.clone()))
            .unwrap_or_else(|| "unknown".into()),
        license_url: mj.and_then(|j| j.license_url.clone()).or_else(|| meta.and_then(|m| m.license_link.clone())),
        source_repo: meta.and_then(|m| m.source_repo.clone()),
        arch: arch.clone(),
        modalities,
        ctx_max,
        files,
        requirements: ModelRequirements {
            min_free_vram_mib: required_mib,
            min_ram_mib: 8192,
            tiers: tiers_for_requirement(required_mib, d.role),
        },
        defaults,
        capabilities: serde_json::json!({ "json_schema": true }),
        prompt_profile: mj
            .and_then(|j| j.prompt_profile.clone())
            .or_else(|| prompt_profile_for_arch(&arch).map(str::to_string)),
        verified: d.hashed,
        user_added: true,
    }
}

/// Built-in registry plus one entry per unmatched drop-in folder.
pub fn merged_registry(base: &Registry, discovered: &[DiscoveredModel]) -> Registry {
    let mut out = base.clone();
    let mut seen: BTreeSet<String> = out.models.iter().map(|m| m.id.clone()).collect();
    for d in discovered {
        if d.registry_match.is_some() || matches!(d.health, ModelHealth::Invalid { .. }) {
            continue;
        }
        let mut entry = entry_from_discovered(d);
        if seen.contains(&entry.id) {
            // Two drop-in folders with the same id → disambiguate by folder.
            entry.id = format!("{}@{}", entry.id, sanitize_id(&d.folder_name));
        }
        seen.insert(entry.id.clone());
        out.models.push(entry);
    }
    out
}

// ---------------------------------------------------------------------------
// Import (copy / move / link into the models dir)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    Copy,
    Move,
    /// Hard link when on the same volume, else copy.
    Link,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportPlanFile {
    pub source: PathBuf,
    pub dest_name: String,
    pub size: u64,
    pub meta: Option<ModelMeta>,
    pub is_mmproj: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportPlan {
    pub folder_name: String,
    pub target_folder: PathBuf,
    pub files: Vec<ImportPlanFile>,
    pub license_files: Vec<PathBuf>,
    pub total_bytes: u64,
    pub mode: ImportMode,
    pub auto_paired_mmproj: bool,
    pub architecture: Option<String>,
    pub quantization: Option<String>,
    pub has_vision: bool,
    /// i18n keys (`models.warn.*`).
    pub warnings: Vec<String>,
    /// `Some` when the folder already holds the same files (nothing to copy).
    pub already_present: bool,
}

/// Validates the selected files and decides the destination folder. When a
/// single weights file is chosen and the same directory holds exactly one
/// `mmproj*.gguf`, it is paired automatically (§3.3.4).
pub fn plan_import(
    models_dir: &Path,
    sources: &[PathBuf],
    mode: ImportMode,
    folder_name: Option<&str>,
) -> Result<ImportPlan> {
    if sources.is_empty() {
        return Err(CoreError::Invalid("no files selected".into()));
    }
    let mut files: Vec<ImportPlanFile> = Vec::new();
    let mut warnings = Vec::new();
    for src in sources {
        let name = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if !name.to_ascii_lowercase().ends_with(".gguf") {
            return Err(CoreError::Invalid(format!("not a .gguf file: {name}")));
        }
        let md = fs::metadata(src).map_err(|e| CoreError::io(src, e))?;
        let meta = gguf::inspect(src)?;
        files.push(ImportPlanFile {
            source: src.clone(),
            dest_name: name.clone(),
            size: md.len(),
            is_mmproj: meta.kind == GgufKind::Mmproj || gguf::is_mmproj_name(&name),
            meta: Some(meta),
        });
    }
    let mut auto_paired = false;
    let n_weights = files.iter().filter(|f| !f.is_mmproj).count();
    if n_weights == 1 && !files.iter().any(|f| f.is_mmproj) {
        let dir = files[0].source.parent().map(Path::to_path_buf);
        if let Some(dir) = dir {
            let candidates: Vec<PathBuf> = fs::read_dir(&dir)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .filter(|p| p.file_name().map(|n| gguf::is_mmproj_name(&n.to_string_lossy())).unwrap_or(false))
                        .collect()
                })
                .unwrap_or_default();
            if candidates.len() == 1 {
                let p = &candidates[0];
                if let (Ok(md), Ok(meta)) = (fs::metadata(p), gguf::inspect(p)) {
                    files.push(ImportPlanFile {
                        source: p.clone(),
                        dest_name: p.file_name().unwrap().to_string_lossy().into_owned(),
                        size: md.len(),
                        meta: Some(meta),
                        is_mmproj: true,
                    });
                    auto_paired = true;
                }
            }
        }
    }
    if n_weights == 0 {
        return Err(CoreError::Invalid("select the language model .gguf file (mmproj alone is not a model)".into()));
    }
    let primary = files.iter().find(|f| !f.is_mmproj).and_then(|f| f.meta.clone()).unwrap_or_default();
    if primary.kind != GgufKind::Llm {
        return Err(CoreError::Invalid(format!("{} is not a language model", files[0].dest_name)));
    }
    if prompt_profile_for_arch(&primary.architecture).is_none() {
        warnings.push("models.warn.unsupported_arch".into());
    }
    if !primary.has_chat_template && !is_embedding_arch(&primary.architecture) {
        warnings.push("models.warn.no_chat_template".into());
    }
    if !primary.gdn_low_precision.is_empty() {
        warnings.push("models.warn.gdn_low_precision".into());
    }
    // LICENSE / NOTICE next to the weights file.
    let mut license_files = Vec::new();
    if let Some(dir) = files.iter().find(|f| !f.is_mmproj).and_then(|f| f.source.parent()) {
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                if e.file_type().map(|t| t.is_file()).unwrap_or(false)
                    && is_license_file(&e.file_name().to_string_lossy())
                {
                    license_files.push(e.path());
                }
            }
        }
    }
    if license_files.is_empty() {
        warnings.push("models.warn.no_license_file".into());
    }
    let has_vision = files.iter().any(|f| f.is_mmproj);
    let folder = folder_name.map(sanitize_id).filter(|s| !s.is_empty()).unwrap_or_else(|| default_folder_name(&files));
    let target = models_dir.join(&folder);
    let already_present =
        files.iter().all(|f| fs::metadata(target.join(&f.dest_name)).map(|m| m.len() == f.size).unwrap_or(false));
    let total_bytes = files.iter().map(|f| f.size).sum();
    Ok(ImportPlan {
        folder_name: folder,
        target_folder: target,
        files,
        license_files,
        total_bytes,
        mode,
        auto_paired_mmproj: auto_paired,
        architecture: Some(primary.architecture.clone()).filter(|a| !a.is_empty()),
        quantization: primary.quantization.clone(),
        has_vision,
        warnings,
        already_present,
    })
}

fn default_folder_name(files: &[ImportPlanFile]) -> String {
    let weights = files.iter().find(|f| !f.is_mmproj);
    let Some(w) = weights else { return "model".into() };
    let stem = w.dest_name.strip_suffix(".gguf").unwrap_or(&w.dest_name);
    let stem = match stem.rsplit_once("-00") {
        Some((s, rest)) if rest.contains("-of-") => s,
        _ => stem,
    };
    sanitize_id(stem)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportProgress {
    pub file: String,
    pub file_done: u64,
    pub file_total: u64,
    pub total_done: u64,
    pub total: u64,
}

/// Executes an import plan. Files are written as `<name>.part` and renamed
/// on completion; cancelling (`progress` returns `false`) removes the partial
/// file. Returns the destination folder.
pub fn run_import(plan: &ImportPlan, progress: &mut dyn FnMut(&ImportProgress) -> bool) -> Result<PathBuf> {
    let target = &plan.target_folder;
    fs::create_dir_all(target).map_err(|e| CoreError::io(target, e))?;
    let mut total_done = 0u64;
    for f in &plan.files {
        let dest = target.join(&f.dest_name);
        if fs::metadata(&dest).map(|m| m.len() == f.size).unwrap_or(false) {
            total_done += f.size;
            continue;
        }
        let part = target.join(format!("{}.part", f.dest_name));
        let _ = fs::remove_file(&part);
        let same_volume_rename = |src: &Path, dst: &Path| fs::rename(src, dst).is_ok();
        let done = match plan.mode {
            ImportMode::Move if same_volume_rename(&f.source, &dest) => true,
            ImportMode::Link if fs::hard_link(&f.source, &dest).is_ok() => true,
            _ => false,
        };
        if !done {
            copy_with_progress(&f.source, &part, f.size, &mut |done| {
                progress(&ImportProgress {
                    file: f.dest_name.clone(),
                    file_done: done,
                    file_total: f.size,
                    total_done: total_done + done,
                    total: plan.total_bytes,
                })
            })?;
            fs::rename(&part, &dest).map_err(|e| CoreError::io(&dest, e))?;
            if plan.mode == ImportMode::Move {
                let _ = fs::remove_file(&f.source);
            }
        }
        mark_settled(&dest);
        total_done += f.size;
        progress(&ImportProgress {
            file: f.dest_name.clone(),
            file_done: f.size,
            file_total: f.size,
            total_done,
            total: plan.total_bytes,
        });
    }
    for lf in &plan.license_files {
        if let Some(name) = lf.file_name() {
            let _ = fs::copy(lf, target.join(name));
        }
    }
    Ok(target.clone())
}

fn copy_with_progress(src: &Path, dst: &Path, total: u64, progress: &mut dyn FnMut(u64) -> bool) -> Result<()> {
    let mut input = fs::File::open(src).map_err(|e| CoreError::io(src, e))?;
    let mut output = fs::File::create(dst).map_err(|e| CoreError::io(dst, e))?;
    let mut buf = vec![0u8; 4 << 20];
    let mut done = 0u64;
    let mut last_report = Instant::now();
    if !progress(0) {
        drop(output);
        let _ = fs::remove_file(dst);
        return Err(CoreError::Cancelled);
    }
    loop {
        let n = input.read(&mut buf).map_err(|e| CoreError::io(src, e))?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n]).map_err(|e| CoreError::io(dst, e))?;
        done += n as u64;
        if last_report.elapsed() >= Duration::from_millis(200) {
            last_report = Instant::now();
            if !progress(done) {
                drop(output);
                let _ = fs::remove_file(dst);
                return Err(CoreError::Cancelled);
            }
        }
    }
    output.sync_all().map_err(|e| CoreError::io(dst, e))?;
    if total > 0 && done != total {
        log::warn!("{}: copied {done} bytes, expected {total}", src.display());
    }
    Ok(())
}

/// Deletes a drop-in folder (only files we recognise plus the folder itself
/// when it becomes empty). Refuses paths outside `models_dir`.
pub fn delete_model_folder(models_dir: &Path, folder: &Path) -> Result<()> {
    let canon_dir = fs::canonicalize(models_dir).map_err(|e| CoreError::io(models_dir, e))?;
    let canon = fs::canonicalize(folder).map_err(|e| CoreError::io(folder, e))?;
    if !canon.starts_with(&canon_dir) || canon == canon_dir {
        return Err(CoreError::Invalid(format!("{} is not inside the models folder", folder.display())));
    }
    // A just-unloaded instance may keep the mmap'd GGUF open for a moment.
    let mut last = None;
    for attempt in 0..6 {
        match fs::remove_dir_all(&canon) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                last = Some(e);
                if attempt < 5 {
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
    }
    Err(CoreError::io(&canon, last.expect("error recorded")))
}

// ---------------------------------------------------------------------------
// Download (resumable, SHA-256 verified)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadRequest {
    pub url: String,
    /// Final destination; data is written to `<dest>.part` first.
    pub dest: PathBuf,
    pub expected_size: Option<u64>,
    pub expected_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadOutcome {
    pub path: PathBuf,
    pub bytes: u64,
    pub resumed_from: u64,
    pub sha256: Option<String>,
    /// `Some(true)` when the hash matched the pin, `None` when nothing was pinned.
    pub verified: Option<bool>,
}

/// HTTP agent used for downloads (TLS via the platform store; honours the
/// usual proxy environment variables).
pub fn download_agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    // ureq never picks a TLS provider automatically: without this an https
    // request panics ("provider is Rustls but feature is not enabled").
    let tls = TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build();
    let config = ureq::Agent::config_builder()
        .tls_config(tls)
        // statuses (206 / 416 / 4xx) are handled explicitly in `download_file`
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(20)))
        // stall detection per read, not a budget for the whole (multi-GB) body
        .timeout_per_read(Some(Duration::from_secs(60)))
        .user_agent(concat!("DocuLens/", env!("CARGO_PKG_VERSION")))
        .build();
    config.new_agent()
}

/// Downloads `req.url` to `req.dest` with resume support. `progress(done,
/// total)` is called periodically; returning `false` cancels and keeps the
/// partial file for a later resume.
pub fn download_file(
    agent: &ureq::Agent,
    req: &DownloadRequest,
    progress: &mut dyn FnMut(u64, Option<u64>) -> bool,
) -> Result<DownloadOutcome> {
    let dest = &req.dest;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    let part =
        dest.with_file_name(format!("{}.part", dest.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()));
    let existing = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);

    let mut request = agent.get(&req.url);
    if existing > 0 {
        request = request.header("Range", &format!("bytes={existing}-"));
    }
    let mut response = request.call().map_err(|e| CoreError::Download(e.to_string()))?;
    let status = response.status().as_u16();
    let content_range = response.headers().get("Content-Range").and_then(|v| v.to_str().ok()).map(str::to_string);
    let (resumed_from, append) = match status {
        206 if existing > 0 => {
            // The server must continue exactly where the partial file ends.
            match content_range_start(content_range.as_deref()) {
                Some(start) if start == existing => (existing, true),
                _ => {
                    let _ = fs::remove_file(&part);
                    return Err(CoreError::Download(format!("server ignored the resume offset for {}", req.url)));
                }
            }
        }
        200 => (0, false),
        416 => {
            // Range not satisfiable: complete when the partial file already has the full size.
            let full = content_range_total(content_range.as_deref()).or(req.expected_size);
            if existing > 0 && full == Some(existing) {
                return finish_download(req, &part, existing, progress);
            }
            let _ = fs::remove_file(&part);
            return Err(CoreError::Download(format!(
                "partial file for {} was unusable and has been discarded; retry",
                req.url
            )));
        }
        s => return Err(CoreError::Download(format!("HTTP {s} for {}", req.url))),
    };
    let total = if append {
        content_range_total(response.headers().get("Content-Range").and_then(|v| v.to_str().ok())).or_else(|| {
            response
                .headers()
                .get("Content-Length")
                .and_then(|v| v.to_str().ok()?.parse::<u64>().ok())
                .map(|l| l + existing)
        })
    } else {
        response.headers().get("Content-Length").and_then(|v| v.to_str().ok()?.parse::<u64>().ok())
    }
    .or(req.expected_size);

    let mut file = if append {
        fs::OpenOptions::new().append(true).open(&part).map_err(|e| CoreError::io(&part, e))?
    } else {
        fs::File::create(&part).map_err(|e| CoreError::io(&part, e))?
    };
    let mut done = resumed_from;
    if !progress(done, total) {
        return Err(CoreError::Cancelled);
    }
    let mut body = response.body_mut().as_reader();
    let mut buf = vec![0u8; 1 << 20];
    let mut last_report = Instant::now();
    loop {
        let n = body.read(&mut buf).map_err(|e| CoreError::Download(e.to_string()))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| CoreError::io(&part, e))?;
        done += n as u64;
        if last_report.elapsed() >= Duration::from_millis(250) {
            last_report = Instant::now();
            if !progress(done, total) {
                let _ = file.sync_all();
                return Err(CoreError::Cancelled);
            }
        }
    }
    file.sync_all().map_err(|e| CoreError::io(&part, e))?;
    drop(file);
    if let Some(t) = total {
        if done != t {
            return Err(CoreError::Download(format!("incomplete: {done} of {t} bytes")));
        }
    }
    let mut outcome = finish_download(req, &part, done, progress)?;
    outcome.resumed_from = resumed_from;
    Ok(outcome)
}

fn content_range_total(header: Option<&str>) -> Option<u64> {
    // "bytes 100-999/1000" or "bytes */1000"
    header?.rsplit_once('/')?.1.trim().parse().ok()
}

fn content_range_start(header: Option<&str>) -> Option<u64> {
    // "bytes 100-999/1000"
    let h = header?.trim().strip_prefix("bytes")?.trim();
    h.split_once('-')?.0.trim().parse().ok()
}

/// Files the app wrote itself are complete by construction: back-date the
/// mtime past the copy-settle window so the next scan reports them `Ready`.
pub fn mark_settled(path: &Path) {
    let t = SystemTime::now() - Duration::from_secs(COPY_SETTLE_SECONDS + 1);
    if let Ok(f) = fs::File::options().write(true).open(path) {
        let _ = f.set_modified(t);
    }
}

fn finish_download(
    req: &DownloadRequest,
    part: &Path,
    bytes: u64,
    progress: &mut dyn FnMut(u64, Option<u64>) -> bool,
) -> Result<DownloadOutcome> {
    let (sha256, verified) = match &req.expected_sha256 {
        Some(pin) => {
            let total = Some(bytes);
            let hash = gguf::sha256_file(part, &mut |_, _| progress(bytes, total))?.ok_or(CoreError::Cancelled)?;
            let ok = hash.eq_ignore_ascii_case(pin);
            if !ok {
                let _ = fs::remove_file(part);
                return Err(CoreError::Download(format!(
                    "SHA-256 mismatch for {}: expected {pin}, got {hash}",
                    req.url
                )));
            }
            (Some(hash), Some(true))
        }
        None => (None, None),
    };
    fs::rename(part, &req.dest).map_err(|e| CoreError::io(&req.dest, e))?;
    mark_settled(&req.dest);
    Ok(DownloadOutcome { path: req.dest.clone(), bytes, resumed_from: 0, sha256, verified })
}

// ---------------------------------------------------------------------------
// Folder watcher
// ---------------------------------------------------------------------------

/// Watches a models directory and calls `on_change` once the folder has been
/// quiet for `settle`. Dropping the handle stops the watcher.
pub struct ModelWatcher {
    _watcher: notify::RecommendedWatcher,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub root: PathBuf,
}

impl ModelWatcher {
    pub fn start(root: &Path, settle: Duration, on_change: impl Fn() + Send + 'static) -> Result<ModelWatcher> {
        use notify::{RecursiveMode, Watcher};
        let (tx, rx) = mpsc::channel::<()>();
        let mut watcher = notify::recommended_watcher(move |res: std::result::Result<notify::Event, notify::Error>| {
            if let Ok(ev) = res {
                if !is_content_change(&ev.kind) {
                    return; // opens / reads / metadata-only changes (e.g. the engine mmapping a model)
                }
                // Only react to GGUF / model.json / license files and folders
                // (ignore our own hash-cache writes).
                let relevant = ev.paths.iter().any(|p| {
                    let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                    name.ends_with(".gguf")
                        || name.ends_with(".gguf.part")
                        || name == MODEL_JSON
                        || is_license_file(&name)
                }) || ev.paths.iter().any(|p| p.is_dir() || p.extension().is_none());
                if relevant {
                    let _ = tx.send(());
                }
            }
        })
        .map_err(|e| CoreError::engine(format!("watcher: {e}")))?;
        fs::create_dir_all(root).map_err(|e| CoreError::io(root, e))?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| CoreError::engine(format!("watch {}: {e}", root.display())))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("doculens-model-watch".into())
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
        Ok(ModelWatcher { _watcher: watcher, stop, thread: Some(thread), root: root.to_path_buf() })
    }
}

impl Drop for ModelWatcher {
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
fn is_content_change(kind: &notify::EventKind) -> bool {
    use notify::event::{EventKind, ModifyKind};
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any => true,
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_) | ModifyKind::Any) => true,
        EventKind::Modify(ModifyKind::Metadata(_) | ModifyKind::Other) => false,
        EventKind::Access(_) | EventKind::Other => false,
    }
}

/// Stable signature of a scan result (folders, ids, health, files, sizes,
/// hashes) so callers can tell whether anything relevant to the engine changed.
pub fn scan_signature(models: &[DiscoveredModel]) -> String {
    let mut hasher = blake3::Hasher::new();
    for d in models {
        hasher.update(d.folder.to_string_lossy().as_bytes());
        hasher.update(d.id.as_bytes());
        hasher.update(format!("{:?}", d.health).as_bytes());
        for f in d.all_files() {
            hasher.update(f.name.as_bytes());
            hasher.update(&f.size.to_le_bytes());
            if let Some(h) = &f.hashes {
                hasher.update(h.sha256.as_bytes());
            }
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// Unix timestamp helper for tests/tools.
pub fn system_time_secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gguf::{synthetic_qwen35_metadata, write_minimal_gguf, GgufValue};

    fn mmproj_meta() -> Vec<(&'static str, GgufValue)> {
        vec![
            ("general.architecture", GgufValue::String("clip".into())),
            ("clip.projector_type", GgufValue::String("qwen3vl_merger".into())),
            ("clip.has_vision_encoder", GgufValue::Bool(true)),
        ]
    }

    fn make_folder(models_dir: &Path, folder: &str, weights: &str, with_mmproj: bool) -> PathBuf {
        let dir = models_dir.join(folder);
        fs::create_dir_all(&dir).unwrap();
        write_minimal_gguf(
            &dir.join(weights),
            &synthetic_qwen35_metadata("Qwen3.5 4B", 18, true),
            &[("blk.0.ssm_a", &[32], 0)],
        )
        .unwrap();
        if with_mmproj {
            write_minimal_gguf(&dir.join("mmproj-F16.gguf"), &mmproj_meta(), &[("v.x", &[8], 1)]).unwrap();
        }
        backdate(&dir);
        dir
    }

    /// Make files look "settled" (older than the copy window).
    fn backdate(dir: &Path) {
        let t = SystemTime::now() - Duration::from_secs(COPY_SETTLE_SECONDS + 5);
        for e in fs::read_dir(dir).unwrap().flatten() {
            let f = fs::File::options().write(true).open(e.path()).unwrap();
            f.set_modified(t).unwrap();
        }
    }

    #[test]
    fn scan_matches_registry_by_name_and_reports_health() {
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        make_folder(&models, "my-qwen", "Qwen3.5-4B-Q6_K.gguf", true);
        let reg = Registry::builtin();
        let found = scan_models_dir(&models, &reg, ScanOptions::default());
        assert_eq!(found.len(), 1);
        let d = &found[0];
        assert_eq!(d.id, "qwen3.5-4b-q6k");
        assert_eq!(d.registry_match.as_ref().unwrap().by, MatchBy::Name);
        assert_eq!(d.registry_match.as_ref().unwrap().verified, None);
        assert!(d.health.is_ready(), "{:?}", d.health);
        assert!(d.has_vision);
        assert_eq!(d.quantization.as_deref(), Some("Q6_K"));
        assert!(d.warnings.contains(&"models.warn.unverified_hash".to_string()));
        assert!(d.warnings.contains(&"models.warn.no_license_file".to_string()));

        let st = install_state(&models, reg.get("qwen3.5-4b-q6k").unwrap(), &found);
        assert!(st.installed);
        assert_eq!(st.folder, Some(models.join("my-qwen")));
        let st2 = install_state(&models, reg.get("qwen3.5-4b-q4km").unwrap(), &found);
        assert!(!st2.installed);
        assert_eq!(st2.folder, Some(models.join("qwen3.5-4b-q4km")));
        assert_eq!(st2.missing_files.len(), 2);
    }

    #[test]
    fn unknown_model_becomes_user_added_entry_and_unsupported_arch_is_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        let dir = models.join("custom one");
        fs::create_dir_all(&dir).unwrap();
        write_minimal_gguf(
            &dir.join("custom-model-q4_k_m.gguf"),
            &synthetic_qwen35_metadata("Custom Model", 15, true),
            &[("a", &[8], 0)],
        )
        .unwrap();
        fs::write(dir.join("LICENSE"), "Apache-2.0").unwrap();
        fs::write(dir.join(MODEL_JSON), r#"{"display":"My Custom","defaults":{"ctx":4096}}"#).unwrap();
        backdate(&dir);
        let other = models.join("weird");
        fs::create_dir_all(&other).unwrap();
        let mut meta = synthetic_qwen35_metadata("Weird", 1, true);
        meta[0] = ("general.architecture", GgufValue::String("mystery-arch".into()));
        write_minimal_gguf(&other.join("weird.gguf"), &meta, &[("a", &[8], 0)]).unwrap();
        backdate(&other);

        let reg = Registry::builtin();
        let found = scan_models_dir(&models, &reg, ScanOptions::default());
        assert_eq!(found.len(), 2);
        let custom = found.iter().find(|d| d.folder_name == "custom one").unwrap();
        assert_eq!(custom.id, "custom-one");
        assert_eq!(custom.display, "My Custom");
        assert!(custom.registry_match.is_none());
        assert!(custom.health.is_ready());
        assert_eq!(custom.role, ModelRole::Llm);
        assert!(!custom.warnings.contains(&"models.warn.no_license_file".to_string()));
        let weird = found.iter().find(|d| d.folder_name == "weird").unwrap();
        assert!(matches!(weird.health, ModelHealth::Unsupported { .. }));

        let merged = merged_registry(&reg, &found);
        assert_eq!(merged.models.len(), reg.models.len() + 2);
        let e = merged.get("custom-one").unwrap();
        let st = install_state(&models, e, &found);
        assert!(st.installed, "user-added models must report their folder as installed: {st:?}");
        assert_eq!(st.folder, Some(models.join("custom one")));
        assert!(e.user_added);
        assert_eq!(e.defaults["ctx"], 4096);
        assert_eq!(e.prompt_profile.as_deref(), Some("qwen3.5-doc-v1"));
        assert_eq!(e.modalities, vec!["text".to_string()]);
        assert!(merged.lint().is_empty(), "{:?}", merged.lint());
    }

    #[test]
    fn split_sets_and_copy_in_progress() {
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        let dir = models.join("big");
        fs::create_dir_all(&dir).unwrap();
        for i in [1u16, 3u16] {
            write_minimal_gguf(
                &dir.join(format!("Big-Q4_K_M-{i:05}-of-00003.gguf")),
                &synthetic_qwen35_metadata("Big", 15, true),
                &[("a", &[8], 0)],
            )
            .unwrap();
        }
        backdate(&dir);
        let reg = Registry::builtin();
        let found = scan_models_dir(&models, &reg, ScanOptions::default());
        let d = &found[0];
        assert_eq!(d.weights.len(), 2);
        assert_eq!(d.health, ModelHealth::Incomplete { missing: vec!["Big-Q4_K_M-00002-of-00003.gguf".into()] });
        // a file written just now → copying
        write_minimal_gguf(
            &dir.join("Big-Q4_K_M-00002-of-00003.gguf"),
            &synthetic_qwen35_metadata("Big", 15, true),
            &[("a", &[8], 0)],
        )
        .unwrap();
        let found = scan_models_dir(&models, &reg, ScanOptions::default());
        assert_eq!(found[0].health, ModelHealth::Copying);
        backdate(&dir);
        let found = scan_models_dir(&models, &reg, ScanOptions::default());
        assert!(found[0].health.is_ready());
        assert_eq!(found[0].weights.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().len(), 3);
    }

    #[test]
    fn hashing_updates_cache_and_hash_match_verifies() {
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        make_folder(&models, "q", "Qwen3.5-4B-Q4_K_M.gguf", true);
        let mut reg = Registry::builtin();
        let found = scan_models_dir(&models, &reg, ScanOptions::default());
        let mut calls = 0;
        let hashes = hash_model(&models, &found[0], &mut |_, _, _| {
            calls += 1;
            true
        })
        .unwrap();
        assert_eq!(hashes.len(), 2);
        assert!(models.join(HASH_CACHE_FILE).is_file());
        // pin the weights hash in the registry → second scan verifies by hash
        let sha = hashes.iter().find(|(n, _)| n == "Qwen3.5-4B-Q4_K_M.gguf").unwrap().1.sha256.clone();
        let m = reg.models.iter_mut().find(|m| m.id == "qwen3.5-4b-q4km").unwrap();
        m.files[0].sha256 = Some(sha);
        let found = scan_models_dir(&models, &reg, ScanOptions::default());
        assert!(found[0].hashed);
        let rm = found[0].registry_match.as_ref().unwrap();
        assert_eq!(rm.by, MatchBy::Hash);
        assert_eq!(rm.verified, Some(true));
        // a wrong pin → mismatch → invalid
        let m = reg.models.iter_mut().find(|m| m.id == "qwen3.5-4b-q4km").unwrap();
        m.files[0].sha256 = Some("00".repeat(32));
        let found = scan_models_dir(&models, &reg, ScanOptions::default());
        assert_eq!(found[0].registry_match.as_ref().unwrap().verified, Some(false));
        assert_eq!(found[0].health, ModelHealth::Invalid { reason: "hash_mismatch".into() });
        assert!(calls > 0);
    }

    #[test]
    fn import_plan_pairs_mmproj_and_copies_with_progress() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("usb");
        fs::create_dir_all(&src).unwrap();
        write_minimal_gguf(
            &src.join("Qwen3.5-4B-Q6_K.gguf"),
            &synthetic_qwen35_metadata("Qwen3.5 4B", 18, true),
            &[("a", &[4096], 0)],
        )
        .unwrap();
        write_minimal_gguf(&src.join("mmproj-F16.gguf"), &mmproj_meta(), &[("v", &[64], 1)]).unwrap();
        fs::write(src.join("LICENSE"), "apache").unwrap();
        let models = tmp.path().join("models");
        let plan = plan_import(&models, &[src.join("Qwen3.5-4B-Q6_K.gguf")], ImportMode::Copy, None).unwrap();
        assert!(plan.auto_paired_mmproj);
        assert_eq!(plan.files.len(), 2);
        assert_eq!(plan.folder_name, "qwen3.5-4b-q6_k");
        assert_eq!(plan.license_files.len(), 1);
        assert!(!plan.already_present);
        let mut reports = 0;
        let folder = run_import(&plan, &mut |_| {
            reports += 1;
            true
        })
        .unwrap();
        assert!(folder.join("Qwen3.5-4B-Q6_K.gguf").is_file());
        assert!(folder.join("mmproj-F16.gguf").is_file());
        assert!(folder.join("LICENSE").is_file());
        assert!(reports >= 2);
        let plan2 = plan_import(&models, &[src.join("Qwen3.5-4B-Q6_K.gguf")], ImportMode::Copy, None).unwrap();
        assert!(plan2.already_present);
        // mmproj alone is rejected, non-gguf is rejected
        assert!(plan_import(&models, &[src.join("mmproj-F16.gguf")], ImportMode::Copy, None).is_err());
        assert!(plan_import(&models, &[src.join("LICENSE")], ImportMode::Copy, None).is_err());
        // cancel removes the partial file
        let src2 = src.join("Other-Q4_K_M.gguf");
        write_minimal_gguf(&src2, &synthetic_qwen35_metadata("Other", 15, true), &[("a", &[4096], 0)]).unwrap();
        let plan3 = plan_import(&models, std::slice::from_ref(&src2), ImportMode::Copy, Some("other")).unwrap();
        let err = run_import(&plan3, &mut |_| false).unwrap_err();
        assert_eq!(err.code(), "cancelled");
        assert!(!models.join("other").join("Other-Q4_K_M.gguf").exists());
        assert!(!models.join("other").join("Other-Q4_K_M.gguf.part").exists());
        // delete refuses paths outside the models dir
        assert!(delete_model_folder(&models, &src).is_err());
        delete_model_folder(&models, &folder).unwrap();
        assert!(!folder.exists());
    }

    #[test]
    fn hash_match_ignores_shared_mmproj_pins() {
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        make_folder(&models, "six", "Qwen3.5-4B-Q6_K.gguf", true);
        let mut reg = Registry::builtin();
        let found = scan_models_dir(&models, &reg, ScanOptions::default());
        let hashes = hash_model(&models, &found[0], &mut |_, _, _| true).unwrap();
        let weights_sha = hashes.iter().find(|(n, _)| n == "Qwen3.5-4B-Q6_K.gguf").unwrap().1.sha256.clone();
        let mmproj_sha = hashes.iter().find(|(n, _)| n == "mmproj-F16.gguf").unwrap().1.sha256.clone();
        // Pin the (shared) mmproj on every 4B entry and the weights only on q6k.
        for m in reg.models.iter_mut().filter(|m| m.id.starts_with("qwen3.5-4b")) {
            for f in m.files.iter_mut().filter(|f| f.is_mmproj()) {
                f.sha256 = Some(mmproj_sha.clone());
            }
        }
        reg.models.iter_mut().find(|m| m.id == "qwen3.5-4b-q6k").unwrap().files[0].sha256 = Some(weights_sha);
        let found = scan_models_dir(&models, &reg, ScanOptions::default());
        let rm = found[0].registry_match.as_ref().unwrap();
        assert_eq!(rm.id, "qwen3.5-4b-q6k");
        assert_eq!(rm.by, MatchBy::Hash);
        assert_eq!(rm.verified, Some(true));
        assert!(found[0].health.is_ready());
        // A differing mmproj pin alone never invalidates a name match.
        for m in reg.models.iter_mut().filter(|m| m.id == "qwen3.5-4b-q6k") {
            m.files[0].sha256 = None;
            m.files[1].sha256 = Some("ff".repeat(32));
        }
        let found = scan_models_dir(&models, &reg, ScanOptions::default());
        let rm = found[0].registry_match.as_ref().unwrap();
        assert_eq!((rm.id.as_str(), rm.by, rm.verified), ("qwen3.5-4b-q6k", MatchBy::Name, None));
        assert!(found[0].health.is_ready());
    }

    #[test]
    fn download_agent_has_a_tls_provider() {
        // Must fail with a connection error, never panic on the missing-provider path.
        let agent = download_agent();
        let r = agent.get("https://127.0.0.1:1/x.gguf").call();
        assert!(r.is_err());
        assert_eq!(content_range_start(Some("bytes 100-999/1000")), Some(100));
        assert_eq!(content_range_total(Some("bytes */1000")), Some(1000));
    }

    #[test]
    fn imported_files_scan_as_ready_immediately() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("usb");
        fs::create_dir_all(&src).unwrap();
        write_minimal_gguf(
            &src.join("Fresh-Q4_K_M.gguf"),
            &synthetic_qwen35_metadata("Fresh", 15, true),
            &[("a", &[64], 0)],
        )
        .unwrap();
        let models = tmp.path().join("models");
        let plan = plan_import(&models, &[src.join("Fresh-Q4_K_M.gguf")], ImportMode::Copy, None).unwrap();
        run_import(&plan, &mut |_| true).unwrap();
        let found = scan_models_dir(&models, &Registry::builtin(), ScanOptions::default());
        assert_eq!(found.len(), 1);
        assert!(found[0].health.is_ready(), "{:?}", found[0].health);
    }

    #[test]
    fn sanitize_ids() {
        assert_eq!(sanitize_id("My Model (Q4)"), "my-model-q4");
        assert_eq!(sanitize_id("  ///  "), "model");
        assert_eq!(sanitize_id("Qwen3.5-4B_Q6_K"), "qwen3.5-4b_q6_k");
    }

    #[test]
    fn tiers_from_requirement() {
        assert_eq!(tiers_for_requirement(0, ModelRole::Embedding).len(), 6);
        let t = tiers_for_requirement(7000, ModelRole::Vlm);
        assert!(!t.contains(&Tier::Vram6gb));
        assert!(t.contains(&Tier::Vram8gb));
        assert!(!t.contains(&Tier::CpuOnly));
    }
}
