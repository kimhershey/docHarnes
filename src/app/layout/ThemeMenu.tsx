import { Check, ChevronDown, Contrast, Languages, Monitor, Moon, Sun } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { LANGUAGES, THEMES, type Theme } from "@/lib/types";
import { useResolvedTheme, useSettingsStore, useShortcut } from "@/stores/settings";

export const THEME_ICONS: Record<Theme, typeof Sun> = { light: Sun, dark: Moon, system: Monitor, "high-contrast": Contrast };

export function ThemeMenu() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings);
  const setTheme = useSettingsStore((s) => s.setTheme);
  const toggleTheme = useSettingsStore((s) => s.toggleTheme);
  const setLanguage = useSettingsStore((s) => s.setLanguage);
  const resolved = useResolvedTheme();
  const toggleChord = useShortcut("toggle_theme");
  const ResolvedIcon = THEME_ICONS[resolved];
  const current = settings?.ui.theme ?? "system";
  const language = settings?.ui.language ?? "ko";

  return (
    <div className="flex items-center">
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon" onClick={() => void toggleTheme()} aria-label={t("theme.toggle")} data-testid="theme-toggle">
            <ResolvedIcon />
          </Button>
        </TooltipTrigger>
        <TooltipContent>
          {t("theme.toggle")} <Kbd chord={toggleChord} />
        </TooltipContent>
      </Tooltip>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon-sm" aria-label={t("topbar.theme")} data-testid="theme-menu">
            <ChevronDown />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          <DropdownMenuLabel>{t("topbar.theme")}</DropdownMenuLabel>
          {THEMES.map((theme) => {
            const Icon = THEME_ICONS[theme];
            return (
              <DropdownMenuItem key={theme} onSelect={() => void setTheme(theme)} data-testid={`theme-option-${theme}`}>
                <Icon />
                <span className="flex-1">{t(`theme.${theme}`)}</span>
                {current === theme && <Check className="size-4" />}
              </DropdownMenuItem>
            );
          })}
          <DropdownMenuSeparator />
          <DropdownMenuLabel>{t("topbar.language")}</DropdownMenuLabel>
          {LANGUAGES.map((lang) => (
            <DropdownMenuItem key={lang} onSelect={() => void setLanguage(lang)} data-testid={`language-option-${lang}`}>
              <Languages />
              <span className="flex-1">{t(`language.${lang}`)}</span>
              {language === lang && <Check className="size-4" />}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
