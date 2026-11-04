import { useEffect, useState } from "react";
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
  useEffect(() => {
    if (open) {
      setDocId(documentId ?? "");
      setName("");
      setBusy(false);
    }
  }, [open, documentId]);
  const derive = async () => {
    if (!docId) return;
    setBusy(true);
    try {
      const d = await api.pipelineFormDerive(docId, name.trim() || null);
      onDraft(d);
      onOpenChange(false);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
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
        <DialogFooter>
          {busy && (
            <span className="mr-auto flex items-center gap-2 text-sm text-fg-muted" role="status">
              <Spinner />
              {t("schemas.form.deriving")}
            </span>
          )}
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
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
