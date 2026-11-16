import { describe, expect, it } from "vitest";

import type { PageTextEdit } from "@/lib/types";

import { correctionsOf } from "./editCorrections";

const entry = (id: number, before: string, after: string, extra: Partial<PageTextEdit> = {}): PageTextEdit => ({ id, page_no: 1, source: "layer", edited_at: "2026-11-16T00:00:00Z", undone: false, before, after, ...extra });

describe("correctionsOf (viewer corrections → replacements in the original file)", () => {
  it("turns the corrections into replacements, oldest first, each pair once", () => {
    // Newest first, as the history command returns them.
    const history = [entry(4, "납품 기안", "납품 기한"), entry(3, "(주)한빗", "(주)한빛", { page_no: 2 }), entry(2, "납품 기안", "납품 기한", { page_no: 3 }), entry(1, "11월 3O일", "11월 30일")];
    expect(correctionsOf(history)).toEqual([
      { find: "11월 3O일", replace: "11월 30일" },
      { find: "납품 기안", replace: "납품 기한" },
      { find: "(주)한빗", replace: "(주)한빛" },
    ]);
  });

  it("keeps the order of a word corrected twice", () => {
    expect(correctionsOf([entry(2, "한빛", "㈜한빛"), entry(1, "한빗", "한빛")])).toEqual([
      { find: "한빗", replace: "한빛" },
      { find: "한빛", replace: "㈜한빛" },
    ]);
  });

  it("leaves out what cannot be a replacement in the file", () => {
    const history = [
      // taken back with Ctrl+Z
      entry(5, "가", "나", { undone: true }),
      // words that were only added: nothing to find
      entry(4, "", "새로 넣은 말", { source: "add" }),
      entry(3, "   ", "x"),
      // spans lines: a replacement stays inside one paragraph
      entry(2, "첫 줄\n둘째 줄", "첫 줄 둘째 줄", { source: "manual" }),
      // nothing but blanks changed
      entry(1, "같은 글 ", "같은 글"),
    ];
    expect(correctionsOf(history)).toEqual([]);
  });
});
