import { create } from "zustand";

import { subscribe } from "@/lib/ipc";
import { invalidateProjects } from "@/lib/queries";
import { EVENTS, type ProjectsChanged } from "@/lib/types";

/** Projects screen state that survives navigation plus the `projects://changed` wiring (§3.15). */
interface ProjectsState {
  initialized: boolean;
  view: "list" | "kanban";
  /** Project open on the detail route — the assistant's `current_project` context. */
  currentProject: string | null;
  init: () => Promise<void>;
  setView: (view: "list" | "kanban") => void;
  setCurrentProject: (id: string | null) => void;
}

let timer: ReturnType<typeof setTimeout> | null = null;
let pendingIds = new Set<string>();

/** Coalesces bursts of `projects://changed` into one refetch. */
function scheduleInvalidate(ids: string[]) {
  for (const id of ids) pendingIds.add(id);
  if (timer) return;
  timer = setTimeout(() => {
    timer = null;
    const ids = [...pendingIds];
    pendingIds = new Set();
    invalidateProjects(ids);
  }, 150);
}

export const useProjectsStore = create<ProjectsState>((set, get) => ({
  initialized: false,
  view: "kanban",
  currentProject: null,

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });
    await subscribe<ProjectsChanged>(EVENTS.projectsChanged, (ev) => scheduleInvalidate(ev.project_ids ?? []));
  },

  setView: (view) => set({ view }),
  setCurrentProject: (id) => set({ currentProject: id }),
}));

/** Workspace switch: the open project belongs to the old workspace. */
export function resetProjects(): void {
  useProjectsStore.setState({ currentProject: null });
  invalidateProjects();
}
