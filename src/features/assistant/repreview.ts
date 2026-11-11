// The preview of a plan the user edited on its card (PLN-14).
//
// The planner's response carries the preview of the plan as it was interpreted. Once the scope or a
// parameter is edited those numbers describe another plan: the card said "대상 문서 3건" while the
// edited plan covered more. There is no command that previews an arbitrary plan, so the edited plan
// is previewed with the commands the screens use for the same questions — `organize_plan` (the
// documents of a scope, the diff of a file op) and `export_preview` — into a `PlanPreview` of the
// shape `planner/exec.rs` builds, so the card renders it like the original one. What only the
// planner's own preview knows (rule drafts, import counts, project suggestions …) is kept for a step
// the edit did not touch and dropped for one it did, rather than shown stale.

import { DEFAULT_NAME_TEMPLATE, FOLDER_TEMPLATES, type ExportPreview, type ExportRequest, type Language, type Op, type OrganizePlan, type OrganizeRequest, type Plan, type PlanPreview, type Scope, type StepPreview } from "@/lib/types";

export interface RepreviewDeps {
  organizePlan: (req: OrganizeRequest) => Promise<OrganizePlan>;
  exportPreview: (req: ExportRequest) => Promise<ExportPreview>;
}

export interface RepreviewOptions {
  language: Language;
  /** `settings.processing.organize_reviewed_only`: what a file op without `only_reviewed` does. */
  onlyReviewedDefault: boolean;
  /** "문서 N건": the line of a step that works on every document in scope. */
  countLabel: (count: number) => string;
  /** "문서 N건 휴지통으로": the line of a `delete` step. */
  deleteLabel: (count: number) => string;
  /** A step's preview failed (unknown template variable, path outside the workspace …): the text shown instead. */
  describeError: (e: unknown, kind: "organize" | "export") => string;
}

/** The error of a step whose preview could not be made (an edited template with an unknown variable, a path outside the workspace …). */
export function previewFailure(step: StepPreview | undefined): string | null {
  const payload = step?.payload;
  if (typeof payload !== "object" || payload === null) return null;
  const message = (payload as { preview_error?: unknown }).preview_error;
  return typeof message === "string" ? message : null;
}

