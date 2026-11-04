// Every static `t("…")` key used in the UI must resolve to a *string* in both
// languages (an object-valued key renders an i18next error message, a missing
// key renders the key itself). Dynamic keys (template literals) and
// `returnObjects` calls are skipped.
import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";

import i18next from "i18next";
import { describe, expect, it } from "vitest";

import en from "@/i18n/en.json";
import ko from "@/i18n/ko.json";

const SRC = path.resolve(import.meta.dirname, "..");

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = path.join(dir, name);
    if (statSync(p).isDirectory()) {
      if (name !== "__tests__") walk(p, out);
    } else if (/\.(tsx?|ts)$/.test(name) && !name.endsWith(".test.ts")) out.push(p);
  }
  return out;
}

function staticKeys(): Map<string, string[]> {
  const keys = new Map<string, string[]>();
  for (const file of walk(SRC)) {
    const text = readFileSync(file, "utf8");
    for (const m of text.matchAll(/\bt\(\s*"([^"]+)"\s*(,\s*\{[^}]*\})?\s*\)/g)) {
      const key = m[1]!;
      if (m[2]?.includes("returnObjects")) continue;
      const list = keys.get(key) ?? [];
      list.push(path.relative(SRC, file));
      keys.set(key, list);
    }
  }
  return keys;
}

describe("static i18n keys resolve to strings", () => {
  const instance = i18next.createInstance();
  instance.init({ lng: "ko", fallbackLng: false, resources: { ko: { translation: ko }, en: { translation: en } }, interpolation: { escapeValue: false } });
  const keys = staticKeys();
  it("finds keys", () => {
    expect(keys.size).toBeGreaterThan(50);
  });
  for (const lng of ["ko", "en"] as const) {
    it(`resolves every key in ${lng}`, () => {
      const bad: string[] = [];
      for (const [key, files] of keys) {
        const v = instance.t(key, { lng, defaultValue: "__missing__" });
        if (typeof v !== "string" || v === "__missing__" || v === key) bad.push(`${key} (${files[0]})`);
      }
      expect(bad).toEqual([]);
    });
  }
});
