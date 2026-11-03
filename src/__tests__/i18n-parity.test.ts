// Every key in ko.json must exist in en.json and vice versa, with the same
// value shape (string vs array vs object) and the same interpolation vars.
import { describe, expect, it } from "vitest";

import en from "@/i18n/en.json";
import ko from "@/i18n/ko.json";

type Tree = { [k: string]: string | string[] | Tree };

function flatten(tree: Tree, prefix = ""): Map<string, string | string[]> {
  const out = new Map<string, string | string[]>();
  for (const [k, v] of Object.entries(tree)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (typeof v === "string" || Array.isArray(v)) out.set(key, v);
    else for (const [kk, vv] of flatten(v, key)) out.set(kk, vv);
  }
  return out;
}

function vars(s: string): string[] {
  return [...s.matchAll(/\{\{(\w+)\}\}/g)].map((m) => m[1]!).sort();
}

describe("i18n parity ko ↔ en", () => {
  const k = flatten(ko as Tree);
  const e = flatten(en as Tree);
  it("has identical key sets", () => {
    const onlyKo = [...k.keys()].filter((x) => !e.has(x));
    const onlyEn = [...e.keys()].filter((x) => !k.has(x));
    expect(onlyKo, "keys only in ko").toEqual([]);
    expect(onlyEn, "keys only in en").toEqual([]);
  });
  it("matches value shapes and interpolation variables", () => {
    for (const [key, kv] of k) {
      const ev = e.get(key)!;
      expect(Array.isArray(kv), `${key} array shape`).toBe(Array.isArray(ev));
      if (typeof kv === "string" && typeof ev === "string") {
        expect(vars(kv), `${key} vars`).toEqual(vars(ev));
        expect(kv.length, `${key} empty`).toBeGreaterThan(0);
      }
    }
  });
  it("covers every hardware warning key emitted by the Rust core", () => {
    for (const w of ["cpu_only_slow", "no_nvml", "low_ram", "cuda_driver_too_old", "old_gpu_arch"]) {
      expect(k.has(`hw.warn.${w}`), w).toBe(true);
    }
    for (const tier of ["cpu_only", "vram_6gb", "vram_8gb", "vram_12gb", "vram_16gb", "vram_24gb_plus"]) {
      expect(k.has(`hw.tier.${tier}`), tier).toBe(true);
    }
  });
});
