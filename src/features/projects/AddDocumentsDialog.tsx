import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { docTypeLabel } from "@/features/pipeline/labels";
import { useDocTypes, useDocuments } from "@/lib/queries";

/** "라이브러리에서 추가": picks library documents for the project. */
export function AddDocumentsDialog({ open, onOpenChange, excludeIds, onAdd }: { open: boolean; onOpenChange: (o: boolean) => void; excludeIds: string[]; onAdd: (ids: string[]) => void }) {
  const { t, i18n } = useTranslation();
  const [query, setQuery] = useState("");
  // The search text goes to the backend (name / title / text match) so the pick is not limited to the first page.
  const { data: docs } = useDocuments({ limit: 500, sort: "imported_desc", query: query.trim() || null }, open);
  const { data: docTypes } = useDocTypes(open);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  useEffect(() => {
    if (open) {
      setPicked(new Set());
      setQuery("");
    }
  }, [open]);
  const list = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (docs ?? []).filter((d) => !excludeIds.includes(d.id) && (!q || d.original_name.toLowerCase().includes(q) || (d.title ?? "").toLowerCase().includes(q)));
  }, [docs, excludeIds, query]);
  const toggle = (id: string) =>
    setPicked((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl" closeLabel={t("common.close")} data-testid="project-add-dialog">
        <DialogHeader>
          <DialogTitle>{t("projects.documents.add_title")}</DialogTitle>
          <DialogDescription>{t("projects.documents.add_hint")}</DialogDescription>
        </DialogHeader>
        <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("projects.documents.search")} aria-label={t("projects.documents.search")} data-testid="project-add-search" />
        <div className="max-h-80 overflow-y-auto rounded-md border border-border">
          {list.length === 0 ? (
            <p className="p-3 text-sm text-fg-muted">{t("projects.documents.none")}</p>
          ) : (
            <ul className="divide-y divide-border text-sm">
              {list.map((d) => (
                <li key={d.id}>
                  <label className="flex cursor-pointer items-center gap-3 px-3 py-2 hover:bg-bg-sunken">
                    <input type="checkbox" checked={picked.has(d.id)} onChange={() => toggle(d.id)} data-testid={`project-add-doc-${d.id}`} />
                    <span className="min-w-0 flex-1 truncate">{d.title ?? d.original_name}</span>
                    <span className="text-xs text-fg-subtle">{docTypeLabel(d.doc_type, docTypes, t, i18n.language)}</span>
                  </label>
                </li>
              ))}
            </ul>
          )}
        </div>
        <DialogFooter>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button
            disabled={picked.size === 0}
            onClick={() => {
              onAdd([...picked]);
              onOpenChange(false);
            }}
            data-testid="project-add-done"
          >
            {t("projects.documents.add_done", { count: picked.size })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
