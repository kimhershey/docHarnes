import { expect, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };

// Shared flows for the mock-backend specs (one copy instead of five, review L12): app reset,
// onboarding, library import, card lookups and ko.json template interpolation.

export type MockFlags = Record<string, string>;

/** Loads the app with cleared mock state. `fast` turns the mock's sped-up timers on (`doculens.mock.fast`). */
export async function freshApp(page: Page, flags: MockFlags = {}, { fast = true }: { fast?: boolean } = {}): Promise<void> {
  await page.goto("/");
  await page.evaluate(
    ({ flags, fast }) => {
      localStorage.clear();
      if (fast) localStorage.setItem("doculens.mock.fast", "1");
      for (const [k, v] of Object.entries(flags)) localStorage.setItem(k, v);
    },
    { flags, fast },
  );
  await page.reload();
}

/**
 * Onboarding: workspace path → hardware step → (unless `finish` is false) the model step is finished
 * and the app shell is up. `expectTier` asserts the detected tier badge on the hardware step.
 */
export async function createWorkspace(page: Page, path = "C:\\Work\\Docs", { finish = true, expectTier }: { finish?: boolean; expectTier?: string } = {}): Promise<void> {
  await expect(page.getByTestId("onboarding")).toBeVisible();
  await page.getByTestId("workspace-path").fill(path);
  await expect(page.getByTestId("workspace-inspection")).toBeVisible();
  await page.getByTestId("workspace-submit").click();
  await expect(page.getByTestId("hardware-panel")).toBeVisible();
  if (expectTier) await expect(page.getByTestId("tier-badge")).toHaveText(expectTier);
  await page.getByTestId("onboarding-next").click();
  if (!finish) return;
  await page.getByTestId("onboarding-finish").click();
  await expect(page.getByTestId("app-shell")).toBeVisible();
}

/** Opens the library import dialog with `paths` and waits for the preview; the caller runs or inspects it. */
export async function openImportPreview(page: Page, paths: string[]): Promise<void> {
  await page.getByTestId("library-import").click();
  await page.getByTestId("doc-import-paths").fill(paths.join("\n"));
  await page.getByTestId("doc-import-preview").click();
  await expect(page.getByTestId("doc-import-plan")).toBeVisible();
}

export type AutoLevel = keyof typeof ko.settings.processing.auto_levels;

/**
 * Imports `paths` through the library dialog and waits until every card is rendered. `autoLevel`
 * overrides the post-import processing for the batch (e.g. "none" when a test starts processing itself).
 */
export async function importViaLibrary(page: Page, paths: string[], { autoLevel }: { autoLevel?: AutoLevel } = {}): Promise<void> {
  await page.getByTestId("nav-library").click();
  await openImportPreview(page, paths);
  if (autoLevel) {
    await page.getByTestId("doc-import-auto-level").click();
    await page.getByRole("option", { name: ko.settings.processing.auto_levels[autoLevel] }).click();
  }
  await page.getByTestId("doc-import-run").click();
  for (const p of paths) {
    await expect(card(page, fileName(p))).toHaveAttribute("data-doc-status", "rendered", { timeout: 15_000 });
  }
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop()!;
}

/** The library card of a document, by its original file name. */
export const card = (page: Page, name: string) => page.locator(`[data-testid^="doc-card-"][data-doc-name="${name}"]`);

export async function docId(page: Page, name: string): Promise<string> {
  return (await card(page, name).getAttribute("data-testid"))!.replace("doc-card-", "");
}

/** Fills the `{{var}}` placeholders of a ko.json template (unknown placeholders are left as they are). */
export function tpl(template: string, vars: Record<string, string | number>): string {
  return template.replace(/\{\{(\w+)\}\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m));
}

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** A regex for a ko.json template: the given vars are fixed, every other placeholder matches a number. */
export function tplPattern(template: string, vars: Record<string, string | number>): RegExp {
  return new RegExp(escapeRe(template).replace(/\\\{\\\{(\w+)\\\}\\\}/g, (_, k: string) => (k in vars ? escapeRe(String(vars[k])) : "\\d+")));
}
