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
async function processReceipt(page: Page) {
  const select = page.getByTestId(`doc-select-${await docId(page, "견적서-1p.jpg")}`);
  await select.check();
  await page.getByTestId("library-process").click();
  await page.getByTestId("library-process-standard").click();
  await expect(card(page, "견적서-1p.jpg")).toHaveAttribute("data-doc-status", "extracted", { timeout: 15_000 });
  await select.uncheck();
}

/** Approves every flagged field of the receipt in the review queue so the file operations may move it. */
async function approveReceipt(page: Page) {
  await page.getByTestId("nav-review").click();
  await expect(page.getByTestId("page-review")).toBeVisible();
  await page.getByTestId("review-approve-doc").click();
  await expect(page.getByTestId("review-empty")).toBeVisible();
  await page.getByTestId("nav-library").click();
  await expect(card(page, "견적서-1p.jpg")).toHaveAttribute("data-doc-status", "reviewed");
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
