import { Check, ChevronRight, ExternalLink, Pencil, Pause, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Spinner } from "@/components/ui/spinner";
import { confidenceGlyph, confidenceTone, docTypeLabel, fieldTitle, formatConfidence, reasonVariant } from "@/features/pipeline/labels";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useDocTypes, usePipelineStatus, useReviewQueue } from "@/lib/queries";
import { queryClient } from "@/lib/query-client";
import { isComposing } from "@/lib/shortcuts";
import { REVIEW_REASONS, type ReviewAction, type ReviewItem, type ReviewReason } from "@/lib/types";
import { cn } from "@/lib/utils";
import { usePipelineStore } from "@/stores/pipeline";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";

/** Keys the queue owns; inputs, dialogs and controls keep their own keyboard behaviour. */
function isQueueTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return true;
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target.isContentEditable) return false;
  return !target.closest('[role="dialog"],[role="combobox"],[role="listbox"]');
}

/**
 * 검토 큐 (design §3.7.2): low-confidence / failed / missing fields across
 * documents, keyboard review (Enter approve · E edit · S hold · N next · R
 * reject, Enter ignored while composing Korean), page preview, bulk approval
 * per document, threshold slider, re-extraction with the fine-print profile.
 */
export function ReviewPage() {
  const { t, i18n } = useTranslation();
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
  const { data: items, isLoading } = useReviewQueue(query);
  const [index, setIndex] = useState(0);
  const [editOpen, setEditOpen] = useState(false);
  const [editValue, setEditValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [handled, setHandled] = useState(0);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);

  const list: ReviewItem[] = items ?? [];
  const current = list[Math.min(index, Math.max(0, list.length - 1))];
  useEffect(() => {
    if (index >= list.length) setIndex(Math.max(0, list.length - 1));
  }, [list.length, index]);
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${index}"]`)?.scrollIntoView({ block: "nearest" });
  }, [index]);

  const apply = useCallback(
    async (action: ReviewAction, item: ReviewItem | undefined = current) => {
      if (!item || busy) return;
      setBusy(true);
      try {
        const out = await api.pipelineReviewApply([item.id], action);
        const key = action.kind === "approve" ? "approved" : action.kind === "correct" ? "corrected" : action.kind === "hold" ? "held" : "rejected";
        toast(t(`review.${key}`), "ok");
        setHandled((n) => n + 1);
        if (action.kind === "hold") setIndex((i) => Math.min(i + 1, Math.max(0, list.length - 1)));
        // The item leaves the queue (unless held); the same index now shows the next one.
        await queryClient.invalidateQueries({ queryKey: ["pipeline", "review"] });
        void out;
      } catch (e) {
        toast(describeError(e, t), "danger");
      } finally {
        setBusy(false);
      }
    },
    [busy, current, list.length, t, toast],
  );

  const next = () => setIndex((i) => (list.length === 0 ? 0 : (i + 1) % list.length));
  const openEdit = () => {
    if (!current) return;
    setEditValue(current.value ?? "");
    setEditOpen(true);
  };
  const approveDocument = async () => {
    if (!current) return;
    try {
      const n = await api.pipelineReviewApproveDocument(current.document_id, null);
      toast(t("review.doc_approved", { count: n }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const reextract = async (profile: "fine_print" | null, precise: boolean) => {
    if (!current) return;
    try {
      await api.pipelineProcess({ document_ids: [current.document_id], level: "extract", force: true, profile, precise });
      toast(t("pipeline.process.started", { count: 1 }), "info");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
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
      next();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setIndex((i) => Math.max(0, i - 1));
    } else if (e.key === "r" || e.key === "R") {
      e.preventDefault();
      void apply({ kind: "reject" });
    }
  };

  const threshold = settings?.processing.review_threshold ?? 0.8;

  return (
    <div ref={rootRef} tabIndex={-1} onKeyDown={onKeyDown} className="flex h-full min-h-0 flex-col outline-none" data-testid="page-review">
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
                {docTypeLabel(d.slug, docTypes, t, i18n.language)}
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
        {filter.document && (
          <Button size="sm" variant="ghost" onClick={() => setFilter({ reviewDocument: null })} data-testid="review-clear-document">
            <X />
            {t("review.filters.document")}
          </Button>
        )}
        <div className="flex items-center gap-2 text-xs" title={t("settings.processing.review_threshold_hint")}>
          <span>{t("review.threshold", { value: threshold.toFixed(2) })}</span>
          <Slider min={0.5} max={1} step={0.05} value={[threshold]} onValueCommit={([v]) => void updateSettings((s) => void (s.processing.review_threshold = v ?? 0.8))} className="w-32" aria-label={t("settings.processing.review_threshold")} data-testid="review-threshold" />
        </div>
      </header>
      <div className="flex items-center gap-3 border-b border-border px-4 py-1.5 text-xs text-fg-muted" data-testid="review-stats">
        <span data-testid="review-count">{t("review.count", { count: list.length })}</span>
        <span>{t("review.status_line", { done: handled, total: handled + list.length })}</span>
        <span className="ml-auto flex items-center gap-1">
          <Kbd chord="Enter" /> {t("review.approve")} · <Kbd chord="E" /> {t("review.edit")} · <Kbd chord="S" /> {t("review.hold")} · <Kbd chord="N" /> {t("review.next")} · <Kbd chord="R" /> {t("review.reject")}
        </span>
      </div>
      <div className="flex min-h-0 flex-1">
        <ul ref={listRef} className="w-80 shrink-0 overflow-y-auto border-r border-border" aria-label={t("review.list")} data-testid="review-list">
          {isLoading && !items && (
            <li className="p-3">
              <Spinner />
            </li>
          )}
          {list.length === 0 && !isLoading && (
            <li className="p-3 text-sm text-fg-muted" data-testid="review-empty">
              {t("review.empty")}
            </li>
          )}
          {list.map((item, i) => (
            <li key={item.id} data-index={i}>
              <button
                type="button"
                onClick={() => setIndex(i)}
                className={cn("flex w-full flex-col gap-0.5 border-b border-border px-3 py-2 text-left text-sm hover:bg-bg-sunken", i === index && "bg-accent-soft")}
                aria-current={i === index ? "true" : undefined}
                data-testid={`review-item-${item.id}`}
                data-field-path={item.path}
              >
                <span className="flex items-center gap-1">
                  <span className="truncate font-medium">{fieldTitle(item, t, i18n.language)}</span>
                  <Badge variant={reasonVariant(item.review_reason)} className="ml-auto text-[10px]">
                    {item.review_reason ? t(`pipeline.reason.${item.review_reason}`) : ""}
                  </Badge>
                </span>
                <span className="truncate text-xs text-fg-muted">
                  {item.document_name} · {docTypeLabel(item.doc_type, docTypes, t, i18n.language)}
                </span>
                <span className={cn("text-xs", `text-${confidenceTone(item.confidence)}`)}>
                  {confidenceGlyph(item.confidence)} {formatConfidence(item.confidence)} · {item.value ?? t("viewer.fields.empty_value")}
                </span>
              </button>
            </li>
          ))}
        </ul>
        <section className="flex min-w-0 flex-1 flex-col gap-3 overflow-y-auto p-4" aria-label={t("review.current")}>
          {current ? (
            <>
              <div className="flex flex-wrap items-center gap-2">
                <h2 className="text-base font-semibold" data-testid="review-current-title">
                  {fieldTitle(current, t, i18n.language)}
                </h2>
                <Badge variant={reasonVariant(current.review_reason)}>{current.review_reason ? t(`pipeline.reason.${current.review_reason}`) : ""}</Badge>
                <Badge variant={confidenceTone(current.confidence)} data-testid="review-current-confidence">
                  {confidenceGlyph(current.confidence)} {formatConfidence(current.confidence)}
                </Badge>
                {current.status === "held" && <Badge variant="warn">{t("pipeline.field_status.held")}</Badge>}
                <span className="ml-auto flex items-center gap-1">
                  <Button size="sm" variant="ghost" onClick={() => navigate(`/library/${current.document_id}?page=${current.page ?? 1}&q=${encodeURIComponent(current.evidence ?? current.value ?? "")}`)} data-testid="review-open-doc">
                    <ExternalLink />
                    {t("review.open_doc")}
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => void reextract("fine_print", false)}>
                    {t("review.reextract")}
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => void reextract(null, true)}>
                    {t("review.reextract_precise")}
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => void approveDocument()} data-testid="review-approve-doc">
                    {t("review.approve_doc")}
                  </Button>
                </span>
              </div>
              <div className="grid gap-3 md:grid-cols-[1fr_320px]">
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
                        {current.validation.map((v, k) => (
                          <span key={k} className={v.level === "fail" ? "text-danger" : v.level === "warn" ? "text-warn" : ""}>
                            {v.message}
                          </span>
                        ))}
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
                <Button variant="ghost" onClick={next} data-testid="review-next">
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
      <Dialog open={editOpen} onOpenChange={setEditOpen}>
        <DialogContent className="max-w-md" closeLabel={t("common.close")} data-testid="review-edit-dialog">
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
                setEditOpen(false);
                void apply({ kind: "correct", value: editValue });
              }
            }}
            aria-label={t("review.value")}
            data-testid="review-edit-input"
          />
          <DialogFooter>
            <Button variant="secondary" onClick={() => setEditOpen(false)}>
              {t("common.cancel")}
            </Button>
            <Button
              onClick={() => {
                setEditOpen(false);
                void apply({ kind: "correct", value: editValue });
              }}
              disabled={!editValue.trim()}
              data-testid="review-edit-save"
            >
              {t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      {status && status.review_fields > 0 && list.length === 0 && filter.docType === "all" && filter.reason === "all" && !filter.document && <span className="sr-only">{t("home.review_fields", { count: status.review_fields })}</span>}
    </div>
  );
}
