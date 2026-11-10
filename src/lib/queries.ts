import { keepPreviousData, useQuery } from "@tanstack/react-query";

import { api } from "./ipc";
import { queryClient, queryKeys } from "./query-client";
import type { AnswerKind, DocumentFilter, ReviewFilter, SearchRequest } from "./types";

export function useAppInfo() {
  return useQuery({ queryKey: queryKeys.appInfo, queryFn: () => api.appInfo(), staleTime: Infinity });
}

export function useHardware() {
  return useQuery({ queryKey: queryKeys.hardware, queryFn: () => api.hwProbe(false), staleTime: 60_000 });
}

export function useAssessment() {
  return useQuery({ queryKey: queryKeys.assessment, queryFn: () => api.hwAssess() });
}

export function useRegistry() {
  return useQuery({ queryKey: queryKeys.registry, queryFn: () => api.registryList() });
}

export function useModelIndex() {
  return useQuery({ queryKey: queryKeys.modelIndex, queryFn: () => api.modelsList() });
}

export function useEngineBinaryInfo(enabled = true) {
  return useQuery({ queryKey: queryKeys.engineBinary, queryFn: () => api.engineBinaryInfo(), enabled, staleTime: 60_000 });
}

export function useDocuments(filter: DocumentFilter, enabled = true) {
  return useQuery({
    queryKey: queryKeys.documentsList(filter),
    queryFn: () => api.documentsList(filter),
    enabled,
    staleTime: 5_000,
    // Keep the previous list on filter / search changes so the grid never unmounts into a spinner.
    placeholderData: keepPreviousData,
  });
}

export function useDocumentStats(enabled = true) {
  return useQuery({ queryKey: queryKeys.documentsStats, queryFn: () => api.documentsStats(), enabled, staleTime: 5_000 });
}

export function useDocument(id: string | undefined) {
  return useQuery({ queryKey: queryKeys.document(id ?? ""), queryFn: () => api.documentsGet(id ?? ""), enabled: !!id });
}

export function useDocumentPages(id: string | undefined, withText = false) {
  return useQuery({
    queryKey: queryKeys.documentPages(id ?? "", withText),
    queryFn: () => api.documentsPages(id ?? "", withText),
    enabled: !!id,
  });
}

export function usePdfiumInfo(enabled = true) {
  return useQuery({ queryKey: queryKeys.pdfium, queryFn: () => api.documentsPdfiumInfo(), enabled, staleTime: Infinity });
}

// -- knowledge base (P3) --

export function useKnowledgeStatus(enabled = true) {
  return useQuery({ queryKey: queryKeys.knowledgeStatus, queryFn: () => api.knowledgeStatus(), enabled, staleTime: 5_000 });
}

export function useCollections(enabled = true) {
  return useQuery({ queryKey: queryKeys.collections, queryFn: () => api.knowledgeCollections(), enabled, staleTime: 10_000 });
}

export function useCollectionDocs(id: string | null) {
  return useQuery({ queryKey: queryKeys.collectionDocs(id ?? ""), queryFn: () => api.knowledgeCollectionDocs(id ?? ""), enabled: !!id });
}

export function useDocumentCollections(id: string | undefined) {
  return useQuery({
    queryKey: queryKeys.documentCollections(id ?? ""),
    queryFn: () => api.knowledgeDocumentCollections(id ?? ""),
    enabled: !!id,
  });
}

export function useGlossary(enabled = true) {
  return useQuery({ queryKey: queryKeys.glossary, queryFn: () => api.knowledgeGlossary(), enabled });
}

export function useVersions(enabled = true) {
  return useQuery({ queryKey: queryKeys.versions, queryFn: () => api.knowledgeVersions(), enabled });
}

export function useAnswers(kind: AnswerKind | null, documentId: string | null, enabled = true) {
  return useQuery({
    queryKey: queryKeys.answers(kind, documentId),
    queryFn: () => api.knowledgeAnswers(kind, documentId, 50),
    enabled,
  });
}

