import { expect, test } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { card, createWorkspace, docId, freshApp, importViaLibrary, tplPattern } from "./helpers";

// P4 acceptance (design §4.1): bulk processing (classify → OCR → extract) →
// review flags on the card and in the viewer's Fields tab → keyboard review
// queue → document promoted to "reviewed"; schemas (NL creation, form
// registration, type mapping) and the processing settings — against the
// browser mock backend. Real VLM calls, validators and the mirror are covered
// by the Rust tests.

/** Imports the receipt without auto-processing, then runs 표준 처리 on it from the library bulk bar. */
async function processReceipt(page: Parameters<typeof freshApp>[0]) {
  await importViaLibrary(page, ["D:\\scan\\영수증-1p.jpg"], { autoLevel: "none" });
  await page.getByTestId(`doc-select-${await docId(page, "영수증-1p.jpg")}`).check();
  await page.getByTestId("library-process").click();
  await page.getByTestId("library-process-standard").click();
  await expect(card(page, "영수증-1p.jpg")).toHaveAttribute("data-doc-status", "extracted", { timeout: 15_000 });
}

test.describe("P4 pipeline", () => {
  test("bulk processing extracts fields, flags low-confidence values and the viewer review promotes the document", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\영수증-1p.jpg", "D:\\scan\\세금계산서-2p.pdf"], { autoLevel: "none" });

    // Select both cards → 처리 ▾ → 표준 처리.
    for (const name of ["영수증-1p.jpg", "세금계산서-2p.pdf"]) {
      await page.getByTestId(`doc-select-${await docId(page, name)}`).check();
    }
    await page.getByTestId("library-process").click();
    await page.getByTestId("library-process-standard").click();
    await expect(card(page, "영수증-1p.jpg")).toHaveAttribute("data-doc-status", "extracted", { timeout: 15_000 });
    await expect(card(page, "영수증-1p.jpg")).toContainText(ko.documents.doc_types.receipt);
    await expect(card(page, "영수증-1p.jpg").getByTestId(/doc-review-/)).toHaveText(ko.library.review_pending);
    await expect(card(page, "세금계산서-2p.pdf")).toContainText(ko.documents.doc_types.tax_invoice);

    // Jobs page shows the Korean summary of the pipeline job.
    await page.getByTestId("nav-jobs").click();
    await page.getByTestId("jobs-tab-finished").click();
    const job = page.locator('[data-job-kind="pipeline"]').first();
    await expect(job).toContainText(ko.jobs.kind.pipeline);
    await expect(job.getByTestId("job-detail")).toContainText(tplPattern(ko.jobs.detail.process_summary, { processed: 2, classified: 2, extracted: 2, failed: 0 }));

    // Viewer: Fields tab lists extracted values with confidence glyphs; OCR text is in the text tab.
    await page.getByTestId("nav-library").click();
    await card(page, "영수증-1p.jpg").getByTestId(/doc-open-/).click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();
    await expect(page.getByTestId("viewer-status")).toHaveText(ko.documents.status.extracted);
    await expect(page.getByTestId("viewer-review-badge")).toHaveText(ko.viewer.review_badge.pending);
    await expect(page.getByTestId("viewer-text")).toContainText("스타벅스 역삼점");
    await page.getByTestId("viewer-tab-fields").click();
    await expect(page.getByTestId("fields-panel")).toBeVisible();
    const storeName = page.getByTestId("field-store_name");
    await expect(storeName.getByTestId("field-value")).toHaveText("스타벅스 역삼점");
    await expect(storeName.getByTestId("field-confidence")).toContainText("✔");
    const approval = page.getByTestId("field-approval_no");
    await expect(approval).toHaveAttribute("data-field-review", "pending");
    await expect(approval.getByTestId("field-confidence")).toContainText("▲");
    await expect(page.getByTestId("field-items[0].amount").getByTestId("field-value")).toHaveText("9,000");

    // Correct the approval number inline, approve the rest → reviewed.
    await approval.getByTestId("field-correct").click();
    await approval.getByTestId("field-edit-input").fill("87654321");
    await approval.getByTestId("field-edit-save").click();
    await expect(approval.getByTestId("field-value")).toHaveText("87654321");
    await expect(approval.getByTestId("field-status")).toHaveText(ko.pipeline.field_status.corrected);
    await page.getByTestId("fields-approve-all").click();
    await expect(page.getByTestId("viewer-status")).toHaveText(ko.documents.status.reviewed);
    await expect(page.getByTestId("viewer-review-badge")).toHaveText(ko.viewer.review_badge.done);

    // History tab records the correction; Tables tab parsed the OCR table.
    await page.getByTestId("viewer-tab-history").click();
    await expect(page.getByTestId("history-change").filter({ hasText: "approval_no" }).first()).toContainText(ko.pipeline.field_status.corrected);
    await page.getByTestId("viewer-tab-tables").click();
    await expect(page.getByTestId("table-card").first()).toContainText("아메리카노");
  });

  test("review queue: keyboard approval, editing and the reason filter", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await processReceipt(page);

    await page.getByTestId("nav-review").click();
    await expect(page.getByTestId("page-review")).toBeVisible();
    const items = page.locator('[data-testid^="review-item-"]');
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
    await expect(card(page, "영수증-1p.jpg")).toHaveAttribute("data-doc-status", "reviewed");
  });

  test("review edit dialog: Enter during a Korean IME composition neither saves nor approves", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await processReceipt(page);

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
    await importViaLibrary(page, ["D:\\forms\\견적서 양식.hwpx"], { autoLevel: "none" });

    // Built-in samples are listed; NL creation opens a draft in the editor.
    await page.getByTestId("nav-schemas").click();
    await expect(page.getByTestId("page-schemas")).toBeVisible();
    await expect(page.locator('[data-testid^="schema-row-builtin-"]')).toHaveCount(7);
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

    // Form registration from the library: derive → confirm → the document becomes a form.
    await page.getByTestId("nav-library").click();
    await page.getByTestId(`doc-select-${await docId(page, "견적서 양식.hwpx")}`).check();
    await page.getByTestId("library-register-form").click();
    await expect(page.getByTestId("form-register-dialog")).toBeVisible();
    await page.getByTestId("form-register-name").fill("견적서");
    await page.getByTestId("form-register-derive").click();
    await expect(page.getByTestId("schema-editor")).toBeVisible();
    await expect(page.getByTestId("schema-doc-type")).toHaveValue("quotation");
    await expect(page.getByTestId("schema-field").first()).toBeVisible();
    await page.getByTestId("schema-save").click();
    await expect(page.locator('[data-schema-doc-type="quotation"]')).toHaveCount(1);
    await expect(page.getByTestId("schemas-reclassify-banner")).toBeVisible();
    await page.getByTestId("nav-library").click();
    await expect(card(page, "견적서 양식.hwpx")).toContainText("견적서");

    // Settings: the processing tab exposes the new pipeline options.
    await page.getByTestId("nav-settings").click();
    await page.getByTestId("settings-tab-processing").click();
    await expect(page.getByTestId("settings-inbox-auto-level")).toBeVisible();
    await expect(page.getByTestId("settings-precise-mode")).toBeVisible();
  });
});
