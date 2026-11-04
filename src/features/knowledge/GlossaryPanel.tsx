import { Pencil, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input, Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useGlossary } from "@/lib/queries";
import type { GlossaryEntry } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

export function GlossaryPanel() {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: entries, isLoading } = useGlossary();
  const [edit, setEdit] = useState<{ open: boolean; entry: GlossaryEntry | null }>({ open: false, entry: null });
  const [term, setTerm] = useState("");
  const [definition, setDefinition] = useState("");
  const [synonyms, setSynonyms] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);

  const openEditor = (entry: GlossaryEntry | null) => {
    setTerm(entry?.term ?? "");
    setDefinition(entry?.definition ?? "");
    setSynonyms(entry?.synonyms.join(", ") ?? "");
    setEdit({ open: true, entry });
  };
  const save = async () => {
    setBusy(true);
    try {
      await api.knowledgeGlossarySave({
        id: edit.entry?.id ?? null,
        term,
        definition: definition.trim() ? definition : null,
        synonyms: synonyms.split(/[,，\n]/).map((s) => s.trim()).filter(Boolean),
      });
      toast(t("knowledge.glossary.saved"), "ok");
      setEdit({ open: false, entry: null });
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  const remove = (entry: GlossaryEntry) =>
    setConfirm({
      title: t("knowledge.glossary.delete"),
      description: t("knowledge.glossary.delete_confirm", { term: entry.term }),
      confirmLabel: t("knowledge.glossary.delete"),
      danger: true,
      onConfirm: async () => {
        try {
          await api.knowledgeGlossaryDelete(entry.id);
          toast(t("knowledge.glossary.deleted"), "ok");
        } catch (e) {
          toast(describeError(e, t), "danger");
        }
      },
    });

  return (
    <div className="flex flex-col gap-2" data-testid="knowledge-glossary">
      <div className="flex items-center gap-2">
        <div>
          <h2 className="text-sm font-semibold">{t("knowledge.glossary.title")}</h2>
          <p className="text-xs text-fg-muted">{t("knowledge.glossary.hint")}</p>
        </div>
        <Button size="sm" variant="outline" className="ml-auto" onClick={() => openEditor(null)} data-testid="glossary-add">
          <Plus />
          {t("knowledge.glossary.add")}
        </Button>
      </div>
      {isLoading && <Spinner />}
      {entries && entries.length === 0 && <p className="text-sm text-fg-muted">{t("knowledge.glossary.empty")}</p>}
      {entries && entries.length > 0 && (
        <table className="w-full text-sm">
          <thead className="text-left text-xs text-fg-subtle">
            <tr>
              <th className="py-1 pr-2">{t("knowledge.glossary.term")}</th>
              <th className="py-1 pr-2">{t("knowledge.glossary.synonyms")}</th>
              <th className="py-1 pr-2">{t("knowledge.glossary.definition")}</th>
              <th className="w-20" />
            </tr>
          </thead>
          <tbody>
            {entries.map((e) => (
              <tr key={e.id} className="border-t border-border" data-testid="glossary-row" data-term={e.term}>
                <td className="py-1 pr-2 font-medium selectable">{e.term}</td>
                <td className="py-1 pr-2 text-fg-muted selectable">{e.synonyms.join(", ")}</td>
                <td className="py-1 pr-2 text-fg-muted selectable">{e.definition}</td>
                <td className="py-1 text-right">
                  <Button size="icon-sm" variant="ghost" onClick={() => openEditor(e)} aria-label={t("knowledge.glossary.edit")}>
                    <Pencil />
                  </Button>
                  <Button size="icon-sm" variant="ghost" className="text-danger" onClick={() => remove(e)} aria-label={t("knowledge.glossary.delete")}>
                    <Trash2 />
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <Dialog open={edit.open} onOpenChange={(open) => setEdit((s) => ({ ...s, open }))}>
        <DialogContent data-testid="glossary-dialog" closeLabel={t("common.close")}>
          <DialogHeader>
            <DialogTitle>{edit.entry ? t("knowledge.glossary.edit") : t("knowledge.glossary.add")}</DialogTitle>
          </DialogHeader>
          <div className="flex flex-col gap-3">
            <div className="flex flex-col gap-1">
              <Label htmlFor="gl-term">{t("knowledge.glossary.term")}</Label>
              <Input id="gl-term" value={term} onChange={(e) => setTerm(e.target.value)} data-testid="glossary-term" />
            </div>
            <div className="flex flex-col gap-1">
              <Label htmlFor="gl-syn">{t("knowledge.glossary.synonyms")}</Label>
              <Input id="gl-syn" value={synonyms} onChange={(e) => setSynonyms(e.target.value)} data-testid="glossary-synonyms" />
            </div>
            <div className="flex flex-col gap-1">
              <Label htmlFor="gl-def">{t("knowledge.glossary.definition")}</Label>
              <Textarea id="gl-def" rows={2} value={definition} onChange={(e) => setDefinition(e.target.value)} />
            </div>
          </div>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setEdit({ open: false, entry: null })}>
              {t("knowledge.glossary.cancel")}
            </Button>
            <Button onClick={() => void save()} disabled={busy || !term.trim()} data-testid="glossary-save">
              {t("knowledge.glossary.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
