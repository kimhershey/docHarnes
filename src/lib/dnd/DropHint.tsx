import { Download } from "lucide-react";
import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * Affordance drawn inside a drop target while a compatible drag is in progress: a dashed frame
 * that fills the target, highlighted when hovered. Render it as the last child of a
 * `relative` container.
 */
export function DropHint({ active, over, children, className, testId }: { active: boolean; over: boolean; children: ReactNode; className?: string; testId?: string }) {
  if (!active) return null;
  return (
    <div
      className={cn(
        "pointer-events-none absolute inset-0 z-10 flex items-center justify-center gap-2 rounded-md border-2 border-dashed px-3 text-center text-xs font-medium transition-colors",
        over ? "border-accent bg-accent-soft/90 text-accent" : "border-border bg-bg/70 text-fg-muted",
        className,
      )}
      data-testid={testId}
      data-over={over ? "true" : "false"}
      aria-hidden="true"
    >
      <Download className="size-4 shrink-0" />
      <span>{children}</span>
    </div>
  );
}
