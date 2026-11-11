import { expect, test, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { card, createWorkspace, docId, freshApp, importViaLibrary, openSettingsTab, tpl } from "./helpers";

// P5 assistant planner (design §3.5 / §3.7.6) against the browser mock: the
// aggregate card, plan → execution → result cards of an export, the organise
// diff preview + Ctrl+Enter + undo, instant slash commands and the
// clarification card. Rule planning, validation and the executor are covered
// by the Rust tests.

const assistantCard = (page: Page, kind: string) => page.locator(`[data-testid="assistant-card-${kind}"]`);

async function openAssistant(page: Page) {
  await page.getByTestId("nav-assistant").click();
  await expect(page.getByTestId("page-assistant")).toBeVisible();
}

async function send(page: Page, text: string) {
  await page.getByTestId("assistant-input").fill(text);
  await page.getByTestId("assistant-send").click();
}

/** Runs 표준 처리 on the (already imported) receipt from the library bulk bar; the selection is cleared again. */
async function processReceipt(page: Page, name = "견적서-1p.jpg") {
  const select = page.getByTestId(`doc-select-${await docId(page, name)}`);
  await select.check();
  await page.getByTestId("library-process").click();
  await page.getByTestId("library-process-standard").click();
  await expect(card(page, name)).toHaveAttribute("data-doc-status", "extracted", { timeout: 15_000 });
  await select.uncheck();
}

/** Approves every flagged field of the receipt in the review queue so the file operations may move it. */
async function approveReceipt(page: Page, name = "견적서-1p.jpg") {
  await page.getByTestId("nav-review").click();
  await expect(page.getByTestId("page-review")).toBeVisible();
  await page.getByTestId("review-approve-doc").click();
  await expect(page.getByTestId("review-empty")).toBeVisible();
  await page.getByTestId("nav-library").click();
  await expect(card(page, name)).toHaveAttribute("data-doc-status", "reviewed");
}

/**
 * Horizontal overflow of the assistant's message list (UI-2; `e2e/scale.spec.ts` checks the other screens):
 * the list's scroll width against its width, the cards that reach past its right edge and the document root.
 */
async function assistantOverflow(page: Page) {
  return page.evaluate(() => {
    const list = document.querySelector<HTMLElement>('[data-testid="assistant-messages"]')!;
    const right = list.getBoundingClientRect().right;
    const cards = [...list.querySelectorAll<HTMLElement>('[data-testid^="assistant-card-"]')];
    return {
      list: list.scrollWidth - list.clientWidth,
      cardsPastEdge: cards.filter((c) => c.getBoundingClientRect().right > right + 0.5).map((c) => c.dataset.testid),
      root: document.documentElement.scrollWidth - window.innerWidth,
    };
  });
}

/** Plans without side effects run at once; otherwise approve the plan card. Resolves once the card reports done. */
async function approveIfPending(page: Page, plan: ReturnType<typeof assistantCard>) {
  const execute = plan.getByTestId("plan-execute");
  if ((await plan.getAttribute("data-plan-status")) === "pending" && (await execute.isVisible())) await execute.click();
  await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
}

test.describe("P5 planner", () => {
  test("aggregate: '지난달 견적서 합계' shows the aggregate card with the 검토 대기 포함 switch", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서-1p.jpg", "D:\\scan\\개발계약서-2p.pdf"], { autoLevel: "none" });
    await processReceipt(page);

    // The viewer's document stays the assistant's "현재 문서" after leaving the viewer (review H1).
    await card(page, "견적서-1p.jpg").locator('[data-testid^="doc-open-"]').click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();
    await openAssistant(page);
    await expect(page.getByTestId("assistant-current-document")).toContainText(tpl(ko.assistant.scope.document_named, { name: "견적서-1p.jpg" }));
    await page.getByTestId("assistant-scope").click();
    await expect(page.getByTestId("assistant-scope-document")).toContainText("견적서-1p.jpg");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("assistant-scope-document")).toBeHidden();

    await send(page, "지난달 견적서 합계");
    const aggregate = assistantCard(page, "aggregate").last();
    await expect(aggregate).toBeVisible({ timeout: 15_000 });
    await expect(aggregate).toContainText(ko.planner.aggregate.basis.doc_date);
    const value = aggregate.getByTestId("aggregate-value");
    await expect(value).toBeVisible();
    // The receipt's total is review pending: excluded from the headline until "검토 대기 포함" is on.
    const before = (await value.textContent()) ?? "";
    expect(before).not.toBe("");
    await expect(aggregate.getByTestId("aggregate-split")).toContainText("대기 1건");
    const toggle = aggregate.getByTestId("aggregate-include-unreviewed");
    await expect(toggle).toBeVisible();
    await expect(toggle).toHaveAttribute("aria-checked", "false");
    await toggle.click();
    await expect(aggregate).toHaveAttribute("data-include-unreviewed", "true", { timeout: 10_000 });
    await expect(value).not.toHaveText(before);
    await expect(value).toContainText("12,500,000");
    await aggregate.getByTestId("aggregate-toggle-documents").click();
    await expect(aggregate.getByTestId("aggregate-documents")).toBeVisible();
  });

  test("export: '개발 계약서 → 엑셀' plan card → execution card → result card with the export path", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서-1p.jpg", "D:\\scan\\개발계약서-2p.pdf"], { autoLevel: "none" });
    // Classify + extract both documents so the contract carries its type (the export scope is `doc_type = dev_contract`).
    for (const name of ["견적서-1p.jpg", "개발계약서-2p.pdf"]) await page.getByTestId(`doc-select-${await docId(page, name)}`).check();
    await page.getByTestId("library-process").click();
    await page.getByTestId("library-process-standard").click();
    await expect(card(page, "개발계약서-2p.pdf")).toHaveAttribute("data-doc-status", "extracted", { timeout: 15_000 });

    await openAssistant(page);
    await send(page, "개발 계약서 → 엑셀");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toBeVisible({ timeout: 15_000 });
    await expect(plan).toContainText(ko.planner.ops.export);
    await expect(plan.getByTestId("plan-step-0")).toBeVisible();
    await expect(plan.getByTestId("plan-impact")).toContainText(tpl(ko.planner.card.impact_export, { count: 1 }));
    await approveIfPending(page, plan);
    await expect(assistantCard(page, "execution").last()).toBeVisible({ timeout: 15_000 });
    const result = assistantCard(page, "result").last();
    await expect(result).toBeVisible({ timeout: 20_000 });
    await expect(result).toHaveAttribute("data-result-status", "done");
    await expect(result.getByTestId("result-export-path")).toContainText(/exports\/.*\.xlsx$/);
    await expect(result.getByTestId("result-open-folder")).toBeVisible();

    // The job appears on the Jobs page's 완료 tab with its structured result.
    await page.getByTestId("nav-jobs").click();
    await page.getByTestId("jobs-tab-finished").click();
    const job = page.locator('[data-job-kind="plan"]').first();
    await expect(job).toContainText(ko.jobs.kind.plan);
    await expect(job.getByTestId("job-state")).toHaveText(ko.jobs.state.done);
    await expect(job.getByTestId("job-priority")).toBeVisible();
    await job.getByTestId("job-result").locator("summary").click();
    await expect(job.getByTestId("job-result-steps")).toContainText(ko.planner.ops.export);
  });

  test("organise: diff preview → Ctrl+Enter → result card → 되돌리기 toast", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서-1p.jpg"], { autoLevel: "none" });
    await processReceipt(page);
    await approveReceipt(page);

    await openAssistant(page);
    await send(page, "견적서 월별 폴더로 옮겨");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toBeVisible({ timeout: 15_000 });
    await expect(plan).toHaveAttribute("data-plan-status", "pending");
    await expect(plan).toHaveAttribute("data-plan-active", "true");
    await expect(plan).toContainText(ko.planner.ops.move);
    // UI-15: the step names the grouping and the folder in words, not `documents/{{year}}/… · group_by=month`.
    await expect(plan.getByTestId("plan-step-params-0")).toHaveText(tpl(ko.planner.params.folder_grouped, { group: ko.planner.params.group_month, path: "documents/<연도>/<월>/<유형>" }));
    await expect(plan.getByTestId("plan-only-reviewed")).toBeVisible();
    await expect(plan.getByTestId("plan-impact")).toContainText(tpl(ko.planner.card.impact_moves, { count: 1 }));
    const row = plan.locator('[data-testid="plan-preview-row"][data-item-status="move"]').first();
    await expect(row).toBeVisible();
    await expect(row).toContainText("견적서-1p.jpg");
    // Esc inside the composer belongs to the text field: the card stays pending (review M5).
    await page.getByTestId("assistant-input").focus();
    await page.keyboard.press("Escape");
    await expect(plan).toHaveAttribute("data-plan-status", "pending");
    // Inline edit into an invalid plan: the executor rejects it with the issue list (plan_invalid), path included.
    await plan.getByTestId("plan-edit").click();
    const folderField = plan.getByTestId("plan-editor-param-0-folder");
    await expect(folderField).toHaveValue("documents/{{year}}/{{month}}/{{doc_type}}");
    await folderField.fill("../x");
    await plan.getByTestId("plan-execute").click();
    const errors = plan.getByTestId("plan-errors");
    await expect(errors).toBeVisible({ timeout: 10_000 });
    await expect(errors).toContainText(ko.planner.card.invalid);
    await expect(errors).toContainText("operations[0].folder");
    await expect(plan).toHaveAttribute("data-plan-status", "pending");
    // Fixing the field clears the errors; the corrected plan runs.
    await folderField.fill("documents/{{year}}/{{month}}/{{doc_type}}");
    await expect(errors).toBeHidden();
    await plan.getByTestId("plan-edit").click();
    await page.keyboard.press("Control+Enter");
    await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    const result = assistantCard(page, "result").last();
    await expect(result).toBeVisible({ timeout: 20_000 });
    await expect(result).toHaveAttribute("data-result-status", "done");
    await expect(result.getByTestId("result-step-0")).toHaveAttribute("data-step-status", "done");
    await result.getByTestId("result-toggle-moves").click();
    await expect(result.getByTestId("result-moves")).toBeVisible();
    const undo = result.getByTestId("result-undo");
    await expect(undo).toBeVisible();
    await undo.click();
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.palette.undo_done, { restored: 1, skipped: 0 }), { timeout: 10_000 });
    await expect(undo).toBeDisabled();
  });

  // UI-2: a file-op plan lists paths; a long file name made the preview table — and with it the card and the
  // whole message list — wider than the column (2,036 px in an 832 px column; at 150 % even ordinary names).
  test("plan and result cards with a long file path stay inside the message column at 100 % and 150 %", async ({ page }) => {
    test.setTimeout(60_000);
    const name = "견적서-한빛소프트웨어-차세대-통합-문서관리-시스템-구축-사업-2차-수정-최종본-검토용-사본-1p.jpg";
    const none = { list: 0, cardsPastEdge: [], root: 0 };
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, [`D:\\scan\\${name}`], { autoLevel: "none" });
    await processReceipt(page, name);
    await approveReceipt(page, name);

    await openAssistant(page);
    await send(page, "견적서 월별 폴더로 옮겨");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    const row = plan.locator('[data-testid="plan-preview-row"][data-item-status="move"]').first();
    await expect(row).toContainText(name);
    // The whole path is in the row (wrapped) and in the cell's title.
    await expect(row.locator("td").first()).toHaveAttribute("title", `documents/${name}`);
    expect(await assistantOverflow(page)).toEqual(none);

    // 150 %: every rem grows, the column gets narrower. (Enter, not the send button: a toast may lie over it.)
    const sendByEnter = async (text: string) => {
      await page.getByTestId("assistant-input").fill(text);
      await page.getByTestId("assistant-input").press("Enter");
    };
    await sendByEnter("UI 배율 150%로");
    await expect(assistantCard(page, "executed").last()).toBeVisible({ timeout: 15_000 });
    await expect(page.locator("html")).toHaveCSS("font-size", "24px");
    await expect.poll(() => assistantOverflow(page)).toEqual(none);

    // The result card's move log has the same paths.
    await plan.getByTestId("plan-execute").click();
    const result = assistantCard(page, "result").last();
    await expect(result).toBeVisible({ timeout: 20_000 });
    await result.getByTestId("result-toggle-moves").click();
    await expect(result.getByTestId("result-moves")).toContainText(name);
    await expect.poll(() => assistantOverflow(page)).toEqual(none);
    await sendByEnter("UI 배율 100%로");
    await expect(page.locator("html")).toHaveCSS("font-size", "16px");
    await expect.poll(() => assistantOverflow(page)).toEqual(none);
  });

  // KNO-9: the card's state used to be written 300 ms after it changed, so a reload (or a workspace switch /
  // the app closing) inside that window restored the finished plan as "승인 대기" — runnable again.
  test("a plan that finished right before a reload is restored as executed, not as waiting for approval", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서-1p.jpg"], { autoLevel: "none" });
    await processReceipt(page);
    await approveReceipt(page);

    await openAssistant(page);
    await send(page, "견적서 월별 폴더로 옮겨");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await plan.getByTestId("plan-execute").click();
    await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    // No pause before the reload.
    await page.reload();
    await expect(page.getByTestId("app-shell")).toBeVisible();
    await openAssistant(page);
    const restored = assistantCard(page, "plan").last();
    await expect(restored).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    await expect(restored.getByTestId("plan-execute")).toHaveCount(0);
    // Ctrl+Enter finds no plan waiting: the move does not run a second time.
    await expect(assistantCard(page, "execution")).toHaveCount(1);
    await page.getByTestId("assistant-messages").click();
    await page.keyboard.press("Control+Enter");
    await page.waitForTimeout(500);
    await expect(assistantCard(page, "execution")).toHaveCount(1);
    await expect(assistantCard(page, "plan")).toHaveCount(1);
  });

  // PLN-7: a command that changes documents but names none is answered with "어떤 문서를 … (전체면 '전부')".
  // The plan made from the reply "전부" could not run: execution checked the scope against the original
  // sentence, which does not say "전부". The card keeps the 되묻기 text and sends it along (`planned_text`).
  test("a plan made by answering the scope question with '전부' can run", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서-1p.jpg", "D:\\scan\\개발계약서-2p.pdf"], { autoLevel: "none" });
    await openAssistant(page);

    // No selection, folder or type: the planner asks which documents.
    await send(page, "태그 '긴급' 달아줘");
    const first = assistantCard(page, "clarify").last();
    await expect(first).toHaveAttribute("data-clarify-status", "pending", { timeout: 15_000 });
    await expect(first.getByTestId("clarify-question")).toContainText("전부");
    // A reply that does not name the whole library is asked again (the question's own "전부" does not count).
    await first.getByTestId("clarify-reply").fill("그냥 해줘");
    await first.getByTestId("clarify-ask-back").click();
    await expect(assistantCard(page, "clarify")).toHaveCount(2, { timeout: 15_000 });
    const second = assistantCard(page, "clarify").last();
    await expect(second).toHaveAttribute("data-clarify-status", "pending");
    await expect(assistantCard(page, "plan")).toHaveCount(0);

    // "전부": the plan covers the library and runs.
    await second.getByTestId("clarify-reply").fill("전부");
    await second.getByTestId("clarify-ask-back").click();
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toBeVisible({ timeout: 15_000 });
    await approveIfPending(page, plan);
    await expect(plan.getByTestId("plan-errors")).toHaveCount(0);
    const result = assistantCard(page, "result").last();
    await expect(result).toHaveAttribute("data-result-status", "done", { timeout: 15_000 });
    await expect(result.getByTestId("result-step-0")).toHaveAttribute("data-step-status", "done");
  });

  // CON-12: clearing a required value in the plan editor sent a plan the IPC layer cannot read — a raw English
  // "invalid args … missing field `days`" instead of a field error.
  test("a required value cleared in the plan editor is a field error on the card, not a raw error", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistant(page);
    await send(page, "대화 기록 무기한 보관");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await plan.getByTestId("plan-edit").click();
    const days = plan.getByTestId("plan-editor-param-0-days");
    await expect(days).toHaveValue("-1");
    await days.fill("");
    await expect(days).toHaveAttribute("data-missing", "true");
    await plan.getByTestId("plan-execute").click();
    const errors = plan.getByTestId("plan-errors");
    await expect(errors).toBeVisible();
    await expect(errors).toContainText(ko.planner.editor.required_missing.replace("{{label}}", ""));
    await expect(errors).toContainText("operations[0].days");
    await expect(plan).toHaveAttribute("data-plan-status", "pending");
    await expect(page.getByTestId("toaster")).not.toContainText("missing field");
    await expect(assistantCard(page, "result")).toHaveCount(0);

    // Filling the field clears the error; a day count — or -1 (무기한) again — can be typed and the plan runs.
    await days.fill("30");
    await expect(days).toHaveValue("30");
    await expect(errors).toBeHidden();
    await days.fill("-1");
    await expect(days).toHaveValue("-1");
    await expect(days).not.toHaveAttribute("data-missing", "true");
    await expect(errors).toBeHidden();
    await plan.getByTestId("plan-execute").click();
    await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    const result = assistantCard(page, "result").last();
    await expect(result).toHaveAttribute("data-result-status", "done", { timeout: 15_000 });
    await expect(result.getByTestId("result-step-0")).toHaveAttribute("data-ui-status", "done");
  });

  // KNO-11: the execution card reported a finished job to the page every time it mounted, so the failure
  // toast of an old job came back whenever its chat was opened again.
  test("a failed job is announced once: reopening its chat does not toast again", async ({ page }) => {
    // The mock's engine is off: a plan that needs the model fails as a job without a result.
    await freshApp(page, { "doculens.mock.vision": "engine_off" });
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\견적서-1p.jpg"], { autoLevel: "none" });
    await page.getByTestId(`doc-select-${await docId(page, "견적서-1p.jpg")}`).check();
    await openAssistant(page);
    await page.getByTestId("assistant-scope").click();
    await page.getByTestId("assistant-scope-selection").click();
    await send(page, "선택 문서 요약");
    const execution = assistantCard(page, "execution").last();
    await expect(execution).toHaveAttribute("data-job-state", "failed", { timeout: 15_000 });
    await expect(execution.getByTestId("execution-outcome")).toContainText(ko.errors.engine_not_running);
    const failureToasts = page.getByTestId("toaster").getByRole("status").filter({ hasText: ko.errors.engine_not_running });
    await expect(failureToasts).toHaveCount(1);
    await failureToasts.getByRole("button", { name: "dismiss" }).click();
    await expect(failureToasts).toHaveCount(0);

    // Another chat and back: the card mounts again on the job that already failed.
    const sessions = page.locator('[data-testid="assistant-sessions"] [data-session-id]');
    await page.getByTestId("assistant-new-chat").click();
    await expect(sessions).toHaveCount(2);
    await expect(assistantCard(page, "execution")).toHaveCount(0);
    await sessions.filter({ hasText: "선택 문서 요약" }).locator("button").first().click();
    await expect(assistantCard(page, "execution").last()).toHaveAttribute("data-job-state", "failed", { timeout: 15_000 });
    await expect(assistantCard(page, "execution").last().getByTestId("execution-outcome")).toContainText(ko.errors.engine_not_running);
    await page.waitForTimeout(500);
    await expect(failureToasts).toHaveCount(0);
  });

  test("되묻기 replies with the question's context; 취소 (button or typed) drops pending cards by rule", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    // The interpretation mode moved to 설정 > 일반 > 고급 (plan §3): model first by default.
    await openSettingsTab(page, "general");
    await expect(page.getByTestId("settings-routing")).toContainText(ko.assistant.routing.model_first);
    await openAssistant(page);

    // A sentence the (mock) model cannot read → a clarification card waiting for a reply.
    await send(page, "보라색 코끼리를 춤추게 해줘");
    const clarify = assistantCard(page, "clarify").last();
    await expect(clarify).toBeVisible({ timeout: 15_000 });
    await expect(clarify).toHaveAttribute("data-clarify-status", "pending");
    // 되묻기: the reply becomes the user's turn, the card is marked answered and the composed command
    // (original + question + reply) is interpreted again — here as an aggregate.
    await clarify.getByTestId("clarify-reply").fill("지난달 견적서 합계 말이야");
    await clarify.getByTestId("clarify-ask-back").click();
    await expect(clarify).toHaveAttribute("data-clarify-status", "answered");
    await expect(page.getByTestId("assistant-messages")).toContainText("지난달 견적서 합계 말이야");
    await expect(assistantCard(page, "aggregate").last()).toBeVisible({ timeout: 15_000 });

    // 취소 button on a fresh clarification.
    await send(page, "보라색 코끼리를 춤추게 해줘");
    const second = assistantCard(page, "clarify").last();
    await expect(second).toHaveAttribute("data-clarify-status", "pending", { timeout: 15_000 });
    await second.getByTestId("clarify-cancel").click();
    await expect(second).toHaveAttribute("data-clarify-status", "cancelled");

    // A typed bare "취소" drops a pending plan at once (rule, no model) and says so.
    await send(page, "견적서 월별 폴더로 옮겨");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await send(page, "취소");
    await expect(plan).toHaveAttribute("data-plan-status", "cancelled");
    await expect(assistantCard(page, "executed").last()).toContainText(ko.assistant.cancelled_pending);

    // 되묻기 on a plan card: the reply re-plans with the plan as context; the old card is closed.
    await send(page, "견적서 월별 폴더로 옮겨");
    const plan2 = assistantCard(page, "plan").last();
    await expect(plan2).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await plan2.getByTestId("plan-ask-back-input").fill("검토 안 된 것도 같이 옮겨");
    await plan2.getByTestId("plan-ask-back").click();
    await expect(plan2).toHaveAttribute("data-plan-status", "cancelled");
    await expect(page.locator('[data-testid="assistant-card-plan"]')).toHaveCount(3, { timeout: 15_000 });
    await expect(assistantCard(page, "plan").last()).toHaveAttribute("data-plan-status", "pending");
  });

  test("a pasted text with 다듬어줘 is answered from the text itself: no plan card, no retrieval", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistant(page);
    const body = "금번 과업은 당사가 수행하며 기한은 12월말까지로 하고 산출물은 설계서와 소스코드로 함. 검수는 발주처가 납품 후 2주 내에 완료하는 것으로 한다.";
    await send(page, `다음 문장을 다듬어 정리해줘: ${body}`);
    const card = page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]').last();
    await expect(card).toBeVisible();
    await expect(card).toHaveAttribute("data-answer-state", "done", { timeout: 15_000 });
    await expect(card.getByTestId("answer-text-only")).toBeVisible();
    await expect(card.getByTestId("answer-no-evidence")).toHaveCount(0);
    await expect(card).toContainText("금번 과업은 당사가 수행하며");
    // No library op ran: no plan card and no execution card.
    await expect(page.locator('[data-testid="assistant-card-plan"]')).toHaveCount(0);
  });

  test("slash commands still run at once; an unknown sentence asks for clarification; Esc / Ctrl+Z", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistant(page);

    await send(page, "/테마 다크");
    await expect(assistantCard(page, "executed").last()).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");

    await send(page, "보라색 코끼리를 춤추게 해줘");
    const clarify = assistantCard(page, "clarify").last();
    await expect(clarify).toBeVisible({ timeout: 15_000 });
    await expect(clarify.getByTestId("clarify-question")).not.toBeEmpty();
    // The composer keeps the sentence for a rephrase.
    await expect(page.getByTestId("assistant-input")).toHaveValue("보라색 코끼리를 춤추게 해줘");

    // A file operation waits for approval; Esc outside a text field cancels the active card.
    await send(page, "견적서 월별 폴더로 옮겨");
    const plan = assistantCard(page, "plan").last();
    await expect(plan).toBeVisible({ timeout: 15_000 });
    await expect(plan).toHaveAttribute("data-plan-status", "pending");
    await page.getByTestId("assistant-messages").click();
    await page.keyboard.press("Escape");
    await expect(plan).toHaveAttribute("data-plan-status", "cancelled");
    await expect(plan).toContainText(ko.planner.card.cancelled);
    await expect(plan.getByTestId("plan-execute")).toBeHidden();

    // Ctrl+Z outside a text field: nothing journaled yet → "되돌릴 작업이 없습니다".
    await page.getByTestId("assistant-messages").click();
    await page.keyboard.press("Control+z");
    await expect(page.getByTestId("toaster")).toContainText(ko.palette.nothing_to_undo);
  });
});
