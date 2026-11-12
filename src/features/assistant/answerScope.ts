// Where a question looks: the knowledge scope of an answer request. Pure helpers of the assistant page —
// whether two requests look in the same place (the previous evidence is only kept then, KNO-1) and what a
// single-`answer` plan narrows the search to (KNO-5).

import type { Plan, PlanPreview, Scope, SearchScope } from "@/lib/types";

const list = (values: readonly string[] | undefined): string => [...new Set(values ?? [])].sort().join("\u0000");

/**
 * Whether two answer requests search the same place: the same documents, groups, roles, folder and project,
 * with the same "최신본만" / active-groups switches (an absent switch is its default, on). Order and
 * duplicates in the lists do not matter; an absent list is an empty one.
 */
export function sameSearchScope(a: SearchScope | undefined, b: SearchScope | undefined): boolean {
  const x = a ?? {};
  const y = b ?? {};
  return (
    list(x.documents) === list(y.documents) &&
    list(x.collections) === list(y.collections) &&
    list(x.roles) === list(y.roles) &&
    (x.folder || null) === (y.folder || null) &&
    (x.project_id || null) === (y.project_id || null) &&
    (x.latest_only ?? true) === (y.latest_only ?? true) &&
    (x.active_collections_only ?? true) === (y.active_collections_only ?? true)
  );
}

/** `Scope::is_empty` of the core: no folder, no documents and no filter. `ignoreProject`: the project filter does not count. */
export function planScopeIsEmpty(scope: Scope | undefined, ignoreProject = false): boolean {
  if (!scope) return true;
  const f = scope.filter ?? {};
  return (
    !scope.folder &&
    (scope.doc_ids ?? []).length === 0 &&
    !f.doc_type &&
    !f.date_from &&
    !f.date_to &&
    !f.keyword &&
    !f.vendor &&
    !f.status &&
    (f.tags ?? []).length === 0 &&
    f.min_confidence == null &&
    !f.review_status &&
    !f.role &&
    (ignoreProject || !f.project_id)
  );
}

/**
 * What a plan that is a single `answer` asks the knowledge base about (the same decision as the executor's,
 * `exec.rs` `Op::Answer`):
 * - `selector`: the plan names no scope — the assistant's scope selector applies (with the plan's documents /
 *   folder, should it carry some without a preview);
 * - `documents`: the plan narrows the scope (a date range, a document type, a vendor, tags, a project, a folder
 *   or documents) — the search goes over the documents the plan preview resolved, together with the project's
 *   reference material when the scope names a project. `documents` is empty only for a scope that names
 *   nothing but a project without work documents: the project scope alone applies then;
 * - `none`: the scope narrows to nothing — no document of that type / date / folder. The answer is "no
 *   evidence" for that scope; an empty document list in a request would mean "everything".
 */
export type PlanAnswerScope = { kind: "selector" } | { kind: "none" } | { kind: "documents"; documents: string[]; projectId: string | null; role?: "work" | "knowledge" | "form" | null };

export function planAnswerScope(plan: Pick<Plan, "scope">, preview: Pick<PlanPreview, "document_ids"> | null | undefined): PlanAnswerScope {
  // Without a preview nothing is known about what the scope resolves to (a response of an older shell).
  if (planScopeIsEmpty(plan.scope) || !preview) return { kind: "selector" };
  const documents = preview.document_ids ?? [];
  // A project alone does not narrow the documents: with none of its own the project still has its reference material.
  if (documents.length === 0 && !planScopeIsEmpty(plan.scope, true)) return { kind: "none" };
  // 0.8.1: "참고 자료에서 …" / "업무 문서만 …" travels as the role of the search scope.
  const role = plan.scope.filter?.role ?? null;
  return { kind: "documents", documents, projectId: plan.scope.filter?.project_id || null, ...(role ? { role } : {}) };
}
