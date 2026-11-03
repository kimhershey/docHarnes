// Browser-only mock of the Rust commands (used by `vite dev`, `vite preview`
// and Playwright). State lives in localStorage so reloads behave like the
// real settings.json. Only the subset of behaviour needed by the UI is
// reproduced; the Rust crate is the source of truth and has its own tests.

import registryJson from "../../models/registry.json";
import type {
  AppInfo,
  AppSettings,
  Assessment,
  DateRange,
  HardwareSummary,
  Language,
  ModelEntry,
  PreRouteResult,
  PreRoutedOp,
  RegistryView,
  RetentionPolicy,
  Screen,
  StartupStatus,
  Theme,
  Tier,
  TierMode,
  UiError,
  WorkspaceInspection,
  WorkspaceManifest,
  WorkspaceView,
} from "./types";

const SETTINGS_KEY = "doculens.mock.settings";
const WORKSPACES_KEY = "doculens.mock.workspaces";

export function defaultRetention(): RetentionPolicy {
  return {
    documents_days: null,
    extractions_days: null,
    page_cache_max_gb: 5,
    job_logs_days: 90,
    app_logs_days: 90,
    journal_days: 90,
    chats_days: 180,
    temp_days: 0,
    golden_exports_days: null,
    purge_hour_local: 3,
    purge_enabled: true,
  };
}

export function defaultShortcuts(): Record<string, string> {
  return {
    command_palette: "Ctrl+K",
    toggle_theme: "Ctrl+Shift+T",
    execute_plan: "Ctrl+Enter",
    close: "Escape",
    review_approve: "Enter",
    review_edit: "E",
    review_hold: "S",
    review_next: "N",
    explain_selection: "Ctrl+E",
    undo: "Ctrl+Z",
    redo: "Ctrl+Y",
    zoom_in: "Ctrl+=",
    zoom_out: "Ctrl+-",
    zoom_reset: "Ctrl+0",
    ui_scale_up: "Ctrl+Shift+=",
    ui_scale_down: "Ctrl+Shift+-",
    go_home: "Ctrl+1",
    go_library: "Ctrl+2",
    go_assistant: "Ctrl+3",
    go_settings: "Ctrl+,",
  };
}

export function defaultSettings(): AppSettings {
  return {
    schema_version: 1,
    active_workspace: null,
    workspaces: [],
    ui: { theme: "system", language: "ko", scale_percent: 100, reduced_motion: "system", sidebar_collapsed: false },
    models: {
      tier_mode: "auto",
      manual_tier: null,
      active_model: "auto",
      profile: "accuracy",
      backend: "auto",
      resident_models_max: 2,
      sleep_idle_seconds: 900,
      allow_online_download: true,
    },
    processing: {
      auto_level: "classify_and_ocr",
      render_dpi: 200,
      review_threshold: 0.8,
      audit_sample_percent: 5,
      organize_reviewed_only: true,
      inbox_watch: true,
    },
    cache: { page_cache_max_gb: 5, keep_thumbnails: true },
    privacy: { pii_masking: true, db_encryption: false },
    notifications: { job_complete: true, prevent_sleep_during_jobs: false },
    shortcuts: defaultShortcuts(),
    window: { width: 1280, height: 800, maximized: false, x: null, y: null },
  };
}

function read<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

function write(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* ignore */
  }
}

function loadSettings(): AppSettings {
  const stored = read<Partial<AppSettings>>(SETTINGS_KEY, {});
  const d = defaultSettings();
  return {
    ...d,
    ...stored,
    ui: { ...d.ui, ...(stored.ui ?? {}) },
    models: { ...d.models, ...(stored.models ?? {}) },
    processing: { ...d.processing, ...(stored.processing ?? {}) },
    cache: { ...d.cache, ...(stored.cache ?? {}) },
    privacy: { ...d.privacy, ...(stored.privacy ?? {}) },
    notifications: { ...d.notifications, ...(stored.notifications ?? {}) },
    shortcuts: { ...d.shortcuts, ...(stored.shortcuts ?? {}) },
    window: { ...d.window, ...(stored.window ?? {}) },
  };
}

