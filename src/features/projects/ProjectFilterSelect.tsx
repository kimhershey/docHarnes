import { useTranslation } from "react-i18next";

import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useProjects } from "@/lib/queries";
import type { DocumentView } from "@/lib/types";

const ALL = "__all__";

/** Whether the document belongs to the project — as a work document or as its reference material. */
export function inProject(doc: DocumentView, projectId: string | null): boolean {
  return projectId === null || (doc.projects ?? []).some((p) => p.id === projectId);
}

/**
 * 1.0.0: narrows a document check list to one project's documents (work documents and its
 * reference material), so "전체 선택" picks a whole project at once (user request 2026-11-12:
 * "프로젝트에 해당하는 문서들을 일괄로 선택하는 기능"). Nothing is rendered while there is no project.
 */
export function ProjectFilterSelect({ value, onChange, enabled = true, testId = "project-filter" }: { value: string | null; onChange: (projectId: string | null) => void; enabled?: boolean; testId?: string }) {
  const { t } = useTranslation();
  const { data: projects } = useProjects(enabled);
  if (!projects || projects.length === 0) return null;
  return (
    <Select value={value ?? ALL} onValueChange={(v) => onChange(v === ALL ? null : v)}>
      {/* The trigger is capped in rem and cuts a long project name: it must not widen the dialog. */}
      <SelectTrigger className="h-8 w-auto min-w-0 max-w-[16rem]" aria-label={t("projects.filter.label")} data-testid={testId}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={ALL}>{t("projects.filter.all")}</SelectItem>
        {projects.map((p) => (
          <SelectItem key={p.id} value={p.id} data-testid={`${testId}-option-${p.id}`}>
            <span className="block max-w-[20rem] truncate" title={p.name}>
              {p.name}
            </span>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
