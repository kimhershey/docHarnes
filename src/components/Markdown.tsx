import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * A list item: "- ", "* ", "• " or "1. " … "999. ". The ordered marker is limited to three digits so a
 * Korean date line such as "2026. 11. 8. 검사 결과" is a paragraph, not a numbered list.
 */
const LIST_ITEM = /^\s*(?:[-*•]|\d{1,3}\.)\s+/;

/**
 * Minimal Markdown renderer for backend reports (headings, paragraphs, bullet / numbered lists, pipe
 * tables, **bold**, `code`). No HTML pass-through: the text is rendered as React nodes.
 */
export function Markdown({ text, className }: { text: string; className?: string }) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const out: ReactNode[] = [];
  let i = 0;
  let key = 0;
  const inline = (s: string): ReactNode[] => {
    const parts = s.split(/(\*\*[^*]+\*\*|`[^`]+`)/g);
    return parts.map((p, n) => {
      if (/^\*\*[^*]+\*\*$/.test(p)) return <strong key={n}>{p.slice(2, -2)}</strong>;
      if (/^`[^`]+`$/.test(p)) return <code key={n} className="rounded bg-bg-sunken px-1 text-[0.9em]">{p.slice(1, -1)}</code>;
      return <span key={n}>{p}</span>;
    });
  };
  while (i < lines.length) {
    const line = lines[i]!;
    if (!line.trim()) {
      i += 1;
      continue;
    }
    const heading = line.match(/^(#{1,6})\s+(.*)$/);
    if (heading) {
      const level = heading[1]!.length;
      const Tag = (`h${Math.min(6, level)}`) as "h1" | "h2" | "h3" | "h4" | "h5" | "h6";
      out.push(
        <Tag key={key++} className={cn("font-semibold", level === 1 ? "text-lg" : level === 2 ? "mt-3 text-base" : "mt-2 text-sm")}>
          {inline(heading[2]!)}
        </Tag>,
      );
      i += 1;
      continue;
    }
    if (/^\|/.test(line)) {
      const rows: string[][] = [];
      while (i < lines.length && /^\|/.test(lines[i]!)) {
        const cells = lines[i]!.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((c) => c.trim());
        if (!cells.every((c) => /^:?-{2,}:?$/.test(c))) rows.push(cells);
        i += 1;
      }
      const [head, ...body] = rows;
      out.push(
        <table key={key++} className="my-2 w-full text-xs">
          {head && (
            <thead>
              <tr className="text-left text-fg-subtle">
                {head.map((c, n) => (
                  <th key={n} scope="col" className="border-b border-border px-2 py-1 font-medium">
                    {inline(c)}
                  </th>
                ))}
              </tr>
            </thead>
          )}
          <tbody>
            {body.map((r, rn) => (
              <tr key={rn} className="border-b border-border/60">
                {r.map((c, n) => (
                  <td key={n} className="px-2 py-1 align-top selectable">
                    {inline(c)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>,
      );
      continue;
    }
    if (LIST_ITEM.test(line)) {
      const ordered = /^\s*\d{1,3}\.\s/.test(line);
      const items: string[] = [];
      while (i < lines.length && LIST_ITEM.test(lines[i]!)) {
        items.push(lines[i]!.replace(LIST_ITEM, ""));
        i += 1;
      }
      const Tag = ordered ? "ol" : "ul";
      out.push(
        <Tag key={key++} className={cn("my-1 pl-5 text-sm", ordered ? "list-decimal" : "list-disc")}>
          {items.map((it, n) => (
            <li key={n} className="selectable">
              {inline(it)}
            </li>
          ))}
        </Tag>,
      );
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && lines[i]!.trim() && !/^(#{1,6}\s|\|)/.test(lines[i]!) && !LIST_ITEM.test(lines[i]!)) {
      para.push(lines[i]!);
      i += 1;
    }
    out.push(
      <p key={key++} className="my-1 text-sm leading-relaxed selectable">
        {inline(para.join(" "))}
      </p>,
    );
  }
  return <div className={cn("flex flex-col", className)}>{out}</div>;
}
