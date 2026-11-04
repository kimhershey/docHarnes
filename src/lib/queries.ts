import { useQuery } from "@tanstack/react-query";

import { api } from "./ipc";
import { queryKeys } from "./query-client";
import type { DocumentFilter } from "./types";

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
  return useQuery({ queryKey: queryKeys.documentsList(filter), queryFn: () => api.documentsList(filter), enabled, staleTime: 5_000 });
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

export function useDocumentTags(enabled = true) {
  return useQuery({ queryKey: queryKeys.documentTags, queryFn: () => api.documentsTags(), enabled, staleTime: 10_000 });
}

export function usePdfiumInfo(enabled = true) {
  return useQuery({ queryKey: queryKeys.pdfium, queryFn: () => api.documentsPdfiumInfo(), enabled, staleTime: Infinity });
}
