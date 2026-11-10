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
export type AutoProcessingLevel = "none" | "classify_only" | "classify_and_ocr" | "full";
export const AUTO_PROCESSING_LEVELS: AutoProcessingLevel[] = ["none", "classify_only", "classify_and_ocr", "full"];
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
  inbox_auto_level: AutoProcessingLevel;
  render_dpi: number;
  review_threshold: number;
  audit_sample_percent: number;
  organize_reviewed_only: boolean;
  inbox_watch: boolean;
  ocr_digital_pdf: boolean;
  kie_max_pages: number;
  precise_mode: boolean;
  dense_reocr: boolean;
  dense_cells: number;
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
  knowledge: KnowledgeSettings;
  generation: GenerationSettings;
  shortcuts: Record<string, string>;
  window: WindowState;
}

/** Reply token caps of document generation by engine backend (settings > 처리). */
export interface GenerationSettings {
  max_tokens_gpu: number;
  max_tokens_cpu: number;
}

export interface KnowledgeSettings {
  auto_index: boolean;
  auto_embed: boolean;
  embedding_model: string | null;
  latest_only: boolean;
  allow_general_knowledge: boolean;
  context_tokens: number;
  top_k: number;
  chunk_target_tokens: number;
  min_vector_similarity: number;
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
  | "document_render"
  | "knowledge_index"
  | "knowledge_embed"
  | "pipeline"
  | "document_split"
  | "document_merge"
  | "plan"
  | "export"
  | "organize"
  | "undo"
  | "redo"
  | "draft"
  | "maintenance"
  | "schema_discovery";
export type JobState = "queued" | "running" | "paused" | "done" | "failed" | "cancelled" | "interrupted";
export type JobPriority = "low" | "normal" | "high" | "interactive";

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
  // -- persistent runner (P5) --
  priority: JobPriority;
  /** Stored in the workspace database: survives restarts, can be paused / retried / prioritised. */
  persistent: boolean;
  attempts: number;
  /** Structured outcome (PlanExecution, ExportResult, ApplyReport, UndoReport …) once finished. */
  result?: unknown;
  /** Human-readable label of the request (plan text, organise label …). */
  label?: string | null;
}

export function isJobFinished(j: Job): boolean {
  return j.state === "done" || j.state === "failed" || j.state === "cancelled" || j.state === "interrupted";
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
  /** Structured detail: an object (template vars, pids …) or an array (`plan_invalid` → `PlanIssue[]`). */
  detail?: unknown;
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
  knowledgeChanged: "knowledge://changed",
  knowledgeAnswer: "knowledge://answer",
  pipelineChanged: "pipeline://changed",
  projectsChanged: "projects://changed",
} as const;

// ---------------------------------------------------------------------------
// Documents (P2) — mirrors crates/doculens-core/src/documents and
// src-tauri/src/commands/documents.rs
// ---------------------------------------------------------------------------

export type SourceKind = "pdf" | "image" | "hwpx" | "docx" | "hwp" | "xlsx" | "pptx" | "text" | "xls" | "doc" | "ppt" | "rtf" | "hml";
export const SOURCE_KINDS: SourceKind[] = ["pdf", "image", "hwpx", "docx", "hwp", "xlsx", "pptx", "text", "xls", "doc", "ppt", "rtf", "hml"];
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
export type ReviewStatus = "none" | "pending" | "done";
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
  // -- pipeline (P4) --
  doc_type_source: string | null;
  classified_at: string | null;
  ocr_at: string | null;
  extracted_at: string | null;
  last_run_id: string | null;
  schema_id: string | null;
  review_status: ReviewStatus;
  confidence: number | null;
  pipeline_error: string | null;
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
  // -- pipeline (P4) --
  text_source: string | null;
  ocr_at: string | null;
  ocr_profile: string | null;
  ocr_model: string | null;
  classified_at: string | null;
  page_doc_type: string | null;
  page_language: string | null;
  page_rotation: number | null;
}

export interface PageView extends PageRecord {
  image_url: string | null;
  thumb_url: string | null;
}

/** How the word boxes of a page layout were obtained (`documents::layout`). */
export type LayoutSource = "layer" | "estimated";

/** One word of a page's text layer; `x`/`y`/`w`/`h` are fractions of the page image. */
export interface PageWord {
  t: string;
  x: number;
  y: number;
  w: number;
  h: number;
  /** Reading-order line index (0-based). */
  line: number;
  edited?: boolean;
}

/** Positioned text layer of a page: selection and correction happen on the page image. */
export interface PageLayout {
  version: number;
  source: LayoutSource;
  words: PageWord[];
}

/** One correction of a page's text (viewer 이력 "글자 수정"); `undone` after Ctrl+Z (0.3.0 item 7). */
export interface PageTextEdit {
  id: number;
  page_no: number;
  /** `layer` (words edited on the image), `add` (words added), `manual` (whole text). */
  source: string;
  edited_at: string;
  undone: boolean;
  /** The words that changed (common head and tail stripped). */
  before: string;
  after: string;
}

/** Corrections of a page (or document), newest first, and whether the page can undo / redo. */
export interface PageTextHistory {
  entries: PageTextEdit[];
  can_undo: boolean;
  can_redo: boolean;
}

/** The page after an undo / redo / revert of its text. */
export interface PageTextRevert {
  page_no: number;
  text: string | null;
  text_source: string | null;
  layout: PageLayout | null;
  history: PageTextHistory;
}

export interface DocumentFilter {
  statuses?: DocumentStatus[];
  kinds?: SourceKind[];
  folder?: string | null;
  query?: string | null;
  tag?: string | null;
  doc_type?: string | null;
  review_status?: ReviewStatus | null;
  max_confidence?: number | null;
  role?: DocumentRole | null;
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
  /** Knowledge collection that receives the whole batch (ingest wizard). */
  collection_id?: string | null;
  /** Project that receives the whole batch (files dropped onto a project). */
  project_id?: string | null;
  auto_level?: AutoProcessingLevel | null;
}

/** Structured result of a finished `document_import` job (`Job.result`). */
export interface ImportReport {
  batch_id: string;
  imported: string[];
  relinked: string[];
  failed: [string, string][];
  skipped: number;
  cancelled: boolean;
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
  /** Classification remark (the content, not the extension, decided the kind). */
  note?: string | null;
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
  return { destination: "documents", move_files: false, render: true, role: "work", dpi: null, collection_id: null };
}

// ---------------------------------------------------------------------------
// Knowledge base (P3) — mirrors crates/doculens-core/src/knowledge and
// src-tauri/src/commands/knowledge.rs
// ---------------------------------------------------------------------------

/**
 * 참고 자료 묶음 (0.5.0 plan §2): `reference` = a group of the common reference material, `project` = the one
 * reference group of a project (`project_id` set; created on demand by `project_reference_group`).
 */
export type CollectionKind = "reference" | "project";

export interface Collection {
  id: string;
  name: string;
  description: string | null;
  color: string | null;
  active: boolean;
  kind: CollectionKind;
  /** The owning project of a `project` group; null for the common reference groups. */
  project_id: string | null;
  created_at: string;
  updated_at: string;
  doc_count: number;
  chunk_count: number;
}

export interface CollectionInput {
  id?: string | null;
  name: string;
  description?: string | null;
  color?: string | null;
  active?: boolean;
  /** Set on a project's reference group (kind `project`); omitted / null = a common reference group. */
  project_id?: string | null;
}

/** The common reference material's virtual id on the projects screen (`/projects/common`). */
export const COMMON_REFERENCE_ID = "common";

