import { expect, test, type Locator, type Page } from "@playwright/test";

import en from "../src/i18n/en.json" with { type: "json" };
import ko from "../src/i18n/ko.json" with { type: "json" };
import { card, createWorkspace, docId, freshApp, importViaLibrary, openImportPreview, openModelsAdvanced, openSettingsTab, tpl } from "./helpers";

// Layout guards for the 0.7.0 audit (docs/audit/v0.7.0.md): the window itself never scrolls on any screen (UI-1),
// every dialog stays inside the window with its buttons in reach at UI 배율 150 % and in the minimum window (UI-3),
// the 문서함 list fits its box and its filters say what they filter (UI-6, UI-7), settings switches have names
// (UI-9), toasts keep off the buttons people press next (UI-11), particles follow the word (UI-13), the English
// UI shows no Korean model descriptors (UI-14), cut-off texts carry a tooltip (UI-16) and the 유형 → 추출 항목
// mapping table has two columns (USR-5). `scale.spec.ts` covers clipping inside cards, rows and the bars.

type Scale = 90 | 100 | 110 | 120 | 130 | 140 | 150;

/** Sets the UI 배율 with the keyboard on 설정 > 일반 (Home = 90 %, one step = 10 %) and waits for the root font size. */
async function setScale(page: Page, percent: Scale): Promise<void> {
  await openSettingsTab(page, "general");
  await page.getByTestId("scale-slider").getByRole("slider").focus();
  await page.keyboard.press("Home");
  for (let p = 90; p < percent; p += 10) await page.keyboard.press("ArrowRight");
  await expect(page.getByTestId("scale-value")).toHaveText(`${percent}%`);
  await expect.poll(() => page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).fontSize))).toBe((16 * percent) / 100);
}

async function send(page: Page, text: string): Promise<void> {
  await page.getByTestId("assistant-input").fill(text);
  await page.getByTestId("assistant-send").click();
}

/** Toasts of the steps before would sit over what is measured next: close them. */
async function dismissToasts(page: Page): Promise<void> {
  const buttons = page.getByTestId("toaster").locator('button[aria-label="dismiss"]');
  for (let n = await buttons.count(); n > 0; n = await buttons.count()) await buttons.first().click({ timeout: 2_000 }).catch(() => undefined);
}

interface WindowReport {
  window: [number, number];
  root: [number, number];
  /** Scroll offsets of the viewport, <body> and #root, and the app shell's top-left corner: all zero when nothing moved. */
  moved: number[];
  /** Visually hidden absolute elements that are not held by the box that scrolls them (see `fixedWindow`). */
  escaping: string[];
}

/** Measured in the page; `expectWindowFixed` asserts on it. */
function fixedWindow(): WindowReport {
  const describe = (el: Element | null): string => {
    if (!el) return "null";
    const id = el.getAttribute("data-testid");
    return `${el.tagName.toLowerCase()}${id ? `[${id}]` : ""}.${String(el.getAttribute("class") ?? "").split(/\s+/).slice(0, 3).join(".")}`;
  };
  const clips = (el: Element) => {
    const cs = getComputedStyle(el);
    return cs.overflowX !== "visible" || cs.overflowY !== "visible";
  };
  const scrolls = (el: Element) => {
    const cs = getComputedStyle(el);
    return [cs.overflowX, cs.overflowY].some((v) => v === "auto" || v === "scroll" || v === "hidden");
  };
  const rootLevel = (el: Element) => el === document.documentElement || el === document.body || el.id === "root";
  const escaping: string[] = [];
  for (const el of Array.from(document.querySelectorAll<HTMLElement>("body *"))) {
    const cs = getComputedStyle(el);
    if (cs.position !== "absolute") continue;
    const srOnly = el.classList.contains("sr-only");
    // A library's own hidden control (Radix bubble input / select, cmdk label): a 1 px box clipped away.
    const hiddenControl = (cs.clip === "rect(0px, 0px, 0px, 0px)" || cs.clipPath === "inset(50%)") && parseFloat(cs.width) <= 1 && parseFloat(cs.height) <= 1;
    if (!srOnly && !hiddenControl) continue;
    // The box whose scrolling moves the element's place …
    let box = el.parentElement;
    while (box && !rootLevel(box) && !clips(box)) box = box.parentElement;
    if (!box || rootLevel(box)) continue;
    // … must hold the element: the element's containing block is that box or lies inside it.
    const containing = el.offsetParent;
    if (containing && (containing === box || box.contains(containing))) continue;
    if (!srOnly) {
      // Not ours to position; harmless as long as no scroll box above its containing block can be widened by it.
      let above: Element | null = containing;
      while (above && (rootLevel(above) || !scrolls(above))) above = rootLevel(above) ? null : above.parentElement;
      if (!above) continue;
    }
    escaping.push(`${describe(el)} "${(el.textContent ?? "").trim().slice(0, 24)}" scrolls with ${describe(box)} but is positioned against ${describe(containing)}`);
  }
  const shell = document.querySelector('[data-testid="app-shell"], [data-testid="onboarding"]')?.getBoundingClientRect();
  const root = document.getElementById("root");
  return {
    window: [window.innerWidth, window.innerHeight],
    root: [document.documentElement.scrollWidth, document.documentElement.scrollHeight],
    moved: [window.scrollX, window.scrollY, document.documentElement.scrollTop, document.body.scrollTop, document.body.scrollLeft, root?.scrollTop ?? 0, root?.scrollLeft ?? 0, shell?.left ?? 0, shell?.top ?? 0],
    escaping,
  };
}

/**
 * UI-1: the document is exactly as large as the window, a wheel turn over `over` (default: the middle of the
 * window) scrolls nothing at window level, and every `sr-only` element is held by the box that scrolls it.
 */
async function expectWindowFixed(page: Page, screen: string, over?: Locator): Promise<void> {
  const size = page.viewportSize()!;
  let x = size.width / 2;
  let y = size.height / 2;
  const target = over ? await over.boundingBox() : null;
  if (target) {
    x = target.x + target.width / 2;
    y = target.y + target.height / 2;
  }
  await page.mouse.move(x, y);
  await page.mouse.wheel(0, 900);
  await page.mouse.wheel(600, 0);
  // The wheel is applied asynchronously: give a window scroll the chance to show up.
  await page.waitForTimeout(150);
  const r = await page.evaluate(fixedWindow);
  // Soft: the tour goes on, so a failure lists every screen that is affected and not only the first one.
  expect.soft(r.root[0], `${screen}: document wider than the window`).toBeLessThanOrEqual(r.window[0]);
  expect.soft(r.root[1], `${screen}: document taller than the window`).toBeLessThanOrEqual(r.window[1]);
  expect.soft(r.moved, `${screen}: the window / body / #root scrolled or the shell moved after a wheel turn`).toEqual([0, 0, 0, 0, 0, 0, 0, 0, 0]);
  expect.soft(r.escaping, `${screen}: hidden elements outside the box that scrolls them`).toEqual([]);
  // Put a scrolled window back so the next screen is measured on its own.
  if (r.moved.some((v) => v !== 0)) {
    await page.evaluate(() => {
      window.scrollTo(0, 0);
      document.body.scrollTop = 0;
      const root = document.getElementById("root");
      if (root) root.scrollTop = 0;
    });
  }
}

