// Browser mock of the assistant chat-session commands (0.7.0 §1.6): sessions and their messages
// per workspace in localStorage, the same contract as the Rust `chat_*` commands (messages are
// the UI's JSON payloads, the title follows the first user message until renamed).

import { currentWorkspacePath, err } from "./mock-backend";
import type { ChatMessageRow, ChatSessionSummary } from "./types";

const KEY = "doculens.mock.chat";
/** Auto titles: the first user message clipped to this many characters (mirrors the core). */
export const AUTO_TITLE_CHARS = 40;

interface ChatState {
  sessions: ChatSessionSummary[];
  messages: Record<string, ChatMessageRow[]>;
  seq: number;
}

function empty(): ChatState {
  return { sessions: [], messages: {}, seq: 1 };
}

function loadAll(): Record<string, ChatState> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "{}") as Record<string, ChatState>;
  } catch {
    return {};
  }
}

function load(): ChatState {
  return loadAll()[currentWorkspacePath() ?? ""] ?? empty();
}

function save(state: ChatState): void {
  const all = loadAll();
  all[currentWorkspacePath() ?? ""] = state;
  localStorage.setItem(KEY, JSON.stringify(all));
}

function now(): string {
  return new Date().toISOString();
}

function summary(s: ChatState, c: ChatSessionSummary): ChatSessionSummary {
  return { ...c, message_count: (s.messages[c.id] ?? []).length };
}

function session(s: ChatState, id: string): ChatSessionSummary {
  const c = s.sessions.find((x) => x.id === id);
  if (!c) throw err("not_found", "chat_session_not_found");
  return c;
}

/** The auto title of a session from a user message payload (null when the payload is not one). */
export function autoTitle(payload: unknown): string | null {
  if (!payload || typeof payload !== "object") return null;
  const p = payload as { role?: unknown; text?: unknown };
  if (p.role !== "user" || typeof p.text !== "string") return null;
  const text = p.text.replace(/\s+/g, " ").trim();
  if (!text) return null;
  const chars = [...text];
  return chars.length > AUTO_TITLE_CHARS ? `${chars.slice(0, AUTO_TITLE_CHARS - 1).join("")}…` : text;
}

export async function mockChat(cmd: string, a: Record<string, any>): Promise<unknown> { // eslint-disable-line @typescript-eslint/no-explicit-any
  const s = load();
  switch (cmd) {
    case "chat_sessions_list":
      return [...s.sessions].sort((x, y) => y.updated_at.localeCompare(x.updated_at)).map((c) => summary(s, c));
    case "chat_session_create": {
      const title = typeof a.title === "string" ? a.title.trim() : "";
      const c: ChatSessionSummary = { id: `chat-${s.seq++}`, title, title_auto: title === "", created_at: now(), updated_at: now(), message_count: 0 };
      s.sessions.push(c);
      s.messages[c.id] = [];
      save(s);
      return summary(s, c);
    }
    case "chat_session_rename": {
      const c = session(s, String(a.id));
      const title = String(a.title ?? "").trim();
      if (!title) throw err("invalid", "chat_title_empty");
      c.title = title;
      c.title_auto = false;
      save(s);
      return undefined;
    }
    case "chat_session_delete": {
      const id = String(a.id);
      s.sessions = s.sessions.filter((x) => x.id !== id);
      delete s.messages[id];
      save(s);
      return undefined;
    }
    case "chat_messages_list": {
      const id = String(a.sessionId);
      session(s, id);
      return [...(s.messages[id] ?? [])].sort((x, y) => x.seq - y.seq);
    }
    case "chat_message_put": {
      const c = session(s, String(a.sessionId));
      const seq = Number(a.seq);
      if (!Number.isInteger(seq)) throw err("invalid", "chat_seq_invalid");
      const rows = s.messages[c.id] ?? [];
      const existing = rows.find((x) => x.seq === seq);
      if (existing) existing.payload = a.payload;
      else rows.push({ seq, payload: a.payload, created_at: now() });
      s.messages[c.id] = rows;
      c.updated_at = now();
      if (c.title_auto && c.title === "") c.title = autoTitle(a.payload) ?? "";
      save(s);
      return undefined;
    }
    default:
      throw err("internal", `unknown chat command: ${cmd}`);
  }
}
