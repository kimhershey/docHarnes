import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects, useProjectTemplates } from "@/lib/queries";
import type { Project, ProjectInput } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

const NONE = "__none__";

/** "새 프로젝트" / 편집: name, template, 접수번호, 담당, 기한, inbox subfolder mapping. */
export function NewProjectDialog({ open, onOpenChange, project, onSaved }: { open: boolean; onOpenChange: (o: boolean) => void; project?: Project | null; onSaved: (p: Project) => void }) {
  const { t, i18n } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: templates } = useProjectTemplates(open);
  const [name, setName] = useState("");
  const [template, setTemplate] = useState<string>(NONE);
  const [intakeNo, setIntakeNo] = useState("");
  const [owner, setOwner] = useState("");
  const [due, setDue] = useState("");
  const [inbox, setInbox] = useState("");
  const [busy, setBusy] = useState(false);
  const ko = i18n.language.startsWith("ko");
  useEffect(() => {
    if (!open) return;
    setName(project?.name ?? "");
    setTemplate(project?.template_id ?? NONE);
    setIntakeNo(project?.intake_no ?? "");
    setOwner(project?.owner ?? "");
    setDue(project?.due_date ?? "");
    setInbox(project?.inbox_folder ?? "");
  }, [open, project]);
  const submit = async () => {
    setBusy(true);
    try {
      const input: ProjectInput = { id: project?.id ?? null, name: name.trim(), template_id: template === NONE ? null : template, intake_no: intakeNo.trim() || null, owner: owner.trim() || null, due_date: due || null, inbox_folder: inbox.trim() || null };
      const saved = project ? await api.projectUpdate(input) : await api.projectCreate(input);
      invalidateProjects(project ? [project.id] : []);
      toast(project ? t("common.saved") : t("projects.dialog.created"), "ok");
      onOpenChange(false);
      onSaved(saved);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent closeLabel={t("common.close")} data-testid="project-dialog">
        <DialogHeader>
          <DialogTitle>{project ? t("projects.dialog.edit_title") : t("projects.dialog.title")}</DialogTitle>
          <DialogDescription>{t("projects.empty_body")}</DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (name.trim() && !busy) void submit();
          }}
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="project-name">
              {t("projects.dialog.name")} <span className="text-danger">*</span>
            </Label>
            <Input id="project-name" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("projects.dialog.name_placeholder")} data-testid="project-name" />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="project-template">{t("projects.dialog.template")}</Label>
            <Select value={template} onValueChange={setTemplate}>
              <SelectTrigger id="project-template" aria-label={t("projects.dialog.template")} data-testid="project-template">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={NONE}>{t("projects.dialog.template_none")}</SelectItem>
                {(templates ?? []).map((tpl) => (
                  <SelectItem key={tpl.id} value={tpl.id} data-testid={`project-template-${tpl.id}`}>
                    {ko ? tpl.name_ko : tpl.name_en}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="grid grid-cols-3 gap-2">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="project-intake">{t("projects.dialog.intake_no")}</Label>
              <Input id="project-intake" value={intakeNo} onChange={(e) => setIntakeNo(e.target.value)} data-testid="project-intake" />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="project-owner">{t("projects.dialog.owner")}</Label>
              <Input id="project-owner" value={owner} onChange={(e) => setOwner(e.target.value)} data-testid="project-owner" />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="project-due">{t("projects.dialog.due")}</Label>
              <Input id="project-due" type="date" value={due} onChange={(e) => setDue(e.target.value)} data-testid="project-due" />
            </div>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="project-inbox">{t("projects.dialog.inbox_folder")}</Label>
            <div className="flex items-center gap-2">
              <span className="text-sm text-fg-muted">inbox/</span>
              <Input id="project-inbox" value={inbox} onChange={(e) => setInbox(e.target.value)} data-testid="project-inbox" />
            </div>
            <p className="text-xs text-fg-subtle">{t("projects.dialog.inbox_hint")}</p>
          </div>
          <DialogFooter>
            <Button type="button" variant="secondary" onClick={() => onOpenChange(false)}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={busy || !name.trim()} data-testid="project-submit">
              {project ? t("projects.dialog.save") : t("projects.dialog.create")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