export interface VersionInfo {
  document_id: string;
  title: string;
  group_key: string;
  version_label: string | null;
  effective_date: string | null;
  is_latest: boolean;
  note: string | null;
}

export interface VersionGroup {
  group_key: string;
  documents: VersionInfo[];
}

export interface VersionInput {
  document_id: string;
  group_key: string;
  version_label?: string | null;
  effective_date?: string | null;
  is_latest?: boolean;
  note?: string | null;
}

export interface GlossaryEntry {
  id: number;
  term: string;
  definition: string | null;
  synonyms: string[];
  created_at: string;
  updated_at: string;
}

export interface GlossaryInput {
  id?: number | null;
  term: string;
  definition?: string | null;
  synonyms?: string[];
}

export type ChunkKind = "text" | "table" | "heading" | "list";

export interface Chunk {
  id: number;
  document_id: string;
  chunk_index: number;
  page_start: number;
  page_end: number;
  kind: ChunkKind;
  heading_path: string | null;
  clause: string | null;
  text: string;
  token_estimate: number;
  embedded: boolean;
}

export type SearchMode = "hybrid" | "keyword" | "semantic";
export type SearchChannel = "morph" | "trigram" | "like" | "vector";

export interface SearchScope {
  collections?: string[];
  documents?: string[];
  roles?: DocumentRole[];
  folder?: string | null;
  latest_only?: boolean;
  active_collections_only?: boolean;
  /**
   * Project scope (0.5.0 plan §2): the allowed set is the project's work documents ∪ its reference group ∪ the
   * active common reference groups; with `documents` also set, those documents replace the work documents.
   */
  project_id?: string | null;
}

export interface SearchRequest {
  query: string;
  mode?: SearchMode;
  scope?: SearchScope;
  limit?: number;
  expand_glossary?: boolean;
  per_document?: number;
}

export interface SearchHit {
  chunk_id: number;
  document_id: string;
  title: string;
  rel_path: string;
  role: DocumentRole;
  doc_type: string | null;
  page_start: number;
  page_end: number;
  kind: ChunkKind;
  heading_path: string | null;
  clause: string | null;
  text: string;
  snippet: string;
  highlights: [number, number][];
  score: number;
  channels: SearchChannel[];
  vector_similarity: number | null;
}

export interface SearchResponse {
  hits: SearchHit[];
  query_terms: string[];
  expanded_terms: string[];
  used_vector: boolean;
  used_like_fallback: boolean;
  elapsed_ms: number;
  total_candidates: number;
}

export interface TokenizerInfo {
  kind: string;
  path: string | null;
  version: string | null;
}

export interface EmbeddingModelInfo {
  model: string | null;
  installed: boolean;
  engine_running: boolean;
  loaded: boolean;
}

export interface KnowledgeStatus {
  documents_total: number;
  documents_indexed: number;
  documents_pending: number;
  documents_no_text: number;
  documents_failed: number;
  knowledge_documents: number;
  chunks_total: number;
  chunks_embedded: number;
  embedding_model: string | null;
  embedding_dim: number | null;
  tokenizer: TokenizerInfo;
  collections: number;
  glossary_terms: number;
  answers: number;
  answers_helpful: number;
  answers_wrong: number;
  embedding: EmbeddingModelInfo;
  index_running: boolean;
  embed_running: boolean;
}

export type AnswerKind = "answer" | "explain" | "summary" | "compare";
export type SummaryMode = "one_line" | "key_points" | "one_page";
export type AnswerLanguage = "auto" | "ko" | "en";

/** One earlier turn of the assistant conversation (context for follow-ups, never evidence). */
export type TurnRole = "user" | "assistant";
export interface ConversationTurn {
  role: TurnRole;
  text: string;
}

/** One assistant chat session (0.7.0 §1.6): stored in the workspace DB, listed by `updated_at` desc. */
export interface ChatSessionSummary {
  id: string;
  title: string;
  /** The title is derived from the first user message until the user renames the session. */
  title_auto: boolean;
  created_at: string;
  updated_at: string;
  message_count: number;
}

/** A stored chat message: the UI's message JSON as it was put (`seq` = the message id within the session). */
export interface ChatMessageRow {
  seq: number;
  payload: unknown;
  created_at: string;
}

/** What the user is looking at while asking (names resolved by the shell from the ids the UI sends). */
export interface SessionState {
  current_document?: string | null;
  current_page?: number | null;
  selected_documents?: string[];
  folder?: string | null;
  project?: string | null;
  recent_documents?: string[];
}

export interface AnswerRequest {
  question: string;
  kind?: AnswerKind;
  scope?: SearchScope;
  allow_general?: boolean;
  top_k?: number;
  context_tokens?: number;
  language?: AnswerLanguage;
  selection?: string | null;
  document_id?: string | null;
  compare_document_id?: string | null;
  summary_mode?: SummaryMode;
  keep_chunks?: number[];
  save?: boolean;
  /** Look evidence up (default true). false for a request that brings its own text (다듬기 · 교정 · 번역): nothing is retrieved or cited. */
  retrieve?: boolean;
  min_similarity?: number | null;
  /** Earlier turns, oldest first (the question itself is not included). */
  history?: ConversationTurn[];
  session?: SessionState | null;
}

export interface Citation {
  index: number;
  chunk_id: number;
  document_id: string;
  title: string;
  page_start: number;
  page_end: number;
  heading_path: string | null;
  clause: string | null;
  score: number;
  excerpt: string;
}

export interface AnswerSentence {
  text: string;
  citations: number[];
}

export interface Answer {
  id: number | null;
  kind: AnswerKind;
  question: string;
  text: string;
  sentences: AnswerSentence[];
  sources: Citation[];
  no_evidence: boolean;
  used_general_knowledge: boolean;
  conflicts: string | null;
  model: string | null;
  elapsed_ms: number;
  prompt_tokens: number;
  completion_tokens: number;
  document_id: string | null;
  created_at: string | null;
  feedback: number | null;
}

export interface SavedAnswer {
  id: number;
  kind: AnswerKind;
  question: string;
  text: string;
  sentences: AnswerSentence[];
  sources: Citation[];
  no_evidence: boolean;
  used_general_knowledge: boolean;
  model: string | null;
  document_id: string | null;
  feedback: number | null;
  feedback_note: string | null;
  created_at: string;
}

export interface KnowledgeChanged {
  kind: "indexed" | "embedded" | "collections" | "glossary" | "versions" | "answers" | "status";
  ids: string[];
}

export interface AnswerDelta {
  request_id: string;
  delta: string;
  done: boolean;
}

// ---------------------------------------------------------------------------
// Pipeline (P4) — mirrors crates/doculens-core/src/pipeline and
// src-tauri/src/commands/pipeline.rs
// ---------------------------------------------------------------------------

export type PipelineLevel = "classify" | "ocr" | "extract";
export const PIPELINE_LEVELS: PipelineLevel[] = ["classify", "ocr", "extract"];
export type SchemaChoice = { kind: "auto" } | { kind: "id"; id: string };

export interface ProcessRequest {
  document_ids: string[];
  level: PipelineLevel;
  schema?: SchemaChoice;
  force?: boolean;
  profile?: Profile | null;
  pages?: number[];
  precise?: boolean | null;
}

