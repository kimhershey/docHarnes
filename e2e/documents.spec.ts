import { expect, test, type Page } from "@playwright/test";

// P2 acceptance (design §4.1): import (preview with duplicates / unsupported
// reasons) → rendered pages → library grid/list → viewer navigation, rotation,
// tags and deletion, driven against the browser mock backend (the real
// PDFium / HWPX path is covered by the Rust integration tests).

async function freshApp(page: Page) {
  await page.goto("/");
  await page.evaluate(() => {
    localStorage.clear();
    localStorage.setItem("doculens.mock.fast", "1");
  });
  await page.reload();
}

async function createWorkspace(page: Page, path = "C:\\Work\\Docs") {
  await expect(page.getByTestId("onboarding")).toBeVisible();
  await page.getByTestId("workspace-path").fill(path);
  await expect(page.getByTestId("workspace-inspection")).toBeVisible();
  await page.getByTestId("workspace-submit").click();
  await expect(page.getByTestId("hardware-panel")).toBeVisible();
  await page.getByTestId("onboarding-next").click();
  await page.getByTestId("onboarding-finish").click();
  await expect(page.getByTestId("app-shell")).toBeVisible();
}

async function importFiles(page: Page, paths: string[]) {
  await page.getByTestId("library-import").click();
  await page.getByTestId("doc-import-paths").fill(paths.join("\n"));
  await page.getByTestId("doc-import-preview").click();
  await expect(page.getByTestId("doc-import-plan")).toBeVisible();
}

test.describe("P2 documents", () => {
  test("import preview classifies files, the job renders pages and the library lists them", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-library").click();
    await expect(page.getByTestId("library-empty")).toBeVisible();

    await importFiles(page, ["D:\\scan\\영수증-3p.pdf", "D:\\scan\\견적서.hwpx", "D:\\scan\\photo.heic", "D:\\scan\\영수증-3p.pdf", "D:\\scan\\IMG_0001.jpg"]);
    const items = page.getByTestId("doc-import-item");
    await expect(items).toHaveCount(5);
    await expect(items.filter({ has: page.locator('[data-item-status="unsupported"]') })).toHaveCount(0);
    await expect(page.locator('[data-testid="doc-import-item"][data-item-status="unsupported"]')).toHaveCount(1);
    await expect(page.locator('[data-testid="doc-import-item"][data-item-status="duplicate_in_batch"]')).toHaveCount(1);
    await expect(page.locator('[data-testid="doc-import-item"][data-item-status="new"]')).toHaveCount(3);
    await expect(page.getByTestId("doc-import-plan")).toContainText("HEIC");
    await page.getByTestId("doc-import-run").click();

    // Cards appear while importing and settle to "rendered" with page counts.
    const receipt = page.locator('[data-testid^="doc-card-"][data-doc-name="영수증-3p.pdf"]');
    await expect(receipt).toBeVisible({ timeout: 10_000 });
    await expect(receipt).toHaveAttribute("data-doc-status", "rendered", { timeout: 15_000 });
    await expect(receipt).toContainText("3쪽");
    const hwpx = page.locator('[data-testid^="doc-card-"][data-doc-name="견적서.hwpx"]');
    await expect(hwpx).toHaveAttribute("data-doc-status", "rendered", { timeout: 15_000 });
    await expect(hwpx).toContainText("변환본");
    await expect(page.getByTestId("library-stats")).toContainText("문서 3개");

    // Jobs page shows the finished import.
    await page.getByTestId("nav-jobs").click();
    await page.getByTestId("jobs-tab-finished").click();
    await expect(page.locator('[data-job-kind="document_import"]').first()).toContainText("imported 3");

    // Re-importing the same files is reported as duplicates only.
    await page.getByTestId("nav-library").click();
    await importFiles(page, ["D:\\scan\\영수증-3p.pdf"]);
    await expect(page.locator('[data-testid="doc-import-item"][data-item-status="duplicate"]')).toHaveCount(1);
    await expect(page.getByTestId("doc-import-run")).toBeDisabled();
    await page.keyboard.press("Escape");

    // Filters: list view, search and the folder tree.
    await page.getByTestId("library-view-list").click();
    await expect(page.locator('[data-testid^="doc-row-"]')).toHaveCount(3);
    await page.getByTestId("library-search").fill("견적");
    await expect(page.locator('[data-testid^="doc-row-"]')).toHaveCount(1);
    await page.getByTestId("library-search").fill("");
    await page.getByTestId("folder-documents").click();
    await expect(page.locator('[data-testid^="doc-row-"]')).toHaveCount(3);
    await page.getByTestId("folder-all").click();

    // Home shows the recent documents and counts.
    await page.getByTestId("nav-home").click();
    await expect(page.getByTestId("home-recent")).toContainText("견적서.hwpx");
    await expect(page.getByTestId("home-stat-processing")).toHaveText("3");
  });

  test("viewer: pages, thumbnails, rotation, tags, type and delete", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-library").click();
    await importFiles(page, ["D:\\scan\\계약서-12p.pdf"]);
    await page.getByTestId("doc-import-run").click();
    const card = page.locator('[data-testid^="doc-card-"][data-doc-name="계약서-12p.pdf"]');
    await expect(card).toHaveAttribute("data-doc-status", "rendered", { timeout: 15_000 });
    await card.locator('[data-testid^="doc-open-"]').click();

    await expect(page.getByTestId("page-viewer")).toBeVisible();
    await expect(page.getByTestId("viewer-page")).toHaveText("1 / 12");
    await expect(page.getByTestId("viewer-image")).toBeVisible();
    await expect(page.getByTestId("viewer-text")).toContainText("1쪽의 텍스트");
    await page.getByTestId("viewer-next").click();
    await expect(page.getByTestId("viewer-page")).toHaveText("2 / 12");
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("viewer-page")).toHaveText("3 / 12");
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

    // Delete from the viewer returns to an empty library.
    await page.locator('[data-testid^="doc-open-"]').first().click();
    page.once("dialog", (d) => void d.accept());
    await page.getByTestId("viewer-delete").click();
    await expect(page.getByTestId("library-empty")).toBeVisible();
  });

  test("library bulk actions: select all, re-render job, delete", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-library").click();
    await importFiles(page, ["D:\\a\\one.pdf", "D:\\a\\two.docx"]);
    await page.getByTestId("doc-import-run").click();
    await expect(page.locator('[data-testid^="doc-card-"][data-doc-status="rendered"]')).toHaveCount(2, { timeout: 15_000 });
    await page.getByLabel("모두 선택").check();
    await expect(page.getByTestId("library-selected")).toHaveText("2개 선택");
    await page.getByTestId("library-rerender").click();
    await page.getByTestId("nav-jobs").click();
    // Fast mock mode finishes the render job almost immediately.
    await page.getByTestId("jobs-tab-finished").click();
    await expect(page.locator('[data-job-kind="document_render"]').first()).toContainText("rendered 2");
    await page.getByTestId("nav-library").click();
    await page.getByLabel("모두 선택").check();
    page.once("dialog", (d) => void d.accept());
    await page.getByTestId("library-delete").click();
    await expect(page.getByTestId("library-empty")).toBeVisible();
    await page.getByTestId("library-inbox-watch").click();
    await expect(page.getByTestId("library-inbox-watch")).toHaveAttribute("data-state", "unchecked");
  });
});
