import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import { ArrowRight, ChevronDown, ChevronRight, History, ImageOff, Images, Plus, Sparkles, X } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import i18n from "@/i18n";
import { describeError } from "@/lib/errors";
import { api, isTauri, pickPictureFile } from "@/lib/ipc";
import type { DocumentView, EditAction, EditPicture, EditReplacement, EditReport } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

import { correctionsOf } from "./editCorrections";

/** How many lines the dialog holds. */
const MAX_ROWS = 20;

/** The formats whose own file can be edited in place (core `documents::edit::supported`). */
export const EDITABLE_KINDS = ["hwpx", "docx", "pptx", "xlsx"];

/** What a row of the request can do: paragraphs and table rows are a document's, cells a workbook's. */
const DOCUMENT_ACTIONS: EditAction[] = ["replace", "insert_after", "insert_before", "delete_paragraph", "insert_row_after", "insert_row_before", "delete_row"];
const WORKBOOK_ACTIONS: EditAction[] = ["replace", "set_cell"];
export const actionsFor = (kind: string): EditAction[] => (kind === "xlsx" ? WORKBOOK_ACTIONS : DOCUMENT_ACTIONS);

interface Row {
  action: EditAction;
  find: string;
  replace: string;
  /** Which of the paragraphs / rows that hold `find` (structure edits; empty = the only one). */
  occurrence: string;
}

const EMPTY: Row = { action: "replace", find: "", replace: "", occurrence: "" };

/** Whether the action takes a new text (a removal names its place only). */
const takesText = (a: EditAction) => a !== "delete_paragraph" && a !== "delete_row";
/** Whether the place is a text that several paragraphs / rows may hold (a cell's address names one cell). */
const countsPlaces = (a: EditAction) => a !== "replace" && a !== "set_cell";
/** The label of the first field: the text to find, the text that names the place, or the cell's address. */
const placeLabel = (a: EditAction) =>
  a === "replace" ? "find" : a === "set_cell" ? "cell_address" : a === "delete_paragraph" ? "delete_paragraph_find" : a === "delete_row" ? "delete_row_find" : a.includes("row") ? "anchor_row" : "anchor_paragraph";
const textLabel = (a: EditAction) => (a === "replace" ? "replace" : a === "set_cell" ? "cell_value" : a.includes("row") ? "new_row" : "new_paragraph");

/** The request of one row (core `documents::edit::Replacement`). */
export function editOf(row: Row): EditReplacement {
  if (row.action === "set_cell") return { find: row.find, replace: row.replace, action: "set_cell" };
  const which = Number.parseInt(row.occurrence, 10);
  const occurrence = Number.isFinite(which) && which >= 1 ? { occurrence: which } : {};
  if (row.action === "replace") return { find: row.find, replace: row.replace, ...occurrence };
  return { find: row.find, replace: takesText(row.action) ? row.replace : "", action: row.action, ...occurrence };
}

/** `ppt/media/image3.png` → `image3.png`. */
const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

/** Where a picture is shown, as far as its part says: [i18n key under `pictures.where`, number]. */
export function shownIn(part: string): [string, number | null] | null {
  const lower = part.toLowerCase();
  const n = Number(/(\d+)\.xml$/.exec(lower)?.[1] ?? Number.NaN);
  const numbered = Number.isFinite(n) ? n : null;
  if (lower.startsWith("ppt/slides/")) return ["slide", numbered];
  if (lower.startsWith("ppt/slidelayouts/")) return ["layout", null];
  if (lower.startsWith("ppt/slidemasters/")) return ["master", null];
  if (lower.startsWith("ppt/notesslides/")) return ["notes", numbered];
  if (lower === "word/document.xml") return ["body", null];
  if (lower.startsWith("word/header")) return ["header", null];
  if (lower.startsWith("word/footer")) return ["footer", null];
  if (lower.startsWith("xl/drawings/")) return ["sheet", null];
  return null;
}

