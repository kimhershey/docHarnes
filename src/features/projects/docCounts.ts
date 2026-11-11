import type { DocumentRecord, ProjectDocument } from "@/lib/types";

/**
 * 업무 문서 / 참고 자료 counts of a project, the way the 문서 tab lists them: a document that is in the project's
 * documents and in its reference group shows as 참고 자료 (0.5.0: reference wins), so it counts once — the header
 * used to add the two memberships and count such a document twice (PRJ-16).
 */
export function projectDocCounts(documents: ProjectDocument[], referenceDocs: DocumentRecord[] | undefined): { work: number; reference: number } {
  const referenceIds = new Set((referenceDocs ?? []).map((d) => d.id));
  return { work: documents.filter((d) => !referenceIds.has(d.document.id)).length, reference: referenceIds.size };
}
