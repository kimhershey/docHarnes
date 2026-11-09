import { Files } from "lucide-react";
import { useEffect } from "react";
import { useTranslation } from "react-i18next";

import { isTauri } from "@/lib/ipc";
import { cn } from "@/lib/utils";

import { endDocDrag, ghostLabel, moveDocDrag, nativeDragLeave, nativeDragOver, nativeDrop, useDndStore } from "./store";

/**
 * Mounted once in the app shell: listens for pointer moves while a document drag is active,
 * for native file drags (Tauri) or HTML5 file drags (browser preview), and draws the drag ghost.
 */
export function DndLayer() {
  const { t } = useTranslation();
  const dragging = useDndStore((s) => s.dragging);
  const pointer = useDndStore((s) => s.pointer);
  const overTarget = useDndStore((s) => s.overTarget);

  // In-app drag: follow the pointer on the window (the source element may unmount when a nav item spring-loads).
  useEffect(() => {
    if (!dragging) return undefined;
    const onMove = (e: PointerEvent) => moveDocDrag(e.clientX, e.clientY);
    const onUp = () => {
      endDocDrag(true);
      // The pointerup also produces a click on whatever is under the pointer: swallow that one.
      const swallow = (ev: MouseEvent) => {
        ev.stopPropagation();
        ev.preventDefault();
      };
      window.addEventListener("click", swallow, { capture: true, once: true });
      setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
    };
    const onCancel = () => endDocDrag(false);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") endDocDrag(false);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("blur", onCancel);
    window.addEventListener("keydown", onKey);
    document.body.classList.add("dnd-dragging");
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("blur", onCancel);
      window.removeEventListener("keydown", onKey);
      document.body.classList.remove("dnd-dragging");
    };
  }, [dragging]);

  // Native file drags. Tauri reports physical pixels relative to the webview; the browser preview only has file names.
  useEffect(() => {
    if (isTauri) {
      let unlisten: (() => void) | null = null;
      let disposed = false;
      void import("@tauri-apps/api/webview")
        .then(({ getCurrentWebview }) =>
          getCurrentWebview().onDragDropEvent((event) => {
            const p = event.payload;
            const scale = window.devicePixelRatio || 1;
            if (p.type === "enter" || p.type === "over") nativeDragOver(p.position.x / scale, p.position.y / scale);
            else if (p.type === "leave") nativeDragLeave();
            else if (p.type === "drop") nativeDrop(p.position.x / scale, p.position.y / scale, p.paths);
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
    const hasFiles = (e: DragEvent) => !!e.dataTransfer && Array.from(e.dataTransfer.types).includes("Files");
    const onDragOver = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      nativeDragOver(e.clientX, e.clientY);
    };
    const onDragLeave = (e: DragEvent) => {
      if (e.relatedTarget === null) nativeDragLeave();
    };
    const onDrop = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      const names = Array.from(e.dataTransfer?.files ?? []).map((f) => f.name);
      nativeDrop(e.clientX, e.clientY, names);
    };
    window.addEventListener("dragover", onDragOver);
    window.addEventListener("dragleave", onDragLeave);
    window.addEventListener("drop", onDrop);
    return () => {
      window.removeEventListener("dragover", onDragOver);
      window.removeEventListener("dragleave", onDragLeave);
      window.removeEventListener("drop", onDrop);
    };
  }, []);

  if (!dragging || !pointer) return null;
  return (
    <div
      className={cn(
        "pointer-events-none fixed z-[1000] flex max-w-72 items-center gap-2 rounded-md border px-2.5 py-1.5 text-xs shadow-lg",
        overTarget ? "border-accent bg-accent text-accent-fg" : "border-border bg-bg-elevated text-fg",
      )}
      style={{ left: pointer.x + 12, top: pointer.y + 12 }}
      role="status"
      aria-live="polite"
      data-testid="dnd-ghost"
      data-over={overTarget ? "true" : "false"}
    >
      <Files className="size-3.5 shrink-0" aria-hidden="true" />
      <span className="truncate">{ghostLabel(dragging, (count) => t("dnd.ghost_many", { count }))}</span>
      {dragging.length > 1 && <span className="rounded-full bg-bg/30 px-1.5 tabular-nums">{dragging.length}</span>}
    </div>
  );
}
