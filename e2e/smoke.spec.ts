import { expect, test, type Page } from "@playwright/test";

import { createWorkspace, freshApp } from "./helpers";

// P0 acceptance (design §4.1): "empty screens work together with theme and
// language switching", plus settings persistence and the NL pre-router.
// These run the mock at its normal speed and check the detected tier on the way in.

const freshShell = (page: Page) => freshApp(page, {}, { fast: false });
const onboard = (page: Page, path?: string) => createWorkspace(page, path, { expectTier: "8 GB" });

test.describe("P0 shell", () => {
  test("onboarding → home, Korean by default", async ({ page }) => {
    await freshShell(page);
    await expect(page.locator("html")).toHaveAttribute("lang", "ko");
    await onboard(page);
    await expect(page.getByTestId("page-home")).toBeVisible();
    await expect(page.getByTestId("nav-settings")).toHaveText("설정");
    await expect(page.getByTestId("workspace-switcher")).toContainText("Docs");
  });

  test("all four themes apply and persist across reload", async ({ page }) => {
    await freshShell(page);
    await onboard(page);
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
    await freshShell(page);
    await onboard(page);
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
    await freshShell(page);
    await onboard(page);
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
    await freshShell(page);
    await onboard(page);
    await page.getByTestId("home-command").fill("설정 열어줘");
    await page.getByTestId("home-command-send").click();
    await expect(page.getByTestId("page-settings")).toBeVisible();
    await page.getByTestId("nav-home").click();
    await page.getByTestId("home-command").fill("지난달 견적서 합계");
    await page.getByTestId("home-command-send").click();
    await expect(page.getByTestId("page-assistant")).toBeVisible();
    // Work items go to the planner (P5): the assistant answers with a plan / result / aggregate card.
    await expect(page.locator('[data-testid^="assistant-card-"]').first()).toBeVisible({ timeout: 15_000 });
    await page.getByTestId("assistant-input").fill("/언어 영어");
    await page.getByTestId("assistant-send").click();
    await expect(page.getByTestId("assistant-card-executed")).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
  });

  test("settings: workspace path mirrors the active workspace and retention saves", async ({ page }) => {
    await freshShell(page);
    await onboard(page, "D:\\Scans\\2026");
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
    await freshShell(page);
    await onboard(page);
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
