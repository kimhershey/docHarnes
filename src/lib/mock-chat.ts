// Browser mock of the assistant chat-session commands (0.7.0 §1.6): sessions and their messages
// per workspace in localStorage, the same contract as the Rust `chat_*` commands (messages are
// the UI's JSON payloads, the title follows the first user message until renamed). Where the UI
// could tell the difference the mock follows the core (`knowledge/chat.rs`): session ids are
// unique across workspaces, a message put for a session the workspace does not have re-creates
// that session (untitled), listing an unknown session is an empty list.

import { currentWorkspacePath, err } from "./mock-backend";
import type { ChatMessageRow, ChatSessionSummary } from "./types";

const KEY = "doculens.mock.chat";
const FAIL_LIST_KEY = "doculens.mock.chat.fail_list";
/** Auto titles: the first user message clipped to this many characters (mirrors the core). */
export const AUTO_TITLE_CHARS = 40;

interface ChatState {
  sessions: ChatSessionSummary[];
  messages: Record<string, ChatMessageRow[]>;
}

function empty(): ChatState {
  return { sessions: [], messages: {} };
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

/** Like the core's `new_id` (a UUID without dashes): never the same in two workspaces. */
function newId(): string {
  return crypto.randomUUID().replace(/-/g, "");
}

function summary(s: ChatState, c: ChatSessionSummary): ChatSessionSummary {
  return { ...c, message_count: (s.messages[c.id] ?? []).length };
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
      const c: ChatSessionSummary = { id: newId(), title, title_auto: title === "", created_at: now(), updated_at: now(), message_count: 0 };
      s.sessions.push(c);
      s.messages[c.id] = [];
      save(s);
      return summary(s, c);
    }
    case "chat_session_rename": {
      const title = String(a.title ?? "").trim();
      if (!title) throw err("invalid", "title_empty");
      const c = s.sessions.find((x) => x.id === String(a.id));
      if (!c) return false;
      c.title = title;
      c.title_auto = false;
      c.updated_at = now();
      save(s);
      return true;
    }
    case "chat_session_delete": {
      const id = String(a.id);
      const existed = s.sessions.some((x) => x.id === id);
      s.sessions = s.sessions.filter((x) => x.id !== id);
      delete s.messages[id];
      save(s);
      return existed;
    }
    case "chat_messages_list": {
      // Test hook: `doculens.mock.chat.fail_list` = N makes the next N reads fail (a locked database).
      const fails = Number(localStorage.getItem(FAIL_LIST_KEY) ?? "0");
      if (fails > 0) {
        localStorage.setItem(FAIL_LIST_KEY, String(fails - 1));
        throw err("internal", "database is locked");
      }
      return [...(s.messages[String(a.sessionId)] ?? [])].sort((x, y) => x.seq - y.seq);
    }
    case "chat_message_put": {
      const id = String(a.sessionId ?? "");
      if (!id.trim()) return false;
      const seq = Number(a.seq);
      if (!Number.isInteger(seq)) throw err("invalid", "chat_seq_invalid");
      // `chat::put_message`: a session this workspace does not have (purged meanwhile — or, the bug the UI
      // must not trigger, a message of another workspace's chat) is re-created untitled.
      let c = s.sessions.find((x) => x.id === id);
      if (!c) {
        c = { id, title: "", title_auto: true, created_at: now(), updated_at: now(), message_count: 0 };
        s.sessions.push(c);
      }
      const rows = s.messages[c.id] ?? [];
      const existing = rows.find((x) => x.seq === seq);
      if (existing) existing.payload = a.payload;
      else rows.push({ seq, payload: a.payload, created_at: now() });
      s.messages[c.id] = rows;
      c.updated_at = now();
      if (c.title_auto && c.title === "") c.title = autoTitle(a.payload) ?? "";
      save(s);
      return true;
    }
    default:
      throw err("internal", `unknown chat command: ${cmd}`);
  }
}
