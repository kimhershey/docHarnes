import { useVirtualizer } from "@tanstack/react-virtual";
import { BookOpen, FileText, FolderInput, FolderOpen, Image as ImageIcon, LayoutGrid, List, Merge, RefreshCw, Trash2, Upload } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router";

import { Badge, badgeVariants } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { AddToCollectionDialog } from "@/features/knowledge/AddToCollectionDialog";
import { docTypeLabel } from "@/features/pipeline/labels";
import { PipelineStatusChip } from "@/features/pipeline/PipelineStatusChip";
import { ProcessMenu } from "@/features/pipeline/ProcessMenu";
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
import { useDocumentsStore } from "@/stores/documents";
import { usePipelineStore } from "@/stores/pipeline";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

import { ImportDocumentsDialog } from "./ImportDocumentsDialog";
import { MergeDialog } from "./SplitMergeDialogs";
import { statusVariant } from "./status";
import { useDocumentDrop } from "./useDocumentDrop";

const CARD_W = 200;
const CARD_H = 268;
const ROW_H = 44;
const SORTS: DocumentSort[] = ["imported_desc", "imported_asc", "name_asc", "name_desc", "size_desc"];

type TFn = (k: string, o?: Record<string, unknown>) => string;

function KindIcon({ kind, className }: { kind: SourceKind; className?: string }) {
  return kind === "image" ? <ImageIcon className={className} aria-hidden="true" /> : <FileText className={className} aria-hidden="true" />;
}

