// Browser-only mock of the Rust commands (used by `vite dev`, `vite preview`
// and Playwright). State lives in localStorage so reloads behave like the
// real settings.json. Only the subset of behaviour needed by the UI is
// reproduced; the Rust crate is the source of truth and has its own tests.
//
// Phase 1 adds a simulated llama-server (start/load/chat with timers), a
// simulated drop-in model folder (import / download / hash / delete) and the
// job book, all publishing the same events as the Rust shell.

import registryJson from "../../models/registry.json";
import type {
  AppInfo,
  AppSettings,
  Assessment,
  ChatTestResult,
  DateRange,
  DiscoveredModel,
  DocumentFilter,
  DocumentImportPlan,
  DocumentRecord,
  DocumentSort,
  DocumentView,
  DocumentsChanged,
  ExportRequest,
  GateStatus,
  ImportItem,
  ImportOptions,
  ImportSource,
  LibraryStats,
  OrganizePlan,
  PageLayout,
  PageView,
  PdfiumInfo,
  Plan,
  PlanContext,
  ProcessRequest,
  SourceKind,
  EngineBinaryInfo,
  EngineDevice,
  EngineStatus,
  HardwareSummary,
  ImportMode,
  ImportPlan,
  InstallState,
  Job,
  JobKind,
  JobPriority,
  JobState,
  Language,
  LogLine,
  ModelEntry,
  ModelHealth,
  ModelIndex,
  ModelInstanceStatus,
  PreRouteResult,
  PreRoutedOp,
  RegistryView,
  RetentionPolicy,
  ScannedFile,
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
import { addWords as addLayoutWords, applyEdit as applyLayoutEdit, synthLayout } from "./mock-layout";
import { historyView as textHistoryView, moveTarget as textMoveTarget, recordEdit as recordTextEdit, recordMove as recordTextMove, revertTargets as textRevertTargets, type MockTextHistoryRow, type MockTextState } from "./mock-text-history";
import { defaultImportOptions, EVENTS, type ImportReport } from "./types";
import type { DraftJobSpec } from "./mock-generate";

const SETTINGS_KEY = "doculens.mock.settings";
const WORKSPACES_KEY = "doculens.mock.workspaces";
const MODELS_KEY = "doculens.mock.models";
/** Persistent jobs per workspace (`{ [workspace]: { job, spec }[] }`), restored on the next load like the shell's `jobs` table. */
const JOBS_KEY = "doculens.mock.jobs";

/** Speed-up factor for simulated work (Playwright sets `doculens.mock.fast`). */
export function tick(ms: number): number {
  try {
    return localStorage.getItem("doculens.mock.fast") ? Math.max(5, ms / 10) : ms;
  } catch {
    return ms;
  }
}

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
    ui: { theme: "system", language: "ko", scale_percent: 100, reduced_motion: "system", sidebar_collapsed: false, show_assistant_steps: false },
    models: {
      tier_mode: "auto",
      manual_tier: null,
      active_model: "auto",
      profile: "accuracy",
      backend: "auto",
      resident_models_max: 2,
      sleep_idle_seconds: 900,
      allow_online_download: true,
      auto_start_engine: true,
      warmup_after_load: true,
      vision_on_cpu: false,
      engine_binary: null,
    },
    processing: {
      auto_level: "classify_only",
      inbox_auto_level: "full",
      render_dpi: 200,
      review_threshold: 0.8,
      project_second_choice: 0.6,
      audit_sample_percent: 5,
      organize_reviewed_only: true,
      inbox_watch: true,
      ocr_digital_pdf: false,
      kie_max_pages: 4,
      precise_mode: false,
      dense_reocr: true,
      dense_cells: 80,
    },
    cache: { page_cache_max_gb: 5, keep_thumbnails: true },
    privacy: { pii_masking: true, db_encryption: false },
    notifications: { job_complete: true, prevent_sleep_during_jobs: false },
    knowledge: { auto_index: true, auto_embed: true, embedding_model: null, latest_only: true, allow_general_knowledge: false, context_tokens: 6000, top_k: 8, chunk_target_tokens: 500, min_vector_similarity: 0.5 },
    generation: { max_tokens_gpu: 32768, max_tokens_cpu: 8192 },
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

export function loadSettings(): AppSettings {
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
    knowledge: { ...d.knowledge, ...(stored.knowledge ?? {}) },
    generation: { ...d.generation, ...(stored.generation ?? {}) },
    shortcuts: { ...d.shortcuts, ...(stored.shortcuts ?? {}) },
    window: { ...d.window, ...(stored.window ?? {}) },
  };
}

