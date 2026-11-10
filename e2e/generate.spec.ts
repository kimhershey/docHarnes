import { expect, test, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { createWorkspace, docId, freshApp, importViaLibrary, tpl } from "./helpers";

// P6 문서 생성 (design §3.13) against the browser mock: template → content → draft job → preview →
// field edit → validator suggestion → validate → save with library registration → saved-draft list,
// and the assistant's `draft_document` plan handing its draft over to the screen. The real model
// call, the HWPX / DOCX / PDF writers and the form fill are covered by the Rust tests.

async function openGenerate(page: Page) {
  await page.getByTestId("nav-generate").click();
  await expect(page.getByTestId("page-generate")).toBeVisible();
}

test.describe("P6 document generation", () => {
  test("quotation: generate → preview → apply the sum suggestion → validate → save → drafts list", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openGenerate(page);
    await expect(page.getByTestId("generate-empty")).toBeVisible();

    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.quotation }).click();
    await page.getByTestId("generate-content").fill("㈜한빛에 노트북 10대, 대당 1,200,000원, 유효기간 30일 견적서");
    await page.getByTestId("generate-run").click();

    // The draft job's progress is announced, then the preview and the editor appear.
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("generate-preview-frame")).toHaveAttribute("data-loaded", "true", { timeout: 10_000 });
    await expect(page.getByTestId("generate-title")).toHaveValue(/견적서/);
    await expect(page.getByTestId("generate-field-input-recipient")).toHaveValue("㈜한빛");
    await expect(page.getByTestId("generate-row-items-0")).toBeVisible();
    await expect(page.getByTestId("generate-cell-items-0-item_name")).toHaveValue("노트북");
    await expect(page.getByTestId("generate-cell-items-0-quantity")).toHaveValue("10");
    // The mock model wrote the total with VAT: the validator flags the sum and offers the item sum.
    await expect(page.getByTestId("generate-review-badge")).toHaveText(ko.generate.preview.unreviewed);
    await expect(page.getByTestId("generate-field-input-total")).toHaveValue("13,200,000");
    const issue = page.getByTestId("generate-field-total").getByTestId("generate-issue-sum_mismatch");
    await expect(issue).toBeVisible();

    // Editing a field marks it as user-edited.
    await page.getByTestId("generate-field-input-note").fill("부가세 별도");
    await expect(page.getByTestId("generate-field-note")).toContainText(ko.generate.editor.user_edited);

    // 제안 적용 fixes the total and drops the issue; 검증 confirms a clean draft.
    await page.getByTestId("generate-field-total").getByTestId("generate-apply-suggestion").click();
    await expect(page.getByTestId("generate-field-input-total")).toHaveValue("12,000,000");
    await expect(issue).toBeHidden();
    await page.getByTestId("generate-validate").click();
    await expect(page.getByTestId("generate-issues-ok")).toBeVisible({ timeout: 10_000 });
    await expect(page.getByTestId("generate-review-badge")).toHaveText(ko.generate.preview.validated);

    // Save as HWPX with library registration → toast with "라이브러리에서 보기" and the saved path.
    await page.getByTestId("generate-save").click();
    await expect(page.getByTestId("generate-save-dialog")).toBeVisible();
    await expect(page.getByTestId("generate-save-name")).toHaveValue(/^\d{4}-\d{2}-\d{2}_/);
    await page.getByTestId("generate-save-confirm").click();
    await expect(page.getByTestId("generate-saved")).toContainText(/^documents\/generated\/.+\.hwpx/, { timeout: 10_000 });
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.generate.save.saved, { path: "" }).trim());
    await expect(page.getByTestId("toast-action")).toHaveText(ko.generate.save.open_library);
    await expect(page.getByTestId("generate-saved-library")).toBeVisible();

    // Saving again under the same (default) name is a conflict until "덮어쓰기" is on; the overwrite keeps one file / one document.
    await page.getByTestId("generate-save").click();
    await expect(page.getByTestId("generate-save-dialog")).toBeVisible();
    await page.getByTestId("generate-save-confirm").click();
    await expect(page.getByTestId("generate-save-error")).toHaveText(ko.generate.save.exists);
    await page.getByTestId("generate-save-overwrite").click();
    await page.getByTestId("generate-save-confirm").click();
    await expect(page.getByTestId("generate-save-dialog")).toBeHidden({ timeout: 10_000 });
    await expect(page.getByTestId("generate-saved")).not.toContainText("_2");

    // The saved-draft list shows the draft with its path.
    await page.getByTestId("generate-drafts-open").click();
    const drafts = page.getByTestId("generate-drafts");
    await expect(drafts).toBeVisible();
    await expect(drafts.locator('[data-testid^="generate-draft-row-"]')).toHaveCount(1);
    await expect(drafts).toContainText("documents/generated/");
    await page.keyboard.press("Escape");

    // The registered document is in the library as a generated document (the overwrite re-used it: still one card).
    await page.getByTestId("generate-saved-library").click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();
    await page.getByTestId("nav-library").click();
    await expect(page.locator('[data-testid^="doc-card-"]')).toHaveCount(1);
  });

  test("regeneration: feedback and an excluded quoted document are applied to the next draft", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\개발계약서-2p.pdf"], { autoLevel: "none" });
    const invoice = await docId(page, "개발계약서-2p.pdf");
    await openGenerate(page);

    // Quote the imported document: it shows up as a reference of the draft.
    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.official_letter }).click();
    await page.getByTestId("generate-content").fill("㈜한빛소프트에 개발 계약 이행 현황 확인을 요청하는 협조문");
    await page.getByTestId("generate-pick-sources").click();
    await expect(page.getByTestId("generate-sources-dialog")).toBeVisible();
    await page.getByTestId(`generate-source-${invoice}`).check();
    await page.getByTestId("generate-sources-done").click();
    await expect(page.getByTestId("generate-sources").locator("li")).toHaveCount(1);
    await page.getByTestId("generate-run").click();
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 15_000 });
    const references = page.getByTestId("generate-references").getByTestId("generate-reference");
    await expect(references).toHaveCount(1);
    await expect(references.first()).toHaveAttribute("data-excluded", "false");
    await expect(references.first()).toContainText("개발계약서-2p.pdf");

    // Exclude the reference, give feedback, regenerate: the new draft drops the document and records the instruction.
    await page.getByTestId("generate-reference-exclude").first().check();
    await expect(references.first()).toHaveAttribute("data-excluded", "true");
    await expect(page.getByTestId("generate-regenerate-panel")).toContainText(tpl(ko.generate.form.excluded_hint, { count: 1 }));
    await page.getByTestId("generate-feedback").fill("더 짧게");
    await page.getByTestId("generate-regenerate").click();
    await expect(page.getByTestId("generate-notes")).toContainText("재생성 반영: 더 짧게", { timeout: 15_000 });
    await expect(references).toHaveCount(0);
    await expect(page.getByTestId("generate-references")).toContainText(ko.generate.editor.no_references);
    // The instruction box is cleared for the next round; the editor is usable again.
    await expect(page.getByTestId("generate-feedback")).toHaveValue("");
    await expect(page.getByTestId("generate-editor")).toHaveAttribute("data-disabled", "false");
  });

  test("assistant: '견적서 만들어줘 …' → plan card → draft job → result card opens the draft here", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-assistant").click();
    await page.getByTestId("assistant-input").fill("㈜두리에 모니터 3대 대당 350,000원 견적서 만들어줘");
    await page.getByTestId("assistant-send").click();
    const plan = page.locator('[data-testid="assistant-card-plan"]').last();
    await expect(plan).toBeVisible({ timeout: 15_000 });
    await expect(plan).toHaveAttribute("data-plan-status", "pending");
    await expect(plan).toContainText(ko.planner.ops.draft_document);
    await expect(plan.getByTestId("plan-impact")).toContainText(tpl(ko.planner.card.impact_draft, { count: 1 }));
    await plan.getByTestId("plan-execute").click();
    await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    const result = page.locator('[data-testid="assistant-card-result"]').last();
    await expect(result).toBeVisible({ timeout: 20_000 });
    await expect(result).toHaveAttribute("data-result-status", "done");
    await expect(result.getByTestId("result-draft")).toContainText("견적서");
    await result.getByTestId("result-open-draft").click();
    await expect(page.getByTestId("page-generate")).toBeVisible();
    await expect(page.getByTestId("toaster")).toContainText(ko.generate.handoff);
    await expect(page.getByTestId("generate-title")).toHaveValue(/견적서/);
    await expect(page.getByTestId("generate-field-input-recipient")).toHaveValue("㈜두리");
    await expect(page.getByTestId("generate-cell-items-0-item_name")).toHaveValue("모니터");

    // The Jobs page lists the draft job with its result and a way back to the screen.
    await page.getByTestId("nav-jobs").click();
    await page.getByTestId("jobs-tab-finished").click();
    const job = page.locator('[data-job-kind="plan"]').first();
    await expect(job).toContainText(ko.jobs.kind.plan);
    await job.getByTestId("job-result").locator("summary").click();
    await expect(job.getByTestId("job-result-steps")).toContainText(ko.planner.ops.draft_document);
  });
});
