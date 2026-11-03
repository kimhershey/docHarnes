// Self-made contrast checker for the theme tokens (design §3.7.4):
// light/dark must reach WCAG AA (4.5:1) for text, high-contrast AAA (7:1).
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

import { contrastRatio } from "@/lib/theme";

const css = readFileSync(path.resolve(__dirname, "../styles/globals.css"), "utf8");

function tokens(selector: string): Record<string, string> {
  const start = css.indexOf(selector);
  if (start < 0) throw new Error(`selector not found: ${selector}`);
  const open = css.indexOf("{", start);
  const close = css.indexOf("}", open);
  const body = css.slice(open + 1, close);
  const out: Record<string, string> = {};
  for (const m of body.matchAll(/--([a-z0-9-]+):\s*([^;]+);/g)) out[m[1]!] = m[2]!.trim();
  return out;
}

const THEMES: { selector: string; min: number }[] = [
  { selector: '[data-theme="light"]', min: 4.5 },
  { selector: '[data-theme="dark"]', min: 4.5 },
  { selector: '[data-theme="high-contrast"]', min: 7 },
];

describe("theme token contrast", () => {
  for (const { selector, min } of THEMES) {
    const t = tokens(selector);
    it(`${selector}: body text on backgrounds ≥ ${min}:1`, () => {
      for (const bg of ["bg", "bg-elevated", "bg-sunken"]) {
        expect(contrastRatio(t.fg!, t[bg]!), `fg on ${bg}`).toBeGreaterThanOrEqual(min);
        expect(contrastRatio(t["fg-muted"]!, t[bg]!), `fg-muted on ${bg}`).toBeGreaterThanOrEqual(min);
      }
    });
    it(`${selector}: accent text and status colours are readable`, () => {
      expect(contrastRatio(t["accent-fg"]!, t.accent!), "accent-fg on accent").toBeGreaterThanOrEqual(min);
      for (const k of ["ok", "warn", "danger", "info"]) {
        expect(contrastRatio(t[k]!, t[`${k}-soft`]!), `${k} on ${k}-soft`).toBeGreaterThanOrEqual(min === 7 ? 7 : 4.5);
        expect(contrastRatio(t[k]!, t["bg-elevated"]!), `${k} on bg-elevated`).toBeGreaterThanOrEqual(min === 7 ? 7 : 3);
      }
    });
  }
  it("all themes define the same colour token set", () => {
    const colourKeys = (sel: string) =>
      Object.keys(tokens(sel))
        .filter((k) => !k.startsWith("radius") && k !== "shadow")
        .sort()
        .join(",");
    const sets = THEMES.map(({ selector }) => colourKeys(selector));
    expect(new Set(sets).size, sets.join("\n")).toBe(1);
  });
});
