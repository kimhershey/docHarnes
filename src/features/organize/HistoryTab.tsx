import { ChevronDown, ChevronRight, Redo2, Undo2 } from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Spinner } from "@/components/ui/spinner";
import { api } from "@/lib/ipc";
import { invalidateJournal, useJournalBatches, useJournalEntries } from "@/lib/queries";
import type { JournalBatch, JournalEntry } from "@/lib/types";
import { formatDate } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

import { describeOrganizeError } from "./errors";

/** A batch that can still be reverted (mirrors `JournalBatch::undoable`). */
function undoable(b: JournalBatch): boolean {
  return b.kind !== "undo" && b.undone_at === null && b.entries > 0;
}

function kindVariant(kind: string): "ok" | "warn" | "danger" | "muted" | "info" | "outline" {
  switch (kind) {
    case "delete":
      return "danger";
    case "undo":
    case "redo":
      return "muted";
    default:
      return "info";
  }
}

function Entries({ batchId }: { batchId: string }) {
  const { t, i18n } = useTranslation();
  const { data: entries, isLoading } = useJournalEntries(batchId);
  const reason = (e: JournalEntry) => (e.undo_error ? (i18n.exists(`organize.history.undo_reason.${e.undo_error}`) ? t(`organize.history.undo_reason.${e.undo_error}`) : e.undo_error) : null);
  if (isLoading && !entries) return <Spinner />;
  if (!entries || entries.length === 0) return <p className="text-xs text-fg-muted">{t("organize.history.no_entries")}</p>;
  return (
    <ul className="flex flex-col gap-1 rounded-md border border-border bg-bg-sunken/50 p-2 text-xs" data-testid="history-entries">
      {entries.map((e) => (
        <li key={e.id} className="flex flex-wrap items-center gap-2" data-testid="history-entry" data-entry-undone={e.undone_at ? "true" : "false"}>
          <span className="w-6 text-right tabular-nums text-fg-subtle">{e.seq + 1}</span>
          <Badge variant="outline">{t(`organize.history.entry_kind.${e.kind}`)}</Badge>
          <span className="selectable font-mono">{e.from_path}</span>
          <span aria-hidden="true">→</span>
          <span className="selectable font-mono">{e.to_path ?? e.trash_path ?? "—"}</span>
          {e.undone_at && <Badge variant="muted">{t("organize.history.undone")}</Badge>}
          {reason(e) && <span className="text-danger">{t("organize.history.undo_failed", { reason: reason(e) })}</span>}
        </li>
      ))}
    </ul>
  );
}

/** 이력 tab: journal batches (newest first) with undo / redo and the per-file entries. */
export function HistoryTab() {
  const { t, i18n } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data, isLoading, error } = useJournalBatches(100);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const batches = useMemo(() => [...(data ?? [])].sort((a, b) => (a.created_at < b.created_at ? 1 : a.created_at > b.created_at ? -1 : 0)), [data]);
  // Redo re-applies the latest undo (one step, design §3.5).
  const latestUndo = batches.find((b) => b.kind === "undo" && b.undone_at === null && b.entries > 0);
  const kindLabel = (kind: string) => (i18n.exists(`organize.history.kind.${kind}`) ? t(`organize.history.kind.${kind}`) : kind);

  const report = (res: Awaited<ReturnType<typeof api.journalUndo>>, startedKey: string) => {
    if (res.report) toast(t("organize.undo_done", { restored: res.report.restored, skipped: res.report.skipped.length }), res.report.skipped.length > 0 ? "warn" : "ok");
    else toast(t(startedKey), "info");
    invalidateJournal();
  };
  const undo = async (id: string) => {
    setBusy(id);
    try {
      report(await api.journalUndo(id), "organize.undo_started");
    } catch (e) {
      toast(describeOrganizeError(e, t), "danger");
    } finally {
      setBusy(null);
    }
  };
  const redo = async (id: string) => {
    setBusy(id);
    try {
      report(await api.journalRedo(), "organize.redo_started");
    } catch (e) {
      toast(describeOrganizeError(e, t), "danger");
    } finally {
      setBusy(null);
    }
  };

  return (
    <Card data-testid="history-tab">
      <CardHeader>
        <CardTitle>{t("organize.history.title")}</CardTitle>
        <CardDescription>{t("organize.history.hint")}</CardDescription>
      </CardHeader>
      <CardContent>
        {error && (
          <p className="text-sm text-danger" role="alert">
            {describeOrganizeError(error, t)}
          </p>
        )}
        {isLoading && !data && <Spinner />}
        {data && batches.length === 0 && (
          <p className="text-sm text-fg-muted" data-testid="history-empty">
            {t("organize.history.empty")}
          </p>
        )}
        {batches.length > 0 && (
          <ul className="divide-y divide-border" data-testid="history-list">
            {batches.map((b) => (
              <li key={b.id} className="flex flex-col gap-2 py-2" data-testid={`history-batch-${b.id}`} data-batch-kind={b.kind} data-batch-undone={b.undone_at ? "true" : "false"}>
                <div className="flex flex-wrap items-center gap-2 text-sm">
                  <Badge variant={kindVariant(b.kind)}>{kindLabel(b.kind)}</Badge>
                  <span className="font-medium">{b.label ?? kindLabel(b.kind)}</span>
                  <span className="text-xs text-fg-muted">{formatDate(b.created_at, i18n.language)}</span>
                  <span className="text-xs text-fg-muted">{t("organize.history.entries", { count: b.entries })}</span>
                  {b.undone_at && <Badge variant="muted">{t("organize.history.undone")}</Badge>}
                  <div className="ml-auto flex items-center gap-1">
                    {undoable(b) && (
                      <Button size="sm" variant="ghost" disabled={busy !== null} onClick={() => void undo(b.id)} data-testid="history-undo">
                        {busy === b.id ? <Spinner /> : <Undo2 />}
                        {t("organize.undo")}
                      </Button>
                    )}
                    {latestUndo?.id === b.id && (
                      <Button size="sm" variant="ghost" disabled={busy !== null} onClick={() => void redo(b.id)} data-testid="history-redo">
                        {busy === b.id ? <Spinner /> : <Redo2 />}
                        {t("organize.history.redo")}
                      </Button>
                    )}
                    <Button size="sm" variant="ghost" aria-expanded={expanded === b.id} onClick={() => setExpanded((cur) => (cur === b.id ? null : b.id))} data-testid="history-show-entries">
                      {expanded === b.id ? <ChevronDown /> : <ChevronRight />}
                      {expanded === b.id ? t("organize.history.hide_entries") : t("organize.history.show_entries")}
                    </Button>
                  </div>
                </div>
                {expanded === b.id && <Entries batchId={b.id} />}
              </li>
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}
