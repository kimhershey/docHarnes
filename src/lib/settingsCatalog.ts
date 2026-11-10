// Settings catalog of the planner's generic `set_setting` (0.6.0 plan §1.5): every editable field
// of `AppSettings` by its path ("processing.render_dpi"), with the type / range the validator and
// the UI action use to coerce a spoken value ("125%", "켜줘", "accuracy"). The pre-0.6.0 keys
// (`render_dpi`, `pii_masking`, …) stay valid as aliases; `journal_days` is a retention period and
// keeps its own branch in `commands.ts`. Mirrors `crates/doculens-core/src/settings/catalog.rs`.

import type { AppSettings, LegacySettingKey } from "./types";

export type SettingType =
  | { kind: "boolean" }
  | { kind: "integer"; min: number; max: number }
  | { kind: "number"; min: number; max: number }
  | { kind: "enum"; values: readonly string[]; aliases?: Readonly<Record<string, string>> }
  | { kind: "string"; nullable?: boolean };

export interface SettingSpec {
  /** Path inside `AppSettings` (`section.field`). */
  key: string;
  type: SettingType;
  /** i18n key of the label shown on cards and toasts. */
  label: string;
  /** Shown on the confirmation card before it is applied (paths, encryption). */
  confirm?: boolean;
}

const THEMES = ["light", "dark", "system", "high-contrast"] as const;
const LANGS = ["ko", "en"] as const;
const REDUCED_MOTION = ["system", "on", "off"] as const;
const TIER_MODES = ["auto", "manual"] as const;
const TIERS = ["cpu_only", "vram_6gb", "vram_8gb", "vram_12gb", "vram_16gb", "vram_24gb_plus"] as const;
const PROFILES = ["accuracy", "balanced", "speed", "fine_print"] as const;
const BACKENDS = ["auto", "cuda", "vulkan", "cpu"] as const;
const AUTO_LEVELS = ["none", "classify_only", "classify_and_ocr", "full"] as const;
const AUTO_LEVEL_ALIASES = { off: "none", classify: "classify_only", ocr: "classify_and_ocr", extract: "full", all: "full" } as const;

const bool: SettingType = { kind: "boolean" };
const int = (min: number, max: number): SettingType => ({ kind: "integer", min, max });
const num = (min: number, max: number): SettingType => ({ kind: "number", min, max });
const en = (values: readonly string[], aliases?: Readonly<Record<string, string>>): SettingType => ({ kind: "enum", values, aliases });

function spec(key: string, type: SettingType, confirm = false): SettingSpec {
  return { key, type, label: `planner.settings.${key.replace(".", "_")}`, confirm };
}

export const SETTINGS_CATALOG: readonly SettingSpec[] = [
  // ui
  spec("ui.theme", en(THEMES)),
  spec("ui.language", en(LANGS, { korean: "ko", 한국어: "ko", english: "en", 영어: "en" })),
  spec("ui.scale_percent", int(90, 150)),
  spec("ui.reduced_motion", en(REDUCED_MOTION)),
  spec("ui.sidebar_collapsed", bool),
  // models
  spec("models.tier_mode", en(TIER_MODES)),
  spec("models.manual_tier", en(TIERS)),
  spec("models.active_model", { kind: "string" }),
  spec("models.profile", en(PROFILES)),
  spec("models.backend", en(BACKENDS)),
  spec("models.resident_models_max", int(1, 4)),
  spec("models.sleep_idle_seconds", int(0, 86_400)),
  spec("models.allow_online_download", bool),
  spec("models.auto_start_engine", bool),
  spec("models.warmup_after_load", bool),
  spec("models.vision_on_cpu", bool),
  spec("models.engine_binary", { kind: "string", nullable: true }, true),
  // processing
  spec("processing.auto_level", en(AUTO_LEVELS, AUTO_LEVEL_ALIASES)),
  spec("processing.inbox_auto_level", en(AUTO_LEVELS, AUTO_LEVEL_ALIASES)),
  spec("processing.render_dpi", int(72, 600)),
  spec("processing.review_threshold", num(0, 1)),
  spec("processing.audit_sample_percent", int(0, 100)),
  spec("processing.organize_reviewed_only", bool),
  spec("processing.inbox_watch", bool),
  spec("processing.ocr_digital_pdf", bool),
  spec("processing.kie_max_pages", int(1, 50)),
  spec("processing.precise_mode", bool),
  spec("processing.dense_reocr", bool),
  spec("processing.dense_cells", int(1, 10_000)),
  // cache
  spec("cache.page_cache_max_gb", num(0.5, 1000)),
  spec("cache.keep_thumbnails", bool),
  // privacy
  spec("privacy.pii_masking", bool),
  spec("privacy.db_encryption", bool, true),
  // notifications
  spec("notifications.job_complete", bool),
  spec("notifications.prevent_sleep_during_jobs", bool),
  // knowledge
  spec("knowledge.auto_index", bool),
  spec("knowledge.auto_embed", bool),
  spec("knowledge.embedding_model", { kind: "string", nullable: true }),
  spec("knowledge.latest_only", bool),
  spec("knowledge.allow_general_knowledge", bool),
  spec("knowledge.context_tokens", int(512, 32_768)),
  spec("knowledge.top_k", int(1, 50)),
  spec("knowledge.chunk_target_tokens", int(100, 2000)),
  spec("knowledge.min_vector_similarity", num(0, 1)),
  // generation
  spec("generation.max_tokens_gpu", int(256, 65_536)),
  spec("generation.max_tokens_cpu", int(256, 12_288)),
];

