// TypeScript mirror of the Rust types exchanged over Tauri IPC
// (crates/doculens-core). Keep field names in snake_case to match serde.

export type Theme = "light" | "dark" | "system" | "high-contrast";
export type ResolvedTheme = Exclude<Theme, "system">;
export const THEMES: Theme[] = ["light", "dark", "system", "high-contrast"];

export type Language = "ko" | "en";
export const LANGUAGES: Language[] = ["ko", "en"];

export type Tier = "cpu_only" | "vram_6gb" | "vram_8gb" | "vram_12gb" | "vram_16gb" | "vram_24gb_plus";
export const TIERS: Tier[] = ["cpu_only", "vram_6gb", "vram_8gb", "vram_12gb", "vram_16gb", "vram_24gb_plus"];

export type TierMode = "auto" | "manual";
export type Profile = "accuracy" | "balanced" | "speed" | "fine_print";
export type BackendPreference = "auto" | "cuda" | "vulkan" | "cpu";
export type AutoProcessingLevel = "classify_only" | "classify_and_ocr" | "full";
export type ReducedMotion = "system" | "on" | "off";

export interface WorkspaceEntry {
  path: string;
  name: string;
  last_opened: string | null;
}

export interface UiSettings {
  theme: Theme;
  language: Language;
  scale_percent: number;
  reduced_motion: ReducedMotion;
  sidebar_collapsed: boolean;
}

export interface ModelSettings {
  tier_mode: TierMode;
  manual_tier: Tier | null;
  active_model: string;
  profile: Profile;
  backend: BackendPreference;
  resident_models_max: number;
  sleep_idle_seconds: number;
  allow_online_download: boolean;
}

export interface ProcessingSettings {
  auto_level: AutoProcessingLevel;
  render_dpi: number;
  review_threshold: number;
  audit_sample_percent: number;
  organize_reviewed_only: boolean;
  inbox_watch: boolean;
}

export interface CacheSettings {
  page_cache_max_gb: number;
  keep_thumbnails: boolean;
}

export interface PrivacySettings {
  pii_masking: boolean;
  db_encryption: boolean;
}

export interface NotificationSettings {
  job_complete: boolean;
  prevent_sleep_during_jobs: boolean;
}

export interface WindowState {
  width: number;
  height: number;
  maximized: boolean;
  x: number | null;
  y: number | null;
}

export interface AppSettings {
  schema_version: number;
  active_workspace: string | null;
  workspaces: WorkspaceEntry[];
  ui: UiSettings;
  models: ModelSettings;
  processing: ProcessingSettings;
  cache: CacheSettings;
  privacy: PrivacySettings;
  notifications: NotificationSettings;
  shortcuts: Record<string, string>;
  window: WindowState;
}

export interface RetentionPolicy {
  documents_days: number | null;
  extractions_days: number | null;
  page_cache_max_gb: number;
  job_logs_days: number | null;
  app_logs_days: number | null;
  journal_days: number | null;
  chats_days: number | null;
  temp_days: number | null;
  golden_exports_days: number | null;
  purge_hour_local: number;
  purge_enabled: boolean;
}

export type RetentionDayItem =
  | "documents_days"
  | "extractions_days"
  | "job_logs_days"
  | "app_logs_days"
  | "journal_days"
  | "chats_days"
  | "temp_days"
  | "golden_exports_days";

export const RETENTION_DAY_ITEMS: RetentionDayItem[] = [
  "documents_days",
  "extractions_days",
  "job_logs_days",
  "app_logs_days",
  "journal_days",
  "chats_days",
  "temp_days",
  "golden_exports_days",
];

export type ModelsLocation = "workspace" | "global";

export interface WorkspaceManifest {
  workspace_id: string;
  schema_version: number;
  app_version: string;
  last_opened_app_version: string;
  name: string;
  created_at: string;
  last_opened_at: string | null;
  locale: Language;
  preferred_model: string | null;
  models_location: ModelsLocation;
  retention: RetentionPolicy;
}

export interface WorkspaceView {
  path: string;
  manifest: WorkspaceManifest;
  models_dir: string;
  inbox_dir: string;
  documents_dir: string;
  exports_dir: string;
}

export type LockState = "free" | "stale" | "held_here" | "held_elsewhere";

export interface LockInfo {
  pid: number;
  host: string;
  app_version: string;
  created_at: string;
}

export interface WorkspaceInspection {
  path: string;
  exists: boolean;
  is_dir: boolean;
  initialized: boolean;
  manifest: WorkspaceManifest | null;
  lock_state: LockState;
  lock: LockInfo | null;
  document_files: number;
  unsafe_location: boolean;
}

