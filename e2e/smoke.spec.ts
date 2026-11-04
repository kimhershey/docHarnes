import { expect, test, type Page } from "@playwright/test";

// P0 acceptance (design §4.1): "empty screens work together with theme and
// language switching", plus settings persistence and the NL pre-router.

async function freshApp(page: Page) {
  await page.goto("/");
  await page.evaluate(() => localStorage.clear());
  await page.reload();
}

async function createWorkspace(page: Page, path = "C:\\Work\\Docs") {
  await expect(page.getByTestId("onboarding")).toBeVisible();
  await page.getByTestId("workspace-path").fill(path);
  await expect(page.getByTestId("workspace-inspection")).toBeVisible();
  await page.getByTestId("workspace-submit").click();
  await expect(page.getByTestId("hardware-panel")).toBeVisible();
  await expect(page.getByTestId("tier-badge")).toHaveText("8 GB");
  await page.getByTestId("onboarding-next").click();
  await page.getByTestId("onboarding-finish").click();
  await expect(page.getByTestId("app-shell")).toBeVisible();
}

test.describe("P0 shell", () => {
  test("onboarding → home, Korean by default", async ({ page }) => {
    await freshApp(page);
    await expect(page.locator("html")).toHaveAttribute("lang", "ko");
    await createWorkspace(page);
    await expect(page.getByTestId("page-home")).toBeVisible();
    await expect(page.getByTestId("nav-settings")).toHaveText("설정");
    await expect(page.getByTestId("workspace-switcher")).toContainText("Docs");
  });

  test("all four themes apply and persist across reload", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    for (const theme of ["dark", "light", "high-contrast", "system"]) {
      await page.getByTestId("theme-menu").click();
      await page.getByTestId(`theme-option-${theme}`).click();
      await expect(page.locator("html")).toHaveAttribute("data-theme-setting", theme);
      if (theme !== "system") await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
      await page.screenshot({ path: `test-results/theme-${theme}.png` });
    }
    await page.getByTestId("theme-menu").click();
    await page.getByTestId("theme-option-dark").click();
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    // Ctrl+Shift+T toggles light/dark
    await page.keyboard.press("Control+Shift+T");
    await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  });

  test("language switch re-renders navigation and persists", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("theme-menu").click();
    await page.getByTestId("language-option-en").click();
    await expect(page.getByTestId("nav-settings")).toHaveText("Settings");
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await page.reload();
    await expect(page.getByTestId("nav-settings")).toHaveText("Settings");
    await page.getByTestId("theme-menu").click();
    await page.getByTestId("language-option-ko").click();
    await expect(page.getByTestId("nav-settings")).toHaveText("설정");
  });

  test("every route renders and the command palette navigates", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    for (const screen of ["library", "search", "assistant", "review", "schemas", "knowledge", "organize", "jobs", "models", "settings", "generate", "projects"]) {
      await page.getByTestId(`nav-${screen}`).click();
      await expect(page.getByTestId(`page-${screen}`)).toBeVisible();
    }
    await page.keyboard.press("Control+K");
    await expect(page.getByTestId("command-palette")).toBeVisible();
    await page.getByTestId("palette-input").fill("/테마 다크");
    await expect(page.getByTestId("palette-run")).toContainText("다크");
    await page.getByTestId("palette-run").click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  });

  test("natural-language commands: settings ops run, work items go to the assistant", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("home-command").fill("설정 열어줘");
    await page.getByTestId("home-command-send").click();
    await expect(page.getByTestId("page-settings")).toBeVisible();
    await page.getByTestId("nav-home").click();
    await page.getByTestId("home-command").fill("지난달 영수증 합계");
    await page.getByTestId("home-command-send").click();
    await expect(page.getByTestId("page-assistant")).toBeVisible();
    // Work items are answered from the knowledge base (P3); the planner (P5) will route them as ops.
    await expect(page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]').first()).toHaveAttribute("data-answer-state", "no_evidence", { timeout: 15_000 });
    await page.getByTestId("assistant-input").fill("/언어 영어");
    await page.getByTestId("assistant-send").click();
    await expect(page.getByTestId("assistant-card-executed")).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
  });

  test("settings: workspace path mirrors the active workspace and retention saves", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page, "D:\\Scans\\2026");
    await page.getByTestId("nav-settings").click();
    await page.getByTestId("settings-tab-workspace").click();
    await expect(page.getByTestId("workspace-path-display")).toHaveText("D:\\Scans\\2026");
    await page.getByTestId("settings-tab-privacy").click();
    await page.getByTestId("retention-chats_days").fill("30");
    await page.getByTestId("retention-save").click();
    await expect(page.getByTestId("toaster")).toContainText("저장");
    await page.reload();
    await page.getByTestId("nav-settings").click();
    await page.getByTestId("settings-tab-privacy").click();
    await expect(page.getByTestId("retention-chats_days")).toHaveValue("30");
  });

  test("model manager lists the registry and switches tier manually", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-models").click();
    await expect(page.getByTestId("registry-table")).toContainText("Qwen3.5-4B");
    await page.getByTestId("tier-mode").click();
    await page.getByRole("option", { name: "수동 고정" }).click();
    await page.getByTestId("manual-tier").click();
    await page.getByRole("option", { name: "CPU 전용" }).click();
    await expect(page.getByTestId("tier-badge")).toHaveText("CPU 전용");
    await expect(page.getByTestId("model-badge")).toContainText("Qwen3.5-2B");
  });
});
