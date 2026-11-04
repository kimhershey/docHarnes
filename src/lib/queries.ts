import { useQuery } from "@tanstack/react-query";

import { api } from "./ipc";
import { queryKeys } from "./query-client";

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
