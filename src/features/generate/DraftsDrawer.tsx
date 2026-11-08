import { Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";
import { useDrafts, useSchemas } from "@/lib/queries";
import type { DraftSummary } from "@/lib/types";
import { formatDate } from "@/lib/utils";

import { templateLabel } from "./draft";

/** "초안 목록": the drafts kept under `.doculens/drafts/` — open or delete. */
export function DraftsDrawer({ open, onOpenChange, onOpen, onDelete, currentId }: { open: boolean; onOpenChange: (o: boolean) => void; onOpen: (d: DraftSummary) => void; onDelete: (d: DraftSummary) => void; currentId: string | null }) {
  const { t, i18n } = useTranslation();
  const { data: drafts, isLoading } = useDrafts(open);
  const { data: schemas } = useSchemas(open);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl" closeLabel={t("common.close")} data-testid="generate-drafts-dialog">
        <DialogHeader>
          <DialogTitle>{t("generate.drafts_list.title")}</DialogTitle>
          <DialogDescription>{t("generate.drafts_list.hint")}</DialogDescription>
        </DialogHeader>
        {isLoading ? (
          <Spinner />
        ) : !drafts || drafts.length === 0 ? (
          <p className="text-sm text-fg-muted">{t("generate.drafts_list.empty")}</p>
        ) : (
          <ul className="max-h-96 divide-y divide-border overflow-y-auto text-sm" data-testid="generate-drafts">
            {drafts.map((d) => (
              <li key={d.id} className="flex items-center gap-2 py-2" data-testid={`generate-draft-row-${d.id}`} data-current={d.id === currentId}>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate font-medium">{d.title}</span>
                    <Badge variant="muted">{templateLabel(d.template, schemas, t)}</Badge>
                    {d.issues > 0 && <Badge variant="warn">{t("generate.drafts_list.issues", { count: d.issues })}</Badge>}
                  </div>
                  <div className="truncate text-xs text-fg-subtle">
                    {formatDate(d.updated_at, i18n.language)} · {d.saved_path ? t("generate.drafts_list.saved", { path: d.saved_path }) : t("generate.drafts_list.unsaved")}
                  </div>
                </div>
                <Button size="sm" variant="outline" onClick={() => onOpen(d)} data-testid="generate-draft-open">
                  {t("generate.drafts_list.open")}
                </Button>
                <Button size="icon-sm" variant="ghost" className="text-danger" onClick={() => onDelete(d)} aria-label={t("generate.drafts_list.delete")} data-testid="generate-draft-delete">
                  <Trash2 />
                </Button>
              </li>
            ))}
          </ul>
        )}
      </DialogContent>
    </Dialog>
  );
}
