// Typed IPC layer. In the Tauri webview this calls Rust commands; in a plain
// browser (vite dev/preview, Playwright) it falls back to the in-memory mock
// backend so the whole UI can be exercised without the desktop runtime.

import type {
  AppInfo,
  AppSettings,
  Assessment,
  ChatTestResult,
  DateRange,
  DocumentFilter,
  DocumentImportPlan,
  DocumentView,
  EngineBinaryInfo,
  EngineDevice,
  EngineStatus,
  HardwareSummary,
  ImportMode,
  ImportOptions,
  ImportPlan,
  ImportSource,
  Job,
  Language,
  LibraryStats,
  LogLine,
  ModelIndex,
  ModelInstanceStatus,
  PageView,
  PdfiumInfo,
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

function mock(): Promise<typeof import("./mock-backend")> {
  mockModule ??= import("./mock-backend");
  return mockModule;
}

async function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  if (isTauri) {
    const { invoke: tauriInvoke } = await import("@tauri-apps/api/core");
    try {
      return await tauriInvoke<T>(cmd, args);
    } catch (e) {
      // Surface IPC failures in the Rust log file as well as the webview console.
      void import("@tauri-apps/plugin-log").then(({ error }) => error(`invoke ${cmd} failed: ${JSON.stringify(e)}`)).catch(() => undefined);
      throw e;
    }
  }
  const m = await mock();
  return m.mockInvoke<T>(cmd, args);
}

/**
 * Subscribes to a backend event (`engine://status`, `jobs://update`, …).
 * Resolves to an unsubscribe function.
 */
export async function subscribe<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
  if (isTauri) {
    const { listen } = await import("@tauri-apps/api/event");
    const unlisten = await listen<T>(event, (e) => handler(e.payload));
    return () => void unlisten();
  }
  const m = await mock();
  return m.mockSubscribe<T>(event, handler);
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

  modelsScan: () => invoke<ModelIndex>("models_scan"),
  modelsList: () => invoke<ModelIndex>("models_list"),
  modelsImportPlan: (paths: string[], mode: ImportMode, folderName: string | null = null) =>
    invoke<ImportPlan>("models_import_plan", { args: { paths, mode, folder_name: folderName } }),
  modelsImportRun: (paths: string[], mode: ImportMode, folderName: string | null = null) =>
    invoke<string>("models_import_run", { args: { paths, mode, folder_name: folderName } }),
  modelsDownload: (modelId: string) => invoke<string>("models_download", { modelId }),
  modelsHash: (folder: string) => invoke<string>("models_hash", { folder }),
  modelsDelete: (folder: string) => invoke<void>("models_delete", { folder }),

  engineStatus: () => invoke<EngineStatus>("engine_status"),
  engineStart: () => invoke<EngineStatus>("engine_start"),
  engineStop: () => invoke<void>("engine_stop"),
  engineRestart: () => invoke<EngineStatus>("engine_restart"),
  engineLoadModel: (modelId: string, warmup: boolean | null = null) => invoke<string>("engine_load_model", { modelId, warmup }),
  engineUnloadModel: (modelId: string) => invoke<void>("engine_unload_model", { modelId }),
  engineRefreshModels: (reload = false) => invoke<ModelInstanceStatus[]>("engine_refresh_models", { reload }),
  engineChatTest: (modelId: string | null = null, prompt: string | null = null) =>
    invoke<ChatTestResult>("engine_chat_test", { modelId, prompt }),
  engineListDevices: () => invoke<EngineDevice[]>("engine_list_devices"),
  engineBinaryInfo: () => invoke<EngineBinaryInfo>("engine_binary_info"),
  engineLogs: (lines = 200) => invoke<LogLine[]>("engine_logs", { lines }),
  presetsPreview: () => invoke<string>("presets_preview"),

  jobsList: () => invoke<Job[]>("jobs_list"),
  jobsCancel: (jobId: string) => invoke<boolean>("jobs_cancel", { jobId }),
  jobsClearFinished: () => invoke<void>("jobs_clear_finished"),

  prerouterRoute: (text: string) => invoke<PreRouteResult>("prerouter_route", { text }),
  prerouterResolveDate: (text: string) => invoke<DateRange | null>("prerouter_resolve_date", { text }),

  documentsList: (filter: DocumentFilter = {}) => invoke<DocumentView[]>("documents_list", { filter }),
  documentsStats: () => invoke<LibraryStats>("documents_stats"),
  documentsGet: (id: string) => invoke<DocumentView>("documents_get", { id }),
  documentsPages: (id: string, withText = false) => invoke<PageView[]>("documents_pages", { id, withText }),
  documentsPageText: (id: string, pageNo: number) => invoke<string | null>("documents_page_text", { id, pageNo }),
  documentsImportPlan: (paths: string[], options: ImportOptions, source: ImportSource = "drop") =>
    invoke<DocumentImportPlan>("documents_import_plan", { args: { paths, options, source } }),
  documentsImportRun: (token: string | null, paths: string[], options: ImportOptions, source: ImportSource = "drop") =>
    invoke<string>("documents_import_run", { args: { token, paths, options, source } }),
  documentsRescan: () => invoke<string | null>("documents_rescan"),
  documentsInboxScan: () => invoke<void>("documents_inbox_scan"),
  documentsRerender: (ids: string[], dpi: number | null = null) => invoke<string>("documents_rerender", { ids, dpi }),
  documentsRotatePage: (id: string, pageNo: number, rotation: number) =>
    invoke<PageView>("documents_rotate_page", { id, pageNo, rotation }),
  documentsDelete: (ids: string[], keepFiles = false) => invoke<string[]>("documents_delete", { ids, keepFiles }),
  documentsSetTags: (id: string, tags: string[]) => invoke<DocumentView>("documents_set_tags", { id, tags }),
  documentsSetDocType: (id: string, docType: string | null) => invoke<DocumentView>("documents_set_doc_type", { id, docType }),
  documentsSetTitle: (id: string, title: string | null) => invoke<DocumentView>("documents_set_title", { id, title }),
  documentsTags: () => invoke<[string, number][]>("documents_tags"),
  documentsSetInboxWatch: (enabled: boolean) => invoke<boolean>("documents_set_inbox_watch", { enabled }),
  documentsPdfiumInfo: () => invoke<PdfiumInfo>("documents_pdfium_info"),
};

