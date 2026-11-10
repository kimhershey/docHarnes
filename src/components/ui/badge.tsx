import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

// `max-w-full`: a badge never grows past its box; a long name inside it goes in a `min-w-0 truncate` span (0.7.0 plan §1.7).
const badgeVariants = cva("inline-flex max-w-full items-center gap-1 rounded-full border px-2 py-0.5 text-xs font-medium whitespace-nowrap", {
  variants: {
    variant: {
      default: "border-transparent bg-accent-soft text-fg",
      ok: "border-transparent bg-ok-soft text-ok",
      warn: "border-transparent bg-warn-soft text-warn",
      danger: "border-transparent bg-danger-soft text-danger",
      info: "border-transparent bg-info-soft text-info",
      outline: "border-border-strong text-fg-muted",
      muted: "border-transparent bg-bg-sunken text-fg-muted",
    },
  },
  defaultVariants: { variant: "default" },
});

export interface BadgeProps extends React.HTMLAttributes<HTMLSpanElement>, VariantProps<typeof badgeVariants> {}

function Badge({ className, variant, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ variant }), className)} {...props} />;
}

export { Badge, badgeVariants };
