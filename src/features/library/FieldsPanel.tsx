import { Check, ChevronRight, Pause, Pencil, RefreshCw, X } from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { confidenceGlyph, confidenceTone, fieldLabel, fieldStatusVariant, formatConfidence, reasonVariant } from "@/features/pipeline/labels";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useFields, useRuns, useSchemas } from "@/lib/queries";
import { isComposing } from "@/lib/shortcuts";
import { fieldNeedsReview, type DocumentView, type FieldRecord, type ReviewAction } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

const AUTO = "__auto__";

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

/**
 * Viewer "필드" tab (design §3.7.6): schema grid with value · confidence
 * (colour + glyph) · evidence · approve / correct / hold / next, validation
 * results with "검증 다시 실행", schema override and the run selector.
 */
export function FieldsPanel({ doc, onGoToPage, onStartExtract }: { doc: DocumentView; onGoToPage: (page: number, needle?: string) => void; onStartExtract: () => void }) {
  const { t, i18n } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: runs } = useRuns(doc.id);
  const [runId, setRunId] = useState<string | null>(null);
  const { data: fields, isLoading } = useFields(doc.id, runId);
  const { data: schemas } = useSchemas();
  const [cursor, setCursor] = useState(0);
  const [editing, setEditing] = useState<{ id: number; value: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const grouped = useMemo(() => groupFields(fields ?? []), [fields]);
  const pending = useMemo(() => (fields ?? []).filter(fieldNeedsReview), [fields]);
  const pendingIds = pending.map((f) => f.id);
  const focusId = pendingIds[Math.min(cursor, Math.max(0, pendingIds.length - 1))];

  const apply = async (id: number, action: ReviewAction) => {
    setBusy(true);
    try {
      await api.pipelineReviewApply([id], action);
      setEditing(null);
      if (action.kind !== "hold") setCursor((c) => Math.min(c, Math.max(0, pendingIds.length - 2)));
      else setCursor((c) => Math.min(c + 1, Math.max(0, pendingIds.length - 1)));
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
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

  const Row = ({ f, compact }: { f: FieldRecord; compact?: boolean }) => {
    const needs = fieldNeedsReview(f);
    const isFocus = f.id === focusId;
    const isEditing = editing?.id === f.id;
    return (
      <div className={cn("rounded-md border px-2 py-1.5 text-xs", isFocus ? "border-accent bg-accent-soft" : "border-border", compact && "py-1")} data-testid={`field-${f.path}`} data-field-status={f.status} data-field-review={needs ? "pending" : "none"}>
        <div className="flex items-center gap-1">
          <span className="truncate font-medium" title={f.path}>
            {fieldLabel(f, i18n.language)}
            {f.required && <span className="text-danger">*</span>}
          </span>
          <span className={cn("ml-auto whitespace-nowrap", `text-${confidenceTone(f.value === null ? null : f.confidence)}`)} title={t("viewer.fields.confidence")} data-testid="field-confidence">
            {confidenceGlyph(f.value === null ? null : f.confidence)} {f.value === null ? "–" : formatConfidence(f.confidence)}
          </span>
          <Badge variant={fieldStatusVariant(f.status)} className="text-[10px]" data-testid="field-status">
            {t(`pipeline.field_status.${f.status}`)}
          </Badge>
          {f.review_reason && needs && (
            <Badge variant={reasonVariant(f.review_reason)} className="text-[10px]">
              {t(`pipeline.reason.${f.review_reason}`)}
            </Badge>
          )}
        </div>
        {isEditing ? (
          <div className="mt-1 flex items-center gap-1">
            <Input
              value={editing.value}
              onChange={(e) => setEditing({ id: f.id, value: e.target.value })}
              autoFocus
              className="h-7 text-xs"
              placeholder={t("viewer.fields.edit_placeholder")}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !isComposing(e)) {
                  e.preventDefault();
                  void apply(f.id, { kind: "correct", value: editing.value });
                } else if (e.key === "Escape") setEditing(null);
              }}
              data-testid="field-edit-input"
            />
            <Button size="sm" className="h-7" onClick={() => void apply(f.id, { kind: "correct", value: editing.value })} disabled={busy || !editing.value.trim()} data-testid="field-edit-save">
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
                {v.message}
              </li>
            ))}
          </ul>
        )}
        {(needs || isFocus) && !isEditing && (
          <div className="mt-1 flex flex-wrap gap-1">
            <Button size="sm" variant="secondary" className="h-6 px-2" onClick={() => void apply(f.id, { kind: "approve" })} disabled={busy} data-testid="field-approve">
              <Check />
              {t("viewer.fields.approve")}
            </Button>
            <Button size="sm" variant="ghost" className="h-6 px-2" onClick={() => setEditing({ id: f.id, value: f.value ?? "" })} data-testid="field-correct">
              <Pencil />
              {t("viewer.fields.correct")}
            </Button>
            <Button size="sm" variant="ghost" className="h-6 px-2" onClick={() => void apply(f.id, { kind: "hold" })} disabled={busy} data-testid="field-hold">
              <Pause />
              {t("viewer.fields.hold")}
            </Button>
            <Button size="sm" variant="ghost" className="h-6 px-2" onClick={() => void apply(f.id, { kind: "reject" })} disabled={busy} data-testid="field-reject">
              <X />
              {t("viewer.fields.reject")}
            </Button>
            {pendingIds.length > 1 && (
              <Button size="sm" variant="ghost" className="h-6 px-2" onClick={() => setCursor((c) => (c + 1) % pendingIds.length)} data-testid="field-next">
                <ChevronRight />
                {t("viewer.fields.next")}
              </Button>
            )}
          </div>
        )}
      </div>
    );
  };

  if (isLoading && !fields) return <Spinner />;
  const run = runs?.find((r) => r.id === (runId ?? doc.last_run_id)) ?? runs?.[0];
  return (
    <div className="flex flex-col gap-2" data-testid="fields-panel">
      <div className="flex flex-wrap items-center gap-1 text-xs">
        {runs && runs.length > 0 && (
          <Select value={runId ?? run?.id ?? ""} onValueChange={(v) => setRunId(v)}>
            <SelectTrigger className="h-7 w-36" aria-label={t("viewer.fields.run", { n: "" })} data-testid="fields-run">
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
                {i18n.language.startsWith("ko") ? s.name : (s.name_en ?? s.name)} ({s.doc_type})
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {pending.length > 0 && (
          <>
            <Badge variant="warn" data-testid="fields-pending">
              {t("viewer.fields.pending", { count: pending.length })}
            </Badge>
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
              {run.precise ? " · precise" : ""}
            </p>
          )}
          <div className="flex flex-col gap-1">
            {grouped.scalars.map((f) => (
              <Row key={f.id} f={f} />
            ))}
          </div>
          {grouped.tables.map((tbl) => (
            <div key={tbl.key} className="flex flex-col gap-1" data-testid={`fields-table-${tbl.key}`}>
              <span className="text-xs font-semibold text-fg-muted">{fieldLabel({ label_ko: tbl.rows[0]?.[0] ? tbl.key : tbl.key, label_en: tbl.key, key: tbl.key }, i18n.language)}</span>
              {tbl.rows.map((row, ri) => (
                <div key={ri} className="rounded-md border border-dashed border-border p-1">
                  <span className="text-[10px] text-fg-subtle">{t("viewer.fields.table_row", { table: tbl.key, row: ri + 1 })}</span>
                  <div className="grid grid-cols-2 gap-1">
                    {row.map((f) => (
                      <Row key={f.id} f={f} compact />
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
