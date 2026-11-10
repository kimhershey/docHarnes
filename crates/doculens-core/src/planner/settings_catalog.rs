//! The settings the assistant may change with `set_setting` (0.6.0 plan §1.5): one entry per
//! editable field of `settings::AppSettings`, keyed by its path (`processing.render_dpi`), with
//! the value type the validator coerces to and the labels the cards show. The 0.1–0.5 short keys
//! (`render_dpi` …) remain as aliases. Window geometry, the workspace list and the shortcut map are
//! not commands. The frontend mirrors this table (`src/lib/settingsCatalog.ts`) to apply the value.

use serde_json::Value;

use super::schema::{LANGS, PROFILES, THEMES};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueType {
    Bool,
    /// Integer within `min..=max`.
    Int { min: i64, max: i64 },
    /// One of a few integers (`render_dpi`).
    IntChoice(&'static [i64]),
    /// Number within `min..=max`; `percent` also accepts "90%" / 90 for a 0..1 field.
    Float { min: f64, max: f64, percent: bool },
    Enum(&'static [&'static str]),
    /// Free text (model ids, paths); `none` / empty clears an optional field.
    Str,
}

#[derive(Debug, Clone, Copy)]
pub struct SettingSpec {
    pub key: &'static str,
    pub ty: ValueType,
    pub label_ko: &'static str,
    pub label_en: &'static str,
    /// Shown on a confirmation card before it changes (engine binary, encryption).
    pub confirm: bool,
}

const fn s(key: &'static str, ty: ValueType, label_ko: &'static str, label_en: &'static str) -> SettingSpec {
    SettingSpec { key, ty, label_ko, label_en, confirm: false }
}

const fn sc(key: &'static str, ty: ValueType, label_ko: &'static str, label_en: &'static str) -> SettingSpec {
    SettingSpec { key, ty, label_ko, label_en, confirm: true }
}

pub const REDUCED_MOTION: &[&str] = &["system", "on", "off"];
pub const TIER_MODES: &[&str] = &["auto", "manual"];
pub const TIERS: &[&str] = &["cpu_only", "vram_6gb", "vram_8gb", "vram_12gb", "vram_16gb", "vram_24gb_plus"];
pub const BACKENDS: &[&str] = &["auto", "cuda", "vulkan", "cpu"];
pub const AUTO_LEVELS: &[&str] = &["none", "classify_only", "classify_and_ocr", "full"];

