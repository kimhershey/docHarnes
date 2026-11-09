import { describe, expect, it } from "vitest";

import type { PageLayout } from "@/lib/types";

import { displayUnits, fitFontSize, selectionText, wordAt } from "./textLayer";

const layout: PageLayout = {
  version: 2,
  source: "layer",
  words: [
    // One table row: a wrapped cell ("번" over "호") beside a centred cell — all on row 0.
    { t: "번", x: 0.1, y: 0.1, w: 0.02, h: 0.016, line: 0 },
    { t: "호", x: 0.1, y: 0.117, w: 0.02, h: 0.016, line: 0 },
    { t: "품목", x: 0.2, y: 0.108, w: 0.04, h: 0.016, line: 0 },
    { t: "1", x: 0.1, y: 0.2, w: 0.01, h: 0.016, line: 1 },
  ],
};

describe("textLayer helpers", () => {
  it("joins the words of a table row with spaces and rows with newlines", () => {
    expect(selectionText(layout, [0, 1, 2, 3])).toBe("번 호 품목\n1");
  });

  it("resolves a point between cells to the nearest word of that row", () => {
    expect(wordAt(layout, 0.17, 0.116)).toBe(2);
    expect(wordAt(layout, 0.5, 0.21)).toBe(3);
  });

  it("measures CJK as full width and Latin as half", () => {
    expect(displayUnits("한글")).toBe(2);
    expect(displayUnits("ab")).toBeCloseTo(1.1);
    expect(displayUnits("")).toBe(0.55);
  });

  it("fits a corrected word into its box by height, then by width", () => {
    expect(fitFontSize("가", 100, 20)).toBeCloseTo(15.6);
    expect(fitFontSize("가나다라마바사", 35, 20)).toBeCloseTo(5 /* 35 / 7 */ < 6 ? 6 : 35 / 7);
    expect(fitFontSize("x", 2, 2)).toBe(6);
  });
});
