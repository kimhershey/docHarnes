import { SCREEN_ROUTES } from "@/lib/types";

/** Settings tabs (0.5.0): 일반 · 워크스페이스 · 처리 · AI 모델 · 양식·추출 항목 · 참고 자료 · 프라이버시·보존. */
export const SETTINGS_TABS = ["general", "workspace", "processing", "models", "schemas", "reference", "privacy"] as const;
export type SettingsTab = (typeof SETTINGS_TABS)[number];

export function isSettingsTab(value: string | null | undefined): value is SettingsTab {
  return !!value && (SETTINGS_TABS as readonly string[]).includes(value);
}

/** Route of a settings tab (`/settings?tab=models`); extra params are kept for the embedded screens (`register=` of 양식 등록). */
export function settingsTabPath(tab: SettingsTab, params?: URLSearchParams | Record<string, string>): string {
  const query = new URLSearchParams(params);
  query.set("tab", tab);
  return `${SCREEN_ROUTES.settings}?${query.toString()}`;
}
