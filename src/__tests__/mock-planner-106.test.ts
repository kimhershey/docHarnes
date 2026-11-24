// 1.0.6 (user decision 2026-11-24): "다듬어줘 / 수정해줘" is not document writing — as the browser mock mirrors
// it (the Rust core is the source of truth: `planner/tests/v106.rs`). A request to polish wording is answered
// in the chat, over the text the sentence brings or over the assistant's last reply in full.
import { beforeEach, describe, expect, it } from "vitest";

import type { Answer, ConversationTurn, PlanResponse } from "@/lib/types";
import type { Message } from "@/stores/assistant";

const store = new Map<string, string>();
(globalThis as unknown as { localStorage: Storage }).localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
  removeItem: (k: string) => void store.delete(k),
  clear: () => store.clear(),
  key: () => null,
  length: 0,
} as Storage;

const backend = await import("@/lib/mock-backend");
const planner = await import("@/lib/mock-planner");
const { polishRequest, polishWhat, says } = await import("@/lib/polish-request");
const { textToWorkOn } = await import("@/lib/carried-text");
const { conversationTurns, lastReply } = await import("@/features/assistant/conversation");

const REPLY = "납품 기한은 계약 체결일로부터 60일입니다 [1].\n지체 시에는 지체상금이 부과됩니다 [2].\n검수는 납품 후 14일 안에 마칩니다 [3].";
const BODY = "본 계약의 납품 기한은 계약 체결일로부터 60일로 하며 지체 시에는 지체상금을 부과하는 것으로 한다고 되어 있음";
const HISTORY: ConversationTurn[] = [
  { role: "user", text: "납품 기한 알려줘" },
  { role: "assistant", text: REPLY },
];
const MEASURED = ["방금 답변 좀 더 자연스럽게 다듬어줘", "위 내용 수정해줘", "더 간결하게 수정해줘", "요구사항 부분을 좀 다듬어줘"];
const TEMPLATES = ["보고서", "회의록", "계약서", "견적서", "report", "contract"];
const bare = { doc_types: [{ label_ko: "영수증", label_en: "Receipt" }], schemas: [{ name: "영수증" }], history: HISTORY };

const answer = (text: string, noEvidence = false): Answer =>
  ({ id: null, kind: "answer", question: "q", text, sentences: [], sources: [], no_evidence: noEvidence, used_general_knowledge: false, conflicts: null, model: null, elapsed_ms: 0, prompt_tokens: 0, completion_tokens: 0, document_id: null, created_at: "", feedback: null }) as Answer;

beforeEach(async () => {
  backend.resetMock();
  for (const key of ["doculens.mock.projects", "doculens.mock.knowledge", "doculens.mock.pipeline", "doculens.mock.maintenance"]) localStorage.removeItem(key);
  localStorage.setItem("doculens.mock.fast", "1");
  await backend.mockInvoke("workspace_create", { args: { path: "C:\\Work\\Docs106", name: null, locale: null, force_unlock: false } });
});

