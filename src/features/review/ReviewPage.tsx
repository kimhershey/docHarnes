import { defaultRangeExtractor, useVirtualizer, type Range } from "@tanstack/react-virtual";
import type { TFunction } from "i18next";
import { Check, ChevronRight, ExternalLink, Pencil, Pause, X } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { remPx, useRootFontSize } from "@/app/hooks/useRootFontSize";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Spinner } from "@/components/ui/spinner";
import { CONFIDENCE_TEXT_CLASS, DEFAULT_REVIEW_THRESHOLD, confidenceGlyph, confidenceTone, docTypeLabel, fieldTitle, formatConfidence, reasonVariant, validationRuleLabel } from "@/features/pipeline/labels";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useDocTypes, usePipelineStatus, useReviewQueue } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import { isComposing } from "@/lib/shortcuts";
import { REVIEW_REASONS, type DocTypeInfo, type ReviewAction, type ReviewItem, type ReviewReason } from "@/lib/types";
import { cn } from "@/lib/utils";
import { usePipelineStore } from "@/stores/pipeline";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";

import { reextractProcessRequest, type ReextractRequest } from "./reextract";
import { ReextractDialog } from "./ReextractDialog";

/** Estimated row height (three text lines + padding); mounted rows are measured. */
/** Queue row estimate in rem (three text lines + padding) until the rendered rows are measured; rem so it follows the UI 배율. */
const ROW_EST_REM = 4.5;
/** Prefix of every review-queue query (all filters). */
const REVIEW_QUEUE_KEY = ["pipeline", "review"] as const;

/**
 * Keys the queue owns: the page root and the options of its own listbox. Inputs, buttons, links,
 * sliders and foreign widgets (selects, dialogs) keep their own keyboard behaviour, so Enter on a
 * focused 거부 / 다음 / 문서 열기 button activates that button instead of approving.
 */
function isQueueTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return true;
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement || target.isContentEditable) return false;
  if (target.closest('button,a,[role="slider"],[role="switch"],[role="tab"],[role="menuitem"]')) return false;
  if (target.closest("[data-review-list]")) return true;
  return !target.closest('[role="dialog"],[role="combobox"],[role="listbox"]');
}

function shortId(id: string): string {
  return id.length > 8 ? `${id.slice(0, 8)}…` : id;
}

interface ReviewRowProps {
  item: ReviewItem;
  index: number;
  selected: boolean;
  /** Offset inside the virtualized list. */
  start: number;
  threshold: number;
  docTypes: DocTypeInfo[] | undefined;
  lang: string;
  t: TFunction;
  onSelect: (index: number) => void;
  measure: (el: HTMLLIElement | null) => void;
}

/** One queue row; module-level + memo so moving the selection re-renders only the two rows that changed. */
const ReviewRow = memo(function ReviewRow({ item, index, selected, start, threshold, docTypes, lang, t, onSelect, measure }: ReviewRowProps) {
  const tone = confidenceTone(item.confidence, threshold);
  return (
    <li
      ref={measure}
      role="option"
      aria-selected={selected}
      tabIndex={selected ? 0 : -1}
      onClick={() => onSelect(index)}
      className={cn(
        "absolute left-0 top-0 flex w-full cursor-pointer flex-col gap-0.5 border-b border-border px-3 py-2 text-left text-sm outline-none hover:bg-bg-sunken focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent",
        selected && "bg-accent-soft",
      )}
      style={{ transform: `translateY(${start}px)` }}
      data-index={index}
      data-testid={`review-item-${item.id}`}
      data-field-path={item.path}
    >
      <span className="flex items-center gap-1">
        <span className="truncate font-medium">{fieldTitle(item, t, lang)}</span>
        <Badge variant={reasonVariant(item.review_reason)} className="ml-auto text-[0.625rem]">
          {item.review_reason ? t(`pipeline.reason.${item.review_reason}`) : ""}
        </Badge>
      </span>
      <span className="truncate text-xs text-fg-muted">
        {item.document_name} · {docTypeLabel(item.doc_type, docTypes, t, lang)}
      </span>
      <span className={cn("truncate text-xs", CONFIDENCE_TEXT_CLASS[tone])}>
        {confidenceGlyph(item.confidence, threshold)} {formatConfidence(item.confidence)} · {item.value ?? t("viewer.fields.empty_value")}
        {item.status === "held" && ` · ${t("pipeline.field_status.held")}`}
      </span>
    </li>
  );
});

