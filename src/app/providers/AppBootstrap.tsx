import { useEffect, useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { isTauri } from "@/lib/ipc";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";
import { Spinner } from "@/components/ui/spinner";

/** Loads settings + workspace once and surfaces startup notices as toasts. */
export function AppBootstrap({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const settingsLoaded = useSettingsStore((s) => s.loaded);
  const settingsError = useSettingsStore((s) => s.error);
  const loadSettings = useSettingsStore((s) => s.load);
  const workspaceLoaded = useWorkspaceStore((s) => s.loaded);
  const startup = useWorkspaceStore((s) => s.startup);
  const loadWorkspace = useWorkspaceStore((s) => s.load);
  const toast = useUiStore((s) => s.toast);
  const announced = useRef(false);

  useEffect(() => {
    void loadSettings();
    void loadWorkspace();
  }, [loadSettings, loadWorkspace]);

  useEffect(() => {
    if (!startup || announced.current) return;
    announced.current = true;
    if (startup.settings.kind === "recovered_from_corrupt") {
      toast(t("startup.settings_recovered", { backup: startup.settings.backup }), "warn");
    }
    if (startup.settings.kind === "partially_recovered") {
      toast(t("startup.settings_partial", { sections: startup.settings.sections.join(", ") }), "warn");
    }
    const ws = startup.workspace;
    if (ws.kind === "missing") toast(t("startup.workspace_missing", { path: ws.path }), "warn");
    if (ws.kind === "locked") toast(t("startup.workspace_locked", { path: ws.path, pid: ws.pid, host: ws.host }), "danger");
    if (ws.kind === "failed") toast(t("startup.workspace_failed", { path: ws.path, message: ws.message }), "danger");
    if (!isTauri) toast(t("app.preview_mode"), "info");
  }, [startup, toast, t]);

  if (!settingsLoaded || !workspaceLoaded) {
    return (
      <div className="flex h-full items-center justify-center gap-3 text-fg-muted" role="status">
        <Spinner />
        <span>{t("app.loading")}</span>
      </div>
    );
  }
  if (settingsError) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-8 text-center">
        <p className="text-danger font-medium">{t("common.error")}</p>
        <p className="text-fg-muted selectable">{settingsError}</p>
      </div>
    );
  }
  return <>{children}</>;
}
