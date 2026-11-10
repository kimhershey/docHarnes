import { ArrowLeft, CheckCircle2, FileOutput, Pencil, Play, RotateCcw, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useParams, useSearchParams } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { describeJobError } from "@/features/jobs/describe";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects, useProject } from "@/lib/queries";
import { useProjectReferenceDocs } from "@/features/knowledge/reference";
import { COMMON_REFERENCE_ID, SCREEN_ROUTES, isDraftOutcome, isJobFinished, type OutputFormat } from "@/lib/types";
import { useGenerateStore } from "@/stores/generate";
import { useJob, useJobsStore } from "@/stores/jobs";
import { useProjectsStore } from "@/stores/projects";
import { useUiStore } from "@/stores/ui";

import { CommonReferencePage } from "./CommonReferencePage";
import { DocumentsTab } from "./DocumentsTab";
import { NewProjectDialog, todayIso } from "./NewProjectDialog";
import { PhaseBadges } from "./ProjectsPage";
import { ProjectSettingsTab } from "./ProjectSettingsTab";
import { ResultsTab } from "./ResultsTab";
import { RulesTab } from "./RulesTab";
import { SupplementDialog } from "./SupplementDialog";

type TabId = "documents" | "rules" | "results" | "settings";
const TABS: TabId[] = ["documents", "rules", "results", "settings"];
/** 0.4 links (`?tab=relations|report`) land on the merged 결과 tab; 0.5 links (`?tab=reference`) on the unified 문서 tab (0.7.0). */
function tabOf(param: string | null): TabId | null {
  if (!param) return null;
  if (param === "relations" || param === "report") return "results";
  if (param === "reference") return "documents";
  return (TABS as string[]).includes(param) ? (param as TabId) : null;
}

/** `/projects/:id` — the virtual id `common` is the 공통 참고 자료 view; anything else is a project. */
export function ProjectDetailPage() {
  const { id = "" } = useParams();
  if (id === COMMON_REFERENCE_ID) return <CommonReferencePage />;
  return <ProjectView key={id} id={id} />;
}

/**
 * One project (design §3.15, 0.3.0 plan §5, 0.5.0 plan §2): header with the computed phase (시작·종료일 editors,
 * 기한 경과 badge) and the tabs 문서 (checklist + 업무 문서 and 참고 자료 in one list, 0.7.0 plan §1.1) · 규칙 ·
 * 결과 (check results with evidence links, relation graph, report) · 설정 (정리·내보내기); "보완요청서" starts a
 * draft job and opens the result on the 문서 작성 screen.
 */
