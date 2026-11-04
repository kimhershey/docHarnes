import { useTranslation } from "react-i18next";

import { Button } from "./button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "./dialog";

export interface ConfirmRequest {
  title: string;
  description?: string;
  confirmLabel?: string;
  danger?: boolean;
  onConfirm: () => void | Promise<void>;
}

/**
 * Themed, focus-trapped confirmation (replaces `window.confirm`, which renders
 * OS-language buttons outside the app theme / scale / high-contrast palette).
 */
export function ConfirmDialog({ request, onClose }: { request: ConfirmRequest | null; onClose: () => void }) {
  const { t } = useTranslation();
  return (
    <Dialog open={request !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-md" closeLabel={t("common.close")} data-testid="confirm-dialog">
        <DialogHeader>
          <DialogTitle>{request?.title}</DialogTitle>
          {request?.description && <DialogDescription>{request.description}</DialogDescription>}
        </DialogHeader>
        <DialogFooter>
          <Button variant="secondary" onClick={onClose} data-testid="confirm-cancel">
            {t("common.cancel")}
          </Button>
          <Button
            variant={request?.danger ? "danger" : "default"}
            onClick={() => {
              const r = request;
              onClose();
              if (r) void r.onConfirm();
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
