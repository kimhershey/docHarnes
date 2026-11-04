import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, ChevronLeft, ChevronRight, FolderOpen, Maximize2, RefreshCw, RotateCcw, RotateCw, Trash2, X, ZoomIn, ZoomOut } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useParams } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { describeError } from "@/lib/errors";
import { api, revealPath } from "@/lib/ipc";
import { useDocument, useDocumentPages } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import { isComposing } from "@/lib/shortcuts";
import type { PageView } from "@/lib/types";
import { cn, formatBytes, formatDate } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

import { statusVariant } from "./LibraryPage";

const DOC_TYPES = ["tax_invoice", "invoice_exempt", "receipt", "business_registration", "contract", "application_form", "general", "other"];
const NONE = "__none__";

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
  const { data: pages } = useDocumentPages(id, false);
  const [pageNo, setPageNo] = useState(1);
  const [zoom, setZoom] = useState<number | "fit">("fit");
  const [tagInput, setTagInput] = useState("");
  const [titleDraft, setTitleDraft] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const total = pages?.length ?? 0;
  const page: PageView | undefined = pages?.find((p) => p.page_no === pageNo) ?? pages?.[0];
  const { data: text, isLoading: textLoading } = useQuery({
    queryKey: queryKeys.documentPageText(id ?? "", page?.page_no ?? 0),
    queryFn: () => api.documentsPageText(id ?? "", page?.page_no ?? 0),
    enabled: !!id && !!page,
  });

  useEffect(() => {
    if (total > 0 && pageNo > total) setPageNo(total);
  }, [total, pageNo]);

  const go = useCallback(
    (delta: number) => setPageNo((p) => Math.min(Math.max(1, p + delta), Math.max(1, total))),
    [total],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement || e.ctrlKey || e.metaKey) return;
      if (e.key === "ArrowLeft" || e.key === "PageUp") go(-1);
      else if (e.key === "ArrowRight" || e.key === "PageDown") go(1);
      else if (e.key === "+" || e.key === "=") setZoom((z) => Math.min(4, (z === "fit" ? 1 : z) + 0.25));
      else if (e.key === "-") setZoom((z) => Math.max(0.25, (z === "fit" ? 1 : z) - 0.25));
      else if (e.key === "0") setZoom("fit");
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [go]);

  const invalidate = () => {
    if (!id) return;
    void queryClient.invalidateQueries({ queryKey: queryKeys.document(id) });
    void queryClient.invalidateQueries({ queryKey: queryKeys.documentPages(id, false) });
  };

  const rotate = async (delta: number) => {
    if (!id || !page) return;
    const rotation = (page.rotation + delta + 360) % 360;
    try {
      await api.documentsRotatePage(id, page.page_no, rotation);
      invalidate();
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

  const remove = async () => {
    if (!doc) return;
    if (!window.confirm(t("viewer.delete_confirm", { name: doc.original_name }))) return;
    try {
      await api.documentsDelete([doc.id], false);
      toast(t("viewer.deleted"), "ok");
      navigate("/library");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const setTags = async (tags: string[]) => {
    if (!doc) return;
    try {
      await api.documentsSetTags(doc.id, tags);
      invalidate();
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
      await api.documentsSetDocType(doc.id, v === NONE ? null : v);
      invalidate();
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
      await api.documentsSetTitle(doc.id, v || null);
      invalidate();
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
      <div className="flex h-full items-center justify-center text-fg-muted">
        <Spinner />
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="page-viewer" data-doc-id={doc.id}>
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
        <Select value={doc.doc_type ?? NONE} onValueChange={(v) => void setDocType(v)}>
          <SelectTrigger className="h-8 w-40" aria-label={t("viewer.doc_type")} data-testid="viewer-doc-type">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NONE}>{t("viewer.no_doc_type")}</SelectItem>
            {DOC_TYPES.map((d) => (
              <SelectItem key={d} value={d}>
                {t(`documents.doc_types.${d}`)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <div className="flex flex-wrap items-center gap-1" aria-label={t("viewer.tags")} data-testid="viewer-tags">
          {doc.tags.map((tag) => (
            <Badge key={tag} variant="muted" className="gap-1">
              {tag}
              <button type="button" onClick={() => void setTags(doc.tags.filter((x) => x !== tag))} aria-label={`${t("common.close")} ${tag}`} className="rounded-full hover:text-danger">
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

      <div className="flex flex-wrap items-center gap-1 border-b border-border px-3 py-1.5">
        <Button size="icon-sm" variant="ghost" onClick={() => go(-1)} disabled={pageNo <= 1} aria-label={t("viewer.prev")} data-testid="viewer-prev">
          <ChevronLeft />
        </Button>
        <span className="min-w-20 text-center text-sm tabular-nums" data-testid="viewer-page">
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
        <Button size="sm" variant="ghost" onClick={() => void revealPath(doc.absolute_path)} data-testid="viewer-reveal">
          <FolderOpen />
          {t("viewer.reveal")}
        </Button>
        <Button size="sm" variant="ghost" className="text-danger" onClick={() => void remove()} data-testid="viewer-delete">
          <Trash2 />
          {t("viewer.delete")}
        </Button>
        <span className="ml-auto text-xs text-fg-subtle">{t("viewer.shortcuts")}</span>
      </div>

      <div className="flex min-h-0 flex-1">
        <aside className="hidden w-32 shrink-0 overflow-y-auto border-r border-border p-2 md:block" aria-label={t("viewer.thumbnails")} data-testid="viewer-thumbs">
          {(pages ?? []).map((p) => (
            <button
              key={p.page_no}
              type="button"
              onClick={() => setPageNo(p.page_no)}
              className={cn("mb-2 flex w-full flex-col items-center gap-1 rounded-md border p-1 text-xs", p.page_no === pageNo ? "border-accent bg-accent-soft" : "border-transparent hover:bg-bg-sunken")}
              aria-current={p.page_no === pageNo ? "page" : undefined}
              data-testid={`viewer-thumb-${p.page_no}`}
            >
              {p.thumb_url ? <img src={p.thumb_url} alt="" className="w-full bg-white object-contain" loading="lazy" draggable={false} /> : <div className="h-20 w-full bg-bg-sunken" />}
              <span>{p.page_no}</span>
            </button>
          ))}
        </aside>

        <div ref={scrollRef} className="min-w-0 flex-1 overflow-auto bg-bg-sunken p-4" data-testid="viewer-canvas">
          {page?.image_url ? (
            <img
              key={`${page.page_no}-${page.rendered_at ?? ""}`}
              src={page.image_url}
              alt={t("viewer.page", { page: page.page_no, total })}
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
          <Tabs defaultValue="text" className="flex min-h-0 flex-1 flex-col">
            <TabsList className="m-2">
              {(["text", "fields", "tables", "qa", "history"] as const).map((tab) => (
                <TabsTrigger key={tab} value={tab} data-testid={`viewer-tab-${tab}`}>
                  {t(`viewer.tabs.${tab}`)}
                </TabsTrigger>
              ))}
            </TabsList>
            <TabsContent value="text" className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
              {textLoading ? (
                <Spinner />
              ) : text ? (
                <pre className="whitespace-pre-wrap break-words font-sans text-sm selectable" data-testid="viewer-text">
                  {text}
                </pre>
              ) : (
                <p className="text-sm text-fg-muted" data-testid="viewer-text-empty">
                  {t("viewer.text_empty")}
                </p>
              )}
            </TabsContent>
            <TabsContent value="fields" className="px-3 pb-3 text-sm text-fg-muted">
              {t("viewer.placeholder_fields")}
            </TabsContent>
            <TabsContent value="tables" className="px-3 pb-3 text-sm text-fg-muted">
              {t("viewer.placeholder_tables")}
            </TabsContent>
            <TabsContent value="qa" className="px-3 pb-3 text-sm text-fg-muted">
              {t("viewer.placeholder_qa")}
            </TabsContent>
            <TabsContent value="history" className="px-3 pb-3 text-sm text-fg-muted">
              {t("viewer.placeholder_history")}
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
            <dd>{doc.has_text_layer ? t("viewer.info.yes") : t("viewer.info.no")}</dd>
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
          </dl>
        </aside>
      </div>
    </div>
  );
}
