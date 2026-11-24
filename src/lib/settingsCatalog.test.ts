import { describe, expect, it } from "vitest";

import ko from "@/i18n/ko.json";
import en from "@/i18n/en.json";
import catalog from "./settingsCatalog.json";
import { SETTINGS_CATALOG, coerceSettingValue, integerRange, settingSpec } from "./settingsCatalog";

const spec = (key: string) => {
  const s = settingSpec(key);
  if (!s) throw new Error(`no such setting: ${key}`);
  return s;
};

describe("settings catalog", () => {
  it("is built from the core's table (settingsCatalog.json)", () => {
    const rows = catalog.filter((r) => r.key.includes("."));
    expect(SETTINGS_CATALOG.map((s) => s.key)).toEqual(rows.map((r) => r.key));
    // The retention period is not an AppSettings field; commands.ts applies it on its own.
    expect(catalog.some((r) => r.key === "journal_days")).toBe(true);
    expect(settingSpec("journal_days")).toBeNull();
    expect(spec("models.engine_binary").confirm).toBe(true);
    // Nothing encrypts the database yet, so the switch is not offered (0.8.0, PLT-10).
    expect(settingSpec("privacy.db_encryption")).toBeNull();
    expect(spec("ui.scale_percent").type).toEqual({ kind: "integer", min: 90, max: 150 });
  });

  it("has a label in both languages for every setting", () => {
    const labels = (dict: unknown) => (dict as { planner: { settings: Record<string, string> } }).planner.settings;
    for (const s of SETTINGS_CATALOG) {
      const key = s.label.replace("planner.settings.", "");
      expect(labels(ko)[key], `ko ${s.label}`).toBeTruthy();
      expect(labels(en)[key], `en ${s.label}`).toBeTruthy();
    }
  });

  // Audit CON-8: these passed the planner's validator and failed (or were silently clamped) later.
  it("rejects what the core rejects", () => {
    // 1.0.6 (user decision 2026-11-24): the evidence budget goes up to the largest context a registry model takes,
    // the passage count to 200 — what a PC or a model cannot hold fails and is reported.
    expect(() => coerceSettingValue(spec("knowledge.context_tokens"), 300_000)).toThrow(/between 1000 and 262144/);
    expect(() => coerceSettingValue(spec("knowledge.context_tokens"), 999)).toThrow(/between 1000 and 262144/);
    expect(coerceSettingValue(spec("knowledge.context_tokens"), 40_000)).toBe(40_000);
    expect(coerceSettingValue(spec("knowledge.context_tokens"), 262_144)).toBe(262_144);
    expect(() => coerceSettingValue(spec("knowledge.top_k"), 201)).toThrow(/between 1 and 200/);
    expect(coerceSettingValue(spec("knowledge.top_k"), 50)).toBe(50);
    expect(coerceSettingValue(spec("knowledge.top_k"), 200)).toBe(200);
    // The chunk size stays: a chunk has to fit the embedding model's context.
    expect(() => coerceSettingValue(spec("knowledge.chunk_target_tokens"), 2_001)).toThrow(/between 120 and 2000/);
    // The settings screen's number fields take these ranges (`integerRange`).
    expect(integerRange("knowledge.context_tokens")).toEqual({ min: 1000, max: 262144 });
    expect(integerRange("knowledge.top_k")).toEqual({ min: 1, max: 200 });
    expect(integerRange("knowledge.chunk_target_tokens")).toEqual({ min: 120, max: 2000 });
    expect(() => integerRange("ui.theme")).toThrow(/not an integer/);
    expect(() => coerceSettingValue(spec("processing.render_dpi"), 250)).toThrow(/one of 150, 200, 300/);
    expect(coerceSettingValue(spec("processing.render_dpi"), "200")).toBe(200);
    expect(() => coerceSettingValue(spec("ui.scale_percent"), 200)).toThrow(/between 90 and 150/);
    expect(() => coerceSettingValue(spec("processing.kie_max_pages"), 50)).toThrow();
    expect(() => coerceSettingValue(spec("generation.max_tokens_gpu"), 256)).toThrow();
  });

  it("reads spoken values", () => {
    expect(coerceSettingValue(spec("ui.scale_percent"), "125%")).toBe(125);
    expect(coerceSettingValue(spec("processing.review_threshold"), "90%")).toBe(0.9);
    expect(coerceSettingValue(spec("privacy.pii_masking"), "꺼줘")).toBe(false);
    expect(coerceSettingValue(spec("ui.language"), "영어")).toBe("en");
    expect(coerceSettingValue(spec("processing.auto_level"), "classify")).toBe("classify_only");
    expect(coerceSettingValue(spec("knowledge.embedding_model"), "기본값")).toBeNull();
    expect(() => coerceSettingValue(spec("models.active_model"), "")).toThrow();
  });
});
