import { expect, test, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { card, createWorkspace, docId, freshApp, importViaLibrary, openAssistantScreen, openSettingsTab } from "./helpers";

// 0.8.0 assistant harness (docs/plan/v0.8.0.md) against the browser mock. The sentences of the
// 2026-11-11 report are the first tests: a settings sentence changes the setting it names, a greeting
// gets a reply instead of an empty plan card, "프로젝트 별 요약" makes one summary per project. Then the
// contract of a model plan: what it leaves out or adds on its own waits for approval with a note.
// The fast path, the routing call, the grammar order and the guard are covered by the Rust tests.

const assistantCard = (page: Page, kind: string) => page.locator(`[data-testid="assistant-card-${kind}"]`);
const answerCards = (page: Page) => page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]');

async function send(page: Page, text: string) {
  await page.getByTestId("assistant-input").fill(text);
  await page.getByTestId("assistant-send").click();
}

async function newProject(page: Page, name: string): Promise<void> {
  await page.getByTestId("nav-projects").click();
  await page.getByTestId("projects-new").click();
  await page.getByTestId("project-name").fill(name);
  await page.getByTestId("project-submit").click();
  await expect(page.getByTestId("project-title")).toHaveText(name);
}

async function addToOpenProject(page: Page, id: string): Promise<void> {
  await page.getByTestId("project-add-documents").click();
  await page.getByTestId(`project-add-doc-${id}`).check();
  await page.getByTestId("project-add-done").click();
  await expect(page.getByTestId(`project-doc-${id}`)).toBeVisible();
}

const settingsValue = (page: Page, path: string) =>
  page.evaluate((p) => {
    const raw = window.localStorage.getItem("doculens.mock.settings");
    let v: unknown = raw ? JSON.parse(raw) : null;
    for (const part of p.split(".")) v = v && typeof v === "object" ? (v as Record<string, unknown>)[part] : undefined;
    return v;
  }, path);

