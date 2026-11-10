import { FolderInput, FolderPlus, Pencil, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { DropHint, importDroppedFiles, useDocumentDropTarget, type DragDoc } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import type { Collection } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

import { IngestWizard } from "./IngestWizard";
import { ReferenceDocsPanel } from "./ReferenceDocsPanel";
import { addToReferenceGroup, invalidateReference, useReferenceGroups } from "./reference";

/** "새 묶음" / "이름 변경": one name field. */
function NameDialog({ open, initial, onOpenChange, onSaved }: { open: boolean; initial: Collection | null; onOpenChange: (o: boolean) => void; onSaved: (c: Collection) => void }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (open) setName(initial?.name ?? "");
  }, [open, initial]);
  const save = async () => {
    setBusy(true);
    try {
      const saved = await api.knowledgeCollectionSave(initial ? { id: initial.id, name: name.trim(), description: initial.description, color: initial.color, active: initial.active } : { name: name.trim() });
      invalidateReference(saved.id);
      toast(t("projects.reference.saved"), "ok");
      onOpenChange(false);
      onSaved(saved);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md" closeLabel={t("common.close")} data-testid="reference-name-dialog">
        <DialogHeader>
          <DialogTitle>{initial ? t("projects.reference.rename_title") : t("projects.reference.new_group")}</DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-1">
          <Label htmlFor="reference-name">{t("projects.reference.name")}</Label>
          <Input id="reference-name" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("projects.reference.name_placeholder")} data-testid="reference-name" />
        </div>
        <DialogFooter>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button onClick={() => void save()} disabled={busy || !name.trim()} data-testid="reference-name-save">
            {t("projects.reference.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** One group of the list; documents / files dropped on it join it. */
function GroupItem({ c, selected, onSelect, onDropDocs, onDropFiles }: { c: Collection; selected: boolean; onSelect: () => void; onDropDocs: (c: Collection, docs: DragDoc[]) => void; onDropFiles: (c: Collection, paths: string[]) => void }) {
  const { t } = useTranslation();
  const drop = useDocumentDropTarget({ onDocuments: (docs) => onDropDocs(c, docs), onFiles: (paths) => onDropFiles(c, paths) });
  return (
    <li className="relative" {...drop.props}>
      <DropHint active={drop.isActive} over={drop.isOver} className="text-[0.6875rem]" testId={`reference-group-drop-${c.id}`}>
        {t("projects.reference.drop_add", { name: c.name })}
      </DropHint>
      <button
        type="button"
        onClick={onSelect}
        className={cn("flex w-full flex-col gap-0.5 rounded-md border border-border px-3 py-2 text-left hover:border-accent cursor-pointer", selected && "border-accent bg-accent-soft", drop.isOver && "border-accent")}
        aria-pressed={selected}
        data-testid="reference-group-item"
        data-group-name={c.name}
        data-group-active={c.active}
      >
        <span className="flex items-center gap-2 text-sm font-medium">
          <span className="min-w-0 flex-1 truncate">{c.name}</span>
          <Badge variant={c.active ? "ok" : "muted"}>{c.active ? t("projects.reference.active") : t("projects.reference.inactive")}</Badge>
        </span>
        <span className="text-xs text-fg-subtle">{t("projects.reference.docs_count", { count: c.doc_count })}</span>
      </button>
    </li>
  );
}

/**
 * 공통 참고 자료 (0.5.0 plan §2): the common reference groups (새 묶음 · 이름 변경 · 사용 toggle · 삭제), the
 * selected group's documents, the import wizard and drop targets. A group switched off is left out of every
 * search / answer scope until it is switched on again.
 */
export function ReferenceGroupsView() {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { groups, isLoading } = useReferenceGroups();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selected = groups?.find((c) => c.id === selectedId) ?? groups?.[0] ?? null;
  const [name, setName] = useState<{ open: boolean; initial: Collection | null }>({ open: false, initial: null });
  const [wizard, setWizard] = useState<{ open: boolean; collectionId: string | null }>({ open: false, collectionId: null });
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);

  const addDocs = async (c: Collection, ids: string[]) => {
    if (ids.length === 0) return;
    try {
      await addToReferenceGroup(c.id, ids);
      toast(t("projects.reference.added", { count: ids.length }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const onDropDocs = (c: Collection, docs: DragDoc[]) => void addDocs(c, docs.map((d) => d.id));
  const onDropFiles = (c: Collection, paths: string[]) =>
    void importDroppedFiles(paths, { role: "knowledge", collectionId: c.id }, t).then((ids) => {
      invalidateReference(c.id);
      if (ids.length > 0) toast(t("projects.reference.added", { count: ids.length }), "ok");
    });
  const setActive = async (c: Collection, active: boolean) => {
    try {
      await api.knowledgeCollectionSave({ id: c.id, name: c.name, description: c.description, color: c.color, active });
      invalidateReference(c.id);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const remove = (c: Collection) =>
    setConfirm({
      title: t("projects.reference.delete"),
      description: t("projects.reference.delete_confirm", { name: c.name }),
      confirmLabel: t("projects.reference.delete"),
      danger: true,
      onConfirm: async () => {
        try {
          await api.knowledgeCollectionDelete(c.id);
          invalidateReference(c.id);
          if (selectedId === c.id) setSelectedId(null);
          toast(t("projects.reference.deleted"), "ok");
        } catch (e) {
          toast(describeError(e, t), "danger");
        }
      },
    });

  return (
    <div className="grid gap-4 md:grid-cols-[minmax(14rem,1fr)_3fr]" data-testid="reference-groups">
      <div className="flex flex-col gap-2">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold">{t("projects.reference.groups")}</h2>
          <Button size="sm" variant="outline" className="ml-auto" onClick={() => setName({ open: true, initial: null })} data-testid="reference-group-new">
            <FolderPlus />
            {t("projects.reference.new_group")}
          </Button>
        </div>
        {isLoading && !groups && <Spinner />}
        {groups && groups.length === 0 && (
          <p className="text-sm text-fg-muted" data-testid="reference-groups-empty">
            {t("projects.reference.no_groups")}
          </p>
        )}
        <ul className="flex flex-col gap-1">
          {groups?.map((c) => (
            <GroupItem key={c.id} c={c} selected={selected?.id === c.id} onSelect={() => setSelectedId(c.id)} onDropDocs={onDropDocs} onDropFiles={onDropFiles} />
          ))}
        </ul>
        {groups && groups.length === 0 && (
          <Button variant="outline" className="self-start" onClick={() => setWizard({ open: true, collectionId: null })} data-testid="reference-ingest-first">
            <FolderInput />
            {t("projects.reference.ingest")}
          </Button>
        )}
      </div>
      <div className="flex min-w-0 flex-col gap-3">
        {selected ? (
          <>
            <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-bg-elevated px-3 py-2">
              <h2 className="min-w-0 flex-1 truncate text-sm font-semibold" data-testid="reference-group-title">
                {selected.name}
              </h2>
              <label className="flex items-center gap-2 text-xs text-fg-muted">
                <Switch checked={selected.active} onCheckedChange={(v) => void setActive(selected, v)} aria-label={t("projects.reference.active")} data-testid="reference-group-active" />
                {selected.active ? t("projects.reference.active") : t("projects.reference.inactive")}
              </label>
              <Button size="sm" variant="ghost" onClick={() => setName({ open: true, initial: selected })} data-testid="reference-group-rename">
                <Pencil />
                {t("projects.reference.rename")}
              </Button>
              <Button size="sm" variant="ghost" className="text-danger" onClick={() => remove(selected)} data-testid="reference-group-delete">
                <Trash2 />
                {t("projects.reference.delete")}
              </Button>
            </div>
            <ReferenceDocsPanel group={selected} testIdPrefix="reference" onIngest={() => setWizard({ open: true, collectionId: selected.id })} />
          </>
        ) : (
          <p className="text-sm text-fg-muted">{groups && groups.length > 0 ? t("projects.reference.select_group") : t("projects.reference.no_groups")}</p>
        )}
      </div>
      <NameDialog open={name.open} initial={name.initial} onOpenChange={(o) => setName((s) => ({ ...s, open: o }))} onSaved={(c) => setSelectedId(c.id)} />
      <IngestWizard open={wizard.open} collectionId={wizard.collectionId} onOpenChange={(open) => setWizard((w) => ({ ...w, open }))} />
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