pub const SETTINGS: &[SettingSpec] = &[
    // ui
    s("ui.theme", ValueType::Enum(THEMES), "테마", "theme"),
    s("ui.language", ValueType::Enum(LANGS), "언어", "language"),
    s("ui.scale_percent", ValueType::Int { min: 90, max: 150 }, "UI 배율(%)", "UI scale (%)"),
    s("ui.reduced_motion", ValueType::Enum(REDUCED_MOTION), "움직임 줄이기", "reduced motion"),
    s("ui.sidebar_collapsed", ValueType::Bool, "사이드바 접기", "sidebar collapsed"),
    // models
    s("models.tier_mode", ValueType::Enum(TIER_MODES), "티어 선택(자동/수동)", "tier mode"),
    s("models.manual_tier", ValueType::Enum(TIERS), "수동 티어", "manual tier"),
    s("models.active_model", ValueType::Str, "활성 모델", "active model"),
    s("models.profile", ValueType::Enum(PROFILES), "추론 프로파일", "inference profile"),
    s("models.backend", ValueType::Enum(BACKENDS), "엔진 백엔드", "engine backend"),
    s("models.resident_models_max", ValueType::Int { min: 1, max: 4 }, "동시 상주 모델 수", "resident models"),
    s("models.sleep_idle_seconds", ValueType::Int { min: 0, max: 86_400 }, "엔진 절전 대기(초)", "engine sleep after (s)"),
    s("models.allow_online_download", ValueType::Bool, "온라인 다운로드 허용", "allow online downloads"),
    s("models.auto_start_engine", ValueType::Bool, "엔진 자동 시작", "start the engine automatically"),
    s("models.warmup_after_load", ValueType::Bool, "로드 후 워밍업", "warm up after load"),
    s("models.vision_on_cpu", ValueType::Bool, "비전 인코더 CPU 실행", "vision encoder on the CPU"),
    sc("models.engine_binary", ValueType::Str, "엔진 실행 파일 경로", "engine binary path"),
    // processing
    s("processing.auto_level", ValueType::Enum(AUTO_LEVELS), "가져온 뒤 자동 처리 수준", "processing after import"),
    s("processing.inbox_auto_level", ValueType::Enum(AUTO_LEVELS), "inbox 자동 처리 수준", "inbox processing level"),
    s("processing.render_dpi", ValueType::IntChoice(&[150, 200, 300]), "렌더 DPI", "render DPI"),
    s("processing.review_threshold", ValueType::Float { min: 0.0, max: 1.0, percent: true }, "검토 임계값", "review threshold"),
    s("processing.audit_sample_percent", ValueType::Int { min: 0, max: 100 }, "감사 표본(%)", "audit sample (%)"),
    s("processing.organize_reviewed_only", ValueType::Bool, "검토 완료 문서만 정리", "organise reviewed documents only"),
    s("processing.inbox_watch", ValueType::Bool, "inbox 감시", "inbox watching"),
    s("processing.ocr_digital_pdf", ValueType::Bool, "디지털 PDF도 OCR", "OCR digital PDFs too"),
    s("processing.kie_max_pages", ValueType::Int { min: 1, max: 50 }, "추출 최대 쪽수", "extraction page limit"),
    s("processing.precise_mode", ValueType::Bool, "정밀 모드", "precise mode"),
    s("processing.dense_reocr", ValueType::Bool, "빽빽한 표 재OCR", "re-OCR dense tables"),
    s("processing.dense_cells", ValueType::Int { min: 0, max: 10_000 }, "빽빽한 표 기준 셀 수", "dense-table cell threshold"),
    // cache
    s("cache.page_cache_max_gb", ValueType::Float { min: 0.5, max: 1000.0, percent: false }, "페이지 캐시 상한(GB)", "page cache limit (GB)"),
    s("cache.keep_thumbnails", ValueType::Bool, "썸네일 보관", "keep thumbnails"),
    // privacy
    s("privacy.pii_masking", ValueType::Bool, "개인정보 마스킹", "PII masking"),
    sc("privacy.db_encryption", ValueType::Bool, "DB 암호화", "database encryption"),
    // notifications
    s("notifications.job_complete", ValueType::Bool, "작업 완료 알림", "job-complete notification"),
    s("notifications.prevent_sleep_during_jobs", ValueType::Bool, "배치 중 절전 방지", "prevent sleep during jobs"),
    // knowledge
    s("knowledge.auto_index", ValueType::Bool, "자동 색인", "automatic indexing"),
    s("knowledge.auto_embed", ValueType::Bool, "자동 임베딩", "automatic embedding"),
    s("knowledge.embedding_model", ValueType::Str, "임베딩 모델", "embedding model"),
    s("knowledge.latest_only", ValueType::Bool, "최신본만 사용", "latest editions only"),
    s("knowledge.allow_general_knowledge", ValueType::Bool, "근거 없을 때 일반 지식 허용", "allow general knowledge"),
    s("knowledge.context_tokens", ValueType::Int { min: 512, max: 131_072 }, "근거 컨텍스트 토큰", "evidence context tokens"),
    s("knowledge.top_k", ValueType::Int { min: 1, max: 50 }, "근거 개수(top-k)", "evidence hits (top-k)"),
    s("knowledge.chunk_target_tokens", ValueType::Int { min: 100, max: 4_000 }, "청크 목표 토큰", "chunk target tokens"),
    s("knowledge.min_vector_similarity", ValueType::Float { min: 0.0, max: 1.0, percent: true }, "최소 벡터 유사도", "minimum vector similarity"),
    // generation
    s("generation.max_tokens_gpu", ValueType::Int { min: 256, max: 65_536 }, "문서 작성 토큰 상한(GPU)", "drafting token cap (GPU)"),
    s("generation.max_tokens_cpu", ValueType::Int { min: 256, max: 12_288 }, "문서 작성 토큰 상한(CPU)", "drafting token cap (CPU)"),
    // retention (workspace manifest, applied by the UI like `set_retention`)
    s("journal_days", ValueType::Int { min: 0, max: 36_500 }, "저널 보관 일수", "journal retention (days)"),
];

/// 0.1–0.5 keys → catalog paths (`journal_days` is a catalog key of its own).
pub const ALIASES: &[(&str, &str)] = &[
    ("render_dpi", "processing.render_dpi"),
    ("pii_masking", "privacy.pii_masking"),
    ("auto_pipeline", "processing.auto_level"),
    ("inbox_watch", "processing.inbox_watch"),
    ("review_threshold", "processing.review_threshold"),
    ("cache_limit_gb", "cache.page_cache_max_gb"),
];