/**
 * 1.0.0 §2-1 "원본 고치기": edits inside the document's own file — a text is replaced where it stands, a
 * paragraph or a table row is added next to another (and looks like it) or taken out; a workbook's cell gets
 * a new value; a picture is replaced inside its frame. Font, size, tables, pictures and page setup stay, and
 * the result is saved as a new edition next to the original, which is not touched. The changes are shown
 * before anything is written.
 */
export function EditOriginalDialog({ open, onOpenChange, doc, initialFind }: { open: boolean; onOpenChange: (open: boolean) => void; doc: DocumentView; initialFind?: string }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const [rows, setRows] = useState<Row[]>([EMPTY]);
  const [report, setReport] = useState<EditReport | null>(null);
  const [busy, setBusy] = useState(false);
  // What was put right in the viewer (the page text) and is still wrong in the file.
  const [corrections, setCorrections] = useState<EditReplacement[]>([]);
  // An instruction in words: the model finds the places, the lines below show them.
  const [instruction, setInstruction] = useState("");
  const [suggesting, setSuggesting] = useState(false);
  // The pictures of the file, and the new picture file chosen for some of them (part → path).
  const [pictures, setPictures] = useState<EditPicture[]>([]);
  const [showPictures, setShowPictures] = useState(false);
  const [swaps, setSwaps] = useState<Record<string, string>>({});
  const actions = actionsFor(doc.source_kind);

  useEffect(() => {
    if (!open) return;
    // A selection in the viewer is what the user wants to change (one line of it).
    const first = (initialFind ?? "").split(/\r?\n/)[0]?.trim() ?? "";
    setRows([{ ...EMPTY, find: first }]);
    setReport(null);
    setCorrections([]);
    setInstruction("");
    setPictures([]);
    setShowPictures(false);
    setSwaps({});
    let stale = false;
    api
      .documentsPageTextHistory(doc.id, null)
      .then((history) => !stale && setCorrections(correctionsOf(history.entries)))
      .catch(() => undefined);
    api
      .documentsEditPictures(doc.id)
      .then((list) => !stale && setPictures(list))
      .catch(() => undefined);
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, doc.id]);

  // The pictures that get a new file, in the order of the list: they follow the rows in the request.
  const swapped = useMemo(() => pictures.filter((p) => (swaps[p.part] ?? "").trim() !== ""), [pictures, swaps]);
  const rowEdits: EditReplacement[] = useMemo(() => rows.filter((r) => r.find.trim() !== "").map(editOf), [rows]);
  const edits: EditReplacement[] = useMemo(
    () => [...rowEdits, ...swapped.map((p): EditReplacement => ({ find: p.part, replace: swaps[p.part]!.trim(), action: "replace_picture" }))],
    [rowEdits, swapped, swaps],
  );
  // Which request row each replacement of the report belongs to.
  const editRow = useMemo(() => rows.map((r, i) => (r.find.trim() !== "" ? i : -1)).filter((i) => i >= 0), [rows]);
  const total = report?.replaced.reduce((a, b) => a + b, 0) ?? 0;
  // An edit that cannot be made as asked: nothing is saved before it is settled.
  const problems = report?.problems ?? [];

  const change = (i: number, patch: Partial<Row>) => {
    setRows((list) => list.map((r, k) => (k === i ? { ...r, ...patch } : r)));
    setReport(null);
  };
  /** A new picture file for one picture of the document (an empty path: the field is shown, nothing is asked yet). */
  const swap = (part: string, path: string | null) => {
    setSwaps((all) => {
      const next = { ...all };
      if (path === null) delete next[part];
      else next[part] = path;
      return next;
    });
    setReport(null);
  };
  const pickPicture = async (part: string) => {
    // The browser has no file dialog that gives a path: the path is typed.
    if (!isTauri) return swap(part, swaps[part] ?? "");
    const picked = await pickPictureFile(t("viewer.edit_original.pictures.pick_title"), t("viewer.edit_original.pictures.filter_name"));
    if (picked) swap(part, picked);
  };
  /** The viewer's corrections become lines of the request (the ones that are not there yet). */
  const bringCorrections = () => {
    setRows((list) => {
      const kept = list.filter((r) => r.find.trim() !== "");
      const have = new Set(kept.filter((r) => r.action === "replace").map((r) => `${r.find}\u0000${r.replace}`));
      const fresh = corrections.filter((c) => !have.has(`${c.find}\u0000${c.replace}`)).map((c) => ({ ...EMPTY, find: c.find, replace: c.replace }));
      const next = [...kept, ...fresh].slice(0, MAX_ROWS);
      return next.length > 0 ? next : [EMPTY];
    });
    setReport(null);
  };
  /** The model's reading of the instruction becomes lines, and their preview is shown at once. */
  const suggest = async () => {
    if (!instruction.trim() || suggesting) return;
    setSuggesting(true);
    try {
      const s = await api.documentsEditSuggest(doc.id, instruction);
      if (s.dropped > 0) toast(t("viewer.edit_original.suggest_dropped", { count: s.dropped }), "warn");
      if (s.edits.length === 0) {
        toast(s.note || t("viewer.edit_original.suggest_none"), "warn");
        return;
      }
      const kept = rows.filter((r) => r.find.trim() !== "");
      const next = [...kept, ...s.edits.map((e) => ({ ...EMPTY, find: e.find, replace: e.replace, occurrence: e.occurrence != null ? String(e.occurrence) : "" }))].slice(0, MAX_ROWS);
      setRows(next);
      toast(t("viewer.edit_original.suggested", { count: s.edits.length }), "ok");
      setReport(await api.documentsEditPreview(doc.id, [...next.map(editOf), ...edits.slice(rowEdits.length)]));
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setSuggesting(false);
    }
  };
  const preview = async () => {
    setBusy(true);
    try {
      setReport(await api.documentsEditPreview(doc.id, edits));
    } catch (e) {
      setReport(null);
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  const save = async () => {
    setBusy(true);
    try {
      const edition = await api.documentsEditSave(doc.id, edits);
      const name = edition.rel_path.split("/").pop() ?? edition.rel_path;
      toast(t("viewer.edit_original.saved", { name, count: edition.report.replaced.reduce((a, b) => a + b, 0) }), "ok", {
        label: t("viewer.edit_original.open"),
        onClick: () => navigate(`/library/${edition.doc_id}`),
      });
      for (const w of edition.warnings) toast(w, "warn");
      onOpenChange(false);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl" data-testid="edit-original-dialog" closeLabel={t("common.close")}>
        <DialogHeader>
          <DialogTitle>{t("viewer.edit_original.title")}</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-fg-muted" data-testid="edit-original-lead">
          {t(doc.source_kind === "xlsx" ? "viewer.edit_original.lead_workbook" : "viewer.edit_original.lead")}
        </p>
        {/* A workbook is edited by its cells' addresses: the model is shown texts only, not where they stand. */}
        {doc.source_kind !== "xlsx" && (
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <Input
              className="min-w-40 flex-1"
              value={instruction}
              onChange={(e) => setInstruction(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.nativeEvent.isComposing) void suggest();
              }}
              placeholder={t("viewer.edit_original.instruction")}
              aria-label={t("viewer.edit_original.instruction")}
              data-testid="edit-original-instruction"
            />
            <Button size="sm" variant="outline" className="shrink-0" onClick={() => void suggest()} disabled={busy || suggesting || instruction.trim() === ""} title={t("viewer.edit_original.suggest_hint")} data-testid="edit-original-suggest" data-busy={suggesting || undefined}>
              <Sparkles />
              {t("viewer.edit_original.suggest")}
            </Button>
          </div>
        )}
        <div className="flex flex-col gap-2">
          {rows.map((r, i) => {
            const n = editRow.indexOf(i);
            const done = report && n >= 0 ? (report.replaced[n] ?? 0) : null;
            const found = report && n >= 0 ? (report.found[n] ?? 0) : 0;
            const problem = report && n >= 0 ? (problems.find((p) => p.edit === n)?.code ?? null) : null;
            return (
              <div key={i} className="flex min-w-0 flex-col gap-1" data-testid={`edit-original-row-${i}`} data-action={r.action}>
                <div className="flex min-w-0 flex-wrap items-center gap-2">
                  <Select value={r.action} onValueChange={(v) => change(i, { action: v as EditAction })}>
                    <SelectTrigger className="w-44 shrink-0" aria-label={t("viewer.edit_original.action")} data-testid={`edit-original-action-${i}`}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {actions.map((a) => (
                        <SelectItem key={a} value={a} data-testid={`edit-original-action-${i}-${a}`}>
                          {t(`viewer.edit_original.actions.${a}`)}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Input className="min-w-40 flex-1" value={r.find} onChange={(e) => change(i, { find: e.target.value })} placeholder={t(`viewer.edit_original.${placeLabel(r.action)}`)} aria-label={t(`viewer.edit_original.${placeLabel(r.action)}`)} data-testid={`edit-original-find-${i}`} />
                  {takesText(r.action) && (
                    <>
                      <ArrowRight className="size-4 shrink-0 text-fg-muted" aria-hidden />
                      <Input className="min-w-40 flex-1" value={r.replace} onChange={(e) => change(i, { replace: e.target.value })} placeholder={t(`viewer.edit_original.${textLabel(r.action)}`)} aria-label={t(`viewer.edit_original.${textLabel(r.action)}`)} data-testid={`edit-original-replace-${i}`} />
                    </>
                  )}
                  {r.action !== "set_cell" && (countsPlaces(r.action) || r.occurrence !== "") && (
                    <Input className="w-24 shrink-0" type="number" min={1} value={r.occurrence} onChange={(e) => change(i, { occurrence: e.target.value })} placeholder={t("viewer.edit_original.occurrence")} aria-label={t("viewer.edit_original.occurrence")} data-testid={`edit-original-occurrence-${i}`} />
                  )}
                  {done !== null && (
                    <Badge variant={problem ? "warn" : done > 0 ? "info" : "warn"} className="shrink-0" data-testid={`edit-original-count-${i}`}>
                      {problem ? t("viewer.edit_original.check") : done > 0 ? t("viewer.edit_original.places", { count: done }) : t("viewer.edit_original.not_found")}
                    </Badge>
                  )}
                  <Button size="icon-sm" variant="ghost" className="shrink-0" onClick={() => { setRows((list) => (list.length > 1 ? list.filter((_, k) => k !== i) : [EMPTY])); setReport(null); }} aria-label={t("viewer.edit_original.remove_row")} data-testid={`edit-original-remove-${i}`}>
                    <X />
                  </Button>
                </div>
                {problem && (
                  <p className="text-xs text-warn" role="alert" data-testid={`edit-original-problem-${i}`} data-code={problem}>
                    {problem === "edit_anchor_ambiguous" ? t("viewer.edit_original.ambiguous", { count: found }) : i18n.exists(`errors.reasons.${problem}`) ? t(`errors.reasons.${problem}`) : problem}
                  </p>
                )}
              </div>
            );
          })}
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <Button size="sm" variant="ghost" onClick={() => setRows((list) => [...list, EMPTY])} disabled={rows.length >= MAX_ROWS} data-testid="edit-original-add">
              <Plus />
              {t("viewer.edit_original.add_row")}
            </Button>
            {corrections.length > 0 && (
              <Button size="sm" variant="ghost" onClick={bringCorrections} title={t("viewer.edit_original.from_viewer_hint")} data-testid="edit-original-from-viewer" data-count={corrections.length}>
                <History />
                {t("viewer.edit_original.from_viewer", { count: corrections.length })}
              </Button>
            )}
            {pictures.length > 0 && (
              <Button size="sm" variant="ghost" onClick={() => setShowPictures((v) => !v)} aria-expanded={showPictures} title={t("viewer.edit_original.pictures.hint")} data-testid="edit-original-pictures-toggle" data-count={pictures.length}>
                {showPictures ? <ChevronDown /> : <ChevronRight />}
                <Images />
                {t("viewer.edit_original.pictures.toggle", { count: pictures.length })}
                {swapped.length > 0 && <Badge variant="info">{t("viewer.edit_original.pictures.chosen", { count: swapped.length })}</Badge>}
              </Button>
            )}
          </div>
          {showPictures && pictures.length > 0 && (
            <div className="flex min-h-0 flex-col gap-1" data-testid="edit-original-pictures">
              <p className="text-xs text-fg-muted">{t("viewer.edit_original.pictures.hint")}</p>
              <ul className="max-h-56 overflow-auto rounded-md border border-border text-sm">
                {pictures.map((p, i) => {
                  const path = swaps[p.part];
                  const n = report ? rowEdits.length + swapped.findIndex((x) => x.part === p.part) : -1;
                  const asked = report !== null && n >= rowEdits.length;
                  const problem = asked ? (problems.find((x) => x.edit === n)?.code ?? null) : null;
                  const where = [...new Set(p.used_in.map(shownIn).filter((x): x is [string, number | null] => x !== null).map(([key, num]) => t(`viewer.edit_original.pictures.where.${key}`, { n: num ?? "" }).trim()))];
                  const size = p.width > 0 ? t("viewer.edit_original.pictures.size", { width: p.width, height: p.height, format: p.format.toUpperCase() }) : p.format.toUpperCase();
                  return (
                    <li key={p.part} className="flex min-w-0 flex-col gap-1 border-b border-border px-2 py-1.5 last:border-b-0" data-testid={`edit-original-picture-${i}`} data-part={p.part}>
                      <div className="flex min-w-0 flex-wrap items-center gap-2">
                        <div className="flex size-12 shrink-0 items-center justify-center overflow-hidden rounded border border-border bg-bg-sunken">
                          {p.thumbnail ? <img src={p.thumbnail} alt="" className="max-h-full max-w-full object-contain" /> : <ImageOff className="size-4 text-fg-subtle" aria-hidden />}
                        </div>
                        <div className="min-w-32 flex-1">
                          <div className="truncate" title={p.part}>
                            {fileName(p.part)}
                          </div>
                          <div className="truncate text-xs text-fg-muted">{[size, ...where].join(" · ")}</div>
                        </div>
                        {!p.replaceable ? (
                          <Badge variant="outline" className="shrink-0" title={t("errors.reasons.edit_picture_format")} data-testid={`edit-original-picture-fixed-${i}`}>
                            {t("viewer.edit_original.pictures.fixed")}
                          </Badge>
                        ) : path === undefined ? (
                          <Button size="sm" variant="outline" className="shrink-0" onClick={() => void pickPicture(p.part)} data-testid={`edit-original-picture-pick-${i}`}>
                            {t("viewer.edit_original.pictures.pick")}
                          </Button>
                        ) : (
                          <div className="flex min-w-48 flex-1 items-center gap-1">
                            <Input className="min-w-0 flex-1" value={path} onChange={(e) => swap(p.part, e.target.value)} placeholder={t("viewer.edit_original.pictures.path")} aria-label={t("viewer.edit_original.pictures.path")} title={path} data-testid={`edit-original-picture-path-${i}`} />
                            {isTauri && (
                              <Button size="sm" variant="outline" className="shrink-0" onClick={() => void pickPicture(p.part)} data-testid={`edit-original-picture-pick-${i}`}>
                                {t("viewer.edit_original.pictures.pick")}
                              </Button>
                            )}
                            {asked && !problem && (
                              <Badge variant="info" className="shrink-0" data-testid={`edit-original-picture-state-${i}`}>
                                {t("viewer.edit_original.pictures.replaced")}
                              </Badge>
                            )}
                            <Button size="icon-sm" variant="ghost" className="shrink-0" onClick={() => swap(p.part, null)} aria-label={t("viewer.edit_original.pictures.clear")} title={t("viewer.edit_original.pictures.clear")} data-testid={`edit-original-picture-clear-${i}`}>
                              <X />
                            </Button>
                          </div>
                        )}
                      </div>
                      {problem && (
                        <p className="text-xs text-warn" role="alert" data-testid={`edit-original-picture-problem-${i}`} data-code={problem}>
                          {i18n.exists(`errors.reasons.${problem}`) ? t(`errors.reasons.${problem}`) : problem}
                        </p>
                      )}
                    </li>
                  );
                })}
              </ul>
            </div>
          )}
        </div>
        {report && (
          <div className="flex min-h-0 flex-col gap-1" data-testid="edit-original-preview" data-total={total} data-problems={problems.length}>
            <div className="text-sm font-medium">{total > 0 ? t("viewer.edit_original.summary", { count: total }) : t("viewer.edit_original.nothing")}</div>
            {(report.notes ?? []).map((note, i) => (
              <p key={i} className="text-xs text-warn" data-testid={`edit-original-note-${i}`} data-code={note.code}>
                {i18n.exists(`viewer.edit_original.notes.${note.code}`) ? t(`viewer.edit_original.notes.${note.code}`, { count: note.count, name: fileName(edits[note.edit]?.find ?? "") }) : note.code}
              </p>
            ))}
            {report.changes.length > 0 && (
              <ul className="max-h-64 overflow-auto rounded-md border border-border text-sm">
                {report.changes.map((c, i) => {
                  const action = c.action ?? "replace";
                  // One thing takes the place of another (a text, a cell's value, a picture); or something is added / taken out.
                  const exchanged = action === "replace" || action === "set_cell" || action === "replace_picture";
                  const where = action === "replace_picture" ? fileName(c.part) : c.unit != null ? t(c.part.startsWith("ppt/") ? "viewer.edit_original.slide" : "viewer.edit_original.section", { n: c.unit }) : null;
                  return (
                    <li key={i} className="flex min-w-0 flex-col gap-0.5 border-b border-border px-2 py-1.5 last:border-b-0" data-testid={`edit-original-change-${i}`} data-action={action}>
                      {(where || action !== "replace") && <span className="text-xs text-fg-muted">{[where, action !== "replace" ? t(`viewer.edit_original.change.${action}`) : null].filter(Boolean).join(" · ")}</span>}
                      {exchanged || action.startsWith("delete") ? (
                        <span className="break-words text-fg-muted line-through decoration-danger/60">{c.before}</span>
                      ) : (
                        // The paragraph / row the new one stands next to: it stays as it is.
                        <span className="break-words text-fg-muted">{c.before}</span>
                      )}
                      {!action.startsWith("delete") && <span className="break-words">{exchanged ? c.after : `+ ${c.after || t("viewer.edit_original.blank")}`}</span>}
                    </li>
                  );
                })}
              </ul>
            )}
            {report.changes.length < total && <div className="text-xs text-fg-muted">{t("viewer.edit_original.more", { shown: report.changes.length })}</div>}
          </div>
        )}
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button variant="outline" onClick={() => void preview()} disabled={busy || edits.length === 0} data-testid="edit-original-preview-run">
            {t("viewer.edit_original.preview")}
          </Button>
          <Button onClick={() => void save()} disabled={busy || !report || total === 0 || problems.length > 0} title={!report ? t("viewer.edit_original.preview_first") : undefined} data-testid="edit-original-save">
            {t("viewer.edit_original.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