/** `main` has nothing to scroll on a screen that scrolls inside its own boxes. */
async function expectMainNotScrollable(page: Page, screen: string): Promise<void> {
  const overflow = await page.locator("main#main").evaluate((el) => [el.scrollWidth - el.clientWidth, el.scrollHeight - el.clientHeight]);
  expect.soft(overflow[0], `${screen}: main scrolls sideways`).toBeLessThanOrEqual(1);
  expect.soft(overflow[1], `${screen}: main scrolls vertically`).toBeLessThanOrEqual(1);
}

interface DocIds {
  estimate: string;
  contract: string;
  srs: string;
  wbs: string;
  test: string;
  long: string;
}

/** Six documents (five extracted): what the screens and dialogs below need. */
async function importDocuments(page: Page): Promise<DocIds> {
  const names = ["견적서-1p.jpg", "개발계약서-2p.pdf", "요구사항정의서-v1.2.pdf", "WBS-한빛포털.xlsx", "테스트결과서-1.2.pdf", "계약서-12p.pdf"];
  await importViaLibrary(page, names.map((n) => `D:\\scan\\${n}`), { autoLevel: "none" });
  const ids = await Promise.all(names.map((n) => docId(page, n)));
  for (const id of ids.slice(0, 5)) await page.getByTestId(`doc-select-${id}`).check();
  await page.getByTestId("library-process").click();
  await page.getByTestId("library-process-standard").click();
  for (const n of names.slice(0, 5)) await expect(card(page, n)).toHaveAttribute("data-doc-status", /extracted|reviewed/, { timeout: 20_000 });
  await page.getByTestId("library-clear-selection").click();
  return { estimate: ids[0]!, contract: ids[1]!, srs: ids[2]!, wbs: ids[3]!, test: ids[4]!, long: ids[5]! };
}

/** A project from the 내부 개발 template with three documents, a rule with a literal operand and a check run. */
async function createProject(page: Page, ids: DocIds): Promise<string> {
  await page.getByTestId("nav-projects").click();
  await page.getByTestId("projects-new").click();
  await page.getByTestId("project-name").fill("한빛 포털 구축");
  await page.getByTestId("project-template").click();
  await page.getByRole("option", { name: ko.projects.templates.builtin.internal_dev }).click();
  await page.getByTestId("project-submit").click();
  await expect(page.getByTestId("page-project")).toHaveAttribute("data-project-id", /.+/);
  const projectId = (await page.getByTestId("page-project").getAttribute("data-project-id"))!;
  await page.getByTestId("project-add-documents").click();
  for (const id of [ids.srs, ids.wbs, ids.test]) await page.getByTestId(`project-add-doc-${id}`).check();
  await page.getByTestId("project-add-done").click();
  await expect(page.getByTestId("project-documents-table").locator("tbody tr")).toHaveCount(3);
  await page.getByTestId("project-tab-rules").click();
  await page.getByTestId("rule-nl-input").fill("테스트 결과서의 통과율은 95 이상이어야 해");
  await page.getByTestId("rule-define").click();
  await expect(page.getByTestId("rule-grid")).toBeVisible({ timeout: 10_000 });
  await page.getByTestId("rule-save").click();
  await expect(page.getByTestId("rule-list").locator('[data-testid^="rule-"][data-rule-enabled]')).toHaveCount(4);
  await page.getByTestId("project-check-header").click();
  await expect(page.getByTestId("project-verdicts")).toBeVisible({ timeout: 10_000 });
  return projectId;
}

/** An estimate draft on the 문서 작성 screen: its item table carries the `sr-only` caption and column of UI-1. */
async function generateEstimate(page: Page): Promise<void> {
  await page.getByTestId("nav-generate").click();
  await page.getByTestId("generate-template").click();
  await page.getByRole("option", { name: ko.generate.templates.estimate }).click();
  await page.getByTestId("generate-content").fill("DocuLens에 백엔드개발 고급 3MM 단가 2,500,000원, 유효기간 30일 견적서");
  await page.getByTestId("generate-run").click();
  await expect(page.getByTestId("generate-preview-frame")).toHaveAttribute("data-loaded", "true", { timeout: 15_000 });
}

