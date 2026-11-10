import type { SearchHit } from "@/lib/types";

/** Link to the viewer at the hit's page with the query highlighted. */
export function hitHref(hit: Pick<SearchHit, "document_id" | "page_start" | "chunk_id">, query: string): string {
  const params = new URLSearchParams({ page: String(hit.page_start), q: query, chunk: String(hit.chunk_id) });
  return `/library/${hit.document_id}?${params.toString()}`;
}

/** A snippet with its highlight ranges (character offsets) rendered as `<mark>`. */
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
