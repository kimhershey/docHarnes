import { Copy, Pencil, Plus, Sparkles, Tag, X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { queryClient, queryKeys } from "@/lib/query-client";
import { isComposing } from "@/lib/shortcuts";
import type { DocumentView, FieldRecord, PageLayout, PageView } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

import { bbox, fitFontSize, isContiguous, range, selectionText, wordAt, wordsInRect } from "./textLayer";

type Rect = [number, number, number, number];

/** Pointer movement (px) before a press on empty space becomes a rubber-band selection. */
const DRAG_PX = 4;
const EMPTY_LAYOUT: PageLayout = { version: 2, source: "estimated", words: [] };

/**
 * The text layer drawn over the page image (design §3.7 "직접 수정", 2026-11-09): every recognised word
 * sits in a transparent box at its position. Drag across words (or click / Shift+click) to select
 * them, drag on empty space to rubber-band; the floating toolbar edits the words in place, assigns
 * the selection to an extracted field, explains it or copies it; a box drawn where nothing was
 * recognised takes new text. Edits go to `documents_edit_page_words` / `documents_add_page_words`.
 */
export function PageTextLayer({
  doc,
  page,
  layout,
  hits,
  selected,
  onSelect,
  fields,
  onExplain,
  editSeq,
  onEditorClose,
}: {
  doc: DocumentView;
  page: PageView;
  layout: PageLayout | null | undefined;
  /** Word indices highlighted by a search / citation. */
  hits: Set<number>;
  /** Selected word indices (sorted, reading order). */
  selected: number[];
  onSelect: (indices: number[]) => void;
  fields: FieldRecord[];
  onExplain: (text: string) => void;
  /** Bumped by the viewer when Enter / F2 asks to edit the current selection. */
  editSeq: number;
  /** The inline editor (correct / add words) closed — saved, cancelled or dropped by a page change. */
  onEditorClose?: () => void;
}) {
  const { t, i18n } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const words = layout ?? EMPTY_LAYOUT;
  const rootRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const [size, setSize] = useState<{ w: number; h: number }>({ w: 0, h: 0 });
  const [band, setBand] = useState<Rect | null>(null);
  const [editing, setEditing] = useState<{ start: number; end: number } | null>(null);
  const [adding, setAdding] = useState<Rect | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const drag = useRef<{ x: number; y: number; anchor: number | null; moved: boolean; base: number[] } | null>(null);
  const lang = i18n.language.startsWith("ko") ? "ko" : "en";

  // The layer's rendered size drives the word font sizes (fractions → px).
  useLayoutEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const measure = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // A new page / document drops any open editor.
  useEffect(() => {
    setEditing(null);
    setAdding(null);
    setBand(null);
    drag.current = null;
  }, [doc.id, page.page_no]);

  const startEdit = (indices: number[]) => {
    if (!isContiguous(indices)) return;
    const start = indices[0]!;
    const end = indices[indices.length - 1]! + 1;
    setAdding(null);
    setDraft(selectionText(words, indices).replace(/\n/g, " "));
    setEditing({ start, end });
  };
  useEffect(() => {
    if (editSeq > 0 && selected.length > 0) startEdit(selected);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editSeq]);
  useEffect(() => {
    if ((editing || adding) && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editing, adding]);
  // The editor's input holds the focus while it is open. When it closes the input is gone and the focus would
  // fall to <body>, where the viewer's keys are not heard — Ctrl+Z for the correction just saved reached the
  // global file-journal undo instead (DOC-2). The viewer takes the focus back.
  const editorOpen = editing !== null || adding !== null;
  const editorWasOpen = useRef(false);
  useEffect(() => {
    if (editorWasOpen.current && !editorOpen) onEditorClose?.();
    editorWasOpen.current = editorOpen;
  }, [editorOpen, onEditorClose]);

  const point = (e: ReactPointerEvent): [number, number] => {
    const r = rootRef.current?.getBoundingClientRect();
    if (!r || r.width === 0 || r.height === 0) return [0, 0];
    return [Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)), Math.min(1, Math.max(0, (e.clientY - r.top) / r.height))];
  };

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || editing || adding) return;
    if ((e.target as HTMLElement).closest("[data-layer-ui]")) return;
    const target = (e.target as HTMLElement).closest<HTMLElement>("[data-word-index]");
    const anchor = target ? Number(target.dataset.wordIndex) : null;
    const [x, y] = point(e);
    drag.current = { x, y, anchor, moved: false, base: selected };
    e.currentTarget.setPointerCapture(e.pointerId);
    if (anchor !== null) {
      if (e.shiftKey && selected.length > 0) onSelect(range(Math.min(selected[0]!, anchor), Math.max(selected[selected.length - 1]!, anchor)));
      else onSelect([anchor]);
    }
    // The layer takes focus (it is inside the viewer root), so Enter / Escape / arrows keep working
    // after a click on the page; native selection is off (`select-none`) and the image is not draggable.
    e.currentTarget.focus();
  };
  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    const [x, y] = point(e);
    const r = rootRef.current?.getBoundingClientRect();
    const px = r ? Math.hypot((x - d.x) * r.width, (y - d.y) * r.height) : 0;
    if (!d.moved && px < DRAG_PX) return;
    d.moved = true;
    if (d.anchor !== null) {
      const focus = wordAt(words, x, y);
      if (focus !== null) onSelect(range(d.anchor, focus));
    } else {
      const rect: Rect = [Math.min(d.x, x), Math.min(d.y, y), Math.abs(x - d.x), Math.abs(y - d.y)];
      setBand(rect);
      onSelect(wordsInRect(words, rect));
    }
  };
  const onPointerUp = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    const rect = band;
    setBand(null);
    if (d.anchor === null) {
      if (!d.moved) {
        onSelect([]);
        return;
      }
      // A box drawn where nothing was recognised takes new text.
      if (rect && selected.length === 0 && rect[2] > 0.02 && rect[3] > 0.004) {
        setDraft("");
        setAdding(rect);
      }
    }
  };
  const onDoubleClick = (e: React.MouseEvent<HTMLDivElement>) => {
    const target = (e.target as HTMLElement).closest<HTMLElement>("[data-word-index]");
    if (!target) return;
    const i = Number(target.dataset.wordIndex);
    onSelect([i]);
    startEdit([i]);
  };

  const refresh = (next: PageLayout) => {
    queryClient.setQueryData(queryKeys.documentPageLayout(doc.id, page.page_no), next);
    void queryClient.invalidateQueries({ queryKey: queryKeys.documentPageText(doc.id, page.page_no) });
    void queryClient.invalidateQueries({ queryKey: queryKeys.documentPages(doc.id, false) });
  };

  const save = async () => {
    if (busy || !editing) return;
    const before = words.words.slice(editing.start, editing.end).map((w) => w.t).join(" ");
    if (draft.trim() === before) {
      toast(t("viewer.text_edit.unchanged"), "info");
      setEditing(null);
      return;
    }
    setBusy(true);
    try {
      const next = await api.documentsEditPageWords(doc.id, page.page_no, editing.start, editing.end, draft);
      refresh(next);
      toast(t("viewer.text_edit.saved", { page: page.page_no }), "ok");
      setEditing(null);
      onSelect([]);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  const saveAdded = async () => {
    if (busy || !adding) return;
    if (!draft.trim()) {
      setAdding(null);
      return;
    }
    setBusy(true);
    try {
      const next = await api.documentsAddPageWords(doc.id, page.page_no, adding, draft);
      refresh(next);
      toast(t("viewer.text_edit.saved", { page: page.page_no }), "ok");
      setAdding(null);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };

  const text = selectionText(words, selected);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      toast(t("viewer.layer.copied"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const setField = async (fieldId: number, label: string) => {
    const value = text.replace(/\s+/g, " ").trim();
    if (!value) return;
    try {
      await api.pipelineReviewApply([fieldId], { kind: "correct", value });
      void queryClient.invalidateQueries({ queryKey: queryKeys.fields(doc.id, null) });
      toast(t("viewer.text_edit.field_set", { label }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const fieldList = fields.filter((f) => f.status !== "rejected");
  const selBox = selected.length > 0 ? bbox(words, selected) : null;
  const editBox = editing ? bbox(words, range(editing.start, editing.end - 1)) : null;
  const px = (r: Rect): CSSProperties => ({ left: `${r[0] * 100}%`, top: `${r[1] * 100}%`, width: `${r[2] * 100}%`, height: `${r[3] * 100}%` });
  // The toolbar sits above the selection, or below it near the top edge; it never leaves the page.
  const toolbarStyle = (r: Rect): CSSProperties => {
    const top = r[1] * size.h;
    const above = top > 40;
    return { left: Math.max(0, Math.min(r[0] * size.w, size.w - 420)), top: above ? top - 36 : (r[1] + r[3]) * size.h + 6 };
  };
  const inputStyle = (r: Rect): CSSProperties => ({
    left: `${r[0] * 100}%`,
    top: `${r[1] * 100}%`,
    width: Math.max(160, r[2] * size.w + 24),
    height: Math.max(28, r[3] * size.h + 6),
    fontSize: Math.max(12, Math.min(28, r[3] * size.h * 0.7)),
  });

  // Separator text nodes keep `textContent` readable ("단어 단어\n…") for assistive tech and tests;
  // highlighted words render as <mark>.
  const wordNodes: ReactNode[] = [];
  words.words.forEach((w, i) => {
    const isSelected = selected.includes(i);
    const isHit = hits.has(i);
    const Box = isHit ? "mark" : "span";
    // Recognised words are transparent (the image shows the glyphs); a corrected word is drawn
    // visibly over the old glyphs so the change is seen on the page (the image itself never changes).
    const visible = w.edited === true;
    wordNodes.push(
      <Box
        key={i}
        data-word-index={i}
        data-selected={isSelected || undefined}
        data-edited={w.edited || undefined}
        data-fig={w.fig || undefined}
        className={cn(
          "absolute overflow-hidden whitespace-nowrap rounded-[2px] leading-none",
          visible ? "flex items-center text-[#0b6a8a]" : "text-transparent",
          isHit ? "bg-warn-soft/80 ring-1 ring-warn/60" : isSelected ? "bg-accent/35 ring-1 ring-accent" : visible ? "bg-white ring-1 ring-[#0b6a8a]/40" : "hover:bg-accent/15",
          // A word read from a picture: its box is an estimate, shown as such when pointed at.
          w.fig && !visible && !isHit && !isSelected && "hover:outline-dashed hover:outline-1 hover:outline-accent",
        )}
        style={{
          ...px([w.x, w.y, w.w, w.h]),
          fontSize: visible ? fitFontSize(w.t, w.w * size.w, w.h * size.h) : Math.max(6, w.h * size.h * 0.78),
        }}
        title={visible ? t("viewer.text_edit.edited_title") : w.fig ? t("viewer.text_edit.figure_title") : undefined}
      >
        {w.t}
      </Box>,
    );
    const next = words.words[i + 1];
    if (next) wordNodes.push(next.line === w.line ? " " : "\n");
  });

  return (
    <div
      ref={rootRef}
      tabIndex={-1}
      className="absolute inset-0 select-none text-[0px] text-transparent outline-none"
      style={{ touchAction: "none", cursor: "text" }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDoubleClick={onDoubleClick}
      data-testid="viewer-text-layer"
      data-layer-source={layout?.source ?? "none"}
      data-word-count={words.words.length}
      aria-label={t("viewer.layer.aria")}
    >
      {wordNodes}
      {band && <div className="pointer-events-none absolute border border-accent bg-accent/10" style={px(band)} data-testid="viewer-layer-band" />}
      {selBox && !editing && !adding && (
        <div
          data-layer-ui
          className="absolute z-20 flex max-w-[26.25rem] items-center gap-1 rounded-md border border-border bg-bg-elevated px-1.5 py-1 text-xs text-fg shadow-[var(--shadow)]"
          style={toolbarStyle(selBox)}
          data-testid="viewer-selection-toolbar"
          onPointerDown={(e) => e.stopPropagation()}
        >
          <span className="min-w-0 max-w-40 flex-1 truncate" title={text}>
            {t("viewer.text_edit.selection", { text: text.replace(/\n/g, " ") })}
          </span>
          <Button size="sm" variant="secondary" className="h-6 px-2 text-xs" onClick={() => startEdit(selected)} disabled={!isContiguous(selected)} data-testid="viewer-selection-edit">
            <Pencil />
            {t("viewer.text_edit.edit")}
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="sm" variant="secondary" className="h-6 px-2 text-xs" data-testid="viewer-selection-field">
                <Tag />
                {t("viewer.text_edit.set_field")}
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="max-h-72 overflow-y-auto text-sm">
              {fieldList.length === 0 ? (
                <DropdownMenuLabel className="text-fg-muted">{t("viewer.text_edit.set_field_none")}</DropdownMenuLabel>
              ) : (
                fieldList.map((f) => {
                  const label = lang === "ko" ? f.label_ko : f.label_en;
                  return (
                    <DropdownMenuItem key={f.id} onSelect={() => void setField(f.id, label)} data-testid={`viewer-selection-field-${f.id}`}>
                      <span className="min-w-0 flex-1 truncate">{label}</span>
                      <span className="ml-2 max-w-32 truncate text-xs text-fg-subtle">{f.value ?? "—"}</span>
                    </DropdownMenuItem>
                  );
                })
              )}
            </DropdownMenuContent>
          </DropdownMenu>
          <Button size="sm" variant="ghost" className="h-6 px-2 text-xs" onClick={() => onExplain(text)} data-testid="viewer-selection-explain">
            <Sparkles />
            {t("viewer.explain")}
          </Button>
          <Button size="icon-sm" variant="ghost" className="h-6 w-6" onClick={() => void copy()} aria-label={t("viewer.layer.copy")} title={t("viewer.layer.copy")} data-testid="viewer-selection-copy">
            <Copy />
          </Button>
          <Button size="icon-sm" variant="ghost" className="h-6 w-6" onClick={() => onSelect([])} aria-label={t("common.close")}>
            <X />
          </Button>
        </div>
      )}
      {editing && editBox && (
        <div data-layer-ui className="absolute z-30" style={inputStyle(editBox)} onPointerDown={(e) => e.stopPropagation()} data-testid="viewer-text-editor">
          <input
            ref={inputRef}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            className="h-full w-full rounded border border-accent bg-bg px-1.5 font-sans text-fg shadow outline-none"
            style={{ fontSize: "inherit" }}
            lang={i18n.language}
            aria-label={t("viewer.text_edit.edit")}
            data-testid="viewer-text-input"
            disabled={busy}
            onKeyDown={(e) => {
              if (isComposing(e)) return;
              if (e.key === "Enter") {
                e.preventDefault();
                void save();
              } else if (e.key === "Escape") {
                e.preventDefault();
                setEditing(null);
              }
              e.stopPropagation();
            }}
          />
          <div className="mt-1 flex items-center gap-1 text-xs text-fg">
            <Button size="sm" className="h-6 px-2 text-xs" onClick={() => void save()} disabled={busy} data-testid="viewer-text-save">
              {t("common.save")}
            </Button>
            <Button size="sm" variant="ghost" className="h-6 px-2 text-xs" onClick={() => setEditing(null)} disabled={busy} data-testid="viewer-text-cancel">
              {t("common.cancel")}
            </Button>
            <span className="rounded bg-bg-elevated px-1 text-fg-muted shadow">{t("viewer.layer.edit_hint")}</span>
          </div>
        </div>
      )}
      {adding && (
        <div data-layer-ui className="absolute z-30" style={inputStyle(adding)} onPointerDown={(e) => e.stopPropagation()} data-testid="viewer-text-add">
          <input
            ref={inputRef}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder={t("viewer.layer.add_placeholder")}
            className="h-full w-full rounded border border-dashed border-accent bg-bg px-1.5 font-sans text-fg shadow outline-none placeholder:text-fg-subtle"
            style={{ fontSize: "inherit" }}
            lang={i18n.language}
            aria-label={t("viewer.layer.add_text")}
            data-testid="viewer-text-add-input"
            disabled={busy}
            onKeyDown={(e) => {
              if (isComposing(e)) return;
              if (e.key === "Enter") {
                e.preventDefault();
                void saveAdded();
              } else if (e.key === "Escape") {
                e.preventDefault();
                setAdding(null);
              }
              e.stopPropagation();
            }}
          />
          <div className="mt-1 flex items-center gap-1 text-xs text-fg">
            <Button size="sm" className="h-6 px-2 text-xs" onClick={() => void saveAdded()} disabled={busy || !draft.trim()} data-testid="viewer-text-add-save">
              <Plus />
              {t("viewer.layer.add_text")}
            </Button>
            <Button size="sm" variant="ghost" className="h-6 px-2 text-xs" onClick={() => setAdding(null)} disabled={busy}>
              {t("common.cancel")}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
