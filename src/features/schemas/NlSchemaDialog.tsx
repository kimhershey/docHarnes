import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Textarea } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import type { SchemaDraft, SchemaRecord } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

/** "자연어로 스키마 만들기/수정하기" (design §3.5): description → metaschema-constrained draft → editor. */
export function NlSchemaDialog({ open, onOpenChange, base, onDraft }: { open: boolean; onOpenChange: (o: boolean) => void; base: SchemaRecord | null; onDraft: (d: SchemaDraft) => void }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const [text, setText] = useState("");
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
    if (!open) dropPending();
  }, [open, dropPending]);

  const close = () => {
    dropPending();
    onOpenChange(false);
  };
  const generate = async () => {
    requestSeq.current += 1;
    const seq = requestSeq.current;
    setBusy(true);
    try {
      const d = await api.pipelineSchemaFromNl(text, base?.id ?? null);
      if (seq !== requestSeq.current) return; // closed / cancelled while the model was working
      setBusy(false);
      onDraft(d);
      onOpenChange(false);
      setText("");
    } catch (e) {
      if (seq !== requestSeq.current) return;
      setBusy(false);
      toast(describeError(e, t), "danger");
    }
  };
  return (
    <Dialog open={open} onOpenChange={(o) => (o ? onOpenChange(true) : close())}>
      <DialogContent closeLabel={t("common.close")} data-testid="nl-schema-dialog">
        <DialogHeader>
          <DialogTitle>{base ? t("schemas.nl.modify_title", { name: base.name }) : t("schemas.nl.title")}</DialogTitle>
          <DialogDescription>{t("schemas.nl.hint")}</DialogDescription>
        </DialogHeader>
        <Textarea rows={5} value={text} onChange={(e) => setText(e.target.value)} placeholder={t("schemas.nl.placeholder")} aria-label={t("schemas.nl.description")} data-testid="nl-schema-text" />
        <DialogFooter className="items-center">
          {busy && (
            <span className="mr-auto flex flex-col gap-0.5 text-sm text-fg-muted" role="status" data-testid="nl-schema-pending">
              <span className="flex items-center gap-2">
                <Spinner />
                {t("schemas.nl.generating")}
              </span>
              <span className="text-xs text-fg-subtle">{t("schemas.pending_hint")}</span>
            </span>
          )}
          <Button variant="secondary" onClick={close} data-testid="nl-schema-cancel">
            {t("common.cancel")}
          </Button>
          <Button onClick={() => void generate()} disabled={busy || !text.trim()} data-testid="nl-schema-generate">
            {t("schemas.nl.generate")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
