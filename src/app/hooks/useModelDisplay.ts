import { useCallback } from "react";
import { useTranslation } from "react-i18next";

/**
 * Display names of the model registry carry a Korean descriptor in parentheses — "Gemma 4 E4B (Q6_K, 정확도)",
 * "Gemma 4 E2B (Q4_K_M, CPU·저여유)" — and `models/registry.json` is the single source of truth for them. In the
 * English UI the descriptor's words are translated here, at display time (0.7.0 audit UI-14); the Korean UI shows
 * the name as it is. A word that is not in the table (a model the user dropped in) stays as it is.
 */
const TERMS: readonly (readonly [term: string, key: string])[] = [
  ["저여유", "models.display_terms.low_headroom"],
  ["정확도", "models.display_terms.accuracy"],
  ["균형", "models.display_terms.balanced"],
  ["임베딩", "models.display_terms.embedding"],
];

const HANGUL = /[가-힣]/;

export function localizeModelDisplay(display: string, language: string, t: (key: string) => string): string {
  if (language.startsWith("ko") || !HANGUL.test(display)) return display;
  // Only the descriptor — the last parenthesised group — is translated; the model's name is never touched.
  return display.replace(/\(([^()]*)\)\s*$/, (group: string) => TERMS.reduce((text, [term, key]) => text.split(term).join(t(key)), group));
}

/** `(display) => string` for the current UI language; `null` / `undefined` pass through as `null`. */
export function useModelDisplay(): (display: string | null | undefined) => string | null {
  const { t, i18n } = useTranslation();
  const language = i18n.language;
  return useCallback((display) => (display == null ? null : localizeModelDisplay(display, language, t)), [language, t]);
}
