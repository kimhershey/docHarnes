import { expect, test, type Page } from "@playwright/test";

import { createWorkspace, freshApp, openAssistantScreen } from "./helpers";

import ko from "../src/i18n/ko.json" with { type: "json" };

// 0.7.0 §1.6: assistant chat sessions — a conversation per session, stored per workspace (the mock keeps
// them in localStorage): 새 채팅 starts an empty one, the rail switches back with the messages restored,
// a reload keeps the list and the active session, rename and delete work from the row menu.

const rows = (page: Page) => page.locator('[data-testid="assistant-sessions"] [data-session-id]');
const activeRow = (page: Page) => page.locator('[data-testid="assistant-sessions"] [data-session-id][data-active="true"]');

async function send(page: Page, text: string) {
  await page.getByTestId("assistant-input").fill(text);
  await page.getByTestId("assistant-send").click();
}

test.describe("assistant chat sessions", () => {
  test("new chat, switch back, reload, rename and delete", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);

    // A fresh workspace has one (empty) session; a slash command fills it with two remembered turns.
    await expect(rows(page)).toHaveCount(1);
    await expect(page.getByTestId("assistant-memory")).toHaveAttribute("data-turns", "0");
    await send(page, "/테마 다크");
    await expect(page.locator('[data-testid="assistant-card-executed"]').last()).toBeVisible();
    await expect(page.getByTestId("assistant-memory")).toHaveAttribute("data-turns", "2");
    // The session takes its title from the first user message.
    await expect(activeRow(page)).toContainText("/테마 다크");

    // 새 채팅: an empty second session becomes active; the first keeps its conversation.
    await page.getByTestId("assistant-new-chat").click();
    await expect(rows(page)).toHaveCount(2);
    await expect(page.getByTestId("assistant-memory")).toHaveAttribute("data-turns", "0");
    await expect(page.locator('[data-testid="assistant-card-executed"]')).toHaveCount(0);
    await expect(activeRow(page)).toContainText(ko.assistant.sessions.untitled);
    // Another 새 채팅 while the active one is empty does not pile up sessions.
    await page.getByTestId("assistant-new-chat-header").click();
    await expect(rows(page)).toHaveCount(2);

    // Back to the first session: its messages and memory are restored.
    await rows(page).filter({ hasText: "/테마 다크" }).locator("button").first().click();
    await expect(page.locator('[data-testid="assistant-card-executed"]').last()).toBeVisible();
    await expect(page.getByTestId("assistant-memory")).toHaveAttribute("data-turns", "2");
    const firstId = await activeRow(page).getAttribute("data-session-id");
    expect(firstId).toBeTruthy();

    // A reload keeps the sessions (stored per workspace) and the active one.
    await page.reload();
    await expect(page.getByTestId("app-shell")).toBeVisible();
    await openAssistantScreen(page);
    await expect(rows(page)).toHaveCount(2);
    await expect(activeRow(page)).toHaveAttribute("data-session-id", firstId!);
    await expect(page.locator('[data-testid="assistant-card-executed"]').last()).toBeVisible();
    await expect(page.getByTestId("assistant-memory")).toHaveAttribute("data-turns", "2");

    // Rename from the row menu (Enter commits).
    await page.getByTestId(`assistant-session-menu-${firstId}`).click();
    await page.getByTestId("assistant-session-rename").click();
    const input = page.getByTestId("assistant-session-rename-input");
    await expect(input).toBeVisible();
    await input.fill("테마 바꾸기");
    await input.press("Enter");
    await expect(activeRow(page)).toContainText("테마 바꾸기");
    await page.reload();
    await expect(page.getByTestId("app-shell")).toBeVisible();
    await openAssistantScreen(page);
    await expect(activeRow(page)).toContainText("테마 바꾸기");

    // Delete the active session after confirming: the remaining one becomes active, empty.
    await page.getByTestId(`assistant-session-menu-${firstId}`).click();
    await page.getByTestId("assistant-session-delete").click();
    await expect(page.getByTestId("confirm-dialog")).toBeVisible();
    await page.getByTestId("confirm-ok").click();
    await expect(rows(page)).toHaveCount(1);
    await expect(activeRow(page)).not.toHaveAttribute("data-session-id", firstId!);
    await expect(page.getByTestId("assistant-memory")).toHaveAttribute("data-turns", "0");
    await expect(page.locator('[data-testid="assistant-card-executed"]')).toHaveCount(0);
  });

  // KNO-2: the assistant's own "워크스페이스 … 만들어" switches the workspace before its "실행됨" card is
  // pushed. That card belongs to the conversation of the old workspace: written under the old session id
  // it would land in the new workspace's database, where the backend (the mock does the same) re-creates
  // the unknown session — an untitled ghost chat.
  test("a card that arrives after the assistant switched the workspace is not written into the new one", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);
    await send(page, "새 워크스페이스 D:\\Other 만들어");
    await expect(page.getByTestId("page-library")).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("workspace-switcher")).toHaveAttribute("title", "D:\\Other");

    await openAssistantScreen(page);
    // Exactly the one fresh chat of the new workspace, empty.
    await expect(rows(page)).toHaveCount(1);
    await expect(activeRow(page)).toContainText(ko.assistant.sessions.untitled);
    await expect(page.getByTestId("assistant-memory")).toHaveAttribute("data-turns", "0");
    await expect(page.getByTestId("assistant-send")).toBeEnabled();
    const stored = await page.evaluate(() => JSON.parse(localStorage.getItem("doculens.mock.chat") ?? "{}") as Record<string, { sessions: { id: string }[]; messages: Record<string, unknown[]> }>);
    expect(stored["D:\\Other"]!.sessions).toHaveLength(1);
    expect(Object.values(stored["D:\\Other"]!.messages).flat()).toHaveLength(0);
    // The first workspace keeps the command in its own chat.
    expect(stored["C:\\Work\\Docs"]!.sessions).toHaveLength(1);
    expect(Object.values(stored["C:\\Work\\Docs"]!.messages).flat()).toHaveLength(1);

    // A reload hydrates from the store alone: still one chat.
    await page.reload();
    await expect(page.getByTestId("app-shell")).toBeVisible();
    await openAssistantScreen(page);
    await expect(rows(page)).toHaveCount(1);
  });

  // KNO-15: message ids continue after the stored ones. A chat whose stored messages could not be read was
  // shown as empty and took input — the new message got id 1 and replaced the stored first message.
  test("a chat whose messages cannot be read takes no input until the retry succeeds", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);
    await send(page, "/테마 다크");
    await expect(page.locator('[data-testid="assistant-card-executed"]')).toHaveCount(1);
    await expect(page.getByTestId("assistant-memory")).toHaveAttribute("data-turns", "2");

    // The next read of the chat's messages fails (the mock's stand-in for a locked database).
    await page.evaluate(() => localStorage.setItem("doculens.mock.chat.fail_list", "1"));
    await page.reload();
    await expect(page.getByTestId("app-shell")).toBeVisible();
    await openAssistantScreen(page);
    await expect(page.getByTestId("assistant-load-failed")).toBeVisible();
    await expect(page.getByTestId("assistant-load-failed")).toContainText(ko.assistant.sessions.load_failed);
    await expect(page.getByTestId("assistant-send")).toBeDisabled();
    await expect(page.locator('[data-testid="assistant-card-executed"]')).toHaveCount(0);

    await page.getByTestId("assistant-load-retry").click();
    await expect(page.getByTestId("assistant-load-failed")).toBeHidden();
    await expect(page.locator('[data-testid="assistant-card-executed"]')).toHaveCount(1);
    await expect(page.getByTestId("assistant-send")).toBeEnabled();
    // The next command continues after the stored messages instead of replacing them.
    await send(page, "/테마 라이트");
    await expect(page.locator('[data-testid="assistant-card-executed"]')).toHaveCount(2);
    await expect(page.getByTestId("assistant-memory")).toHaveAttribute("data-turns", "4");
    await expect(activeRow(page)).toContainText("/테마 다크");
  });

  // KNO-10: a sentence handed over by the command palette while the assistant was answering vanished
  // (the page consumed it and `send` refused it). It waits in the composer now.
  test("a sentence sent from the palette while the assistant answers waits in the composer", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);
    // The mock reads its speed flag per stream delta: slow it down so the answer below is still running
    // when the palette hands its sentence over.
    await page.evaluate(() => localStorage.removeItem("doculens.mock.fast"));
    const body = "금번 과업은 당사가 수행하며 기한은 12월말까지로 하고 산출물은 설계서와 소스코드로 함. ".repeat(20);
    await send(page, `다음 문장을 다듬어 정리해줘: ${body}`);
    const answer = page.locator('[data-testid="assistant-card-answer"] [data-testid="answer-card"]').last();
    await expect(answer).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("assistant-send")).toBeDisabled();

    await page.keyboard.press("Control+K");
    await expect(page.getByTestId("command-palette")).toBeVisible();
    await page.getByTestId("palette-input").fill("견적서 월별 폴더로 옮겨");
    await page.getByTestId("palette-send").click();
    await expect(page.getByTestId("assistant-input")).toHaveValue("견적서 월별 폴더로 옮겨");
    // The answer was not interrupted and no second command started.
    await expect(page.locator('[data-testid="assistant-card-plan"], [data-testid="assistant-card-clarify"]')).toHaveCount(0);
    await page.evaluate(() => localStorage.setItem("doculens.mock.fast", "1"));
    await expect(answer).toHaveAttribute("data-answer-state", "done", { timeout: 30_000 });

    // Still in the composer once the answer is done; Enter sends it.
    await expect(page.getByTestId("assistant-input")).toHaveValue("견적서 월별 폴더로 옮겨");
    await page.getByTestId("assistant-input").press("Enter");
    await expect(page.getByTestId("assistant-input")).toHaveValue("");
    await expect(page.getByTestId("assistant-messages")).toContainText("견적서 월별 폴더로 옮겨");
  });

  test("the input waits for the sessions and the composer sends into the active one", async ({ page }) => {
    await freshApp(page);
    await createWorkspace(page);
    await openAssistantScreen(page);
    await expect(page.getByTestId("assistant-send")).toBeEnabled();
    await send(page, "/테마 라이트");
    await expect(page.locator('[data-testid="assistant-card-executed"]').last()).toBeVisible();
    await expect(activeRow(page)).toContainText("/테마 라이트");
    await expect(activeRow(page)).toContainText("2");
  });
});