test.describe("창 전체 스크롤 없음 (UI-1)", () => {
  test("every screen at 100 % and 150 %: the document is as large as the window, a wheel turn moves nothing at window level and sr-only content stays in its scroll box", async ({ page }) => {
    test.setTimeout(240_000);
    await freshApp(page);
    await expectWindowFixed(page, "onboarding");
    await createWorkspace(page);
    const ids = await importDocuments(page);
    const projectId = await createProject(page, ids);

    // 공통 참고 자료: a group with two documents (its table has a sr-only column header).
    await page.getByTestId("project-back").click();
    await page.getByTestId("projects-common-reference").click();
    await page.getByTestId("reference-group-new").click();
    await page.getByTestId("reference-name").fill("사내 규정");
    await page.getByTestId("reference-name-save").click();
    await page.getByTestId("reference-ingest").click();
    await page.getByTestId("ingest-paths").fill("D:\\규정\\출장비규정-4p.pdf\nD:\\규정\\정보보안지침.pdf");
    await page.getByTestId("ingest-next").click();
    await page.getByTestId("ingest-preview").click();
    await expect(page.getByTestId("ingest-plan")).toBeVisible();
    await page.getByTestId("ingest-start").click();
    await expect(page.getByTestId("reference-table").locator("tbody tr")).toHaveCount(2, { timeout: 20_000 });

    // AI 어시스턴트: an aggregate card, an organise plan with its preview table that is run with "검토 완료만" off so
    // the result card lists the moves, and a second plan left waiting — two kinds of tables with a sr-only caption
    // and head (PlanCard, ResultCard) and enough cards for the message list to scroll.
    await page.getByTestId("nav-assistant").click();
    await send(page, "지난달 견적서 합계");
    await expect(page.locator('[data-testid="assistant-card-aggregate"]').last()).toBeVisible({ timeout: 15_000 });
    await send(page, "견적서 월별 폴더로 옮겨");
    const organise = page.locator('[data-testid="assistant-card-plan"]').last();
    await expect(organise).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    await organise.getByTestId("plan-only-reviewed").click();
    await expect(organise.locator('[data-testid="plan-preview-row"][data-item-status="move"]').first()).toBeVisible({ timeout: 10_000 });
    await organise.getByTestId("plan-execute").click();
    await expect(organise).toHaveAttribute("data-plan-status", "done", { timeout: 20_000 });
    const moved = page.locator('[data-testid="assistant-card-result"]').last();
    await moved.getByTestId("result-toggle-moves").click();
    await expect(moved.getByTestId("result-moves")).toBeVisible();
    await send(page, "전부 유형별 폴더로 정리해줘");
    await expect(page.locator('[data-testid="assistant-card-plan"]').last()).toHaveAttribute("data-plan-status", "pending", { timeout: 15_000 });
    expect(await page.getByTestId("assistant-messages").locator("caption.sr-only").count(), "plan preview and moves tables with a sr-only caption").toBeGreaterThanOrEqual(2);

    await generateEstimate(page);
    expect(await page.getByTestId("generate-editor").locator("caption.sr-only, th.sr-only").count(), "the estimate's item table has its sr-only caption and column").toBe(2);

    for (const scale of [100, 150] as const) {
      await setScale(page, scale);
      const at = (screen: string) => `${screen} at ${scale} %`;
      await expectWindowFixed(page, at("설정 > 일반"));
      for (const tab of ["workspace", "processing", "models", "schemas", "reference", "privacy"] as const) {
        if (tab === "models") await openModelsAdvanced(page);
        else await openSettingsTab(page, tab);
        await expectWindowFixed(page, at(`설정 > ${tab}`));
      }

      await page.getByTestId("nav-library").click();
      await page.getByTestId("library-view-grid").click();
      await expect(page.getByTestId("library-grid")).toBeVisible();
      await expectWindowFixed(page, at("문서함 (격자)"), page.getByTestId("library-grid"));
      await page.getByTestId("library-view-list").click();
      await expect(page.getByTestId("library-list")).toBeVisible();
      await expectWindowFixed(page, at("문서함 (목록)"), page.getByTestId("library-list"));

      await page.getByTestId(`doc-row-${ids.long}`).getByTestId(`doc-open-${ids.long}`).click();
      await expect(page.getByTestId("viewer-image")).toBeVisible();
      await expectWindowFixed(page, at("뷰어"), page.getByTestId("viewer-canvas"));
      await expectMainNotScrollable(page, at("뷰어"));

      await page.getByTestId("nav-assistant").click();
      const messages = page.getByTestId("assistant-messages");
      await expect(messages).toBeVisible();
      // The result card folds its moves table when the screen is left: open it again, so the plan preview and the
      // moves table (both with a sr-only caption and head) are in the list while it is measured.
      const movesToggle = messages.getByTestId("result-toggle-moves").last();
      if ((await movesToggle.getAttribute("aria-pressed")) !== "true") await movesToggle.click();
      await expect(messages.getByTestId("result-moves")).toBeVisible();
      expect(await messages.locator("caption.sr-only").count(), "plan preview and moves tables with a sr-only caption").toBeGreaterThanOrEqual(2);
      await expectWindowFixed(page, at("AI 어시스턴트"), page.getByTestId("assistant-messages"));
      // The plan previews' captions used to lie outside the message list and gave `main` a scroll range of its own.
      await expectMainNotScrollable(page, at("AI 어시스턴트"));
      await page.getByTestId("assistant-messages").evaluate((el) => el.scrollTo({ top: 0 }));
      await expectWindowFixed(page, at("AI 어시스턴트 (맨 위)"), page.getByTestId("assistant-messages"));
      await expectMainNotScrollable(page, at("AI 어시스턴트 (맨 위)"));

      await page.getByTestId("nav-review").click();
      await expect(page.locator('[data-testid^="review-item-"]').first()).toBeVisible({ timeout: 15_000 });
      await expectWindowFixed(page, at("검토함"));
      await expectMainNotScrollable(page, at("검토함"));

      await page.getByTestId("nav-projects").click();
      await expect(page.getByTestId("projects-kanban")).toBeVisible();
      await expectWindowFixed(page, at("프로젝트 (칸반)"));
      await page.getByTestId("projects-view-list").click();
      await expect(page.getByTestId("projects-table")).toBeVisible();
      await expectWindowFixed(page, at("프로젝트 (목록)"));
      await page.getByTestId("projects-view-kanban").click();
      await page.getByTestId("projects-common-reference").click();
      await expect(page.getByTestId("reference-table")).toBeVisible();
      await expectWindowFixed(page, at("공통 참고 자료"));
      await page.goto(`/#/projects/${projectId}`);
      await expect(page.getByTestId("project-documents-table")).toBeVisible();
      await expectWindowFixed(page, at("프로젝트 > 문서"));
      for (const tab of ["rules", "results", "settings"] as const) {
        await page.getByTestId(`project-tab-${tab}`).click();
        await expect(page.getByTestId(`project-tab-${tab}`)).toHaveAttribute("data-state", "active");
        await expectWindowFixed(page, at(`프로젝트 > ${tab}`));
      }

      // 문서 작성 with the draft: the screen of the audit's finding (1441 × 1242 px document, scrollY 442 after a wheel turn).
      await page.getByTestId("nav-generate").click();
      await expect(page.getByTestId("generate-preview-frame")).toBeVisible();
      await expectWindowFixed(page, at("문서 작성 (초안, 미리보기 위)"), page.getByTestId("generate-preview-box"));
      await expectWindowFixed(page, at("문서 작성 (초안, 편집기 위)"), page.getByTestId("generate-editor"));
      await expectMainNotScrollable(page, at("문서 작성"));
      // Script and focus cannot scroll the window either: the item table's hidden caption and the editor's last field are far below the fold.
      await page.getByTestId("generate-editor").locator("caption.sr-only").evaluate((el) => el.scrollIntoView());
      await page.getByTestId("generate-editor").locator("textarea, input").last().focus();
      await page.evaluate(() => {
        window.scrollTo(0, 500);
        document.body.scrollTop = 500;
        document.getElementById("root")!.scrollTop = 500;
      });
      await expectWindowFixed(page, at("문서 작성 (스크립트·포커스 뒤)"), page.getByTestId("generate-preview-box"));

      await page.getByTestId("nav-jobs").click();
      await expect(page.getByTestId("page-jobs")).toBeVisible();
      await expectWindowFixed(page, at("작업 현황"));
      for (const tab of ["finished", "history"] as const) {
        await page.getByTestId(`jobs-tab-${tab}`).click();
        await expectWindowFixed(page, at(`작업 현황 > ${tab}`));
      }

      await page.goto("/#/organize");
      await expect(page.getByTestId("page-organize")).toBeVisible();
      await expectWindowFixed(page, at("정리"));
      await page.getByTestId("organize-tab-export").click();
      await expectWindowFixed(page, at("내보내기"));

      await page.getByTestId("command-bar").click();
      await expect(page.getByTestId("command-palette")).toBeVisible();
      await expectWindowFixed(page, at("명령 팔레트"));
      await page.keyboard.press("Escape");
      await expect(page.getByTestId("command-palette")).toBeHidden();
    }
  });
});

