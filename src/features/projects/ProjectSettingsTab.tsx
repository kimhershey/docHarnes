import { FolderOutput, FolderTree, Save } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects } from "@/lib/queries";
import { DEFAULT_NAME_TEMPLATE, DEFAULT_PROJECT_FOLDER, EXPORT_FORMATS, SCREEN_ROUTES, type ExportFormat, type ExportLayout, type ProjectDetail, type ProjectSettings } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

const LAYOUTS: ExportLayout[] = ["sheet_per_type", "sheet_per_project", "single"];
const DEFAULT = "__default__";

/**
 * 정리·내보내기 settings of one project (plan §6): the folder / file-name templates used by "이 프로젝트 정리"
 * and for the tracked copies of shared documents, the copies switch, export defaults, and the two shortcuts
 * into the organise screen scoped to this project.
 */
export function ProjectSettingsTab({ detail }: { detail: ProjectDetail }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const p = detail.project;
  const [folder, setFolder] = useState("");
  const [name, setName] = useState("");
  const [copies, setCopies] = useState(true);
  const [format, setFormat] = useState<string>(DEFAULT);
  const [layout, setLayout] = useState<string>(DEFAULT);
  const [exportPath, setExportPath] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    setFolder(p.settings.folder_template ?? "");
    setName(p.settings.name_template ?? "");
    setCopies(p.settings.copies ?? true);
    setFormat(p.settings.export_format ?? DEFAULT);
    setLayout(p.settings.export_layout ?? DEFAULT);
    setExportPath(p.settings.export_path ?? "");
  }, [p]);
  const save = async () => {
    setBusy(true);
    try {
      const settings: ProjectSettings = {
        folder_template: folder.trim() || null,
        name_template: name.trim() || null,
        group_by: p.settings.group_by ?? null,
        export_format: format === DEFAULT ? null : (format as ExportFormat),
        export_layout: layout === DEFAULT ? null : (layout as ExportLayout),
        export_path: exportPath.trim() || null,
        copies,
      };
      await api.projectUpdate({ id: p.id, name: p.name, settings });
      invalidateProjects([p.id]);
      toast(t("projects.settings.saved"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="flex max-w-3xl flex-col gap-4" data-testid="project-settings">
      <div>
        <h3 className="text-sm font-semibold">{t("projects.settings.title")}</h3>
        <p className="text-xs text-fg-subtle">{t("projects.settings.hint", { default: DEFAULT_PROJECT_FOLDER })}</p>
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="project-settings-folder">{t("projects.settings.folder_template")}</Label>
        <Input id="project-settings-folder" value={folder} onChange={(e) => setFolder(e.target.value)} placeholder={DEFAULT_PROJECT_FOLDER} className="font-mono text-xs" data-testid="project-settings-folder" />
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="project-settings-name">{t("projects.settings.name_template")}</Label>
        <Input id="project-settings-name" value={name} onChange={(e) => setName(e.target.value)} placeholder={DEFAULT_NAME_TEMPLATE} className="font-mono text-xs" data-testid="project-settings-name" />
        <p className="text-xs text-fg-subtle">{t("projects.settings.name_default")}</p>
      </div>
      <label className="flex items-start gap-2 text-sm">
        <Switch checked={copies} onCheckedChange={setCopies} data-testid="project-settings-copies" />
        <span className="flex flex-col">
          {t("projects.settings.copies")}
          <span className="text-xs text-fg-subtle">{t("projects.settings.copies_hint")}</span>
        </span>
      </label>
      <div className="grid gap-3 sm:grid-cols-3">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="project-settings-format">{t("projects.settings.export_format")}</Label>
          <Select value={format} onValueChange={setFormat}>
            <SelectTrigger id="project-settings-format" className="h-8" data-testid="project-settings-format">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={DEFAULT}>{t("projects.settings.export_default")}</SelectItem>
              {EXPORT_FORMATS.map((f) => (
                <SelectItem key={f} value={f}>
                  {t(`export.format.${f}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="project-settings-layout">{t("projects.settings.export_layout")}</Label>
          <Select value={layout} onValueChange={setLayout}>
            <SelectTrigger id="project-settings-layout" className="h-8" data-testid="project-settings-layout">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={DEFAULT}>{t("projects.settings.export_default")}</SelectItem>
              {LAYOUTS.map((l) => (
                <SelectItem key={l} value={l}>
                  {t(`export.layout.${l}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="project-settings-export-path">{t("projects.settings.export_path")}</Label>
          <Input id="project-settings-export-path" value={exportPath} onChange={(e) => setExportPath(e.target.value)} placeholder="exports/{{project}}" className="h-8 font-mono text-xs" data-testid="project-settings-export-path" />
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" onClick={() => void save()} disabled={busy} data-testid="project-settings-save">
          <Save />
          {t("projects.settings.save")}
        </Button>
        <Button size="sm" variant="secondary" onClick={() => navigate(`${SCREEN_ROUTES.organize}?project=${encodeURIComponent(p.id)}`)} data-testid="project-organize-now">
          <FolderTree />
          {t("projects.settings.organize_now")}
        </Button>
        <Button size="sm" variant="secondary" onClick={() => navigate(`${SCREEN_ROUTES.organize}?tab=export&project=${encodeURIComponent(p.id)}`)} data-testid="project-export-now">
          <FolderOutput />
          {t("projects.settings.export_now")}
        </Button>
      </div>
    </div>
  );
}
