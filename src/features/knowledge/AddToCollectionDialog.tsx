import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useCollections } from "@/lib/queries";
import { useUiStore } from "@/stores/ui";

const NEW = "__new__";

/** Adds documents to a (new or existing) collection and marks them as knowledge documents. */
export function AddToCollectionDialog({ open, onOpenChange, docIds, onDone }: { open: boolean; onOpenChange: (open: boolean) => void; docIds: string[]; onDone?: () => void }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: collections } = useCollections(open);
  const [choice, setChoice] = useState<string>(NEW);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);

  const [touched, setTouched] = useState(false);
  useEffect(() => {
    if (open) {
      setChoice(NEW);
      setName("");
      setTouched(false);
    }
  }, [open]);
  // Default to the first existing collection once the list is known (never after the user chose).
  useEffect(() => {
    if (open && !touched && collections && collections.length > 0) setChoice(collections[0]?.id ?? NEW);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, collections === undefined]);

  const submit = async () => {
    setBusy(true);
    try {
      let id = choice;
      let label = collections?.find((c) => c.id === choice)?.name ?? "";
      if (choice === NEW) {
        const created = await api.knowledgeCollectionSave({ name });
        id = created.id;
        label = created.name;
      }
      const n = await api.knowledgeCollectionAdd(id, docIds, true);
      toast(t("library.kb_added", { count: n, name: label }), "ok");
      onOpenChange(false);
      onDone?.();
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-testid="add-to-kb-dialog" closeLabel={t("common.close")}>
        <DialogHeader>
          <DialogTitle>{t("knowledge.collections.add_title")}</DialogTitle>
          <DialogDescription>{t("knowledge.collections.add_hint", { count: docIds.length })}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3">
          <Select
            value={choice}
            onValueChange={(v) => {
              setTouched(true);
              setChoice(v);
            }}
          >
            <SelectTrigger aria-label={t("knowledge.collections.select")} data-testid="add-to-kb-collection">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {(collections ?? []).map((c) => (
                <SelectItem key={c.id} value={c.id}>
                  {c.name}
                </SelectItem>
              ))}
              <SelectItem value={NEW}>{t("knowledge.collections.create_inline")}</SelectItem>
            </SelectContent>
          </Select>
          {choice === NEW && (
            <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={t("knowledge.collections.name_placeholder")} aria-label={t("knowledge.collections.name")} data-testid="add-to-kb-name" />
          )}
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button onClick={() => void submit()} disabled={busy || (choice === NEW && !name.trim())} data-testid="add-to-kb-submit">
            {t("library.actions.add_to_kb")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
