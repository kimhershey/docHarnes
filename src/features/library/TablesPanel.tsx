import { Copy, Download, FileSpreadsheet } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import { api, isTauri } from "@/lib/ipc";
import { useTables } from "@/lib/queries";
import type { DocumentView, ExtractedTable } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

function tsv(t: ExtractedTable): string {
  return [t.header.length ? t.header : null, ...t.rows]
    .filter((r): r is string[] => r !== null)
    .map((r) => r.join("\t"))
    .join("\n");
}

/** Viewer "표" tab: HTML / Markdown tables found in the page text with copy and CSV export. */
export function TablesPanel({ doc, onGoToPage }: { doc: DocumentView; onGoToPage: (page: number) => void }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: tables, isLoading } = useTables(doc.id);
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
  // One XLSX for the whole document (a sheet per table) through the P5 export job; the result
  // (path, "폴더 열기") is on the Jobs page.
  const xlsx = async () => {
    try {
      await api.exportRun({ format: "xlsx", target: "table", scope: { doc_ids: [doc.id] }, overwrite: true });
      toast(t("export.started"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
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
            <Button size="icon-sm" variant="ghost" onClick={() => void xlsx()} aria-label={t("export.table_xlsx")} title={t("export.table_xlsx")} data-testid="table-xlsx">
              <FileSpreadsheet />
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
