import type { TFunction } from "i18next";
import { Check, ChevronRight, Ellipsis, ListChecks, Pause, Pencil, RefreshCw, RotateCcw, X } from "lucide-react";
import { memo, useCallback, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { CONFIDENCE_TEXT_CLASS, confidenceGlyph, confidenceTone, DEFAULT_REVIEW_THRESHOLD, fieldLabel, fieldStatusVariant, formatConfidence, reasonVariant } from "@/features/pipeline/labels";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useFields, usePipelineStatus, useRuns, useSchemas } from "@/lib/queries";
import { isComposing } from "@/lib/shortcuts";
import { fieldNeedsReview, type DocumentView, type FieldRecord, type ReviewAction } from "@/lib/types";
import { cn } from "@/lib/utils";
import { usePipelineStore } from "@/stores/pipeline";
import { useUiStore } from "@/stores/ui";

const AUTO = "__auto__";

/** Core validation messages are bilingual ("한국어 / English"); keep the half for the UI language. */
function localizeValidation(message: string, lang: string): string {
  const i = message.indexOf(" / ");
  if (i < 0) return message;
  return lang.startsWith("ko") ? message.slice(0, i) : message.slice(i + 3);
}

/** Groups the flat field rows: scalars (incl. group members) then tables by key. */
function groupFields(fields: FieldRecord[]): { scalars: FieldRecord[]; tables: { key: string; rows: FieldRecord[][] }[] } {
  const scalars = fields.filter((f) => f.row === null);
  const tables: { key: string; rows: FieldRecord[][] }[] = [];
  for (const f of fields.filter((f) => f.row !== null)) {
    const key = f.path.split("[")[0] ?? f.key;
    let t = tables.find((x) => x.key === key);
    if (!t) {
      t = { key, rows: [] };
      tables.push(t);
    }
    (t.rows[f.row!] ??= []).push(f);
  }
  return { scalars, tables };
}

interface FieldRowProps {
  f: FieldRecord;
  /** Table cell: no evidence line, actions behind a "⋯" menu. */
  compact?: boolean;
  /** The current item of the pending cursor. */
  focused: boolean;
  /** Draft value while this row's inline editor is open; null otherwise. */
  editing: string | null;
  busy: boolean;
  threshold: number;
  hasNext: boolean;
  lang: string;
  t: TFunction;
  onApply: (id: number, action: ReviewAction) => void;
  onEdit: (id: number, value: string | null) => void;
  onNext: () => void;
  onGoToPage: (page: number, needle?: string) => void;
}

/**
 * One schema field. Module-level and memoised so a parent state change never
 * remounts the row: the inline correction `<Input>` keeps its DOM node across
 * keystrokes, which is what keeps a Korean IME composition alive.
 *
 * Actions (design §3.7.2 / §3.7.6): pending rows get 승인·수정·보류·거부·다음;
 * every other non-rejected row can still be corrected or rejected (confident
 * mistakes are the dangerous ones), and resolved rows can be reset.
 */
