import { expect, test, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { createWorkspace, docId, freshApp, importViaLibrary, openAssistantScreen, openSettingsTab } from "./helpers";

// 0.8.0 assistant harness (docs/plan/v0.8.0.md) against the browser mock. The sentences of the
// 2026-11-11 report are the first tests: a settings sentence changes the setting it names, a greeting
// gets a reply instead of an empty plan card, "프로젝트 별 요약" makes one summary per project. Then the
// contract of a model plan: what it leaves out or adds on its own waits for approval with a note.
// The fast path, the routing call, the grammar order and the guard are covered by the Rust tests.

const assistantCard = (page: Page, kind: string) => page.locator(`[data-testid="assistant-card-${kind}"]`);
const answerCards = (page: Page) => page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]');

async function send(page: Page, text: string) {
  await page.getByTestId("assistant-input").fill(text);
  await page.getByTestId("assistant-send").click();
}

async function newProject(page: Page, name: string): Promise<void> {
  await page.getByTestId("nav-projects").click();
  await page.getByTestId("projects-new").click();
  await page.getByTestId("project-name").fill(name);
  await page.getByTestId("project-submit").click();
  await expect(page.getByTestId("project-title")).toHaveText(name);
}

async function addToOpenProject(page: Page, id: string): Promise<void> {
  await page.getByTestId("project-add-documents").click();
  await page.getByTestId(`project-add-doc-${id}`).check();
  await page.getByTestId("project-add-done").click();
  await expect(page.getByTestId(`project-doc-${id}`)).toBeVisible();
}

const settingsValue = (page: Page, path: string) =>
  page.evaluate((p) => {
    const raw = window.localStorage.getItem("doculens.mock.settings");
    let v: unknown = raw ? JSON.parse(raw) : null;
    for (const part of p.split(".")) v = v && typeof v === "object" ? (v as Record<string, unknown>)[part] : undefined;
    return v;
  }, path);

