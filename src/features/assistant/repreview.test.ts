// PLN-14: the preview of a plan the user edited on its card, built from `organize_plan` / `export_preview`
// into the shape the planner's own preview has.
import { describe, expect, it } from "vitest";

import type { ExportPreview, ExportRequest, OrganizeItem, OrganizePlan, OrganizeRequest, Plan, PlanPreview } from "@/lib/types";

import { exportPreview, impactOf, organizePreview } from "./describePlan";
import { planKey, previewEditedPlan, previewFailure, type RepreviewDeps } from "./repreview";

const options = { language: "ko" as const, onlyReviewedDefault: true, countLabel: (n: number) => `문서 ${n}건`, deleteLabel: (n: number) => `문서 ${n}건 휴지통으로`, describeError: (e: unknown) => String((e as { message?: string }).message ?? e) };

/** Two receipts (one review-pending) and a contract, selected by `filter.doc_type`. */
const LIBRARY = [
  { id: "r1", type: "receipt", pending: false },
  { id: "r2", type: "receipt", pending: true },
  { id: "c1", type: "contract", pending: false },
];

function fakeDeps(): RepreviewDeps & { organizeCalls: OrganizeRequest[]; exportCalls: ExportRequest[] } {
  const organizeCalls: OrganizeRequest[] = [];
  const exportCalls: ExportRequest[] = [];
  const inScope = (req: { scope?: OrganizeRequest["scope"] }) => LIBRARY.filter((d) => !req.scope?.filter?.doc_type || d.type === req.scope.filter.doc_type);
  return {
    organizeCalls,
    exportCalls,
    organizePlan: async (req) => {
      organizeCalls.push(req);
      if ((req.folder_template ?? "").includes("{{nope}}")) throw { code: "template_variable", message: "template_variable: nope" };
      const moving = req.name_template != null || req.folder_template != null;
      const items: OrganizeItem[] = inScope(req).map((d) => {
        const held = moving && req.only_reviewed === true && d.pending;
        return { doc_id: d.id, name: d.id, from: `inbox/${d.id}.pdf`, to: moving && !held ? `documents/${d.id}.pdf` : null, status: held ? "held" : moving ? "move" : "unchanged" };
      });
      const count = (status: OrganizeItem["status"]) => items.filter((i) => i.status === status).length;
      return { items, moves: count("move"), unchanged: count("unchanged"), held: count("held"), missing_files: 0, needs_review: 0, name_template: req.name_template ?? null, folder_template: req.folder_template ?? null, only_reviewed: req.only_reviewed ?? true } satisfies OrganizePlan;
    },
    exportPreview: async (req) => {
      exportCalls.push(req);
      return { path: req.path ?? "exports/out.xlsx", exists: req.path === "exports/old.xlsx", documents: inScope(req).length, skipped: 0, columns: [], sheets: [] } satisfies ExportPreview;
    },
  };
}

function plan(operations: Plan["operations"], docType: string | null): Plan {
  return { intent: operations[0]!.op, scope: docType ? { filter: { doc_type: docType } } : {}, operations, requires_confirmation: true, confidence: 0.9, summary: "요약" };
}

