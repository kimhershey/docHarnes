import { expect, test, type Page } from "@playwright/test";

// P1 acceptance (design §4.1): engine lifecycle, model import / download /
// load and the job list, driven against the browser mock backend (the real
// llama-server path is covered by the Rust integration tests).

async function freshApp(page: Page) {
  await page.goto("/");
  await page.evaluate(() => {
    localStorage.clear();
    localStorage.setItem("doculens.mock.fast", "1");
  });
  await page.reload();
}

async function createWorkspace(page: Page, path = "C:\\Work\\Docs") {
  await expect(page.getByTestId("onboarding")).toBeVisible();
  await page.getByTestId("workspace-path").fill(path);
  await expect(page.getByTestId("workspace-inspection")).toBeVisible();
  await page.getByTestId("workspace-submit").click();
  await expect(page.getByTestId("hardware-panel")).toBeVisible();
  await page.getByTestId("onboarding-next").click();
}

test.describe("P1 engine & models", () => {
  test("engine auto-starts, import registers a model, load shows it ready in the status bar", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("onboarding-finish").click();
    await expect(page.getByTestId("app-shell")).toBeVisible();

    // Engine auto-start → running, but no model → status bar "no model".
    await page.getByTestId("nav-models").click();
    await expect(page.getByTestId("engine-state")).toHaveText("실행 중", { timeout: 10_000 });
    await expect(page.getByTestId("statusbar-engine")).toContainText("모델 없음");
    await expect(page.getByTestId("discovered-empty")).toBeVisible();
    await expect(page.getByTestId("models-import")).toHaveText("GGUF 가져오기");

    // Import: one weights file, the mmproj next to it is paired automatically.
    await page.getByTestId("models-import").click();
    await page.getByTestId("import-paths").fill("D:\\usb\\Qwen3.5-4B-Q6_K.gguf");
    await page.getByTestId("import-validate").click();
    await expect(page.getByTestId("import-plan")).toContainText("mmproj-F16.gguf");
    await expect(page.getByTestId("import-plan")).toContainText("자동으로 짝지었습니다");
    await page.getByTestId("import-run").click();
    await expect(page.getByTestId("discovered-qwen3.5-4b-q6_k")).toBeVisible({ timeout: 10_000 });
    await expect(page.getByTestId("discovered-qwen3.5-4b-q6_k")).toContainText("준비됨");
    await expect(page.getByTestId("install-qwen3.5-4b-q6k")).toHaveText("설치됨");

    // Load → instance badge + status bar ready + VRAM figure.
    await page.getByTestId("load-qwen3.5-4b-q6k").click();
    await expect(page.getByTestId("instance-qwen3.5-4b-q6k")).toHaveText("로드됨", { timeout: 15_000 });
    await expect(page.getByTestId("statusbar-engine")).toContainText("준비");
    await expect(page.getByTestId("statusbar-vram")).toContainText("VRAM 사용");
    await expect(page.getByTestId("engine-loaded")).toContainText("qwen3.5-4b-q6k");

    // Chat test dialog returns a response.
    await page.getByTestId("engine-test").click();
    await page.getByTestId("chat-test-run").click();
    await expect(page.getByTestId("chat-test-result")).toContainText("DocuLens", { timeout: 10_000 });
    await page.keyboard.press("Escape");

    // Jobs screen lists the finished load.
    await page.getByTestId("nav-jobs").click();
    await page.getByTestId("jobs-tab-finished").click();
    await expect(page.locator('[data-job-kind="model_load"][data-job-subject="qwen3.5-4b-q6k"]').first()).toContainText("완료");
    await expect(page.locator('[data-job-kind="model_import"][data-job-subject="qwen3.5-4b-q6_k"]').first()).toContainText("완료");
    await expect(page.locator('[data-job-kind="engine_start"]').first()).toContainText("완료");
  });

  test("download job installs a registry model and engine stop/start buttons work", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("onboarding-finish").click();
    await page.getByTestId("nav-models").click();
    await expect(page.getByTestId("engine-state")).toHaveText("실행 중", { timeout: 10_000 });

    await page.getByTestId("download-qwen3.5-4b-q4km").click();
    await expect(page.getByTestId("topbar-jobs")).toHaveText(/작업 1건/);
    await expect(page.getByTestId("install-qwen3.5-4b-q4km")).toHaveText("설치됨", { timeout: 15_000 });
    await expect(page.getByTestId("discovered-qwen3.5-4b-q4km")).toBeVisible();

    await page.getByTestId("engine-stop").click();
    await expect(page.getByTestId("engine-state")).toHaveText("중지됨", { timeout: 10_000 });
    await expect(page.getByTestId("statusbar-engine")).toContainText("엔진 중지");
    await page.getByTestId("engine-start").click();
    await expect(page.getByTestId("engine-state")).toHaveText("실행 중", { timeout: 10_000 });

    // Deleting the folder removes the discovered row and the install badge.
    page.once("dialog", (d) => void d.accept());
    await page.getByTestId("delete-qwen3.5-4b-q4km").click();
    await expect(page.getByTestId("discovered-qwen3.5-4b-q4km")).toHaveCount(0);
    await expect(page.getByTestId("install-qwen3.5-4b-q4km")).toHaveText("미설치");
  });

  test("onboarding step 3 offers download and import, and English labels exist", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
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
    await page.getByTestId("nav-models").click();
    await expect(page.getByTestId("engine-card")).toContainText("Engine (llama-server)");
    await expect(page.getByTestId("statusbar-engine")).toContainText("Engine:");
  });
});
