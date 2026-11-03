import { Bot, HelpCircle, Send, Sparkles, User } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/input";
import { describePreRoute, executePreRoute } from "@/lib/commands";
import { api } from "@/lib/ipc";
import { isComposing } from "@/lib/shortcuts";
import type { DateRange, PreRouteResult } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

type Message =
  | { id: number; role: "user"; text: string }
  | { id: number; role: "assistant"; kind: "executed"; text: string; route: PreRouteResult }
  | { id: number; role: "assistant"; kind: "pending"; text: string; date: DateRange | null }
  | { id: number; role: "assistant"; kind: "help" };

type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;
type NewMessage = DistributiveOmit<Message, "id">;

let seq = 1;

export function AssistantPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const pending = useUiStore((s) => s.pendingAssistantInput);
  const setPending = useUiStore((s) => s.setPendingAssistantInput);
  const [messages, setMessages] = useState<Message[]>([]);
  const [text, setText] = useState("");
  const [scope, setScope] = useState<"all" | "folder" | "selection">("all");
  const listRef = useRef<HTMLDivElement>(null);
  const consumed = useRef<string | null>(null);
  const examples = t("assistant.examples", { returnObjects: true }) as string[];

  const push = (m: NewMessage) => setMessages((prev) => [...prev, { ...m, id: seq++ } as Message]);

  const send = async (value: string) => {
    const v = value.trim();
    if (!v) return;
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
    const date = await api.prerouterResolveDate(v);
    push({ role: "assistant", kind: "pending", text: v, date });
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
  }, [messages]);

  return (
    <div className="flex h-full flex-col" data-testid="page-assistant">
      <div className="flex items-center gap-3 border-b border-border px-4 py-2">
        <h1 className="text-base font-semibold">{t("assistant.title")}</h1>
        <Button variant="ghost" size="sm" onClick={() => setMessages([])}>
          {t("assistant.new_chat")}
        </Button>
        <div className="ml-auto flex items-center gap-1 text-xs text-fg-muted" role="radiogroup" aria-label={t("assistant.scope.label")}>
          <span>{t("assistant.scope.label")}:</span>
          {(["all", "folder", "selection"] as const).map((s) => (
            <button
              key={s}
              type="button"
              role="radio"
              aria-checked={scope === s}
              onClick={() => setScope(s)}
              className={cn("rounded-full px-2 py-0.5 cursor-pointer", scope === s ? "bg-accent-soft text-fg" : "hover:bg-bg-sunken")}
            >
              {t(`assistant.scope.${s}`)}
            </button>
          ))}
        </div>
      </div>

      <div ref={listRef} className="flex-1 overflow-y-auto p-4" data-testid="assistant-messages">
        {messages.length === 0 && (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-fg-muted">
            <Sparkles className="size-8 text-accent" aria-hidden="true" />
            <p>{t("assistant.empty")}</p>
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
                    <div className="flex flex-col gap-1">
                      <div className="flex items-center gap-2">
                        <Badge variant="info">{t("assistant.pending_title")}</Badge>
                      </div>
                      <p className="text-fg-muted">{t("assistant.pending_body")}</p>
                      {m.date && <p className="text-fg">{t("assistant.date_resolved", { from: m.date.from, to: m.date.to, label: m.date.label })}</p>}
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
            <Button type="submit" aria-label={t("assistant.send")} data-testid="assistant-send">
              <Send />
            </Button>
          </form>
        </div>
      </div>
    </div>
  );
}
