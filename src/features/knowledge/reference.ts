// Reference material (0.5.0 plan §2): the query hooks shared by the common reference view, the project's
// 참고 자료 tab and the "프로젝트에 넣기" dialog. Collections are now "참고 자료 묶음": `reference` groups form
// the common reference material, a `project` group belongs to one project.
import { useQuery } from "@tanstack/react-query";

import { api } from "@/lib/ipc";
import { useCollections } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import type { Collection } from "@/lib/types";

export const referenceGroupKey = (projectId: string) => ["knowledge", "reference-group", projectId] as const;

/** The common reference groups (kind `reference`), name order. */
export function useReferenceGroups(enabled = true): { groups: Collection[] | undefined; isLoading: boolean } {
  const { data, isLoading } = useCollections(enabled);
  return { groups: data?.filter((c) => c.kind === "reference"), isLoading };
}

/** The project's own reference group; the backend creates it on the first call. */
export function useProjectReferenceGroup(projectId: string | null | undefined, enabled = true) {
  return useQuery({ queryKey: referenceGroupKey(projectId ?? ""), queryFn: () => api.projectReferenceGroup(projectId ?? ""), enabled: !!projectId && enabled, staleTime: 30_000 });
}

/** After a group / membership change made locally (the `knowledge://changed` event covers the rest). */
export function invalidateReference(groupId?: string | null): void {
  void queryClient.invalidateQueries({ queryKey: queryKeys.collections });
  void queryClient.invalidateQueries({ queryKey: queryKeys.knowledgeStatus });
  void queryClient.invalidateQueries({ queryKey: ["knowledge", "doc-collections"] });
  void queryClient.invalidateQueries({ queryKey: ["knowledge", "reference-group"] });
  if (groupId) void queryClient.invalidateQueries({ queryKey: queryKeys.collectionDocs(groupId) });
}

/** Adds library documents to a reference group and marks them as 참고 자료 (`knowledge` role). */
export async function addToReferenceGroup(groupId: string, docIds: string[]): Promise<number> {
  if (docIds.length === 0) return 0;
  const n = await api.knowledgeCollectionAdd(groupId, docIds, true);
  invalidateReference(groupId);
  void queryClient.invalidateQueries({ queryKey: queryKeys.documents });
  return n;
}
