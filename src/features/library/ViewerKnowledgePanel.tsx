import { MessageSquare, Send } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/input";
import { AnswerCard } from "@/features/knowledge/AnswerCard";
import { useAsk } from "@/features/knowledge/useAsk";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useAnswers } from "@/lib/queries";
import { isComposing } from "@/lib/shortcuts";
import type { AnswerRequest, Citation, DocumentView, SummaryMode } from "@/lib/types";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";

/**
 * Viewer Q&A tab (design §3.7.6 / §3.14.2–3): ask about this document (or the
 * whole knowledge base), explain a selection, generate summaries, browse
 * earlier answers for the document.
 */
export function ViewerKnowledgePanel({ doc, explainSelection, onExplained, onCitation }: { doc: DocumentView; explainSelection: string | null; onExplained: () => void; onCitation: (c: Citation) => void }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const knowledge = useSettingsStore((s) => s.settings?.knowledge);
  const [question, setQuestion] = useState("");
  const [scope, setScope] = useState<"doc" | "kb">("doc");
  const [summaryMode, setSummaryMode] = useState<SummaryMode>("key_points");
  const qa = useAsk();
  const summary = useAsk();
  const { data: history } = useAnswers(null, doc.id);

  const baseRequest = (): Partial<AnswerRequest> => ({
    scope: scope === "doc" ? { documents: [doc.id], latest_only: false } : { latest_only: knowledge?.latest_only ?? true },
    allow_general: knowledge?.allow_general_knowledge ?? false,
    top_k: knowledge?.top_k ?? 8,
    context_tokens: knowledge?.context_tokens ?? 6000,
    document_id: doc.id,
  });

  const ask = async (q: string, extra: Partial<AnswerRequest> = {}) => {
    const text = q.trim();
    if ((!text && !extra.selection) || qa.pending) return;
    setQuestion("");
    await qa.ask({ question: text, kind: "answer", ...baseRequest(), ...extra });
  };

  useEffect(() => {
    if (explainSelection) {
      if (qa.pending) qa.cancel();
      void qa.ask({ question: "", kind: "explain", selection: explainSelection, ...baseRequest(), scope: { latest_only: knowledge?.latest_only ?? true } });
      onExplained();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [explainSelection]);

  const summarize = () => void summary.ask({ question: "", kind: "summary", summary_mode: summaryMode, document_id: doc.id, scope: { documents: [doc.id], latest_only: false }, context_tokens: knowledge?.context_tokens ?? 6000 });

  const feedback = async (id: number, value: number | null) => {
    try {
      await api.knowledgeAnswerFeedback(id, value);
      toast(t("answer.feedback_saved"), "ok");
      if (qa.answer?.id === id) qa.setAnswer({ ...qa.answer, feedback: value });
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  return (
    <div className="flex flex-col gap-3 text-sm" data-testid="viewer-qa">
      <form
        className="flex flex-col gap-1"
        onSubmit={(e) => {
          e.preventDefault();
          void ask(question);
        }}
      >
        <div className="flex items-center gap-2">
          <MessageSquare className="size-4 text-accent" aria-hidden="true" />
          <span className="font-medium">{t("viewer.qa.title")}</span>
          <Select value={scope} onValueChange={(v) => setScope(v as "doc" | "kb")}>
            <SelectTrigger className="ml-auto h-7 w-36 text-xs" aria-label={t("assistant.scope.label")} data-testid="viewer-qa-scope">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="doc">{t("viewer.qa.scope_doc")}</SelectItem>
              <SelectItem value="kb">{t("viewer.qa.scope_kb")}</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="flex items-end gap-1">
          <Textarea
            rows={2}
            value={question}
            onChange={(e) => setQuestion(e.target.value)}
            placeholder={t("viewer.qa.placeholder")}
            aria-label={t("viewer.qa.title")}
            className="resize-none text-sm"
            data-testid="viewer-qa-input"
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                if (isComposing(e)) return;
                e.preventDefault();
                void ask(question);
              }
            }}
          />
          <Button type="submit" size="icon-sm" disabled={qa.pending || !question.trim()} aria-label={t("viewer.qa.ask")} data-testid="viewer-qa-ask">
            <Send />
          </Button>
        </div>
      </form>
      {(qa.pending || !!qa.answer || !!qa.error) && (
        <AnswerCard
          answer={qa.answer}
          pending={qa.pending}
          streamText={qa.streamText}
          error={qa.error}
          onCitation={onCitation}
          onAskGeneral={qa.lastRequest ? () => void qa.ask({ ...qa.lastRequest!, allow_general: true }) : undefined}
          onFeedback={(id, v) => void feedback(id, v)}
          onCancel={qa.pending ? qa.cancel : undefined}
          compact
        />
      )}

      <div className="flex flex-col gap-1 border-t border-border pt-2">
        <div className="flex items-center gap-2">
          <span className="font-medium">{t("viewer.summary.title")}</span>
          <Select value={summaryMode} onValueChange={(v) => setSummaryMode(v as SummaryMode)}>
            <SelectTrigger className="h-7 w-28 text-xs" aria-label={t("viewer.summary.title")} data-testid="viewer-summary-mode">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {(["one_line", "key_points", "one_page"] as const).map((m) => (
                <SelectItem key={m} value={m}>
                  {t(`viewer.summary.modes.${m}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button size="sm" variant="outline" className="ml-auto" onClick={summarize} disabled={summary.pending} data-testid="viewer-summary-run">
            {summary.pending ? t("viewer.summary.running") : t("viewer.summary.run")}
          </Button>
        </div>
        {summary.pending || !!summary.answer || !!summary.error ? (
          <AnswerCard answer={summary.answer} pending={summary.pending} streamText={summary.streamText} error={summary.error} onCitation={onCitation} onCancel={summary.pending ? summary.cancel : undefined} compact />
        ) : (
          (() => {
            const last = history?.find((a) => a.kind === "summary");
            return last ? <AnswerCard answer={last} onCitation={onCitation} compact /> : <p className="text-xs text-fg-muted">{t("viewer.summary.none")}</p>;
          })()
        )}
      </div>

      <div className="flex flex-col gap-2 border-t border-border pt-2">
        <span className="font-medium">{t("viewer.qa.history")}</span>
        {history && history.filter((a) => a.kind !== "summary").length === 0 && <p className="text-xs text-fg-muted">{t("viewer.qa.none")}</p>}
        {history
          ?.filter((a) => a.kind !== "summary" && a.id !== qa.answer?.id)
          .slice(0, 5)
          .map((a) => <AnswerCard key={a.id} answer={a} onCitation={onCitation} onFeedback={(id, v) => void feedback(id, v)} compact />)}
      </div>
    </div>
  );
}
