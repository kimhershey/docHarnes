import type { TFunction } from "i18next";
import { AlertCircle, Bot, Briefcase, HelpCircle, Paperclip, Plus, Send, Sparkles, User, X } from "lucide-react";
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
import { describePreRoute, executePreRoute, executeUiOps, undoSettingsChange, type UiOpResult } from "@/lib/commands";
import { docsFromIds, DropHint, importDroppedFiles, useDocumentDropTarget, type DragDoc } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { askBackText, isCancelCommand, isSlashCommand, loadRouting } from "@/lib/assistant-control";
import { looksLikeFollowUp } from "@/lib/follow-up";
import { api } from "@/lib/ipc";
import { useDocument, useKnowledgeStatus, usePipelineStatus } from "@/lib/queries";
import { isComposing, matchesChord } from "@/lib/shortcuts";
import { isJobFinished, type Answer, type AnswerRequest, type Job, type Op, type Plan, type PlanExecution, type PlanIssue, type PlanResponse, type PreRouteResult, type SearchMode, type SearchScope, type UiPlanContext } from "@/lib/types";
import { errorCode, isMac } from "@/lib/utils";
import { selectMessages, useAssistantStore, type ClarifyStatus, type Message, type NewMessage, type PlanMessage } from "@/stores/assistant";
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

import { planAnswerScope, sameSearchScope } from "./answerScope";
import { ClarifyCard } from "./ClarifyCard";
import { conversationTurns } from "./conversation";
import { clarificationQuestion, isPlanExecution, planKind, requiresConfirmation, sourceLabel, uiContextFor, unmetRequestNote } from "./describePlan";
import { EvidenceCard } from "./evidence";
import { ExecutionCard } from "./ExecutionCard";
import { UI_OPS, missingRequiredParams, paramDesc } from "./opSpecs";
import { PlanCard } from "./PlanCard";
import { ResultCard } from "./ResultCard";
import { SessionList, SessionPicker } from "./SessionList";

/** Question-like text (used only when the planner is unavailable: such input still goes to the knowledge base). */
export function looksLikeQuestion(text: string): boolean {
  const t = text.trim();
  return /[?？]$/.test(t) || /(무엇|뭐|어떻게|어디|언제|누가|얼마|왜|인가요|인지|나요|까요|가요|어떤|가능해|되나|있나|맞나|what|how|when|where|why|which|who|is there|can i)/i.test(t);
}

function hasOpenLayer(): boolean {
  return document.querySelector('[role="dialog"], [role="listbox"], [role="menu"], [cmdk-dialog]') !== null;
}

/** An "executed" card whose UI ops did not all run says so: `failed` when none ran, `partial` when some did (PLN-17). */
function executedStatus(m: Message): "done" | "partial" | "failed" {
  if (m.role !== "assistant" || m.kind !== "executed" || !m.failures || m.failures.length === 0) return "done";
  return (m.ran ?? 0) > 0 ? "partial" : "failed";
}

/** The card's badge text. */
function executedLabel(m: Message, t: TFunction): string {
  const status = executedStatus(m);
  return status === "done" ? t("assistant.executed") : status === "partial" ? t("assistant.executed_partial") : t("assistant.executed_failed");
}

