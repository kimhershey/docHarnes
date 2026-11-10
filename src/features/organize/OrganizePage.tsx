import { ArrowLeft } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link, useSearchParams } from "react-router";

import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { SCREEN_ROUTES, type ExportTarget } from "@/lib/types";
import { useDocumentsStore } from "@/stores/documents";

import { ExportTab, type ExportPreset } from "./ExportTab";
import { OrganizeTab } from "./OrganizeTab";
import type { ProjectRequest } from "./scope";

type TabId = "organize" | "export";
const TABS: TabId[] = ["organize", "export"];
const EXPORT_TARGETS: ExportTarget[] = ["fields", "table", "text"];

/**
 * 정리 · 내보내기 (design §3.7; a sub-view of 문서함 since 0.5.0, plan §3): organise (templates + dry-run
 * diff + journaled apply) and the export wizard. The batch history moved to 작업 현황 > 정리 이력.
 * `/organize?tab=export&selection=1` from the library toolbar picks the tab and scopes the request to
 * the library selection (`useDocumentsStore.selectedIds`); `&target=table` (viewer "표 → XLSX") presets
 * the export target; `?project=<id>` ("이 프로젝트 정리 / 내보내기") scopes both tabs to that project.
 */
export function OrganizePage() {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();
  const selectedIds = useDocumentsStore((s) => s.selectedIds);
  const [tab, setTab] = useState<TabId>("organize");
  /** Bumped by `?selection=1` so the tabs switch their scope to the library selection. */
  const [selectionRequest, setSelectionRequest] = useState(0);
  const [exportPreset, setExportPreset] = useState<ExportPreset | null>(null);
  const [projectRequest, setProjectRequest] = useState<ProjectRequest | null>(null);

  useEffect(() => {
    const tabParam = searchParams.get("tab");
    const selection = searchParams.get("selection");
    const target = searchParams.get("target");
    const project = searchParams.get("project");
    if (tabParam === null && selection === null && target === null && project === null) return;
    if (tabParam && (TABS as string[]).includes(tabParam)) setTab(tabParam as TabId);
    if (selection === "1") setSelectionRequest((n) => n + 1);
    if (target && (EXPORT_TARGETS as string[]).includes(target)) setExportPreset((p) => ({ target: target as ExportTarget, nonce: (p?.nonce ?? 0) + 1 }));
    if (project) setProjectRequest((p) => ({ id: project, nonce: (p?.nonce ?? 0) + 1 }));
    const next = new URLSearchParams(searchParams);
    next.delete("tab");
    next.delete("selection");
    next.delete("target");
    next.delete("project");
    setSearchParams(next, { replace: true });
  }, [searchParams, setSearchParams]);

  return (
    <div className="flex h-full min-h-0 flex-col overflow-y-auto p-6" data-testid="page-organize">
      <div className="mb-4 flex flex-wrap items-center gap-3">
        <Button variant="ghost" size="sm" asChild>
          <Link to={SCREEN_ROUTES.library} data-testid="organize-back">
            <ArrowLeft />
            {t("organize.back")}
          </Link>
        </Button>
        <h1 className="text-xl font-semibold">{t("organize.title")}</h1>
      </div>
      <Tabs value={tab} onValueChange={(v) => setTab(v as TabId)}>
        <TabsList>
          {TABS.map((id) => (
            <TabsTrigger key={id} value={id} data-testid={`organize-tab-${id}`}>
              {t(`organize.tabs.${id}`)}
            </TabsTrigger>
          ))}
        </TabsList>
        {/* The panels stay mounted so a built plan / export preview survives a tab switch. */}
        <TabsContent value="organize" forceMount className="data-[state=inactive]:hidden">
          <OrganizeTab selectedIds={selectedIds} selectionRequest={selectionRequest} projectRequest={projectRequest} />
        </TabsContent>
        <TabsContent value="export" forceMount className="data-[state=inactive]:hidden">
          <ExportTab selectedIds={selectedIds} selectionRequest={selectionRequest} active={tab === "export"} preset={exportPreset} projectRequest={projectRequest} />
        </TabsContent>
      </Tabs>
    </div>
  );
}