function saveSettings(s: AppSettings): AppSettings {
  s.ui.scale_percent = Math.min(150, Math.max(90, s.ui.scale_percent));
  // mirrors AppSettings::normalize()
  s.models.resident_models_max = Math.min(4, Math.max(1, Math.round(s.models.resident_models_max) || 1));
  s.models.sleep_idle_seconds = Math.max(0, Math.round(s.models.sleep_idle_seconds) || 0);
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

/** Path of the open mock workspace (null when none). */
/** The open workspace as the UI sees it (`workspace_current`), for mocks that need its manifest. */
export function currentWorkspaceView(): WorkspaceView | null {
  return current ? view(current) : null;
}

export function currentWorkspacePath(): string | null {
  return current?.path ?? null;
}

/** True when a job was cancelled from the UI (shared with the knowledge mock). */
export function isCancelled(jobId: string): boolean {
  return cancelled.has(jobId);
}

export function err(code: string, message: string, detail?: unknown): UiError {
  return { code, message, detail };
}

function normalizePath(p: string): string {
  return p.trim().replace(/[\\/]+$/, "");
}

function basename(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

function sep(path: string): string {
  return path.includes("\\") ? "\\" : "/";
}

function view(ws: MockWorkspace): WorkspaceView {
  const s = sep(ws.path);
  return {
    path: ws.path,
    manifest: ws.manifest,
    models_dir: `${ws.path}${s}.doculens${s}models`,
    inbox_dir: `${ws.path}${s}inbox`,
    documents_dir: `${ws.path}${s}documents`,
    exports_dir: `${ws.path}${s}exports`,
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
        app_version: "1.0.0",
        last_opened_app_version: "1.0.0",
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
  if (current && current.path !== ws.path) detachJobs();
  current = ws;
  const s = loadSettings();
  rememberWorkspace(s, p, ws.manifest.name);
  saveSettings(s);
  afterWorkspaceChanged();
  restoreJobs();
  return view(ws);
}

function resolveStartup(): void {
  if (startupResolved) return;
  startupResolved = true;
  const s = loadSettings();
  if (s.active_workspace) {
    const ws = loadWorkspaces().find((w) => w.path === s.active_workspace);
    if (ws) {
      current = ws;
      restoreJobs();
    }
  }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

const listeners = new Map<string, Set<(payload: unknown) => void>>();

export function mockSubscribe<T>(event: string, handler: (payload: T) => void): () => void {
  const set = listeners.get(event) ?? new Set();
  const h = handler as (payload: unknown) => void;
  set.add(h);
  listeners.set(event, set);
  return () => {
    set.delete(h);
  };
}

export function emit(event: string, payload: unknown): void {
  const set = listeners.get(event);
  if (!set) return;
  for (const h of Array.from(set)) {
    try {
      h(payload);
    } catch (e) {
      console.error("[mock] listener failed", event, e);
    }
  }
}

// ---------------------------------------------------------------------------
// Hardware / registry
// ---------------------------------------------------------------------------

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
  return mergedModels().find((m) => m.id === idOrAuto) ?? null;
}

// ---------------------------------------------------------------------------
// Drop-in model folder (simulated)
// ---------------------------------------------------------------------------

interface MockModel {
  folder_name: string;
  id: string;
  display: string;
  files: { name: string; size: number; sha256: string | null }[];
  mmproj: string | null;
  arch: string;
  quant: string | null;
  registry_id: string | null;
  hashed: boolean;
  health: ModelHealth;
}

/** Embedding architectures (bge-m3-style BERT drop-ins and EmbeddingGemma 2). */
function isEmbeddingArch(arch: string): boolean {
  return arch === "bert" || arch === "xlm-roberta" || arch === "nomic-bert" || arch.includes("embedding");
}

function loadModels(): MockModel[] {
  return read<MockModel[]>(MODELS_KEY, []);
}

function saveModels(list: MockModel[]): void {
  write(MODELS_KEY, list);
}

function modelsDir(): string | null {
  return current ? view(current).models_dir : null;
}

function scannedFile(folder: string, name: string, size: number, sha: string | null, mmproj: boolean, m: MockModel): ScannedFile {
  return {
    path: `${folder}${sep(folder)}${name}`,
    name,
    size,
    modified: new Date().toISOString(),
    stable: true,
    meta: {
      kind: mmproj ? "mmproj" : "llm",
      architecture: mmproj ? "clip" : m.arch,
      name: m.display,
      file_type: null,
      quantization: mmproj ? "F16" : m.quant,
      license: "apache-2.0",
      license_link: null,
      source_url: null,
      source_repo: null,
      has_chat_template: !mmproj,
      block_count: mmproj ? null : 42,
      context_length: mmproj ? null : 131072,
      embedding_length: null,
      head_count: null,
      head_count_kv: mmproj ? null : 2,
      key_length: mmproj ? null : 512,
      full_attention_interval: null,
      split_count: null,
      split_no: null,
      projector_type: mmproj ? "gemma4v" : null,
      has_vision_encoder: mmproj ? true : null,
      has_audio_encoder: null,
      tensor_count: mmproj ? 400 : 500,
      tensor_types: {},
      gdn_tensor_types: {},
      gdn_low_precision: [],
      parameter_count: null,
      data_bytes: size,
    },
    hashes: sha ? { size, sha256: sha, blake3: sha } : null,
    error: null,
  };
}

function discovered(): DiscoveredModel[] {
  const dir = modelsDir();
  if (!dir) return [];
  return loadModels().map((m) => {
    const folder = `${dir}${sep(dir)}${m.folder_name}`;
    const weights = m.files.filter((f) => f.name !== m.mmproj).map((f) => scannedFile(folder, f.name, f.size, f.sha256, false, m));
    const mm = m.files.find((f) => f.name === m.mmproj);
    return {
      folder,
      folder_name: m.folder_name,
      id: m.id,
      display: m.display,
      weights,
      mmproj: mm ? scannedFile(folder, mm.name, mm.size, mm.sha256, true, m) : null,
      extra_ggufs: [],
      model_json: null,
      license_files: ["LICENSE"],
      architecture: m.arch,
      quantization: m.quant,
      has_chat_template: true,
      has_vision: !!m.mmproj,
      role: isEmbeddingArch(m.arch) ? "embedding" : m.mmproj ? "vlm" : "llm",
      total_bytes: m.files.reduce((a, f) => a + f.size, 0),
      health: m.health,
      warnings: m.registry_id && !m.hashed ? ["models.warn.unverified_hash"] : [],
      registry_match: m.registry_id ? { id: m.registry_id, by: m.hashed ? "hash" : "name", verified: m.hashed ? true : null } : null,
      hashed: m.hashed,
      scanned_at: new Date().toISOString(),
    };
  });
}

function modelIndex(): ModelIndex {
  const dir = modelsDir();
  return { discovered: discovered(), roots: dir ? [dir] : [], scanned_at: new Date().toISOString() };
}

function userAddedEntries(): ModelEntry[] {
  return loadModels()
    .filter((m) => !m.registry_id)
    .map((m) => ({
      id: m.id,
      family: m.arch,
      role: isEmbeddingArch(m.arch) ? "embedding" : m.mmproj ? "vlm" : "llm",
      display: m.display,
      license: "apache-2.0",
      license_url: null,
      source_repo: null,
      arch: m.arch,
      modalities: m.mmproj ? ["text", "image"] : ["text"],
      ctx_max: 131072,
      files: m.files.map((f) => ({ name: f.name, sha256: f.sha256, size: f.size, hf: null })),
      requirements: { min_free_vram_mib: Math.round(m.files.reduce((a, f) => a + f.size, 0) / 1048576) + 1536, min_ram_mib: 8192, tiers: ["vram_8gb", "vram_12gb", "vram_16gb", "vram_24gb_plus"] },
      defaults: { ctx: 32768, kv: "q8_0" },
      capabilities: { json_schema: true },
      prompt_profile: "gemma4-doc-v1",
      verified: m.hashed,
      user_added: true,
    }));
}

function mergedModels(): ModelEntry[] {
  return [...registry.models, ...userAddedEntries()];
}

function installState(entry: ModelEntry): InstallState | null {
  const dir = modelsDir();
  if (!dir) return null;
  const m = loadModels().find((x) => x.registry_id === entry.id || (!x.registry_id && x.id === entry.id));
  if (!m) {
    return { installed: false, folder: `${dir}${sep(dir)}${entry.id}`, missing_files: entry.files.map((f) => f.name), verified: null, health: null, matched_by: null, warnings: [] };
  }
  return {
    installed: m.health.kind === "ready",
    folder: `${dir}${sep(dir)}${m.folder_name}`,
    missing_files: m.health.kind === "incomplete" ? m.health.missing : [],
    verified: m.hashed ? true : null,
    health: m.health,
    matched_by: m.registry_id ? (m.hashed ? "hash" : "name") : null,
    warnings: m.registry_id && !m.hashed ? ["models.warn.unverified_hash"] : [],
  };
}

function addRegistryModel(entry: ModelEntry, folderName = entry.id): void {
  const list = loadModels().filter((m) => m.folder_name !== folderName);
  list.push({
    folder_name: folderName,
    id: entry.id,
    display: entry.display,
    files: entry.files.map((f) => ({ name: f.name, size: f.size, sha256: null })),
    mmproj: entry.files.find((f) => f.name.toLowerCase().startsWith("mmproj"))?.name ?? null,
    arch: entry.arch,
    quant: entry.id.includes("q6k") ? "Q6_K" : entry.id.includes("q5km") ? "Q5_K_M" : entry.id.includes("q4km") ? "Q4_K_M" : "Q8_0",
    registry_id: entry.id,
    hashed: false,
    health: { kind: "ready" },
  });
  saveModels(list);
  emit(EVENTS.modelsChanged, modelIndex());
}

function assess(): Assessment {
  const s = loadSettings();
  const gpu = MOCK_HW.gpus[0] ?? null;
  const free = gpu?.free_mib ?? null;
  const manual = s.models.tier_mode === "manual" && s.models.manual_tier;
  const tier: Tier = manual ? s.models.manual_tier! : tierFromFree(free);
  const recommended = resolveModel(registry.tier_defaults[tier], free);
  const active = s.models.active_model === "auto" ? recommended : mergedModels().find((m) => m.id === s.models.active_model) ?? null;
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
    active_model_installed: active ? installState(active) : null,
  };
}

// ---------------------------------------------------------------------------
// Jobs: lightweight shell jobs + the persistent runner (P5)
//
// Mirrors src-tauri/src/jobs.rs: persistent jobs carry a replayable spec, are
// scheduled by priority through lanes (one engine-bound job at a time, up to
// IO_LIMIT I/O jobs alongside), can be paused / resumed / retried /
// prioritised, and survive a reload (stored per workspace in localStorage;
// jobs that were running come back as `interrupted`).
// ---------------------------------------------------------------------------

const MAX_FINISHED = 50;
/** Overlapping I/O-lane persistent jobs (exports, organise …). */
const IO_LIMIT = 2;
const PRIORITY_RANK: Record<JobPriority, number> = { low: 0, normal: 1, high: 2, interactive: 3 };

export function isFinishedState(state: JobState): boolean {
  return state === "done" || state === "failed" || state === "cancelled" || state === "interrupted";
}

/** Replayable description of a persistent job (`doculens_core::jobs::JobSpec`). */
export type JobSpec =
  | { kind: "pipeline"; req: ProcessRequest }
  | { kind: "plan"; plan: Plan; text: string; context: PlanContext }
  | { kind: "export"; req: ExportRequest }
  | { kind: "organize"; plan: OrganizePlan; label: string; source: string; batch_kind: string }
  | { kind: "undo"; batch_id: string | null }
  | { kind: "redo" }
  | DraftJobSpec
  | { kind: "maintenance" }
  | { kind: "schema_discovery"; doc_ids: string[]; name_hint: string | null };

export type JobLane = "engine" | "io";

const MODEL_OPS: ReadonlySet<string> = new Set(["classify", "ocr", "extract", "answer", "summarize", "import", "draft_document", "explain", "compare", "define_rule", "request_supplement"]);

/** Ops that need the vision / chat model (`OpKind::needs_model`). */
export function planNeedsModel(plan: Plan): boolean {
  return plan.operations.some((op) => MODEL_OPS.has(op.op));
}

/** Engine-lane jobs run one at a time; I/O jobs may overlap with them (`JobSpec::lane`). */
export function specLane(spec: JobSpec): JobLane {
  if (spec.kind === "pipeline" || (spec.kind === "plan" && planNeedsModel(spec.plan)) || (spec.kind === "draft" && !spec.prepared) || spec.kind === "schema_discovery") return "engine";
  return "io";
}

/** Handed to a persistent job's worker (`doculens_core::jobs::JobControl`). */
export interface JobControl {
  readonly id: string;
  cancelled(): boolean;
  paused(): boolean;
  /** Between documents / pages: waits while paused, rejects with `cancelled` when cancelled. */
  checkpoint(): Promise<void>;
}

/** What a worker returns: the finishing detail and the structured outcome (`Job.result`). */
export interface WorkerOutcome {
  detail: string | null;
  result?: unknown;
}

/** Thrown by a worker: the message becomes `Job.error` (`cancelled` → state cancelled), `result` the partial outcome. */
export interface WorkerFailure {
  message: string;
  result?: unknown;
}

export function workerFailure(message: string, result?: unknown): WorkerFailure {
  return { message, result };
}

export function cancelledError(): UiError {
  return err("cancelled", "cancelled");
}

interface StoredJob {
  job: Job;
  spec: JobSpec | null;
}

let jobs: Job[] = [];
const cancelled = new Set<string>();
const paused = new Set<string>();
const specs = new Map<string, JobSpec>();
const controls = new Map<string, JobControl>();
let gateWaiting = 0;

/**
 * `jobs://update` carries the job list without `result` (Rust review M8: the structured outcome is
 * large and the event fires on every progress tick); `jobs_list` / `jobs_get` return it.
 */
function broadcastJobs(): void {
  emit(
    EVENTS.jobsUpdate,
    jobs.map((j) => {
      const { result: _result, ...rest } = j;
      return rest;
    }),
  );
}

/** `jobs_list` / `jobs_get` shape: `result` present (null when the job has none), like serde's `Option<Value>`. */
export function withResult(job: Job): Job {
  return { ...job, result: job.result ?? null };
}

function pushJob(job: Job): void {
  const list = jobs.filter((j) => j.id !== job.id);
  list.push(job);
  // Trim the oldest finished jobs beyond the cap (like the shell's job book).
  const finished = list.filter((j) => isFinishedState(j.state));
  if (finished.length > MAX_FINISHED) {
    const drop = new Set(finished.slice(0, finished.length - MAX_FINISHED).map((j) => j.id));
    jobs = list.filter((j) => !drop.has(j.id));
  } else {
    jobs = list;
  }
}

export function createJob(kind: JobKind, subject: string): Job {
  const now = new Date().toISOString();
  const job: Job = {
    id: crypto.randomUUID().replace(/-/g, ""),
    kind,
    subject,
    state: "queued",
    progress: null,
    detail: null,
    bytes_done: null,
    bytes_total: null,
    error: null,
    cancel_requested: false,
    created_at: now,
    updated_at: now,
    finished_at: null,
    priority: "normal",
    persistent: false,
    attempts: 1,
    result: undefined,
    label: null,
  };
  pushJob(job);
  broadcastJobs();
  return job;
}

export function getJob(id: string): Job | null {
  return jobs.find((j) => j.id === id) ?? null;
}

export function listJobs(): Job[] {
  return jobs;
}

function persistJobs(): void {
  const ws = current?.path;
  if (!ws) return;
  const all = read<Record<string, StoredJob[]>>(JOBS_KEY, {});
  all[ws] = jobs.filter((j) => j.persistent).map((j) => ({ job: j, spec: specs.get(j.id) ?? null }));
  write(JOBS_KEY, all);
}

export function updateJob(id: string, patch: Partial<Job>): void {
  let persistent = false;
  jobs = jobs.map((j) => {
    if (j.id !== id) return j;
    persistent = j.persistent;
    // Lightweight jobs start on their first update; persistent ones are started by the scheduler.
    const state = patch.state ?? (j.state === "queued" && !j.persistent ? "running" : j.state);
    return { ...j, ...patch, state, updated_at: new Date().toISOString() };
  });
  if (persistent) persistJobs();
  broadcastJobs();
}

export function finishJob(id: string, result: { ok: true; detail?: string; result?: unknown } | { ok: false; error: string; result?: unknown }): void {
  const wasCancelled = cancelled.has(id);
  cancelled.delete(id);
  updateJob(id, {
    state: result.ok ? "done" : wasCancelled ? "cancelled" : "failed",
    progress: result.ok ? 1 : undefined,
    detail: result.ok ? (result.detail ?? null) : undefined,
    error: result.ok || wasCancelled ? null : result.error,
    finished_at: new Date().toISOString(),
    ...(result.result !== undefined ? { result: result.result } : {}),
  });
}

/** True while a job of `kind` is queued, running or paused. */
export function hasActiveJob(kind: JobKind): boolean {
  return jobs.some((j) => j.kind === kind && !isFinishedState(j.state));
}

/** Runs `steps` ticks of simulated work; stalls while the job is paused, resolves false when cancelled. */
export function simulate(jobId: string, steps: number, stepMs: number, onTick: (i: number) => void): Promise<boolean> {
  return new Promise((resolve) => {
    let i = 0;
    const run = () => {
      if (cancelled.has(jobId)) {
        resolve(false);
        return;
      }
      if (paused.has(jobId)) {
        setTimeout(run, tick(100));
        return;
      }
      i += 1;
      onTick(i);
      if (i >= steps) {
        resolve(true);
        return;
      }
      setTimeout(run, tick(stepMs));
    };
    setTimeout(run, tick(stepMs));
  });
}

export function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

/** One unit of simulated work for a persistent job: waits `ms` (sped up by `doculens.mock.fast`), then honours pause / cancel. */
export async function pace(ctl: JobControl | null, ms: number): Promise<void> {
  await sleep(tick(ms));
  if (ctl) await ctl.checkpoint();
}

// -- persistent runner -------------------------------------------------------

/** Queues a persistent job (stored per workspace) and schedules it; returns its id. */
export function submitJob(spec: JobSpec, subject: string, priority: JobPriority = "normal"): string {
  if (!current) throw err("no_workspace", "no workspace is open");
  const now = new Date().toISOString();
  const job: Job = {
    id: crypto.randomUUID().replace(/-/g, ""),
    kind: spec.kind,
    subject,
    state: "queued",
    progress: null,
    detail: null,
    bytes_done: null,
    bytes_total: null,
    error: null,
    cancel_requested: false,
    created_at: now,
    updated_at: now,
    finished_at: null,
    priority,
    persistent: true,
    attempts: 0,
    result: undefined,
    label: spec.kind === "plan" ? spec.text : spec.kind === "organize" || spec.kind === "draft" ? spec.label : spec.kind === "schema_discovery" ? spec.name_hint : null,
  };
  specs.set(job.id, spec);
  pushJob(job);
  persistJobs();
  broadcastJobs();
  schedule();
  return job.id;
}

function laneOf(job: Job): JobLane {
  const spec = specs.get(job.id);
  return spec ? specLane(spec) : "io";
}

/** Starts the next runnable queued jobs (priority first, oldest first, lanes respected). */
export function schedule(): void {
  for (;;) {
    const active = jobs.filter((j) => j.persistent && (j.state === "running" || j.state === "paused"));
    const engineBusy = active.some((j) => laneOf(j) === "engine");
    const ioRunning = active.filter((j) => laneOf(j) === "io").length;
    const queued = jobs
      .filter((j) => j.persistent && j.state === "queued")
      .sort((a, b) => PRIORITY_RANK[b.priority] - PRIORITY_RANK[a.priority] || a.created_at.localeCompare(b.created_at));
    const next = queued.find((j) => (laneOf(j) === "engine" ? !engineBusy : ioRunning < IO_LIMIT));
    if (!next) return;
    startJob(next.id);
  }
}

function makeControl(id: string): JobControl {
  return {
    id,
    cancelled: () => cancelled.has(id),
    paused: () => paused.has(id),
    checkpoint: () =>
      new Promise<void>((resolve, reject) => {
        const poll = () => {
          if (cancelled.has(id)) {
            reject(cancelledError());
            return;
          }
          if (!paused.has(id)) {
            resolve();
            return;
          }
          setTimeout(poll, tick(100));
        };
        poll();
      }),
  };
}

function startJob(id: string): void {
  const spec = specs.get(id);
  const before = getJob(id);
  if (!spec || !before) {
    updateJob(id, { state: "failed", error: "spec missing", finished_at: new Date().toISOString() });
    return;
  }
  const ctl = makeControl(id);
  controls.set(id, ctl);
  updateJob(id, { state: "running", attempts: before.attempts + 1, error: null });
  const job = getJob(id) ?? before;
  void (async () => {
    let state: JobState = "done";
    let error: string | null = null;
    let detail: string | null = null;
    let result: unknown;
    try {
      const worker = await import("./mock-planner");
      const out = await worker.runJobSpec(job, spec, ctl);
      detail = out.detail;
      result = out.result;
    } catch (e) {
      const f = e as Partial<WorkerFailure> | null;
      const message = typeof f?.message === "string" ? f.message : String(e);
      result = f?.result;
      if (cancelled.has(id) || message.includes("cancelled")) state = "cancelled";
      else {
        state = "failed";
        error = message;
      }
    }
    cancelled.delete(id);
    paused.delete(id);
    controls.delete(id);
    const patch: Partial<Job> = { state, error, finished_at: new Date().toISOString(), result };
    if (state === "done") patch.progress = 1;
    if (detail !== null) patch.detail = detail;
    updateJob(id, patch);
    schedule();
  })();
}

export function pauseJob(id: string): boolean {
  const j = getJob(id);
  if (!j || !j.persistent) return false;
  if (j.state === "running") {
    paused.add(id);
    updateJob(id, { state: "paused" });
    return true;
  }
  if (j.state === "queued") {
    updateJob(id, { state: "paused" });
    return true;
  }
  return false;
}

export function resumeJob(id: string): boolean {
  const j = getJob(id);
  if (!j || !j.persistent || j.state !== "paused") return false;
  if (controls.has(id)) {
    paused.delete(id);
    updateJob(id, { state: "running" });
  } else {
    updateJob(id, { state: "queued" });
    schedule();
  }
  return true;
}

/** Re-queues a finished persistent job with the same spec (same id, attempts keep counting). */
export function retryJob(id: string): boolean {
  if (!current) throw err("no_workspace", "no workspace is open");
  const j = getJob(id);
  if (!j || !j.persistent || !isFinishedState(j.state) || !specs.has(id)) return false;
  updateJob(id, { state: "queued", error: null, progress: null, detail: null, result: undefined, finished_at: null, cancel_requested: false });
  schedule();
  return true;
}

/** Moves a queued persistent job to the front of its lane (priority `high`). */
export function prioritizeJob(id: string): boolean {
  const j = getJob(id);
  if (!j || !j.persistent || j.state !== "queued") return false;
  const me: Job = { ...j, priority: "high", updated_at: new Date().toISOString() };
  const rest = jobs.filter((x) => x.id !== id);
  const at = rest.findIndex((x) => x.persistent && x.state === "queued");
  jobs = at < 0 ? [...rest, me] : [...rest.slice(0, at), me, ...rest.slice(at)];
  persistJobs();
  broadcastJobs();
  schedule();
  return true;
}

export function cancelJob(id: string): boolean {
  const j = getJob(id);
  if (!j || isFinishedState(j.state)) return false;
  if (j.persistent && (j.state === "queued" || j.state === "paused") && !controls.has(id)) {
    // Not started: finish it right away.
    paused.delete(id);
    updateJob(id, { state: "cancelled", error: null, finished_at: new Date().toISOString() });
    schedule();
    return true;
  }
  cancelled.add(id);
  updateJob(id, { cancel_requested: true });
  return true;
}

export function clearFinishedJobs(): void {
  jobs = jobs.filter((j) => !isFinishedState(j.state));
  persistJobs();
  broadcastJobs();
}

export function persistentQueued(): number {
  return jobs.filter((j) => j.persistent && j.state === "queued").length;
}

/** Workspace opened: stored persistent jobs come back; ones that were running when the page went away are `interrupted`. */
function restoreJobs(): void {
  const ws = current?.path;
  if (!ws) return;
  const stored = read<Record<string, StoredJob[]>>(JOBS_KEY, {})[ws] ?? [];
  const now = new Date().toISOString();
  for (const s of stored) {
    if (!s?.job) continue;
    const job: Job = { ...s.job };
    if (job.state === "running") {
      job.state = "interrupted";
      job.error = "interrupted";
      job.finished_at = now;
      job.updated_at = now;
    }
    if (s.spec) specs.set(job.id, s.spec);
    pushJob(job);
  }
  persistJobs();
  broadcastJobs();
  schedule();
}

/** Workspace closing: running / paused persistent jobs are cancelled and marked `interrupted`; all of them leave the in-memory list. */
function detachJobs(): void {
  const now = new Date().toISOString();
  jobs = jobs.map((j) => {
    if (!j.persistent || (j.state !== "running" && j.state !== "paused")) return j;
    cancelled.add(j.id);
    return { ...j, state: "interrupted" as const, error: "workspace closed", finished_at: now, updated_at: now };
  });
  persistJobs();
  for (const j of jobs) {
    if (!j.persistent) continue;
    paused.delete(j.id);
    controls.delete(j.id);
  }
  jobs = jobs.filter((j) => !j.persistent);
  specs.clear();
  broadcastJobs();
}

/** Engine gate (§3.9): a batch is "inside a page" while an engine-lane job runs. */
export function gateStatus(): GateStatus {
  const busy = jobs.some((j) => j.persistent && j.state === "running" && laneOf(j) === "engine");
  return { batch_busy: busy, waiting: gateWaiting, estimated_wait_ms: busy ? tick(120) : 0 };
}

/** Runs an interactive request at the next page boundary of a running batch (`EngineGate::interactive`). */
export async function gateInteractive<T>(f: () => Promise<T>): Promise<T> {
  if (gateStatus().batch_busy) {
    gateWaiting += 1;
    try {
      await sleep(tick(300));
    } finally {
      gateWaiting -= 1;
    }
  }
  return f();
}

// ---------------------------------------------------------------------------
// Engine (simulated llama-server router)
// ---------------------------------------------------------------------------

const MOCK_BINARY = { path: "C:\\Users\\me\\AppData\\Local\\DocuLens\\engines\\b12066\\cu121\\llama-server.exe", backend: "cu121" as const, version: "b12066", build_number: 12066, source: "app_data" as const };

let engine: EngineStatus = {
  state: { kind: "stopped" },
  binary: null,
  pid: null,
  host: "127.0.0.1",
  port: null,
  started_at: null,
  restarts: 0,
  last_error: null,
  last_exit_code: null,
  models: [],
  presets_path: null,
  active_model: null,
  sleep_idle_seconds: 900,
};
let engineLogs: LogLine[] = [];

function log(text: string, stream: LogLine["stream"] = "app"): void {
  const line: LogLine = { at: new Date().toISOString(), stream, text };
  engineLogs = [...engineLogs.slice(-499), line];
  emit(EVENTS.engineLog, line);
}

function setEngine(patch: Partial<EngineStatus>): void {
  engine = { ...engine, ...patch };
  emit(EVENTS.engineStatus, engine);
}

function setModel(id: string, patch: Partial<ModelInstanceStatus>): ModelInstanceStatus {
  const existing = engine.models.find((m) => m.id === id);
  const next: ModelInstanceStatus = {
    id,
    status: "unloaded",
    failed: false,
    exit_code: null,
    progress: null,
    stage: null,
    memory_effective_bytes: null,
    memory_min_bytes: null,
    mmproj_bytes: 0,
    input_modalities: ["text", "image"],
    loaded_at: null,
    warmed_up: false,
    ...existing,
    ...patch,
  };
  setEngine({ models: engine.models.some((m) => m.id === id) ? engine.models.map((m) => (m.id === id ? next : m)) : [...engine.models, next] });
  emit(EVENTS.engineModel, next);
  return next;
}

function loadableIds(): string[] {
  return discovered().filter((d) => d.health.kind === "ready").map((d) => d.id);
}

function presetsText(): string {
  const s = loadSettings();
  const lines = ["version = 1", "; generated by DocuLens (browser preview)", "", "[*]", "flash-attn = on", "cache-type-k = q8_0", "cache-type-v = q8_0", "fit = off", "n-gpu-layers = all", "image-min-tokens = 1024", "jinja = true", "metrics = true", "slots = false"];
  for (const d of discovered().filter((d) => d.health.kind === "ready")) {
    lines.push("", `[${d.id}]`, `model = ${d.weights[0]?.path ?? ""}`);
    if (d.mmproj) lines.push(`mmproj = ${d.mmproj.path}`);
    lines.push(`load-on-startup = ${assess().active_model?.id === d.id}`, `ctx-size = ${s.models.profile === "speed" ? 8192 : s.models.profile === "balanced" ? 16384 : 32768}`, "parallel = 1", "reasoning = off", "reasoning-budget = 0");
  }
  return lines.join("\n") + "\n";
}

async function engineStart(): Promise<EngineStatus> {
  if (!current) throw err("no_workspace", "no workspace is open");
  if (engine.state.kind === "running") return engine;
  setEngine({ state: { kind: "starting" }, binary: MOCK_BINARY, pid: 4242, port: 41805, started_at: new Date().toISOString(), presets_path: "(browser) presets.ini", models: [], last_error: null });
  log(`starting ${MOCK_BINARY.path} --host 127.0.0.1 --port 41805 --models-preset presets.ini --models-max 2 --models-memory-margin 1024 --sleep-idle-seconds 900 --jinja --metrics --no-slots`);
  await new Promise((r) => setTimeout(r, tick(400)));
  log("srv  server_on_li: listening on http://127.0.0.1:41805", "stderr");
  setEngine({ state: { kind: "running" }, models: loadableIds().map((id) => ({ ...setModelSilently(id) })) });
  log("engine ready on 127.0.0.1:41805 (pid 4242)");
  return engine;
}

function setModelSilently(id: string): ModelInstanceStatus {
  const d = discovered().find((x) => x.id === id);
  return {
    id,
    status: "unloaded",
    failed: false,
    exit_code: null,
    progress: null,
    stage: null,
    memory_effective_bytes: null,
    memory_min_bytes: null,
    mmproj_bytes: d?.mmproj?.size ?? 0,
    input_modalities: d?.has_vision ? ["text", "image"] : ["text"],
    loaded_at: null,
    warmed_up: false,
  };
}

async function engineStop(): Promise<void> {
  if (engine.state.kind === "stopped") return;
  setEngine({ state: { kind: "stopping" } });
  await new Promise((r) => setTimeout(r, tick(200)));
  log("engine stopped (pid 4242, exit Some(0))");
  setEngine({ state: { kind: "stopped" }, pid: null, port: null, models: [], active_model: null });
}

async function loadModelJob(modelId: string, warmup: boolean): Promise<string> {
  const existing = jobs.find((j) => j.kind === "model_load" && j.subject === modelId && (j.state === "queued" || j.state === "running"));
  if (existing) return existing.id;
  if (engine.state.kind !== "running") await engineStart();
  if (!loadableIds().includes(modelId)) throw err("unknown_model", `unknown model id: ${modelId}`);
  const job = createJob("model_load", modelId);
  void (async () => {
    updateJob(job.id, { detail: "loading" });
    log(`loading model ${modelId}`);
    setModel(modelId, { status: "loading", progress: 0, stage: "text_model" });
    const ok = await simulate(job.id, 8, 150, (i) => {
      const p = i / 8;
      setModel(modelId, { status: "loading", progress: p, stage: p < 0.7 ? "text_model" : "mmproj_model" });
      updateJob(job.id, { progress: p, detail: p < 0.7 ? "text_model" : "mmproj_model" });
    });
    if (!ok) {
      setModel(modelId, { status: "unloaded", progress: null, stage: null });
      finishJob(job.id, { ok: false, error: "cancelled" });
      return;
    }
    const d = discovered().find((x) => x.id === modelId);
    const bytes = Math.round((d?.total_bytes ?? 3.4e9) * 1.05) + 1.1e9;
    setModel(modelId, { status: "loaded", progress: null, stage: null, memory_effective_bytes: bytes, loaded_at: new Date().toISOString(), warmed_up: warmup });
    setEngine({ active_model: modelId });
    log(`warm-up done for ${modelId} in 420 ms`);
    finishJob(job.id, { ok: true, detail: `loaded (${bytes} bytes)` });
  })();
  return job.id;
}

function afterWorkspaceChanged(): void {
  emit(EVENTS.modelsChanged, modelIndex());
  const s = loadSettings();
  if (current && s.models.auto_start_engine) {
    void (async () => {
      if (engine.state.kind !== "running") {
        const job = createJob("engine_start", "engine");
        updateJob(job.id, { detail: "starting" });
        await engineStart();
        finishJob(job.id, { ok: true, detail: `running on port ${engine.port}` });
      }
      const a = assess();
      if (a.active_model && a.active_model_installed?.installed) {
        await loadModelJob(a.active_model.id, s.models.warmup_after_load);
      }
    })();
  } else if (!current) {
    void engineStop();
  }
}

// ---------------------------------------------------------------------------
// Import / download / hash / delete (simulated)
// ---------------------------------------------------------------------------

function importPlan(paths: string[], mode: ImportMode, folderName: string | null): ImportPlan {
  const dir = modelsDir();
  if (!dir) throw err("no_workspace", "no workspace is open");
  if (paths.length === 0) throw err("invalid", "no files selected");
  for (const p of paths) {
    if (!p.toLowerCase().endsWith(".gguf")) throw err("invalid", `not a .gguf file: ${basename(p)}`);
  }
  const files = paths.map((p) => {
    const name = basename(p);
    const isMm = name.toLowerCase().startsWith("mmproj");
    return { source: p, dest_name: name, size: isMm ? 660_000_000 : 3_400_000_000, meta: null, is_mmproj: isMm };
  });
  const weights = files.filter((f) => !f.is_mmproj);
  if (weights.length === 0) throw err("invalid", "select the language model .gguf file (mmproj alone is not a model)");
  let autoPaired = false;
  if (weights.length === 1 && !files.some((f) => f.is_mmproj)) {
    const base = weights[0]!.source.slice(0, -basename(weights[0]!.source).length);
    files.push({ source: `${base}mmproj-F16.gguf`, dest_name: "mmproj-F16.gguf", size: 990_000_000, meta: null, is_mmproj: true });
    autoPaired = true;
  }
  const stem = weights[0]!.dest_name.replace(/\.gguf$/i, "");
  const folder = (folderName?.trim() || stem).toLowerCase().replace(/[^a-z0-9._]+/g, "-").replace(/^-+|-+$/g, "") || "model";
  const already = loadModels().some((m) => m.folder_name === folder);
  return {
    folder_name: folder,
    target_folder: `${dir}${sep(dir)}${folder}`,
    files,
    license_files: [`${weights[0]!.source.slice(0, -basename(weights[0]!.source).length)}LICENSE`],
    total_bytes: files.reduce((a, f) => a + f.size, 0),
    mode,
    auto_paired_mmproj: autoPaired,
    architecture: "gemma4",
    quantization: /q6_k/i.test(stem) ? "Q6_K" : /q5_k_m/i.test(stem) ? "Q5_K_M" : /q4_k_m/i.test(stem) ? "Q4_K_M" : "F16",
    has_vision: files.some((f) => f.is_mmproj),
    warnings: [],
    already_present: already,
  };
}

function registryMatchFor(fileNames: string[]): ModelEntry | null {
  const lower = fileNames.map((n) => n.toLowerCase());
  return registry.models.find((m) => m.files.every((f) => lower.includes(f.name.toLowerCase()))) ?? null;
}

function importRun(plan: ImportPlan): string {
  const job = createJob("model_import", plan.folder_name);
  void (async () => {
    updateJob(job.id, { bytes_total: plan.total_bytes, detail: plan.mode });
    const ok = await simulate(job.id, 10, 120, (i) => {
      const done = Math.round((plan.total_bytes * i) / 10);
      updateJob(job.id, { progress: i / 10, bytes_done: done, bytes_total: plan.total_bytes, detail: plan.files[Math.min(plan.files.length - 1, Math.floor((i - 1) / (10 / plan.files.length)))]?.dest_name ?? null });
    });
    if (!ok) {
      finishJob(job.id, { ok: false, error: "cancelled" });
      return;
    }
    const match = registryMatchFor(plan.files.map((f) => f.dest_name));
    const list = loadModels().filter((m) => m.folder_name !== plan.folder_name);
    list.push({
      folder_name: plan.folder_name,
      id: match?.id ?? plan.folder_name,
      display: match?.display ?? `${plan.folder_name} (${plan.quantization ?? "?"})`,
      files: plan.files.map((f) => ({ name: f.dest_name, size: f.size, sha256: null })),
      mmproj: plan.files.find((f) => f.is_mmproj)?.dest_name ?? null,
      arch: plan.architecture ?? "gemma4",
      quant: plan.quantization,
      registry_id: match?.id ?? null,
      hashed: false,
      health: { kind: "ready" },
    });
    saveModels(list);
    emit(EVENTS.modelsChanged, modelIndex());
    if (engine.state.kind === "running") setModel(match?.id ?? plan.folder_name, {});
    finishJob(job.id, { ok: true, detail: plan.target_folder });
  })();
  return job.id;
}

function downloadJob(modelId: string): string {
  const s = loadSettings();
  if (!s.models.allow_online_download) throw err("invalid", "online downloads are disabled in settings");
  if (!modelsDir()) throw err("no_workspace", "no workspace is open");
  const entry = registry.models.find((m) => m.id === modelId);
  if (!entry) throw err("unknown_model", `unknown model id: ${modelId}`);
  if (jobs.some((j) => j.kind === "model_download" && j.subject === modelId && !isFinishedState(j.state))) throw err("invalid", "download already running");
  const job = createJob("model_download", modelId);
  const total = entry.files.reduce((a, f) => a + f.size, 0);
  void (async () => {
    updateJob(job.id, { bytes_total: total, detail: entry.files[0]?.name ?? null });
    const ok = await simulate(job.id, 20, 100, (i) => {
      updateJob(job.id, { progress: i / 20, bytes_done: Math.round((total * i) / 20), bytes_total: total, detail: entry.files[i > 16 ? entry.files.length - 1 : 0]?.name ?? null });
    });
    if (!ok) {
      finishJob(job.id, { ok: false, error: "cancelled" });
      return;
    }
    addRegistryModel(entry);
    if (engine.state.kind === "running") setModel(entry.id, {});
    finishJob(job.id, { ok: true, detail: `${modelsDir()}${sep(modelsDir()!)}${entry.id}` });
  })();
  return job.id;
}

function hashJob(folder: string): string {
  const list = loadModels();
  const m = list.find((x) => x.folder_name === folder || `${modelsDir()}${sep(modelsDir() ?? "/")}${x.folder_name}` === folder);
  if (!m) throw err("invalid", `unknown model folder ${folder}`);
  const job = createJob("model_hash", m.folder_name);
  const total = m.files.reduce((a, f) => a + f.size, 0);
  void (async () => {
    const ok = await simulate(job.id, 10, 100, (i) => updateJob(job.id, { progress: i / 10, bytes_done: Math.round((total * i) / 10), bytes_total: total, detail: m.files[0]?.name ?? null }));
    if (!ok) {
      finishJob(job.id, { ok: false, error: "cancelled" });
      return;
    }
    const updated = loadModels().map((x) => (x.folder_name === m.folder_name ? { ...x, hashed: true, files: x.files.map((f, i) => ({ ...f, sha256: `${(i + 1).toString(16).padStart(4, "0")}${"ab".repeat(30)}` })) } : x));
    saveModels(updated);
    emit(EVENTS.modelsChanged, modelIndex());
    finishJob(job.id, { ok: true, detail: m.files.map((f) => `${f.name}: ${"0001abab"}…`).join(", ") });
  })();
  return job.id;
}

function deleteModel(folder: string): void {
  const list = loadModels();
  const m = list.find((x) => x.folder_name === folder || `${modelsDir()}${sep(modelsDir() ?? "/")}${x.folder_name}` === folder);
  if (!m) throw err("invalid", `${folder} is not inside the models folder`);
  if (engine.models.some((x) => x.id === m.id && x.status === "loaded")) {
    setModel(m.id, { status: "unloaded", memory_effective_bytes: null, loaded_at: null, warmed_up: false });
    if (engine.active_model === m.id) setEngine({ active_model: null });
  }
  saveModels(list.filter((x) => x.folder_name !== m.folder_name));
  setEngine({ models: engine.models.filter((x) => x.id !== m.id) });
  // cancel a running load job for the deleted model
  for (const j of jobs.filter((j) => j.kind === "model_load" && j.subject === m.id && (j.state === "queued" || j.state === "running"))) cancelled.add(j.id);
  emit(EVENTS.modelsChanged, modelIndex());
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

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/** Local calendar date as `YYYY-MM-DD` (the shell uses `chrono::Local`). */
export function isoLocal(d: Date): string {
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

function addDays(d: Date, n: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
}

function subMonths(d: Date, n: number): Date {
  const first = new Date(d.getFullYear(), d.getMonth() - n, 1);
  const last = new Date(first.getFullYear(), first.getMonth() + 1, 0).getDate();
  return new Date(first.getFullYear(), first.getMonth(), Math.min(d.getDate(), last));
}

function dateRange(from: Date, to: Date, label: string): DateRange {
  return { from: isoLocal(from), to: isoLocal(to), label };
}

function monthRange(y: number, m0: number, label: string): DateRange {
  return dateRange(new Date(y, m0, 1), new Date(y, m0 + 1, 0), label);
}

function absoluteDate(compact: string, today: Date): DateRange | null {
  // YYYY년 M월 [D일] / YYYY-MM[-DD] / YYYY.MM[.DD] / YYYY/MM[/DD]
  const ymd = compact.match(/(?<!\d)((?:19|20|21)\d{2})(?:년|[-./])(\d{1,2})(?:월|[-./])?(\d{1,2})?일?/);
  if (ymd) {
    const y = Number(ymd[1]);
    const m = Number(ymd[2]);
    if (m >= 1 && m <= 12) {
      const d = ymd[3] ? Number(ymd[3]) : 0;
      if (d >= 1 && d <= new Date(y, m, 0).getDate()) return dateRange(new Date(y, m - 1, d), new Date(y, m - 1, d), "date");
      return monthRange(y, m - 1, "month");
    }
  }
  // YYYY년 / bare YYYY → the whole year
  const year = compact.match(/(?<!\d)((?:19|20|21)\d{2})(?!\d)/);
  if (year) {
    const y = Number(year[1]);
    return dateRange(new Date(y, 0, 1), new Date(y, 11, 31), "year");
  }
  // M월 [D일] → this year, or last year when the month lies in the future
  const md = compact.match(/(?<!\d)(\d{1,2})월(?:(\d{1,2})일)?/);
  if (md) {
    const m = Number(md[1]);
    if (m >= 1 && m <= 12) {
      const y = m - 1 > today.getMonth() ? today.getFullYear() - 1 : today.getFullYear();
      const d = md[2] ? Number(md[2]) : 0;
      if (d >= 1 && d <= new Date(y, m, 0).getDate()) return dateRange(new Date(y, m - 1, d), new Date(y, m - 1, d), "date");
      return monthRange(y, m - 1, "month");
    }
  }
  return null;
}

/** Mirror of `prerouter::resolve_relative_date`: relative phrases (어제 … 작년, 최근 N일/주/개월) and absolute dates. */
export function resolveRelativeDate(text: string, today: Date = new Date()): DateRange | null {
  const compact = text.trim().toLowerCase().replace(/\s+/g, "");
  const y = today.getFullYear();
  const m = today.getMonth(); // 0-based
  const base = new Date(y, m, today.getDate());
  const anyOf = (...words: string[]) => words.some((w) => compact.includes(w));
  const week = (d: Date, label: string) => {
    const monday = addDays(d, -((d.getDay() + 6) % 7));
    return dateRange(monday, addDays(monday, 6), label);
  };
  if (anyOf("그저께", "그제", "daybeforeyesterday")) return dateRange(addDays(base, -2), addDays(base, -2), "day_before_yesterday");
  if (anyOf("어제", "yesterday")) return dateRange(addDays(base, -1), addDays(base, -1), "yesterday");
  if (anyOf("오늘", "today")) return dateRange(base, base, "today");
  if (anyOf("지난주", "저번주", "lastweek", "previousweek")) return week(addDays(base, -7), "last_week");
  if (anyOf("이번주", "금주", "thisweek")) return week(base, "this_week");
  if (anyOf("지난달", "저번달", "전월", "lastmonth", "previousmonth")) return monthRange(y, m - 1, "last_month");
  if (anyOf("이번달", "당월", "금월", "thismonth")) return monthRange(y, m, "this_month");
  if (anyOf("작년", "지난해", "lastyear")) return dateRange(new Date(y - 1, 0, 1), new Date(y - 1, 11, 31), "last_year");
  if (anyOf("올해", "금년", "thisyear")) return dateRange(new Date(y, 0, 1), new Date(y, 11, 31), "this_year");
  // 최근 N일 / last N days / N주 / N개월 (counts clamped like the Rust resolver)
  for (const marker of ["최근", "지난", "last", "past"]) {
    const idx = compact.indexOf(marker);
    if (idx < 0) continue;
    const rest = compact.slice(idx + marker.length);
    const digits = rest.match(/^\d{1,9}/);
    if (!digits) continue;
    const n = Number(digits[0]);
    const after = rest.slice(digits[0].length);
    if (after.startsWith("일") || after.startsWith("day")) {
      const k = Math.min(36_500, Math.max(1, n));
      return dateRange(addDays(base, -(k - 1)), base, `last_${k}_days`);
    }
    if (after.startsWith("주") || after.startsWith("week")) {
      const k = Math.min(5_200, Math.max(1, n));
      return dateRange(addDays(base, -(k * 7 - 1)), base, `last_${k}_weeks`);
    }
    if (after.startsWith("개월") || after.startsWith("달") || after.startsWith("month")) {
      const k = Math.min(1_200, Math.max(1, n));
      return dateRange(subMonths(base, k), base, `last_${k}_months`);
    }
  }
  return absoluteDate(compact, today);
}

const MOCK_DEVICES: EngineDevice[] = [{ name: "CUDA0", description: "NVIDIA GeForce RTX 4060 (mock)", total_mib: 8188, free_mib: 7126 }];

// ---------------------------------------------------------------------------
// Documents (P2, simulated): import plan/run, rendering, rescan, viewer data
// ---------------------------------------------------------------------------

const DOCS_KEY = "doculens.mock.documents";

export interface MockPage {
  page_no: number;
  width_px: number;
  height_px: number;
  rotation: number;
  text: string | null;
  is_blank: boolean;
  rendered_at: string;
  text_source?: string | null;
  ocr_at?: string | null;
  page_doc_type?: string | null;
  classified_at?: string | null;
  /** Word layout after an on-image edit (synthesised from the text until then). */
  layout?: PageLayout | null;
}

export interface MockDoc extends DocumentRecord {
  workspace: string;
  pages: MockPage[];
  /** Page text corrections with their undo bookkeeping (documents/import.rs `page_text_history`). */
  text_history?: MockTextHistoryRow[];
}

/** Pipeline columns added in P4 (older stored documents get the defaults). */
const DOC_PIPELINE_DEFAULTS = { doc_type_source: null, classified_at: null, ocr_at: null, extracted_at: null, last_run_id: null, schema_id: null, review_status: "none" as const, confidence: null, pipeline_error: null };

export function loadDocs(): MockDoc[] {
  return read<MockDoc[]>(DOCS_KEY, []).map((d) => ({ ...DOC_PIPELINE_DEFAULTS, ...d }));
}

export function saveDocs(list: MockDoc[]): void {
  write(DOCS_KEY, list);
}

export function wsDocs(): MockDoc[] {
  const ws = current?.path;
  return loadDocs().filter((d) => d.workspace === ws && !d.deleted_at);
}

export function relDir(rel: string): string {
  const i = rel.lastIndexOf("/");
  return i >= 0 ? rel.slice(0, i) : "";
}

/** Ids of the models the planner may switch to (`set_model`): every registry / user model except embedders. */
export function listModelIds(): string[] {
  return mergedModels()
    .filter((m) => m.role !== "embedding")
    .map((m) => m.id);
}

function kindOf(name: string): SourceKind | null {
  const ext = name.toLowerCase().split(".").pop() ?? "";
  if (ext === "pdf") return "pdf";
  if (["png", "jpg", "jpeg", "tif", "tiff", "webp", "bmp", "gif"].includes(ext)) return "image";
  if (ext === "hwpx") return "hwpx";
  if (ext === "docx") return "docx";
  if (ext === "hwp") return "hwp";
  if (["xlsx", "xlsm"].includes(ext)) return "xlsx";
  if (["pptx", "ppsx"].includes(ext)) return "pptx";
  if (["txt", "md", "markdown", "csv", "tsv", "log", "json", "xml", "html", "htm"].includes(ext)) return "text";
  if (["xls", "xlsb", "ods"].includes(ext)) return "xls";
  if (["doc", "dot"].includes(ext)) return "doc";
  if (["ppt", "pps", "pot"].includes(ext)) return "ppt";
  if (ext === "rtf") return "rtf";
  if (["hml", "hwpml"].includes(ext)) return "hml";
  return null;
}

function unsupportedReason(name: string): string {
  const ext = name.toLowerCase().split(".").pop() ?? "";
  if (["heic", "heif"].includes(ext)) return "heic";
  if (["vsd", "pub", "msg", "mdb"].includes(ext)) return "office_legacy";
  if (["zip", "7z", "rar", "gz"].includes(ext)) return "archive";
  if (["exe", "dll", "msi"].includes(ext)) return "executable";
  if (!name.includes(".")) return "no_extension";
  return "unsupported_extension";
}

/** Deterministic page count from the file name (`…-12p.pdf` → 12 pages). */
function pageCountFor(name: string, kind: SourceKind): number {
  const m = name.match(/(\d+)p\./i);
  if (m) return Math.max(1, Math.min(500, Number(m[1])));
  if (kind === "image" || kind === "text") return 1;
  if (kind === "pdf") return 3;
  if (kind === "pptx") return 4;
  return 2;
}

function svgPage(title: string, pageNo: number, w: number, h: number, thumb: boolean): string {
  const fs = thumb ? 28 : 36;
  const lines = thumb ? 6 : 18;
  let body = "";
  for (let i = 0; i < lines; i += 1) {
    const y = Math.round(h * 0.2 + i * (h * 0.7) / lines);
    const len = 0.5 + ((i * 7) % 5) / 10;
    body += `<rect x="${Math.round(w * 0.1)}" y="${y}" width="${Math.round(w * 0.8 * len)}" height="${Math.max(4, Math.round(h / 90))}" fill="#9aa3ad"/>`;
  }
  const safe = title.replace(/[<>&"]/g, "");
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}"><rect width="100%" height="100%" fill="#ffffff"/><rect x="2" y="2" width="${w - 4}" height="${h - 4}" fill="none" stroke="#d0d5dc"/><text x="${Math.round(w * 0.1)}" y="${Math.round(h * 0.12)}" font-family="sans-serif" font-size="${fs}" fill="#333">${safe} — p.${pageNo}</text>${body}</svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

/** The synthetic word layout of a mock page (PDF / converted text → "layer", OCR / manual → "estimated"). */
/** Text, layout and source of a page as the edit history records them. */
function pageTextState(d: MockDoc, p: MockPage): MockTextState {
  // The layout is edited in place by the word edits: the history keeps its own copy.
  return { text: p.text, layout: p.layout ? structuredClone(p.layout) : null, source: p.text_source ?? (p.text ? (d.converted ? "converted" : "layer") : null) };
}

/** Puts a page back into a recorded state (undo / redo / revert). */
function applyPageTextState(d: MockDoc, p: MockPage, state: MockTextState): void {
  p.text = state.text;
  p.layout = state.layout ? structuredClone(state.layout) : null;
  p.text_source = state.source;
  d.has_text_layer = d.pages.some((x) => !!x.text);
  d.updated_at = new Date().toISOString();
}

function pageTextRevert(d: MockDoc, p: MockPage) {
  return { page_no: p.page_no, text: p.text, text_source: p.text_source ?? (p.text ? (d.converted ? "converted" : "layer") : null), layout: p.layout ?? (p.text && p.width_px > 0 ? mockLayout(d, p) : null), history: textHistoryView(d.text_history ?? [], p.page_no) };
}

function mockLayout(d: MockDoc, p: MockPage): PageLayout {
  const source = p.text_source ?? (d.converted ? "converted" : "layer");
  return synthLayout(p.text, source === "layer" || source === "converted" ? "layer" : "estimated");
}

export function docView(d: MockDoc): DocumentView {
  const { workspace, pages, text_history, ...record } = d;
  void text_history;
  const first = pages[0];
  const s = sep(workspace);
  return {
    ...record,
    absolute_path: `${workspace}${s}${d.rel_path.split("/").join(s)}`,
    thumb_url: first && first.width_px > 0 ? svgPage(d.original_name, 1, 160, Math.round((160 * first.height_px) / first.width_px), true) : null,
  };
}

function pageView(d: MockDoc, p: MockPage, withText: boolean): PageView {
  const hasImage = p.width_px > 0;
  return {
    document_id: d.id,
    page_no: p.page_no,
    width_px: p.width_px,
    height_px: p.height_px,
    width_pt: null,
    height_pt: null,
    dpi: d.render_dpi,
    rotation: p.rotation,
    has_text: !!p.text,
    text: withText ? p.text : undefined,
    is_blank: p.is_blank,
    rendered_at: p.rendered_at,
    text_source: p.text_source ?? (p.text ? (d.converted ? "converted" : "layer") : null),
    ocr_at: p.ocr_at ?? null,
    ocr_profile: null,
    ocr_model: null,
    classified_at: p.classified_at ?? null,
    page_doc_type: p.page_doc_type ?? null,
    page_language: null,
    page_rotation: null,
    image_url: hasImage ? svgPage(d.original_name, p.page_no, p.width_px, p.height_px, false) : null,
    thumb_url: hasImage ? svgPage(d.original_name, p.page_no, 160, Math.round((160 * p.height_px) / p.width_px), true) : null,
  };
}

function matchesFilter(d: MockDoc, f: DocumentFilter): boolean {
  if (f.statuses && f.statuses.length > 0 && !f.statuses.includes(d.status)) return false;
  if (f.kinds && f.kinds.length > 0 && !f.kinds.includes(d.source_kind)) return false;
  if (f.folder !== undefined && f.folder !== null) {
    const dir = relDir(d.rel_path);
    if (f.folder === "" ? dir !== "" : !(dir === f.folder || dir.startsWith(`${f.folder}/`))) return false;
  }
  if (f.query && f.query.trim()) {
    const q = f.query.trim().toLowerCase();
    if (!d.original_name.toLowerCase().includes(q) && !(d.title ?? "").toLowerCase().includes(q) && !d.rel_path.toLowerCase().includes(q)) return false;
  }
  if (f.tag && !d.tags.some((t) => t.toLowerCase() === f.tag!.toLowerCase())) return false;
  if (f.doc_type && d.doc_type !== f.doc_type) return false;
  if (f.review_status && d.review_status !== f.review_status) return false;
  if (f.role && d.role !== f.role) return false;
  if (f.max_confidence != null && !(d.confidence !== null && d.confidence < f.max_confidence)) return false;
  return true;
}

function sortDocs(list: MockDoc[], sort: DocumentSort | undefined): MockDoc[] {
  const s = [...list];
  switch (sort ?? "imported_desc") {
    case "imported_asc":
      return s.sort((a, b) => a.imported_at.localeCompare(b.imported_at));
    case "name_asc":
      return s.sort((a, b) => a.original_name.localeCompare(b.original_name));
    case "name_desc":
      return s.sort((a, b) => b.original_name.localeCompare(a.original_name));
    case "size_desc":
      return s.sort((a, b) => b.size_bytes - a.size_bytes);
    default:
      return s.sort((a, b) => b.imported_at.localeCompare(a.imported_at));
  }
}

function libraryStats(): LibraryStats {
  const docs = wsDocs();
  const by_status: Record<string, number> = {};
  const by_kind: Record<string, number> = {};
  const folders: Record<string, number> = {};
  let total_pages = 0;
  for (const d of docs) {
    by_status[d.status] = (by_status[d.status] ?? 0) + 1;
    by_kind[d.source_kind] = (by_kind[d.source_kind] ?? 0) + 1;
    folders[relDir(d.rel_path)] = (folders[relDir(d.rel_path)] ?? 0) + 1;
    total_pages += d.page_count;
  }
  return {
    total: docs.length,
    by_status,
    by_kind,
    folders: Object.entries(folders)
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([rel_dir, count]) => ({ rel_dir, count })),
    total_pages,
  };
}

const importPlans = new Map<string, DocumentImportPlan>();

function makeImportPlan(paths: string[], options: ImportOptions, source: ImportSource): DocumentImportPlan {
  const docs = wsDocs();
  const wsPath = current?.path ?? "";
  const seen = new Map<string, string>();
  const items: ImportItem[] = [];
  const seenPaths = new Set<string>();
  for (const raw of paths) {
    const path = raw.trim().replace(/^"|"$/g, "");
    if (!path || seenPaths.has(path)) continue; // Rust dedupes identical paths in collect_paths
    seenPaths.add(path);
    const name = basename(path);
    const kind = kindOf(name);
    const inside = wsPath !== "" && path.startsWith(wsPath);
    const rel = inside ? path.slice(wsPath.length).replace(/^[\\/]+/, "").split(/[\\/]/).join("/") : null;
    // Content hash stands in for the file name: the same name in two folders counts as the same content.
    const hash = `mock-${name.toLowerCase()}`;
    const item: ImportItem = { path, name, size: 120_000 + name.length * 9_000, kind, inside_workspace: inside, rel_path: rel, content_hash: null, status: { kind: "new" } };
    if (!kind) {
      item.status = { kind: "unsupported", reason: unsupportedReason(name) };
    } else if (rel && docs.some((d) => d.rel_path === rel && d.status !== "missing")) {
      item.status = { kind: "already_registered", id: docs.find((d) => d.rel_path === rel)!.id };
    } else if (seen.has(hash)) {
      item.status = { kind: "duplicate_in_batch", of: seen.get(hash)! };
    } else {
      item.content_hash = hash;
      const dup = docs.find((d) => d.content_hash === hash && d.status !== "missing");
      const missing = docs.find((d) => d.content_hash === hash && d.status === "missing");
      if (dup) item.status = { kind: "duplicate", of: dup.id, of_rel_path: dup.rel_path };
      else if (missing) item.status = { kind: "relink", id: missing.id, old_rel_path: missing.rel_path };
      seen.set(hash, name);
    }
    items.push(item);
  }
  const count = (k: string) => items.filter((i) => i.status.kind === k).length;
  const plan: DocumentImportPlan = {
    token: crypto.randomUUID().replace(/-/g, ""),
    source,
    options,
    items,
    new_count: count("new"),
    relink_count: count("relink"),
    duplicate_count: count("duplicate") + count("duplicate_in_batch"),
    registered_count: count("already_registered"),
    unsupported_count: count("unsupported"),
    copying_count: 0,
    new_bytes: items.filter((i) => i.status.kind === "new").reduce((a, i) => a + i.size, 0),
    truncated: false,
  };
  importPlans.set(plan.token, plan);
  return plan;
}

/**
 * `documents::import::process_document` stores this on the row before it converts / renders a document (crash
 * safety: a row that dies in a parser must not stay `importing`), and replaces it with the outcome afterwards.
 */
const PROCESSING_MARKER = "interrupted while processing";

/** Marks a document as being processed, like the first statement of `process_document`. */
function beginProcessing(d: MockDoc): void {
  d.status = "error";
  d.error = PROCESSING_MARKER;
  d.updated_at = new Date().toISOString();
  upsertDoc(d);
  emit(EVENTS.documentsChanged, { kind: "updated", ids: [d.id] } satisfies DocumentsChanged);
}

function renderDocPages(d: MockDoc, dpi: number): void {
  const n = pageCountFor(d.original_name, d.source_kind);
  const scale = dpi / 200;
  const now = new Date().toISOString();
  d.pages = Array.from({ length: n }, (_, i) => ({
    page_no: i + 1,
    width_px: Math.round(1654 * scale),
    height_px: Math.round(2339 * scale),
    rotation: 0,
    text: d.source_kind === "image" ? null : `${d.original_name} ${i + 1}쪽의 텍스트 레이어입니다.\n합계 12,500원`,
    is_blank: false,
    rendered_at: now,
  }));
  d.page_count = n;
  d.has_text_layer = d.source_kind !== "image";
  d.render_dpi = dpi;
  d.converted = ["hwpx", "docx", "hwp"].includes(d.source_kind);
  d.conversion_note = d.converted ? "변환본(근사 렌더): 글상자·수식은 생략될 수 있습니다." : null;
  d.status = "rendered";
  d.error = null;
  d.updated_at = now;
}

export function upsertDoc(doc: MockDoc): void {
  const all = loadDocs().filter((d) => d.id !== doc.id);
  all.push(doc);
  saveDocs(all);
}

async function runDocumentImport(plan: DocumentImportPlan): Promise<string> {
  const importable = plan.items.filter((i) => i.status.kind === "new" || i.status.kind === "relink");
  // Registered documents the batch merely met again — the same path, or the same content under another path —
  // each once (`ImportPlan::existing_ids`): they still join the batch's collection / project.
  const reencountered = [...new Set(plan.items.flatMap((i) => (i.status.kind === "already_registered" ? [i.status.id] : i.status.kind === "duplicate" ? [i.status.of] : [])))];
  const hasTarget = !!(plan.options.collection_id || plan.options.project_id);
  // `documents_import_run`: a plan with nothing new is refused — unless it has a collection / project to put
  // its registered documents into (`attaches_existing`); then the job only attaches them and names them in
  // `ImportReport.existing`.
  if (importable.length === 0 && !(hasTarget && reencountered.length > 0)) throw err("invalid", "nothing to import");
  const subject = plan.source === "inbox" ? "inbox" : importable.length === 0 ? `${reencountered.length} documents` : `${importable.length} files`;
  const job = createJob("document_import", subject);
  const settings = loadSettings();
  const dpi = plan.options.dpi ?? settings.processing.render_dpi;
  void (async () => {
    let done = 0;
    const ids: string[] = [];
    for (const item of importable) {
      if (cancelled.has(job.id)) break;
      updateJob(job.id, { progress: done / importable.length, detail: item.name });
      const now = new Date().toISOString();
      const destDir = plan.options.destination === "inbox" ? "inbox" : "documents";
      const rel = item.inside_workspace && item.rel_path ? item.rel_path : `${destDir}/${item.name}`;
      let doc: MockDoc;
      if (item.status.kind === "relink") {
        const existing = loadDocs().find((d) => d.id === (item.status as { id: string }).id)!;
        doc = { ...existing, rel_path: rel, original_name: item.name, status: existing.pages.length > 0 ? "rendered" : "importing", updated_at: now };
      } else {
        doc = {
          workspace: current!.path,
          pages: [],
          id: crypto.randomUUID().replace(/-/g, ""),
          rel_path: rel,
          original_name: item.name,
          source_kind: item.kind!,
          content_hash: item.content_hash ?? `mock-${item.name.toLowerCase()}`,
          size_bytes: item.size,
          file_modified_at: now,
          imported_at: now,
          updated_at: now,
          status: "importing",
          role: plan.options.role,
          title: null,
          doc_type: null,
          vendor: null,
          doc_date: null,
          total_amount: null,
          language: null,
          page_count: 0,
          has_text_layer: false,
          converted: false,
          conversion_note: null,
          render_dpi: null,
          error: null,
          import_batch_id: job.id,
          deleted_at: null,
          tags: [],
          ...DOC_PIPELINE_DEFAULTS,
        };
      }
      upsertDoc(doc);
      emit(EVENTS.documentsChanged, { kind: "imported", ids: [doc.id] } satisfies DocumentsChanged);
      if (doc.status === "importing") {
        const pages = pageCountFor(doc.original_name, doc.source_kind);
        const steps = Math.min(6, pages);
        // While the document is converted / rendered its row carries the crash marker (status `error`).
        beginProcessing(doc);
        const ok = await simulate(job.id, steps, 150, (i) => {
          updateJob(job.id, { progress: (done + i / steps) / importable.length, detail: `${item.name} (${Math.round((pages * i) / steps)}/${pages})` });
        });
        if (!ok) {
          doc.status = "new";
          doc.error = null;
          upsertDoc(doc);
          emit(EVENTS.documentsChanged, { kind: "updated", ids: [doc.id] } satisfies DocumentsChanged);
          break;
        }
        if (plan.options.render) renderDocPages(doc, dpi);
        else {
          doc.status = "new";
          doc.error = null;
        }
        upsertDoc(doc);
        emit(EVENTS.documentsChanged, { kind: "updated", ids: [doc.id] } satisfies DocumentsChanged);
      }
      ids.push(doc.id);
      done += 1;
    }
    if (ids.length > 0) emit(EVENTS.documentsChanged, { kind: "batch", ids } satisfies DocumentsChanged);
    const wasCancelled = cancelled.has(job.id);
    // The plan may be minutes old: a document deleted since is simply not there to attach.
    const existing = reencountered.filter((id) => wsDocs().some((d) => d.id === id));
    const touched = [...ids, ...existing];
    const kb = await import("./mock-knowledge");
    if (plan.options.collection_id) kb.attachToCollection(plan.options.collection_id, touched, plan.options.role);
    if (plan.options.project_id) {
      const pj = await import("./mock-projects");
      try {
        pj.addDocuments(plan.options.project_id, touched);
      } catch {
        /* project deleted meanwhile */
      }
    }
    // Their role may have followed the collection: the lists refresh for them as well.
    if (hasTarget && existing.length > 0) emit(EVENTS.documentsChanged, { kind: "updated", ids: existing } satisfies DocumentsChanged);
    // The report mirrors Rust's `ImportReport` (the job's structured result; drop targets read the ids).
    const report: ImportReport = { batch_id: job.id, imported: ids, relinked: [], existing, failed: [], skipped: plan.duplicate_count + plan.unsupported_count, cancelled: wasCancelled };
    if (wasCancelled) finishJob(job.id, { ok: false, error: "cancelled", result: report });
    else finishJob(job.id, { ok: true, detail: `imported ${ids.length} / skipped ${plan.duplicate_count + plan.unsupported_count}`, result: report });
    kb.autoIndex();
    const pl = await import("./mock-pipeline");
    pl.autoProcess(ids, plan.options.auto_level ?? null, plan.source);
  })();
  return job.id;
}

function mockDocuments(cmd: string, a: Record<string, any>): unknown { // eslint-disable-line @typescript-eslint/no-explicit-any
  if (!current) throw err("no_workspace", "no workspace is open");
  switch (cmd) {
    case "documents_list": {
      const f = (a.filter ?? {}) as DocumentFilter;
      let list = sortDocs(wsDocs().filter((d) => matchesFilter(d, f)), f.sort);
      if (f.limit) list = list.slice(f.offset ?? 0, (f.offset ?? 0) + f.limit);
      return list.map(docView);
    }
    case "documents_stats":
      return libraryStats();
    case "documents_get": {
      const d = wsDocs().find((x) => x.id === a.id);
      if (!d) throw err("not_found", String(a.id));
      return docView(d);
    }
    case "documents_pages": {
      const d = wsDocs().find((x) => x.id === a.id);
      if (!d) throw err("not_found", String(a.id));
      return d.pages.map((p) => pageView(d, p, Boolean(a.withText)));
    }
    case "documents_page_text": {
      const d = wsDocs().find((x) => x.id === a.id);
      return d?.pages.find((p) => p.page_no === Number(a.pageNo))?.text ?? null;
    }
    case "documents_set_page_text": {
      const d = wsDocs().find((x) => x.id === a.id);
      const p = d?.pages.find((x) => x.page_no === Number(a.pageNo));
      if (!d || !p) return false;
      const before = pageTextState(d, p);
      const text = String(a.text ?? "").trimEnd();
      p.text = text.trim() ? text : null;
      p.text_source = "manual";
      p.layout = null;
      d.has_text_layer = d.pages.some((x) => !!x.text);
      d.updated_at = new Date().toISOString();
      d.text_history = recordTextEdit(d.text_history ?? [], p.page_no, "manual", before, pageTextState(d, p));
      upsertDoc(d);
      emit(EVENTS.documentsChanged, { kind: "updated", ids: [d.id] } satisfies DocumentsChanged);
      void import("./mock-knowledge").then((kb) => kb.autoIndex());
      return true;
    }
    case "documents_page_layout": {
      const d = wsDocs().find((x) => x.id === a.id);
      const p = d?.pages.find((x) => x.page_no === Number(a.pageNo));
      if (!d || !p || !p.text || p.width_px === 0) return null;
      return p.layout ?? mockLayout(d, p);
    }
    case "documents_edit_page_words": {
      const d = wsDocs().find((x) => x.id === a.id);
      const p = d?.pages.find((x) => x.page_no === Number(a.pageNo));
      if (!d || !p) throw err("not_found", String(a.id));
      if (!p.text) throw err("not_found", "text layer");
      const layout = p.layout ?? mockLayout(d, p);
      const start = Number(a.start);
      const end = Number(a.end);
      if (!(start >= 0 && start < end && end <= layout.words.length)) throw err("invalid", `word range ${start}..${end}`);
      const before = pageTextState(d, p);
      const text = applyLayoutEdit(layout, p.text, start, end, String(a.text ?? ""));
      p.text = text.trim() ? text.trimEnd() : null;
      p.text_source = "manual";
      p.layout = layout;
      d.has_text_layer = d.pages.some((x) => !!x.text);
      d.updated_at = new Date().toISOString();
      d.text_history = recordTextEdit(d.text_history ?? [], p.page_no, "layer", before, pageTextState(d, p));
      upsertDoc(d);
      emit(EVENTS.documentsChanged, { kind: "updated", ids: [d.id] } satisfies DocumentsChanged);
      void import("./mock-knowledge").then((kb) => kb.autoIndex());
      return layout;
    }
    case "documents_add_page_words": {
      const d = wsDocs().find((x) => x.id === a.id);
      const p = d?.pages.find((x) => x.page_no === Number(a.pageNo));
      if (!d || !p) throw err("not_found", String(a.id));
      const words = String(a.text ?? "");
      if (!words.trim()) throw err("invalid", "empty text");
      const layout = p.layout ?? (p.text ? mockLayout(d, p) : { version: 1, source: "estimated" as const, words: [] });
      const before = pageTextState(d, p);
      const text = addLayoutWords(layout, p.text ?? "", a.bbox as [number, number, number, number], words);
      p.text = text.trimEnd();
      p.text_source = "manual";
      p.layout = layout;
      d.has_text_layer = true;
      d.updated_at = new Date().toISOString();
      d.text_history = recordTextEdit(d.text_history ?? [], p.page_no, "add", before, pageTextState(d, p));
      upsertDoc(d);
      emit(EVENTS.documentsChanged, { kind: "updated", ids: [d.id] } satisfies DocumentsChanged);
      void import("./mock-knowledge").then((kb) => kb.autoIndex());
      return layout;
    }
    case "documents_page_text_history": {
      const d = wsDocs().find((x) => x.id === a.id);
      if (!d) throw err("not_found", String(a.id));
      return textHistoryView(d.text_history ?? [], a.pageNo === null || a.pageNo === undefined ? null : Number(a.pageNo));
    }
    case "documents_undo_page_text":
    case "documents_redo_page_text": {
      const d = wsDocs().find((x) => x.id === a.id);
      const p = d?.pages.find((x) => x.page_no === Number(a.pageNo));
      if (!d || !p) throw err("not_found", String(a.id));
      const rows = d.text_history ?? [];
      const redo = cmd === "documents_redo_page_text";
      const target = textMoveTarget(rows, p.page_no, redo);
      if (!target) return null;
      applyPageTextState(d, p, recordTextMove(rows, target, redo, pageTextState(d, p)));
      d.text_history = rows;
      upsertDoc(d);
      emit(EVENTS.documentsChanged, { kind: "updated", ids: [d.id] } satisfies DocumentsChanged);
      void import("./mock-knowledge").then((kb) => kb.autoIndex());
      return pageTextRevert(d, p);
    }
    case "documents_revert_page_text": {
      const d = wsDocs().find((x) => x.id === a.id);
      if (!d) throw err("not_found", String(a.id));
      const rows = d.text_history ?? [];
      const found = textRevertTargets(rows, Number(a.historyId));
      if ("error" in found) throw err(found.error === "not_found" ? "not_found" : "invalid", `text edit ${a.historyId}`);
      const p = d.pages.find((x) => x.page_no === found.pageNo);
      if (!p) throw err("not_found", `page ${found.pageNo}`);
      for (const target of found.targets) applyPageTextState(d, p, recordTextMove(rows, target, false, pageTextState(d, p)));
      d.text_history = rows;
      upsertDoc(d);
      emit(EVENTS.documentsChanged, { kind: "updated", ids: [d.id] } satisfies DocumentsChanged);
      void import("./mock-knowledge").then((kb) => kb.autoIndex());
      return pageTextRevert(d, p);
    }
    case "documents_import_plan": {
      const args = a.args as { paths: string[]; options: ImportOptions; source: ImportSource };
      if (!args.paths?.length) throw err("invalid", "no files");
      return makeImportPlan(args.paths, args.options, args.source ?? "drop");
    }
    case "documents_import_run": {
      const args = a.args as { token: string | null; paths: string[]; options: ImportOptions | null; source: ImportSource };
      let plan = args.token ? importPlans.get(args.token) : undefined;
      if (plan && args.options) plan = { ...plan, options: args.options };
      if (!plan) plan = makeImportPlan(args.paths ?? [], args.options ?? defaultImportOptions(), args.source ?? "drop");
      if (args.token) importPlans.delete(args.token);
      return runDocumentImport(plan);
    }
    case "documents_rescan": {
      if (jobs.some((j) => ["document_import", "document_rescan", "document_render"].includes(j.kind) && !isFinishedState(j.state))) return null;
      const job = createJob("document_rescan", "library");
      void simulate(job.id, 2, 120, () => undefined).then(() => finishJob(job.id, { ok: true, detail: `checked ${wsDocs().length}` }));
      return job.id;
    }
    case "documents_inbox_scan":
      return undefined;
    case "documents_rerender": {
      const ids = (a.ids ?? []) as string[];
      if (ids.length === 0) throw err("invalid", "no documents");
      const job = createJob("document_render", `${ids.length} documents`);
      const dpi = (a.dpi as number | null) ?? loadSettings().processing.render_dpi;
      void (async () => {
        for (const [i, id] of ids.entries()) {
          const d = wsDocs().find((x) => x.id === id);
          if (!d) continue;
          updateJob(job.id, { progress: i / ids.length, detail: d.original_name });
          beginProcessing(d);
          const ok = await simulate(job.id, 2, 150, () => undefined);
          if (!ok) {
            // `process_document`: a cancelled run leaves the document `new`.
            d.status = "new";
            d.error = null;
            upsertDoc(d);
            emit(EVENTS.documentsChanged, { kind: "updated", ids: [id] } satisfies DocumentsChanged);
            break;
          }
          renderDocPages(d, dpi);
          upsertDoc(d);
          emit(EVENTS.documentsChanged, { kind: "updated", ids: [id] } satisfies DocumentsChanged);
        }
        if (cancelled.has(job.id)) finishJob(job.id, { ok: false, error: "cancelled" });
        else finishJob(job.id, { ok: true, detail: `rendered ${ids.length}` });
      })();
      return job.id;
    }
    case "documents_rotate_page": {
      const d = wsDocs().find((x) => x.id === a.id);
      const p = d?.pages.find((x) => x.page_no === Number(a.pageNo));
      if (!d || !p) throw err("not_found", `page ${a.pageNo}`);
      const rotation = Number(a.rotation) % 360;
      if (rotation % 90 !== 0) throw err("invalid", "rotation must be a multiple of 90");
      const swapped = (rotation / 90) % 2 !== (p.rotation / 90) % 2;
      if (swapped) [p.width_px, p.height_px] = [p.height_px, p.width_px];
      p.rotation = rotation;
      p.rendered_at = new Date().toISOString();
      d.updated_at = p.rendered_at;
      upsertDoc(d);
      emit(EVENTS.documentsChanged, { kind: "updated", ids: [d.id] } satisfies DocumentsChanged);
      return pageView(d, p, true);
    }
    case "documents_delete": {
      const ids = (a.ids ?? []) as string[];
      const all = loadDocs();
      const removed = all.filter((d) => ids.includes(d.id)).map((d) => d.id);
      saveDocs(all.filter((d) => !ids.includes(d.id)));
      if (removed.length > 0) emit(EVENTS.documentsChanged, { kind: "removed", ids: removed } satisfies DocumentsChanged);
      return removed;
    }
    case "documents_set_tags":
    case "documents_set_doc_type":
    case "documents_set_title": {
      const d = wsDocs().find((x) => x.id === a.id);
      if (!d) throw err("not_found", String(a.id));
      if (cmd === "documents_set_tags") {
        const tags = ((a.tags ?? []) as string[]).map((t) => t.trim()).filter(Boolean);
        d.tags = Array.from(new Set(tags)).sort((x, y) => x.localeCompare(y));
      } else if (cmd === "documents_set_doc_type") {
        d.doc_type = (a.docType as string | null)?.trim() || null;
        d.doc_type_source = "manual";
      } else {
        d.title = (a.title as string | null)?.trim() || null;
      }
      d.updated_at = new Date().toISOString();
      upsertDoc(d);
      emit(EVENTS.documentsChanged, { kind: "updated", ids: [d.id] } satisfies DocumentsChanged);
      return docView(d);
    }
    case "documents_tags": {
      const counts = new Map<string, number>();
      for (const d of wsDocs()) for (const t of d.tags) counts.set(t, (counts.get(t) ?? 0) + 1);
      return Array.from(counts.entries()).sort(([x], [y]) => x.localeCompare(y));
    }
    case "documents_set_inbox_watch": {
      const s = loadSettings();
      s.processing.inbox_watch = Boolean(a.enabled);
      saveSettings(s);
      return s.processing.inbox_watch;
    }
    case "documents_pdfium_info": {
      // `localStorage.doculens.mock.nopdfium` simulates a missing renderer (UI banner).
      const missing = read<string | null>("doculens.mock.nopdfium", null) !== null;
      return (missing
        ? { library: null, error: "pdfium_not_found", searched: ["(browser)/pdfium"] }
        : { library: "(browser) simulated pdfium", error: null, searched: [] }) satisfies PdfiumInfo;
    }
    default:
      throw err("internal", `mock backend: unknown command ${cmd}`);
  }
}

export async function mockInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  resolveStartup();
  const a = (args ?? {}) as Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
  const r = (v: unknown) => v as T;
  switch (cmd) {
    case "app_info":
      return r({
        name: "DocuLens",
        version: "1.0.0",
        settings_path: "(browser) localStorage:doculens.mock.settings",
        app_data_dir: "(browser)",
        engines_dir: "(browser)/engines",
        presets_path: "(browser)/presets.ini",
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
      if (id !== "auto" && !mergedModels().some((m) => m.id === id)) throw err("unknown_model", `unknown model id: ${id}`);
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
      const { path } = a.args as { path: string; force_unlock: boolean };
      return r(openWorkspace(path, false, null, null));
    }
    case "workspace_create": {
      const { path, name, locale } = a.args as { path: string; name: string | null; locale: Language | null };
      return r(openWorkspace(path, true, name, locale));
    }
    case "workspace_close": {
      detachJobs();
      current = null;
      const s = loadSettings();
      s.active_workspace = null;
      saveSettings(s);
      afterWorkspaceChanged();
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
      const models_dir = modelsDir();
      const out: RegistryView = {
        models: mergedModels().map((m) => ({
          ...m,
          install: installState(m),
          total_size: m.files.reduce((acc, f) => acc + f.size, 0),
        })),
        tier_defaults: registry.tier_defaults,
        default_embedding: registry.default_embedding,
        models_dir,
      };
      return r(out);
    }
    case "registry_required_files_text": {
      const m = mergedModels().find((x) => x.id === a.modelId);
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
          // `ModelFile::download_url`: the pinned address of the registry entry, else the repository's main branch.
          `  url: ${f.url ?? (f.hf ? `https://huggingface.co/${f.hf}/resolve/main/${f.name}` : "(no public URL)")}`,
        ]),
        "",
        "Also copy LICENSE / NOTICE files from the source repository into the same folder.",
      ];
      return r(lines.join("\n"));
    }
    case "models_scan":
    case "models_list":
      return r(modelIndex());
    case "models_import_plan": {
      const { paths, mode, folder_name } = a.args as { paths: string[]; mode: ImportMode; folder_name: string | null };
      return r(importPlan(paths, mode ?? "copy", folder_name ?? null));
    }
    case "models_import_run": {
      const { paths, mode, folder_name } = a.args as { paths: string[]; mode: ImportMode; folder_name: string | null };
      const plan = importPlan(paths, mode ?? "copy", folder_name ?? null);
      if (plan.already_present) throw err("invalid", "the same files are already in the target folder");
      return r(importRun(plan));
    }
    case "models_download":
      return r(downloadJob(String(a.modelId)));
    case "models_hash":
      return r(hashJob(String(a.folder)));
    case "models_delete":
      deleteModel(String(a.folder));
      return r(undefined);
    case "engine_status":
      return r(engine);
    case "engine_start":
      return r(await engineStart());
    case "engine_stop":
      await engineStop();
      return r(undefined);
    case "engine_restart": {
      const restarts = engine.restarts;
      await engineStop();
      const st = await engineStart();
      setEngine({ restarts });
      return r(st);
    }
    case "engine_load_model":
      return r(await loadModelJob(String(a.modelId), (a.warmup as boolean | null) ?? loadSettings().models.warmup_after_load));
    case "engine_unload_model": {
      if (engine.state.kind !== "running") throw err("engine_not_running", "engine is not running");
      const id = String(a.modelId);
      setModel(id, { status: "unloaded", memory_effective_bytes: null, loaded_at: null, warmed_up: false, progress: null, stage: null });
      if (engine.active_model === id) setEngine({ active_model: null });
      log(`unloaded model ${id} in 310 ms`);
      return r(undefined);
    }
    case "engine_refresh_models": {
      if (engine.state.kind !== "running") throw err("engine_not_running", "engine is not running");
      const ids = loadableIds();
      setEngine({ models: [...engine.models.filter((m) => ids.includes(m.id)), ...ids.filter((id) => !engine.models.some((m) => m.id === id)).map(setModelSilently)] });
      return r(engine.models);
    }
    case "engine_chat_test": {
      if (engine.state.kind !== "running") throw err("engine_not_running", "engine is not running");
      const model = (a.modelId as string | null) ?? engine.active_model ?? assess().active_model?.id ?? null;
      if (!model) throw err("invalid", "no active model");
      const inst = engine.models.find((m) => m.id === model);
      if (!inst) throw err("unknown_model", `unknown model id: ${model}`);
      if (inst.status !== "loaded") await loadModelJob(model, true);
      await new Promise((res) => setTimeout(res, tick(600)));
      const prompt = (a.prompt as string | null) ?? "안녕하세요. 한 문장으로 자기소개를 해 주세요.";
      const out: ChatTestResult = {
        model,
        prompt,
        content: "안녕하세요, 저는 DocuLens에 내장된 문서 처리 모델(브라우저 미리보기 응답)입니다.",
        finish_reason: "stop",
        prompt_tokens: 31,
        completion_tokens: 24,
        elapsed_ms: 612,
        tokens_per_second: 48.7,
      };
      return r(out);
    }
    case "engine_list_devices":
      return r(MOCK_DEVICES);
    case "engine_binary_info": {
      const info: EngineBinaryInfo = {
        resolved: MOCK_BINARY,
        error: null,
        installed_packs: [
          { backend: "cpu", version: "b12066", build_number: 12066, path: MOCK_BINARY.path.replace("cu121", "cpu") },
          { backend: "cu121", version: "b12066", build_number: 12066, path: MOCK_BINARY.path },
        ],
        engines_dir: "C:\\Users\\me\\AppData\\Local\\DocuLens\\engines",
        bundled_roots: [],
        min_build: 11747,
      };
      return r(info);
    }
    case "engine_logs":
      return r(engineLogs.slice(-Number(a.lines ?? 200)));
    case "presets_preview":
      return r(presetsText());
    case "jobs_list":
      return r(jobs.map(withResult));
    case "jobs_cancel":
      return r(cancelJob(String(a.jobId)));
    case "jobs_clear_finished":
      clearFinishedJobs();
      return r(undefined);
    case "jobs_get":
    case "jobs_pause":
    case "jobs_resume":
    case "jobs_retry":
    case "jobs_prioritize": {
      const pl = await import("./mock-planner");
      return r(await pl.mockPlanner(cmd, a));
    }
    case "documents_list":
    case "documents_get": {
      // 1.0.0: the views carry the projects of each document (work members first, then reference groups).
      const out = await mockDocuments(cmd, a);
      const pj = await import("./mock-projects");
      const kb = await import("./mock-knowledge");
      const summaries = pj.projectSummaries();
      const attach = (v: DocumentView): DocumentView => {
        const work = summaries.filter((p) => pj.isMember(p.id, v.id)).map((p) => ({ id: p.id, name: p.name, reference: false }));
        const reference = summaries
          .filter((p) => !work.some((w) => w.id === p.id) && kb.projectReferenceDocIds(p.id).includes(v.id))
          .map((p) => ({ id: p.id, name: p.name, reference: true }));
        return { ...v, projects: [...work, ...reference] };
      };
      return r(Array.isArray(out) ? (out as DocumentView[]).map(attach) : attach(out as DocumentView));
    }
    case "documents_stats":
    case "documents_pages":
    case "documents_page_text":
    case "documents_set_page_text":
    case "documents_page_layout":
    case "documents_edit_page_words":
    case "documents_add_page_words":
    case "documents_page_text_history":
    case "documents_undo_page_text":
    case "documents_redo_page_text":
    case "documents_revert_page_text":
    case "documents_import_plan":
    case "documents_import_run":
    case "documents_rescan":
    case "documents_inbox_scan":
    case "documents_rerender":
    case "documents_rotate_page":
    case "documents_delete":
    case "documents_set_tags":
    case "documents_set_doc_type":
    case "documents_set_title":
    case "documents_tags":
    case "documents_set_inbox_watch":
    case "documents_pdfium_info":
      return r(await mockDocuments(cmd, a));
    case "knowledge_status":
    case "knowledge_collections":
    case "knowledge_collection_save":
    case "knowledge_collection_delete":
    case "knowledge_collection_docs":
    case "knowledge_collection_add":
    case "knowledge_collection_remove":
    case "knowledge_document_collections":
    case "knowledge_set_role":
    case "knowledge_index":
    case "knowledge_embed":
    case "knowledge_reset_embeddings":
    case "knowledge_search":
    case "knowledge_chunks":
    case "knowledge_answer":
    case "knowledge_answer_cancel":
    case "knowledge_answers":
    case "knowledge_answer_feedback":
    case "knowledge_answer_delete":
    case "knowledge_glossary":
    case "knowledge_glossary_save":
    case "knowledge_glossary_delete":
    case "knowledge_versions":
    case "knowledge_version_set":
    case "knowledge_version_clear": {
      if (!current) throw err("no_workspace", "no workspace is open");
      const kb = await import("./mock-knowledge");
      return r(await kb.mockKnowledge(cmd, a));
    }
    case "chat_sessions_list":
    case "chat_session_create":
    case "chat_session_rename":
    case "chat_session_delete":
    case "chat_messages_list":
    case "chat_message_put": {
      if (!current) throw err("no_workspace", "no workspace is open");
      const chat = await import("./mock-chat");
      return r(await chat.mockChat(cmd, a));
    }
    case "pipeline_status":
    case "pipeline_process":
    case "pipeline_doc_types":
    case "pipeline_schemas":
    case "pipeline_schema_get":
    case "pipeline_schema_versions":
    case "pipeline_schema_save":
    case "pipeline_schema_delete":
    case "pipeline_schema_set_default":
    case "pipeline_schema_set_enabled":
    case "pipeline_schema_lint":
    case "pipeline_schema_from_nl":
    case "pipeline_form_derive":
    case "pipeline_form_register":
    case "pipeline_schema_discover":
    case "pipeline_schema_name_exists":
    case "pipeline_document_schema":
    case "pipeline_runs":
    case "pipeline_fields":
    case "pipeline_field_history":
    case "pipeline_restore":
    case "pipeline_revalidate":
    case "pipeline_review_queue":
    case "pipeline_review_apply":
    case "pipeline_review_approve_document":
    case "pipeline_tables":
    case "pipeline_table_csv":
    case "pipeline_table_export":
    case "pipeline_split":
    case "pipeline_merge": {
      if (!current) throw err("no_workspace", "no workspace is open");
      const pl = await import("./mock-pipeline");
      return r(await pl.mockPipeline(cmd, a));
    }
    case "planner_plan":
    case "planner_execute":
    case "organize_plan":
    case "organize_apply":
    case "journal_batches":
    case "journal_entries":
    case "journal_undo":
    case "journal_redo":
    case "aggregate_run":
    case "export_preview":
    case "export_run": {
      if (!current) throw err("no_workspace", "no workspace is open");
      const pl = await import("./mock-planner");
      return r(await pl.mockPlanner(cmd, a));
    }
    case "generate_templates":
    case "generate_budget_info":
    case "generate_draft":
    case "generate_preview":
    case "generate_validate":
    case "generate_save":
    case "generate_drafts":
    case "generate_draft_get":
    case "generate_draft_put":
    case "generate_draft_delete": {
      if (!current) throw err("no_workspace", "no workspace is open");
      const mg = await import("./mock-generate");
      return r(await mg.mockGenerate(cmd, a));
    }
    case "projects_list":
    case "project_create":
    case "project_update":
    case "project_delete":
    case "project_get":
    case "project_add_documents":
    case "project_remove_document":
    case "project_sync_copies":
    case "project_templates":
    case "project_template_save":
    case "project_template_delete":
    case "project_define_rule":
    case "project_save_rule":
    case "project_delete_rule":
    case "project_check":
    case "project_runs":
    case "project_report":
    case "project_request_supplement":
    case "project_classify_preview":
    case "project_classify_apply":
    case "project_assign":
    case "project_reference_group":
    case "maintenance_status":
    case "maintenance_run_now": {
      if (!current) throw err("no_workspace", "no workspace is open");
      const mp = await import("./mock-projects");
      return r(await mp.mockProjects(cmd, a));
    }
    case "prerouter_route":
      return r(mockRoute(String(a.text)));
    case "prerouter_resolve_date":
      return r(resolveRelativeDate(String(a.text)));
    default:
      throw err("internal", `mock backend: unknown command ${cmd}`);
  }
}

/** Test helper: wipe all mock state. */
export function resetMock(): void {
  localStorage.removeItem(SETTINGS_KEY);
  localStorage.removeItem(WORKSPACES_KEY);
  localStorage.removeItem(MODELS_KEY);
  localStorage.removeItem(DOCS_KEY);
  localStorage.removeItem("doculens.mock.knowledge");
  localStorage.removeItem("doculens.mock.pipeline");
  localStorage.removeItem("doculens.mock.schema-versions");
  localStorage.removeItem("doculens.mock.vision");
  localStorage.removeItem(JOBS_KEY);
  localStorage.removeItem("doculens.mock.journal");
  localStorage.removeItem("doculens.mock.exports");
  localStorage.removeItem("doculens.mock.planner_model");
  localStorage.removeItem("doculens.mock.os_trash");
  localStorage.removeItem("doculens.mock.drafts");
  localStorage.removeItem("doculens.mock.projects");
  localStorage.removeItem("doculens.mock.maintenance");
  importPlans.clear();
  current = null;
  startupResolved = false;
  jobs = [];
  cancelled.clear();
  paused.clear();
  specs.clear();
  controls.clear();
  gateWaiting = 0;
  engineLogs = [];
  engine = { ...engine, state: { kind: "stopped" }, pid: null, port: null, models: [], active_model: null, restarts: 0 };
}
