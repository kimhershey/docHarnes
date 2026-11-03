// Typed IPC layer. In the Tauri webview this calls Rust commands; in a plain
// browser (vite dev/preview, Playwright) it falls back to the in-memory mock
// backend so the whole UI can be exercised without the desktop runtime.

import type {
  AppInfo,
  AppSettings,
  Assessment,
  DateRange,
  HardwareSummary,
  Language,
  PreRouteResult,
  RegistryView,
  RetentionPolicy,
  StartupStatus,
  Theme,
  Tier,
  TierMode,
  WorkspaceInspection,
  WorkspaceView,
} from "./types";

export const isTauri: boolean = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type InvokeArgs = Record<string, unknown> | undefined;

let mockModule: Promise<typeof import("./mock-backend")> | null = null;

async function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  if (isTauri) {
    const { invoke: tauriInvoke } = await import("@tauri-apps/api/core");
    return tauriInvoke<T>(cmd, args);
  }
  mockModule ??= import("./mock-backend");
  const mock = await mockModule;
  return mock.mockInvoke<T>(cmd, args);
}

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  startupStatus: () => invoke<StartupStatus>("startup_status"),

  settingsGet: () => invoke<AppSettings>("settings_get"),
  settingsUpdate: (settings: AppSettings) => invoke<AppSettings>("settings_update", { settings }),
  settingsSetTheme: (theme: Theme) => invoke<AppSettings>("settings_set_theme", { theme }),
  settingsSetLanguage: (language: Language) => invoke<AppSettings>("settings_set_language", { language }),
  settingsSetUiScale: (scalePercent: number) => invoke<AppSettings>("settings_set_ui_scale", { scalePercent }),
  settingsSetActiveModel: (modelId: string) => invoke<AppSettings>("settings_set_active_model", { modelId }),
  settingsSetTierMode: (mode: TierMode, manualTier: Tier | null) =>
    invoke<AppSettings>("settings_set_tier_mode", { mode, manualTier }),

  workspaceInspect: (path: string) => invoke<WorkspaceInspection>("workspace_inspect", { path }),
  workspaceCurrent: () => invoke<WorkspaceView | null>("workspace_current"),
  workspaceOpen: (path: string, forceUnlock = false) =>
    invoke<WorkspaceView>("workspace_open", { args: { path, force_unlock: forceUnlock } }),
  workspaceCreate: (path: string, name: string | null, locale: Language | null, forceUnlock = false) =>
    invoke<WorkspaceView>("workspace_create", { args: { path, name, locale, force_unlock: forceUnlock } }),
  workspaceClose: () => invoke<void>("workspace_close"),
  workspaceForget: (path: string) => invoke<AppSettings>("workspace_forget", { path }),
  workspaceSetRetention: (retention: RetentionPolicy) => invoke<WorkspaceView>("workspace_set_retention", { retention }),
  workspaceSetPreferredModel: (modelId: string | null) =>
    invoke<WorkspaceView>("workspace_set_preferred_model", { modelId }),

  hwProbe: (refresh = false) => invoke<HardwareSummary>("hw_probe", { refresh }),
  hwAssess: () => invoke<Assessment>("hw_assess"),

  registryList: () => invoke<RegistryView>("registry_list"),
  registryRequiredFilesText: (modelId: string) => invoke<string>("registry_required_files_text", { modelId }),

  prerouterRoute: (text: string) => invoke<PreRouteResult>("prerouter_route", { text }),
  prerouterResolveDate: (text: string) => invoke<DateRange | null>("prerouter_resolve_date", { text }),
};

/** Native folder picker (Tauri only). Returns null when cancelled or unavailable. */
export async function pickFolder(title: string, defaultPath?: string): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const result = await open({ directory: true, multiple: false, title, defaultPath });
  return typeof result === "string" ? result : null;
}

/** Reveals a path in the OS file manager (Tauri only). */
export async function revealPath(path: string): Promise<void> {
  if (!isTauri) {
    console.info("[mock] reveal", path);
    return;
  }
  const opener = await import("@tauri-apps/plugin-opener");
  try {
    await opener.revealItemInDir(path);
  } catch {
    await opener.openPath(path);
  }
}

export async function openExternal(url: string): Promise<void> {
  if (!isTauri) {
    window.open(url, "_blank", "noopener");
    return;
  }
  const opener = await import("@tauri-apps/plugin-opener");
  await opener.openUrl(url);
}
