import { Command } from "cmdk";
import { Languages, MessageSquare, Play, Sun } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { describePreRoute, executePreRoute } from "@/lib/commands";
import { api } from "@/lib/ipc";
import { isComposing } from "@/lib/shortcuts";
import { SCREEN_ROUTES, type PreRouteResult, type Screen } from "@/lib/types";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";

const SCREENS = Object.keys(SCREEN_ROUTES) as Screen[];

export function CommandPalette() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const open = useUiStore((s) => s.paletteOpen);
  const initialQuery = useUiStore((s) => s.paletteQuery);
  const close = useUiStore((s) => s.closePalette);
  const setPending = useUiStore((s) => s.setPendingAssistantInput);
  const toggleTheme = useSettingsStore((s) => s.toggleTheme);
  const setLanguage = useSettingsStore((s) => s.setLanguage);
  const openWorkspaceDialog = useUiStore((s) => s.openWorkspaceDialog);
  const [query, setQuery] = useState("");
  const [route, setRoute] = useState<PreRouteResult | null>(null);

  useEffect(() => {
    if (open) setQuery(initialQuery);
  }, [open, initialQuery]);

  useEffect(() => {
    const text = query.trim();
    if (!text) {
      setRoute(null);
      return;
    }
    let cancelled = false;
    const handle = window.setTimeout(() => {
      void api.prerouterRoute(text).then((r) => {
        if (!cancelled) setRoute(r.matched ? r : null);
      });
    }, 80);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [query]);

  const run = async (r: PreRouteResult) => {
    close();
    await executePreRoute(r, navigate, t);
  };

  const sendToAssistant = () => {
    const text = query.trim();
    close();
    if (text) setPending(text);
    navigate(SCREEN_ROUTES.assistant);
  };

  return (
    <Command.Dialog
      open={open}
      onOpenChange={(o) => (o ? undefined : close())}
      label={t("topbar.command")}
      className="fixed left-1/2 top-24 z-50 w-[40rem] max-w-[calc(100vw-2rem)] -translate-x-1/2 overflow-hidden rounded-lg border border-border bg-bg-elevated text-fg shadow-[var(--shadow)]"
      overlayClassName="fixed inset-0 z-40 bg-black/40"
      shouldFilter={true}
      onKeyDownCapture={(e) => {
        if (e.key === "Enter" && isComposing(e)) {
          e.preventDefault();
          e.stopPropagation();
        }
      }}
      data-testid="command-palette"
    >
      <Command.Input
        value={query}
        onValueChange={setQuery}
        placeholder={t("palette.placeholder")}
        className="h-12 w-full border-b border-border bg-transparent px-4 text-sm outline-none placeholder:text-fg-subtle"
        autoFocus
        data-testid="palette-input"
      />
      <Command.List className="max-h-80 overflow-y-auto p-2 [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1 [&_[cmdk-group-heading]]:text-xs [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:text-fg-muted">
        {route && (
          <Command.Group heading={t("palette.run")} forceMount>
            <Command.Item value={`__run ${query}`} onSelect={() => void run(route)} className="palette-item" forceMount data-testid="palette-run">
              <Play className="size-4 text-accent" />
              <span className="flex-1">{describePreRoute(route, t)}</span>
              <span className="text-xs text-fg-subtle">{route.source === "slash" ? "/" : "NL"}</span>
            </Command.Item>
          </Command.Group>
        )}
        <Command.Empty className="px-2 py-6 text-center text-sm text-fg-muted">{t("palette.no_results")}</Command.Empty>
        <Command.Group heading={t("palette.navigate")}>
          {SCREENS.map((screen) => (
            <Command.Item
              key={screen}
              value={`${t(`nav.${screen}`)} ${screen}`}
              onSelect={() => {
                close();
                navigate(SCREEN_ROUTES[screen]);
              }}
              className="palette-item"
              data-testid={`palette-nav-${screen}`}
            >
              {t(`nav.${screen}`)}
            </Command.Item>
          ))}
        </Command.Group>
        <Command.Group heading={t("palette.actions")}>
          <Command.Item value={`${t("palette.toggle_theme")} theme 테마`} onSelect={() => void (close(), toggleTheme())} className="palette-item">
            <Sun className="size-4" />
            {t("palette.toggle_theme")}
          </Command.Item>
          <Command.Item value={`${t("language.ko")} 한국어 korean language 언어`} onSelect={() => void (close(), setLanguage("ko"))} className="palette-item">
            <Languages className="size-4" />
            {t("palette.set_language", { language: t("language.ko") })}
          </Command.Item>
          <Command.Item value={`${t("language.en")} english language 언어`} onSelect={() => void (close(), setLanguage("en"))} className="palette-item">
            <Languages className="size-4" />
            {t("palette.set_language", { language: t("language.en") })}
          </Command.Item>
          <Command.Item value={`${t("palette.open_workspace")} workspace 워크스페이스`} onSelect={() => void (close(), openWorkspaceDialog("open"))} className="palette-item">
            {t("palette.open_workspace")}
          </Command.Item>
        </Command.Group>
        {query.trim() && (
          <Command.Group heading={t("nav.assistant")} forceMount>
            <Command.Item value={query} onSelect={sendToAssistant} className="palette-item" forceMount data-testid="palette-send">
              <MessageSquare className="size-4" />
              {t("palette.send_to_assistant", { text: query.trim() })}
            </Command.Item>
          </Command.Group>
        )}
      </Command.List>
    </Command.Dialog>
  );
}
