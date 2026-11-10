import { describe, expect, it } from "vitest";

import { DEFAULT_NAME_TEMPLATE, FOLDER_TEMPLATES } from "@/lib/types";

import { EXAMPLE_DOCUMENT, dateParts, formatTotal, insertAtCursor, parseTemplate, renderTargetPath, renderTemplate, sanitizeComponent, sanitizeFolder, splitName, unknownVariables } from "./template";

// Mirrors the Rust unit tests in crates/doculens-core/src/organize/template.rs so the live example
// in the organise screen agrees with the backend's plan.

const typeLabel = (slug: string) => ({ estimate: "견적서", dev_contract: "소프트웨어 개발 계약서", receipt: "영수증" })[slug] ?? null;

describe("template parsing", () => {
  it("splits literals and variables and reports unknown names", () => {
    const tokens = parseTemplate("{{year}}-{{month}}_{{vendor}}{{bogus}}.pdf");
    expect(tokens).toHaveLength(7);
    expect(tokens[5]).toEqual({ kind: "var", name: "bogus" });
    expect(unknownVariables("{{year}}/{{bogus}}/{{nope}}")).toEqual(["bogus", "nope"]);
    expect(unknownVariables("plain {{ year }}")).toEqual([]);
    expect(parseTemplate("{{open")).toEqual([{ kind: "lit", text: "{{open" }]);
  });

  it("renders unknown values as 미상 and lists them", () => {
    const r = renderTemplate("{{year}}-{{month}}_{{vendor}}_{{total}}", (v) => ({ year: "2026", month: "10", total: "12,500" })[v] ?? null);
    expect(r.text).toBe("2026-10_미상_12,500");
    expect(r.missing).toEqual(["vendor"]);
  });
});

describe("path sanitising", () => {
  it("replaces forbidden characters, trailing dots and reserved names", () => {
    expect(sanitizeComponent('a<b>:c"d/e\\f|g?h*i')).toBe("a_b__c_d_e_f_g_h_i");
    expect(sanitizeComponent("name. . ")).toBe("name");
    expect(sanitizeComponent("CON")).toBe("_CON");
    expect(sanitizeComponent("con.txt")).toBe("_con.txt");
    expect(sanitizeComponent("   ")).toBe("_");
    expect(sanitizeFolder("documents\\2026/../10//영수증")).toBe("documents/2026/10/영수증");
    expect(sanitizeComponent("한한")).toBe("한한");
  });

  it("formats dates, totals and file names like the backend", () => {
    expect(dateParts("2026-10-14")).toEqual(["2026", "10", "14"]);
    expect(dateParts("2026-1")).toEqual(["2026", "01", ""]);
    expect(dateParts("2026-10-14 12:31")).toEqual(["2026", "10", "14"]);
    expect(dateParts("oct 2026")).toBeNull();
    expect(formatTotal(12500)).toBe("12,500");
    expect(formatTotal(-1234567.4)).toBe("-1,234,567");
    expect(formatTotal(999)).toBe("999");
    expect(splitName("영수증-1p.jpg")).toEqual(["영수증-1p", "jpg"]);
    expect(splitName(".hidden")).toEqual([".hidden", ""]);
    expect(splitName("archive.tar.verylongext")).toEqual(["archive.tar.verylongext", ""]);
  });
});

describe("live example", () => {
  it("renders the default templates for the sample estimate", () => {
    const r = renderTargetPath(DEFAULT_NAME_TEMPLATE, FOLDER_TEMPLATES.month, EXAMPLE_DOCUMENT, { typeLabel });
    expect(r.path).toBe("documents/2026/10/견적서/2026-10-14_견적서_㈜한빛소프트_12,500,000.jpg");
    expect(r.missing).toEqual([]);
  });

  it("keeps the current folder / name when a template is off and flags missing values", () => {
    const doc = { ...EXAMPLE_DOCUMENT, vendor: null, total_amount: null, doc_type: null };
    const r = renderTargetPath("{{vendor}}_{{total}}_{{seq}}", null, doc, { typeLabel, seq: 3 });
    expect(r.path).toBe("inbox/미상_미상_03.jpg");
    expect(r.missing).toEqual(["total", "vendor"]);
    const folderOnly = renderTargetPath(null, FOLDER_TEMPLATES.doc_type, doc, { typeLabel });
    expect(folderOnly.path).toBe("documents/미상/scan_0008.jpg");
    expect(folderOnly.missing).toEqual(["doc_type"]);
  });

  it("falls back to the import date and the original stem for title, and caps the path length", () => {
    const doc = { ...EXAMPLE_DOCUMENT, doc_date: null, title: null };
    expect(renderTargetPath("{{year}}-{{month}}-{{day}}_{{title}}_{{tags}}.{{ext}}", "documents", doc, { typeLabel }).path).toBe("documents/2026-10-15_scan_0008_외주.jpg.jpg");
    const long = renderTargetPath("{{vendor}}".repeat(40), "documents/{{year}}", EXAMPLE_DOCUMENT, { typeLabel });
    expect([...long.path].length).toBeLessThanOrEqual(240);
    expect(long.path.startsWith("documents/2026/")).toBe(true);
    expect(long.path.endsWith(".jpg")).toBe(true);
  });

  it("inserts a variable at the caret", () => {
    expect(insertAtCursor("ab", 1, 1, "{{year}}")).toEqual({ value: "a{{year}}b", caret: 9 });
    expect(insertAtCursor("abc", null, null, "{{x}}")).toEqual({ value: "abc{{x}}", caret: 8 });
    expect(insertAtCursor("abcd", 1, 3, "-")).toEqual({ value: "a-d", caret: 2 });
  });
});