/// Every key the grammar accepts: the catalog paths and the aliases (a literal list because the
/// grammar wants a `&'static [&'static str]`; `tests::setting_keys_match_the_catalog` keeps it true).
pub const SETTING_KEYS: &[&str] = &[
    "ui.theme",
    "ui.language",
    "ui.scale_percent",
    "ui.reduced_motion",
    "ui.sidebar_collapsed",
    "models.tier_mode",
    "models.manual_tier",
    "models.active_model",
    "models.profile",
    "models.backend",
    "models.resident_models_max",
    "models.sleep_idle_seconds",
    "models.allow_online_download",
    "models.auto_start_engine",
    "models.warmup_after_load",
    "models.vision_on_cpu",
    "models.engine_binary",
    "processing.auto_level",
    "processing.inbox_auto_level",
    "processing.render_dpi",
    "processing.review_threshold",
    "processing.audit_sample_percent",
    "processing.organize_reviewed_only",
    "processing.inbox_watch",
    "processing.ocr_digital_pdf",
    "processing.kie_max_pages",
    "processing.precise_mode",
    "processing.dense_reocr",
    "processing.dense_cells",
    "cache.page_cache_max_gb",
    "cache.keep_thumbnails",
    "privacy.pii_masking",
    "privacy.db_encryption",
    "notifications.job_complete",
    "notifications.prevent_sleep_during_jobs",
    "knowledge.auto_index",
    "knowledge.auto_embed",
    "knowledge.embedding_model",
    "knowledge.latest_only",
    "knowledge.allow_general_knowledge",
    "knowledge.context_tokens",
    "knowledge.top_k",
    "knowledge.chunk_target_tokens",
    "knowledge.min_vector_similarity",
    "generation.max_tokens_gpu",
    "generation.max_tokens_cpu",
    "journal_days",
    "render_dpi",
    "pii_masking",
    "auto_pipeline",
    "inbox_watch",
    "review_threshold",
    "cache_limit_gb",
];

/// Canonical catalog key of `key` (alias-aware, case-insensitive, `/` tolerated), with its spec.
pub fn find(key: &str) -> Option<(&'static str, &'static SettingSpec)> {
    let k = key.trim().to_lowercase().replace('/', ".");
    let canonical = ALIASES.iter().find(|(a, _)| *a == k).map(|(_, c)| *c).unwrap_or(k.as_str());
    SETTINGS.iter().find(|s| s.key == canonical).map(|s| (s.key, s))
}

pub fn needs_confirmation(key: &str) -> bool {
    find(key).is_some_and(|(_, s)| s.confirm)
}

fn truthy(s: &str) -> Option<bool> {
    match s.trim().to_lowercase().as_str() {
        "true" | "on" | "yes" | "1" | "켜기" | "켜" | "켬" | "사용" | "허용" | "enable" | "enabled" => Some(true),
        "false" | "off" | "no" | "0" | "끄기" | "꺼" | "끔" | "해제" | "금지" | "disable" | "disabled" => Some(false),
        _ => None,
    }
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str().and_then(|s| s.trim().trim_end_matches('%').replace(',', "").parse().ok()))
}

