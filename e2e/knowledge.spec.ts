import { expect, test, type Page } from "@playwright/test";

// P3 acceptance (design §4.1): ingest wizard → chunking/index status →
// collections → hybrid search with highlights → grounded answers with
// per-sentence citations (assistant + viewer), explain/summary, glossary,
// versions and settings — against the browser mock backend. Korean
// morphology / recall@10 are covered by the Rust golden-set tests.

async function freshApp(page: Page, flags: Record<string, string> = {}) {
  await page.goto("/");
  await page.evaluate((f) => {
    localStorage.clear();
    localStorage.setItem("doculens.mock.fast", "1");
    for (const [k, v] of Object.entries(f)) localStorage.setItem(k, v);
  }, flags);
  await page.reload();
}

async function createWorkspace(page: Page, path = "C:\\Work\\Docs") {
  await expect(page.getByTestId("onboarding")).toBeVisible();
  await page.getByTestId("workspace-path").fill(path);
  await expect(page.getByTestId("workspace-inspection")).toBeVisible();
  await page.getByTestId("workspace-submit").click();
  await expect(page.getByTestId("hardware-panel")).toBeVisible();
  await page.getByTestId("onboarding-next").click();
  await page.getByTestId("onboarding-finish").click();
  await expect(page.getByTestId("app-shell")).toBeVisible();
}

async function importViaLibrary(page: Page, paths: string[]) {
  await page.getByTestId("nav-library").click();
  await page.getByTestId("library-import").click();
  await page.getByTestId("doc-import-paths").fill(paths.join("\n"));
  await page.getByTestId("doc-import-preview").click();
  await expect(page.getByTestId("doc-import-plan")).toBeVisible();
  await page.getByTestId("doc-import-run").click();
  for (const p of paths) {
    const name = p.split("\\").pop()!;
    await expect(page.locator(`[data-testid^="doc-card-"][data-doc-name="${name}"]`)).toHaveAttribute("data-doc-status", "rendered", { timeout: 15_000 });
  }
}

