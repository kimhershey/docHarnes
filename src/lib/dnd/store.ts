// Document drag-and-drop (design §3.4 "끌어다 놓기", v1.1 follow-up): one registry of drop targets
// shared by three input paths —
//   1. in-app drags of library documents (pointer events, not HTML5 DnD: Tauri's `dragDropEnabled`
//      breaks HTML5 drags on Windows and we need it for native file drops),
//   2. native OS file drops (Tauri `onDragDropEvent` with a window position → hit-test),
//   3. the browser preview's HTML5 file drops (vite / Playwright, where only file names exist).
// Targets register handlers and mark their element with `data-doc-drop="<id>"`; the layer
// (`DndLayer`) hit-tests the pointer and dispatches. Nav items use `onHover` (spring-loading:
// hovering a screen's menu entry while dragging switches to it so the drop can land inside).
import { create } from "zustand";

export interface DragDoc {
  id: string;
  name: string;
  /** Absolute path (library views carry it) — lets the ingest wizard re-register a dropped document. */
  absolute_path?: string | null;
}

export interface DropTargetHandlers {
  /** Accepts in-app document drags. */
  onDocuments?: (docs: DragDoc[]) => void;
  /** Accepts OS file drops: absolute paths in Tauri, bare file names in the browser preview. */
  onFiles?: (paths: string[]) => void;
  /** Spring-loading: fired once when a drag has hovered the target for `springMs` (default 700 ms). */
  onHover?: () => void;
  springMs?: number;
  disabled?: boolean;
}

type PayloadKind = "documents" | "files";

interface DndState {
  /** In-app document drag in progress (the dragged documents). */
  dragging: DragDoc[] | null;
  /** Whether the in-app drag moved far enough to count (suppresses the click that follows). */
  moved: boolean;
  /** A native (OS) file drag is over the window. */
  nativeDragging: boolean;
  pointer: { x: number; y: number } | null;
  /** Registered target under the pointer that accepts the current payload. */
  overTarget: string | null;
}

export const useDndStore = create<DndState>(() => ({ dragging: null, moved: false, nativeDragging: false, pointer: null, overTarget: null }));

const targets = new Map<string, { current: DropTargetHandlers }>();
let targetSeq = 0;
/** Window-level handlers for file drops that no element target claimed (the library's import dialog). */
const fallbackFileHandlers = new Set<(paths: string[]) => void>();

export function registerDropTarget(ref: { current: DropTargetHandlers }): { id: string; unregister: () => void } {
  const id = `dt${++targetSeq}`;
  targets.set(id, ref);
  return {
    id,
    unregister: () => {
      targets.delete(id);
      if (useDndStore.getState().overTarget === id) useDndStore.setState({ overTarget: null });
    },
  };
}

export function registerFallbackFileHandler(handler: (paths: string[]) => void): () => void {
  fallbackFileHandlers.add(handler);
  return () => void fallbackFileHandlers.delete(handler);
}

export function hasFallbackFileHandler(): boolean {
  return fallbackFileHandlers.size > 0;
}

function accepts(h: DropTargetHandlers | undefined, kind: PayloadKind): boolean {
  if (!h || h.disabled) return false;
  return kind === "documents" ? !!h.onDocuments : !!h.onFiles;
}

/** The innermost registered target at a viewport point that accepts `kind` (nested targets fall back to their parents). */
export function targetAt(x: number, y: number, kind: PayloadKind): string | null {
  if (typeof document === "undefined") return null;
  let el: Element | null = document.elementFromPoint(x, y);
  while (el) {
    const host = el.closest<HTMLElement>("[data-doc-drop]");
    if (!host) return null;
    const id = host.dataset.docDrop ?? "";
    if (accepts(targets.get(id)?.current, kind)) return id;
    el = host.parentElement;
  }
  return null;
}

/**
 * The innermost registered target at a viewport point that spring-loads (`onHover`), whether or not it takes
 * the payload: since 0.5.0 most sidebar entries only switch screens while a drag rests on them (plan §3).
 */