interface DialogReport {
  box: [number, number, number, number];
  boxInside: boolean;
  closeInside: boolean | null;
  footerInside: boolean | null;
  controls: number;
  unreachable: string[];
}

/** Measured on the open dialog (`node`): its frame, the close button, the footer and every control. */
function dialogReport(node: HTMLElement): DialogReport {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const inside = (b: DOMRect) => b.left >= -0.5 && b.top >= -0.5 && b.right <= vw + 0.5 && b.bottom <= vh + 0.5;
  const box = node.getBoundingClientRect();
  const body = node.querySelector<HTMLElement>("[data-dialog-body]");
  const footer = node.querySelector<HTMLElement>("[data-dialog-footer]");
  if (body) body.scrollTop = 0;
  // Without scrolling: the close button and the whole footer row are in view.
  const close = node.querySelector<HTMLElement>(":scope > button[aria-label]");
  const closeInside = close ? inside(close.getBoundingClientRect()) : null;
  const footerInside = footer ? inside(footer.getBoundingClientRect()) && Array.from(footer.querySelectorAll("button")).every((b) => inside(b.getBoundingClientRect())) : null;
  const controls = Array.from(node.querySelectorAll<HTMLElement>('button, input, select, textarea, [role="switch"], [role="combobox"], [role="option"], [cmdk-item], a[href]')).filter((el) => {
    const cs = getComputedStyle(el);
    return cs.display !== "none" && cs.visibility !== "hidden" && el.getAttribute("aria-hidden") !== "true" && el.getBoundingClientRect().width > 0;
  });
  const unreachable: string[] = [];
  for (const el of controls) {
    // The way a click or Tab brings a control into view; it must then be inside the window and not under the footer.
    el.scrollIntoView({ block: "nearest", inline: "nearest" });
    const b = el.getBoundingClientRect();
    const hit = document.elementFromPoint(b.left + b.width / 2, b.top + b.height / 2);
    const reached = !!hit && (hit === el || el.contains(hit) || hit.contains(el) || !!hit.closest("label")?.contains(el));
    if (!inside(b) || !reached) unreachable.push(`${el.tagName.toLowerCase()}[${el.getAttribute("data-testid") ?? el.getAttribute("aria-label") ?? (el.textContent ?? "").trim().slice(0, 16)}] at ${Math.round(b.left)},${Math.round(b.top)} ${Math.round(b.width)}×${Math.round(b.height)}`);
  }
  if (body) body.scrollTop = 0;
  return { box: [Math.round(box.left), Math.round(box.top), Math.round(box.width), Math.round(box.height)], boxInside: inside(box), closeInside, footerInside, controls: controls.length, unreachable };
}

/** UI-3: the open dialog's box, close button and footer are inside the window and every control can be brought into view. */
async function expectDialogInside(page: Page, name: string, dialog: Locator = page.locator('[role="dialog"][data-state="open"]').last()): Promise<void> {
  await expect(dialog, `${name}: opens`).toBeVisible();
  const r = await dialog.evaluate(dialogReport);
  const size = page.viewportSize()!;
  const where = `${name}: box ${r.box[2]}×${r.box[3]} at ${r.box[0]},${r.box[1]} in a ${size.width}×${size.height} window`;
  // Soft: every dialog is opened, so a failure lists all the dialogs that do not fit.
  expect.soft(r.boxInside, `${where} — inside the window`).toBe(true);
  expect.soft(r.box[3], `${where} — a margin is left above and below`).toBeLessThan(size.height);
  if (r.closeInside !== null) expect.soft(r.closeInside, `${where} — close button in view`).toBe(true);
  if (r.footerInside !== null) expect.soft(r.footerInside, `${where} — footer buttons in view without scrolling`).toBe(true);
  expect.soft(r.controls, `${where} — has controls`).toBeGreaterThan(0);
  expect.soft(r.unreachable, `${where} — controls outside the window or covered`).toEqual([]);
}

/** Opens a dialog with `open`, checks it and closes it with Escape. */
async function checkDialog(page: Page, name: string, open: () => Promise<unknown>): Promise<void> {
  await dismissToasts(page);
  await open();
  await expectDialogInside(page, name);
  await page.keyboard.press("Escape");
  await expect(page.locator('[role="dialog"][data-state="open"]'), `${name}: closes`).toHaveCount(0);
}

