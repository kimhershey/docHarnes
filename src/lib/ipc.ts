// Typed IPC layer. In the Tauri webview this calls Rust commands; in a plain
// browser (vite dev/preview, Playwright) it falls back to the in-memory mock
// backend so the whole UI can be exercised without the desktop runtime.

import type {
  CheckRun,
  Draft,
  DraftRequest,
  DraftSummary,
  MaintenanceStatus,
  OutputFormat,
  ClassifyApplyResult,
  ClassifyAssignment,
  ClassifyRequest,
  ClassifyResult,
  Project,
  ProjectDetail,
  ProjectInput,
  ProjectCopySyncReport,
  ProjectTemplate,
  Rule,
  RuleDraft,
  SaveRequest,
  SavedDocument,
  BudgetInfo,
  TemplateInfo,
  TemplateInput,
  AggregateRequest,
  AggregateResult,
  ExecuteArgs,
  ExecuteResponse,
  ExportPreview,
  ExportRequest,
  JournalBatch,
  JournalEntry,
  OrganizePlan,
  OrganizeRequest,
  Plan,
  PlanResponse,
  UiPlanContext,
  UndoResponse,
  Answer,
  AnswerKind,
  AnswerRequest,
  AppInfo,
  ChatMessageRow,
  ChatSessionSummary,
  Chunk,
  Collection,
  CollectionInput,
  GlossaryEntry,
  GlossaryInput,
  KnowledgeStatus,
  SavedAnswer,
  SearchRequest,
  SearchResponse,
  VersionGroup,
  VersionInfo,
  VersionInput,
  AppSettings,
  Assessment,
  DocTypeInfo,
  ExtractedTable,
  ExtractionRun,
  FieldChange,
  FieldRecord,
  LintIssue,
  PipelineStatus,
  ProcessRequest,
  ReviewAction,
  ReviewFilter,
  ReviewItem,
  ReviewReason,
  SchemaDef,
  SchemaDraft,
  SchemaRecord,
  SchemaVersionView,
  ChatTestResult,
  DateRange,
  DocumentFilter,
  DocumentImportPlan,
  DocumentRole,
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
  PageLayout,
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
  WorkspaceView, PageTextHistory, PageTextRevert
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
  /** Manual correction of a page's text (viewer text tab); the knowledge index follows. */
  documentsSetPageText: (id: string, pageNo: number, text: string) => invoke<boolean>("documents_set_page_text", { id, pageNo, text }),
  documentsPageLayout: (id: string, pageNo: number) => invoke<PageLayout | null>("documents_page_layout", { id, pageNo }),
  documentsEditPageWords: (id: string, pageNo: number, start: number, end: number, text: string) =>
    invoke<PageLayout>("documents_edit_page_words", { id, pageNo, start, end, text }),
  documentsAddPageWords: (id: string, pageNo: number, bbox: [number, number, number, number], text: string) =>
    invoke<PageLayout>("documents_add_page_words", { id, pageNo, bbox, text }),
  documentsPageTextHistory: (id: string, pageNo: number | null) => invoke<PageTextHistory>("documents_page_text_history", { id, pageNo }),
  documentsUndoPageText: (id: string, pageNo: number) => invoke<PageTextRevert | null>("documents_undo_page_text", { id, pageNo }),
  documentsRedoPageText: (id: string, pageNo: number) => invoke<PageTextRevert | null>("documents_redo_page_text", { id, pageNo }),
  documentsRevertPageText: (id: string, historyId: number) => invoke<PageTextRevert>("documents_revert_page_text", { id, historyId }),
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

  // -- knowledge base (P3) --
  knowledgeStatus: () => invoke<KnowledgeStatus>("knowledge_status"),
  knowledgeCollections: () => invoke<Collection[]>("knowledge_collections"),
  knowledgeCollectionSave: (input: CollectionInput) => invoke<Collection>("knowledge_collection_save", { input }),
  knowledgeCollectionDelete: (id: string) => invoke<boolean>("knowledge_collection_delete", { id }),
  knowledgeCollectionDocs: (id: string) => invoke<DocumentView[]>("knowledge_collection_docs", { id }),
  knowledgeCollectionAdd: (id: string, docIds: string[], setRole = true) =>
    invoke<number>("knowledge_collection_add", { args: { id, doc_ids: docIds, set_role: setRole } }),
  knowledgeCollectionRemove: (id: string, docIds: string[]) =>
    invoke<number>("knowledge_collection_remove", { args: { id, doc_ids: docIds, set_role: false } }),
  knowledgeDocumentCollections: (id: string) => invoke<[string, string][]>("knowledge_document_collections", { id }),
  knowledgeSetRole: (ids: string[], role: DocumentRole) => invoke<number>("knowledge_set_role", { ids, role }),
  knowledgeIndex: (ids: string[] | null = null, force = false) => invoke<string | null>("knowledge_index", { ids, force }),
  knowledgeEmbed: (loadModel = true) => invoke<string | null>("knowledge_embed", { loadModel }),
  knowledgeResetEmbeddings: () => invoke<void>("knowledge_reset_embeddings"),
  knowledgeSearch: (req: SearchRequest) => invoke<SearchResponse>("knowledge_search", { req }),
  knowledgeChunks: (documentId: string) => invoke<Chunk[]>("knowledge_chunks", { documentId }),
  knowledgeAnswer: (req: AnswerRequest, requestId: string, ui?: UiPlanContext) => invoke<Answer>("knowledge_answer", { req, requestId, ui }),
  knowledgeAnswerCancel: (requestId: string) => invoke<boolean>("knowledge_answer_cancel", { requestId }),
  knowledgeAnswers: (kind: AnswerKind | null = null, documentId: string | null = null, limit = 50) =>
    invoke<SavedAnswer[]>("knowledge_answers", { kind, documentId, limit }),
  knowledgeAnswerFeedback: (id: number, feedback: number | null, note: string | null = null) =>
    invoke<boolean>("knowledge_answer_feedback", { id, feedback, note }),
  knowledgeAnswerDelete: (id: number) => invoke<boolean>("knowledge_answer_delete", { id }),
  knowledgeGlossary: () => invoke<GlossaryEntry[]>("knowledge_glossary"),
  knowledgeGlossarySave: (input: GlossaryInput) => invoke<GlossaryEntry>("knowledge_glossary_save", { input }),
  knowledgeGlossaryDelete: (id: number) => invoke<boolean>("knowledge_glossary_delete", { id }),
  knowledgeVersions: () => invoke<VersionGroup[]>("knowledge_versions"),
  knowledgeVersionSet: (input: VersionInput) => invoke<VersionInfo>("knowledge_version_set", { input }),
  knowledgeVersionClear: (documentId: string) => invoke<boolean>("knowledge_version_clear", { documentId }),

  // -- assistant chat sessions (0.7.0 §1.6; messages are stored as the UI's JSON) --
  chatSessionsList: () => invoke<ChatSessionSummary[]>("chat_sessions_list"),
  chatSessionCreate: (title: string | null = null) => invoke<ChatSessionSummary>("chat_session_create", { title }),
  chatSessionRename: (id: string, title: string) => invoke<void>("chat_session_rename", { id, title }),
  chatSessionDelete: (id: string) => invoke<void>("chat_session_delete", { id }),
  chatMessagesList: (sessionId: string) => invoke<ChatMessageRow[]>("chat_messages_list", { sessionId }),
  chatMessagePut: (sessionId: string, seq: number, payload: unknown) => invoke<void>("chat_message_put", { sessionId, seq, payload }),

  // -- pipeline (P4) --
  pipelineStatus: () => invoke<PipelineStatus>("pipeline_status"),
  pipelineProcess: (req: ProcessRequest) => invoke<string | null>("pipeline_process", { req }),
  pipelineDocTypes: () => invoke<DocTypeInfo[]>("pipeline_doc_types"),
  pipelineSchemas: () => invoke<SchemaRecord[]>("pipeline_schemas"),
  pipelineSchemaGet: (id: string) => invoke<SchemaRecord>("pipeline_schema_get", { id }),
  pipelineSchemaVersions: (id: string) => invoke<SchemaVersionView[]>("pipeline_schema_versions", { id }),
  pipelineSchemaSave: (draft: SchemaDraft) => invoke<SchemaRecord>("pipeline_schema_save", { draft }),
  pipelineSchemaDelete: (id: string) => invoke<boolean>("pipeline_schema_delete", { id }),
  pipelineSchemaSetDefault: (id: string) => invoke<void>("pipeline_schema_set_default", { id }),
  pipelineSchemaSetEnabled: (id: string, enabled: boolean) => invoke<void>("pipeline_schema_set_enabled", { id, enabled }),
  pipelineSchemaLint: (definition: SchemaDef) => invoke<LintIssue[]>("pipeline_schema_lint", { definition }),
  pipelineSchemaFromNl: (description: string, baseId: string | null = null) => invoke<SchemaDraft>("pipeline_schema_from_nl", { description, baseId }),
  pipelineFormDerive: (documentId: string, name: string | null = null) => invoke<SchemaDraft>("pipeline_form_derive", { documentId, name }),
  pipelineFormRegister: (draft: SchemaDraft) => invoke<SchemaRecord>("pipeline_form_register", { draft }),
  /** 0.3.0 §2: field discovery over 1..N sample documents as a job (its result is a `SchemaDiscoveryOutcome`). */
  pipelineSchemaDiscover: (documentIds: string[], name: string | null = null) => invoke<string>("pipeline_schema_discover", { documentIds, name }),
  pipelineSchemaNameExists: (name: string, excludingId: string | null = null) => invoke<boolean>("pipeline_schema_name_exists", { name, excludingId }),
  pipelineDocumentSchema: (documentId: string, schemaId: string | null) => invoke<void>("pipeline_document_schema", { documentId, schemaId }),
  pipelineRuns: (documentId: string) => invoke<ExtractionRun[]>("pipeline_runs", { documentId }),
  pipelineFields: (documentId: string, runId: string | null = null) => invoke<FieldRecord[]>("pipeline_fields", { documentId, runId }),
  pipelineFieldHistory: (documentId: string) => invoke<FieldChange[]>("pipeline_field_history", { documentId }),
  pipelineRestore: (changeId: number) => invoke<FieldRecord>("pipeline_restore", { changeId }),
  pipelineRevalidate: (documentId: string) => invoke<FieldRecord[]>("pipeline_revalidate", { documentId }),
  pipelineReviewQueue: (filter: ReviewFilter = {}) => invoke<ReviewItem[]>("pipeline_review_queue", { filter }),
  pipelineReviewApply: (fieldIds: number[], action: ReviewAction, note: string | null = null) =>
    invoke<FieldRecord[]>("pipeline_review_apply", { args: { field_ids: fieldIds, action, note } }),
  pipelineReviewApproveDocument: (documentId: string, reason: ReviewReason | null = null) =>
    invoke<number>("pipeline_review_approve_document", { documentId, reason }),
  pipelineTables: (documentId: string) => invoke<ExtractedTable[]>("pipeline_tables", { documentId }),
  pipelineTableCsv: (documentId: string, page: number, index: number) => invoke<string>("pipeline_table_csv", { documentId, page, index }),
  pipelineTableExport: (documentId: string, page: number, index: number, path: string) => invoke<void>("pipeline_table_export", { documentId, page, index, path }),
  pipelineSplit: (documentId: string, boundaries: number[]) => invoke<string>("pipeline_split", { documentId, boundaries }),
  pipelineMerge: (ids: string[], name: string | null = null) => invoke<string>("pipeline_merge", { ids, name }),

  // -- planner / organise / journal / aggregate / export / job control (P5) --
  plannerPlan: (text: string, context: UiPlanContext = {}) => invoke<PlanResponse>("planner_plan", { text, context }),
  /** `plannedText`: the 되묻기 text the plan was made from, when it is not `text` (`ExecuteArgs.planned_text`). */
  plannerExecute: (plan: Plan, text: string, context: UiPlanContext = {}, plannedText: string | null = null) =>
    invoke<ExecuteResponse>("planner_execute", { args: { plan, text, context, ...(plannedText ? { planned_text: plannedText } : {}) } satisfies ExecuteArgs }),
  organizePlan: (req: OrganizeRequest) => invoke<OrganizePlan>("organize_plan", { req }),
  organizeApply: (plan: OrganizePlan, label: string | null = null) => invoke<string>("organize_apply", { plan, label }),
  journalBatches: (limit = 50) => invoke<JournalBatch[]>("journal_batches", { limit }),
  journalEntries: (batchId: string) => invoke<JournalEntry[]>("journal_entries", { batchId }),
  journalUndo: (batchId: string | null = null) => invoke<UndoResponse>("journal_undo", { batchId }),
  journalRedo: () => invoke<UndoResponse>("journal_redo"),
  aggregateRun: (req: AggregateRequest) => invoke<AggregateResult>("aggregate_run", { req }),
  exportPreview: (req: ExportRequest) => invoke<ExportPreview>("export_preview", { req }),
  exportRun: (req: ExportRequest) => invoke<string>("export_run", { req }),
  jobsGet: (jobId: string) => invoke<Job | null>("jobs_get", { jobId }),
  jobsPause: (jobId: string) => invoke<boolean>("jobs_pause", { jobId }),
  jobsResume: (jobId: string) => invoke<boolean>("jobs_resume", { jobId }),
  jobsRetry: (jobId: string) => invoke<boolean>("jobs_retry", { jobId }),
  jobsPrioritize: (jobId: string) => invoke<boolean>("jobs_prioritize", { jobId }),

  // -- document generation (P6, §3.13) --
  generateTemplates: () => invoke<TemplateInfo[]>("generate_templates"),
  generateBudgetInfo: () => invoke<BudgetInfo>("generate_budget_info"),
  /** Starts the `draft` job; the finished job's `result` is a `DraftOutcome`. */
  generateDraft: (req: DraftRequest) => invoke<{ job_id: string }>("generate_draft", { req }),
  generatePreview: (draft: Draft) => invoke<{ html: string; markdown: string }>("generate_preview", { draft }),
  generateValidate: (draft: Draft) => invoke<Draft>("generate_validate", { draft }),
  generateSave: (req: SaveRequest) => invoke<SavedDocument>("generate_save", { req }),
  generateDrafts: () => invoke<DraftSummary[]>("generate_drafts"),
  generateDraftGet: (id: string) => invoke<Draft>("generate_draft_get", { id }),
  generateDraftPut: (draft: Draft) => invoke<DraftSummary>("generate_draft_put", { draft }),
  generateDraftDelete: (id: string) => invoke<boolean>("generate_draft_delete", { id }),

  // -- projects (P6, §3.15) --
  projectsList: () => invoke<Project[]>("projects_list"),
  projectCreate: (input: ProjectInput) => invoke<Project>("project_create", { input }),
  projectUpdate: (input: ProjectInput) => invoke<Project>("project_update", { input }),
  projectDelete: (id: string) => invoke<boolean>("project_delete", { id }),
  projectGet: (id: string) => invoke<ProjectDetail>("project_get", { id }),
  /** `asReference` (0.5.0 plan §2): into the project's reference group (role 참고 자료) instead of its 업무 문서. */
  projectAddDocuments: (id: string, docIds: string[], asReference = false) => invoke<ProjectDetail>("project_add_documents", { id, docIds, asReference }),
  projectRemoveDocument: (id: string, docId: string) => invoke<ProjectDetail>("project_remove_document", { id, docId }),
  projectSyncCopies: (id: string) => invoke<ProjectCopySyncReport>("project_sync_copies", { id }),
  projectTemplates: () => invoke<ProjectTemplate[]>("project_templates"),
  projectTemplateSave: (input: TemplateInput) => invoke<ProjectTemplate>("project_template_save", { input }),
  projectTemplateDelete: (id: string) => invoke<boolean>("project_template_delete", { id }),
  projectDefineRule: (id: string, description: string) => invoke<RuleDraft>("project_define_rule", { id, description }),
  projectSaveRule: (id: string, rule: Rule) => invoke<Rule>("project_save_rule", { id, rule }),
  projectDeleteRule: (id: string, ruleId: string) => invoke<boolean>("project_delete_rule", { id, ruleId }),
  projectCheck: (id: string, ruleIds: string[] | null = null) => invoke<CheckRun>("project_check", { id, ruleIds }),
  projectRuns: (id: string) => invoke<CheckRun[]>("project_runs", { id }),
  projectReport: (id: string) => invoke<{ markdown: string }>("project_report", { id }),
  /** Starts the `draft` job for the 보완요청서; the finished job's `result` is a `DraftOutcome`. */
  projectRequestSupplement: (id: string, findingIds: string[] | null = null, output: OutputFormat | null = null) =>
    invoke<{ job_id: string }>("project_request_supplement", { id, findingIds, output }),
  projectClassifyPreview: (req: ClassifyRequest) => invoke<ClassifyResult>("project_classify_preview", { req }),
  projectClassifyApply: (assignments: ClassifyAssignment[]) => invoke<ClassifyApplyResult>("project_classify_apply", { assignments }),
  /** The project's reference group (0.5.0 plan §2), created on first use. */
  projectReferenceGroup: (id: string) => invoke<Collection>("project_reference_group", { id }),

  // -- maintenance (P7 retention job) --
  maintenanceStatus: () => invoke<MaintenanceStatus>("maintenance_status"),
  maintenanceRunNow: () => invoke<{ job_id: string }>("maintenance_run_now"),
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
    filters: [{ name: filterName, extensions: ["pdf", "png", "jpg", "jpeg", "tif", "tiff", "webp", "bmp", "gif", "hwpx", "docx", "hwp", "xlsx", "xlsm", "pptx", "ppsx", "txt", "md", "csv", "tsv", "log", "json", "xml", "html", "htm", "xls", "xlsb", "ods", "doc", "ppt", "pps", "rtf", "hml"] }],
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
