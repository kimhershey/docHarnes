import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { runJournal } from "@/lib/commands";
import { matchesChord } from "@/lib/shortcuts";
import { SCREEN_ROUTES } from "@/lib/types";
import { isMac } from "@/lib/utils";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";

const NAV_ACTIONS: Record<string, keyof typeof SCREEN_ROUTES> = {
  go_home: "home",
  go_library: "library",
  go_assistant: "assistant",
  go_settings: "settings",
};

/** True when the key event comes from a text field (input, textarea, select or contenteditable). */
export function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}

/** A modal layer (dialog, command palette) owns the keyboard while open. */
function dialogOpen(): boolean {
  return document.querySelector('[role="dialog"], [cmdk-dialog]') !== null;
}

/** App-wide key bindings from settings.shortcuts (design §3.7.4). */
export function useGlobalShortcuts() {
  const navigate = useNavigate();
  const { t } = useTranslation();
  const shortcuts = useSettingsStore((s) => s.settings?.shortcuts);

  useEffect(() => {
    if (!shortcuts) return;
    const mac = isMac();
    const handler = (e: KeyboardEvent) => {
      if (e.isComposing) return;
      const ui = useUiStore.getState();
      const settings = useSettingsStore.getState();
      const hasModifier = e.ctrlKey || e.metaKey || e.altKey;
      for (const [action, chord] of Object.entries(shortcuts)) {
        if (!chord || !matchesChord(e, chord, mac)) continue;
        // Single-key chords never fire while typing.
        if (!hasModifier && isEditable(e.target)) continue;
        switch (action) {
          case "command_palette":
            e.preventDefault();
            ui.togglePalette();
            return;
          case "toggle_theme":
            e.preventDefault();
            void settings.toggleTheme();
            return;
          case "ui_scale_up":
          case "ui_scale_down": {
            e.preventDefault();
            const cur = settings.settings?.ui.scale_percent ?? 100;
            void settings.setScale(cur + (action === "ui_scale_up" ? 10 : -10));
            return;
          }
          case "close":
            if (ui.paletteOpen) {
              e.preventDefault();
              ui.closePalette();
            }
            return;
          case "undo":
          case "redo":
            // Ctrl+Z / Ctrl+Y revert the last journaled file batch (design §3.5) — never inside a text
            // field (the editor's own undo) or while a dialog / the palette owns the keyboard.
            if (isEditable(e.target) || dialogOpen() || ui.paletteOpen) return;
            e.preventDefault();
            void runJournal(t, { redo: action === "redo" });
            return;
          default: {
            const screen = NAV_ACTIONS[action];
            if (screen) {
              e.preventDefault();
              navigate(SCREEN_ROUTES[screen]);
              return;
            }
          }
        }
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [shortcuts, navigate, t]);
}