/** Every dialog the mock backend can reach; returns how many were checked. */
async function checkAllDialogs(page: Page, ids: DocIds, projectId: string): Promise<number> {
  let checked = 0;
  const dialog = async (name: string, open: () => Promise<unknown>) => {
    await checkDialog(page, name, open);
    checked++;
  };

  // 프로젝트
  await page.getByTestId("nav-projects").click();
  await dialog("새 프로젝트", () => page.getByTestId("projects-new").click());
  await dialog("템플릿 관리", () => page.getByTestId("projects-templates").click());
  await dialog("자동 분류", () => page.getByTestId("projects-classify").click());
  // `?put=` opens the dialog by itself: clear the toasts first (a click on one would close the dialog).
  await dismissToasts(page);
  await page.goto(`/#/projects?put=${ids.contract}`);
  await dialog("프로젝트에 넣기", () => expect(page.getByTestId("put-dialog")).toBeVisible());
  await page.goto(`/#/projects/${projectId}`);
  await expect(page.getByTestId("page-project")).toHaveAttribute("data-project-id", projectId);
  await dialog("프로젝트 편집", () => page.getByTestId("project-edit").click());
  await dialog("프로젝트 문서 추가", () => page.getByTestId("project-add-documents").click());
  await dialog("참고 자료 가져오기 마법사", () => page.getByTestId("project-reference-ingest").click());
  await dialog("보완요청서", () => page.getByTestId("project-supplement").click());
  await dialog("프로젝트 삭제 확인", () => page.getByTestId("project-delete").click());
  await page.getByTestId("project-back").click();
  await page.getByTestId("projects-common-reference").click();
  await dialog("참고 자료 묶음 이름", () => page.getByTestId("reference-group-new").click());

  // 문서함 · 뷰어
  await page.getByTestId("nav-library").click();
  await dialog("문서 가져오기 (미리보기 포함)", () => openImportPreview(page, ["D:\\scan\\회의록-3p.pdf", "D:\\scan\\photo.heic", "D:\\scan\\IMG_0001.jpg", "D:\\scan\\a.pdf", "D:\\scan\\b.pdf", "D:\\scan\\c.pdf", "D:\\scan\\d.pdf", "D:\\scan\\e.pdf"]));
  await page.getByTestId("library-view-list").click();
  for (const id of [ids.estimate, ids.contract]) await page.getByTestId(`doc-row-${id}`).locator('input[type="checkbox"]').check();
  await dialog("문서 병합", () => page.getByTestId("library-merge").click());
  await dialog("프로젝트에 넣기 (문서함)", () => page.getByTestId("library-put-in-project").click());
  await dialog("문서 삭제 확인", () => page.getByTestId("library-delete").click());
  await page.getByTestId("library-clear-selection").click();
  await page.getByTestId(`doc-row-${ids.long}`).getByTestId(`doc-open-${ids.long}`).click();
  await expect(page.getByTestId("page-viewer")).toBeVisible();
  await dialog("문서 분할", () => page.getByTestId("viewer-split").click());
  await dialog("버전 지정", () => page.getByTestId("viewer-version").click());

  // 검토함
  await page.getByTestId("nav-review").click();
  await dialog("검토 값 수정", () => page.getByTestId("review-edit").click());

  // 문서 작성
  await page.getByTestId("nav-generate").click();
  await dialog("초안 저장", () => page.getByTestId("generate-save").click());
  await dialog("저장된 초안", () => page.getByTestId("generate-drafts-open").click());
  await dialog("참고 문서 고르기", () => page.getByTestId("generate-pick-sources").click());

  // 정리
  await page.goto("/#/organize");
  await dialog("정리 실행 확인", async () => {
    await page.getByTestId("organize-preview").click();
    await expect(page.getByTestId("organize-preview-table")).toBeVisible();
    await page.getByTestId("organize-run").click();
  });

  // 설정
  await openSettingsTab(page, "workspace");
  await dialog("워크스페이스 열기", () => page.getByTestId("settings-open-workspace").click());
  await openSettingsTab(page, "schemas");
  await dialog("양식 등록", () => page.getByTestId("schemas-register-form").click());
  await dialog("자연어로 추출 항목 만들기", () => page.getByTestId("schemas-nl-create").click());
  // A form named like the built-in 견적서 sample: the derived draft opens with the "이미 쓰는 이름" popup.
  await dialog("양식 이름 중복", async () => {
    await page.getByTestId("schemas-register-form").click();
    await page.getByTestId(`form-register-doc-${ids.estimate}`).check();
    await page.getByTestId("form-register-name").fill(ko.documents.doc_types.estimate);
    await page.getByTestId("form-register-derive").click();
    await expect(page.getByTestId("schema-name-taken")).toBeVisible({ timeout: 15_000 });
  });
  // Leaving the tab drops the unsaved draft.
  await openModelsAdvanced(page);
  await dialog("GGUF 가져오기", () => page.getByTestId("models-import").click());
  await dialog("필요 파일 목록", () => page.getByTestId("registry-table").getByRole("button", { name: ko.models.required_files }).first().click());
  await dialog("presets.ini 미리보기", () => page.getByTestId("engine-card").getByRole("button", { name: ko.models.engine_card.presets }).click());
  // 채팅 테스트 needs a loaded model: import one, load it.
  await page.getByTestId("models-import").click();
  await page.getByTestId("import-paths").fill("D:\\usb\\gemma-4-E4B-it-Q5_K_M.gguf");
  await page.getByTestId("import-validate").click();
  await page.getByTestId("import-run").click();
  await expect(page.getByTestId("discovered-gemma-4-e4b-it-q5_k_m")).toBeVisible({ timeout: 15_000 });
  await page.getByTestId("load-gemma4-e4b-q5km").click();
  await expect(page.getByTestId("instance-gemma4-e4b-q5km")).toHaveText(ko.models.instance.loaded, { timeout: 20_000 });
  await dialog("채팅 테스트", () => page.getByTestId("engine-test").click());
  await openSettingsTab(page, "reference");
  if ((await page.getByTestId("reference-section-glossary").getAttribute("data-open")) !== "true") await page.getByTestId("reference-section-toggle-glossary").click();
  await dialog("용어 추가", () => page.getByTestId("glossary-add").click());

  // 명령 팔레트 (cmdk puts its classes on the inner root; the role=dialog wrapper has an empty box).
  await dismissToasts(page);
  await page.getByTestId("command-bar").click();
  await expectDialogInside(page, "명령 팔레트", page.getByTestId("command-palette"));
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("command-palette")).toBeHidden();
  checked++;
  return checked;
}

