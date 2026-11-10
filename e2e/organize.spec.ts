import { expect, test, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { card, createWorkspace, docId, freshApp, importViaLibrary, tpl, tplPattern } from "./helpers";

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

// P5 acceptance (design §3.7 "정리/내보내기", §3.5 "되돌리기"): organise preview (held while
// review pending → moved under the monthly folder), journaled apply with undo from the result and
// from the batch history, the export wizard, the library aggregate bar and the bulk-bar shortcuts
// — against the browser mock backend. Real file moves, the journal and the writers are covered by
// the Rust tests.

/** Imports the estimate + development contract without auto-processing and runs 표준 처리 on the estimate. */
async function importAndProcessReceipt(page: Page): Promise<{ receipt: string; invoice: string }> {
  await importViaLibrary(page, ["D:\\scan\\견적서-1p.jpg", "D:\\scan\\개발계약서-2p.pdf"], { autoLevel: "none" });
  const receipt = await docId(page, "견적서-1p.jpg");
  const invoice = await docId(page, "개발계약서-2p.pdf");
  await page.getByTestId(`doc-select-${receipt}`).check();
  await page.getByTestId("library-process").click();
  await page.getByTestId("library-process-standard").click();
  await expect(card(page, "견적서-1p.jpg")).toHaveAttribute("data-doc-status", "extracted", { timeout: 15_000 });
  await page.getByTestId(`doc-select-${receipt}`).uncheck();
  return { receipt, invoice };
}

test.describe("P5 organise / export", () => {
  test("organise: held while review pending, moved under the monthly folder, journaled and undone from the history", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    const { receipt, invoice } = await importAndProcessReceipt(page);

    await page.getByTestId("nav-organize").click();
    await expect(page.getByTestId("page-organize")).toBeVisible();
    // Defaults: monthly preset, default name template, "reviewed only" from the settings (on).
    await expect(page.getByTestId("organize-folder-template")).toHaveValue("documents/{{year}}/{{month}}/{{doc_type}}");
    await expect(page.getByTestId("organize-name-template")).toHaveValue("{{year}}-{{month}}-{{day}}_{{doc_type}}_{{vendor}}_{{total}}");
    await expect(page.getByTestId("organize-only-reviewed")).toHaveAttribute("aria-checked", "true");
    await expect(page.getByTestId("organize-example")).toHaveText("documents/2026/10/견적서/2026-10-14_견적서_㈜한빛소프트_12,500,000.jpg");
    // A variable chip inserts at the caret and an unknown variable is flagged locally.
    await page.getByTestId("organize-name-template").fill("{{bogus}}");
    await expect(page.getByTestId("organize-name-template-error")).toContainText("bogus");
    await page.getByTestId("organize-name-template").fill("");
    await page.getByTestId("organize-name-template-var-vendor").click();
    await expect(page.getByTestId("organize-name-template")).toHaveValue("{{vendor}}");
    await page.getByTestId("organize-name-template").fill("{{year}}-{{month}}-{{day}}_{{doc_type}}_{{vendor}}_{{total}}");

    // Preview: the review-pending receipt is held, the unprocessed invoice moves with unknown values.
    await page.getByTestId("organize-preview").click();
    await expect(page.getByTestId("organize-preview-table")).toBeVisible();
    const receiptRow = page.getByTestId(`organize-row-${receipt}`);
    await expect(receiptRow).toHaveAttribute("data-row-status", "held");
    await expect(receiptRow).toContainText(ko.organize.status.held);
    const invoiceRow = page.getByTestId(`organize-row-${invoice}`);
    await expect(invoiceRow).toHaveAttribute("data-row-status", "move");
    await expect(invoiceRow).toContainText("미상");
    await expect(page.getByTestId("organize-summary")).toContainText(tpl(ko.organize.summary, { moves: 1, held: 1, unchanged: 0, needs_review: 1 }));

    // Reviewed-only off → the estimate moves under documents/<year>/<month>/견적서/.
    await page.getByTestId("organize-only-reviewed").click();
    await page.getByTestId("organize-preview").click();
    await expect(receiptRow).toHaveAttribute("data-row-status", "move");
    await expect(receiptRow).toContainText("documents/2026/10/견적서/");
    await expect(receiptRow).toContainText("한빛소프트");

    // Run → confirm → job → result with undo.
    await page.getByTestId("organize-run").click();
    await expect(page.getByTestId("organize-confirm-dialog")).toContainText(tpl(ko.organize.confirm_body, { count: 2 }));
    await page.getByTestId("organize-confirm").click();
    await expect(page.getByTestId("organize-result")).toContainText(tplPattern(ko.organize.done, { failed: 0 }), { timeout: 15_000 });
    await expect(page.getByTestId("organize-undo")).toBeVisible();

    // History lists the batch; undo it there and inspect the entries.
    await page.getByTestId("organize-tab-history").click();
    const batch = page.locator('[data-testid^="history-batch-"][data-batch-kind="organize"]').first();
    await expect(batch).toBeVisible();
    await expect(batch).toContainText(ko.organize.history.kind.organize);
    await expect(batch).toContainText(tpl(ko.organize.history.entries, { count: 2 }));
    await batch.getByTestId("history-undo").click();
    await expect(batch).toHaveAttribute("data-batch-undone", "true", { timeout: 10_000 });
    await expect(batch).toContainText(ko.organize.history.undone);
    await batch.getByTestId("history-show-entries").click();
    const entries = batch.getByTestId("history-entry");
    await expect(entries).toHaveCount(2);
    await expect(entries.first()).toHaveAttribute("data-entry-undone", "true");
    // The undo batch is listed newest-first and offers redo.
    const undoBatch = page.locator('[data-testid^="history-batch-"][data-batch-kind="undo"]').first();
    await expect(undoBatch).toBeVisible();
    await expect(undoBatch.getByTestId("history-redo")).toBeVisible();
    // The organise tab's result reflects the undo.
    await page.getByTestId("organize-tab-organize").click();
    await expect(page.getByTestId("organize-undo")).toBeHidden();
  });

  test("export wizard: CSV preview shows the default path, the job finishes with the path", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importAndProcessReceipt(page);

    await page.getByTestId("nav-organize").click();
    await page.getByTestId("organize-tab-export").click();
    await expect(page.getByTestId("export-target")).toContainText(ko.export.target.fields);
    await page.getByTestId("export-format").click();
    await page.getByRole("option", { name: ko.export.format.csv }).click();
    await expect(page.getByTestId("export-path")).toHaveValue(/^exports\/\d{4}-\d{2}-\d{2}_.+\.csv$/);
    await expect(page.getByTestId("export-preview-line")).toContainText(tplPattern(ko.export.preview_line, {}));
    await page.getByTestId("export-run").click();
    await expect(page.getByTestId("export-result")).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("export-result-path")).toContainText(/^exports\/.+\.csv$/);
    await expect(page.getByTestId("export-open-folder")).toBeVisible();

    // The searchable PDF forces the body-text target and explains it.
    await page.getByTestId("export-format").click();
    await page.getByRole("option", { name: ko.export.format.pdf_searchable }).click();
    await expect(page.getByTestId("export-target")).toContainText(ko.export.target.text);
    await expect(page.getByTestId("export-target")).toBeDisabled();
    await expect(page.getByTestId("export-tab")).toContainText(ko.export.pdf_hint);
    // The default path follows the format (a per-document folder for a multi-document searchable PDF).
    await expect(page.getByTestId("export-path")).not.toHaveValue(/\.csv$/);
    await expect(page.getByTestId("export-path")).toHaveValue(/^exports\/\d{4}-\d{2}-\d{2}_/);
  });

  test("organise by project: '이 프로젝트 정리' scopes the screen to the project and fills the {{project}} templates", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    const { receipt, invoice } = await importAndProcessReceipt(page);

    // A project holding the estimate; its 정리·내보내기 tab opens the organise screen scoped to it.
    await page.getByTestId("nav-projects").click();
    await page.getByTestId("projects-new").click();
    await page.getByTestId("project-name").fill("한빛 포털 구축");
    await page.getByTestId("project-intake").fill("2026-001");
    await page.getByTestId("project-submit").click();
    await expect(page.getByTestId("page-project")).toBeVisible();
    await page.getByTestId("project-add-documents").click();
    await page.getByTestId(`project-add-doc-${receipt}`).check();
    await page.getByTestId("project-add-done").click();
    await expect(page.getByTestId(`project-doc-${receipt}`)).toBeVisible();
    await page.getByTestId("project-tab-settings").click();
    await page.getByTestId("project-organize-now").click();
    await expect(page.getByTestId("page-organize")).toBeVisible();
    await expect(page.getByTestId("organize-scope-mode")).toContainText(tpl(ko.organize.scope.project, { name: "한빛 포털 구축" }));
    await expect(page.getByTestId("organize-project-settings")).toHaveAttribute("aria-checked", "true");
    await expect(page.getByTestId("organize-folder-template")).toHaveValue("documents/{{project}}/{{doc_type}}");
    await expect(page.getByTestId("organize-example")).toContainText("documents/한빛 포털 구축/견적서/");

    // Preview: only the member moves, under the project folder; the contract outside the project is not listed.
    await page.getByTestId("organize-only-reviewed").click();
    await page.getByTestId("organize-name-template").fill("{{project_code}}_{{project_phase}}_{{title}}");
    await page.getByTestId("organize-preview").click();
    await expect(page.getByTestId("organize-preview-table")).toBeVisible();
    const row = page.getByTestId(`organize-row-${receipt}`);
    await expect(row).toHaveAttribute("data-row-status", "move");
    await expect(row).toContainText(`documents/한빛 포털 구축/견적서/2026-001_${ko.projects.phase.active}_`);
    await expect(page.getByTestId(`organize-row-${invoice}`)).toHaveCount(0);
    await expect(page.getByTestId("organize-summary")).toContainText(tpl(ko.organize.summary, { moves: 1, held: 0, unchanged: 0, needs_review: 0 }));

    // The 프로젝트별 preset is offered for any scope; back on the whole library the contract has no project → 미상.
    await page.getByTestId("organize-project-settings").click();
    await page.getByTestId("organize-scope-mode").click();
    await page.getByRole("option", { name: ko.organize.scope.all }).click();
    await page.getByTestId("organize-folder-preset").click();
    await page.getByRole("option", { name: ko.organize.folder.presets.project }).click();
    await expect(page.getByTestId("organize-folder-template")).toHaveValue("documents/{{project}}/{{doc_type}}");
    await page.getByTestId("organize-preview").click();
    await expect(page.getByTestId(`organize-row-${invoice}`)).toContainText("documents/미상/");
    await expect(page.getByTestId(`organize-row-${receipt}`)).toContainText("documents/한빛 포털 구축/견적서/");

    // Run → the estimate lands under the project folder and the move is journaled.
    await page.getByTestId("organize-run").click();
    await page.getByTestId("organize-confirm").click();
    await expect(page.getByTestId("organize-result")).toContainText(tplPattern(ko.organize.done, { failed: 0 }), { timeout: 15_000 });
    await page.getByTestId("nav-library").click();
    await expect(card(page, "견적서-1p.jpg")).toBeVisible();
  });

  test("library: the aggregate bar sums the selection and the bulk-bar shortcut carries it to 정리", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    const { receipt } = await importAndProcessReceipt(page);

    await expect(page.getByTestId("aggregate-bar")).toBeHidden();
    await page.getByTestId(`doc-select-${receipt}`).check();
    const bar = page.getByTestId("aggregate-bar");
    await expect(bar).toBeVisible();
    await expect(bar).toContainText(tpl(ko.aggregate.scope_selection, { count: 1 }));
    // The receipt's total is review pending: kept apart from the headline until "검토 대기 포함" is on.
    const value = page.getByTestId("aggregate-value");
    const pendingExcluded = new RegExp(escapeRe(tpl(ko.aggregate.pending_excluded, { count: 1, value: "@@" })).replace("@@", "([\\d,.]+)"));
    await expect(value).toContainText(`${ko.aggregate.fn.sum} 0 = ${tpl(ko.aggregate.reviewed, { count: 0, value: 0 })}`, { timeout: 10_000 });
    await expect(value).toHaveText(pendingExcluded);
    const total = (await value.textContent())?.match(pendingExcluded)?.[1] ?? "";
    expect(total).not.toBe("");
    await page.getByTestId("aggregate-include-unreviewed").click();
    await expect(value).toContainText(`${ko.aggregate.fn.sum} ${total} = ${tpl(ko.aggregate.reviewed, { count: 0, value: 0 })} + ${tpl(ko.aggregate.pending, { count: 1, value: total })}`, { timeout: 10_000 });
    await page.getByTestId("aggregate-monthly").click();
    await expect(page.getByTestId("aggregate-groups")).toContainText("2026-10", { timeout: 10_000 });
    // Count mode disables the function select.
    await page.getByTestId("aggregate-field").click();
    await page.getByRole("option", { name: ko.aggregate.count }).click();
    await expect(page.getByTestId("aggregate-fn")).toBeDisabled();
    await expect(page.getByTestId("aggregate-value")).toContainText(ko.aggregate.fn.count, { timeout: 10_000 });
    // Collapsing keeps the bar as a slim row.
    await page.getByTestId("aggregate-toggle").click();
    await expect(bar).toHaveAttribute("data-open", "false");
    await page.getByTestId("aggregate-toggle").click();
    await expect(bar).toHaveAttribute("data-open", "true");

    // "정리…" opens the organise screen scoped to the selection; "내보내기…" the export tab.
    await page.getByTestId("library-organize").click();
    await expect(page.getByTestId("page-organize")).toBeVisible();
    await expect(page.getByTestId("organize-scope-mode")).toContainText(tpl(ko.organize.scope.selection, { count: 1 }));
    await page.getByTestId("nav-library").click();
    await expect(page.getByTestId(`doc-select-${receipt}`)).toBeChecked();
    await page.getByTestId("library-export").click();
    await expect(page.getByTestId("export-tab")).toBeVisible();
    await expect(page.getByTestId("export-scope-mode")).toContainText(tpl(ko.organize.scope.selection, { count: 1 }));
  });
});
