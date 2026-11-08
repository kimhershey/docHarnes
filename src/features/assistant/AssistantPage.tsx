import { AlertCircle, Bot, HelpCircle, Send, Sparkles, User } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { AnswerCard } from "@/features/knowledge/AnswerCard";
import { openCitation } from "@/features/knowledge/AnswersPanel";
import { scopeFor } from "@/features/search/SearchPage";
import { describePreRoute, executePreRoute, executeUiOps } from "@/lib/commands";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useCollections, usePipelineStatus } from "@/lib/queries";
import { isComposing, matchesChord } from "@/lib/shortcuts";
import { SCREEN_ROUTES, type Answer, type AnswerRequest, type Job, type Plan, type PlanExecution, type PlanIssue, type PlanResponse, type PreRouteResult, type UiPlanContext } from "@/lib/types";
import { errorCode, isMac } from "@/lib/utils";
import { useDocumentsStore } from "@/stores/documents";
import { useEngineStore } from "@/stores/engine";
import { newRequestId, useKnowledgeStore } from "@/stores/knowledge";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

import { ClarifyCard } from "./ClarifyCard";
import { clarificationQuestion, planKind, requiresConfirmation, sourceLabel, uiContextFor } from "./describePlan";
import { ExecutionCard } from "./ExecutionCard";
import { PlanCard, type PlanStatus } from "./PlanCard";
import { ResultCard } from "./ResultCard";

type Message =
  | { id: number; role: "user"; text: string }
  | { id: number; role: "assistant"; kind: "executed"; text: string; source: string; confidence: number }
  | { id: number; role: "assistant"; kind: "answer"; requestId: string; request: AnswerRequest; answer: Answer | null; error: unknown; pending: boolean }
  | { id: number; role: "assistant"; kind: "help" }
  | { id: number; role: "assistant"; kind: "plan"; text: string; response: PlanResponse; plan: Plan; context: UiPlanContext; status: PlanStatus; errors: PlanIssue[] | null; editing: boolean }
  | { id: number; role: "assistant"; kind: "execution"; text: string; response: PlanResponse; plan: Plan; jobId: string }
  | { id: number; role: "assistant"; kind: "result"; text: string; response: PlanResponse; plan: Plan; execution: PlanExecution }
  | { id: number; role: "assistant"; kind: "clarify"; text: string; response: PlanResponse }
  | { id: number; role: "assistant"; kind: "error"; text: string };

type PlanMessage = Extract<Message, { kind: "plan" }>;
type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;
type NewMessage = DistributiveOmit<Message, "id">;

let seq = 1;

/** Question-like text (used only when the planner is unavailable: such input still goes to the knowledge base). */
export function looksLikeQuestion(text: string): boolean {
  const t = text.trim();
  return /[?？]$/.test(t) || /(무엇|뭐|어떻게|어디|언제|누가|얼마|왜|인가요|인지|나요|까요|가요|어떤|가능해|되나|있나|맞나|what|how|when|where|why|which|who|is there|can i)/i.test(t);
}

function hasOpenLayer(): boolean {
  return document.querySelector('[role="dialog"], [role="listbox"], [role="menu"], [cmdk-dialog]') !== null;
}

/**
 * Assistant (design §3.7.6): slash / phrase commands run through the
 * pre-router; everything else goes to the natural-language planner (§3.5).
 * Questions stream a grounded answer, UI ops run at once, work plans show the
 * plan card (confirmation, preview, inline editing) and then the execution /
 * result cards of the job that replays them.
 */
