import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { OUTPUT_FORMATS, type Draft, type OutputFormat, type SaveRequest } from "@/lib/types";

import { hasBlockingIssues } from "./draft";

function defaultName(draft: Draft): string {
  const d = new Date();
  const date = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  return `${date}_${draft.title || "document"}`.replace(/[\\/:*?"<>|]/g, "_");
}

/** "저장": output format, file name, library registration → `generate_save`. */
export function SaveDialog({ open, onOpenChange, draft, defaultOutput, busy, error, onSave }: { open: boolean; onOpenChange: (o: boolean) => void; draft: Draft; defaultOutput: OutputFormat; busy: boolean; error: string | null; onSave: (req: SaveRequest) => void }) {
  const { t } = useTranslation();
  const [output, setOutput] = useState<OutputFormat>(defaultOutput);
  const [fileName, setFileName] = useState("");
  const [register, setRegister] = useState(true);
  const [overwrite, setOverwrite] = useState(false);
  useEffect(() => {
    if (open) {
      setOutput(defaultOutput);
      setFileName(defaultName(draft));
      setRegister(true);
      setOverwrite(false);
    }
  }, [open, defaultOutput, draft]);
  const blocking = draft.issues.filter((i) => i.blocking).length;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent closeLabel={t("common.close")} data-testid="generate-save-dialog">
        <DialogHeader>
          <DialogTitle>{t("generate.save.title")}</DialogTitle>
          <DialogDescription>{t("generate.save.hint")}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="gen-save-format">{t("generate.save.format")}</Label>
            <Select value={output} onValueChange={(v) => setOutput(v as OutputFormat)}>
              <SelectTrigger id="gen-save-format" aria-label={t("generate.save.format")} data-testid="generate-save-format">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {OUTPUT_FORMATS.map((f) => (
                  <SelectItem key={f} value={f}>
                    {t(`generate.output.${f}`)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {output === "md" && <p className="text-xs text-fg-muted">{t("generate.save.warning_markdown")}</p>}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="gen-save-name">{t("generate.save.file_name")}</Label>
            <div className="flex items-center gap-2">
              <Input id="gen-save-name" value={fileName} onChange={(e) => setFileName(e.target.value)} data-testid="generate-save-name" />
              <span className="text-sm text-fg-muted">.{output}</span>
            </div>
            <p className="text-xs text-fg-subtle">documents/generated/</p>
          </div>
          <label className="flex items-center justify-between gap-2 text-sm">
            <span>{t("generate.save.register")}</span>
            <Switch checked={register} onCheckedChange={setRegister} disabled={output === "md"} aria-label={t("generate.save.register")} data-testid="generate-save-register" />
          </label>
          <label className="flex items-center justify-between gap-2 text-sm">
            <span>{t("generate.save.overwrite")}</span>
            <Switch checked={overwrite} onCheckedChange={setOverwrite} aria-label={t("generate.save.overwrite")} data-testid="generate-save-overwrite" />
          </label>
          {hasBlockingIssues(draft) && (
            <p className="rounded-md bg-warn-soft px-2 py-1.5 text-xs text-warn" role="status" data-testid="generate-save-blocking">
              {t("generate.save.blocking_warning", { count: blocking })}
            </p>
          )}
          {error && (
            <p className="text-xs text-danger" role="alert" data-testid="generate-save-error">
              {error}
            </p>
          )}
        </div>
        <DialogFooter>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button disabled={busy || !fileName.trim()} onClick={() => onSave({ draft, output, file_name: fileName.trim(), folder: null, register: output === "md" ? false : register, overwrite })} data-testid="generate-save-confirm">
            {t("generate.save.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
