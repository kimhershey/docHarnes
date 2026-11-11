import { ArrowLeftRight, Check, ExternalLink, FolderInput, Plus, RefreshCw, X } from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { IngestWizard } from "@/features/knowledge/IngestWizard";
import { addToReferenceGroup, invalidateReference, useProjectReferenceDocs } from "@/features/knowledge/reference";
import { DocStatusText } from "@/features/library/DocStatusBadge";
import { docTypeLabel } from "@/features/pipeline/labels";
import { DropHint, importDroppedFiles, useDocumentDropTarget } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects, useDocTypes } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import type { CopyState, DocumentRecord, ProjectDetail, ProjectDocument } from "@/lib/types";
import { cn, formatDate } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

import { AddDocumentsDialog, type DocKind } from "./AddDocumentsDialog";

type Filter = "all" | DocKind;

/** One row of the unified table: a work document, a reference document, or a document that is both (shown as reference). */
interface Row {
  doc: DocumentRecord;
  kind: DocKind;
  both: boolean;
  added_at: string | null;
  copy: ProjectDocument["copy"] | null;
}

/**
 * The columns that give way when the tab is narrow (UI-4): each shows from a container width at which the name
 * column still keeps about 10 rem. Container queries are in rem, so a larger UI 배율 hides them like a narrower window.
 */
const COLUMN_FROM = {
  status: "hidden @xl:table-cell",
  review: "hidden @2xl:table-cell",
  added: "hidden @4xl:table-cell",
} as const;

function copyVariant(state: CopyState): "ok" | "warn" | "danger" | "muted" {
  switch (state) {
    case "synced":
      return "muted";
    case "modified":
      return "warn";
    case "missing":
      return "danger";
  }
}

/**
 * 문서 tab (0.7.0 plan §1.1 — one list for the project's 업무 문서 and 참고 자료): the required-type checklist
 * (work documents only), then one table with a 구분 column whose button switches a document between the two
 * kinds, filter chips with counts, one add dialog that asks for the kind, the import wizard pinned to the
 * project's reference group, the tracked-copy path / resync of work documents (plan §6, decision 6) and the
 * drop target (library documents and files join as 업무 문서). Storage is unchanged: work documents live in
 * `project_documents`, reference documents in the project's reference group; a document in both shows as
 * 참고 자료 (0.5.0: reference wins).
 */
