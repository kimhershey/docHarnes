import { describe, expect, it } from "vitest";

import { looksLikeFollowUp } from "@/lib/follow-up";
import type { Answer, PlanResponse } from "@/lib/types";
import type { Message } from "@/stores/assistant";

import { conversationTurns } from "./conversation";

const answer = (text: string, noEvidence = false): Answer =>
  ({ id: null, kind: "answer", question: "q", text, sentences: [], sources: [], no_evidence: noEvidence, used_general_knowledge: false, conflicts: null, model: null, elapsed_ms: 0, prompt_tokens: 0, completion_tokens: 0, document_id: null, created_at: "", feedback: null }) as Answer;

describe("conversationTurns", () => {
  it("keeps user questions and answers, drops the pending question and empty answers", () => {
    const messages: Message[] = [
      { id: 1, role: "user", text: "합계 금액은?" },
      { id: 2, role: "assistant", kind: "answer", requestId: "r1", request: { question: "합계 금액은?" }, answer: answer("합계는 12,000원입니다 [1]."), error: null, pending: false },
      { id: 3, role: "user", text: "xyzzy" },
      { id: 4, role: "assistant", kind: "answer", requestId: "r2", request: { question: "xyzzy" }, answer: answer("지식 베이스에 근거가 없습니다.", true), error: null, pending: false },
      { id: 5, role: "assistant", kind: "help" },
      { id: 6, role: "assistant", kind: "executed", text: "다크 테마로 바꿨습니다", source: "규칙", confidence: 1 },
      { id: 7, role: "user", text: "그건 누가 작성했어?" },
    ];
    expect(conversationTurns(messages)).toEqual([
      { role: "user", text: "합계 금액은?" },
      { role: "assistant", text: "합계는 12,000원입니다 [1]." },
      { role: "user", text: "xyzzy" },
      { role: "assistant", text: "다크 테마로 바꿨습니다" },
    ]);
  });

  it("remembers a UI op that failed as failed", () => {
    const messages: Message[] = [
      { id: 1, role: "user", text: "워크스페이스 D:\\NoSuch 열어" },
      { id: 2, role: "assistant", kind: "executed", text: "워크스페이스 D:\\NoSuch을(를) 엽니다", source: "모델", confidence: 0.9, failures: ["워크스페이스로 사용할 수 없는 경로입니다"], ran: 0 },
      { id: 3, role: "user", text: "다음" },
    ];
    expect(conversationTurns(messages)[1]).toEqual({ role: "assistant", text: "워크스페이스 D:\\NoSuch을(를) 엽니다 ✖ 워크스페이스로 사용할 수 없는 경로입니다" });
  });

  it("keeps only the last turns and clips long ones", () => {
    const messages: Message[] = Array.from({ length: 12 }, (_, i) => ({ id: i, role: "user", text: `질문 ${i} ${"가".repeat(600)}` }) as Message);
    messages.push({ id: 99, role: "assistant", kind: "executed", text: "끝", source: "", confidence: 1 });
    const turns = conversationTurns(messages);
    expect(turns).toHaveLength(8);
    expect(turns[0]!.text.startsWith("질문 5 ")).toBe(true);
    expect([...turns[0]!.text]).toHaveLength(500);
    expect(turns[0]!.text.endsWith("…")).toBe(true);
  });

  it("uses the clarification question and the plan summary as the assistant's words", () => {
    const response = { result: { plan: { intent: "clarify", scope: {}, operations: [{ op: "clarify", question: "어떤 폴더인가요?" }], requires_confirmation: false, confidence: 0, summary: "" } } } as unknown as PlanResponse;
    const messages: Message[] = [
      { id: 1, role: "user", text: "정리해줘" },
      { id: 2, role: "assistant", kind: "clarify", text: "정리해줘", response, status: "pending" },
    ];
    expect(conversationTurns(messages)).toEqual([
      { role: "user", text: "정리해줘" },
      { role: "assistant", text: "어떤 폴더인가요?" },
    ]);
  });
});

describe("looksLikeFollowUp", () => {
  it("detects demonstratives and very short questions, not fresh questions", () => {
    for (const q of ["그건 누가 작성했어?", "아까 금액의 부가세는?", "이 문서 요약해줘", "그 문서의 합계는?", "What about that one?", "왜?"]) expect(looksLikeFollowUp(q), q).toBe(true);
    for (const q of ["xyzzy 플루토늄 원자로", "출장비 한도는?", "그림 파일 목록", "이번 달 영수증 합계", ""]) expect(looksLikeFollowUp(q), q).toBe(false);
  });
});
