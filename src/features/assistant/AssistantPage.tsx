import { AlertCircle, Bot, Briefcase, HelpCircle, Paperclip, Send, Sparkles, User, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router";

import { isEditable } from "@/app/hooks/useGlobalShortcuts";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { describeJobError } from "@/features/jobs/describe";
import { AnswerCard } from "@/features/knowledge/AnswerCard";
import { openCitation } from "@/features/knowledge/AnswersPanel";
import { settingsTabPath } from "@/features/settings/tabs";
import { describePreRoute, executePreRoute, executeUiOps } from "@/lib/commands";
import { docsFromIds, DropHint, importDroppedFiles, useDocumentDropTarget, type DragDoc } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { askBackText, isCancelCommand, isSlashCommand, loadRouting } from "@/lib/assistant-control";
import { looksLikeFollowUp } from "@/lib/follow-up";
import { api } from "@/lib/ipc";
import { useDocument, useKnowledgeStatus, usePipelineStatus } from "@/lib/queries";
import { isComposing, matchesChord } from "@/lib/shortcuts";
import { isJobFinished, type AnswerRequest, type Job, type Plan, type PlanExecution, type PlanIssue, type PlanResponse, type PreRouteResult, type SearchMode, type SearchScope, type UiPlanContext } from "@/lib/types";
import { errorCode, isMac } from "@/lib/utils";
import { useAssistantStore, type ClarifyStatus, type Message, type NewMessage, type PlanMessage } from "@/stores/assistant";
import { useAttachmentsStore } from "@/stores/attachments";
import { useDocumentsStore } from "@/stores/documents";
import { useEngineStore } from "@/stores/engine";
import { useGenerateStore } from "@/stores/generate";
import { useJobsStore } from "@/stores/jobs";
import { newRequestId, useKnowledgeStore } from "@/stores/knowledge";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

import { ClarifyCard } from "./ClarifyCard";
import { conversationTurns } from "./conversation";
import { clarificationQuestion, isPlanExecution, planKind, requiresConfirmation, sourceLabel, uiContextFor } from "./describePlan";
import { EvidenceCard } from "./evidence";
import { ExecutionCard } from "./ExecutionCard";
import { UI_OPS } from "./opSpecs";
import { PlanCard } from "./PlanCard";
import { ResultCard } from "./ResultCard";

/** Question-like text (used only when the planner is unavailable: such input still goes to the knowledge base). */
export function looksLikeQuestion(text: string): boolean {
  const t = text.trim();
  return /[?？]$/.test(t) || /(무엇|뭐|어떻게|어디|언제|누가|얼마|왜|인가요|인지|나요|까요|가요|어떤|가능해|되나|있나|맞나|what|how|when|where|why|which|who|is there|can i)/i.test(t);
}

function hasOpenLayer(): boolean {
  return document.querySelector('[role="dialog"], [role="listbox"], [role="menu"], [cmdk-dialog]') !== null;
}

/** The three scopes of 0.5.0 (plan §0): everything, the current project, or the selected / attached / open documents. */
type ScopeValue = "all" | "project" | "selection";
const SEARCH_MODES: SearchMode[] = ["hybrid", "keyword", "semantic"];

/**
 * Assistant (design §3.7.6): slash / phrase commands run through the
 * pre-router; everything else goes to the natural-language planner (§3.5).
 * Questions stream a grounded answer, UI ops run at once, work plans show the
 * plan card (confirmation, preview, inline editing) and then the execution /
 * result cards of the job that replays them. The conversation lives in
 * `useAssistantStore` (per workspace) so it survives a visit to Jobs, and the
 * `ui_actions` of job-backed plans run exactly once when the finished job is
 * observed here. "근거만 찾기" (0.5.0, the former search screen) lists the hits of a
 * knowledge search instead of asking the model.
 */
export function AssistantPage() {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const pending = useUiStore((s) => s.pendingAssistantInput);
  const setPending = useUiStore((s) => s.setPendingAssistantInput);
  const lastDocument = useUiStore((s) => s.lastDocument);
  const workspace = useWorkspaceStore((s) => s.current);
  const settings = useSettingsStore((s) => s.settings);
  const knowledge = settings?.knowledge;
  const shortcuts = settings?.shortcuts;
  const streams = useKnowledgeStore((s) => s.streams);
  const clearStream = useKnowledgeStore((s) => s.clearStream);
  const folder = useDocumentsStore((s) => s.folder);
  const selectedIds = useDocumentsStore((s) => s.selectedIds);
  const currentProject = useProjectsStore((s) => s.currentProject);
  const currentProjectName = useProjectsStore((s) => s.currentProjectName);
  const { data: knowledgeStatus } = useKnowledgeStatus(!!workspace);
  const { data: pipeline } = usePipelineStatus(!!workspace);
  const messages = useAssistantStore((s) => s.messages);
  const pendingUiActions = useAssistantStore((s) => s.pendingUiActions);
  const jobs = useJobsStore((s) => s.jobs);
  // The last viewed document is offered as a scope only while no library selection exists (the
  // rules give an explicit selection precedence anyway).
  const documentScope = lastDocument !== null && selectedIds.length === 0;
  const { data: lastDoc } = useDocument(documentScope ? lastDocument.id : undefined);
  const [text, setText] = useState("");
  const [scope, setScope] = useState<ScopeValue>("all");
  // Documents dropped on the assistant (or carried here by spring-loading): the "첨부 문서" selection of questions and commands.
  const attached = useAttachmentsStore((s) => s.attached);
  const attachedIds = attached.map((d) => d.id);
  // The "selection" scope, by precedence: attachments, the library selection, the document the viewer showed last.
  const selectionKind: "attached" | "selection" | "document" | null = attached.length > 0 ? "attached" : selectedIds.length > 0 ? "selection" : documentScope ? "document" : null;
  const selectionIds = selectionKind === "attached" ? attachedIds : selectionKind === "selection" ? selectedIds : selectionKind === "document" && lastDocument ? [lastDocument.id] : [];
  // "근거만 찾기": the knowledge search alone (hybrid / keyword / semantic), no model call.
  const [evidenceOnly, setEvidenceOnly] = useState(false);
  const searchMode = useKnowledgeStore((s) => s.searchMode);
  const setSearchMode = (mode: SearchMode) => useKnowledgeStore.getState().setSearch({ searchMode: mode });
  const embeddingReady = !!knowledgeStatus?.embedding.loaded;
  // `/assistant?evidence=1` (the former /search route, the viewer's "AI 검색") starts in that mode.
  const evidenceParam = searchParams.get("evidence");
  useEffect(() => {
    if (evidenceParam !== "1") return;
    setEvidenceOnly(true);
    const next = new URLSearchParams(searchParams);
    next.delete("evidence");
    setSearchParams(next, { replace: true });
  }, [evidenceParam, searchParams, setSearchParams]);
  const attachDocs = (docs: DragDoc[]) => {
    useAttachmentsStore.getState().attach(docs);
    useUiStore.getState().toast(t("assistant.attachments.added", { count: docs.length }), "ok");
  };
  const drop = useDocumentDropTarget({
    disabled: !workspace,
    onDocuments: attachDocs,
    onFiles: (paths) =>
      void importDroppedFiles(paths, {}, t)
        .then(docsFromIds)
        .then((docs) => docs.length > 0 && attachDocs(docs)),
  });
  const [followUp, setFollowUp] = useState(false);
  const [cancelling, setCancelling] = useState<string | null>(null);
  const [planning, setPlanning] = useState(false);
  const [loadingModel, setLoadingModel] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const consumed = useRef<string | null>(null);
  const observing = useRef(new Set<string>());
  const examples = t("assistant.examples", { returnObjects: true }) as string[];

  // The conversation is keyed by the workspace path (a switch also clears it through `resetDocumentQueries`).
  const workspacePath = workspace?.path ?? null;
  useEffect(() => {
    useAssistantStore.getState().bind(workspacePath);
  }, [workspacePath]);

  const push = (m: NewMessage) => useAssistantStore.getState().push(m);
  const patch = (id: number, f: (m: Message) => Message) => useAssistantStore.getState().patch(id, f);
  const patchPlan = (id: number, f: (m: PlanMessage) => PlanMessage) => patch(id, (m) => (m.role === "assistant" && m.kind === "plan" ? f(m) : m));
  const currentMessages = () => useAssistantStore.getState().messages;

  const anyPending = messages.some((m) => m.role === "assistant" && m.kind === "answer" && m.pending);
  // How much of the conversation the next question / command will carry.
  const memoryTurns = useMemo(() => conversationTurns(messages).length, [messages]);
  const busy = anyPending || planning;
  const activePlan = [...messages].reverse().find((m): m is PlanMessage => m.role === "assistant" && m.kind === "plan" && m.status === "pending") ?? null;
  const pendingClarify = [...messages].reverse().find((m): m is Extract<Message, { kind: "clarify" }> => m.role === "assistant" && m.kind === "clarify" && m.status === "pending") ?? null;

  // A scope that no longer applies (selection cleared, project context dropped) falls back to 전체.
  useEffect(() => {
    if ((scope === "selection" && selectionKind === null) || (scope === "project" && !currentProject)) setScope("all");
  }, [scope, selectionKind, currentProject]);
  // Attaching documents selects them as the scope; an opened project becomes the scope (the chip is the scope, plan §1).
  const attachedCount = attached.length;
  useEffect(() => {
    if (attachedCount > 0) setScope("selection");
  }, [attachedCount]);
  useEffect(() => {
    if (currentProject) setScope((s) => (s === "all" ? "project" : s));
  }, [currentProject]);

  const focusInput = (value?: string) => {
    if (value !== undefined) setText(value);
    window.setTimeout(() => inputRef.current?.focus(), 0);
  };

  /**
   * Knowledge scope of the selector: 현재 프로젝트 = the project's work documents ∪ its reference group ∪ the
   * common reference material (`project_id`, plan §2); 선택 문서 asks about those documents only.
   */
  const knowledgeScope = (): SearchScope => {
    const s: SearchScope = { latest_only: knowledge?.latest_only ?? true };
    if (scope === "project" && currentProject) s.project_id = currentProject;
    if (scope === "selection" && selectionIds.length > 0) {
      s.documents = selectionIds;
      if (selectionKind === "attached") s.latest_only = false;
    }
    return s;
  };
  /** Planner context: the selection (attachments / library selection / open document) and the project scope. */
  const planContext = (): UiPlanContext => {
    const ctx = uiContextFor(scope === "selection" ? "selection" : "all", folder, selectionIds, lastDocument, currentProject);
    // The project scope travels as `project_id` next to `current_project` (the default target of project ops).
    if (scope === "project" && currentProject) ctx.project_id = currentProject;
    // 0.7.0: the draft open on the 문서 작성 screen is the default target of `revise_draft`.
    ctx.draft_id = useGenerateStore.getState().draft?.id ?? null;
    return ctx;
  };

  /** "근거만 찾기": lists the hits of a knowledge search for `query` (no model call). */
  const findEvidence = async (query: string) => {
    const mode = searchMode;
    const id = push({ role: "assistant", kind: "evidence", query, mode, response: null, error: null, pending: true });
    try {
      const response = await api.knowledgeSearch({ query, mode, scope: knowledgeScope(), limit: 30 });
      patch(id, (m) => (m.role === "assistant" && m.kind === "evidence" ? { ...m, response, pending: false } : m));
    } catch (e) {
      patch(id, (m) => (m.role === "assistant" && m.kind === "evidence" ? { ...m, error: e, pending: false } : m));
    }
  };

  const askKnowledge = async (question: string, extra: Partial<AnswerRequest> = {}) => {
    // The conversation so far travels with every question (the model resolves "그 문서", "아까 금액");
    // the previous evidence is kept for an explicit follow-up or a question that leans on the
    // conversation, so an unrelated question is still retrieved afresh and can end in "no evidence".
    const history = conversationTurns(currentMessages());
    const previous =
      followUp || (history.length > 0 && looksLikeFollowUp(question))
        ? [...currentMessages()].reverse().find((m): m is Extract<Message, { kind: "answer" }> => m.role === "assistant" && m.kind === "answer" && !!m.answer && !m.answer.no_evidence)
        : undefined;
    const request: AnswerRequest = {
      question,
      kind: "answer",
      scope: knowledgeScope(),
      allow_general: knowledge?.allow_general_knowledge ?? false,
      top_k: knowledge?.top_k ?? 8,
      context_tokens: knowledge?.context_tokens ?? 6000,
      keep_chunks: previous ? previous.answer!.sources.slice(0, 4).map((s) => s.chunk_id) : [],
      history,
      ...extra,
    };
    const requestId = newRequestId();
    useKnowledgeStore.getState().beginStream(requestId);
    const id = push({ role: "assistant", kind: "answer", requestId, request, answer: null, error: null, pending: true });
    try {
      // What the screen shows (open document, selection, folder, project) goes along as the session context.
      const answer = await api.knowledgeAnswer(request, requestId, planContext());
      patch(id, (m) => (m.role === "assistant" && m.kind === "answer" ? { ...m, answer, pending: false } : m));
    } catch (e) {
      patch(id, (m) => (m.role === "assistant" && m.kind === "answer" ? { ...m, error: e, pending: false } : m));
    } finally {
      clearStream(requestId);
      setCancelling(null);
    }
  };

  const cancelAnswer = (requestId: string) => {
    setCancelling(requestId);
    void api.knowledgeAnswerCancel(requestId).catch(() => undefined);
  };

  const runUiActions = useCallback(
    async (ops: Plan["operations"]) => {
      if (ops.length === 0) return;
      await executeUiOps(ops, navigate, t);
    },
    [navigate, t],
  );

  /** Sends a (confirmed, possibly edited) plan to the executor: a job or an inline execution. */
  const runPlan = async (m: PlanMessage) => {
    const id = m.id;
    if (m.status !== "pending") return;
    patchPlan(id, (x) => ({ ...x, status: "executing", errors: null }));
    try {
      const res = await api.plannerExecute(m.plan, m.text, m.context);
      patchPlan(id, (x) => ({ ...x, status: "done", editing: false }));
      if (res.job_id) {
        // The job's `ui_actions` run when its finished state is observed here (once, even after navigating away).
        useAssistantStore.getState().trackJob(
          res.job_id,
          m.plan.operations.filter((o) => UI_OPS.has(o.op)),
        );
        push({ role: "assistant", kind: "execution", text: m.text, response: m.response, plan: m.plan, jobId: res.job_id });
      } else if (res.execution) {
        push({ role: "assistant", kind: "result", text: m.text, response: m.response, plan: m.plan, execution: res.execution });
        await runUiActions(res.execution.ui_actions);
      }
    } catch (e) {
      const code = errorCode(e);
      const detail = (e as { detail?: unknown }).detail;
      if (code === "plan_invalid" && Array.isArray(detail)) {
        patchPlan(id, (x) => ({ ...x, status: "pending", errors: (detail as PlanIssue[]).filter((i) => i.blocking) }));
        return;
      }
      patchPlan(id, (x) => ({ ...x, status: "pending" }));
      if (code === "plan_invalid") {
        patchPlan(id, (x) => ({ ...x, errors: [{ path: "", code: "plan_invalid", message: describeError(e, t), blocking: true }] }));
        return;
      }
      useUiStore.getState().toast(describeError(e, t), "danger");
    }
  };

  /** Executes the plan card `id` as it currently is (edited plan included). */
  const execute = async (id: number) => {
    const m = currentMessages().find((x): x is PlanMessage => x.id === id && x.role === "assistant" && x.kind === "plan");
    if (m) await runPlan(m);
  };

  const cancelPlan = (id: number) => patchPlan(id, (x) => ({ ...x, status: "cancelled", editing: false }));
  const patchClarify = (id: number, status: ClarifyStatus) => patch(id, (m) => (m.role === "assistant" && m.kind === "clarify" ? { ...m, status } : m));
  const cancelClarify = (id: number) => patchClarify(id, "cancelled");
  /** 취소 (button or a bare "취소" typed): drops the pending question / plan without any inference. */
  const cancelPending = (): boolean => {
    let cancelled = false;
    if (activePlan) {
      cancelPlan(activePlan.id);
      cancelled = true;
    }
    if (pendingClarify) {
      cancelClarify(pendingClarify.id);
      cancelled = true;
    }
    return cancelled;
  };
  /**
   * 되묻기: the user's reply to the assistant's question (or to a proposed plan) goes back to the
   * model together with the original command and the question — routed by rule, never guessed.
   */
  const askBack = async (messageId: number, original: string, question: string, reply: string) => {
    if (busy) return;
    patchClarify(messageId, "answered");
    patchPlan(messageId, (x) => ({ ...x, status: "cancelled", editing: false }));
    push({ role: "user", text: reply });
    const composed = askBackText(original, question, reply, i18n.language.startsWith("ko"));
    const context: UiPlanContext = { ...planContext(), history: conversationTurns(currentMessages()), routing: loadRouting() };
    setPlanning(true);
    let response: PlanResponse;
    try {
      response = await api.plannerPlan(composed, context);
    } catch (e) {
      setPlanning(false);
      push({ role: "assistant", kind: "error", text: describeError(e, t) });
      return;
    }
    setPlanning(false);
    await handlePlan(original, response, context);
  };

  /** Routes a planner response to the matching card / action. */
  const handlePlan = async (textValue: string, response: PlanResponse, context: UiPlanContext) => {
    const plan = response.result.plan;
    switch (planKind(plan)) {
      case "clarify":
        push({ role: "assistant", kind: "clarify", text: textValue, response, status: "pending" });
        focusInput(textValue);
        return;
      case "help":
        push({ role: "assistant", kind: "help" });
        return;
      case "answer": {
        const op = plan.operations.find((o) => o.op === "answer");
        const question = op && op.op === "answer" ? op.question : textValue;
        const base = knowledgeScope();
        if (plan.scope.doc_ids && plan.scope.doc_ids.length > 0) base.documents = plan.scope.doc_ids;
        if (plan.scope.folder) base.folder = plan.scope.folder;
        // knowledge=false: the sentence carries the text to work on — answer from it, look nothing up.
        const retrieve = !(op && op.op === "answer" && op.knowledge === false);
        await askKnowledge(question, { scope: base, retrieve });
        return;
      }
      case "ui":
        if (!requiresConfirmation(response)) {
          const summaries = await executeUiOps(plan.operations, navigate, t);
          push({ role: "assistant", kind: "executed", text: summaries.join(" · ") || plan.summary || textValue, source: sourceLabel(response.result.source, t), confidence: plan.confidence });
          return;
        }
        break;
      default:
        break;
    }
    const card: NewMessage = { role: "assistant", kind: "plan", text: textValue, response, plan, context, status: "pending", errors: null, editing: false };
    const id = push(card);
    // An immediate execution works from the message itself (the store is updated synchronously).
    if (!requiresConfirmation(response)) await runPlan({ ...card, id } as PlanMessage);
  };

  const send = async (value: string) => {
    const v = value.trim();
    if (!v || busy) return;
    setText("");
    push({ role: "user", text: v, attachments: scope === "selection" && selectionKind === "attached" ? attached.map((d) => ({ id: d.id, name: d.name })) : undefined });
    // 0. "근거만 찾기": every sentence is a search (slash commands still run).
    if (evidenceOnly && !isSlashCommand(v)) {
      await findEvidence(v);
      return;
    }
    // 1. Rules first, and only these (user decision 2026-11-09): a bare 취소 while a question or plan
    //    is pending drops it, and slash commands run at once — neither waits for the model.
    if (isCancelCommand(v) && (activePlan || pendingClarify)) {
      cancelPending();
      push({ role: "assistant", kind: "executed", text: t("assistant.cancelled_pending"), source: t("assistant.source_control"), confidence: 1 });
      return;
    }
    if (isSlashCommand(v)) {
      let route: PreRouteResult | null = null;
      try {
        route = await api.prerouterRoute(v);
      } catch {
        route = null;
      }
      if (route?.matched && route.op === "help") {
        push({ role: "assistant", kind: "help" });
        return;
      }
      if (route?.matched) {
        const outcome = await executePreRoute(route, navigate, t);
        push({ role: "assistant", kind: "executed", text: outcome.summary || describePreRoute(route, t), source: t("assistant.source_slash"), confidence: route.confidence });
        return;
      }
    }
    // 2. Everything else is the model's to interpret (the phrase rules only in the legacy "rules_first" mode,
    //    설정 > 일반 > 고급), with the conversation so far.
    const context: UiPlanContext = { ...planContext(), history: conversationTurns(currentMessages()), routing: loadRouting() };
    setPlanning(true);
    let response: PlanResponse;
    try {
      response = await api.plannerPlan(v, context);
    } catch (e) {
      setPlanning(false);
      // Planner unavailable: questions still reach the knowledge base; anything else is reported.
      if (looksLikeQuestion(v)) {
        await askKnowledge(v);
        return;
      }
      push({ role: "assistant", kind: "error", text: describeError(e, t) });
      return;
    }
    setPlanning(false);
    await handlePlan(v, response, context);
  };

  useEffect(() => {
    if (pending && consumed.current !== pending) {
      consumed.current = pending;
      setPending(null);
      void send(pending);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pending]);

  useEffect(() => {
    listRef.current?.scrollTo({ top: listRef.current.scrollHeight });
  }, [messages, streams]);

  // Ctrl+Enter approves / Esc cancels the active plan card (settings.shortcuts; IME-safe). Esc inside a text
  // field (the composer, a plan editor input) is left to that field (review M5).
  useEffect(() => {
    if (!activePlan && pendingClarify) {
      const closeChord = shortcuts?.close || "Escape";
      const mac = isMac();
      const id = pendingClarify.id;
      const handler = (e: KeyboardEvent) => {
        if (isComposing(e) || e.defaultPrevented || !matchesChord(e, closeChord, mac)) return;
        if (isEditable(e.target) || hasOpenLayer() || useUiStore.getState().paletteOpen) return;
        e.preventDefault();
        cancelClarify(id);
      };
      window.addEventListener("keydown", handler);
      return () => window.removeEventListener("keydown", handler);
    }
    if (!activePlan) return;
    const executeChord = shortcuts?.execute_plan || "Ctrl+Enter";
    const closeChord = shortcuts?.close || "Escape";
    const mac = isMac();
    const id = activePlan.id;
    const handler = (e: KeyboardEvent) => {
      if (isComposing(e) || e.defaultPrevented) return;
      if (matchesChord(e, executeChord, mac)) {
        e.preventDefault();
        void execute(id);
      } else if (matchesChord(e, closeChord, mac)) {
        if (isEditable(e.target) || hasOpenLayer() || useUiStore.getState().paletteOpen) return;
        e.preventDefault();
        cancelPlan(id);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activePlan?.id, pendingClarify?.id, shortcuts?.execute_plan, shortcuts?.close]);

  // Owed `ui_actions` of job-backed plans: run once when the finished job is observed (on mount and on
  // every job update), whether or not the execution card was mounted when the job finished.
  useEffect(() => {
    for (const jobId of Object.keys(pendingUiActions)) {
      if (observing.current.has(jobId)) continue;
      const live = jobs.find((j) => j.id === jobId);
      if (live && !isJobFinished(live)) continue;
      observing.current.add(jobId);
      void (async () => {
        try {
          const store = useJobsStore.getState();
          const job = live ?? (await store.fetch(jobId));
          if (job && !isJobFinished(job)) return; // not in the list yet but still running: observed later
          const full = job && !isPlanExecution(job.result) ? await store.fetch(jobId) : job;
          const execution = full && isPlanExecution(full.result) ? full.result : null;
          const ops = useAssistantStore.getState().claimUiActions(jobId, execution ? execution.ui_actions : []);
          if (ops && ops.length > 0) await runUiActions(ops);
        } finally {
          observing.current.delete(jobId);
        }
      })();
    }
  }, [pendingUiActions, jobs, runUiActions]);

  const feedback = async (messageId: number, id: number, value: number | null) => {
    try {
      await api.knowledgeAnswerFeedback(id, value);
      patch(messageId, (m) => (m.role === "assistant" && m.kind === "answer" && m.answer ? { ...m, answer: { ...m.answer, feedback: value } } : m));
    } catch (e) {
      useUiStore.getState().toast(describeError(e, t), "danger");
    }
  };

  const visionModel = pipeline?.model.model ?? null;
  const loadModel = async () => {
    if (!visionModel) {
      navigate(settingsTabPath("models"));
      return;
    }
    setLoadingModel(true);
    try {
      await useEngineStore.getState().loadModel(visionModel);
      useUiStore.getState().toast(t("planner.ui.set_model", { id: visionModel }), "info");
    } catch (e) {
      useUiStore.getState().toast(describeError(e, t), "danger");
    } finally {
      setLoadingModel(false);
    }
  };

  const onJobFinished = useCallback(
    (execution: PlanExecution | null, job: Job) => {
      if (execution) {
        const ops = useAssistantStore.getState().claimUiActions(job.id, execution.ui_actions);
        if (ops) void runUiActions(ops);
      } else if (job.state === "failed") {
        const message = describeJobError(job, t);
        if (message) useUiStore.getState().toast(message, "danger");
      }
    },
    [runUiActions, t],
  );

  const gate = pipeline?.gate;
  const gateSeconds = gate ? Math.ceil((gate.estimated_wait_ms ?? 0) / 1000) : 0;
  const lastDocName = lastDoc?.original_name ?? null;

  return (
    <div className="relative flex h-full flex-col" data-testid="page-assistant" {...drop.props}>
      <DropHint active={drop.isActive} over={drop.isOver} testId="assistant-drop-hint">
        {t("assistant.attachments.drop")}
      </DropHint>
      <div className="flex flex-wrap items-center gap-3 border-b border-border px-4 py-2">
        <h1 className="text-base font-semibold">{t("assistant.title")}</h1>
        <Button variant="ghost" size="sm" onClick={() => useAssistantStore.getState().clear()}>
          {t("assistant.new_chat")}
        </Button>
        <div className="ml-auto flex flex-wrap items-center gap-3 text-xs text-fg-muted">
          <label className="flex items-center gap-1" title={t("assistant.evidence.hint")}>
            <Switch checked={evidenceOnly} onCheckedChange={setEvidenceOnly} aria-label={t("assistant.evidence.toggle")} data-testid="assistant-evidence-only" />
            {t("assistant.evidence.toggle")}
          </label>
          {evidenceOnly && (
            <Select value={searchMode} onValueChange={(v) => setSearchMode(v as SearchMode)}>
              <SelectTrigger className="h-7 w-28" aria-label={t("search.mode.label")} data-testid="assistant-evidence-mode">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {SEARCH_MODES.map((m) => (
                  <SelectItem key={m} value={m}>
                    {t(`search.mode.${m}`)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          <span className="rounded-full bg-bg-muted px-2 py-0.5" title={t("assistant.memory.hint")} data-testid="assistant-memory" data-turns={memoryTurns}>
            {t("assistant.memory.turns", { count: memoryTurns })}
          </span>
          <label className="flex items-center gap-1" title={t("answer.follow_up_hint")}>
            <Switch checked={followUp} onCheckedChange={setFollowUp} aria-label={t("answer.follow_up")} data-testid="assistant-follow-up" />
            {t("answer.follow_up")}
          </label>
          {currentProject && (
            <span className="flex items-center gap-1 rounded-full bg-accent-soft px-2 py-0.5 text-accent" data-testid="assistant-current-project" data-project-id={currentProject}>
              <Briefcase className="size-3.5" aria-hidden="true" />
              <span className="max-w-48 truncate">{t("assistant.scope.project_named", { name: currentProjectName ?? currentProject })}</span>
              <button type="button" className="cursor-pointer rounded-full hover:bg-accent hover:text-accent-fg" onClick={() => useProjectsStore.getState().clearCurrentProject()} aria-label={t("assistant.scope.project_clear")} data-testid="assistant-current-project-clear">
                <X className="size-3" aria-hidden="true" />
              </button>
            </span>
          )}
          <span>{t("assistant.scope.label")}:</span>
          <Select value={scope} onValueChange={(v) => setScope(v as ScopeValue)}>
            <SelectTrigger className="h-8 w-52" aria-label={t("assistant.scope.label")} data-testid="assistant-scope">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">{t("assistant.scope.all")}</SelectItem>
              <SelectItem value="project" disabled={!currentProject} data-testid="assistant-scope-project">
                {currentProject ? t("assistant.scope.project_named", { name: currentProjectName ?? currentProject }) : t("assistant.scope.project")}
              </SelectItem>
              {/* One "selection" entry whose label (and test id) says what it holds: attachments, the library selection or the open document. */}
              <SelectItem value="selection" disabled={selectionKind === null} data-testid={`assistant-scope-${selectionKind ?? "selection"}`}>
                {selectionKind === "attached"
                  ? t("assistant.scope.attached", { count: attached.length })
                  : selectionKind === "document"
                    ? lastDocName
                      ? t("assistant.scope.document_named", { name: lastDocName })
                      : t("assistant.scope.document")
                    : t("assistant.scope.selection_count", { count: selectedIds.length })}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      <div ref={listRef} className="flex-1 overflow-y-auto p-4" data-testid="assistant-messages">
        {messages.length === 0 && (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-fg-muted">
            <Sparkles className="size-8 text-accent" aria-hidden="true" />
            <p>{t("assistant.empty")}</p>
            <p className="text-xs">{t("assistant.qa_hint")}</p>
            <p className="text-xs">{t("assistant.attachments.hint")}</p>
            {selectionKind === "document" && lastDocName && scope !== "selection" && (
              <p className="text-xs" data-testid="assistant-current-document">
                {t("assistant.scope.document_named", { name: lastDocName })}
              </p>
            )}
          </div>
        )}
        <div className="mx-auto flex max-w-3xl flex-col gap-3">
          {messages.map((m) =>
            m.role === "user" ? (
              <div key={m.id} className="flex items-start gap-2 self-end">
                <div className="rounded-lg bg-accent px-3 py-2 text-sm text-accent-fg selectable">
                  {m.text}
                  {m.attachments && m.attachments.length > 0 && (
                    <div className="mt-1 flex flex-wrap items-center gap-1 text-xs opacity-90" data-testid="assistant-message-attachments">
                      <Paperclip className="size-3" aria-hidden="true" />
                      {m.attachments.map((a) => (
                        <span key={a.id} className="max-w-40 truncate rounded-full bg-bg/20 px-1.5">
                          {a.name}
                        </span>
                      ))}
                    </div>
                  )}
                </div>
                <User className="mt-2 size-4 text-fg-muted" aria-hidden="true" />
              </div>
            ) : (
              <div key={m.id} className="flex items-start gap-2">
                <Bot className="mt-2 size-4 shrink-0 text-accent" aria-hidden="true" />
                {m.kind === "answer" && (
                  <div className="w-full" data-testid="assistant-card-answer">
                    <AnswerCard
                      answer={m.answer}
                      pending={m.pending}
                      streamText={streams[m.requestId] ?? ""}
                      error={m.error}
                      onCitation={(c) => openCitation(navigate, c)}
                      onAskGeneral={() => void askKnowledge(m.request.question, { allow_general: true, keep_chunks: [] })}
                      onFeedback={(id, v) => void feedback(m.id, id, v)}
                      onCancel={m.pending && cancelling !== m.requestId ? () => cancelAnswer(m.requestId) : undefined}
                    />
                  </div>
                )}
                {m.kind === "plan" && (
                  <PlanCard
                    response={m.response}
                    plan={m.plan}
                    status={m.status}
                    errors={m.errors}
                    editing={m.editing}
                    active={activePlan?.id === m.id}
                    onChange={(plan) => patchPlan(m.id, (x) => ({ ...x, plan, errors: null }))}
                    onExecute={() => void execute(m.id)}
                    onCancel={() => cancelPlan(m.id)}
                    onRephrase={() => focusInput(m.text)}
                    onToggleEdit={() => patchPlan(m.id, (x) => ({ ...x, editing: !x.editing }))}
                    onAskBack={(reply) => void askBack(m.id, m.text, m.plan.summary ?? m.text, reply)}
                  />
                )}
                {m.kind === "evidence" && (
                  <EvidenceCard
                    query={m.query}
                    mode={m.mode}
                    response={m.response}
                    error={m.error}
                    pending={m.pending}
                    embeddingReady={embeddingReady}
                    onAsk={(q) => {
                      setEvidenceOnly(false);
                      void send(q);
                    }}
                  />
                )}
                {m.kind === "execution" && <ExecutionCard jobId={m.jobId} plan={m.plan} response={m.response} onFinished={onJobFinished} />}
                {m.kind === "result" && <ResultCard execution={m.execution} plan={m.plan} response={m.response} />}
                {m.kind === "clarify" && (
                  <ClarifyCard
                    question={clarificationQuestion(m.response.result.plan) || t("planner.clarify.fallback")}
                    confidence={m.response.result.plan.confidence}
                    status={m.status}
                    noModel={m.response.result.source === "none" && m.response.result.issues.some((i) => i.code === "no_model")}
                    modelId={visionModel}
                    loading={loadingModel}
                    onLoadModel={() => void loadModel()}
                    onRephrase={() => focusInput(m.text)}
                    onAskBack={(reply) => void askBack(m.id, m.text, clarificationQuestion(m.response.result.plan) || t("planner.clarify.fallback"), reply)}
                    onCancel={() => cancelClarify(m.id)}
                  />
                )}
                {(m.kind === "executed" || m.kind === "help" || m.kind === "error") && (
                  <div className="w-full rounded-lg border border-border bg-bg-elevated p-3 text-sm" role="group" aria-label={m.kind === "help" ? t("assistant.help_title") : m.kind === "error" ? t("common.error") : t("assistant.executed")} data-testid={`assistant-card-${m.kind}`}>
                    {m.kind === "executed" && (
                      <div className="flex flex-col gap-1">
                        <div className="flex items-center gap-2">
                          <Badge variant="ok">{t("assistant.executed")}</Badge>
                          <span className="text-xs text-fg-subtle">
                            {m.source} · {t("assistant.confidence", { value: m.confidence.toFixed(2) })}
                          </span>
                        </div>
                        <p className="selectable">{m.text}</p>
                      </div>
                    )}
                    {m.kind === "help" && (
                      <div className="flex flex-col gap-1">
                        <div className="flex items-center gap-2 font-medium">
                          <HelpCircle className="size-4" aria-hidden="true" />
                          {t("assistant.help_title")}
                        </div>
                        <p className="whitespace-pre-line text-fg-muted">{t("assistant.help_body")}</p>
                      </div>
                    )}
                    {m.kind === "error" && (
                      <div className="flex items-start gap-2 text-danger" role="alert">
                        <AlertCircle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
                        <p className="selectable">{m.text}</p>
                      </div>
                    )}
                  </div>
                )}
              </div>
            ),
          )}
          {planning && (
            <div className="flex items-center gap-2 text-xs text-fg-muted" role="status" aria-live="polite" data-testid="assistant-planning">
              <Spinner />
              <span>{t("assistant.planning")}</span>
              {gate?.batch_busy && <span data-testid="assistant-gate-notice">· {t("planner.execution.gate_notice", { seconds: gateSeconds })}</span>}
            </div>
          )}
        </div>
      </div>

      <div className="border-t border-border p-3">
        <div className="mx-auto flex max-w-3xl flex-col gap-2">
          {attached.length > 0 && (
            <div className="flex flex-wrap items-center gap-1.5 text-xs" data-testid="assistant-attachments">
              <Paperclip className="size-3.5 text-fg-muted" aria-hidden="true" />
              <span className="text-fg-muted">{t("assistant.attachments.count", { count: attached.length })}</span>
              {attached.map((d) => (
                <span key={d.id} className="flex items-center gap-1 rounded-full bg-bg-sunken px-2 py-0.5" data-testid={`assistant-attachment-${d.id}`}>
                  <button type="button" className="max-w-40 truncate hover:underline cursor-pointer" onClick={() => navigate(`/library/${d.id}`)} title={d.name}>
                    {d.name}
                  </button>
                  <button type="button" className="text-fg-muted hover:text-fg cursor-pointer" onClick={() => useAttachmentsStore.getState().detach(d.id)} aria-label={t("assistant.attachments.remove", { name: d.name })} data-testid={`assistant-attachment-remove-${d.id}`}>
                    <X className="size-3" aria-hidden="true" />
                  </button>
                </span>
              ))}
              <button type="button" className="text-fg-muted underline hover:text-fg cursor-pointer" onClick={() => useAttachmentsStore.getState().clearAttached()} data-testid="assistant-attachments-clear">
                {t("assistant.attachments.clear")}
              </button>
            </div>
          )}
          <div className="flex flex-wrap gap-2">
            {examples.map((ex) => (
              <button key={ex} type="button" className="rounded-full border border-border bg-bg px-3 py-1 text-xs text-fg-muted hover:border-accent hover:text-fg cursor-pointer" onClick={() => void send(ex)} data-testid="assistant-example">
                {ex}
              </button>
            ))}
          </div>
          <form
            className="flex items-end gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              void send(text);
            }}
          >
            <Textarea
              ref={inputRef}
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder={evidenceOnly ? t("search.placeholder") : t("assistant.placeholder")}
              aria-label={evidenceOnly ? t("search.placeholder") : t("assistant.placeholder")}
              rows={2}
              className="resize-none"
              lang={i18n.language}
              data-testid="assistant-input"
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey && !e.ctrlKey && !e.metaKey) {
                  if (isComposing(e)) return; // Korean IME: let the composition finish
                  e.preventDefault();
                  void send(text);
                }
              }}
            />
            <Button type="submit" aria-label={t("assistant.send")} disabled={busy} data-testid="assistant-send">
              <Send />
            </Button>
          </form>
        </div>
      </div>
    </div>
  );
}
