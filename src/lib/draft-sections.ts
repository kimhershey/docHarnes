// Sections of a draft body (0.7.0 plan §1.5) — shared by the 문서 작성 screen, the browser mock and the plan
// descriptions. Mirrors `generate::sections_of` in the core: the body is split at heading blocks of level ≤ 2;
// a leading run without such a heading is section 1 (untitled); a body without headings is one section.

import type { DraftBlock, ReviseScope } from "@/lib/types";

export interface DraftSection {
  /** 1-based ordinal (the `section` of a `ReviseScope`). */
  ordinal: number;
  /** Index of the first block (the heading when the section has one). */
  start: number;
  /** Index after the last block. */
  end: number;
  heading: string | null;
}

export function isSectionHeading(block: DraftBlock): boolean {
  return block.kind === "heading" && block.level <= 2;
}

export function sectionsOf(body: DraftBlock[]): DraftSection[] {
  const out: DraftSection[] = [];
  let start = 0;
  for (let i = 0; i < body.length; i++) {
    if (isSectionHeading(body[i]!) && i > start) {
      out.push({ ordinal: out.length + 1, start, end: i, heading: sectionHeading(body, start) });
      start = i;
    }
  }
  if (body.length > 0) out.push({ ordinal: out.length + 1, start, end: body.length, heading: sectionHeading(body, start) });
  return out;
}

function sectionHeading(body: DraftBlock[], start: number): string | null {
  const b = body[start];
  return b && b.kind === "heading" && b.level <= 2 ? b.text : null;
}

/** The section a block index belongs to (null for an empty body). */
export function sectionAt(body: DraftBlock[], index: number): DraftSection | null {
  return sectionsOf(body).find((s) => index >= s.start && index < s.end) ?? null;
}

/** Short label of a revision scope: "3. 결론" / "끝" / "전체". */
export function reviseScopeLabel(scope: ReviseScope, body: DraftBlock[], ko: boolean): string {
  const sections = sectionsOf(body);
  const name = (n: number) => {
    const s = sections.find((x) => x.ordinal === n);
    const heading = s?.heading ?? (ko ? "(제목 없음)" : "(untitled)");
    return `${n}. ${heading}`;
  };
  switch (scope.scope) {
    case "whole":
      return ko ? "전체" : "whole draft";
    case "section":
      return name(scope.section);
    case "insert_after":
      return scope.section == null ? (ko ? "끝에 새 절" : "new section at the end") : ko ? `${name(scope.section)} 뒤에 새 절` : `new section after ${name(scope.section)}`;
  }
}
