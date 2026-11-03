import { describe, expect, it } from "vitest";

import { contrastRatio, parseColor, resolveTheme, toggledTheme } from "@/lib/theme";

describe("resolveTheme", () => {
  it("resolves system to the OS scheme", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
  });
  it("prefers high contrast when forced colours are active", () => {
    expect(resolveTheme("system", false, true)).toBe("high-contrast");
    expect(resolveTheme("dark", false, true)).toBe("dark");
  });
  it("keeps explicit themes", () => {
    expect(resolveTheme("high-contrast", false)).toBe("high-contrast");
    expect(resolveTheme("light", true)).toBe("light");
  });
});

describe("toggledTheme (mirrors Theme::toggled in Rust)", () => {
  it("toggles light/dark and leaves system by the OS state", () => {
    expect(toggledTheme("light", false)).toBe("dark");
    expect(toggledTheme("dark", true)).toBe("light");
    expect(toggledTheme("high-contrast", false)).toBe("light");
    expect(toggledTheme("system", true)).toBe("light");
    expect(toggledTheme("system", false)).toBe("dark");
  });
});

describe("contrast", () => {
  it("parses colours", () => {
    expect(parseColor("#fff")).toEqual([255, 255, 255]);
    expect(parseColor("#1b1b1f")).toEqual([27, 27, 31]);
    expect(parseColor("rgb(10, 20, 30)")).toEqual([10, 20, 30]);
    expect(parseColor("Canvas")).toBeNull();
  });
  it("computes WCAG ratios", () => {
    expect(contrastRatio("#000", "#fff")).toBeCloseTo(21, 1);
    expect(contrastRatio("#777", "#777")).toBeCloseTo(1, 3);
  });
});
