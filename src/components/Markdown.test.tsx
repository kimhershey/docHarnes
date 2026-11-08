// The report renderer: block structure from the backend's Markdown (headings, lists, tables,
// inline bold / code) and the date-line guard (review L19: "2026. 11. 8." is not a numbered list).
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { Markdown } from "./Markdown";

const render = (text: string) => renderToStaticMarkup(<Markdown text={text} />);

describe("Markdown", () => {
  it("renders headings, paragraphs and inline bold / code without HTML pass-through", () => {
    const html = render("# 프로젝트 리포트\n\n본문 **강조** 와 `코드` <b>x</b>");
    expect(html).toContain("<h1");
    expect(html).toContain("프로젝트 리포트");
    expect(html).toContain("<strong>강조</strong>");
    expect(html).toContain("<code");
    expect(html).toContain("&lt;b&gt;x&lt;/b&gt;");
  });

  it("renders bullet and numbered lists", () => {
    const html = render("- 하나\n- 둘\n\n1. 첫째\n2. 둘째\n10. 열째");
    expect(html.match(/<ul/g)).toHaveLength(1);
    expect(html.match(/<ol/g)).toHaveLength(1);
    expect(html.match(/<li/g)).toHaveLength(5);
    expect(html).toContain("열째");
  });

  it("keeps a Korean date line as a paragraph (not a numbered list)", () => {
    const html = render("2026. 11. 8. 검사 결과\n다음 줄");
    expect(html).not.toContain("<ol");
    expect(html).toContain("<p");
    expect(html).toContain("2026. 11. 8. 검사 결과 다음 줄");
    // A marker over three digits is not a list either.
    expect(render("20261108. 비고")).not.toContain("<ol");
  });

  it("renders pipe tables with the header row and skips the separator", () => {
    const html = render("| 규칙 | 판정 |\n|---|---|\n| 사업자번호 | **통과** |");
    expect(html).toContain("<table");
    expect(html.match(/<th[\s>]/g)).toHaveLength(2);
    expect(html.match(/<td[\s>]/g)).toHaveLength(2);
    expect(html).not.toContain("---");
    expect(html).toContain("<strong>통과</strong>");
  });

  it("ignores blank lines and joins paragraph continuation lines", () => {
    const html = render("첫 줄\n둘째 줄\n\n\n새 문단");
    expect(html.match(/<p/g)).toHaveLength(2);
    expect(html).toContain("첫 줄 둘째 줄");
  });
});