test.describe("0.8.0 assistant harness", () => {
  test("the reported settings sentences change the setting they name, show before → after and can be undone", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);

    await send(page, "UI 배율 120%");
    const first = assistantCard(page, "executed").last();
    await expect(first).toBeVisible({ timeout: 15_000 });
    // Read without the model: the card names the path it took and the change it made.
    await expect(first).toContainText(ko.planner.source.fast);
    await expect(first).toContainText("100");
    await expect(first).toContainText("120");
    await expect(page.locator('[data-testid="assistant-card-plan"]')).toHaveCount(0);
    expect(await settingsValue(page, "ui.scale_percent")).toBe(120);

    await send(page, "배율을 130 으로 변경해줘");
    await expect(assistantCard(page, "executed")).toHaveCount(2, { timeout: 15_000 });
    expect(await settingsValue(page, "ui.scale_percent")).toBe(130);
    // Neither sentence touched the theme or the model profile (what 0.7.0 did with them).
    await expect(page.locator("html")).not.toHaveAttribute("data-theme", "dark");

    // The render DPI is 200 already: the card says so instead of claiming a change, and offers no undo.
    await send(page, "DPI 를 200 으로 변경해줘");
    await expect(assistantCard(page, "executed")).toHaveCount(3, { timeout: 15_000 });
    const dpi = assistantCard(page, "executed").last();
    await expect(dpi).toContainText("200");
    await expect(dpi.getByTestId("executed-undo")).toHaveCount(0);
    expect(await settingsValue(page, "processing.render_dpi")).toBe(200);
    expect(await settingsValue(page, "ui.scale_percent")).toBe(130);

    // 되돌리기 on the second card puts the value it replaced back.
    const second = assistantCard(page, "executed").nth(1);
    await second.getByTestId("executed-undo").click();
    await expect(second.getByTestId("executed-undone")).toBeVisible();
    expect(await settingsValue(page, "ui.scale_percent")).toBe(120);

    await openSettingsTab(page, "general");
    await expect(page.getByTestId("scale-value")).toContainText("120");
  });

  // 0.8.0 review: 되돌리기 replayed the value from before the change whatever had happened since —
  // "배율 120", "배율 150", 되돌리기 on the first card set 100 and said "이전 값으로 되돌렸습니다".
  test("되돌리기 on a settings card whose value was changed again changes nothing and says so", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);

    await send(page, "UI 배율 120%");
    await expect(assistantCard(page, "executed")).toHaveCount(1, { timeout: 15_000 });
    await send(page, "UI 배율 150%");
    await expect(assistantCard(page, "executed")).toHaveCount(2, { timeout: 15_000 });
    expect(await settingsValue(page, "ui.scale_percent")).toBe(150);
    const first = assistantCard(page, "executed").nth(0);
    const second = assistantCard(page, "executed").nth(1);

    // The first card would put 100 back, but what it set (120) is no longer the value: nothing changes.
    await first.getByTestId("executed-undo").click();
    await expect(first.getByTestId("executed-undo-stale")).toHaveText(ko.assistant.undo_stale);
    await expect(first.getByTestId("executed-undo")).toHaveCount(0);
    await expect(first.getByTestId("executed-undone")).toHaveCount(0);
    expect(await settingsValue(page, "ui.scale_percent")).toBe(150);

    // The card keeps saying so after a restart; the last change can still be undone.
    await page.reload();
    await expect(page.getByTestId("app-shell")).toBeVisible();
    await openAssistantScreen(page);
    await expect(assistantCard(page, "executed")).toHaveCount(2, { timeout: 15_000 });
    await expect(first.getByTestId("executed-undo-stale")).toHaveText(ko.assistant.undo_stale);
    await second.getByTestId("executed-undo").click();
    await expect(second.getByTestId("executed-undone")).toBeVisible();
    expect(await settingsValue(page, "ui.scale_percent")).toBe(120);
  });

  // 0.8.0 review: an answer has no approval step, so the part of the request the planner could not cover
  // ("… 메일로 보내줘") was dropped without a word.
  test("a question that also asks for something the app cannot do says what is left out, then answers", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\개발계약서-2p.pdf"]);
    await openAssistantScreen(page);

    await send(page, "위약금 조항 알려주고 메일로 보내줘");
    const note = page.locator('[data-testid="assistant-card-reply"][data-reply-note="unmet"]');
    await expect(note).toHaveCount(1, { timeout: 15_000 });
    await expect(note).toContainText("메일");
    // The answer follows without waiting for an approval: no plan card.
    await expect(answerCards(page)).toHaveCount(1, { timeout: 15_000 });
    await expect(answerCards(page).first()).toHaveAttribute("data-answer-state", /^(done|no_evidence)$/, { timeout: 15_000 });
    await expect(page.locator('[data-testid="assistant-card-plan"]')).toHaveCount(0);
    // The note comes before the answer.
    const cards = page.locator('[data-testid="assistant-messages"] [data-testid^="assistant-card-"]');
    await expect(cards.nth(0)).toHaveAttribute("data-testid", "assistant-card-reply");
    await expect(cards.nth(1)).toHaveAttribute("data-testid", "assistant-card-answer");

    // A plain question has no note.
    await send(page, "위약금 조항 알려줘");
    await expect(answerCards(page)).toHaveCount(2, { timeout: 15_000 });
    await expect(note).toHaveCount(1);
  });

  // The core's fast path takes a sentence that is nothing but a greeting; "안녕?" is left to the model, which
  // answers it as small talk.
  test("a greeting with a question mark is answered by the model as small talk, not by the fast path", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);
    await send(page, "안녕?");
    const chat = answerCards(page).first();
    await expect(chat).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(chat).toContainText("DocuLens 어시스턴트");
    await expect(chat.getByTestId("answer-text-only")).toHaveCount(0);
    await expect(assistantCard(page, "reply")).toHaveCount(0);
  });

  test("a greeting gets a reply at once: no plan card, no answer card, no model", async ({ page }) => {
    // The planner model is off: a greeting must not need it.
    await freshApp(page, { planner_model: "off" });
    await createWorkspace(page);
    await openAssistantScreen(page);

    await send(page, "안녕하세요.");
    const reply = assistantCard(page, "reply").last();
    await expect(reply).toBeVisible({ timeout: 15_000 });
    await expect(reply).toContainText("안녕하세요!");
    await expect(reply).toContainText(ko.planner.source.fast);
    await expect(page.locator('[data-testid="assistant-card-plan"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="assistant-card-clarify"]')).toHaveCount(0);
    await expect(answerCards(page)).toHaveCount(0);

    await send(page, "고마워!");
    await expect(assistantCard(page, "reply")).toHaveCount(2);
    await expect(assistantCard(page, "reply").last()).toContainText("도움이 되어 다행입니다");
  });

  test("small talk and a question that is not about the documents are answered, not planned", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx"]);
    await openAssistantScreen(page);

    await send(page, "넌 누구야?");
    const chat = answerCards(page).first();
    await expect(chat).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(chat).toContainText("DocuLens 어시스턴트");
    await expect(chat.getByTestId("answer-no-evidence")).toHaveCount(0);
    // Nothing was looked up, but this is small talk, not an answer from a text the user sent: no "검색 없이 답변".
    await expect(chat.getByTestId("answer-text-only")).toHaveCount(0);

    // "문서와 무관한 지식 질문에 답하게 할지 → 응답하도록 하자": answered even though no document covers it.
    await send(page, "대한민국의 수도는 어디야?");
    const general = answerCards(page).nth(1);
    await expect(general).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(general.getByTestId("answer-general-badge")).toBeVisible();
    await expect(page.locator('[data-testid="assistant-card-plan"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="assistant-card-clarify"]')).toHaveCount(0);

    // A text the user sends to be worked on is the text-only answer: it keeps the badge.
    await send(page, "다음 문장을 다듬어 정리해줘: 금번 과업은 당사가 수행하며 기한은 12월말까지로 하고 산출물은 설계서와 소스코드로 함. 검수는 발주처가 납품 후 2주 내에 완료하는 것으로 한다.");
    const textOnly = answerCards(page).nth(2);
    await expect(textOnly).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(textOnly.getByTestId("answer-text-only")).toHaveText(ko.answer.text_only);

    // The request's style is kept on the stored message: the same after a restart.
    await page.reload();
    await expect(page.getByTestId("app-shell")).toBeVisible();
    await openAssistantScreen(page);
    await expect(answerCards(page)).toHaveCount(3, { timeout: 15_000 });
    await expect(answerCards(page).nth(0)).toContainText("DocuLens 어시스턴트");
    await expect(answerCards(page).nth(0).getByTestId("answer-text-only")).toHaveCount(0);
    await expect(answerCards(page).nth(2).getByTestId("answer-text-only")).toBeVisible();
  });

  test("'프로젝트 별 내용을 요약해줘' makes one summary per project, with the documents outside every project last", async ({ page }) => {
    test.setTimeout(90_000);
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx", "D:\\scan\\개발계약서-2p.pdf", "D:\\규정\\정보보안지침.pdf"]);
    const estimate = await docId(page, "견적서.hwpx");
    const contract = await docId(page, "개발계약서-2p.pdf");
    await newProject(page, "한빛 포털 구축");
    await addToOpenProject(page, estimate);
    await newProject(page, "새봄 ERP 고도화");
    await addToOpenProject(page, contract);

    await openAssistantScreen(page);
    await send(page, "프로젝트 별 내용을 요약해줘");
    const result = assistantCard(page, "result").last();
    await expect(result).toHaveAttribute("data-result-status", "done", { timeout: 30_000 });
    const groups = result.getByTestId("result-summary-groups");
    await expect(groups).toHaveAttribute("data-group-by", "project");
    const items = groups.getByTestId("result-summary-group");
    await expect(items).toHaveCount(3);
    await expect(items.filter({ hasText: "한빛 포털 구축" })).toHaveCount(1);
    await expect(items.filter({ hasText: "새봄 ERP 고도화" })).toHaveCount(1);
    // The guideline belongs to no project: its group comes last and says so.
    await expect(items.nth(2)).toContainText("프로젝트 없음");
  });

  test("a change the sentence does not name waits for approval with the reason on the card", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);

    // No setting name, no value: the (simulated) model picks a scale on its own.
    await send(page, "글자를 더 크게 해줘");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await expect(plan.locator('[data-warning-code="ungrounded_change"]')).toBeVisible();
    // Nothing changed yet.
    expect(await settingsValue(page, "ui.scale_percent")).toBe(100);
    // A settings plan reads no documents: the card shows no document count.
    await expect(plan).not.toContainText("0건");

    await plan.getByTestId("plan-execute").click();
    await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    expect(await settingsValue(page, "ui.scale_percent")).toBe(125);

    // The same for a change that is not a number: a theme the sentence does not name.
    await send(page, "눈이 편한 화면으로 해줘");
    const theme = assistantCard(page, "plan").last();
    await expect(theme).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await expect(theme.locator('[data-warning-code="ungrounded_change"]')).toContainText("ui.theme");
    await expect(page.locator("html")).not.toHaveAttribute("data-theme", "dark");
    await theme.getByTestId("plan-cancel").click();
    await expect(page.locator("html")).not.toHaveAttribute("data-theme", "dark");
  });

  test("a part the plan leaves out and a split it cannot make are said on the card before anything runs", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx"]);
    await page.getByTestId(`doc-select-${await docId(page, "견적서.hwpx")}`).check();
    await openAssistantScreen(page);
    await page.getByTestId("assistant-scope").click();
    await page.getByTestId("assistant-scope-selection").click();

    await send(page, "선택한 문서를 요약해서 메일로 보내줘");
    const unmet = assistantCard(page, "plan").last();
    await expect(unmet).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await expect(unmet.locator('[data-warning-code="unmet_request"]')).toContainText("메일");
    await unmet.getByTestId("plan-cancel").click();

    await send(page, "선택한 문서를 담당자별로 요약해줘");
    const split = assistantCard(page, "plan").last();
    await expect(split).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await expect(split.locator('[data-warning-code="grouping_dropped"]')).toBeVisible();
  });
});

