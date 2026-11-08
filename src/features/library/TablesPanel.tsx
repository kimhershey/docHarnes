import { Copy, Download, FileSpreadsheet } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { describeJobError } from "@/features/jobs/describe";
import { describeExportError } from "@/features/organize/errors";
import { asExportResult } from "@/features/organize/ExportTab";
import { useJob } from "@/features/organize/useJob";
import { describeError } from "@/lib/errors";
import { api, isTauri, revealPath } from "@/lib/ipc";
import { useTables } from "@/lib/queries";
import { isJobFinished, type DocumentView, type ExportRequest, type ExtractedTable } from "@/lib/types";
import { useDocumentsStore } from "@/stores/documents";
import { useJobsStore } from "@/stores/jobs";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";

function tsv(t: ExtractedTable): string {
  return [t.header.length ? t.header : null, ...t.rows]
    .filter((r): r is string[] => r !== null)
    .map((r) => r.join("\t"))
    .join("\n");
}

/** Viewer "표" tab: HTML / Markdown tables found in the page text with copy, CSV and XLSX export. */
export function TablesPanel({ doc, onGoToPage }: { doc: DocumentView; onGoToPage: (page: number) => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const language = useSettingsStore((s) => s.settings?.ui.language ?? "ko");
  const { data: tables, isLoading } = useTables(doc.id);
  const [xlsxJobId, setXlsxJobId] = useState<string | null>(null);
  const xlsxJob = useJob(xlsxJobId);
  const handled = useRef<string | null>(null);
  const copy = async (tbl: ExtractedTable) => {
    try {
      await navigator.clipboard.writeText(tsv(tbl));
      toast(t("viewer.tables.copied"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const csv = async (tbl: ExtractedTable) => {
    try {
      const name = `${doc.original_name.replace(/\.[^.]+$/, "")}_p${tbl.page}_t${tbl.index + 1}.csv`;
      if (isTauri) {
        const { save } = await import("@tauri-apps/plugin-dialog");
        const path = await save({ defaultPath: name, filters: [{ name: "CSV", extensions: ["csv"] }] });
        if (!path) return;
        await api.pipelineTableExport(doc.id, tbl.page, tbl.index, path);
      } else {
        const text = await api.pipelineTableCsv(doc.id, tbl.page, tbl.index);
        const url = URL.createObjectURL(new Blob([text], { type: "text/csv;charset=utf-8" }));
        const a = document.createElement("a");
        a.href = url;
        a.download = name;
        a.click();
        // The browser fetches the blob after the click returns; revoking at once can cancel the download.
        setTimeout(() => URL.revokeObjectURL(url), 1000);
      }
      toast(t("viewer.tables.saved"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  // One XLSX for the whole document (a sheet per table) through the P5 export job. The default path is
  // `exports/<today>_<type>_표.xlsx`: an existing file is never overwritten silently (design §3.7 "기존
  // 파일은 overwrite 필요") — the export tab is opened instead, scoped to this document.
  const xlsx = async () => {
    const req: ExportRequest = { format: "xlsx", target: "table", scope: { doc_ids: [doc.id] }, language };
    try {
      const preview = await api.exportPreview(req);
      if (preview.exists) {
        toast(t("export.table_xlsx_exists"), "warn");
        useDocumentsStore.getState().setSelectedIds([doc.id]);
        navigate("/organize?tab=export&selection=1&target=table");
        return;
      }
      const id = await api.exportRun(req);
      handled.current = null;
      setXlsxJobId(id);
      toast(t("export.started"), "info");
    } catch (e) {
      toast(describeExportError(e, t), "danger");
    }
  };
  // Follow the export job: the result (path, "폴더 열기") is toasted; `jobs://update` omits it, so it is fetched.
  useEffect(() => {
    if (!xlsxJob || !isJobFinished(xlsxJob) || handled.current === xlsxJob.id) return;
    handled.current = xlsxJob.id;
    void (async () => {
      if (xlsxJob.state === "done") {
        const full = asExportResult(xlsxJob.result) ? xlsxJob : await useJobsStore.getState().fetch(xlsxJob.id);
        const res = asExportResult(full?.result);
        const path = res?.path ?? xlsxJob.subject;
        toast(t("export.table_xlsx_done", { path }), "ok", { label: t("common.open_folder"), onClick: () => void revealPath(res?.absolute || path) });
      } else if (xlsxJob.state === "failed") {
        toast(t("export.failed", { message: describeJobError(xlsxJob, t) ?? "" }), "danger");
      } else {
        toast(t("export.cancelled"), "warn");
      }
    })();
  }, [xlsxJob, t, toast]);
  const exporting = !!xlsxJob && !isJobFinished(xlsxJob);
  if (isLoading && !tables) return <Spinner />;
  if (!tables || tables.length === 0) return <p className="text-sm text-fg-muted" data-testid="tables-empty">{t("viewer.tables.none")}</p>;
  return (
    <div className="flex flex-col gap-3" data-testid="tables-panel">
      {tables.map((tbl) => (
        <div key={`${tbl.page}-${tbl.index}`} className="flex flex-col gap-1" data-testid="table-card">
          <div className="flex items-center gap-1 text-xs">
            <button type="button" className="font-medium hover:underline" onClick={() => onGoToPage(tbl.page)}>
              {t("viewer.tables.page", { page: tbl.page, index: tbl.index + 1 })}
            </button>
            <span className="text-fg-subtle">· {t("viewer.tables.rows", { count: tbl.rows.length })}</span>
            <Button size="icon-sm" variant="ghost" className="ml-auto" onClick={() => void copy(tbl)} aria-label={t("viewer.tables.copy")} data-testid="table-copy">
              <Copy />
            </Button>
            <Button size="icon-sm" variant="ghost" onClick={() => void csv(tbl)} aria-label={t("viewer.tables.csv")} data-testid="table-csv">
              <Download />
            </Button>
            <Button size="icon-sm" variant="ghost" onClick={() => void xlsx()} disabled={exporting} aria-label={t("export.table_xlsx")} title={t("export.table_xlsx")} data-testid="table-xlsx">
              {exporting ? <Spinner /> : <FileSpreadsheet />}
            </Button>
          </div>
          <div className="overflow-auto rounded-md border border-border">
            <table className="w-full text-xs">
              {tbl.header.length > 0 && (
                <thead className="bg-bg-sunken">
                  <tr>
                    {tbl.header.map((h, i) => (
                      <th key={i} className="px-2 py-1 text-left font-medium">
                        {h}
                      </th>
                    ))}
                  </tr>
                </thead>
              )}
              <tbody>
                {tbl.rows.map((r, i) => (
                  <tr key={i} className="border-t border-border">
                    {r.map((c, k) => (
                      <td key={k} className="px-2 py-1 tabular-nums selectable">
                        {c}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      ))}
    </div>
  );
}
