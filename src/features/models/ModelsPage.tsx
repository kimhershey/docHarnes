import { Download, FolderOpen, FolderInput, ListChecks } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { revealPath } from "@/lib/ipc";
import { useAssessment, useRegistry } from "@/lib/queries";
import { TIERS, type BackendPreference, type Profile, type Tier } from "@/lib/types";
import { errorMessage, formatBytes } from "@/lib/utils";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

import { HardwarePanel } from "./HardwarePanel";
import { RequiredFilesDialog } from "./RequiredFilesDialog";

const PROFILES: Profile[] = ["accuracy", "balanced", "speed", "fine_print"];
const BACKENDS: BackendPreference[] = ["auto", "cuda", "vulkan", "cpu"];

export function ModelsPage() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings);
  const setTierMode = useSettingsStore((s) => s.setTierMode);
  const setActiveModel = useSettingsStore((s) => s.setActiveModel);
  const update = useSettingsStore((s) => s.update);
  const workspace = useWorkspace();
  const toast = useUiStore((s) => s.toast);
  const { data: assessment } = useAssessment();
  const { data: registry } = useRegistry();
  const [filesFor, setFilesFor] = useState<string | null>(null);

  if (!settings) return null;
  const tierMode = settings.models.tier_mode;
  const manualTier = settings.models.manual_tier ?? assessment?.tier.tier ?? "vram_8gb";
  const vlms = registry?.models.filter((m) => m.role === "vlm") ?? [];

  const guard = (p: Promise<unknown>) => p.catch((e) => toast(errorMessage(e), "danger"));

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-4 p-6" data-testid="page-models">
      <h1 className="text-xl font-semibold">{t("models.title")}</h1>

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>{t("models.hardware")}</CardTitle>
          </CardHeader>
          <CardContent>
            <HardwarePanel />
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>{t("models.tier")}</CardTitle>
            <CardDescription>{t("onboarding.hardware.intro")}</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <Label>{t("models.tier_mode")}</Label>
              <div className="flex gap-2">
                <Select value={tierMode} onValueChange={(v) => void guard(setTierMode(v as "auto" | "manual", v === "manual" ? (manualTier as Tier) : null))}>
                  <SelectTrigger className="w-56" data-testid="tier-mode">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="auto">{t("models.tier_auto")}</SelectItem>
                    <SelectItem value="manual">{t("models.tier_manual")}</SelectItem>
                  </SelectContent>
                </Select>
                {tierMode === "manual" && (
                  <Select value={manualTier} onValueChange={(v) => void guard(setTierMode("manual", v as Tier))}>
                    <SelectTrigger className="w-40" data-testid="manual-tier">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {TIERS.map((tier) => (
                        <SelectItem key={tier} value={tier}>
                          {t(`hw.tier.${tier}`)}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              </div>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label>{t("models.active_model")}</Label>
              <Select value={settings.models.active_model} onValueChange={(v) => void guard(setActiveModel(v))}>
                <SelectTrigger data-testid="active-model">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="auto">
                    {t("models.active_auto")}
                    {assessment?.recommended_model ? ` — ${assessment.recommended_model.display}` : ""}
                  </SelectItem>
                  {vlms.map((m) => (
                    <SelectItem key={m.id} value={m.id}>
                      {m.display}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {assessment?.active_model && (
                <div className="flex flex-wrap items-center gap-2 text-sm">
                  <Badge variant={assessment.active_model_installed?.installed ? "ok" : "warn"}>
                    {assessment.active_model_installed?.installed ? t("common.installed") : t("common.not_installed")}
                  </Badge>
                  {assessment.active_model_installed && !assessment.active_model_installed.installed && (
                    <span className="text-fg-muted">{t("models.missing_files", { files: assessment.active_model_installed.missing_files.join(", ") })}</span>
                  )}
                  {!workspace && <span className="text-fg-muted">{t("models.no_workspace")}</span>}
                </div>
              )}
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1.5">
                <Label>{t("models.profile")}</Label>
                <Select value={settings.models.profile} onValueChange={(v) => void guard(update((s) => void (s.models.profile = v as Profile)))}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {PROFILES.map((p) => (
                      <SelectItem key={p} value={p}>
                        {t(`models.profiles.${p}`)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="flex flex-col gap-1.5">
                <Label>{t("models.backend")}</Label>
                <Select value={settings.models.backend} onValueChange={(v) => void guard(update((s) => void (s.models.backend = v as BackendPreference)))}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {BACKENDS.map((b) => (
                      <SelectItem key={b} value={b}>
                        {t(`models.backends.${b}`)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>

            <div className="flex flex-wrap gap-2">
              <Button variant="secondary" size="sm" disabled={!workspace} onClick={() => workspace && void revealPath(workspace.models_dir)}>
                <FolderOpen />
                {t("models.open_models_dir")}
              </Button>
              <Tooltip>
                <TooltipTrigger asChild>
                  <span>
                    <Button variant="secondary" size="sm" disabled>
                      <Download />
                      {t("models.download")}
                    </Button>
                  </span>
                </TooltipTrigger>
                <TooltipContent>{t("models.p1_hint")}</TooltipContent>
              </Tooltip>
              <Tooltip>
                <TooltipTrigger asChild>
                  <span>
                    <Button variant="secondary" size="sm" disabled>
                      <FolderInput />
                      {t("models.import")}
                    </Button>
                  </span>
                </TooltipTrigger>
                <TooltipContent>{t("models.p1_hint")}</TooltipContent>
              </Tooltip>
            </div>
            <p className="text-xs text-fg-subtle">
              {t("models.models_dir")}: <span className="selectable">{workspace?.models_dir ?? "—"}</span>
            </p>
            <p className="text-xs text-fg-subtle">{t("models.drop_in_hint")}</p>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>{t("models.registry")}</CardTitle>
          <CardDescription>{t("models.engine_hint")}</CardDescription>
        </CardHeader>
        <CardContent className="overflow-x-auto">
          <table className="w-full text-sm [&_td]:align-top [&_th]:whitespace-nowrap" data-testid="registry-table">
            <thead className="text-left text-xs uppercase tracking-wide text-fg-subtle">
              <tr>
                <th className="py-2 pr-3">{t("models.columns.model")}</th>
                <th className="py-2 pr-3">{t("models.columns.role")}</th>
                <th className="py-2 pr-3">{t("models.columns.size")}</th>
                <th className="py-2 pr-3">{t("models.columns.license")}</th>
                <th className="py-2 pr-3">{t("models.columns.tiers")}</th>
                <th className="py-2 pr-3">{t("models.columns.status")}</th>
                <th className="py-2" />
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {registry?.models.map((m) => {
                const isActive = assessment?.active_model?.id === m.id;
                const isRecommended = assessment?.recommended_model?.id === m.id;
                return (
                  <tr key={m.id} className={isActive ? "bg-accent-soft/40" : undefined}>
                    <td className="py-2 pr-3 min-w-64">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span className="font-medium">{m.display}</span>
                        {isRecommended && <Badge variant="info">{t("models.recommended")}</Badge>}
                        {!m.verified && <Badge variant="outline">{t("models.unverified")}</Badge>}
                        {m.modalities.includes("image") ? <Badge variant="muted">{t("models.vision")}</Badge> : <Badge variant="muted">{t("models.text_only")}</Badge>}
                      </div>
                      <div className="text-xs text-fg-subtle selectable">{m.id}</div>
                    </td>
                    <td className="py-2 pr-3 whitespace-nowrap">{t(`models.role.${m.role}`)}</td>
                    <td className="py-2 pr-3 tabular-nums whitespace-nowrap">{formatBytes(m.total_size)}</td>
                    <td className="py-2 pr-3 whitespace-nowrap">{m.license}</td>
                    <td className="py-2 pr-3">
                      <div className="flex flex-wrap gap-1">
                        {m.requirements.tiers.map((tier) => (
                          <Badge key={tier} variant="outline" className="text-[10px]">
                            {t(`hw.tier.${tier}`)}
                          </Badge>
                        ))}
                      </div>
                    </td>
                    <td className="py-2 pr-3">
                      {m.install ? (
                        <Badge variant={m.install.installed ? "ok" : "muted"}>{m.install.installed ? t("common.installed") : t("common.not_installed")}</Badge>
                      ) : (
                        <span className="text-fg-subtle">—</span>
                      )}
                    </td>
                    <td className="py-2 whitespace-nowrap">
                      <div className="flex justify-end gap-1">
                        <Button variant="ghost" size="sm" onClick={() => setFilesFor(m.id)}>
                          <ListChecks />
                          {t("models.required_files")}
                        </Button>
                        {m.role === "vlm" && !isActive && (
                          <Button variant="outline" size="sm" onClick={() => void guard(setActiveModel(m.id))}>
                            {t("models.activate")}
                          </Button>
                        )}
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </CardContent>
      </Card>

      {filesFor && <RequiredFilesDialog modelId={filesFor} open={filesFor !== null} onOpenChange={(o) => !o && setFilesFor(null)} />}
    </div>
  );
}
