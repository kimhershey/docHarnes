import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router";

import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { subscribe } from "@/lib/ipc";
import { invalidateJournal } from "@/lib/queries";
import { EVENTS, type PipelineChanged } from "@/lib/types";
import { useDocumentsStore } from "@/stores/documents";

import { ExportTab } from "./ExportTab";
import { HistoryTab } from "./HistoryTab";
import { OrganizeTab } from "./OrganizeTab";

type TabId = "organize" | "export" | "history";
const TABS: TabId[] = ["organize", "export", "history"];

/**
 * 정리 / 내보내기 (design §3.7): organise (templates + dry-run diff + journaled apply), the export
 * wizard and the batch history. `/organize?tab=export&selection=1` from the library bulk bar picks
 * the tab and scopes the request to the library selection (`useDocumentsStore.selectedIds`).
 */
export function OrganizePage() {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();
  const selectedIds = useDocumentsStore((s) => s.selectedIds);
  const [tab, setTab] = useState<TabId>("organize");
  /** Bumped by `?selection=1` so the tabs switch their scope to the library selection. */
  const [selectionRequest, setSelectionRequest] = useState(0);

  useEffect(() => {
    const tabParam = searchParams.get("tab");
    const selection = searchParams.get("selection");
    if (tabParam === null && selection === null) return;
    if (tabParam && (TABS as string[]).includes(tabParam)) setTab(tabParam as TabId);
    if (selection === "1") setSelectionRequest((n) => n + 1);
    const next = new URLSearchParams(searchParams);
    next.delete("tab");
    next.delete("selection");
    setSearchParams(next, { replace: true });
  }, [searchParams, setSearchParams]);

  // Batches change from the assistant / viewer too (`pipeline://changed` kind `journal`).
  useEffect(() => {
    let off: (() => void) | null = null;
    let cancelled = false;
    void subscribe<PipelineChanged>(EVENTS.pipelineChanged, (ev) => {
      if (ev.kind === "journal") invalidateJournal();
    }).then((unsub) => {
      if (cancelled) unsub();
      else off = unsub;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  return (
    <div className="flex h-full min-h-0 flex-col overflow-y-auto p-6" data-testid="page-organize">
      <h1 className="mb-4 text-xl font-semibold">{t("nav.organize")}</h1>
      <Tabs value={tab} onValueChange={(v) => setTab(v as TabId)}>
        <TabsList>
          {TABS.map((id) => (
            <TabsTrigger key={id} value={id} data-testid={`organize-tab-${id}`}>
              {t(`organize.tabs.${id}`)}
            </TabsTrigger>
          ))}
        </TabsList>
        {/* The panels stay mounted so a built plan / export preview survives a look at the history. */}
        <TabsContent value="organize" forceMount className="data-[state=inactive]:hidden">
          <OrganizeTab selectedIds={selectedIds} selectionRequest={selectionRequest} />
        </TabsContent>
        <TabsContent value="export" forceMount className="data-[state=inactive]:hidden">
          <ExportTab selectedIds={selectedIds} selectionRequest={selectionRequest} />
        </TabsContent>
        <TabsContent value="history" forceMount className="data-[state=inactive]:hidden">
          <HistoryTab />
        </TabsContent>
      </Tabs>
    </div>
  );
}
