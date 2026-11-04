import { useVirtualizer } from "@tanstack/react-virtual";
import { FileText, FolderInput, FolderOpen, Image as ImageIcon, LayoutGrid, List, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useDocumentStats, useDocuments, usePdfiumInfo } from "@/lib/queries";
import {
  DOCUMENT_STATUSES,
  SOURCE_KINDS,
  defaultImportOptions,
  type DocumentFilter,
  type DocumentSort,
  type DocumentStatus,
  type DocumentView,
  type SourceKind,
} from "@/lib/types";
import { cn, formatBytes, formatDate } from "@/lib/utils";
import { useDocumentsStore } from "@/stores/documents";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

import { ImportDocumentsDialog } from "./ImportDocumentsDialog";
import { useDocumentDrop } from "./useDocumentDrop";

const CARD_W = 200;
const CARD_H = 268;
const ROW_H = 44;

export function statusVariant(s: DocumentStatus): "ok" | "warn" | "danger" | "muted" | "info" {
  switch (s) {
    case "rendered":
    case "extracted":
    case "reviewed":
      return "ok";
    case "importing":
      return "info";
    case "error":
    case "missing":
      return "danger";
    case "new":
      return "warn";
    default:
      return "muted";
  }
}

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
  const [sort, setSort] = useState<DocumentSort>("imported_desc");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [importOpen, setImportOpen] = useState(false);
  const [dropped, setDropped] = useState<string[] | undefined>(undefined);
  const [debounced, setDebounced] = useState(query);

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
      sort,
    }),
    [status, kind, folder, debounced, sort],
  );
  const { data: docs, isLoading, error } = useDocuments(filter, !!workspace);
  const { data: stats } = useDocumentStats(!!workspace);
  const { data: pdfium } = usePdfiumInfo(!!workspace);

  const onDrop = useCallback((paths: string[]) => {
    setDropped(paths);
    setImportOpen(true);
  }, []);
  const dragOver = useDocumentDrop(!!workspace, onDrop);

  const openImport = () => {
    setDropped(undefined);
    setImportOpen(true);
  };

  const list = docs ?? [];
  const allSelected = list.length > 0 && list.every((d) => selected.has(d.id));
  const toggle = (id: string) =>
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const deleteSelected = async () => {
    const ids = Array.from(selected);
    if (ids.length === 0) return;
    if (!window.confirm(t("library.actions.delete_confirm", { count: ids.length }))) return;
    try {
      const removed = await api.documentsDelete(ids, false);
      toast(t("library.delete_done", { count: removed.length }), "ok");
      setSelected(new Set());
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const rerenderSelected = async () => {
    const ids = Array.from(selected);
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
      await api.documentsRescan();
      toast(t("library.rescan_started"), "info");
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

  const importExisting = async () => {
    if (!workspace) return;
    try {
      await api.documentsImportRun(null, [workspace.path], { ...defaultImportOptions(), destination: "documents" }, "folder");
      toast(t("import_docs.started"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const open = (id: string) => navigate(`/library/${id}`);

  return (
    <div className="relative flex h-full min-h-0" data-testid="page-library">
      {dragOver && (
        <div className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center bg-accent-soft/80 text-lg font-medium" data-testid="library-drop-overlay">
          {t("library.drop_overlay")}
        </div>
      )}
      <aside className="hidden w-56 shrink-0 flex-col border-r border-border p-3 md:flex" aria-label={t("library.folders.title")}>
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
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-2">
          <h1 className="mr-2 text-lg font-semibold">{t("library.title")}</h1>
          <Button size="sm" onClick={openImport} data-testid="library-import">
            <FolderInput />
            {t("library.import")}
          </Button>
          <Button size="sm" variant="secondary" onClick={() => void rescan()} aria-label={t("library.rescan")} data-testid="library-rescan">
            <RefreshCw />
            {t("library.rescan")}
          </Button>
          <label className="flex items-center gap-2 text-sm" title={t("library.inbox_watch_hint")}>
            <Switch checked={inboxWatch} onCheckedChange={(v) => void setInbox(v)} data-testid="library-inbox-watch" />
            {t("library.inbox_watch")}
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
            <Select value={sort} onValueChange={(v) => setSort(v as DocumentSort)}>
              <SelectTrigger className="h-8 w-36" aria-label={t("library.columns.imported")} data-testid="library-sort">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="imported_desc">{t("library.columns.imported")} ↓</SelectItem>
                <SelectItem value="imported_asc">{t("library.columns.imported")} ↑</SelectItem>
                <SelectItem value="name_asc">{t("library.columns.name")} A→Z</SelectItem>
                <SelectItem value="name_desc">{t("library.columns.name")} Z→A</SelectItem>
                <SelectItem value="size_desc">{t("library.columns.size")} ↓</SelectItem>
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
          <p className="border-b border-border bg-warn-soft px-4 py-1.5 text-xs text-warn" data-testid="library-pdfium-missing">
            {t("library.pdfium_missing")}
          </p>
        )}

        <div className="flex items-center gap-2 border-b border-border px-4 py-1.5 text-xs text-fg-muted" data-testid="library-stats">
          <span>{t("library.total", { count: stats?.total ?? 0, pages: stats?.total_pages ?? 0 })}</span>
          {selected.size > 0 && (
            <>
              <span className="mx-1">·</span>
              <span data-testid="library-selected">{t("library.selected", { count: selected.size })}</span>
              <Button size="sm" variant="ghost" onClick={() => void rerenderSelected()} data-testid="library-rerender">
                <RefreshCw />
                {t("library.actions.rerender")}
              </Button>
              <Button size="sm" variant="ghost" className="text-danger" onClick={() => void deleteSelected()} data-testid="library-delete">
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
          <div className="flex items-center gap-2 p-6 text-fg-muted">
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
                  <Button onClick={openImport} data-testid="library-empty-import">
                    <FolderInput />
                    {t("library.import")}
                  </Button>
                  <Button variant="secondary" onClick={() => void importExisting()} data-testid="library-import-existing">
                    {t("onboarding.workspace.import_existing")}
                  </Button>
                </div>
              </>
            )}
          </div>
        )}
        {docs && list.length > 0 && (view === "grid" ? (
          <GridView docs={list} selected={selected} onToggle={toggle} onOpen={open} />
        ) : (
          <ListView docs={list} selected={selected} onToggle={toggle} onOpen={open} locale={i18n.language} />
        ))}
      </div>

      <ImportDocumentsDialog open={importOpen} onOpenChange={setImportOpen} initialPaths={dropped} />
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
      data-testid={testId}
    >
      <span className="truncate">{label}</span>
      {count !== undefined && <span className="ml-2 text-xs text-fg-subtle">{count}</span>}
    </button>
  );
}

function useColumns(ref: React.RefObject<HTMLDivElement | null>, itemWidth: number): number {
  const [cols, setCols] = useState(4);
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

function GridView({ docs, selected, onToggle, onOpen }: { docs: DocumentView[]; selected: Set<string>; onToggle: (id: string) => void; onOpen: (id: string) => void }) {
  const { t } = useTranslation();
  const parentRef = useRef<HTMLDivElement | null>(null);
  const cols = useColumns(parentRef, CARD_W);
  const rows = Math.ceil(docs.length / cols);
  const virtualizer = useVirtualizer({ count: rows, getScrollElement: () => parentRef.current, estimateSize: () => CARD_H + 16, overscan: 3 });
  return (
    <div ref={parentRef} className="min-h-0 flex-1 overflow-y-auto p-2" data-testid="library-grid">
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((row) => (
          <div key={row.key} className="absolute left-0 flex w-full gap-4 px-2" style={{ transform: `translateY(${row.start}px)`, height: row.size }}>
            {docs.slice(row.index * cols, row.index * cols + cols).map((doc) => (
              <DocCard key={doc.id} doc={doc} selected={selected.has(doc.id)} onToggle={() => onToggle(doc.id)} onOpen={() => onOpen(doc.id)} t={t} />
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

function DocCard({ doc, selected, onToggle, onOpen, t }: { doc: DocumentView; selected: boolean; onToggle: () => void; onOpen: () => void; t: (k: string, o?: Record<string, unknown>) => string }) {
  return (
    <div
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
        aria-label={doc.original_name}
        data-testid={`doc-select-${doc.id}`}
      />
      <button type="button" className="flex min-h-0 flex-1 flex-col text-left" onClick={onOpen} onDoubleClick={onOpen} data-testid={`doc-open-${doc.id}`}>
        <Thumb doc={doc} className="h-44 w-full border-b border-border" />
        <div className="flex min-h-0 flex-1 flex-col gap-1 p-2">
          <span className="truncate text-sm font-medium" title={doc.original_name}>
            {doc.title ?? doc.original_name}
          </span>
          <span className="flex flex-wrap items-center gap-1 text-xs text-fg-muted">
            <Badge variant="outline">{t(`documents.kind.${doc.source_kind}`)}</Badge>
            <Badge variant={statusVariant(doc.status)} data-testid={`doc-status-${doc.id}`}>
              {t(`documents.status.${doc.status}`)}
            </Badge>
            {doc.converted && <Badge variant="info">{t("library.converted")}</Badge>}
          </span>
          <span className="text-xs text-fg-subtle">
            {t("library.pages_count", { count: doc.page_count })} · {formatBytes(doc.size_bytes)}
          </span>
        </div>
      </button>
    </div>
  );
}

function ListView({ docs, selected, onToggle, onOpen, locale }: { docs: DocumentView[]; selected: Set<string>; onToggle: (id: string) => void; onOpen: (id: string) => void; locale: string }) {
  const { t } = useTranslation();
  const parentRef = useRef<HTMLDivElement | null>(null);
  const virtualizer = useVirtualizer({ count: docs.length, getScrollElement: () => parentRef.current, estimateSize: () => ROW_H, overscan: 10 });
  return (
    <div ref={parentRef} className="min-h-0 flex-1 overflow-y-auto" data-testid="library-list">
      <div className="sticky top-0 z-10 grid grid-cols-[28px_minmax(0,2fr)_80px_60px_100px_150px_90px] gap-2 border-b border-border bg-bg px-3 py-1.5 text-xs font-medium text-fg-muted">
        <span />
        <span>{t("library.columns.name")}</span>
        <span>{t("library.columns.kind")}</span>
        <span>{t("library.columns.pages")}</span>
        <span>{t("library.columns.status")}</span>
        <span>{t("library.columns.imported")}</span>
        <span>{t("library.columns.size")}</span>
      </div>
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((row) => {
          const doc = docs[row.index]!;
          return (
            <div
              key={doc.id}
              className={cn("absolute left-0 grid w-full grid-cols-[28px_minmax(0,2fr)_80px_60px_100px_150px_90px] items-center gap-2 border-b border-border px-3 text-sm hover:bg-bg-sunken", selected.has(doc.id) && "bg-accent-soft")}
              style={{ transform: `translateY(${row.start}px)`, height: row.size }}
              data-testid={`doc-row-${doc.id}`}
              data-doc-name={doc.original_name}
            >
              <input type="checkbox" checked={selected.has(doc.id)} onChange={() => onToggle(doc.id)} aria-label={doc.original_name} />
              <button type="button" className="truncate text-left hover:underline" onClick={() => onOpen(doc.id)} title={doc.rel_path} data-testid={`doc-open-${doc.id}`}>
                {doc.title ?? doc.original_name}
                {doc.tags.length > 0 && <span className="ml-2 text-xs text-fg-subtle">{doc.tags.map((x) => `#${x}`).join(" ")}</span>}
              </button>
              <span className="text-xs">{t(`documents.kind.${doc.source_kind}`)}</span>
              <span className="text-xs tabular-nums">{doc.page_count}</span>
              <Badge variant={statusVariant(doc.status)}>{t(`documents.status.${doc.status}`)}</Badge>
              <span className="text-xs text-fg-muted">{formatDate(doc.imported_at, locale)}</span>
              <span className="text-xs tabular-nums text-fg-muted">{formatBytes(doc.size_bytes)}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
