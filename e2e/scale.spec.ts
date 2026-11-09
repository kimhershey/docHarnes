import { expect, test, type Locator, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { card, createWorkspace, docId, freshApp, importViaLibrary } from "./helpers";

// UI 배율 (설정 > 일반, Ctrl+Shift+=) scales the root font size. Boxes that were fixed in px — library cards and
// list rows, viewer thumbnails, review rows — kept their size while the text grew and clipped it from 125 % up.
// Everything is rem-based or measured now; these checks run at the slider maximum (150 %) and at 100 %.

/** Moves the UI 배율 slider with the keyboard (End = 150 %, Home = 90 %) and waits for the root font size to follow. */
async function setScale(page: Page, percent: 90 | 150): Promise<void> {
  await page.getByTestId("nav-settings").click();
  await page.getByTestId("scale-slider").getByRole("slider").focus();
  await page.keyboard.press(percent === 150 ? "End" : "Home");
  await expect(page.getByTestId("scale-value")).toHaveText(`${percent}%`);
  await expect.poll(() => page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).fontSize))).toBe((16 * percent) / 100);
}

/** Nothing inside `el` is cut off: its content fits its own box vertically and every text descendant lies inside it. */
async function expectNotClipped(el: Locator): Promise<void> {
  const r = await el.evaluate((node) => {
    const box = node.getBoundingClientRect();
    let outside = 0;
    for (const child of Array.from(node.querySelectorAll("*"))) {
      if (!(child.textContent ?? "").trim() || getComputedStyle(child).display === "none") continue;
      const c = child.getBoundingClientRect();
      if (c.height === 0) continue;
      if (c.bottom > box.bottom + 1 || c.top < box.top - 1) outside++;
    }
    return { overflowY: node.scrollHeight - node.clientHeight, outside };
  });
  expect(r.overflowY, "content taller than its box").toBeLessThanOrEqual(1);
  expect(r.outside, "text descendants outside the box").toBe(0);
}

type Box = { x: number; y: number; width: number; height: number };

/** Boxes of `items` (top to bottom) and the assertion that no two of them overlap vertically. */
async function expectStacked(items: Locator[]): Promise<Box[]> {
  const boxes: Box[] = [];
  for (const it of items) boxes.push((await it.boundingBox())!);
  boxes.sort((a, b) => a.y - b.y);
  for (let i = 1; i < boxes.length; i++) {
    const prev = boxes[i - 1]!;
    const cur = boxes[i]!;
    if (Math.abs(cur.y - prev.y) < 2) continue; // same row of a grid
    expect(cur.y, `row ${i} overlaps the row above`).toBeGreaterThanOrEqual(prev.y + prev.height - 1);
  }
  return boxes;
}

test.describe("UI 배율", () => {
  test("150 %: library cards, list rows, viewer thumbnails, review rows and the bars keep their text inside", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\영수증-1p.jpg", "D:\\scan\\세금계산서-2p.pdf", "D:\\scan\\견적서.hwpx"], { autoLevel: "none" });
    const invoiceId = await docId(page, "세금계산서-2p.pdf");
    for (const name of ["영수증-1p.jpg", "세금계산서-2p.pdf"]) {
      await page.getByTestId(`doc-select-${await docId(page, name)}`).check();
    }
    await page.getByTestId("library-process").click();
    await page.getByTestId("library-process-standard").click();
    await expect(card(page, "영수증-1p.jpg")).toHaveAttribute("data-doc-status", "extracted", { timeout: 15_000 });
    await page.getByTestId("library-clear-selection").click();

    await setScale(page, 150);

    // Grid: three cards no longer fit one row at this scale; every card shows its title, meta line and badges inside its box.
    await page.getByTestId("nav-library").click();
    await page.getByTestId("library-view-grid").click();
    const cards = page.locator('[data-testid^="doc-card-"]');
    await expect(cards).toHaveCount(3);
    for (const c of await cards.all()) {
      await expect(c.getByText(/쪽 · /)).toBeVisible();
      await expect(c.getByTestId(/doc-status-/)).toBeVisible();
      await expectNotClipped(c);
    }
    const cardBoxes = await expectStacked(await cards.all());
    expect(new Set(cardBoxes.map((b) => Math.round(b.y))).size, "cards wrap to more than one row").toBeGreaterThanOrEqual(2);

    // List: rows are measured, not fixed, so they neither overlap nor clip their cells.
    await page.getByTestId("library-view-list").click();
    const rows = page.locator('[data-testid^="doc-row-"]');
    await expect(rows).toHaveCount(3);
    for (const r of await rows.all()) await expectNotClipped(r);
    await expectStacked(await rows.all());

    // Viewer: thumbnail rows grow with the scale; page 1's number is not hidden under page 2.
    await page.getByTestId(`doc-open-${invoiceId}`).click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();
    const thumb1 = page.getByTestId("viewer-thumb-1");
    const thumb2 = page.getByTestId("viewer-thumb-2");
    await expect(thumb1.getByText("1", { exact: true })).toBeVisible();
    await expect(thumb2.getByText("2", { exact: true })).toBeVisible();
    await expectStacked([thumb1, thumb2]);
    await expectNotClipped(thumb1);

    // Review queue rows.
    await page.getByTestId("nav-review").click();
    const items = page.locator('[data-testid^="review-item-"]');
    await expect(items.first()).toBeVisible({ timeout: 15_000 });
    const firstItems = (await items.all()).slice(0, 5);
    for (const it of firstItems) await expectNotClipped(it);
    await expectStacked(firstItems);

    // Top bar and status bar.
    await expectNotClipped(page.getByTestId("topbar"));
    await expectNotClipped(page.getByTestId("statusbar"));
    expect(await page.getByTestId("statusbar").locator("kbd, span, a").evaluateAll((els) => els.filter((e) => e.scrollWidth > e.clientWidth + 1 && getComputedStyle(e).textOverflow !== "ellipsis").length)).toBe(0);
  });

  test("100 %: a card whose badges wrap to a second row still shows its title and meta line", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    // Default auto-processing adds the doc-type badge: HWPX · 렌더됨 · 변환본 · 기타 is two badge rows on a 12.5 rem card.
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx"]);
    const c = card(page, "견적서.hwpx");
    await expect(c.getByTestId(/doc-type-/)).toBeVisible({ timeout: 15_000 });
    const title = c.getByText("견적서.hwpx", { exact: true });
    await expect(title).toBeVisible();
    expect((await title.boundingBox())!.height, "title line is not squeezed").toBeGreaterThan(12);
    await expect(c.getByText(/쪽 · /)).toBeVisible();
    await expectNotClipped(c);
  });
});

test.describe("UI 배율 — 설정", () => {
  test("the slider's maximum is the 150 % the layout is checked at", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await setScale(page, 150);
    await expect(page.getByTestId("scale-value")).toHaveText(`150%`);
    expect(ko.settings.general.scale).toBe("UI 배율");
  });
});
