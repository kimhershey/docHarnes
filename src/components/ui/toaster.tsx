import { AlertTriangle, CheckCircle2, Info, X, XCircle } from "lucide-react";

import { cn } from "@/lib/utils";
import { useUiStore, type ToastKind } from "@/stores/ui";

const ICONS: Record<ToastKind, typeof Info> = { info: Info, ok: CheckCircle2, warn: AlertTriangle, danger: XCircle };
const COLORS: Record<ToastKind, string> = {
  info: "border-info text-info",
  ok: "border-ok text-ok",
  warn: "border-warn text-warn",
  danger: "border-danger text-danger",
};

export function Toaster() {
  const toasts = useUiStore((s) => s.toasts);
  const dismiss = useUiStore((s) => s.dismissToast);
  return (
    <div className="pointer-events-none fixed bottom-10 right-4 z-[60] flex w-96 max-w-[calc(100vw-2rem)] flex-col gap-2" aria-live="polite" aria-atomic="false" data-testid="toaster">
      {toasts.map((t) => {
        const Icon = ICONS[t.kind];
        return (
          <div key={t.id} className={cn("pointer-events-auto flex items-start gap-2 rounded-md border-l-4 border border-border bg-bg-elevated p-3 text-sm text-fg shadow-[var(--shadow)]", COLORS[t.kind])} role="status">
            <Icon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            <span className="flex-1 text-fg selectable">{t.message}</span>
            <button type="button" className="text-fg-muted hover:text-fg cursor-pointer" onClick={() => dismiss(t.id)} aria-label="dismiss">
              <X className="size-4" />
            </button>
          </div>
        );
      })}
    </div>
  );
}