describe("1.0.6 — a request to polish wording", () => {
  it("is recognised by its words; when in doubt it is not one", () => {
    for (const text of [...MEASURED, `이 문장 다듬어줘: ${BODY}`, "다듬어줘", "다시 써줘", "더 간결하게", "좀 더 자연스럽게 써줘", "더 짧게 다시 작성해줘", "방금 답변 존댓말로 고쳐줘", "오타 고쳐줘", "맞춤법 교정해줘", "위 답변을 표로 정리해서 다듬어줘", "이 문장 교정", "다듬어줘: 짧은 글입니다", "'본 계약의 납품 기한은 60일로 한다' 이 문장 다듬어줘", "polish the last answer", "rewrite it more formally", "Could you revise the answer?"]) {
      expect(polishRequest(text, bare, TEMPLATES), text).not.toBeNull();
    }
    for (const text of [
      "'갑'을 '을'로 수정해줘",
      "다듬어서 새 문서로 만들어줘",
      "이 내용으로 보고서 작성해줘",
      "회의록 초안 수정해줘",
      "계약서 문구 수정해줘",
      "이 문서 다듬어줘",
      "선택한 것 내용 수정해줘",
      "영수증 내용 수정해줘",
      "테마 수정해줘",
      "공급가액 수정해줘",
      "프로젝트 이름 수정해줘",
      "경비 태그 내용 수정해줘",
      "제목 다듬어줘",
      "2쪽 내용 수정해줘",
      "3절 다시 써줘",
      "rewrite section 2 more formally",
      "수정해줘",
      "지난달 영수증 합계 다시 써줘",
      "더 간결하게 요약해줘",
      "더 간결하게 작성해줘",
      "make it more concise",
      "이 답변 수정해서 메일로 보내줘",
      "방금 답변 다듬어서 저장해줘",
      "지체상금 조항을 어떻게 다듬어야 해?",
      "글 다듬는 법 좀 알려줘",
      "다듬을 수 있어?",
      "어떤 부분을 다듬었는지 알려줘",
      "수정된 내용 알려줘",
      "교정 일정 알려줘",
      "다듬지 말고 그대로 둬",
    ]) {
      expect(polishRequest(text, bare, TEMPLATES), text).toBeNull();
    }
    // (an English word is a whole word, a Korean one a part of the compact text)
    expect(says("remove the page", ["move"])).toBe(false);
    expect(says("polished text", ["polish*"])).toBe(true);
    expect(says("위 내용 수정 해줘", ["수정해"])).toBe(true);
  });

  it("works on the text the sentence brings, else on the last reply in full", () => {
    const pasted = `이 문장 다듬어줘: ${BODY}`;
    expect(polishRequest(pasted, bare, TEMPLATES)).toMatchObject({ question: pasted, fromReply: false });
    expect(polishRequest("위 내용 수정해줘", bare, TEMPLATES)).toEqual({ instruction: "위 내용 수정해줘", question: `위 내용 수정해줘:\n${REPLY}`, fromReply: true });
    // (the apostrophes of an English sentence quote nothing: the reply is worked on)
    expect(polishRequest("Please polish the last answer, it's too long and doesn't read well", bare, TEMPLATES)).toMatchObject({ fromReply: true });
    // The reply the shell sends in full wins over the turn of the history, which is cut.
    const whole = Array.from({ length: 80 }, (_, i) => `${i + 1}번 조항은 납품과 검수에 관한 내용입니다.`).join("\n");
    expect(polishRequest("방금 답변 다듬어줘", { ...bare, last_reply: whole }, TEMPLATES)?.question).toBe(`방금 답변 다듬어줘:\n${whole}`);
    // "There is none" is none; so is a conversation without a reply.
    expect(polishRequest("방금 답변 다듬어줘", { ...bare, last_reply: "" }, TEMPLATES)).toMatchObject({ question: null, fromReply: false });
    expect(polishRequest("방금 답변 다듬어줘", { ...bare, history: [] }, TEMPLATES)?.question).toBeNull();
    // A reply to the question for the text is the text; a short one is more of the request.
    const question = polishWhat(true);
    const asked = { ...bare, history: [...HISTORY, { role: "user", text: "위 내용 수정해줘" } as ConversationTurn, { role: "assistant", text: question } as ConversationTurn] };
    expect(polishRequest(`위 내용 수정해줘\n\n[어시스턴트 질문] ${question}\n[사용자 답변] ${BODY}`, asked, TEMPLATES)?.question).toBe(`위 내용 수정해줘:\n${BODY}`);
    expect(polishRequest(`위 내용 수정해줘\n\n[어시스턴트 질문] ${question}\n[사용자 답변] 더 짧게`, asked, TEMPLATES)?.question).toBe(`위 내용 수정해줘 더 짧게:\n${REPLY}`);
    // The text-only answer reads the text back, whatever the instruction says.
    expect(textToWorkOn(`더 간결하게:\n${REPLY}`)).toBe(REPLY);
    expect(textToWorkOn(`위 내용 수정해줘:\n${REPLY}`)).toBe(REPLY);
    expect(textToWorkOn("납품 기한 알려줘")).toBe("납품 기한 알려줘");
  });

  it("the mock planner answers it in the chat — in both interpretation modes — or asks for the text", () => {
    const after = planner.buildContext({ history: HISTORY, last_reply: REPLY });
    const fresh = planner.buildContext({ last_reply: "" });
    for (const routing of ["model_first", "rules_first"] as const) {
      for (const text of MEASURED) {
        const r = planner.planText(text, after, { routing });
        expect(r.plan.operations, text).toEqual([{ op: "answer", question: `${text}:\n${REPLY}`, knowledge: false }]);
        expect(r.plan).toMatchObject({ intent: "answer", requires_confirmation: false, summary: "글을 다듬어 답합니다" });
        expect(r.choices ?? []).toEqual([]);
        const none = planner.planText(text, fresh, { routing });
        expect(none.plan.clarification_question, text).toBe(polishWhat(true));
      }
    }
    // A document is written when the sentence asks for one; a section of the open draft stays the draft's.
    expect(planner.planText("다듬어서 새 문서로 만들어줘", after).plan.operations[0]).toMatchObject({ op: "draft_document" });
    expect(planner.planText("이 내용으로 보고서 작성해줘", after).plan.operations[0]).toMatchObject({ op: "draft_document", form_ref: "report" });
    const drafting = planner.buildContext({ history: HISTORY, last_reply: REPLY, draft_id: "dr-1" });
    expect(planner.planText("3절 다시 써줘", drafting).plan.operations[0]).toMatchObject({ op: "revise_draft", section: 3 });
    // The reply to the question for the text is worked on.
    const question = polishWhat(true);
    const replied = planner.planText(`위 내용 수정해줘\n\n[어시스턴트 질문] ${question}\n[사용자 답변] ${BODY}`, fresh);
    expect(replied.plan.operations).toEqual([{ op: "answer", question: `위 내용 수정해줘:\n${BODY}`, knowledge: false }]);
  });

  it("planner_plan takes the last reply with the context and does not hand it back", async () => {
    const res = await backend.mockInvoke<PlanResponse>("planner_plan", { text: "방금 답변 다듬어줘", context: { history: [{ role: "user", text: "q" }, { role: "assistant", text: "잘린 답변…" }], last_reply: REPLY } });
    expect(res.result.plan.operations).toEqual([{ op: "answer", question: `방금 답변 다듬어줘:\n${REPLY}`, knowledge: false }]);
    expect("last_reply" in res.context).toBe(false);
    // The answer step works on the reply (the mock "model" tidies its whitespace), not on the instruction.
    const op = res.result.plan.operations[0] as { question: string };
    const polished = await backend.mockInvoke<Answer>("knowledge_answer", { req: { question: op.question, retrieve: false, save: false }, requestId: "t-polish-1" });
    expect(polished.text).toBe(REPLY.replace(/\s+/g, " "));
    expect(polished.sources).toEqual([]);
  });
});

