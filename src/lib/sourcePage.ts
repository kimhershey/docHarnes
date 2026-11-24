/**
 * Where a search hit / an answer's source is shown and opened (1.0.5).
 *
 * User report: "근거로 출력하는 위치와 검색의 위치가 문서에서 찾은 내용과 맞지 않음". A passage (chunk) covers
 * `page_start..page_end` — two or three pages at the usual chunk size — and every label and every link said
 * `page_start`, whatever page the match was on. The core now says the page of the match (`match_page`,
 * `knowledge::locate`), and this is the one place that turns it into a label and a link:
 *
 * - a number: that page — "p.4", or "p.4 (3–5쪽 대목)" when the passage runs over several pages;
 * - `null`: the document's pages say nothing about where its text is (a converted document whose whole text is
 *   kept on page 1) — no page in the label, no `page=` in the link;
 * - absent: a source stored before 1.0.5 — what it was shown with, `page_start` ("p.3" / "p.3-5").
 */

/** The page fields of a `SearchHit` / `Citation`. */
export interface PagedSource {
  page_start: number;
  page_end: number;
  match_page?: number | null;
}

type Translate = (key: string, options?: Record<string, unknown>) => string;

/** The page a source is shown on and opened at — `null` when none is to be shown. */
export function sourcePage(source: PagedSource): number | null {
  if (source.match_page === null) return null;
  return source.match_page ?? source.page_start;
}

/** "p.4", "p.4 (3–5쪽 대목)", or "" when no page is to be shown. */
export function sourcePageLabel(source: PagedSource, t: Translate): string {
  const page = sourcePage(source);
  if (page === null) return "";
  if (source.page_start === source.page_end) return t("answer.citation_page", { page });
  // (stored before 1.0.5: the range it was shown with)
  if (source.match_page === undefined) return t("answer.citation_pages", { from: source.page_start, to: source.page_end });
  return t("answer.citation_page_in", { page, from: source.page_start, to: source.page_end });
}

/** Link to the viewer at the source's page with its passage marked — and the words of `query`, when given. */
export function sourceHref(source: PagedSource & { document_id: string; chunk_id: number }, query?: string): string {
  const params = new URLSearchParams();
  const page = sourcePage(source);
  if (page !== null) params.set("page", String(page));
  if (query) params.set("q", query);
  params.set("chunk", String(source.chunk_id));
  return `/library/${source.document_id}?${params.toString()}`;
}
