import { expect, test, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };

import { createWorkspace, freshApp, openImportPreview } from "./helpers";

// P2 acceptance (design §4.1): import (preview with duplicates / unsupported
// reasons) → rendered pages → library grid/list → viewer navigation, rotation,
// tags and deletion, driven against the browser mock backend (the real
// PDFium / HWPX path is covered by the Rust integration tests).

const item = (page: Page, status: string) => page.locator(`[data-testid="doc-import-item"][data-item-status="${status}"]`);

test.describe("P2 documents", () => {
  test("import preview classifies files, the job renders pages and the library lists them", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-library").click();
    await expect(page.getByTestId("library-empty")).toBeVisible();

    // The same minutes in two folders = same content (mock hashes by name) → in-batch duplicate.
    await openImportPreview(page, ["D:\\scan\\회의록-3p.pdf", "D:\\scan\\견적서.hwpx", "D:\\scan\\photo.heic", "D:\\backup\\회의록-3p.pdf", "D:\\scan\\IMG_0001.jpg"]);
    await expect(page.getByTestId("doc-import-item")).toHaveCount(5);
    await expect(item(page, "unsupported")).toHaveCount(1);
    await expect(item(page, "duplicate_in_batch")).toHaveCount(1);
    await expect(item(page, "new")).toHaveCount(3);
    await expect(item(page, "unsupported")).toContainText("HEIC");
    await expect(page.getByTestId("doc-import-plan")).toContainText("신규 3 · 다시 연결 0 · 중복 1 · 이미 등록 0 · 미지원 1");
    await page.getByTestId("doc-import-run").click();

    // Cards appear while importing and settle to "rendered" with page counts.
    const receipt = page.locator('[data-testid^="doc-card-"][data-doc-name="회의록-3p.pdf"]');
    await expect(receipt).toBeVisible({ timeout: 10_000 });
    await expect(receipt).toHaveAttribute("data-doc-status", "rendered", { timeout: 15_000 });
    await expect(receipt).toContainText("3쪽");
    const hwpx = page.locator('[data-testid^="doc-card-"][data-doc-name="견적서.hwpx"]');
    await expect(hwpx).toHaveAttribute("data-doc-status", "rendered", { timeout: 15_000 });
    await expect(hwpx).toContainText("변환본");
    await expect(page.getByTestId("library-stats")).toContainText("문서 3개");

    // Jobs page shows the finished import with a Korean summary.
    await page.getByTestId("nav-jobs").click();
    await page.getByTestId("jobs-tab-finished").click();
    const importJob = page.locator('[data-job-kind="document_import"]').first();
    await expect(importJob).toContainText("파일 3개");
    await expect(importJob.getByTestId("job-detail")).toHaveText("가져옴 3 · 다시 연결 0 · 실패 0 · 건너뜀 2");

    // Re-importing the same file is a duplicate of the registered copy and nothing can run.
    await page.getByTestId("nav-library").click();
    await openImportPreview(page, ["D:\\scan\\회의록-3p.pdf", "C:\\Work\\Docs\\documents\\견적서.hwpx"]);
    await expect(item(page, "duplicate")).toHaveCount(1);
    await expect(item(page, "already_registered")).toHaveCount(1);
    await expect(page.getByTestId("doc-import-run")).toBeDisabled();
    await page.keyboard.press("Escape");

    // Filters: list view, search and the folder tree; the list keeps rendering while the query changes.
    await page.getByTestId("library-view-list").click();
    await expect(page.locator('[data-testid^="doc-row-"]')).toHaveCount(3);
    await page.getByTestId("library-search").fill("견적");
    await expect(page.locator('[data-testid^="doc-row-"]')).toHaveCount(1);
    await page.getByTestId("library-search").fill("");
    await page.getByTestId("folder-documents").click();
    await expect(page.locator('[data-testid^="doc-row-"]')).toHaveCount(3);
    await page.getByTestId("folder-all").click();
    await page.getByTestId("library-filter-kind").click();
    await page.getByRole("option", { name: "이미지" }).click();
    await expect(page.locator('[data-testid^="doc-row-"]')).toHaveCount(1);
    await page.getByTestId("library-filter-kind").click();
    await page.getByRole("option", { name: "전체" }).click();
    await expect(page.locator('[data-testid^="doc-row-"]')).toHaveCount(3);

    // Home shows the recent documents and counts.
    await page.getByTestId("nav-home").click();
    await expect(page.getByTestId("home-recent")).toContainText("견적서.hwpx");
    await expect(page.getByTestId("home-stat-processing")).toHaveText("3");
  });

  test("viewer: pages, thumbnails, keyboard, rotation, tags, type and delete", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-library").click();
    await openImportPreview(page, ["D:\\scan\\계약서-12p.pdf"]);
    await page.getByTestId("doc-import-run").click();
    const card = page.locator('[data-testid^="doc-card-"][data-doc-name="계약서-12p.pdf"]');
    await expect(card).toHaveAttribute("data-doc-status", "rendered", { timeout: 15_000 });
    await card.locator('[data-testid^="doc-open-"]').click();

    await expect(page.getByTestId("page-viewer")).toBeVisible();
    await expect(page.getByTestId("viewer-page")).toHaveText("1 / 12");
    await expect(page.getByTestId("viewer-image")).toBeVisible();
    await expect(page.getByTestId("viewer-text-layer")).toContainText("1쪽의 텍스트");
    await page.getByTestId("viewer-next").click();
    await expect(page.getByTestId("viewer-page")).toHaveText("2 / 12");
    await page.getByTestId("viewer-canvas").click();
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("viewer-page")).toHaveText("3 / 12");
    await page.keyboard.press("+");
    await expect(page.getByTestId("viewer-zoom")).toHaveText("125%");
    await page.keyboard.press("0");
    await expect(page.getByTestId("viewer-zoom")).toHaveText("맞춤");
    // Arrow keys inside the tab list move between tabs without flipping the page; the right pane
    // only has AI 검색 and 이력 (user decision 2026-11-09).
    await expect(page.getByTestId("viewer-tab-text")).toHaveCount(0);
    await expect(page.getByTestId("viewer-tab-fields")).toHaveCount(0);
    await page.getByTestId("viewer-tab-qa").focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("viewer-page")).toHaveText("3 / 12");
    await expect(page.getByTestId("viewer-tab-history")).toHaveAttribute("data-state", "active");
    // The strip is virtualised: scroll it to the end so the last thumbnail is mounted.
    await page.getByTestId("viewer-thumbs").evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    await page.getByTestId("viewer-thumb-12").click();
    await expect(page.getByTestId("viewer-page")).toHaveText("12 / 12");
    await expect(page.getByTestId("viewer-next")).toBeDisabled();

    // Rotation swaps the page dimensions (mock) and the image reloads.
    const before = await page.getByTestId("viewer-image").getAttribute("src");
    await page.getByTestId("viewer-rotate-cw").click();
    await expect.poll(async () => page.getByTestId("viewer-image").getAttribute("src")).not.toBe(before);

    // Tags and document type persist.
    await page.getByTestId("viewer-tag-input").fill("출장");
    await page.getByTestId("viewer-tag-input").press("Enter");
    await expect(page.getByTestId("viewer-tags")).toContainText("출장");
    await page.getByTestId("viewer-doc-type").click();
    await page.getByRole("option", { name: "계약서" }).click();
    await expect(page.getByTestId("viewer-doc-type")).toContainText("계약서");
    await page.getByTestId("viewer-title").fill("2026 공급 계약");
    await page.getByTestId("viewer-title").press("Enter");
    await page.getByTestId("viewer-back").click();
    await expect(page.locator('[data-testid^="doc-card-"]').first()).toContainText("2026 공급 계약");

    // Delete from the viewer (themed confirm) returns to an empty library.
    await page.locator('[data-testid^="doc-open-"]').first().click();
    await page.getByTestId("viewer-delete").click();
    await expect(page.getByTestId("confirm-dialog")).toContainText("계약서-12p.pdf");
    await page.getByTestId("confirm-ok").click();
    await expect(page.getByTestId("library-empty")).toBeVisible();
  });

  test("library bulk actions: selection follows the filter, re-render job, delete, inbox toggle", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-library").click();
    await openImportPreview(page, ["D:\\a\\one.pdf", "D:\\a\\two.docx"]);
    await page.getByTestId("doc-import-run").click();
    await expect(page.locator('[data-testid^="doc-card-"][data-doc-status="rendered"]')).toHaveCount(2, { timeout: 15_000 });
    await page.getByLabel("모두 선택").check();
    await expect(page.getByTestId("library-selected")).toHaveText("2개 선택");
    // A filter that hides the selection also drops it from the bulk actions.
    await page.getByTestId("library-filter-kind").click();
    await page.getByRole("option", { name: "이미지" }).click();
    await expect(page.getByTestId("library-selected")).toHaveCount(0);
    await page.getByTestId("library-filter-kind").click();
    await page.getByRole("option", { name: "전체" }).click();
    await page.getByLabel("모두 선택").check();
    await page.getByTestId("library-rerender").click();
    await page.getByTestId("nav-jobs").click();
    // Fast mock mode finishes the render job almost immediately.
    await page.getByTestId("jobs-tab-finished").click();
    await expect(page.locator('[data-job-kind="document_render"]').first().getByTestId("job-detail")).toHaveText("2개 렌더 완료");
    await page.getByTestId("nav-library").click();
    await page.getByLabel("모두 선택").check();
    await page.getByTestId("library-delete").click();
    await expect(page.getByTestId("confirm-dialog")).toContainText("2개");
    await page.getByTestId("confirm-ok").click();
    await expect(page.getByTestId("library-empty")).toBeVisible();
    await page.getByTestId("library-inbox-watch").click();
    await expect(page.getByTestId("library-inbox-watch")).toHaveAttribute("data-state", "unchecked");
  });

  test("missing PDF renderer shows the banner; the folder import opens the preview", async ({ page }) => {
    await freshApp(page, { "doculens.mock.nopdfium": "1" });
    await createWorkspace(page);
    await page.getByTestId("nav-library").click();
    await expect(page.getByTestId("library-pdfium-missing")).toContainText("PDFium");
    await page.getByTestId("library-import-existing").click();
    await expect(page.getByTestId("doc-import-paths")).toHaveValue("C:\\Work\\Docs");
    await expect(page.getByTestId("doc-import-error").or(page.getByTestId("doc-import-plan"))).toBeVisible();
  });

  test("words on the page image can be corrected in place: click / drag-select, Enter, type, Enter", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-library").click();
    await openImportPreview(page, ["D:\\scan\\메모-1p.pdf"]);
    await page.getByTestId("doc-import-run").click();
    const card = page.locator('[data-testid^="doc-card-"][data-doc-name="메모-1p.pdf"]');
    await expect(card).toHaveAttribute("data-doc-status", "rendered", { timeout: 15_000 });
    await card.locator('[data-testid^="doc-open-"]').click();
    const layer = page.getByTestId("viewer-text-layer");
    await expect(layer).toContainText("1쪽의 텍스트");
    await expect(layer).toHaveAttribute("data-layer-source", "layer");
    // A click selects one word on the image and the floating toolbar names it.
    const word = (text: string) => layer.locator("[data-word-index]", { hasText: text }).first();
    await word("1쪽의").click();
    await expect(page.getByTestId("viewer-selection-toolbar")).toContainText("1쪽의");
    // Dragging from one word to another extends the selection in reading order.
    const from = (await word("1쪽의").boundingBox())!;
    const to = (await word("텍스트").boundingBox())!;
    await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
    await page.mouse.down();
    await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 6 });
    await page.mouse.up();
    await expect(page.getByTestId("viewer-selection-toolbar")).toContainText("1쪽의 텍스트");
    await expect(layer.locator("[data-selected]")).toHaveCount(2);
    // Enter opens the inline editor pre-filled with the selected words; Enter saves the replacement.
    await page.getByTestId("page-viewer").focus();
    await page.keyboard.press("Enter");
    const input = page.getByTestId("viewer-text-input");
    await expect(input).toBeVisible();
    await expect(input).toHaveValue("1쪽의 텍스트");
    await input.fill("첫째 쪽의 본문");
    await page.keyboard.press("Enter");
    await expect(layer).toContainText("첫째 쪽의 본문 레이어입니다.");
    await expect(layer.locator("[data-edited]")).toHaveCount(3);
    // Corrected words are drawn visibly (blue on white) over the old glyphs; recognised ones stay transparent.
    await expect(layer.locator("[data-edited]").first()).toHaveCSS("color", "rgb(11, 106, 138)");
    await expect(layer.locator("[data-word-index]:not([data-edited])").first()).toHaveCSS("color", "rgba(0, 0, 0, 0)");
    await expect(page.getByTestId("viewer-text-source")).toHaveText(ko.viewer.text_edit.source.manual);
    // Escape drops a selection; a drag on empty space offers to add text the recogniser missed.
    await word("합계").click();
    await page.getByTestId("page-viewer").focus();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("viewer-selection-toolbar")).toHaveCount(0);
    // The foot of the page is below the fold: scroll the canvas down before drawing the box there.
    await page.getByTestId("viewer-canvas").evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    const box = (await layer.boundingBox())!;
    await page.mouse.move(box.x + box.width * 0.15, box.y + box.height * 0.95);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.55, box.y + box.height * 0.975, { steps: 6 });
    await page.mouse.up();
    const add = page.getByTestId("viewer-text-add-input");
    await expect(add).toBeVisible();
    await add.fill("담당자 확인");
    await page.keyboard.press("Enter");
    await expect(layer).toContainText("담당자 확인");
    // The corrections are searchable right away (the mock re-indexes on save).
    await page.getByTestId("nav-search").click();
    await page.getByTestId("search-input").fill("첫째 쪽");
    await page.getByTestId("search-run").click();
    await expect(page.getByTestId("search-hit").first()).toContainText("메모-1p.pdf", { timeout: 15_000 });
  });
});
