import { useState } from "react";
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
  const generate = async () => {
    setBusy(true);
    try {
      const d = await api.pipelineSchemaFromNl(text, base?.id ?? null);
      onDraft(d);
      onOpenChange(false);
      setText("");
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent closeLabel={t("common.close")} data-testid="nl-schema-dialog">
        <DialogHeader>
          <DialogTitle>{base ? t("schemas.nl.modify_title", { name: base.name }) : t("schemas.nl.title")}</DialogTitle>
          <DialogDescription>{t("schemas.nl.hint")}</DialogDescription>
        </DialogHeader>
        <Textarea rows={5} value={text} onChange={(e) => setText(e.target.value)} placeholder={t("schemas.nl.placeholder")} aria-label={t("schemas.nl.description")} data-testid="nl-schema-text" />
        <DialogFooter>
          {busy && (
            <span className="mr-auto flex items-center gap-2 text-sm text-fg-muted" role="status">
              <Spinner />
              {t("schemas.nl.generating")}
            </span>
          )}
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
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
