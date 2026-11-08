// 문서 생성 store: per-workspace binding, the draft hand-over from the assistant / jobs page and "새로 만들기".
import { beforeEach, describe, expect, it } from "vitest";

import type { Draft } from "@/lib/types";

import { resetGenerate, useGenerateStore } from "./generate";

const draft: Draft = {
  id: "d1",
  title: "견적서",
  template: { kind: "builtin", template: "quotation" },
  doc_type: "quotation",
  fields: [],
  tables: [],
  body: [],
  references: [],
  issues: [],
  language: "ko",
  tone: "concise",
  length: "short",
  created_at: "",
  updated_at: "",
  model: null,
  request: { form_ref: "quotation", format_description: null, content: "노트북 10대", sources: [], output: "pdf", tone: "concise", length: "short", language: "ko", use_knowledge: false, feedback: "더 짧게", previous: null, excluded_sources: ["x"] },
  notes: [],
  saved_path: null,
  saved_doc_id: null,
};

beforeEach(() => resetGenerate());

describe("generate store", () => {
  it("binds to a workspace, keeps a draft handed over before the first mount and starts over for another one", () => {
    useGenerateStore.getState().openDraft(draft);
    useGenerateStore.getState().bind("C:\\\\A", "ko");
    expect(useGenerateStore.getState().draft).toBe(draft);
    expect(useGenerateStore.getState().handoff).toBe(true);
    const s = useGenerateStore.getState();
    s.setRequest({ content: "hello" });
    useGenerateStore.getState().bind("C:\\\\A", "ko");
    expect(useGenerateStore.getState().request.content).toBe("hello");
    useGenerateStore.getState().bind("C:\\\\B", "en");
    expect(useGenerateStore.getState().draft).toBeNull();
    expect(useGenerateStore.getState().request).toMatchObject({ content: "", language: "en" });
  });

  it("opens a handed-over draft with its request (feedback / previous cleared) and announces it once", () => {
    useGenerateStore.getState().openDraft(draft);
    const s = useGenerateStore.getState();
    expect(s.draft).toBe(draft);
    expect(s.handoff).toBe(true);
    expect(s.jobId).toBeNull();
    expect(s.request).toMatchObject({ form_ref: "quotation", content: "노트북 10대", output: "pdf", feedback: null, previous: null, excluded_sources: ["x"] });
    s.consumeHandoff();
    expect(useGenerateStore.getState().handoff).toBe(false);
  });

  it("resets the draft but keeps the form preferences", () => {
    const s = useGenerateStore.getState();
    s.setRequest({ form_ref: "report", output: "docx", content: "x", use_knowledge: false });
    s.setDraft(draft);
    s.setJobId("job");
    useGenerateStore.getState().reset("ko");
    const after = useGenerateStore.getState();
    expect(after.draft).toBeNull();
    expect(after.jobId).toBeNull();
    expect(after.request).toMatchObject({ form_ref: "report", output: "docx", content: "", use_knowledge: false });
  });

  it("updates the draft in place", () => {
    const s = useGenerateStore.getState();
    s.setDraft(draft);
    s.updateDraft((d) => ({ ...d, title: "edited" }));
    expect(useGenerateStore.getState().draft?.title).toBe("edited");
  });
});
