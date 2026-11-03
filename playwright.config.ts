import { defineConfig, devices } from "@playwright/test";
import process from "node:process";

// Browser-mode UI tests against `vite preview` with the mock backend.
// In this cloud environment the pre-installed Chromium is used via
// PW_CHROMIUM_PATH; in CI Playwright installs its own browser.
const executablePath = process.env.PW_CHROMIUM_PATH || undefined;

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  fullyParallel: false,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: "http://127.0.0.1:4173",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    ...devices["Desktop Chrome"],
    viewport: { width: 1280, height: 800 },
    launchOptions: executablePath ? { executablePath } : {},
  },
  webServer: {
    command: "npm run preview",
    url: "http://127.0.0.1:4173",
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
});