test.describe("P3 knowledge base", () => {
  test("ingest wizard indexes documents into a collection and search finds them with highlights", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-knowledge").click();
    await expect(page.getByTestId("page-knowledge")).toBeVisible();
    await expect(page.getByTestId("kb-stat-documents")).toHaveText("0");

    await page.getByTestId("knowledge-ingest").click();
    await expect(page.getByTestId("ingest-wizard")).toBeVisible();
    await page.getByTestId("ingest-paths").fill("D:\\규정\\출장비규정-4p.pdf\nD:\\규정\\정보보안지침.pdf");
    await page.getByTestId("ingest-next").click();
    await page.getByTestId("ingest-new-collection").fill("사내 규정");
    await page.getByTestId("ingest-preview").click();
    await expect(page.getByTestId("ingest-plan")).toBeVisible();
    await expect(page.getByTestId("ingest-plan")).toContainText("신규 2");
    await page.getByTestId("ingest-start").click();
    await expect(page.getByTestId("ingest-wizard")).toBeHidden();

    // Import → chunking run in the background; the dashboard follows the events.
    await expect(page.getByTestId("kb-stat-documents")).toHaveText("2", { timeout: 15_000 });
    await expect(page.getByTestId("kb-stat-indexed")).toHaveText("2", { timeout: 15_000 });
    await expect(page.getByTestId("kb-stat-knowledge")).toHaveText("2");
    await expect(page.getByTestId("kb-stat-collections")).toHaveText("1");
    const chunks = Number(await page.getByTestId("kb-stat-chunks").textContent());
    expect(chunks).toBeGreaterThanOrEqual(2);
    await expect(page.getByTestId("kb-tokenizer")).toContainText("ko-dic");

    await page.getByTestId("knowledge-tab-collections").click();
    const col = page.locator('[data-testid="collection-item"][data-collection-name="사내 규정"]');
    await expect(col).toBeVisible();
    await expect(col).toContainText("문서 2");
    await col.click();
    await expect(page.getByTestId("collection-doc")).toHaveCount(2);

    // Library shows the knowledge role badge.
    await page.getByTestId("nav-library").click();
    await expect(page.locator('[data-testid^="doc-role-"]')).toHaveCount(2);

    // Search: terms are highlighted, the hit opens the viewer at the page with the query highlighted.
    await page.getByTestId("nav-search").click();
    await page.getByTestId("search-input").fill("텍스트 레이어");
    await page.getByTestId("search-run").click();
    await expect(page.getByTestId("search-meta")).toBeVisible();
    const hits = page.getByTestId("search-hit");
    await expect(hits.first()).toBeVisible();
    expect(await hits.count()).toBeGreaterThanOrEqual(2);
    await expect(hits.first().locator("mark").first()).toContainText("텍스트");
    await expect(page.getByTestId("search-keyword-only")).toBeVisible();
    await hits.first().getByTestId("search-hit-title").click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();
    await expect(page.getByTestId("viewer-highlight")).toContainText("텍스트 레이어");
    await expect(page.getByTestId("viewer-text").locator("mark").first()).toBeVisible();

    // Scoped to the collection, a query that matches nothing reports no results.
    await page.getByTestId("nav-search").click();
    await page.getByTestId("search-input").fill("플루토늄 냉각수");
    await page.getByTestId("search-run").click();
    await expect(page.getByTestId("search-no-results")).toBeVisible();
  });

  test("assistant answers from the knowledge base with citations and handles missing evidence", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx"]);
    await page.getByTestId("nav-assistant").click();
    await page.getByTestId("assistant-input").fill("합계 금액은?");
    await page.getByTestId("assistant-send").click();
    const card = page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]').first();
    await expect(card).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(card.getByTestId("citation-chip").first()).toBeVisible();
    await expect(card.getByTestId("answer-source").first()).toBeVisible();
    await card.getByTestId("answer-helpful").click();
    // Citation chips open the cited page in the viewer.
    await card.getByTestId("citation-chip").first().click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();
    await expect(page).toHaveURL(/page=1/);

    // No evidence → explicit banner; the user may opt into general knowledge.
    await page.getByTestId("nav-assistant").click();
    await page.getByTestId("assistant-input").fill("xyzzy 플루토늄 원자로");
    await page.getByTestId("assistant-send").click();
    const second = page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]').last();
    await expect(second).toHaveAttribute("data-answer-state", "no_evidence", { timeout: 15_000 });
    await expect(second.getByTestId("answer-no-evidence")).toBeVisible();
    await second.getByTestId("answer-ask-general").click();
    const third = page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]').last();
    await expect(third).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(third).toContainText("일반 지식");

    // The answer log keeps both, with the feedback.
    await page.getByTestId("nav-knowledge").click();
    await page.getByTestId("knowledge-tab-answers").click();
    await expect(page.getByTestId("knowledge-answers").getByTestId("answer-card")).toHaveCount(3);
    await expect(page.getByTestId("kb-stat-feedback")).toBeHidden();
  });

  test("viewer Q&A, explain, summary, version and glossary and knowledge settings", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\영수증-3p.pdf"]);
    await page.locator('[data-testid^="doc-open-"]').first().click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();

    await page.getByTestId("viewer-tab-qa").click();
    await page.getByTestId("viewer-qa-input").fill("합계 금액");
    await page.getByTestId("viewer-qa-ask").click();
    const qa = page.getByTestId("viewer-qa").getByTestId("answer-card").first();
    await expect(qa).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(qa.getByTestId("citation-chip").first()).toBeVisible();

    await page.getByTestId("viewer-summary-run").click();
    await expect(page.getByTestId("viewer-qa").locator('[data-testid="answer-card"][data-answer-state="done"]')).toHaveCount(2, { timeout: 15_000 });

    // Explain a text selection (Ctrl+E / toolbar button).
    await page.getByTestId("viewer-tab-text").click();
    await page.getByTestId("viewer-text").evaluate((el) => {
      const range = document.createRange();
      range.selectNodeContents(el);
      const sel = window.getSelection();
      sel?.removeAllRanges();
      sel?.addRange(range);
      el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    });
    await page.getByTestId("viewer-explain").click();
    const explain = page.getByTestId("viewer-qa").getByTestId("answer-card").first();
    await expect(explain).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(explain).toContainText("설명");

    // Version registration.
    await page.getByTestId("viewer-version").click();
    await page.getByTestId("version-group").fill("영수증 모음");
    await page.getByTestId("version-label").fill("2024");
    await page.getByTestId("version-save").click();
    await expect(page.getByTestId("version-dialog")).toBeHidden();

    // Add to a collection from the viewer.
    await page.getByTestId("viewer-add-to-kb").click();
    await page.getByTestId("add-to-kb-name").fill("증빙");
    await page.getByTestId("add-to-kb-submit").click();
    await expect(page.getByTestId("viewer-kb-badge")).toContainText("증빙");

    await page.getByTestId("nav-knowledge").click();
    await page.getByTestId("knowledge-tab-versions").click();
    await expect(page.locator('[data-testid="version-group"][data-group="영수증 모음"]')).toBeVisible();
    await expect(page.locator('[data-testid="version-doc"][data-latest="true"]')).toHaveCount(1);

    await page.getByTestId("knowledge-tab-glossary").click();
    await page.getByTestId("glossary-add").click();
    await page.getByTestId("glossary-term").fill("출장비");
    await page.getByTestId("glossary-synonyms").fill("여비, 출장 경비");
    await page.getByTestId("glossary-save").click();
    await expect(page.locator('[data-testid="glossary-row"][data-term="출장비"]')).toContainText("여비");

    await page.getByTestId("knowledge-tab-test").click();
    await page.getByTestId("kb-test-input").fill("합계");
    await page.getByTestId("kb-test-run").click();
    await expect(page.getByTestId("kb-test-hit").first()).toBeVisible();

    await page.getByTestId("nav-settings").click();
    await page.getByTestId("settings-tab-knowledge").click();
    const allowGeneral = page.getByTestId("kb-allow-general");
    await expect(allowGeneral).toHaveAttribute("data-state", "unchecked");
    await allowGeneral.click();
    await expect(allowGeneral).toHaveAttribute("data-state", "checked");
    await page.reload();
    await page.getByTestId("nav-settings").click();
    await page.getByTestId("settings-tab-knowledge").click();
    await expect(page.getByTestId("kb-allow-general")).toHaveAttribute("data-state", "checked");
  });
});
