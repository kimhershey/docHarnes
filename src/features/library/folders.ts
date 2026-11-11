import type { FolderCount } from "@/lib/types";

/**
 * True when the directory `dir` belongs to what the folder filter lists for `folder`: the folder itself and
 * everything below it (`documents::repo`: `rel_path LIKE 'folder/%'`). The workspace root ("") is the exception —
 * it lists only the files directly in it.
 */
export function inFolder(dir: string, folder: string): boolean {
  if (folder === "") return dir === "";
  return dir === folder || dir.startsWith(`${folder}/`);
}

/**
 * Documents the folder filter shows for `folder`. The statistics count each directory on its own, so the number
 * next to a folder used to be smaller than the list it opens (DOC-15); this adds the folders below it.
 */
export function folderCount(folders: readonly FolderCount[], folder: string): number {
  return folders.reduce((n, f) => (inFolder(f.rel_dir, folder) ? n + f.count : n), 0);
}

/**
 * The folders of the library's navigation: every directory that holds documents, each with the count of its
 * filter, plus the selected folder when only folders below it hold documents — "받은 문서함 N건" selects `inbox`
 * also when the files sit in `inbox/<sub>`, and the selection has to be visible (and must not fall back to "전체").
 */
export function listedFolders(folders: readonly FolderCount[], selected: string | null): FolderCount[] {
  const listed = folders.map((f) => ({ rel_dir: f.rel_dir, count: folderCount(folders, f.rel_dir) }));
  if (selected !== null && !folders.some((f) => f.rel_dir === selected) && folderCount(folders, selected) > 0) {
    listed.push({ rel_dir: selected, count: folderCount(folders, selected) });
    listed.sort((a, b) => (a.rel_dir < b.rel_dir ? -1 : a.rel_dir > b.rel_dir ? 1 : 0));
  }
  return listed;
}