test.describe("대화상자는 창 안에 (UI-3)", () => {
  for (const config of [
    { name: "UI 배율 150 % (1280×800)", viewport: { width: 1280, height: 800 }, scale: 150 as const },
    { name: "최소 창 960×600 (100 %)", viewport: { width: 960, height: 600 }, scale: 100 as const },
  ]) {
    test(`${config.name}: every dialog fits the window, the close button and the footer stay in view and each control can be reached`, async ({ page }) => {
      test.setTimeout(240_000);
      await page.setViewportSize(config.viewport);
      await freshApp(page);
      await createWorkspace(page);
      const ids = await importDocuments(page);
      const projectId = await createProject(page, ids);
      await generateEstimate(page);
      if (config.scale !== 100) await setScale(page, config.scale);
      const checked = await checkAllDialogs(page, ids, projectId);
      // All 27 `DialogContent` uses (some opened from two places) and the command palette.
      expect(checked, "dialogs checked").toBe(31);
    });
  }

  test("새 프로젝트 at 150 %: the body scrolls under the pinned footer, a field focused at the bottom stays above it, and the project can be created", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await setScale(page, 150);
    await page.getByTestId("nav-projects").click();
    await dismissToasts(page);
    await page.getByTestId("projects-new").click();
    const dialog = page.getByTestId("project-dialog");
    await expect(dialog).toBeVisible();
    // The audit measured 959 px (top −79, bottom 880) in the 800 px window.
    const box = (await dialog.boundingBox())!;
    expect(box.y).toBeGreaterThanOrEqual(0);
    expect(box.y + box.height).toBeLessThanOrEqual(800);
    const body = dialog.locator("[data-dialog-body]");
    expect(await body.evaluate((el) => el.scrollHeight > el.clientHeight), "the form is taller than the dialog: its body scrolls").toBe(true);
    const submit = page.getByTestId("project-submit");
    await expect(submit).toBeInViewport({ ratio: 1 });
    await expect(dialog.getByRole("button", { name: ko.common.close })).toBeInViewport({ ratio: 1 });
    // The last field, focused with the keyboard, is scrolled into view above the footer row.
    const keywords = page.getByTestId("project-keywords");
    await keywords.focus();
    await expect(keywords).toBeInViewport({ ratio: 1 });
    const footerTop = (await dialog.locator("[data-dialog-footer]").boundingBox())!.y;
    const field = (await keywords.boundingBox())!;
    expect(field.y + field.height, "the focused field ends above the footer").toBeLessThanOrEqual(footerTop + 1);
    await expect(submit).toBeInViewport({ ratio: 1 });
    await page.getByTestId("project-name").fill("배율 150 프로젝트");
    await submit.click();
    await expect(page.getByTestId("project-title")).toHaveText("배율 150 프로젝트");
  });
});

test.describe("문서함 목록 · 필터 (UI-6, UI-7, UI-16)", () => {
  /** Column headers of the list that are laid out, in order. */
  const shownColumns = (page: Page) => page.getByTestId("library-list").locator('[role="columnheader"][data-column]').evaluateAll((els) => els.filter((e) => getComputedStyle(e).display !== "none").map((e) => e.getAttribute("data-column")));
  const listOverflow = (page: Page) => page.getByTestId("library-list").evaluate((el) => el.scrollWidth - el.clientWidth);

  test("the list fits its box at the default window and at 150 %, showing the columns its width allows; the filters name themselves", async ({ page }) => {
    test.setTimeout(90_000);
    await freshApp(page);
    await createWorkspace(page);
    const ids = await importDocuments(page);
    await page.getByTestId("library-view-list").click();
    const list = page.getByTestId("library-list");
    await expect(list.locator('[data-testid^="doc-row-"]')).toHaveCount(6);

    // 1280 × 800 at 100 %: the list is 52 rem wide (the audit measured 928 px of content in 832 px).
    expect(await listOverflow(page), "no horizontal scrollbar at the default window").toBeLessThanOrEqual(0);
    expect(await shownColumns(page)).toEqual(["name", "status", "doc_type", "review", "imported"]);
    const name = (await page.getByTestId(`doc-open-${ids.contract}`).boundingBox())!;
    expect((await list.locator('[role="columnheader"][data-column="name"]').boundingBox())!.width, "the name column keeps a usable width").toBeGreaterThan(200);
    expect(name.width).toBeGreaterThan(60);
    // The 유형 cell is cut to its column and says the whole label in its tooltip (UI-16).
    const type = page.getByTestId(`doc-row-${ids.contract}`).getByTestId(`doc-type-${ids.contract}`);
    await expect(type).toHaveAttribute("title", ko.documents.doc_types.dev_contract);
    await expect(type).toHaveText(ko.documents.doc_types.dev_contract);

    // A wide window shows every column, as before.
    await page.setViewportSize({ width: 1700, height: 900 });
    await expect.poll(() => shownColumns(page)).toEqual(["name", "kind", "pages", "status", "doc_type", "review", "imported", "size"]);
    expect(await listOverflow(page)).toBeLessThanOrEqual(0);
    await page.setViewportSize({ width: 1280, height: 800 });
    await expect.poll(() => shownColumns(page)).toEqual(["name", "status", "doc_type", "review", "imported"]);

    // Filters: "상태: 전체" instead of four selects that all read "전체" (UI-7); the chosen value follows the label.
    const filters = [
      ["library-filter-status", ko.library.filters.status],
      ["library-filter-kind", ko.library.filters.kind],
      ["library-filter-type", ko.library.filters.doc_type],
      ["library-filter-review", ko.library.filters.review],
    ] as const;
    for (const [id, label] of filters) {
      await expect(page.getByTestId(id)).toHaveText(tpl(ko.library.filters.labelled, { label, value: ko.library.filters.all }));
      await expect(page.getByTestId(id)).toHaveAccessibleName(new RegExp(label));
    }
    expect(new Set(await Promise.all(filters.map(([id]) => page.getByTestId(id).innerText()))).size, "four different labels").toBe(4);
    await page.getByTestId("library-filter-kind").click();
    await page.getByRole("option", { name: ko.documents.kind.image }).click();
    await expect(page.getByTestId("library-filter-kind")).toHaveText(tpl(ko.library.filters.labelled, { label: ko.library.filters.kind, value: ko.documents.kind.image }));
    await expect(list.locator('[data-testid^="doc-row-"]')).toHaveCount(1);
    await page.getByTestId("library-filter-kind").click();
    await page.getByRole("option", { name: ko.library.filters.all }).click();
    await expect(list.locator('[data-testid^="doc-row-"]')).toHaveCount(6);

    // 150 %: the list is ~25 rem wide (the audit measured 1,392 px of content in 608 px).
    await setScale(page, 150);
    await page.getByTestId("nav-library").click();
    await expect(list.locator('[data-testid^="doc-row-"]').first()).toBeVisible();
    expect(await listOverflow(page), "no horizontal scrollbar at 150 %").toBeLessThanOrEqual(0);
    expect(await shownColumns(page)).toEqual(["name", "status"]);
    expect((await list.locator('[role="columnheader"][data-column="name"]').boundingBox())!.width).toBeGreaterThan(200);
  });
});

