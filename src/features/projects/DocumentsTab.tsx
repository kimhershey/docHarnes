import { Check, ExternalLink, Plus, X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { docTypeLabel } from "@/features/pipeline/labels";
import { DropHint, importDroppedFiles, useDocumentDropTarget } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects, useDocTypes } from "@/lib/queries";
import type { ProjectDetail } from "@/lib/types";
import { cn, formatDate } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

import { AddDocumentsDialog } from "./AddDocumentsDialog";

/** 문서 tab: required-type checklist (missing marks) + the project's document table + add / remove. */
export function DocumentsTab({ detail }: { detail: ProjectDetail }) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const { data: docTypes } = useDocTypes();
  const [addOpen, setAddOpen] = useState(false);
  const id = detail.project.id;
  const add = async (ids: string[]) => {
    try {
      await api.projectAddDocuments(id, ids);
      invalidateProjects([id]);
      toast(t("projects.documents.added", { count: ids.length }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  // Library documents dropped here join the project; files are imported into it first.
  const drop = useDocumentDropTarget({
    onDocuments: (docs) => void add(docs.map((d) => d.id)),
    onFiles: (paths) =>
      void importDroppedFiles(paths, { projectId: id }, t).then((ids) => {
        invalidateProjects([id]);
        if (ids.length > 0) toast(t("projects.documents.added", { count: ids.length }), "ok");
      }),
  });
  const remove = async (docId: string) => {
    try {
      await api.projectRemoveDocument(id, docId);
      invalidateProjects([id]);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  return (
    <div className="flex flex-col gap-4" data-testid="project-documents">
      <section aria-labelledby="checklist-title" className="flex flex-col gap-2">
        <h3 id="checklist-title" className="text-sm font-semibold">
          {t("projects.documents.checklist")}
        </h3>
        {detail.checklist.length === 0 ? (
          <p className="text-xs text-fg-subtle">{t("projects.documents.no_checklist")}</p>
        ) : (
          <ul className="flex flex-wrap gap-2" data-testid="project-checklist">
            {detail.checklist.map((c) => (
              <li key={c.doc_type} className={cn("flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs", c.missing ? "border-danger bg-danger-soft text-danger" : "border-border bg-bg-elevated")} data-testid={`checklist-${c.doc_type}`} data-missing={c.missing}>
                {c.missing ? <X className="size-3.5" aria-hidden="true" /> : <Check className="size-3.5 text-ok" aria-hidden="true" />}
                <span>{i18n.language.startsWith("ko") ? c.label_ko : c.label_en}</span>
                <span className="text-fg-subtle">{c.missing ? t("projects.documents.missing") : t("projects.documents.present", { count: c.doc_ids.length })}</span>
                {!c.required && <Badge variant="muted">{t("projects.documents.extra")}</Badge>}
              </li>
            ))}
          </ul>
        )}
      </section>
      <section aria-labelledby="docs-title" className="relative flex flex-col gap-2" {...drop.props} data-testid="project-documents-drop">
        <DropHint active={drop.isActive} over={drop.isOver} testId="project-documents-drop-hint">
          {t("projects.documents.drop_hint")}
        </DropHint>
        <div className="flex items-center justify-between">
          <h3 id="docs-title" className="text-sm font-semibold">
            {t("projects.tabs.documents")} ({detail.documents.length})
          </h3>
          <Button size="sm" onClick={() => setAddOpen(true)} data-testid="project-add-documents">
            <Plus />
            {t("projects.documents.add")}
          </Button>
        </div>
        {detail.documents.length === 0 ? (
          <p className="text-sm text-fg-muted">
            {t("projects.documents.empty")} {t("projects.documents.drop_hint")}
          </p>
        ) : (
          <table className="w-full text-sm" data-testid="project-documents-table">
            <thead className="text-left text-xs uppercase tracking-wide text-fg-subtle">
              <tr>
                <th className="py-2 pr-3">{t("projects.documents.columns.name")}</th>
                <th className="py-2 pr-3">{t("projects.documents.columns.type")}</th>
                <th className="py-2 pr-3">{t("projects.documents.columns.status")}</th>
                <th className="py-2 pr-3">{t("projects.documents.columns.review")}</th>
                <th className="py-2 pr-3">{t("projects.documents.columns.added")}</th>
                <th className="py-2">
                  <span className="sr-only">{t("projects.documents.columns.actions")}</span>
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {detail.documents.map(({ document: d, added_at }) => (
                <tr key={d.id} data-testid={`project-doc-${d.id}`}>
                  <td className="py-2 pr-3 font-medium">{d.title ?? d.original_name}</td>
                  <td className="py-2 pr-3 text-fg-muted">{docTypeLabel(d.doc_type, docTypes, t, i18n.language)}</td>
                  <td className="py-2 pr-3 text-fg-muted">{t(`documents.status.${d.status}`)}</td>
                  <td className="py-2 pr-3 text-fg-muted">{d.review_status === "pending" ? t("library.review_pending") : d.review_status === "done" ? t("documents.status.reviewed") : "—"}</td>
                  <td className="py-2 pr-3 text-xs text-fg-subtle">{formatDate(added_at, i18n.language)}</td>
                  <td className="py-2">
                    <div className="flex justify-end gap-1">
                      <Button size="icon-sm" variant="ghost" onClick={() => navigate(`/library/${d.id}`)} aria-label={t("projects.documents.open")}>
                        <ExternalLink />
                      </Button>
                      <Button size="icon-sm" variant="ghost" className="text-danger" onClick={() => void remove(d.id)} aria-label={t("projects.documents.remove")} data-testid="project-doc-remove">
                        <X />
                      </Button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
      <AddDocumentsDialog open={addOpen} onOpenChange={setAddOpen} excludeIds={detail.documents.map((d) => d.document.id)} onAdd={(ids) => void add(ids)} />
    </div>
  );
}
