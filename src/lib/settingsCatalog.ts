// Settings catalog of the planner's generic `set_setting` (0.6.0 plan §1.5): every editable field
// of `AppSettings` by its path ("processing.render_dpi"), with the type / range the UI action uses
// to coerce a spoken value ("125%", "켜줘", "accuracy"). `journal_days` is a retention period and
// keeps its own branch in `commands.ts`.
//
// The keys, types and ranges come from `settingsCatalog.json`, which the core writes from its own
// table (`crates/doculens-core/src/planner/settings_catalog.rs`; a core test fails when the file
// is stale). Until 0.8.0 the ranges were typed in twice and disagreed (audit CON-8). Only what
// the core does not know lives here: the i18n label keys, spoken aliases of enum values and which
// text settings may be cleared.

import catalog from "./settingsCatalog.json";
import type { AppSettings } from "./types";

export type SettingType =
  | { kind: "boolean" }
  | { kind: "integer"; min: number; max: number; choices?: readonly number[] }
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

/** One row of `settingsCatalog.json`. */
interface CatalogRow {
  key: string;
  confirm: boolean;
  type: {
    kind: string;
    min?: number;
    max?: number;
    choices?: number[];
    values?: string[];
    percent?: boolean;
  };
}

const AUTO_LEVEL_ALIASES = { off: "none", classify: "classify_only", ocr: "classify_and_ocr", extract: "full", all: "full" } as const;
/** How people say enum values that are not the value itself. */
const ENUM_ALIASES: Readonly<Record<string, Readonly<Record<string, string>>>> = {
  "ui.language": { korean: "ko", 한국어: "ko", english: "en", 영어: "en" },
  "processing.auto_level": AUTO_LEVEL_ALIASES,
  "processing.inbox_auto_level": AUTO_LEVEL_ALIASES,
};
/** Text settings that may be cleared ("기본값으로"). */
const NULLABLE_STRINGS: ReadonlySet<string> = new Set(["models.engine_binary", "knowledge.embedding_model"]);

function typeOf(row: CatalogRow): SettingType {
  const t = row.type;
  switch (t.kind) {
    case "boolean":
      return { kind: "boolean" };
    case "integer": {
      const choices = t.choices;
      if (choices && choices.length > 0) {
        return { kind: "integer", min: Math.min(...choices), max: Math.max(...choices), choices };
      }
      return { kind: "integer", min: t.min ?? 0, max: t.max ?? Number.MAX_SAFE_INTEGER };
    }
    case "number":
      return { kind: "number", min: t.min ?? 0, max: t.max ?? Number.MAX_VALUE };
    case "enum":
      return { kind: "enum", values: t.values ?? [], aliases: ENUM_ALIASES[row.key] };
    default:
      return NULLABLE_STRINGS.has(row.key) ? { kind: "string", nullable: true } : { kind: "string" };
  }
}

function spec(row: CatalogRow): SettingSpec {
  return { key: row.key, type: typeOf(row), label: `planner.settings.${row.key.replace(".", "_")}`, confirm: row.confirm };
}

/** Every `AppSettings` field the assistant may change (`journal_days` is not one: see `commands.ts`). */
export const SETTINGS_CATALOG: readonly SettingSpec[] = (catalog as CatalogRow[]).filter((row) => row.key.includes(".")).map(spec);

export const SETTING_CATALOG_KEYS: readonly string[] = SETTINGS_CATALOG.map((s) => s.key);

/** Every key the grammar accepts: the catalog paths and `journal_days` (a retention period, not a settings field). */
export const SETTING_KEYS_ALL: readonly string[] = SETTING_CATALOG_KEYS.includes("journal_days") ? SETTING_CATALOG_KEYS : [...SETTING_CATALOG_KEYS, "journal_days"];

/** The key as the catalog writes it. */
export function resolveSettingKey(key: string): string {
  return key.trim();
}

export function settingSpec(key: string): SettingSpec | null {
  const path = resolveSettingKey(key);
  return SETTINGS_CATALOG.find((s) => s.key === path) ?? null;
}

/**
 * The range of an integer setting, as the core's catalog has it — the settings screen takes the limits of
 * its number fields from here instead of repeating them (the screen said 1–20 for a setting the core took to 32).
 */
export function integerRange(key: string): { min: number; max: number } {
  const type = settingSpec(key)?.type;
  if (type?.kind !== "integer") throw new Error(`${key} is not an integer setting`);
  return { min: type.min, max: type.max };
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
      if (t.kind === "integer" && t.choices && !t.choices.includes(n)) {
        throw new Error(`${spec.key} must be one of ${t.choices.join(", ")}`);
      }
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
