import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Spinner } from "@/components/ui/spinner";
import type { DocumentRecord } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useJobsStore } from "@/stores/jobs";

import { displayStatusVariant, documentDisplayStatus, documentWorkRunning, type DocumentDisplayStatus } from "./docStatus";

type StatusDoc = Pick<DocumentRecord, "status" | "error">;

/** The status to show for a document right now (DOC-8): follows the job list for the "처리 중" reading of the marker. */
export function useDocumentDisplayStatus(doc: StatusDoc): DocumentDisplayStatus {
  const working = useJobsStore((s) => documentWorkRunning(s.jobs));
  return documentDisplayStatus(doc, working);
}

/**
 * Status badge of a document — the library cards and rows, the viewer header. A document that is being
 * converted / rendered reads "처리 중", one whose processing was cut off "중단됨" with the way out in the
 * tooltip; everything else is the stored status.
 */
export function DocStatusBadge({ doc, className, testId }: { doc: StatusDoc; className?: string; testId?: string }) {
  const { t } = useTranslation();
  const status = useDocumentDisplayStatus(doc);
  const hint = status === "processing" ? t("documents.status_hint.processing") : status === "interrupted" ? t("documents.status_hint.interrupted") : undefined;
  return (
    <Badge variant={displayStatusVariant(status)} className={className} title={hint} data-testid={testId} data-display-status={status}>
      {status === "processing" && <Spinner className="size-3 text-current" />}
      {t(`documents.status.${status}`)}
    </Badge>
  );
}

/** The same reading as plain text, for table cells. */
export function DocStatusText({ doc }: { doc: StatusDoc }) {
  const { t } = useTranslation();
  return <>{t(`documents.status.${useDocumentDisplayStatus(doc)}`)}</>;
}

/**
 * The 오류 row of the viewer's information list: nothing while the document is being processed (the marker is
 * not an error then), the cut-off processing in words with the way out, any other error verbatim.
 */
export function DocErrorRow({ doc }: { doc: StatusDoc }) {
  const { t } = useTranslation();
  const status = useDocumentDisplayStatus(doc);
  if (!doc.error || status === "processing") return null;
  return (
    <>
      <dt className="text-fg-subtle">{t("common.error")}</dt>
      <dd className={cn("whitespace-pre-wrap selectable", status === "interrupted" ? "text-warn" : "text-danger")} data-testid="viewer-doc-error">
        {status === "interrupted" ? t("documents.status_hint.interrupted") : doc.error}
      </dd>
    </>
  );
}
