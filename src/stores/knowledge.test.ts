// Streamed answer text (`knowledge://answer`): deltas are kept per request id from `beginStream` until the
// request's `done` event — a delta that arrives after it (the backend may emit one more after a cancel) is
// dropped, as is anything for a request this UI did not start.
import { beforeEach, describe, expect, it, vi } from "vitest";

import { EVENTS, type AnswerDelta } from "@/lib/types";

const bus = vi.hoisted(() => ({ handlers: new Map<string, (payload: unknown) => void>() }));

vi.mock("@/lib/ipc", () => ({
  subscribe: async (event: string, handler: (payload: unknown) => void) => {
    bus.handlers.set(event, handler);
    return () => bus.handlers.delete(event);
  },
}));

import { useKnowledgeStore } from "./knowledge";

const emit = (delta: AnswerDelta) => bus.handlers.get(EVENTS.knowledgeAnswer)!(delta);
const streams = () => useKnowledgeStore.getState().streams;

beforeEach(async () => {
  await useKnowledgeStore.getState().init();
  useKnowledgeStore.setState({ streams: {} });
});

describe("answer streams", () => {
  it("collects the deltas of a request this UI started", () => {
    useKnowledgeStore.getState().beginStream("r1");
    emit({ request_id: "r1", delta: "납품 ", done: false });
    emit({ request_id: "r1", delta: "기한은", done: false });
    expect(streams()).toEqual({ r1: "납품 기한은" });
    // Another window's / an unknown request's deltas are not tracked.
    emit({ request_id: "other", delta: "x", done: false });
    expect(streams()).toEqual({ r1: "납품 기한은" });
  });

  it("ignores a delta that arrives after the request's `done` event (a late one after a cancel)", () => {
    useKnowledgeStore.getState().beginStream("r1");
    emit({ request_id: "r1", delta: "납품", done: false });
    emit({ request_id: "r1", delta: "", done: true });
    expect(streams()).toEqual({});
    const settled = streams();
    emit({ request_id: "r1", delta: " 늦은 조각", done: false });
    // Nothing comes back — not the entry, not even a new state object (no re-render of the card).
    expect(streams()).toBe(settled);
    expect("r1" in streams()).toBe(false);
    // `clearStream` after the command returned is a no-op, and a late delta after it is dropped as well.
    useKnowledgeStore.getState().clearStream("r1");
    emit({ request_id: "r1", delta: "또", done: false });
    expect(streams()).toEqual({});
  });

  it("keeps the streams of other requests apart", () => {
    useKnowledgeStore.getState().beginStream("r1");
    useKnowledgeStore.getState().beginStream("r2");
    emit({ request_id: "r1", delta: "a", done: false });
    emit({ request_id: "r1", delta: "", done: true });
    emit({ request_id: "r1", delta: "late", done: false });
    emit({ request_id: "r2", delta: "b", done: false });
    expect(streams()).toEqual({ r2: "b" });
  });
});
