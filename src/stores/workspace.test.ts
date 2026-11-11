// Workspace store: after a switch no query cache may still hold rows of the previous workspace
// (review items, forms, journal batches … whose ids the new workspace rejects).
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "@/lib/ipc";
import { generateKeys, journalKeys, maintenanceKey, projectKeys } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import type { SchemaDiscoveryOutcome } from "@/lib/types";

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
});