export type FieldKind = "string" | "amount" | "integer" | "date" | "boolean" | "enum" | "brn" | "rrn" | "account" | "phone" | "email" | "table" | "group";
export const SCALAR_FIELD_KINDS: FieldKind[] = ["string", "amount", "integer", "date", "boolean", "enum", "brn", "rrn", "account", "phone", "email"];
export const FIELD_KINDS: FieldKind[] = [...SCALAR_FIELD_KINDS, "table", "group"];

export interface FieldDef {
  key: string;
  label_ko: string;
  label_en: string;
  kind: FieldKind;
  required: boolean;
  regex_hint: string | null;
  example: string | null;
  description: string | null;
  options: string[];
  fields: FieldDef[];
  mask: boolean;
}

export function newFieldDef(key = "", kind: FieldKind = "string"): FieldDef {
  return { key, label_ko: "", label_en: "", kind, required: false, regex_hint: null, example: null, description: null, options: [], fields: [], mask: false };
}

export interface SchemaDef {
  fields: FieldDef[];
  rules: string[];
}

export type SchemaSource = "builtin" | "nl" | "user" | "form";

export interface SchemaRecord {
  id: string;
  name: string;
  name_en: string | null;
  doc_type: string;
  source: SchemaSource;
  version: number;
  description_ko: string | null;
  description_en: string | null;
  definition: SchemaDef;
  form_document_id: string | null;
  builtin_key: string | null;
  enabled: boolean;
  is_default: boolean;
  use_count: number;
  /** Change note of the current version (older notes are in the version list). */
  note?: string | null;
  created_at: string;
  updated_at: string;
}

export interface SchemaDraft {
  id: string | null;
  name: string;
  name_en: string | null;
  doc_type: string;
  source: SchemaSource;
  description_ko: string | null;
  description_en: string | null;
  definition: SchemaDef;
  form_document_id: string | null;
  note: string | null;
  /** Further sample documents of a multi-document discovery (0.3.0 §2); registered as forms with the first. */
  sample_document_ids?: string[];
}

/** `forms::DocDiscovery` — what one sample document contributed to a `schema_discovery` job. */
export interface DocDiscovery {
  doc_id: string;
  title: string;
  /** `structure` (office tables) | `model` | `text` (heuristics) | `failed`. */
  source: string;
  pages_used: number[];
  field_count: number;
  note?: string | null;
}

/** `forms::DiscoveryOutcome` — the result of a `schema_discovery` job: the unsaved draft + per-document summary. */
export interface SchemaDiscoveryOutcome {
  kind: "schema_discovery";
  draft: SchemaDraft;
  documents: DocDiscovery[];
  merged_fields: number;
  dropped_fields: number;
}

export function isSchemaDiscoveryOutcome(v: unknown): v is SchemaDiscoveryOutcome {
  const o = v as SchemaDiscoveryOutcome | null;
  return typeof o === "object" && o !== null && o.kind === "schema_discovery" && typeof o.draft === "object" && o.draft !== null && Array.isArray(o.documents);
}

export function draftFromSchema(s: SchemaRecord): SchemaDraft {
  return {
    id: s.id,
    name: s.name,
    name_en: s.name_en,
    doc_type: s.doc_type,
    source: s.source,
    description_ko: s.description_ko,
    description_en: s.description_en,
    definition: structuredClone(s.definition),
    form_document_id: s.form_document_id,
    note: s.note ?? null,
  };
}

export interface LintIssue {
  level: "error" | "warn";
  path: string;
  code: string;
  message: string;
}

export interface SchemaVersionView {
  version: number;
  definition: SchemaDef;
  note: string | null;
  created_at: string;
}

export type FieldStatus = "auto" | "approved" | "corrected" | "rejected" | "held";
export type ReviewReason = "low_confidence" | "validation_failed" | "required_missing" | "audit";
export const REVIEW_REASONS: ReviewReason[] = ["low_confidence", "validation_failed", "required_missing", "audit"];

export interface ValidationIssue {
  rule: string;
  level: "ok" | "warn" | "fail";
  message: string;
}

export interface FieldRecord {
  id: number;
  run_id: string;
  document_id: string;
  path: string;
  key: string;
  row: number | null;
  label_ko: string;
  label_en: string;
  kind: FieldKind;
  required: boolean;
  value: string | null;
  normalized: string | null;
  original_value: string | null;
  page: number | null;
  bbox: [number, number, number, number] | null;
  evidence: string | null;
  confidence: number;
  model_confidence: number | null;
  status: FieldStatus;
  review_reason: ReviewReason | null;
  validation: ValidationIssue[];
  reviewed_at: string | null;
}

/** A field whose review is still open (flagged and not approved / corrected / rejected). */
export function fieldNeedsReview(f: FieldRecord): boolean {
  return f.review_reason !== null && !["approved", "corrected", "rejected"].includes(f.status);
}

export interface ExtractionRun {
  id: string;
  document_id: string;
  schema_id: string | null;
  schema_version: number | null;
  doc_type: string | null;
  model: string | null;
  prompt_version: string | null;
  profile: string | null;
  precise: boolean;
  status: "running" | "done" | "failed" | "cancelled";
  error: string | null;
  started_at: string;
  finished_at: string | null;
  duration_ms: number | null;
  prompt_tokens: number;
  completion_tokens: number;
  pages_done: number;
  confidence: number | null;
  field_count: number;
  review_count: number;
}

export interface FieldChange {
  id: number;
  field_id: number;
  document_id: string;
  path: string;
  old_value: string | null;
  new_value: string | null;
  old_status: FieldStatus;
  new_status: FieldStatus;
  changed_at: string;
  note: string | null;
}

export interface DocTypeInfo {
  slug: string;
  label_ko: string;
  label_en: string;
  schema_id: string | null;
  builtin: boolean;
  use_count: number;
}

export interface VisionModelInfo {
  model: string | null;
  engine_running: boolean;
  loaded: boolean;
}

export interface PipelineStatus {
  documents: number;
  classified: number;
  ocr_done: number;
  extracted: number;
  review_pending: number;
  reviewed: number;
  review_fields: number;
  failed: number;
  running: boolean;
  queued: number;
  model: VisionModelInfo;
  review_threshold: number;
  /** Engine gate (P5): a batch is inside a page / interactive requests waiting. */
  gate: GateStatus;
}

export interface GateStatus {
  batch_busy: boolean;
  waiting: number;
  estimated_wait_ms: number;
}

export interface ExtractedTable {
  page: number;
  index: number;
  header: string[];
  rows: string[][];
}

export interface ReviewFilter {
  document_id?: string | null;
  doc_type?: string | null;
  reason?: ReviewReason | null;
  include_resolved?: boolean;
  limit?: number | null;
}

export interface ReviewItem extends FieldRecord {
  document_name: string;
  doc_type: string | null;
  page_image_url: string | null;
  page_count: number;
}

export type ReviewAction = { kind: "approve" } | { kind: "reject" } | { kind: "hold" } | { kind: "reset" } | { kind: "correct"; value: string };

export interface PipelineChanged {
  kind: "processed" | "fields" | "schemas" | "review" | "status" | "engine_off" | "no_model" | "journal";
  ids: string[];
}

// ---------------------------------------------------------------------------
// Planner / organise / journal / aggregate / export (P5) — mirrors
// crates/doculens-core/src/{planner,organize,aggregate,export}.rs and
// src-tauri/src/commands/planner.rs
// ---------------------------------------------------------------------------

