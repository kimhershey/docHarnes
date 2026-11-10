import { Copy, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Markdown } from "@/components/Markdown";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { useProjectReport } from "@/lib/queries";
import { useUiStore } from "@/stores/ui";

/** 리포트 section of the 결과 tab: the backend's Markdown summary (§3.14.3), rendered + copyable. */
export function ReportSection({ projectId, active }: { projectId: string; active: boolean }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data, isLoading, refetch } = useProjectReport(projectId, active);
  const copy = async () => {
    if (!data) return;
    try {
      await navigator.clipboard.writeText(data.markdown);
      toast(t("projects.report.copied"), "ok");
    } catch {
      /* clipboard unavailable */
    }
  };
  return (
    <div className="flex flex-col gap-3" data-testid="project-report">
      <div className="flex items-center gap-2">
        <Button size="sm" variant="outline" onClick={() => void copy()} disabled={!data} data-testid="project-report-copy">
          <Copy />
          {t("projects.report.copy")}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => void refetch()} aria-label={t("projects.report.refresh")}>
          <RefreshCw />
        </Button>
      </div>
      {isLoading || !data ? (
        <span className="flex items-center gap-2 text-sm text-fg-muted" role="status">
          <Spinner />
          {t("projects.report.loading")}
        </span>
      ) : (
        <div className="rounded-lg border border-border bg-bg-elevated p-4" data-testid="project-report-body">
          <Markdown text={data.markdown} />
        </div>
      )}
    </div>
  );
}
