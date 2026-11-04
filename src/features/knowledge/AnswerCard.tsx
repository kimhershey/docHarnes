import { Copy, ThumbsDown, ThumbsUp, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import type { Answer, AnswerSentence, Citation, SavedAnswer } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

type AnyAnswer = Answer | SavedAnswer;

export function citationLabel(c: Citation, t: (k: string, o?: Record<string, unknown>) => string): string {
  const page = c.page_start === c.page_end ? t("answer.citation_page", { page: c.page_start }) : t("answer.citation_pages", { from: c.page_start, to: c.page_end });
  return [c.title, page, c.clause ?? ""].filter(Boolean).join(" · ");
}

/** Splits a sentence into text and `[n]` markers rendered as clickable chips. */
function SentenceView({ sentence, sources, onCitation }: { sentence: AnswerSentence; sources: Citation[]; onCitation?: (c: Citation) => void }) {
  const { t } = useTranslation();
  const parts = sentence.text.split(/(\[[\d,\s]+\])/g);
  const bullet = /^[-*•]\s/.test(sentence.text);
  const cls = cn("selectable", bullet && "block pl-3");
  return (
    <span className={cls}>
      {parts.map((p, i) => {
        const m = p.match(/^\[([\d,\s]+)\]$/);
        if (!m) return <span key={i}>{p}</span>;
        const nums = (m[1] ?? "").split(",").map((n) => Number(n.trim())).filter((n) => n > 0);
        return nums.map((n) => {
          const c = sources.find((s) => s.index === n);
          if (!c) return <span key={`${i}-${n}`}>[{n}]</span>;
          return (
            <button
              key={`${i}-${n}`}
              type="button"
              className="mx-0.5 inline-flex h-5 min-w-5 items-center justify-center rounded-full bg-accent-soft px-1.5 align-text-bottom text-[11px] font-medium text-accent hover:bg-accent hover:text-accent-fg cursor-pointer"
              title={citationLabel(c, t)}
              onClick={() => onCitation?.(c)}
              data-testid="citation-chip"
              data-citation={n}
            >
              {n}
            </button>
          );
        });
      })}{" "}
    </span>
  );
}

export function AnswerCard({
  answer,
  streamText,
  pending,
  error,
  onCitation,
  onAskGeneral,
  onFeedback,
  onDelete,
  compact,
}: {
  answer: AnyAnswer | null;
  streamText?: string;
  pending?: boolean;
  error?: unknown;
  onCitation?: (c: Citation) => void;
  onAskGeneral?: () => void;
  onFeedback?: (id: number, feedback: number | null) => void;
  onDelete?: (id: number) => void;
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const copy = async () => {
    if (!answer) return;
    try {
      await navigator.clipboard.writeText(answer.text);
      toast(t("answer.copied"), "ok");
    } catch {
      /* clipboard unavailable (headless) */
    }
  };
  const id = answer?.id ?? null;
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border bg-bg-elevated p-3 text-sm" data-testid="answer-card" data-answer-state={pending ? "pending" : answer ? (answer.no_evidence && !answer.used_general_knowledge ? "no_evidence" : "done") : error ? "error" : "idle"}>
      {answer && (
        <div className="flex flex-wrap items-center gap-2 text-xs text-fg-subtle">
          <Badge variant="info">{t(`answer.kinds.${answer.kind}`)}</Badge>
          {answer.used_general_knowledge && <Badge variant="warn">{t("answer.general_badge")}</Badge>}
          {answer.model && "elapsed_ms" in answer && <span>{t("answer.meta", { model: answer.model, ms: answer.elapsed_ms })}</span>}
          {!compact && answer.question && <span className="truncate">· {answer.question}</span>}
        </div>
      )}
      {pending && (
        <div className="flex items-start gap-2" role="status">
          <Spinner className="mt-0.5" />
          <p className="whitespace-pre-wrap break-words selectable" data-testid="answer-stream">
            {streamText || t("answer.thinking")}
          </p>
        </div>
      )}
      {!pending && !!error && (
        <p className="text-danger" role="alert" data-testid="answer-error">
          {describeError(error, t)}
        </p>
      )}
      {!pending && answer && answer.no_evidence && !answer.used_general_knowledge && (
        <div className="flex flex-wrap items-center gap-2 rounded-md bg-warn-soft px-2 py-1.5 text-warn" data-testid="answer-no-evidence">
          <span>{t("answer.no_evidence")}</span>
          {onAskGeneral && (
            <Button size="sm" variant="outline" onClick={onAskGeneral} data-testid="answer-ask-general">
              {t("answer.ask_general")}
            </Button>
          )}
        </div>
      )}
      {!pending && answer && !(answer.no_evidence && !answer.used_general_knowledge) && (
        <div className="leading-relaxed" data-testid="answer-text">
          {answer.sentences.length > 0 ? (
            answer.sentences.map((s, i) => <SentenceView key={i} sentence={s} sources={answer.sources} onCitation={onCitation} />)
          ) : (
            <p className="whitespace-pre-wrap selectable">{answer.text}</p>
          )}
        </div>
      )}
      {!pending && answer && "conflicts" in answer && answer.conflicts && (
        <div className="rounded-md border border-warn/40 bg-warn-soft px-2 py-1.5 text-xs" data-testid="answer-conflicts">
          <span className="font-medium text-warn">{t("answer.conflicts")}: </span>
          <span className="selectable">{answer.conflicts}</span>
        </div>
      )}
      {!pending && answer && answer.sources.length > 0 && (
        <details className="text-xs" open={!compact}>
          <summary className="cursor-pointer text-fg-subtle">
            {t("answer.sources")} ({answer.sources.length})
          </summary>
          <ol className="mt-1 flex flex-col gap-1">
            {answer.sources.map((c) => (
              <li key={c.index} className="flex items-start gap-2">
                <button
                  type="button"
                  className="shrink-0 rounded bg-accent-soft px-1.5 py-0.5 font-medium text-accent hover:bg-accent hover:text-accent-fg cursor-pointer"
                  onClick={() => onCitation?.(c)}
                  data-testid="answer-source"
                  title={t("answer.open_source")}
                >
                  [{c.index}]
                </button>
                <div className="min-w-0">
                  <div className="truncate font-medium text-fg">{citationLabel(c, t)}</div>
                  <div className="truncate text-fg-muted selectable">{c.excerpt}</div>
                </div>
              </li>
            ))}
          </ol>
        </details>
      )}
      {!pending && answer && (onFeedback || onDelete) && (
        <div className="flex items-center gap-1">
          {onFeedback && id != null && (
            <>
              <Button size="sm" variant={answer.feedback === 1 ? "default" : "ghost"} onClick={() => onFeedback(id, answer.feedback === 1 ? null : 1)} aria-label={t("answer.feedback_helpful")} data-testid="answer-helpful">
                <ThumbsUp />
                {t("answer.feedback_helpful")}
              </Button>
              <Button size="sm" variant={answer.feedback === -1 ? "default" : "ghost"} onClick={() => onFeedback(id, answer.feedback === -1 ? null : -1)} aria-label={t("answer.feedback_wrong")} data-testid="answer-wrong">
                <ThumbsDown />
                {t("answer.feedback_wrong")}
              </Button>
            </>
          )}
          <Button size="sm" variant="ghost" onClick={() => void copy()} aria-label={t("answer.copy")}>
            <Copy />
          </Button>
          {onDelete && id != null && (
            <Button size="sm" variant="ghost" className="ml-auto text-danger" onClick={() => onDelete(id)} aria-label={t("answer.delete")} data-testid="answer-delete">
              <Trash2 />
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
