// The browser mock must behave like the Rust backend for the flows the UI
// exercises (settings persistence, workspace open/create, pre-router).
import { beforeEach, describe, expect, it } from "vitest";

const store = new Map<string, string>();
// Minimal localStorage + crypto for the node environment.
(globalThis as unknown as { localStorage: Storage }).localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
  removeItem: (k: string) => void store.delete(k),
  clear: () => store.clear(),
  key: () => null,
  length: 0,
} as Storage;

const mock = await import("@/lib/mock-backend");

describe("mock backend", () => {
  beforeEach(() => mock.resetMock());

  it("persists theme and language like settings.json", async () => {
    const s = await mock.mockInvoke<{ ui: { theme: string; language: string } }>("settings_get");
    expect(s.ui.theme).toBe("system");
    expect(s.ui.language).toBe("ko");
    await mock.mockInvoke("settings_set_theme", { theme: "dark" });
    await mock.mockInvoke("settings_set_language", { language: "en" });
    const again = await mock.mockInvoke<{ ui: { theme: string; language: string } }>("settings_get");
    expect(again.ui).toMatchObject({ theme: "dark", language: "en" });
  });

  it("creates, remembers and reopens a workspace", async () => {
    const insp = await mock.mockInvoke<{ initialized: boolean }>("workspace_inspect", { path: "C:\\Work\\Docs" });
    expect(insp.initialized).toBe(false);
    const ws = await mock.mockInvoke<{ path: string; manifest: { name: string } }>("workspace_create", { args: { path: "C:\\Work\\Docs\\", name: null, locale: null, force_unlock: false } });
    expect(ws.path).toBe("C:\\Work\\Docs");
    expect(ws.manifest.name).toBe("Docs");
    const s = await mock.mockInvoke<{ active_workspace: string; workspaces: { path: string }[] }>("settings_get");
    expect(s.active_workspace).toBe("C:\\Work\\Docs");
    expect(s.workspaces[0]?.path).toBe("C:\\Work\\Docs");
    await expect(mock.mockInvoke("workspace_open", { args: { path: "D:\\nope", force_unlock: false } })).rejects.toMatchObject({ code: "invalid_workspace" });
    await expect(mock.mockInvoke("workspace_create", { args: { path: "C:\\", name: null, locale: null, force_unlock: false } })).rejects.toMatchObject({ code: "invalid_workspace" });
  });

  it("assesses the mock GPU into the 8 GB tier with the Q6_K model", async () => {
    const a = await mock.mockInvoke<{ tier: { tier: string }; recommended_model: { id: string } }>("hw_assess");
    expect(a.tier.tier).toBe("vram_8gb");
    expect(a.recommended_model.id).toBe("gemma4-e4b-q6k");
    await mock.mockInvoke("settings_set_tier_mode", { mode: "manual", manualTier: "cpu_only" });
    const b = await mock.mockInvoke<{ tier: { tier: string; basis: string }; recommended_model: { id: string } }>("hw_assess");
    expect(b.tier).toMatchObject({ tier: "cpu_only", basis: "manual_override" });
    expect(b.recommended_model.id).toBe("gemma4-e2b-q4km");
  });

  it("pre-routes slash commands and phrases like the Rust router", () => {
    expect(mock.mockRoute("/테마 다크")).toMatchObject({ matched: true, op: "set_theme", theme: "dark", source: "slash" });
    expect(mock.mockRoute("/open settings")).toMatchObject({ op: "open", screen: "settings" });
    expect(mock.mockRoute("영어로 바꿔줘")).toMatchObject({ op: "set_language", language: "en" });
    expect(mock.mockRoute("설정 열어줘")).toMatchObject({ op: "open", screen: "settings" });
    expect(mock.mockRoute("switch to dark mode")).toMatchObject({ op: "set_theme", theme: "dark" });
    expect(mock.mockRoute("지난달 영수증 합계")).toMatchObject({ matched: false });
    expect(mock.mockRoute("")).toMatchObject({ matched: false });
  });
});
