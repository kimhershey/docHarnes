import { Pencil, Save, Sparkles, Tag, X } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Textarea } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { Snippet } from "@/features/search/SearchPage";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useFields } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import { isComposing } from "@/lib/shortcuts";
import type { DocumentView, PageView } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

/** Character offsets of the current selection inside `container` (null when the selection is elsewhere or empty). */
export function selectionOffsets(container: HTMLElement): [number, number] | null {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null;
  const range = sel.getRangeAt(0);
  if (!container.contains(range.startContainer) || !container.contains(range.endContainer)) return null;
  const before = document.createRange();
  before.selectNodeContents(container);
  before.setEnd(range.startContainer, range.startOffset);
  const start = before.toString().length;
  return [start, start + range.toString().length];
}

/**
 * Viewer text tab (design §3.7 "직접 수정"): the page text with search highlights, a selection
 * toolbar (edit in place, assign the selection to an extracted field, explain) and an edit mode that
 * saves to the page's text (`text_source = manual`) so search, questions and extraction follow.
 */
export function PageTextPanel({
  doc,
  page,
  text,
  loading,
  highlights,
  textRef,
  selectionText,
  onExplain,
  editRequest,
  onEditRequestHandled,
}: {
  doc: DocumentView;
  page: PageView | undefined;
  text: string | null | undefined;
  loading: boolean;
  highlights: [number, number][];
  textRef: RefObject<HTMLPreElement | null>;
  selectionText: string;
  onExplain: (selection: string) => void;
  /** The viewer asks for edit mode (Enter / F2 on a selection); the offsets select that range. */
  editRequest: { offsets: [number, number] | null; seq: number } | null;
  onEditRequestHandled: () => void;
}) {
  const { t, i18n } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const pendingRange = useRef<[number, number] | null>(null);
  const areaRef = useRef<HTMLTextAreaElement | null>(null);
  const { data: fields } = useFields(doc.id);
  const pageNo = page?.page_no ?? 1;
  const lang = i18n.language.startsWith("ko") ? "ko" : "en";

  // Leaving the page or the document drops an unsaved draft.
  useEffect(() => {
    setEditing(false);
    setDraft("");
  }, [doc.id, pageNo]);

  const startEdit = (range: [number, number] | null) => {
    pendingRange.current = range;
    setDraft(text ?? "");
    setEditing(true);
  };
  useEffect(() => {
    if (editing && areaRef.current) {
      const el = areaRef.current;
      el.focus();
      const r = pendingRange.current;
      if (r) el.setSelectionRange(r[0], r[1]);
      else el.setSelectionRange(el.value.length, el.value.length);
      pendingRange.current = null;
    }
  }, [editing]);
  useEffect(() => {
    if (!editRequest || editing) return;
    startEdit(editRequest.offsets);
    onEditRequestHandled();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editRequest?.seq]);

  const save = async () => {
    if (busy) return;
    if (draft.trimEnd() === (text ?? "").trimEnd()) {
      toast(t("viewer.text_edit.unchanged"), "info");
      setEditing(false);
      return;
    }
    setBusy(true);
    try {
      await api.documentsSetPageText(doc.id, pageNo, draft);
      await queryClient.invalidateQueries({ queryKey: queryKeys.documentPageText(doc.id, pageNo) });
      await queryClient.invalidateQueries({ queryKey: queryKeys.documentPages(doc.id, false) });
      toast(t("viewer.text_edit.saved", { page: pageNo }), "ok");
      setEditing(false);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };

  const setField = async (fieldId: number, label: string) => {
    const value = selectionText.trim();
    if (!value) return;
    try {
      await api.pipelineReviewApply([fieldId], { kind: "correct", value });
      toast(t("viewer.text_edit.field_set", { label }), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const source = page?.text_source ?? null;
  const fieldList = (fields ?? []).filter((f) => f.status !== "rejected");

  if (loading) return <Spinner />;

  if (editing) {
    return (
      <div className="flex h-full flex-col gap-2" data-testid="viewer-text-editor">
        <p className="text-xs text-fg-muted">{t("viewer.text_edit.editing_hint")}</p>
        <Textarea
          ref={areaRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          className="min-h-64 flex-1 resize-none font-sans text-sm"
          placeholder={t("viewer.text_edit.placeholder")}
          lang={i18n.language}
          aria-label={t("viewer.text_edit.edit")}
          data-testid="viewer-text-input"
          onKeyDown={(e) => {
            if (isComposing(e)) return;
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              void save();
            } else if (e.key === "Escape") {
              e.preventDefault();
              setEditing(false);
            }
          }}
        />
        <div className="flex items-center gap-2">
          <Button size="sm" onClick={() => void save()} disabled={busy} data-testid="viewer-text-save">
            <Save />
            {t("common.save")}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setEditing(false)} disabled={busy} data-testid="viewer-text-cancel">
            {t("common.cancel")}
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-1.5 text-xs text-fg-muted">
        {source && (
          <Badge variant={source === "manual" ? "info" : "outline"} data-testid="viewer-text-source">
            {t(`viewer.text_edit.source.${source}`, { defaultValue: source })}
          </Badge>
        )}
        <span className="truncate">{t("viewer.text_edit.hint")}</span>
        <Button size="sm" variant="outline" className="ml-auto h-7" onClick={() => startEdit(textRef.current ? selectionOffsets(textRef.current) : null)} data-testid="viewer-text-edit">
          <Pencil />
          {text ? t("viewer.text_edit.edit") : t("viewer.text_edit.type_in")}
        </Button>
      </div>
      {selectionText && (
        <div className="sticky top-0 z-10 flex flex-wrap items-center gap-1.5 rounded-md border border-accent/40 bg-accent-soft px-2 py-1.5 text-xs" data-testid="viewer-selection-toolbar">
          <span className="min-w-0 flex-1 truncate" title={selectionText}>
            {t("viewer.text_edit.selection", { text: selectionText.length > 40 ? `${selectionText.slice(0, 40)}…` : selectionText })}
          </span>
          <Button size="sm" variant="secondary" className="h-6 px-2" onClick={() => startEdit(textRef.current ? selectionOffsets(textRef.current) : null)} data-testid="viewer-selection-edit">
            <Pencil />
            {t("viewer.text_edit.edit")}
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="sm" variant="secondary" className="h-6 px-2" data-testid="viewer-selection-field">
                <Tag />
                {t("viewer.text_edit.set_field")}
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="max-h-72 overflow-y-auto">
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
          <Button size="sm" variant="ghost" className="h-6 px-2" onClick={() => onExplain(selectionText)} data-testid="viewer-selection-explain">
            <Sparkles />
            {t("viewer.explain")}
          </Button>
          <Button size="icon-sm" variant="ghost" className="h-6" onClick={() => window.getSelection()?.removeAllRanges()} aria-label={t("common.close")}>
            <X />
          </Button>
        </div>
      )}
      {text ? (
        <pre ref={textRef} className="whitespace-pre-wrap break-words font-sans text-sm selectable" data-testid="viewer-text">
          {highlights.length > 0 ? <Snippet text={text} highlights={highlights} /> : text}
        </pre>
      ) : (
        <p className="text-sm text-fg-muted" data-testid="viewer-text-empty">
          {t("viewer.text_empty")}
        </p>
      )}
    </div>
  );
}
