import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";

import type { ReextractRequest } from "./reextract";

/**
 * Confirmation before the 검토함 extracts a document again (PIP-2), worded like the one of the library's 처리 menu
 * (`pipeline.process.confirm_*`): the new run replaces the document's fields, and with them the approvals and
 * corrections made so far — the two buttons used to start a forced run at once. The fine-print profile also
 * offers to start over with classification and OCR, which replaces page text corrected by hand; that is off
 * unless ticked.
 */
export function ReextractDialog({ request, onClose, onConfirm }: { request: ReextractRequest | null; onClose: () => void; onConfirm: (request: ReextractRequest, startOver: boolean) => void }) {
  const { t } = useTranslation();
  const [startOver, setStartOver] = useState(false);
  // The request on show: kept while the dialog closes so its text does not vanish before the dialog does.
  const [shown, setShown] = useState<ReextractRequest | null>(request);
  useEffect(() => {
    if (!request) return;
    setShown(request);
    setStartOver(false);
  }, [request]);
  return (
    <Dialog open={request !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-md" closeLabel={t("common.close")} data-testid="review-reextract-dialog" data-mode={shown?.mode}>
        <DialogHeader>
          <DialogTitle>{t("pipeline.process.confirm_title")}</DialogTitle>
          <DialogDescription>{t("pipeline.process.confirm_body", { count: 1 })}</DialogDescription>
        </DialogHeader>
        {shown && (
          <div className="flex flex-col gap-2 text-sm">
            <p className="truncate text-fg-muted" title={shown.documentName} data-testid="review-reextract-what">
              {t(`review.reextract_what.${shown.mode}`, { name: shown.documentName })}
            </p>
            {shown.mode === "fine_print" && (
              <label className="flex cursor-pointer items-start gap-2">
                <input type="checkbox" className="mt-1" checked={startOver} onChange={(e) => setStartOver(e.target.checked)} data-testid="review-reextract-start-over" />
                <span className="min-w-0">
                  <span className="block">{t("review.reextract_start_over")}</span>
                  <span className="block text-xs text-fg-muted">{t("review.reextract_start_over_hint")}</span>
                </span>
              </label>
            )}
          </div>
        )}
        <DialogFooter>
          <Button variant="secondary" onClick={onClose} data-testid="review-reextract-cancel">
            {t("common.cancel")}
          </Button>
          <Button
            variant="danger"
            onClick={() => {
              const r = request;
              onClose();
              if (r) onConfirm(r, startOver);
            }}
            data-testid="review-reextract-ok"
          >
            {t("pipeline.process.confirm_ok")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
