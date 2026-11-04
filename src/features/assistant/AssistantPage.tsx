import { Bot, HelpCircle, Send, Sparkles, User } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { AnswerCard } from "@/features/knowledge/AnswerCard";
import { openCitation } from "@/features/knowledge/AnswersPanel";
import { scopeFor } from "@/features/search/SearchPage";
import { describePreRoute, executePreRoute } from "@/lib/commands";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useCollections } from "@/lib/queries";
import { isComposing } from "@/lib/shortcuts";
import type { Answer, AnswerRequest, DateRange, PreRouteResult } from "@/lib/types";
import { newRequestId, useKnowledgeStore } from "@/stores/knowledge";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

type Message =
  | { id: number; role: "user"; text: string }
  | { id: number; role: "assistant"; kind: "executed"; text: string; route: PreRouteResult }
  | { id: number; role: "assistant"; kind: "pending"; text: string; date: DateRange | null }
  | { id: number; role: "assistant"; kind: "answer"; requestId: string; request: AnswerRequest; answer: Answer | null; error: unknown; pending: boolean }
  | { id: number; role: "assistant"; kind: "help" };

type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;
type NewMessage = DistributiveOmit<Message, "id">;

let seq = 1;

/** Questions go to the knowledge base; imperative work items (totals, organising…) wait for the P5 planner. */
export function looksLikeQuestion(text: string): boolean {
  const t = text.trim();
  return /[?？]$/.test(t) || /(무엇|뭐|어떻게|어디|언제|누가|얼마|왜|인가요|인지|나요|까요|가요|어떤|가능해|되나|있나|맞나|what|how|when|where|why|which|who|is there|can i)/i.test(t);
}

export function looksLikeCommand(text: string): boolean {
  const t = text.trim();
  return /(해\s?줘|해주세요|하세요|해라|하자|정리|추출|분류|내보내|변환|이름\s?바꿔|옮겨|삭제|지워|합계|집계|통계|목록|만들어)/.test(t) || /\b(export|organize|organise|rename|move|delete|extract|classify|summar(y|ize) all|total|sum)\b/i.test(t);
}

/**
 * Assistant (design §3.7.6, P3 subset): slash / phrase commands run through the
 * pre-router; everything else is a question answered from the knowledge base
 * with per-sentence citations. The planner (P5) will route the remaining ops.
 */
