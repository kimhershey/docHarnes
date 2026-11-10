import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Spinner } from "@/components/ui/spinner";
import { IngestWizard } from "@/features/knowledge/IngestWizard";
import { ReferenceDocsPanel } from "@/features/knowledge/ReferenceDocsPanel";
import { useProjectReferenceGroup } from "@/features/knowledge/reference";
import { describeError } from "@/lib/errors";
import type { ProjectDetail } from "@/lib/types";

/**
 * 참고 자료 tab (0.5.0 plan §2): the project's own reference group — created by the backend on the first call —
 * with its documents, the library picker, the import wizard locked to the group and the drop target.
 */
export function ReferenceTab({ detail }: { detail: ProjectDetail }) {
  const { t } = useTranslation();
  const { data: group, isLoading, error } = useProjectReferenceGroup(detail.project.id);
  const [wizard, setWizard] = useState(false);
  return (
    <div className="flex flex-col gap-3" data-testid="project-reference" data-group-id={group?.id ?? ""}>
      <p className="text-xs text-fg-subtle">{t("projects.reference.hint")}</p>
      {error ? (
        <p className="text-sm text-danger" role="alert">
          {describeError(error, t)}
        </p>
      ) : !group ? (
        <span className="flex items-center gap-2 text-sm text-fg-muted" role="status">
          <Spinner />
          {isLoading ? t("projects.reference.group_loading") : null}
        </span>
      ) : (
        <>
          <ReferenceDocsPanel group={group} testIdPrefix="project-reference" onIngest={() => setWizard(true)} />
          <IngestWizard open={wizard} collectionId={group.id} lockCollection onOpenChange={setWizard} />
        </>
      )}
    </div>
  );
}
