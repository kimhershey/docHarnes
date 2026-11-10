import { defineConfig, devices } from "@playwright/test";
import process from "node:process";

// Browser-mode UI tests against `vite preview` with the mock backend.
// In this cloud environment the pre-installed Chromium is used via
// PW_CHROMIUM_PATH; in CI Playwright installs its own browser.
const executablePath = process.env.PW_CHROMIUM_PATH || undefined;
// Several agents / terminals can run suites side by side: PW_PORT picks the preview port and PW_OUT_DIR
// the build folder (defaults match `npm run preview`), so one run never serves another run's build.
const port = Number(process.env.PW_PORT || 4173);
const outDir = process.env.PW_OUT_DIR || "dist";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  fullyParallel: false,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    ...devices["Desktop Chrome"],
    viewport: { width: 1280, height: 800 },
    launchOptions: executablePath ? { executablePath } : {},
  },
  webServer: {
    // Build first so the preview never serves a stale dist/ (CI builds separately; the rebuild is cheap).
    command: `npx tsc -p tsconfig.json --noEmit && npx vite build --outDir ${outDir} && npx vite preview --host 127.0.0.1 --port ${port} --strictPort --outDir ${outDir}`,
    url: `http://127.0.0.1:${port}/`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    stdout: "pipe",
    stderr: "pipe",
  },
});