function saveSettings(s: AppSettings): AppSettings {
  s.ui.scale_percent = Math.min(150, Math.max(90, s.ui.scale_percent));
  write(SETTINGS_KEY, s);
  return s;
}

type MockWorkspace = { path: string; manifest: WorkspaceManifest };

function loadWorkspaces(): MockWorkspace[] {
  return read<MockWorkspace[]>(WORKSPACES_KEY, []);
}

function saveWorkspaces(list: MockWorkspace[]): void {
  write(WORKSPACES_KEY, list);
}

let current: MockWorkspace | null = null;
let startupResolved = false;

function err(code: string, message: string, detail?: Record<string, unknown>): UiError {
  return { code, message, detail };
}

function normalizePath(p: string): string {
  return p.trim().replace(/[\\/]+$/, "");
}

function basename(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

function view(ws: MockWorkspace): WorkspaceView {
  const sep = ws.path.includes("\\") ? "\\" : "/";
  return {
    path: ws.path,
    manifest: ws.manifest,
    models_dir: `${ws.path}${sep}.doculens${sep}models`,
    inbox_dir: `${ws.path}${sep}inbox`,
    documents_dir: `${ws.path}${sep}documents`,
    exports_dir: `${ws.path}${sep}exports`,
  };
}

function rememberWorkspace(s: AppSettings, path: string, name: string): void {
  s.workspaces = s.workspaces.filter((w) => w.path !== path);
  s.workspaces.unshift({ path, name, last_opened: new Date().toISOString() });
  s.workspaces = s.workspaces.slice(0, 20);
  s.active_workspace = path;
}

function openWorkspace(path: string, create: boolean, name: string | null, locale: Language | null): WorkspaceView {
  const p = normalizePath(path);
  if (!p) throw err("invalid_workspace", "empty path");
  if (/^([A-Za-z]:)?[\\/]?$/.test(p)) throw err("invalid_workspace", `${p} is a drive root or home directory`);
  const list = loadWorkspaces();
  let ws = list.find((w) => w.path === p);
  if (!ws) {
    if (!create) throw err("invalid_workspace", `${p} is not a DocuLens workspace`);
    ws = {
      path: p,
      manifest: {
        workspace_id: crypto.randomUUID(),
        schema_version: 1,
        app_version: "0.1.0",
        last_opened_app_version: "0.1.0",
        name: name?.trim() || basename(p),
        created_at: new Date().toISOString(),
        last_opened_at: null,
        locale: locale ?? loadSettings().ui.language,
        preferred_model: null,
        models_location: "workspace",
        retention: defaultRetention(),
      },
    };
    list.push(ws);
  }
  ws.manifest.last_opened_at = new Date().toISOString();
  saveWorkspaces(list);
  current = ws;
  const s = loadSettings();
  rememberWorkspace(s, p, ws.manifest.name);
  saveSettings(s);
  return view(ws);
}

function resolveStartup(): void {
  if (startupResolved) return;
  startupResolved = true;
  const s = loadSettings();
  if (s.active_workspace) {
    const ws = loadWorkspaces().find((w) => w.path === s.active_workspace);
    if (ws) current = ws;
  }
}

const MOCK_HW: HardwareSummary = {
  os: "Browser preview",
  os_version: typeof navigator !== "undefined" ? navigator.userAgent.slice(0, 48) : "",
  hostname: "browser",
  cpu_name: "Mock CPU (browser preview)",
  cpu_cores_physical: 8,
  cpu_cores_logical: typeof navigator !== "undefined" ? navigator.hardwareConcurrency || 8 : 8,
  ram_total_mib: 32768,
  ram_available_mib: 20480,
  gpus: [
    {
      index: 0,
      name: "NVIDIA GeForce RTX 4060 (mock)",
      vendor: "nvidia",
      total_mib: 8192,
      free_mib: 7126,
      driver_version: "560.00 (mock)",
      cuda_driver_version: [12, 6],
      compute_capability: [8, 9],
      source: "manual",
    },
  ],
  nvml_available: true,
  probed_at: new Date().toISOString(),
};

function tierFromFree(free: number | null): Tier {
  if (free === null) return "cpu_only";
  if (free < 3584) return "cpu_only";
  if (free < 5632) return "vram_6gb";
  if (free < 9728) return "vram_8gb";
  if (free < 13824) return "vram_12gb";
  if (free <= 20480) return "vram_16gb";
  return "vram_24gb_plus";
}

const registry = registryJson as unknown as {
  models: ModelEntry[];
  tier_defaults: Record<Tier, string>;
  adaptive: Record<string, { min_free_vram_mib: number; model: string }[]>;
  default_embedding: string | null;
};

function resolveModel(idOrAuto: string, free: number | null): ModelEntry | null {
  if (idOrAuto.startsWith("auto:")) {
    const rules = registry.adaptive[idOrAuto.slice(5)] ?? [];
    const f = free ?? 0;
    const chosen = rules.filter((r) => f >= r.min_free_vram_mib).sort((a, b) => b.min_free_vram_mib - a.min_free_vram_mib)[0]
      ?? rules.sort((a, b) => a.min_free_vram_mib - b.min_free_vram_mib)[0];
    return chosen ? registry.models.find((m) => m.id === chosen.model) ?? null : null;
  }
  return registry.models.find((m) => m.id === idOrAuto) ?? null;
}

function assess(): Assessment {
  const s = loadSettings();
  const gpu = MOCK_HW.gpus[0] ?? null;
  const free = gpu?.free_mib ?? null;
  const manual = s.models.tier_mode === "manual" && s.models.manual_tier;
  const tier: Tier = manual ? s.models.manual_tier! : tierFromFree(free);
  const recommended = resolveModel(registry.tier_defaults[tier], free);
  const active = s.models.active_model === "auto" ? recommended : registry.models.find((m) => m.id === s.models.active_model) ?? null;
  return {
    tier: {
      tier,
      basis: manual ? "manual_override" : gpu ? "nvml_free_vram" : "no_gpu",
      free_vram_mib: free,
      total_vram_mib: gpu?.total_mib ?? null,
      gpu_name: gpu?.name ?? null,
      warnings: tier === "cpu_only" ? ["hw.warn.cpu_only_slow"] : [],
    },
    recommended_model: recommended,
    active_model: active,
    active_model_installed: active && current ? { installed: false, folder: null, missing_files: active.files.map((f) => f.name) } : null,
  };
}

// --- minimal pre-router mirror (slash + a few phrases) -------------------
const SCREEN_ALIASES: Record<Screen, string[]> = {
  home: ["홈", "대시보드", "home", "dashboard"],
  library: ["라이브러리", "문서", "library", "documents"],
  search: ["검색", "search"],
  assistant: ["어시스턴트", "assistant", "chat"],
  review: ["검토", "검토큐", "review"],
  schemas: ["스키마", "양식", "schemas", "schema", "forms"],
  knowledge: ["지식", "지식베이스", "knowledge", "kb"],
  generate: ["문서생성", "generate"],
  projects: ["프로젝트", "projects"],
  organize: ["정리", "내보내기", "organize", "export"],
  jobs: ["작업", "jobs"],
  models: ["모델", "모델관리자", "models", "engine"],
  settings: ["설정", "settings", "preferences"],
};

function parseScreen(s: string): Screen | null {
  const key = s.toLowerCase().replace(/[\s_-]/g, "").replace(/(으로|에서|을|를|로|은|는|이|가|에)$/, "");
  for (const [screen, aliases] of Object.entries(SCREEN_ALIASES) as [Screen, string[]][]) {
    if (aliases.includes(key)) return screen;
  }
  return null;
}

function parseTheme(s: string): Theme | null {
  const t = s.toLowerCase().replace(/[\s_]/g, "-");
  if (["light", "라이트", "밝게", "화이트"].includes(t)) return "light";
  if (["dark", "다크", "어둡게"].includes(t)) return "dark";
  if (["system", "시스템", "자동"].includes(t)) return "system";
  if (["high-contrast", "고대비", "contrast"].includes(t)) return "high-contrast";
  return null;
}

function parseLanguage(s: string): Language | null {
  const t = s.toLowerCase();
  if (["ko", "한국어", "한글", "korean"].includes(t)) return "ko";
  if (["en", "영어", "english"].includes(t)) return "en";
  return null;
}

export function mockRoute(text: string): PreRouteResult {
  const t = text.trim();
  const none: PreRouteResult = { matched: false, source: "none", confidence: 0, normalized_text: t };
  const hit = (op: PreRoutedOp, source: "slash" | "phrase"): PreRouteResult => ({
    matched: true,
    source,
    confidence: source === "slash" ? 1 : 0.9,
    normalized_text: t,
    ...op,
  });
  if (!t) return none;
  if (t.startsWith("/")) {
    const [cmdRaw, ...rest] = t.slice(1).split(/\s+/);
    const cmd = (cmdRaw ?? "").toLowerCase();
    const arg = rest.join(" ");
    if (["도움말", "help", "?"].includes(cmd)) return hit({ op: "help" }, "slash");
    if (["테마", "theme"].includes(cmd)) {
      const theme = parseTheme(arg);
      return theme ? hit({ op: "set_theme", theme }, "slash") : hit({ op: "open", screen: "settings" }, "slash");
    }
    if (["언어", "lang", "language"].includes(cmd)) {
      const language = parseLanguage(arg);
      return language ? hit({ op: "set_language", language }, "slash") : hit({ op: "open", screen: "settings" }, "slash");
    }
    if (["열기", "open", "go", "이동"].includes(cmd)) {
      const screen = parseScreen(arg);
      return screen ? hit({ op: "open", screen }, "slash") : none;
    }
    const screen = parseScreen(cmd);
    return screen ? hit({ op: "open", screen }, "slash") : none;
  }
  const compact = t.toLowerCase().replace(/\s+/g, "");
  if (/(도움말|뭘할수있|help|whatcanyoudo)/.test(compact) && compact.length <= 24) return hit({ op: "help" }, "phrase");
  const verb = /(바꿔|바꾸|변경|전환|적용|해줘|켜|으로|switch|change|set|use|enable|turn|make)/.test(t.toLowerCase());
  if (/(테마|모드|theme|mode|고대비|contrast)/.test(compact) && (verb || compact.length <= 12)) {
    for (const [word, theme] of [
      ["고대비", "high-contrast"],
      ["high-contrast", "high-contrast"],
      ["highcontrast", "high-contrast"],
      ["다크", "dark"],
      ["어둡게", "dark"],
      ["dark", "dark"],
      ["라이트", "light"],
      ["밝게", "light"],
      ["light", "light"],
      ["시스템", "system"],
      ["system", "system"],
    ] as [string, Theme][]) {
      if (compact.includes(word)) return hit({ op: "set_theme", theme }, "phrase");
    }
  }
  if (/(언어|language|ui)/.test(compact) || verb) {
    for (const [word, language] of [["한국어", "ko"], ["한글", "ko"], ["korean", "ko"], ["영어", "en"], ["english", "en"]] as [string, Language][]) {
      if (compact.includes(word) && compact.length <= 30) return hit({ op: "set_language", language }, "phrase");
    }
  }
  if (/(열어|열기|보여|이동|가자|가줘|open|show|go to|goto)/.test(t.toLowerCase())) {
    const cleaned = t.toLowerCase().replace(/(열어|열기|보여|이동|가자|가줘|open|show|go to|goto|the|screen|page|화면|페이지|탭|줘|주세요|to)/g, " ");
    for (const token of cleaned.split(/\s+/).filter(Boolean)) {
      const screen = parseScreen(token);
      if (screen) return hit({ op: "open", screen }, "phrase");
    }
  }
  return none;
}

function resolveDate(text: string): DateRange | null {
  const today = new Date();
  const y = today.getFullYear();
  const m = today.getMonth(); // 0-based
  const iso = (d: Date) => d.toISOString().slice(0, 10);
  const compact = text.toLowerCase().replace(/\s+/g, "");
  const monthRange = (yy: number, mm: number, label: string): DateRange => ({
    from: iso(new Date(Date.UTC(yy, mm, 1))),
    to: iso(new Date(Date.UTC(yy, mm + 1, 0))),
    label,
  });
  if (/(지난달|저번달|lastmonth)/.test(compact)) return monthRange(y, m - 1, "last_month");
  if (/(이번달|thismonth)/.test(compact)) return monthRange(y, m, "this_month");
  if (/(어제|yesterday)/.test(compact)) {
    const d = new Date(Date.UTC(y, m, today.getDate() - 1));
    return { from: iso(d), to: iso(d), label: "yesterday" };
  }
  if (/(오늘|today)/.test(compact)) {
    const d = new Date(Date.UTC(y, m, today.getDate()));
    return { from: iso(d), to: iso(d), label: "today" };
  }
  const n = compact.match(/(?:최근|last|past)(\d+)(?:일|days?)/);
  if (n) {
    const days = Math.max(1, Number(n[1]));
    return { from: iso(new Date(Date.UTC(y, m, today.getDate() - days + 1))), to: iso(new Date(Date.UTC(y, m, today.getDate()))), label: `last_${days}_days` };
  }
  return null;
}

export async function mockInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  resolveStartup();
  const a = (args ?? {}) as Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
  const r = (v: unknown) => v as T;
  switch (cmd) {
    case "app_info":
      return r({
        name: "DocuLens",
        version: "0.1.0",
        settings_path: "(browser) localStorage:doculens.mock.settings",
        app_data_dir: "(browser)",
        os: "browser",
        arch: "wasm",
        debug: true,
      } satisfies AppInfo);
    case "startup_status": {
      const s = loadSettings();
      const status: StartupStatus = {
        settings: { kind: "loaded" },
        settings_path: "(browser) localStorage",
        workspace: current ? { kind: "opened", path: current.path } : s.active_workspace ? { kind: "missing", path: s.active_workspace } : { kind: "none" },
      };
      return r(status);
    }
    case "settings_get":
      return r(loadSettings());
    case "settings_update": {
      const incoming = a.settings as AppSettings;
      const prev = loadSettings();
      return r(saveSettings({ ...incoming, active_workspace: prev.active_workspace }));
    }
    case "settings_set_theme": {
      const s = loadSettings();
      s.ui.theme = a.theme as Theme;
      return r(saveSettings(s));
    }
    case "settings_set_language": {
      const s = loadSettings();
      s.ui.language = a.language as Language;
      return r(saveSettings(s));
    }
    case "settings_set_ui_scale": {
      const s = loadSettings();
      s.ui.scale_percent = Number(a.scalePercent);
      return r(saveSettings(s));
    }
    case "settings_set_active_model": {
      const s = loadSettings();
      const id = String(a.modelId);
      if (id !== "auto" && !registry.models.some((m) => m.id === id)) throw err("unknown_model", `unknown model id: ${id}`);
      s.models.active_model = id;
      return r(saveSettings(s));
    }
    case "settings_set_tier_mode": {
      const s = loadSettings();
      s.models.tier_mode = a.mode as TierMode;
      s.models.manual_tier = (a.manualTier as Tier | null) ?? null;
      return r(saveSettings(s));
    }
    case "workspace_inspect": {
      const p = normalizePath(String(a.path));
      const ws = loadWorkspaces().find((w) => w.path === p);
      const insp: WorkspaceInspection = {
        path: p,
        exists: true,
        is_dir: true,
        initialized: !!ws,
        manifest: ws?.manifest ?? null,
        lock_state: "free",
        lock: null,
        document_files: ws ? 0 : 3,
        unsafe_location: /^([A-Za-z]:)?[\\/]?$/.test(p),
      };
      return r(insp);
    }
    case "workspace_current":
      return r(current ? view(current) : null);
    case "workspace_open": {
      const { path, force_unlock: _force } = a.args as { path: string; force_unlock: boolean };
      return r(openWorkspace(path, false, null, null));
    }
    case "workspace_create": {
      const { path, name, locale } = a.args as { path: string; name: string | null; locale: Language | null };
      return r(openWorkspace(path, true, name, locale));
    }
    case "workspace_close": {
      current = null;
      const s = loadSettings();
      s.active_workspace = null;
      saveSettings(s);
      return r(undefined);
    }
    case "workspace_forget": {
      const p = normalizePath(String(a.path));
      const s = loadSettings();
      s.workspaces = s.workspaces.filter((w) => w.path !== p);
      if (s.active_workspace === p) s.active_workspace = null;
      return r(saveSettings(s));
    }
    case "workspace_set_retention": {
      if (!current) throw err("no_workspace", "no workspace is open");
      const retention = a.retention as RetentionPolicy;
      if (retention.purge_hour_local > 23) throw err("invalid", "purge_hour_local must be 0-23");
      current.manifest.retention = retention;
      const list = loadWorkspaces().map((w) => (w.path === current!.path ? current! : w));
      saveWorkspaces(list);
      return r(view(current));
    }
    case "workspace_set_preferred_model": {
      if (!current) throw err("no_workspace", "no workspace is open");
      current.manifest.preferred_model = (a.modelId as string | null) ?? null;
      saveWorkspaces(loadWorkspaces().map((w) => (w.path === current!.path ? current! : w)));
      return r(view(current));
    }
    case "hw_probe":
      return r({ ...MOCK_HW, probed_at: new Date().toISOString() });
    case "hw_assess":
      return r(assess());
    case "registry_list": {
      const models_dir = current ? view(current).models_dir : null;
      const out: RegistryView = {
        models: registry.models.map((m) => ({
          ...m,
          install: models_dir ? { installed: false, folder: null, missing_files: m.files.map((f) => f.name) } : null,
          total_size: m.files.reduce((acc, f) => acc + f.size, 0),
        })),
        tier_defaults: registry.tier_defaults,
        default_embedding: registry.default_embedding,
        models_dir,
      };
      return r(out);
    }
    case "registry_required_files_text": {
      const m = registry.models.find((x) => x.id === a.modelId);
      if (!m) throw err("unknown_model", `unknown model id: ${a.modelId}`);
      const target = current ? `${view(current).models_dir}/${m.id}` : `<workspace>/.doculens/models/${m.id}`;
      const lines = [
        `DocuLens - required model files for ${m.display} (${m.id})`,
        `License: ${m.license} (${m.license_url ?? ""})`,
        `Copy the files into: ${target}`,
        "",
        ...m.files.flatMap((f) => [
          `- ${f.name}`,
          `  size: ${f.size} bytes (~${(f.size / 1024 ** 3).toFixed(2)} GiB)`,
          `  sha256: ${f.sha256 ?? "(not pinned yet - verify after download)"}`,
          `  url: ${f.hf ? `https://huggingface.co/${f.hf}/resolve/main/${f.name}` : "(no public URL)"}`,
        ]),
        "",
        "Also copy LICENSE / NOTICE files from the source repository into the same folder.",
      ];
      return r(lines.join("\n"));
    }
    case "prerouter_route":
      return r(mockRoute(String(a.text)));
    case "prerouter_resolve_date":
      return r(resolveDate(String(a.text)));
    default:
      throw err("internal", `mock backend: unknown command ${cmd}`);
  }
}

/** Test helper: wipe all mock state. */
export function resetMock(): void {
  localStorage.removeItem(SETTINGS_KEY);
  localStorage.removeItem(WORKSPACES_KEY);
  current = null;
  startupResolved = false;
}
