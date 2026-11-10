import { expect, test } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { createWorkspace, freshApp, importViaLibrary, tpl } from "./helpers";

const t_general_badge = ko.answer.general_badge;

// P3 acceptance (design §4.1): ingest wizard → chunking/index status →
// collections → hybrid search with highlights → grounded answers with
// per-sentence citations (assistant + viewer), explain/summary, glossary,
// versions and settings — against the browser mock backend. Korean
// morphology / recall@10 are covered by the Rust golden-set tests.

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
    await expect(page.getByTestId("ingest-plan")).toContainText(tpl(ko.knowledge.wizard.summary, { new: 2, relink: 0, dup: 0, registered: 0, unsupported: 0 }));
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
    await expect(col).toContainText(tpl(ko.knowledge.collections.docs_count, { count: 2 }));
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
    await expect(hits.nth(1)).toBeVisible();
    await expect(hits.first().locator("mark").first()).toContainText("텍스트");
    await expect(page.getByTestId("search-keyword-only")).toBeVisible();
    await hits.first().getByTestId("search-hit-title").click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();
    await expect(page.getByTestId("viewer-highlight")).toContainText("텍스트 레이어");
    await expect(page.getByTestId("viewer-text-layer").locator("mark").first()).toBeVisible();

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
    await expect(card.getByTestId("answer-helpful")).toHaveAttribute("aria-pressed", "true");

    // A second, unrelated question is retrieved afresh (no follow-up) → explicit "no evidence" banner,
    // and the user may opt into general knowledge.
    await page.getByTestId("assistant-input").fill("xyzzy 플루토늄 원자로");
    await page.getByTestId("assistant-send").click();
    const second = page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]').last();
    await expect(second).toHaveAttribute("data-answer-state", "no_evidence", { timeout: 15_000 });
    await expect(second.getByTestId("answer-no-evidence")).toBeVisible();
    await second.getByTestId("answer-ask-general").click();
    const third = page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]').last();
    await expect(third).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(third.getByTestId("answer-general-badge")).toHaveText(t_general_badge);

    // Work items are not answered as questions: the planner (P5) replies with a plan / clarification card.
    await page.getByTestId("assistant-input").fill("지난달 견적서 정리해줘");
    await page.getByTestId("assistant-send").click();
    await expect(page.locator('[data-testid="assistant-card-plan"], [data-testid="assistant-card-clarify"], [data-testid="assistant-card-result"], [data-testid="assistant-card-execution"]').last()).toBeVisible({ timeout: 15_000 });

    // Citation chips of the first answer open the cited page in the viewer.
    await card.getByTestId("citation-chip").first().click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();
    await expect(page).toHaveURL(/page=1/);
    await expect(page.getByTestId("viewer-text-layer").locator("mark").first()).toBeVisible();

    // The answer log keeps the three answers, with the feedback.
    await page.getByTestId("nav-knowledge").click();
    await page.getByTestId("knowledge-tab-answers").click();
    await expect(page.getByTestId("knowledge-answers").getByTestId("answer-card")).toHaveCount(3);
    // The log is newest-first: the feedback went to the first (oldest) answer.
    await expect(page.getByTestId("knowledge-answers").getByTestId("answer-helpful").last()).toHaveAttribute("aria-pressed", "true");
  });

  test("assistant remembers the conversation: a follow-up keeps the evidence, 새 대화 forgets it", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx"]);
    await page.getByTestId("nav-assistant").click();
    const memory = page.getByTestId("assistant-memory");
    await expect(memory).toHaveAttribute("data-turns", "0");
    await page.getByTestId("assistant-input").fill("합계 금액은?");
    await page.getByTestId("assistant-send").click();
    const first = page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]').first();
    await expect(first).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(memory).toHaveAttribute("data-turns", "2");
    // "그건 …" points back at the previous answer: on its own the question matches nothing, with the
    // conversation it keeps the evidence and is answered.
    await page.getByTestId("assistant-input").fill("그건 누가 작성했어?");
    await page.getByTestId("assistant-send").click();
    const second = page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]').nth(1);
    await expect(second).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(second.getByTestId("citation-chip").first()).toBeVisible();
    await expect(memory).toHaveAttribute("data-turns", "4");
    // 새 대화 clears the memory: the same follow-up now has nothing to refer to.
    await page.getByRole("button", { name: ko.assistant.new_chat }).click();
    await expect(memory).toHaveAttribute("data-turns", "0");
    await page.getByTestId("assistant-input").fill("그건 누가 작성했어?");
    await page.getByTestId("assistant-send").click();
    const third = page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]').last();
    await expect(third).toHaveAttribute("data-answer-state", "no_evidence", { timeout: 15_000 });
  });

  test("viewer Q&A, explain, summary, version and glossary and knowledge settings", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서-3p.pdf"]);
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

    // Explain words selected on the page image (Ctrl+E / toolbar button).
    await expect(page.getByTestId("viewer-explain")).toBeDisabled();
    await page.getByTestId("viewer-text-layer").locator("[data-word-index]").first().click();
    await expect(page.getByTestId("viewer-selection-toolbar")).toBeVisible();
    await page.getByTestId("viewer-explain").click();
    const explain = page.getByTestId("viewer-qa").getByTestId("answer-card").first();
    await expect(explain).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(explain).toContainText(ko.answer.kinds.explain);

    // Version registration.
    await page.getByTestId("viewer-version").click();
    await page.getByTestId("version-group").fill("견적서 모음");
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
    await expect(page.locator('[data-testid="version-group"][data-group="견적서 모음"]')).toBeVisible();
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
