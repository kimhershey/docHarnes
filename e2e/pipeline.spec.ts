import { expect, test } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { card, createWorkspace, docId, freshApp, importViaLibrary, openSettingsTab, tpl, tplPattern } from "./helpers";

// P4 acceptance (design §4.1): bulk processing (classify → OCR → extract) →
// review flags on the card and in the viewer → keyboard review queue →
// document promoted to "reviewed"; schemas (NL creation, form
// registration, type mapping) and the processing settings — against the
// browser mock backend. Real VLM calls, validators and the mirror are covered
// by the Rust tests.

/** Imports the estimate without auto-processing, then runs 표준 처리 on it from the library bulk bar. */
async function processEstimate(page: Parameters<typeof freshApp>[0]) {
  await importViaLibrary(page, ["D:\\scan\\견적서-1p.jpg"], { autoLevel: "none" });
  await page.getByTestId(`doc-select-${await docId(page, "견적서-1p.jpg")}`).check();
  await page.getByTestId("library-process").click();
  await page.getByTestId("library-process-standard").click();
  await expect(card(page, "견적서-1p.jpg")).toHaveAttribute("data-doc-status", "extracted", { timeout: 15_000 });
}

test.describe("P4 pipeline", () => {
  test("bulk processing extracts fields, flags low-confidence values and the viewer review promotes the document", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서-1p.jpg", "D:\\scan\\개발계약서-2p.pdf"], { autoLevel: "none" });

    // Select both cards → 처리 ▾ → 표준 처리.
    for (const name of ["견적서-1p.jpg", "개발계약서-2p.pdf"]) {
      await page.getByTestId(`doc-select-${await docId(page, name)}`).check();
    }
    await page.getByTestId("library-process").click();
    await page.getByTestId("library-process-standard").click();
    await expect(card(page, "견적서-1p.jpg")).toHaveAttribute("data-doc-status", "extracted", { timeout: 15_000 });
    await expect(card(page, "견적서-1p.jpg")).toContainText(ko.documents.doc_types.estimate);
    await expect(card(page, "견적서-1p.jpg").getByTestId(/doc-review-/)).toHaveText(ko.library.review_pending);
    await expect(card(page, "개발계약서-2p.pdf")).toContainText(ko.documents.doc_types.dev_contract);

    // Jobs page shows the Korean summary of the pipeline job.
    await page.getByTestId("nav-jobs").click();
    await page.getByTestId("jobs-tab-finished").click();
    const job = page.locator('[data-job-kind="pipeline"]').first();
    await expect(job).toContainText(ko.jobs.kind.pipeline);
    await expect(job.getByTestId("job-detail")).toContainText(tplPattern(ko.jobs.detail.process_summary, { processed: 2, classified: 2, extracted: 2, failed: 0 }));

    // Viewer: the OCR text sits on the page image (estimated positions for a photo); the right pane
    // only has AI 검색 and 이력 — the review itself happens in the queue.
    await page.getByTestId("nav-library").click();
    await card(page, "견적서-1p.jpg").getByTestId(/doc-open-/).click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();
    await expect(page.getByTestId("viewer-status")).toHaveText(ko.documents.status.extracted);
    await expect(page.getByTestId("viewer-review-badge")).toHaveText(ko.viewer.review_badge.pending);
    await expect(page.getByTestId("viewer-text-layer")).toContainText("㈜한빛소프트");
    await expect(page.getByTestId("viewer-text-layer")).toContainText("백엔드 개발");
    await expect(page.getByTestId("viewer-layer-estimated")).toBeVisible();
    await expect(page.getByTestId("viewer-tab-fields")).toHaveCount(0);

    // Review queue: correct the validity date, approve the rest of the estimate → reviewed.
    await page.getByTestId("nav-review").click();
    await expect(page.getByTestId("page-review")).toBeVisible();
    const receiptItems = page.locator('[data-testid^="review-item-"]').filter({ hasText: "견적서-1p.jpg" });
    await receiptItems.filter({ hasText: "유효기간" }).first().click();
    await expect(page.getByTestId("review-current-title")).toContainText("유효기간");
    await page.getByTestId("review-edit").click();
    await page.getByTestId("review-edit-input").fill("2026-12-31");
    await page.getByTestId("review-edit-save").click();
    await expect(page.getByTestId("review-edit-dialog")).toHaveCount(0);
    await receiptItems.first().click();
    await page.getByTestId("review-approve-doc").click();
    await expect(receiptItems).toHaveCount(0);

    // Back in the viewer the document is reviewed and the 이력 tab records the correction.
    await page.getByTestId("nav-library").click();
    await card(page, "견적서-1p.jpg").getByTestId(/doc-open-/).click();
    await expect(page.getByTestId("viewer-status")).toHaveText(ko.documents.status.reviewed);
    await expect(page.getByTestId("viewer-review-badge")).toHaveText(ko.viewer.review_badge.done);
    await page.getByTestId("viewer-tab-history").click();
    const change = page.getByTestId("history-change").filter({ hasText: "valid_until" }).first();
    await expect(change).toContainText(ko.pipeline.field_status.corrected);
    await expect(change).toContainText("2026-12-31");
  });

  test("review queue: keyboard approval, editing and the reason filter", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await processEstimate(page);

    await page.getByTestId("nav-review").click();
    await expect(page.getByTestId("page-review")).toBeVisible();
    const items = page.locator('[data-testid^="review-item-"]');
    // The queue query resolves after the page mounts: wait for the rows before counting them.
    await expect(items.first()).toBeVisible();
    const initial = await items.count();
    expect(initial).toBeGreaterThanOrEqual(2);
    await expect(page.getByTestId("review-current-confidence")).toBeVisible();

    // Enter approves the current item and the queue shrinks.
    await page.getByTestId("page-review").focus();
    await page.keyboard.press("Enter");
    await expect(items).toHaveCount(initial - 1);

    // E opens the editor; Enter saves the correction.
    await page.keyboard.press("e");
    await expect(page.getByTestId("review-edit-dialog")).toBeVisible();
    await page.getByTestId("review-edit-input").fill("수정값");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("review-edit-dialog")).toBeHidden();
    await expect(items).toHaveCount(initial - 2);

    // Reason filter narrows the list; "approve document" empties it.
    await page.getByTestId("review-filter-reason").click();
    await page.getByRole("option", { name: ko.pipeline.reason.low_confidence }).click();
    await expect(page.getByTestId("review-count")).toBeVisible();
    await page.getByTestId("review-approve-doc").click();
    await expect(page.getByTestId("review-empty")).toBeVisible();
    await page.getByTestId("nav-library").click();
    await expect(card(page, "견적서-1p.jpg")).toHaveAttribute("data-doc-status", "reviewed");
  });

  test("review edit dialog: Enter during a Korean IME composition neither saves nor approves", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await processEstimate(page);

    await page.getByTestId("nav-review").click();
    await expect(page.getByTestId("page-review")).toBeVisible();
    const items = page.locator('[data-testid^="review-item-"]');
    await expect(items.first()).toBeVisible();
    const initial = await items.count();
    await page.getByTestId("page-review").focus();
    await page.keyboard.press("e");
    const dialog = page.getByTestId("review-edit-dialog");
    await expect(dialog).toBeVisible();
    const input = page.getByTestId("review-edit-input");
    await input.fill("수정");

    // While a composition is open the browser reports Enter with isComposing = true / keyCode 229
    // (the IME commits the syllable); that Enter must neither save the value nor approve the item.
    await input.dispatchEvent("compositionstart");
    await input.dispatchEvent("keydown", { key: "Enter", code: "Enter", keyCode: 229, isComposing: true });
    await expect(dialog).toBeVisible();
    await expect(input).toHaveValue("수정");
    await expect(items).toHaveCount(initial);

    // After the composition ends, Enter saves the correction and the item leaves the queue.
    await input.dispatchEvent("compositionend");
    await page.keyboard.press("Enter");
    await expect(dialog).toBeHidden();
    await expect(items).toHaveCount(initial - 1);
  });

  test("schemas: natural-language creation, form registration from the library and the type mapping", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\forms\\견적서 양식.hwpx", "D:\\scan\\견적서-1p.jpg"], { autoLevel: "none" });

    // Built-in samples are listed; NL creation opens a draft in the editor.
    await openSettingsTab(page, "schemas");
    await expect(page.getByTestId("page-schemas")).toBeVisible();
    // 13 software forms are listed; the 6 business forms of 0.1 stay hidden (disabled) behind a disclosure.
    await expect(page.locator('[data-testid^="schema-row-builtin-"]:visible')).toHaveCount(13);
    await expect(page.locator('[data-testid="schema-row-builtin-estimate"]')).toHaveAttribute("data-schema-enabled", "true");
    const hiddenBuiltins = page.getByTestId("schemas-hidden-builtins");
    await expect(hiddenBuiltins).toContainText(tpl(ko.schemas.list.hidden, { count: 6 }));
    await hiddenBuiltins.locator("summary").click();
    await expect(page.locator('[data-testid="schema-row-builtin-tax_invoice"]')).toBeVisible();
    await expect(page.locator('[data-testid="schema-row-builtin-tax_invoice"]')).toHaveAttribute("data-schema-enabled", "false");
    await expect(page.locator('[data-testid^="schema-row-builtin-"]')).toHaveCount(19);
    await page.getByTestId("schemas-nl-create").click();
    await page.getByTestId("nl-schema-text").fill("발주서. 발주번호, 거래처, 납기일자, 합계금액, 품목");
    await page.getByTestId("nl-schema-generate").click();
    await expect(page.getByTestId("schema-editor")).toBeVisible();
    await expect(page.getByTestId("schema-doc-type")).toHaveValue("purchase_order");
    await expect(page.locator('[data-testid="schema-field"]')).toHaveCount(5);
    await expect(page.getByTestId("schema-lint")).toContainText(ko.schemas.editor.lint_ok);
    await page.getByTestId("schema-save").click();
    await expect(page.locator('[data-schema-doc-type="purchase_order"]')).toHaveCount(1);
    await expect(page.getByTestId("schemas-mapping")).toContainText("purchase_order");

    // Grammar lint flags a bad key and blocks saving.
    await page.getByTestId("schema-add-field").click();
    await page.locator('[data-testid="schema-field"]').last().getByTestId("schema-field-key").fill("Bad Key");
    await expect(page.getByTestId("schema-lint")).toContainText("snake_case");
    await page.locator('[data-testid="schema-field"]').last().getByTestId("schema-field-remove").click();

    // Form registration from the library (0.3.0 §2): two samples → one discovery job → the merged draft opens
    // unsaved. The blank HWPX form contributes 8 printed labels, the filled scan 9 (사업명·공급자 상호 instead of
    // 비고): the union is 10 fields, the 7 common ones required.
    await page.getByTestId("nav-library").click();
    for (const name of ["견적서 양식.hwpx", "견적서-1p.jpg"]) await page.getByTestId(`doc-select-${await docId(page, name)}`).check();
    await page.getByTestId("library-register-form").click();
    await expect(page.getByTestId("form-register-dialog")).toBeVisible();
    await expect(page.getByTestId("form-register-doc")).toContainText("견적서 양식.hwpx");
    await expect(page.getByTestId("form-register-doc")).toContainText("견적서-1p.jpg");
    await page.getByTestId("form-register-name").fill("견적서");
    await page.getByTestId("form-register-derive").click();
    await expect(page.getByTestId("schema-editor")).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("schema-discovery-summary")).toContainText(tplPattern(ko.schemas.form.discovery_summary, { docs: 2, fields: 10, dropped: 0 }));
    await expect(page.locator('[data-testid="schema-field"]')).toHaveCount(10);
    // 견적서 is a software type: the form joins the built-in estimate schema as a second schema of the type.
    await expect(page.getByTestId("schema-doc-type")).toHaveValue("estimate");
    // The built-in sample is already called 견적서: the popup proposes "견적서 (2)" (decision 2026-11-10).
    const nameTaken = page.getByTestId("schema-name-taken");
    await expect(nameTaken).toBeVisible();
    await expect(nameTaken.getByTestId("schema-name-taken-input")).toHaveValue("견적서 (2)");
    await nameTaken.getByTestId("schema-name-taken-apply").click();
    await expect(page.getByTestId("schema-name")).toHaveValue("견적서 (2)");
    await expect(page.getByTestId("schema-save")).toBeEnabled();
    // Typing the taken name back disables saving with an explanation; a free name re-enables it.
    await page.getByTestId("schema-name").fill("견적서");
    await expect(page.getByTestId("schema-name-error")).toHaveText(ko.schemas.editor.name_taken);
    await expect(page.getByTestId("schema-save")).toBeDisabled();
    await page.getByTestId("schema-name").fill("견적서 (2)");
    await expect(page.getByTestId("schema-name-error")).toHaveCount(0);
    await expect(page.getByTestId("schema-save")).toBeEnabled();
    await page.getByTestId("schema-save").click();
    await expect(page.locator('[data-schema-doc-type="estimate"]')).toHaveCount(2);
    await expect(page.getByTestId("schemas-reclassify-banner")).toHaveCount(0);
    // Both samples are forms of the type now.
    await page.getByTestId("nav-library").click();
    await expect(card(page, "견적서 양식.hwpx")).toContainText("견적서");
    await expect(card(page, "견적서-1p.jpg")).toContainText("견적서");

    // Settings: the processing tab exposes the new pipeline options.
    await page.getByTestId("nav-settings").click();
    await page.getByTestId("settings-tab-processing").click();
    await expect(page.getByTestId("settings-inbox-auto-level")).toBeVisible();
    await expect(page.getByTestId("settings-precise-mode")).toBeVisible();
  });

  test("a word selected on the page image becomes a field value with one click", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await processEstimate(page);
    await card(page, "견적서-1p.jpg").getByTestId(/doc-open-/).click();
    const layer = page.getByTestId("viewer-text-layer");
    await expect(layer).toContainText("㈜한빛소프트");
    await layer.locator("[data-word-index]", { hasText: "㈜한빛소프트" }).first().click();
    await expect(page.getByTestId("viewer-selection-toolbar")).toContainText("㈜한빛소프트");
    await page.getByTestId("viewer-selection-field").click();
    // The menu lists each field with its current value: pick the recipient by its value.
    await page.getByRole("menuitem").filter({ hasText: "DocuLens" }).click();
    // The 이력 tab shows the correction (old → new value).
    await page.getByTestId("viewer-tab-history").click();
    const change = page.getByTestId("history-change").filter({ hasText: "recipient" }).first();
    await expect(change).toContainText(ko.pipeline.field_status.corrected);
    await expect(change).toContainText("DocuLens → ㈜한빛소프트");
  });
});
