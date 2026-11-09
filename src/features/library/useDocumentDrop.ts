import { useEffect, useRef } from "react";

import { registerFallbackFileHandler, useDndStore } from "@/lib/dnd/store";

/**
 * Window-level file drops that no drop target claimed (the library's import dialog). The native
 * / browser listeners live in `DndLayer`; this only registers the fallback and reports whether a
 * file drag hovers the window outside every element target (the page shows its overlay then).
 */
export function useDocumentDrop(enabled: boolean, onPaths: (paths: string[]) => void): boolean {
  const cb = useRef(onPaths);
  cb.current = onPaths;
  useEffect(() => {
    if (!enabled) return undefined;
    return registerFallbackFileHandler((paths) => cb.current(paths));
  }, [enabled]);
  return useDndStore((s) => enabled && s.nativeDragging && s.overTarget === null);
}
