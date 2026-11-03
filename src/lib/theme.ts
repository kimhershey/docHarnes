import type { ResolvedTheme, Theme, UiSettings } from "./types";

export const UI_MIRROR_KEY = "doculens.ui";

/** Resolves the user's theme setting to a concrete theme (design §3.7.4). */
export function resolveTheme(setting: Theme, systemDark: boolean, forcedColors = false): ResolvedTheme {
  if (setting === "system") {
    if (forcedColors) return "high-contrast";
    return systemDark ? "dark" : "light";
  }
  return setting;
}

/** Light/dark toggle for the top-bar button (mirrors `Theme::toggled` in Rust). */
export function toggledTheme(setting: Theme, systemDark: boolean): Theme {
  switch (setting) {
    case "light":
      return "dark";
    case "dark":
      return "light";
    case "high-contrast":
      return "light";
    case "system":
      return systemDark ? "light" : "dark";
  }
}

export function systemPrefersDark(): boolean {
  return typeof window !== "undefined" && !!window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

export function forcedColorsActive(): boolean {
  return typeof window !== "undefined" && !!window.matchMedia && window.matchMedia("(forced-colors: active)").matches;
}

export function applyThemeToDocument(setting: Theme, resolved: ResolvedTheme): void {
  const html = document.documentElement;
  html.setAttribute("data-theme", resolved);
  html.setAttribute("data-theme-setting", setting);
}

export function applyUiToDocument(ui: Pick<UiSettings, "language" | "scale_percent" | "reduced_motion">): void {
  const html = document.documentElement;
  html.setAttribute("lang", ui.language);
  html.style.setProperty("--ui-scale", String(ui.scale_percent / 100));
  if (ui.reduced_motion === "on") html.setAttribute("data-reduced-motion", "on");
  else html.removeAttribute("data-reduced-motion");
}

/** Mirror used by the inline script in index.html for a flash-free first paint. */
export function persistUiMirror(ui: Pick<UiSettings, "theme" | "language" | "scale_percent">): void {
  try {
    localStorage.setItem(UI_MIRROR_KEY, JSON.stringify({ theme: ui.theme, language: ui.language, scale_percent: ui.scale_percent }));
  } catch {
    /* storage unavailable */
  }
}

/** Parses a CSS colour (#rgb, #rrggbb, rgb()) into [r, g, b] 0–255. */
export function parseColor(input: string): [number, number, number] | null {
  const s = input.trim().toLowerCase();
  const hex = s.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/);
  if (hex) {
    const h = hex[1]!;
    const full = h.length === 3 ? h.split("").map((c) => c + c).join("") : h;
    return [parseInt(full.slice(0, 2), 16), parseInt(full.slice(2, 4), 16), parseInt(full.slice(4, 6), 16)];
  }
  const rgb = s.match(/^rgba?\(\s*(\d+)[\s,]+(\d+)[\s,]+(\d+)/);
  if (rgb) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])];
  return null;
}

function channel(c: number): number {
  const v = c / 255;
  return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
}

export function relativeLuminance([r, g, b]: [number, number, number]): number {
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** WCAG 2.x contrast ratio between two colours. */
export function contrastRatio(a: string, b: string): number {
  const ca = parseColor(a);
  const cb = parseColor(b);
  if (!ca || !cb) return 0;
  const la = relativeLuminance(ca);
  const lb = relativeLuminance(cb);
  const [hi, lo] = la > lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}
