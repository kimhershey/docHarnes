import { create } from "zustand";

import { api, subscribe } from "@/lib/ipc";
import { invalidateProjects, projectKeys } from "@/lib/queries";
import { queryClient } from "@/lib/query-client";
import { EVENTS, type Project, type ProjectsChanged } from "@/lib/types";

/** Projects screen state that survives navigation plus the `projects://changed` wiring (§3.15). */
interface ProjectsState {
  initialized: boolean;
  view: "list" | "kanban";
  /**
   * The project opened last on the detail route — the assistant's `current_project` context ("정합성 검사해"
   * targets it). Kept after leaving the page; cleared only by `resetProjects()` (workspace switch), the
   * project's deletion — wherever it happens, see `reconcileCurrentProject` — or the chip's clear button on
   * the assistant screen.
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
    void reconcileCurrentProject();
  }, 150);
}

/**
 * Keeps the project context true to the project list after a `projects://changed`: a project deleted anywhere
 * but on its own page — by the assistant's `delete_project`, from another window — used to stay the assistant's
 * "프로젝트: …" context and the target of files dropped on the 프로젝트 menu entry (PRJ-16); a rename is picked up
 * the same way. A failed list call (no workspace, backend error) changes nothing: the next event checks again.
 */
export async function reconcileCurrentProject(list: () => Promise<Project[]> = () => api.projectsList()): Promise<void> {
  const current = useProjectsStore.getState().currentProject;
  if (!current) return;
  let projects: Project[];
  try {
    // The same query as the project list: the refetch the invalidation started is shared, not repeated.
    projects = await queryClient.fetchQuery({ queryKey: projectKeys.list, queryFn: list, staleTime: 0 });
    // A fetch that was already under way may predate a project created since: ask once more before dropping it.
    if (!projects.some((p) => p.id === current)) projects = await list();
  } catch {
    return;
  }
  const state = useProjectsStore.getState();
  if (state.currentProject !== current) return;
  const found = projects.find((p) => p.id === current);
  if (!found) state.clearCurrentProject(current);
  else if (found.name !== state.currentProjectName) state.setCurrentProject(current, found.name);
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

/**
 * A document changed — imported, removed, classified, extracted, reviewed (`documents://changed` /
 * `pipeline://changed`). The project screens show the documents' type, status and review state, the checklist
 * follows the types and the cards count the documents, so the project list and the open details are refetched
 * with the library (PRJ-10: after a file was dropped on a project its type stayed "—" and the checklist "누락"
 * until the screen was left and opened again). A project's 참고 자료 rows come from its reference group, so the
 * groups' document lists follow too. Runs and reports are stored results: they change with a check, not here.
 * Called from the two stores' debounced refresh, so a burst of events is one refetch.
 */
export function refreshProjectsForDocuments(): void {
  void queryClient.invalidateQueries({ queryKey: projectKeys.list });
  void queryClient.invalidateQueries({ queryKey: ["projects", "detail"] });
  void queryClient.invalidateQueries({ predicate: (q) => q.queryKey[0] === "knowledge" && q.queryKey[1] === "collections" && q.queryKey[3] === "docs" });
}

/** Workspace switch: the open project belongs to the old workspace. */
export function resetProjects(): void {
  useProjectsStore.setState({ currentProject: null, currentProjectName: null });
  invalidateProjects();
}
