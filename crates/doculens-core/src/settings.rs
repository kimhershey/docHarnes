//! Global settings — `<AppData>/DocuLens/settings.json` (design §3.6, decision 14).
//! The file is the single source of truth for the active workspace path, theme,
//! language, UI scale and tier/model pins. Editing it by hand is supported:
//! unknown fields are ignored, missing fields take defaults, and a corrupt file
//! is moved aside instead of blocking startup.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::hw::Tier;
use crate::version::SETTINGS_SCHEMA_VERSION;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    Light,
    Dark,
    #[default]
    System,
    HighContrast,
}

impl Theme {
    pub const ALL: [Theme; 4] = [Theme::Light, Theme::Dark, Theme::System, Theme::HighContrast];

    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::System => "system",
            Theme::HighContrast => "high-contrast",
        }
    }

    /// Parses machine names and common Korean/English words.
    pub fn parse(s: &str) -> Option<Theme> {
        let t = s.trim().to_lowercase().replace(['_', ' '], "-");
        match t.as_str() {
            "light" | "라이트" | "밝게" | "밝은" | "화이트" | "white" | "bright" => Some(Theme::Light),
            "dark" | "다크" | "어둡게" | "어두운" | "블랙" | "black" => Some(Theme::Dark),
            "system" | "시스템" | "자동" | "auto" | "os" => Some(Theme::System),
            "high-contrast" | "highcontrast" | "고대비" | "contrast" | "hc" => Some(Theme::HighContrast),
            _ => None,
        }
    }

    /// Light/dark toggle used by the top-bar theme button (Ctrl+Shift+T).
    /// `system_is_dark` tells which variant the OS currently resolves to.
    pub fn toggled(self, system_is_dark: bool) -> Theme {
        match self {
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::Light,
            Theme::HighContrast => Theme::Light,
            Theme::System => {
                if system_is_dark {
                    Theme::Light
                } else {
                    Theme::Dark
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Ko,
    En,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::Ko, Language::En];

    pub fn as_str(self) -> &'static str {
        match self {
            Language::Ko => "ko",
            Language::En => "en",
        }
    }

    pub fn parse(s: &str) -> Option<Language> {
        let t = s.trim().to_lowercase();
        match t.as_str() {
            "ko" | "ko-kr" | "kr" | "한국어" | "한글" | "korean" => Some(Language::Ko),
            "en" | "en-us" | "en-gb" | "영어" | "english" => Some(Language::En),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TierMode {
    #[default]
    Auto,
    Manual,
}

/// Inference profile (prompt/image-token/sampling preset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    Accuracy,
    Balanced,
    Speed,
    FinePrint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackendPreference {
    #[default]
    Auto,
    Cuda,
    Vulkan,
    Cpu,
}

/// How much of the pipeline runs automatically after an import (§3.4
/// `auto_pipeline`): nothing, classification, classification + OCR, or the
/// full chain up to extraction / validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoProcessingLevel {
    None,
    ClassifyOnly,
    #[default]
    ClassifyAndOcr,
    Full,
}

impl AutoProcessingLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            AutoProcessingLevel::None => "none",
            AutoProcessingLevel::ClassifyOnly => "classify_only",
            AutoProcessingLevel::ClassifyAndOcr => "classify_and_ocr",
            AutoProcessingLevel::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReducedMotion {
    #[default]
    System,
    On,
    Off,
}

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub theme: Theme,
    pub language: Language,
    /// 90..=150 (%), independent of the Windows DPI setting.
    pub scale_percent: u16,
    pub reduced_motion: ReducedMotion,
    pub sidebar_collapsed: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            language: Language::Ko,
            scale_percent: 100,
            reduced_motion: ReducedMotion::System,
            sidebar_collapsed: false,
        }
    }
}

