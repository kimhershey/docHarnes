import { RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useFieldHistory, useRuns } from "@/lib/queries";
import type { DocumentView } from "@/lib/types";
import { formatDate } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

/** Viewer "이력" tab: extraction runs (model · tokens · time) and field snapshots with "이 값으로 복원". */
export function HistoryPanel({ doc }: { doc: DocumentView }) {
  const { t, i18n } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: runs, isLoading } = useRuns(doc.id);
  const { data: changes } = useFieldHistory(doc.id);
  const restore = async (id: number) => {
    try {
      await api.pipelineRestore(id);
      toast(t("viewer.history.restored"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  if (isLoading && !runs) return <Spinner />;
  return (
    <div className="flex flex-col gap-3 text-xs" data-testid="history-panel">
      <section>
        <h3 className="mb-1 font-semibold text-fg-muted">{t("viewer.history.runs")}</h3>
        {!runs || runs.length === 0 ? (
          <p className="text-fg-muted">{t("viewer.history.none")}</p>
        ) : (
          <ul className="flex flex-col gap-1">
            {runs.map((r, i) => (
              <li key={r.id} className="rounded-md border border-border px-2 py-1" data-testid="history-run" data-run-status={r.status}>
                <div className="flex items-center gap-1">
                  <span className="font-medium">{t("viewer.fields.run", { n: runs.length - i })}</span>
                  <Badge variant={r.status === "done" ? "ok" : r.status === "failed" ? "danger" : "muted"} className="text-[10px]">
                    {t(`viewer.history.status.${r.status}`)}
                  </Badge>
                  <span className="ml-auto text-fg-subtle">{formatDate(r.started_at, i18n.language)}</span>
                </div>
                <div className="text-fg-muted">{t("viewer.history.run_line", { model: r.model ?? "–", fields: r.field_count, review: r.review_count, ms: r.duration_ms ?? 0 })}</div>
                {r.error && <div className="text-danger selectable">{r.error}</div>}
              </li>
            ))}
          </ul>
        )}
      </section>
      <section>
        <h3 className="mb-1 font-semibold text-fg-muted">{t("viewer.history.changes")}</h3>
        {!changes || changes.length === 0 ? (
          <p className="text-fg-muted">{t("viewer.history.none")}</p>
        ) : (
          <ul className="flex flex-col gap-1">
            {changes.map((c) => (
              <li key={c.id} className="flex items-center gap-2 rounded-md border border-border px-2 py-1" data-testid="history-change">
                <div className="min-w-0 flex-1">
                  <div className="truncate font-medium">{c.path}</div>
                  <div className="truncate text-fg-muted">
                    {c.old_value ?? "–"} → {c.new_value ?? "–"} · {t(`pipeline.field_status.${c.old_status}`)} → {t(`pipeline.field_status.${c.new_status}`)}
                    {c.note ? ` · ${c.note}` : ""}
                  </div>
                  <div className="text-fg-subtle">{formatDate(c.changed_at, i18n.language)}</div>
                </div>
                <Button size="sm" variant="ghost" className="h-6" onClick={() => void restore(c.id)} data-testid="history-restore">
                  <RotateCcw />
                  {t("viewer.history.restore")}
                </Button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
