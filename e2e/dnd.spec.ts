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

  test("the merge dialog keeps the typed name and a dropped file while the library refreshes behind it (DOC-7)", async ({ page }) => {
    const ids = await libraryWithDocs(page, ["C:\\scan\\계약서.pdf", "C:\\scan\\부속서.pdf"]);
    for (const id of ids) await page.getByTestId(`doc-select-${id}`).check();
    await page.getByTestId("library-merge").click();
    const dialog = page.getByTestId("merge-dialog");
    await expect(dialog.getByTestId("merge-list").locator("li")).toHaveCount(2);
    await page.getByTestId("merge-name").fill("2026 계약 묶음");
    // A file dropped on the dialog is imported first: every import event refetches the library list behind
    // the dialog, which used to reset the name and the list (the dropped document vanished again).
    await dropFiles(page, dialog, ["합의서.pdf"]);
    await expect(dialog.getByTestId("merge-list").locator("li")).toHaveCount(3, { timeout: 20_000 });
    await expect(dialog.getByTestId("merge-list")).toContainText("합의서.pdf");
    await expect(card(page, "합의서.pdf")).toHaveAttribute("data-doc-status", "rendered", { timeout: 15_000 });
    // Let the auto pipeline of the new document send its events too.
    await page.waitForTimeout(800);
    await expect(page.getByTestId("merge-name")).toHaveValue("2026 계약 묶음");
    await expect(dialog.getByTestId("merge-list").locator("li")).toHaveCount(3);
    await expect(dialog.getByTestId("merge-list")).toContainText("합의서.pdf");
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

  test("a file that is already in the library joins the project / reference group it is dropped on (PRJ-1)", async ({ page }) => {
    const [contract, annex, rules] = await libraryWithDocs(page, ["C:\\scan\\계약서.pdf", "C:\\scan\\부속서.pdf", "C:\\scan\\취업규칙.pdf"]);
    await page.getByTestId("nav-projects").click();
    await page.getByTestId("projects-new").click();
    await page.getByTestId("project-name").fill("드롭 프로젝트");
    await page.getByTestId("project-submit").click();
    await expect(page.getByTestId("project-title")).toContainText("드롭 프로젝트");
    // The 문서 tab. The file holds nothing new, so no import runs — the document it stands for joins the project
    // (the drop used to say "문서 1건을 추가했습니다" and add nothing).
    await expect(page.getByTestId("project-documents-empty")).toBeVisible();
    await dropFiles(page, page.getByTestId("project-documents-drop"), ["계약서.pdf"]);
    await expect(page.getByTestId(`project-doc-${contract}`)).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId(`project-doc-${contract}`)).toHaveAttribute("data-kind", "work");
    await expect(page.getByTestId("project-documents-table").locator("tbody tr")).toHaveCount(1);
    // A project card.
    await page.getByTestId("project-back").click();
    const projectCard = page.locator('[data-testid^="project-card-"]').first();
    await expect(projectCard).toContainText(tpl(ko.projects.card.documents, { count: 1 }));
    await dropFiles(page, projectCard, ["부속서.pdf"]);
    await expect(projectCard).toContainText(tpl(ko.projects.card.documents, { count: 2 }), { timeout: 15_000 });
    await projectCard.getByTestId("project-open").click();
    await expect(page.getByTestId(`project-doc-${annex}`)).toBeVisible();
    // A common reference group: the document joins it as 참고 자료.
    await page.getByTestId("project-back").click();
    await page.getByTestId("projects-common-reference-open").click();
    await expect(page.getByTestId("page-common-reference")).toBeVisible();
    await page.getByTestId("reference-group-new").click();
    await page.getByTestId("reference-name").fill("규정");
    await page.getByTestId("reference-name-save").click();
    const group = page.getByTestId("reference-group-item").filter({ hasText: "규정" });
    await expect(group).toBeVisible();
    await dropFiles(page, group, ["취업규칙.pdf"]);
    await expect(group).toContainText(tpl(ko.projects.reference.docs_count, { count: 1 }), { timeout: 15_000 });
    await page.getByTestId("nav-library").click();
    await expect(page.getByTestId(`doc-card-${rules}`).locator('[data-testid^="doc-role-"]')).toHaveText(ko.library.role_knowledge);
  });

  // 0.8.0: an import where every file is registered already runs when it has somewhere to put them — the job
  // names them in `ImportReport.existing` — and the drop stands for imported ∪ existing.
  test("a drop of registered files runs an attach-only import; a mixed drop attaches the new and the registered ones; the assistant takes a registered file", async ({ page }) => {
    test.setTimeout(60_000);
    const [contract, annex] = await libraryWithDocs(page, ["C:\\scan\\계약서.pdf", "C:\\scan\\부속서.pdf"]);
    await page.getByTestId("nav-projects").click();
    await page.getByTestId("projects-new").click();
    await page.getByTestId("project-name").fill("드롭 프로젝트");
    await page.getByTestId("project-submit").click();
    await expect(page.getByTestId("project-title")).toContainText("드롭 프로젝트");
    const rows = page.getByTestId("project-documents-table").locator("tbody tr");

    // Only a registered file: nothing is imported, the job attaches the document and the drop counts it.
    await dropFiles(page, page.getByTestId("project-documents-drop"), ["계약서.pdf"]);
    await expect(page.getByTestId(`project-doc-${contract}`)).toBeVisible({ timeout: 15_000 });
    await expect(rows).toHaveCount(1);
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.projects.documents.added, { count: 1 }));

    // A registered file and a new one in one drop: both join (imported ∪ existing).
    await dropFiles(page, page.getByTestId("project-documents-drop"), ["부속서.pdf", "신규 계획서.pdf"]);
    await expect(page.getByTestId(`project-doc-${annex}`)).toBeVisible({ timeout: 20_000 });
    await expect(rows).toHaveCount(3, { timeout: 20_000 });
    await expect(page.getByTestId("project-documents-table")).toContainText("신규 계획서.pdf");
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.projects.documents.added, { count: 2 }));

    // 작업 현황: the attach-only run is an import job of its own ("문서 1개": nothing imported, one skipped).
    await page.getByTestId("nav-jobs").click();
    await page.getByTestId("jobs-tab-finished").click();
    const attachOnly = page.locator('[data-job-kind="document_import"][data-job-subject="1 documents"]');
    await expect(attachOnly).toHaveCount(1);
    await expect(attachOnly).toHaveAttribute("data-job-state", "done");
    await expect(attachOnly).toContainText(tpl(ko.jobs.subject.documents, { count: 1 }));
    await expect(attachOnly.getByTestId("job-detail")).toContainText(tpl(ko.jobs.detail.import_summary, { imported: 0, relinked: 0, failed: 0, skipped: 1 }));
    // The library still holds each document once.
    await page.getByTestId("nav-library").click();
    await expect(card(page, "계약서.pdf")).toHaveCount(1);
    await expect(card(page, "부속서.pdf")).toHaveCount(1);
    await expect(card(page, "신규 계획서.pdf")).toHaveCount(1);

    // No target to attach to (the assistant): no import job runs, the registered document becomes an attachment.
    await page.getByTestId("nav-assistant").click();
    await expect(page.getByTestId("page-assistant")).toBeVisible();
    await dropFiles(page, page.getByTestId("assistant-messages"), ["계약서.pdf"]);
    await expect(page.getByTestId(`assistant-attachment-${contract}`)).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("assistant-attachments")).toContainText("계약서.pdf");
    await page.getByTestId("nav-jobs").click();
    await page.getByTestId("jobs-tab-finished").click();
    await expect(page.locator('[data-job-kind="document_import"]')).toHaveCount(3);
  });

  test("a new file dropped on the 문서 tab: its type and the checklist follow the automatic classification without leaving the screen (PRJ-10)", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-projects").click();
    await page.getByTestId("projects-new").click();
    await page.getByTestId("project-name").fill("자동 분류 사업");
    await page.getByTestId("project-template").click();
    await page.getByRole("option", { name: ko.projects.templates.builtin.internal_dev }).click();
    await page.getByTestId("project-submit").click();
    await expect(page.getByTestId("page-project")).toBeVisible();
    await expect(page.getByTestId("checklist-srs")).toHaveAttribute("data-missing", "true");
    // The import attaches the file to the project; the classification that follows only sends document /
    // pipeline events — the tab used to keep the type "—" and the checklist "누락" until it was opened again.
    await dropFiles(page, page.getByTestId("project-documents-drop"), ["요구사항정의서-v2.pdf"]);
    const row = page.getByTestId("project-documents-table").locator("tbody tr");
    await expect(row).toHaveCount(1, { timeout: 20_000 });
    await expect(row.getByTestId("project-doc-type")).toHaveText(ko.documents.doc_types.srs, { timeout: 15_000 });
    await expect(page.getByTestId("checklist-srs")).toHaveAttribute("data-missing", "false");
  });
});
