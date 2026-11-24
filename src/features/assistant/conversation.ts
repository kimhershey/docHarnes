// What the assistant remembers of the current conversation: the earlier turns sent with every
// question and command (design §3.7.6 follow-ups). Pure helpers over the store's messages.

import type { ConversationTurn } from "@/lib/types";
import type { Message } from "@/stores/assistant";

import { clarificationQuestion, deriveResultActions } from "./describePlan";

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
      case "reply":
        turns.push({ role: "assistant", text: clip(m.text) });
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

/**
 * 1.0.6: the assistant's last reply of the conversation, in full — what "방금 답변 다듬어줘" works on. The turns
 * of `conversationTurns` are cut to fit a prompt; this is the whole text of the most recent answer, or of the
 * summaries / answers of the most recent result card. Cards in between that say nothing to polish (a setting
 * that ran, a question back, an error) are passed over; an answer that found nothing is not a reply. `""` when
 * there is none — also for a reply read back from a stored copy that was cut (`clipped: "all"`): a part of a
 * reply is not polished as if it were the reply.
 */
export function lastReply(messages: Message[]): string {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i]!;
    if (m.role !== "assistant") continue;
    if (m.kind === "answer") {
      if (m.pending || !m.answer) continue;
      if (m.clipped === "all" || (m.answer.no_evidence && !m.answer.used_general_knowledge)) return "";
      const text = m.answer.text.trim();
      if (text) return text;
    } else if (m.kind === "result") {
      const actions = deriveResultActions(m.execution, m.plan);
      const texts = [...actions.answers.map((a) => a.answer.text), ...actions.summaries.flatMap((s) => s.groups.map((g) => (s.groups.length > 1 && g.label ? `${g.label}\n${g.answer.text}` : g.answer.text)))].map((t) => t.trim()).filter(Boolean);
      if (texts.length === 0) continue;
      return m.clipped === "all" ? "" : texts.join("\n\n");
    }
  }
  return "";
}
