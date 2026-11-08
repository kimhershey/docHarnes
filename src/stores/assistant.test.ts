// Assistant conversation store: messages survive navigation, the owed
// `ui_actions` of a job-backed plan are claimed exactly once.
import { beforeEach, describe, expect, it } from "vitest";

import type { Op } from "@/lib/types";

import { resetAssistant, useAssistantStore } from "./assistant";

const theme: Op = { op: "set_theme", theme: "dark" };
const open: Op = { op: "open", target: "screen", ref: "jobs" };

beforeEach(() => resetAssistant());

describe("assistant store", () => {
  it("pushes and patches messages with increasing ids", () => {
    const s = useAssistantStore.getState();
    const a = s.push({ role: "user", text: "inbox 처리" });
    const b = s.push({ role: "assistant", kind: "help" });
    expect(b).toBeGreaterThan(a);
    expect(useAssistantStore.getState().messages.map((m) => m.id)).toEqual([a, b]);
    s.patch(a, (m) => (m.role === "user" ? { ...m, text: "edited" } : m));
    expect(useAssistantStore.getState().messages[0]).toMatchObject({ id: a, text: "edited" });
    s.clear();
    expect(useAssistantStore.getState().messages).toEqual([]);
  });

  it("claims the ui_actions of a tracked job exactly once", () => {
    const s = useAssistantStore.getState();
    s.trackJob("job1", [theme]);
    // The execution's list wins over the provisional plan ops.
    expect(useAssistantStore.getState().claimUiActions("job1", [theme, open])).toEqual([theme, open]);
    expect(useAssistantStore.getState().claimUiActions("job1", [theme, open])).toBeNull();
    expect(useAssistantStore.getState().claimUiActions("job1")).toBeNull();
    expect(useAssistantStore.getState().pendingUiActions).toEqual({});
  });

  it("falls back to the tracked plan ops and ignores unknown jobs", () => {
    const s = useAssistantStore.getState();
    s.trackJob("job2", [open]);
    expect(useAssistantStore.getState().claimUiActions("unknown")).toBeNull();
    expect(useAssistantStore.getState().claimUiActions("job2")).toEqual([open]);
    expect(useAssistantStore.getState().claimUiActions("job2")).toBeNull();
  });

  it("keeps owed actions across 새 대화 but drops them with the workspace", () => {
    const s = useAssistantStore.getState();
    s.bind("C:\\Work\\Docs");
    s.push({ role: "user", text: "x" });
    s.trackJob("job3", [theme]);
    s.clear();
    expect(useAssistantStore.getState().messages).toEqual([]);
    expect(useAssistantStore.getState().pendingUiActions).toEqual({ job3: [theme] });
    // Same workspace: nothing changes; another one starts afresh.
    useAssistantStore.getState().bind("C:\\Work\\Docs");
    expect(useAssistantStore.getState().pendingUiActions).toEqual({ job3: [theme] });
    useAssistantStore.getState().bind("D:\\Other");
    expect(useAssistantStore.getState().pendingUiActions).toEqual({});
    expect(useAssistantStore.getState().workspace).toBe("D:\\Other");
    useAssistantStore.getState().trackJob("job4", [open]);
    resetAssistant();
    expect(useAssistantStore.getState()).toMatchObject({ workspace: null, messages: [], pendingUiActions: {} });
  });
});
