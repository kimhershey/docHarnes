import { Activity, FileText, Play, RefreshCw, ScrollText, Square, Cpu } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { api, isTauri, pickExecutable } from "@/lib/ipc";
import { useEngineBinaryInfo } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import type { EngineDevice, EngineState, EngineStatus } from "@/lib/types";
import { describeError } from "@/lib/errors";
import { isComposing } from "@/lib/shortcuts";
import { formatBytes, formatMiB } from "@/lib/utils";
import { useEngineStore } from "@/stores/engine";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

import { ChatTestDialog } from "./ChatTestDialog";

export function engineStateVariant(state: EngineState | undefined): "ok" | "warn" | "danger" | "muted" | "info" {
  switch (state?.kind) {
    case "running":
      return "ok";
    case "starting":
    case "restarting":
    case "stopping":
      return "warn";
    case "degraded":
      return "danger";
    default:
      return "muted";
  }
}

/** Text/number field that commits on blur or Enter (not on every keystroke). */
function CommitInput({
  id,
  value,
  onCommit,
  type = "text",
  min,
  max,
  step,
  placeholder,
  className,
}: {
  id: string;
  value: string;
  onCommit: (v: string) => void;
  type?: "text" | "number";
  min?: number;
  max?: number;
  step?: number;
  placeholder?: string;
  className?: string;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = () => {
    if (draft !== value) onCommit(draft);
  };
  return (
    <Input
      id={id}
      type={type}
      min={min}
      max={max}
      step={step}
      className={className}
      placeholder={placeholder}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter" && !isComposing(e.nativeEvent)) {
          e.preventDefault();
          commit();
        }
      }}
    />
  );
}

/** Subscribes to the log ring only while open (hundreds of lines per second during a load). */
function EngineLogPanel() {
  const { t, i18n } = useTranslation();
  const logs = useEngineStore((s) => s.logs);
  const loadLogs = useEngineStore((s) => s.loadLogs);
  useEffect(() => {
    void loadLogs();
  }, [loadLogs]);
  return (
    <pre className="max-h-64 overflow-auto rounded-md border border-border bg-bg p-3 text-xs selectable whitespace-pre-wrap" data-testid="engine-logs">
      {logs.length === 0
        ? t("models.engine_card.no_logs")
        : logs
            .slice(-150)
            .map((l) => `${new Date(l.at).toLocaleTimeString(i18n.language)} [${l.stream}] ${l.text}`)
            .join("\n")}
    </pre>
  );
}