describe("1.0.6 — the last reply of a conversation", () => {
  it("is the whole text of the most recent answer, whatever cards follow it", () => {
    const long = `${"가나다라마바사아자차 ".repeat(120)}끝`;
    const messages: Message[] = [
      { id: 1, role: "user", text: "납품 기한은?" },
      { id: 2, role: "assistant", kind: "answer", requestId: "r1", request: { question: "납품 기한은?" }, answer: answer("먼저 한 답변입니다."), error: null, pending: false },
      { id: 3, role: "user", text: "자세히" },
      { id: 4, role: "assistant", kind: "answer", requestId: "r2", request: { question: "자세히" }, answer: answer(long), error: null, pending: false },
      { id: 5, role: "user", text: "다크 테마로" },
      { id: 6, role: "assistant", kind: "executed", text: "다크 테마로 바꿨습니다", source: "규칙", confidence: 1 },
      { id: 7, role: "user", text: "방금 답변 다듬어줘" },
    ];
    expect(lastReply(messages)).toBe(long.trim());
    // (the turn of the history is cut to 500 characters — the reason the reply travels on its own)
    expect(Array.from(conversationTurns(messages)[3]!.text)).toHaveLength(500);
    // An answer still streaming is not a reply yet; one that found nothing is none.
    expect(lastReply([...messages, { id: 8, role: "assistant", kind: "answer", requestId: "r3", request: { question: "x" }, answer: null, error: null, pending: true }])).toBe(long.trim());
    expect(lastReply([...messages, { id: 8, role: "assistant", kind: "answer", requestId: "r3", request: { question: "x" }, answer: answer("지식 베이스에 근거가 없습니다.", true), error: null, pending: false }])).toBe("");
    // A reply read back from a stored copy that was cut is not the reply.
    expect(lastReply([{ ...messages[3]!, clipped: "all" } as Message])).toBe("");
    expect(lastReply([messages[0]!, messages[5]!])).toBe("");
    expect(lastReply([])).toBe("");
  });
});