/** JSON with sorted object keys: a filter value removed and set again in the editor lands behind the other keys. */
function stable(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stable).join(",")}]`;
  if (typeof value !== "object" || value === null) return JSON.stringify(value) ?? "null";
  const record = value as Record<string, unknown>;
  return `{${Object.keys(record)
    .filter((k) => record[k] !== undefined)
    .sort()
    .map((k) => `${JSON.stringify(k)}:${stable(record[k])}`)
    .join(",")}}`;
}

/** What a preview depends on: the scope and the operations (not the summary, confidence … or the order of keys). */
export function planKey(plan: Pick<Plan, "scope" | "operations">): string {
  return stable({ scope: plan.scope ?? {}, operations: plan.operations });
}

/** `exec::plan_scope`: the project of the scope is the destination of `add_to_project`, not a filter, when the op targets it. */
function effectiveScope(plan: Plan): Scope {
  const target = plan.scope.filter?.project_id ?? null;
  if (!target) return plan.scope;
  const adds = plan.operations.some((op) => op.op === "add_to_project" && (op.project == null || op.project === target));
  if (!adds) return plan.scope;
  const filter = { ...plan.scope.filter };
  delete filter.project_id;
  return { ...plan.scope, filter };
}

function sameOp(a: Op | undefined, b: Op | undefined): boolean {
  return a !== undefined && b !== undefined && stable(a) === stable(b);
}

/**
 * Previews `plan` (an edited version of `original`, same operations in the same order — the editor
 * changes parameters and the scope only). `originalPreview` is the planner's preview of `original`.
 */
export async function previewEditedPlan(plan: Plan, original: Plan, originalPreview: PlanPreview | null, deps: RepreviewDeps, options: RepreviewOptions): Promise<PlanPreview> {
  const { language } = options;
  const scope = effectiveScope(plan);
  const scopeChanged = stable(scope) !== stable(effectiveScope(original));
  // The documents in scope: `organize_plan` without templates lists them (unchanged / missing rows).
  const inScope = await deps.organizePlan({ scope, name_template: null, folder_template: null, only_reviewed: false, language });
  const count = inScope.items.length;
  const fileOp = async (index: number, op: Op, req: Pick<OrganizeRequest, "name_template" | "folder_template" | "only_reviewed">, confirm: boolean): Promise<StepPreview> => {
    try {
      const diff = await deps.organizePlan({ scope: plan.scope, ...req, language });
      return { index, op: op.op, summary: "", payload: diff, confirm };
    } catch (e) {
      return { index, op: op.op, summary: "", payload: { preview_error: options.describeError(e, "organize") }, confirm };
    }
  };
  const steps: StepPreview[] = [];
  for (const [index, op] of plan.operations.entries()) {
    const before = originalPreview?.steps.find((s) => s.index === index);
    switch (op.op) {
      case "organize":
        steps.push(
          await fileOp(
            index,
            op,
            { name_template: op.template ?? DEFAULT_NAME_TEMPLATE, folder_template: op.folder_template ?? FOLDER_TEMPLATES[op.group_by ?? "doc_type"], only_reviewed: op.only_reviewed ?? options.onlyReviewedDefault },
            op.dry_run !== true,
          ),
        );
        break;
      case "rename":
        steps.push(await fileOp(index, op, { name_template: op.template, folder_template: null, only_reviewed: op.only_reviewed ?? options.onlyReviewedDefault }, true));
        break;
      case "move":
        steps.push(await fileOp(index, op, { name_template: null, folder_template: op.folder, only_reviewed: op.only_reviewed ?? options.onlyReviewedDefault }, true));
        break;
      case "export":
        try {
          const preview = await deps.exportPreview({
            format: op.format,
            target: op.target ?? "fields",
            path: op.path ?? null,
            columns: op.columns ?? [],
            layout: op.layout ?? "sheet_per_type",
            include: op.include ?? [],
            scope: plan.scope,
            only_reviewed: false,
            overwrite: false,
            language,
          });
          steps.push({ index, op: op.op, summary: "", payload: preview, confirm: preview.exists });
        } catch (e) {
          steps.push({ index, op: op.op, summary: "", payload: { preview_error: options.describeError(e, "export") }, confirm: false });
        }
        break;
      case "delete":
        steps.push({ index, op: op.op, summary: options.deleteLabel(count), payload: { documents: count }, confirm: true });
        break;
      case "classify":
      case "ocr":
      case "validate":
      case "summarize":
        steps.push({ index, op: op.op, summary: options.countLabel(count), payload: { documents: count }, confirm: false });
        break;
      case "tag":
      case "set_doc_type":
        // Bulk edits of several documents are confirmed first.
        steps.push({ index, op: op.op, summary: options.countLabel(count), payload: { documents: count }, confirm: count > 1 });
        break;
      case "review":
        if (op.action === "show") {
          if (before) steps.push(before);
        } else steps.push({ index, op: op.op, summary: options.countLabel(count), payload: { documents: count }, confirm: count > 1 });
        break;
      default: {
        // Only the planner's preview has this step's payload: it still holds when neither the step nor (for
        // the one step that reads the scope) the scope changed.
        const untouched = sameOp(op, original.operations[index]) && !(op.op === "classify_projects" && scopeChanged);
        if (before && untouched) steps.push(before);
        else if (before) steps.push({ index, op: op.op, summary: "", payload: null, confirm: before.confirm });
        break;
      }
    }
  }
  return {
    document_count: count,
    document_ids: inScope.items.map((item) => item.doc_id),
    steps,
    requires_confirmation: (originalPreview?.requires_confirmation ?? false) || steps.some((s) => s.confirm),
  };
}