export function EngineCard() {
  const { t } = useTranslation();
  const status = useEngineStore((s) => s.status);
  const start = useEngineStore((s) => s.start);
  const stop = useEngineStore((s) => s.stop);
  const restart = useEngineStore((s) => s.restart);
  const settings = useSettingsStore((s) => s.settings);
  const update = useSettingsStore((s) => s.update);
  const toast = useUiStore((s) => s.toast);
  const workspace = useWorkspace();
  const binary = useEngineBinaryInfo(!!workspace);
  const [busy, setBusy] = useState<string | null>(null);
  const [showLogs, setShowLogs] = useState(false);
  const [presets, setPresets] = useState<string | null>(null);
  const [devices, setDevices] = useState<EngineDevice[] | null>(null);
  const [testOpen, setTestOpen] = useState(false);

  if (!settings) return null;
  const state = status?.state;
  const running = state?.kind === "running";
  const stopped = !state || state.kind === "stopped" || state.kind === "degraded";

  const run = async (key: string, f: () => Promise<unknown>, okMessage?: string) => {
    setBusy(key);
    try {
      await f();
      if (okMessage) toast(okMessage, "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(null);
    }
  };
  const save = (mutate: (s: NonNullable<typeof settings>) => void) => update(mutate).catch((e) => toast(describeError(e, t), "danger"));

  const loaded = status?.models.filter((m) => m.status === "loaded" || m.status === "sleeping") ?? [];
  const resolved = binary.data?.resolved ?? status?.binary ?? null;

  return (
    <Card data-testid="engine-card">
      <CardHeader>
        <div className="flex flex-wrap items-center justify-between gap-2">
          <CardTitle className="flex items-center gap-2">
            <Cpu className="size-4 text-fg-muted" aria-hidden="true" />
            {t("models.engine_card.title")}
            <Badge variant={engineStateVariant(state)} data-testid="engine-state">
              {stateLabel(t, state)}
            </Badge>
          </CardTitle>
          <div className="flex flex-wrap gap-2">
            <Button size="sm" disabled={!workspace || !stopped || busy !== null} onClick={() => void run("start", () => start())} data-testid="engine-start">
              <Play />
              {t("models.engine_card.start")}
            </Button>
            <Button size="sm" variant="secondary" disabled={stopped || busy !== null} onClick={() => void run("stop", () => stop())} data-testid="engine-stop">
              <Square />
              {t("models.engine_card.stop")}
            </Button>
            <Button size="sm" variant="secondary" disabled={!workspace || busy !== null} onClick={() => void run("restart", () => restart())} data-testid="engine-restart">
              <RefreshCw />
              {t("models.engine_card.restart")}
            </Button>
            <Button size="sm" variant="outline" disabled={!running || loaded.length === 0} onClick={() => setTestOpen(true)} data-testid="engine-test">
              <Activity />
              {t("models.engine_card.test")}
            </Button>
          </div>
        </div>
        <CardDescription>{t("models.engine_hint")}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        {state?.kind === "degraded" && (
          <div className="rounded-md border border-danger bg-danger-soft p-3 text-sm">
            <p className="font-medium text-danger">{t("models.engine_card.degraded_hint")}</p>
            <p className="text-fg-muted selectable">{state.reason}</p>
          </div>
        )}
        {status?.last_error && state?.kind !== "degraded" && (
          <p className="text-sm text-danger selectable">
            {t("models.engine_card.last_error")}: {status.last_error}
          </p>
        )}
        {binary.data?.error && !resolved && (
          <div className="rounded-md border border-warn bg-warn-soft p-3 text-sm">
            <p className="font-medium text-warn">{t("models.engine_card.not_found", { error: binary.data.error })}</p>
            <p className="text-fg-muted">{t("models.engine_card.install_hint")}</p>
            <p className="text-xs text-fg-subtle selectable">{binary.data.engines_dir}</p>
          </div>
        )}

        <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5 text-sm">
          <dt className="text-fg-muted">{t("models.engine_card.binary")}</dt>
          <dd className="selectable break-all">{resolved?.path ?? "—"}</dd>
          <dt className="text-fg-muted">{t("models.engine_card.version")}</dt>
          <dd>
            {resolved?.version ?? t("common.unknown")}
            {resolved?.build_number != null && binary.data && resolved.build_number > 0 && resolved.build_number < binary.data.min_build && (
              <Badge variant="danger" className="ml-2">
                {t("models.engine_card.too_old", { min: binary.data.min_build })}
              </Badge>
            )}
          </dd>
          <dt className="text-fg-muted">{t("models.engine_card.backend")}</dt>
          <dd>{resolved ? t(`models.packs.${resolved.backend}`) : "—"}</dd>
          <dt className="text-fg-muted">{t("models.engine_card.port")}</dt>
          <dd>
            {status?.port ? `${status.host}:${status.port}` : "—"}
            {status?.pid ? <span className="text-fg-subtle"> · pid {status.pid}</span> : null}
            {status && status.restarts > 0 ? <span className="text-fg-subtle"> · {t("models.engine_card.restarts", { count: status.restarts })}</span> : null}
          </dd>
          <dt className="text-fg-muted">{t("models.engine_card.loaded_models")}</dt>
          <dd data-testid="engine-loaded">
            {loaded.length === 0 ? (
              <span className="text-fg-subtle">{t("models.engine_card.none_loaded")}</span>
            ) : (
              <ul className="flex flex-col gap-1">
                {loaded.map((m) => (
                  <li key={m.id} className="flex flex-wrap items-center gap-2">
                    <span className="font-medium">{m.id}</span>
                    <Badge variant={m.status === "loaded" ? "ok" : "muted"}>{t(`models.instance.${m.status}`, { defaultValue: m.status })}</Badge>
                    {m.memory_effective_bytes != null && (
                      <span className="text-xs text-fg-muted">
                        {t("models.engine_card.memory")}: {formatBytes(m.memory_effective_bytes)}
                      </span>
                    )}
                    {m.warmed_up && <span className="text-xs text-fg-subtle">{t("models.engine_card.warmed_up")}</span>}
                  </li>
                ))}
              </ul>
            )}
          </dd>
          <dt className="text-fg-muted">{t("models.engine_card.packs")}</dt>
          <dd>
            {binary.data && binary.data.installed_packs.length > 0 ? (
              <div className="flex flex-wrap gap-1">
                {binary.data.installed_packs.map((p) => (
                  <Badge key={`${p.version}-${p.backend}`} variant="outline" title={p.path}>
                    {t(`models.packs.${p.backend}`)} · {p.version}
                  </Badge>
                ))}
              </div>
            ) : (
              <span className="text-fg-subtle">{t("models.engine_card.no_packs")}</span>
            )}
          </dd>
        </dl>

        <div className="grid gap-3 md:grid-cols-2">
          <label className="flex items-center justify-between gap-3 text-sm">
            <span>{t("models.engine_card.auto_start")}</span>
            <Switch checked={settings.models.auto_start_engine} onCheckedChange={(v) => void save((s) => void (s.models.auto_start_engine = v))} />
          </label>
          <label className="flex items-center justify-between gap-3 text-sm">
            <span>{t("models.engine_card.warmup")}</span>
            <Switch checked={settings.models.warmup_after_load} onCheckedChange={(v) => void save((s) => void (s.models.warmup_after_load = v))} />
          </label>
          <label className="flex items-center justify-between gap-3 text-sm">
            <span>{t("models.engine_card.vision_cpu")}</span>
            <Switch checked={settings.models.vision_on_cpu} onCheckedChange={(v) => void save((s) => void (s.models.vision_on_cpu = v))} />
          </label>
          <div className="flex items-center justify-between gap-3 text-sm">
            <Label htmlFor="sleep-idle">{t("models.engine_card.sleep_idle")}</Label>
            <CommitInput
              id="sleep-idle"
              type="number"
              min={0}
              step={60}
              className="w-28"
              value={String(settings.models.sleep_idle_seconds)}
              onCommit={(v) => void save((s) => void (s.models.sleep_idle_seconds = Math.max(0, Math.round(Number(v)) || 0)))}
            />
          </div>
          <div className="flex items-center justify-between gap-3 text-sm">
            <Label htmlFor="resident-max">{t("models.engine_card.resident_max")}</Label>
            <CommitInput
              id="resident-max"
              type="number"
              min={1}
              max={4}
              className="w-28"
              value={String(settings.models.resident_models_max)}
              onCommit={(v) => void save((s) => void (s.models.resident_models_max = Math.min(4, Math.max(1, Math.round(Number(v)) || 1))))}
            />
          </div>
          <div className="flex flex-col gap-1.5 text-sm md:col-span-2">
            <Label htmlFor="engine-binary">{t("models.engine_card.binary_override")}</Label>
            <div className="flex gap-2">
              <CommitInput
                id="engine-binary"
                className="flex-1"
                value={settings.models.engine_binary ?? ""}
                placeholder="…\\engines\\b12066\\cu121\\llama-server.exe"
                onCommit={(v) => void save((s) => void (s.models.engine_binary = v.trim() || null))}
              />
              {isTauri && (
                <Button
                  variant="secondary"
                  onClick={() =>
                    void pickExecutable(t("models.engine_card.binary_override")).then((p) => {
                      if (p) void save((s) => void (s.models.engine_binary = p));
                    })
                  }
                >
                  {t("models.engine_card.binary_pick")}
                </Button>
              )}
              <Button variant="ghost" disabled={!settings.models.engine_binary} onClick={() => void save((s) => void (s.models.engine_binary = null))}>
                {t("models.engine_card.binary_clear")}
              </Button>
            </div>
            <p className="text-xs text-fg-subtle">{t("models.engine_card.apply_on_restart")}</p>
          </div>
        </div>

        <div className="flex flex-wrap gap-2">
          <Button variant="ghost" size="sm" onClick={() => setShowLogs((v) => !v)} data-testid="engine-logs-toggle">
            <ScrollText />
            {t("models.engine_card.logs")}
          </Button>
          <Button variant="ghost" size="sm" disabled={!workspace} onClick={() => void run("presets", async () => setPresets(await api.presetsPreview()))}>
            <FileText />
            {t("models.engine_card.presets")}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            disabled={!workspace}
            onClick={() =>
              void run("devices", async () => {
                setDevices(await api.engineListDevices());
                // Engine-reported devices replace the NVML rows → tier may change.
                await Promise.all([
                  queryClient.invalidateQueries({ queryKey: queryKeys.hardware }),
                  queryClient.invalidateQueries({ queryKey: queryKeys.assessment }),
                ]);
              })
            }
          >
            {t("models.engine_card.devices")}
          </Button>
        </div>
        {devices && (
          <div className="rounded-md border border-border bg-bg p-3 text-sm" data-testid="engine-devices">
            <p className="mb-1 font-medium">{t("models.engine_card.devices_title")}</p>
            {devices.length === 0 ? (
              <p className="text-fg-muted">{t("common.none")}</p>
            ) : (
              <ul className="flex flex-col gap-1">
                {devices.map((d) => (
                  <li key={d.name}>
                    <span className="font-mono text-xs">{d.name}</span> · {d.description} · {formatMiB(d.free_mib)} / {formatMiB(d.total_mib)}
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
        {showLogs && <EngineLogPanel />}
      </CardContent>

      <Dialog open={presets !== null} onOpenChange={(o) => !o && setPresets(null)}>
        <DialogContent className="max-w-3xl" closeLabel={t("common.close")}>
          <DialogHeader>
            <DialogTitle>{t("models.engine_card.presets_title")}</DialogTitle>
            <DialogDescription>{status?.presets_path ?? ""}</DialogDescription>
          </DialogHeader>
          <pre className="max-h-96 overflow-auto rounded-md border border-border bg-bg p-3 text-xs selectable whitespace-pre-wrap">{presets}</pre>
        </DialogContent>
      </Dialog>
      <ChatTestDialog open={testOpen} onOpenChange={setTestOpen} modelId={status?.active_model ?? loaded[0]?.id ?? null} />
    </Card>
  );
}

export function stateLabel(t: (k: string, o?: Record<string, unknown>) => string, state: EngineState | undefined | null): string {
  if (!state) return t("models.engine_card.state.stopped");
  switch (state.kind) {
    case "restarting":
      return t("models.engine_card.state.restarting", { attempt: state.attempt });
    default:
      return t(`models.engine_card.state.${state.kind}`);
  }
}

export function engineSummary(status: EngineStatus | null): { loadedId: string | null; memory: number | null } {
  const loaded = status?.models.find((m) => m.status === "loaded") ?? null;
  return { loadedId: loaded?.id ?? null, memory: loaded?.memory_effective_bytes ?? null };
}
