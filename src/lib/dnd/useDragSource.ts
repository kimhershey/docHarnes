import { useCallback, useRef, type PointerEvent as ReactPointerEvent } from "react";

import { beginDocDrag, type DragDoc } from "./store";

const THRESHOLD_PX = 6;

/**
 * Pointer-down handler of a document drag source. The drag starts after the pointer moved
 * `THRESHOLD_PX` with the button held, so plain clicks on the element (open, checkbox) still work;
 * `DndLayer` tracks the pointer from there and suppresses the click that ends a drag.
 */
export function startDocumentDrag(e: ReactPointerEvent<HTMLElement>, getDocs: () => DragDoc[]): void {
  if (e.button !== 0 || e.pointerType === "touch") return;
  // Text inputs, links and range selections inside the element keep their native behaviour.
  const target = e.target as HTMLElement | null;
  if (target?.closest("input[type=text], textarea, [contenteditable=true], a[href]")) return;
  const start = { x: e.clientX, y: e.clientY };
  const cleanup = () => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", cleanup);
    window.removeEventListener("pointercancel", cleanup);
  };
  const onMove = (ev: PointerEvent) => {
    if (Math.hypot(ev.clientX - start.x, ev.clientY - start.y) < THRESHOLD_PX) return;
    cleanup();
    const docs = getDocs();
    if (docs.length === 0) return;
    ev.preventDefault();
    beginDocDrag(docs, { x: ev.clientX, y: ev.clientY });
  };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", cleanup);
  window.addEventListener("pointercancel", cleanup);
}

/** Hook form of `startDocumentDrag` for components that render one document. */
export function useDocumentDragSource(getDocs: () => DragDoc[]): { onPointerDown: (e: ReactPointerEvent<HTMLElement>) => void } {
  const ref = useRef(getDocs);
  ref.current = getDocs;
  const onPointerDown = useCallback((e: ReactPointerEvent<HTMLElement>) => startDocumentDrag(e, () => ref.current()), []);
  return { onPointerDown };
}
