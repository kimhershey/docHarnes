import { afterEach, describe, expect, it, vi } from "vitest";

import { beginDocDrag, endDocDrag, ghostLabel, moveDocDrag, nativeDrop, registerDropTarget, registerFallbackFileHandler, resetDnd, targetAt, useDndStore, type DropTargetHandlers } from "./store";

/** Minimal DOM stand-in: `document.elementFromPoint` returns an element whose closest `[data-doc-drop]` host carries `id`. */
function fakeDocument(hostIds: (string | null)[]) {
  const chain = hostIds.map((id) => {
    const host = {
      dataset: { docDrop: id ?? undefined },
      parentElement: null as unknown,
      closest: () => (id === null ? null : host),
    };
    return host;
  });
  for (let i = 0; i < chain.length - 1; i++) chain[i]!.parentElement = { closest: () => chain[i + 1] };
  const leaf = { closest: () => (hostIds[0] === null ? null : chain[0]) };
  (globalThis as { document?: unknown }).document = { elementFromPoint: () => leaf };
}

afterEach(() => {
  resetDnd();
  delete (globalThis as { document?: unknown }).document;
});

describe("drop target registry", () => {
  it("resolves the innermost target that accepts the payload kind, falling back to its parents", () => {
    const inner: DropTargetHandlers = { onFiles: vi.fn() };
    const outer: DropTargetHandlers = { onDocuments: vi.fn() };
    const a = registerDropTarget({ current: inner });
    const b = registerDropTarget({ current: outer });
    fakeDocument([a.id, b.id]);
    expect(targetAt(1, 1, "files")).toBe(a.id);
    expect(targetAt(1, 1, "documents")).toBe(b.id);
    a.unregister();
    expect(targetAt(1, 1, "files")).toBeNull();
  });

  it("ignores disabled targets and returns null without a DOM", () => {
    const t = registerDropTarget({ current: { onDocuments: vi.fn(), disabled: true } });
    fakeDocument([t.id]);
    expect(targetAt(0, 0, "documents")).toBeNull();
    delete (globalThis as { document?: unknown }).document;
    expect(targetAt(0, 0, "documents")).toBeNull();
  });
});

describe("in-app document drag", () => {
  it("tracks the pointer and hands the documents to the hovered target on drop", () => {
    const onDocuments = vi.fn();
    const t = registerDropTarget({ current: { onDocuments } });
    fakeDocument([t.id]);
    const docs = [{ id: "d1", name: "a.pdf" }];
    beginDocDrag(docs, { x: 5, y: 5 });
    expect(useDndStore.getState().dragging).toEqual(docs);
    expect(useDndStore.getState().overTarget).toBe(t.id);
    moveDocDrag(7, 9);
    expect(useDndStore.getState().pointer).toEqual({ x: 7, y: 9 });
    expect(endDocDrag(true)).toBe(true);
    expect(onDocuments).toHaveBeenCalledWith(docs);
    expect(useDndStore.getState().dragging).toBeNull();
    expect(useDndStore.getState().overTarget).toBeNull();
  });

  it("drops nothing when cancelled or when no target accepts documents", () => {
    const onDocuments = vi.fn();
    const t = registerDropTarget({ current: { onDocuments } });
    fakeDocument([t.id]);
    beginDocDrag([{ id: "d1", name: "a.pdf" }], { x: 0, y: 0 });
    expect(endDocDrag(false)).toBe(false);
    expect(onDocuments).not.toHaveBeenCalled();
    const files = registerDropTarget({ current: { onFiles: vi.fn() } });
    fakeDocument([files.id]);
    beginDocDrag([{ id: "d1", name: "a.pdf" }], { x: 0, y: 0 });
    expect(useDndStore.getState().overTarget).toBeNull();
    expect(endDocDrag(true)).toBe(false);
  });

  it("fires the spring-loading hover callback after the delay and cancels it when the pointer leaves", () => {
    vi.useFakeTimers();
    const onHover = vi.fn();
    const nav = registerDropTarget({ current: { onHover, onDocuments: vi.fn(), springMs: 100 } });
    fakeDocument([nav.id]);
    beginDocDrag([{ id: "d1", name: "a.pdf" }], { x: 0, y: 0 });
    vi.advanceTimersByTime(99);
    expect(onHover).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onHover).toHaveBeenCalledTimes(1);
    // Leaving and re-entering restarts the timer; a drop before it fires cancels it.
    fakeDocument([null]);
    moveDocDrag(1, 1);
    fakeDocument([nav.id]);
    moveDocDrag(2, 2);
    endDocDrag(true);
    vi.advanceTimersByTime(500);
    expect(onHover).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });
});

describe("native file drops", () => {
  it("dispatches to the target under the point, else to the window-level fallback", () => {
    const onFiles = vi.fn();
    const fallback = vi.fn();
    const t = registerDropTarget({ current: { onFiles } });
    registerFallbackFileHandler(fallback);
    fakeDocument([t.id]);
    expect(nativeDrop(1, 1, ["C:\\a.pdf"])).toBe("target");
    expect(onFiles).toHaveBeenCalledWith(["C:\\a.pdf"]);
    fakeDocument([null]);
    expect(nativeDrop(1, 1, ["C:\\b.pdf"])).toBe("fallback");
    expect(fallback).toHaveBeenCalledWith(["C:\\b.pdf"]);
    expect(nativeDrop(1, 1, [])).toBe("none");
  });
});

describe("ghost label", () => {
  it("shows the name for one document and the count otherwise", () => {
    expect(ghostLabel([{ id: "1", name: "견적서.pdf" }], (n) => `문서 ${n}개`)).toBe("견적서.pdf");
    expect(ghostLabel([{ id: "1", name: "a" }, { id: "2", name: "b" }], (n) => `문서 ${n}개`)).toBe("문서 2개");
  });
});