/** Native multi-file picker for documents (Tauri only). Returns [] when cancelled or unavailable. */
export async function pickDocumentFiles(title: string, filterName: string, defaultPath?: string): Promise<string[]> {
  if (!isTauri) return [];
  const { open } = await import("@tauri-apps/plugin-dialog");
  const result = await open({
    directory: false,
    multiple: true,
    title,
    defaultPath,
    filters: [{ name: filterName, extensions: ["pdf", "png", "jpg", "jpeg", "tif", "tiff", "webp", "bmp", "gif", "hwpx", "docx", "hwp"] }],
  });
  if (Array.isArray(result)) return result;
  return typeof result === "string" ? [result] : [];
}

/** Native folder picker (Tauri only). Returns null when cancelled or unavailable. */
export async function pickFolder(title: string, defaultPath?: string): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const result = await open({ directory: true, multiple: false, title, defaultPath });
  return typeof result === "string" ? result : null;
}

/** Native multi-file picker for GGUF files (Tauri only). Returns [] when cancelled or unavailable. */
export async function pickGgufFiles(title: string, defaultPath?: string): Promise<string[]> {
  if (!isTauri) return [];
  const { open } = await import("@tauri-apps/plugin-dialog");
  const result = await open({
    directory: false,
    multiple: true,
    title,
    defaultPath,
    filters: [{ name: "GGUF", extensions: ["gguf"] }],
  });
  if (Array.isArray(result)) return result;
  return typeof result === "string" ? [result] : [];
}

/** Native single-file picker for the llama-server binary (Tauri only). */
export async function pickExecutable(title: string): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const result = await open({ directory: false, multiple: false, title });
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
    // Fallback for paths the reveal API rejects (e.g. a missing file): open the containing folder.
    const dir = path.replace(/[\\/][^\\/]*$/, "");
    await opener.openPath(dir || path);
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
