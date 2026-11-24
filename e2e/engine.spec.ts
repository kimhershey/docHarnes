import { expect, test } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { createWorkspace, freshApp, openModelsAdvanced, openSettingsTab } from "./helpers";

// P1 acceptance (design §4.1): engine lifecycle, model import / download /
// load and the job list, driven against the browser mock backend (the real
// llama-server path is covered by the Rust integration tests).
// The tests stay on the onboarding model step (`finish: false`) so step 3 can be inspected.

test.describe("P1 engine & models", () => {
  test("engine auto-starts, import registers a model, load shows it ready in the status bar", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page, undefined, { finish: false });
    await page.getByTestId("onboarding-finish").click();
    await expect(page.getByTestId("app-shell")).toBeVisible();

    // Engine auto-start → running, but no model → status bar "no model".
    await openModelsAdvanced(page);
    await expect(page.getByTestId("engine-state")).toHaveText("실행 중", { timeout: 10_000 });
    await expect(page.getByTestId("statusbar-engine")).toContainText("모델 없음");
    await expect(page.getByTestId("discovered-empty")).toBeVisible();
    await expect(page.getByTestId("models-import")).toHaveText("GGUF 가져오기");

    // Import: one weights file, the mmproj next to it is paired automatically.
    await page.getByTestId("models-import").click();
    await page.getByTestId("import-paths").fill("D:\\usb\\gemma-4-E4B-it-Q5_K_M.gguf");
    await page.getByTestId("import-validate").click();
    await expect(page.getByTestId("import-plan")).toContainText("mmproj-F16.gguf");
    await expect(page.getByTestId("import-plan")).toContainText("자동으로 짝지었습니다");
    await page.getByTestId("import-run").click();
    await expect(page.getByTestId("discovered-gemma-4-e4b-it-q5_k_m")).toBeVisible({ timeout: 10_000 });
    await expect(page.getByTestId("discovered-gemma-4-e4b-it-q5_k_m")).toContainText("준비됨");
    await expect(page.getByTestId("install-gemma4-e4b-q5km")).toHaveText("설치됨");

    // Load → instance badge + status bar ready + VRAM figure.
    await page.getByTestId("load-gemma4-e4b-q5km").click();
    await expect(page.getByTestId("instance-gemma4-e4b-q5km")).toHaveText("로드됨", { timeout: 15_000 });
    await expect(page.getByTestId("statusbar-engine")).toContainText("준비");
    await expect(page.getByTestId("statusbar-model")).toContainText("gemma4-e4b-q5km");
    await expect(page.getByTestId("engine-loaded")).toContainText("gemma4-e4b-q5km");

    // Chat test dialog returns a response.
    await page.getByTestId("engine-test").click();
    await page.getByTestId("chat-test-run").click();
    await expect(page.getByTestId("chat-test-result")).toContainText("DocuLens", { timeout: 10_000 });
    await page.keyboard.press("Escape");

    // Jobs screen lists the finished load.
    await page.getByTestId("nav-jobs").click();
    await page.getByTestId("jobs-tab-finished").click();
    await expect(page.locator('[data-job-kind="model_load"][data-job-subject="gemma4-e4b-q5km"]').first()).toContainText("완료");
    await expect(page.locator('[data-job-kind="model_import"][data-job-subject="gemma-4-e4b-it-q5_k_m"]').first()).toContainText("완료");
    await expect(page.locator('[data-job-kind="engine_start"]').first()).toContainText("완료");
  });

  test("download job installs a registry model and engine stop/start buttons work", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page, undefined, { finish: false });
    await page.getByTestId("onboarding-finish").click();
    await openModelsAdvanced(page);
    await expect(page.getByTestId("engine-state")).toHaveText("실행 중", { timeout: 10_000 });

    await page.getByTestId("download-gemma4-e4b-q4km").click();
    await expect(page.getByTestId("topbar-jobs")).toHaveText(/작업 1건/);
    await expect(page.getByTestId("install-gemma4-e4b-q4km")).toHaveText("설치됨", { timeout: 15_000 });
    await expect(page.getByTestId("discovered-gemma4-e4b-q4km")).toBeVisible();

    await page.getByTestId("engine-stop").click();
    await expect(page.getByTestId("engine-state")).toHaveText("중지됨", { timeout: 10_000 });
    await expect(page.getByTestId("statusbar-engine")).toContainText("엔진 중지");
    await page.getByTestId("engine-start").click();
    await expect(page.getByTestId("engine-state")).toHaveText("실행 중", { timeout: 10_000 });

    // Deleting the folder removes the discovered row and the install badge.
    page.once("dialog", (d) => void d.accept());
    await page.getByTestId("delete-gemma4-e4b-q4km").click();
    await expect(page.getByTestId("discovered-gemma4-e4b-q4km")).toHaveCount(0);
    await expect(page.getByTestId("install-gemma4-e4b-q4km")).toHaveText("미설치");
  });

  test("onboarding step 3 offers download and import, and English labels exist", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page, undefined, { finish: false });
    await expect(page.getByText("다운로드 시작")).toBeVisible();
    await expect(page.getByText("GGUF 가져오기…")).toBeVisible();
    await page.getByText("GGUF 가져오기…").click();
    await expect(page.getByTestId("import-paths")).toBeVisible();
    await page.keyboard.press("Escape");
    await page.getByTestId("onboarding-finish").click();
    await page.getByTestId("nav-settings").click();
    await page.getByTestId("settings-tab-general").click();
    await page.getByTestId("language-select").click();
    await page.getByRole("option", { name: "English" }).click();
    await openModelsAdvanced(page);
    await expect(page.getByTestId("engine-card")).toContainText("Engine (llama-server)");
    await expect(page.getByTestId("statusbar-engine")).toContainText("AI engine:");
  });

  // 1.0.6 (user decision 2026-11-24): the limits are off the evidence settings — 근거 토큰 예산 goes up to the largest
  // context a model takes and the model is loaded with a context that holds it; the chunk size stays, with its reason.
  test("the evidence budget goes above 24,000 and the model is loaded with a context that holds it", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page, undefined, { finish: false });
    await page.getByTestId("onboarding-finish").click();
    await expect(page.getByTestId("app-shell")).toBeVisible();

    await openSettingsTab(page, "reference");
    const budget = page.getByTestId("kb-context_tokens");
    await expect(budget).toHaveValue("6000");
    await expect(budget).toHaveAttribute("max", "262144");
    await budget.fill("60000");
    await budget.blur();
    await expect(budget).toHaveValue("60000");
    const passages = page.getByTestId("kb-top_k");
    await expect(passages).toHaveAttribute("max", "200");
    await passages.fill("150");
    await passages.blur();
    await expect(passages).toHaveValue("150");
    // The chunk size keeps its range: a chunk has to fit the embedding model's context — the hint says so.
    const chunk = page.getByTestId("kb-chunk_target_tokens");
    await chunk.fill("5000");
    await chunk.blur();
    await expect(chunk).toHaveValue("2000");
    const knowledge = page.getByTestId("settings-knowledge");
    await expect(knowledge).toContainText(ko.settings.knowledge.chunk_target_hint);
    await expect(knowledge).toContainText("이 값을 낮추세요");
    await expect(page.getByTestId("kb-context-loaded")).toHaveCount(0);

    // The preset of a chat model: 60,000 of evidence + a quarter for a summary's reply + the scaffold, in 1,024s.
    await openModelsAdvanced(page);
    await expect(page.getByTestId("engine-state")).toHaveText("실행 중", { timeout: 10_000 });
    await page.getByTestId("models-import").click();
    await page.getByTestId("import-paths").fill("D:\\usb\\gemma-4-E4B-it-Q5_K_M.gguf");
    await page.getByTestId("import-validate").click();
    await page.getByTestId("import-run").click();
    await expect(page.getByTestId("discovered-gemma-4-e4b-it-q5_k_m")).toBeVisible({ timeout: 10_000 });
    await page.getByTestId("engine-card").getByRole("button", { name: ko.models.engine_card.presets }).click();
    await expect(page.getByRole("dialog")).toContainText("ctx-size = 75776");
    await page.keyboard.press("Escape");
    await page.getByTestId("load-gemma4-e4b-q5km").click();
    await expect(page.getByTestId("instance-gemma4-e4b-q5km")).toHaveText("로드됨", { timeout: 15_000 });

    // … and the settings screen shows the context the loaded model runs with.
    await openSettingsTab(page, "reference");
    await expect(page.getByTestId("kb-context-loaded")).toContainText("75,776");
    await expect(page.getByTestId("kb-context-loaded")).toContainText("gemma4-e4b-q5km");
  });
});
