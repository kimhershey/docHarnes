import { ExternalLink, MessageSquareText } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import type { SearchHit, SearchMode, SearchResponse } from "@/lib/types";
import { cn } from "@/lib/utils";

/** Link to the viewer at the hit's page with the query highlighted. */
export function hitHref(hit: Pick<SearchHit, "document_id" | "page_start" | "chunk_id">, query: string): string {
  const params = new URLSearchParams({ page: String(hit.page_start), q: query, chunk: String(hit.chunk_id) });
  return `/library/${hit.document_id}?${params.toString()}`;
}

/** A snippet with its highlighted ranges (code-point offsets, as the core reports them). */
export function Snippet({ text, highlights }: { text: string; highlights: [number, number][] }) {
  const chars = Array.from(text);
  const parts: { s: string; hit: boolean }[] = [];
  let pos = 0;
  for (const [a, b] of highlights) {
    if (a > pos) parts.push({ s: chars.slice(pos, a).join(""), hit: false });
    parts.push({ s: chars.slice(a, b).join(""), hit: true });
    pos = b;
  }
  if (pos < chars.length) parts.push({ s: chars.slice(pos).join(""), hit: false });
  return (
    <span className="selectable">
      {parts.map((p, i) =>
        p.hit ? (
          <mark key={i} className="rounded bg-warn-soft px-0.5 text-fg">
            {p.s}
          </mark>
        ) : (
          <span key={i}>{p.s}</span>
        ),
      )}
    </span>
  );
}

/**
 * "근거만 찾기" result (0.5.0, replaces the search screen): the hits of a knowledge search without a
 * model call — title → viewer page, 업무 문서 / 참고 자료, page · clause · heading, the channels that
 * found the chunk and the snippet. "이 결과로 질문" sends the same text as a question.
 */
export function EvidenceCard({
  query,
  mode,
  response,
  error,
  pending,
  embeddingReady,
  onAsk,
}: {
  query: string;
  mode: SearchMode;
  response: SearchResponse | null;
  error: unknown;
  pending: boolean;
  embeddingReady: boolean;
  onAsk?: (question: string) => void;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const pageLabel = (hit: SearchHit) => (hit.page_start === hit.page_end ? t("answer.citation_page", { page: hit.page_start }) : t("answer.citation_pages", { from: hit.page_start, to: hit.page_end }));
  return (
    <div className="w-full rounded-lg border border-border bg-bg-elevated p-3 text-sm" role="group" aria-label={t("search.title")} data-testid="assistant-card-evidence" data-pending={pending ? "true" : "false"}>
      <div className="flex flex-wrap items-center gap-2 text-xs text-fg-subtle" data-testid="evidence-meta">
        <Badge variant="info">{t("search.title")}</Badge>
        <span>{t(`search.mode.${mode}`)}</span>
        {pending && (
          <span className="flex items-center gap-1" role="status">
            <Spinner />
            {t("search.searching")}
          </span>
        )}
        {response && <span>{t("search.results", { count: response.hits.length, ms: response.elapsed_ms })}</span>}
        {response && response.query_terms.length > 0 && (
          <span>
            {t("search.terms")}: {response.query_terms.join(", ")}
          </span>
        )}
        {response && response.expanded_terms.length > 0 && (
          <span>
            {t("search.expanded")}: {response.expanded_terms.join(", ")}
          </span>
        )}
        {response?.used_like_fallback && <span className="text-warn">{t("search.like_fallback")}</span>}
      </div>
      {mode === "semantic" && !embeddingReady && (
        <p className="mt-1 text-xs text-warn" role="status">
          {t("search.semantic_unavailable")}
        </p>
      )}
      {mode === "hybrid" && !embeddingReady && response && (
        <p className="mt-1 text-xs text-fg-subtle" role="status" data-testid="evidence-keyword-only">
          {t("search.keyword_only")}
        </p>
      )}
      {error != null && (
        <p className="mt-2 text-danger" role="alert">
          {describeError(error, t)}
        </p>
      )}
      {response && response.hits.length === 0 && (
        <p className="mt-2 text-fg-muted" data-testid="evidence-no-results">
          {t("search.no_results")}
        </p>
      )}
      {response && response.hits.length > 0 && (
        <ul className="mt-2 flex flex-col gap-2">
          {response.hits.map((hit) => (
            <li key={hit.chunk_id} className="rounded-md border border-border bg-bg p-2" data-testid="evidence-hit" data-doc-id={hit.document_id} data-role={hit.role}>
              <div className="flex flex-wrap items-center gap-2 text-xs text-fg-subtle">
                <button type="button" className="text-left text-sm font-medium text-fg hover:underline cursor-pointer" onClick={() => navigate(hitHref(hit, query))} data-testid="evidence-hit-title">
                  {hit.title}
                </button>
                <Badge variant={hit.role === "knowledge" ? "info" : "outline"}>{hit.role === "knowledge" ? t("library.role_knowledge") : t("library.role_work")}</Badge>
                <span>{pageLabel(hit)}</span>
                {hit.clause && <span>§ {hit.clause}</span>}
                {hit.heading_path && <span className="truncate">{hit.heading_path}</span>}
                <span className="ml-auto flex items-center gap-1">
                  {hit.channels.map((c) => (
                    <Badge key={c} variant={c === "vector" ? "ok" : "muted"} className={cn(c === "vector" && "font-medium")}>
                      {t(`search.channel.${c}`)}
                    </Badge>
                  ))}
                </span>
              </div>
              <p className="mt-1 leading-relaxed">
                <Snippet text={hit.snippet} highlights={hit.highlights} />
              </p>
              <div className="mt-1 flex gap-1">
                <Button size="sm" variant="ghost" onClick={() => navigate(hitHref(hit, query))} data-testid="evidence-open">
                  <ExternalLink />
                  {t("search.open")}
                </Button>
                {onAsk && (
                  <Button size="sm" variant="ghost" onClick={() => onAsk(query)} data-testid="evidence-ask">
                    <MessageSquareText />
                    {t("search.ask")}
                  </Button>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