export type GpuVendor = "nvidia" | "amd" | "intel" | "apple" | "other";
export type GpuSource = "nvml" | "engine" | "manual";

export interface GpuInfo {
  index: number;
  name: string;
  vendor: GpuVendor;
  total_mib: number;
  free_mib: number | null;
  driver_version: string | null;
  cuda_driver_version: [number, number] | null;
  compute_capability: [number, number] | null;
  source: GpuSource;
}

export interface HardwareSummary {
  os: string;
  os_version: string;
  hostname: string;
  cpu_name: string;
  cpu_cores_physical: number | null;
  cpu_cores_logical: number;
  ram_total_mib: number;
  ram_available_mib: number;
  gpus: GpuInfo[];
  nvml_available: boolean;
  probed_at: string;
}

export type TierBasis = "nvml_free_vram" | "engine_free_vram" | "no_gpu" | "manual_override";

export interface TierAssessment {
  tier: Tier;
  basis: TierBasis;
  free_vram_mib: number | null;
  total_vram_mib: number | null;
  gpu_name: string | null;
  warnings: string[];
}

export type ModelRole = "vlm" | "llm" | "embedding" | "reranker";

export interface ModelFile {
  name: string;
  sha256: string | null;
  size: number;
  hf: string | null;
  url?: string | null;
}

export interface ModelEntry {
  id: string;
  family: string;
  role: ModelRole;
  display: string;
  license: string;
  license_url: string | null;
  source_repo: string | null;
  arch: string;
  modalities: string[];
  ctx_max: number;
  files: ModelFile[];
  requirements: { min_free_vram_mib: number; min_ram_mib: number; tiers: Tier[] };
  defaults: Record<string, unknown>;
  capabilities: Record<string, unknown>;
  prompt_profile: string | null;
  verified: boolean;
  user_added: boolean;
}

export interface InstallState {
  installed: boolean;
  folder: string | null;
  missing_files: string[];
}

export interface RegistryModelView extends ModelEntry {
  install: InstallState | null;
  total_size: number;
}

export interface RegistryView {
  models: RegistryModelView[];
  tier_defaults: Partial<Record<Tier, string>>;
  default_embedding: string | null;
  models_dir: string | null;
}

export interface Assessment {
  tier: TierAssessment;
  recommended_model: ModelEntry | null;
  active_model: ModelEntry | null;
  active_model_installed: InstallState | null;
}

export type Screen =
  | "home"
  | "library"
  | "search"
  | "assistant"
  | "review"
  | "schemas"
  | "knowledge"
  | "generate"
  | "projects"
  | "organize"
  | "jobs"
  | "models"
  | "settings";

export type PreRoutedOp =
  | { op: "set_theme"; theme: Theme }
  | { op: "set_language"; language: Language }
  | { op: "open"; screen: Screen }
  | { op: "help" };

export interface PreRouteBase {
  matched: boolean;
  source: "slash" | "phrase" | "none";
  confidence: number;
  normalized_text: string;
}

/** Discriminated on `op` (absent when nothing matched). */
export type PreRouteResult = PreRouteBase & (PreRoutedOp | { op?: undefined });

export interface DateRange {
  from: string;
  to: string;
  label: string;
}

export type LoadStatus =
  | { kind: "loaded" }
  | { kind: "created" }
  | { kind: "recovered_from_corrupt"; backup: string };

export type StartupWorkspaceStatus =
  | { kind: "none" }
  | { kind: "opened"; path: string }
  | { kind: "missing"; path: string }
  | { kind: "locked"; path: string; pid: number; host: string }
  | { kind: "failed"; path: string; message: string };

export interface StartupStatus {
  settings: LoadStatus;
  settings_path: string;
  workspace: StartupWorkspaceStatus;
}

export interface AppInfo {
  name: string;
  version: string;
  settings_path: string;
  app_data_dir: string;
  os: string;
  arch: string;
  debug: boolean;
}

export interface UiError {
  code: string;
  message: string;
  detail?: Record<string, unknown>;
}

export function isUiError(e: unknown): e is UiError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

export const SCREEN_ROUTES: Record<Screen, string> = {
  home: "/",
  library: "/library",
  search: "/search",
  assistant: "/assistant",
  review: "/review",
  schemas: "/schemas",
  knowledge: "/knowledge",
  generate: "/generate",
  projects: "/projects",
  organize: "/organize",
  jobs: "/jobs",
  models: "/models",
  settings: "/settings",
};
