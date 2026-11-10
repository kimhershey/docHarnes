import { useSettingsStore } from "@/stores/settings";

/** Root font size at UI 배율 100 %: `html { font-size: calc(16px * var(--ui-scale)) }` in globals.css. */
export const BASE_FONT_PX = 16;

/**
 * Current root font size in CSS px (16 px × UI 배율). Layout code that has to think in px — virtualiser
 * row estimates, cards-per-row counts — derives its numbers from rem through this hook instead of
 * hard-coding px, so a higher UI 배율 grows those boxes together with the text they hold (fixed px boxes
 * clipped titles and badges from 125 % up).
 */
export function useRootFontSize(): number {
  const scale = useSettingsStore((s) => s.settings?.ui.scale_percent ?? 100);
  return (BASE_FONT_PX * scale) / 100;
}

/** `rem` → whole CSS px at the given root font size. */
export function remPx(rem: number, rootPx: number): number {
  return Math.round(rem * rootPx);
}
