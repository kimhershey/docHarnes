// Executes pre-routed operations (theme / language / open / help) — shared by
// the command palette, the home command bar and the assistant composer.

import type { TFunction } from "i18next";
import type { NavigateFunction } from "react-router";

import { SCREEN_ROUTES, type PreRouteResult } from "./types";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";

export interface ExecuteOutcome {
  /** Human-readable summary of what happened (already translated). */
  summary: string;
}

export function describePreRoute(result: PreRouteResult, t: TFunction): string {
  switch (result.op) {
    case "set_theme":
      return t("palette.set_theme", { theme: t(`theme.${result.theme}`) });
    case "set_language":
      return t("palette.set_language", { language: t(`language.${result.language}`) });
    case "open":
      return t("palette.open_screen", { screen: t(`nav.${result.screen}`) });
    case "help":
      return t("palette.help");
    default:
      return "";
  }
}

export async function executePreRoute(result: PreRouteResult, navigate: NavigateFunction, t: TFunction): Promise<ExecuteOutcome> {
  const settings = useSettingsStore.getState();
  const ui = useUiStore.getState();
  switch (result.op) {
    case "set_theme": {
      await settings.setTheme(result.theme);
      const summary = t("theme.changed", { theme: t(`theme.${result.theme}`) });
      ui.toast(summary, "ok");
      return { summary };
    }
    case "set_language": {
      await settings.setLanguage(result.language);
      const summary = t("language.changed", { language: t(`language.${result.language}`) });
      ui.toast(summary, "ok");
      return { summary };
    }
    case "open": {
      navigate(SCREEN_ROUTES[result.screen]);
      return { summary: t("palette.open_screen", { screen: t(`nav.${result.screen}`) }) };
    }
    case "help": {
      ui.setPendingAssistantInput("/help");
      navigate(SCREEN_ROUTES.assistant);
      return { summary: t("palette.help") };
    }
    default:
      return { summary: "" };
  }
}
