import { expect, test, type Page } from "@playwright/test";

import ko from "../src/i18n/ko.json" with { type: "json" };
import { createWorkspace, docId, freshApp, importViaLibrary, tpl } from "./helpers";

// P6 문서 생성 (design §3.13) against the browser mock: template → content → draft job → preview →
// field edit → validator suggestion → validate → save with library registration → saved-draft list,
// and the assistant's `draft_document` plan handing its draft over to the screen. The real model
// call, the HWPX / DOCX / PDF writers and the form fill are covered by the Rust tests.

async function openGenerate(page: Page) {
  await page.getByTestId("nav-generate").click();
  await expect(page.getByTestId("page-generate")).toBeVisible();
}

test.describe("P6 document generation", () => {
  test("estimate: generate → preview → apply the sum suggestion → validate → save → drafts list", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openGenerate(page);
    await expect(page.getByTestId("generate-empty")).toBeVisible();

    // The picker offers the software templates (0.3.0); the generic 0.1 ones are hidden.
    await page.getByTestId("generate-template").click();
    await expect(page.getByTestId("generate-template-estimate")).toBeVisible();
    await expect(page.getByTestId("generate-template-rfp")).toBeVisible();
    await expect(page.locator('[data-testid="generate-template-quotation"]')).toHaveCount(0);
    await page.getByRole("option", { name: ko.generate.templates.estimate }).click();
    await page.getByTestId("generate-content").fill("DocuLens에 백엔드개발 고급 3MM 단가 2,500,000원, 유효기간 30일 견적서");
    await page.getByTestId("generate-run").click();

    // The draft job's progress is announced, then the preview and the editor appear.
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("generate-preview-frame")).toHaveAttribute("data-loaded", "true", { timeout: 10_000 });
    await expect(page.getByTestId("generate-title")).toHaveValue(/견적서/);
    await expect(page.getByTestId("generate-field-input-recipient")).toHaveValue("DocuLens");
    await expect(page.getByTestId("generate-row-items-0")).toBeVisible();
    await expect(page.getByTestId("generate-cell-items-0-role")).toHaveValue("백엔드개발");
    await expect(page.getByTestId("generate-cell-items-0-months")).toHaveValue("3");
    // The mock model forgot the VAT in the grand total: the validator flags the sum and offers 공급가액 + 세액.
    await expect(page.getByTestId("generate-review-badge")).toHaveText(ko.generate.preview.unreviewed);
    await expect(page.getByTestId("generate-field-input-grand_total")).toHaveValue("7,500,000");
    const issue = page.getByTestId("generate-field-grand_total").getByTestId("generate-issue-sum_mismatch");
    await expect(issue).toBeVisible();
    // The issue list names the field by its label, not by the internal path "(fields.grand_total)" (UI-8).
    const totalLabel = ((await page.locator('label[for="gen-field-grand_total"]').textContent()) ?? "").replace("*", "").trim();
    expect(totalLabel.length).toBeGreaterThan(0);
    await expect(page.getByTestId("generate-issue-row").filter({ has: page.locator('[data-testid="generate-issue-where"]', { hasText: `(${totalLabel})` }) })).toHaveCount(1);
    await expect(page.getByTestId("generate-issues")).not.toContainText("fields.grand_total");

    // Editing a field marks it as user-edited.
    await page.getByTestId("generate-field-input-note").fill("부가세 별도");
    await expect(page.getByTestId("generate-field-note")).toContainText(ko.generate.editor.user_edited);

    // 제안 적용 fixes the total and drops the issue; 검증 confirms a clean draft.
    await page.getByTestId("generate-field-grand_total").getByTestId("generate-apply-suggestion").click();
    await expect(page.getByTestId("generate-field-input-grand_total")).toHaveValue("8,250,000");
    await expect(issue).toBeHidden();
    await page.getByTestId("generate-validate").click();
    await expect(page.getByTestId("generate-issues-ok")).toBeVisible({ timeout: 10_000 });
    await expect(page.getByTestId("generate-review-badge")).toHaveText(ko.generate.preview.validated);

    // Save as HWPX with library registration → toast with "라이브러리에서 보기" and the saved path.
    await page.getByTestId("generate-save").click();
    await expect(page.getByTestId("generate-save-dialog")).toBeVisible();
    await expect(page.getByTestId("generate-save-name")).toHaveValue(/^\d{4}-\d{2}-\d{2}_/);
    await page.getByTestId("generate-save-confirm").click();
    await expect(page.getByTestId("generate-saved")).toContainText(/^documents\/generated\/.+\.hwpx/, { timeout: 10_000 });
    await expect(page.getByTestId("toaster")).toContainText(tpl(ko.generate.save.saved, { path: "" }).trim());
    await expect(page.getByTestId("toast-action")).toHaveText(ko.generate.save.open_library);
    await expect(page.getByTestId("generate-saved-library")).toBeVisible();

    // Saving again under the same (default) name is a conflict until "덮어쓰기" is on; the overwrite keeps one file / one document.
    await page.getByTestId("generate-save").click();
    await expect(page.getByTestId("generate-save-dialog")).toBeVisible();
    await page.getByTestId("generate-save-confirm").click();
    await expect(page.getByTestId("generate-save-error")).toHaveText(ko.generate.save.exists);
    await page.getByTestId("generate-save-overwrite").click();
    await page.getByTestId("generate-save-confirm").click();
    await expect(page.getByTestId("generate-save-dialog")).toBeHidden({ timeout: 10_000 });
    await expect(page.getByTestId("generate-saved")).not.toContainText("_2");

    // The saved-draft list shows the draft with its path.
    await page.getByTestId("generate-drafts-open").click();
    const drafts = page.getByTestId("generate-drafts");
    await expect(drafts).toBeVisible();
    await expect(drafts.locator('[data-testid^="generate-draft-row-"]')).toHaveCount(1);
    await expect(drafts).toContainText("documents/generated/");
    await page.keyboard.press("Escape");

    // The registered document is in the library as a generated document (the overwrite re-used it: still one card).
    await page.getByTestId("generate-saved-library").click();
    await expect(page.getByTestId("page-viewer")).toBeVisible();
    await page.getByTestId("nav-library").click();
    await expect(page.locator('[data-testid^="doc-card-"]')).toHaveCount(1);
  });

  test("token budget: the '문서 한 통' preset drafts an RFP section by section", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openGenerate(page);
    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.rfp }).click();
    await page.getByTestId("generate-content").fill("한빛 포털 구축 사업 제안요청서, 사업 예산 250,000,000원, 사업 기간 12개월");
    // The default budget follows the length preset and is drafted in one pass; the cap and context are shown.
    await expect(page.getByTestId("generate-token-budget-hint")).toContainText("한 번에");
    await expect(page.getByTestId("generate-token-budget")).toContainText("8,192");
    await page.getByTestId("generate-token-budget-select").click();
    await page.getByTestId("generate-token-budget-24000").click();
    await expect(page.getByTestId("generate-token-budget-input")).toHaveValue("24000");
    await expect(page.getByTestId("generate-token-budget-hint")).toContainText("절별");
    // A number typed into the box is applied when the box is left or on Enter, never per keystroke (GEN-12:
    // "6000" used to end as 32768 because "6" was raised to 200 at once).
    const budget = page.getByTestId("generate-token-budget-input");
    await budget.click();
    await page.keyboard.press("Control+a");
    await page.keyboard.press("Delete");
    await page.keyboard.type("6000");
    await expect(budget).toHaveValue("6000");
    await expect(page.getByTestId("generate-token-budget-hint")).toContainText("6,000");
    await page.keyboard.press("Enter");
    await expect(budget).toHaveValue("6000");
    await expect(page.getByTestId("generate-token-budget-select")).toContainText("6,000");
    // Out-of-range numbers are clamped once, on blur: below 200 → 200, above the cap → the cap.
    await budget.fill("50");
    await page.getByTestId("generate-content").focus();
    await expect(budget).toHaveValue("200");
    await budget.fill("99999");
    await page.getByTestId("generate-content").focus();
    await expect(budget).toHaveValue("32768");
    // Emptying the box goes back to the length preset; the "문서 한 통" preset is picked again for the run below.
    await budget.fill("");
    await page.getByTestId("generate-content").focus();
    await expect(page.getByTestId("generate-token-budget-hint")).toContainText("한 번에");
    await page.getByTestId("generate-token-budget-select").click();
    await page.getByTestId("generate-token-budget-24000").click();
    await expect(budget).toHaveValue("24000");
    await page.getByTestId("generate-run").click();
    // Section progress is reported while the job runs, then every template section is in the draft.
    await expect(page.getByTestId("generate-progress")).toContainText("절 작성 중", { timeout: 15_000 });
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 30_000 });
    await expect(page.getByTestId("generate-notes")).toContainText("8개 절");
    await expect(page.locator('[data-testid^="generate-block-input-"].font-semibold')).toHaveCount(8);
    await expect(page.getByTestId("generate-field-input-project_name")).toHaveValue(/한빛 포털 구축/);
  });

  test("regeneration: feedback and an excluded quoted document are applied to the next draft", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\개발계약서-2p.pdf"], { autoLevel: "none" });
    const invoice = await docId(page, "개발계약서-2p.pdf");
    await openGenerate(page);

    // Quote the imported document: it shows up as a reference of the draft.
    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.progress_report }).click();
    await page.getByTestId("generate-content").fill("한빛 포털 구축 10월 진척 보고서, 전체 진척률 60%");
    await page.getByTestId("generate-pick-sources").click();
    await expect(page.getByTestId("generate-sources-dialog")).toBeVisible();
    await page.getByTestId(`generate-source-${invoice}`).check();
    await page.getByTestId("generate-sources-done").click();
    await expect(page.getByTestId("generate-sources").locator("li")).toHaveCount(1);
    await page.getByTestId("generate-run").click();
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 15_000 });
    const references = page.getByTestId("generate-references").getByTestId("generate-reference");
    await expect(references).toHaveCount(1);
    await expect(references.first()).toHaveAttribute("data-excluded", "false");
    await expect(references.first()).toContainText("개발계약서-2p.pdf");

    // Exclude the reference, instruct the 초안 어시스턴트 on the whole draft (0.7.0): the new draft drops the
    // document, records the instruction and the revision log keeps it.
    await page.getByTestId("generate-reference-exclude").first().check();
    await expect(references.first()).toHaveAttribute("data-excluded", "true");
    await expect(page.getByTestId("generate-revise-panel")).toContainText(tpl(ko.generate.form.excluded_hint, { count: 1 }));
    await expect(page.getByTestId("generate-revise-log")).toContainText(ko.generate.revise.log_empty);
    await expect(page.getByTestId("generate-revise-run")).toBeDisabled();
    await page.getByTestId("generate-revise-instruction").fill("더 짧게");
    await page.getByTestId("generate-revise-run").click();
    await expect(page.getByTestId("generate-notes")).toContainText("재생성 반영: 더 짧게", { timeout: 15_000 });
    await expect(references).toHaveCount(0);
    await expect(page.getByTestId("generate-references")).toContainText(ko.generate.editor.no_references);
    await expect(page.getByTestId("generate-revise-log").getByTestId("generate-revise-log-entry")).toHaveCount(1);
    await expect(page.getByTestId("generate-revise-log")).toContainText("더 짧게");
    await expect(page.getByTestId("generate-revise-log")).toContainText(tpl(ko.generate.revise.done, { scope: "전체" }));
    // The instruction box is cleared for the next round; the editor is usable again.
    await expect(page.getByTestId("generate-revise-instruction")).toHaveValue("");
    await expect(page.getByTestId("generate-editor")).toHaveAttribute("data-disabled", "false");
  });

  test("body blocks and the 초안 어시스턴트 (0.7.0): add / move / remove a paragraph, rewrite one section, add a new section", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openGenerate(page);
    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.meeting_minutes }).click();
    await page.getByTestId("generate-content").fill("한빛 포털 구축 주간 회의록, 일정 점검과 리스크 논의");
    await page.getByTestId("generate-run").click();
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 15_000 });
    const headings = page.locator('[data-testid^="generate-block-"][data-block-kind="heading"]');
    expect(await headings.count()).toBeGreaterThanOrEqual(2);
    const frame = page.getByTestId("generate-preview-frame");
    const srcdoc = async () => (await frame.getAttribute("srcdoc")) ?? "";

    // Add a paragraph after the first heading: it is focused, typed into, and the preview follows.
    const inputs = page.locator('[data-testid^="generate-block-input-"]');
    const before = await inputs.count();
    await page.getByTestId("draft-block-add-paragraph-0").click();
    await expect(inputs).toHaveCount(before + 1);
    await expect(page.getByTestId("generate-block-input-1")).toBeFocused();
    await expect(page.getByTestId("generate-block-1")).toHaveAttribute("data-block-kind", "paragraph");
    await page.getByTestId("generate-block-input-1").fill("새로 넣은 문단입니다.");
    await expect.poll(srcdoc, { timeout: 10_000 }).toContain("새로 넣은 문단입니다.");
    // Move it one down, then remove it: the block count and the preview go back.
    await page.getByTestId("draft-block-down-1").click();
    await expect(page.getByTestId("generate-block-input-2")).toHaveValue("새로 넣은 문단입니다.");
    await page.getByTestId("draft-block-remove-2").click();
    await expect(inputs).toHaveCount(before);
    await expect.poll(srcdoc, { timeout: 10_000 }).not.toContain("새로 넣은 문단입니다.");
    // An edit is persisted to the draft store (debounced) — the mock keeps drafts in localStorage.
    await page.getByTestId("generate-block-input-1").fill("유지되는 문단입니다.");
    await expect.poll(() => page.evaluate(() => Object.keys(localStorage).some((k) => (localStorage.getItem(k) ?? "").includes("유지되는 문단입니다."))), { timeout: 10_000 }).toBe(true);

    // The per-section "AI 수정" button picks that section and focuses the instruction box.
    const second = Number((await headings.nth(1).getAttribute("data-testid"))!.replace("generate-block-", ""));
    const heading2 = await page.getByTestId(`generate-block-input-${second}`).inputValue();
    await page.getByTestId(`draft-section-revise-${second}`).click();
    await expect(page.getByTestId("generate-revise-instruction")).toBeFocused();
    await expect(page.getByTestId("generate-revise-scope")).toContainText(`2. ${heading2}`);
    // Rewrite section 2: its heading stays, its body quotes the instruction, the note and the log record it.
    await page.getByTestId("generate-revise-instruction").fill("더 자세히");
    await page.getByTestId("generate-revise-run").click();
    await expect(page.getByTestId("generate-notes")).toContainText(`AI 수정(2절 '${heading2}'): 더 자세히`, { timeout: 15_000 });
    await expect(page.getByTestId(`generate-block-input-${second}`)).toHaveValue(heading2);
    await expect(page.getByTestId(`generate-block-input-${second + 1}`)).toHaveValue(/지시 반영: 더 자세히/);
    await expect(page.getByTestId("generate-revise-log")).toContainText(tpl(ko.generate.revise.done, { scope: `2. ${heading2}` }));
    await expect(page.getByTestId("generate-revise-instruction")).toHaveValue("");
    await expect(page.getByTestId("generate-editor")).toHaveAttribute("data-disabled", "false");
    // Add a new section at the end with its own heading.
    await page.getByTestId("generate-revise-scope").click();
    await page.getByTestId("generate-revise-scope-append").click();
    await page.getByTestId("generate-revise-heading").fill("결론");
    await page.getByTestId("generate-revise-instruction").fill("기대 효과를 정리");
    await page.getByTestId("generate-revise-run").click();
    await expect(page.getByTestId("generate-notes")).toContainText("AI 수정(새 절 '결론' — 끝): 기대 효과를 정리", { timeout: 15_000 });
    await expect(page.locator('[data-testid^="generate-block-input-"].font-semibold').last()).toHaveValue("결론");
    await expect(page.getByTestId("generate-revise-log").getByTestId("generate-revise-log-entry")).toHaveCount(2);
    // The preview is scaled to its column ("맞춤"); "100%" shows the sheet at its real size.
    await expect(page.getByTestId("generate-preview-zoom")).toHaveAttribute("data-zoom", "fit");
    await page.getByTestId("generate-preview-zoom").click();
    await expect(page.getByTestId("generate-preview-zoom")).toHaveAttribute("data-zoom", "full");
    await expect(page.getByTestId("generate-preview-box")).toHaveAttribute("data-scale", "1.00");
  });

  test("a list block takes what is typed: Enter and spaces stay, the box is cleaned on blur (GEN-5)", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openGenerate(page);
    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.meeting_minutes }).click();
    await page.getByTestId("generate-content").fill("한빛 포털 구축 주간 회의록, 일정 점검과 리스크 논의");
    await page.getByTestId("generate-run").click();
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 15_000 });

    // "목록 추가" after the first heading: the new, empty list block is focused.
    await page.getByTestId("draft-block-add-list-0").click();
    const list = page.getByTestId("generate-block-input-1");
    await expect(page.getByTestId("generate-block-1")).toHaveAttribute("data-block-kind", "list");
    await expect(list).toBeFocused();
    // Typed key by key — the audit's reproduction ended as "첫째항목둘째항목".
    await page.keyboard.type("첫째 항목");
    await page.keyboard.press("Enter");
    await page.keyboard.type("둘째 항목");
    await expect(list).toHaveValue("첫째 항목\n둘째 항목");
    // The draft holds the two items: the preview lists both.
    const srcdoc = async () => (await page.getByTestId("generate-preview-frame").getAttribute("srcdoc")) ?? "";
    await expect.poll(srcdoc, { timeout: 10_000 }).toContain("<li>첫째 항목</li><li>둘째 항목</li>");
    // The empty line after another Enter stays while typing and is cleaned when the box loses focus.
    await page.keyboard.press("Enter");
    await expect(list).toHaveValue("첫째 항목\n둘째 항목\n");
    await page.getByTestId("generate-title").focus();
    await expect(list).toHaveValue("첫째 항목\n둘째 항목");
  });

  test("the revise scope follows its section across edits and is cleared — never turned into 전체 다시 쓰기 — when the section is removed (GEN-10)", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openGenerate(page);
    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.meeting_minutes }).click();
    await page.getByTestId("generate-content").fill("한빛 포털 구축 주간 회의록, 일정 점검과 리스크 논의");
    await page.getByTestId("generate-run").click();
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 15_000 });
    const headings = page.locator('[data-testid^="generate-block-"][data-block-kind="heading"]');
    const count = await headings.count();
    expect(count).toBeGreaterThanOrEqual(2);

    // Pick the last section with its "AI 수정" button.
    const last = Number((await headings.last().getAttribute("data-testid"))!.replace("generate-block-", ""));
    const lastHeading = await page.getByTestId(`generate-block-input-${last}`).inputValue();
    await page.getByTestId(`draft-section-revise-${last}`).click();
    const scope = page.getByTestId("generate-revise-scope");
    await expect(scope).toContainText(lastHeading);
    const ordinal = Number(((await scope.textContent()) ?? "").match(/(\d+)\.\s/)?.[1]);
    expect(ordinal).toBeGreaterThanOrEqual(2);
    await expect(scope).toHaveAttribute("data-scope", `section:${ordinal}`);
    // A new section heading inserted above shifts the ordinals: the scope still names the picked section.
    await page.getByTestId("draft-block-add-heading-0").click();
    await page.getByTestId("generate-block-input-1").fill("새 절");
    await expect(scope).toHaveAttribute("data-scope", `section:${ordinal + 1}`);
    await expect(scope).toContainText(`${ordinal + 1}. ${lastHeading}`);
    // Its heading block removed (the section is merged into the one before): the scope is cleared with a notice.
    await page.getByTestId(`draft-block-remove-${last + 1}`).click();
    await expect(scope).toHaveAttribute("data-scope", "none");
    await expect(scope).toContainText(ko.generate.revise.scope_none);
    await expect(page.getByTestId("generate-revise-scope-lost")).toHaveText(ko.generate.revise.scope_lost);
    await expect(page.getByTestId("toaster")).toContainText(ko.generate.revise.scope_lost);
    // Nothing runs without a scope; picking one again enables the button.
    await page.getByTestId("generate-revise-instruction").fill("더 짧게");
    await expect(page.getByTestId("generate-revise-run")).toBeDisabled();
    await scope.click();
    await page.getByTestId("generate-revise-scope-section-1").click();
    await expect(page.getByTestId("generate-revise-run")).toBeEnabled();
    await expect(page.getByTestId("generate-revise-scope-lost")).toHaveCount(0);
  });

  test("the form's selects show their values in full; a narrow window gives the editor a pane of its own (UI-5)", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openGenerate(page);
    /** The value of a select is neither wrapped into clamped lines ("격…") nor cut sideways. */
    const shownInFull = async (testId: string, text: string) => {
      const value = page.getByTestId(testId).locator("span").first();
      await expect(value).toHaveText(text);
      expect(await value.evaluate((el) => el.scrollHeight <= el.clientHeight + 1 && el.scrollWidth <= el.clientWidth + 1), `${testId} shows "${text}" in full`).toBe(true);
    };
    const checkSelects = async () => {
      await shownInFull("generate-tone", ko.generate.tone.formal);
      await shownInFull("generate-length", ko.generate.length.medium);
      await shownInFull("generate-output", ko.generate.output.hwpx);
      // The "토큰 예산" label stays on one line.
      const label = (await page.locator('label[for="gen-budget"]').boundingBox())!;
      expect(label.height, "토큰 예산 is one line").toBeLessThan(24);
    };

    // 1280 px at 100 %: three columns.
    await expect(page.getByTestId("generate-layout")).toHaveAttribute("data-layout", "wide");
    await checkSelects();
    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.estimate }).click();
    await page.getByTestId("generate-content").fill("DocuLens에 백엔드개발 고급 3MM 단가 2,500,000원, 유효기간 30일 견적서");
    await page.getByTestId("generate-run").click();
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("generate-pane-switch")).toHaveCount(0);
    await expect(page.getByTestId("generate-editor")).toBeVisible();
    await expect(page.getByTestId("generate-revise-panel")).toHaveAttribute("data-placement", "bottom");

    // A 1000 px window (like 1280 px at about 130 %): the form column beside one pane. The preview shows first;
    // "편집" gives the editor the whole height instead of two fifths of it.
    await page.setViewportSize({ width: 1000, height: 800 });
    const layout = page.getByTestId("generate-layout");
    await expect(layout).toHaveAttribute("data-layout", "narrow");
    await checkSelects();
    await expect(page.getByTestId("generate-preview-frame")).toBeVisible();
    await expect(page.getByTestId("generate-editor")).toBeHidden();
    // The 초안 어시스턴트 is the first thing in the form column: in view without scrolling.
    await expect(page.getByTestId("generate-revise-panel")).toHaveAttribute("data-placement", "top");
    await expect(page.getByTestId("generate-revise-instruction")).toBeInViewport();
    await page.getByTestId("generate-pane-editor").click();
    await expect(page.getByTestId("generate-editor")).toBeVisible();
    await expect(page.getByTestId("generate-preview-frame")).toBeHidden();
    const whole = (await layout.boundingBox())!;
    const editor = (await page.getByTestId("generate-pane-editor-box").boundingBox())!;
    expect(editor.height, "the editor pane takes (almost) the whole height").toBeGreaterThan(whole.height * 0.85);
    // Editing works in the pane, and the preview follows when it is shown again.
    await page.getByTestId("generate-title").fill("견적서 (좁은 창)");
    await page.getByTestId("generate-pane-preview").click();
    await expect(page.getByTestId("generate-preview-frame")).toBeVisible();
    await expect.poll(async () => (await page.getByTestId("generate-preview-frame").getAttribute("srcdoc")) ?? "", { timeout: 10_000 }).toContain("견적서 (좁은 창)");
    // Nothing scrolls sideways and the page itself does not scroll.
    const main = page.locator("main#main");
    expect(await main.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
    expect(await main.evaluate((el) => el.scrollHeight - el.clientHeight)).toBeLessThanOrEqual(1);

    // Back to the wide window: both panes side by side again.
    await page.setViewportSize({ width: 1280, height: 800 });
    await expect(layout).toHaveAttribute("data-layout", "wide");
    await expect(page.getByTestId("generate-editor")).toBeVisible();
    await expect(page.getByTestId("generate-preview-frame")).toBeVisible();
  });

  test("saving as Markdown registers the file in the library like the other formats (GEN-9)", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openGenerate(page);
    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.meeting_minutes }).click();
    await page.getByTestId("generate-content").fill("한빛 포털 구축 주간 회의록, 일정 점검과 리스크 논의");
    await page.getByTestId("generate-run").click();
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 15_000 });

    await page.getByTestId("generate-save").click();
    await page.getByTestId("generate-save-format").click();
    await page.getByRole("option", { name: ko.generate.output.md }).click();
    // The hint says a Markdown file is registered too: the switch stays on and usable (it was forced off).
    await expect(page.getByTestId("generate-save-dialog")).toContainText(ko.generate.save.warning_markdown);
    await expect(page.getByTestId("generate-save-register")).toBeEnabled();
    await expect(page.getByTestId("generate-save-register")).toHaveAttribute("aria-checked", "true");
    await page.getByTestId("generate-save-confirm").click();
    await expect(page.getByTestId("generate-saved")).toContainText(/^documents\/generated\/.+\.md/, { timeout: 10_000 });
    await expect(page.getByTestId("toast-action")).toHaveText(ko.generate.save.open_library);
    await expect(page.getByTestId("generate-saved-library")).toBeVisible();
    await page.getByTestId("nav-library").click();
    await expect(page.locator('[data-testid^="doc-card-"]')).toHaveCount(1);
    await expect(page.locator('[data-testid^="doc-card-"]').first()).toHaveAttribute("data-doc-name", /\.md$/);
  });

  test("the 문서 선택 dialog keeps its ticks and search while a job reports progress behind it (GEN-6)", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\개발계약서-2p.pdf", "D:\\scan\\견적서-1p.jpg"], { autoLevel: "none" });
    const contract = await docId(page, "개발계약서-2p.pdf");
    await openGenerate(page);
    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.progress_report }).click();
    await page.getByTestId("generate-content").fill("한빛 포털 구축 10월 진척 보고서, 전체 진척률 60%");
    // At the mock's real pace the draft job reports progress for more than a second: every report re-renders the form.
    await page.evaluate(() => localStorage.removeItem("doculens.mock.fast"));
    await page.getByTestId("generate-run").click();
    await expect(page.getByTestId("generate-progress")).toBeVisible();
    await page.getByTestId("generate-pick-sources").click();
    await expect(page.getByTestId("generate-sources-dialog")).toBeVisible();
    await page.getByTestId("generate-sources-search").fill("계약");
    await page.getByTestId(`generate-source-${contract}`).check();
    // The job finishes behind the dialog; the search text and the tick are still there.
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 20_000 });
    await expect(page.getByTestId("generate-sources-search")).toHaveValue("계약");
    await expect(page.getByTestId(`generate-source-${contract}`)).toBeChecked();
    await page.getByTestId("generate-sources-done").click();
    await expect(page.getByTestId("generate-sources").locator("li")).toHaveCount(1);
  });

  test("assistant: '2절 다시 써줘 …' revises the open draft (plan → result card → the generate screen); without a draft it asks to open one", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    // No draft open: the command is a clarification, not a plan.
    await page.getByTestId("nav-assistant").click();
    await page.getByTestId("assistant-input").fill("2절 다시 써줘");
    await page.getByTestId("assistant-send").click();
    await expect(page.locator('[data-testid="assistant-card-clarify"]').last()).toContainText("초안을 먼저", { timeout: 15_000 });

    await openGenerate(page);
    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.progress_report }).click();
    await page.getByTestId("generate-content").fill("한빛 포털 구축 10월 진척 보고서, 전체 진척률 60%");
    await page.getByTestId("generate-run").click();
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 15_000 });
    const heading2 = await page.locator('[data-testid^="generate-block-"][data-block-kind="heading"]').nth(1).locator("textarea").inputValue();

    await page.getByTestId("nav-assistant").click();
    await page.getByTestId("assistant-input").fill("2절 다시 써줘 — 리스크를 더 자세히");
    await page.getByTestId("assistant-send").click();
    const plan = page.locator('[data-testid="assistant-card-plan"]').last();
    await expect(plan).toBeVisible({ timeout: 15_000 });
    await expect(plan).toContainText(ko.planner.ops.revise_draft);
    await expect(plan).toContainText(tpl(ko.planner.params.revise_section, { index: 2 }));
    await expect(plan).toContainText("리스크를 더 자세히");
    // No confirmation needed (the revision is previewed on the generate screen): the plan runs at once.
    await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    const result = page.locator('[data-testid="assistant-card-result"]').last();
    await expect(result).toBeVisible({ timeout: 20_000 });
    await expect(result).toHaveAttribute("data-result-status", "done");
    await result.getByTestId("result-open-draft").click();
    await expect(page.getByTestId("page-generate")).toBeVisible();
    await expect(page.getByTestId("generate-notes")).toContainText(`AI 수정(2절 '${heading2}'): 리스크를 더 자세히`);
  });

  test("문서 작성 화면에서 열기 on the jobs page opens the saved draft, not the job's first snapshot (GEN-4)", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openGenerate(page);
    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.estimate }).click();
    await page.getByTestId("generate-content").fill("DocuLens에 백엔드개발 고급 3MM 단가 2,500,000원, 유효기간 30일 견적서");
    await page.getByTestId("generate-run").click();
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("generate-field-input-note")).toHaveValue("");

    // Edit the draft and leave at once: the edit is written when the screen is left, not dropped.
    await page.getByTestId("generate-field-input-note").fill("부가세 별도");
    await page.getByTestId("nav-jobs").click();
    await expect.poll(() => page.evaluate(() => (localStorage.getItem("doculens.mock.drafts") ?? "").includes("부가세 별도")), { timeout: 10_000 }).toBe(true);

    // The job's result is the draft as it was generated (no note). Opening it shows the saved draft with the edit.
    await page.getByTestId("jobs-tab-finished").click();
    const job = page.locator('[data-job-kind="draft"]').first();
    await job.getByTestId("job-result").locator("summary").click();
    await job.getByTestId("job-open-draft").click();
    await expect(page.getByTestId("page-generate")).toBeVisible();
    await expect(page.getByTestId("generate-field-input-note")).toHaveValue("부가세 별도");
    await expect(page.getByTestId("toaster")).toContainText(ko.generate.reloaded);
    // The next keystroke is saved on top of the saved draft: the earlier edit survives.
    await page.getByTestId("generate-title").fill("견적서 (최종)");
    await expect
      .poll(
        () =>
          page.evaluate(() => {
            const all = JSON.parse(localStorage.getItem("doculens.mock.drafts") ?? "{}") as Record<string, { title: string; fields: { key: string; value: string }[] }[]>;
            const d = Object.values(all).flat()[0];
            return `${d?.title} / ${d?.fields.find((f) => f.key === "note")?.value}`;
          }),
        { timeout: 10_000 },
      )
      .toBe("견적서 (최종) / 부가세 별도");
  });

  test("an edit made just before leaving is in the assistant's revision, and the revision shows when coming back by the sidebar (GEN-7)", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openGenerate(page);
    await page.getByTestId("generate-template").click();
    await page.getByRole("option", { name: ko.generate.templates.progress_report }).click();
    await page.getByTestId("generate-content").fill("한빛 포털 구축 10월 진척 보고서, 전체 진척률 60%");
    await page.getByTestId("generate-run").click();
    await expect(page.getByTestId("generate-preview")).toBeVisible({ timeout: 15_000 });
    const heading2 = await page.locator('[data-testid^="generate-block-"][data-block-kind="heading"]').nth(1).locator("textarea").inputValue();

    // An edit, then straight to the assistant — inside the autosave pause.
    await page.getByTestId("generate-title").fill("10월 진척 보고서 (수정)");
    await page.getByTestId("nav-assistant").click();
    await page.getByTestId("assistant-input").fill("2절 다시 써줘 — 리스크를 더 자세히");
    await page.getByTestId("assistant-send").click();
    const plan = page.locator('[data-testid="assistant-card-plan"]').last();
    await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    await expect(page.locator('[data-testid="assistant-card-result"]').last()).toHaveAttribute("data-result-status", "done", { timeout: 20_000 });

    // Back by the sidebar, not by the result card: the screen shows the revised draft, with the edit in it.
    await page.getByTestId("nav-generate").click();
    await expect(page.getByTestId("generate-notes")).toContainText(`AI 수정(2절 '${heading2}'): 리스크를 더 자세히`);
    await expect(page.getByTestId("generate-title")).toHaveValue("10월 진척 보고서 (수정)");
    await expect(page.getByTestId("toaster")).toContainText(ko.generate.reloaded);
    await expect(page.getByTestId("generate-editor")).toHaveAttribute("data-disabled", "false");
    // The next edit is saved on top of the revision instead of overwriting it with the old draft.
    await page.getByTestId("generate-title").fill("10월 진척 보고서 (최종)");
    await expect
      .poll(
        () =>
          page.evaluate(() => {
            const all = JSON.parse(localStorage.getItem("doculens.mock.drafts") ?? "{}") as Record<string, { title: string; notes: string[] }[]>;
            const d = Object.values(all).flat()[0];
            return `${d?.title} / ${d?.notes.some((n) => n.includes("AI 수정")) ? "revised" : "not revised"}`;
          }),
        { timeout: 10_000 },
      )
      .toBe("10월 진척 보고서 (최종) / revised");
  });

  test("assistant: '견적서 만들어줘 …' → plan card → draft job → result card opens the draft here", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-assistant").click();
    await page.getByTestId("assistant-input").fill("㈜두리에 모니터 3대 대당 350,000원 견적서 만들어줘");
    await page.getByTestId("assistant-send").click();
    const plan = page.locator('[data-testid="assistant-card-plan"]').last();
    await expect(plan).toBeVisible({ timeout: 15_000 });
    await expect(plan).toHaveAttribute("data-plan-status", "pending");
    await expect(plan).toContainText(ko.planner.ops.draft_document);
    await expect(plan.getByTestId("plan-impact")).toContainText(tpl(ko.planner.card.impact_draft, { count: 1 }));
    await plan.getByTestId("plan-execute").click();
    await expect(plan).toHaveAttribute("data-plan-status", "done", { timeout: 15_000 });
    const result = page.locator('[data-testid="assistant-card-result"]').last();
    await expect(result).toBeVisible({ timeout: 20_000 });
    await expect(result).toHaveAttribute("data-result-status", "done");
    await expect(result.getByTestId("result-draft")).toContainText("견적서");
    await result.getByTestId("result-open-draft").click();
    await expect(page.getByTestId("page-generate")).toBeVisible();
    await expect(page.getByTestId("toaster")).toContainText(ko.generate.handoff);
    await expect(page.getByTestId("generate-title")).toHaveValue(/견적서/);
    await expect(page.getByTestId("generate-field-input-recipient")).toHaveValue("㈜두리");
    await expect(page.getByTestId("generate-cell-items-0-role")).toHaveValue("모니터");

    // The Jobs page lists the draft job with its result and a way back to the screen.
    await page.getByTestId("nav-jobs").click();
    await page.getByTestId("jobs-tab-finished").click();
    const job = page.locator('[data-job-kind="plan"]').first();
    await expect(job).toContainText(ko.jobs.kind.plan);
    await job.getByTestId("job-result").locator("summary").click();
    await expect(job.getByTestId("job-result-steps")).toContainText(ko.planner.ops.draft_document);
  });
});