test.describe("설정 스위치의 이름 (UI-9)", () => {
  test("every switch and slider on the seven settings tabs has an accessible name", async ({ page }) => {
    test.setTimeout(60_000);
    await freshApp(page);
    await createWorkspace(page);
    let total = 0;
    let sliders = 0;
    for (const tab of ["general", "workspace", "processing", "models", "schemas", "reference", "privacy"] as const) {
      if (tab === "models") await openModelsAdvanced(page);
      else await openSettingsTab(page, tab);
      const switches = await page.getByTestId("page-settings").getByRole("switch").all();
      for (const [i, sw] of switches.entries()) await expect(sw, `설정 > ${tab}: switch ${i + 1} of ${switches.length}`).toHaveAccessibleName(/\S/);
      total += switches.length;
      // A slider's label has to sit on its thumb (the element with the role), not on the root.
      const thumbs = await page.getByTestId("page-settings").getByRole("slider").all();
      for (const [i, thumb] of thumbs.entries()) await expect(thumb, `설정 > ${tab}: slider ${i + 1} of ${thumbs.length}`).toHaveAccessibleName(/\S/);
      sliders += thumbs.length;
    }
    // 일반 3 · 처리 5 · AI 모델 3 · 참고 자료 4 · 프라이버시 10 on 0.7.0; the count only has to stay above the dozen the audit found unnamed.
    expect(total).toBeGreaterThanOrEqual(12);
    expect(sliders, "UI 배율 · 검토 임계값 · 최소 유사도").toBeGreaterThanOrEqual(3);
    await openSettingsTab(page, "general");
    await expect(page.getByTestId("scale-slider").getByRole("slider")).toHaveAccessibleName(ko.settings.general.scale);
    // The rows the audit listed, by the name a screen reader announces.
    await openSettingsTab(page, "processing");
    await expect(page.getByTestId("settings-ocr-digital")).toHaveAccessibleName(ko.settings.processing.ocr_digital_pdf);
    await expect(page.getByTestId("settings-precise-mode")).toHaveAccessibleName(ko.settings.processing.precise_mode);
    await expect(page.getByTestId("settings-dense-reocr")).toHaveAccessibleName(ko.settings.processing.dense_reocr);
    await expect(page.getByTestId("settings-inbox-watch")).toHaveAccessibleName(ko.settings.processing.inbox_watch);
    await openSettingsTab(page, "reference");
    await expect(page.getByTestId("kb-auto-index")).toHaveAccessibleName(ko.settings.knowledge.auto_index);
    await expect(page.getByTestId("kb-allow-general")).toHaveAccessibleName(ko.settings.knowledge.allow_general);
    await openSettingsTab(page, "privacy");
    await expect(page.getByTestId("pii-masking")).toHaveAccessibleName(ko.settings.privacy.pii_masking);
  });
});

/** Whether two boxes overlap. */
const overlaps = (a: { x: number; y: number; width: number; height: number }, b: { x: number; y: number; width: number; height: number }) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

test.describe("토스트 위치 (UI-11) · 조사 (UI-13)", () => {
  test("a toast covers neither the assistant's send button, nor the last table row's buttons, nor the theme switch", async ({ page }) => {
    test.setTimeout(60_000);
    await freshApp(page);
    await createWorkspace(page);
    await importViaLibrary(page, ["D:\\scan\\요구사항정의서-v1.2.pdf", "D:\\scan\\정보보안지침.pdf"], { autoLevel: "none" });
    const guide = await docId(page, "정보보안지침.pdf");
    const srs = await docId(page, "요구사항정의서-v1.2.pdf");

    // AI 어시스턴트: Ctrl+Z with nothing to undo raises a toast while the composer is in view.
    await page.getByTestId("nav-assistant").click();
    await dismissToasts(page);
    await page.getByTestId("assistant-messages").click();
    await page.keyboard.press("Control+z");
    const toaster = page.getByTestId("toaster");
    const toast = toaster.locator('[role="status"]').last();
    await expect(toast).toContainText(ko.palette.nothing_to_undo);
    const toastBox = (await toast.boundingBox())!;
    expect(overlaps(toastBox, (await page.getByTestId("assistant-send").boundingBox())!), "toast over the send button").toBe(false);
    expect(overlaps(toastBox, (await page.getByTestId("assistant-input").boundingBox())!), "toast over the composer").toBe(false);
    expect(overlaps(toastBox, (await page.getByTestId("theme-menu").boundingBox())!), "toast over the theme menu").toBe(false);
    expect(overlaps(toastBox, (await page.getByTestId("theme-toggle").boundingBox())!), "toast over the theme switch").toBe(false);
    expect(toastBox.y, "toast at the top of the window").toBeLessThan(40);
    // The send button works while the toast is up.
    await page.getByTestId("assistant-input").fill("/테마 다크");
    await page.getByTestId("assistant-send").click({ timeout: 2_000 });
    // UI-13: "(으)로" follows the word — 다크로, 시스템으로.
    const executed = page.locator('[data-testid="assistant-card-executed"]');
    await expect(executed.last()).toContainText("테마를 '다크'로 변경했습니다");
    await send(page, "/테마 시스템");
    await expect(executed.last()).toContainText("테마를 '시스템'으로 변경했습니다");
    await send(page, "/테마 라이트");
    await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

    // 프로젝트 > 문서: switching the 구분 of the last row raises a toast; the row's buttons stay free.
    await page.getByTestId("nav-projects").click();
    await page.getByTestId("projects-new").click();
    await page.getByTestId("project-name").fill("신규 과제");
    await page.getByTestId("project-submit").click();
    await expect(page.getByTestId("page-project")).toHaveAttribute("data-project-id", /.+/);
    await page.getByTestId("project-add-documents").click();
    await page.getByTestId(`project-add-doc-${srs}`).check();
    await page.getByTestId(`project-add-doc-${guide}`).check();
    await page.getByTestId("project-add-done").click();
    const rows = page.getByTestId("project-documents-table").locator("tbody tr");
    await expect(rows).toHaveCount(2);
    await dismissToasts(page);
    const last = rows.last();
    const lastName = (await last.getAttribute("data-testid")) === `project-doc-${guide}` ? "정보보안지침.pdf" : "요구사항정의서-v1.2.pdf";
    await last.getByTestId("project-doc-kind").click();
    await expect(last).toHaveAttribute("data-kind", "reference", { timeout: 10_000 });
    const kindToast = toaster.locator('[role="status"]').last();
    // UI-13: the file name ends in Latin letters (its particle stays written), the kind is resolved: 참고 자료로.
    await expect(kindToast).toHaveText(new RegExp(`'${lastName.replace(/\./g, "\\.")}'을\\(를\\) ${ko.projects.put.kind_reference}로 바꿨습니다\\.`));
    const kindToastBox = (await kindToast.boundingBox())!;
    for (const button of await last.getByRole("button").all()) expect(overlaps(kindToastBox, (await button.boundingBox())!), "toast over a button of the last row").toBe(false);
    await last.getByTestId("project-doc-kind").click({ timeout: 2_000 });
    await expect(last).toHaveAttribute("data-kind", "work", { timeout: 10_000 });
  });
});

