// 1.0.0 plan §12 / §12-B: "프로젝트에 넣기" with several projects — every document into every ticked
// project, or the sheet where each document has its own projects and kind, a membership is unticked to
// take the document out, the classify suggestion fills the ticks (the second candidate from the
// 2순위 추천 임계값 on) and one toast action undoes the whole assignment.
import { expect, test, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { card, createWorkspace, docId, freshApp, importViaLibrary, openAssistantScreen, tpl } from "./helpers";

async function newProject(page: Page, name: string, keywords?: string): Promise<string> {
  await page.getByTestId("nav-projects").click();
  await page.getByTestId("projects-new").click();
  await page.getByTestId("project-name").fill(name);
  if (keywords) await page.getByTestId("project-keywords").fill(keywords);
  await page.getByTestId("project-submit").click();
  await expect(page.getByTestId("page-project")).toHaveAttribute("data-project-id", /.+/);
  await expect(page.getByTestId("project-title")).toHaveText(name);
  const id = (await page.getByTestId("page-project").getAttribute("data-project-id"))!;
  await page.getByTestId("project-back").click();
  return id;
}

const projectCard = (page: Page, name: string) => page.locator(`[data-testid^="project-card-"][data-project-name="${name}"]`);
const documents = (count: number) => tpl(ko.projects.card.documents, { count });
const settingsValue = (page: Page, path: string) =>
  page.evaluate((p) => {
    const raw = window.localStorage.getItem("doculens.mock.settings");
    let v: unknown = raw ? JSON.parse(raw) : null;
    for (const part of p.split(".")) v = v && typeof v === "object" ? (v as Record<string, unknown>)[part] : undefined;
    return v;
  }, path);
/** The project names on a document's library badges ("업무 문서" and "참고 자료" alike). */
const badgeNames = (page: Page, id: string) => page.getByTestId(`doc-project-${id}`).evaluateAll((els) => els.map((e) => e.textContent?.trim() ?? "").sort());

test.describe("1.0.0 프로젝트에 넣기: several projects", () => {
  test("모두 같은 프로젝트에: two ticked projects both get the documents, a repeat changes nothing, and the toast undoes it", async ({ page }) => {
    test.setTimeout(120_000);
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서-1p.jpg", "D:\\scan\\개발계약서-한빛.pdf", "D:\\규정\\정보보안지침.pdf"], { autoLevel: "none" });
    const d1 = await docId(page, "견적서-1p.jpg");
    const d2 = await docId(page, "개발계약서-한빛.pdf");
    const d3 = await docId(page, "정보보안지침.pdf");
    const a = await newProject(page, "AAA");
    const b = await newProject(page, "BBB");
    const c = await newProject(page, "CCC");

    await page.goto(`/#/projects?put=${d1},${d2}`);
    const dialog = page.getByTestId("put-dialog");
    await expect(dialog).toHaveAttribute("data-mode", "same");
    // Nothing ticked: nothing to apply.
    await expect(dialog.getByTestId("put-confirm")).toBeDisabled();
    await dialog.getByTestId(`put-project-${a}`).check();
    await dialog.getByTestId(`put-project-${b}`).check();
    await expect(dialog.getByTestId("put-projects-select-count")).toHaveText(tpl(ko.common.selected_count, { count: 2 }));
    await dialog.getByTestId("put-confirm").click();
    await expect(dialog).toBeHidden();
    const toaster = page.getByTestId("toaster");
    const added = (name: string, count: number) => tpl(ko.projects.put.done_piece, { name, parts: tpl(ko.projects.put.done_added, { count }) });
    await expect(toaster).toContainText(added("AAA", 2));
    await expect(toaster).toContainText(added("BBB", 2));
    await expect(projectCard(page, "AAA").getByText(documents(2))).toBeVisible();
    await expect(projectCard(page, "BBB").getByText(documents(2))).toBeVisible();
    await expect(projectCard(page, "CCC").getByText(documents(0))).toBeVisible();

    // The same documents into AAA again: no duplicate, and the message says so.
    await page.goto(`/#/projects?put=${d1},${d2}`);
    await dialog.getByTestId(`put-project-${a}`).check();
    await dialog.getByTestId("put-confirm").click();
    await expect(toaster).toContainText(ko.projects.put.done_nothing);
    await expect(projectCard(page, "AAA").getByText(documents(2))).toBeVisible();

    // "전체 선택" ticks every listed project; the third document goes into all three — and comes back out with 되돌리기.
    await page.goto(`/#/projects?put=${d3}`);
    await expect(dialog.getByTestId("put-mode-sheet")).toHaveCount(0); // one document: no per-document mode
    await dialog.getByTestId("put-projects-select-all").click();
    await expect(dialog.locator('input[name="put-project"]:checked')).toHaveCount(3);
    await dialog.getByTestId("put-confirm").click();
    await expect(toaster).toContainText(added("CCC", 1));
    await expect(projectCard(page, "CCC").getByText(documents(1))).toBeVisible();
    await expect(projectCard(page, "AAA").getByText(documents(3))).toBeVisible();
    await toaster.getByTestId("toast-action").last().click();
    await expect(toaster).toContainText(ko.projects.put.undone);
    await expect(projectCard(page, "CCC").getByText(documents(0))).toBeVisible();
    await expect(projectCard(page, "AAA").getByText(documents(2))).toBeVisible();
    expect(c).not.toBe(a);
  });

  test("문서별로 지정: all in AAA, some in BBB or CCC as well, a membership unticked leaves after a confirmation, the kind is per document", async ({ page }) => {
    test.setTimeout(180_000);
    await freshApp(page);
    await createWorkspace(page);
    const names = ["요구사항-1.pdf", "요구사항-2.pdf", "설계서-1.pdf", "설계서-2.pdf", "시험결과-1.pdf", "시험결과-2.pdf"];
    await importViaLibrary(page, names.map((n) => `D:\\work\\${n}`), { autoLevel: "none" });
    const ids: string[] = [];
    for (const n of names) ids.push(await docId(page, n));
    const [d1, d2, d3, d4, d5, d6] = ids as [string, string, string, string, string, string];
    const a = await newProject(page, "AAA");
    const b = await newProject(page, "BBB");
    const c = await newProject(page, "CCC");
    const dialog = page.getByTestId("put-dialog");
    const toaster = page.getByTestId("toaster");

    // 요구사항-1 is in BBB already.
    await page.goto(`/#/projects?put=${d1}`);
    await dialog.getByTestId(`put-project-${b}`).check();
    await dialog.getByTestId("put-confirm").click();
    await expect(toaster).toContainText(tpl(ko.projects.put.done_work, { name: "BBB", count: 1 }));

    await page.goto(`/#/projects?put=${ids.join(",")}`);
    await dialog.getByTestId("put-mode-sheet").check();
    await expect(dialog).toHaveAttribute("data-mode", "sheet");
    const sheet = dialog.getByTestId("put-sheet");
    await expect(sheet.locator('tbody tr[data-testid^="put-sheet-row-"]')).toHaveCount(6, { timeout: 10_000 });
    // The project a picked document is in has its column from the start, ticked for that document, and cannot be hidden.
    const cell = (doc: string, project: string) => sheet.getByTestId(`put-sheet-cell-${doc}-${project}`);
    await expect(cell(d1, b)).toBeChecked();
    await expect(cell(d1, b)).toHaveAttribute("data-state", "member");
    await expect(dialog.getByTestId(`put-project-${b}`)).toBeDisabled();
    await expect(dialog.getByTestId("put-confirm")).toBeDisabled();
    await dialog.getByTestId(`put-project-${a}`).check();
    await dialog.getByTestId(`put-project-${c}`).check();
    await expect(sheet.locator("thead th")).toHaveCount(5); // 문서 · 종류 · three projects

    // Every document into AAA: the column header.
    await sheet.getByTestId(`put-sheet-col-${a}`).check();
    for (const d of ids) await expect(cell(d, a)).toHaveAttribute("data-state", "added");
    // The 설계서 documents into BBB as well: narrow, select what is shown, add.
    const bulkAdd = async (query: string, project: string) => {
      await sheet.getByTestId("put-sheet-search").fill(query);
      await expect(sheet.locator('tbody tr[data-testid^="put-sheet-row-"]')).toHaveCount(2);
      await sheet.getByTestId("put-sheet-select-all").click();
      await sheet.getByTestId("put-sheet-bulk-project").click();
      await page.getByTestId(`put-sheet-bulk-option-${project}`).click();
      await sheet.getByTestId("put-sheet-bulk-add").click();
      await sheet.getByTestId("put-sheet-select-none").click();
      await sheet.getByTestId("put-sheet-search").fill("");
    };
    await bulkAdd("설계서", b);
    await bulkAdd("시험결과", c);
    await expect(cell(d3, b)).toBeChecked();
    await expect(cell(d4, b)).toBeChecked();
    await expect(cell(d5, c)).toBeChecked();
    await expect(cell(d2, b)).not.toBeChecked();
    const summary = dialog.getByTestId("put-summary");
    await expect(summary).toHaveAttribute("data-add", "10");
    await expect(summary.getByTestId(`put-summary-add-${a}`)).toHaveText(tpl(ko.projects.put.summary_add, { name: "AAA", count: 6 }));
    await expect(summary.getByTestId(`put-summary-add-${b}`)).toHaveText(tpl(ko.projects.put.summary_add, { name: "BBB", count: 2 }));
    await expect(summary.getByTestId(`put-summary-add-${c}`)).toHaveText(tpl(ko.projects.put.summary_add, { name: "CCC", count: 2 }));

    // 요구사항-1 leaves BBB (its tick is taken off); 시험결과-2 goes in as 참고 자료.
    await cell(d1, b).uncheck();
    await expect(cell(d1, b)).toHaveAttribute("data-state", "removed");
    await expect(summary.getByTestId(`put-summary-remove-${b}`)).toHaveText(tpl(ko.projects.put.summary_remove, { name: "BBB", count: 1 }));
    await sheet.getByTestId(`put-sheet-kind-${d6}`).click();
    await expect(sheet.getByTestId(`put-sheet-kind-${d6}`)).toHaveAttribute("data-kind", "reference");
    // "지정 안 된 문서만" is empty now: every row has a project.
    await sheet.getByTestId("put-sheet-unassigned").check();
    await expect(sheet.locator('tbody tr[data-testid^="put-sheet-row-"]')).toHaveCount(0);
    await sheet.getByTestId("put-sheet-unassigned").uncheck();

    // A removal is confirmed on its own, with the pair spelled out; "돌아가기" keeps the sheet as it was.
    await dialog.getByTestId("put-confirm").click();
    const ask = dialog.getByTestId("put-remove-confirm");
    await expect(ask.locator("li")).toHaveCount(1);
    await expect(ask).toContainText("요구사항-1.pdf");
    await expect(ask).toContainText("BBB");
    await dialog.getByTestId("put-remove-back").click();
    await expect(cell(d1, b)).toHaveAttribute("data-state", "removed");
    await dialog.getByTestId("put-confirm").click();
    await dialog.getByTestId("put-remove-ok").click();
    await expect(dialog).toBeHidden();
    const piece = (name: string, parts: string[]) => tpl(ko.projects.put.done_piece, { name, parts: parts.join(", ") });
    await expect(toaster).toContainText(piece("AAA", [tpl(ko.projects.put.done_added, { count: 6 })]));
    await expect(toaster).toContainText(piece("BBB", [tpl(ko.projects.put.done_added, { count: 2 }), tpl(ko.projects.put.done_removed, { count: 1 })]));
    await expect(toaster).toContainText(piece("CCC", [tpl(ko.projects.put.done_added, { count: 2 })]));

    // The library badges show where each document is now.
    await page.getByTestId("nav-library").click();
    await expect(card(page, "요구사항-1.pdf")).toBeVisible();
    await expect.poll(() => badgeNames(page, d1), { timeout: 10_000 }).toEqual(["AAA"]);
    expect(await badgeNames(page, d2)).toEqual(["AAA"]);
    expect(await badgeNames(page, d3)).toEqual(["AAA", "BBB"]);
    expect(await badgeNames(page, d5)).toEqual(["AAA", "CCC"]);
    expect(await badgeNames(page, d6)).toEqual(["AAA", "CCC"]);
    await expect(page.getByTestId(`doc-project-${d6}`).first()).toHaveAttribute("data-reference", "true");
    await expect(page.getByTestId(`doc-project-${d5}`).first()).not.toHaveAttribute("data-reference", "true");
  });

  test("자동 분류 제안 fills the sheet: the first choice is ticked, the second one from the 2순위 추천 임계값 on", async ({ page }) => {
    test.setTimeout(180_000);
    await freshApp(page);
    await createWorkspace(page);
    const names = ["한빛-포털-구축-제안서.pdf", "한빛-포털-구축-새봄-연계.pdf", "새봄-erp-운영계획.pdf"];
    await importViaLibrary(page, names.map((n) => `D:\\work\\${n}`), { autoLevel: "none" });
    const [p1, p2, p3] = [await docId(page, names[0]!), await docId(page, names[1]!), await docId(page, names[2]!)];
    const hanbit = await newProject(page, "한빛 포털 구축", "한빛, 포털, 구축");
    const saebom = await newProject(page, "새봄 ERP", "새봄, erp");
    const dialog = page.getByTestId("put-dialog");
    const cell = (doc: string, project: string) => dialog.getByTestId(`put-sheet-cell-${doc}-${project}`);

    // The documents point to different projects, so the suggestion opens the sheet and ticks per document.
    await page.goto(`/#/projects?classify=${p1},${p2},${p3}`);
    await expect(dialog).toHaveAttribute("data-mode", "sheet", { timeout: 10_000 });
    await expect(dialog.getByTestId("put-fill-note")).toHaveText(tpl(ko.projects.put.suggest_filled, { count: 3 }), { timeout: 10_000 });
    await expect(cell(p1, hanbit)).toHaveAttribute("data-state", "suggested");
    await expect(cell(p2, hanbit)).toHaveAttribute("data-state", "suggested");
    await expect(cell(p3, saebom)).toHaveAttribute("data-state", "suggested");
    // The connection document names 새봄 once: a 50 % second choice stays a candidate at the default 60 %.
    await expect(cell(p2, saebom)).not.toBeChecked();
    const candidate = dialog.getByTestId(`put-sheet-candidate-${p2}-${saebom}`);
    await expect(candidate).toHaveText(tpl(ko.projects.put.sheet_candidate, { name: "새봄 ERP", percent: 50 }));
    await expect(dialog.getByTestId("put-summary")).toHaveAttribute("data-add", "3");
    // The candidate can be taken by hand: it becomes a tick of the user's own.
    await candidate.click();
    await expect(cell(p2, saebom)).toHaveAttribute("data-state", "added");
    await expect(dialog.getByTestId("put-summary")).toHaveAttribute("data-add", "4");
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();

    // With the threshold at 50 % the same suggestion ticks the second choice too.
    await openAssistantScreen(page);
    await page.getByTestId("assistant-input").fill("2순위 추천 임계값 50%로 바꿔줘");
    await page.getByTestId("assistant-send").click();
    await expect.poll(() => settingsValue(page, "processing.project_second_choice"), { timeout: 15_000 }).toBe(0.5);
    await page.goto(`/#/projects?classify=${p1},${p2},${p3}`);
    await expect(dialog.getByTestId("put-fill-note")).toHaveText(tpl(ko.projects.put.suggest_filled, { count: 4 }), { timeout: 10_000 });
    await expect(cell(p2, saebom)).toHaveAttribute("data-state", "suggested");
    await expect(dialog.getByTestId(`put-sheet-candidate-${p2}-${saebom}`)).toHaveCount(0);
    // Apply what the suggestion ticked.
    await dialog.getByTestId("put-confirm").click();
    await expect(dialog).toBeHidden();
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.projects.put.done_piece, { name: "새봄 ERP", parts: tpl(ko.projects.put.done_added, { count: 2 }) }));
  });

  test("the sheet stays inside the window at UI 배율 150 %: it scrolls in its own box, header and document column stay in view", async ({ page }) => {
    test.setTimeout(240_000);
    await freshApp(page);
    await createWorkspace(page);
    const names = Array.from({ length: 24 }, (_, i) => `2026년도-통합-유지보수-및-고도화-사업-산출물-${String(i + 1).padStart(2, "0")}.pdf`);
    await page.getByTestId("nav-library").click();
    await page.getByTestId("library-import").click();
    await page.getByTestId("doc-import-paths").fill(names.map((n) => `D:\\work\\${n}`).join("\n"));
    await page.getByTestId("doc-import-preview").click();
    await expect(page.getByTestId("doc-import-plan")).toBeVisible();
    await page.getByTestId("doc-import-auto-level").click();
    await page.getByRole("option", { name: ko.settings.processing.auto_levels.none }).click();
    await page.getByTestId("doc-import-run").click();
    await expect(page.getByTestId("library-stats")).toContainText(tpl(ko.library.total, { count: 24, pages: "" }).split("·")[0]!.trim(), { timeout: 60_000 });
    const projects: string[] = [];
    for (const name of ["한빛 포털 구축 2026년도 통합 유지보수 및 고도화 사업", "새봄 ERP 재구축", "미래 데이터 허브", "청운 모바일 앱", "누리 통합 관제", "가온 차세대 시스템"]) projects.push(await newProject(page, name));

    await openAssistantScreen(page);
    await page.getByTestId("assistant-input").fill("UI 배율 150으로 바꿔줘");
    await page.getByTestId("assistant-send").click();
    await expect.poll(() => settingsValue(page, "ui.scale_percent"), { timeout: 15_000 }).toBe(150);

    // Every document of the library, through the library's own selection.
    await page.getByTestId("nav-library").click();
    await page.getByLabel(ko.library.actions.select_all).check();
    await expect(page.getByTestId("library-selected")).toHaveText(tpl(ko.library.selected, { count: 24 }));
    await page.getByTestId("library-put-in-project").click();
    const dialog = page.getByTestId("put-dialog");
    await dialog.getByTestId("put-mode-sheet").check();
    await dialog.getByTestId("put-projects-select-all").click();
    const rows = dialog.locator('tbody tr[data-testid^="put-sheet-row-"]');
    await expect(rows).toHaveCount(24, { timeout: 15_000 });
    await expect(dialog.locator("thead th")).toHaveCount(8);

    const view = page.viewportSize()!;
    const box = (await dialog.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.y).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(view.width);
    expect(box.y + box.height).toBeLessThanOrEqual(view.height);
    // Nothing scrolls sideways but the sheet's own box.
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    const body = dialog.locator("[data-dialog-body]");
    expect(await body.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
    const scroll = dialog.getByTestId("put-sheet-scroll");
    const m = await scroll.evaluate((el) => ({ sw: el.scrollWidth, cw: el.clientWidth, sh: el.scrollHeight, ch: el.clientHeight }));
    expect(m.sh).toBeGreaterThan(m.ch); // 24 rows do not fit: the box scrolls
    // Scrolled to the far corner, the header row and the document column are still at the box's edges.
    await scroll.evaluate((el) => el.scrollTo(el.scrollWidth, el.scrollHeight));
    const edges = await scroll.evaluate((el) => {
      const outer = el.getBoundingClientRect();
      const head = el.querySelector("thead th:nth-child(3)")!.getBoundingClientRect();
      const corner = el.querySelector("thead th")!.getBoundingClientRect();
      const lastName = el.querySelector("tbody tr:last-child td")!.getBoundingClientRect();
      return { headTop: head.top - outer.top, cornerLeft: corner.left - outer.left, cornerTop: corner.top - outer.top, nameLeft: lastName.left - outer.left, nameRight: lastName.right, outerRight: outer.right };
    });
    expect(Math.abs(edges.headTop)).toBeLessThanOrEqual(2);
    expect(Math.abs(edges.cornerTop)).toBeLessThanOrEqual(2);
    expect(Math.abs(edges.cornerLeft)).toBeLessThanOrEqual(2);
    expect(Math.abs(edges.nameLeft)).toBeLessThanOrEqual(2);
    expect(edges.nameRight).toBeLessThanOrEqual(edges.outerRight);
    // The action row is in view and usable: one column for every row, then apply.
    await scroll.evaluate((el) => el.scrollTo(0, 0));
    await dialog.getByTestId(`put-sheet-col-${projects[0]}`).check();
    await expect(dialog.getByTestId("put-summary")).toHaveAttribute("data-add", "24");
    const confirm = dialog.getByTestId("put-confirm");
    const cb = (await confirm.boundingBox())!;
    expect(cb.y + cb.height).toBeLessThanOrEqual(view.height);
    expect(cb.x + cb.width).toBeLessThanOrEqual(view.width);
    await confirm.click();
    await expect(dialog).toBeHidden();
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.projects.put.done_added, { count: 24 }));
  });
});
