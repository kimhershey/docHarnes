// Projects store: the last opened project stays the assistant's project context until the
// workspace switches, the project is deleted or the chip is cleared.
import { beforeEach, describe, expect, it } from "vitest";

import { resetProjects, useProjectsStore } from "./projects";

beforeEach(() => resetProjects());

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
