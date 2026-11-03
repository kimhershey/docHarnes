import { create } from "zustand";

import i18n from "@/i18n";
import { api } from "@/lib/ipc";
import { queryClient, queryKeys } from "@/lib/query-client";
import {
  applyThemeToDocument,
  applyUiToDocument,
  forcedColorsActive,
  persistUiMirror,
  resolveTheme,
  systemPrefersDark,
  toggledTheme,
} from "@/lib/theme";
import type { AppSettings, Language, ResolvedTheme, Theme, Tier, TierMode } from "@/lib/types";
import { errorMessage } from "@/lib/utils";

interface SettingsState {
  settings: AppSettings | null;
  loaded: boolean;
  error: string | null;
  systemDark: boolean;
  forcedColors: boolean;
  load: () => Promise<void>;
  apply: (settings: AppSettings) => void;
  setSystemDark: (dark: boolean, forced: boolean) => void;
  setTheme: (theme: Theme) => Promise<void>;
  toggleTheme: () => Promise<void>;
  setLanguage: (language: Language) => Promise<void>;
  setScale: (scalePercent: number) => Promise<void>;
  setTierMode: (mode: TierMode, manualTier: Tier | null) => Promise<void>;
  setActiveModel: (modelId: string) => Promise<void>;
  update: (mutate: (draft: AppSettings) => void) => Promise<void>;
}

function invalidateModelQueries() {
  void queryClient.invalidateQueries({ queryKey: queryKeys.assessment });
  void queryClient.invalidateQueries({ queryKey: queryKeys.registry });
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settings: null,
  loaded: false,
  error: null,
  systemDark: systemPrefersDark(),
  forcedColors: forcedColorsActive(),

  apply: (settings) => {
    const { systemDark, forcedColors } = get();
    applyThemeToDocument(settings.ui.theme, resolveTheme(settings.ui.theme, systemDark, forcedColors));
    applyUiToDocument(settings.ui);
    persistUiMirror(settings.ui);
    if (i18n.language !== settings.ui.language) void i18n.changeLanguage(settings.ui.language);
    set({ settings, loaded: true, error: null });
  },

  load: async () => {
    try {
      const s = await api.settingsGet();
      get().apply(s);
    } catch (e) {
      set({ error: errorMessage(e), loaded: true });
    }
  },

  setSystemDark: (dark, forced) => {
    set({ systemDark: dark, forcedColors: forced });
    const s = get().settings;
    if (s) applyThemeToDocument(s.ui.theme, resolveTheme(s.ui.theme, dark, forced));
  },

  setTheme: async (theme) => {
    const prev = get().settings;
    if (prev) {
      // optimistic: apply immediately for a snappy toggle
      get().apply({ ...prev, ui: { ...prev.ui, theme } });
    }
    get().apply(await api.settingsSetTheme(theme));
  },

  toggleTheme: async () => {
    const s = get().settings;
    if (!s) return;
    await get().setTheme(toggledTheme(s.ui.theme, get().systemDark));
  },

  setLanguage: async (language) => {
    get().apply(await api.settingsSetLanguage(language));
  },

  setScale: async (scalePercent) => {
    const clamped = Math.min(150, Math.max(90, Math.round(scalePercent)));
    get().apply(await api.settingsSetUiScale(clamped));
  },

  setTierMode: async (mode, manualTier) => {
    get().apply(await api.settingsSetTierMode(mode, manualTier));
    invalidateModelQueries();
  },

  setActiveModel: async (modelId) => {
    get().apply(await api.settingsSetActiveModel(modelId));
    invalidateModelQueries();
  },

  update: async (mutate) => {
    const current = get().settings;
    if (!current) return;
    const draft: AppSettings = structuredClone(current);
    mutate(draft);
    get().apply(await api.settingsUpdate(draft));
    invalidateModelQueries();
  },
}));

export function useResolvedTheme(): ResolvedTheme {
  return useSettingsStore((s) => resolveTheme(s.settings?.ui.theme ?? "system", s.systemDark, s.forcedColors));
}

export function useSettings(): AppSettings | null {
  return useSettingsStore((s) => s.settings);
}

export function useShortcut(action: string): string {
  return useSettingsStore((s) => s.settings?.shortcuts[action] ?? "");
}