const FieldRow = memo(function FieldRow({ f, compact, focused, editing, busy, threshold, hasNext, lang, t, onApply, onEdit, onNext, onGoToPage }: FieldRowProps) {
  const needs = fieldNeedsReview(f);
  const isEditing = editing !== null;
  const resolved = f.status !== "auto";
  const canCorrect = f.status !== "rejected";
  const confidence = f.value === null ? null : f.confidence;
  const tone = confidenceTone(confidence, threshold);
  const save = () => {
    if (editing !== null && editing.trim()) onApply(f.id, { kind: "correct", value: editing });
  };
  const correct = () => onEdit(f.id, f.value ?? "");
  const reject = () => onApply(f.id, { kind: "reject" });
  const reset = () => onApply(f.id, { kind: "reset" });
  return (
    <div className={cn("rounded-md border px-2 py-1.5 text-xs", focused ? "border-accent bg-accent-soft" : "border-border", compact && "py-1")} data-testid={`field-${f.path}`} data-field-status={f.status} data-field-review={needs ? "pending" : "none"}>
      <div className="flex items-center gap-1">
        <span className="truncate font-medium" title={f.path}>
          {fieldLabel(f, lang)}
          {f.required && <span className="text-danger">*</span>}
        </span>
        <span className={cn("ml-auto whitespace-nowrap", CONFIDENCE_TEXT_CLASS[tone])} title={t("viewer.fields.confidence")} data-testid="field-confidence">
          {confidenceGlyph(confidence, threshold)} {f.value === null ? "–" : formatConfidence(f.confidence)}
        </span>
        <Badge variant={fieldStatusVariant(f.status)} className="text-[10px]" data-testid="field-status">
          {t(`pipeline.field_status.${f.status}`)}
        </Badge>
        {f.review_reason && needs && (
          <Badge variant={reasonVariant(f.review_reason)} className="text-[10px]">
            {t(`pipeline.reason.${f.review_reason}`)}
          </Badge>
        )}
        {compact && !needs && !isEditing && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="icon-sm" variant="ghost" className="size-6" aria-label={t("viewer.fields.more")} data-testid="field-menu">
                <Ellipsis />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {canCorrect && (
                <DropdownMenuItem onSelect={correct} data-testid="field-menu-correct">
                  <Pencil />
                  {t("viewer.fields.correct")}
                </DropdownMenuItem>
              )}
              {canCorrect && (
                <DropdownMenuItem disabled={busy} onSelect={reject} data-testid="field-menu-reject">
                  <X />
                  {t("viewer.fields.reject")}
                </DropdownMenuItem>
              )}
              {resolved && (
                <DropdownMenuItem disabled={busy} onSelect={reset} data-testid="field-menu-reset">
                  <RotateCcw />
                  {t("review.reset")}
                </DropdownMenuItem>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </div>
      {isEditing ? (
        <div className="mt-1 flex items-center gap-1">
          <Input
            value={editing}
            onChange={(e) => onEdit(f.id, e.target.value)}
            autoFocus
            className="h-7 text-xs"
            placeholder={t("viewer.fields.edit_placeholder")}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !isComposing(e)) {
                e.preventDefault();
                save();
              } else if (e.key === "Escape") onEdit(f.id, null);
            }}
            data-testid="field-edit-input"
          />
          <Button size="sm" className="h-7" onClick={save} disabled={busy || !editing.trim()} data-testid="field-edit-save">
            {t("common.save")}
          </Button>
        </div>
      ) : (
        <div className={cn("mt-0.5 selectable", f.status === "rejected" && "line-through text-fg-subtle")} data-testid="field-value">
          {f.value ?? <span className="text-fg-subtle">{t("viewer.fields.empty_value")}</span>}
        </div>
      )}
      {!compact && f.evidence && (
        <button type="button" className="mt-0.5 block max-w-full truncate text-left text-fg-subtle hover:text-fg" title={f.evidence} onClick={() => onGoToPage(f.page ?? 1, f.evidence ?? undefined)} data-testid="field-evidence">
          {t("viewer.fields.evidence")}: {f.evidence} {f.page ? `· ${t("viewer.fields.page_link", { page: f.page })}` : ""}
        </button>
      )}
      {f.validation.length > 0 && (
        <ul className="mt-0.5">
          {f.validation.map((v, k) => (
            <li key={k} className={v.level === "fail" ? "text-danger" : v.level === "warn" ? "text-warn" : "text-ok"}>
              {localizeValidation(v.message, lang)}
            </li>
          ))}
        </ul>
      )}
      {needs && !isEditing && (
        <div className="mt-1 flex flex-wrap gap-1">
          <Button size="sm" variant="secondary" className="h-6 px-2" onClick={() => onApply(f.id, { kind: "approve" })} disabled={busy} data-testid="field-approve">
            <Check />
            {t("viewer.fields.approve")}
          </Button>
          <Button size="sm" variant="ghost" className="h-6 px-2" onClick={correct} data-testid="field-correct">
            <Pencil />
            {t("viewer.fields.correct")}
          </Button>
          <Button size="sm" variant="ghost" className="h-6 px-2" onClick={() => onApply(f.id, { kind: "hold" })} disabled={busy} data-testid="field-hold">
            <Pause />
            {t("viewer.fields.hold")}
          </Button>
          <Button size="sm" variant="ghost" className="h-6 px-2" onClick={reject} disabled={busy} data-testid="field-reject">
            <X />
            {t("viewer.fields.reject")}
          </Button>
          {resolved && (
            <Button size="sm" variant="ghost" className="h-6 px-2" onClick={reset} disabled={busy} data-testid="field-reset">
              <RotateCcw />
              {t("review.reset")}
            </Button>
          )}
          {hasNext && (
            <Button size="sm" variant="ghost" className="h-6 px-2" onClick={onNext} data-testid="field-next">
              <ChevronRight />
              {t("viewer.fields.next")}
            </Button>
          )}
        </div>
      )}
      {!compact && !needs && !isEditing && (
        <div className="mt-1 flex flex-wrap gap-1">
          {canCorrect && (
            <Button size="sm" variant="ghost" className="h-6 px-2" onClick={correct} data-testid="field-correct">
              <Pencil />
              {t("viewer.fields.correct")}
            </Button>
          )}
          {canCorrect && (
            <Button size="sm" variant="ghost" className="h-6 px-2" onClick={reject} disabled={busy} data-testid="field-reject">
              <X />
              {t("viewer.fields.reject")}
            </Button>
          )}
          {resolved && (
            <Button size="sm" variant="ghost" className="h-6 px-2" onClick={reset} disabled={busy} data-testid="field-reset">
              <RotateCcw />
              {t("review.reset")}
            </Button>
          )}
        </div>
      )}
    </div>
  );
});

