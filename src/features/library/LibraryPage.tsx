import { useVirtualizer } from "@tanstack/react-virtual";
import { Briefcase, FileOutput, FileSpreadsheet, FileText, FolderInput, FolderOpen, FolderTree, Image as ImageIcon, LayoutGrid, List, Merge, Presentation, Trash2, Upload } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router";

import { remPx, useRootFontSize } from "@/app/hooks/useRootFontSize";
import { Badge, badgeVariants } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { docTypeLabel } from "@/features/pipeline/labels";
import { PipelineStatusChip } from "@/features/pipeline/PipelineStatusChip";
import { ProcessMenu } from "@/features/pipeline/ProcessMenu";
import { PutInProjectDialog } from "@/features/projects/PutInProjectDialog";
import { settingsTabPath } from "@/features/settings/tabs";
import { startDocumentDrag, type DragDoc } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useDocTypes, useDocumentStats, useDocuments, usePdfiumInfo } from "@/lib/queries";
import {
  DOCUMENT_STATUSES,
  SCREEN_ROUTES,
  SOURCE_KINDS,
  type DocumentFilter,
  type DocumentSort,
  type DocumentStatus,
  type DocumentView,
  type ImportSource,
  type ReviewStatus,
  type SourceKind,
} from "@/lib/types";
import { cn, formatBytes, formatDate } from "@/lib/utils";
import { useAttachmentsStore } from "@/stores/attachments";
import { useDocumentsStore } from "@/stores/documents";
import { usePipelineStore } from "@/stores/pipeline";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

import { ImportDocumentsDialog } from "./ImportDocumentsDialog";
import { LibraryContextMenu, type LibraryContextRequest } from "./LibraryContextMenu";
import { RetryImage } from "./RetryImage";
import { MergeDialog } from "./SplitMergeDialogs";
import { DocStatusBadge } from "./DocStatusBadge";
import { folderCount, listedFolders } from "./folders";
import { TodayStrip } from "./TodayStrip";
import { useDocumentDrop } from "./useDocumentDrop";

/** Card / row geometry in rem: it follows the UI 배율 together with the text it holds (the former px sizes clipped titles and badges from 125 % up). */
const CARD_W_REM = 12.5;
/** Row estimate until the virtualiser has measured the rendered cards (thumbnail 11 rem + title, meta and one badge row). */
const CARD_EST_REM = 18;
const LIST_ROW_REM = 2.75;
const SORTS: DocumentSort[] = ["imported_desc", "imported_asc", "name_asc", "name_desc", "size_desc"];

type TFn = (k: string, o?: Record<string, unknown>) => string;

function KindIcon({ kind, className }: { kind: SourceKind; className?: string }) {
  if (kind === "image") return <ImageIcon className={className} aria-hidden="true" />;
  if (kind === "xlsx" || kind === "xls") return <FileSpreadsheet className={className} aria-hidden="true" />;
  if (kind === "pptx" || kind === "ppt") return <Presentation className={className} aria-hidden="true" />;
  return <FileText className={className} aria-hidden="true" />;
}

function Thumb({ doc, className }: { doc: DocumentView; className?: string }) {
  const placeholder = (
    <div className={cn("flex items-center justify-center bg-bg-sunken text-fg-subtle", className)}>
      <KindIcon kind={doc.source_kind} className="size-8" />
    </div>
  );
  if (!doc.thumb_url) return placeholder;
  // A thumbnail purged from the page cache is re-rendered on request: retry before falling back to the icon.
  return <RetryImage src={doc.thumb_url} alt="" loading="lazy" className={cn("bg-white object-contain", className)} draggable={false} fallback={placeholder} />;
}