export function useDocumentChunks(documentId: string | undefined) {
  return useQuery({ queryKey: queryKeys.chunks(documentId ?? ""), queryFn: () => api.knowledgeChunks(documentId ?? ""), enabled: !!documentId });
}

/** Runs a search when `req` is set (null = idle); previous results stay while typing. */
export function useSearch(req: SearchRequest | null) {
  return useQuery({
    queryKey: queryKeys.search(req),
    queryFn: () => api.knowledgeSearch(req as SearchRequest),
    enabled: !!req && req.query.trim().length > 0,
    staleTime: 15_000,
    placeholderData: keepPreviousData,
  });
}

// -- pipeline (P4) --

export function usePipelineStatus(enabled = true) {
  return useQuery({ queryKey: queryKeys.pipelineStatus, queryFn: () => api.pipelineStatus(), enabled, staleTime: 5_000 });
}

export function useDocTypes(enabled = true) {
  return useQuery({ queryKey: queryKeys.docTypes, queryFn: () => api.pipelineDocTypes(), enabled, staleTime: 30_000 });
}

export function useSchemas(enabled = true) {
  return useQuery({ queryKey: queryKeys.schemas, queryFn: () => api.pipelineSchemas(), enabled, staleTime: 30_000 });
}

export function useSchemaVersions(id: string | null) {
  return useQuery({ queryKey: queryKeys.schemaVersions(id ?? ""), queryFn: () => api.pipelineSchemaVersions(id ?? ""), enabled: !!id });
}

export function useRuns(documentId: string | undefined) {
  return useQuery({ queryKey: queryKeys.runs(documentId ?? ""), queryFn: () => api.pipelineRuns(documentId ?? ""), enabled: !!documentId });
}

export function useFields(documentId: string | undefined, runId: string | null = null) {
  return useQuery({
    queryKey: queryKeys.fields(documentId ?? "", runId),
    queryFn: () => api.pipelineFields(documentId ?? "", runId),
    enabled: !!documentId,
  });
}

/** Corrections of a page (`pageNo`) or of the whole document (`null`), with the page's undo / redo state. */
export function useDocumentTextHistory(documentId: string | undefined, pageNo: number | null) {
  return useQuery({
    queryKey: queryKeys.documentTextHistory(documentId ?? "", pageNo),
    queryFn: () => api.documentsPageTextHistory(documentId ?? "", pageNo),
    enabled: !!documentId,
  });
}

export function useFieldHistory(documentId: string | undefined) {
  return useQuery({ queryKey: queryKeys.fieldHistory(documentId ?? ""), queryFn: () => api.pipelineFieldHistory(documentId ?? ""), enabled: !!documentId });
}

export function useReviewQueue(filter: ReviewFilter, enabled = true) {
  return useQuery({ queryKey: queryKeys.reviewQueue(filter), queryFn: () => api.pipelineReviewQueue(filter), enabled, placeholderData: keepPreviousData });
}

export function useTables(documentId: string | undefined) {
  return useQuery({ queryKey: queryKeys.tables(documentId ?? ""), queryFn: () => api.pipelineTables(documentId ?? ""), enabled: !!documentId });
}

// -- document generation / projects / maintenance (P6, P7) --

export const generateKeys = {
  all: ["generate"] as const,
  templates: ["generate", "templates"] as const,
  drafts: ["generate", "drafts"] as const,
  budget: ["generate", "budget"] as const,
};

/** Token budget context of the generate screen (model context, backend, cap). */
export function useGenerateBudgetInfo(enabled = true) {
  return useQuery({ queryKey: generateKeys.budget, queryFn: () => api.generateBudgetInfo(), enabled, staleTime: 30_000 });
}

export function useGenerateTemplates(enabled = true) {
  return useQuery({ queryKey: generateKeys.templates, queryFn: () => api.generateTemplates(), enabled, staleTime: Infinity });
}

