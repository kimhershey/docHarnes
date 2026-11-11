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

/**
 * Toasts sit at the top, over the top bar's status badges (job count, model): in the bottom right corner they
 * covered the assistant's send button, the action buttons of a table's last row and dialog footers for the seconds
 * they stay (0.7.0 audit UI-11). A single one-line toast is about as tall as the top bar, so it hides nothing of
 * the screen below; a stack grows downwards. `right-[5.5rem]` leaves the theme switch at the bar's right end
 * (two icon buttons, 4.25 rem, plus the bar's padding) uncovered — on the onboarding screen as well.
 */
export function Toaster() {
  const toasts = useUiStore((s) => s.toasts);
  const dismiss = useUiStore((s) => s.dismissToast);
  return (
    <div className="pointer-events-none fixed right-[5.5rem] top-1 z-[60] flex w-96 max-w-[calc(100vw-6rem)] flex-col gap-2" aria-live="polite" aria-atomic="false" data-testid="toaster">
      {toasts.map((t) => {
        const Icon = ICONS[t.kind];
        return (
          <div key={t.id} className={cn("pointer-events-auto flex items-start gap-2 rounded-md border-l-4 border border-border bg-bg-elevated p-3 text-sm text-fg shadow-[var(--shadow)]", COLORS[t.kind])} role="status">
            <Icon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            <span className="flex-1 text-fg selectable">{t.message}</span>
            {t.action && (
              <button
                type="button"
                className="shrink-0 rounded-md border border-border px-2 py-0.5 text-xs text-fg hover:bg-bg-sunken cursor-pointer"
                onClick={() => {
                  t.action?.onClick();
                  dismiss(t.id);
                }}
                data-testid="toast-action"
              >
                {t.action.label}
              </button>
            )}
            <button type="button" className="text-fg-muted hover:text-fg cursor-pointer" onClick={() => dismiss(t.id)} aria-label="dismiss">
              <X className="size-4" />
            </button>
          </div>
        );
      })}
    </div>
  );
}