export function AssistantPage() {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
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
  const { data: collections } = useCollections(!!workspace);
  const { data: pipeline } = usePipelineStatus(!!workspace);
  const [messages, setMessages] = useState<Message[]>([]);
  const [text, setText] = useState("");
  const [scope, setScope] = useState<string>("all");
  const [followUp, setFollowUp] = useState(false);
  const [cancelling, setCancelling] = useState<string | null>(null);
  const [planning, setPlanning] = useState(false);
  const [loadingModel, setLoadingModel] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const consumed = useRef<string | null>(null);
  const messagesRef = useRef<Message[]>([]);
  messagesRef.current = messages;
  const examples = t("assistant.examples", { returnObjects: true }) as string[];

  const push = (m: NewMessage) => {
    const id = seq++;
    setMessages((prev) => [...prev, { ...m, id } as Message]);
    return id;
  };
  const patch = (id: number, f: (m: Message) => Message) => setMessages((prev) => prev.map((m) => (m.id === id ? f(m) : m)));
  const patchPlan = (id: number, f: (m: PlanMessage) => PlanMessage) => patch(id, (m) => (m.role === "assistant" && m.kind === "plan" ? f(m) : m));

  const anyPending = messages.some((m) => m.role === "assistant" && m.kind === "answer" && m.pending);
  const busy = anyPending || planning;
  const activePlan = [...messages].reverse().find((m): m is PlanMessage => m.role === "assistant" && m.kind === "plan" && m.status === "pending") ?? null;

  // A scope that no longer applies (selection cleared, folder left) falls back to the whole library.
  useEffect(() => {
    if ((scope === "selection" && selectedIds.length === 0) || (scope === "folder" && !folder)) setScope("all");
  }, [scope, selectedIds.length, folder]);

  const focusInput = (value?: string) => {
    if (value !== undefined) setText(value);
    window.setTimeout(() => inputRef.current?.focus(), 0);
  };

  const askKnowledge = async (question: string, extra: Partial<AnswerRequest> = {}) => {
    // Only an explicit follow-up keeps the previous evidence; otherwise every question is retrieved afresh
    // so unrelated questions can still end in "no evidence".
    const previous = followUp
      ? [...messagesRef.current].reverse().find((m): m is Extract<Message, { kind: "answer" }> => m.role === "assistant" && m.kind === "answer" && !!m.answer && !m.answer.no_evidence)
      : undefined;
    const request: AnswerRequest = {
      question,
      kind: "answer",
      scope: scopeFor(scope, knowledge?.latest_only ?? true),
      allow_general: knowledge?.allow_general_knowledge ?? false,
      top_k: knowledge?.top_k ?? 8,
      context_tokens: knowledge?.context_tokens ?? 6000,
      keep_chunks: previous ? previous.answer!.sources.slice(0, 4).map((s) => s.chunk_id) : [],
      ...extra,
    };
    const requestId = newRequestId();
    useKnowledgeStore.getState().beginStream(requestId);
    const id = push({ role: "assistant", kind: "answer", requestId, request, answer: null, error: null, pending: true });
    try {
      const answer = await api.knowledgeAnswer(request, requestId);
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
    const m = messagesRef.current.find((x): x is PlanMessage => x.id === id && x.role === "assistant" && x.kind === "plan");
    if (m) await runPlan(m);
  };

  const cancelPlan = (id: number) => patchPlan(id, (x) => ({ ...x, status: "cancelled", editing: false }));

  /** Routes a planner response to the matching card / action. */
  const handlePlan = async (textValue: string, response: PlanResponse, context: UiPlanContext) => {
    const plan = response.result.plan;
    switch (planKind(plan)) {
      case "clarify":
        push({ role: "assistant", kind: "clarify", text: textValue, response });
        focusInput(textValue);
        return;
      case "help":
        push({ role: "assistant", kind: "help" });
        return;
      case "answer": {
        const op = plan.operations.find((o) => o.op === "answer");
        const question = op && op.op === "answer" ? op.question : textValue;
        const base = scopeFor(scope, knowledge?.latest_only ?? true);
        if (plan.scope.doc_ids && plan.scope.doc_ids.length > 0) base.documents = plan.scope.doc_ids;
        if (plan.scope.folder) base.folder = plan.scope.folder;
        await askKnowledge(question, { scope: base });
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
    // The card is pushed before the state flushes, so an immediate execution works from the message itself.
    const message: PlanMessage = { id: seq++, role: "assistant", kind: "plan", text: textValue, response, plan, context, status: "pending", errors: null, editing: false };
    setMessages((prev) => [...prev, message]);
    if (!requiresConfirmation(response)) await runPlan(message);
  };

  const send = async (value: string) => {
    const v = value.trim();
    if (!v || busy) return;
    setText("");
    push({ role: "user", text: v });
    // 1. Pre-router: slash / settings commands run at once (no model, no plan card).
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
      push({ role: "assistant", kind: "executed", text: outcome.summary || describePreRoute(route, t), source: route.source === "slash" ? t("assistant.source_slash") : t("assistant.source_phrase"), confidence: route.confidence });
      return;
    }
    // 2. Planner (rules, then the model behind the engine gate).
    const context = uiContextFor(scope, folder, selectedIds, lastDocument);
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

  // Ctrl+Enter approves / Esc cancels the active plan card (settings.shortcuts; IME-safe).
  useEffect(() => {
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
        if (hasOpenLayer() || useUiStore.getState().paletteOpen) return;
        e.preventDefault();
        cancelPlan(id);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activePlan?.id, shortcuts?.execute_plan, shortcuts?.close]);

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
      navigate(SCREEN_ROUTES.models);
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
      if (execution) void runUiActions(execution.ui_actions);
      else if (job.state === "failed" && job.error) useUiStore.getState().toast(job.error, "danger");
    },
    [runUiActions],
  );

  const gate = pipeline?.gate;
  const gateSeconds = gate ? Math.ceil((gate.estimated_wait_ms ?? 0) / 1000) : 0;

  return (
    <div className="flex h-full flex-col" data-testid="page-assistant">
      <div className="flex flex-wrap items-center gap-3 border-b border-border px-4 py-2">
        <h1 className="text-base font-semibold">{t("assistant.title")}</h1>
        <Button variant="ghost" size="sm" onClick={() => setMessages([])}>
          {t("assistant.new_chat")}
        </Button>
        <div className="ml-auto flex items-center gap-3 text-xs text-fg-muted">
          <label className="flex items-center gap-1" title={t("answer.follow_up_hint")}>
            <Switch checked={followUp} onCheckedChange={setFollowUp} aria-label={t("answer.follow_up")} data-testid="assistant-follow-up" />
            {t("answer.follow_up")}
          </label>
          <span>{t("assistant.scope.label")}:</span>
          <Select value={scope} onValueChange={setScope}>
            <SelectTrigger className="h-8 w-52" aria-label={t("assistant.scope.label")} data-testid="assistant-scope">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">{t("assistant.scope.all")}</SelectItem>
              <SelectItem value="folder" disabled={!folder} data-testid="assistant-scope-folder">
                {folder ? t("assistant.scope.folder_named", { folder }) : t("assistant.scope.folder")}
              </SelectItem>
              <SelectItem value="selection" disabled={selectedIds.length === 0} data-testid="assistant-scope-selection">
                {t("assistant.scope.selection_count", { count: selectedIds.length })}
              </SelectItem>
              <SelectItem value="knowledge">{t("assistant.scope.knowledge")}</SelectItem>
              {(collections ?? []).map((c) => (
                <SelectItem key={c.id} value={`col:${c.id}`}>
                  {t("assistant.scope.collection", { name: c.name })}
                </SelectItem>
              ))}
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
          </div>
        )}
        <div className="mx-auto flex max-w-3xl flex-col gap-3">
          {messages.map((m) =>
            m.role === "user" ? (
              <div key={m.id} className="flex items-start gap-2 self-end">
                <div className="rounded-lg bg-accent px-3 py-2 text-sm text-accent-fg selectable">{m.text}</div>
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
                  />
                )}
                {m.kind === "execution" && <ExecutionCard jobId={m.jobId} plan={m.plan} response={m.response} onFinished={onJobFinished} />}
                {m.kind === "result" && <ResultCard execution={m.execution} plan={m.plan} response={m.response} />}
                {m.kind === "clarify" && (
                  <ClarifyCard
                    question={clarificationQuestion(m.response.result.plan) || t("planner.clarify.fallback")}
                    confidence={m.response.result.plan.confidence}
                    noModel={m.response.result.source === "none" && m.response.result.issues.some((i) => i.code === "no_model")}
                    modelId={visionModel}
                    loading={loadingModel}
                    onLoadModel={() => void loadModel()}
                    onRephrase={() => focusInput(m.text)}
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
              placeholder={t("assistant.placeholder")}
              aria-label={t("assistant.placeholder")}
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
