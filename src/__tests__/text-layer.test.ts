// The on-image text layer helpers (viewer §3.7 "직접 수정" on the page image) and the browser mock's
// layout twin: selection text, hit testing, highlights, word-range edits that rewrite the page text.
import { describe, expect, it } from "vitest";

import { isContiguous, matchWords, range, selectionText, wordAt, wordsInRect } from "@/features/library/textLayer";
import { addWords, applyEdit, layoutText, synthLayout, textLines } from "@/lib/mock-layout";

const TEXT = "메모-1p.pdf 1쪽의 텍스트 레이어입니다.\n합계 12,500원";

describe("text layer helpers", () => {
  const layout = synthLayout(TEXT, "layer");

  it("lays one text line per mock page bar, in reading order", () => {
    expect(layout.words.map((w) => w.t)).toEqual(["메모-1p.pdf", "1쪽의", "텍스트", "레이어입니다.", "합계", "12,500원"]);
    expect(layout.words.map((w) => w.line)).toEqual([0, 0, 0, 0, 1, 1]);
    const [a, b] = [layout.words[0]!, layout.words[1]!];
    expect(b.x).toBeGreaterThan(a.x + a.w);
    expect(layout.words[4]!.y).toBeGreaterThan(a.y + a.h);
    expect(layoutText(layout)).toBe(TEXT);
  });

  it("selection text joins same-line words with spaces and lines with newlines", () => {
    expect(selectionText(layout, [1, 2])).toBe("1쪽의 텍스트");
    expect(selectionText(layout, [3, 4])).toBe("레이어입니다.\n합계");
    expect(selectionText(null, [0])).toBe("");
    expect(isContiguous([2, 3, 4])).toBe(true);
    expect(isContiguous([2, 4])).toBe(false);
    expect(isContiguous([])).toBe(false);
    expect(range(4, 2)).toEqual([2, 3, 4]);
  });

  it("hit-tests words directly and by nearest line / word", () => {
    const w = layout.words[2]!;
    expect(wordAt(layout, w.x + w.w / 2, w.y + w.h / 2)).toBe(2);
    // Between the two lines, nearer the second → its first word by horizontal distance.
    const second = layout.words[4]!;
    expect(wordAt(layout, 0.05, second.y - second.h * 0.2)).toBe(4);
    expect(wordAt(layout, 0.5, 0.99, false)).toBeNull();
    expect(wordsInRect(layout, [0, second.y, 1, second.h])).toEqual([4, 5]);
    expect(wordsInRect(layout, [0.95, 0.95, 0.04, 0.04])).toEqual([]);
  });

  it("highlights the words touched by a needle, phrases included", () => {
    expect([...matchWords(layout, ["텍스트"])]).toEqual([2]);
    expect([...matchWords(layout, ["텍스트 레이어"])]).toEqual([2, 3]);
    expect([...matchWords(layout, ["12,500"])]).toEqual([5]);
    expect(matchWords(layout, [" ", ""]).size).toBe(0);
    expect(matchWords(undefined, ["x"]).size).toBe(0);
  });
});

describe("mock layout edits", () => {
  it("strips OCR markup into lines", () => {
    expect(textLines("# 상호\n<table><tr><th>품명</th><th>수량</th></tr><tr><td>아메리카노</td><td>2</td></tr></table>\n합계 12,500")).toEqual(["# 상호", "품명 수량", "아메리카노 2", "합계 12,500"]);
  });

  it("rewrites the page text in place and keeps boxes for equal word counts", () => {
    const layout = synthLayout(TEXT, "layer");
    const before = { ...layout.words[1]! };
    const t1 = applyEdit(layout, TEXT, 1, 2, "첫째쪽의");
    expect(t1).toBe("메모-1p.pdf 첫째쪽의 텍스트 레이어입니다.\n합계 12,500원");
    expect(layout.words[1]).toMatchObject({ t: "첫째쪽의", x: before.x, w: before.w, edited: true });
    // Two words → three: the span is replaced, the new words share the span's box on the same line.
    const t2 = applyEdit(layout, t1, 2, 4, "본문 글자 입니다");
    expect(t2).toBe("메모-1p.pdf 첫째쪽의 본문 글자 입니다\n합계 12,500원");
    expect(layout.words.length).toBe(7);
    expect(layout.words.slice(2, 5).every((w) => w.line === 0 && w.edited)).toBe(true);
    // Deleting eats the neighbouring blank.
    const t3 = applyEdit(layout, t2, 6, 7, "");
    expect(t3).toBe("메모-1p.pdf 첫째쪽의 본문 글자 입니다\n합계");
    expect(layout.words.length).toBe(6);
    // A stale layout falls back to the layout's own text.
    const other = synthLayout("완전히 다른 글", "estimated");
    expect(applyEdit(other, "xyz", 0, 1, "아주")).toBe("아주 다른 글");
  });

  it("keeps OCR table tags around edited cells", () => {
    const text = "<table><tr><td>아메리카노</td><td>2</td></tr></table>\n합계 12,500";
    const layout = synthLayout(text, "estimated");
    expect(applyEdit(layout, text, 0, 2, "카페라떼 3")).toBe("<table><tr><td>카페라떼</td><td>3</td></tr></table>\n합계 12,500");
  });

  it("adds words in reading order", () => {
    const layout = synthLayout(TEXT, "layer");
    const first = layout.words[0]!;
    const second = layout.words[4]!;
    // A box midway between the two lines becomes a line of its own before "합계".
    const mid = (first.y + first.h / 2 + second.y + second.h / 2) / 2;
    const t1 = addWords(layout, TEXT, [0.1, mid - 0.01, 0.4, 0.02], "추가 메모");
    expect(t1).toBe("메모-1p.pdf 1쪽의 텍스트 레이어입니다.\n추가 메모\n합계 12,500원");
    expect(layout.words[4]).toMatchObject({ t: "추가", line: 1, edited: true });
    expect(layout.words[6]).toMatchObject({ t: "합계", line: 2 });
    // Below everything → appended.
    const t2 = addWords(layout, t1, [0.1, 0.95, 0.3, 0.03], "끝");
    expect(t2.endsWith("\n끝")).toBe(true);
    expect(addWords(layout, t2, [0, 0, 0.1, 0.1], "  ")).toBe(t2);
  });
});
