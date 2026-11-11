// Global undo / redo routing (DOC-2): a screen with its own history comes first, and the file journal is
// reverted from that screen only after it has asked.
import { describe, expect, it, vi } from "vitest";

import type { JournalBatch } from "@/lib/types";

import { nextJournalBatch, routeUndo, setUndoScope, type UndoScope } from "./useGlobalShortcuts";

function scopeWith(taken: boolean) {
  const run = vi.fn(async (_redo: boolean) => taken);
  const confirmJournal = vi.fn((_redo: boolean, _proceed: () => void): void => undefined);
  return { run, confirmJournal } satisfies UndoScope;
}

const batch = (id: string, kind: string, created_at: string, extra: Partial<JournalBatch> = {}): JournalBatch => ({ id, kind, label: null, source: null, created_at, entries: 2, undone_at: null, undone_by: null, redo_of: null, ...extra });

describe("routeUndo", () => {
  it("reverts the file journal directly when no screen has a history of its own", async () => {
    const journal = vi.fn();
    await routeUndo(false, journal, null);
    expect(journal).toHaveBeenCalledTimes(1);
  });

  it("gives the undo to the screen and leaves the journal alone when the screen took it", async () => {
    const journal = vi.fn();
    const scope = scopeWith(true);
    await routeUndo(false, journal, scope);
    expect(scope.run).toHaveBeenCalledWith(false);
    expect(scope.confirmJournal).not.toHaveBeenCalled();
    expect(journal).not.toHaveBeenCalled();
  });

  it("asks through the screen before the journal when the screen had nothing to take back", async () => {
    const journal = vi.fn();
    const scope = scopeWith(false);
    await routeUndo(true, journal, scope);
    expect(scope.run).toHaveBeenCalledWith(true);
    expect(scope.confirmJournal).toHaveBeenCalledWith(true, journal);
    // Nothing is reverted until the user agrees.
    expect(journal).not.toHaveBeenCalled();
    scope.confirmJournal.mock.calls[0]![1]();
    expect(journal).toHaveBeenCalledTimes(1);
  });

  it("uses the registered scope until it is removed; a stale removal does not drop a newer scope", async () => {
    const journal = vi.fn();
    const first = scopeWith(true);
    const removeFirst = setUndoScope(first);
    await routeUndo(false, journal);
    expect(first.run).toHaveBeenCalledTimes(1);
    const second = scopeWith(true);
    const removeSecond = setUndoScope(second);
    removeFirst();
    await routeUndo(false, journal);
    expect(second.run).toHaveBeenCalledTimes(1);
    removeSecond();
    await routeUndo(false, journal);
    expect(journal).toHaveBeenCalledTimes(1);
  });
});

describe("nextJournalBatch", () => {
  const organize = batch("b1", "organize", "2026-11-10T09:00:00Z");
  const rename = batch("b2", "rename", "2026-11-11T09:00:00Z");

  it("undo: the newest file batch that is not undone", () => {
    expect(nextJournalBatch([rename, organize], false)?.id).toBe("b2");
    expect(nextJournalBatch([{ ...rename, undone_at: "2026-11-11T10:00:00Z" }, organize], false)?.id).toBe("b1");
    // The order of the list does not matter.
    expect(nextJournalBatch([organize, rename], false)?.id).toBe("b2");
  });

  it("undo: nothing when every batch is undone, empty or an undo itself", () => {
    expect(nextJournalBatch([], false)).toBeNull();
    expect(nextJournalBatch([{ ...organize, undone_at: "2026-11-10T10:00:00Z" }, batch("u1", "undo", "2026-11-10T10:00:00Z")], false)).toBeNull();
    expect(nextJournalBatch([{ ...organize, entries: 0 }], false)).toBeNull();
  });

  it("redo: the newest undo, unless a newer file batch has invalidated it", () => {
    const undone = { ...organize, undone_at: "2026-11-10T10:00:00Z", undone_by: "u1" };
    const undo = batch("u1", "undo", "2026-11-10T10:00:00Z");
    expect(nextJournalBatch([undo, undone], true)?.id).toBe("u1");
    expect(nextJournalBatch([rename, undo, undone], true)).toBeNull();
    expect(nextJournalBatch([{ ...undo, undone_at: "2026-11-10T11:00:00Z" }, undone], true)).toBeNull();
    expect(nextJournalBatch([organize], true)).toBeNull();
  });
});