/**
 * 검토 큐 (design §3.7.2): low-confidence / failed / missing fields across
 * documents, keyboard review (Enter approve · E edit · S hold · N next · R
 * reject, Enter ignored while composing Korean), page preview, bulk approval
 * per document, threshold slider, re-extraction with the fine-print profile.
 * The list is a virtualized roving-tabindex listbox: the shortcuts act when
 * the page root or one of its options has focus.
 */
export function ReviewPage() {
  const { t, i18n } = useTranslation();
  const lang = i18n.language;
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const settings = useSettingsStore((s) => s.settings);
  const updateSettings = useSettingsStore((s) => s.update);
  const reviewDocType = usePipelineStore((s) => s.reviewDocType);
  const reviewReason = usePipelineStore((s) => s.reviewReason);
  const reviewDocument = usePipelineStore((s) => s.reviewDocument);
  const filter = useMemo(() => ({ docType: reviewDocType, reason: reviewReason, document: reviewDocument }), [reviewDocType, reviewReason, reviewDocument]);
  const setFilter = usePipelineStore((s) => s.setReviewFilter);
  const { data: docTypes } = useDocTypes();
  const { data: status } = usePipelineStatus();
  const query = useMemo(() => ({ doc_type: filter.docType === "all" ? null : filter.docType, reason: filter.reason === "all" ? null : filter.reason, document_id: filter.document }), [filter]);
  const queueKey = useMemo(() => queryKeys.reviewQueue(query), [query]);
  const { data: items, isLoading } = useReviewQueue(query);
  const [index, setIndex] = useState(0);
  const [editOpen, setEditOpen] = useState(false);
  const [editValue, setEditValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [handled, setHandled] = useState(0);
  const [reextractAsk, setReextractAsk] = useState<ReextractRequest | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);
  /** Roving focus: set when a key moved the selection (or the focused row left the queue) while an option had focus. */
  const focusOptionRef = useRef(false);
  const hadCurrentRef = useRef(false);

  const list: ReviewItem[] = items ?? [];
  const safeIndex = Math.min(index, Math.max(0, list.length - 1));
  const current = list[safeIndex];
  const nextIndex = list.length === 0 ? 0 : (safeIndex + 1) % list.length;
  const threshold = settings?.processing.review_threshold ?? DEFAULT_REVIEW_THRESHOLD;
  /** Colour / glyph cut-off: the threshold the backend flagged with; the slider value until the status refetches. */
  const toneThreshold = status?.review_threshold ?? threshold;
  const currentTitle = current ? fieldTitle(current, t, lang) : "";
  const rootPx = useRootFontSize();

  const virtualizer = useVirtualizer<HTMLDivElement, HTMLLIElement>({
    count: list.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => remPx(ROW_EST_REM, rootPx),
    overscan: 16,
    // Always render the current option, even when scrolled out of view, so roving focus never sits on an unmounted row.
    rangeExtractor: (range: Range) => {
      const r = defaultRangeExtractor(range);
      return list.length === 0 || r.includes(safeIndex) ? r : [...r, safeIndex].sort((a, b) => a - b);
    },
  });

  useEffect(() => {
    if (index >= list.length) setIndex(Math.max(0, list.length - 1));
  }, [list.length, index]);
  useEffect(() => {
    if (list.length > 0) virtualizer.scrollToIndex(safeIndex, { align: "auto" });
  }, [safeIndex, list.length, virtualizer]);
  // Shortcuts work on arrival: take focus from the nav link we came by, never from a control inside the page.
  useEffect(() => {
    rootRef.current?.focus({ preventScroll: true });
  }, []);
  useEffect(() => {
    const had = hadCurrentRef.current;
    hadCurrentRef.current = current !== undefined;
    if (!current || had || editOpen) return;
    const root = rootRef.current;
    const active = document.activeElement;
    if (root && (!active || active === document.body || !root.contains(active))) root.focus({ preventScroll: true });
  }, [current, editOpen]);
  // Roving tabindex: focus follows the current option once it is rendered (or the root when the queue ran empty).
  useEffect(() => {
    if (!focusOptionRef.current) return;
    const el = listRef.current?.querySelector<HTMLElement>(`[data-index="${safeIndex}"]`);
    if (el) el.focus({ preventScroll: true });
    else if (list.length === 0) rootRef.current?.focus({ preventScroll: true });
    else return; // not rendered yet; the virtualizer's scroll triggers another render
    focusOptionRef.current = false;
  });

  const apply = useCallback(
    async (action: ReviewAction, item: ReviewItem | undefined = current): Promise<boolean> => {
      if (!item || busy) return false;
      setBusy(true);
      const focusInList = listRef.current?.contains(document.activeElement) ?? false;
      try {
        await api.pipelineReviewApply([item.id], action);
        const key = action.kind === "approve" ? "approved" : action.kind === "correct" ? "corrected" : action.kind === "hold" ? "held" : "rejected";
        toast(t(`review.${key}`), "ok");
        if (action.kind === "hold") {
          // Held fields stay in the queue (marked held); step past it.
          queryClient.setQueryData<ReviewItem[]>(queueKey, (old) => old?.map((x) => (x.id === item.id ? { ...x, status: "held" } : x)));
          setIndex((i) => Math.min(i + 1, Math.max(0, list.length - 1)));
        } else {
          // The field leaves the queue; the same index now shows the next one.
          setHandled((n) => n + 1);
          queryClient.setQueryData<ReviewItem[]>(queueKey, (old) => old?.filter((x) => x.id !== item.id));
        }
        if (focusInList) focusOptionRef.current = true;
        void queryClient.invalidateQueries({ queryKey: REVIEW_QUEUE_KEY });
        return true;
      } catch (e) {
        toast(describeError(e, t), "danger");
        void queryClient.invalidateQueries({ queryKey: REVIEW_QUEUE_KEY });
        return false;
      } finally {
        setBusy(false);
      }
    },
    [busy, current, list.length, queueKey, t, toast],
  );

  const selectRow = useCallback((i: number) => setIndex(i), []);
  /** Keyboard navigation: the selection moves and, when an option had focus, focus follows it. */
  const moveTo = (i: number) => {
    focusOptionRef.current = listRef.current?.contains(document.activeElement) ?? false;
    setIndex(i);
  };
  const openEdit = () => {
    if (!current) return;
    setEditValue(current.value ?? "");
    setEditOpen(true);
  };
  const saveEdit = async () => {
    const value = editValue.trim();
    if (!value || busy) return;
    if (await apply({ kind: "correct", value })) setEditOpen(false);
  };
  const approveDocument = async () => {
    if (!current || busy) return;
    const docId = current.document_id;
    setBusy(true);
    try {
      const n = await api.pipelineReviewApproveDocument(docId, null);
      toast(t("review.doc_approved", { count: n }), "ok");
      setHandled((h) => h + list.filter((x) => x.document_id === docId).length);
      queryClient.setQueryData<ReviewItem[]>(queueKey, (old) => old?.filter((x) => x.document_id !== docId));
      void queryClient.invalidateQueries({ queryKey: REVIEW_QUEUE_KEY });
    } catch (e) {
      toast(describeError(e, t), "danger");
      void queryClient.invalidateQueries({ queryKey: REVIEW_QUEUE_KEY });
    } finally {
      setBusy(false);
    }
  };
  // Re-extraction replaces the document's fields and with them the approvals / corrections of this queue, so it
  // asks first like the library's 처리 menu, and it is not a forced run unless the reviewer says so (PIP-2).
  const askReextract = (mode: ReextractRequest["mode"]) => {
    if (current) setReextractAsk({ documentId: current.document_id, documentName: current.document_name, mode });
  };
  const reextract = async (request: ReextractRequest, startOver: boolean) => {
    try {
      await api.pipelineProcess(reextractProcessRequest(request, startOver));
      toast(t("pipeline.process.started", { count: 1 }), "info");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const commitThreshold = async (v: number) => {
    await updateSettings((s) => void (s.processing.review_threshold = v));
    // Existing flags are recomputed only by future extractions / "re-run validation"; refresh what the backend reports now.
    void queryClient.invalidateQueries({ queryKey: queryKeys.pipeline });
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey || editOpen || !isQueueTarget(e.target)) return;
    if (e.key === "Enter") {
      if (isComposing(e)) return; // Korean IME: Enter commits the composition, never approves
      e.preventDefault();
      void apply({ kind: "approve" });
    } else if (e.key === "e" || e.key === "E") {
      e.preventDefault();
      openEdit();
    } else if (e.key === "s" || e.key === "S") {
      e.preventDefault();
      void apply({ kind: "hold" });
    } else if (e.key === "n" || e.key === "N" || e.key === "ArrowDown") {
      e.preventDefault();
      moveTo(nextIndex);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      moveTo(Math.max(0, safeIndex - 1));
    } else if (e.key === "Home") {
      e.preventDefault();
      moveTo(0);
    } else if (e.key === "End") {
      e.preventDefault();
      moveTo(Math.max(0, list.length - 1));
    } else if (e.key === "r" || e.key === "R") {
      e.preventDefault();
      void apply({ kind: "reject" });
    }
  };

  const docChipName = filter.document ? (list.find((x) => x.document_id === filter.document)?.document_name ?? shortId(filter.document)) : null;

  return (
    <div ref={rootRef} tabIndex={-1} onKeyDown={onKeyDown} className="relative flex h-full min-h-0 flex-col outline-none" data-testid="page-review">
      <header className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-3">
        <div className="mr-auto">
          <h1 className="text-lg font-semibold">{t("review.title")}</h1>
          <p className="text-xs text-fg-muted">{t("review.subtitle")}</p>
        </div>
        <Select value={filter.docType} onValueChange={(v) => setFilter({ reviewDocType: v })}>
          <SelectTrigger className="h-8 w-40" aria-label={t("review.filters.doc_type")} data-testid="review-filter-type">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t("review.filters.all")}</SelectItem>
            {(docTypes ?? []).map((d) => (
              <SelectItem key={d.slug} value={d.slug}>
                {docTypeLabel(d.slug, docTypes, t, lang)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={filter.reason} onValueChange={(v) => setFilter({ reviewReason: v as ReviewReason | "all" })}>
          <SelectTrigger className="h-8 w-36" aria-label={t("review.filters.reason")} data-testid="review-filter-reason">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t("review.filters.all")}</SelectItem>
            {REVIEW_REASONS.map((r) => (
              <SelectItem key={r} value={r}>
                {t(`pipeline.reason.${r}`)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {docChipName && (
          <Button size="sm" variant="ghost" onClick={() => setFilter({ reviewDocument: null })} aria-label={t("review.filters.clear_document", { name: docChipName })} title={t("review.filters.clear_document", { name: docChipName })} data-testid="review-clear-document">
            <X />
            <span className="max-w-48 truncate">{t("review.filters.document_chip", { name: docChipName })}</span>
          </Button>
        )}
        <div className="flex flex-col gap-0.5 text-xs" title={t("settings.processing.review_threshold_hint")}>
          <div className="flex items-center gap-2">
            <span>{t("review.threshold", { value: threshold.toFixed(2) })}</span>
            <Slider min={0.5} max={1} step={0.05} value={[threshold]} onValueCommit={([v]) => void commitThreshold(v ?? DEFAULT_REVIEW_THRESHOLD)} className="w-32" aria-label={t("settings.processing.review_threshold")} data-testid="review-threshold" />
          </div>
          <p className="max-w-64 text-[0.6875rem] leading-tight text-fg-muted" data-testid="review-threshold-hint">
            {t("review.threshold_hint")}
          </p>
        </div>
      </header>
      <div className="flex items-center gap-3 border-b border-border px-4 py-1.5 text-xs text-fg-muted" data-testid="review-stats">
        <span data-testid="review-count">{t("review.count", { count: list.length })}</span>
        <span>{t("review.status_line", { done: handled, total: handled + list.length })}</span>
        <span className="ml-auto flex items-center gap-1">
          <Kbd chord="Enter" /> {t("review.approve")} · <Kbd chord="E" /> {t("review.edit")} · <Kbd chord="S" /> {t("review.hold")} · <Kbd chord="N" /> {t("review.next")} · <Kbd chord="R" /> {t("review.reject")}
        </span>
      </div>
      {/* Screen readers hear which field is current as the selection moves or the queue advances. */}
      <div aria-live="polite" aria-atomic="true" className="sr-only" data-testid="review-live">
        {current ? t("review.announce", { title: currentTitle, index: safeIndex + 1, total: list.length }) : !isLoading && list.length === 0 ? t("review.empty") : ""}
      </div>
      <div className="flex min-h-0 flex-1">
        <div ref={scrollRef} className="w-80 shrink-0 overflow-y-auto border-r border-border">
          {isLoading && !items && (
            <div className="p-3">
              <Spinner />
            </div>
          )}
          {list.length === 0 && !isLoading && (
            <p className="p-3 text-sm text-fg-muted" data-testid="review-empty">
              {t("review.empty")}
            </p>
          )}
          <ul ref={listRef} role="listbox" aria-label={t("review.list")} className="relative w-full" style={{ height: virtualizer.getTotalSize() }} data-review-list="" data-testid="review-list">
            {virtualizer.getVirtualItems().map((row) => {
              const item = list[row.index];
              return item ? <ReviewRow key={item.id} item={item} index={row.index} selected={row.index === safeIndex} start={row.start} threshold={toneThreshold} docTypes={docTypes} lang={lang} t={t} onSelect={selectRow} measure={virtualizer.measureElement} /> : null;
            })}
          </ul>
        </div>
        <section className="flex min-w-0 flex-1 flex-col gap-3 overflow-y-auto p-4" aria-label={t("review.current")}>
          {current ? (
            <>
              <div className="flex flex-wrap items-center gap-2">
                <h2 className="text-base font-semibold" data-testid="review-current-title">
                  {currentTitle}
                </h2>
                <Badge variant={reasonVariant(current.review_reason)}>{current.review_reason ? t(`pipeline.reason.${current.review_reason}`) : ""}</Badge>
                <Badge variant={confidenceTone(current.confidence, toneThreshold)} data-testid="review-current-confidence">
                  {confidenceGlyph(current.confidence, toneThreshold)} {formatConfidence(current.confidence)}
                </Badge>
                {current.status === "held" && <Badge variant="warn">{t("pipeline.field_status.held")}</Badge>}
                <span className="ml-auto flex items-center gap-1">
                  <Button size="sm" variant="ghost" onClick={() => navigate(`/library/${current.document_id}?page=${current.page ?? 1}&q=${encodeURIComponent(current.evidence ?? current.value ?? "")}`)} data-testid="review-open-doc">
                    <ExternalLink />
                    {t("review.open_doc")}
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => askReextract("fine_print")} data-testid="review-reextract-fine-print">
                    {t("review.reextract")}
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => askReextract("precise")} data-testid="review-reextract-precise">
                    {t("review.reextract_precise")}
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => void approveDocument()} disabled={busy} data-testid="review-approve-doc">
                    {t("review.approve_doc")}
                  </Button>
                </span>
              </div>
              <div className="grid gap-3 md:grid-cols-[1fr_20rem]">
                <div className="min-h-64 overflow-auto rounded-md border border-border bg-bg-sunken p-2" data-testid="review-preview">
                  {current.page_image_url ? <img src={current.page_image_url} alt={t("review.page", { page: current.page ?? 1 })} className="mx-auto max-h-[60vh] bg-white shadow" draggable={false} /> : <p className="text-sm text-fg-muted">{t("review.no_image")}</p>}
                </div>
                <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-2 text-sm">
                  <dt className="text-fg-subtle">{t("review.value")}</dt>
                  <dd className="font-medium selectable" data-testid="review-current-value">
                    {current.value ?? t("viewer.fields.empty_value")}
                  </dd>
                  <dt className="text-fg-subtle">{t("review.evidence")}</dt>
                  <dd className="selectable text-fg-muted">{current.evidence ?? "–"}</dd>
                  <dt className="text-fg-subtle">{t("review.filters.document")}</dt>
                  <dd>
                    {current.document_name} · {t("review.page", { page: current.page ?? 1 })}
                  </dd>
                  {current.validation.length > 0 && (
                    <>
                      <dt className="text-fg-subtle">{t("review.validation")}</dt>
                      <dd className="flex flex-col gap-1">
                        {current.validation.map((v, k) => {
                          // The rule's name first where it has one (the core's message is the detail: which page, why).
                          const rule = validationRuleLabel(v.rule, t);
                          return (
                            <span key={k} className={v.level === "fail" ? "text-danger" : v.level === "warn" ? "text-warn" : ""} data-testid="review-validation" data-rule={v.rule}>
                              {rule && <span className="font-medium">{rule}</span>}
                              {rule && v.message ? " — " : ""}
                              {v.message}
                            </span>
                          );
                        })}
                      </dd>
                    </>
                  )}
                </dl>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <Button onClick={() => void apply({ kind: "approve" })} disabled={busy} data-testid="review-approve">
                  <Check />
                  {t("review.approve")} <Kbd chord="Enter" />
                </Button>
                <Button variant="secondary" onClick={openEdit} disabled={busy} data-testid="review-edit">
                  <Pencil />
                  {t("review.edit")} <Kbd chord="E" />
                </Button>
                <Button variant="secondary" onClick={() => void apply({ kind: "hold" })} disabled={busy} data-testid="review-hold">
                  <Pause />
                  {t("review.hold")} <Kbd chord="S" />
                </Button>
                <Button variant="secondary" onClick={() => void apply({ kind: "reject" })} disabled={busy} data-testid="review-reject">
                  <X />
                  {t("review.reject")} <Kbd chord="R" />
                </Button>
                <Button variant="ghost" onClick={() => setIndex(nextIndex)} data-testid="review-next">
                  <ChevronRight />
                  {t("review.next")} <Kbd chord="N" />
                </Button>
              </div>
            </>
          ) : (
            !isLoading && <p className="text-sm text-fg-muted">{t("review.empty")}</p>
          )}
        </section>
      </div>
      <Dialog
        open={editOpen}
        onOpenChange={(open) => {
          if (!open && busy) return; // the correction is in flight: keep the dialog until it settles
          setEditOpen(open);
        }}
      >
        <DialogContent
          className="max-w-md"
          closeLabel={t("common.close")}
          onCloseAutoFocus={(e) => {
            e.preventDefault();
            rootRef.current?.focus({ preventScroll: true });
          }}
          data-testid="review-edit-dialog"
        >
          <DialogHeader>
            <DialogTitle>{t("review.edit_title")}</DialogTitle>
            <DialogDescription>{t("review.edit_hint")}</DialogDescription>
          </DialogHeader>
          <Input
            value={editValue}
            onChange={(e) => setEditValue(e.target.value)}
            autoFocus
            onKeyDown={(e) => {
              if (e.key === "Enter" && !isComposing(e)) {
                e.preventDefault();
                void saveEdit();
              }
            }}
            aria-label={t("review.value")}
            data-testid="review-edit-input"
          />
          <DialogFooter>
            <Button variant="secondary" onClick={() => setEditOpen(false)} disabled={busy}>
              {t("common.cancel")}
            </Button>
            <Button onClick={() => void saveEdit()} disabled={busy || !editValue.trim()} aria-busy={busy || undefined} data-testid="review-edit-save">
              {t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <ReextractDialog request={reextractAsk} onClose={() => setReextractAsk(null)} onConfirm={(request, startOver) => void reextract(request, startOver)} />
    </div>
  );
}
