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
