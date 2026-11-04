import { Search as SearchIcon, Sparkles } from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import { useCollections, useKnowledgeStatus, useSearch } from "@/lib/queries";
import type { SearchHit, SearchMode, SearchRequest, SearchScope } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useKnowledgeStore } from "@/stores/knowledge";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

/** Builds the scope for the shared scope selector values ("all" | "knowledge" | "col:<id>"). */
export function scopeFor(value: string, latestOnly: boolean): SearchScope {
  const scope: SearchScope = { latest_only: latestOnly };
  if (value === "knowledge") scope.roles = ["knowledge"];
  else if (value.startsWith("col:")) scope.collections = [value.slice(4)];
  return scope;
}

/** Link to the viewer at the hit's page with the query highlighted. */
export function hitHref(hit: Pick<SearchHit, "document_id" | "page_start" | "chunk_id">, query: string): string {
  const params = new URLSearchParams({ page: String(hit.page_start), q: query, chunk: String(hit.chunk_id) });
  return `/library/${hit.document_id}?${params.toString()}`;
}

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

export function SearchPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const workspace = useWorkspaceStore((s) => s.current);
  const settings = useSettingsStore((s) => s.settings);
  const query = useKnowledgeStore((s) => s.searchQuery);
  const mode = useKnowledgeStore((s) => s.searchMode);
  const scopeValue = useKnowledgeStore((s) => s.searchScope);
  const setSearch = useKnowledgeStore((s) => s.setSearch);
  const setPendingAssistantInput = useUiStore((s) => s.setPendingAssistantInput);
  const [draft, setDraft] = useState(query);
  const [latestOnly, setLatestOnly] = useState(settings?.knowledge.latest_only ?? true);
  const { data: collections } = useCollections(!!workspace);
  const { data: status } = useKnowledgeStatus(!!workspace);

  const request = useMemo<SearchRequest | null>(
    () => (query.trim() ? { query: query.trim(), mode, scope: scopeFor(scopeValue, latestOnly), limit: 30 } : null),
    [query, mode, scopeValue, latestOnly],
  );
  const { data, isFetching, error } = useSearch(request);
  const activeQuery = request?.query ?? query;

  const submit = (e?: React.FormEvent) => {
    e?.preventDefault();
    setSearch({ searchQuery: draft });
  };
  const embeddingReady = !!status?.embedding.loaded;

  return (
    <div className="flex h-full flex-col" data-testid="page-search">
      <div className="border-b border-border px-4 py-3">
        <h1 className="mb-2 text-base font-semibold">{t("search.title")}</h1>
        <form className="flex flex-wrap items-center gap-2" onSubmit={submit} role="search">
          <Input value={draft} onChange={(e) => setDraft(e.target.value)} placeholder={t("search.placeholder")} className="min-w-64 flex-1" aria-label={t("search.title")} data-testid="search-input" />
          <Select value={mode} onValueChange={(v) => setSearch({ searchMode: v as SearchMode })}>
            <SelectTrigger className="w-32" aria-label={t("search.mode.label")} data-testid="search-mode">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {(["hybrid", "keyword", "semantic"] as const).map((m) => (
                <SelectItem key={m} value={m}>
                  {t(`search.mode.${m}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={scopeValue} onValueChange={(v) => setSearch({ searchScope: v })}>
            <SelectTrigger className="w-44" aria-label={t("search.scope.label")} data-testid="search-scope">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">{t("search.scope.all")}</SelectItem>
              <SelectItem value="knowledge">{t("search.scope.knowledge")}</SelectItem>
              {(collections ?? []).map((c) => (
                <SelectItem key={c.id} value={`col:${c.id}`}>
                  {t("search.scope.collection", { name: c.name })}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <label className="flex items-center gap-1 text-xs text-fg-muted">
            <input type="checkbox" checked={latestOnly} onChange={(e) => setLatestOnly(e.target.checked)} />
            {t("search.latest_only")}
          </label>
          <Button type="submit" data-testid="search-run">
            <SearchIcon />
            {t("search.run")}
          </Button>
        </form>
        {mode === "semantic" && !embeddingReady && (
          <p className="mt-2 text-xs text-warn" role="status">
            {t("search.semantic_unavailable")}
          </p>
        )}
        {mode === "hybrid" && !embeddingReady && data && (
          <p className="mt-2 text-xs text-fg-subtle" role="status" data-testid="search-keyword-only">
            {t("search.keyword_only")}
          </p>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        {!request && (
          <div className="flex h-full flex-col items-center justify-center gap-2 text-fg-muted">
            <Sparkles className="size-8 text-accent" aria-hidden="true" />
            <p>{t("search.empty_hint")}</p>
          </div>
        )}
        {error && (
          <p className="text-sm text-danger" role="alert">
            {describeError(error, t)}
          </p>
        )}
        {request && data && (
          <div className="mb-2 flex flex-wrap items-center gap-2 text-xs text-fg-subtle" data-testid="search-meta">
            {isFetching && <Spinner />}
            <span>{t("search.results", { count: data.hits.length, ms: data.elapsed_ms })}</span>
            {data.query_terms.length > 0 && (
              <span>
                {t("search.terms")}: {data.query_terms.join(", ")}
              </span>
            )}
            {data.expanded_terms.length > 0 && (
              <span>
                {t("search.expanded")}: {data.expanded_terms.join(", ")}
              </span>
            )}
            {data.used_like_fallback && <span className="text-warn">{t("search.like_fallback")}</span>}
          </div>
        )}
        {request && data && data.hits.length === 0 && (
          <p className="text-sm text-fg-muted" data-testid="search-no-results">
            {t("search.no_results")}
          </p>
        )}
        <ul className="flex flex-col gap-2">
          {data?.hits.map((hit) => (
            <li key={hit.chunk_id} className="rounded-lg border border-border bg-bg-elevated p-3" data-testid="search-hit" data-doc-id={hit.document_id}>
              <div className="flex flex-wrap items-center gap-2 text-xs text-fg-subtle">
                <button type="button" className="text-left text-sm font-medium text-fg hover:underline cursor-pointer" onClick={() => navigate(hitHref(hit, activeQuery))} data-testid="search-hit-title">
                  {hit.title}
                </button>
                {hit.role === "knowledge" && <Badge variant="info">{t("library.role_knowledge")}</Badge>}
                <span>{hit.page_start === hit.page_end ? t("answer.citation_page", { page: hit.page_start }) : t("answer.citation_pages", { from: hit.page_start, to: hit.page_end })}</span>
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
              <p className="mt-1 text-sm leading-relaxed">
                <Snippet text={hit.snippet} highlights={hit.highlights} />
              </p>
              <div className="mt-1 flex gap-1">
                <Button size="sm" variant="ghost" onClick={() => navigate(hitHref(hit, activeQuery))}>
                  {t("search.open")}
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    setPendingAssistantInput(activeQuery);
                    navigate("/assistant");
                  }}
                >
                  {t("search.ask")}
                </Button>
              </div>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
