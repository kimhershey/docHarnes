// Workspace store: after a switch no query cache may still hold rows of the previous workspace
// (review items, forms, journal batches … whose ids the new workspace rejects).
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "@/lib/ipc";
import { generateKeys, journalKeys, maintenanceKey, projectKeys } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import type { ChatSessionSummary, SchemaDiscoveryOutcome, WorkspaceView } from "@/lib/types";

import { resetAssistant, useAssistantStore } from "./assistant";
import { usePipelineStore } from "./pipeline";
import { useSchemasStore } from "./schemas";
import { resetWorkspaceQueries, useWorkspaceStore } from "./workspace";

/** One key of every query family that reads the open workspace — and one nobody has written yet. */
const scoped: (readonly unknown[])[] = [
  queryKeys.documentsList({}),
  queryKeys.documentsStats,
  queryKeys.collections,
  queryKeys.knowledgeStatus,
  queryKeys.pipelineStatus,
  queryKeys.docTypes,
  queryKeys.schemas,
  queryKeys.reviewQueue({ status: "pending" }),
  queryKeys.fields("doc-1", null),
  journalKeys.batches(100),
  journalKeys.entries("batch-1"),
  maintenanceKey,
  projectKeys.list,
  projectKeys.detail("project-1"),
  projectKeys.templates,
  generateKeys.drafts,
  generateKeys.templates,
  queryKeys.assessment,
  queryKeys.registry,
  queryKeys.modelIndex,
  ["a-query-family-added-later", "list"],
];

/** Facts about the PC and the installation: the same in every workspace. */
const machine: (readonly unknown[])[] = [queryKeys.appInfo, queryKeys.hardware, queryKeys.engineBinary, queryKeys.pdfium];

const fromPreviousWorkspace = { from: "workspace A" };

function seed(): void {
  for (const key of [...scoped, ...machine]) queryClient.setQueryData(key, fromPreviousWorkspace);
}

function expectOnlyMachineDataLeft(): void {
  for (const key of scoped) expect(queryClient.getQueryData(key), JSON.stringify(key)).toBeUndefined();
  for (const key of machine) expect(queryClient.getQueryData(key), JSON.stringify(key)).toEqual(fromPreviousWorkspace);
}

beforeEach(() => queryClient.clear());

describe("workspace switch", () => {
  it("drops every workspace-scoped query result and keeps the machine-level ones", () => {
    seed();
    resetWorkspaceQueries();
    expectOnlyMachineDataLeft();
  });

  it("does so on create, open and close", async () => {
    const store = useWorkspaceStore.getState();
    const a = await store.create("C:\\Work\\A");
    seed();
    await store.create("C:\\Work\\B");
    expectOnlyMachineDataLeft();
    expect(useWorkspaceStore.getState().current?.path).toBe("C:\\Work\\B");

    // The mock backend keeps its workspace list in localStorage (absent under node): answer the open call directly.
    const open = vi.spyOn(api, "workspaceOpen").mockResolvedValue(a);
    seed();
    await store.open("C:\\Work\\A");
    expect(open).toHaveBeenCalledWith("C:\\Work\\A", false);
    expectOnlyMachineDataLeft();
    expect(useWorkspaceStore.getState().current?.path).toBe("C:\\Work\\A");
    open.mockRestore();

    seed();
    await store.close();
    expectOnlyMachineDataLeft();
    expect(useWorkspaceStore.getState().current).toBeNull();
  });

  it("forgets ids of the previous workspace that live outside the query cache", async () => {
    usePipelineStore.getState().openReviewForDocument("doc-of-workspace-a");
    useSchemasStore.getState().openDiscovery({ job_id: "job-of-workspace-a" } as unknown as SchemaDiscoveryOutcome);
    await useWorkspaceStore.getState().create("C:\\Work\\C");
    expect(usePipelineStore.getState().reviewDocument).toBeNull();
    expect(useSchemasStore.getState().pending).toBeNull();
  });

  it("keeps the caches when the switch fails", async () => {
    const open = vi.spyOn(api, "workspaceOpen").mockRejectedValue({ code: "invalid_workspace", message: "gone" });
    seed();
    await expect(useWorkspaceStore.getState().open("E:\\unplugged")).rejects.toMatchObject({ code: "invalid_workspace" });
    for (const key of scoped) expect(queryClient.getQueryData(key), JSON.stringify(key)).toEqual(fromPreviousWorkspace);
    open.mockRestore();
  });

  // KNO-9: the plan editor's keystrokes are written a moment after the last one. A switch in that moment sent
  // them to the other workspace's database (or dropped them); they go out before the backend is told to switch.
  it("sends the chat writes that still wait before the backend opens, creates or closes a workspace", async () => {
    const calls: string[] = [];
    const session: ChatSessionSummary = { id: "s1", title: "", title_auto: true, created_at: "2026-11-11T00:00:00Z", updated_at: "2026-11-11T00:00:00Z", message_count: 0 };
    const view = { path: "C:\\Work\\B" } as WorkspaceView;
    const spies = [
      vi.spyOn(api, "chatSessionsList").mockResolvedValue([session]),
      vi.spyOn(api, "chatMessagesList").mockResolvedValue([]),
      vi.spyOn(api, "chatMessagePut").mockImplementation(async (_sid, _seq, payload) => {
        // The write takes a moment: the switch must wait for it, not merely start it.
        await new Promise((r) => setTimeout(r, 5));
        calls.push(`put ${(payload as { text?: string }).text ?? ""}`);
      }),
      vi.spyOn(api, "workspaceOpen").mockImplementation(async () => (calls.push("open"), view)),
      vi.spyOn(api, "workspaceCreate").mockImplementation(async () => (calls.push("create"), view)),
      vi.spyOn(api, "workspaceClose").mockImplementation(async () => void calls.push("close")),
    ];
    const typed = async (text: string) => {
      resetAssistant();
      useAssistantStore.getState().bind("C:\\Work\\A");
      for (let i = 0; i < 10 && !useAssistantStore.getState().loaded.s1; i++) await new Promise((r) => setTimeout(r, 0));
      const s = useAssistantStore.getState();
      const id = s.push("s1", { role: "user", text: "초안" });
      // Let the push itself be stored, then leave a deferred edit waiting (written 300 ms later on its own).
      await new Promise((r) => setTimeout(r, 20));
      calls.length = 0;
      s.patch("s1", id, (m) => (m.role === "user" ? { ...m, text } : m), { defer: true });
    };
    try {
      await typed("닫기 전");
      await useWorkspaceStore.getState().close();
      expect(calls).toEqual(["put 닫기 전", "close"]);

      await typed("열기 전");
      await useWorkspaceStore.getState().open("C:\\Work\\B");
      expect(calls).toEqual(["put 열기 전", "open"]);

      await typed("만들기 전");
      await useWorkspaceStore.getState().create("C:\\Work\\B");
      expect(calls).toEqual(["put 만들기 전", "create"]);
    } finally {
      for (const spy of spies) spy.mockRestore();
      resetAssistant();
    }
  });
});
