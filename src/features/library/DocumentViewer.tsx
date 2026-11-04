import { useQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowLeft, BookOpen, ChevronLeft, ChevronRight, FolderOpen, GitBranch, Maximize2, MessageSquareText, RefreshCw, RotateCcw, RotateCw, Scissors, Trash2, X, ZoomIn, ZoomOut } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useParams, useSearchParams } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { AddToCollectionDialog } from "@/features/knowledge/AddToCollectionDialog";
import { confidenceGlyph, confidenceTone, DEFAULT_REVIEW_THRESHOLD, docTypeLabel, formatConfidence } from "@/features/pipeline/labels";
import { ProcessMenu } from "@/features/pipeline/ProcessMenu";
import { Snippet } from "@/features/search/SearchPage";
import { describeError } from "@/lib/errors";
import { api, revealPath } from "@/lib/ipc";
import { useDocTypes, useDocument, useDocumentChunks, useDocumentCollections, useDocumentPages, usePipelineStatus } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import { formatChord, isComposing, matchesChord } from "@/lib/shortcuts";
import type { Citation, DocumentView, PageView } from "@/lib/types";
import { cn, formatBytes, formatDate, isMac } from "@/lib/utils";
import { useShortcut } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";

import { FieldsPanel } from "./FieldsPanel";
import { HistoryPanel } from "./HistoryPanel";
import { SplitDialog } from "./SplitMergeDialogs";
import { statusVariant } from "./status";
import { TablesPanel } from "./TablesPanel";
import { VersionDialog } from "./VersionDialog";
import { ViewerKnowledgePanel } from "./ViewerKnowledgePanel";

const NONE = "__none__";
const THUMB_H = 128;

/** Keys the viewer owns; everything else (tabs, selects, buttons, inputs) keeps its own keyboard behaviour. */
function isViewerKeyTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return true;
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return false;
  if (target.isContentEditable) return false;
  return !target.closest('[role="tablist"],[role="tab"],[role="combobox"],[role="listbox"],[role="dialog"],button,a');
}

/**
 * Document viewer (design §3.7.6, P2 subset): page image with zoom, thumbnail
 * strip, rotation save, tags / type / title editing and the text-layer tab.
 * Fields / tables / Q&A / history tabs arrive with P3–P4.
 */