function ProjectView({ id }: { id: string }) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const toast = useUiStore((s) => s.toast);
  const setCurrentProject = useProjectsStore((s) => s.setCurrentProject);
  const { data: detail, isLoading, error } = useProject(id);
  const { docs: refDocs } = useProjectReferenceDocs(id);
  const [tab, setTab] = useState<TabId>(tabOf(searchParams.get("tab")) ?? "documents");
  const [checking, setChecking] = useState(false);
  const [supplementOpen, setSupplementOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [jobId, setJobId] = useState<string | null>(null);
  const job = useJob(jobId);
  const handled = useRef<string | null>(null);
  const ko = i18n.language.startsWith("ko");

  // The opened project becomes the assistant's `current_project` (default target of project ops) and stays
  // so after leaving the page ("정합성 검사해" from the assistant screen); only a loaded project counts.
  const loadedId = detail?.project.id ?? null;
  const loadedName = detail?.project.name ?? null;
  useEffect(() => {
    if (loadedId) setCurrentProject(loadedId, loadedName);
  }, [loadedId, loadedName, setCurrentProject]);

  useEffect(() => {
    const param = tabOf(searchParams.get("tab"));
    if (param && param !== tab) setTab(param);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams]);
  const switchTab = (next: TabId) => {
    setTab(next);
    const params = new URLSearchParams(searchParams);
    params.set("tab", next);
    setSearchParams(params, { replace: true });
  };

  // The 보완요청서 draft job: when done, hand the draft over to 문서 생성.
  useEffect(() => {
    if (!job || !isJobFinished(job) || handled.current === job.id) return;
    handled.current = job.id;
    void (async () => {
      let result: unknown = job.result;
      if (!isDraftOutcome(result)) result = (await useJobsStore.getState().fetch(job.id))?.result;
      setJobId(null);
      if (job.state === "done") {
        invalidateProjects([id]);
        if (isDraftOutcome(result)) {
          useGenerateStore.getState().openDraft(result.draft);
          toast(t("projects.supplement.ready"), "ok");
          navigate(SCREEN_ROUTES.generate);
        } else {
          // Job book cleared / result not delivered: the draft store still has it.
          toast(t("projects.supplement.result_missing"), "warn", { label: t("generate.drafts"), onClick: () => navigate(SCREEN_ROUTES.generate) });
        }
      } else if (job.state === "failed") {
        toast(describeJobError(job, t) ?? t("common.error"), "danger");
      } else if (job.state === "cancelled") {
        toast(t("projects.supplement.cancelled"), "warn");
      } else if (job.state === "interrupted") {
        toast(t("projects.supplement.interrupted", { reason: t("jobs.error.interrupted") }), "warn");
      }
    })();
  }, [job, id, navigate, toast, t]);

  const check = async () => {
    setChecking(true);
    try {
      const run = await api.projectCheck(id);
      invalidateProjects([id]);
      toast(t("projects.detail.checked", { pass: run.pass, mismatch: run.mismatch, undetermined: run.undetermined, missing: run.missing }), run.mismatch > 0 || run.missing > 0 ? "warn" : "ok");
      switchTab("results");
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setChecking(false);
    }
  };
  // Dates drive the phase (plan §5): "" clears a date, an omitted date keeps its stored value.
  const saveDates = async (dates: { start_date?: string; end_date?: string }) => {
    if (!detail) return;
    try {
      await api.projectUpdate({ id, name: detail.project.name, ...dates });
      invalidateProjects([id]);
      toast(t("projects.detail.dates_saved"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const supplement = async (findingIds: string[], output: OutputFormat) => {
    try {
      const { job_id } = await api.projectRequestSupplement(id, findingIds, output);
      handled.current = null;
      setJobId(job_id);
      setSupplementOpen(false);
      toast(t("projects.supplement.started"), "info");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const remove = () => {
    if (!detail) return;
    setConfirm({
      title: t("projects.detail.delete"),
      description: t("projects.detail.delete_confirm", { name: detail.project.name }),
      confirmLabel: t("projects.detail.delete"),
      danger: true,
      onConfirm: async () => {
        try {
          await api.projectDelete(id);
          useProjectsStore.getState().clearCurrentProject(id);
          invalidateProjects();
          toast(t("projects.detail.deleted"), "ok");
          navigate(SCREEN_ROUTES.projects);
        } catch (e) {
          toast(describeError(e, t), "danger");
        }
      },
    });
  };

  if (isLoading && !detail) {
    return (
      <div className="flex h-full items-center justify-center" data-testid="page-project">
        <Spinner />
      </div>
    );
  }
  if (!detail) {
    return (
      <div className="flex flex-col gap-3 p-6" data-testid="page-project">
        <p className="text-sm text-danger">{error ? describeError(error, t) : t("projects.detail.not_found")}</p>
        <Button variant="outline" className="self-start" onClick={() => navigate(SCREEN_ROUTES.projects)}>
          <ArrowLeft />
          {t("projects.detail.back")}
        </Button>
      </div>
    );
  }
  const p = detail.project;
  const generating = !!job && !isJobFinished(job);
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="page-project" data-project-id={p.id} data-project-phase={p.phase}>
      <div className="flex flex-col gap-2 border-b border-border px-4 py-3">
        <div className="flex flex-wrap items-center gap-2">
          <Button size="sm" variant="ghost" onClick={() => navigate(SCREEN_ROUTES.projects)} data-testid="project-back">
            <ArrowLeft />
            {t("projects.detail.back")}
          </Button>
          <h1 className="text-base font-semibold" data-testid="project-title">
            {p.name}
          </h1>
          <PhaseBadges project={p} />
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <Button size="sm" variant="outline" onClick={() => setEditOpen(true)} data-testid="project-edit">
              <Pencil />
              {t("projects.detail.edit")}
            </Button>
            <Button size="sm" onClick={() => void check()} disabled={checking} data-testid="project-check-header">
              <Play />
              {checking ? t("projects.detail.checking") : t("projects.detail.check")}
            </Button>
            <Button size="sm" variant="secondary" onClick={() => setSupplementOpen(true)} disabled={generating} data-testid="project-supplement">
              <FileOutput />
              {generating ? t("projects.supplement.generating") : t("projects.detail.supplement")}
            </Button>
            <Button size="sm" variant="ghost" className="text-danger" onClick={remove} data-testid="project-delete">
              <Trash2 />
              {t("projects.detail.delete")}
            </Button>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2 text-xs text-fg-muted">
          <label className="flex items-center gap-1">
            <span>{t("projects.detail.start")}</span>
            <Input type="date" className="h-7 w-36 text-xs" value={p.start_date ?? ""} onChange={(e) => void saveDates({ start_date: e.target.value })} aria-label={t("projects.detail.start")} data-testid="project-start" />
          </label>
          <label className="flex items-center gap-1">
            <span>{t("projects.detail.end")}</span>
            <Input type="date" className="h-7 w-36 text-xs" value={p.end_date ?? ""} onChange={(e) => void saveDates({ end_date: e.target.value })} aria-label={t("projects.detail.end")} data-testid="project-end" />
          </label>
          {p.end_date ? (
            <Button size="sm" variant="ghost" className="h-7" onClick={() => void saveDates({ end_date: "" })} title={t("projects.detail.reopen_hint")} data-testid="project-reopen">
              <RotateCcw />
              {t("projects.detail.reopen")}
            </Button>
          ) : (
            <Button size="sm" variant="ghost" className="h-7" onClick={() => void saveDates({ end_date: todayIso() })} title={t("projects.detail.complete_hint")} data-testid="project-complete">
              <CheckCircle2 />
              {t("projects.detail.complete")}
            </Button>
          )}
          {p.intake_no && <span>{t("projects.card.intake_no", { no: p.intake_no })}</span>}
          {p.owner && <span>{t("projects.card.owner", { owner: p.owner })}</span>}
          {detail.template && (
            <span>
              {t("projects.detail.template")}: {ko ? detail.template.name_ko : detail.template.name_en}
            </span>
          )}
          <Badge variant="outline" data-testid="project-doc-counts">
            {t("projects.detail.doc_counts", { work: p.document_count, reference: refDocs?.length ?? 0 })}
          </Badge>
          {p.missing_count > 0 && <Badge variant="danger">{t("projects.card.missing", { count: p.missing_count })}</Badge>}
          {generating && (
            <span className="flex items-center gap-1" role="status" aria-live="polite">
              <Spinner className="size-3.5" />
              {t("projects.supplement.generating")}
            </span>
          )}
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        <Tabs value={tab} onValueChange={(v) => switchTab(v as TabId)}>
          <TabsList>
            {TABS.map((x) => (
              <TabsTrigger key={x} value={x} data-testid={`project-tab-${x}`}>
                {t(`projects.tabs.${x}`)}
              </TabsTrigger>
            ))}
          </TabsList>
          <TabsContent value="documents">
            <DocumentsTab detail={detail} />
          </TabsContent>
          <TabsContent value="rules">
            <RulesTab detail={detail} />
          </TabsContent>
          <TabsContent value="results">
            <ResultsTab detail={detail} onCheck={() => void check()} checking={checking} />
          </TabsContent>
          <TabsContent value="settings">
            <ProjectSettingsTab detail={detail} />
          </TabsContent>
        </Tabs>
      </div>
      <SupplementDialog open={supplementOpen} onOpenChange={setSupplementOpen} detail={detail} busy={generating} onGenerate={(ids, output) => void supplement(ids, output)} />
      <NewProjectDialog open={editOpen} onOpenChange={setEditOpen} project={p} onSaved={() => invalidateProjects([id])} />
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
