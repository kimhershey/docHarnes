import { expect, test, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { card, createWorkspace, docId, freshApp, importViaLibrary, openAssistantScreen, openModelsAdvanced, openSettingsTab } from "./helpers";

// 0.6.0 (plan docs/plan/v0.6.0.md): every screen action as an assistant command, against the browser
// mock — a document op (rotate), a confirmed file op (merge), a catalog setting (UI scale), a reference
// group, a project deletion and an engine stop. The Rust tests cover the rules / validator / executor.

const assistantCard = (page: Page, kind: string) => page.locator(`[data-testid="assistant-card-${kind}"]`);

async function send(page: Page, text: string) {
  await page.getByTestId("assistant-input").fill(text);
  await page.getByTestId("assistant-send").click();
}

/** Plans without side effects run at once; otherwise approve the plan card. Resolves once the card reports done. */
async function approveIfPending(page: Page, plan: ReturnType<typeof assistantCard>) {
  const execute = plan.getByTestId("plan-execute");
  if ((await plan.getAttribute("data-plan-status")) === "pending" && (await execute.isVisible())) await execute.click();
  await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
}

test.describe("0.6.0 agent ops", () => {
  test("rotate: '이 문서 90도 돌려줘' from the viewer → plan card → result with the rotation detail", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서-1p.jpg"], { autoLevel: "none" });
    await card(page, "견적서-1p.jpg").locator('[data-testid^="doc-open-"]').click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();

    await openAssistantScreen(page);
    await send(page, "이 문서 90도 돌려줘");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toBeVisible();
    await approveIfPending(page, plan);
    const result = assistantCard(page, "result").last();
    await expect(result).toHaveAttribute("data-result-status", "done", { timeout: 15_000 });
    await expect(result.getByTestId("result-agent-detail")).toContainText("90°");
  });

  test("merge: two selected documents → confirmation → one merged PDF in the library", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서-1p.jpg", "D:\\scan\\개발계약서-2p.pdf"], { autoLevel: "none" });
    await page.getByTestId(`doc-select-${await docId(page, "견적서-1p.jpg")}`).check();
    await page.getByTestId(`doc-select-${await docId(page, "개발계약서-2p.pdf")}`).check();

    await openAssistantScreen(page);
    await page.getByTestId("assistant-scope").click();
    await page.getByTestId("assistant-scope-selection").click();
    await send(page, "선택한 문서 하나로 합쳐줘 '계약 묶음'");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toHaveAttribute("data-plan-status", "pending");
    await plan.getByTestId("plan-execute").click();
    await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    const result = assistantCard(page, "result").last();
    await expect(result).toHaveAttribute("data-result-status", "done", { timeout: 15_000 });
    await expect(result.getByTestId("result-agent-detail")).toContainText("2");

    await page.getByTestId("nav-library").click();
    await expect(card(page, "계약 묶음.pdf")).toBeVisible({ timeout: 15_000 });
  });

  test("setting: 'UI 배율 125%로' changes the catalog setting at once", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);
    await send(page, "UI 배율 125%로");
    // A settings op without a confirmation runs at once and leaves an "executed" card, not a plan card.
    await expect(assistantCard(page, "executed").last()).toBeVisible({ timeout: 15_000 });
    await openSettingsTab(page, "general");
    await expect(page.getByTestId("scale-value")).toContainText("125");
  });

  test("reference group: \"'사규' 참고 자료 묶음 만들어\" appears under 공통 참고 자료", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);
    await send(page, "'사규' 참고 자료 묶음 만들어");
    const plan = assistantCard(page, "plan").last();
    await approveIfPending(page, plan);
    await expect(assistantCard(page, "result").last()).toHaveAttribute("data-result-status", "done", { timeout: 15_000 });

    await page.getByTestId("nav-projects").click();
    await page.getByTestId("projects-common-reference").click();
    await expect(page.getByTestId("page-common-reference")).toBeVisible();
    await expect(page.locator('[data-testid="reference-group-item"][data-group-name="사규"]')).toBeVisible();
  });

  test("project: '한빛 심사 프로젝트 삭제' asks first, then the project is gone", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-projects").click();
    await page.getByTestId("projects-new").click();
    await page.getByTestId("project-name").fill("한빛 심사");
    await page.getByTestId("project-submit").click();
    await expect(page.getByTestId("page-project")).toBeVisible();

    await openAssistantScreen(page);
    await send(page, "한빛 심사 프로젝트 삭제");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toHaveAttribute("data-plan-status", "pending");
    await plan.getByTestId("plan-execute").click();
    await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    await expect(assistantCard(page, "result").last()).toHaveAttribute("data-result-status", "done", { timeout: 15_000 });

    await page.getByTestId("nav-projects").click();
    await expect(page.getByTestId("page-projects")).toBeVisible();
    await expect(page.locator('[data-testid^="project-card-"][data-project-name="한빛 심사"]')).toHaveCount(0);
  });

  // PLN-17: an op the UI runs itself can fail. Only a toast said so; the card kept its "실행됨" / ✓.
  test("a UI op that fails is shown as failed: the executed card and a result card's step", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);

    // Runs at once (no confirmation): the folder is not a workspace, so nothing was opened.
    await send(page, "워크스페이스 D:\\NoSuch 열어");
    const executed = assistantCard(page, "executed").last();
    await expect(executed).toBeVisible({ timeout: 15_000 });
    await expect(executed).toHaveAttribute("data-executed-status", "failed");
    await expect(executed).toContainText(ko.assistant.executed_failed);
    await expect(executed.getByTestId("executed-failure")).toHaveCount(1);
    await expect(executed.getByTestId("executed-failure")).toContainText("D:\\NoSuch");
    // Still in the first workspace.
    await expect(page.getByTestId("workspace-switcher")).toHaveAttribute("title", "C:\\Work\\Docs");

    // (Enter instead of the send button from here on: the failure toast lies over the button for a while.)
    const sendWithEnter = async (text: string) => {
      await page.getByTestId("assistant-input").fill(text);
      await page.getByTestId("assistant-input").press("Enter");
    };
    // Confirmed plan → result card: the model is not installed, so the UI's delete fails on its step.
    await sendWithEnter("12B 모델 삭제해줘");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await plan.getByTestId("plan-execute").click();
    const result = assistantCard(page, "result").last();
    await expect(result).toBeVisible({ timeout: 15_000 });
    await expect(result.getByTestId("result-step-0")).toHaveAttribute("data-ui-status", "failed");
    await expect(result.getByTestId("result-ui-error")).toBeVisible();
    await expect(result).toHaveAttribute("data-result-status", "failed");

    // An op that works keeps the plain "실행했습니다" card.
    await sendWithEnter("UI 배율 125%로");
    const ok = assistantCard(page, "executed").last();
    await expect(ok).toHaveAttribute("data-executed-status", "done", { timeout: 15_000 });
    await expect(ok.getByTestId("executed-failure")).toHaveCount(0);
  });

  test("engine: '엔진 꺼' is confirmed and the engine card shows it stopped", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    // Auto-start after onboarding: running, no model installed yet (the status bar says "모델 없음" either way).
    await openModelsAdvanced(page);
    await expect(page.getByTestId("engine-state")).toHaveText("실행 중", { timeout: 10_000 });
    await openAssistantScreen(page);
    await send(page, "엔진 꺼");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toHaveAttribute("data-plan-status", "pending");
    await plan.getByTestId("plan-execute").click();
    await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    await openModelsAdvanced(page);
    await expect(page.getByTestId("engine-state")).toHaveText("중지됨", { timeout: 10_000 });
  });
});