test.describe("영어 UI의 모델 이름 (UI-14)", () => {
  test("no Korean descriptor is left in the model names of the English UI", async ({ page }) => {
    test.setTimeout(60_000);
    await freshApp(page);
    // Onboarding in Korean shows the registry name as it is …
    await createWorkspace(page, "C:\\Work\\Docs", { finish: false });
    await expect(page.getByTestId("onboarding-model-state")).toContainText(/Gemma 4 .*\(.*[가-힣]/);
    await page.getByTestId("onboarding-finish").click();
    await expect(page.getByTestId("app-shell")).toBeVisible();
    await expect(page.getByTestId("model-badge")).toHaveAttribute("title", /[가-힣]/);
    // … and in English the descriptor is translated: registry table, active model select, top bar and status texts.
    await page.getByTestId("theme-menu").click();
    await page.getByTestId("language-option-en").click();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await expect(page.getByTestId("model-badge")).toHaveAttribute("title", /Gemma 4/);
    await expect(page.getByTestId("model-badge")).not.toHaveAttribute("title", /[가-힣]/);
    await expect(page.getByTestId("model-badge")).not.toContainText(/[가-힣]/);
    await openModelsAdvanced(page);
    const registry = page.getByTestId("registry-table");
    await expect(registry).toContainText(`Gemma 4 E4B (Q6_K, ${en.models.display_terms.accuracy})`);
    await expect(registry).toContainText(`(Q4_K_M, CPU·${en.models.display_terms.low_headroom})`);
    await expect(registry).toContainText(`(Q5_K_M, ${en.models.display_terms.balanced})`);
    await expect(registry).toContainText(`(Q8_0, ${en.models.display_terms.embedding} · CPU)`);
    for (const name of await registry.locator("tbody tr td:first-child span.font-medium").allInnerTexts()) expect(name, "registry model name").not.toMatch(/[가-힣]/);
    await expect(page.getByTestId("active-model-state")).not.toContainText(/[가-힣]/);
    await page.getByTestId("active-model").click();
    for (const option of await page.getByRole("option").allInnerTexts()) expect(option, "active model option").not.toMatch(/[가-힣]/);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("hardware-panel")).not.toContainText(/[가-힣]/);
    // 설정 > 참고 자료: the embedding model select.
    await openSettingsTab(page, "reference");
    await page.getByTestId("settings-knowledge").getByRole("combobox", { name: en.settings.knowledge.embedding_model }).click();
    for (const option of await page.getByRole("option").allInnerTexts()) expect(option, "embedding model option").not.toMatch(/[가-힣]/);
    await page.keyboard.press("Escape");
  });
});

test.describe("양식·추출 항목 매핑 표 (USR-5) · 가져오기 문구 (UI-12) · 툴팁 (UI-16)", () => {
  test("the mapping table has two columns, a slug stays on one line with its full value in the tooltip, and no 내장 / 사용자 column", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openSettingsTab(page, "schemas");
    const table = page.getByTestId("schemas-mapping");
    await table.scrollIntoViewIfNeeded();
    await expect(table.locator("thead th")).toHaveCount(2);
    await expect(table.locator("thead th").nth(0)).toHaveText(ko.schemas.mapping.type);
    await expect(table.locator("thead th").nth(1)).toHaveText(ko.schemas.mapping.schema);
    const rows = table.locator("tbody tr");
    expect(await rows.count()).toBeGreaterThanOrEqual(13);
    for (const cells of await rows.evaluateAll((trs) => trs.map((tr) => tr.children.length))) expect(cells, "cells per row").toBe(2);
    // The third column's words are gone from the table (they stay on the list's section titles and the source badge).
    await expect(table).not.toContainText("내장");
    await expect(table).not.toContainText("사용자");
    // Every slug is one line high, inside its cell, with the whole slug in the tooltip (the screenshot showed "progress_repor/t").
    const slugs = await rows.evaluateAll((trs) =>
      trs.map((tr) => {
        const td = tr.children[0] as HTMLElement;
        const style = getComputedStyle(td);
        return { text: td.textContent?.trim() ?? "", title: td.getAttribute("title"), lines: Math.round((td.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom)) / parseFloat(style.lineHeight)), nowrap: style.whiteSpace, ellipsis: style.textOverflow };
      }),
    );
    expect(slugs.map((s) => s.text)).toContain("progress_report");
    expect(slugs.map((s) => s.text)).toContain("meeting_minutes");
    for (const s of slugs) {
      expect(s.title, `${s.text}: tooltip`).toBe(s.text);
      expect(s.lines, `${s.text}: one line`).toBe(1);
      expect(s.nowrap, `${s.text}: no wrap`).toBe("nowrap");
      expect(s.ellipsis, `${s.text}: ellipsis`).toBe("ellipsis");
    }
    // The table stays inside the narrow pane.
    expect(await page.getByTestId("schemas-list").evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
  });

  test("the import dialog names every supported format; the viewer's layer hint and the top bar's command hint carry tooltips", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await page.getByTestId("nav-library").click();
    // UI-12: the empty library and the import dialog list the formats the importer takes (the same list as its "unsupported" message).
    for (const format of ["HWP/HWPX/HML", "DOC/DOCX", "XLS/XLSX", "PPT/PPTX", "RTF"]) {
      await expect(page.getByTestId("library-empty")).toContainText(format);
      expect(ko.import_docs.description).toContain(format);
      expect(en.import_docs.description).toContain(format);
      expect(ko.documents.unsupported.unsupported_extension).toContain(format);
    }
    await page.getByTestId("library-import").click();
    await expect(page.getByRole("dialog")).toContainText(ko.import_docs.description);
    await page.keyboard.press("Escape");
    await importViaLibrary(page, ["D:\\scan\\계약서-12p.pdf"], { autoLevel: "none" });
    await card(page, "계약서-12p.pdf").locator('[data-testid^="doc-open-"]').click();
    await expect(page.getByTestId("viewer-text-layer")).toBeVisible();
    // UI-16: one cut-off line → the tooltip has all of it.
    await expect(page.getByTestId("viewer-layer-hint")).toHaveAttribute("title", ko.viewer.layer.hint);
    await expect(page.getByTestId("command-bar")).toHaveAttribute("title", ko.topbar.command);
  });
});
