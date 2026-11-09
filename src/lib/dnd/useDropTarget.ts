import { useEffect, useMemo, useRef } from "react";

import { registerDropTarget, useDndStore, type DropTargetHandlers } from "./store";

export interface DropTargetState {
  /** Spread onto the element that should receive drops (`data-doc-drop`, `data-drop-over`, `data-drop-active`). */
  props: { "data-doc-drop": string; "data-drop-over": boolean | undefined; "data-drop-active": boolean | undefined };
  /** A compatible drag hovers this target. */
  isOver: boolean;
  /** A compatible drag is in progress anywhere (show the dashed "drop here" affordance). */
  isActive: boolean;
}

/**
 * Registers a drop target for in-app document drags and / or OS file drops. Handlers are read
 * through a ref, so they may close over the latest render without re-registering.
 */
export function useDocumentDropTarget(handlers: DropTargetHandlers): DropTargetState {
  const ref = useRef(handlers);
  ref.current = handlers;
  const reg = useMemo(() => registerDropTarget(ref), []);
  useEffect(() => () => reg.unregister(), [reg]);
  const isOver = useDndStore((s) => s.overTarget === reg.id);
  const isActive = useDndStore((s) => !handlers.disabled && ((s.dragging !== null && !!handlers.onDocuments) || (s.nativeDragging && !!handlers.onFiles)));
  return {
    props: { "data-doc-drop": reg.id, "data-drop-over": isOver || undefined, "data-drop-active": isActive || undefined },
    isOver,
    isActive,
  };
}
