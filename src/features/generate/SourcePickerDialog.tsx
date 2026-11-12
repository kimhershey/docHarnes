import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { docTypeLabel } from "@/features/pipeline/labels";
import { useDocTypes, useDocuments } from "@/lib/queries";
import { SelectAllBar } from "@/components/ui/select-all-bar";
import { inProject, ProjectFilterSelect } from "@/features/projects/ProjectFilterSelect";

/** Picks the library documents whose extracted fields a draft quotes (§3.13 "내용 = … 추출 결과 인용"). */
export function SourcePickerDialog({ open, onOpenChange, selected, onChange }: { open: boolean; onOpenChange: (o: boolean) => void; selected: string[]; onChange: (ids: string[]) => void }) {
  const { t, i18n } = useTranslation();
  const [query, setQuery] = useState("");
  const [project, setProject] = useState<string | null>(null);
  // The search text goes to the backend (name / title / text match) so the pick is not limited to the first page.
  const { data: docs } = useDocuments({ limit: 500, sort: "imported_desc", query: query.trim() || null }, open);
  const { data: docTypes } = useDocTypes(open);
  const [picked, setPicked] = useState<Set<string>>(() => new Set(selected));
  // The ticks and the search start from the form's sources when the dialog opens — only then. `selected` is a
  // fresh array on every render of the form (job progress, a library refresh), which used to reset both (GEN-6).
  const wasOpen = useRef(false);
  useEffect(() => {
    if (open && !wasOpen.current) {
      setPicked(new Set(selected));
      setQuery("");
      setProject(null);
    }
    wasOpen.current = open;
  }, [open, selected]);
  const list = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (docs ?? []).filter((d) => d.role !== "generated" && inProject(d, project) && (!q || d.original_name.toLowerCase().includes(q) || (d.title ?? "").toLowerCase().includes(q)));
  }, [docs, query, project]);
  const toggle = (id: string) =>
    setPicked((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl" closeLabel={t("common.close")} data-testid="generate-sources-dialog">
        <DialogHeader>
          <DialogTitle>{t("generate.sources_dialog.title")}</DialogTitle>
          <DialogDescription>{t("generate.sources_dialog.hint")}</DialogDescription>
        </DialogHeader>
        {/* 1.0.0: search, project filter and select all / none — the row wraps on a narrow dialog. */}
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <div className="min-w-[12rem] flex-1">
    <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("generate.sources_dialog.search")} aria-label={t("generate.sources_dialog.search")} data-testid="generate-sources-search" />
          </div>
          <ProjectFilterSelect value={project} onChange={setProject} enabled={open} testId="generate-sources-project" />
        </div>
        <SelectAllBar visibleIds={list.map((d) => d.id)} picked={picked} onChange={setPicked} testId="generate-sources-select" />
        <div className="max-h-80 overflow-y-auto rounded-md border border-border">
          {list.length === 0 ? (
            <p className="p-3 text-sm text-fg-muted">{t("generate.sources_dialog.empty")}</p>
          ) : (
            <ul className="divide-y divide-border text-sm">
              {list.map((d) => (
                <li key={d.id}>
                  <label className="flex cursor-pointer items-center gap-3 px-3 py-2 hover:bg-bg-sunken">
                    <input type="checkbox" checked={picked.has(d.id)} onChange={() => toggle(d.id)} data-testid={`generate-source-${d.id}`} />
                    <span className="min-w-0 flex-1 truncate" title={d.original_name}>
                      {d.title ?? d.original_name}
                    </span>
                    <span className="shrink-0 text-xs text-fg-subtle">{docTypeLabel(d.doc_type, docTypes, t, i18n.language)}</span>
                    {d.extracted_at && <span className="text-xs text-ok">{t("documents.status.extracted")}</span>}
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
            onClick={() => {
              onChange([...picked]);
              onOpenChange(false);
            }}
            data-testid="generate-sources-done"
          >
            {t("generate.sources_dialog.done", { count: picked.size })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
