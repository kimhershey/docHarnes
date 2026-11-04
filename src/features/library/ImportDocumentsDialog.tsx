import { FolderInput } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { describeError } from "@/lib/errors";
import { api, isTauri, pickDocumentFiles } from "@/lib/ipc";
import { defaultImportOptions, type DocumentImportPlan, type ImportDestination, type ImportItem, type ImportOptions } from "@/lib/types";
import { formatBytes } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

function itemVariant(item: ImportItem): "ok" | "warn" | "muted" | "info" | "danger" {
  switch (item.status.kind) {
    case "new":
      return "ok";
    case "relink":
      return "info";
    case "unsupported":
      return "danger";
    case "copying":
      return "warn";
    default:
      return "muted";
  }
}

/**
 * Import preview (design §3.4): file list with new / duplicate / unsupported
 * reasons, destination and copy-vs-move, then the import runs as a job.
 */
export function ImportDocumentsDialog({
  open,
  onOpenChange,
  initialPaths,
  onStarted,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  initialPaths?: string[];
  onStarted?: (jobId: string) => void;
}) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const workspace = useWorkspace();
  const [pathsText, setPathsText] = useState("");
  const [options, setOptions] = useState<ImportOptions>(defaultImportOptions());
  const [plan, setPlan] = useState<DocumentImportPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const paths = pathsText
    .split(/\r?\n/)
    .map((s) => s.trim().replace(/^"|"$/g, ""))
    .filter(Boolean);

  useEffect(() => {
    if (open) {
      setPlan(null);
      setError(null);
      if (initialPaths && initialPaths.length > 0) setPathsText(initialPaths.join("\n"));
    }
  }, [open, initialPaths]);

  // Dropped files skip straight to the preview.
  useEffect(() => {
    if (open && initialPaths && initialPaths.length > 0 && !plan && !busy) {
      void preview(initialPaths);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, initialPaths]);

  const pick = async () => {
    const files = await pickDocumentFiles(t("import_docs.pick"), workspace?.inbox_dir);
    if (files.length > 0) {
      setPathsText(files.join("\n"));
      setPlan(null);
      setError(null);
    }
  };

  const preview = async (list = paths) => {
    if (list.length === 0) {
      setError(t("import_docs.no_paths"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setPlan(await api.documentsImportPlan(list, options, "drop"));
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
    setError(null);
    try {
      const jobId = await api.documentsImportRun(plan.token, paths, options, plan.source);
      toast(t("import_docs.started"), "ok");
      onStarted?.(jobId);
      onOpenChange(false);
      setPathsText("");
      setPlan(null);
    } catch (e) {
      setError(describeError(e, t));
    } finally {
      setBusy(false);
    }
  };

  const importable = plan ? plan.new_count + plan.relink_count : 0;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-3xl" closeLabel={t("common.close")}>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <FolderInput className="size-4" aria-hidden="true" />
            {t("import_docs.title")}
          </DialogTitle>
          <DialogDescription>{t("import_docs.description")}</DialogDescription>
        </DialogHeader>

        <div className="grid gap-4">
          <div className="grid gap-2">
            <div className="flex items-center justify-between">
              <Label htmlFor="doc-import-paths">{t("import_docs.paths_label")}</Label>
              {isTauri && (
                <Button size="sm" variant="secondary" onClick={() => void pick()} data-testid="doc-import-pick">
                  {t("import_docs.pick")}
                </Button>
              )}
            </div>
            <Textarea
              id="doc-import-paths"
              value={pathsText}
              onChange={(e) => {
                setPathsText(e.target.value);
                setPlan(null);
              }}
              placeholder={t("import_docs.paths_placeholder")}
              rows={4}
              className="font-mono text-xs"
              data-testid="doc-import-paths"
            />
            {!isTauri && <p className="text-xs text-fg-subtle">{t("import_docs.browser_hint")}</p>}
          </div>

          <div className="grid gap-3 md:grid-cols-3">
            <div className="grid gap-1.5">
              <Label htmlFor="doc-import-dest">{t("import_docs.destination")}</Label>
              <Select value={options.destination} onValueChange={(v) => setOptions({ ...options, destination: v as ImportDestination })}>
                <SelectTrigger id="doc-import-dest" data-testid="doc-import-dest">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {(["documents", "inbox"] as ImportDestination[]).map((d) => (
                    <SelectItem key={d} value={d}>
                      {t(`import_docs.destinations.${d}`)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-xs text-fg-subtle">{t("import_docs.in_place")}</p>
            </div>
            <label className="flex items-center gap-2 text-sm">
              <Switch checked={options.move_files} onCheckedChange={(v) => setOptions({ ...options, move_files: v })} />
              {t("import_docs.move_files")}
            </label>
            <label className="flex items-center gap-2 text-sm">
              <Switch checked={options.render} onCheckedChange={(v) => setOptions({ ...options, render: v })} data-testid="doc-import-render" />
              {t("import_docs.render_pages")}
            </label>
          </div>

          {error && (
            <p className="text-sm text-danger selectable" role="alert" data-testid="doc-import-error">
              {error}
            </p>
          )}

          {plan && (
            <div className="grid gap-2" data-testid="doc-import-plan">
              <p className="text-sm">
                {t("import_docs.summary", {
                  new: plan.new_count,
                  relink: plan.relink_count,
                  dup: plan.duplicate_count,
                  registered: plan.registered_count,
                  unsupported: plan.unsupported_count,
                })}
                {plan.new_bytes > 0 && <span className="text-fg-muted"> · {t("import_docs.new_bytes", { size: formatBytes(plan.new_bytes) })}</span>}
              </p>
              {plan.copying_count > 0 && <p className="text-xs text-warn">{t("import_docs.copying", { count: plan.copying_count })}</p>}
              {plan.truncated && <p className="text-xs text-warn">{t("import_docs.truncated")}</p>}
              <ul className="max-h-64 divide-y divide-border overflow-y-auto rounded-md border border-border text-sm">
                {plan.items.map((item) => (
                  <li key={item.path} className="flex items-center gap-2 px-3 py-1.5" data-testid="doc-import-item" data-item-status={item.status.kind}>
                    <Badge variant={itemVariant(item)}>
                      {item.status.kind === "duplicate" && t("import_docs.item_status.duplicate", { of: item.status.of_rel_path })}
                      {item.status.kind === "duplicate_in_batch" && t("import_docs.item_status.duplicate_in_batch", { of: item.status.of })}
                      {item.status.kind === "relink" && t("import_docs.item_status.relink", { old: item.status.old_rel_path })}
                      {item.status.kind === "unsupported" && t("import_docs.item_status.unsupported")}
                      {(item.status.kind === "new" || item.status.kind === "already_registered" || item.status.kind === "copying") &&
                        t(`import_docs.item_status.${item.status.kind}`)}
                    </Badge>
                    <span className="min-w-0 flex-1 truncate" title={item.path}>
                      {item.name}
                    </span>
                    {item.kind && <Badge variant="outline">{t(`documents.kind.${item.kind}`)}</Badge>}
                    {item.status.kind === "unsupported" && (
                      <span className="text-xs text-fg-muted">{t(`documents.unsupported.${item.status.reason.split(":")[0]}`, { defaultValue: item.status.reason })}</span>
                    )}
                    <span className="text-xs text-fg-subtle">{formatBytes(item.size)}</span>
                  </li>
                ))}
              </ul>
              {importable === 0 && <p className="text-sm text-fg-muted">{t("import_docs.nothing")}</p>}
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button variant="outline" onClick={() => void preview()} disabled={busy || paths.length === 0} data-testid="doc-import-preview">
            {busy && !plan ? <Spinner className="size-4" /> : null}
            {t("import_docs.preview")}
          </Button>
          <Button onClick={() => void run()} disabled={busy || !plan || importable === 0} data-testid="doc-import-run">
            {t("import_docs.run")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