function Thumb({ doc, className }: { doc: DocumentView; className?: string }) {
  if (!doc.thumb_url) {
    return (
      <div className={cn("flex items-center justify-center bg-bg-sunken text-fg-subtle", className)}>
        <KindIcon kind={doc.source_kind} className="size-8" />
      </div>
    );
  }
  return <img src={doc.thumb_url} alt="" loading="lazy" className={cn("bg-white object-contain", className)} draggable={false} />;
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
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [importOpen, setImportOpen] = useState(false);
  const [importPaths, setImportPaths] = useState<string[] | undefined>(undefined);
  const [importSource, setImportSource] = useState<ImportSource>("drop");
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [addToKb, setAddToKb] = useState(false);
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
  // every list change (filters, folder, search, workspace switch, deletions).
  useEffect(() => {
    if (isPlaceholderData) return;
    setSelected((s) => {
      const next = new Set([...s].filter((id) => visibleIds.has(id)));
      return next.size === s.size ? s : next;
    });
  }, [visibleIds, isPlaceholderData]);

  // A folder that lost its last document falls back to "all".
  useEffect(() => {
    if (folder !== null && stats && !stats.folders.some((f) => f.rel_dir === folder)) setFolder(null);
  }, [folder, stats, setFolder]);

  const openImport = useCallback((paths?: string[], source: ImportSource = "drop") => {
    setImportPaths(paths);
    setImportSource(source);
    setImportOpen(true);
  }, []);
  const onDrop = useCallback((paths: string[]) => openImport(paths, "drop"), [openImport]);
  const dragOver = useDocumentDrop(!!workspace, onDrop);

  // `/library?import=1` (the home quick action) opens the import dialog once and drops the flag from the URL.
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

  const rerenderSelected = async () => {
    const ids = selectedVisible;
    if (ids.length === 0) return;
    try {
      await api.documentsRerender(ids, null);
      toast(t("library.rerender_started"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
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
        {(stats?.folders ?? []).map((f) => (
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

      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-2">
          <h1 ref={headingRef} tabIndex={-1} className="mr-2 text-lg font-semibold outline-none">
            {t("library.title")}
          </h1>
          <Button size="sm" onClick={() => openImport()} data-testid="library-import">
            <FolderInput />
            {t("library.import")}
          </Button>
          <Button size="sm" variant="secondary" onClick={() => void rescan()} data-testid="library-rescan">
            <RefreshCw />
            {t("library.rescan")}
          </Button>
          <label className="flex items-center gap-2 text-sm">
            <Switch checked={inboxWatch} onCheckedChange={(v) => void setInbox(v)} aria-describedby="inbox-watch-hint" data-testid="library-inbox-watch" />
            {t("library.inbox_watch")}
            <span id="inbox-watch-hint" className="sr-only">
              {t("library.inbox_watch_hint")}
            </span>
          </label>
          <div className="ml-auto flex items-center gap-2">
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
              {selectedVisible.length === 1 && (
                <Button size="sm" variant="ghost" onClick={() => navigate(`/schemas?register=${selectedVisible[0]}`)} data-testid="library-register-form">
                  <Upload />
                  {t("library.actions.register_form")}
                </Button>
              )}
              {selectedVisible.length >= 2 && (
                <Button size="sm" variant="ghost" onClick={() => setMergeOpen(true)} data-testid="library-merge">
                  <Merge />
                  {t("library.actions.merge")}
                </Button>
              )}
              <Button size="sm" variant="ghost" onClick={() => void rerenderSelected()} data-testid="library-rerender">
                <RefreshCw />
                {t("library.actions.rerender")}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setAddToKb(true)} data-testid="library-add-to-kb">
                <BookOpen />
                {t("library.actions.add_to_kb")}
              </Button>
              <Button size="sm" variant="ghost" className="text-danger" onClick={deleteSelected} data-testid="library-delete">
                <Trash2 />
                {t("library.actions.delete")}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setSelected(new Set())}>
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
              <GridView docs={list} selected={selected} onToggle={toggle} onOpen={open} onOpenReview={openReview} typeLabel={typeLabel} />
            ) : (
              <ListView docs={list} selected={selected} onToggle={toggle} onOpen={open} onOpenReview={openReview} typeLabel={typeLabel} locale={i18n.language} />
            )}
          </div>
        )}
      </div>

      <ImportDocumentsDialog open={importOpen} onOpenChange={setImportOpen} initialPaths={importPaths} source={importSource} />
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
      <AddToCollectionDialog open={addToKb} onOpenChange={setAddToKb} docIds={selectedVisible} onDone={() => setSelected(new Set())} />
      <MergeDialog open={mergeOpen} onOpenChange={setMergeOpen} ids={selectedVisible} onDone={() => setSelected(new Set())} />
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

function useColumns(ref: React.RefObject<HTMLDivElement | null>, itemWidth: number): number {
  const [cols, setCols] = useState(() => {
    const w = typeof window === "undefined" ? 1280 : window.innerWidth - 300;
    return Math.max(1, Math.floor((w - 16) / (itemWidth + 16)));
  });
  useEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    const update = () => setCols(Math.max(1, Math.floor((el.clientWidth - 16) / (itemWidth + 16))));
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref, itemWidth]);
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

function GridView({ docs, selected, onToggle, onOpen, onOpenReview, typeLabel }: { docs: DocumentView[]; selected: Set<string>; onToggle: (id: string) => void; onOpen: (id: string) => void; onOpenReview: (id: string) => void; typeLabel: (slug: string) => string }) {
  const { t } = useTranslation();
  const parentRef = useRef<HTMLDivElement | null>(null);
  const cols = useColumns(parentRef, CARD_W);
  const rows = Math.ceil(docs.length / cols);
  const virtualizer = useVirtualizer({ count: rows, getScrollElement: () => parentRef.current, estimateSize: () => CARD_H + 16, overscan: 3 });
  return (
    <div ref={parentRef} className="min-h-0 flex-1 overflow-y-auto p-2" role="list" aria-label={t("library.grid_label")} data-testid="library-grid">
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((row) => (
          <div key={row.key} className="absolute left-0 flex w-full gap-4 px-2" style={{ transform: `translateY(${row.start}px)`, height: row.size }}>
            {docs.slice(row.index * cols, row.index * cols + cols).map((doc) => (
              <DocCard key={doc.id} doc={doc} selected={selected.has(doc.id)} onToggle={() => onToggle(doc.id)} onOpen={() => onOpen(doc.id)} onOpenReview={onOpenReview} t={t} typeLabel={typeLabel} />
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

function DocCard({ doc, selected, onToggle, onOpen, onOpenReview, t, typeLabel }: { doc: DocumentView; selected: boolean; onToggle: () => void; onOpen: () => void; onOpenReview: (id: string) => void; t: TFn; typeLabel: (slug: string) => string }) {
  return (
    <div
      role="listitem"
      className={cn("group relative flex flex-col overflow-hidden rounded-lg border border-border bg-bg-elevated text-left transition-colors hover:border-accent", selected && "border-accent ring-2 ring-accent/40")}
      style={{ width: CARD_W, height: CARD_H }}
      data-testid={`doc-card-${doc.id}`}
      data-doc-name={doc.original_name}
      data-doc-status={doc.status}
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
        <Thumb doc={doc} className="h-44 w-full border-b border-border" />
        <div className="flex min-h-0 flex-1 flex-col gap-0.5 p-2 pb-1">
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
        <Badge variant={statusVariant(doc.status)} data-testid={`doc-status-${doc.id}`}>
          {t(`documents.status.${doc.status}`)}
        </Badge>
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

const LIST_COLS = "grid-cols-[28px_minmax(0,2fr)_80px_60px_100px_110px_90px_150px_90px]";

function ListView({ docs, selected, onToggle, onOpen, onOpenReview, typeLabel, locale }: { docs: DocumentView[]; selected: Set<string>; onToggle: (id: string) => void; onOpen: (id: string) => void; onOpenReview: (id: string) => void; typeLabel: (slug: string) => string; locale: string }) {
  const { t } = useTranslation();
  const parentRef = useRef<HTMLDivElement | null>(null);
  const virtualizer = useVirtualizer({ count: docs.length, getScrollElement: () => parentRef.current, estimateSize: () => ROW_H, overscan: 10, scrollMargin: ROW_H });
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
    <div ref={parentRef} className="min-h-0 flex-1 overflow-y-auto" role="grid" aria-label={t("library.list_label")} aria-rowcount={docs.length + 1} data-testid="library-list">
      <div role="row" className={cn("sticky top-0 z-10 grid gap-2 border-b border-border bg-bg px-3 py-1.5 text-xs font-medium text-fg-muted", LIST_COLS)} style={{ height: ROW_H }}>
        <span role="columnheader" aria-label={t("common.select")} />
        {headers.map((h) => (
          <span key={h} role="columnheader">
            {h}
          </span>
        ))}
      </div>
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((row) => {
          const doc = docs[row.index]!;
          return (
            <div
              key={doc.id}
              role="row"
              aria-rowindex={row.index + 2}
              aria-selected={selected.has(doc.id)}
              className={cn("absolute left-0 grid w-full items-center gap-2 border-b border-border px-3 text-sm hover:bg-bg-sunken", LIST_COLS, selected.has(doc.id) && "bg-accent-soft")}
              style={{ transform: `translateY(${row.start - virtualizer.options.scrollMargin}px)`, height: row.size }}
              data-testid={`doc-row-${doc.id}`}
              data-doc-name={doc.original_name}
              data-doc-status={doc.status}
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
                <Badge variant={statusVariant(doc.status)}>{t(`documents.status.${doc.status}`)}</Badge>
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
