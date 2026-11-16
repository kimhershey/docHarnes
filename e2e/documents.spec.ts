import { expect, test, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };

import { createWorkspace, docId, freshApp, importViaLibrary, openAssistantScreen, openImportPreview, tpl, tplPattern } from "./helpers";

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

    // The 문서함 "오늘 할 일" strip shows the recent documents and the counts (the former home screen).
    await expect(page.getByTestId("library-today-recent")).toContainText("견적서.hwpx");
    await expect(page.getByTestId("library-today-stat-processing")).toHaveText(tpl(ko.library.today.processing, { count: 3 }));
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
    // The viewer has the keyboard as soon as the document is shown — no click needed first (DOC-2).
    await expect(page.getByTestId("page-viewer")).toBeFocused();
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("viewer-page")).toHaveText("2 / 12");
    await page.keyboard.press("ArrowLeft");
    await expect(page.getByTestId("viewer-page")).toHaveText("1 / 12");
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
    await page.getByLabel("전체 선택").check();
    await expect(page.getByTestId("library-selected")).toHaveText("2개 선택");
    // A filter that hides the selection also drops it from the bulk actions.
    await page.getByTestId("library-filter-kind").click();
    await page.getByRole("option", { name: "이미지" }).click();
    await expect(page.getByTestId("library-selected")).toHaveCount(0);
    await page.getByTestId("library-filter-kind").click();
    await page.getByRole("option", { name: "전체" }).click();
    await page.getByLabel("전체 선택").check();
    // 다시 렌더 lives in the right-click menu (plan §3): on a selected card it covers the whole selection.
    await page.locator('[data-testid^="doc-card-"]').first().click({ button: "right" });
    await page.getByTestId("library-context-rerender").click();
    await page.getByTestId("nav-jobs").click();
    // Fast mock mode finishes the render job almost immediately.
    await page.getByTestId("jobs-tab-finished").click();
    await expect(page.locator('[data-job-kind="document_render"]').first().getByTestId("job-detail")).toHaveText("2개 렌더 완료");
    await page.getByTestId("nav-library").click();
    await page.getByLabel("전체 선택").check();
    await page.getByTestId("library-delete").click();
    await expect(page.getByTestId("confirm-dialog")).toContainText("2개");
    await page.getByTestId("confirm-ok").click();
    await expect(page.getByTestId("library-empty")).toBeVisible();
    // The inbox watch switch is in the right-click menu of the empty space (and in 설정 > 처리).
    await page.getByTestId("library-empty").click({ button: "right" });
    await expect(page.getByTestId("library-context-inbox-watch")).toHaveAttribute("data-checked", "true");
    await page.getByTestId("library-context-inbox-watch").click();
    await page.getByTestId("library-empty").click({ button: "right" });
    await expect(page.getByTestId("library-context-inbox-watch")).toHaveAttribute("data-checked", "false");
    await page.keyboard.press("Escape");
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
    // Ctrl+Z in the viewer takes the correction back (the page's own history, not the file journal) and the
    // words lose their corrected colour; Ctrl+Shift+Z re-applies it. The layer bar counts the corrections.
    await expect(page.getByTestId("viewer-text-edits-count")).toHaveText(tpl(ko.viewer.text_edit.count, { count: 1 }));
    // No click in between: when the editor closed, the focus went back to the viewer (DOC-2).
    await page.keyboard.press("Control+z");
    await expect(layer).toContainText("1쪽의 텍스트 레이어입니다.");
    await expect(layer.locator("[data-edited]")).toHaveCount(0);
    await expect(page.getByTestId("toaster")).toContainText(ko.viewer.text_edit.undone);
    await expect(page.getByTestId("viewer-text-edits-count")).toHaveText(tpl(ko.viewer.text_edit.count, { count: 0 }));
    await page.keyboard.press("Control+Shift+z");
    await expect(layer).toContainText("첫째 쪽의 본문 레이어입니다.");
    await expect(layer.locator("[data-edited]")).toHaveCount(3);
    await expect(page.getByTestId("viewer-text-source")).toHaveText(ko.viewer.text_edit.source.manual);
    // Escape drops a selection; a drag on empty space offers to add text the recogniser missed.
    await word("합계").click();
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
    // The 이력 tab lists both corrections (newest first); "이 수정 되돌리기" takes the added words back and
    // marks the entry undone, while the earlier correction stays.
    await page.getByTestId("viewer-tab-history").click();
    const edits = page.locator('[data-testid="history-text-edit"]');
    await expect(edits).toHaveCount(2);
    await expect(edits.first()).toContainText("담당자 확인");
    await expect(edits.nth(1)).toContainText("첫째 쪽의 본문");
    await edits.first().getByTestId("history-text-revert").click();
    await expect(layer).not.toContainText("담당자 확인");
    await expect(layer).toContainText("첫째 쪽의 본문 레이어입니다.");
    await expect(edits.first()).toHaveAttribute("data-undone", "true");
    await expect(edits.first().getByTestId("history-text-revert")).toHaveCount(0);
    // The corrections are searchable right away (the mock re-indexes on save): "근거만 찾기" in the assistant.
    await page.getByTestId("nav-assistant").click();
    await page.getByTestId("assistant-evidence-only").click();
    await page.getByTestId("assistant-input").fill("첫째 쪽");
    await page.getByTestId("assistant-send").click();
    await expect(page.getByTestId("evidence-hit").first()).toContainText("메모-1p.pdf", { timeout: 15_000 });
  });

  test("Ctrl+Z after a correction on the page takes the correction back; a file batch is reverted only after asking (DOC-2)", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\메모-1p.pdf"], { autoLevel: "none" });
    const id = await docId(page, "메모-1p.pdf");
    // A journaled file batch: 정리 moves the document.
    await page.getByTestId("library-organize").click();
    await page.getByTestId("organize-preview").click();
    await expect(page.getByTestId("organize-preview-table")).toBeVisible();
    await page.getByTestId("organize-run").click();
    await page.getByTestId("organize-confirm").click();
    await expect(page.getByTestId("organize-result")).toContainText(tplPattern(ko.organize.done, { failed: 0 }), { timeout: 15_000 });

    // A word is corrected on the page image.
    await page.getByTestId("nav-library").click();
    await page.getByTestId(`doc-open-${id}`).click();
    const layer = page.getByTestId("viewer-text-layer");
    await layer.locator("[data-word-index]", { hasText: "1쪽의" }).first().click();
    await page.keyboard.press("Enter");
    await page.getByTestId("viewer-text-input").fill("첫째");
    await page.keyboard.press("Enter");
    await expect(layer.locator("[data-edited]")).toHaveCount(1);

    // Ctrl+Z straight after saving used to revert the organise batch ("1건 되돌림") and leave the correction.
    const undone = tplPattern(ko.palette.undo_done, {});
    await page.keyboard.press("Control+z");
    await expect(layer.locator("[data-edited]")).toHaveCount(0);
    await expect(page.getByTestId("toaster")).toContainText(ko.viewer.text_edit.undone);
    await expect(page.getByTestId("toaster")).not.toContainText(undone);
    // With no correction left on the page the file batch is offered, not reverted.
    await page.keyboard.press("Control+z");
    const confirm = page.getByTestId("confirm-dialog");
    await expect(confirm).toContainText(ko.viewer.journal.undo_title);
    await expect(confirm).toContainText(ko.organize.history.kind.organize);
    await page.getByTestId("confirm-cancel").click();
    await expect(confirm).toBeHidden();
    await expect(page.getByTestId("toaster")).not.toContainText(undone);
    // The same from a toolbar button of the viewer (outside its own key handling); agreeing reverts the batch.
    await page.getByTestId("viewer-rerender").focus();
    await page.keyboard.press("Control+z");
    await expect(confirm).toContainText(ko.viewer.journal.undo_title);
    await page.getByTestId("confirm-ok").click();
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.palette.undo_done, { restored: 1, skipped: 0 }));
    // Nothing is left anywhere: a plain notice, no question.
    await page.keyboard.press("Control+z");
    await expect(page.getByTestId("toaster")).toContainText(ko.palette.nothing_to_undo);
    await expect(confirm).toBeHidden();
  });

  test("a document that is being converted / rendered reads 처리 중, not 오류; cut off, it reads 중단됨 until it is rendered again (DOC-8)", async ({ page }) => {
    // The mock at its real pace: the state during the work has to be seen.
    await freshApp(page, {}, { fast: false });
    await createWorkspace(page);
    await page.getByTestId("nav-library").click();
    await openImportPreview(page, ["D:\\scan\\계약서-12p.pdf"]);
    await page.getByTestId("doc-import-run").click();
    const card = page.locator('[data-testid^="doc-card-"][data-doc-name="계약서-12p.pdf"]');
    const badge = card.locator('[data-testid^="doc-status-"]');
    // While the import job converts / renders the document, its row carries the crash marker (status `error`).
    await expect(card).toHaveAttribute("data-doc-status", "error", { timeout: 10_000 });
    await expect(badge).toHaveAttribute("data-display-status", "processing");
    await expect(badge).toHaveText(ko.documents.status.processing);

    // The app goes away in the middle of the work: the marker stays, no job is alive any more.
    await page.reload();
    await page.getByTestId("nav-library").click();
    await expect(badge).toHaveAttribute("data-display-status", "interrupted");
    await expect(badge).toHaveText(ko.documents.status.interrupted);
    await expect(badge).toHaveAttribute("title", ko.documents.status_hint.interrupted);
    await card.locator('[data-testid^="doc-open-"]').click();
    await expect(page.getByTestId("viewer-status")).toHaveText(ko.documents.status.interrupted);
    await expect(page.getByTestId("viewer-doc-error")).toHaveText(ko.documents.status_hint.interrupted);
    // 다시 렌더 processes it again.
    await page.getByTestId("viewer-rerender").click();
    await expect(page.getByTestId("viewer-status")).toHaveText(ko.documents.status.rendered, { timeout: 15_000 });
    await expect(page.getByTestId("viewer-doc-error")).toHaveCount(0);
    await page.getByTestId("viewer-back").click();
    await expect(card).toHaveAttribute("data-doc-status", "rendered");
    await expect(badge).toHaveText(ko.documents.status.rendered);
  });

  test("받은 문서함 chip: documents that sit only in a folder below the inbox are listed, and a folder's count is the size of its list (DOC-15)", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page); // C:\Work\Docs
    await importViaLibrary(page, ["C:\\Work\\Docs\\inbox\\2026-11\\청구서.pdf", "C:\\Work\\Docs\\inbox\\2026-11\\영수증.pdf", "D:\\scan\\계약서.pdf"], { autoLevel: "none" });
    const cards = page.locator('[data-testid^="doc-card-"]');
    await expect(cards).toHaveCount(3);
    // Nothing lies in `inbox` itself. The chip counts the folders below it, and a click lists them (the selection
    // used to be dropped at once, so the click did nothing).
    const chip = page.getByTestId("library-today-stat-inbox");
    await expect(chip).toContainText(tpl(ko.library.today.inbox, { count: 2 }));
    await chip.click();
    await expect(cards).toHaveCount(2);
    await expect(page.locator('[data-testid^="doc-card-"][data-doc-name="계약서.pdf"]')).toHaveCount(0);
    // The selected folder is shown in the navigation, and the number next to a folder is what its list holds.
    const inbox = page.getByTestId("folder-inbox");
    await expect(inbox).toHaveAttribute("aria-current", "true");
    await expect(inbox).toContainText("2");
    await expect(page.getByTestId("folder-inbox/2026-11")).toContainText("2");
    await page.getByTestId("folder-inbox/2026-11").click();
    await expect(cards).toHaveCount(2);
    await page.getByTestId("folder-all").click();
    await expect(cards).toHaveCount(3);
  });

  test("1.0.0: 원본 고치기 — a replacement is previewed, then saved as a new edition next to the original", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\docs\\개발계약서.docx", "D:\\docs\\스캔본.pdf"]);
    const original = await docId(page, "개발계약서.docx");
    const scan = await docId(page, "스캔본.pdf");

    // Only formats whose own file can be edited in place offer it.
    await page.getByTestId(`doc-open-${scan}`).click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();
    await expect(page.getByTestId("viewer-edit-original")).toHaveCount(0);
    await page.getByTestId("viewer-back").click();

    await page.getByTestId(`doc-open-${original}`).click();
    await expect(page.getByTestId("viewer-name")).toContainText("개발계약서.docx");
    await page.getByTestId("viewer-edit-original").click();
    const dialog = page.getByTestId("edit-original-dialog");
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId("edit-original-lead")).toContainText("원본 파일은 건드리지 않고");
    // Nothing is saved before the changes were looked at.
    await expect(page.getByTestId("edit-original-save")).toBeDisabled();
    await expect(page.getByTestId("edit-original-preview-run")).toBeDisabled();

    // A text that is not in the document: the row says so and there is nothing to save.
    await page.getByTestId("edit-original-find-0").fill("없는 글");
    await page.getByTestId("edit-original-replace-0").fill("x");
    await page.getByTestId("edit-original-preview-run").click();
    await expect(page.getByTestId("edit-original-count-0")).toHaveText("없음");
    await expect(page.getByTestId("edit-original-preview")).toHaveAttribute("data-total", "0");
    await expect(page.getByTestId("edit-original-save")).toBeDisabled();

    // Two replacements: the preview shows each paragraph before and after.
    await page.getByTestId("edit-original-find-0").fill("12,500원");
    await page.getByTestId("edit-original-replace-0").fill("13,000원");
    await expect(page.getByTestId("edit-original-preview")).toHaveCount(0);
    await page.getByTestId("edit-original-add").click();
    await page.getByTestId("edit-original-find-1").fill("텍스트 레이어");
    await page.getByTestId("edit-original-replace-1").fill("본문");
    await page.getByTestId("edit-original-preview-run").click();
    // (the mock's DOCX has two pages with the same two lines each)
    await expect(page.getByTestId("edit-original-preview")).toHaveAttribute("data-total", "4");
    await expect(page.getByTestId("edit-original-count-0")).toHaveText("2곳");
    await expect(page.getByTestId("edit-original-count-1")).toHaveText("2곳");
    await expect(page.getByTestId("edit-original-change-0")).toContainText("합계 12,500원");
    await expect(page.getByTestId("edit-original-change-0")).toContainText("합계 13,000원");
    // The dialog stays inside the window.
    const box = (await dialog.boundingBox())!;
    const view = page.viewportSize()!;
    expect(box.x >= 0 && box.y >= 0 && box.x + box.width <= view.width && box.y + box.height <= view.height).toBe(true);

    await page.getByTestId("edit-original-save").click();
    await expect(dialog).toHaveCount(0);
    const toast = page.getByTestId("toaster").locator("div", { hasText: "새 판으로 저장했습니다" }).first();
    await expect(toast).toContainText("개발계약서 (수정).docx");
    // "열기" goes to the new edition: its text carries both changes and it is the latest edition.
    await page.getByTestId("toast-action").first().click();
    await expect(page.getByTestId("viewer-name")).toContainText("개발계약서 (수정).docx");
    const layer = page.getByTestId("viewer-text-layer");
    await expect(layer).toContainText("13,000원");
    await expect(layer).toContainText("1쪽의 본문입니다");
    await page.getByTestId("viewer-version").click();
    await expect(page.getByTestId("version-label")).toHaveValue(/^수정 20\d\d-\d\d-\d\d \d\d:\d\d$/);
    await expect(page.getByTestId("version-latest")).toBeChecked();
    await page.keyboard.press("Escape");
    // The original is as it was — its text, and it stepped back to an earlier edition called 원본.
    await page.getByTestId("viewer-back").click();
    await expect(page.locator('[data-testid^="doc-card-"]')).toHaveCount(3);
    await page.getByTestId(`doc-open-${original}`).click();
    await expect(page.getByTestId("viewer-name")).toContainText("개발계약서.docx");
    await expect(layer).toContainText("12,500원");
    await expect(layer).toContainText("텍스트 레이어입니다");
    await page.getByTestId("viewer-version").click();
    await expect(page.getByTestId("version-label")).toHaveValue("원본");
    await expect(page.getByTestId("version-latest")).not.toBeChecked();
  });

  // 1.0.0 §2-1 step 2: a paragraph next to another, a paragraph taken out, and what cannot be done.
  test("1.0.0: 원본 고치기 — a paragraph is added and one is taken out; a text in several places asks which one", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\docs\\개발계약서.docx"]);
    const original = await docId(page, "개발계약서.docx");
    // At a UI scale of 150 %: the rows of the dialog wrap instead of running out of it (plan §11).
    await openAssistantScreen(page);
    await page.getByTestId("assistant-input").fill("UI 배율 150으로 바꿔줘");
    await page.getByTestId("assistant-send").click();
    await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize), { timeout: 15_000 }).toBe("24px");
    await page.getByTestId("nav-library").click();
    await page.getByTestId(`doc-open-${original}`).click();
    await page.getByTestId("viewer-edit-original").click();
    const dialog = page.getByTestId("edit-original-dialog");
    await expect(dialog).toBeVisible();
    const act = async (row: number, action: string) => {
      await page.getByTestId(`edit-original-action-${row}`).click();
      await page.getByTestId(`edit-original-action-${row}-${action}`).click();
      await expect(page.getByTestId(`edit-original-row-${row}`)).toHaveAttribute("data-action", action);
    };

    // A new paragraph after the one that holds "합계 12,500원" — both pages have such a line.
    await act(0, "insert_after");
    await expect(page.getByTestId("edit-original-find-0")).toHaveAttribute("placeholder", "기준 문단에 들어 있는 글");
    await page.getByTestId("edit-original-find-0").fill("합계 12,500원");
    await page.getByTestId("edit-original-replace-0").fill("부가세 별도");
    await page.getByTestId("edit-original-preview-run").click();
    await expect(page.getByTestId("edit-original-problem-0")).toHaveText("이 글이 든 곳이 2곳입니다. 더 길게 적거나 몇 번째인지 넣으세요.");
    await expect(page.getByTestId("edit-original-count-0")).toHaveText("확인 필요");
    await expect(page.getByTestId("edit-original-preview")).toHaveAttribute("data-problems", "1");
    // Nothing is saved while an edit is not settled.
    await expect(page.getByTestId("edit-original-save")).toBeDisabled();

    // The second of the two; the first line of page 1 goes; and a row edit on a text outside tables says why.
    await page.getByTestId("edit-original-occurrence-0").fill("2");
    await page.getByTestId("edit-original-add").click();
    await act(1, "delete_paragraph");
    await expect(page.getByTestId("edit-original-replace-1")).toHaveCount(0);
    await page.getByTestId("edit-original-find-1").fill("1쪽의 텍스트 레이어");
    await page.getByTestId("edit-original-add").click();
    await act(2, "delete_row");
    await page.getByTestId("edit-original-find-2").fill("2쪽의 텍스트");
    await page.getByTestId("edit-original-preview-run").click();
    await expect(page.getByTestId("edit-original-problem-2")).toHaveText(ko.errors.reasons.edit_not_in_table);
    await expect(page.getByTestId("edit-original-count-0")).toHaveText("1곳");
    await expect(page.getByTestId("edit-original-save")).toBeDisabled();
    // Three rows with their fields stay inside the window (the rows wrap instead of widening the dialog).
    const box = (await dialog.boundingBox())!;
    const view = page.viewportSize()!;
    expect(box.x >= 0 && box.y >= 0 && box.x + box.width <= view.width && box.y + box.height <= view.height).toBe(true);
    expect(await dialog.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);

    await page.getByTestId("edit-original-remove-2").click();
    await page.getByTestId("edit-original-preview-run").click();
    const preview = page.getByTestId("edit-original-preview");
    await expect(preview).toHaveAttribute("data-total", "2");
    await expect(preview).toHaveAttribute("data-problems", "0");
    await expect(page.getByTestId("edit-original-change-0")).toHaveAttribute("data-action", "insert_after");
    await expect(page.getByTestId("edit-original-change-0")).toContainText("이 문단 뒤에 추가");
    await expect(page.getByTestId("edit-original-change-0")).toContainText("+ 부가세 별도");
    await expect(page.getByTestId("edit-original-change-1")).toContainText("문단 삭제");
    await expect(page.getByTestId("edit-original-change-1")).toContainText("1쪽의 텍스트 레이어입니다.");

    await page.getByTestId("edit-original-save").click();
    await expect(dialog).toHaveCount(0);
    await page.getByTestId("toast-action").first().click();
    await expect(page.getByTestId("viewer-name")).toContainText("개발계약서 (수정).docx");
    // Page 1 lost its first line; the new paragraph stands on page 2, after the total.
    const layer = page.getByTestId("viewer-text-layer");
    await expect(layer).toContainText("합계 12,500원");
    await expect(layer).not.toContainText("1쪽의 텍스트 레이어");
    await expect(layer).not.toContainText("부가세 별도");
  });
});
