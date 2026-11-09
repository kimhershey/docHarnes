import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import type { Project } from "@/lib/types";

/** Documents / files dropped on the sidebar's 프로젝트 entry: pick the project that receives them. */
export function PickProjectDialog({ open, onOpenChange, projects, count, mode, onPick, onCreate }: { open: boolean; onOpenChange: (o: boolean) => void; projects: Project[]; count: number; mode: "docs" | "files"; onPick: (projectId: string) => void; onCreate: () => void }) {
  const { t } = useTranslation();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md" closeLabel={t("common.close")} data-testid="project-pick-dialog">
        <DialogHeader>
          <DialogTitle>{t("projects.pick.title")}</DialogTitle>
          <DialogDescription>{mode === "files" ? t("projects.pick.hint_files", { count }) : t("projects.pick.hint", { count })}</DialogDescription>
        </DialogHeader>
        {projects.length === 0 ? (
          <p className="text-sm text-fg-muted" data-testid="project-pick-none">
            {t("projects.pick.none")}
          </p>
        ) : (
          <ul className="max-h-80 divide-y divide-border overflow-y-auto rounded-md border border-border text-sm">
            {projects.map((p) => (
              <li key={p.id}>
                <button
                  type="button"
                  className="flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-bg-sunken cursor-pointer"
                  onClick={() => {
                    onPick(p.id);
                    onOpenChange(false);
                  }}
                  data-testid={`project-pick-${p.id}`}
                >
                  <span className="min-w-0 flex-1 truncate font-medium">{p.name}</span>
                  <Badge variant="outline">{t(`projects.status.${p.status}`)}</Badge>
                  <span className="text-xs text-fg-subtle">{t("projects.card.documents", { count: p.document_count })}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
        <DialogFooter>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="outline"
            onClick={() => {
              onOpenChange(false);
              onCreate();
            }}
            data-testid="project-pick-new"
          >
            {t("projects.new")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
