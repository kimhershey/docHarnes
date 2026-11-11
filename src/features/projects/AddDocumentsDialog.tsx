import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { docTypeLabel } from "@/features/pipeline/labels";
import { useDocTypes, useDocuments } from "@/lib/queries";
import { cn } from "@/lib/utils";

/** What a document is to a project (0.5.0 plan A): 업무 문서 or 참고 자료. */
export type DocKind = "work" | "reference";

/**
 * "문서함에서 고르기": picks library documents for a project (the default texts / `project-add-*` ids) or for a
 * reference group (the caller passes its own title, hint and test-id prefix). With `withKind` (the project's
 * unified 문서 tab, 0.7.0 plan §1.1) the dialog also asks whether the picked documents go in as 업무 문서 or
 * 참고 자료 (`${prefix}-kind-work` / `-reference`).
 */
export function AddDocumentsDialog({
  open,
  onOpenChange,
  excludeIds,
  onAdd,
  title,
  hint,
  testIdPrefix = "project-add",
  withKind = false,
  defaultKind = "work",
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  excludeIds: string[];
  onAdd: (ids: string[], kind: DocKind) => void;
  title?: string;
  hint?: string;
  testIdPrefix?: string;
  withKind?: boolean;
  defaultKind?: DocKind;
}) {
  const { t, i18n } = useTranslation();
  const [query, setQuery] = useState("");
  // The search text goes to the backend (name / title / text match) so the pick is not limited to the first page.
  const { data: docs } = useDocuments({ limit: 500, sort: "imported_desc", query: query.trim() || null }, open);
  const { data: docTypes } = useDocTypes(open);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [kind, setKind] = useState<DocKind>(defaultKind);
  useEffect(() => {
    if (open) {
      setPicked(new Set());
      setQuery("");
      setKind(defaultKind);
    }
  }, [open, defaultKind]);
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
  const p = testIdPrefix;
  const choice = (selected: boolean) => cn("flex min-w-0 cursor-pointer items-center gap-2 rounded-md border px-3 py-2 text-sm", selected ? "border-accent bg-accent-soft" : "border-border hover:border-accent");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl" closeLabel={t("common.close")} data-testid={`${p}-dialog`}>
        <DialogHeader>
          <DialogTitle>{title ?? t("projects.documents.add_title")}</DialogTitle>
          <DialogDescription>{hint ?? t("projects.documents.add_hint")}</DialogDescription>
        </DialogHeader>
        <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("projects.documents.search")} aria-label={t("projects.documents.search")} data-testid={`${p}-search`} />
        <div className="max-h-80 min-w-0 overflow-y-auto rounded-md border border-border">
          {list.length === 0 ? (
            <p className="p-3 text-sm text-fg-muted">{t("projects.documents.none")}</p>
          ) : (
            <ul className="divide-y divide-border text-sm">
              {list.map((d) => (
                <li key={d.id}>
                  <label className="flex min-w-0 cursor-pointer items-center gap-3 px-3 py-2 hover:bg-bg-sunken">
                    <input type="checkbox" checked={picked.has(d.id)} onChange={() => toggle(d.id)} data-testid={`${p}-doc-${d.id}`} />
                    <span className="min-w-0 flex-1 truncate" title={d.original_name}>
                      {d.title ?? d.original_name}
                    </span>
                    <span className="shrink-0 text-xs text-fg-subtle">{docTypeLabel(d.doc_type, docTypes, t, i18n.language)}</span>
                  </label>
                </li>
              ))}
            </ul>
          )}
        </div>
        {withKind && (
          <fieldset className="flex min-w-0 flex-col gap-2">
            <legend className="text-xs font-semibold uppercase tracking-wide text-fg-subtle">{t("projects.put.kind")}</legend>
            <div className="grid gap-2 sm:grid-cols-2">
              <label className={choice(kind === "work")}>
                <input type="radio" name={`${p}-kind`} checked={kind === "work"} onChange={() => setKind("work")} data-testid={`${p}-kind-work`} />
                <span className="flex min-w-0 flex-col">
                  <span>{t("projects.put.kind_work")}</span>
                  <span className="text-xs text-fg-subtle">{t("projects.put.kind_work_hint")}</span>
                </span>
              </label>
              <label className={choice(kind === "reference")}>
                <input type="radio" name={`${p}-kind`} checked={kind === "reference"} onChange={() => setKind("reference")} data-testid={`${p}-kind-reference`} />
                <span className="flex min-w-0 flex-col">
                  <span>{t("projects.put.kind_reference")}</span>
                  <span className="text-xs text-fg-subtle">{t("projects.put.kind_reference_hint")}</span>
                </span>
              </label>
            </div>
          </fieldset>
        )}
        <DialogFooter>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button
            disabled={picked.size === 0}
            onClick={() => {
              onAdd([...picked], kind);
              onOpenChange(false);
            }}
            data-testid={`${p}-done`}
          >
            {t("projects.documents.add_done", { count: picked.size })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