/**
 * Viewer "필드" tab (design §3.7.6): schema grid with value · confidence
 * (colour + glyph) · evidence · approve / correct / hold / next, validation
 * results with "검증 다시 실행", schema override, the run selector and the
 * jump into the review queue for this document.
 */
export function FieldsPanel({ doc, onGoToPage, onStartExtract }: { doc: DocumentView; onGoToPage: (page: number, needle?: string) => void; onStartExtract: () => void }) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const { data: runs } = useRuns(doc.id);
  const [runId, setRunId] = useState<string | null>(null);
  const { data: fields, isLoading } = useFields(doc.id, runId);
  const { data: schemas } = useSchemas();
  const { data: pipeline } = usePipelineStatus();
  const threshold = pipeline?.review_threshold ?? DEFAULT_REVIEW_THRESHOLD;
  const [cursor, setCursor] = useState(0);
  const [editing, setEditing] = useState<{ id: number; value: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const grouped = useMemo(() => groupFields(fields ?? []), [fields]);
  const pending = useMemo(() => (fields ?? []).filter(fieldNeedsReview), [fields]);
  const pendingCount = pending.length;
  const focusId = pending[Math.min(cursor, Math.max(0, pendingCount - 1))]?.id;
  // Refs let the row callbacks stay referentially stable (memoised rows) while reading the latest values.
  const pendingRef = useRef(pending);
  pendingRef.current = pending;
  const goToPageRef = useRef(onGoToPage);
  goToPageRef.current = onGoToPage;

  const apply = useCallback(
    async (id: number, action: ReviewAction) => {
      setBusy(true);
      try {
        await api.pipelineReviewApply([id], action);
        setEditing(null);
        const n = pendingRef.current.length;
        const wasPending = pendingRef.current.some((p) => p.id === id);
        // A held (or reset) field stays in the queue: move on. Anything else leaves it, so the cursor
        // keeps pointing at the item that shifts into this slot.
        if (action.kind === "hold") setCursor((c) => Math.min(c + 1, Math.max(0, n - 1)));
        else if (wasPending && action.kind !== "reset") setCursor((c) => Math.min(c, Math.max(0, n - 2)));
      } catch (e) {
        toast(describeError(e, t), "danger");
      } finally {
        setBusy(false);
      }
    },
    [toast, t],
  );
  const onEdit = useCallback((id: number, value: string | null) => setEditing(value === null ? null : { id, value }), []);
  const onNext = useCallback(() => {
    setCursor((c) => {
      const n = pendingRef.current.length;
      return n === 0 ? 0 : (Math.min(c, n - 1) + 1) % n;
    });
  }, []);
  const goToPage = useCallback((page: number, needle?: string) => goToPageRef.current(page, needle), []);

  const revalidate = async () => {
    try {
      await api.pipelineRevalidate(doc.id);
      toast(t("viewer.fields.revalidated"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const approveAll = async () => {
    try {
      const n = await api.pipelineReviewApproveDocument(doc.id, null);
      toast(t("review.doc_approved", { count: n }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const changeSchema = async (v: string) => {
    try {
      await api.pipelineDocumentSchema(doc.id, v === AUTO ? null : v);
      toast(t("viewer.fields.schema_changed"), "info");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  /** Design §3.7.2 filter "문서": the queue scoped to this document. */
  const openQueue = () => {
    usePipelineStore.getState().openReviewForDocument(doc.id);
    navigate("/review");
  };

  if (isLoading && !fields) return <Spinner />;
  const run = runs?.find((r) => r.id === (runId ?? doc.last_run_id)) ?? runs?.[0];
  const lang = i18n.language;
  const rowProps = { busy, threshold, hasNext: pendingCount > 1, lang, t, onApply: apply, onEdit, onNext, onGoToPage: goToPage };
  return (
    <div className="flex flex-col gap-2" data-testid="fields-panel">
      <div className="flex flex-wrap items-center gap-1 text-xs">
        {runs && runs.length > 0 && (
          <Select value={runId ?? run?.id ?? ""} onValueChange={(v) => setRunId(v)}>
            <SelectTrigger className="h-7 w-36" aria-label={t("viewer.fields.run_select")} data-testid="fields-run">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {runs.map((r, i) => (
                <SelectItem key={r.id} value={r.id}>
                  {t("viewer.fields.run", { n: runs.length - i })} · {t(`viewer.history.status.${r.status}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
        <Select value={doc.schema_id ?? AUTO} onValueChange={(v) => void changeSchema(v)}>
          <SelectTrigger className="h-7 w-40" aria-label={t("viewer.fields.change_schema")} data-testid="fields-schema">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={AUTO}>{t("viewer.fields.schema_auto")}</SelectItem>
            {(schemas ?? []).map((s) => (
              <SelectItem key={s.id} value={s.id}>
                {lang.startsWith("ko") ? s.name : (s.name_en ?? s.name)} ({s.doc_type})
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {pendingCount > 0 && (
          <>
            <Badge variant="warn" data-testid="fields-pending">
              {t("viewer.fields.pending", { count: pendingCount })}
            </Badge>
            <Button size="sm" variant="ghost" className="h-7" onClick={openQueue} data-testid="fields-open-queue">
              <ListChecks />
              {t("viewer.fields.open_queue")}
            </Button>
            <Button size="sm" variant="ghost" className="h-7" onClick={() => void approveAll()} data-testid="fields-approve-all">
              {t("viewer.fields.approve_all")}
            </Button>
          </>
        )}
        {fields && fields.length > 0 && (
          <Button size="sm" variant="ghost" className="ml-auto h-7" onClick={() => void revalidate()} data-testid="fields-revalidate">
            <RefreshCw />
            {t("viewer.fields.revalidate")}
          </Button>
        )}
      </div>
      {!fields || fields.length === 0 ? (
        <div className="flex flex-col items-start gap-2 text-sm text-fg-muted" data-testid="fields-empty">
          <p>{t("viewer.fields.none")}</p>
          <Button size="sm" variant="secondary" onClick={onStartExtract} data-testid="fields-extract">
            {t("viewer.fields.extract")}
          </Button>
        </div>
      ) : (
        <>
          {run && (
            <p className="text-[11px] text-fg-subtle">
              {t("viewer.fields.schema")}: {run.doc_type ?? "–"} · {run.model ?? "–"} · {run.prompt_version ?? ""} · {run.profile ?? ""}
              {run.precise ? ` · ${t("viewer.fields.precise")}` : ""}
            </p>
          )}
          <div className="flex flex-col gap-1">
            {grouped.scalars.map((f) => (
              <FieldRow key={f.id} f={f} focused={f.id === focusId} editing={editing?.id === f.id ? editing.value : null} {...rowProps} />
            ))}
          </div>
          {grouped.tables.map((tbl) => (
            <div key={tbl.key} className="flex flex-col gap-1" data-testid={`fields-table-${tbl.key}`}>
              <span className="text-xs font-semibold text-fg-muted">{tbl.key}</span>
              {tbl.rows.map((row, ri) => (
                <div key={ri} className="rounded-md border border-dashed border-border p-1">
                  <span className="text-[10px] text-fg-subtle">{t("viewer.fields.table_row", { table: tbl.key, row: ri + 1 })}</span>
                  <div className="grid grid-cols-2 gap-1">
                    {row.map((f) => (
                      <FieldRow key={f.id} f={f} compact focused={f.id === focusId} editing={editing?.id === f.id ? editing.value : null} {...rowProps} />
                    ))}
                  </div>
                </div>
              ))}
            </div>
          ))}
        </>
      )}
    </div>
  );
}