export const SETTING_CATALOG_KEYS: readonly string[] = SETTINGS_CATALOG.map((s) => s.key);

/** Pre-0.6.0 keys → catalog paths (`journal_days` is a retention period, not a settings field). */
export const LEGACY_SETTING_KEYS: Readonly<Record<LegacySettingKey, string | null>> = {
  render_dpi: "processing.render_dpi",
  pii_masking: "privacy.pii_masking",
  auto_pipeline: "processing.auto_level",
  inbox_watch: "processing.inbox_watch",
  review_threshold: "processing.review_threshold",
  cache_limit_gb: "cache.page_cache_max_gb",
  journal_days: null,
};

/** Every key the grammar accepts: catalog paths + legacy aliases. */
export const SETTING_KEYS_ALL: readonly string[] = [...SETTING_CATALOG_KEYS, ...Object.keys(LEGACY_SETTING_KEYS)];

/** Catalog path of a key (legacy aliases resolved); `journal_days` and unknown keys come back unchanged. */
export function resolveSettingKey(key: string): string {
  const k = key.trim();
  const legacy = (LEGACY_SETTING_KEYS as Record<string, string | null>)[k];
  return legacy ?? k;
}

export function settingSpec(key: string): SettingSpec | null {
  const path = resolveSettingKey(key);
  return SETTINGS_CATALOG.find((s) => s.key === path) ?? null;
}

const TRUE_WORDS = ["true", "1", "on", "yes", "y", "켜", "켬", "켜기", "켜줘", "사용", "허용", "enable", "enabled"];
const FALSE_WORDS = ["false", "0", "off", "no", "n", "꺼", "끔", "끄기", "꺼줘", "사용안함", "해제", "disable", "disabled", "none"];

/**
 * Coerces a spoken / typed value to the field's type ("125%" → 125, "켜줘" → true, "classify" →
 * "classify_only"); a percentage given for a 0–1 ratio is divided by 100. Throws with a short
 * English reason when the value cannot be read or is out of range (the caller translates).
 */
export function coerceSettingValue(spec: SettingSpec, value: unknown): string | number | boolean | null {
  const t = spec.type;
  switch (t.kind) {
    case "boolean": {
      if (typeof value === "boolean") return value;
      if (typeof value === "number") return value !== 0;
      const s = String(value).trim().toLowerCase();
      if (TRUE_WORDS.includes(s)) return true;
      if (FALSE_WORDS.includes(s)) return false;
      throw new Error(`${spec.key} must be true or false`);
    }
    case "integer":
    case "number": {
      const raw = typeof value === "number" ? value : Number(String(value).trim().replace(/%$/, "").replace(/[^0-9.-]/g, ""));
      if (!Number.isFinite(raw)) throw new Error(`${spec.key} must be a number`);
      let n = raw;
      // A ratio spoken as a percentage ("검토 임계값 90%").
      if (t.max === 1 && t.min === 0 && n > 1 && n <= 100) n /= 100;
      if (t.kind === "integer") n = Math.round(n);
      if (n < t.min || n > t.max) throw new Error(`${spec.key} must be between ${t.min} and ${t.max}`);
      return n;
    }
    case "enum": {
      const s = String(value).trim().toLowerCase().replace(/\s+/g, "_");
      const hit = t.values.find((v) => v.toLowerCase() === s) ?? (t.aliases ? t.aliases[s] : undefined);
      if (!hit) throw new Error(`${spec.key} must be one of ${t.values.join(", ")}`);
      return hit;
    }
    case "string": {
      if (value == null || value === "") {
        if (t.nullable) return null;
        throw new Error(`${spec.key} needs a value`);
      }
      const s = String(value).trim();
      if (t.nullable && ["none", "null", "default", "기본", "기본값", "없음"].includes(s.toLowerCase())) return null;
      return s;
    }
  }
}

/** Writes `value` at `path` ("section.field") of a settings draft. */
export function setSettingByPath(settings: AppSettings, path: string, value: unknown): void {
  const [section, field] = path.split(".");
  if (!section || !field) throw new Error(`unknown setting: ${path}`);
  const target = (settings as unknown as Record<string, Record<string, unknown>>)[section];
  if (!target || typeof target !== "object" || !(field in target)) throw new Error(`unknown setting: ${path}`);
  target[field] = value;
}

/** Reads the current value at `path`. */
export function getSettingByPath(settings: AppSettings, path: string): unknown {
  const [section, field] = path.split(".");
  if (!section || !field) return undefined;
  const target = (settings as unknown as Record<string, Record<string, unknown>>)[section];
  return target && typeof target === "object" ? target[field] : undefined;
}
