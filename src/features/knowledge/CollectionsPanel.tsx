import { FolderPlus, Pencil, Trash2, Upload } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input, Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { DropHint, importDroppedFiles, useDocumentDropTarget, type DragDoc } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useCollectionDocs, useCollections } from "@/lib/queries";
import type { Collection, CollectionInput } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

function EditDialog({ open, initial, onOpenChange }: { open: boolean; initial: Collection | null; onOpenChange: (open: boolean) => void }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const [draft, setDraft] = useState<CollectionInput>({ name: "", description: "", active: true });
  const [busy, setBusy] = useState(false);
  const key = `${open}-${initial?.id ?? "new"}`;
  const [seen, setSeen] = useState("");
  if (open && seen !== key) {
    setSeen(key);
    setDraft(initial ? { id: initial.id, name: initial.name, description: initial.description ?? "", active: initial.active } : { name: "", description: "", active: true });
  }
  const save = async () => {
    setBusy(true);
    try {
      await api.knowledgeCollectionSave({ ...draft, description: draft.description?.trim() ? draft.description : null });
      toast(t("knowledge.collections.saved"), "ok");
      onOpenChange(false);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-testid="collection-dialog" closeLabel={t("common.close")}>
        <DialogHeader>
          <DialogTitle>{initial ? t("knowledge.collections.edit") : t("knowledge.collections.new")}</DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <Label htmlFor="col-name">{t("knowledge.collections.name")}</Label>
            <Input id="col-name" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} placeholder={t("knowledge.collections.name_placeholder")} data-testid="collection-name" />
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor="col-desc">{t("knowledge.collections.description")}</Label>
            <Textarea id="col-desc" rows={2} value={draft.description ?? ""} onChange={(e) => setDraft({ ...draft, description: e.target.value })} />
          </div>
          <label className="flex items-center gap-2 text-sm">
            <Switch checked={draft.active ?? true} onCheckedChange={(v) => setDraft({ ...draft, active: v })} />
            {t("knowledge.collections.active")}
          </label>
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("knowledge.collections.cancel")}
          </Button>
          <Button onClick={() => void save()} disabled={busy || !draft.name.trim()} data-testid="collection-save">
            {t("knowledge.collections.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

type TFn = (k: string, o?: Record<string, unknown>) => string;

/** One collection of the list; documents / files dropped on it join it. */
function CollectionItem({ c, selected, onSelect, onDropDocs, onDropFiles, t }: { c: Collection; selected: boolean; onSelect: () => void; onDropDocs: (c: Collection, docs: DragDoc[]) => void; onDropFiles: (c: Collection, paths: string[]) => void; t: TFn }) {
  const drop = useDocumentDropTarget({ onDocuments: (docs) => onDropDocs(c, docs), onFiles: (paths) => onDropFiles(c, paths) });
  return (
    <li className="relative" {...drop.props}>
      <DropHint active={drop.isActive} over={drop.isOver} className="text-[0.6875rem]" testId={`collection-drop-${c.id}`}>
        {t("knowledge.collections.drop_add", { name: c.name })}
      </DropHint>
      <button
        type="button"
        onClick={onSelect}
        className={cn("flex w-full flex-col gap-0.5 rounded-md border border-border px-3 py-2 text-left hover:border-accent cursor-pointer", selected && "border-accent bg-accent-soft", drop.isOver && "border-accent")}
        data-testid="collection-item"
        data-collection-name={c.name}
      >
        <span className="flex items-center gap-2 text-sm font-medium">
          {c.name}
          <Badge variant={c.active ? "ok" : "muted"}>{c.active ? t("knowledge.collections.active") : t("knowledge.collections.inactive")}</Badge>
        </span>
        <span className="text-xs text-fg-subtle">
          {t("knowledge.collections.docs_count", { count: c.doc_count })} · {t("knowledge.collections.chunks_count", { count: c.chunk_count })}
        </span>
        {c.description && <span className="text-xs text-fg-muted">{c.description}</span>}
      </button>
    </li>
  );
}

export function CollectionsPanel({ onIngest }: { onIngest: (collectionId: string) => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const { data: collections, isLoading } = useCollections();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selected = collections?.find((c) => c.id === selectedId) ?? null;
  const { data: docs } = useCollectionDocs(selectedId);
  const [edit, setEdit] = useState<{ open: boolean; initial: Collection | null }>({ open: false, initial: null });
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);

  const remove = (c: Collection) =>
    setConfirm({
      title: t("knowledge.collections.delete"),
      description: t("knowledge.collections.delete_confirm", { name: c.name }),
      confirmLabel: t("knowledge.collections.delete"),
      danger: true,
      onConfirm: async () => {
        try {
          await api.knowledgeCollectionDelete(c.id);
          if (selectedId === c.id) setSelectedId(null);
          toast(t("knowledge.collections.deleted"), "ok");
        } catch (e) {
          toast(describeError(e, t), "danger");
        }
      },
    });
  const removeDoc = async (docId: string) => {
    if (!selectedId) return;
    try {
      await api.knowledgeCollectionRemove(selectedId, [docId]);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  // Drops: library documents join the collection at once; files are imported as knowledge documents of it first.
  const addDocs = async (c: Collection, ids: string[]) => {
    if (ids.length === 0) return;
    try {
      await api.knowledgeCollectionAdd(c.id, ids, true);
      toast(t("library.kb_added", { count: ids.length, name: c.name }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const onDropDocs = (c: Collection, docs: DragDoc[]) => void addDocs(c, docs.map((d) => d.id));
  const onDropFiles = (c: Collection, paths: string[]) => void importDroppedFiles(paths, { role: "knowledge", collectionId: c.id }, t).then((ids) => addDocs(c, ids));
  const dropSelected = useDocumentDropTarget({
    disabled: !selected,
    onDocuments: (docs) => selected && onDropDocs(selected, docs),
    onFiles: (paths) => selected && onDropFiles(selected, paths),
  });

  return (
    <div className="grid gap-3 md:grid-cols-[minmax(16rem,1fr)_2fr]" data-testid="knowledge-collections">
      <div className="flex flex-col gap-2">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold">{t("knowledge.collections.title")}</h2>
          <Button size="sm" variant="outline" className="ml-auto" onClick={() => setEdit({ open: true, initial: null })} data-testid="collection-new">
            <FolderPlus />
            {t("knowledge.collections.new")}
          </Button>
        </div>
        {isLoading && <Spinner />}
        {collections && collections.length === 0 && (
          <p className="text-sm text-fg-muted" data-testid="collections-empty">
            {t("knowledge.collections.empty")}
          </p>
        )}
        <ul className="flex flex-col gap-1">
          {collections?.map((c) => (
            <CollectionItem key={c.id} c={c} selected={selectedId === c.id} onSelect={() => setSelectedId(c.id)} onDropDocs={onDropDocs} onDropFiles={onDropFiles} t={t} />
          ))}
        </ul>
        {collections && collections.length > 0 && <p className="text-xs text-fg-subtle">{t("knowledge.collections.drop_hint")}</p>}
      </div>
      <div className="relative flex flex-col gap-2" {...dropSelected.props} data-testid="collection-docs-drop">
        <DropHint active={dropSelected.isActive} over={dropSelected.isOver} testId="collection-docs-drop-hint">
          {selected ? t("knowledge.collections.drop_add", { name: selected.name }) : ""}
        </DropHint>
        {selected ? (
          <>
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="text-sm font-semibold">
                {t("knowledge.collections.docs_title")} · {selected.name}
              </h2>
              <span className="ml-auto flex gap-1">
                <Button size="sm" variant="outline" onClick={() => onIngest(selected.id)} data-testid="collection-ingest">
                  <Upload />
                  {t("knowledge.actions.ingest")}
                </Button>
                <Button size="sm" variant="ghost" onClick={() => setEdit({ open: true, initial: selected })} aria-label={t("knowledge.collections.edit")}>
                  <Pencil />
                </Button>
                <Button size="sm" variant="ghost" className="text-danger" onClick={() => remove(selected)} aria-label={t("knowledge.collections.delete")} data-testid="collection-delete">
                  <Trash2 />
                </Button>
              </span>
            </div>
            {docs && docs.length === 0 && <p className="text-sm text-fg-muted">{t("knowledge.collections.no_docs")}</p>}
            <ul className="divide-y divide-border rounded-md border border-border">
              {docs?.map((d) => (
                <li key={d.id} className="flex items-center gap-2 px-3 py-2 text-sm" data-testid="collection-doc">
                  <button type="button" className="min-w-0 flex-1 truncate text-left hover:underline cursor-pointer" onClick={() => navigate(`/library/${d.id}`)} title={t("knowledge.collections.open_doc")}>
                    {d.title ?? d.original_name}
                  </button>
                  <span className="text-xs text-fg-subtle">{t("library.pages_count", { count: d.page_count })}</span>
                  <Button size="sm" variant="ghost" onClick={() => void removeDoc(d.id)}>
                    {t("knowledge.collections.remove_doc")}
                  </Button>
                </li>
              ))}
            </ul>
          </>
        ) : (
          <p className="text-sm text-fg-muted">{t("knowledge.collections.select")}</p>
        )}
      </div>
      <EditDialog open={edit.open} initial={edit.initial} onOpenChange={(open) => setEdit((e) => ({ ...e, open }))} />
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
