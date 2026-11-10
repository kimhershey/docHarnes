import { expect, test, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { card, createWorkspace, docId, freshApp, importViaLibrary, tpl } from "./helpers";

// 0.5.0 참고 자료 (plan §2) against the browser mock: the common reference material pinned on the projects
// screen (groups, the import wizard, documents, the 사용 toggle), a project's 참고 자료 tab (group created on
// demand, library picker, wizard pinned to the group, deleted with the project), the "프로젝트에 넣기" dialog
// (target / kind / suggestion / inline project / common group), the assistant's project scope (the mock search
// honours `project_id`) and the advanced panel under 설정 > 참고 자료. Korean morphology / recall are covered by
// the Rust golden-set tests.

async function newProject(page: Page, name: string, keywords?: string): Promise<string> {
  await page.getByTestId("projects-new").click();
  await page.getByTestId("project-name").fill(name);
  if (keywords) await page.getByTestId("project-keywords").fill(keywords);
  await page.getByTestId("project-submit").click();
  // `page-project` is also the loading / not-found frame: wait for the loaded project (its id attribute).
  await expect(page.getByTestId("page-project")).toHaveAttribute("data-project-id", /.+/);
  await expect(page.getByTestId("project-title")).toHaveText(name);
  return (await page.getByTestId("page-project").getAttribute("data-project-id"))!;
}

const answerCards = (page: Page) => page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]');

