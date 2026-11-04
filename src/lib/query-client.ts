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
  documents: ["documents"] as const,
  documentsList: (filter: unknown) => ["documents", "list", filter] as const,
  documentsStats: ["documents", "stats"] as const,
  document: (id: string) => ["documents", "doc", id] as const,
  documentPages: (id: string, withText: boolean) => ["documents", "pages", id, withText] as const,
  documentPageText: (id: string, pageNo: number) => ["documents", "text", id, pageNo] as const,
  pdfium: ["pdfium"] as const,
};
