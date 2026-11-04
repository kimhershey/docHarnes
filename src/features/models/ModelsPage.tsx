import { Download, FolderInput, FolderOpen, Hash, ListChecks, RefreshCw, Trash2, Upload, X } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { api, revealPath } from "@/lib/ipc";
import { useAssessment, useModelIndex, useRegistry } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import { TIERS, type BackendPreference, type DiscoveredModel, type ModelHealth, type Profile, type Tier } from "@/lib/types";
import { describeError } from "@/lib/errors";
import { formatBytes, formatDate } from "@/lib/utils";
import { useEngineStore } from "@/stores/engine";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

import { EngineCard } from "./EngineCard";
import { HardwarePanel } from "./HardwarePanel";
import { ImportDialog } from "./ImportDialog";
import { RequiredFilesDialog } from "./RequiredFilesDialog";

const PROFILES: Profile[] = ["accuracy", "balanced", "speed", "fine_print"];

function useOnline(): boolean {
  const [online, setOnline] = useState(typeof navigator === "undefined" ? false : navigator.onLine);
  useEffect(() => {
    const on = () => setOnline(true);
    const off = () => setOnline(false);
    window.addEventListener("online", on);
    window.addEventListener("offline", off);
    return () => {
      window.removeEventListener("online", on);
      window.removeEventListener("offline", off);
    };
  }, []);
  return online;
}
const BACKENDS: BackendPreference[] = ["auto", "cuda", "vulkan", "cpu"];

function healthVariant(h: ModelHealth | null | undefined): "ok" | "warn" | "danger" | "muted" | "info" {
  switch (h?.kind) {
    case "ready":
      return "ok";
    case "copying":
      return "info";
    case "incomplete":
    case "unsupported":
      return "warn";
    case "invalid":
      return "danger";
    default:
      return "muted";
  }
}

function HealthBadge({ health }: { health: ModelHealth | null | undefined }) {
  const { t } = useTranslation();
  if (!health) return <Badge variant="muted">{t("common.not_installed")}</Badge>;
  const reason = health.kind === "incomplete" ? health.missing.join(", ") : health.kind === "unsupported" || health.kind === "invalid" ? health.reason : null;
  return (
    <div className="flex flex-col gap-0.5">
      <Badge variant={healthVariant(health)}>
        {t(`models.health.${health.kind}`)}
        {health.kind === "incomplete" ? ` (${health.missing.length})` : ""}
      </Badge>
      {reason && <span className="max-w-56 break-all text-xs text-fg-muted selectable">{reason}</span>}
    </div>
  );
}

