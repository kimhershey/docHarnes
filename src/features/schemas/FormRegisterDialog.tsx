import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useDocuments } from "@/lib/queries";
import type { SchemaDraft } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

/**
 * "양식 등록" step 1 (design §3.4.0): pick the sample document + name → derive
 * fields; the draft then opens in the editor for confirmation (step 2).
 */
export function FormRegisterDialog({ open, onOpenChange, documentId, onDraft }: { open: boolean; onOpenChange: (o: boolean) => void; documentId?: string | null; onDraft: (d: SchemaDraft) => void }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: docs } = useDocuments({ limit: 500, sort: "imported_desc" }, open);
  const [docId, setDocId] = useState<string>("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  // Sequence of the model request this dialog is waiting for. Closing or
  // cancelling bumps it, so a result that arrives afterwards is dropped
  // instead of replacing whatever the user has opened in the editor meanwhile.
  const requestSeq = useRef(0);
  const dropPending = useCallback(() => {
    requestSeq.current += 1;
    setBusy(false);
  }, []);
  useEffect(() => {
    if (open) {
      setDocId(documentId ?? "");
      setName("");
    } else {
      dropPending();
    }
  }, [open, documentId, dropPending]);

  const close = () => {
    dropPending();
    onOpenChange(false);
  };
  const derive = async () => {
    if (!docId) return;
    requestSeq.current += 1;
    const seq = requestSeq.current;
    setBusy(true);
    try {
      const d = await api.pipelineFormDerive(docId, name.trim() || null);
      if (seq !== requestSeq.current) return; // closed / cancelled while the model was working
      setBusy(false);
      onDraft(d);
      onOpenChange(false);
    } catch (e) {
      if (seq !== requestSeq.current) return;
      setBusy(false);
      toast(describeError(e, t), "danger");
    }
  };
  return (
    <Dialog open={open} onOpenChange={(o) => (o ? onOpenChange(true) : close())}>
      <DialogContent closeLabel={t("common.close")} data-testid="form-register-dialog">
        <DialogHeader>
          <DialogTitle>{t("schemas.form.title")}</DialogTitle>
          <DialogDescription>{t("schemas.form.hint")}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3">
          <div>
            <Label htmlFor="form-doc">{t("schemas.form.document")}</Label>
            {docs && docs.length === 0 ? (
              <p className="text-sm text-fg-muted">{t("schemas.form.no_docs")}</p>
            ) : (
              <Select value={docId} onValueChange={setDocId}>
                <SelectTrigger id="form-doc" aria-label={t("schemas.form.pick")} data-testid="form-register-doc">
                  <SelectValue placeholder={t("schemas.form.pick")} />
                </SelectTrigger>
                <SelectContent>
                  {(docs ?? []).map((d) => (
                    <SelectItem key={d.id} value={d.id}>
                      {d.title ?? d.original_name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          </div>
          <div>
            <Label htmlFor="form-name">{t("schemas.form.name")}</Label>
            <Input id="form-name" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("schemas.form.name_hint")} data-testid="form-register-name" />
          </div>
        </div>
        <DialogFooter className="items-center">
          {busy && (
            <span className="mr-auto flex flex-col gap-0.5 text-sm text-fg-muted" role="status" data-testid="form-register-pending">
              <span className="flex items-center gap-2">
                <Spinner />
                {t("schemas.form.deriving")}
              </span>
              <span className="text-xs text-fg-subtle">{t("schemas.pending_hint")}</span>
            </span>
          )}
          <Button variant="secondary" onClick={close} data-testid="form-register-cancel">
            {t("common.cancel")}
          </Button>
          <Button onClick={() => void derive()} disabled={busy || !docId} data-testid="form-register-derive">
            {t("schemas.form.derive")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