export function DocumentsTab({ detail }: { detail: ProjectDetail }) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const { data: docTypes } = useDocTypes();
  const id = detail.project.id;
  const { group, docs: refDocs, isLoading: refLoading, error: refError } = useProjectReferenceDocs(id);
  const [addOpen, setAddOpen] = useState(false);
  const [wizard, setWizard] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [filter, setFilter] = useState<Filter>("all");
  const [switching, setSwitching] = useState<string | null>(null);

  const rows = useMemo<Row[]>(() => {
    const refIds = new Set((refDocs ?? []).map((d) => d.id));
    const workIds = new Set(detail.documents.map((d) => d.document.id));
    const work: Row[] = detail.documents.map(({ document, added_at, copy }) => ({ doc: document, kind: refIds.has(document.id) ? "reference" : "work", both: refIds.has(document.id), added_at, copy: copy ?? null }));
    const reference: Row[] = (refDocs ?? []).filter((d) => !workIds.has(d.id)).map((d) => ({ doc: d, kind: "reference", both: false, added_at: null, copy: null }));
    return [...work, ...reference];
  }, [detail.documents, refDocs]);
  const workCount = rows.filter((r) => r.kind === "work").length;
  const referenceCount = rows.length - workCount;
  const visible = filter === "all" ? rows : rows.filter((r) => r.kind === filter);
  const kindLabel = (kind: DocKind) => t(kind === "work" ? "projects.put.kind_work" : "projects.put.kind_reference");
  const name = (d: DocumentRecord) => d.title ?? d.original_name;

  /** Both lists plus the library's role badges follow a change. */
  const refresh = (groupId: string | null | undefined = group?.id) => {
    invalidateProjects([id]);
    invalidateReference(groupId);
    void queryClient.invalidateQueries({ queryKey: queryKeys.documents });
  };
  const groupId = async () => (group ?? (await api.projectReferenceGroup(id))).id;

  const add = async (ids: string[], kind: DocKind) => {
    if (ids.length === 0) return;
    try {
      if (kind === "work") {
        await api.projectAddDocuments(id, ids);
        refresh();
        toast(t("projects.documents.added", { count: ids.length }), "ok");
      } else {
        const gid = await groupId();
        await addToReferenceGroup(gid, ids);
        invalidateProjects([id]);
        toast(t("projects.reference.added", { count: ids.length }), "ok");
      }
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  // Library documents dropped here join the project as 업무 문서; files are imported into it first.
  const drop = useDocumentDropTarget({
    onDocuments: (docs) => void add(docs.map((d) => d.id), "work"),
    onFiles: (paths) =>
      void importDroppedFiles(paths, { projectId: id }, t).then((ids) => {
        refresh();
        if (ids.length > 0) toast(t("projects.documents.added", { count: ids.length }), "ok");
      }),
  });
  /** 업무 문서 ↔ 참고 자료: the group membership and the role follow (removal alone does not reset the role). */
  const switchKind = async (row: Row) => {
    setSwitching(row.doc.id);
    const target: DocKind = row.kind === "work" ? "reference" : "work";
    try {
      if (target === "reference") {
        await api.projectAddDocuments(id, [row.doc.id], true);
        await api.projectRemoveDocument(id, row.doc.id);
      } else {
        const gid = await groupId();
        await api.knowledgeCollectionRemove(gid, [row.doc.id]);
        await api.projectAddDocuments(id, [row.doc.id]);
        await api.knowledgeSetRole([row.doc.id], "work");
      }
      refresh();
      toast(t("projects.documents.kind_changed", { name: name(row.doc), kind: kindLabel(target) }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setSwitching(null);
    }
  };
  const remove = async (row: Row) => {
    try {
      if (row.kind === "reference") {
        const gid = await groupId();
        await api.knowledgeCollectionRemove(gid, [row.doc.id]);
      }
      if (row.kind === "work" || row.both) await api.projectRemoveDocument(id, row.doc.id);
      refresh();
      toast(t("projects.documents.removed"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const resync = async () => {
    setSyncing(true);
    try {
      const r = await api.projectSyncCopies(id);
      invalidateProjects([id]);
      toast(t("projects.documents.copy.resynced", { rewritten: r.rewritten + r.missing, renamed: r.renamed, created: r.created }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setSyncing(false);
    }
  };
  const hasCopies = detail.documents.some((d) => d.copy);
  const filters: { key: Filter; count: number }[] = [
    { key: "all", count: rows.length },
    { key: "work", count: workCount },
    { key: "reference", count: referenceCount },
  ];
  return (
    <div className="flex flex-col gap-4" data-testid="project-documents" data-group-id={group?.id ?? ""}>
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
      <section aria-labelledby="docs-title" className="@container relative flex flex-col gap-2" {...drop.props} data-testid="project-documents-drop">
        <DropHint active={drop.isActive} over={drop.isOver} testId="project-documents-drop-hint">
          {t("projects.documents.drop_hint")}
        </DropHint>
        <div className="flex flex-wrap items-center gap-2">
          <h3 id="docs-title" className="text-sm font-semibold">
            {t("projects.documents.title")} ({rows.length})
          </h3>
          <div className="flex flex-wrap items-center gap-1" role="group" aria-label={t("projects.documents.kind")}>
            {filters.map((f) => (
              <Button key={f.key} size="sm" variant={filter === f.key ? "secondary" : "ghost"} className="h-7 px-2" aria-pressed={filter === f.key} onClick={() => setFilter(f.key)} data-testid={`project-docs-filter-${f.key}`}>
                {t(`projects.documents.filter_${f.key}`, { count: f.count })}
              </Button>
            ))}
          </div>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            {hasCopies && (
              <Button size="sm" variant="outline" onClick={() => void resync()} disabled={syncing} title={t("projects.documents.copy.resync_hint")} data-testid="project-resync-copies">
                <RefreshCw className={cn(syncing && "animate-spin")} />
                {t("projects.documents.copy.resync")}
              </Button>
            )}
            <Button size="sm" variant="outline" onClick={() => setWizard(true)} disabled={!group} title={t("projects.documents.ingest_hint")} data-testid="project-reference-ingest">
              <FolderInput />
              {t("projects.reference.ingest")}
            </Button>
            <Button size="sm" onClick={() => setAddOpen(true)} data-testid="project-add-documents">
              <Plus />
              {t("projects.documents.add")}
            </Button>
          </div>
        </div>
        <p className="text-xs text-fg-subtle">{t("projects.documents.hint")}</p>
        {refError ? (
          <p className="text-sm text-danger" role="alert">
            {describeError(refError, t)}
          </p>
        ) : null}
        {rows.length === 0 ? (
          refLoading ? (
            <span className="flex items-center gap-2 text-sm text-fg-muted" role="status">
              <Spinner />
              {t("projects.reference.group_loading")}
            </span>
          ) : (
            <p className="text-sm text-fg-muted" data-testid="project-documents-empty">
              {t("projects.documents.empty")} {t("projects.documents.drop_hint")}
            </p>
          )
        ) : (
          <table className="w-full text-sm" data-testid="project-documents-table">
            {/* Column widths (UI-4): every column but the name is as wide as its content and never breaks (the page
                lets text break anywhere, which squeezed 유형 / 상태 / 추가일 to a character per line); the name takes
                what is left (`w-full max-w-0`: its own long content does not widen it) and carries the path as a
                second, truncated line. The less important columns show only when the tab is wide enough —
                container width in rem, so the UI 배율 counts like a narrower window. */}
            <thead className="text-left text-xs uppercase tracking-wide text-fg-subtle">
              <tr>
                <th className="py-2 pr-3">{t("projects.documents.columns.name")}</th>
                <th className="whitespace-nowrap py-2 pr-3">{t("projects.documents.columns.kind")}</th>
                <th className="whitespace-nowrap py-2 pr-3">{t("projects.documents.columns.type")}</th>
                <th className={cn("whitespace-nowrap py-2 pr-3", COLUMN_FROM.status)}>{t("projects.documents.columns.status")}</th>
                <th className={cn("whitespace-nowrap py-2 pr-3", COLUMN_FROM.review)}>{t("projects.documents.columns.review")}</th>
                <th className={cn("whitespace-nowrap py-2 pr-3", COLUMN_FROM.added)}>{t("projects.documents.columns.added")}</th>
                <th className="py-2">
                  <span className="sr-only">{t("projects.documents.columns.actions")}</span>
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {visible.map((row) => {
                const d = row.doc;
                const copy = row.copy;
                const other: DocKind = row.kind === "work" ? "reference" : "work";
                const path = copy ? copy.rel_path : d.rel_path;
                const typeLabel = docTypeLabel(d.doc_type, docTypes, t, i18n.language);
                return (
                  <tr key={d.id} data-testid={`project-doc-${d.id}`} data-kind={row.kind} data-both={row.both} data-copy-state={copy?.state ?? "original"}>
                    <td className="w-full min-w-[10rem] max-w-0 py-2 pr-3">
                      <div className="font-medium" data-testid="project-doc-name">
                        {name(d)}
                      </div>
                      <div className="mt-0.5 flex min-w-0 items-center gap-1 text-xs text-fg-muted">
                        <span className="sr-only">{t("projects.documents.columns.path")}</span>
                        <code className="selectable min-w-0 truncate" title={path} data-testid="project-doc-path">
                          {path}
                        </code>
                        {copy && (
                          <Badge variant={copyVariant(copy.state)} className="shrink-0" data-testid="project-doc-copy" data-state={copy.state}>
                            {t(`projects.documents.copy.${copy.state}`)}
                          </Badge>
                        )}
                      </div>
                    </td>
                    <td className="whitespace-nowrap py-2 pr-3">
                      <span className="flex flex-wrap items-center gap-1">
                        <Button size="sm" variant="outline" className="h-7 gap-1 px-2" onClick={() => void switchKind(row)} disabled={switching === d.id} title={t("projects.documents.kind_switch_hint", { kind: kindLabel(other) })} aria-label={t("projects.documents.kind_switch_hint", { kind: kindLabel(other) })} data-testid="project-doc-kind" data-kind={row.kind}>
                          <ArrowLeftRight className="size-3" aria-hidden="true" />
                          {kindLabel(row.kind)}
                        </Button>
                        {row.both && (
                          <Badge variant="muted" title={t("projects.documents.both_hint")} data-testid="project-doc-both">
                            {t("projects.documents.both")}
                          </Badge>
                        )}
                      </span>
                    </td>
                    <td className="py-2 pr-3 text-fg-muted" data-testid="project-doc-type">
                      {/* One line; only a very long (user-defined) type name is cut, with the whole name on hover. */}
                      <span className="block max-w-[12rem] truncate" title={typeLabel}>
                        {typeLabel}
                      </span>
                    </td>
                    <td className={cn("whitespace-nowrap py-2 pr-3 text-fg-muted", COLUMN_FROM.status)} data-testid="project-doc-status">
                      <DocStatusText doc={d} />
                    </td>
                    <td className={cn("whitespace-nowrap py-2 pr-3 text-fg-muted", COLUMN_FROM.review)} data-testid="project-doc-review">
                      {row.kind === "reference" ? "—" : d.review_status === "pending" ? t("library.review_pending") : d.review_status === "done" ? t("documents.status.reviewed") : "—"}
                    </td>
                    <td className={cn("whitespace-nowrap py-2 pr-3 text-xs text-fg-subtle", COLUMN_FROM.added)} data-testid="project-doc-added">
                      {row.added_at ? formatDate(row.added_at, i18n.language) : "—"}
                    </td>
                    <td className="py-2">
                      <div className="flex justify-end gap-1">
                        <Button size="icon-sm" variant="ghost" onClick={() => navigate(`/library/${d.id}`)} aria-label={t("projects.documents.open")}>
                          <ExternalLink />
                        </Button>
                        <Button size="icon-sm" variant="ghost" className="text-danger" onClick={() => void remove(row)} aria-label={t("projects.documents.remove")} data-testid="project-doc-remove">
                          <X />
                        </Button>
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </section>
      <AddDocumentsDialog open={addOpen} onOpenChange={setAddOpen} excludeIds={rows.map((r) => r.doc.id)} onAdd={(ids, kind) => void add(ids, kind)} withKind />
      {group && <IngestWizard open={wizard} collectionId={group.id} lockCollection onOpenChange={setWizard} />}
    </div>
  );
}