export type OpKind =
  | "import"
  | "set_inbox_watch"
  | "classify"
  | "set_doc_type"
  | "ocr"
  | "define_schema"
  | "extract"
  | "validate"
  | "review"
  | "reindex"
  | "search"
  | "answer"
  | "summarize"
  | "aggregate"
  | "organize"
  | "rename"
  | "move"
  | "tag"
  | "delete"
  | "export"
  | "draft_document"
  | "revise_draft"
  | "register_form"
  | "explain"
  | "compare"
  | "create_project"
  | "add_to_project"
  | "define_rule"
  | "check_consistency"
  | "request_supplement"
  | "set_retention"
  | "set_model"
  | "set_profile"
  | "set_theme"
  | "set_language"
  | "set_setting"
  | "open"
  | "job"
  | "engine"
  | "undo"
  | "redo"
  | "help"
  | "clarify"
  | "noop"
  // 0.6.0 (plan §1): every screen action as a planner op.
  | "rotate"
  | "rerender"
  | "split"
  | "merge"
  | "set_title"
  | "edit_text"
  | "set_role"
  | "manage_reference"
  | "define_term"
  | "set_version"
  | "update_project"
  | "delete_project"
  | "remove_from_project"
  | "manage_template"
  | "classify_projects"
  | "manage_schema"
  | "manage_model"
  | "workspace"
  | "maintenance";

/** Op families of the two-stage planner (0.6.0 plan §2): stage 1 picks one or two, stage 2 plans with their ops. */
export type OpFamily = "ask" | "documents" | "reference" | "projects" | "forms" | "compose" | "output" | "system";
export const OP_FAMILIES: OpFamily[] = ["ask", "documents", "reference", "projects", "forms", "compose", "output", "system"];

export type AggregateFn = "sum" | "count" | "avg" | "min" | "max";
export type GroupBy = "month" | "vendor" | "doc_type" | "project" | "none";
export type ExportFormat = "xlsx" | "csv" | "jsonl" | "md" | "pdf_searchable";
export const EXPORT_FORMATS: ExportFormat[] = ["xlsx", "csv", "jsonl", "md", "pdf_searchable"];
export type ExportLayout = "sheet_per_type" | "sheet_per_project" | "single";
export type ExportInclude = "evidence" | "confidence" | "status";
export type ExportTarget = "fields" | "table" | "text";
/** The pre-0.6.0 `set_setting` keys; still accepted as aliases of their catalog paths (`src/lib/settingsCatalog.ts`). */
export type LegacySettingKey = "render_dpi" | "pii_masking" | "auto_pipeline" | "inbox_watch" | "review_threshold" | "cache_limit_gb" | "journal_days";
/** `set_setting.key` (0.6.0): a settings-catalog path such as `processing.render_dpi`, or a legacy alias. */
export type SettingKey = string;
export type ReviewOp = "show" | "approve" | "reject" | "correct";
export type ReviewOnly = "low_confidence" | "failed" | "all";
export type JobOp = "show" | "cancel" | "retry" | "pause" | "resume" | "prioritize";
export type EngineOp = "start" | "stop" | "restart" | "sleep" | "wake";
/** 0.6.0 management ops (plan §1). */
export type ReferenceAction = "create" | "rename" | "delete" | "enable" | "disable" | "add" | "remove";
export type TermAction = "save" | "delete";
export type TemplateAction = "save" | "delete";
export type SchemaAction = "enable" | "disable" | "set_default" | "delete" | "rename";
export type ModelAction = "set_tier" | "set_backend" | "import" | "download" | "delete" | "verify" | "benchmark" | "load" | "unload";
export type ModelTierChoice = Tier | "auto";
export type WorkspaceAction = "open" | "create" | "close" | "forget";
export type MaintenanceAction = "purge_now" | "clear_finished_jobs" | "rescan" | "inbox_scan";
export type RoleChoice = "work" | "knowledge" | "form";
export type OpenTarget = "document" | "page" | "folder" | "screen";
/** `explain.scope` (appendix D). */
/** `reference` = the 공통 참고 자료 (0.5.0; `collection` was its pre-0.5.0 name and still deserialises in Rust). */
export type ExplainScope = "kb" | "reference" | "project" | "document";
/** `set_retention.item` (appendix D; the page cache is size-capped → `set_setting cache_limit_gb`). */
export type RetentionTarget = "documents" | "extractions" | "logs" | "journal" | "chats" | "temp";
export const RETENTION_TARGETS: RetentionTarget[] = ["documents", "extractions", "logs", "journal", "chats", "temp"];

/** One plan step (appendix D parameters; optional ones may be absent or null). */
export type Op =
  | { op: "import"; source?: "inbox" | "folder" | "files"; path?: string | null; paths?: string[]; recursive?: boolean | null; auto_pipeline?: "none" | "classify" | "full" | null }
  | { op: "set_inbox_watch"; enabled: boolean }
  | { op: "classify"; doc_types?: string[]; force?: boolean | null }
  | { op: "set_doc_type"; doc_type: string }
  | { op: "ocr"; pages?: number[]; profile?: "standard" | "fine_print" | null; force?: boolean | null }
  | { op: "define_schema"; name?: string | null; description?: string | null; mode?: "create" | "modify" | null; base?: string | null }
  | { op: "extract"; schema_ref?: string | null; fields?: string[]; force?: boolean | null }
  | { op: "validate" }
  | { op: "review"; action: ReviewOp; field?: string | null; value?: string | null; only?: ReviewOnly | null }
  | { op: "reindex"; scope?: "fts" | "vec" | "all" | null }
  | { op: "search"; query: string; mode?: SearchMode | null; top_k?: number | null }
  | { op: "answer"; question: string; cite?: boolean | null; knowledge?: boolean | null }
  | { op: "summarize"; target?: "each_doc" | "set" | null; length?: "short" | "long" | null }
  | { op: "aggregate"; fn: AggregateFn; field?: string | null; group_by?: GroupBy | null; include_unreviewed?: boolean | null }
  | { op: "organize"; template?: string | null; folder_template?: string | null; group_by?: GroupBy | null; only_reviewed?: boolean | null; dry_run?: boolean | null }
  | { op: "rename"; template: string; only_reviewed?: boolean | null }
  | { op: "move"; folder: string; group_by?: GroupBy | null; only_reviewed?: boolean | null }
  | { op: "tag"; add?: string[]; remove?: string[] }
  | { op: "delete"; to_trash?: boolean | null }
  | { op: "export"; format: ExportFormat; path?: string | null; columns?: string[]; layout?: ExportLayout | null; include?: ExportInclude[]; target?: ExportTarget | null }
  | { op: "draft_document"; form_ref?: string | null; format_description?: string | null; content: string; sources?: string[]; output?: OutputFormat | null; tone?: Tone | null; length?: DraftLength | null }
  /** 0.7.0: revise the open draft (`draft_id` null = `UiPlanContext.draft_id`) — a section, a new section after one (`section` null = at the end) or the whole draft. */
  | { op: "revise_draft"; draft_id?: string | null; scope: "section" | "insert_after" | "whole"; section?: number | null; heading?: string | null; instruction: string }
  | { op: "register_form"; doc_ref?: string | null; name?: string | null; doc_type_slug?: string | null; doc_ids?: string[] }
  | { op: "explain"; selection?: string | null; question?: string | null; scope?: ExplainScope | null; allow_general_knowledge?: boolean | null }
  | { op: "compare"; doc_ids: string[]; aspects?: string[] }
  | { op: "create_project"; name: string; template?: string | null; doc_types_required?: string[]; start_date?: string | null; end_date?: string | null }
  | { op: "add_to_project"; project?: string | null; doc_ids?: string[]; as_reference?: boolean | null }
  | { op: "define_rule"; project?: string | null; template?: string | null; description: string }
  | { op: "check_consistency"; project?: string | null; rules?: string[] }
  | { op: "request_supplement"; project?: string | null; findings?: string[]; output?: OutputFormat | null }
  | { op: "set_retention"; item: RetentionTarget; days: number }
  | { op: "set_model"; id: string }
  | { op: "set_profile"; profile: Profile }
  | { op: "set_theme"; theme: Theme }
  | { op: "set_language"; lang: Language }
  | { op: "set_setting"; key: SettingKey; value: string | number | boolean }
  | { op: "open"; target: OpenTarget; ref: string }
  | { op: "job"; action: JobOp; id?: string | null }
  | { op: "engine"; action: EngineOp }
  | { op: "undo"; target?: string | null }
  | { op: "redo" }
  | { op: "help" }
  | { op: "clarify"; question: string }
  | { op: "noop" }
  // 0.6.0 (plan §1.1 documents)
  | { op: "rotate"; degrees: number; pages?: number[] }
  | { op: "rerender"; dpi?: number | null }
  | { op: "split"; boundaries?: number[]; every?: number | null }
  | { op: "merge"; name?: string | null }
  | { op: "set_title"; title: string }
  | { op: "edit_text"; find: string; replace: string; pages?: number[] }
  | { op: "set_role"; role: RoleChoice }
  // §1.2 reference material
  | { op: "manage_reference"; action: ReferenceAction; group?: string | null; name?: string | null; project?: string | null }
  | { op: "define_term"; action: TermAction; term: string; definition?: string | null; synonyms?: string[] }
  | { op: "set_version"; group_key?: string | null; label?: string | null; effective_date?: string | null; latest?: boolean | null; clear?: boolean | null }
  // §1.3 projects
  | { op: "update_project"; project?: string | null; name?: string | null; template?: string | null; start_date?: string | null; end_date?: string | null; owner?: string | null; intake_no?: string | null; notes?: string | null; keywords?: string[]; inbox_folder?: string | null }
  | { op: "delete_project"; project?: string | null }
  | { op: "remove_from_project"; project?: string | null; doc_ids?: string[] }
  | { op: "manage_template"; action: TemplateAction; template?: string | null; name?: string | null; doc_types_required?: string[]; description?: string | null }
  | { op: "classify_projects"; apply?: boolean | null; use_model?: boolean | null }
  // §1.4 forms
  | { op: "manage_schema"; action: SchemaAction; schema: string; name?: string | null }
  // §1.5 system (UI actions)
  | { op: "manage_model"; action: ModelAction; id?: string | null; tier?: ModelTierChoice | null; backend?: BackendPreference | null; path?: string | null }
  | { op: "workspace"; action: WorkspaceAction; path?: string | null }
  | { op: "maintenance"; action: MaintenanceAction };