export function LibraryPage() {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const workspace = useWorkspace();
  const toast = useUiStore((s) => s.toast);
  const view = useDocumentsStore((s) => s.view);
  const setView = useDocumentsStore((s) => s.setView);
  const folder = useDocumentsStore((s) => s.folder);
  const setFolder = useDocumentsStore((s) => s.setFolder);
  const query = useDocumentsStore((s) => s.query);
  const setQuery = useDocumentsStore((s) => s.setQuery);
  const setSelectedIds = useDocumentsStore((s) => s.setSelectedIds);
  const inboxWatch = useSettingsStore((s) => s.settings?.processing.inbox_watch ?? true);
  const reloadSettings = useSettingsStore((s) => s.load);
  const [status, setStatus] = useState<DocumentStatus | "all">("all");
  const [kind, setKind] = useState<SourceKind | "all">("all");
  const [docType, setDocType] = useState<string>("all");
  const [review, setReview] = useState<ReviewStatus | "all">("all");
  const [mergeOpen, setMergeOpen] = useState(false);
  const notice = usePipelineStore((s) => s.notice);
  const clearNotice = usePipelineStore((s) => s.clearNotice);
  const openReviewForDocument = usePipelineStore((s) => s.openReviewForDocument);
  const { data: docTypes } = useDocTypes(!!workspace);
  const [sort, setSort] = useState<DocumentSort>("imported_desc");
  // Starts from the shared selection so coming back from 정리 / 내보내기 keeps the picked documents.
  const [selected, setSelected] = useState<Set<string>>(() => new Set(useDocumentsStore.getState().selectedIds));
  const [importOpen, setImportOpen] = useState(false);
  const [importPaths, setImportPaths] = useState<string[] | undefined>(undefined);
  const [importSource, setImportSource] = useState<ImportSource>("drop");
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [putInProject, setPutInProject] = useState(false);
  /** Right-click menu (다시 렌더 · 다시 검사 · inbox 감시): on a card / row or the empty space. */
  const [contextMenu, setContextMenu] = useState<LibraryContextRequest | null>(null);
  const [debounced, setDebounced] = useState(query);
  const headingRef = useRef<HTMLHeadingElement | null>(null);

  useEffect(() => {
    const h = setTimeout(() => setDebounced(query), 200);
    return () => clearTimeout(h);
  }, [query]);

  const filter: DocumentFilter = useMemo(
    () => ({
      statuses: status === "all" ? [] : [status],
      kinds: kind === "all" ? [] : [kind],
      folder,
      query: debounced || null,
      doc_type: docType === "all" ? null : docType,
      review_status: review === "all" ? null : review,
      sort,
    }),
    [status, kind, folder, debounced, docType, review, sort],
  );
  const { data: docs, isLoading, isPlaceholderData, error } = useDocuments(filter, !!workspace);
  const { data: stats } = useDocumentStats(!!workspace);
  const { data: pdfium } = usePdfiumInfo(!!workspace);

  const list = useMemo(() => docs ?? [], [docs]);
  const visibleIds = useMemo(() => new Set(list.map((d) => d.id)), [list]);

  // Selection only ever refers to documents that are currently listed: prune on
  // every list change (filters, folder, search, workspace switch, deletions) once the list is known.
  useEffect(() => {
    if (isPlaceholderData || !docs) return;
    setSelected((s) => {
      const next = new Set([...s].filter((id) => visibleIds.has(id)));
      return next.size === s.size ? s : next;
    });
  }, [visibleIds, isPlaceholderData, docs]);

  // A folder that lost its last document falls back to "all" — counting the folders below it, like the list
  // filter does: "받은 문서함 N건" selects `inbox` also when the files sit in `inbox/<sub>`, and that selection
  // used to be dropped at once (DOC-15).
  useEffect(() => {
    if (folder !== null && stats && folderCount(stats.folders, folder) === 0) setFolder(null);
  }, [folder, stats, setFolder]);
  const navFolders = useMemo(() => listedFolders(stats?.folders ?? [], folder), [stats, folder]);

  const openImport = useCallback((paths?: string[], source: ImportSource = "drop") => {
    setImportPaths(paths);
    setImportSource(source);
    setImportOpen(true);
  }, []);
  const onDrop = useCallback((paths: string[]) => openImport(paths, "drop"), [openImport]);
  const dragOver = useDocumentDrop(!!workspace, onDrop);
  // Files dropped on the sidebar's 라이브러리 entry arrive here once the screen is up.
  const pendingPaths = useAttachmentsStore((s) => s.pendingLibraryPaths);
  useEffect(() => {
    if (!pendingPaths || !workspace) return;
    useAttachmentsStore.getState().setPending({ pendingLibraryPaths: null });
    openImport(pendingPaths, "drop");
  }, [pendingPaths, workspace, openImport]);

  // `/library?import=1` (the assistant / palette "가져오기") opens the import dialog once and drops the flag from the URL.
  const importParam = searchParams.get("import");
  useEffect(() => {
    if (importParam !== "1") return;
    openImport();
    const next = new URLSearchParams(searchParams);
    next.delete("import");
    setSearchParams(next, { replace: true });
  }, [importParam, openImport, searchParams, setSearchParams]);

  // Selected documents in the library's current order: the merge dialog promises "라이브러리 순서대로",
  // and a `Set` would hand the backend the click order instead (review M7).
  const selectedDocs = useMemo(() => list.filter((d) => selected.has(d.id)), [list, selected]);
  const selectedVisible = useMemo(() => selectedDocs.map((d) => d.id), [selectedDocs]);
  // Mirror the selection into the store: the assistant's "선택 문서" scope and 정리 / 내보내기 read it
  // (it is kept on unmount; a workspace switch clears it in `resetDocumentQueries`).
  useEffect(() => {
    if (!docs) return;
    setSelectedIds(selectedVisible);
  }, [docs, selectedVisible, setSelectedIds]);
  // Drag payload of a card / row: the whole selection when the document is part of it, else the document alone.
  const dragDocs = useCallback(
    (doc: DocumentView): DragDoc[] => (selected.has(doc.id) ? selectedDocs : [doc]).map((d) => ({ id: d.id, name: d.title ?? d.original_name, absolute_path: d.absolute_path })),
    [selected, selectedDocs],
  );
  const allSelected = list.length > 0 && selectedVisible.length === list.length;
  const someSelected = selectedVisible.length > 0 && !allSelected;
  const toggle = (id: string) =>
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const deleteSelected = () => {
    const ids = selectedVisible;
    if (ids.length === 0) return;
    setConfirm({
      title: t("library.actions.delete_title"),
      description: t("library.actions.delete_confirm", { count: ids.length }),
      confirmLabel: t("library.actions.delete"),
      danger: true,
      onConfirm: async () => {
        try {
          const removed = await api.documentsDelete(ids, false);
          toast(t("library.delete_done", { count: removed.length }), "ok");
          setSelected(new Set());
          headingRef.current?.focus();
        } catch (e) {
          toast(describeError(e, t), "danger");
        }
      },
    });
  };

  /** Re-renders the right-clicked document, or the selection when it is part of it (none → the whole selection). */
  const rerender = async (docId: string | null) => {
    const ids = docId && !selected.has(docId) ? [docId] : selectedVisible.length > 0 ? selectedVisible : docId ? [docId] : [];
    if (ids.length === 0) return;
    try {
      await api.documentsRerender(ids, null);
      toast(t("library.rerender_started"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const onContextMenu = (e: React.MouseEvent<HTMLDivElement>) => {
    if (!workspace) return;
    const target = e.target as HTMLElement;
    if (target.closest("input, textarea, select, [role=dialog], [role=menu], [role=listbox]")) return;
    e.preventDefault();
    const docId = target.closest<HTMLElement>("[data-doc-id]")?.dataset.docId ?? null;
    const next = { x: e.clientX, y: e.clientY, docId };
    // A second right-click while the menu is open re-anchors it at the new point.
    if (contextMenu) {
      setContextMenu(null);
      window.requestAnimationFrame(() => setContextMenu(next));
    } else setContextMenu(next);
  };

  const rescan = async () => {
    try {
      const job = await api.documentsRescan();
      toast(job ? t("library.rescan_started") : t("library.rescan_running"), "info");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const setInbox = async (enabled: boolean) => {
    try {
      await api.documentsSetInboxWatch(enabled);
      await reloadSettings();
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const open = (id: string) => navigate(`/library/${id}`);
  // The "검토 대기" badge opens the review queue narrowed to that document (review M9).
  const openReview = useCallback(
    (id: string) => {
      openReviewForDocument(id);
      navigate(SCREEN_ROUTES.review);
    },
    [navigate, openReviewForDocument],
  );
  const typeLabel = useCallback((slug: string) => docTypeLabel(slug, docTypes, t, i18n.language), [docTypes, t, i18n.language]);

  return (
    <div className="relative flex h-full min-h-0" data-testid="page-library">
      {dragOver && (
        <div
          className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center bg-accent-soft/80 text-lg font-medium"
          role="status"
          aria-live="polite"
          data-testid="library-drop-overlay"
        >
          {t("library.drop_overlay")}
        </div>
      )}
      <nav className="hidden w-56 shrink-0 flex-col border-r border-border p-3 md:flex" aria-label={t("library.folders.title")}>
        <h2 className="mb-2 px-2 text-xs font-semibold uppercase tracking-wide text-fg-subtle">{t("library.folders.title")}</h2>
        <FolderItem label={t("library.folders.all")} count={stats?.total} active={folder === null} onClick={() => setFolder(null)} testId="folder-all" />
        {navFolders.map((f) => (
          <FolderItem
            key={f.rel_dir}
            label={f.rel_dir === "" ? t("library.folders.root") : f.rel_dir}
            count={f.count}
            active={folder === f.rel_dir}
            onClick={() => setFolder(f.rel_dir)}
            testId={`folder-${f.rel_dir || "root"}`}
          />
        ))}
      </nav>

      <div className="flex min-w-0 flex-1 flex-col" onContextMenu={onContextMenu} data-testid="library-main">
        <div className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-2">
          <h1 ref={headingRef} tabIndex={-1} className="mr-2 text-lg font-semibold outline-none">
            {t("library.title")}
          </h1>
          <Button size="sm" onClick={() => openImport()} data-testid="library-import">
            <FolderInput />
            {t("library.import")}
          </Button>
          {/* 정리 · 내보내기 open their screen scoped to the selection when there is one (plan §3). */}
          <Button size="sm" variant="secondary" onClick={() => navigate(selectedVisible.length > 0 ? "/organize?selection=1" : "/organize")} data-testid="library-organize">
            <FolderTree />
            {t("library.organize_selected")}
          </Button>
          <Button size="sm" variant="secondary" onClick={() => navigate(selectedVisible.length > 0 ? "/organize?tab=export&selection=1" : "/organize?tab=export")} data-testid="library-export">
            <FileOutput />
            {t("library.export_selected")}
          </Button>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t("library.search_placeholder")}
              className="h-8 w-48"
              aria-label={t("library.search_placeholder")}
              data-testid="library-search"
            />
            <Select value={status} onValueChange={(v) => setStatus(v as DocumentStatus | "all")}>
              <SelectTrigger className="h-8 w-32" aria-label={t("library.filters.status")} data-testid="library-filter-status">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">{t("library.filters.all")}</SelectItem>
                {DOCUMENT_STATUSES.map((s) => (
                  <SelectItem key={s} value={s}>
                    {t(`documents.status.${s}`)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select value={kind} onValueChange={(v) => setKind(v as SourceKind | "all")}>
              <SelectTrigger className="h-8 w-28" aria-label={t("library.filters.kind")} data-testid="library-filter-kind">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">{t("library.filters.all")}</SelectItem>
                {SOURCE_KINDS.map((k) => (
                  <SelectItem key={k} value={k}>
                    {t(`documents.kind.${k}`)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select value={docType} onValueChange={setDocType}>
              <SelectTrigger className="h-8 w-36" aria-label={t("library.filters.doc_type")} data-testid="library-filter-type">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">{t("library.filters.all")}</SelectItem>
                {[...(docTypes ?? []).map((d) => d.slug), "other"].map((d) => (
                  <SelectItem key={d} value={d}>
                    {typeLabel(d)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select value={review} onValueChange={(v) => setReview(v as ReviewStatus | "all")}>
              <SelectTrigger className="h-8 w-32" aria-label={t("library.filters.review")} data-testid="library-filter-review">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">{t("library.filters.all")}</SelectItem>
                {(["pending", "done", "none"] as const).map((r) => (
                  <SelectItem key={r} value={r}>
                    {t(`pipeline.review_status.${r}`)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select value={sort} onValueChange={(v) => setSort(v as DocumentSort)}>
              <SelectTrigger className="h-8 w-40" aria-label={t("library.sort")} data-testid="library-sort">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {SORTS.map((s) => (
                  <SelectItem key={s} value={s}>
                    {t(`library.sort_options.${s}`)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button size="icon-sm" variant={view === "grid" ? "secondary" : "ghost"} onClick={() => setView("grid")} aria-label={t("library.view_grid")} aria-pressed={view === "grid"} data-testid="library-view-grid">
              <LayoutGrid />
            </Button>
            <Button size="icon-sm" variant={view === "list" ? "secondary" : "ghost"} onClick={() => setView("list")} aria-label={t("library.view_list")} aria-pressed={view === "list"} data-testid="library-view-list">
              <List />
            </Button>
          </div>
        </div>

        <TodayStrip />

        {pdfium && !pdfium.library && (
          <p className="border-b border-border bg-warn-soft px-4 py-1.5 text-xs text-warn" role="status" data-testid="library-pdfium-missing">
            {t("library.pdfium_missing")}
          </p>
        )}

        {notice && (
          <p className="flex items-center gap-2 border-b border-border bg-warn-soft px-4 py-1.5 text-xs text-warn" role="status" data-testid="library-pipeline-notice">
            <span>{t(`pipeline.process.${notice}`)}</span>
            <Button size="sm" variant="ghost" className="ml-auto h-6" onClick={clearNotice}>
              {t("common.close")}
            </Button>
          </p>
        )}
        <div className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-1.5 text-xs text-fg-muted" data-testid="library-stats">
          <span>{t("library.total", { count: stats?.total ?? 0, pages: stats?.total_pages ?? 0 })}</span>
          <PipelineStatusChip />
          {selectedVisible.length > 0 && (
            <>
              <span className="mx-1">·</span>
              <span data-testid="library-selected">{t("library.selected", { count: selectedVisible.length })}</span>
              <ProcessMenu ids={selectedVisible} docs={selectedDocs} variant="ghost" testId="library-process" />
              <Button size="sm" variant="ghost" onClick={() => setPutInProject(true)} data-testid="library-put-in-project">
                <Briefcase />
                {t("library.actions.put_in_project")}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => navigate(settingsTabPath("schemas", { register: selectedVisible.join(",") }))} data-testid="library-register-form">
                <Upload />
                {t("library.actions.register_form")}
              </Button>
              {selectedVisible.length >= 2 && (
                <Button size="sm" variant="ghost" onClick={() => setMergeOpen(true)} data-testid="library-merge">
                  <Merge />
                  {t("library.actions.merge")}
                </Button>
              )}
              <Button size="sm" variant="ghost" className="text-danger" onClick={deleteSelected} data-testid="library-delete">
                <Trash2 />
                {t("library.actions.delete")}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setSelected(new Set())} data-testid="library-clear-selection">
                {t("library.actions.clear_selection")}
              </Button>
            </>
          )}
          {list.length > 0 && (
            <label className="ml-auto flex items-center gap-1.5">
              <input
                type="checkbox"
                checked={allSelected}
                ref={(el) => {
                  if (el) el.indeterminate = someSelected;
                }}
                onChange={(e) => setSelected(e.target.checked ? new Set(list.map((d) => d.id)) : new Set())}
                aria-label={t("library.actions.select_all")}
              />
              {t("library.actions.select_all")}
            </label>
          )}
        </div>

        {error && (
          <p className="px-4 py-2 text-sm text-danger" role="alert">
            {describeError(error, t)}
          </p>
        )}
        {isLoading && !docs && (
          <div className="flex items-center gap-2 p-6 text-fg-muted" role="status">
            <Spinner />
          </div>
        )}
        {docs && list.length === 0 && (
          <div className="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-center" data-testid="library-empty">
            <FolderOpen className="size-10 text-fg-subtle" aria-hidden="true" />
            <p className="font-medium">{(stats?.total ?? 0) === 0 ? t("library.empty_title") : t("library.no_match")}</p>
            {(stats?.total ?? 0) === 0 && (
              <>
                <p className="max-w-md text-sm text-fg-muted">{t("library.empty_body")}</p>
                <div className="flex gap-2">
                  <Button onClick={() => openImport()} data-testid="library-empty-import">
                    <FolderInput />
                    {t("library.import")}
                  </Button>
                  {workspace && (
                    <Button variant="secondary" title={t("library.import_existing_hint")} onClick={() => openImport([workspace.path], "folder")} data-testid="library-import-existing">
                      {t("onboarding.workspace.import_existing")}
                    </Button>
                  )}
                </div>
              </>
            )}
          </div>
        )}
        {docs && list.length > 0 && (
          <div className={cn("flex min-h-0 flex-1 flex-col transition-opacity", isPlaceholderData && "opacity-60")} aria-busy={isPlaceholderData}>
            {view === "grid" ? (
              <GridView docs={list} selected={selected} onToggle={toggle} onOpen={open} onOpenReview={openReview} typeLabel={typeLabel} dragDocs={dragDocs} />
            ) : (
              <ListView docs={list} selected={selected} onToggle={toggle} onOpen={open} onOpenReview={openReview} typeLabel={typeLabel} locale={i18n.language} dragDocs={dragDocs} />
            )}
          </div>
        )}
      </div>

      <ImportDocumentsDialog open={importOpen} onOpenChange={setImportOpen} initialPaths={importPaths} source={importSource} />
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
      <PutInProjectDialog open={putInProject} onOpenChange={setPutInProject} docIds={selectedVisible} onDone={() => setSelected(new Set())} />
      <MergeDialog open={mergeOpen} onOpenChange={setMergeOpen} ids={selectedVisible} onDone={() => setSelected(new Set())} />
      <LibraryContextMenu
        request={contextMenu}
        onClose={() => setContextMenu(null)}
        inboxWatch={inboxWatch}
        canRerender={contextMenu?.docId != null || selectedVisible.length > 0}
        onOpen={open}
        onRerender={(id) => void rerender(id)}
        onRescan={() => void rescan()}
        onToggleInbox={() => void setInbox(!inboxWatch)}
      />
    </div>
  );
}

function FolderItem({ label, count, active, onClick, testId }: { label: string; count?: number; active: boolean; onClick: () => void; testId: string }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn("flex w-full items-center justify-between rounded-md px-2 py-1.5 text-left text-sm hover:bg-bg-sunken", active && "bg-accent-soft font-medium")}
      aria-current={active ? "true" : undefined}
      title={label}
      data-testid={testId}
    >
      <span className="truncate">{label}</span>
      {count !== undefined && <span className="ml-2 text-xs text-fg-subtle">{count}</span>}
    </button>
  );
}

/** Cards per row. `gap` is the 1 rem between cards (half the grid's horizontal padding) in px at the current UI 배율. */
function useColumns(ref: React.RefObject<HTMLDivElement | null>, itemWidth: number, gap: number): number {
  const [cols, setCols] = useState(() => {
    const w = typeof window === "undefined" ? 1280 : window.innerWidth - 300;
    return Math.max(1, Math.floor((w - gap) / (itemWidth + gap)));
  });
  useEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    const update = () => setCols(Math.max(1, Math.floor((el.clientWidth - gap) / (itemWidth + gap))));
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref, itemWidth, gap]);
  return cols;
}

/** "검토 대기" as a control: opens the review queue filtered to the document (same badge in grid and list). */
function ReviewBadge({ doc, onOpen, t }: { doc: DocumentView; onOpen: (id: string) => void; t: TFn }) {
  const label = t("library.review_open", { name: doc.title ?? doc.original_name });
  return (
    <button
      type="button"
      className={cn(badgeVariants({ variant: "warn" }), "cursor-pointer hover:underline focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent")}
      onClick={(e) => {
        e.stopPropagation();
        onOpen(doc.id);
      }}
      title={label}
      aria-label={label}
      data-testid={`doc-review-${doc.id}`}
    >
      {t("library.review_pending")}
    </button>
  );
}

function GridView({ docs, selected, onToggle, onOpen, onOpenReview, typeLabel, dragDocs }: { docs: DocumentView[]; selected: Set<string>; dragDocs: (doc: DocumentView) => DragDoc[]; onToggle: (id: string) => void; onOpen: (id: string) => void; onOpenReview: (id: string) => void; typeLabel: (slug: string) => string }) {
  const { t } = useTranslation();
  const parentRef = useRef<HTMLDivElement | null>(null);
  const rootPx = useRootFontSize();
  const cols = useColumns(parentRef, remPx(CARD_W_REM, rootPx), rootPx);
  const rows = Math.ceil(docs.length / cols);
  // Rows are measured after render (`measureElement`): a card grows with its title and badge rows instead of clipping them.
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({ count: rows, getScrollElement: () => parentRef.current, estimateSize: () => remPx(CARD_EST_REM, rootPx), overscan: 3 });
  useEffect(() => {
    virtualizer.measure();
  }, [rootPx, virtualizer]);
  return (
    <div ref={parentRef} className="min-h-40 flex-1 overflow-y-auto p-2" role="list" aria-label={t("library.grid_label")} data-testid="library-grid">
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((row) => (
          <div key={row.key} ref={virtualizer.measureElement} data-index={row.index} className="absolute left-0 flex w-full gap-4 px-2 pb-4" style={{ transform: `translateY(${row.start}px)` }}>
            {docs.slice(row.index * cols, row.index * cols + cols).map((doc) => (
              <DocCard key={doc.id} doc={doc} selected={selected.has(doc.id)} onToggle={() => onToggle(doc.id)} onOpen={() => onOpen(doc.id)} onOpenReview={onOpenReview} t={t} typeLabel={typeLabel} dragDocs={dragDocs} />
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

function DocCard({ doc, selected, onToggle, onOpen, onOpenReview, t, typeLabel, dragDocs }: { doc: DocumentView; selected: boolean; dragDocs: (doc: DocumentView) => DragDoc[]; onToggle: () => void; onOpen: () => void; onOpenReview: (id: string) => void; t: TFn; typeLabel: (slug: string) => string }) {
  return (
    <div
      role="listitem"
      className={cn("group relative flex flex-col overflow-hidden rounded-lg border border-border bg-bg-elevated text-left transition-colors hover:border-accent", selected && "border-accent ring-2 ring-accent/40")}
      style={{ width: `${CARD_W_REM}rem` }}
      data-testid={`doc-card-${doc.id}`}
      data-doc-id={doc.id}
      data-doc-name={doc.original_name}
      data-doc-status={doc.status}
      data-dnd-source="document"
      onPointerDown={(e) => startDocumentDrag(e, () => dragDocs(doc))}
    >
      <input
        type="checkbox"
        className="absolute left-2 top-2 z-10 size-4"
        checked={selected}
        onChange={onToggle}
        onClick={(e) => e.stopPropagation()}
        aria-label={`${t("common.select")}: ${doc.original_name}`}
        data-testid={`doc-select-${doc.id}`}
      />
      <button type="button" className="flex min-h-0 flex-1 flex-col text-left" onClick={onOpen} aria-label={`${t("library.actions.open_doc")}: ${doc.title ?? doc.original_name}`} data-testid={`doc-open-${doc.id}`}>
        <Thumb doc={doc} className="h-44 w-full shrink-0 border-b border-border" />
        <div className="flex flex-col gap-0.5 p-2 pb-1">
          <span className="truncate text-sm font-medium" title={doc.original_name}>
            {doc.title ?? doc.original_name}
          </span>
          <span className="text-xs text-fg-subtle">
            {t("library.pages_count", { count: doc.page_count })} · {formatBytes(doc.size_bytes)}
          </span>
        </div>
      </button>
      {/* Badges live outside the open button so the review badge can be a button of its own (no nested controls). */}
      <div className="flex shrink-0 flex-wrap items-center gap-1 px-2 pb-2 text-xs text-fg-muted">
        <Badge variant="outline">{t(`documents.kind.${doc.source_kind}`)}</Badge>
        <DocStatusBadge doc={doc} testId={`doc-status-${doc.id}`} />
        {doc.converted && <Badge variant="info">{t("library.converted")}</Badge>}
        {doc.doc_type && (
          <Badge variant="outline" data-testid={`doc-type-${doc.id}`}>
            {typeLabel(doc.doc_type)}
          </Badge>
        )}
        {doc.review_status === "pending" && <ReviewBadge doc={doc} onOpen={onOpenReview} t={t} />}
        {doc.role === "knowledge" && (
          <Badge variant="info" data-testid={`doc-role-${doc.id}`}>
            {t("library.role_knowledge")}
          </Badge>
        )}
      </div>
    </div>
  );
}

/** Column tracks in rem so they grow with the UI 배율 like the cell text (px tracks wrapped the date and size cells from 125 % up). */
const LIST_COLS = "grid-cols-[1.75rem_minmax(0,2fr)_5rem_3.75rem_6.25rem_6.875rem_5.625rem_9.375rem_5.625rem]";
/** Fixed tracks + gaps + padding come to ~50 rem; below that the name column would vanish, so the list scrolls sideways instead. */
const LIST_MIN_W = "min-w-[58rem]";

function ListView({ docs, selected, onToggle, onOpen, onOpenReview, typeLabel, locale, dragDocs }: { docs: DocumentView[]; selected: Set<string>; dragDocs: (doc: DocumentView) => DragDoc[]; onToggle: (id: string) => void; onOpen: (id: string) => void; onOpenReview: (id: string) => void; typeLabel: (slug: string) => string; locale: string }) {
  const { t } = useTranslation();
  const parentRef = useRef<HTMLDivElement | null>(null);
  const rootPx = useRootFontSize();
  const rowPx = remPx(LIST_ROW_REM, rootPx);
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({ count: docs.length, getScrollElement: () => parentRef.current, estimateSize: () => rowPx, overscan: 10, scrollMargin: rowPx });
  useEffect(() => {
    virtualizer.measure();
  }, [rootPx, virtualizer]);
  const headers = [
    t("library.columns.name"),
    t("library.columns.kind"),
    t("library.columns.pages"),
    t("library.columns.status"),
    t("library.columns.doc_type"),
    t("library.columns.review"),
    t("library.columns.imported"),
    t("library.columns.size"),
  ];
  return (
    <div ref={parentRef} className="min-h-40 flex-1 overflow-auto" role="grid" aria-label={t("library.list_label")} aria-rowcount={docs.length + 1} data-testid="library-list">
      <div role="row" className={cn("sticky top-0 z-10 grid items-center gap-2 border-b border-border bg-bg px-3 text-xs font-medium text-fg-muted", LIST_COLS, LIST_MIN_W)} style={{ height: rowPx }}>
        <span role="columnheader" aria-label={t("common.select")} />
        {headers.map((h) => (
          <span key={h} role="columnheader">
            {h}
          </span>
        ))}
      </div>
      <div className={LIST_MIN_W} style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((row) => {
          const doc = docs[row.index]!;
          return (
            <div
              key={doc.id}
              ref={virtualizer.measureElement}
              data-index={row.index}
              role="row"
              aria-rowindex={row.index + 2}
              aria-selected={selected.has(doc.id)}
              className={cn("absolute left-0 grid w-full items-center gap-2 border-b border-border px-3 py-1 text-sm hover:bg-bg-sunken", LIST_COLS, selected.has(doc.id) && "bg-accent-soft")}
              style={{ transform: `translateY(${row.start - virtualizer.options.scrollMargin}px)`, minHeight: rowPx }}
              data-testid={`doc-row-${doc.id}`}
              data-doc-id={doc.id}
              data-doc-name={doc.original_name}
              data-doc-status={doc.status}
              data-dnd-source="document"
              onPointerDown={(e) => startDocumentDrag(e, () => dragDocs(doc))}
            >
              <span role="gridcell">
                <input type="checkbox" checked={selected.has(doc.id)} onChange={() => onToggle(doc.id)} aria-label={`${t("common.select")}: ${doc.original_name}`} />
              </span>
              <span role="gridcell" className="min-w-0">
                <button type="button" className="max-w-full truncate text-left hover:underline" onClick={() => onOpen(doc.id)} title={doc.rel_path} data-testid={`doc-open-${doc.id}`}>
                  {doc.title ?? doc.original_name}
                  {doc.role === "knowledge" && <span className="ml-2 rounded bg-info-soft px-1 text-xs text-info">{t("library.role_knowledge")}</span>}
                  {doc.tags.length > 0 && <span className="ml-2 text-xs text-fg-subtle">{doc.tags.map((x) => `#${x}`).join(" ")}</span>}
                </button>
              </span>
              <span role="gridcell" className="text-xs">
                {t(`documents.kind.${doc.source_kind}`)}
              </span>
              <span role="gridcell" className="text-xs tabular-nums">
                {doc.page_count}
              </span>
              <span role="gridcell">
                <DocStatusBadge doc={doc} />
              </span>
              <span role="gridcell" className="min-w-0">
                {doc.doc_type && (
                  <Badge variant="outline" className="max-w-full truncate" data-testid={`doc-type-${doc.id}`}>
                    {typeLabel(doc.doc_type)}
                  </Badge>
                )}
              </span>
              <span role="gridcell">
                {doc.review_status === "pending" && <ReviewBadge doc={doc} onOpen={onOpenReview} t={t} />}
                {doc.review_status === "done" && (
                  <Badge variant="ok" data-testid={`doc-review-done-${doc.id}`}>
                    {t("pipeline.review_status.done")}
                  </Badge>
                )}
              </span>
              <span role="gridcell" className="text-xs text-fg-muted">
                {formatDate(doc.imported_at, locale)}
              </span>
              <span role="gridcell" className="text-xs tabular-nums text-fg-muted">
                {formatBytes(doc.size_bytes)}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