/// Coerces `value` to the spec's type in place; `Err(message)` when it cannot be a value of the key.
pub fn coerce(spec: &SettingSpec, value: &mut Value) -> std::result::Result<(), String> {
    match spec.ty {
        ValueType::Bool => {
            let b = match value {
                Value::Bool(b) => Some(*b),
                Value::String(s) => truthy(s),
                Value::Number(n) => n.as_f64().map(|f| f != 0.0),
                _ => None,
            };
            match b {
                Some(b) => {
                    *value = Value::Bool(b);
                    Ok(())
                }
                None => Err(format!("{} must be true or false", spec.key)),
            }
        }
        ValueType::Int { min, max } => match number(value) {
            Some(n) if n.fract() == 0.0 && (min as f64..=max as f64).contains(&n) => {
                *value = Value::from(n as i64);
                Ok(())
            }
            Some(n) if (min as f64..=max as f64).contains(&n.round()) => {
                *value = Value::from(n.round() as i64);
                Ok(())
            }
            _ => Err(format!("{} must be an integer between {min} and {max}", spec.key)),
        },
        ValueType::IntChoice(choices) => match number(value) {
            Some(n) if choices.contains(&(n as i64)) && n.fract() == 0.0 => {
                *value = Value::from(n as i64);
                Ok(())
            }
            _ => Err(format!(
                "{} must be one of {}",
                spec.key,
                choices.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
            )),
        },
        ValueType::Float { min, max, percent } => match number(value) {
            Some(n) if (min..=max).contains(&n) => {
                *value = Value::from(n);
                Ok(())
            }
            Some(n) if percent && (1.0..=100.0).contains(&n) && max <= 1.0 => {
                *value = Value::from(n / 100.0);
                Ok(())
            }
            _ => Err(format!("{} must be between {min} and {max}", spec.key)),
        },
        ValueType::Enum(values) => {
            let raw = match value {
                Value::String(s) => s.trim().to_lowercase().replace([' ', '-'], "_"),
                Value::Bool(b) => b.to_string(),
                other => other.to_string(),
            };
            // Korean / short forms of the common enums.
            let mapped = match (spec.key, raw.as_str()) {
                ("ui.theme", "다크" | "어두운" | "어둡게") => "dark".to_string(),
                ("ui.theme", "라이트" | "밝은" | "밝게") => "light".to_string(),
                ("ui.theme", "시스템") => "system".to_string(),
                ("ui.theme", "고대비" | "high_contrast") => "high-contrast".to_string(),
                ("ui.language", "한국어" | "한글" | "korean") => "ko".to_string(),
                ("ui.language", "영어" | "english") => "en".to_string(),
                ("models.tier_mode", "자동") => "auto".to_string(),
                ("models.tier_mode", "수동") => "manual".to_string(),
                ("models.profile", "정확" | "정확도") => "accuracy".to_string(),
                ("models.profile", "균형") => "balanced".to_string(),
                ("models.profile", "속도" | "빠르게") => "speed".to_string(),
                ("models.profile", "잔글씨") => "fine_print".to_string(),
                ("processing.auto_level" | "processing.inbox_auto_level", "classify") => "classify_only".to_string(),
                ("processing.auto_level" | "processing.inbox_auto_level", "ocr") => "classify_and_ocr".to_string(),
                ("processing.auto_level" | "processing.inbox_auto_level", "전체" | "모두") => "full".to_string(),
                ("processing.auto_level" | "processing.inbox_auto_level", "없음" | "안함" | "off") => "none".to_string(),
                _ => raw.clone(),
            };
            if values.contains(&mapped.as_str()) {
                *value = Value::String(mapped);
                Ok(())
            } else {
                Err(format!("{} must be one of {}", spec.key, values.join(", ")))
            }
        }
        ValueType::Str => {
            let text = match value {
                Value::String(s) => s.trim().to_string(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            if text.chars().count() > 500 {
                return Err(format!("{} is too long", spec.key));
            }
            *value = if text.is_empty() || text.eq_ignore_ascii_case("none") { Value::Null } else { Value::String(text) };
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn setting_keys_match_the_catalog() {
        let mut expected: Vec<&str> = SETTINGS.iter().map(|s| s.key).collect();
        expected.extend(ALIASES.iter().map(|(a, _)| *a));
        assert_eq!(SETTING_KEYS.to_vec(), expected, "SETTING_KEYS must list the catalog paths and then the aliases");
        for (alias, target) in ALIASES {
            assert!(SETTINGS.iter().any(|s| s.key == *target), "{alias} → {target} is not a catalog key");
        }
        let mut keys: Vec<&str> = SETTING_KEYS.to_vec();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), SETTING_KEYS.len(), "duplicate keys");
        // Every settings struct field the plan names has an entry (count from plan §1.5 + journal_days).
        assert_eq!(SETTINGS.len(), 47);
    }

    #[test]
    fn aliases_and_coercion() {
        let (key, spec) = find("render_dpi").unwrap();
        assert_eq!(key, "processing.render_dpi");
        let mut v = json!("300");
        coerce(spec, &mut v).unwrap();
        assert_eq!(v, json!(300));
        let mut v = json!(250);
        assert!(coerce(spec, &mut v).is_err());
        let (_, spec) = find("review_threshold").unwrap();
        let mut v = json!("90%");
        coerce(spec, &mut v).unwrap();
        assert_eq!(v, json!(0.9));
        let (_, spec) = find("UI.scale_percent").unwrap();
        let mut v = json!("125");
        coerce(spec, &mut v).unwrap();
        assert_eq!(v, json!(125));
        let mut v = json!(200);
        assert!(coerce(spec, &mut v).is_err());
        let (_, spec) = find("knowledge.auto_embed").unwrap();
        let mut v = json!("꺼");
        coerce(spec, &mut v).unwrap();
        assert_eq!(v, json!(false));
        let (_, spec) = find("ui.theme").unwrap();
        let mut v = json!("다크");
        coerce(spec, &mut v).unwrap();
        assert_eq!(v, json!("dark"));
        let (_, spec) = find("processing.auto_level").unwrap();
        let mut v = json!("classify");
        coerce(spec, &mut v).unwrap();
        assert_eq!(v, json!("classify_only"));
        let (_, spec) = find("models.engine_binary").unwrap();
        let mut v = json!("none");
        coerce(spec, &mut v).unwrap();
        assert_eq!(v, Value::Null);
        assert!(needs_confirmation("models.engine_binary") && !needs_confirmation("ui.theme"));
        assert!(find("nope.key").is_none());
    }
}
