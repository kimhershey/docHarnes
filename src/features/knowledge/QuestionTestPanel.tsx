import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import type { SearchResponse } from "@/lib/types";

import { hitHref, Snippet } from "./hit";

/** "Which evidence does this question retrieve?" — search only, no model call (설정 > 참고 자료(고급)). */
export function QuestionTestPanel() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [question, setQuestion] = useState("");
  const [result, setResult] = useState<SearchResponse | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  const run = async (e?: React.FormEvent) => {
    e?.preventDefault();
    if (!question.trim()) return;
    setBusy(true);
    setError(null);
    try {
      setResult(await api.knowledgeSearch({ query: question, mode: "hybrid", limit: 10, per_document: 3 }));
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-2" data-testid="knowledge-test">
      <p className="text-xs text-fg-muted">{t("knowledge.test.hint")}</p>
      <form className="flex gap-2" onSubmit={run}>
        <Input value={question} onChange={(e) => setQuestion(e.target.value)} placeholder={t("knowledge.test.placeholder")} aria-label={t("knowledge.test.title")} data-testid="kb-test-input" />
        <Button type="submit" disabled={busy || !question.trim()} data-testid="kb-test-run">
          {t("knowledge.test.run")}
        </Button>
      </form>
      {busy && <Spinner />}
      {!!error && (
        <p className="text-sm text-danger" role="alert">
          {describeError(error, t)}
        </p>
      )}
      {result && (
        <div className="flex flex-col gap-2" data-testid="kb-test-result">
          <div className="flex flex-wrap gap-3 text-xs text-fg-subtle">
            <span>
              {t("knowledge.test.terms")}: {result.query_terms.join(", ") || "-"}
            </span>
            {result.expanded_terms.length > 0 && (
              <span>
                {t("knowledge.test.expanded")}: {result.expanded_terms.join(", ")}
              </span>
            )}
            <span>{t("knowledge.test.elapsed", { ms: result.elapsed_ms })}</span>
          </div>
          {result.hits.length === 0 && <p className="text-sm text-fg-muted">{t("knowledge.test.none")}</p>}
          <ol className="flex flex-col gap-1">
            {result.hits.map((h, i) => (
              <li key={h.chunk_id} className="rounded-md border border-border p-2 text-sm" data-testid="kb-test-hit">
                <div className="flex flex-wrap items-center gap-2 text-xs text-fg-subtle">
                  <span className="font-medium text-fg">[{i + 1}]</span>
                  <button type="button" className="text-fg hover:underline cursor-pointer" onClick={() => navigate(hitHref(h, question))}>
                    {h.title}
                  </button>
                  <span>{h.page_start === h.page_end ? t("answer.citation_page", { page: h.page_start }) : t("answer.citation_pages", { from: h.page_start, to: h.page_end })}</span>
                  {h.clause && <span>§ {h.clause}</span>}
                  <span className="ml-auto flex gap-1">
                    {h.channels.map((c) => (
                      <Badge key={c} variant="muted">
                        {t(`search.channel.${c}`)}
                      </Badge>
                    ))}
                    <span>
                      {t("knowledge.test.score")} {h.score.toFixed(3)}
                    </span>
                  </span>
                </div>
                <p className="mt-1">
                  <Snippet text={h.snippet} highlights={h.highlights} />
                </p>
              </li>
            ))}
          </ol>
        </div>
      )}
    </div>
  );
}