export interface ScopeFilter {
  doc_type?: string | null;
  date_from?: string | null;
  date_to?: string | null;
  keyword?: string | null;
  vendor?: string | null;
  status?: string | null;
  tags?: string[];
  min_confidence?: number | null;
  review_status?: ReviewStatus | null;
  /** Documents of one project (plan §6 project scope). */
  project_id?: string | null;
}

export interface Scope {
  folder?: string | null;
  doc_ids?: string[];
  filter?: ScopeFilter;
}

export interface Plan {
  intent: OpKind;
  scope: Scope;
  operations: Op[];
  requires_confirmation: boolean;
  confidence: number;
  clarification_question?: string | null;
  /** One-line interpretation ("해석: …"). */
  summary?: string | null;
}

export interface PlanIssue {
  path: string;
  code: string;
  message: string;
  blocking: boolean;
}

export type PlanSource = "prerouter" | "rules" | "model" | "none";

export interface PlanResult {
  text: string;
  source: PlanSource;
  plan: Plan;
  issues: PlanIssue[];
  date: DateRange | null;
  attempts: number;
  model: string | null;
  prompt_tokens: number;
  completion_tokens: number;
  raw?: string | null;
  /** Op families the two-stage planner routed to (0.6.0 plan §2; absent on rule / pre-router plans). */
  families?: OpFamily[] | null;
}

export interface DocTypeSummary {
  slug: string;
  label_ko: string;
  label_en: string;
}

export interface FieldSummary {
  key: string;
  label_ko: string;
  label_en: string;
  kind: string;
}

export interface SchemaSummary {
  id: string;
  name: string;
  name_en: string;
  doc_type: string;
  fields: FieldSummary[];
}

export interface PlanContext {
  today: string;
  language: Language;
  folders: string[];
  doc_types: DocTypeSummary[];
  schemas: SchemaSummary[];
  tags: string[];
  current_folder: string | null;
  selected_doc_ids: string[];
  current_document: string | null;
  current_page: number | null;
  document_count: number;
  model_ids: string[];
  /** Projects of the workspace (§3.15) — `add_to_project` / `check_consistency` targets. */
  projects: ProjectSummary[];
  /** Project open in the UI (default target of project ops). */
  current_project: string | null;
  /** Earlier turns of the assistant conversation (oldest first). */
  history: ConversationTurn[];
  /** Draft open on the 문서 작성 screen (0.7.0; default target of `revise_draft`). */
  draft_id?: string | null;
}

export interface ProjectSummary {
  id: string;
  name: string;
  /** The computed phase (`planned` | `active` | `done`); the field keeps its 0.2 name. */
  status: ProjectPhase;
}

/** How the assistant interprets free text: the model reads every sentence (default) or the legacy phrase rules go first. */
export type PlanMode = "model_first" | "rules_first";

/** What the UI is showing when a command is sent (default scope). */
export interface UiPlanContext {
  folder?: string | null;
  selected_doc_ids?: string[];
  current_document?: string | null;
  current_page?: number | null;
  /** Project open on the projects screen (default target of project ops). */
  current_project?: string | null;
  /** Project the assistant's "현재 프로젝트" scope names (0.5.0): default scope of commands, evidence scope of questions. */
  project_id?: string | null;
  /** Earlier turns of the assistant conversation (oldest first). */
  history?: ConversationTurn[];
  /** Interpretation mode (omitted = model first). */
  routing?: PlanMode | null;
  /** Draft open on the 문서 작성 screen (0.7.0): the default target of `revise_draft`. */
  draft_id?: string | null;
}

export interface StepPreview {
  index: number;
  op: OpKind;
  summary: string;
  payload: unknown;
  confirm: boolean;
}

export interface PlanPreview {
  document_count: number;
  document_ids: string[];
  steps: StepPreview[];
  requires_confirmation: boolean;
}

export interface PlanResponse {
  result: PlanResult;
  preview: PlanPreview | null;
  context: PlanContext;
}

export type StepStatus = "pending" | "running" | "done" | "skipped" | "failed" | "ui" | "cancelled";

export interface StepResult {
  index: number;
  op: OpKind;
  status: StepStatus;
  summary: string;
  payload: unknown;
  error?: string | null;
  elapsed_ms: number;
}

export interface PlanExecution {
  steps: StepResult[];
  cancelled: boolean;
  documents: string[];
  /** Ops the UI performs itself (settings, navigation, job / engine control, help). */
  ui_actions: Op[];
  /** Journal batch ids created (undo entry points). */
  batches: string[];
  restored_paths?: string[];
}

export interface ExecuteResponse {
  /** Set when the plan runs as a background job (its result lands in `Job.result`). */
  job_id: string | null;
  /** Set when the plan ran inline. */
  execution: PlanExecution | null;
}

// -- organise / journal --

