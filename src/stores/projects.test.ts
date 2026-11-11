// Projects store: the last opened project stays the assistant's project context until the
// workspace switches, the project is deleted or the chip is cleared.
import { beforeEach, describe, expect, it, vi } from "vitest";

import { queryClient } from "@/lib/query-client";
import type { Project, ProjectsChanged } from "@/lib/types";

// The store listens to `projects://changed` and reads the project list through the IPC layer.
const backend = { projects: [] as Project[], fail: false, calls: 0 };
const handlers: ((ev: ProjectsChanged) => void)[] = [];
vi.mock("@/lib/ipc", () => ({
  api: {
    projectsList: async () => {
      backend.calls += 1;
      if (backend.fail) throw { code: "workspace", message: "no workspace" };
      return structuredClone(backend.projects);
    },
  },
  subscribe: async (_event: string, handler: (ev: ProjectsChanged) => void) => {
    handlers.push(handler);
    return () => undefined;
  },
}));

const { reconcileCurrentProject, resetProjects, useProjectsStore } = await import("./projects");

const project = (id: string, name: string) => ({ id, name, phase: "active", document_count: 0 }) as Project;
const state = () => useProjectsStore.getState();

beforeEach(() => {
  resetProjects();
  queryClient.clear();
  backend.projects = [project("p1", "인증 심사 A"), project("p2", "거래처 등록")];
  backend.fail = false;
  backend.calls = 0;
});

describe("projects store", () => {
  it("keeps the last opened project and its name", () => {
    useProjectsStore.getState().setCurrentProject("p1", "인증 심사 A");
    expect(useProjectsStore.getState()).toMatchObject({ currentProject: "p1", currentProjectName: "인증 심사 A" });
    // A re-set without a name keeps the known one; another project replaces it.
    useProjectsStore.getState().setCurrentProject("p1");
    expect(useProjectsStore.getState().currentProjectName).toBe("인증 심사 A");
    useProjectsStore.getState().setCurrentProject("p2", "거래처 등록");
    expect(useProjectsStore.getState()).toMatchObject({ currentProject: "p2", currentProjectName: "거래처 등록" });
  });

  it("clears the context only for the deleted project, for the chip and on a workspace switch", () => {
    useProjectsStore.getState().setCurrentProject("p1", "A");
    useProjectsStore.getState().clearCurrentProject("other");
    expect(useProjectsStore.getState().currentProject).toBe("p1");
    useProjectsStore.getState().clearCurrentProject("p1");
    expect(useProjectsStore.getState()).toMatchObject({ currentProject: null, currentProjectName: null });
    useProjectsStore.getState().setCurrentProject("p1", "A");
    useProjectsStore.getState().clearCurrentProject();
    expect(useProjectsStore.getState().currentProject).toBeNull();
    useProjectsStore.getState().setCurrentProject("p1", "A");
    resetProjects();
    expect(useProjectsStore.getState().currentProject).toBeNull();
  });
});

describe("the project context follows the project list (PRJ-16)", () => {
  it("drops a project that no longer exists — deleted by the assistant's plan, not on its own page", async () => {
    state().setCurrentProject("p1", "인증 심사 A");
    backend.projects = [project("p2", "거래처 등록")];
    await reconcileCurrentProject();
    expect(state()).toMatchObject({ currentProject: null, currentProjectName: null });
    // The list is asked a second time before the context is dropped (a fetch under way may predate the project).
    expect(backend.calls).toBe(2);
  });

  it("keeps an existing project and picks up its new name", async () => {
    state().setCurrentProject("p1", "인증 심사 A");
    await reconcileCurrentProject();
    expect(state()).toMatchObject({ currentProject: "p1", currentProjectName: "인증 심사 A" });
    expect(backend.calls).toBe(1);
    backend.projects = [project("p1", "인증 심사 A (2차)"), project("p2", "거래처 등록")];
    await reconcileCurrentProject();
    expect(state()).toMatchObject({ currentProject: "p1", currentProjectName: "인증 심사 A (2차)" });
  });

  it("changes nothing when the list cannot be read, and asks nothing without a context", async () => {
    await reconcileCurrentProject();
    expect(backend.calls).toBe(0);
    state().setCurrentProject("p1", "인증 심사 A");
    backend.fail = true;
    await reconcileCurrentProject();
    expect(state().currentProject).toBe("p1");
  });

  it("does not drop a project that was opened while the list was being read", async () => {
    state().setCurrentProject("p1", "인증 심사 A");
    backend.projects = [];
    const pending = reconcileCurrentProject();
    state().setCurrentProject("p3", "새 프로젝트");
    await pending;
    expect(state().currentProject).toBe("p3");
  });

  it("checks after a projects://changed event (debounced)", async () => {
    vi.useFakeTimers();
    try {
      await state().init();
      expect(handlers.length).toBeGreaterThan(0);
      state().setCurrentProject("p1", "인증 심사 A");
      backend.projects = [project("p2", "거래처 등록")];
      for (const h of handlers) h({ project_ids: ["p1"] });
      expect(state().currentProject).toBe("p1");
      await vi.advanceTimersByTimeAsync(200);
      await vi.waitFor(() => expect(state().currentProject).toBeNull());
    } finally {
      vi.useRealTimers();
    }
  });
});