pub const UI_SCALE_MIN: u16 = 90;
pub const UI_SCALE_MAX: u16 = 150;
pub const UI_SCALE_STEP: u16 = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSettings {
    pub tier_mode: TierMode,
    /// Only used when `tier_mode == Manual`.
    pub manual_tier: Option<Tier>,
    /// `"auto"` or a registry model id. Global (design §3.3.3).
    pub active_model: String,
    pub profile: Profile,
    pub backend: BackendPreference,
    /// Max resident model instances in the llama-server router.
    pub resident_models_max: u8,
    /// `--sleep-idle-seconds`.
    pub sleep_idle_seconds: u32,
    /// Online downloads are only offered when the OS reports connectivity.
    pub allow_online_download: bool,
    /// Start the llama-server sidecar automatically when a workspace opens.
    pub auto_start_engine: bool,
    /// Send a one-token warm-up request after a model loads (Turing JIT, §3.3.1).
    pub warmup_after_load: bool,
    /// "Audit mode": run the vision encoder on the CPU (`mmproj-offload = false`).
    pub vision_on_cpu: bool,
    /// Explicit `llama-server` binary instead of the installed packs.
    pub engine_binary: Option<PathBuf>,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            tier_mode: TierMode::Auto,
            manual_tier: None,
            active_model: "auto".to_string(),
            profile: Profile::Accuracy,
            backend: BackendPreference::Auto,
            resident_models_max: 2,
            sleep_idle_seconds: 900,
            allow_online_download: true,
            auto_start_engine: true,
            warmup_after_load: true,
            vision_on_cpu: false,
            engine_binary: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessingSettings {
    /// Level for drop / folder imports (design default: classification).
    pub auto_level: AutoProcessingLevel,
    /// Level for files that arrive through the inbox watcher (design default: full).
    pub inbox_auto_level: AutoProcessingLevel,
    /// Default render DPI (150-300).
    pub render_dpi: u16,
    /// Fields below this confidence go to the review queue (0.0-1.0).
    pub review_threshold: f32,
    /// Random audit sample of high-confidence fields (%).
    pub audit_sample_percent: u8,
    /// Organise/rename only documents whose review is complete.
    pub organize_reviewed_only: bool,
    pub inbox_watch: bool,
    /// Run the VLM OCR even on pages that carry a PDF text layer (off: the
    /// text layer is used as is — design §3.4 "digital PDF hybrid").
    pub ocr_digital_pdf: bool,
    /// Pages per document sent to the field extractor (first N−1 + last page).
    pub kie_max_pages: u8,
    /// Precision mode: a second extraction sample (T=0.3) whose agreement feeds the confidence.
    pub precise_mode: bool,
    /// Re-OCR dense pages (≥ `dense_cells` table cells) at the fine-print budget.
    pub dense_reocr: bool,
    pub dense_cells: u16,
}

impl Default for ProcessingSettings {
    fn default() -> Self {
        Self {
            auto_level: AutoProcessingLevel::ClassifyOnly,
            inbox_auto_level: AutoProcessingLevel::Full,
            render_dpi: 200,
            review_threshold: 0.8,
            audit_sample_percent: 5,
            organize_reviewed_only: true,
            inbox_watch: true,
            ocr_digital_pdf: false,
            kie_max_pages: 4,
            precise_mode: false,
            dense_reocr: true,
            dense_cells: 80,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheSettings {
    pub page_cache_max_gb: f32,
    pub keep_thumbnails: bool,
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self { page_cache_max_gb: 5.0, keep_thumbnails: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacySettings {
    /// Store only masked forms of resident registration numbers etc. (design §3.8).
    pub pii_masking: bool,
    /// Optional whole-database encryption (SQLCipher + DPAPI key) — later phase.
    pub db_encryption: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self { pii_masking: true, db_encryption: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    pub job_complete: bool,
    pub prevent_sleep_during_jobs: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self { job_complete: true, prevent_sleep_during_jobs: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl Default for WindowState {
    fn default() -> Self {
        Self { width: 1280, height: 800, maximized: false, x: None, y: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub path: PathBuf,
    pub name: String,
    #[serde(default)]
    pub last_opened: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Knowledge base (design §3.14)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgeSettings {
    /// Chunk + index documents automatically after import / rescan / re-render.
    pub auto_index: bool,
    /// Embed new chunks in the background whenever the embedding model is installed.
    pub auto_embed: bool,
    /// Embedding model id (None = the registry's `default_embedding`). An id the
    /// registry / drop-in scan no longer knows (e.g. a model removed in 0.4.0) is
    /// ignored by the shell, which then uses the default.
    pub embedding_model: Option<String>,
    /// Hide superseded document editions from search and answers.
    pub latest_only: bool,
    /// Let the model answer from general knowledge when the knowledge base has no evidence.
    pub allow_general_knowledge: bool,
    /// Token budget for evidence chunks in one answer.
    pub context_tokens: u32,
    /// Max evidence chunks per answer.
    pub top_k: u32,
    /// Target chunk size in tokens (400–800 per design).
    pub chunk_target_tokens: u32,
    /// Vector-only hits below this cosine similarity are not evidence (≈ 0.5 for the
    /// shipped embedding model, EmbeddingGemma 2; re-check on the user's PC after a model change).
    pub min_vector_similarity: f32,
}

impl Default for KnowledgeSettings {
    fn default() -> Self {
        Self {
            auto_index: true,
            auto_embed: true,
            embedding_model: None,
            latest_only: true,
            allow_general_knowledge: false,
            context_tokens: 6_000,
            top_k: 8,
            chunk_target_tokens: 500,
            min_vector_similarity: 0.5,
        }
    }
}

impl KnowledgeSettings {
    /// Keeps the knobs inside the ranges the answer / chunker code supports.
    pub fn normalize(&mut self) {
        self.context_tokens = self.context_tokens.clamp(1_000, 24_000);
        self.top_k = self.top_k.clamp(1, 32);
        self.chunk_target_tokens = self.chunk_target_tokens.clamp(120, 2_000);
        if !self.min_vector_similarity.is_finite() {
            self.min_vector_similarity = 0.5;
        }
        self.min_vector_similarity = self.min_vector_similarity.clamp(0.0, 1.0);
    }
}

// ---------------------------------------------------------------------------
// Document generation (0.3.0 item 4)
// ---------------------------------------------------------------------------

/// Reply-token caps of the 문서 생성 screen by engine backend (user decision 2026-11-10: GPU raised,
/// CPU limited). A request's `max_tokens` is clamped to the active backend's cap; budgets above
/// `generate::prompts::SPLIT_THRESHOLD_TOKENS` run the outline → sections flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenerationSettings {
    /// Cap when llama-server runs on CUDA / Vulkan.
    pub max_tokens_gpu: u32,
    /// Cap on a CPU-only engine (a "문서 한 통" takes hours there).
    pub max_tokens_cpu: u32,
}

pub const GENERATION_GPU_CAP_MAX: u32 = 65_536;
pub const GENERATION_CPU_CAP_MAX: u32 = 12_288;

impl Default for GenerationSettings {
    fn default() -> Self {
        Self { max_tokens_gpu: 32_768, max_tokens_cpu: 8_192 }
    }
}

impl GenerationSettings {
    pub fn normalize(&mut self) {
        self.max_tokens_gpu = self.max_tokens_gpu.clamp(1_200, GENERATION_GPU_CAP_MAX);
        self.max_tokens_cpu = self.max_tokens_cpu.clamp(1_200, GENERATION_CPU_CAP_MAX);
    }

    /// The cap that applies to the running engine.
    pub fn cap(&self, gpu: bool) -> u32 {
        if gpu {
            self.max_tokens_gpu
        } else {
            self.max_tokens_cpu
        }
    }
}

// ---------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub schema_version: u32,
    /// Absolute path of the workspace to open at startup (decision 14).
    pub active_workspace: Option<PathBuf>,
    /// Recent workspaces, most recent first.
    pub workspaces: Vec<WorkspaceEntry>,
    pub ui: UiSettings,
    pub models: ModelSettings,
    pub processing: ProcessingSettings,
    pub cache: CacheSettings,
    pub privacy: PrivacySettings,
    pub notifications: NotificationSettings,
    pub knowledge: KnowledgeSettings,
    pub generation: GenerationSettings,
    /// Action id → key chord (e.g. `"command_palette": "Ctrl+K"`).
    pub shortcuts: BTreeMap<String, String>,
    pub window: WindowState,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            active_workspace: None,
            workspaces: Vec::new(),
            ui: UiSettings::default(),
            models: ModelSettings::default(),
            processing: ProcessingSettings::default(),
            cache: CacheSettings::default(),
            privacy: PrivacySettings::default(),
            notifications: NotificationSettings::default(),
            knowledge: KnowledgeSettings::default(),
            generation: GenerationSettings::default(),
            shortcuts: default_shortcuts(),
            window: WindowState::default(),
        }
    }
}

/// Default key bindings (design §3.7.4). Keys are stable action ids used by the UI.
pub fn default_shortcuts() -> BTreeMap<String, String> {
    [
        ("command_palette", "Ctrl+K"),
        ("toggle_theme", "Ctrl+Shift+T"),
        ("execute_plan", "Ctrl+Enter"),
        ("close", "Escape"),
        ("review_approve", "Enter"),
        ("review_edit", "E"),
        ("review_hold", "S"),
        ("review_next", "N"),
        ("explain_selection", "Ctrl+E"),
        ("undo", "Ctrl+Z"),
        ("redo", "Ctrl+Y"),
        ("zoom_in", "Ctrl+="),
        ("zoom_out", "Ctrl+-"),
        ("zoom_reset", "Ctrl+0"),
        ("ui_scale_up", "Ctrl+Shift+="),
        ("ui_scale_down", "Ctrl+Shift+-"),
        ("go_home", "Ctrl+1"),
        ("go_library", "Ctrl+2"),
        ("go_assistant", "Ctrl+3"),
        ("go_settings", "Ctrl+,"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// Outcome of loading settings, so the UI can tell the user when a corrupt
/// file was moved aside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoadStatus {
    Loaded,
    Created,
    /// The file is valid JSON but some sections held invalid values; those
    /// sections were reset to defaults and the rest was kept.
    PartiallyRecovered {
        sections: Vec<String>,
    },
    /// The file could not be parsed; it was renamed to `*.broken-<timestamp>`.
    RecoveredFromCorrupt {
        backup: PathBuf,
    },
}

impl AppSettings {
    /// Loads settings from `path`, applying defaults for missing fields and
    /// clamping out-of-range values. Never fails on a corrupt file: invalid
    /// JSON is moved aside, and an invalid value only resets its own section.
    pub fn load(path: &Path) -> Result<(AppSettings, LoadStatus)> {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(value) => {
                    let (mut s, bad) = AppSettings::from_value(value);
                    s.migrate();
                    s.normalize();
                    if bad.is_empty() {
                        Ok((s, LoadStatus::Loaded))
                    } else {
                        log::warn!("settings.json: sections reset to defaults: {bad:?}");
                        Ok((s, LoadStatus::PartiallyRecovered { sections: bad }))
                    }
                }
                Err(e) => {
                    let backup = path.with_extension(format!("json.broken-{}", Utc::now().format("%Y%m%d%H%M%S")));
                    log::warn!("settings.json is corrupt ({e}); moving to {}", backup.display());
                    std::fs::rename(path, &backup).map_err(|e| CoreError::io(path, e))?;
                    Ok((AppSettings::default(), LoadStatus::RecoveredFromCorrupt { backup }))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((AppSettings::default(), LoadStatus::Created)),
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    /// Lenient deserialisation: each top-level section is parsed on its own
    /// and falls back to its default when invalid. Returns the names of the
    /// sections that were reset.
    pub fn from_value(value: serde_json::Value) -> (AppSettings, Vec<String>) {
        fn take<T: serde::de::DeserializeOwned>(
            map: &serde_json::Map<String, serde_json::Value>,
            key: &str,
            target: &mut T,
            bad: &mut Vec<String>,
        ) {
            if let Some(v) = map.get(key) {
                match serde_json::from_value::<T>(v.clone()) {
                    Ok(parsed) => *target = parsed,
                    Err(e) => {
                        log::warn!("settings.json: invalid `{key}` ({e}); using defaults");
                        bad.push(key.to_string());
                    }
                }
            }
        }
        let mut out = AppSettings::default();
        let mut bad = Vec::new();
        let serde_json::Value::Object(map) = value else {
            return (out, vec!["root".to_string()]);
        };
        take(&map, "schema_version", &mut out.schema_version, &mut bad);
        take(&map, "active_workspace", &mut out.active_workspace, &mut bad);
        take(&map, "workspaces", &mut out.workspaces, &mut bad);
        take(&map, "ui", &mut out.ui, &mut bad);
        take(&map, "models", &mut out.models, &mut bad);
        take(&map, "processing", &mut out.processing, &mut bad);
        take(&map, "cache", &mut out.cache, &mut bad);
        take(&map, "privacy", &mut out.privacy, &mut bad);
        take(&map, "notifications", &mut out.notifications, &mut bad);
        take(&map, "knowledge", &mut out.knowledge, &mut bad);
        take(&map, "generation", &mut out.generation, &mut bad);
        take(&map, "shortcuts", &mut out.shortcuts, &mut bad);
        take(&map, "window", &mut out.window, &mut bad);
        (out, bad)
    }

    /// Atomically writes settings (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(self).map_err(|e| CoreError::json(path, e))?;
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp).map_err(|e| CoreError::io(&tmp, e))?;
            f.write_all(&json).map_err(|e| CoreError::io(&tmp, e))?;
            f.sync_all().map_err(|e| CoreError::io(&tmp, e))?;
        }
        std::fs::rename(&tmp, path).map_err(|e| CoreError::io(path, e))?;
        Ok(())
    }

    /// Upgrades older schema versions in place (none yet).
    pub fn migrate(&mut self) {
        if self.schema_version < SETTINGS_SCHEMA_VERSION {
            self.schema_version = SETTINGS_SCHEMA_VERSION;
        }
    }

    /// Clamps numeric ranges, fills missing shortcuts, de-duplicates workspaces.
    pub fn normalize(&mut self) {
        self.knowledge.normalize();
        self.generation.normalize();
        self.ui.scale_percent = self.ui.scale_percent.clamp(UI_SCALE_MIN, UI_SCALE_MAX);
        self.processing.render_dpi = self.processing.render_dpi.clamp(72, 600);
        self.processing.review_threshold = self.processing.review_threshold.clamp(0.0, 1.0);
        self.processing.audit_sample_percent = self.processing.audit_sample_percent.min(100);
        self.processing.kie_max_pages = self.processing.kie_max_pages.clamp(1, 32);
        self.processing.dense_cells = self.processing.dense_cells.clamp(10, 2000);
        self.models.resident_models_max = self.models.resident_models_max.clamp(1, 4);
        self.cache.page_cache_max_gb = self.cache.page_cache_max_gb.clamp(0.5, 1000.0);
        if self.models.active_model.trim().is_empty() {
            self.models.active_model = "auto".to_string();
        }
        for (k, v) in default_shortcuts() {
            self.shortcuts.entry(k).or_insert(v);
        }
        let mut seen = std::collections::HashSet::new();
        self.workspaces.retain(|w| seen.insert(w.path.clone()));
        if self.window.width < 640 || self.window.height < 480 {
            self.window = WindowState::default();
        }
    }

    /// Marks `path` as the active workspace and moves it to the front of the
    /// recent list (inserting it if needed).
    pub fn remember_workspace(&mut self, path: &Path, name: &str) {
        let now = Some(Utc::now());
        self.workspaces.retain(|w| w.path != path);
        self.workspaces
            .insert(0, WorkspaceEntry { path: path.to_path_buf(), name: name.to_string(), last_opened: now });
        self.workspaces.truncate(20);
        self.active_workspace = Some(path.to_path_buf());
    }

    pub fn forget_workspace(&mut self, path: &Path) {
        self.workspaces.retain(|w| w.path != path);
        if self.active_workspace.as_deref() == Some(path) {
            self.active_workspace = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_and_language_parse_korean_and_english() {
        assert_eq!(Theme::parse("다크"), Some(Theme::Dark));
        assert_eq!(Theme::parse("High Contrast"), Some(Theme::HighContrast));
        assert_eq!(Theme::parse("고대비"), Some(Theme::HighContrast));
        assert_eq!(Theme::parse("nope"), None);
        assert_eq!(Language::parse("영어"), Some(Language::En));
        assert_eq!(Language::parse("Korean"), Some(Language::Ko));
        assert_eq!(serde_json::to_string(&Theme::HighContrast).unwrap(), "\"high-contrast\"");
        assert_eq!(serde_json::to_string(&Language::Ko).unwrap(), "\"ko\"");
    }

    #[test]
    fn toggle_from_system_depends_on_os() {
        assert_eq!(Theme::System.toggled(true), Theme::Light);
        assert_eq!(Theme::System.toggled(false), Theme::Dark);
        assert_eq!(Theme::Dark.toggled(false), Theme::Light);
    }

    #[test]
    fn defaults_are_korean_system_theme() {
        let s = AppSettings::default();
        assert_eq!(s.ui.language, Language::Ko);
        assert_eq!(s.ui.theme, Theme::System);
        assert_eq!(s.models.active_model, "auto");
        assert_eq!(s.shortcuts.get("command_palette").map(String::as_str), Some("Ctrl+K"));
    }

    #[test]
    fn invalid_value_resets_only_its_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"active_workspace":"C:\\Work","ui":{"theme":"blue"},"models":{"manual_tier":"8gb","active_model":"x"},"processing":{"render_dpi":300}}"#,
        )
        .unwrap();
        let (s, status) = AppSettings::load(&path).unwrap();
        assert_eq!(status, LoadStatus::PartiallyRecovered { sections: vec!["ui".into(), "models".into()] });
        assert_eq!(s.active_workspace.as_deref(), Some(Path::new("C:\\Work")));
        assert_eq!(s.processing.render_dpi, 300);
        assert_eq!(s.ui, UiSettings::default());
        assert_eq!(s.models, ModelSettings::default());
        assert!(path.exists(), "a partially valid file is kept, not moved aside");
    }

    #[test]
    fn partial_file_takes_defaults_and_ignores_unknown_fields() {
        let json = r#"{"ui":{"theme":"dark"},"future_field":123,"processing":{"review_threshold":5}}"#;
        let mut s: AppSettings = serde_json::from_str(json).unwrap();
        s.normalize();
        assert_eq!(s.ui.theme, Theme::Dark);
        assert_eq!(s.ui.language, Language::Ko);
        assert_eq!(s.processing.review_threshold, 1.0);
        assert_eq!(s.ui.scale_percent, 100);
    }

    /// A settings object in which every top-level section differs from the defaults (and stays
    /// inside the ranges `normalize` clamps to, so a load returns it unchanged).
    fn non_default_settings() -> AppSettings {
        let mut s = AppSettings {
            schema_version: SETTINGS_SCHEMA_VERSION + 1,
            ui: UiSettings {
                theme: Theme::Dark,
                language: Language::En,
                scale_percent: 130,
                reduced_motion: ReducedMotion::On,
                sidebar_collapsed: true,
            },
            models: ModelSettings {
                tier_mode: TierMode::Manual,
                manual_tier: Some(Tier::Vram12gb),
                active_model: "gemma4-e4b-q4km".into(),
                profile: Profile::Speed,
                backend: BackendPreference::Vulkan,
                resident_models_max: 3,
                sleep_idle_seconds: 60,
                allow_online_download: false,
                auto_start_engine: false,
                warmup_after_load: false,
                vision_on_cpu: true,
                engine_binary: Some(PathBuf::from("/opt/llama/llama-server")),
            },
            processing: ProcessingSettings {
                auto_level: AutoProcessingLevel::Full,
                inbox_auto_level: AutoProcessingLevel::None,
                render_dpi: 300,
                review_threshold: 0.65,
                audit_sample_percent: 12,
                organize_reviewed_only: false,
                inbox_watch: false,
                ocr_digital_pdf: true,
                kie_max_pages: 9,
                precise_mode: true,
                dense_reocr: false,
                dense_cells: 120,
            },
            cache: CacheSettings { page_cache_max_gb: 42.5, keep_thumbnails: false },
            privacy: PrivacySettings { pii_masking: false, db_encryption: true },
            notifications: NotificationSettings { job_complete: false, prevent_sleep_during_jobs: true },
            knowledge: KnowledgeSettings {
                auto_index: false,
                auto_embed: false,
                embedding_model: Some("embed-gemma2-q8".into()),
                latest_only: false,
                allow_general_knowledge: true,
                context_tokens: 9_000,
                top_k: 12,
                chunk_target_tokens: 640,
                min_vector_similarity: 0.35,
            },
            generation: GenerationSettings { max_tokens_gpu: 24_000, max_tokens_cpu: 4_096 },
            window: WindowState { width: 1600, height: 900, maximized: true, x: Some(40), y: Some(-8) },
            ..Default::default()
        };
        s.shortcuts.insert("command_palette".into(), "Ctrl+P".into());
        s.remember_workspace(Path::new("/w/one"), "one");
        s
    }

    /// USR-4: every section the file can hold must survive save → load. `from_value` reads the
    /// sections one by one, so a section it forgets silently returns to its defaults on every start
    /// (0.7.0 lost `knowledge` and `generation` this way).
    #[test]
    fn every_section_survives_a_save_and_load() {
        let s = non_default_settings();
        // The fixture really covers every section: a section added later fails here until it gets
        // a non-default value above (and with it a place in this round trip).
        let ours = serde_json::to_value(&s).unwrap();
        let defaults = serde_json::to_value(AppSettings::default()).unwrap();
        for (key, default_value) in defaults.as_object().unwrap() {
            assert_ne!(ours.get(key), Some(default_value), "the fixture leaves `{key}` at its default");
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        s.save(&path).unwrap();
        let (loaded, status) = AppSettings::load(&path).unwrap();
        assert_eq!(status, LoadStatus::Loaded);
        let loaded_json = serde_json::to_value(&loaded).unwrap();
        for (key, saved_value) in ours.as_object().unwrap() {
            assert_eq!(loaded_json.get(key), Some(saved_value), "section `{key}` did not survive the reload");
        }
        assert_eq!(loaded, s);
        // The lenient loader and plain serde agree on which sections exist.
        let (lenient, bad) = AppSettings::from_value(ours.clone());
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(lenient, serde_json::from_value::<AppSettings>(ours).unwrap());
    }

    #[test]
    fn save_load_roundtrip_and_corrupt_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("DocuLens").join("settings.json");
        let (s, status) = AppSettings::load(&path).unwrap();
        assert_eq!(status, LoadStatus::Created);
        let mut s = s;
        s.ui.theme = Theme::HighContrast;
        s.remember_workspace(Path::new("/w/one"), "one");
        s.remember_workspace(Path::new("/w/two"), "two");
        s.remember_workspace(Path::new("/w/one"), "one again");
        s.save(&path).unwrap();
        let (loaded, status) = AppSettings::load(&path).unwrap();
        assert_eq!(status, LoadStatus::Loaded);
        assert_eq!(loaded, s);
        assert_eq!(loaded.workspaces.len(), 2);
        assert_eq!(loaded.workspaces[0].name, "one again");
        assert_eq!(loaded.active_workspace.as_deref(), Some(Path::new("/w/one")));

        std::fs::write(&path, b"{ not json").unwrap();
        let (recovered, status) = AppSettings::load(&path).unwrap();
        assert_eq!(recovered, AppSettings::default());
        assert!(matches!(status, LoadStatus::RecoveredFromCorrupt { .. }));
        assert!(!path.exists());
    }
}
