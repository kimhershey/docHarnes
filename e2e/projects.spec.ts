import { expect, test, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { card, createWorkspace, docId, freshApp, importViaLibrary, tpl } from "./helpers";

// P6 프로젝트 (design §3.15) against the browser mock: project from a template → documents from
// the library (checklist) → natural-language rules confirmed in the grid → deterministic check with
// verdict chips and evidence → 보완요청서 draft job handed to 문서 생성 → kanban move, plus the assistant's
// `check_consistency` plan. Real rule parsing / checking is covered by the Rust tests.

/** Imports and extracts the 내부 개발 trio: requirements specification, WBS and test report. */
async function importTrio(page: Page): Promise<{ srs: string; wbs: string; test: string }> {
  await importViaLibrary(page, ["D:\\scan\\요구사항정의서-v1.2.pdf", "D:\\scan\\WBS-한빛포털.xlsx", "D:\\scan\\테스트결과서-1.2.pdf"], { autoLevel: "none" });
  const srs = await docId(page, "요구사항정의서-v1.2.pdf");
  const wbs = await docId(page, "WBS-한빛포털.xlsx");
  const test = await docId(page, "테스트결과서-1.2.pdf");
  for (const id of [srs, wbs, test]) await page.getByTestId(`doc-select-${id}`).check();
  await page.getByTestId("library-process").click();
  await page.getByTestId("library-process-standard").click();
  // High-confidence documents are promoted to 검토 완료 straight away; the test report keeps a low-confidence field.
  for (const name of ["요구사항정의서-v1.2.pdf", "WBS-한빛포털.xlsx", "테스트결과서-1.2.pdf"]) {
    await expect(card(page, name)).toHaveAttribute("data-doc-status", /extracted|reviewed/, { timeout: 15_000 });
  }
  for (const id of [srs, wbs, test]) await page.getByTestId(`doc-select-${id}`).uncheck();
  return { srs, wbs, test };
}

test.describe("P6 projects", () => {
  test("template → documents → NL rule grid → check → supplement request → kanban", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    const { srs, wbs, test } = await importTrio(page);

    // New project from the 내부 개발 template.
    await page.getByTestId("nav-projects").click();
    await expect(page.getByTestId("page-projects")).toBeVisible();
    await expect(page.getByTestId("projects-empty")).toBeVisible();
    await page.getByTestId("projects-new").click();
    await page.getByTestId("project-name").fill("한빛 포털 구축");
    await page.getByTestId("project-template").click();
    await page.getByRole("option", { name: ko.projects.templates.builtin.internal_dev }).click();
    await page.getByTestId("project-intake").fill("2026-001");
    await page.getByTestId("project-submit").click();
    await expect(page.getByTestId("page-project")).toBeVisible();
    await expect(page.getByTestId("project-title")).toHaveText("한빛 포털 구축");
    // The template's checklist starts with the three required types missing.
    await expect(page.getByTestId("checklist-srs")).toHaveAttribute("data-missing", "true");
    await expect(page.getByTestId("checklist-wbs")).toHaveAttribute("data-missing", "true");
    await expect(page.getByTestId("checklist-test_report")).toHaveAttribute("data-missing", "true");

    // Add the three documents from the library: the checklist is complete.
    await page.getByTestId("project-add-documents").click();
    await page.getByTestId(`project-add-doc-${srs}`).check();
    await page.getByTestId(`project-add-doc-${wbs}`).check();
    await page.getByTestId(`project-add-doc-${test}`).check();
    await page.getByTestId("project-add-done").click();
    await expect(page.getByTestId("project-documents-table").locator("tbody tr")).toHaveCount(3);
    await expect(page.getByTestId("checklist-srs")).toHaveAttribute("data-missing", "false");
    await expect(page.getByTestId("checklist-wbs")).toHaveAttribute("data-missing", "false");
    await expect(page.getByTestId("checklist-test_report")).toHaveAttribute("data-missing", "false");

    // Rules: the template's three rules, then two sentences converted in the grid.
    await page.getByTestId("project-tab-rules").click();
    await expect(page.getByTestId("rule-list").locator('[data-testid^="rule-"][data-rule-enabled]')).toHaveCount(3);
    await page.getByTestId("rule-nl-input").fill("테스트 결과서의 사업명은 요구사항 정의서의 사업명과 같아야 해");
    await page.getByTestId("rule-define").click();
    const grid = page.getByTestId("rule-grid");
    await expect(grid).toBeVisible({ timeout: 10_000 });
    await expect(grid.getByTestId("rule-grid-left-type")).toContainText(ko.documents.doc_types.test_report);
    await expect(grid.getByTestId("rule-grid-op")).toContainText(ko.projects.rules.ops.eq);
    await expect(grid.getByTestId("rule-grid-right-type")).toContainText(ko.documents.doc_types.srs);
    await expect(grid.getByTestId("rule-grid-left-operand")).toHaveAttribute("data-unresolved", "false");
    await page.getByTestId("rule-save").click();
    await expect(page.getByTestId("rule-list").locator('[data-testid^="rule-"][data-rule-enabled]')).toHaveCount(4);
    // A literal comparison the documents fail (the report's pass rate is 92).
    await page.getByTestId("rule-nl-input").fill("테스트 결과서의 통과율은 95 이상이어야 해");
    await page.getByTestId("rule-define").click();
    await expect(grid).toBeVisible({ timeout: 10_000 });
    await expect(grid.getByTestId("rule-grid-op")).toContainText(ko.projects.rules.ops.gte);
    await expect(grid.getByTestId("rule-grid-right-literal")).toHaveValue("95");
    await page.getByTestId("rule-save").click();
    await expect(page.getByTestId("rule-list").locator('[data-testid^="rule-"][data-rule-enabled]')).toHaveCount(5);

    // Check: four rules pass on the shared 한빛 포털 구축 values, the literal one mismatches.
    await page.getByTestId("project-check-header").click();
    await expect(page.getByTestId("project-verdicts")).toBeVisible({ timeout: 10_000 });
    await expect(page.getByTestId("verdict-pass")).toContainText("4");
    await expect(page.getByTestId("verdict-mismatch")).toContainText("1");
    await expect(page.getByTestId("project-results-table").locator('tr[data-verdict="mismatch"]')).toHaveCount(1);
    await expect(page.getByTestId("project-results-table").getByTestId("result-evidence").first()).toBeVisible();
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.projects.detail.checked, { pass: 4, mismatch: 1, undetermined: 0, missing: 0 }));

    // Relations: the shared project name links the documents.
    await page.getByTestId("project-tab-relations").click();
    await expect(page.getByTestId("project-relations").locator('[data-testid="relation-edge"][data-kind="name"]').first()).toBeVisible();

    // Report tab renders the Markdown summary.
    await page.getByTestId("project-tab-report").click();
    await expect(page.getByTestId("project-report-body")).toContainText("한빛 포털 구축", { timeout: 10_000 });

    // 보완요청서: the mismatch is pre-selected; the draft job ends on the 문서 생성 screen.
    await page.getByTestId("project-supplement").click();
    await expect(page.getByTestId("supplement-dialog")).toBeVisible();
    await expect(page.getByTestId("supplement-findings").locator("input:checked")).toHaveCount(1);
    await page.getByTestId("supplement-generate").click();
    await expect(page.getByTestId("page-generate")).toBeVisible({ timeout: 20_000 });
    await expect(page.getByTestId("generate-title")).toHaveValue(/보완요청서/);
    await expect(page.getByTestId("generate-table-findings").locator('[data-testid^="generate-row-findings-"]')).toHaveCount(1);
    await expect(page.getByTestId("generate-field-input-project_name")).toHaveValue("한빛 포털 구축");

    // Kanban: the project moved to 보완요청 with the request; 완료 with an open mismatch is confirmed first (counts shown).
    await page.getByTestId("nav-projects").click();
    const kanbanCard = page.locator('[data-testid^="project-card-"][data-project-name="한빛 포털 구축"]');
    await expect(kanbanCard).toHaveAttribute("data-project-status", "supplement");
    await expect(page.getByTestId("kanban-supplement")).toContainText("한빛 포털 구축");
    await kanbanCard.getByTestId("project-move-next").click();
    const confirm = page.getByTestId("confirm-dialog");
    await expect(confirm).toBeVisible();
    await expect(confirm).toContainText(tpl(ko.projects.card.done_confirm, { mismatch: 1, missing: 0 }));
    await page.getByTestId("confirm-cancel").click();
    await expect(kanbanCard).toHaveAttribute("data-project-status", "supplement");
    await kanbanCard.getByTestId("project-move-next").click();
    await page.getByTestId("confirm-ok").click();
    await expect(kanbanCard).toHaveAttribute("data-project-status", "done", { timeout: 10_000 });
    await expect(page.getByTestId("kanban-done")).toContainText("한빛 포털 구축");
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.projects.card.moved, { name: "한빛 포털 구축", status: ko.projects.status.done }));
    await expect(kanbanCard.getByTestId("project-verdicts")).toContainText(tpl(ko.projects.card.verdicts, { pass: 4, mismatch: 1, undetermined: 0 }));
    // The previous step needs no confirmation; the buttons name the project and the step.
    await expect(kanbanCard.getByTestId("project-move-prev")).toHaveAttribute("aria-label", tpl(ko.projects.card.move_prev_named, { name: "한빛 포털 구축", status: ko.projects.status.supplement }));
    await kanbanCard.getByTestId("project-move-prev").click();
    await expect(kanbanCard).toHaveAttribute("data-project-status", "supplement", { timeout: 10_000 });
    // List view shows the same project; the name is a keyboard-reachable control.
    await page.getByTestId("projects-view-list").click();
    await expect(page.getByTestId("projects-table")).toContainText("2026-001");
    await page.getByTestId("project-row-open").first().focus();
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("page-project")).toBeVisible();

    // Rules: a rule can be switched off and deleted from the list.
    await page.getByTestId("project-tab-rules").click();
    const rules = page.getByTestId("rule-list").locator('[data-testid^="rule-"][data-rule-enabled]');
    await expect(rules).toHaveCount(5);
    await rules.first().getByTestId("rule-enabled").click();
    await expect(rules.first()).toHaveAttribute("data-rule-enabled", "false", { timeout: 10_000 });
    await rules.last().getByTestId("rule-delete").click();
    await expect(rules).toHaveCount(4, { timeout: 10_000 });
    await expect(page.getByTestId("toaster")).toContainText(ko.projects.rules.deleted);
  });

  test("auto-classify: keywords and names suggest projects, the user adjusts the preview, apply assigns", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\개발계약서-한빛.pdf", "D:\\scan\\요구사항정의서-v1.2.pdf", "D:\\scan\\견적서-1p.jpg"], { autoLevel: "none" });
    const invoice = await docId(page, "개발계약서-한빛.pdf");
    const registration = await docId(page, "요구사항정의서-v1.2.pdf");
    const receipt = await docId(page, "견적서-1p.jpg");
    // Two projects: one with a keyword that matches the contract's file name, one unrelated.
    await page.getByTestId("nav-projects").click();
    await page.getByTestId("projects-new").click();
    await page.getByTestId("project-name").fill("한빛상사 거래처 등록");
    await page.getByTestId("project-keywords").fill("개발계약서, 한빛");
    await page.getByTestId("project-submit").click();
    await expect(page.getByTestId("page-project")).toBeVisible();
    await page.getByTestId("project-back").click();
    await page.getByTestId("projects-new").click();
    await page.getByTestId("project-name").fill("신사옥 공사");
    await page.getByTestId("project-submit").click();
    await expect(page.getByTestId("page-project")).toBeVisible();
    await page.getByTestId("project-back").click();
    const hanbit = page.locator('[data-testid^="project-card-"][data-project-name="한빛상사 거래처 등록"]');
    await expect(hanbit).toBeVisible();
    const hanbitId = (await hanbit.getAttribute("data-testid"))!.replace("project-card-", "");

    // From the library: the selection goes to the projects screen with the dialog open on those documents.
    await page.getByTestId("nav-library").click();
    for (const id of [invoice, registration, receipt]) await page.getByTestId(`doc-select-${id}`).check();
    await page.getByTestId("library-classify").click();
    const dialog = page.getByTestId("classify-dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId("classify-scope")).toContainText("3");
    await dialog.getByTestId("classify-run").click();
    await expect(dialog.getByTestId("classify-summary")).toContainText(tpl(ko.projects.classify.summary, { total: 3, suggested: 1, undecided: 2 }));
    // The invoice is suggested for 한빛상사 (keyword in the file name); the others stay undecided.
    await expect(dialog.getByTestId(`classify-row-${invoice}`)).toHaveAttribute("data-suggested", hanbitId);
    await expect(dialog.getByTestId(`classify-row-${invoice}`)).toHaveAttribute("data-source", "rules");
    await expect(dialog.getByTestId(`classify-confidence-${invoice}`)).toContainText("%");
    await expect(dialog.getByTestId(`classify-row-${registration}`)).toHaveAttribute("data-suggested", "");
    await expect(dialog.getByTestId(`classify-confidence-${receipt}`)).toHaveText(ko.projects.classify.undecided);
    // The user puts the registration into 한빛상사 by hand and leaves the receipt out: two assignments.
    await dialog.getByTestId(`classify-project-${registration}`).click();
    await page.getByTestId(`classify-project-${registration}-${hanbitId}`).click();
    await expect(dialog.getByTestId("classify-apply")).toHaveText(tpl(ko.projects.classify.apply, { count: 2 }));
    await dialog.getByTestId("classify-apply").click();
    await expect(dialog).toBeHidden();
    await expect(hanbit.getByText(tpl(ko.projects.card.documents, { count: 2 }))).toBeVisible();

    // From the projects screen: only the unassigned receipt is left; nothing matches it.
    await page.getByTestId("projects-classify").click();
    await expect(page.getByTestId("classify-scope")).toHaveText(ko.projects.classify.scope_unassigned);
    await page.getByTestId("classify-run").click();
    await expect(page.getByTestId("classify-summary")).toContainText(tpl(ko.projects.classify.summary, { total: 1, suggested: 0, undecided: 1 }));
    await expect(page.getByTestId(`classify-row-${receipt}`)).toBeVisible();
    await expect(page.getByTestId("classify-apply")).toBeDisabled();
  });

  test("templates: a built-in is read-only, its copy is editable, an incomplete rule blocks saving", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-projects").click();
    await page.getByTestId("projects-templates").click();
    const dialog = page.getByTestId("templates-dialog");
    await expect(dialog).toBeVisible();
    await dialog.getByTestId("templates-item-builtin-maintenance").click();
    await expect(dialog.getByTestId("templates-editor")).toContainText(ko.projects.templates.readonly);
    await expect(dialog.getByTestId("templates-save")).toBeHidden();
    await dialog.getByTestId("templates-duplicate").click();
    await expect(dialog.getByTestId("templates-name")).toHaveValue(`${ko.projects.templates.builtin.maintenance} ${ko.projects.templates.copy_suffix}`);
    await expect(dialog.getByTestId("templates-save")).toBeEnabled();
    // A new, empty rule cannot be saved: the row says why and the save button is off until it is completed or removed.
    await dialog.getByTestId("templates-add-rule").click();
    await expect(dialog.getByTestId("templates-rule-incomplete-3")).toHaveText(ko.projects.rules.incomplete);
    await expect(dialog.getByTestId("templates-incomplete")).toHaveText(tpl(ko.projects.templates.incomplete_rules, { count: 1 }));
    await expect(dialog.getByTestId("templates-save")).toBeDisabled();
    await dialog.getByTestId("templates-rule-row-3").getByRole("button", { name: ko.projects.rules.delete }).click();
    await expect(dialog.getByTestId("templates-incomplete")).toBeHidden();
    await dialog.getByTestId("templates-save").click();
    await expect(page.getByTestId("toaster")).toContainText(ko.projects.templates.saved);
    await expect(dialog.locator('[data-testid^="templates-item-"]')).toHaveCount(4);
  });

  test("assistant: '정합성 검사해' runs the check of the open project and links to the results", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-projects").click();
    await page.getByTestId("projects-new").click();
    await page.getByTestId("project-name").fill("인증 심사 A");
    await page.getByTestId("project-template").click();
    // 유지보수: three rules over three missing document types → three 누락 verdicts.
    await page.getByRole("option", { name: ko.projects.templates.builtin.maintenance }).click();
    await page.getByTestId("project-submit").click();
    await expect(page.getByTestId("page-project")).toBeVisible();

    // The opened project stays the assistant's default target after leaving its page: the scope row shows it
    // as a chip, the plan runs inline and the result card links back.
    await page.getByTestId("nav-assistant").click();
    await expect(page.getByTestId("assistant-current-project")).toContainText(tpl(ko.assistant.scope.project_named, { name: "인증 심사 A" }));
    await page.getByTestId("assistant-input").fill("정합성 검사해");
    await page.getByTestId("assistant-send").click();
    const result = page.locator('[data-testid="assistant-card-result"]').last();
    await expect(result).toBeVisible({ timeout: 15_000 });
    await expect(result).toContainText(ko.planner.ops.check_consistency);
    await expect(result.getByTestId("result-check")).toContainText(ko.projects.results.verdict.missing);
    await result.getByTestId("result-open-check").click();
    await expect(page.getByTestId("page-project")).toBeVisible();
    await expect(page.getByTestId("project-verdicts")).toBeVisible();
    await expect(page.getByTestId("verdict-missing")).toContainText("3");
    // The chip's clear button drops the project context (the validator then asks for a project).
    await page.getByTestId("nav-assistant").click();
    await page.getByTestId("assistant-current-project-clear").click();
    await expect(page.getByTestId("assistant-current-project")).toBeHidden();
  });
});