// 0.8.1: a project's 참고 자료 are part of the project; the sentence says where to look.
test.describe("0.8.1 projects and reference material", () => {
  test("'프로젝트 열어줘' opens the project; a search can be per project and over the reference material only", async ({ page }) => {
    test.setTimeout(90_000);
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx", "D:\\규정\\정보보안지침.pdf", "D:\\scan\\개발계약서-2p.pdf"]);
    const estimate = await docId(page, "견적서.hwpx");
    const guideline = await docId(page, "정보보안지침.pdf");
    await newProject(page, "한빛 포털 구축");
    await addToOpenProject(page, estimate);
    // The guideline goes into the project's reference group.
    await expect(page.getByTestId("project-documents")).toHaveAttribute("data-group-id", /.+/, { timeout: 10_000 });
    await page.getByTestId("project-add-documents").click();
    await page.getByTestId("project-add-kind-reference").check();
    await page.getByTestId(`project-add-doc-${guideline}`).check();
    await page.getByTestId("project-add-done").click();
    await expect(page.getByTestId(`project-doc-${guideline}`)).toHaveAttribute("data-kind", "reference");

    // "X 프로젝트 열어줘" used to land on the library's folder view.
    await page.getByTestId("nav-library").click();
    await openAssistantScreen(page);
    await page.getByTestId("assistant-scope").click();
    await page.getByRole("option", { name: ko.assistant.scope.all, exact: true }).click();
    await send(page, "한빛 포털 구축 프로젝트 열어줘");
    await expect(page.getByTestId("project-title")).toHaveText("한빛 포털 구축", { timeout: 15_000 });

    // The hits per project: the reference document counts for its project, the contract has none.
    await openAssistantScreen(page);
    await page.getByTestId("assistant-scope").click();
    await page.getByRole("option", { name: ko.assistant.scope.all, exact: true }).click();
    await send(page, "프로젝트별로 텍스트 검색해줘");
    const grouped = assistantCard(page, "result").last();
    await expect(grouped).toHaveAttribute("data-result-status", "done", { timeout: 30_000 });
    const groups = grouped.getByTestId("result-search-group");
    await expect(groups).toHaveCount(2);
    await expect(groups.nth(0)).toContainText("한빛 포털 구축");
    await expect(groups.nth(0)).toContainText("정보보안지침");
    await expect(groups.nth(1)).toContainText("프로젝트 없음");

    // "참고 자료에서 …": only the reference document is searched.
    await send(page, "참고 자료에서 텍스트 검색해줘");
    await expect(assistantCard(page, "result")).toHaveCount(2, { timeout: 30_000 });
    const reference = assistantCard(page, "result").last();
    await expect(reference).toHaveAttribute("data-result-status", "done", { timeout: 30_000 });
    const hits = reference.getByTestId("result-hit");
    await expect(hits.first()).toContainText("정보보안지침");
    await expect(reference.getByTestId("result-search")).not.toContainText("견적서");
    await expect(reference.getByTestId("result-search")).not.toContainText("개발계약서");
  });
  test("1.0.0: the chosen steps are listed only when switched on; a plan that needs a confirmation always lists them", async ({ page }) => {
    test.setTimeout(90_000);
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx", "D:\\scan\\개발계약서-2p.pdf"]);
    await openAssistantScreen(page);
    const plans = assistantCard(page, "plan");

    // Off by default: a search runs at once and its card shows the reading and the result, not the step list.
    expect(await settingsValue(page, "ui.show_assistant_steps")).toBe(false);
    await send(page, "납품 기한 검색해줘");
    await expect(plans).toHaveCount(1, { timeout: 15_000 });
    await expect(plans.nth(0).getByTestId("plan-summary")).toBeVisible();
    await expect(assistantCard(page, "result")).toHaveCount(1, { timeout: 15_000 });
    await expect(plans.nth(0).getByTestId("plan-steps")).toHaveCount(0);

    // A deletion waits for approval, so what it will do is listed whatever the setting says.
    await send(page, "견적서 삭제해줘");
    await expect(plans).toHaveCount(2, { timeout: 15_000 });
    await expect(plans.nth(1)).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await expect(plans.nth(1).getByTestId("plan-steps")).toBeVisible();
    await expect(plans.nth(1).getByTestId("plan-step-0")).toHaveAttribute("data-op", "delete");
    await plans.nth(1).getByTestId("plan-cancel").click();

    // Switched on by a sentence: the next search lists its step.
    await send(page, "어시스턴트 작업 표시 켜줘");
    await expect.poll(() => settingsValue(page, "ui.show_assistant_steps"), { timeout: 15_000 }).toBe(true);
    await send(page, "검수 조건 검색해줘");
    await expect(plans).toHaveCount(3, { timeout: 15_000 });
    await expect(plans.nth(2).getByTestId("plan-step-0")).toHaveAttribute("data-op", "search");
    // … and the card that ran before follows the setting too.
    await expect(plans.nth(0).getByTestId("plan-steps")).toBeVisible();

    // The switch in 설정 > 일반 shows the same value and stays inside the window.
    await openSettingsTab(page, "general");
    const toggle = page.getByTestId("settings-assistant-steps");
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    const box = await toggle.boundingBox();
    const width = page.viewportSize()?.width ?? 0;
    expect(box && box.x >= 0 && box.x + box.width <= width).toBe(true);
    await toggle.click();
    await expect.poll(() => settingsValue(page, "ui.show_assistant_steps")).toBe(false);
  });
  test("1.0.0: a library card shows the projects of its document as badges that stay inside the card", async ({ page }) => {
    test.setTimeout(120_000);
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx", "D:\\규정\\정보보안지침.pdf", "D:\\scan\\개발계약서-2p.pdf"]);
    const estimate = await docId(page, "견적서.hwpx");
    const guideline = await docId(page, "정보보안지침.pdf");
    const contract = await docId(page, "개발계약서-2p.pdf");
    // No project yet: no badge.
    await expect(page.getByTestId(`doc-project-${estimate}`)).toHaveCount(0);

    const longName = "한빛 포털 구축 2026년도 통합 유지보수 및 고도화 사업";
    await newProject(page, longName);
    await addToOpenProject(page, estimate);
    // The guideline goes into the project's reference group.
    await expect(page.getByTestId("project-documents")).toHaveAttribute("data-group-id", /.+/, { timeout: 10_000 });
    await page.getByTestId("project-add-documents").click();
    await page.getByTestId("project-add-kind-reference").check();
    await page.getByTestId(`project-add-doc-${guideline}`).check();
    await page.getByTestId("project-add-done").click();
    await expect(page.getByTestId(`project-doc-${guideline}`)).toHaveAttribute("data-kind", "reference");
    for (const name of ["새봄 ERP", "미래 데이터 허브"]) {
      await newProject(page, name);
      await addToOpenProject(page, estimate);
    }

    await page.getByTestId("nav-library").click();
    const inside = async (name: string, testId: string) => {
      const outer = await card(page, name).boundingBox();
      const boxes = await page.getByTestId(testId).evaluateAll((els) => els.map((e) => e.getBoundingClientRect()).map((r) => ({ left: r.left, right: r.right, top: r.top, bottom: r.bottom })));
      expect(outer).not.toBeNull();
      expect(boxes.length).toBeGreaterThan(0);
      for (const b of boxes) {
        // Not wider than the card, and not cut off below it either (the card grows with its badge rows).
        expect(b.left).toBeGreaterThanOrEqual(outer!.x - 0.5);
        expect(b.right).toBeLessThanOrEqual(outer!.x + outer!.width + 0.5);
        expect(b.top).toBeGreaterThanOrEqual(outer!.y - 0.5);
        expect(b.bottom).toBeLessThanOrEqual(outer!.y + outer!.height + 0.5);
      }
    };
    // Three projects: two badges and "+1"; the long name is cut inside its badge, the full name is the tooltip.
    const badges = page.getByTestId(`doc-project-${estimate}`);
    await expect(badges).toHaveCount(2, { timeout: 10_000 });
    await expect(badges.first()).toHaveAttribute("title", ko.library.project_badge.replace("{{name}}", longName));
    await expect(page.getByTestId(`doc-project-more-${estimate}`)).toHaveText("+1");
    await inside("견적서.hwpx", `doc-project-${estimate}`);
    // Reference material of a project carries the badge too; a document in no project has none.
    await expect(page.getByTestId(`doc-project-${guideline}`)).toHaveCount(1);
    await expect(page.getByTestId(`doc-project-${guideline}`)).toHaveAttribute("data-reference", "true");
    await expect(page.getByTestId(`doc-project-${contract}`)).toHaveCount(0);

    // At the largest UI scale the badges still stay inside the card.
    await openAssistantScreen(page);
    await send(page, "UI 배율 150으로 바꿔줘");
    await expect.poll(() => settingsValue(page, "ui.scale_percent"), { timeout: 15_000 }).toBe(150);
    await page.getByTestId("nav-library").click();
    await expect(badges).toHaveCount(2, { timeout: 10_000 });
    await inside("견적서.hwpx", `doc-project-${estimate}`);
    await inside("견적서.hwpx", `doc-project-more-${estimate}`);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
  });
  test("1.0.0: document pickers have select all / none and narrow to a project's documents", async ({ page }) => {
    test.setTimeout(120_000);
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서.hwpx", "D:\\규정\\정보보안지침.pdf", "D:\\scan\\개발계약서-2p.pdf"]);
    const estimate = await docId(page, "견적서.hwpx");
    const guideline = await docId(page, "정보보안지침.pdf");
    const contract = await docId(page, "개발계약서-2p.pdf");
    await newProject(page, "한빛 포털");
    await addToOpenProject(page, estimate);
    await expect(page.getByTestId("project-documents")).toHaveAttribute("data-group-id", /.+/, { timeout: 10_000 });
    await page.getByTestId("project-add-documents").click();
    await page.getByTestId("project-add-kind-reference").check();
    await page.getByTestId(`project-add-doc-${guideline}`).check();
    await page.getByTestId("project-add-done").click();
    await expect(page.getByTestId(`project-doc-${guideline}`)).toHaveAttribute("data-kind", "reference");
    const count = (n: number) => ko.common.selected_count.replace("{{count}}", String(n));

    // 문서 작성 → 원본 고르기: select all / none act on the visible rows.
    await page.getByTestId("nav-generate").click();
    await page.getByTestId("generate-pick-sources").click();
    const dialog = page.getByTestId("generate-sources-dialog");
    await expect(dialog).toBeVisible();
    const boxes = dialog.locator('input[type="checkbox"]');
    await expect(boxes).toHaveCount(3);
    await expect(page.getByTestId("generate-sources-select-count")).toHaveText(count(0));
    await expect(page.getByTestId("generate-sources-select-none")).toBeDisabled();
    await page.getByTestId("generate-sources-select-all").click();
    await expect(page.getByTestId("generate-sources-select-count")).toHaveText(count(3));
    await expect(page.getByTestId("generate-sources-select-all")).toBeDisabled();
    await page.getByTestId("generate-sources-select-none").click();
    await expect(page.getByTestId("generate-sources-select-count")).toHaveText(count(0));
    // Narrowed to the project: its work document and its reference material, picked in one go.
    await page.getByTestId("generate-sources-project").click();
    await page.getByRole("option", { name: "한빛 포털" }).click();
    await expect(boxes).toHaveCount(2);
    await expect(page.getByTestId(`generate-source-${contract}`)).toHaveCount(0);
    await page.getByTestId("generate-sources-select-all").click();
    await expect(page.getByTestId("generate-sources-select-count")).toHaveText(count(2));
    // Back to every document: the two stay picked, the third is not.
    await page.getByTestId("generate-sources-project").click();
    await page.getByRole("option", { name: ko.projects.filter.all }).click();
    await expect(boxes).toHaveCount(3);
    await expect(page.getByTestId(`generate-source-${estimate}`)).toBeChecked();
    await expect(page.getByTestId(`generate-source-${guideline}`)).toBeChecked();
    await expect(page.getByTestId(`generate-source-${contract}`)).not.toBeChecked();
    // The added controls stay inside the dialog, and the dialog inside the window.
    const outer = (await dialog.boundingBox())!;
    for (const id of ["generate-sources-project", "generate-sources-select"]) {
      const b = (await page.getByTestId(id).boundingBox())!;
      expect(b.x).toBeGreaterThanOrEqual(outer.x - 0.5);
      expect(b.x + b.width).toBeLessThanOrEqual(outer.x + outer.width + 0.5);
    }
    expect(outer.x >= 0 && outer.x + outer.width <= (page.viewportSize()?.width ?? 0)).toBe(true);
    await page.getByTestId("generate-sources-done").click();
    await expect(page.getByTestId("generate-sources").locator("li")).toHaveCount(2);

    // 프로젝트의 문서 추가: another project's documents are picked the same way.
    await newProject(page, "새봄 ERP");
    await page.getByTestId("project-add-documents").click();
    await page.getByTestId("project-add-project").click();
    await page.getByRole("option", { name: "한빛 포털" }).click();
    await expect(page.getByTestId("project-add-dialog").locator('input[type="checkbox"]')).toHaveCount(2);
    await page.getByTestId("project-add-select-all").click();
    await expect(page.getByTestId("project-add-select-count")).toHaveText(count(2));
    await page.getByTestId("project-add-done").click();
    await expect(page.getByTestId(`project-doc-${estimate}`)).toBeVisible();
    await expect(page.getByTestId(`project-doc-${guideline}`)).toBeVisible();
  });
  test("1.0.0: 프로젝트 분류(2순위 추천 임계값) is a setting that a sentence can change", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);
    expect(await settingsValue(page, "processing.project_second_choice")).toBe(0.6);
    // Not the exact label: a near wording names the same setting.
    await send(page, "2순위 추천 임계값 70%로 바꿔줘");
    await expect.poll(() => settingsValue(page, "processing.project_second_choice"), { timeout: 15_000 }).toBe(0.7);
    // 설정 > 처리 shows it as a percentage, inside the window.
    await openSettingsTab(page, "processing");
    const field = page.getByTestId("settings-project-second-choice");
    await expect(field).toHaveValue("70");
    const box = (await field.boundingBox())!;
    expect(box.x >= 0 && box.x + box.width <= (page.viewportSize()?.width ?? 0)).toBe(true);
    await field.fill("85");
    await field.blur();
    await expect.poll(() => settingsValue(page, "processing.project_second_choice")).toBe(0.85);
  });

  test("1.0.0: 이미지 내 글자 추출 is off by default, a switch in 설정 > 처리 and a sentence turn it on", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openSettingsTab(page, "processing");
    const toggle = page.getByTestId("settings-ocr-figures");
    await expect(toggle).toHaveAttribute("aria-checked", "false");
    await expect(page.getByText(ko.settings.processing.ocr_figures, { exact: true })).toBeVisible();
    // The hint says what it costs; the row stays inside the window.
    await expect(page.getByText(ko.settings.processing.ocr_figures_hint)).toBeVisible();
    const box = (await toggle.boundingBox())!;
    expect(box.x >= 0 && box.x + box.width <= (page.viewportSize()?.width ?? 0)).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    await toggle.click();
    await expect.poll(() => settingsValue(page, "processing.ocr_figures")).toBe(true);
    // By a sentence, with a near wording; the other OCR switch is not touched.
    await openAssistantScreen(page);
    await send(page, "그림 글자 추출 꺼줘");
    await expect.poll(() => settingsValue(page, "processing.ocr_figures"), { timeout: 15_000 }).toBe(false);
    await send(page, "이미지 내 글자 추출 켜줘");
    await expect.poll(() => settingsValue(page, "processing.ocr_figures"), { timeout: 15_000 }).toBe(true);
    expect(await settingsValue(page, "processing.ocr_digital_pdf")).toBe(false);
  });
});
