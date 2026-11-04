// TypeScript mirror of the Rust types exchanged over Tauri IPC
// (crates/doculens-core, src-tauri). Keep field names in snake_case to match serde.

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
export type Backend = "cu129" | "cu121" | "vulkan" | "cpu";
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
  auto_start_engine: boolean;
  warmup_after_load: boolean;
  vision_on_cpu: boolean;
  engine_binary: string | null;
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

// --- model store (drop-in folders) -----------------------------------------

export type ModelHealth =
  | { kind: "ready" }
  | { kind: "copying" }
  | { kind: "unsupported"; reason: string }
  | { kind: "incomplete"; missing: string[] }
  | { kind: "invalid"; reason: string };

export type MatchBy = "hash" | "name";

export interface RegistryMatch {
  id: string;
  by: MatchBy;
  verified: boolean | null;
}

export interface FileHashes {
  size: number;
  sha256: string;
  blake3: string;
}

export interface ModelMeta {
  kind: "llm" | "mmproj" | "unknown";
  architecture: string;
  name: string | null;
  file_type: number | null;
  quantization: string | null;
  license: string | null;
  license_link: string | null;
  source_url: string | null;
  source_repo: string | null;
  has_chat_template: boolean;
  block_count: number | null;
  context_length: number | null;
  embedding_length: number | null;
  head_count: number | null;
  head_count_kv: number | null;
  key_length: number | null;
  full_attention_interval: number | null;
  split_count: number | null;
  split_no: number | null;
  projector_type: string | null;
  has_vision_encoder: boolean | null;
  has_audio_encoder: boolean | null;
  tensor_count: number;
  tensor_types: Record<string, number>;
  gdn_tensor_types: Record<string, string>;
  gdn_low_precision: string[];
  parameter_count: number | null;
  data_bytes: number;
}

export interface ScannedFile {
  path: string;
  name: string;
  size: number;
  modified: string | null;
  stable: boolean;
  meta: ModelMeta | null;
  hashes: FileHashes | null;
  error: string | null;
}

export interface ModelJson {
  id: string | null;
  display: string | null;
  family: string | null;
  role: ModelRole | null;
  prompt_profile: string | null;
  license: string | null;
  license_url: string | null;
  min_free_vram_mib: number | null;
  defaults: Record<string, unknown> | null;
}

export interface DiscoveredModel {
  folder: string;
  folder_name: string;
  id: string;
  display: string;
  weights: ScannedFile[];
  mmproj: ScannedFile | null;
  extra_ggufs: string[];
  model_json: ModelJson | null;
  license_files: string[];
  architecture: string | null;
  quantization: string | null;
  has_chat_template: boolean;
  has_vision: boolean;
  role: ModelRole;
  total_bytes: number;
  health: ModelHealth;
  warnings: string[];
  registry_match: RegistryMatch | null;
  hashed: boolean;
  scanned_at: string;
}

export interface ModelIndex {
  discovered: DiscoveredModel[];
  roots: string[];
  scanned_at: string | null;
}