test.describe("0.8.0 assistant harness", () => {
  test("the reported settings sentences change the setting they name, show before → after and can be undone", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);

    await send(page, "UI 배율 120%");
    const first = assistantCard(page, "executed").last();
    await expect(first).toBeVisible({ timeout: 15_000 });
    // Read without the model: the card names the path it took and the change it made.
    await expect(first).toContainText(ko.planner.source.fast);
    await expect(first).toContainText("100");
    await expect(first).toContainText("120");
    await expect(page.locator('[data-testid="assistant-card-plan"]')).toHaveCount(0);
    expect(await settingsValue(page, "ui.scale_percent")).toBe(120);

    await send(page, "배율을 130 으로 변경해줘");
    await expect(assistantCard(page, "executed")).toHaveCount(2, { timeout: 15_000 });
    expect(await settingsValue(page, "ui.scale_percent")).toBe(130);
    // Neither sentence touched the theme or the model profile (what 0.7.0 did with them).
    await expect(page.locator("html")).not.toHaveAttribute("data-theme", "dark");

    // The render DPI is 200 already: the card says so instead of claiming a change, and offers no undo.
    await send(page, "DPI 를 200 으로 변경해줘");
    await expect(assistantCard(page, "executed")).toHaveCount(3, { timeout: 15_000 });
    const dpi = assistantCard(page, "executed").last();
    await expect(dpi).toContainText("200");
    await expect(dpi.getByTestId("executed-undo")).toHaveCount(0);
    expect(await settingsValue(page, "processing.render_dpi")).toBe(200);
    expect(await settingsValue(page, "ui.scale_percent")).toBe(130);

    // 되돌리기 on the second card puts the value it replaced back.
    const second = assistantCard(page, "executed").nth(1);
    await second.getByTestId("executed-undo").click();
    await expect(second.getByTestId("executed-undone")).toBeVisible();
    expect(await settingsValue(page, "ui.scale_percent")).toBe(120);

    await openSettingsTab(page, "general");
    await expect(page.getByTestId("scale-value")).toContainText("120");
  });

  test("a greeting gets a reply at once: no plan card, no answer card, no model", async ({ page }) => {
    // The planner model is off: a greeting must not need it.
    await freshApp(page, { planner_model: "off" });
    await createWorkspace(page);
    await openAssistantScreen(page);

    await send(page, "안녕하세요.");
    const reply = assistantCard(page, "reply").last();
    await expect(reply).toBeVisible({ timeout: 15_000 });
    await expect(reply).toContainText("안녕하세요!");
    await expect(reply).toContainText(ko.planner.source.fast);
    await expect(page.locator('[data-testid="assistant-card-plan"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="assistant-card-clarify"]')).toHaveCount(0);
    await expect(answerCards(page)).toHaveCount(0);

    await send(page, "고마워!");
    await expect(assistantCard(page, "reply")).toHaveCount(2);
    await expect(assistantCard(page, "reply").last()).toContainText("도움이 되어 다행입니다");
  });

  test("small talk and a question that is not about the documents are answered, not planned", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx"]);
    await openAssistantScreen(page);

    await send(page, "넌 누구야?");
    const chat = answerCards(page).first();
    await expect(chat).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(chat).toContainText("DocuLens 어시스턴트");
    await expect(chat.getByTestId("answer-no-evidence")).toHaveCount(0);

    // "문서와 무관한 지식 질문에 답하게 할지 → 응답하도록 하자": answered even though no document covers it.
    await send(page, "대한민국의 수도는 어디야?");
    const general = answerCards(page).nth(1);
    await expect(general).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(general.getByTestId("answer-general-badge")).toBeVisible();
    await expect(page.locator('[data-testid="assistant-card-plan"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="assistant-card-clarify"]')).toHaveCount(0);
  });

  test("'프로젝트 별 내용을 요약해줘' makes one summary per project, with the documents outside every project last", async ({ page }) => {
    test.setTimeout(90_000);
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx", "D:\\scan\\개발계약서-2p.pdf", "D:\\규정\\정보보안지침.pdf"]);
    const estimate = await docId(page, "견적서.hwpx");
    const contract = await docId(page, "개발계약서-2p.pdf");
    await newProject(page, "한빛 포털 구축");
    await addToOpenProject(page, estimate);
    await newProject(page, "새봄 ERP 고도화");
    await addToOpenProject(page, contract);

    await openAssistantScreen(page);
    await send(page, "프로젝트 별 내용을 요약해줘");
    const result = assistantCard(page, "result").last();
    await expect(result).toHaveAttribute("data-result-status", "done", { timeout: 30_000 });
    const groups = result.getByTestId("result-summary-groups");
    await expect(groups).toHaveAttribute("data-group-by", "project");
    const items = groups.getByTestId("result-summary-group");
    await expect(items).toHaveCount(3);
    await expect(items.filter({ hasText: "한빛 포털 구축" })).toHaveCount(1);
    await expect(items.filter({ hasText: "새봄 ERP 고도화" })).toHaveCount(1);
    // The guideline belongs to no project: its group comes last and says so.
    await expect(items.nth(2)).toContainText("프로젝트 없음");
  });

  test("a change the sentence does not name waits for approval with the reason on the card", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);

    // No setting name, no value: the (simulated) model picks a scale on its own.
    await send(page, "글자를 더 크게 해줘");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await expect(plan.locator('[data-warning-code="ungrounded_change"]')).toBeVisible();
    // Nothing changed yet.
    expect(await settingsValue(page, "ui.scale_percent")).toBe(100);
    // A settings plan reads no documents: the card shows no document count.
    await expect(plan).not.toContainText("0건");

    await plan.getByTestId("plan-execute").click();
    await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    expect(await settingsValue(page, "ui.scale_percent")).toBe(125);
  });

  test("a part the plan leaves out and a split it cannot make are said on the card before anything runs", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx"]);
    await page.getByTestId(`doc-select-${await docId(page, "견적서.hwpx")}`).check();
    await openAssistantScreen(page);
    await page.getByTestId("assistant-scope").click();
    await page.getByTestId("assistant-scope-selection").click();

    await send(page, "선택한 문서를 요약해서 메일로 보내줘");
    const unmet = assistantCard(page, "plan").last();
    await expect(unmet).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await expect(unmet.locator('[data-warning-code="unmet_request"]')).toContainText("메일");
    await unmet.getByTestId("plan-cancel").click();

    await send(page, "선택한 문서를 담당자별로 요약해줘");
    const split = assistantCard(page, "plan").last();
    await expect(split).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await expect(split.locator('[data-warning-code="grouping_dropped"]')).toBeVisible();
  });
});
