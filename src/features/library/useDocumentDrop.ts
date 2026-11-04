import { useEffect, useState } from "react";

import { isTauri } from "@/lib/ipc";

/**
 * File drops onto the window. In Tauri the native drag-drop event carries real
 * paths; in the browser preview only file names are available (the mock
 * backend treats them as external files).
 */
export function useDocumentDrop(enabled: boolean, onPaths: (paths: string[]) => void): boolean {
  const [over, setOver] = useState(false);

  useEffect(() => {
    if (!enabled) return undefined;
    if (isTauri) {
      let unlisten: (() => void) | null = null;
      let disposed = false;
      void import("@tauri-apps/api/webview")
        .then(({ getCurrentWebview }) =>
          getCurrentWebview().onDragDropEvent((event) => {
            const p = event.payload;
            if (p.type === "enter" || p.type === "over") setOver(true);
            else if (p.type === "leave") setOver(false);
            else if (p.type === "drop") {
              setOver(false);
              if (p.paths.length > 0) onPaths(p.paths);
            }
          }),
        )
        .then((u) => {
          if (disposed) u();
          else unlisten = u;
        })
        .catch(() => undefined);
      return () => {
        disposed = true;
        unlisten?.();
      };
    }
    const onDragOver = (e: DragEvent) => {
      if (e.dataTransfer?.types.includes("Files")) {
        e.preventDefault();
        setOver(true);
      }
    };
    const onDragLeave = (e: DragEvent) => {
      if (e.relatedTarget === null) setOver(false);
    };
    const onDrop = (e: DragEvent) => {
      e.preventDefault();
      setOver(false);
      const files = Array.from(e.dataTransfer?.files ?? []);
      if (files.length > 0) onPaths(files.map((f) => f.name));
    };
    window.addEventListener("dragover", onDragOver);
    window.addEventListener("dragleave", onDragLeave);
    window.addEventListener("drop", onDrop);
    return () => {
      window.removeEventListener("dragover", onDragOver);
      window.removeEventListener("dragleave", onDragLeave);
      window.removeEventListener("drop", onDrop);
    };
  }, [enabled, onPaths]);

  return over;
}
