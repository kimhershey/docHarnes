import { expect, test, type Locator, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { card, createWorkspace, freshApp, importViaLibrary, openSettingsTab, tpl } from "./helpers";

// Drag-and-drop of library documents (pointer-based in-app drags) and browser file drops onto the
// screens that add documents (design §3.4 끌어다 놓기, v1.1 follow-up).

/** Presses on `source`, moves past the drag threshold, hovers `target` for `hoverMs` and releases. */
async function dragTo(page: Page, source: Locator, target: Locator, hoverMs = 0): Promise<void> {
  const s = (await source.boundingBox())!;
  const t = (await target.boundingBox())!;
  await page.mouse.move(s.x + s.width / 2, s.y + s.height / 2);
  await page.mouse.down();
  await page.mouse.move(s.x + s.width / 2 + 24, s.y + s.height / 2 + 24, { steps: 4 });
  await expect(page.getByTestId("dnd-ghost")).toBeVisible();
  await page.mouse.move(t.x + t.width / 2, t.y + t.height / 2, { steps: 10 });
  if (hoverMs > 0) await page.waitForTimeout(hoverMs);
  await page.mouse.up();
}

/**
 * Drags `source` onto the assistant: since 0.5.0 (plan §3) the menu entry only spring-loads the screen,
 * the page itself (its message list) takes the drop.
 */
async function dragToAssistant(page: Page, source: Locator): Promise<void> {
  const s = (await source.boundingBox())!;
  await page.mouse.move(s.x + s.width / 2, s.y + s.height / 2);
  await page.mouse.down();
  await page.mouse.move(s.x + s.width / 2 + 24, s.y + s.height / 2 + 24, { steps: 4 });
  await expect(page.getByTestId("dnd-ghost")).toBeVisible();
  const nav = (await page.getByTestId("nav-assistant").boundingBox())!;
  await page.mouse.move(nav.x + nav.width / 2, nav.y + nav.height / 2, { steps: 10 });
  await expect(page.getByTestId("page-assistant")).toBeVisible({ timeout: 5_000 });
  const body = (await page.getByTestId("assistant-messages").boundingBox())!;
  await page.mouse.move(body.x + body.width / 2, body.y + body.height / 2, { steps: 10 });
  await expect(page.getByTestId("assistant-drop-hint")).toHaveAttribute("data-over", "true");
  await page.mouse.up();
}

/** Browser-preview file drop: a `drop` with `dataTransfer.files` dispatched at the target's centre. */
async function dropFiles(page: Page, target: Locator, names: string[]): Promise<void> {
  const box = (await target.boundingBox())!;
  await target.evaluate(
    (el, { names, x, y }) => {
      const dt = new DataTransfer();
      for (const n of names) dt.items.add(new File(["%PDF-1.4"], n, { type: "application/pdf" }));
      for (const type of ["dragover", "drop"]) {
        el.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y, dataTransfer: dt }));
      }
    },
    { names, x: box.x + box.width / 2, y: box.y + box.height / 2 },
  );
}

async function libraryWithDocs(page: Page, paths: string[]): Promise<string[]> {
  await freshApp(page);
  await createWorkspace(page);
  await importViaLibrary(page, paths, { autoLevel: "none" });
  const ids: string[] = [];
  for (const p of paths) {
    const id = (await card(page, p.split(/[\\/]/).pop()!).getAttribute("data-testid"))!.replace("doc-card-", "");
    ids.push(id);
  }
  return ids;
}