/** Keeps what became of a finished job's UI actions on that job's execution card (in whichever loaded chat it is). */
function recordJobUiResults(jobId: string, results: UiOpResult[]): void {
  if (results.length === 0) return;
  const store = useAssistantStore.getState();
  for (const [sid, list] of Object.entries(store.messagesBySession)) {
    const card = list.find((m) => m.role === "assistant" && m.kind === "execution" && m.jobId === jobId);
    if (!card) continue;
    store.patch(sid, card.id, (m) => (m.role === "assistant" && m.kind === "execution" ? { ...m, uiResults: results } : m));
    return;
  }
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
  // The active chat session's messages (0.7.0 §1.6: sessions are stored per workspace; see the store).
  const messages = useAssistantStore(selectMessages);
  const activeId = useAssistantStore((s) => s.activeId);
  const sessions = useAssistantStore((s) => s.sessions);
  const hydrating = useAssistantStore((s) => s.hydrating);
  // The active chat's stored messages are read / could not be read (it takes no input before they are, KNO-15).
  const sessionLoaded = useAssistantStore((s) => s.activeId !== null && s.loaded[s.activeId] === true);
  const sessionLoadFailed = useAssistantStore((s) => s.activeId !== null && s.loadFailed[s.activeId] === true);
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
  // The session whose command is being interpreted (only that session shows the spinner / blocks its composer).
  const [planningSession, setPlanningSession] = useState<string | null>(null);
  const planning = planningSession !== null && planningSession === activeId;
  const [loadingModel, setLoadingModel] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const consumed = useRef<string | null>(null);
  const observing = useRef(new Set<string>());
  /** Jobs started from this page whose end has not been reported to it yet (see `onJobFinished`). */
  const startedJobs = useRef(new Set<string>());
  const examples = t("assistant.examples", { returnObjects: true }) as string[];

  // The conversation is keyed by the workspace path (a switch also clears it through `resetDocumentQueries`,
  // which unbinds the store even when the same folder is opened again).
  const workspacePath = workspace?.path ?? null;
  const boundWorkspace = useAssistantStore((s) => s.workspace);
  useEffect(() => {
    if (boundWorkspace !== workspacePath) useAssistantStore.getState().bind(workspacePath);
  }, [workspacePath, boundWorkspace]);

  // Every request writes into the session it started in (`sid`), even if the user switches chats meanwhile —
  // and only while that conversation still exists. The helpers below belong to the render whose handler
  // started the request, so they carry the workspace generation of that moment: a card that arrives after a
  // workspace switch (the assistant's own "워크스페이스 … 열어" included) or after its chat was deleted is
  // dropped by the store instead of being written into the other workspace's database (KNO-2).
  const generation = useAssistantStore((s) => s.generation);
  const push = (sid: string, m: NewMessage) => useAssistantStore.getState().push(sid, m, generation);
  // A patch is stored at once (a settled card must not come back unsettled after a restart, KNO-9); `defer`
  // is for the plan editor, whose every keystroke is a patch.
  const patch = (sid: string, id: number, f: (m: Message) => Message, defer = false) => useAssistantStore.getState().patch(sid, id, f, { generation, defer });
  const patchPlan = (sid: string, id: number, f: (m: PlanMessage) => PlanMessage, defer = false) => patch(sid, id, (m) => (m.role === "assistant" && m.kind === "plan" ? f(m) : m), defer);
  const messagesOf = (sid: string) => useAssistantStore.getState().messagesBySession[sid] ?? [];
  /** Whether the conversation a request started in is still there: when not, the request stops (no card, no side effect). */
  const live = (sid: string) => useAssistantStore.getState().accepts(sid, generation);

  const anyPending = messages.some((m) => m.role === "assistant" && m.kind === "answer" && m.pending);
  // How much of the conversation the next question / command will carry.
  const memoryTurns = useMemo(() => conversationTurns(messages).length, [messages]);
  const busy = anyPending || planning || hydrating || activeId === null || !sessionLoaded;
  const activePlan = [...messages].reverse().find((m): m is PlanMessage => m.role === "assistant" && m.kind === "plan" && m.status === "pending") ?? null;
  const pendingClarify = [...messages].reverse().find((m): m is Extract<Message, { kind: "clarify" }> => m.role === "assistant" && m.kind === "clarify" && m.status === "pending") ?? null;

  // A scope that no longer applies (selection cleared, project context dropped) falls back to 전체.
  useEffect(() => {
    // (back to the open project when there is one: the search stays inside it — 0.8.1)
    if (scope === "selection" && selectionKind === null) setScope(currentProject ? "project" : "all");
    else if (scope === "project" && !currentProject) setScope("all");
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
   * common reference material (`project_id`, plan §2). 선택 문서 asks about those documents — and, while a
   * project is the context (the chip in the header), together with that project's reference material and the
   * common one: `project_id` next to explicit `documents` means exactly that (0.5.0 plan A; PRJ-13). Without
   * a project it is those documents alone.
   */
  const knowledgeScope = (): SearchScope => {
    const s: SearchScope = { latest_only: knowledge?.latest_only ?? true };
    if (scope === "project" && currentProject) s.project_id = currentProject;
    if (scope === "selection" && selectionIds.length > 0) {
      s.documents = selectionIds;
      // The named documents are searched whatever their edition (the core exempts them from "최신본만"); with
      // a project the setting keeps applying to the reference material that comes along.
      if (currentProject) s.project_id = currentProject;
      else if (selectionKind === "attached") s.latest_only = false;
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
  const findEvidence = async (sid: string, query: string) => {
    const mode = searchMode;
    const id = push(sid, { role: "assistant", kind: "evidence", query, mode, response: null, error: null, pending: true });
    try {
      const response = await api.knowledgeSearch({ query, mode, scope: knowledgeScope(), limit: 30 });
      patch(sid, id, (m) => (m.role === "assistant" && m.kind === "evidence" ? { ...m, response, pending: false } : m));
    } catch (e) {
      patch(sid, id, (m) => (m.role === "assistant" && m.kind === "evidence" ? { ...m, error: e, pending: false } : m));
    }
  };

  const askKnowledge = async (sid: string, question: string, extra: Partial<AnswerRequest> = {}) => {
    if (!live(sid)) return;
    // The conversation so far travels with every question (the model resolves "그 문서", "아까 금액");
    // the previous evidence is kept for an explicit follow-up or a question that leans on the
    // conversation, so an unrelated question is still retrieved afresh and can end in "no evidence".
    const history = conversationTurns(messagesOf(sid));
    const previous =
      followUp || (history.length > 0 && looksLikeFollowUp(question))
        ? [...messagesOf(sid)].reverse().find((m): m is Extract<Message, { kind: "answer" }> => m.role === "assistant" && m.kind === "answer" && !!m.answer && !m.answer.no_evidence)
        : undefined;
    const searchScope = extra.scope ?? knowledgeScope();
    // The previous evidence is only kept while this question looks in the same place as the answer it came
    // from (KNO-1). After the scope changed — another project, a document attached, a type the plan names —
    // those passages may lie outside it: sent along, they would be cited for a document the user is not
    // asking about, and the answer could never be "no evidence".
    const keep = previous && sameSearchScope(previous.request.scope, searchScope) ? previous.answer!.sources.slice(0, 4).map((s) => s.chunk_id) : [];
    const request: AnswerRequest = {
      question,
      kind: "answer",
      scope: searchScope,
      allow_general: knowledge?.allow_general_knowledge ?? false,
      top_k: knowledge?.top_k ?? 8,
      context_tokens: knowledge?.context_tokens ?? 6000,
      keep_chunks: keep,
      history,
      ...extra,
    };
    const requestId = newRequestId();
    useKnowledgeStore.getState().beginStream(requestId);
    const id = push(sid, { role: "assistant", kind: "answer", requestId, request, answer: null, error: null, pending: true });
    try {
      // What the screen shows (open document, selection, folder, project) goes along as the session context.
      const answer = await api.knowledgeAnswer(request, requestId, planContext());
      patch(sid, id, (m) => (m.role === "assistant" && m.kind === "answer" ? { ...m, answer, pending: false } : m));
    } catch (e) {
      patch(sid, id, (m) => (m.role === "assistant" && m.kind === "answer" ? { ...m, error: e, pending: false } : m));
    } finally {
      clearStream(requestId);
      setCancelling(null);
    }
  };

  const cancelAnswer = (requestId: string) => {
    setCancelling(requestId);
    void api.knowledgeAnswerCancel(requestId).catch(() => undefined);
  };

  /** Runs the UI's part of a plan; one result per op, so the card can show an op that failed as failed. */
  const runUiActions = useCallback(async (ops: Plan["operations"]): Promise<UiOpResult[]> => (ops.length === 0 ? [] : executeUiOps(ops, navigate, t)), [navigate, t]);

  /** Sends a (confirmed, possibly edited) plan to the executor: a job or an inline execution. */
  const runPlan = async (sid: string, m: PlanMessage) => {
    const id = m.id;
    if (m.status !== "pending" || !live(sid)) return;
    // A required value the editor left empty never reaches the validator — the IPC layer rejects the plan with
    // a raw "missing field `…`". The card names the field instead and the plan stays as it is (CON-12).
    const missing = missingRequiredParams(m.plan.operations);
    if (missing.length > 0) {
      const errors: PlanIssue[] = missing.map(({ index, param }) => ({ path: `operations[${index}].${param.name}`, code: "param_required", message: t("planner.editor.required_missing", { label: paramDesc(param, i18n.language) }), blocking: true }));
      patchPlan(sid, id, (x) => ({ ...x, errors }));
      return;
    }
    patchPlan(sid, id, (x) => ({ ...x, status: "executing", errors: null }));
    try {
      // A plan made from a 되묻기 goes with the text it was made from: the executor re-checks the plan against
      // the user's words (a "전부" in the reply is what allows a plan without a scope).
      const res = await api.plannerExecute(m.plan, m.text, m.context, m.plannedText ?? null);
      // The workspace was switched (or the chat deleted) meanwhile: nothing of this run belongs here any more.
      if (!live(sid)) return;
      patchPlan(sid, id, (x) => ({ ...x, status: "done", editing: false }));
      if (res.job_id) {
        startedJobs.current.add(res.job_id);
        // The job's `ui_actions` run when its finished state is observed here (once, even after navigating away).
        useAssistantStore.getState().trackJob(
          res.job_id,
          m.plan.operations.filter((o) => UI_OPS.has(o.op)),
        );
        push(sid, { role: "assistant", kind: "execution", text: m.text, response: m.response, plan: m.plan, jobId: res.job_id });
      } else if (res.execution) {
        const resultId = push(sid, { role: "assistant", kind: "result", text: m.text, response: m.response, plan: m.plan, execution: res.execution });
        // The UI's part of the plan runs now; what became of it is kept on the result card (PLN-17).
        const uiResults = await runUiActions(res.execution.ui_actions);
        if (uiResults.length > 0) patch(sid, resultId, (x) => (x.role === "assistant" && x.kind === "result" ? { ...x, uiResults } : x));
      }
    } catch (e) {
      const code = errorCode(e);
      const detail = (e as { detail?: unknown }).detail;
      if (code === "plan_invalid" && Array.isArray(detail)) {
        patchPlan(sid, id, (x) => ({ ...x, status: "pending", errors: (detail as PlanIssue[]).filter((i) => i.blocking) }));
        return;
      }
      patchPlan(sid, id, (x) => ({ ...x, status: "pending" }));
      if (code === "plan_invalid") {
        patchPlan(sid, id, (x) => ({ ...x, errors: [{ path: "", code: "plan_invalid", message: describeError(e, t), blocking: true }] }));
        return;
      }
      useUiStore.getState().toast(describeError(e, t), "danger");
    }
  };

  /** Executes the plan card `id` as it currently is (edited plan included). */
  const execute = async (sid: string, id: number) => {
    const m = messagesOf(sid).find((x): x is PlanMessage => x.id === id && x.role === "assistant" && x.kind === "plan");
    if (m) await runPlan(sid, m);
  };

  /**
   * 되돌리기 on an executed card: puts the previous values back and marks the card — unless the setting was
   * changed again since (by a later sentence, on the settings screen, before a restart): then the value the
   * card would put back is no longer "the previous one", nothing is changed and the card says so.
   */
  const undoExecuted = async (sid: string, id: number, ops: Op[], applied: Op[] | undefined) => {
    const outcome = await undoSettingsChange(ops, applied, navigate, t);
    if (outcome === "failed") return;
    patch(sid, id, (m) => (m.role === "assistant" && m.kind === "executed" ? (outcome === "stale" ? { ...m, undoStale: true } : { ...m, undone: true }) : m));
  };

  const cancelPlan = (sid: string, id: number) => patchPlan(sid, id, (x) => ({ ...x, status: "cancelled", editing: false }));
  const patchClarify = (sid: string, id: number, status: ClarifyStatus) => patch(sid, id, (m) => (m.role === "assistant" && m.kind === "clarify" ? { ...m, status } : m));
  const cancelClarify = (sid: string, id: number) => patchClarify(sid, id, "cancelled");
  /** 취소 (button or a bare "취소" typed): drops the pending question / plan without any inference. */
  const cancelPending = (sid: string): boolean => {
    let cancelled = false;
    if (activePlan) {
      cancelPlan(sid, activePlan.id);
      cancelled = true;
    }
    if (pendingClarify) {
      cancelClarify(sid, pendingClarify.id);
      cancelled = true;
    }
    return cancelled;
  };
  /**
   * 되묻기: the user's reply to the assistant's question (or to a proposed plan) goes back to the
   * model together with the original command and the question — routed by rule, never guessed.
   */
  const askBack = async (sid: string, messageId: number, original: string, question: string, reply: string) => {
    if (busy || !live(sid)) return;
    patchClarify(sid, messageId, "answered");
    patchPlan(sid, messageId, (x) => ({ ...x, status: "cancelled", editing: false }));
    push(sid, { role: "user", text: reply });
    const composed = askBackText(original, question, reply, i18n.language.startsWith("ko"));
    const context: UiPlanContext = { ...planContext(), history: conversationTurns(messagesOf(sid)), routing: loadRouting() };
    setPlanningSession(sid);
    let response: PlanResponse;
    try {
      response = await api.plannerPlan(composed, context);
    } catch (e) {
      setPlanningSession(null);
      push(sid, { role: "assistant", kind: "error", text: describeError(e, t) });
      return;
    }
    setPlanningSession(null);
    // The card keeps the user's sentence as its text and the composed text as what the plan was made from.
    await handlePlan(sid, original, response, context, composed);
  };

  /**
   * Routes a planner response to the matching card / action. `plannedText`: the text the planner read when
   * it was not `textValue` (a 되묻기); a plan card keeps it for the executor (PLN-7).
   */
  const handlePlan = async (sid: string, textValue: string, response: PlanResponse, context: UiPlanContext, plannedText?: string) => {
    // An interpretation that arrives for a conversation that is gone (workspace switched, chat deleted) is
    // dropped as a whole: no card, no answer request and no immediate execution in the other workspace.
    if (!live(sid)) return;
    const plan = response.result.plan;
    switch (planKind(plan)) {
      case "clarify":
        push(sid, { role: "assistant", kind: "clarify", text: textValue, response, status: "pending" });
        focusInput(textValue);
        return;
      case "help":
        push(sid, { role: "assistant", kind: "help" });
        return;
      case "noop":
        // Nothing to do: one line instead of an empty plan card and a "건너뜀" result (audit USR-3).
        push(sid, { role: "assistant", kind: "reply", text: t("assistant.nothing_to_do"), source: sourceLabel(response.result.source, t) });
        return;
      case "answer": {
        const found = plan.operations.find((o) => o.op === "answer");
        const op = found && found.op === "answer" ? found : null;
        const question = op ? op.question : textValue;
        // What the planner says the plan leaves out ("위약금 조항 알려주고 메일로 보내줘": no mail is sent) is said
        // before the answer. A plan card would show the note next to its approve button; an answer has no
        // side effect to approve, so it does not wait — but the part that was dropped must not vanish.
        const unmetNote = unmetRequestNote(plan, response.result.issues, t);
        if (unmetNote) push(sid, { role: "assistant", kind: "reply", text: unmetNote, source: sourceLabel(response.result.source, t), note: "unmet" });
        // A ready reply (a fixed greeting, "this cannot be done"): shown as it is, no model call.
        if (op?.reply) {
          push(sid, { role: "assistant", kind: "reply", text: op.reply, source: sourceLabel(response.result.source, t) });
          return;
        }
        // mode=chat: small talk, nothing is looked up. mode=general: not about the documents — evidence when
        // there is some, general knowledge (marked as such) otherwise, whatever the setting says.
        // knowledge=false: the sentence carries the text to work on — answer from it, look nothing up.
        const style = op?.mode === "chat" ? "chat" : op?.mode === "general" ? "general" : null;
        const retrieve = style !== "chat" && op?.knowledge !== false;
        const base = knowledgeScope();
        // The scope the plan names is the scope of the question (KNO-5): "지난달 회의록에서 결정된 사항은?" and
        // "한빛 프로젝트 문서에서 납품 기한 알려줘" look in the documents the plan's scope resolved to (the
        // preview's), not in everything the selector allows. An answer that looks nothing up has no scope.
        const target = retrieve ? planAnswerScope(plan, response.preview) : ({ kind: "selector" } as const);
        if (target.kind === "none") {
          // The scope matches no document: "no evidence" for that scope, without a model call — an empty
          // document list in the request would search everything. (The card offers no "일반 지식으로 답하기":
          // its request has no scope to repeat.)
          const noDocuments: Answer = { id: null, kind: "answer", question, text: t("assistant.scope.no_documents"), sentences: [], sources: [], no_evidence: true, used_general_knowledge: false, conflicts: null, model: null, elapsed_ms: 0, prompt_tokens: 0, completion_tokens: 0, document_id: null, created_at: new Date().toISOString(), feedback: null };
          push(sid, { role: "assistant", kind: "answer", requestId: newRequestId(), request: { question, kind: "answer", scope: base, retrieve, history: [] }, answer: noDocuments, error: null, pending: false, scopeEmpty: true });
          return;
        }
        if (target.kind === "documents") {
          if (target.documents.length > 0) base.documents = target.documents;
          if (target.projectId) base.project_id = target.projectId;
          if (target.role) base.roles = [target.role];
        } else {
          if (plan.scope.doc_ids && plan.scope.doc_ids.length > 0) base.documents = plan.scope.doc_ids;
          if (plan.scope.folder) base.folder = plan.scope.folder;
        }
        await askKnowledge(sid, question, { scope: base, retrieve, ...(style ? { style } : {}), ...(style === "general" ? { allow_general: true } : {}) });
        return;
      }
      case "ui":
        if (!requiresConfirmation(response)) {
          const results = await executeUiOps(plan.operations, navigate, t);
          const ran = results.filter((r) => r.ok);
          const failures = results.filter((r) => !r.ok).map((r) => r.error ?? "");
          // The card says what ran — or, when nothing ran, what was asked for — and lists the ops that failed.
          const summary = ran.map((r) => r.summary).filter(Boolean).join(" · ") || plan.summary || textValue;
          // What a settings change replaced can be put back from the card (last change first) — while the
          // settings still hold what the change set (`applied`).
          const undo = ran.flatMap((r) => r.undo ?? []).reverse();
          const applied = ran.flatMap((r) => r.applied ?? []);
          push(sid, { role: "assistant", kind: "executed", text: summary, source: sourceLabel(response.result.source, t), confidence: plan.confidence, ...(failures.length > 0 ? { failures, ran: ran.length } : {}), ...(undo.length > 0 ? { undo, applied } : {}) });
          return;
        }
        break;
      default:
        break;
    }
    const card: NewMessage = { role: "assistant", kind: "plan", text: textValue, ...(plannedText ? { plannedText } : {}), response, plan, context, status: "pending", errors: null, editing: false };
    const id = push(sid, card);
    // An immediate execution works from the message itself (the store is updated synchronously).
    if (!requiresConfirmation(response)) await runPlan(sid, { ...card, id } as PlanMessage);
  };

  /**
   * Sends a sentence. `searchOnly` is the "근거만 찾기" mode of this one call: a caller that has just
   * switched the mode passes the new value, since the `evidenceOnly` this closure sees is still the old one.
   */
  const send = async (value: string, searchOnly: boolean = evidenceOnly) => {
    const v = value.trim();
    // The session the command belongs to is fixed here: a switch while it runs does not redirect its cards.
    const sid = useAssistantStore.getState().activeId;
    if (!v || busy || !sid || !live(sid)) return;
    setText("");
    push(sid, { role: "user", text: v, attachments: scope === "selection" && selectionKind === "attached" ? attached.map((d) => ({ id: d.id, name: d.name })) : undefined });
    // 0. "근거만 찾기": every sentence is a search (slash commands still run).
    if (searchOnly && !isSlashCommand(v)) {
      await findEvidence(sid, v);
      return;
    }
    // 1. Rules first, and only these (user decision 2026-11-09): a bare 취소 while a question or plan
    //    is pending drops it, and slash commands run at once — neither waits for the model.
    if (isCancelCommand(v) && (activePlan || pendingClarify)) {
      cancelPending(sid);
      push(sid, { role: "assistant", kind: "executed", text: t("assistant.cancelled_pending"), source: t("assistant.source_control"), confidence: 1 });
      return;
    }
    if (isSlashCommand(v)) {
      let route: PreRouteResult | null = null;
      try {
        route = await api.prerouterRoute(v);
      } catch {
        route = null;
      }
      if (!live(sid)) return;
      if (route?.matched && route.op === "help") {
        push(sid, { role: "assistant", kind: "help" });
        return;
      }
      if (route?.matched) {
        const outcome = await executePreRoute(route, navigate, t);
        push(sid, { role: "assistant", kind: "executed", text: outcome.summary || describePreRoute(route, t), source: t("assistant.source_slash"), confidence: route.confidence });
        return;
      }
    }
    // 2. Everything else is the model's to interpret (the phrase rules only in the legacy "rules_first" mode,
    //    설정 > 일반 > 고급), with the conversation so far.
    const context: UiPlanContext = { ...planContext(), history: conversationTurns(messagesOf(sid)), routing: loadRouting() };
    setPlanningSession(sid);
    let response: PlanResponse;
    try {
      response = await api.plannerPlan(v, context);
    } catch (e) {
      setPlanningSession(null);
      // Planner unavailable: questions still reach the knowledge base; anything else is reported.
      if (looksLikeQuestion(v)) {
        await askKnowledge(sid, v);
        return;
      }
      push(sid, { role: "assistant", kind: "error", text: describeError(e, t) });
      return;
    }
    setPlanningSession(null);
    await handlePlan(sid, v, response, context);
  };

  // Text handed over by another screen (command palette, pre-router help) is sent into the active session once
  // it exists and its messages are read. While the assistant is in the middle of an answer or an
  // interpretation `send` takes nothing: the sentence then waits in the composer (after what is already typed
  // there) instead of being lost (KNO-10).
  useEffect(() => {
    if (!pending) {
      consumed.current = null; // the same sentence may be handed over again later
      return;
    }
    if (hydrating || !activeId || !sessionLoaded || consumed.current === pending) return;
    consumed.current = pending;
    setPending(null);
    if (anyPending || planning) {
      setText((current) => (current.trim() ? `${current.trimEnd()}\n${pending}` : pending));
      window.setTimeout(() => inputRef.current?.focus(), 0);
      return;
    }
    void send(pending);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pending, hydrating, activeId, sessionLoaded]);

  useEffect(() => {
    listRef.current?.scrollTo({ top: listRef.current.scrollHeight });
  }, [messages, streams]);

  // Ctrl+Enter approves / Esc cancels the active plan card (settings.shortcuts; IME-safe). Esc inside a text
  // field (the composer, a plan editor input) is left to that field (review M5).
  useEffect(() => {
    const sid = activeId;
    if (!sid) return;
    if (!activePlan && pendingClarify) {
      const closeChord = shortcuts?.close || "Escape";
      const mac = isMac();
      const id = pendingClarify.id;
      const handler = (e: KeyboardEvent) => {
        if (isComposing(e) || e.defaultPrevented || !matchesChord(e, closeChord, mac)) return;
        if (isEditable(e.target) || hasOpenLayer() || useUiStore.getState().paletteOpen) return;
        e.preventDefault();
        cancelClarify(sid, id);
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
        void execute(sid, id);
      } else if (matchesChord(e, closeChord, mac)) {
        if (isEditable(e.target) || hasOpenLayer() || useUiStore.getState().paletteOpen) return;
        e.preventDefault();
        cancelPlan(sid, id);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeId, generation, activePlan?.id, pendingClarify?.id, shortcuts?.execute_plan, shortcuts?.close]);

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
          if (ops && ops.length > 0) recordJobUiResults(jobId, await runUiActions(ops));
        } finally {
          observing.current.delete(jobId);
        }
      })();
    }
  }, [pendingUiActions, jobs, runUiActions]);

  const feedback = async (sid: string, messageId: number, id: number, value: number | null) => {
    try {
      await api.knowledgeAnswerFeedback(id, value);
      patch(sid, messageId, (m) => (m.role === "assistant" && m.kind === "answer" && m.answer ? { ...m, answer: { ...m.answer, feedback: value } } : m));
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
    (execution: PlanExecution | null, job: Job, live: boolean) => {
      // A job this page started and has not reported yet counts as watched even when it failed before its
      // card could see it running.
      const startedHere = startedJobs.current.delete(job.id);
      if (execution) {
        const ops = useAssistantStore.getState().claimUiActions(job.id, execution.ui_actions);
        if (ops) void runUiActions(ops).then((results) => recordJobUiResults(job.id, results));
      } else if (job.state === "failed" && (live || startedHere)) {
        // Only a failure that happens now is announced: the card of a job that failed earlier says so itself,
        // and a toast on every reopening of the chat (session switch, restart) is noise (KNO-11).
        const message = describeJobError(job, t);
        if (message) useUiStore.getState().toast(message, "danger");
      }
    },
    [runUiActions, t],
  );

  const gate = pipeline?.gate;
  const gateSeconds = gate ? Math.ceil((gate.estimated_wait_ms ?? 0) / 1000) : 0;
  const lastDocName = lastDoc?.original_name ?? null;

  // The session this render shows: every card callback below writes into it (a card of another session is never on screen).
  const sid = activeId ?? "";
  const store = () => useAssistantStore.getState();
  const reportError = (e: unknown) => useUiStore.getState().toast(describeError(e, t), "danger");

  return (
    <div className="relative flex h-full" data-testid="page-assistant" {...drop.props}>
      <DropHint active={drop.isActive} over={drop.isOver} testId="assistant-drop-hint">
        {t("assistant.attachments.drop")}
      </DropHint>
      <SessionList
        sessions={sessions}
        activeId={activeId}
        busy={hydrating}
        onNew={() => void store().newSession().catch(reportError)}
        onSelect={(id) => void store().select(id)}
        onRename={(id, title) => void store().rename(id, title).catch(reportError)}
        onRemove={(id) => void store().remove(id).catch(reportError)}
      />
      <div className="flex min-w-0 flex-1 flex-col">
      <div className="flex flex-wrap items-center gap-3 border-b border-border px-4 py-2">
        <h1 className="text-base font-semibold">{t("assistant.title")}</h1>
        <SessionPicker sessions={sessions} activeId={activeId} onSelect={(id) => void store().select(id)} />
        {/* The rail has "새 채팅" (`SessionList`); this one stands in for it only where the rail is hidden (below `md`), under the same name. */}
        <Button variant="ghost" size="sm" className="md:hidden" onClick={() => void store().newSession().catch(reportError)} disabled={hydrating} data-testid="assistant-new-chat-header">
          <Plus className="size-4" aria-hidden="true" />
          {t("assistant.sessions.new")}
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

      <div ref={listRef} className="relative flex-1 overflow-y-auto p-4" data-testid="assistant-messages">
        {sessionLoadFailed && (
          <div className="mx-auto mb-3 flex max-w-3xl flex-wrap items-center gap-2 rounded-lg border border-border bg-danger-soft px-3 py-2 text-sm text-danger" role="alert" data-testid="assistant-load-failed">
            <AlertCircle className="size-4 shrink-0" aria-hidden="true" />
            <span className="min-w-0 flex-1">{t("assistant.sessions.load_failed")}</span>
            <Button size="sm" variant="outline" onClick={() => activeId && void store().reload(activeId)} data-testid="assistant-load-retry">
              {t("common.retry")}
            </Button>
          </div>
        )}
        {messages.length === 0 && sessionLoaded && (
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
        {/* A card never grows past the column (UI-2): the row's children may shrink (`*:min-w-0` — a flex item is
            otherwise as wide as its longest unbreakable line) and long paths inside the cards wrap. */}
        <div className="mx-auto flex max-w-3xl flex-col gap-3">
          {messages.map((m) =>
            m.role === "user" ? (
              <div key={m.id} className="flex max-w-full items-start gap-2 self-end *:min-w-0">
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
              <div key={m.id} className="flex items-start gap-2 *:min-w-0">
                <Bot className="mt-2 size-4 shrink-0 text-accent" aria-hidden="true" />
                {m.kind === "answer" && (
                  <div className="w-full" data-testid="assistant-card-answer">
                    <AnswerCard
                      answer={m.answer}
                      pending={m.pending}
                      streamText={streams[m.requestId] ?? ""}
                      error={m.error}
                      // A small-talk reply (the request went out with `style: "chat"`, kept on the stored message)
                      // is not a text-only answer: no "검색 없이 답변" badge.
                      chat={m.request.style === "chat"}
                      // The scope the question named matches no document (KNO-5): the card says so.
                      noEvidenceText={m.scopeEmpty ? t("assistant.scope.no_documents") : undefined}
                      onCitation={(c) => openCitation(navigate, c)}
                      // "일반 지식으로 답하기": the stored request again — the scope (and history) of the original
                      // question, not what the selector shows by now (KNO-13) — with general knowledge allowed.
                      // Not from a record (CON-16): its request was cut to fit, the scope's documents included.
                      // Not for a scope without documents either: there is no request that searched it.
                      onAskGeneral={m.clipped === "all" || m.scopeEmpty ? undefined : () => void askKnowledge(sid, m.request.question, { ...m.request, allow_general: true, keep_chunks: [] })}
                      onFeedback={(id, v) => void feedback(sid, m.id, id, v)}
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
                    // An edit drops the preview of the plan before it; the card fetches the one of the edited plan
                    // and hands it back (PLN-14).
                    editedPreview={m.preview}
                    clipped={m.clipped}
                    onPreview={(preview) => patchPlan(sid, m.id, (x) => ({ ...x, preview }), true)}
                    onChange={(plan) => patchPlan(sid, m.id, (x) => ({ ...x, plan, errors: null, preview: undefined }), true)}
                    onExecute={() => void execute(sid, m.id)}
                    onCancel={() => cancelPlan(sid, m.id)}
                    onRephrase={() => focusInput(m.text)}
                    onToggleEdit={() => patchPlan(sid, m.id, (x) => ({ ...x, editing: !x.editing }))}
                    onAskBack={(reply) => void askBack(sid, m.id, m.text, m.plan.summary ?? m.text, reply)}
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
                      // "이 결과로 질문": the model is asked now — not another search (KNO-4).
                      setEvidenceOnly(false);
                      void send(q, false);
                    }}
                  />
                )}
                {/* The execution card's result comes from the job book, whole: only a record (`all`) is marked. */}
                {m.kind === "execution" && <ExecutionCard jobId={m.jobId} plan={m.plan} response={m.response} uiResults={m.uiResults} onFinished={onJobFinished} clipped={m.clipped === "all" ? "all" : undefined} />}
                {m.kind === "result" && <ResultCard execution={m.execution} plan={m.plan} response={m.response} uiResults={m.uiResults} clipped={m.clipped} />}
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
                    onAskBack={(reply) => void askBack(sid, m.id, m.text, clarificationQuestion(m.response.result.plan) || t("planner.clarify.fallback"), reply)}
                    onCancel={() => cancelClarify(sid, m.id)}
                  />
                )}
                {m.kind === "reply" && (
                  <div className="w-full rounded-lg border border-border bg-bg-elevated p-3 text-sm" role="group" aria-label={t("assistant.reply_label")} data-testid="assistant-card-reply" data-reply-note={m.note}>
                    {/* A note about the part of the request that was not carried out reads as a warning. */}
                    <p className={m.note === "unmet" ? "flex items-start gap-1.5 whitespace-pre-line text-warn selectable" : "whitespace-pre-line selectable"}>
                      {m.note === "unmet" && <AlertCircle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />}
                      <span className="min-w-0">{m.text}</span>
                    </p>
                    <p className="mt-1 text-xs text-fg-subtle">{t("planner.card.source", { source: m.source })}</p>
                  </div>
                )}
                {(m.kind === "executed" || m.kind === "help" || m.kind === "error") && (
                  <div
                    className="w-full rounded-lg border border-border bg-bg-elevated p-3 text-sm"
                    role="group"
                    aria-label={m.kind === "help" ? t("assistant.help_title") : m.kind === "error" ? t("common.error") : executedLabel(m, t)}
                    data-testid={`assistant-card-${m.kind}`}
                    data-executed-status={m.kind === "executed" ? executedStatus(m) : undefined}
                  >
                    {m.kind === "executed" && (
                      <div className="flex flex-col gap-1">
                        <div className="flex items-center gap-2">
                          <Badge variant={executedStatus(m) === "done" ? "ok" : executedStatus(m) === "failed" ? "danger" : "warn"}>{executedLabel(m, t)}</Badge>
                          <span className="text-xs text-fg-subtle">
                            {m.source} · {t("assistant.confidence", { value: m.confidence.toFixed(2) })}
                          </span>
                        </div>
                        <p className="selectable">{m.text}</p>
                        {m.undo && m.undo.length > 0 && (
                          <div className="flex items-center gap-2">
                            {m.undone ? (
                              <span className="text-xs text-fg-muted" data-testid="executed-undone">
                                {t("assistant.undone")}
                              </span>
                            ) : m.undoStale ? (
                              <span className="text-xs text-fg-muted" data-testid="executed-undo-stale">
                                {t("assistant.undo_stale")}
                              </span>
                            ) : (
                              <Button size="sm" variant="outline" onClick={() => activeId && void undoExecuted(activeId, m.id, m.undo ?? [], m.applied)} data-testid="executed-undo">
                                {t("assistant.undo_change")}
                              </Button>
                            )}
                          </div>
                        )}
                        {(m.failures ?? []).map((failure, i) => (
                          <p key={i} className="text-xs text-danger selectable" role="alert" data-testid="executed-failure">
                            {t("planner.result.step_failed", { error: failure })}
                          </p>
                        ))}
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
                        <p className="min-w-0 selectable">{m.text}</p>
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
    </div>
  );
}
