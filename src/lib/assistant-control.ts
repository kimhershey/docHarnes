// The assistant's deterministic controls (user decision 2026-11-09): slash commands, and the cancel /
// ask-back replies to a pending question or plan, are handled by rules before anything reaches the
// model. Mirrors `prerouter::is_cancel` in the Rust core.

import type { PlanMode } from "./types";

export const ROUTING_KEY = "doculens.assistant.routing";

export function isSlashCommand(text: string): boolean {
  return text.trim().startsWith("/");
}

const CANCEL_WORDS = new Set([
  "취소", "취소해", "취소해줘", "취소요", "그만", "그만해", "그만둬", "그만할래", "됐어", "됐어요", "됐다", "아니", "아니야", "아니요", "아냐",
  "하지마", "하지마요", "중단", "중단해", "멈춰", "cancel", "stop", "nevermind", "nvm", "forgetit", "no", "nope", "abort",
]);

/** A bare cancel word ("취소", "그만", "cancel" …): never a new command while something is pending. */
export function isCancelCommand(text: string): boolean {
  const compact = text.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, "");
  return CANCEL_WORDS.has(compact);
}

/**
 * The text sent to the model when the user replies to the assistant's question: the original
 * command, the question and the reply together, so the model continues from where it asked.
 */
export function askBackText(original: string, question: string, reply: string, ko: boolean): string {
  return ko ? `${original.trim()}\n\n[어시스턴트 질문] ${question.trim()}\n[사용자 답변] ${reply.trim()}` : `${original.trim()}\n\n[Assistant question] ${question.trim()}\n[User reply] ${reply.trim()}`;
}

export function loadRouting(): PlanMode {
  try {
    return localStorage.getItem(ROUTING_KEY) === "rules_first" ? "rules_first" : "model_first";
  } catch {
    return "model_first";
  }
}

export function saveRouting(mode: PlanMode): void {
  try {
    localStorage.setItem(ROUTING_KEY, mode);
  } catch {
    /* private window / blocked storage: the choice lasts for the session */
  }
}