export function AssistantPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const pending = useUiStore((s) => s.pendingAssistantInput);
  const setPending = useUiStore((s) => s.setPendingAssistantInput);
  const workspace = useWorkspaceStore((s) => s.current);
  const knowledge = useSettingsStore((s) => s.settings?.knowledge);
  const streams = useKnowledgeStore((s) => s.streams);
  const clearStream = useKnowledgeStore((s) => s.clearStream);
  const { data: collections } = useCollections(!!workspace);
  const [messages, setMessages] = useState<Message[]>([]);
  const [text, setText] = useState("");
  const [scope, setScope] = useState<string>("all");
  const [followUp, setFollowUp] = useState(false);
  const [cancelling, setCancelling] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const consumed = useRef<string | null>(null);
  const examples = t("assistant.examples", { returnObjects: true }) as string[];

  const push = (m: NewMessage) => {
    const id = seq++;
    setMessages((prev) => [...prev, { ...m, id } as Message]);
    return id;
  };
  const patch = (id: number, f: (m: Message) => Message) => setMessages((prev) => prev.map((m) => (m.id === id ? f(m) : m)));

  const anyPending = messages.some((m) => m.role === "assistant" && m.kind === "answer" && m.pending);

  const askKnowledge = async (question: string, extra: Partial<AnswerRequest> = {}) => {
    // Only an explicit follow-up keeps the previous evidence; otherwise every question is retrieved afresh
    // so unrelated questions can still end in "no evidence".
    const previous = followUp
      ? [...messages].reverse().find((m): m is Extract<Message, { kind: "answer" }> => m.role === "assistant" && m.kind === "answer" && !!m.answer && !m.answer.no_evidence)
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

  const send = async (value: string) => {
    const v = value.trim();
    if (!v || anyPending) return;
    setText("");
    push({ role: "user", text: v });
    const route = await api.prerouterRoute(v);
    if (route.matched && route.op === "help") {
      push({ role: "assistant", kind: "help" });
      return;
    }
    if (route.matched) {
      const outcome = await executePreRoute(route, navigate, t);
      push({ role: "assistant", kind: "executed", text: outcome.summary || describePreRoute(route, t), route });
      return;
    }
    if (!looksLikeQuestion(v)) {
      // Work items (and date-scoped requests) belong to the planner (P5); say so instead of a misleading "no evidence".
      const date = await api.prerouterResolveDate(v);
      if (date || looksLikeCommand(v)) {
        push({ role: "assistant", kind: "pending", text: v, date });
        return;
      }
    }
    await askKnowledge(v);
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

  const feedback = async (messageId: number, id: number, value: number | null) => {
    try {
      await api.knowledgeAnswerFeedback(id, value);
      patch(messageId, (m) => (m.role === "assistant" && m.kind === "answer" && m.answer ? { ...m, answer: { ...m.answer, feedback: value } } : m));
    } catch (e) {
      useUiStore.getState().toast(describeError(e, t), "danger");
    }
  };

  return (
    <div className="flex h-full flex-col" data-testid="page-assistant">
      <div className="flex items-center gap-3 border-b border-border px-4 py-2">
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
            <SelectTrigger className="h-8 w-44" aria-label={t("assistant.scope.label")} data-testid="assistant-scope">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">{t("assistant.scope.all")}</SelectItem>
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
                <Bot className="mt-2 size-4 text-accent" aria-hidden="true" />
                {m.kind === "answer" ? (
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
                ) : (
                  <div className="w-full rounded-lg border border-border bg-bg-elevated p-3 text-sm" data-testid={`assistant-card-${m.kind}`}>
                    {m.kind === "executed" && (
                      <div className="flex flex-col gap-1">
                        <div className="flex items-center gap-2">
                          <Badge variant="ok">{t("assistant.executed")}</Badge>
                          <span className="text-xs text-fg-subtle">
                            {m.route.source === "slash" ? t("assistant.source_slash") : t("assistant.source_phrase")} · {t("assistant.confidence", { value: m.route.confidence.toFixed(2) })}
                          </span>
                        </div>
                        <p>{m.text}</p>
                      </div>
                    )}
                    {m.kind === "pending" && (
                      <div className="flex flex-col gap-1" data-testid="assistant-pending">
                        <div className="flex items-center gap-2">
                          <Badge variant="info">{t("assistant.pending_title")}</Badge>
                        </div>
                        <p className="text-fg-muted">{t("assistant.pending_body")}</p>
                        {m.date && <p className="text-fg">{t("assistant.date_resolved", { from: m.date.from, to: m.date.to, label: m.date.label })}</p>}
                        <p className="text-xs text-fg-subtle">{t("assistant.pending_hint")}</p>
                      </div>
                    )}
                    {m.kind === "help" && (
                      <div className="flex flex-col gap-1">
                        <div className="flex items-center gap-2 font-medium">
                          <HelpCircle className="size-4" aria-hidden="true" />
                          {t("assistant.help_title")}
                        </div>
                        <p className="text-fg-muted">{t("assistant.help_body")}</p>
                      </div>
                    )}
                  </div>
                )}
              </div>
            ),
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
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder={t("assistant.placeholder")}
              aria-label={t("assistant.placeholder")}
              rows={2}
              className="resize-none"
              data-testid="assistant-input"
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  if (isComposing(e)) return; // Korean IME: let the composition finish
                  e.preventDefault();
                  void send(text);
                }
              }}
            />
            <Button type="submit" aria-label={t("assistant.send")} disabled={anyPending} data-testid="assistant-send">
              <Send />
            </Button>
          </form>
        </div>
      </div>
    </div>
  );
}
