import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "./button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "./dialog";

/** One choice the user makes in the confirmation itself (a checkbox under the text, off unless `defaultChecked`). */
export interface ConfirmOption {
  label: string;
  hint?: string;
  defaultChecked?: boolean;
}

export interface ConfirmRequest {
  title: string;
  description?: string;
  confirmLabel?: string;
  danger?: boolean;
  option?: ConfirmOption;
  /** `checked`: the state of `option` when the user confirmed (false without one). */
  onConfirm: (checked: boolean) => void | Promise<void>;
}

/**
 * Themed, focus-trapped confirmation (replaces `window.confirm`, which renders
 * OS-language buttons outside the app theme / scale / high-contrast palette).
 */
export function ConfirmDialog({ request, onClose }: { request: ConfirmRequest | null; onClose: () => void }) {
  const { t } = useTranslation();
  const [checked, setChecked] = useState(false);
  // Every request starts from its own default: a box ticked for the previous request does not carry over.
  useEffect(() => {
    if (request) setChecked(request.option?.defaultChecked ?? false);
  }, [request]);
  return (
    <Dialog open={request !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-md" closeLabel={t("common.close")} data-testid="confirm-dialog">
        <DialogHeader>
          <DialogTitle>{request?.title}</DialogTitle>
          {request?.description && <DialogDescription>{request.description}</DialogDescription>}
        </DialogHeader>
        {request?.option && (
          <label className="flex cursor-pointer items-start gap-2 text-sm">
            <input type="checkbox" className="mt-1" checked={checked} onChange={(e) => setChecked(e.target.checked)} data-testid="confirm-option" />
            <span className="min-w-0">
              <span className="block">{request.option.label}</span>
              {request.option.hint && <span className="block text-xs text-fg-muted">{request.option.hint}</span>}
            </span>
          </label>
        )}
        <DialogFooter>
          <Button variant="secondary" onClick={onClose} data-testid="confirm-cancel">
            {t("common.cancel")}
          </Button>
          <Button
            variant={request?.danger ? "danger" : "default"}
            onClick={() => {
              const r = request;
              onClose();
              if (r) void r.onConfirm(r.option ? checked : false);
            }}
            data-testid="confirm-ok"
          >
            {request?.confirmLabel ?? t("common.ok")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