export function DocumentViewer() {
  const { id } = useParams<{ id: string }>();
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const { data: doc, error, isLoading } = useDocument(id);
  const { data: pages, isLoading: pagesLoading } = useDocumentPages(id, false);
  const [pageNo, setPageNo] = useState(1);
  const [zoom, setZoom] = useState<number | "fit">("fit");
  const [tagInput, setTagInput] = useState("");
  const [titleDraft, setTitleDraft] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [searchParams, setSearchParams] = useSearchParams();
  const highlightQuery = searchParams.get("q") ?? "";
  const chunkParam = searchParams.get("chunk");
  const { data: docChunks } = useDocumentChunks(chunkParam ? id : undefined);
  const explainChord = useShortcut("explain_selection");
  const [tab, setTab] = useState<string>("text");
  const [explainSelection, setExplainSelection] = useState<string | null>(null);
  const [selectionText, setSelectionText] = useState("");
  const [versionOpen, setVersionOpen] = useState(false);
  const [addToKb, setAddToKb] = useState(false);
  const [splitOpen, setSplitOpen] = useState(false);
  const [splitAt, setSplitAt] = useState<number | null>(null);
  const { data: docCollections } = useDocumentCollections(id);
  const { data: docTypes } = useDocTypes();
  const { data: pipeline } = usePipelineStatus();
  // Confidence colour / glyph follow the review threshold (design §3.7.4: colour, icon and label agree).
  const threshold = pipeline?.review_threshold ?? DEFAULT_REVIEW_THRESHOLD;

  // One component instance serves every /library/:id — reset per document.
  useEffect(() => {
    setPageNo(1);
    setZoom("fit");
    setTagInput("");
    setTitleDraft(null);
    setExplainSelection(null);
    setSelectionText("");
    rootRef.current?.focus();
  }, [id]);

  // Search / citation links open a specific page (`?page=N`) and highlight the query (`?q=`).
  const pageParam = searchParams.get("page");
  const tabParam = searchParams.get("tab");
  useEffect(() => {
    const n = Number(pageParam);
    if (pageParam && Number.isFinite(n) && n >= 1) setPageNo(n);
    if (tabParam && ["text", "fields", "tables", "qa", "history"].includes(tabParam)) setTab(tabParam);
    else if (searchParams.get("q") || searchParams.get("chunk")) setTab("text");
  }, [pageParam, tabParam, id, searchParams]);

  // Evidence click in the fields tab: jump to the page and highlight the quoted text.
  const goToEvidence = (page: number, needle?: string) => {
    setPageNo(page);
    if (needle) {
      const next = new URLSearchParams(searchParams);
      next.set("q", needle.slice(0, 80));
      next.delete("chunk");
      next.delete("tab");
      setSearchParams(next, { replace: true });
      setTab("text");
    }
  };
  const extractNow = async () => {
    if (!doc) return;
    try {
      await api.pipelineProcess({ document_ids: [doc.id], level: "extract" });
      toast(t("pipeline.process.started", { count: 1 }), "info");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const clearHighlight = () => {
    const next = new URLSearchParams(searchParams);
    next.delete("q");
    next.delete("chunk");
    setSearchParams(next, { replace: true });
  };

  const textRef = useRef<HTMLPreElement | null>(null);
  const captureSelection = useCallback(() => {
    const selection = window.getSelection();
    const anchor = selection?.anchorNode ?? null;
    const inside = !!anchor && !!textRef.current && textRef.current.contains(anchor);
    const sel = inside ? (selection?.toString().trim() ?? "") : "";
    setSelectionText(sel.length >= 2 ? sel : "");
  }, []);
  useEffect(() => {
    document.addEventListener("selectionchange", captureSelection);
    return () => document.removeEventListener("selectionchange", captureSelection);
  }, [captureSelection]);
  const explain = (selection?: string) => {
    const sel = (selection ?? selectionText).trim();
    if (!sel) return;
    setTab("qa");
    setExplainSelection(sel);
  };
  const onCitation = (c: Citation) => {
    if (c.document_id !== id) {
      navigate(`/library/${c.document_id}?page=${c.page_start}&chunk=${c.chunk_id}`);
      return;
    }
    setPageNo(c.page_start);
  };

  const total = pages?.length ?? 0;
  const page: PageView | undefined = pages?.find((p) => p.page_no === pageNo) ?? pages?.[0];
  const { data: text, isLoading: textLoading } = useQuery({
    queryKey: queryKeys.documentPageText(id ?? "", page?.page_no ?? 0),
    queryFn: () => api.documentsPageText(id ?? "", page?.page_no ?? 0),
    enabled: !!id && !!page,
  });

  const citedChunk = useMemo(() => (chunkParam ? docChunks?.find((c) => String(c.id) === chunkParam) : undefined), [chunkParam, docChunks]);
  const highlightNeedles = useMemo(() => {
    if (highlightQuery.trim()) return highlightQuery.split(/\s+/).map((w) => w.trim()).filter((w) => w.length >= 1);
    if (!citedChunk) return [];
    // A citation without a query: mark the opening lines of the cited passage.
    const body = citedChunk.heading_path && citedChunk.text.startsWith(citedChunk.heading_path) ? citedChunk.text.slice(citedChunk.heading_path.length) : citedChunk.text;
    return body
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l.length >= 4)
      .slice(0, 3)
      .map((l) => Array.from(l).slice(0, 80).join(""));
  }, [highlightQuery, citedChunk]);
  const highlights = useMemo<[number, number][]>(() => {
    if (!text || highlightNeedles.length === 0) return [];
    const chars = Array.from(text);
    const lower = chars.map((c) => c.toLowerCase());
    const out: [number, number][] = [];
    for (const n of [...highlightNeedles].sort((a, b) => b.length - a.length)) {
      const needle = Array.from(n.toLowerCase());
      for (let i = 0; i + needle.length <= lower.length; i += 1) {
        let ok = true;
        for (let k = 0; k < needle.length; k += 1) {
          if (lower[i + k] !== needle[k]) {
            ok = false;
            break;
          }
        }
        if (ok && !out.some(([a, b]) => i < b && i + needle.length > a)) {
          out.push([i, i + needle.length]);
          i += needle.length - 1;
        }
      }
    }
    return out.sort((a, b) => a[0] - b[0]);
  }, [text, highlightNeedles]);


  useEffect(() => {
    if (total > 0 && pageNo > total) setPageNo(total);
  }, [total, pageNo]);

  // New page → start at the top of the canvas.
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0, left: 0 });
  }, [pageNo]);

  const go = useCallback((delta: number) => setPageNo((p) => Math.min(Math.max(1, p + delta), Math.max(1, total))), [total]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (matchesChord(e.nativeEvent, explainChord, isMac())) {
      const selection = window.getSelection();
      const anchor = selection?.anchorNode ?? null;
      const sel = anchor && textRef.current?.contains(anchor) ? (selection?.toString().trim() ?? "") : selectionText;
      if (sel.length >= 2) {
        e.preventDefault();
        explain(sel);
      }
      return;
    }
    if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey || !isViewerKeyTarget(e.target)) return;
    if (e.key === "ArrowLeft" || e.key === "PageUp") go(-1);
    else if (e.key === "ArrowRight" || e.key === "PageDown") go(1);
    else if (e.key === "+" || e.key === "=") setZoom((z) => Math.min(4, (z === "fit" ? 1 : z) + 0.25));
    else if (e.key === "-") setZoom((z) => Math.max(0.25, (z === "fit" ? 1 : z) - 0.25));
    else if (e.key === "0") setZoom("fit");
    else return;
    e.preventDefault();
  };

  // Write-through cache updates: the command returns the fresh record, so no refetch is needed
  // (the `documents://changed` event still invalidates the library lists).
  const applyDoc = (view: DocumentView) => queryClient.setQueryData(queryKeys.document(view.id), view);

  const rotate = async (delta: number) => {
    if (!id || !page) return;
    const rotation = (page.rotation + delta + 360) % 360;
    try {
      const updated = await api.documentsRotatePage(id, page.page_no, rotation);
      queryClient.setQueryData<PageView[]>(queryKeys.documentPages(id, false), (old) =>
        old ? old.map((p) => (p.page_no === updated.page_no ? { ...updated, text: undefined } : p)) : old,
      );
      toast(t("viewer.rotated"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const rerender = async () => {
    if (!id) return;
    try {
      await api.documentsRerender([id], null);
      toast(t("library.rerender_started"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const reveal = async () => {
    if (!doc) return;
    try {
      await revealPath(doc.absolute_path);
    } catch (e) {
      toast(t("viewer.reveal_failed", { message: describeError(e, t) }), "danger");
    }
  };

  const remove = () => {
    if (!doc) return;
    setConfirm({
      title: t("viewer.delete_title"),
      description: t("viewer.delete_confirm", { name: doc.original_name }),
      confirmLabel: t("viewer.delete"),
      danger: true,
      onConfirm: async () => {
        try {
          await api.documentsDelete([doc.id], false);
          toast(t("viewer.deleted"), "ok");
          navigate("/library");
        } catch (e) {
          toast(describeError(e, t), "danger");
        }
      },
    });
  };

  const setTags = async (tags: string[]) => {
    if (!doc) return;
    try {
      applyDoc(await api.documentsSetTags(doc.id, tags));
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const addTag = () => {
    const v = tagInput.trim();
    if (!v || !doc) return;
    setTagInput("");
    if (doc.tags.includes(v)) return;
    void setTags([...doc.tags, v]);
  };

  const setDocType = async (v: string) => {
    if (!doc) return;
    try {
      applyDoc(await api.documentsSetDocType(doc.id, v === NONE ? null : v));
      // Design §3.7.3: a manual type change suggests re-extraction (never runs it automatically);
      // the offer is a real dialog so it can be answered, cancel just closes it.
      if (doc.last_run_id) {
        setConfirm({
          title: t("viewer.type_changed_title"),
          description: t("viewer.type_changed"),
          confirmLabel: t("viewer.reextract"),
          onConfirm: () => extractNow(),
        });
      }
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const commitTitle = async () => {
    if (!doc || titleDraft === null) return;
    const v = titleDraft.trim();
    setTitleDraft(null);
    if ((doc.title ?? "") === v) return;
    try {
      applyDoc(await api.documentsSetTitle(doc.id, v || null));
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const imageStyle = useMemo(() => {
    if (!page) return {};
    if (zoom === "fit") return { width: "100%" };
    return { width: `${Math.round(page.width_px * zoom)}px`, maxWidth: "none" };
  }, [page, zoom]);

  if (error) {
    return (
      <div className="p-6">
        <p className="text-danger" role="alert">
          {describeError(error, t)}
        </p>
        <Button variant="secondary" className="mt-3" onClick={() => navigate("/library")}>
          <ArrowLeft />
          {t("viewer.back")}
        </Button>
      </div>
    );
  }
  if (isLoading || !doc) {
    return (
      <div className="flex h-full items-center justify-center text-fg-muted" role="status">
        <Spinner />
      </div>
    );
  }

  const displayName = doc.title ?? doc.original_name;

  return (
    <div ref={rootRef} tabIndex={-1} onKeyDown={onKeyDown} className="flex h-full min-h-0 flex-col outline-none" data-testid="page-viewer" data-doc-id={doc.id}>
      <header className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-2">
        <Button size="sm" variant="ghost" onClick={() => navigate("/library")} data-testid="viewer-back">
          <ArrowLeft />
          {t("viewer.back")}
        </Button>
        <Input
          value={titleDraft ?? doc.title ?? ""}
          placeholder={doc.original_name}
          onChange={(e) => setTitleDraft(e.target.value)}
          onBlur={() => void commitTitle()}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !isComposing(e)) (e.target as HTMLInputElement).blur();
          }}
          className="h-8 w-64 font-medium"
          aria-label={t("viewer.title_placeholder")}
          data-testid="viewer-title"
        />
        <span className="truncate text-sm text-fg-muted" title={doc.rel_path} data-testid="viewer-name">
          {doc.original_name}
        </span>
        <Badge variant="outline">{t(`documents.kind.${doc.source_kind}`)}</Badge>
        <Badge variant={statusVariant(doc.status)} data-testid="viewer-status">
          {t(`documents.status.${doc.status}`)}
        </Badge>
        {doc.converted && (
          <Badge variant="info" title={doc.conversion_note ?? undefined} data-testid="viewer-converted">
            {t("viewer.converted_badge")}
          </Badge>
        )}
        {doc.role === "knowledge" && (
          <Badge variant="info" data-testid="viewer-kb-badge" title={docCollections?.map(([, name]) => name).join(", ")}>
            <BookOpen className="size-3" />
            {t("viewer.kb_badge")}
            {docCollections && docCollections.length > 0 && <span className="ml-1 font-normal">· {docCollections.map(([, name]) => name).join(", ")}</span>}
          </Badge>
        )}
        <Select value={doc.doc_type ?? NONE} onValueChange={(v) => void setDocType(v)}>
          <SelectTrigger className="h-8 w-44" aria-label={t("viewer.doc_type")} title={doc.doc_type_source ? t(`pipeline.doc_type_source.${doc.doc_type_source}`) : undefined} data-testid="viewer-doc-type">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NONE}>{t("viewer.no_doc_type")}</SelectItem>
            {[...(docTypes ?? []).map((d) => d.slug), ...(doc.doc_type && !docTypes?.some((d) => d.slug === doc.doc_type) && doc.doc_type !== "other" ? [doc.doc_type] : []), "other"].map((d) => (
              <SelectItem key={d} value={d}>
                {docTypeLabel(d, docTypes, t, i18n.language)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {doc.review_status !== "none" && (
          <Badge variant={doc.review_status === "pending" ? "warn" : "ok"} data-testid="viewer-review-badge">
            {t(`viewer.review_badge.${doc.review_status}`)}
          </Badge>
        )}
        {doc.confidence !== null && (
          <Badge variant={confidenceTone(doc.confidence, threshold)} title={t("viewer.confidence_badge", { value: formatConfidence(doc.confidence) })} data-testid="viewer-confidence">
            {confidenceGlyph(doc.confidence, threshold)} {formatConfidence(doc.confidence)}
          </Badge>
        )}
        {doc.pipeline_error && (
          <Badge variant="danger" title={doc.pipeline_error} data-testid="viewer-pipeline-error">
            {t("viewer.process.pipeline_error")}
          </Badge>
        )}
        <div role="group" className="flex flex-wrap items-center gap-1" aria-label={t("viewer.tags")} data-testid="viewer-tags">
          {doc.tags.map((tag) => (
            <Badge key={tag} variant="muted" className="gap-1">
              {tag}
              <button type="button" onClick={() => void setTags(doc.tags.filter((x) => x !== tag))} aria-label={t("viewer.remove_tag", { tag })} className="rounded-full hover:text-danger">
                <X className="size-3" />
              </button>
            </Badge>
          ))}
          <Input
            value={tagInput}
            onChange={(e) => setTagInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !isComposing(e)) {
                e.preventDefault();
                addTag();
              }
            }}
            placeholder={t("viewer.tag_placeholder")}
            className="h-7 w-36 text-xs"
            aria-label={t("viewer.add_tag")}
            data-testid="viewer-tag-input"
          />
        </div>
      </header>

      <div className="flex flex-wrap items-center gap-1 border-b border-border px-3 py-1.5" role="toolbar" aria-label={t("viewer.toolbar")}>
        <Button size="icon-sm" variant="ghost" onClick={() => go(-1)} disabled={pageNo <= 1} aria-label={t("viewer.prev")} data-testid="viewer-prev">
          <ChevronLeft />
        </Button>
        <span className="min-w-20 text-center text-sm tabular-nums" aria-live="polite" data-testid="viewer-page">
          {t("viewer.page", { page: total === 0 ? 0 : pageNo, total })}
        </span>
        <Button size="icon-sm" variant="ghost" onClick={() => go(1)} disabled={pageNo >= total} aria-label={t("viewer.next")} data-testid="viewer-next">
          <ChevronRight />
        </Button>
        <span className="mx-1 h-5 w-px bg-border" aria-hidden="true" />
        <Button size="icon-sm" variant="ghost" onClick={() => setZoom((z) => Math.max(0.25, (z === "fit" ? 1 : z) - 0.25))} aria-label={t("viewer.zoom_out")}>
          <ZoomOut />
        </Button>
        <span className="w-12 text-center text-xs tabular-nums" data-testid="viewer-zoom">
          {zoom === "fit" ? t("viewer.zoom_fit") : `${Math.round(zoom * 100)}%`}
        </span>
        <Button size="icon-sm" variant="ghost" onClick={() => setZoom((z) => Math.min(4, (z === "fit" ? 1 : z) + 0.25))} aria-label={t("viewer.zoom_in")}>
          <ZoomIn />
        </Button>
        <Button size="icon-sm" variant="ghost" onClick={() => setZoom("fit")} aria-label={t("viewer.zoom_fit")}>
          <Maximize2 />
        </Button>
        <span className="mx-1 h-5 w-px bg-border" aria-hidden="true" />
        <Button size="sm" variant="ghost" onClick={() => void rotate(-90)} disabled={!page?.image_url} aria-label={t("viewer.rotate_ccw")} data-testid="viewer-rotate-ccw">
          <RotateCcw />
        </Button>
        <Button size="sm" variant="ghost" onClick={() => void rotate(90)} disabled={!page?.image_url} aria-label={t("viewer.rotate_cw")} data-testid="viewer-rotate-cw">
          <RotateCw />
        </Button>
        <Button size="sm" variant="ghost" onClick={() => void rerender()} data-testid="viewer-rerender">
          <RefreshCw />
          {t("viewer.rerender")}
        </Button>
        <ProcessMenu ids={[doc.id]} label={t("viewer.process.menu")} variant="ghost" testId="viewer-process" />
        <Button size="sm" variant="ghost" onClick={() => { setSplitAt(pageNo > 1 ? pageNo : null); setSplitOpen(true); }} disabled={total < 2} data-testid="viewer-split">
          <Scissors />
          {t("viewer.split")}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => setAddToKb(true)} data-testid="viewer-add-to-kb">
          <BookOpen />
          {t("viewer.add_to_kb")}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => setVersionOpen(true)} data-testid="viewer-version">
          <GitBranch />
          {t("viewer.version.menu")}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => explain()} disabled={!selectionText} title={t("viewer.explain_with_chord", { chord: formatChord(explainChord, isMac()) })} data-testid="viewer-explain">
          <MessageSquareText />
          {t("viewer.explain_with_chord", { chord: formatChord(explainChord, isMac()) })}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => void reveal()} data-testid="viewer-reveal">
          <FolderOpen />
          {t("viewer.reveal")}
        </Button>
        <Button size="sm" variant="ghost" className="text-danger" onClick={remove} data-testid="viewer-delete">
          <Trash2 />
          {t("viewer.delete")}
        </Button>
        <span className="ml-auto text-xs text-fg-subtle">{t("viewer.shortcuts")}</span>
      </div>

      <div className="flex min-h-0 flex-1">
        <ThumbStrip pages={pages ?? []} current={pageNo} onSelect={setPageNo} />

        <div ref={scrollRef} className="min-w-0 flex-1 overflow-auto bg-bg-sunken p-4" data-testid="viewer-canvas">
          {pagesLoading && !pages ? (
            <div className="flex h-full items-center justify-center gap-2 text-sm text-fg-muted" role="status">
              <Spinner />
              {t("viewer.loading_pages")}
            </div>
          ) : page?.image_url ? (
            <img
              key={`${page.page_no}-${page.rendered_at ?? ""}`}
              src={page.image_url}
              alt={t("viewer.page_image_alt", { name: displayName, page: page.page_no })}
              className="mx-auto block bg-white shadow"
              style={imageStyle}
              draggable={false}
              data-testid="viewer-image"
            />
          ) : (
            <div className="flex h-full items-center justify-center text-sm text-fg-muted" data-testid="viewer-no-image">
              {total === 0 ? t("viewer.no_pages") : t("viewer.no_image")}
            </div>
          )}
        </div>

        <aside className="flex w-80 shrink-0 flex-col border-l border-border">
          <Tabs value={tab} onValueChange={setTab} className="flex min-h-0 flex-1 flex-col">
            <TabsList className="m-2">
              {(["text", "fields", "tables", "qa", "history"] as const).map((tab) => (
                <TabsTrigger key={tab} value={tab} data-testid={`viewer-tab-${tab}`}>
                  {t(`viewer.tabs.${tab}`)}
                </TabsTrigger>
              ))}
            </TabsList>
            <TabsContent value="text" className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
              {highlightQuery && (
                <div className="mb-2 flex items-center gap-2 rounded-md bg-warn-soft px-2 py-1 text-xs" data-testid="viewer-highlight">
                  <span className="truncate">{t("viewer.highlight", { q: highlightQuery })}</span>
                  <Button size="sm" variant="ghost" className="ml-auto h-6 px-2" onClick={clearHighlight}>
                    {t("viewer.clear_highlight")}
                  </Button>
                </div>
              )}
              {textLoading ? (
                <Spinner />
              ) : text ? (
                <pre ref={textRef} className="whitespace-pre-wrap break-words font-sans text-sm selectable" data-testid="viewer-text">
                  {highlights.length > 0 ? <Snippet text={text} highlights={highlights} /> : text}
                </pre>
              ) : (
                <p className="text-sm text-fg-muted" data-testid="viewer-text-empty">
                  {t("viewer.text_empty")}
                </p>
              )}
            </TabsContent>
            <TabsContent value="fields" className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
              <FieldsPanel key={doc.id} doc={doc} onGoToPage={goToEvidence} onStartExtract={() => void extractNow()} />
            </TabsContent>
            <TabsContent value="tables" className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
              <TablesPanel key={doc.id} doc={doc} onGoToPage={setPageNo} />
            </TabsContent>
            <TabsContent value="qa" forceMount className="min-h-0 flex-1 overflow-y-auto px-3 pb-3 data-[state=inactive]:hidden">
              <ViewerKnowledgePanel doc={doc} explainSelection={explainSelection} onExplained={() => setExplainSelection(null)} onCitation={onCitation} />
            </TabsContent>
            <TabsContent value="history" className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
              <HistoryPanel key={doc.id} doc={doc} />
            </TabsContent>
          </Tabs>
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 border-t border-border p-3 text-xs" data-testid="viewer-info">
            <dt className="text-fg-subtle">{t("viewer.info.path")}</dt>
            <dd className="truncate selectable" title={doc.absolute_path}>
              {doc.rel_path}
            </dd>
            <dt className="text-fg-subtle">{t("viewer.info.size")}</dt>
            <dd>{formatBytes(doc.size_bytes)}</dd>
            <dt className="text-fg-subtle">{t("viewer.info.pages")}</dt>
            <dd>{doc.page_count}</dd>
            <dt className="text-fg-subtle">{t("viewer.info.imported")}</dt>
            <dd>{formatDate(doc.imported_at, i18n.language)}</dd>
            <dt className="text-fg-subtle">{t("viewer.info.dpi")}</dt>
            <dd>{doc.render_dpi ?? "—"}</dd>
            <dt className="text-fg-subtle">{t("viewer.info.text_layer")}</dt>
            <dd>
              {doc.has_text_layer ? t("viewer.info.yes") : t("viewer.info.no")}
              {page?.text_source === "ocr" && (
                <Badge variant="info" className="ml-1 text-[10px]" data-testid="viewer-ocr-badge">
                  {t("viewer.ocr_badge")}
                </Badge>
              )}
            </dd>
            {doc.doc_type_source && (
              <>
                <dt className="text-fg-subtle">{t("viewer.info.doc_type_source")}</dt>
                <dd data-testid="viewer-doc-type-source">{t(`pipeline.doc_type_source.${doc.doc_type_source}`, { defaultValue: doc.doc_type_source })}</dd>
              </>
            )}
            {doc.conversion_note && (
              <>
                <dt className="text-fg-subtle">{t("viewer.conversion_note")}</dt>
                <dd className="whitespace-pre-wrap text-warn">{doc.conversion_note}</dd>
              </>
            )}
            {doc.error && (
              <>
                <dt className="text-fg-subtle">{t("common.error")}</dt>
                <dd className="whitespace-pre-wrap text-danger selectable">{doc.error}</dd>
              </>
            )}
            {doc.pipeline_error && (
              <>
                <dt className="text-fg-subtle">{t("viewer.process.pipeline_error")}</dt>
                <dd className="whitespace-pre-wrap text-danger selectable" data-testid="viewer-pipeline-error-detail">
                  {doc.pipeline_error}
                </dd>
              </>
            )}
          </dl>
        </aside>
      </div>
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
      <VersionDialog open={versionOpen} onOpenChange={setVersionOpen} doc={doc} />
      <AddToCollectionDialog open={addToKb} onOpenChange={setAddToKb} docIds={[doc.id]} />
      <SplitDialog open={splitOpen} onOpenChange={setSplitOpen} doc={doc} pages={pages ?? []} initial={splitAt} />
    </div>
  );
}

/** Virtualised page thumbnails (a scanned contract can have hundreds of pages). */
function ThumbStrip({ pages, current, onSelect }: { pages: PageView[]; current: number; onSelect: (page: number) => void }) {
  const { t } = useTranslation();
  const parentRef = useRef<HTMLDivElement | null>(null);
  const virtualizer = useVirtualizer({ count: pages.length, getScrollElement: () => parentRef.current, estimateSize: () => THUMB_H, overscan: 6 });
  useEffect(() => {
    if (pages.length > 0) virtualizer.scrollToIndex(Math.max(0, current - 1), { align: "auto" });
  }, [current, pages.length, virtualizer]);
  return (
    <nav ref={parentRef} className="hidden w-32 shrink-0 overflow-y-auto border-r border-border p-2 md:block" aria-label={t("viewer.thumbnails")} data-testid="viewer-thumbs">
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((row) => {
          const p = pages[row.index]!;
          return (
            <button
              key={p.page_no}
              type="button"
              onClick={() => onSelect(p.page_no)}
              className={cn("absolute left-0 flex w-full flex-col items-center gap-1 rounded-md border p-1 text-xs", p.page_no === current ? "border-accent bg-accent-soft" : "border-transparent hover:bg-bg-sunken")}
              style={{ transform: `translateY(${row.start}px)`, height: row.size - 8 }}
              aria-current={p.page_no === current ? "page" : undefined}
              aria-label={t("viewer.thumb_label", { page: p.page_no })}
              data-testid={`viewer-thumb-${p.page_no}`}
            >
              {p.thumb_url ? <img src={p.thumb_url} alt="" className="max-h-24 w-full bg-white object-contain" loading="lazy" draggable={false} /> : <div className="h-20 w-full bg-bg-sunken" />}
              <span>{p.page_no}</span>
            </button>
          );
        })}
      </div>
    </nav>
  );
}
