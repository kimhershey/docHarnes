import { describe, expect, it } from "vitest";

import en from "@/i18n/en.json";
import ko from "@/i18n/ko.json";

import { sourceHref, sourcePage, sourcePageLabel } from "./sourcePage";

/** The catalogue's own strings, filled in as i18next does it. */
function translator(catalogue: { answer: Record<string, unknown> }) {
  return (key: string, options: Record<string, unknown> = {}) => {
    const template = catalogue.answer[key.replace(/^answer\./, "")];
    if (typeof template !== "string") throw new Error(`missing ${key}`);
    return template.replace(/\{\{(\w+)\}\}/g, (_, name: string) => String(options[name]));
  };
}

const tKo = translator(ko);
const tEn = translator(en);
const source = { document_id: "doc-1", chunk_id: 42 };

describe("sourcePage (1.0.5 — a source is shown on the page of its match)", () => {
  it("a passage on one page is on that page", () => {
    const s = { ...source, page_start: 4, page_end: 4, match_page: 4 };
    expect(sourcePage(s)).toBe(4);
    expect(sourcePageLabel(s, tKo)).toBe("p.4");
    expect(sourcePageLabel(s, tEn)).toBe("p.4");
    expect(sourceHref(s)).toBe("/library/doc-1?page=4&chunk=42");
    // (the query's words are marked in the viewer; the order of the parameters is the one links always had)
    expect(sourceHref(s, "지체상금 한도")).toBe(`/library/doc-1?${new URLSearchParams({ page: "4", q: "지체상금 한도", chunk: "42" }).toString()}`);
  });

  it("a passage over several pages shows the page of the match and the pages it covers", () => {
    const s = { ...source, page_start: 3, page_end: 5, match_page: 4 };
    expect(sourcePage(s)).toBe(4);
    expect(sourcePageLabel(s, tKo)).toBe("p.4 (3–5쪽 대목)");
    expect(sourcePageLabel(s, tEn)).toBe("p.4 (passage on pp. 3–5)");
    expect(sourceHref(s, "q")).toBe("/library/doc-1?page=4&q=q&chunk=42");
    // (the match may be on the first page, or the last)
    expect(sourcePageLabel({ ...s, match_page: 3 }, tKo)).toBe("p.3 (3–5쪽 대목)");
    expect(sourceHref({ ...s, match_page: 5 })).toBe("/library/doc-1?page=5&chunk=42");
  });

  it("a document whose pages say nothing about its text shows no page and opens without one", () => {
    const s = { ...source, page_start: 1, page_end: 1, match_page: null };
    expect(sourcePage(s)).toBeNull();
    expect(sourcePageLabel(s, tKo)).toBe("");
    expect(sourceHref(s)).toBe("/library/doc-1?chunk=42");
    expect(sourceHref(s, "q")).toBe("/library/doc-1?q=q&chunk=42");
  });

  it("a source stored before 1.0.5 keeps the page it was shown with", () => {
    const one = { ...source, page_start: 2, page_end: 2 };
    expect(sourcePage(one)).toBe(2);
    expect(sourcePageLabel(one, tKo)).toBe("p.2");
    expect(sourceHref(one)).toBe("/library/doc-1?page=2&chunk=42");
    const several = { ...source, page_start: 3, page_end: 5 };
    expect(sourcePage(several)).toBe(3);
    expect(sourcePageLabel(several, tKo)).toBe("p.3-5");
    expect(sourceHref(several, "q")).toBe("/library/doc-1?page=3&q=q&chunk=42");
  });
});