test.describe("document drag and drop", () => {
  test("a card carried to the assistant (spring-loading on its menu entry) becomes an attachment (scope) and can be removed", async ({ page }) => {
    const [id] = await libraryWithDocs(page, ["C:\\scan\\견적서_한빛.pdf"]);
    await dragToAssistant(page, page.getByTestId(`doc-card-${id}`));
    await expect(page.getByTestId("page-assistant")).toBeVisible();
    await expect(page.getByTestId("assistant-attachments")).toContainText("견적서_한빛.pdf");
    await expect(page.getByTestId("assistant-scope")).toContainText(tpl(ko.assistant.scope.attached, { count: 1 }));
    // The user message records the attachments.
    await page.getByTestId("assistant-input").fill("/테마 다크");
    await page.getByTestId("assistant-send").click();
    await expect(page.getByTestId("assistant-message-attachments")).toContainText("견적서_한빛.pdf");
    await page.getByTestId(`assistant-attachment-remove-${id}`).click();
    await expect(page.getByTestId("assistant-attachments")).toHaveCount(0);
    await expect(page.getByTestId("assistant-scope")).toContainText(ko.assistant.scope.all);
  });

  test("dragging a selected card carries the whole selection", async ({ page }) => {
    const ids = await libraryWithDocs(page, ["C:\\scan\\a.pdf", "C:\\scan\\b.pdf"]);
    for (const id of ids) await page.getByTestId(`doc-select-${id}`).check();
    const s = (await page.getByTestId(`doc-card-${ids[0]}`).boundingBox())!;
    await page.mouse.move(s.x + s.width / 2, s.y + s.height / 2);
    await page.mouse.down();
    await page.mouse.move(s.x + s.width / 2 + 30, s.y + s.height / 2 + 30, { steps: 4 });
    await expect(page.getByTestId("dnd-ghost")).toContainText(tpl(ko.dnd.ghost_many, { count: 2 }));
    const nav = (await page.getByTestId("nav-assistant").boundingBox())!;
    await page.mouse.move(nav.x + nav.width / 2, nav.y + nav.height / 2, { steps: 10 });
    await expect(page.getByTestId("page-assistant")).toBeVisible({ timeout: 5_000 });
    const body = (await page.getByTestId("assistant-messages").boundingBox())!;
    await page.mouse.move(body.x + body.width / 2, body.y + body.height / 2, { steps: 10 });
    await page.mouse.up();
    await expect(page.getByTestId("assistant-attachments")).toContainText(tpl(ko.assistant.attachments.count, { count: 2 }));
  });

  test("resting on the 프로젝트 entry switches screens (spring-loading) so the card can be dropped on a project", async ({ page }) => {
    const [id] = await libraryWithDocs(page, ["C:\\scan\\계약서.pdf"]);
    await page.getByTestId("nav-projects").click();
    await page.getByTestId("projects-new").click();
    await page.getByTestId("project-name").fill("드롭 프로젝트");
    await page.getByTestId("project-submit").click();
    await expect(page.getByTestId("project-title")).toContainText("드롭 프로젝트");
    await page.getByTestId("project-back").click();
    await page.getByTestId("nav-library").click();
    const cardEl = page.getByTestId(`doc-card-${id}`);
    const s = (await cardEl.boundingBox())!;
    await page.mouse.move(s.x + s.width / 2, s.y + s.height / 2);
    await page.mouse.down();
    await page.mouse.move(s.x + s.width / 2 + 24, s.y + s.height / 2 + 24, { steps: 4 });
    const nav = (await page.getByTestId("nav-projects").boundingBox())!;
    await page.mouse.move(nav.x + nav.width / 2, nav.y + nav.height / 2, { steps: 10 });
    await expect(page.getByTestId("page-projects")).toBeVisible({ timeout: 5_000 });
    const target = page.locator('[data-testid^="project-card-"]').first();
    const tb = (await target.boundingBox())!;
    await page.mouse.move(tb.x + tb.width / 2, tb.y + tb.height / 2, { steps: 10 });
    await page.mouse.up();
    await expect(target).toContainText(tpl(ko.projects.card.documents, { count: 1 }));
    await target.getByTestId("project-open").click();
    await expect(page.getByTestId(`project-doc-${id}`)).toBeVisible();
  });

  test("a card dropped on the 문서 생성 entry is quoted as a draft source", async ({ page }) => {
    const [id] = await libraryWithDocs(page, ["C:\\scan\\개발계약서.pdf"]);
    await dragTo(page, page.getByTestId(`doc-card-${id}`), page.getByTestId("nav-generate"));
    await expect(page.getByTestId("page-generate")).toBeVisible();
    await expect(page.getByTestId("generate-sources")).toContainText("개발계약서.pdf");
  });

  test("a card dropped on 설정 > 양식·추출 항목 itself (after spring-loading) opens 양식 등록 on that document", async ({ page }) => {
    const [id] = await libraryWithDocs(page, ["C:\\scan\\신청서.pdf"]);
    // 설정 remembers its last tab: visit 양식·추출 항목 once so spring-loading the 설정 entry lands there.
    await openSettingsTab(page, "schemas");
    await page.getByTestId("nav-library").click();
    const cardEl = page.getByTestId(`doc-card-${id}`);
    await expect(cardEl).toBeVisible();
    const s = (await cardEl.boundingBox())!;
    await page.mouse.move(s.x + s.width / 2, s.y + s.height / 2);
    await page.mouse.down();
    await page.mouse.move(s.x + s.width / 2 + 24, s.y + s.height / 2 + 24, { steps: 4 });
    const nav = (await page.getByTestId("nav-settings").boundingBox())!;
    await page.mouse.move(nav.x + nav.width / 2, nav.y + nav.height / 2, { steps: 10 });
    await expect(page.getByTestId("page-schemas")).toBeVisible({ timeout: 5_000 });
    // The drop lands on the screen body, not on the menu entry (user report 2026-11-09).
    const body = (await page.getByTestId("schemas-list").boundingBox())!;
    await page.mouse.move(body.x + body.width / 2, body.y + body.height / 2, { steps: 10 });
    await expect(page.getByTestId("schemas-drop-hint")).toHaveAttribute("data-over", "true");
    await page.mouse.up();
    await expect(page.getByTestId("form-register-dialog")).toBeVisible();
    await expect(page.getByTestId("form-register-dialog")).toBeInViewport();
    await expect(page.getByTestId("form-register-doc")).toContainText("신청서.pdf");
  });

  test("a file dropped on 설정 > 양식·추출 항목 is imported and offered for registration", async ({ page }) => {
    await libraryWithDocs(page, ["C:\\scan\\계약서.pdf"]);
    await openSettingsTab(page, "schemas");
    await expect(page.getByTestId("page-schemas")).toBeVisible();
    await dropFiles(page, page.getByTestId("schemas-list"), ["휴가신청서.pdf"]);
    await expect(page.getByTestId("form-register-dialog")).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("form-register-dialog")).toBeInViewport();
    await expect(page.getByTestId("form-register-doc")).toContainText("휴가신청서.pdf", { timeout: 15_000 });
  });

  test("files dropped on a common reference group are imported as 참고 자료 of that group", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    // 공통 참고 자료 (plan §2) is the pinned card at the top of 프로젝트; a file dropped on one of its groups is imported into it.
    await page.getByTestId("nav-projects").click();
    await page.getByTestId("projects-common-reference-open").click();
    await expect(page.getByTestId("page-common-reference")).toBeVisible();
    await page.getByTestId("reference-group-new").click();
    await page.getByTestId("reference-name").fill("규정");
    await page.getByTestId("reference-name-save").click();
    const item = page.getByTestId("reference-group-item").filter({ hasText: "규정" });
    await expect(item).toBeVisible();
    await dropFiles(page, item, ["C:\\docs\\취업규칙.pdf"]);
    await expect(item).toContainText(tpl(ko.projects.reference.docs_count, { count: 1 }), { timeout: 20_000 });
    await page.getByTestId("nav-library").click();
    const imported = card(page, "취업규칙.pdf");
    await expect(imported).toBeVisible();
    await expect(imported.locator('[data-testid^="doc-role-"]')).toHaveText(ko.library.role_knowledge);
  });
});