export interface InstallState {
  installed: boolean;
  folder: string | null;
  missing_files: string[];
  verified: boolean | null;
  health: ModelHealth | null;
  matched_by: MatchBy | null;
  warnings: string[];
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

export type ImportMode = "copy" | "move" | "link";
export const IMPORT_MODES: ImportMode[] = ["copy", "move", "link"];

export interface ImportPlanFile {
  source: string;
  dest_name: string;
  size: number;
  meta: ModelMeta | null;
  is_mmproj: boolean;
}

export interface ImportPlan {
  folder_name: string;
  target_folder: string;
  files: ImportPlanFile[];
  license_files: string[];
  total_bytes: number;
  mode: ImportMode;
  auto_paired_mmproj: boolean;
  architecture: string | null;
  quantization: string | null;
  has_vision: boolean;
  warnings: string[];
  already_present: boolean;
}

// --- engine ----------------------------------------------------------------

export type EngineState =
  | { kind: "stopped" }
  | { kind: "starting" }
  | { kind: "running" }
  | { kind: "restarting"; attempt: number; delay_ms: number }
  | { kind: "degraded"; reason: string }
  | { kind: "stopping" };

export type EngineSource = "env" | "override" | "app_data" | "bundled";

export interface EngineBinary {
  path: string;
  backend: Backend;
  version: string | null;
  build_number: number | null;
  source: EngineSource;
}

export interface InstalledPack {
  backend: Backend;
  version: string;
  build_number: number | null;
  path: string;
}

export interface EngineBinaryInfo {
  resolved: EngineBinary | null;
  error: string | null;
  installed_packs: InstalledPack[];
  engines_dir: string;
  bundled_roots: string[];
  min_build: number;
}

export type ModelInstanceState = "unloaded" | "loading" | "loaded" | "sleeping" | "downloading" | "incomplete" | string;

export interface ModelInstanceStatus {
  id: string;
  status: ModelInstanceState;
  failed: boolean;
  exit_code: number | null;
  progress: number | null;
  stage: string | null;
  memory_effective_bytes: number | null;
  memory_min_bytes: number | null;
  mmproj_bytes: number;
  input_modalities: string[];
  loaded_at: string | null;
  warmed_up: boolean;
}

export interface EngineStatus {
  state: EngineState;
  binary: EngineBinary | null;
  pid: number | null;
  host: string;
  port: number | null;
  started_at: string | null;
  restarts: number;
  last_error: string | null;
  last_exit_code: number | null;
  models: ModelInstanceStatus[];
  presets_path: string | null;
  active_model: string | null;
  sleep_idle_seconds: number;
}

export type LogStream = "stdout" | "stderr" | "app";

export interface LogLine {
  at: string;
  stream: LogStream;
  text: string;
}

export interface EngineDevice {
  name: string;
  description: string;
  total_mib: number;
  free_mib: number;
}

export interface ChatTestResult {
  model: string;
  prompt: string;
  content: string;
  finish_reason: string | null;
  prompt_tokens: number;
  completion_tokens: number;
  elapsed_ms: number;
  tokens_per_second: number | null;
}

// --- jobs ------------------------------------------------------------------

export type JobKind =
  | "engine_start"
  | "model_load"
  | "model_import"
  | "model_download"
  | "model_hash"
  | "document_import"
  | "document_rescan"
  | "document_render";
export type JobState = "queued" | "running" | "done" | "failed" | "cancelled";

export interface Job {
  id: string;
  kind: JobKind;
  subject: string;
  state: JobState;
  progress: number | null;
  detail: string | null;
  bytes_done: number | null;
  bytes_total: number | null;
  error: string | null;
  cancel_requested: boolean;
  created_at: string;
  updated_at: string;
  finished_at: string | null;
}

export function isJobFinished(j: Job): boolean {
  return j.state === "done" || j.state === "failed" || j.state === "cancelled";
}

// --- misc ------------------------------------------------------------------

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
  | { kind: "partially_recovered"; sections: string[] }
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
  engines_dir: string;
  presets_path: string;
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

/** Event names emitted by the Rust shell (`engine_bridge.rs`, `jobs.rs`). */
export const EVENTS = {
  engineStatus: "engine://status",
  engineLog: "engine://log",
  engineModel: "engine://model",
  modelsChanged: "models://changed",
  jobsUpdate: "jobs://update",
  documentsChanged: "documents://changed",
} as const;

// ---------------------------------------------------------------------------
// Documents (P2) — mirrors crates/doculens-core/src/documents and
// src-tauri/src/commands/documents.rs
// ---------------------------------------------------------------------------

export type SourceKind = "pdf" | "image" | "hwpx" | "docx" | "hwp";
export const SOURCE_KINDS: SourceKind[] = ["pdf", "image", "hwpx", "docx", "hwp"];
export type DocumentStatus =
  | "importing"
  | "new"
  | "rendered"
  | "extracted"
  | "reviewed"
  | "archived"
  | "missing"
  | "error";
export const DOCUMENT_STATUSES: DocumentStatus[] = [
  "importing",
  "new",
  "rendered",
  "extracted",
  "reviewed",
  "archived",
  "missing",
  "error",
];
export type DocumentRole = "work" | "knowledge" | "form" | "generated";
export type DocumentSort = "imported_desc" | "imported_asc" | "name_asc" | "name_desc" | "size_desc";

export interface DocumentRecord {
  id: string;
  rel_path: string;
  original_name: string;
  source_kind: SourceKind;
  content_hash: string;
  size_bytes: number;
  file_modified_at: string | null;
  imported_at: string;
  updated_at: string;
  status: DocumentStatus;
  role: DocumentRole;
  title: string | null;
  doc_type: string | null;
  vendor: string | null;
  doc_date: string | null;
  total_amount: number | null;
  language: string | null;
  page_count: number;
  has_text_layer: boolean;
  converted: boolean;
  conversion_note: string | null;
  render_dpi: number | null;
  error: string | null;
  import_batch_id: string | null;
  deleted_at: string | null;
  tags: string[];
}

/** `DocumentRecord` plus the URLs / paths the UI needs (`documents_list`, `documents_get`). */
export interface DocumentView extends DocumentRecord {
  absolute_path: string;
  thumb_url: string | null;
}

export interface PageRecord {
  document_id: string;
  page_no: number;
  width_px: number;
  height_px: number;
  width_pt: number | null;
  height_pt: number | null;
  dpi: number | null;
  rotation: number;
  has_text: boolean;
  text?: string | null;
  is_blank: boolean | null;
  rendered_at: string | null;
}

export interface PageView extends PageRecord {
  image_url: string | null;
  thumb_url: string | null;
}

export interface DocumentFilter {
  statuses?: DocumentStatus[];
  kinds?: SourceKind[];
  folder?: string | null;
  query?: string | null;
  tag?: string | null;
  doc_type?: string | null;
  include_deleted?: boolean;
  sort?: DocumentSort;
  limit?: number | null;
  offset?: number;
}

export interface FolderCount {
  rel_dir: string;
  count: number;
}

export interface LibraryStats {
  total: number;
  by_status: Record<string, number>;
  by_kind: Record<string, number>;
  folders: FolderCount[];
  total_pages: number;
}

export type ImportSource = "drop" | "folder" | "inbox" | "onboarding" | "rescan";
export type ImportDestination = "documents" | "inbox";

export interface ImportOptions {
  destination: ImportDestination;
  move_files: boolean;
  render: boolean;
  role: DocumentRole;
  dpi: number | null;
}

export type ImportItemStatus =
  | { kind: "new" }
  | { kind: "duplicate"; of: string; of_rel_path: string }
  | { kind: "duplicate_in_batch"; of: string }
  | { kind: "already_registered"; id: string }
  | { kind: "relink"; id: string; old_rel_path: string }
  | { kind: "unsupported"; reason: string }
  | { kind: "copying" };

export interface ImportItem {
  path: string;
  name: string;
  size: number;
  kind: SourceKind | null;
  inside_workspace: boolean;
  rel_path: string | null;
  content_hash: string | null;
  status: ImportItemStatus;
}

export interface DocumentImportPlan {
  token: string;
  source: ImportSource;
  options: ImportOptions;
  items: ImportItem[];
  new_count: number;
  relink_count: number;
  duplicate_count: number;
  registered_count: number;
  unsupported_count: number;
  copying_count: number;
  new_bytes: number;
  truncated: boolean;
}

export interface DocumentsChanged {
  kind: "imported" | "updated" | "removed" | "rescanned" | "batch";
  ids: string[];
}

export interface PdfiumInfo {
  library: string | null;
  error: string | null;
  searched: string[];
}

export function defaultImportOptions(): ImportOptions {
  return { destination: "documents", move_files: false, render: true, role: "work", dpi: null };
}