test.describe("0.5.0 reference material", () => {
  test("common reference material: groups, the import wizard, documents and the 사용 toggle", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-projects").click();
    const pinned = page.getByTestId("projects-common-reference");
    await expect(pinned).toHaveAttribute("data-groups", "0");
    await pinned.click();
    await expect(page.getByTestId("page-common-reference")).toBeVisible();
    await expect(page.getByTestId("reference-groups-empty")).toBeVisible();

    // 새 묶음, then 파일에서 가져오기 into it (the wizard offers the common groups, the new one preselected).
    await page.getByTestId("reference-group-new").click();
    await page.getByTestId("reference-name").fill("사내 규정");
    await page.getByTestId("reference-name-save").click();
    const item = page.locator('[data-testid="reference-group-item"][data-group-name="사내 규정"]');
    await expect(item).toBeVisible();
    await expect(page.getByTestId("reference-group-title")).toHaveText("사내 규정");
    await expect(page.getByTestId("reference-empty")).toBeVisible();
    await page.getByTestId("reference-ingest").click();
    await expect(page.getByTestId("ingest-wizard")).toBeVisible();
    await page.getByTestId("ingest-paths").fill("D:\\규정\\출장비규정-4p.pdf\nD:\\규정\\정보보안지침.pdf");
    await page.getByTestId("ingest-next").click();
    await expect(page.getByTestId("ingest-collection")).toContainText("사내 규정");
    await page.getByTestId("ingest-preview").click();
    await expect(page.getByTestId("ingest-plan")).toContainText(tpl(ko.knowledge.wizard.summary, { new: 2, relink: 0, dup: 0, registered: 0, unsupported: 0 }));
    await page.getByTestId("ingest-start").click();
    await expect(page.getByTestId("ingest-wizard")).toBeHidden();
    await expect(page.getByTestId("reference-table").locator("tbody tr")).toHaveCount(2, { timeout: 20_000 });
    await expect(item).toContainText(tpl(ko.projects.reference.docs_count, { count: 2 }));

    // 이름 변경 · 사용 toggle · 제거.
    await page.getByTestId("reference-group-rename").click();
    await page.getByTestId("reference-name").fill("사내 규정집");
    await page.getByTestId("reference-name-save").click();
    await expect(page.getByTestId("reference-group-title")).toHaveText("사내 규정집");
    const renamed = page.locator('[data-testid="reference-group-item"][data-group-name="사내 규정집"]');
    await expect(renamed).toHaveAttribute("data-group-active", "true");
    await page.getByTestId("reference-group-active").click();
    await expect(renamed).toHaveAttribute("data-group-active", "false");
    await page.getByTestId("reference-remove").first().click();
    await expect(page.getByTestId("reference-table").locator("tbody tr")).toHaveCount(1);
    await expect(page.getByTestId("toaster")).toContainText(ko.projects.reference.removed);

    // The pinned entry counts groups and documents; 묶음 삭제 keeps the documents in the library.
    await page.getByTestId("common-reference-back").click();
    await expect(page.getByTestId("page-projects")).toBeVisible();
    await expect(pinned).toHaveAttribute("data-groups", "1");
    await expect(pinned).toHaveAttribute("data-documents", "1");
    await pinned.click();
    await page.getByTestId("reference-group-delete").click();
    await page.getByTestId("confirm-ok").click();
    await expect(page.getByTestId("reference-groups-empty")).toBeVisible();
    await page.getByTestId("nav-library").click();
    await expect(card(page, "출장비규정-4p.pdf")).toBeVisible();
    await expect(card(page, "정보보안지침.pdf")).toBeVisible();
  });

  test("project 참고 자료 tab: the group is created on demand, takes library documents and files, and goes with the project", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\요구사항정의서-v1.2.pdf", "D:\\scan\\정보보안지침.pdf"], { autoLevel: "none" });
    const srs = await docId(page, "요구사항정의서-v1.2.pdf");
    const guide = await docId(page, "정보보안지침.pdf");
    await page.getByTestId("nav-projects").click();
    await newProject(page, "한빛 포털 구축");
    // 업무 문서 on the 문서 tab …
    await page.getByTestId("project-add-documents").click();
    await page.getByTestId(`project-add-doc-${srs}`).check();
    await page.getByTestId("project-add-done").click();
    await expect(page.getByTestId(`project-doc-${srs}`)).toBeVisible();
    // … 참고 자료 on their own tab, whose group the backend creates on the first visit.
    await page.getByTestId("project-tab-reference").click();
    const tab = page.getByTestId("project-reference");
    await expect(tab).toBeVisible();
    await expect(tab).toHaveAttribute("data-group-id", /.+/);
    await expect(page.getByTestId("project-reference-empty")).toBeVisible();
    await page.getByTestId("project-reference-add").click();
    await expect(page.getByTestId("project-reference-pick-dialog")).toBeVisible();
    await page.getByTestId(`project-reference-pick-doc-${guide}`).check();
    await page.getByTestId("project-reference-pick-done").click();
    await expect(page.getByTestId(`project-reference-doc-${guide}`)).toBeVisible();
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.projects.reference.added, { count: 1 }));
    // The wizard is pinned to the project's group.
    await page.getByTestId("project-reference-ingest").click();
    await expect(page.getByTestId("ingest-wizard")).toBeVisible();
    await page.getByTestId("ingest-paths").fill("D:\\규정\\출장비규정-4p.pdf");
    await page.getByTestId("ingest-next").click();
    await expect(page.getByTestId("ingest-collection-locked")).toHaveText("한빛 포털 구축");
    await page.getByTestId("ingest-preview").click();
    await expect(page.getByTestId("ingest-plan")).toBeVisible();
    await page.getByTestId("ingest-start").click();
    await expect(page.getByTestId("ingest-wizard")).toBeHidden();
    await expect(page.getByTestId("project-reference-table").locator("tbody tr")).toHaveCount(2, { timeout: 20_000 });
    // The project's group is not one of the common groups.
    await page.getByTestId("project-back").click();
    await expect(page.getByTestId("projects-common-reference")).toHaveAttribute("data-groups", "0");
    // Deleting the project deletes its group; the documents stay in the library.
    await page.locator('[data-testid^="project-card-"][data-project-name="한빛 포털 구축"]').getByTestId("project-open").click();
    await page.getByTestId("project-delete").click();
    await page.getByTestId("confirm-ok").click();
    await expect(page.getByTestId("page-projects")).toBeVisible();
    const kinds = await page.evaluate(() => {
      const all = JSON.parse(localStorage.getItem("doculens.mock.knowledge") ?? "{}") as Record<string, { collections: { kind: string }[] }>;
      return Object.values(all).flatMap((s) => s.collections.map((c) => c.kind));
    });
    expect(kinds).toEqual([]);
    await page.getByTestId("nav-library").click();
    await expect(card(page, "정보보안지침.pdf")).toBeVisible();
    await expect(card(page, "출장비규정-4p.pdf")).toBeVisible();
  });

  test("프로젝트에 넣기: the suggestion picks the project, 참고 자료 / 업무 문서 kinds, an inline new project and a common group", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\개발계약서-한빛.pdf", "D:\\scan\\견적서-1p.jpg"], { autoLevel: "none" });
    const contract = await docId(page, "개발계약서-한빛.pdf");
    const estimate = await docId(page, "견적서-1p.jpg");
    await page.getByTestId("nav-projects").click();
    const hanbitId = await newProject(page, "한빛상사 거래처 등록", "개발계약서, 한빛");
    await page.getByTestId("project-back").click();
    await newProject(page, "신사옥 공사");
    await page.getByTestId("project-back").click();

    // `?classify=` (the library's "프로젝트로 분류" hand-off): the suggestion runs and picks 한빛상사 for the contract.
    await page.goto(`/#/projects?classify=${contract},${estimate}`);
    const dialog = page.getByTestId("put-dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId("put-suggestions")).toBeVisible({ timeout: 10_000 });
    await expect(dialog.getByTestId(`put-suggestion-${contract}`)).toHaveAttribute("data-suggested", hanbitId);
    await expect(dialog.getByTestId(`put-suggestion-${estimate}`)).toHaveAttribute("data-suggested", "");
    await expect(dialog.getByTestId(`put-project-${hanbitId}`)).toBeChecked();
    await expect(dialog.getByTestId("put-kind-work")).toBeChecked();
    // Both go in as the project's 참고 자료.
    await dialog.getByTestId("put-kind-reference").check();
    await dialog.getByTestId("put-confirm").click();
    await expect(dialog).toBeHidden();
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.projects.put.done_reference, { name: "한빛상사 거래처 등록", count: 2 }));
    await page.goto(`/#/projects/${hanbitId}?tab=reference`);
    await expect(page.getByTestId("project-reference-table").locator("tbody tr")).toHaveCount(2, { timeout: 10_000 });
    await page.getByTestId("project-tab-documents").click();
    await expect(page.getByTestId("project-documents-table")).toHaveCount(0);

    // The estimate also goes to the common reference material, as a new group (업무 문서 is not offered there).
    await page.goto(`/#/projects?put=${estimate}`);
    await expect(dialog).toBeVisible();
    await dialog.getByTestId("put-target-common").check();
    await expect(dialog.getByTestId("put-kind-work")).toBeDisabled();
    await expect(dialog.getByTestId("put-kind-reference")).toBeChecked();
    await dialog.getByTestId("put-group-new-name").fill("증빙");
    await dialog.getByTestId("put-confirm").click();
    await expect(dialog).toBeHidden();
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.projects.put.done_reference, { name: "증빙", count: 1 }));
    await expect(page.getByTestId("projects-common-reference")).toHaveAttribute("data-groups", "1");

    // An inline new project takes the contract as 업무 문서.
    await page.goto(`/#/projects?put=${contract}`);
    await expect(dialog).toBeVisible();
    await dialog.getByTestId("put-project-new").click();
    await dialog.getByTestId("put-project-new-name").fill("신규 과제");
    await dialog.getByTestId("put-project-create").click();
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.projects.put.created, { name: "신규 과제" }));
    await expect(dialog.locator('input[name="put-project"]:checked')).toHaveCount(1);
    await dialog.getByTestId("put-kind-work").check();
    await dialog.getByTestId("put-confirm").click();
    await expect(dialog).toBeHidden();
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.projects.put.done_work, { name: "신규 과제", count: 1 }));
    const created = page.locator('[data-testid^="project-card-"][data-project-name="신규 과제"]');
    await expect(created.getByText(tpl(ko.projects.card.documents, { count: 1 }))).toBeVisible();
  });

  test("assistant: the project scope answers from the project's 업무 문서 and 참고 자료 only", async ({ page }) => {
    // Three streamed answers plus the project set-up do not fit the default 30 s budget.
    test.setTimeout(90_000);
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx", "D:\\규정\\정보보안지침.pdf"]);
    const estimate = await docId(page, "견적서.hwpx");
    const guide = await docId(page, "정보보안지침.pdf");
    await page.getByTestId("nav-projects").click();
    const pid = await newProject(page, "한빛 포털 구축");
    await page.getByTestId("project-add-documents").click();
    await page.getByTestId(`project-add-doc-${estimate}`).check();
    await page.getByTestId("project-add-done").click();
    await expect(page.getByTestId(`project-doc-${estimate}`)).toBeVisible();

    // The open project is the assistant's project scope: the guideline is outside it, the estimate inside.
    await page.getByTestId("nav-assistant").click();
    await page.getByTestId("assistant-scope").click();
    await page.getByTestId("assistant-scope-project").click();
    await page.getByTestId("assistant-input").fill("정보보안지침 내용은?");
    await page.getByTestId("assistant-send").click();
    await expect(answerCards(page).first()).toHaveAttribute("data-answer-state", "no_evidence", { timeout: 15_000 });
    // ("합계 금액" would be read as an aggregate command; a plain question keeps the answer path.)
    await page.getByTestId("assistant-input").fill("견적서 내용은?");
    await page.getByTestId("assistant-send").click();
    const second = answerCards(page).nth(1);
    await expect(second).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(second.getByTestId("citation-chip").first()).toBeVisible();

    // Once the guideline is the project's 참고 자료 the same question is answered within the project scope.
    await page.goto(`/#/projects/${pid}?tab=reference`);
    await expect(page.getByTestId("project-reference")).toHaveAttribute("data-group-id", /.+/, { timeout: 10_000 });
    await page.getByTestId("project-reference-add").click();
    await page.getByTestId(`project-reference-pick-doc-${guide}`).check();
    await page.getByTestId("project-reference-pick-done").click();
    await expect(page.getByTestId(`project-reference-doc-${guide}`)).toBeVisible();
    await page.getByTestId("nav-assistant").click();
    await page.getByTestId("assistant-scope").click();
    await page.getByTestId("assistant-scope-project").click();
    await page.getByTestId("assistant-input").fill("정보보안지침 내용은?");
    await page.getByTestId("assistant-send").click();
    await expect(answerCards(page)).toHaveCount(3, { timeout: 15_000 });
    const third = answerCards(page).nth(2);
    await expect(third).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(third.getByTestId("citation-chip").first()).toHaveAttribute("title", /정보보안지침/);
  });

  test("settings > 참고 자료(고급): index statistics, glossary, question test and the answer log", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서-3p.pdf"]);
    await page.getByTestId("nav-assistant").click();
    await page.getByTestId("assistant-input").fill("합계 금액은?");
    await page.getByTestId("assistant-send").click();
    await expect(answerCards(page).first()).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });

    await page.getByTestId("nav-settings").click();
    await page.getByTestId("settings-tab-reference").click();
    const panel = page.getByTestId("reference-advanced");
    await expect(panel).toBeVisible();
    await expect(page.getByTestId("kb-stat-documents")).toHaveText("1");
    await expect(page.getByTestId("kb-stat-indexed")).toHaveText("1", { timeout: 15_000 });
    await expect(page.getByTestId("reference-section-embedding")).toHaveAttribute("data-open", "false");
    await page.getByTestId("reference-section-toggle-glossary").click();
    await page.getByTestId("glossary-add").click();
    await page.getByTestId("glossary-term").fill("출장비");
    await page.getByTestId("glossary-synonyms").fill("여비, 출장 경비");
    await page.getByTestId("glossary-save").click();
    await expect(page.locator('[data-testid="glossary-row"][data-term="출장비"]')).toContainText("여비");
    await page.getByTestId("reference-section-toggle-test").click();
    await page.getByTestId("kb-test-input").fill("합계");
    await page.getByTestId("kb-test-run").click();
    await expect(page.getByTestId("kb-test-hit").first()).toBeVisible();
    await page.getByTestId("reference-section-toggle-answers").click();
    await expect(page.getByTestId("knowledge-answers").getByTestId("answer-card")).toHaveCount(1);
    // The shortcut leads to the common reference material on the projects screen.
    await page.getByTestId("reference-open-common").click();
    await expect(page.getByTestId("page-common-reference")).toBeVisible();
  });
});
