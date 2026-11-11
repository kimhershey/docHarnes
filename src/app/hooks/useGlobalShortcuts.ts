import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { runJournal } from "@/lib/commands";
import { matchesChord } from "@/lib/shortcuts";
import { SCREEN_ROUTES, type JournalBatch } from "@/lib/types";
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

/**
 * A screen with an undo history of its own — the viewer's page-text corrections. While one is registered, an
 * undo / redo that reaches the global handler goes to it first; with nothing to take back there, the file
 * journal is reverted only after the screen has asked the user. (DOC-2: with the focus outside the viewer's own
 * key handling, Ctrl+Z after a correction on the page image reverted the last file-organise batch — "2건 되돌림"
 * — and left the correction in place.)
 */
export interface UndoScope {
  /** Takes the undo / redo itself; resolves to false when the screen has nothing to take back / re-apply. */
  run: (redo: boolean) => Promise<boolean>;
  /** Asks before the file journal is touched from this screen and calls `proceed` when the user agrees. */
  confirmJournal: (redo: boolean, proceed: () => void) => void;
}

let undoScope: UndoScope | null = null;

/** Registers the screen's undo scope; the returned function removes it again (unmount). */
export function setUndoScope(scope: UndoScope): () => void {
  undoScope = scope;
  return () => {
    if (undoScope === scope) undoScope = null;
  };
}

/** Undo / redo from the global shortcut: the registered screen first, the file journal directly only without one. */
export async function routeUndo(redo: boolean, journal: () => void, scope: UndoScope | null = undoScope): Promise<void> {
  if (!scope) {
    journal();
    return;
  }
  if (await scope.run(redo)) return;
  scope.confirmJournal(redo, journal);
}

/**
 * The batch a journal undo / redo would revert right now, out of the newest batches (`journal_batches`, newest
 * first) — the same choice as the core's `journal::last_undoable` / `last_redoable`: the newest file batch that
 * is not undone, or the newest undo that no newer file batch has invalidated. Null when there is nothing.
 */
export function nextJournalBatch(batches: JournalBatch[], redo: boolean): JournalBatch | null {
  const at = (b: JournalBatch) => Date.parse(b.created_at);
  // The newest of a set; on equal times the one listed first (the list is newest first).
  const newest = (set: JournalBatch[]) => set.reduce<JournalBatch | null>((best, b) => (best === null || at(b) > at(best) ? b : best), null);
  const live = batches.filter((b) => b.entries > 0);
  if (!redo) return newest(live.filter((b) => b.kind !== "undo" && b.undone_at === null));
  const undo = newest(live.filter((b) => b.kind === "undo" && b.undone_at === null));
  if (!undo) return null;
  // A newer file batch invalidates the redo (the usual editor rule).
  return live.some((b) => b.kind !== "undo" && at(b) > at(undo)) ? null : undo;
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
      // A screen that handled the key itself (the viewer's text-edit undo) keeps it.
      if (e.isComposing || e.defaultPrevented) return;
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
          case "redo": {
            // Ctrl+Z / Ctrl+Y revert the last journaled file batch (design §3.5) — never inside a text
            // field (the editor's own undo) or while a dialog / the palette owns the keyboard. A screen with
            // its own history (the viewer) comes first and has the journal confirmed (`routeUndo`).
            if (isEditable(e.target) || dialogOpen() || ui.paletteOpen) return;
            e.preventDefault();
            const redo = action === "redo";
            void routeUndo(redo, () => void runJournal(t, { redo }));
            return;
          }
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