export function hoverTargetAt(x: number, y: number): string | null {
  if (typeof document === "undefined") return null;
  let el: Element | null = document.elementFromPoint(x, y);
  while (el) {
    const host = el.closest<HTMLElement>("[data-doc-drop]");
    if (!host) return null;
    const id = host.dataset.docDrop ?? "";
    const h = targets.get(id)?.current;
    if (h && !h.disabled && h.onHover) return id;
    el = host.parentElement;
  }
  return null;
}

/** Spring-loading timer for the target the pointer currently hovers. */
let hoverTimer: ReturnType<typeof setTimeout> | null = null;
let hoverTarget: string | null = null;

function clearHover() {
  if (hoverTimer) clearTimeout(hoverTimer);
  hoverTimer = null;
  hoverTarget = null;
}

/** `id` = the target that takes the payload (highlighted), `hoverId` = the one that spring-loads (may be hover-only). */
function setOver(id: string | null, hoverId: string | null = id) {
  const state = useDndStore.getState();
  if (state.overTarget !== id) useDndStore.setState({ overTarget: id });
  if (hoverTarget === hoverId) return;
  clearHover();
  const h = hoverId ? targets.get(hoverId)?.current : undefined;
  if (hoverId && h?.onHover && !h.disabled) {
    hoverTarget = hoverId;
    hoverTimer = setTimeout(() => {
      hoverTimer = null;
      targets.get(hoverId)?.current.onHover?.();
    }, h.springMs ?? 700);
  }
}

// -- in-app document drags ----------------------------------------------------------------------

export function beginDocDrag(docs: DragDoc[], pointer: { x: number; y: number }): void {
  if (docs.length === 0) return;
  useDndStore.setState({ dragging: docs, moved: true, pointer });
  // Through `setOver` so a drag that starts over a spring-loading target arms its timer at once.
  setOver(targetAt(pointer.x, pointer.y, "documents"), hoverTargetAt(pointer.x, pointer.y));
}

export function moveDocDrag(x: number, y: number): void {
  if (!useDndStore.getState().dragging) return;
  useDndStore.setState({ pointer: { x, y } });
  setOver(targetAt(x, y, "documents"), hoverTargetAt(x, y));
}

/** Ends the drag; with `drop` the hovered target receives the documents. Returns whether a target took them. */
export function endDocDrag(drop: boolean): boolean {
  const { dragging, overTarget } = useDndStore.getState();
  clearHover();
  useDndStore.setState({ dragging: null, pointer: null, overTarget: null });
  if (!dragging || !drop || !overTarget) return false;
  const h = targets.get(overTarget)?.current;
  if (!accepts(h, "documents")) return false;
  h!.onDocuments!(dragging);
  return true;
}

// -- native / browser file drags ------------------------------------------------------------------

export function nativeDragOver(x: number, y: number): void {
  useDndStore.setState({ nativeDragging: true, pointer: { x, y } });
  setOver(targetAt(x, y, "files"), hoverTargetAt(x, y));
}

export function nativeDragLeave(): void {
  clearHover();
  useDndStore.setState({ nativeDragging: false, pointer: null, overTarget: null });
}

/** Dispatches dropped files to the target under the point, else to the window-level fallback handlers. */
export function nativeDrop(x: number, y: number, paths: string[]): "target" | "fallback" | "none" {
  clearHover();
  useDndStore.setState({ nativeDragging: false, pointer: null, overTarget: null });
  if (paths.length === 0) return "none";
  const id = targetAt(x, y, "files");
  const h = id ? targets.get(id)?.current : undefined;
  if (h && accepts(h, "files")) {
    h.onFiles!(paths);
    return "target";
  }
  if (fallbackFileHandlers.size === 0) return "none";
  for (const f of fallbackFileHandlers) f(paths);
  return "fallback";
}

/** Test / reset helper. */
export function resetDnd(): void {
  clearHover();
  targets.clear();
  fallbackFileHandlers.clear();
  useDndStore.setState({ dragging: null, moved: false, nativeDragging: false, pointer: null, overTarget: null });
}

/** Label for the drag ghost: one name, or the count. */
export function ghostLabel(docs: DragDoc[], many: (count: number) => string): string {
  return docs.length === 1 ? docs[0]!.name : many(docs.length);
}
