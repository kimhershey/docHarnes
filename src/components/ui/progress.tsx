import { cn } from "@/lib/utils";

/** Minimal determinate/indeterminate progress bar (0..1). */
export function Progress({ value, className, label }: { value: number | null | undefined; className?: string; label?: string }) {
  const pct = value === null || value === undefined ? null : Math.max(0, Math.min(100, Math.round(value * 100)));
  return (
    <div
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={pct ?? undefined}
      aria-label={label}
      className={cn("relative h-2 w-full overflow-hidden rounded-full bg-bg-sunken", className)}
    >
      {pct === null ? (
        <div className="absolute inset-y-0 w-1/3 animate-pulse rounded-full bg-accent/60" />
      ) : (
        <div className="h-full rounded-full bg-accent transition-[width] duration-200" style={{ width: `${pct}%` }} />
      )}
    </div>
  );
}
