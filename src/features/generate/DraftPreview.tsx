import { FilePlus2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { api } from "@/lib/ipc";
import type { Draft } from "@/lib/types";

import { hasBlockingIssues } from "./draft";

/**
 * Layout width of the preview document in its own CSS pixels: the renderer's A4 sheet (210 mm ≈ 794 px) plus
 * its margins. The document is paper, not UI, so it is sized in px and scaled as a whole to the column
 * ("맞춤"); the UI 배율 does not change the paper.
 */
const SHEET_WIDTH_PX = 860;

/**
 * Centre column: the HTML preview the HWPX / DOCX / PDF writers share, rendered in a fully sandboxed
 * iframe (`sandbox=""`: no scripts, no navigation, no same-origin access). Re-rendered 300 ms after the
 * last edit. 0.7.0 (plan §1.3): the sheet is scaled to the column width by default, so the narrow centre
 * column no longer grows horizontal scrollbars inside the frame; "100%" shows it at its real size and
 * scrolls instead.
 */
export function DraftPreview({ draft }: { draft: Draft | null }) {
  const { t } = useTranslation();
  const [html, setHtml] = useState<string>("");
  const [loading, setLoading] = useState(false);
  const [fit, setFit] = useState(true);
  const [box, setBox] = useState<{ w: number; h: number } | null>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const seq = useRef(0);
  useEffect(() => {
    if (!draft) {
      setHtml("");
      return;
    }
    const mine = ++seq.current;
    setLoading(true);
    const timer = window.setTimeout(() => {
      void api
        .generatePreview(draft)
        .then((p) => {
          if (seq.current === mine) setHtml(p.html);
        })
        .catch(() => undefined)
        .finally(() => {
          if (seq.current === mine) setLoading(false);
        });
    }, 300);
    return () => window.clearTimeout(timer);
  }, [draft]);

  // The frame's box is measured (ResizeObserver) so the sheet can be scaled to it; measured, never fixed.
  const hasDraft = draft !== null;
  useEffect(() => {
    const el = boxRef.current;
    if (!el || !hasDraft) return;
    const update = () => setBox({ w: el.clientWidth, h: el.clientHeight });
    update();
    const observer = new ResizeObserver(update);
    observer.observe(el);
    return () => observer.disconnect();
  }, [hasDraft]);

  if (!draft) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-8 text-center text-fg-muted" data-testid="generate-empty">
        <FilePlus2 className="size-8 text-accent" aria-hidden="true" />
        <p className="font-medium text-fg">{t("generate.preview.empty_title")}</p>
        <p className="max-w-md text-sm">{t("generate.preview.empty_body")}</p>
      </div>
    );
  }
  const scale = fit && box && box.w > 0 ? Math.min(1, box.w / SHEET_WIDTH_PX) : 1;
  const frameStyle = fit ? { width: SHEET_WIDTH_PX, height: box ? Math.max(1, Math.round(box.h / scale)) : undefined, transform: `scale(${scale})`, transformOrigin: "top left" } : { width: SHEET_WIDTH_PX, minHeight: "100%" };
  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden" data-testid="generate-preview">
      <div className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-2 text-xs">
        <span className="font-medium">{t("generate.preview.title")}</span>
        <Badge variant={hasBlockingIssues(draft) ? "warn" : "ok"} data-testid="generate-review-badge">
          {hasBlockingIssues(draft) ? t("generate.preview.unreviewed") : t("generate.preview.validated")}
        </Badge>
        {draft.issues.length > 0 && <Badge variant="muted">{t("generate.preview.issues_badge", { count: draft.issues.length })}</Badge>}
        {draft.model && <span className="text-fg-subtle">{draft.model}</span>}
        <span className="ml-auto flex items-center gap-2">
          {loading && (
            <span className="flex items-center gap-1 text-fg-subtle" role="status">
              <Spinner className="size-3.5" />
              {t("generate.preview.loading")}
            </span>
          )}
          <Button size="sm" variant="outline" className="h-6 px-2 text-[0.6875rem]" onClick={() => setFit((f) => !f)} aria-label={t("generate.preview.zoom_label")} title={t("generate.preview.zoom_label")} data-testid="generate-preview-zoom" data-zoom={fit ? "fit" : "full"}>
            {fit ? t("generate.preview.zoom_fit") : t("generate.preview.zoom_full")}
          </Button>
        </span>
      </div>
      <div className="min-h-0 min-w-0 flex-1 overflow-hidden bg-bg-sunken p-3">
        <div ref={boxRef} className={fit ? "h-full w-full overflow-hidden" : "h-full w-full overflow-auto"} data-testid="generate-preview-box" data-scale={scale.toFixed(2)}>
          <iframe title={t("generate.preview.frame_title")} sandbox="" srcDoc={html} style={frameStyle} className={fit ? "max-w-none rounded-md border border-border bg-white" : "h-full max-w-none rounded-md border border-border bg-white"} data-testid="generate-preview-frame" data-loaded={html ? "true" : "false"} />
        </div>
      </div>
      {draft.notes.length > 0 && (
        <ul className="max-h-32 shrink-0 overflow-y-auto border-t border-border px-3 py-1.5 text-[0.6875rem] text-fg-subtle" data-testid="generate-notes">
          {draft.notes.map((n, i) => (
            <li key={i}>{n}</li>
          ))}
        </ul>
      )}
    </div>
  );
}
