// What the assistant remembers of the current conversation: the earlier turns sent with every
// question and command (design §3.7.6 follow-ups). Pure helpers over the store's messages.

import type { ConversationTurn } from "@/lib/types";
import type { Message } from "@/stores/assistant";

import { clarificationQuestion } from "./describePlan";

/** Turns kept for the model (the most recent ones) and the length of each. */
export const MAX_MEMORY_TURNS = 8;
const MAX_TURN_CHARS = 500;

function clip(text: string): string {
  const t = text.replace(/\s+/g, " ").trim();
  return [...t].length > MAX_TURN_CHARS ? `${[...t].slice(0, MAX_TURN_CHARS - 1).join("")}…` : t;
}

/**
 * Earlier turns of the conversation, oldest first. The trailing user message (the question being
 * asked right now) is left out; help / error cards and "no evidence" answers carry nothing worth
 * remembering, cancelled plans neither.
 */
export function conversationTurns(messages: Message[], max = MAX_MEMORY_TURNS): ConversationTurn[] {
  const turns: ConversationTurn[] = [];
  for (const m of messages) {
    if (m.role === "user") {
      turns.push({ role: "user", text: clip(m.text) });
      continue;
    }
    switch (m.kind) {
      case "answer":
        if (m.answer && !m.pending && !(m.answer.no_evidence && !m.answer.used_general_knowledge)) turns.push({ role: "assistant", text: clip(m.answer.text) });
        break;
      case "executed":
        // An op that failed is remembered as failed (the card's text alone says what was asked for).
        turns.push({ role: "assistant", text: clip(m.failures && m.failures.length > 0 ? `${m.text} ✖ ${m.failures.join(" / ")}` : m.text) });
        break;
      case "plan":
        if (m.status !== "cancelled") turns.push({ role: "assistant", text: clip(m.plan.summary ?? m.text) });
        break;
      case "execution":
      case "result":
        turns.push({ role: "assistant", text: clip(m.plan.summary ?? m.text) });
        break;
      case "clarify": {
        const q = clarificationQuestion(m.response.result.plan);
        if (q) turns.push({ role: "assistant", text: clip(q) });
        break;
      }
      case "evidence":
        // What was found (titles only) so a follow-up question can lean on "그 문서".
        if (m.response && !m.pending && m.response.hits.length > 0) turns.push({ role: "assistant", text: clip([...new Set(m.response.hits.map((h) => h.title))].join(", ")) });
        break;
      default:
        break;
    }
  }
  if (turns.length > 0 && turns[turns.length - 1]!.role === "user") turns.pop();
  return turns.filter((t) => t.text.length > 0).slice(-max);
}
