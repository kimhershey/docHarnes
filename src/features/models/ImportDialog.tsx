import { FolderInput } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { api, isTauri, pickGgufFiles } from "@/lib/ipc";
import { IMPORT_MODES, type ImportMode, type ImportPlan } from "@/lib/types";
import { describeError } from "@/lib/errors";
import { formatBytes } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

/**
 * Offline import (design §3.3.4): pick GGUF files (+ mmproj auto-pairing),
 * validate headers, choose copy/move/link, run as a job.
 */
export function ImportDialog({ open, onOpenChange, onStarted }: { open: boolean; onOpenChange: (o: boolean) => void; onStarted?: (jobId: string) => void }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const workspace = useWorkspace();
  const [pathsText, setPathsText] = useState("");
  const [mode, setMode] = useState<ImportMode>("copy");
  const [folderName, setFolderName] = useState("");
  const [plan, setPlan] = useState<ImportPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const paths = pathsText
    .split(/\r?\n/)
    .map((s) => s.trim().replace(/^"|"$/g, ""))
    .filter(Boolean);

  const reset = () => {
    setPlan(null);
    setError(null);
  };

  const pick = async () => {
    const files = await pickGgufFiles(t("models.import.pick"), workspace?.models_dir);
    if (files.length > 0) {
      setPathsText(files.join("\n"));
      reset();
    }
  };

  const validate = async () => {
    setBusy(true);
    setError(null);
    try {
      setPlan(await api.modelsImportPlan(paths, mode, folderName.trim() || null));
    } catch (e) {
      setPlan(null);
      setError(describeError(e, t));
    } finally {
      setBusy(false);
    }
  };

  const run = async () => {
    if (!plan) return;
    setBusy(true);
    try {
      const job = await api.modelsImportRun(paths, mode, folderName.trim() || null);
      toast(t("models.import.started"), "ok");
      onStarted?.(job);
      onOpenChange(false);
      setPlan(null);
      setPathsText("");
    } catch (e) {
      setError(describeError(e, t));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl" closeLabel={t("common.close")}>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <FolderInput className="size-4" aria-hidden="true" />
            {t("models.import.title")}
          </DialogTitle>
          <DialogDescription>{t("onboarding.model.import_desc")}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="import-paths">{t("models.import.paths_label")}</Label>
            <textarea
              id="import-paths"
              className="min-h-20 w-full rounded-md border border-border bg-bg p-2 font-mono text-xs"
              value={pathsText}
              placeholder={"D:\\models\\gemma-4-E4B-it-Q5_K_M.gguf\nD:\\models\\mmproj-F16.gguf"}
              onChange={(e) => {
                setPathsText(e.target.value);
                reset();
              }}
              data-testid="import-paths"
            />
            {isTauri && (
              <div>
                <Button variant="secondary" size="sm" onClick={() => void pick()}>
                  {t("models.import.pick")}
                </Button>
              </div>
            )}
          </div>
          <div className="grid gap-3 md:grid-cols-2">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="import-mode">{t("models.import.mode")}</Label>
              <Select value={mode} onValueChange={(v) => setMode(v as ImportMode)}>
                <SelectTrigger id="import-mode" data-testid="import-mode">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {IMPORT_MODES.map((m) => (
                    <SelectItem key={m} value={m}>
                      {t(`models.import.modes.${m}`)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="import-folder">{t("models.import.folder")}</Label>
              <Input
                id="import-folder"
                value={folderName}
                onChange={(e) => {
                  setFolderName(e.target.value);
                  reset();
                }}
              />
            </div>
          </div>
          {error && <p className="text-sm text-danger selectable">{error}</p>}
          {plan && (
            <div className="rounded-md border border-border bg-bg p-3 text-sm" data-testid="import-plan">
              <div className="mb-2 flex flex-wrap items-center gap-2">
                <span className="font-medium">{t("models.import.target")}:</span>
                <span className="selectable">{plan.target_folder}</span>
                {plan.architecture && <Badge variant="muted">{plan.architecture}</Badge>}
                {plan.quantization && <Badge variant="muted">{plan.quantization}</Badge>}
                {plan.has_vision && <Badge variant="muted">{t("models.vision")}</Badge>}
              </div>
              <ul className="flex flex-col gap-1">
                {plan.files.map((f) => (
                  <li key={f.source} className="flex flex-wrap justify-between gap-2">
                    <span className="selectable">
                      {f.dest_name} {f.is_mmproj && <span className="text-fg-subtle">(mmproj)</span>}
                    </span>
                    <span className="text-fg-muted tabular-nums">{formatBytes(f.size)}</span>
                  </li>
                ))}
              </ul>
              <p className="mt-2 text-xs text-fg-muted">
                {t("models.import.files")}: {plan.files.length} · {formatBytes(plan.total_bytes)}
                {plan.license_files.length > 0 ? ` · LICENSE ×${plan.license_files.length}` : ""}
              </p>
              {plan.auto_paired_mmproj && <p className="text-xs text-info">{t("models.import.auto_paired")}</p>}
              {plan.already_present && <p className="text-xs text-warn">{t("models.import.already")}</p>}
              {plan.warnings.map((w) => (
                <p key={w} className="text-xs text-warn">
                  {t(w)}
                </p>
              ))}
            </div>
          )}
        </div>
        <DialogFooter>
          <Button variant="secondary" disabled={paths.length === 0 || busy || !workspace} onClick={() => void validate()} data-testid="import-validate">
            {busy && !plan ? <Spinner /> : null}
            {t("models.import.plan")}
          </Button>
          <Button disabled={!plan || busy || plan.already_present} onClick={() => void run()} data-testid="import-run">
            {t("models.import.run")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
