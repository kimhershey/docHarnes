import { useEffect, type ReactNode } from "react";

import { useSettingsStore } from "@/stores/settings";

/**
 * Tracks the OS colour scheme / forced-colours state so a "system" theme
 * follows Windows live, and re-applies tokens whenever settings change.
 */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const setSystemDark = useSettingsStore((s) => s.setSystemDark);

  useEffect(() => {
    if (!window.matchMedia) return;
    const dark = window.matchMedia("(prefers-color-scheme: dark)");
    const forced = window.matchMedia("(forced-colors: active)");
    const update = () => setSystemDark(dark.matches, forced.matches);
    update();
    dark.addEventListener("change", update);
    forced.addEventListener("change", update);
    return () => {
      dark.removeEventListener("change", update);
      forced.removeEventListener("change", update);
    };
  }, [setSystemDark]);

  return <>{children}</>;
}
