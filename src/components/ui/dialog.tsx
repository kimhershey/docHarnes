import * as React from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { X } from "lucide-react";

import { cn } from "@/lib/utils";

const Dialog = DialogPrimitive.Root;
const DialogTrigger = DialogPrimitive.Trigger;
const DialogPortal = DialogPrimitive.Portal;
const DialogClose = DialogPrimitive.Close;

const DialogOverlay = React.forwardRef<React.ElementRef<typeof DialogPrimitive.Overlay>, React.ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay>>(({ className, ...props }, ref) => (
  <DialogPrimitive.Overlay ref={ref} className={cn("fixed inset-0 z-50 bg-black/50", className)} {...props} />
));
DialogOverlay.displayName = DialogPrimitive.Overlay.displayName;

/**
 * The dialog never grows past the window (0.7.0 audit UI-3: at UI 배율 150 % the project dialog was 959 px tall in an
 * 800 px window, its 취소 / 만들기 buttons and the close button out of reach). The frame is bound to the viewport
 * less a 1 rem margin on every side and clips; the body inside it scrolls. What stays in view while it scrolls:
 * the close button (it belongs to the frame, not to the scrolling body) and the `DialogFooter` (sticky).
 * `className` goes to the frame (width caps such as `max-w-2xl`); the children are laid out by the body's grid.
 * The frame is the positioned box, the body is not: an `absolute inset-0` child (a `DropHint`) covers the whole
 * dialog whatever the scroll position; a `sr-only` child needs a `relative` wrapper of its own to scroll with the body.
 */
const DialogContent = React.forwardRef<React.ElementRef<typeof DialogPrimitive.Content>, React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content> & { closeLabel?: string }>(
  ({ className, children, closeLabel = "Close", onEscapeKeyDown, ...props }, ref) => (
    <DialogPortal>
      <DialogOverlay />
      <DialogPrimitive.Content
        ref={ref}
        className={cn(
          "fixed left-1/2 top-1/2 z-50 flex max-h-[calc(100dvh-2rem)] w-[calc(100vw-2rem)] max-w-lg -translate-x-1/2 -translate-y-1/2 flex-col overflow-clip rounded-lg border border-border bg-bg-elevated text-fg shadow-[var(--shadow)]",
          className,
        )}
        onEscapeKeyDown={(e) => {
          // Esc while a Korean IME composition is open cancels the composition, not the dialog (design §3.7.4).
          if (e.isComposing || e.keyCode === 229) {
            e.preventDefault();
            return;
          }
          onEscapeKeyDown?.(e);
        }}
        {...props}
      >
        {/* The space under the last child is a spacer (`::after`, its negative margin cancels the grid gap) and not
            padding: a sticky box stays inside the scroll box's padding, so with `pb-6` the footer would stop 1.5 rem
            above the dialog's edge. `scroll-pb`: a field scrolled into view (focus, Tab) stops above the footer. */}
        <div className="grid min-h-0 grid-cols-[minmax(0,1fr)] gap-4 overflow-y-auto scroll-pb-20 px-6 pt-6 after:-mt-4 after:block after:h-6 after:content-['']" data-dialog-body="">
          {children}
        </div>
        <DialogPrimitive.Close className="absolute right-4 top-4 rounded-sm opacity-70 hover:opacity-100 cursor-pointer" aria-label={closeLabel}>
          <X className="size-4" />
        </DialogPrimitive.Close>
      </DialogPrimitive.Content>
    </DialogPortal>
  ),
);
DialogContent.displayName = DialogPrimitive.Content.displayName;

const DialogHeader = ({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) => <div className={cn("flex flex-col gap-1.5 text-left", className)} {...props} />;
/**
 * The action row sticks to the bottom edge of the dialog body while the body scrolls. The negative margins pull its
 * opaque box over the body's side padding, the spacer below and half a rem into the gap above; the paddings put
 * the buttons back where they were — so a dialog that fits looks as before, and in one that scrolls the fields
 * pass under the row. It works for a footer that is a direct child of `DialogContent` and for one inside a `<form>`.
 * No z-index: as the last element it already paints over the fields, and a drop hint (`z-10`) still covers it.
 */
const DialogFooter = ({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) => (
  <div className={cn("sticky bottom-0 -mx-6 -mb-6 -mt-2 flex flex-row flex-wrap justify-end gap-2 bg-bg-elevated px-6 pb-6 pt-2", className)} data-dialog-footer="" {...props} />
);

const DialogTitle = React.forwardRef<React.ElementRef<typeof DialogPrimitive.Title>, React.ComponentPropsWithoutRef<typeof DialogPrimitive.Title>>(({ className, ...props }, ref) => (
  <DialogPrimitive.Title ref={ref} className={cn("text-lg font-semibold leading-none tracking-tight", className)} {...props} />
));
DialogTitle.displayName = DialogPrimitive.Title.displayName;

const DialogDescription = React.forwardRef<React.ElementRef<typeof DialogPrimitive.Description>, React.ComponentPropsWithoutRef<typeof DialogPrimitive.Description>>(({ className, ...props }, ref) => (
  <DialogPrimitive.Description ref={ref} className={cn("text-sm text-fg-muted", className)} {...props} />
));
DialogDescription.displayName = DialogPrimitive.Description.displayName;

export { Dialog, DialogPortal, DialogOverlay, DialogTrigger, DialogClose, DialogContent, DialogHeader, DialogFooter, DialogTitle, DialogDescription };