export interface OrganizeRequest {
  scope?: Scope;
  /** File-name template (null = keep the name). */
  name_template?: string | null;
  /** Folder template, workspace-relative (null = keep the folder). */
  folder_template?: string | null;
  only_reviewed?: boolean;
  language?: Language;
}

export const TEMPLATE_VARIABLES = ["year", "month", "day", "doc_type", "vendor", "total", "title", "original_name", "name", "tags", "seq", "ext", "project", "project_code", "project_phase", "project_start", "project_end", "project_owner"] as const;
export const DEFAULT_NAME_TEMPLATE = "{{year}}-{{month}}-{{day}}_{{doc_type}}_{{vendor}}_{{total}}";
/** Default project folder (plan §6): `{{project}}` = the document's project. */
export const DEFAULT_PROJECT_FOLDER = "documents/{{project}}/{{doc_type}}";
export const FOLDER_TEMPLATES: Record<GroupBy, string> = {
  month: "documents/{{year}}/{{month}}/{{doc_type}}",
  vendor: "documents/{{vendor}}/{{doc_type}}",
  doc_type: "documents/{{doc_type}}",
  project: DEFAULT_PROJECT_FOLDER,
  none: "documents/{{doc_type}}",
};

export type OrganizeItemStatus = "move" | "unchanged" | "held" | "missing" | "done" | "failed";

export interface OrganizeItem {
  doc_id: string;
  name: string;
  from: string;
  to: string | null;
  status: OrganizeItemStatus;
  /** `review_pending` | `missing_file` | `source_changed` | an error. */
  reason?: string | null;
  /** Template variables that rendered as 미상. */
  missing?: string[];
}

export interface OrganizePlan {
  items: OrganizeItem[];
  moves: number;
  unchanged: number;
  held: number;
  missing_files: number;
  needs_review: number;
  name_template: string | null;
  folder_template: string | null;
  only_reviewed: boolean;
}

export interface ApplyReport {
  batch_id: string | null;
  moved: number;
  skipped: number;
  failed: [string, string][];
  cancelled: boolean;
  items: OrganizeItem[];
  documents: string[];
}

export type JournalEntryKind = "move" | "rename" | "delete";

export interface JournalBatch {
  id: string;
  /** organize | rename | move | delete | undo | redo */
  kind: string;
  label: string | null;
  source: string | null;
  created_at: string;
  entries: number;
  undone_at: string | null;
  undone_by: string | null;
  redo_of: string | null;
}

export interface JournalEntry {
  id: number;
  batch_id: string;
  seq: number;
  kind: JournalEntryKind;
  document_id: string | null;
  from_path: string;
  to_path: string | null;
  trash_path: string | null;
  content_hash: string | null;
  size_bytes: number | null;
  created_at: string;
  undone_at: string | null;
  undo_error: string | null;
}

export interface UndoReport {
  source_batch: string;
  source_kind: string;
  batch_id: string;
  restored: number;
  /** [path, reason] — reason ∈ missing | changed | target_occupied | os_trash. */
  skipped: [string, string][];
  restored_paths: string[];
  documents: string[];
}

export interface UndoResponse {
  job_id: string | null;
  report: UndoReport | null;
}

// -- aggregate --

export interface AggregateRequest {
  func: AggregateFn;
  field?: string | null;
  group_by?: GroupBy;
  include_unreviewed?: boolean;
  scope?: Scope;
}

export interface Totals {
  value: number;
  count: number;
}

export interface AggregateGroup {
  key: string;
  value: number;
  count: number;
  reviewed: Totals;
  unreviewed: Totals;
}

export interface AggregateDocument {
  id: string;
  name: string;
  rel_path: string;
  doc_type: string | null;
  date: string | null;
  basis: string;
  vendor: string | null;
  value: number | null;
  field_key: string | null;
  reviewed: boolean;
}

export interface AggregateResult {
  func: AggregateFn;
  field: string | null;
  field_label: string | null;
  doc_type: string | null;
  /** doc_date | imported_at | mixed */
  basis: string;
  date_from: string | null;
  date_to: string | null;
  include_unreviewed: boolean;
  /** Headline number (reviewed only unless include_unreviewed). */
  value: number;
  count: number;
  reviewed: Totals;
  unreviewed: Totals;
  missing: number;
  groups: AggregateGroup[];
  documents: AggregateDocument[];
}

// -- export --

export interface ExportRequest {
  format: ExportFormat;
  target?: ExportTarget;
  /** Workspace-relative; null → exports/YYYY-MM-DD_<유형>.<ext>. */
  path?: string | null;
  columns?: string[];
  layout?: ExportLayout;
  include?: ExportInclude[];
  scope?: Scope;
  only_reviewed?: boolean;
  overwrite?: boolean;
  language?: Language;
}

export interface ExportPreview {
  path: string;
  exists: boolean;
  documents: number;
  skipped: number;
  columns: string[];
  sheets: string[];
}

export interface ExportResult {
  path: string;
  absolute: string;
  format: ExportFormat;
  target: ExportTarget;
  documents: number;
  rows: number;
  sheets: string[];
  overwritten: boolean;
  skipped: [string, string][];
  extra_files: string[];
}

// ---------------------------------------------------------------------------
// Document generation (P6) — mirrors crates/doculens-core/src/generate/mod.rs
// ---------------------------------------------------------------------------

export type OutputFormat = "hwpx" | "docx" | "pdf" | "md";
export const OUTPUT_FORMATS: OutputFormat[] = ["hwpx", "docx", "pdf", "md"];
export type Tone = "formal" | "plain" | "concise";
export const TONES: Tone[] = ["formal", "plain", "concise"];
/** `generate::Length` (named apart from the summarise length). */
export type DraftLength = "short" | "medium" | "long";
export const DRAFT_LENGTHS: DraftLength[] = ["short", "medium", "long"];
export type TemplateKind =
  | "rfp"
  | "proposal"
  | "sow"
  | "srs"
  | "dev_contract"
  | "wbs"
  | "progress_report"
  | "meeting_minutes"
  | "test_report"
  | "change_request"
  | "acceptance"
  | "estimate"
  | "supplement_request"
  | "official_letter"
  | "quotation"
  | "transaction_statement"
  | "contract"
  | "report"
  | "minutes";
/** Templates offered in the picker (UI order, 0.3.0: the software-development set + 보완요청서). */
export const TEMPLATE_KINDS: TemplateKind[] = ["rfp", "proposal", "sow", "srs", "dev_contract", "wbs", "progress_report", "meeting_minutes", "test_report", "change_request", "acceptance", "estimate", "supplement_request"];
/** The generic templates of 0.1: hidden from the picker, but drafts that use them still open. */
export const HIDDEN_TEMPLATE_KINDS: TemplateKind[] = ["official_letter", "quotation", "transaction_statement", "contract", "report", "minutes"];

export type TemplateRef = { kind: "builtin"; template: TemplateKind } | { kind: "form"; schema_id: string; doc_id?: string | null };

export type SourceRef =
  | { kind: "document"; doc_id: string }
  | { kind: "aggregate"; result: unknown }
  | { kind: "knowledge"; query: string }
  | { kind: "text"; title?: string | null; text: string };

