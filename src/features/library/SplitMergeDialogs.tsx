import { X } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { DropHint, importDroppedFiles, useDocumentDropTarget } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useDocuments } from "@/lib/queries";
import type { DocumentView, PageView } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

/** "여기서 새 문서 시작" — pick the pages where new documents begin (design §3.4 분할). */
export function SplitDialog({ open, onOpenChange, doc, pages, initial }: { open: boolean; onOpenChange: (o: boolean) => void; doc: DocumentView; pages: PageView[]; initial?: number | null }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const [marks, setMarks] = useState<Set<number>>(new Set());
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (open) setMarks(new Set(initial && initial > 1 ? [initial] : []));
  }, [open, initial]);
  const toggle = (p: number) =>
    setMarks((m) => {
      const n = new Set(m);
      if (n.has(p)) n.delete(p);
      else n.add(p);
      return n;
    });
  const run = async () => {
    setBusy(true);
    try {
      await api.pipelineSplit(doc.id, [...marks]);
      toast(t("pipeline.split.started"), "info");
      onOpenChange(false);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  const sorted = [...marks].sort((a, b) => a - b);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl" closeLabel={t("common.close")} data-testid="split-dialog">
        <DialogHeader>
          <DialogTitle>{t("pipeline.split.title")}</DialogTitle>
          <DialogDescription>{t("pipeline.split.hint")}</DialogDescription>
        </DialogHeader>
        <div className="flex max-h-72 flex-wrap gap-2 overflow-y-auto" data-testid="split-pages">
          {pages.map((p) => (
            <button
              key={p.page_no}
              type="button"
              disabled={p.page_no === 1}
              onClick={() => toggle(p.page_no)}
              className={cn("flex w-24 flex-col items-center gap-1 rounded-md border p-1 text-xs", marks.has(p.page_no) ? "border-accent bg-accent-soft" : "border-border", p.page_no === 1 && "opacity-60")}
              aria-pressed={marks.has(p.page_no)}
              aria-label={`${t("pipeline.split.start_here")}: ${t("pipeline.split.page", { page: p.page_no })}`}
              data-testid={`split-page-${p.page_no}`}
            >
              {p.thumb_url ? <img src={p.thumb_url} alt="" className="h-20 w-full bg-white object-contain" loading="lazy" decoding="async" draggable={false} /> : <div className="h-20 w-full bg-bg-sunken" />}
              <span>{t("pipeline.split.page", { page: p.page_no })}</span>
              {marks.has(p.page_no) && <span className="text-[0.625rem] text-accent">{t("pipeline.split.start_here")}</span>}
            </button>
          ))}
        </div>
        <p className="text-sm" data-testid="split-boundaries">
          {sorted.length === 0 ? t("pipeline.split.none") : t("pipeline.split.boundaries", { pages: sorted.join(", ") })}
        </p>
        <DialogFooter>
          <Button variant="ghost" onClick={() => setMarks(new Set())}>
            {t("pipeline.split.clear")}
          </Button>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button onClick={() => void run()} disabled={busy || sorted.length === 0} data-testid="split-run">
            {t("pipeline.split.run")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function MergeDialog({ open, onOpenChange, ids, onDone }: { open: boolean; onOpenChange: (o: boolean) => void; ids: string[]; onDone?: () => void }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  // The merge order starts as the library selection; dropped documents / files append to it.
  const [list, setList] = useState<string[]>(ids);
  useEffect(() => {
    if (open) {
      setName("");
      setList(ids);
    }
  }, [open, ids]);
  const { data: docs } = useDocuments({ limit: 2000, sort: "imported_desc" }, open);
  const addIds = (more: string[]) => setList((l) => [...l, ...more.filter((id) => !l.includes(id))]);
  const drop = useDocumentDropTarget({
    disabled: !open || busy,
    onDocuments: (dropped) => addIds(dropped.map((d) => d.id)),
    onFiles: (paths) => void importDroppedFiles(paths, {}, t).then(addIds),
  });
  const run = async () => {
    setBusy(true);
    try {
      await api.pipelineMerge(list, name.trim() || null);
      toast(t("pipeline.merge.started"), "info");
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
      <DialogContent className="max-w-md" closeLabel={t("common.close")} data-testid="merge-dialog" {...drop.props}>
        <DropHint active={drop.isActive} over={drop.isOver} testId="merge-drop-hint">
          {t("pipeline.merge.drop_hint")}
        </DropHint>
        <DialogHeader>
          <DialogTitle>{t("pipeline.merge.title")}</DialogTitle>
          <DialogDescription>
            {t("pipeline.merge.hint", { count: list.length })} {t("pipeline.merge.order_hint")} {t("pipeline.merge.drop_hint")}
          </DialogDescription>
        </DialogHeader>
        <ol className="max-h-56 divide-y divide-border overflow-y-auto rounded-md border border-border text-sm" data-testid="merge-list">
          {list.map((id, i) => {
            const d = docs?.find((x) => x.id === id);
            return (
              <li key={id} className="flex items-center gap-2 px-3 py-1.5" data-testid={`merge-item-${id}`}>
                <span className="w-5 text-xs tabular-nums text-fg-subtle">{i + 1}</span>
                <span className="min-w-0 flex-1 truncate">{d?.title ?? d?.original_name ?? id.slice(0, 8)}</span>
                <Button size="icon-sm" variant="ghost" onClick={() => setList((l) => l.filter((x) => x !== id))} aria-label={t("pipeline.merge.remove")} data-testid={`merge-remove-${id}`}>
                  <X />
                </Button>
              </li>
            );
          })}
        </ol>
        <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={t("pipeline.merge.name")} aria-label={t("pipeline.merge.name")} data-testid="merge-name" />
        <DialogFooter>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button onClick={() => void run()} disabled={busy || list.length < 2} data-testid="merge-run">
            {t("pipeline.merge.run")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
