import { ArrowLeft, BookOpen, Settings2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Button } from "@/components/ui/button";
import { ReferenceGroupsView } from "@/features/knowledge/ReferenceGroupsView";
import { SCREEN_ROUTES } from "@/lib/types";

/** `/projects/common` — the pinned 공통 참고 자료 entry of the projects list: its groups and documents (0.5.0 plan §2). */
export function CommonReferencePage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="page-common-reference">
      <div className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-3">
        <Button size="sm" variant="ghost" onClick={() => navigate(SCREEN_ROUTES.projects)} data-testid="common-reference-back">
          <ArrowLeft />
          {t("projects.detail.back")}
        </Button>
        <BookOpen className="size-5 text-accent" aria-hidden="true" />
        <div className="min-w-0">
          <h1 className="text-base font-semibold" data-testid="common-reference-title">
            {t("projects.common.title")}
          </h1>
          <p className="text-xs text-fg-muted">{t("projects.common.hint")}</p>
        </div>
        <Button size="sm" variant="ghost" className="ml-auto" onClick={() => navigate(`${SCREEN_ROUTES.settings}?tab=reference`)} data-testid="common-reference-advanced">
          <Settings2 />
          {t("projects.common.advanced")}
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        <ReferenceGroupsView />
      </div>
    </div>
  );
}