export interface DraftRequest {
  form_ref: string | null;
  format_description: string | null;
  content: string;
  sources: SourceRef[];
  output: OutputFormat;
  tone: Tone;
  length: DraftLength;
  language: Language;
  use_knowledge: boolean;
  feedback: string | null;
  previous: Draft | null;
  excluded_sources: string[];
  /** Reply token budget (0.3.0): absent / null → the length preset; above ~3,000 the draft is produced section by section. */
  max_tokens?: number | null;
  /**
   * 0.7.0 초안 어시스턴트: revise `previous` instead of regenerating it — one section, a new section after one, or the
   * whole draft — following `feedback` (the instruction). Absent / null = the ordinary draft / regeneration.
   */
  revise?: ReviseScope | null;
}

/**
 * Scope of a draft revision (plan 0.7.0 §1.5). Sections are the runs of `body` split at heading blocks of level ≤ 2
 * (`sectionsOf`); ordinals are 1-based; a leading run without such a heading is section 1; a body without headings is
 * one section.
 */
export type ReviseScope = { scope: "section"; section: number } | { scope: "insert_after"; section: number | null; heading?: string | null } | { scope: "whole" };

/** Token budget presets of the 문서 생성 screen (the last one is a whole RFP / SOW). */
export const TOKEN_BUDGET_PRESETS = [2600, 6000, 12000, 24000] as const;
/** Reply tokens of the length presets (`generate::prompts::max_tokens`). */
export const LENGTH_TOKENS: Record<DraftLength, number> = { short: 1200, medium: 1800, long: 2600 };

/** `generate_budget_info`: what the screen needs to size a token budget. */
export interface BudgetInfo {
  ctx: number | null;
  gpu: boolean;
  cap: number;
  split_threshold: number;
}

export function defaultDraftRequest(language: Language = "ko"): DraftRequest {
  return { form_ref: null, format_description: null, content: "", sources: [], output: "hwpx", tone: "formal", length: "medium", language, use_knowledge: true, feedback: null, previous: null, excluded_sources: [], max_tokens: null, revise: null };
}

export interface DraftCitation {
  doc_id: string | null;
  title: string;
  page: number | null;
  chunk_id: number | null;
  /** document | knowledge | aggregate | text */
  source: string;
  excerpt: string | null;
}

export interface DraftField {
  key: string;
  label: string;
  /** `FieldKind` as a string (`string`, `amount`, `date`, `brn`, …). */
  kind: string;
  required: boolean;
  value: string;
  normalized: string | null;
  source: DraftCitation | null;
  user_edited: boolean;
}

export interface DraftColumn {
  key: string;
  label: string;
  kind: string;
}

export interface DraftTable {
  key: string;
  label: string;
  columns: DraftColumn[];
  rows: string[][];
}

export type DraftBlock =
  | { kind: "heading"; level: number; text: string }
  | { kind: "paragraph"; text: string; source?: DraftCitation | null }
  | { kind: "list"; items: string[]; ordered: boolean }
  | { kind: "table_ref"; key: string };

export interface DraftIssue {
  /** `fields.total`, `tables.items[2].amount`, `body[3]` … */
  path: string;
  /** `required_missing`, `sum_mismatch`, `brn_invalid`, `date_invalid`, `amount_invalid`, `uncited_claim`, `template_unknown_field` … */
  code: string;
  message: string;
  blocking: boolean;
  suggestion?: string | null;
}

export interface Draft {
  id: string;
  title: string;
  template: TemplateRef;
  doc_type: string | null;
  fields: DraftField[];
  tables: DraftTable[];
  body: DraftBlock[];
  references: DraftCitation[];
  issues: DraftIssue[];
  language: Language;
  tone: Tone;
  length: DraftLength;
  created_at: string;
  updated_at: string;
  model: string | null;
  request: DraftRequest | null;
  notes: string[];
  saved_path: string | null;
  saved_doc_id: string | null;
}

export interface DraftOutcome {
  draft: Draft;
  prompt_tokens: number;
  completion_tokens: number;
  elapsed_ms: number;
  /** How the draft was produced: `rules` (no model call) | `model`. */
  source: string;
  /** Model calls that produced body sections (1 = one shot, >1 = outline → sections). */
  sections?: number;
  /** Draft job payloads only (`generate_bridge::run_draft`): where the job came from (`generate`, `project`, …). */
  job_source?: string;
  /** Draft job / `request_supplement` payloads: the project the draft belongs to. */
  project_id?: string | null;
}

export interface SaveRequest {
  draft: Draft;
  output: OutputFormat;
  file_name: string | null;
  folder: string | null;
  register: boolean;
  overwrite: boolean;
}

export interface SavedDocument {
  rel_path: string;
  absolute: string;
  format: OutputFormat;
  doc_id: string | null;
  filled_form: boolean;
  warnings: string[];
}

export interface TemplateField {
  key: string;
  label_ko: string;
  label_en: string;
  kind: string;
  required: boolean;
  columns?: DraftColumn[];
}

export interface TemplateInfo {
  kind: TemplateKind;
  name_ko: string;
  name_en: string;
  description_ko: string;
  description_en: string;
  fields: TemplateField[];
  sections_ko: string[];
  sections_en: string[];
  keywords: string[];
}

export interface DraftSummary {
  id: string;
  title: string;
  template: TemplateRef;
  updated_at: string;
  issues: number;
  saved_path: string | null;
}

export function isDraft(v: unknown): v is Draft {
  return typeof v === "object" && v !== null && Array.isArray((v as Draft).fields) && Array.isArray((v as Draft).body) && Array.isArray((v as Draft).issues) && typeof (v as Draft).title === "string" && typeof (v as Draft).template === "object";
}

export function isDraftOutcome(v: unknown): v is DraftOutcome {
  return typeof v === "object" && v !== null && isDraft((v as DraftOutcome).draft) && typeof (v as DraftOutcome).source === "string";
}

// ---------------------------------------------------------------------------
// Projects (P6) — mirrors crates/doculens-core/src/projects/mod.rs
// ---------------------------------------------------------------------------

/** Phase computed from the project's dates (0.3.0 plan §5): before the start, between, after the end. */
export type ProjectPhase = "planned" | "active" | "done";
export const PROJECT_PHASES: ProjectPhase[] = ["planned", "active", "done"];

/** Per-project organise / export settings (plan §6); every field optional, `copies` on by default. */
export interface ProjectSettings {
  folder_template?: string | null;
  name_template?: string | null;
  group_by?: GroupBy | null;
  export_format?: ExportFormat | null;
  export_layout?: ExportLayout | null;
  export_path?: string | null;
  copies: boolean;
}

export const DEFAULT_PROJECT_SETTINGS: ProjectSettings = { folder_template: null, name_template: null, group_by: null, export_format: null, export_layout: null, export_path: null, copies: true };

export type CopyState = "synced" | "modified" | "missing";

/** A tracked physical copy of a document in another project's folder (decision 6). */
export interface DocumentCopy {
  document_id: string;
  project_id: string;
  rel_path: string;
  content_hash: string;
  size_bytes: number;
  state: CopyState;
  created_at: string;
  synced_at: string;
}

/** Result of "다시 동기화" (`project_sync_copies`). */
export interface ProjectCopySyncReport {
  renamed: number;
  rewritten: number;
  created: number;
  modified: number;
  missing: number;
  removed: number;
}

export interface Project {
  id: string;
  name: string;
  template_id: string | null;
  intake_no: string | null;
  owner: string | null;
  /** ISO dates; null start = the creation day, null end = open-ended. */
  start_date: string | null;
  end_date: string | null;
  /** Computed from the dates for today. */
  phase: ProjectPhase;
  /** 기한 경과: the end date has passed but the case is incomplete (badge, not a phase). */
  overdue: boolean;
  settings: ProjectSettings;
  notes: string | null;
  inbox_folder: string | null;
  /** Classification keywords (자동 분류), matched against document titles / names / text. */
  keywords: string[];
  created_at: string;
  updated_at: string;
  document_count: number;
  missing_count: number;
  last_check_at: string | null;
  last_pass: number;
  last_mismatch: number;
  last_undetermined: number;
}

