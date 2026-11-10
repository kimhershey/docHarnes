import { FilePlus2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Spinner } from "@/components/ui/spinner";
import { api } from "@/lib/ipc";
import type { Draft } from "@/lib/types";

import { hasBlockingIssues } from "./draft";

/**
 * Centre column: the HTML preview the HWPX / DOCX / PDF writers share, rendered in a fully sandboxed
 * iframe (`sandbox=""`: no scripts, no navigation, no same-origin access). Re-rendered 300 ms after the
 * last edit.
 */
export function DraftPreview({ draft }: { draft: Draft | null }) {
  const { t } = useTranslation();
  const [html, setHtml] = useState<string>("");
  const [loading, setLoading] = useState(false);
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

  if (!draft) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-8 text-center text-fg-muted" data-testid="generate-empty">
        <FilePlus2 className="size-8 text-accent" aria-hidden="true" />
        <p className="font-medium text-fg">{t("generate.preview.empty_title")}</p>
        <p className="max-w-md text-sm">{t("generate.preview.empty_body")}</p>
      </div>
    );
  }
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="generate-preview">
      <div className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-2 text-xs">
        <span className="font-medium">{t("generate.preview.title")}</span>
        <Badge variant={hasBlockingIssues(draft) ? "warn" : "ok"} data-testid="generate-review-badge">
          {hasBlockingIssues(draft) ? t("generate.preview.unreviewed") : t("generate.preview.validated")}
        </Badge>
        {draft.issues.length > 0 && <Badge variant="muted">{t("generate.preview.issues_badge", { count: draft.issues.length })}</Badge>}
        {draft.model && <span className="text-fg-subtle">{draft.model}</span>}
        {loading && (
          <span className="ml-auto flex items-center gap-1 text-fg-subtle" role="status">
            <Spinner className="size-3.5" />
            {t("generate.preview.loading")}
          </span>
        )}
      </div>
      <div className="min-h-0 flex-1 bg-bg-sunken p-3">
        <iframe title={t("generate.preview.frame_title")} sandbox="" srcDoc={html} className="h-full w-full rounded-md border border-border bg-white" data-testid="generate-preview-frame" data-loaded={html ? "true" : "false"} />
      </div>
      {draft.notes.length > 0 && (
        <ul className="border-t border-border px-3 py-1.5 text-[0.6875rem] text-fg-subtle" data-testid="generate-notes">
          {draft.notes.map((n, i) => (
            <li key={i}>{n}</li>
          ))}
        </ul>
      )}
    </div>
  );
}
