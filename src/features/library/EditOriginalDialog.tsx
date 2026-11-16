import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import { ArrowRight, Plus, X } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import type { DocumentView, EditReplacement, EditReport } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

/** The formats whose own file can be edited in place (core `documents::edit::supported`). */
export const EDITABLE_KINDS = ["hwpx", "docx", "pptx"];

interface Row {
  find: string;
  replace: string;
}

/**
 * 1.0.0 §2-1 "원본 고치기": text replacements inside the document's own file. The characters change where
 * they stand — font, size, tables, pictures and page setup stay — and the result is saved as a new edition
 * next to the original, which is not touched. The changes are shown before anything is written.
 */
export function EditOriginalDialog({ open, onOpenChange, doc, initialFind }: { open: boolean; onOpenChange: (open: boolean) => void; doc: DocumentView; initialFind?: string }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const [rows, setRows] = useState<Row[]>([{ find: "", replace: "" }]);
  const [report, setReport] = useState<EditReport | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    // A selection in the viewer is what the user wants to change (one line of it).
    const first = (initialFind ?? "").split(/\r?\n/)[0]?.trim() ?? "";
    setRows([{ find: first, replace: "" }]);
    setReport(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, doc.id]);

  const edits: EditReplacement[] = useMemo(() => rows.filter((r) => r.find.trim() !== "").map((r) => ({ find: r.find, replace: r.replace })), [rows]);
  // Which request row each replacement of the report belongs to.
  const editRow = useMemo(() => rows.map((r, i) => (r.find.trim() !== "" ? i : -1)).filter((i) => i >= 0), [rows]);
  const total = report?.replaced.reduce((a, b) => a + b, 0) ?? 0;

  const change = (i: number, patch: Partial<Row>) => {
    setRows((list) => list.map((r, k) => (k === i ? { ...r, ...patch } : r)));
    setReport(null);
  };
  const preview = async () => {
    setBusy(true);
    try {
      setReport(await api.documentsEditPreview(doc.id, edits));
    } catch (e) {
      setReport(null);
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  const save = async () => {
    setBusy(true);
    try {
      const edition = await api.documentsEditSave(doc.id, edits);
      const name = edition.rel_path.split("/").pop() ?? edition.rel_path;
      toast(t("viewer.edit_original.saved", { name, count: edition.report.replaced.reduce((a, b) => a + b, 0) }), "ok", {
        label: t("viewer.edit_original.open"),
        onClick: () => navigate(`/library/${edition.doc_id}`),
      });
      for (const w of edition.warnings) toast(w, "warn");
      onOpenChange(false);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl" data-testid="edit-original-dialog" closeLabel={t("common.close")}>
        <DialogHeader>
          <DialogTitle>{t("viewer.edit_original.title")}</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-fg-muted" data-testid="edit-original-lead">
          {t("viewer.edit_original.lead")}
        </p>
        <div className="flex flex-col gap-2">
          {rows.map((r, i) => {
            const n = editRow.indexOf(i);
            const found = report && n >= 0 ? (report.replaced[n] ?? 0) : null;
            return (
              <div key={i} className="flex min-w-0 items-center gap-2" data-testid={`edit-original-row-${i}`}>
                <Input className="min-w-0 flex-1" value={r.find} onChange={(e) => change(i, { find: e.target.value })} placeholder={t("viewer.edit_original.find")} aria-label={t("viewer.edit_original.find")} data-testid={`edit-original-find-${i}`} />
                <ArrowRight className="size-4 shrink-0 text-fg-muted" aria-hidden />
                <Input className="min-w-0 flex-1" value={r.replace} onChange={(e) => change(i, { replace: e.target.value })} placeholder={t("viewer.edit_original.replace")} aria-label={t("viewer.edit_original.replace")} data-testid={`edit-original-replace-${i}`} />
                {found !== null && (
                  <Badge variant={found > 0 ? "info" : "warn"} className="shrink-0" data-testid={`edit-original-count-${i}`}>
                    {found > 0 ? t("viewer.edit_original.places", { count: found }) : t("viewer.edit_original.not_found")}
                  </Badge>
                )}
                <Button size="icon-sm" variant="ghost" className="shrink-0" onClick={() => { setRows((list) => (list.length > 1 ? list.filter((_, k) => k !== i) : [{ find: "", replace: "" }])); setReport(null); }} aria-label={t("viewer.edit_original.remove_row")} data-testid={`edit-original-remove-${i}`}>
                  <X />
                </Button>
              </div>
            );
          })}
          <Button size="sm" variant="ghost" className="self-start" onClick={() => setRows((list) => [...list, { find: "", replace: "" }])} disabled={rows.length >= 20} data-testid="edit-original-add">
            <Plus />
            {t("viewer.edit_original.add_row")}
          </Button>
        </div>
        {report && (
          <div className="flex min-h-0 flex-col gap-1" data-testid="edit-original-preview" data-total={total}>
            <div className="text-sm font-medium">{total > 0 ? t("viewer.edit_original.summary", { count: total }) : t("viewer.edit_original.nothing")}</div>
            {report.changes.length > 0 && (
              <ul className="max-h-64 overflow-auto rounded-md border border-border text-sm">
                {report.changes.map((c, i) => (
                  <li key={i} className="flex min-w-0 flex-col gap-0.5 border-b border-border px-2 py-1.5 last:border-b-0" data-testid={`edit-original-change-${i}`}>
                    {c.unit != null && <span className="text-xs text-fg-muted">{t(c.part.startsWith("ppt/") ? "viewer.edit_original.slide" : "viewer.edit_original.section", { n: c.unit })}</span>}
                    <span className="break-words text-fg-muted line-through decoration-danger/60">{c.before}</span>
                    <span className="break-words">{c.after}</span>
                  </li>
                ))}
              </ul>
            )}
            {report.changes.length < total && <div className="text-xs text-fg-muted">{t("viewer.edit_original.more", { shown: report.changes.length })}</div>}
          </div>
        )}
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button variant="outline" onClick={() => void preview()} disabled={busy || edits.length === 0} data-testid="edit-original-preview-run">
            {t("viewer.edit_original.preview")}
          </Button>
          <Button onClick={() => void save()} disabled={busy || !report || total === 0} title={!report ? t("viewer.edit_original.preview_first") : undefined} data-testid="edit-original-save">
            {t("viewer.edit_original.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
