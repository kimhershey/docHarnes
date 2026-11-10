import { ExternalLink, FolderInput, Plus, X } from "lucide-react";
import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { docTypeLabel } from "@/features/pipeline/labels";
import { AddDocumentsDialog } from "@/features/projects/AddDocumentsDialog";
import { DropHint, importDroppedFiles, useDocumentDropTarget } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useCollectionDocs, useDocTypes } from "@/lib/queries";
import type { Collection } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

import { addToReferenceGroup, invalidateReference } from "./reference";

/**
 * The documents of one reference group — a common group or a project's own group (0.5.0 plan §2): pick from
 * the library, import files through the wizard, remove, and a drop target for library documents / OS files.
 * Test ids are prefixed (`reference-*` on the common view, `project-reference-*` on the project tab).
 */
export function ReferenceDocsPanel({ group, testIdPrefix: p, onIngest, heading }: { group: Collection; testIdPrefix: string; onIngest: () => void; heading?: ReactNode }) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const { data: docs, isLoading } = useCollectionDocs(group.id);
  const { data: docTypes } = useDocTypes();
  const [addOpen, setAddOpen] = useState(false);
  const add = async (ids: string[]) => {
    if (ids.length === 0) return;
    try {
      await addToReferenceGroup(group.id, ids);
      toast(t("projects.reference.added", { count: ids.length }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const remove = async (docId: string) => {
    try {
      await api.knowledgeCollectionRemove(group.id, [docId]);
      invalidateReference(group.id);
      toast(t("projects.reference.removed"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  // Library documents join the group at once; files are imported as 참고 자료 of the group (the import attaches them).
  const drop = useDocumentDropTarget({
    onDocuments: (dropped) => void add(dropped.map((d) => d.id)),
    onFiles: (paths) =>
      void importDroppedFiles(paths, { role: "knowledge", collectionId: group.id }, t).then((ids) => {
        invalidateReference(group.id);
        if (ids.length > 0) toast(t("projects.reference.added", { count: ids.length }), "ok");
      }),
  });
  const count = docs?.length ?? group.doc_count;
  return (
    <section className="relative flex flex-col gap-2" {...drop.props} data-testid={`${p}-drop`}>
      <DropHint active={drop.isActive} over={drop.isOver} testId={`${p}-drop-hint`}>
        {t("projects.reference.drop_add", { name: group.name })}
      </DropHint>
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-semibold" data-testid={`${p}-count`} data-count={count}>
          {heading ?? t("projects.reference.docs_title")} ({count})
        </h3>
        <div className="ml-auto flex items-center gap-2">
          <Button size="sm" variant="outline" onClick={onIngest} data-testid={`${p}-ingest`}>
            <FolderInput />
            {t("projects.reference.ingest")}
          </Button>
          <Button size="sm" onClick={() => setAddOpen(true)} data-testid={`${p}-add`}>
            <Plus />
            {t("projects.reference.add")}
          </Button>
        </div>
      </div>
      {isLoading && !docs ? (
        <Spinner />
      ) : !docs || docs.length === 0 ? (
        <p className="text-sm text-fg-muted" data-testid={`${p}-empty`}>
          {t("projects.reference.empty")} {t("projects.reference.drop_hint")}
        </p>
      ) : (
        <table className="w-full text-sm" data-testid={`${p}-table`}>
          <thead className="text-left text-xs uppercase tracking-wide text-fg-subtle">
            <tr>
              <th className="py-2 pr-3">{t("projects.reference.columns.name")}</th>
              <th className="py-2 pr-3">{t("projects.reference.columns.type")}</th>
              <th className="py-2 pr-3">{t("projects.reference.columns.pages")}</th>
              <th className="py-2 pr-3">{t("projects.reference.columns.status")}</th>
              <th className="py-2">
                <span className="sr-only">{t("projects.reference.columns.actions")}</span>
              </th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {docs.map((d) => (
              <tr key={d.id} data-testid={`${p}-doc-${d.id}`}>
                <td className="py-2 pr-3 font-medium">{d.title ?? d.original_name}</td>
                <td className="py-2 pr-3 text-fg-muted">{docTypeLabel(d.doc_type, docTypes, t, i18n.language)}</td>
                <td className="py-2 pr-3 text-fg-muted">{d.page_count}</td>
                <td className="py-2 pr-3 text-fg-muted">{t(`documents.status.${d.status}`)}</td>
                <td className="py-2">
                  <div className="flex justify-end gap-1">
                    <Button size="icon-sm" variant="ghost" onClick={() => navigate(`/library/${d.id}`)} aria-label={t("projects.reference.open_doc")}>
                      <ExternalLink />
                    </Button>
                    <Button size="icon-sm" variant="ghost" className="text-danger" onClick={() => void remove(d.id)} aria-label={t("projects.reference.remove")} data-testid={`${p}-remove`}>
                      <X />
                    </Button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <AddDocumentsDialog open={addOpen} onOpenChange={setAddOpen} excludeIds={(docs ?? []).map((d) => d.id)} onAdd={(ids) => void add(ids)} title={t("projects.reference.add_title")} hint={t("projects.reference.add_hint")} testIdPrefix={`${p}-pick`} />
    </section>
  );
}
