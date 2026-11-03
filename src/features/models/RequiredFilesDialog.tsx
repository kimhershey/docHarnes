import { Copy } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { api } from "@/lib/ipc";
import { useUiStore } from "@/stores/ui";

export function RequiredFilesDialog({ modelId, open, onOpenChange }: { modelId: string; open: boolean; onOpenChange: (o: boolean) => void }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const [text, setText] = useState("");

  useEffect(() => {
    if (!open) return;
    void api.registryRequiredFilesText(modelId).then(setText);
  }, [open, modelId]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      toast(t("common.copied"), "ok");
    } catch {
      toast(t("common.error"), "danger");
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl" closeLabel={t("common.close")}>
        <DialogHeader>
          <DialogTitle>{t("onboarding.model.required_files")}</DialogTitle>
          <DialogDescription>{t("onboarding.model.required_files_hint")}</DialogDescription>
        </DialogHeader>
        <pre className="max-h-80 overflow-auto rounded-md border border-border bg-bg p-3 text-xs selectable whitespace-pre-wrap" data-testid="required-files-text">
          {text}
        </pre>
        <DialogFooter>
          <Button onClick={() => void copy()}>
            <Copy />
            {t("common.copy")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