/** Project create / update payload. On update an optional field that is omitted or `null` keeps its stored value and a blank string clears it. */
export interface ProjectInput {
  id?: string | null;
  name: string;
  template_id?: string | null;
  intake_no?: string | null;
  owner?: string | null;
  /** ISO dates; on update null keeps the stored value and "" clears it. */
  start_date?: string | null;
  end_date?: string | null;
  notes?: string | null;
  inbox_folder?: string | null;
  /** Omitted / null keeps the stored settings. */
  settings?: ProjectSettings | null;
  /** Omitted / null keeps the stored keywords, an empty list clears them. */
  keywords?: string[] | null;
}

// -- 자동 분류 (projects::classify) --
export interface ClassifyRequest {
  /** Documents to classify; empty = every document that belongs to no project yet. */
  doc_ids?: string[];
  project_ids?: string[];
  use_model?: boolean;
  include_assigned?: boolean;
}
export type SuggestionSource = "none" | "rules" | "model";
export interface ClassifyCandidate {
  project_id: string;
  project_name: string;
  score: number;
  reasons: string[];
}
export interface ClassifySuggestion {
  doc_id: string;
  title: string;
  doc_type: string | null;
  project_id: string | null;
  project_name: string | null;
  confidence: number;
  reasons: string[];
  source: SuggestionSource;
  alternatives: ClassifyCandidate[];
  already_in: string[];
}
export interface ClassifyResult {
  suggestions: ClassifySuggestion[];
  suggested: number;
  undecided: number;
  model_calls: number;
  notes: string[];
}
export interface ClassifyAssignment {
  doc_id: string;
  project_id: string;
}
export interface ClassifyApplyResult {
  added: number;
  project_ids: string[];
}

export type RuleOp = "eq" | "neq" | "gte" | "lte" | "within_pct" | "present" | "matches";
export const RULE_OPS: RuleOp[] = ["eq", "neq", "gte", "lte", "within_pct", "present", "matches"];

export interface RuleOperand {
  doc_type: string | null;
  field: string | null;
  value: string | null;
}

export interface Rule {
  id: string;
  description: string;
  left: RuleOperand;
  op: RuleOp;
  right: RuleOperand | null;
  tolerance: number | null;
  required: boolean;
  enabled: boolean;
}

export interface RuleDraft {
  rule: Rule;
  confidence: number;
  /** `rules` | `model` */
  source: string;
  issues: string[];
  unresolved: string[];
}

export interface ProjectTemplate {
  id: string;
  name_ko: string;
  name_en: string;
  description_ko: string;
  description_en: string;
  required_doc_types: string[];
  rules: Rule[];
  builtin: boolean;
  created_at: string;
  updated_at: string;
}

export interface TemplateInput {
  id?: string | null;
  name_ko: string;
  name_en?: string | null;
  description_ko?: string | null;
  description_en?: string | null;
  required_doc_types: string[];
  rules: Rule[];
}

export type Verdict = "pass" | "mismatch" | "undetermined" | "missing";
export const VERDICTS: Verdict[] = ["pass", "mismatch", "undetermined", "missing"];

export interface Evidence {
  doc_id: string | null;
  doc_name: string | null;
  doc_type: string | null;
  field: string | null;
  label: string | null;
  value: string | null;
  normalized: string | null;
  page: number | null;
  status: string | null;
}

export interface RuleResult {
  id: string;
  run_id: string;
  rule_id: string;
  description: string;
  verdict: Verdict;
  required: boolean;
  left: Evidence | null;
  right: Evidence | null;
  message: string;
  checked_at: string;
}

export interface CheckRun {
  id: string;
  project_id: string;
  started_at: string;
  finished_at: string;
  pass: number;
  mismatch: number;
  undetermined: number;
  missing: number;
  results: RuleResult[];
  missing_doc_types: string[];
}

export interface ChecklistItem {
  doc_type: string;
  label_ko: string;
  label_en: string;
  required: boolean;
  doc_ids: string[];
  missing: boolean;
}

export interface RelationNode {
  doc_id: string;
  label: string;
  doc_type: string | null;
}

export interface RelationEdge {
  from: string;
  to: string;
  /** brn | contract_no | amount | date | name | account */
  kind: string;
  value: string;
  from_field: string | null;
  to_field: string | null;
}

export interface RelationGraph {
  nodes: RelationNode[];
  edges: RelationEdge[];
}

export interface ProjectDocument {
  document: DocumentRecord;
  added_at: string;
  /** The tracked copy in this project's folder when the document also belongs to an earlier project. */
  copy?: DocumentCopy | null;
}

export interface ProjectDetail {
  project: Project;
  template: ProjectTemplate | null;
  documents: ProjectDocument[];
  checklist: ChecklistItem[];
  rules: Rule[];
  last_run: CheckRun | null;
  relations: RelationGraph;
}

export interface ProjectsChanged {
  project_ids: string[];
}

export function isProject(v: unknown): v is Project {
  return typeof v === "object" && v !== null && typeof (v as Project).id === "string" && typeof (v as Project).name === "string" && typeof (v as Project).phase === "string" && typeof (v as Project).document_count === "number";
}

export function isProjectDetail(v: unknown): v is ProjectDetail {
  return typeof v === "object" && v !== null && isProject((v as ProjectDetail).project) && Array.isArray((v as ProjectDetail).documents) && Array.isArray((v as ProjectDetail).checklist);
}

export function isRuleDraft(v: unknown): v is RuleDraft {
  return typeof v === "object" && v !== null && typeof (v as RuleDraft).rule === "object" && (v as RuleDraft).rule !== null && typeof (v as RuleDraft).rule.op === "string" && Array.isArray((v as RuleDraft).issues);
}

export function isCheckRun(v: unknown): v is CheckRun {
  return typeof v === "object" && v !== null && typeof (v as CheckRun).project_id === "string" && Array.isArray((v as CheckRun).results) && typeof (v as CheckRun).pass === "number";
}

// ---------------------------------------------------------------------------
// Maintenance (P7 retention job) — `maintenance_status` / `maintenance_run_now`
// ---------------------------------------------------------------------------

export interface MaintenancePurged {
  journal: number;
  jobs: number;
  answers: number;
  cache_bytes: number;
  trash: number;
  logs: number;
  temp: number;
  documents: number;
  extractions: number;
}

export interface MaintenanceSummary {
  started_at: string;
  elapsed_ms: number;
  purged: MaintenancePurged;
  /** Steps that were skipped: a policy set to "keep forever" (`journal`, `jobs`, …), `disabled`, or `<step>: <error>`. */
  skipped: string[];
  cancelled: boolean;
}

export interface MaintenanceStatus {
  /** `RetentionPolicy.purge_enabled` of the open workspace. */
  enabled: boolean;
  purge_hour_local: number;
  last_run_at: string | null;
  next_run_at: string | null;
  running: boolean;
  last_summary: MaintenanceSummary | null;
}

export function isMaintenanceSummary(v: unknown): v is MaintenanceSummary {
  return typeof v === "object" && v !== null && typeof (v as MaintenanceSummary).started_at === "string" && typeof (v as MaintenanceSummary).purged === "object" && (v as MaintenanceSummary).purged !== null;
}