export function useDrafts(enabled = true) {
  return useQuery({ queryKey: generateKeys.drafts, queryFn: () => api.generateDrafts(), enabled, staleTime: 5_000 });
}

export function invalidateDrafts(): void {
  void queryClient.invalidateQueries({ queryKey: generateKeys.drafts });
}

export const projectKeys = {
  all: ["projects"] as const,
  list: ["projects", "list"] as const,
  detail: (id: string) => ["projects", "detail", id] as const,
  runs: (id: string) => ["projects", "runs", id] as const,
  report: (id: string) => ["projects", "report", id] as const,
  templates: ["projects", "templates"] as const,
};

export function useProjects(enabled = true) {
  return useQuery({ queryKey: projectKeys.list, queryFn: () => api.projectsList(), enabled, staleTime: 5_000 });
}

export function useProject(id: string | null | undefined) {
  return useQuery({ queryKey: projectKeys.detail(id ?? ""), queryFn: () => api.projectGet(id ?? ""), enabled: !!id, staleTime: 5_000 });
}

export function useProjectRuns(id: string | null | undefined, enabled = true) {
  return useQuery({ queryKey: projectKeys.runs(id ?? ""), queryFn: () => api.projectRuns(id ?? ""), enabled: !!id && enabled, staleTime: 5_000 });
}

export function useProjectReport(id: string | null | undefined, enabled = true) {
  return useQuery({ queryKey: projectKeys.report(id ?? ""), queryFn: () => api.projectReport(id ?? ""), enabled: !!id && enabled, staleTime: 5_000 });
}

export function useProjectTemplates(enabled = true) {
  return useQuery({ queryKey: projectKeys.templates, queryFn: () => api.projectTemplates(), enabled, staleTime: 30_000 });
}

/** After a project / rule / check change (`projects://changed` or a local mutation). */
export function invalidateProjects(ids: string[] = []): void {
  void queryClient.invalidateQueries({ queryKey: projectKeys.list });
  void queryClient.invalidateQueries({ queryKey: projectKeys.templates });
  if (ids.length === 0) {
    void queryClient.invalidateQueries({ queryKey: ["projects", "detail"] });
    void queryClient.invalidateQueries({ queryKey: ["projects", "runs"] });
    void queryClient.invalidateQueries({ queryKey: ["projects", "report"] });
    return;
  }
  for (const id of ids) {
    void queryClient.invalidateQueries({ queryKey: projectKeys.detail(id) });
    void queryClient.invalidateQueries({ queryKey: projectKeys.runs(id) });
    void queryClient.invalidateQueries({ queryKey: projectKeys.report(id) });
  }
}

export const maintenanceKey = ["maintenance", "status"] as const;

export function useMaintenanceStatus(enabled = true) {
  return useQuery({ queryKey: maintenanceKey, queryFn: () => api.maintenanceStatus(), enabled, staleTime: 5_000 });
}

export function invalidateMaintenance(): void {
  void queryClient.invalidateQueries({ queryKey: maintenanceKey });
}

// -- journal (P5) --

/** Query keys of the file-operation journal (batch list / entries); `journalKeys.all` invalidates both. */
export const journalKeys = {
  all: ["journal"] as const,
  batches: (limit: number) => ["journal", "batches", limit] as const,
  entries: (batchId: string) => ["journal", "entries", batchId] as const,
};

export function useJournalBatches(limit = 100, enabled = true) {
  return useQuery({ queryKey: journalKeys.batches(limit), queryFn: () => api.journalBatches(limit), enabled, staleTime: 5_000 });
}

export function useJournalEntries(batchId: string | null) {
  return useQuery({ queryKey: journalKeys.entries(batchId ?? ""), queryFn: () => api.journalEntries(batchId ?? ""), enabled: !!batchId });
}

/** After an organise / undo / redo: the journal and the library (paths changed) are stale. */
export function invalidateJournal(): void {
  void queryClient.invalidateQueries({ queryKey: journalKeys.all });
  void queryClient.invalidateQueries({ queryKey: queryKeys.documents });
}
