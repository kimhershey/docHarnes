import { describe, expect, it } from "vitest";

import type { FolderCount } from "@/lib/types";

import { folderCount, inFolder, listedFolders } from "./folders";

const folders: FolderCount[] = [
  { rel_dir: "", count: 1 },
  { rel_dir: "documents", count: 4 },
  { rel_dir: "documents/2026", count: 2 },
  { rel_dir: "inbox/2026-11", count: 3 },
  { rel_dir: "inbox2", count: 5 },
];

describe("library folders (DOC-15)", () => {
  it("a folder covers itself and the folders below it — like the list filter; the root only itself", () => {
    expect(inFolder("inbox", "inbox")).toBe(true);
    expect(inFolder("inbox/2026-11", "inbox")).toBe(true);
    expect(inFolder("inbox2", "inbox")).toBe(false);
    expect(inFolder("", "")).toBe(true);
    expect(inFolder("documents", "")).toBe(false);
  });

  it("counts what the filter lists: the folder and everything below", () => {
    expect(folderCount(folders, "documents")).toBe(6);
    expect(folderCount(folders, "documents/2026")).toBe(2);
    expect(folderCount(folders, "inbox")).toBe(3);
    expect(folderCount(folders, "")).toBe(1);
    expect(folderCount(folders, "archive")).toBe(0);
  });

  it("lists every folder with the count of its filter", () => {
    expect(listedFolders(folders, null)).toEqual([
      { rel_dir: "", count: 1 },
      { rel_dir: "documents", count: 6 },
      { rel_dir: "documents/2026", count: 2 },
      { rel_dir: "inbox/2026-11", count: 3 },
      { rel_dir: "inbox2", count: 5 },
    ]);
  });

  it("lists the selected folder when only folders below it hold documents (the 받은 문서함 chip)", () => {
    const listed = listedFolders(folders, "inbox");
    expect(listed.map((f) => f.rel_dir)).toEqual(["", "documents", "documents/2026", "inbox", "inbox/2026-11", "inbox2"]);
    expect(listed.find((f) => f.rel_dir === "inbox")?.count).toBe(3);
    // A selected folder that is listed anyway, or holds nothing at all, adds nothing.
    expect(listedFolders(folders, "documents")).toHaveLength(folders.length);
    expect(listedFolders(folders, "archive")).toHaveLength(folders.length);
  });
});
