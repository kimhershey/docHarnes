import { QueryClient } from "@tanstack/react-query";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 30_000, retry: 1, refetchOnWindowFocus: false },
  },
});

export const queryKeys = {
  appInfo: ["app-info"] as const,
  hardware: ["hardware"] as const,
  assessment: ["assessment"] as const,
  registry: ["registry"] as const,
  modelIndex: ["model-index"] as const,
  engineBinary: ["engine-binary"] as const,
};
