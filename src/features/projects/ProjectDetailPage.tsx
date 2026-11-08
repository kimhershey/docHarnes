import { ArrowLeft, FileOutput, Pencil, Play, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useParams, useSearchParams } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { describeJobError } from "@/features/jobs/describe";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects, useProject } from "@/lib/queries";
import { PROJECT_STATUSES, SCREEN_ROUTES, isDraftOutcome, isJobFinished, type OutputFormat, type ProjectStatus } from "@/lib/types";
import { useGenerateStore } from "@/stores/generate";
import { useJob, useJobsStore } from "@/stores/jobs";
import { useProjectsStore } from "@/stores/projects";
import { useUiStore } from "@/stores/ui";

import { DocumentsTab } from "./DocumentsTab";
import { NewProjectDialog } from "./NewProjectDialog";
import { RelationsTab } from "./RelationsTab";
import { ReportTab } from "./ReportTab";
import { ResultsTab } from "./ResultsTab";
import { doneMoveWarning } from "./rules";
import { RulesTab } from "./RulesTab";
import { SupplementDialog } from "./SupplementDialog";

type TabId = "documents" | "rules" | "results" | "relations" | "report";
const TABS: TabId[] = ["documents", "rules", "results", "relations", "report"];

/**
 * One project (design §3.15): header with the status board step, the document checklist, rules, check
 * results with evidence links, the relation graph and the summary report; "보완요청서" starts a draft job
 * and opens the result on the 문서 생성 screen.
 */
export function ProjectDetailPage() {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const { id = "" } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const toast = useUiStore((s) => s.toast);
  const setCurrentProject = useProjectsStore((s) => s.setCurrentProject);
  const { data: detail, isLoading, error } = useProject(id);
  const initialTab = searchParams.get("tab");
  const [tab, setTab] = useState<TabId>(initialTab && (TABS as string[]).includes(initialTab) ? (initialTab as TabId) : "documents");
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
    const param = searchParams.get("tab");
    if (param && (TABS as string[]).includes(param) && param !== tab) setTab(param as TabId);
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
  const applyStatus = async (status: ProjectStatus) => {
    try {
      await api.projectSetStatus(id, status);
      invalidateProjects([id]);
      toast(t("projects.card.moved", { name: detail?.project.name ?? "", status: t(`projects.status.${status}`) }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  // 완료 with open findings (or without any check) is confirmed first; cancelling keeps the current status.
  const setStatus = (status: ProjectStatus) => {
    const p = detail?.project;
    const warning = p && status === "done" ? doneMoveWarning(p) : null;
    if (!p || !warning) return void applyStatus(status);
    setConfirm({
      title: t("projects.card.done_confirm_title", { name: p.name }),
      description: warning === "unchecked" ? t("projects.card.done_confirm_unchecked") : t("projects.card.done_confirm", { mismatch: p.last_mismatch, missing: p.missing_count }),
      confirmLabel: t("projects.status.done"),
      onConfirm: () => applyStatus(status),
    });
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
    <div className="flex h-full min-h-0 flex-col" data-testid="page-project" data-project-id={p.id}>
      <div className="flex flex-col gap-2 border-b border-border px-4 py-3">
        <div className="flex flex-wrap items-center gap-2">
          <Button size="sm" variant="ghost" onClick={() => navigate(SCREEN_ROUTES.projects)} data-testid="project-back">
            <ArrowLeft />
            {t("projects.detail.back")}
          </Button>
          <h1 className="text-base font-semibold" data-testid="project-title">
            {p.name}
          </h1>
          <Select value={p.status} onValueChange={(v) => setStatus(v as ProjectStatus)}>
            <SelectTrigger className="h-8 w-36" aria-label={t("projects.detail.status")} data-testid="project-status">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {PROJECT_STATUSES.map((s) => (
                <SelectItem key={s} value={s}>
                  {t(`projects.status.${s}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
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
        <div className="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
          {p.intake_no && <span>{t("projects.card.intake_no", { no: p.intake_no })}</span>}
          {p.owner && <span>{t("projects.card.owner", { owner: p.owner })}</span>}
          {p.due_date && <span>{t("projects.card.due", { date: p.due_date })}</span>}
          {detail.template && (
            <span>
              {t("projects.detail.template")}: {ko ? detail.template.name_ko : detail.template.name_en}
            </span>
          )}
          <Badge variant="outline">{t("projects.card.documents", { count: p.document_count })}</Badge>
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
          <TabsContent value="relations">
            <RelationsTab graph={detail.relations} />
          </TabsContent>
          <TabsContent value="report">
            <ReportTab projectId={p.id} active={tab === "report"} />
          </TabsContent>
        </Tabs>
      </div>
      <SupplementDialog open={supplementOpen} onOpenChange={setSupplementOpen} detail={detail} busy={generating} onGenerate={(ids, output) => void supplement(ids, output)} />
      <NewProjectDialog open={editOpen} onOpenChange={setEditOpen} project={p} onSaved={() => invalidateProjects([id])} />
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
