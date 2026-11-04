import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useVersions } from "@/lib/queries";
import type { DocumentView } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

/** Registers a document as an edition of a version group (design §3.14.1 "버전"). */
export function VersionDialog({ open, onOpenChange, doc }: { open: boolean; onOpenChange: (open: boolean) => void; doc: DocumentView }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: groups } = useVersions(open);
  const current = groups?.flatMap((g) => g.documents).find((v) => v.document_id === doc.id) ?? null;
  const [group, setGroup] = useState("");
  const [label, setLabel] = useState("");
  const [effective, setEffective] = useState("");
  const [latest, setLatest] = useState(true);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setGroup(current?.group_key ?? (doc.title ?? doc.original_name).replace(/\.[a-z0-9]+$/i, "").replace(/[\s_-]*\d{4}.*$/, "").trim());
    setLabel(current?.version_label ?? "");
    setEffective(current?.effective_date ?? "");
    setLatest(current?.is_latest ?? true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, current?.document_id]);

  const save = async () => {
    setBusy(true);
    try {
      await api.knowledgeVersionSet({ document_id: doc.id, group_key: group, version_label: label.trim() || null, effective_date: effective.trim() || null, is_latest: latest });
      toast(t("viewer.version.saved"), "ok");
      onOpenChange(false);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  const clear = async () => {
    setBusy(true);
    try {
      await api.knowledgeVersionClear(doc.id);
      onOpenChange(false);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-testid="version-dialog">
        <DialogHeader>
          <DialogTitle>{t("viewer.version.title")}</DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <Label htmlFor="ver-group">{t("viewer.version.group")}</Label>
            <Input id="ver-group" value={group} onChange={(e) => setGroup(e.target.value)} list="version-groups" data-testid="version-group" />
            <datalist id="version-groups">{groups?.map((g) => <option key={g.group_key} value={g.group_key} />)}</datalist>
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor="ver-label">{t("viewer.version.label")}</Label>
            <Input id="ver-label" value={label} onChange={(e) => setLabel(e.target.value)} data-testid="version-label" />
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor="ver-date">{t("viewer.version.effective")}</Label>
            <Input id="ver-date" type="date" value={effective} onChange={(e) => setEffective(e.target.value)} />
          </div>
          <label className="flex items-center gap-2 text-sm">
            <Switch checked={latest} onCheckedChange={setLatest} data-testid="version-latest" />
            {t("viewer.version.latest")}
          </label>
        </div>
        <DialogFooter>
          {current && (
            <Button variant="ghost" className="mr-auto text-danger" onClick={() => void clear()} disabled={busy}>
              {t("viewer.version.clear")}
            </Button>
          )}
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button onClick={() => void save()} disabled={busy || !group.trim()} data-testid="version-save">
            {t("viewer.version.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