export function ModelsPage() {
  const { t, i18n } = useTranslation();
  const settings = useSettingsStore((s) => s.settings);
  const setTierMode = useSettingsStore((s) => s.setTierMode);
  const setActiveModel = useSettingsStore((s) => s.setActiveModel);
  const update = useSettingsStore((s) => s.update);
  const workspace = useWorkspace();
  const toast = useUiStore((s) => s.toast);
  const engineStatus = useEngineStore((s) => s.status);
  const loadModel = useEngineStore((s) => s.loadModel);
  const unloadModel = useEngineStore((s) => s.unloadModel);
  const { data: assessment } = useAssessment();
  const { data: registry } = useRegistry();
  const { data: index } = useModelIndex();
  const [filesFor, setFilesFor] = useState<string | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const online = useOnline();

  if (!settings) return null;
  const tierMode = settings.models.tier_mode;
  const manualTier = settings.models.manual_tier ?? assessment?.tier.tier ?? "vram_8gb";
  const vlms = registry?.models.filter((m) => m.role === "vlm" || (m.user_added && m.role === "llm")) ?? [];

  const guard = async (key: string, p: () => Promise<unknown>, okMessage?: string) => {
    setBusy(key);
    try {
      await p();
      if (okMessage) toast(okMessage, "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(null);
    }
  };

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: queryKeys.registry });
    void queryClient.invalidateQueries({ queryKey: queryKeys.modelIndex });
    void queryClient.invalidateQueries({ queryKey: queryKeys.assessment });
  };

  const instance = (id: string) => engineStatus?.models.find((m) => m.id === id);
  const canDownload = (id: string) => online && settings.models.allow_online_download && !!registry?.models.find((m) => m.id === id)?.files.every((f) => f.hf || f.url);

  const deleteFolder = async (d: DiscoveredModel) => {
    if (!window.confirm(t("models.delete_confirm", { folder: d.folder_name }))) return;
    await guard(`delete-${d.folder}`, async () => {
      await api.modelsDelete(d.folder);
      invalidate();
    });
  };

  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-4 p-6" data-testid="page-models">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h1 className="text-xl font-semibold">{t("models.title")}</h1>
        <div className="flex flex-wrap gap-2">
          <Button variant="secondary" size="sm" disabled={!workspace} onClick={() => setImportOpen(true)} data-testid="models-import">
            <FolderInput />
            {t("models.import.title")}
          </Button>
          <Button variant="secondary" size="sm" disabled={!workspace} onClick={() => workspace && void revealPath(workspace.models_dir)}>
            <FolderOpen />
            {t("models.open_models_dir")}
          </Button>
          <Button variant="ghost" size="sm" disabled={!workspace || busy === "scan"} onClick={() => void guard("scan", async () => (await api.modelsScan(), invalidate()))} data-testid="models-rescan">
            <RefreshCw />
            {t("models.discovered.rescan")}
          </Button>
        </div>
      </div>

      <EngineCard />

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
              <Label htmlFor="tier-mode">{t("models.tier_mode")}</Label>
              <div className="flex gap-2">
                <Select value={tierMode} onValueChange={(v) => void guard("tier", () => setTierMode(v as "auto" | "manual", v === "manual" ? (manualTier as Tier) : null))}>
                  <SelectTrigger id="tier-mode" className="w-56" data-testid="tier-mode">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="auto">{t("models.tier_auto")}</SelectItem>
                    <SelectItem value="manual">{t("models.tier_manual")}</SelectItem>
                  </SelectContent>
                </Select>
                {tierMode === "manual" && (
                  <Select value={manualTier} onValueChange={(v) => void guard("tier", () => setTierMode("manual", v as Tier))}>
                    <SelectTrigger className="w-40" data-testid="manual-tier" aria-label={t("models.tier_manual")}>
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
              <Label htmlFor="active-model">{t("models.active_model")}</Label>
              <Select value={settings.models.active_model} onValueChange={(v) => void guard("active", () => setActiveModel(v))}>
                <SelectTrigger id="active-model" data-testid="active-model">
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
                <div className="flex flex-wrap items-center gap-2 text-sm" data-testid="active-model-state">
                  <Badge variant={assessment.active_model_installed?.installed ? "ok" : "warn"}>
                    {assessment.active_model_installed?.installed ? t("common.installed") : t("common.not_installed")}
                  </Badge>
                  {assessment.active_model_installed?.verified === true && <Badge variant="info">{t("models.verified")}</Badge>}
                  {assessment.active_model_installed && !assessment.active_model_installed.installed && assessment.active_model_installed.missing_files.length > 0 && (
                    <span className="text-fg-muted">{t("models.missing_files", { files: assessment.active_model_installed.missing_files.join(", ") })}</span>
                  )}
                  {!workspace && <span className="text-fg-muted">{t("models.no_workspace")}</span>}
                  {assessment.active_model_installed?.installed && (
                    <Button
                      size="sm"
                      variant={instance(assessment.active_model.id)?.status === "loaded" ? "outline" : "default"}
                      disabled={busy !== null}
                      onClick={() => {
                        const id = assessment.active_model!.id;
                        if (instance(id)?.status === "loaded") void guard("unload", () => unloadModel(id));
                        else void guard("load", () => loadModel(id), t("models.load_started"));
                      }}
                      data-testid="active-model-load"
                    >
                      {instance(assessment.active_model.id)?.status === "loaded" ? t("models.actions.unload") : t("models.actions.load")}
                    </Button>
                  )}
                </div>
              )}
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="model-profile">{t("models.profile")}</Label>
                <Select value={settings.models.profile} onValueChange={(v) => void guard("profile", () => update((s) => void (s.models.profile = v as Profile)))}>
                  <SelectTrigger id="model-profile">
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
                <Label htmlFor="model-backend">{t("models.backend")}</Label>
                <Select value={settings.models.backend} onValueChange={(v) => void guard("backend", () => update((s) => void (s.models.backend = v as BackendPreference)))}>
                  <SelectTrigger id="model-backend">
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
            <p className="text-xs text-fg-subtle">
              {t("models.models_dir")}: <span className="selectable">{workspace?.models_dir ?? "—"}</span>
            </p>
            <p className="text-xs text-fg-subtle">{t("models.drop_in_hint")}</p>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>{t("models.discovered.title")}</CardTitle>
          <CardDescription>
            {index?.scanned_at ? t("models.discovered.scanned_at", { at: formatDate(index.scanned_at, i18n.language) }) : t("models.no_workspace")}
          </CardDescription>
        </CardHeader>
        <CardContent className="overflow-x-auto">
          {!index || index.discovered.length === 0 ? (
            <p className="text-sm text-fg-muted" data-testid="discovered-empty">
              {t("models.discovered.empty")}
            </p>
          ) : (
            <table className="w-full text-sm [&_td]:align-top [&_th]:whitespace-nowrap" data-testid="discovered-table">
              <thead className="text-left text-xs uppercase tracking-wide text-fg-subtle">
                <tr>
                  <th className="py-2 pr-3">{t("models.columns.model")}</th>
                  <th className="py-2 pr-3">{t("models.discovered.folder")}</th>
                  <th className="py-2 pr-3">{t("models.columns.size")}</th>
                  <th className="py-2 pr-3">{t("models.columns.status")}</th>
                  <th className="py-2 pr-3">{t("models.engine")}</th>
                  <th className="py-2" />
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {index.discovered.map((d) => {
                  const inst = instance(d.id);
                  const loaded = inst?.status === "loaded";
                  return (
                    <tr key={d.folder} data-testid={`discovered-${d.folder_name}`}>
                      <td className="py-2 pr-3 min-w-56">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <span className="font-medium">{d.display}</span>
                          {d.registry_match ? (
                            <Badge variant={d.registry_match.verified === true ? "info" : "outline"} title={d.registry_match.id}>
                              {d.registry_match.verified === true ? t("models.verified") : t(`models.matched_by.${d.registry_match.by}`)}
                            </Badge>
                          ) : (
                            <Badge variant="muted">{t("models.user_added")}</Badge>
                          )}
                          {d.has_vision ? <Badge variant="muted">{t("models.vision")}</Badge> : <Badge variant="muted">{t("models.text_only")}</Badge>}
                          {d.quantization && <Badge variant="outline">{d.quantization}</Badge>}
                        </div>
                        <div className="text-xs text-fg-subtle selectable">
                          {d.id} · {d.architecture ?? "?"}
                        </div>
                        {d.warnings.length > 0 && (
                          <ul className="mt-1 text-xs text-warn">
                            {d.warnings.map((w) => (
                              <li key={w}>{t(w)}</li>
                            ))}
                          </ul>
                        )}
                      </td>
                      <td className="py-2 pr-3">
                        <button type="button" className="selectable text-left text-xs text-fg-muted hover:underline" onClick={() => void revealPath(d.folder)}>
                          {d.folder_name}
                        </button>
                        <div className="text-xs text-fg-subtle">
                          {d.weights.map((w) => w.name).join(", ")}
                          {d.mmproj ? ` + ${d.mmproj.name}` : ""}
                        </div>
                      </td>
                      <td className="py-2 pr-3 tabular-nums whitespace-nowrap">{formatBytes(d.total_bytes)}</td>
                      <td className="py-2 pr-3">
                        <HealthBadge health={d.health} />
                      </td>
                      <td className="py-2 pr-3">
                        {inst ? (
                          <div className="flex flex-col gap-0.5">
                            <Badge variant={loaded ? "ok" : inst.status === "loading" ? "warn" : "muted"} data-testid={`instance-${d.id}`}>
                              {t(`models.instance.${inst.failed && inst.status === "unloaded" ? "failed" : inst.status}`, { defaultValue: inst.status })}
                              {inst.status === "loading" && inst.progress != null ? ` ${Math.round(inst.progress * 100)}%` : ""}
                            </Badge>
                            {loaded && inst.memory_effective_bytes != null && <span className="text-xs text-fg-muted">{formatBytes(inst.memory_effective_bytes)}</span>}
                          </div>
                        ) : (
                          <span className="text-fg-subtle">—</span>
                        )}
                      </td>
                      <td className="py-2 whitespace-nowrap">
                        <div className="flex justify-end gap-1">
                          {d.health.kind === "ready" && (
                            <Button
                              variant={loaded ? "outline" : "default"}
                              size="sm"
                              disabled={busy !== null || inst?.status === "loading"}
                              onClick={() => (loaded ? void guard(`unload-${d.id}`, () => unloadModel(d.id)) : void guard(`load-${d.id}`, () => loadModel(d.id), t("models.load_started")))}
                              data-testid={`load-${d.id}`}
                            >
                              {loaded ? <X /> : <Upload />}
                              {loaded ? t("models.actions.unload") : t("models.actions.load")}
                            </Button>
                          )}
                          {!d.hashed && d.health.kind === "ready" && (
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <Button variant="ghost" size="sm" disabled={busy !== null} onClick={() => void guard(`hash-${d.folder}`, () => api.modelsHash(d.folder), t("models.hash_started"))}>
                                  <Hash />
                                  {t("models.actions.hash")}
                                </Button>
                              </TooltipTrigger>
                              <TooltipContent>{t("models.actions.hash_hint")}</TooltipContent>
                            </Tooltip>
                          )}
                          <Button variant="ghost" size="sm" disabled={busy !== null} onClick={() => void deleteFolder(d)} data-testid={`delete-${d.folder_name}`}>
                            <Trash2 />
                            {t("models.actions.delete")}
                          </Button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{t("models.registry")}</CardTitle>
          <CardDescription>{t("models.registry_hint")}</CardDescription>
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
                const installed = m.install?.installed ?? false;
                return (
                  <tr key={m.id} className={isActive ? "bg-accent-soft/40" : undefined} data-testid={`registry-${m.id}`}>
                    <td className="py-2 pr-3 min-w-64">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span className="font-medium">{m.display}</span>
                        {isRecommended && <Badge variant="info">{t("models.recommended")}</Badge>}
                        {m.user_added && <Badge variant="muted">{t("models.user_added")}</Badge>}
                        {!m.verified && !m.user_added && <Badge variant="outline">{t("models.unverified")}</Badge>}
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
                        <div className="flex flex-col gap-1">
                          <Badge variant={installed ? "ok" : m.install.health ? healthVariant(m.install.health) : "muted"} data-testid={`install-${m.id}`}>
                            {installed ? t("common.installed") : m.install.health ? t(`models.health.${m.install.health.kind}`) : t("common.not_installed")}
                          </Badge>
                          {m.install.verified === true && <Badge variant="info">{t("models.verified")}</Badge>}
                        </div>
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
                        {!installed && !m.user_added && (
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <span>
                                <Button variant="ghost" size="sm" disabled={!workspace || !canDownload(m.id) || busy !== null} onClick={() => void guard(`dl-${m.id}`, () => api.modelsDownload(m.id), t("models.download_started"))} data-testid={`download-${m.id}`}>
                                  <Download />
                                  {t("models.download")}
                                </Button>
                              </span>
                            </TooltipTrigger>
                            <TooltipContent>{online ? (settings.models.allow_online_download ? t("models.download_hint") : t("models.download_disabled")) : t("onboarding.model.download_offline")}</TooltipContent>
                          </Tooltip>
                        )}
                        {installed && (m.role === "vlm" || m.role === "llm") && (
                          <Button
                            variant={instance(m.id)?.status === "loaded" ? "outline" : "secondary"}
                            size="sm"
                            disabled={busy !== null}
                            onClick={() => (instance(m.id)?.status === "loaded" ? void guard(`unload-${m.id}`, () => unloadModel(m.id)) : void guard(`load-${m.id}`, () => loadModel(m.id), t("models.load_started")))}
                          >
                            {instance(m.id)?.status === "loaded" ? t("models.actions.unload") : t("models.actions.load")}
                          </Button>
                        )}
                        {(m.role === "vlm" || (m.user_added && m.role === "llm")) && !isActive && (
                          <Button variant="outline" size="sm" onClick={() => void guard("activate", () => setActiveModel(m.id))}>
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
      <ImportDialog open={importOpen} onOpenChange={setImportOpen} onStarted={() => invalidate()} />
    </div>
  );
}
