import { describe, expect, it } from "vitest";

import type { PageLayout } from "@/lib/types";

import { displayUnits, fitFontSize, matchWords, passageNeedles, selectionText, wordAt } from "./textLayer";

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

  it("marks the lines of a cited passage that stand on the open page (1.0.5)", () => {
    // A passage over two pages: the heading and two lines on the first, three on the second.
    const passage = { heading_path: "제3조(납품)", text: "제3조(납품)\n을은 계약일로부터 30일 이내에 납품한다.\n납품 장소는 갑이 지정한다.\n지체상금은 1일당 0.1%로 한다.\n지체상금의 한도는 10%이다.\n검수는 7일 이내에 마친다.\n끝." };
    const second = "지체상금은 1일당 0.1%로 한다.\n지체상금의 한도는 10%이다.\n검수는 7일 이내에 마친다.";
    expect(passageNeedles(passage, (line) => second.includes(line))).toEqual(["지체상금은 1일당 0.1%로 한다.", "지체상금의 한도는 10%이다.", "검수는 7일 이내에 마친다."]);
    // The page the passage begins on: its opening lines, as before — and so while nothing is known of the page.
    const first = "을은 계약일로부터 30일 이내에 납품한다.\n납품 장소는 갑이 지정한다.";
    expect(passageNeedles(passage, (line) => first.includes(line))).toEqual(["을은 계약일로부터 30일 이내에 납품한다.", "납품 장소는 갑이 지정한다."]);
    expect(passageNeedles(passage, () => false)).toEqual(["을은 계약일로부터 30일 이내에 납품한다.", "납품 장소는 갑이 지정한다.", "지체상금은 1일당 0.1%로 한다."]);
    // On the page image the lines are looked for in the word layout.
    expect(passageNeedles({ heading_path: null, text: "다른 쪽의 줄입니다\n번 호 품목" }, (line) => matchWords(layout, [line]).size > 0)).toEqual(["번 호 품목"]);
  });

  it("fits a corrected word into its box by height, then by width", () => {
    expect(fitFontSize("가", 100, 20)).toBeCloseTo(15.6);
    expect(fitFontSize("가나다라마바사", 35, 20)).toBeCloseTo(5 /* 35 / 7 */ < 6 ? 6 : 35 / 7);
    expect(fitFontSize("x", 2, 2)).toBe(6);
  });
});
