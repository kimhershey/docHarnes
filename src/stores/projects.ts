import { create } from "zustand";

import { subscribe } from "@/lib/ipc";
import { invalidateProjects } from "@/lib/queries";
import { EVENTS, type ProjectsChanged } from "@/lib/types";

/** Projects screen state that survives navigation plus the `projects://changed` wiring (§3.15). */
interface ProjectsState {
  initialized: boolean;
  view: "list" | "kanban";
  /**
   * The project opened last on the detail route — the assistant's `current_project` context ("정합성 검사해"
   * targets it). Kept after leaving the page; cleared only by `resetProjects()` (workspace switch), the
   * project's deletion or the chip's clear button on the assistant screen.
   */
  currentProject: string | null;
  /** Name shown in the assistant's "프로젝트: …" chip. */
  currentProjectName: string | null;
  init: () => Promise<void>;
  setView: (view: "list" | "kanban") => void;
  setCurrentProject: (id: string | null, name?: string | null) => void;
  /** Drops the project context when it is the given project (deleted). */
  clearCurrentProject: (id?: string | null) => void;
}

let timer: ReturnType<typeof setTimeout> | null = null;
let pendingIds = new Set<string>();
/** A `project_ids: []` event means "everything changed": kept apart from the ids so it is not lost in a burst. */
let pendingAll = false;

/** Coalesces bursts of `projects://changed` into one refetch. */
function scheduleInvalidate(ids: string[]) {
  if (ids.length === 0) pendingAll = true;
  for (const id of ids) pendingIds.add(id);
  if (timer) return;
  timer = setTimeout(() => {
    timer = null;
    const all = pendingAll;
    const ids = [...pendingIds];
    pendingAll = false;
    pendingIds = new Set();
    invalidateProjects(all ? [] : ids);
  }, 150);
}

export const useProjectsStore = create<ProjectsState>((set, get) => ({
  initialized: false,
  view: "kanban",
  currentProject: null,
  currentProjectName: null,

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });
    await subscribe<ProjectsChanged>(EVENTS.projectsChanged, (ev) => scheduleInvalidate(ev.project_ids ?? []));
  },

  setView: (view) => set({ view }),
  setCurrentProject: (id, name = null) => set((s) => ({ currentProject: id, currentProjectName: id ? (name ?? (s.currentProject === id ? s.currentProjectName : null)) : null })),
  clearCurrentProject: (id = null) => set((s) => (id === null || s.currentProject === id ? { currentProject: null, currentProjectName: null } : s)),
}));

/** Workspace switch: the open project belongs to the old workspace. */
export function resetProjects(): void {
  useProjectsStore.setState({ currentProject: null, currentProjectName: null });
  invalidateProjects();
}
