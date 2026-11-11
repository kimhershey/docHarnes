import { Copy, Square, ThumbsDown, ThumbsUp, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import type { Answer, AnswerSentence, AnswerTruncation, Citation, SavedAnswer } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

type AnyAnswer = Answer | SavedAnswer;

export function citationLabel(c: Citation, t: (k: string, o?: Record<string, unknown>) => string): string {
  const page = c.page_start === c.page_end ? t("answer.citation_page", { page: c.page_start }) : t("answer.citation_pages", { from: c.page_start, to: c.page_end });
  return [c.title, page, c.clause ?? ""].filter(Boolean).join(" · ");
}

/**
 * Groups sentences back into the paragraphs of the original text (the backend
 * flattens lines when it splits sentences): a sentence starts a new paragraph
 * when the original has a line break before it.
 */
export function paragraphsOf(text: string, sentences: AnswerSentence[]): AnswerSentence[][] {
  const paragraphs: AnswerSentence[][] = [];
  let cursor = 0;
  for (const s of sentences) {
    const idx = text.indexOf(s.text, cursor);
    const between = idx >= 0 ? text.slice(cursor, idx) : "";
    const newParagraph = paragraphs.length === 0 || between.includes("\n") || /^[-*•]\s/.test(s.text);
    if (newParagraph) paragraphs.push([s]);
    else paragraphs[paragraphs.length - 1]!.push(s);
    if (idx >= 0) cursor = idx + s.text.length;
  }
  return paragraphs;
}

/**
 * The note under an answer whose request text or reply was cut (`Answer.truncated`, KNO-3): one sentence per
 * side that was cut, both for `both`.
 */
export function truncationNote(truncated: AnswerTruncation | null | undefined, t: (k: string) => string): string | null {
  if (!truncated) return null;
  const parts: string[] = [];
  if (truncated === "input" || truncated === "both") parts.push(t("answer.truncated_input"));
  if (truncated === "output" || truncated === "both") parts.push(t("answer.truncated_output"));
  return parts.length > 0 ? parts.join(" ") : null;
}

/** Splits a sentence into text and `[n]` markers rendered as clickable chips. */
function SentenceView({ sentence, sources, onCitation }: { sentence: AnswerSentence; sources: Citation[]; onCitation?: (c: Citation) => void }) {
  const { t } = useTranslation();
  const parts = sentence.text.split(/(\[[\d,\s]+\])/g);
  return (
    <span className="selectable">
      {parts.map((p, i) => {
        const m = p.match(/^\[([\d,\s]+)\]$/);
        if (!m) return <span key={i}>{p}</span>;
        const nums = (m[1] ?? "").split(",").map((n) => Number(n.trim())).filter((n) => n > 0);
        return nums.map((n) => {
          const c = sources.find((s) => s.index === n);
          if (!c) return <span key={`${i}-${n}`}>[{n}]</span>;
          const label = citationLabel(c, t);
          return (
            <button
              key={`${i}-${n}`}
              type="button"
              className="mx-0.5 inline-flex h-5 min-w-5 items-center justify-center rounded-full bg-accent-soft px-1.5 align-text-bottom text-[0.6875rem] font-medium text-accent hover:bg-accent hover:text-accent-fg cursor-pointer"
              title={label}
              aria-label={t("answer.citation_aria", { index: n, label })}
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
  onCancel,
  compact,
  chat,
  noEvidenceText,
}: {
  answer: AnyAnswer | null;
  streamText?: string;
  pending?: boolean;
  error?: unknown;
  onCitation?: (c: Citation) => void;
  onAskGeneral?: () => void;
  onFeedback?: (id: number, feedback: number | null) => void;
  onDelete?: (id: number) => void;
  onCancel?: () => void;
  compact?: boolean;
  /**
   * The answer is a small-talk reply (the request went out with `style: "chat"`): it has no sources because
   * nothing was looked up, which is not "answered from the text you sent" — no 검색 없이 답변 badge.
   */
  chat?: boolean;
  /** What the "no evidence" note says instead of the general sentence (a scope that matched no document). */
  noEvidenceText?: string;
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
  const state = pending ? "pending" : answer ? (answer.no_evidence && !answer.used_general_knowledge ? "no_evidence" : "done") : error ? "error" : "idle";
  const paragraphs = answer ? paragraphsOf(answer.text, answer.sentences) : [];
  const truncated = answer && "truncated" in answer ? truncationNote(answer.truncated, t) : null;
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border bg-bg-elevated p-3 text-sm" data-testid="answer-card" data-answer-state={state}>
      {answer && (
        <div className="flex flex-wrap items-center gap-2 text-xs text-fg-subtle">
          <Badge variant="info">{t(`answer.kinds.${answer.kind}`)}</Badge>
          {answer.used_general_knowledge && (
            <Badge variant="warn" data-testid="answer-general-badge">
              {t("answer.general_badge")}
            </Badge>
          )}
          {!chat && !answer.no_evidence && !answer.used_general_knowledge && answer.sources.length === 0 && (
            <Badge variant="outline" data-testid="answer-text-only" title={t("answer.text_only_hint")}>
              {t("answer.text_only")}
            </Badge>
          )}
          {answer.model && "elapsed_ms" in answer && <span>{t("answer.meta", { model: answer.model, ms: answer.elapsed_ms })}</span>}
          {!compact && answer.question && <span className="truncate">· {answer.question}</span>}
        </div>
      )}
      {pending && (
        <div className="flex items-start gap-2" role="status">
          <Spinner className="mt-0.5" />
          <p className="min-w-0 flex-1 whitespace-pre-wrap break-words selectable" data-testid="answer-stream">
            {streamText || t("answer.thinking")}
          </p>
          {onCancel && (
            <Button size="sm" variant="ghost" onClick={onCancel} aria-label={t("answer.stop")} data-testid="answer-stop">
              <Square />
              {t("answer.stop")}
            </Button>
          )}
        </div>
      )}
      {!pending && !!error && (
        <p className="text-danger" role="alert" data-testid="answer-error">
          {describeError(error, t)}
        </p>
      )}
      {state === "no_evidence" && (
        <div className="flex flex-wrap items-center gap-2 rounded-md bg-warn-soft px-2 py-1.5 text-warn" data-testid="answer-no-evidence">
          <span>{noEvidenceText ?? t("answer.no_evidence")}</span>
          {onAskGeneral && (
            <Button size="sm" variant="outline" onClick={onAskGeneral} data-testid="answer-ask-general">
              {t("answer.ask_general")}
            </Button>
          )}
        </div>
      )}
      {state === "done" && answer && (
        <div className="flex flex-col gap-1 leading-relaxed" data-testid="answer-text">
          {paragraphs.length > 0 ? (
            paragraphs.map((para, i) => (
              <p key={i} className={cn(/^[-*•]\s/.test(para[0]?.text ?? "") && "pl-3")}>
                {para.map((s, j) => (
                  <SentenceView key={j} sentence={s} sources={answer.sources} onCitation={onCitation} />
                ))}
              </p>
            ))
          ) : (
            <p className="whitespace-pre-wrap selectable">{answer.text}</p>
          )}
        </div>
      )}
      {state === "done" && answer && truncated && (
        <p className="rounded-md bg-warn-soft px-2 py-1 text-xs text-warn" role="note" data-testid="answer-truncated" data-truncated={"truncated" in answer ? answer.truncated : undefined}>
          {truncated}
        </p>
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
                  aria-label={t("answer.citation_aria", { index: c.index, label: citationLabel(c, t) })}
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
              <Button size="sm" variant={answer.feedback === 1 ? "default" : "ghost"} aria-pressed={answer.feedback === 1} onClick={() => onFeedback(id, answer.feedback === 1 ? null : 1)} aria-label={t("answer.feedback_helpful")} data-testid="answer-helpful">
                <ThumbsUp />
                {t("answer.feedback_helpful")}
              </Button>
              <Button size="sm" variant={answer.feedback === -1 ? "default" : "ghost"} aria-pressed={answer.feedback === -1} onClick={() => onFeedback(id, answer.feedback === -1 ? null : -1)} aria-label={t("answer.feedback_wrong")} data-testid="answer-wrong">
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
