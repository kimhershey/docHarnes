import { FolderOpen, Upload } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input, Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { DropHint, useDocumentDropTarget } from "@/lib/dnd";
import { describeError } from "@/lib/errors";
import { api, pickDocumentFiles, pickFolder } from "@/lib/ipc";
import { useCollections } from "@/lib/queries";
import { defaultImportOptions, type DocumentImportPlan, type ImportOptions } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

const NEW = "__new__";

/** Ingest wizard (design §3.14.1): files/folders → collection + options → preview → background import + indexing. */
export function IngestWizard({ open, onOpenChange, collectionId, initialPaths = null }: { open: boolean; onOpenChange: (open: boolean) => void; collectionId: string | null; initialPaths?: string[] | null }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: collections } = useCollections(open);
  const [step, setStep] = useState(0);
  const [pathsText, setPathsText] = useState("");
  const [choice, setChoice] = useState<string>(NEW);
  const [newName, setNewName] = useState("");
  const [render, setRender] = useState(true);
  const [move, setMove] = useState(false);
  const [plan, setPlan] = useState<DocumentImportPlan | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    if (!open) return;
    setStep(0);
    setPlan(null);
    setError(null);
    setPathsText(initialPaths && initialPaths.length > 0 ? initialPaths.join("\n") : "");
    setRender(true);
    setMove(false);
    setChoice(collectionId ?? (collections && collections.length > 0 ? (collections[0]?.id ?? NEW) : NEW));
    setNewName("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, collectionId, initialPaths]);

  const paths = pathsText.split(/\r?\n/).map((p) => p.trim()).filter(Boolean);
  const options = (): ImportOptions => ({ ...defaultImportOptions(), role: "knowledge", render, move_files: move, collection_id: choice === NEW ? null : choice });

  const preview = async () => {
    setBusy(true);
    setError(null);
    try {
      const p = await api.documentsImportPlan(paths, options(), "folder");
      setPlan(p);
      setStep(2);
    } catch (e) {
      setError(e);
    } finally {
      setBusy(false);
    }
  };

  const start = async () => {
    if (!plan) return;
    setBusy(true);
    setError(null);
    try {
      let cid = choice;
      if (choice === NEW) {
        cid = (await api.knowledgeCollectionSave({ name: newName })).id;
        setChoice(cid); // a failed import run can be retried without re-creating the collection
      }
      const opts = { ...options(), collection_id: cid };
      await api.documentsImportRun(plan.token, paths, opts, "folder");
      toast(t("knowledge.wizard.started"), "ok");
      onOpenChange(false);
    } catch (e) {
      setError(e);
    } finally {
      setBusy(false);
    }
  };

  const addPicked = (picked: string[]) => {
    if (picked.length === 0) return;
    setPathsText((prev) => [...prev.split(/\r?\n/).filter((l) => l.trim()), ...picked].join("\n"));
  };
  // Files dropped on the first step are added to the list; dropped library documents re-register by path
  // (the preview then marks them "already registered" and the start adds them to the collection).
  const drop = useDocumentDropTarget({
    disabled: !open || step !== 0,
    onFiles: addPicked,
    onDocuments: (docs) => addPicked(docs.map((d) => d.absolute_path).filter((p): p is string => !!p)),
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl" data-testid="ingest-wizard" closeLabel={t("common.close")}>
        <DialogHeader>
          <DialogTitle>{t("knowledge.wizard.title")}</DialogTitle>
          <DialogDescription>{t("knowledge.wizard.role_hint")}</DialogDescription>
        </DialogHeader>
        <ol className="flex gap-3 text-xs" aria-label={t("knowledge.wizard.steps_label")}>
          {(["step_files", "step_collection", "step_preview"] as const).map((k, i) => (
            <li key={k} className={i === step ? "font-semibold text-accent" : "text-fg-subtle"} aria-current={i === step ? "step" : undefined}>
              {t(`knowledge.wizard.${k}`)}
            </li>
          ))}
        </ol>
        {step === 0 && (
          <div className="relative flex flex-col gap-2" {...drop.props} data-testid="ingest-drop">
            <DropHint active={drop.isActive} over={drop.isOver} testId="ingest-drop-hint">
              {t("dnd.drop_here")}
            </DropHint>
            <p className="text-xs text-fg-muted">
              {t("knowledge.wizard.paths_hint")} {t("knowledge.wizard.drop_hint")}
            </p>
            <Textarea rows={6} value={pathsText} onChange={(e) => setPathsText(e.target.value)} className="font-mono text-xs" aria-label={t("knowledge.wizard.step_files")} data-testid="ingest-paths" />
            <div className="flex gap-2">
              <Button size="sm" variant="outline" onClick={() => void pickDocumentFiles(t("knowledge.wizard.pick_files"), t("import_docs.filter_name")).then(addPicked)}>
                <Upload />
                {t("knowledge.wizard.pick_files")}
              </Button>
              <Button size="sm" variant="outline" onClick={() => void pickFolder(t("knowledge.wizard.pick_folder")).then((f) => addPicked(f ? [f] : []))}>
                <FolderOpen />
                {t("knowledge.wizard.pick_folder")}
              </Button>
            </div>
          </div>
        )}
        {step === 1 && (
          <div className="flex flex-col gap-3">
            <div className="flex flex-col gap-1">
              <Label htmlFor="ingest-collection">{t("knowledge.wizard.collection_label")}</Label>
              <Select value={choice} onValueChange={setChoice}>
                <SelectTrigger id="ingest-collection" aria-label={t("knowledge.wizard.collection_label")} data-testid="ingest-collection">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {(collections ?? []).map((c) => (
                    <SelectItem key={c.id} value={c.id}>
                      {c.name}
                    </SelectItem>
                  ))}
                  <SelectItem value={NEW}>{t("knowledge.collections.create_inline")}</SelectItem>
                </SelectContent>
              </Select>
              {choice === NEW && <Input value={newName} onChange={(e) => setNewName(e.target.value)} placeholder={t("knowledge.wizard.new_collection_name")} data-testid="ingest-new-collection" />}
            </div>
            <label className="flex items-center gap-2 text-sm">
              <Switch checked={render} onCheckedChange={setRender} />
              {t("knowledge.wizard.render_label")}
            </label>
            <label className="flex items-center gap-2 text-sm">
              <Switch checked={move} onCheckedChange={setMove} />
              {t("knowledge.wizard.move_label")}
            </label>
          </div>
        )}
        {step === 2 && plan && (
          <div className="flex flex-col gap-2" data-testid="ingest-plan">
            <p className="text-sm">
              {t("knowledge.wizard.summary", { new: plan.new_count, relink: plan.relink_count, dup: plan.duplicate_count, registered: plan.registered_count, unsupported: plan.unsupported_count })}
            </p>
            {plan.new_count + plan.relink_count === 0 && <p className="text-xs text-warn">{t("knowledge.wizard.nothing")}</p>}
            <ul className="max-h-56 overflow-y-auto rounded-md border border-border text-xs">
              {plan.items.map((it) => (
                <li key={it.path} className="flex items-center gap-2 border-b border-border px-2 py-1 last:border-b-0">
                  <span className="min-w-0 flex-1 truncate" title={it.path}>
                    {it.name}
                  </span>
                  <Badge variant={it.status.kind === "new" || it.status.kind === "relink" ? "ok" : it.status.kind === "unsupported" ? "danger" : "muted"}>{t(`import_docs.item_status.${it.status.kind}`, it.status as unknown as Record<string, unknown>)}</Badge>
                </li>
              ))}
            </ul>
          </div>
        )}
        {busy && (
          <p className="flex items-center gap-2 text-sm text-fg-muted" role="status">
            <Spinner />
            {t("knowledge.wizard.previewing")}
          </p>
        )}
        {!!error && (
          <p className="text-sm text-danger selectable" role="alert" data-testid="ingest-error">
            {describeError(error, t)}
          </p>
        )}
        <DialogFooter>
          {step > 0 && (
            <Button variant="ghost" onClick={() => setStep((s) => s - 1)} disabled={busy}>
              {t("knowledge.wizard.back")}
            </Button>
          )}
          {step === 0 && (
            <Button onClick={() => setStep(1)} disabled={paths.length === 0} data-testid="ingest-next">
              {t("knowledge.wizard.next")}
            </Button>
          )}
          {step === 1 && (
            <Button onClick={() => void preview()} disabled={busy || (choice === NEW && !newName.trim())} data-testid="ingest-preview">
              {t("knowledge.wizard.preview")}
            </Button>
          )}
          {step === 2 && (
            <Button onClick={() => void start()} disabled={busy || !plan || plan.new_count + plan.relink_count + plan.registered_count + plan.duplicate_count === 0} data-testid="ingest-start">
              {t("knowledge.wizard.start")}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
