import { keepPreviousData, useQuery } from "@tanstack/react-query";

import { api } from "./ipc";
import { queryKeys } from "./query-client";
import type { AnswerKind, DocumentFilter, SearchRequest } from "./types";

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
