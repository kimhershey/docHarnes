import { useTranslation } from "react-i18next";

import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { useUiStore } from "@/stores/ui";

import { WorkspaceChooser } from "./WorkspaceChooser";

export function WorkspaceDialog() {
  const { t } = useTranslation();
  const mode = useUiStore((s) => s.workspaceDialog);
  const close = useUiStore((s) => s.closeWorkspaceDialog);
  return (
    <Dialog open={mode !== null} onOpenChange={(o) => (o ? undefined : close())}>
      <DialogContent className="max-w-xl" closeLabel={t("common.close")}>
        <DialogHeader>
          <DialogTitle>{mode === "create" ? t("topbar.new_workspace") : t("topbar.open_other")}</DialogTitle>
          <DialogDescription>{t("onboarding.workspace.settings_sync")}</DialogDescription>
        </DialogHeader>
        {mode && <WorkspaceChooser mode={mode} onOpened={() => close()} />}
      </DialogContent>
    </Dialog>
  );
}