describe("previewEditedPlan", () => {
  it("counts the documents of the edited scope and previews the file op again", async () => {
    const original = plan([{ op: "move", folder: "documents/{{doc_type}}" }], "receipt");
    const originalPreview: PlanPreview = { document_count: 2, document_ids: ["r1", "r2"], steps: [{ index: 0, op: "move", summary: "", payload: { items: [], moves: 1, held: 1, unchanged: 0 }, confirm: true }], requires_confirmation: true };
    // The type filter is removed: three documents now.
    const edited = plan([{ op: "move", folder: "documents/{{doc_type}}" }], null);
    const deps = fakeDeps();
    const fresh = await previewEditedPlan(edited, original, originalPreview, deps, options);
    expect(fresh.document_count).toBe(3);
    expect(fresh.document_ids).toEqual(["r1", "r2", "c1"]);
    const rows = organizePreview(fresh.steps[0]);
    expect(rows).toMatchObject({ moves: 2, held: 1, unchanged: 0 });
    expect(rows!.items.map((i) => i.doc_id)).toEqual(["r1", "r2", "c1"]);
    expect(impactOf(edited, fresh)).toMatchObject({ documents: 3, moves: 2 });
    // First the documents in scope (no templates, nothing held back), then the op as the executor would run it.
    expect(deps.organizeCalls).toEqual([
      { scope: {}, name_template: null, folder_template: null, only_reviewed: false, language: "ko" },
      { scope: {}, name_template: null, folder_template: "documents/{{doc_type}}", only_reviewed: true, language: "ko" },
    ]);
  });

  it("follows an edited parameter: only_reviewed off moves the pending document too", async () => {
    const original = plan([{ op: "organize", group_by: "month" }], "receipt");
    const edited = plan([{ op: "organize", group_by: "month", only_reviewed: false }], "receipt");
    const deps = fakeDeps();
    const fresh = await previewEditedPlan(edited, original, null, deps, options);
    expect(organizePreview(fresh.steps[0])).toMatchObject({ moves: 2, held: 0 });
    expect(deps.organizeCalls[1]).toMatchObject({ name_template: "{{year}}-{{month}}-{{day}}_{{doc_type}}_{{vendor}}_{{total}}", folder_template: "documents/{{year}}/{{month}}/{{doc_type}}", only_reviewed: false });
    expect(fresh.requires_confirmation).toBe(true);
  });

  it("delete, bulk edits and model steps take the count of the edited scope", async () => {
    const original = plan([{ op: "delete" }, { op: "tag", add: ["긴급"] }, { op: "summarize" }], "contract");
    const edited = plan([{ op: "delete" }, { op: "tag", add: ["긴급"] }, { op: "summarize" }], "receipt");
    const fresh = await previewEditedPlan(edited, original, null, fakeDeps(), options);
    expect(fresh.steps.map((s) => [s.op, s.summary, s.payload, s.confirm])).toEqual([
      ["delete", "문서 2건 휴지통으로", { documents: 2 }, true],
      ["tag", "문서 2건", { documents: 2 }, true],
      ["summarize", "문서 2건", { documents: 2 }, false],
    ]);
    // The card's "삭제 N건" follows.
    expect(impactOf(edited, fresh)).toMatchObject({ documents: 2, deletes: 2 });
  });

  it("previews an export again: the count, the path and whether it overwrites", async () => {
    const original = plan([{ op: "export", format: "xlsx" }], "receipt");
    const edited = plan([{ op: "export", format: "xlsx", path: "exports/old.xlsx", layout: "single" }], null);
    const deps = fakeDeps();
    const fresh = await previewEditedPlan(edited, original, null, deps, options);
    expect(exportPreview(fresh.steps[0])).toMatchObject({ documents: 3, path: "exports/old.xlsx", exists: true });
    expect(fresh.steps[0]!.confirm).toBe(true);
    expect(deps.exportCalls[0]).toMatchObject({ format: "xlsx", target: "fields", layout: "single", path: "exports/old.xlsx", only_reviewed: false, overwrite: false, scope: {} });
  });

  it("keeps the planner's preview of a step the edit did not touch and drops it for one it did", async () => {
    const ops: Plan["operations"] = [{ op: "define_rule", description: "합계 일치" }, { op: "set_retention", item: "chats", days: 30 }, { op: "classify_projects" }];
    const original = plan(ops, "receipt");
    const originalPreview: PlanPreview = {
      document_count: 2,
      document_ids: [],
      steps: [
        { index: 0, op: "define_rule", summary: "규칙 그리드", payload: { description: "합계 일치", draft: null }, confirm: true },
        { index: 1, op: "set_retention", summary: "30일", payload: { item: "chats", days: 30 }, confirm: true },
        { index: 2, op: "classify_projects", summary: "제안 2건", payload: { suggested: 2 }, confirm: false },
      ],
      requires_confirmation: true,
    };
    // The retention period and the scope are edited; the rule is not.
    const edited = plan([ops[0]!, { op: "set_retention", item: "chats", days: 7 }, ops[2]!], "contract");
    const fresh = await previewEditedPlan(edited, original, originalPreview, fakeDeps(), options);
    expect(fresh.steps[0]).toEqual(originalPreview.steps[0]);
    expect(fresh.steps[1]).toEqual({ index: 1, op: "set_retention", summary: "", payload: null, confirm: true });
    // `classify_projects` reads the scope: its suggestions are those of the scope before the edit.
    expect(fresh.steps[2]).toEqual({ index: 2, op: "classify_projects", summary: "", payload: null, confirm: false });
    expect(fresh.document_count).toBe(1);
  });

  it("reports a step whose preview fails without losing the rest", async () => {
    const original = plan([{ op: "move", folder: "documents/{{doc_type}}" }, { op: "delete" }], "receipt");
    const edited = plan([{ op: "move", folder: "documents/{{nope}}" }, { op: "delete" }], "receipt");
    const fresh = await previewEditedPlan(edited, original, null, fakeDeps(), options);
    expect(previewFailure(fresh.steps[0])).toBe("template_variable: nope");
    expect(organizePreview(fresh.steps[0])).toBeNull();
    expect(fresh.steps[1]).toMatchObject({ payload: { documents: 2 } });
    expect(previewFailure(fresh.steps[1])).toBeNull();
  });

  it("planKey is the scope and the operations, nothing else", () => {
    const a = plan([{ op: "delete" }], "receipt");
    const reworded: Plan = { ...a, summary: "다른 요약", confidence: 0.1 };
    expect(planKey(reworded)).toBe(planKey(a));
    expect(planKey(plan([{ op: "delete" }], "contract"))).not.toBe(planKey(a));
    expect(planKey(plan([{ op: "delete", to_trash: false }], "receipt"))).not.toBe(planKey(a));
    // A filter value taken out and put back in the editor ends up behind the other keys: still the same plan.
    const dated: Plan = { ...a, scope: { filter: { doc_type: "receipt", date_from: "2026-01-01" } } };
    const reordered: Plan = { ...a, scope: { filter: { date_from: "2026-01-01", doc_type: "receipt" } } };
    expect(planKey(reordered)).toBe(planKey(dated));
  });
});
