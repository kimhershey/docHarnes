import { ExternalLink, FolderOpen, Play, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { THEME_ICONS } from "@/app/layout/ThemeMenu";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { isTauri, openExternal, revealPath } from "@/lib/ipc";
import { invalidateMaintenance, useAppInfo, useMaintenanceStatus, useRegistry } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import { LANGUAGES, RETENTION_DAY_ITEMS, THEMES, isJobFinished, type AutoProcessingLevel, type ReducedMotion, type RetentionPolicy, AUTO_PROCESSING_LEVELS } from "@/lib/types";
import { describeError } from "@/lib/errors";
import { cn, formatDate } from "@/lib/utils";
import { api } from "@/lib/ipc";
import { useJob } from "@/stores/jobs";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

import { maintenanceSkippedText, maintenanceSummaryText } from "./maintenance";

const TABS = ["general", "theme", "workspace", "processing", "knowledge", "privacy", "shortcuts", "about"] as const;

function Row({ label, hint, children, htmlFor }: { label: string; hint?: React.ReactNode; children: React.ReactNode; htmlFor?: string }) {
  return (
    <div className="grid gap-2 py-3 md:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)] md:items-center">
      <div>
        <Label htmlFor={htmlFor}>{label}</Label>
        {hint && <p className="mt-1 text-xs text-fg-muted">{hint}</p>}
      </div>
      <div className="flex items-center gap-3">{children}</div>
    </div>
  );
}

export function SettingsPage() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings);
  if (!settings) return null;
  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-4 p-6" data-testid="page-settings">
      <h1 className="text-xl font-semibold">{t("settings.title")}</h1>
      <Tabs defaultValue="general">
        <TabsList>
          {TABS.map((tab) => (
            <TabsTrigger key={tab} value={tab} data-testid={`settings-tab-${tab}`}>
              {t(`settings.tabs.${tab}`)}
            </TabsTrigger>
          ))}
        </TabsList>
        <TabsContent value="general">
          <GeneralTab />
        </TabsContent>
        <TabsContent value="theme">
          <ThemeTab />
        </TabsContent>
        <TabsContent value="workspace">
          <WorkspaceTab />
        </TabsContent>
        <TabsContent value="processing">
          <div className="flex flex-col gap-4">
            <ProcessingTab />
            <GenerationCard />
          </div>
        </TabsContent>
        <TabsContent value="knowledge">
          <KnowledgeTab />
        </TabsContent>
        <TabsContent value="privacy">
          <PrivacyTab />
        </TabsContent>
        <TabsContent value="shortcuts">
          <ShortcutsTab />
        </TabsContent>
        <TabsContent value="about">
          <AboutTab />
        </TabsContent>
      </Tabs>
    </div>
  );
}

function GeneralTab() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings)!;
  const setLanguage = useSettingsStore((s) => s.setLanguage);
  const setScale = useSettingsStore((s) => s.setScale);
  const update = useSettingsStore((s) => s.update);
  const [scale, setLocalScale] = useState(settings.ui.scale_percent);
  useEffect(() => setLocalScale(settings.ui.scale_percent), [settings.ui.scale_percent]);

  return (
    <Card>
      <CardContent className="divide-y divide-border pt-2">
        <Row label={t("settings.general.language")} hint={t("settings.general.language_hint")}>
          <Select value={settings.ui.language} onValueChange={(v) => void setLanguage(v as "ko" | "en")}>
            <SelectTrigger className="w-48" data-testid="language-select">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {LANGUAGES.map((l) => (
                <SelectItem key={l} value={l} data-testid={`language-item-${l}`}>
                  {t(`language.${l}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row label={t("settings.general.scale")} hint={t("settings.general.scale_hint")}>
          <Slider min={90} max={150} step={10} value={[scale]} onValueChange={([v]) => setLocalScale(v ?? 100)} onValueCommit={([v]) => void setScale(v ?? 100)} className="w-56" aria-label={t("settings.general.scale")} data-testid="scale-slider" />
          <span className="w-12 text-sm tabular-nums" data-testid="scale-value">
            {scale}%
          </span>
        </Row>
        <Row label={t("settings.general.reduced_motion")}>
          <Select value={settings.ui.reduced_motion} onValueChange={(v) => void update((s) => void (s.ui.reduced_motion = v as ReducedMotion))}>
            <SelectTrigger className="w-48">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="system">{t("settings.general.reduced_motion_system")}</SelectItem>
              <SelectItem value="on">{t("settings.general.reduced_motion_on")}</SelectItem>
              <SelectItem value="off">{t("settings.general.reduced_motion_off")}</SelectItem>
            </SelectContent>
          </Select>
        </Row>
        <Row label={t("settings.general.sidebar")}>
          <Switch checked={settings.ui.sidebar_collapsed} onCheckedChange={(v) => void update((s) => void (s.ui.sidebar_collapsed = v))} />
        </Row>
        <Row label={t("settings.notifications.job_complete")} hint={t("settings.notifications.job_complete_hint")}>
          <Switch checked={settings.notifications.job_complete} onCheckedChange={(v) => void update((s) => void (s.notifications.job_complete = v))} aria-label={t("settings.notifications.job_complete")} data-testid="settings-notify-job-complete" />
        </Row>
        <Row label={t("settings.notifications.prevent_sleep")} hint={t("settings.notifications.prevent_sleep_hint")}>
          <Switch checked={settings.notifications.prevent_sleep_during_jobs} onCheckedChange={(v) => void update((s) => void (s.notifications.prevent_sleep_during_jobs = v))} aria-label={t("settings.notifications.prevent_sleep")} data-testid="settings-prevent-sleep" />
        </Row>
      </CardContent>
    </Card>
  );
}

function ThemeTab() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings)!;
  const setTheme = useSettingsStore((s) => s.setTheme);
  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("settings.theme.title")}</CardTitle>
        <CardDescription>{t("settings.theme.hint")}</CardDescription>
      </CardHeader>
      <CardContent className="grid gap-3 md:grid-cols-2">
        {THEMES.map((theme) => {
          const Icon = THEME_ICONS[theme];
          const selected = settings.ui.theme === theme;
          return (
            <button
              key={theme}
              type="button"
              onClick={() => void setTheme(theme)}
              className={cn("flex flex-col gap-2 rounded-lg border p-4 text-left transition-colors cursor-pointer", selected ? "border-accent bg-accent-soft" : "border-border hover:border-border-strong")}
              aria-pressed={selected}
              data-testid={`theme-card-${theme}`}
            >
              <div className="flex items-center gap-2 font-medium">
                <Icon className="size-4" aria-hidden="true" />
                {t(`theme.${theme}`)}
                {selected && <Badge className="ml-auto">{t("common.select")}</Badge>}
              </div>
              <p className="text-xs text-fg-muted">{t(`theme.desc.${theme}`)}</p>
              <ThemePreview theme={theme} />
            </button>
          );
        })}
      </CardContent>
    </Card>
  );
}

/** Mini preview rendered with the target theme's tokens (scoped data-theme). */
function ThemePreview({ theme }: { theme: (typeof THEMES)[number] }) {
  const { t } = useTranslation();
  const resolved = theme === "system" ? undefined : theme;
  return (
    <div data-theme={resolved} className="rounded-md border border-border p-3 text-xs" style={{ background: "var(--bg)", color: "var(--fg)" }}>
      <div className="font-medium">{t("theme.preview_title")}</div>
      <p style={{ color: "var(--fg-muted)" }}>{t("theme.preview_body")}</p>
      <div className="mt-2 flex gap-1.5">
        <span className="rounded px-2 py-0.5" style={{ background: "var(--accent)", color: "var(--accent-fg)" }}>
          Aa
        </span>
        <span className="rounded px-2 py-0.5" style={{ background: "var(--ok-soft)", color: "var(--ok)" }}>
          ✔
        </span>
        <span className="rounded px-2 py-0.5" style={{ background: "var(--warn-soft)", color: "var(--warn)" }}>
          ▲
        </span>
        <span className="rounded px-2 py-0.5" style={{ background: "var(--danger-soft)", color: "var(--danger)" }}>
          ✖
        </span>
      </div>
    </div>
  );
}

function WorkspaceTab() {
  const { t, i18n } = useTranslation();
  const settings = useSettingsStore((s) => s.settings)!;
  const workspace = useWorkspaceStore((s) => s.current);
  const close = useWorkspaceStore((s) => s.close);
  const open = useWorkspaceStore((s) => s.open);
  const forget = useWorkspaceStore((s) => s.forget);
  const openDialog = useUiStore((s) => s.openWorkspaceDialog);
  const toast = useUiStore((s) => s.toast);
  const { data: info } = useAppInfo();

  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle>{t("settings.workspace.active")}</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3 text-sm">
          {workspace ? (
            <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1">
              <dt className="text-fg-muted">{t("common.name")}</dt>
              <dd>{workspace.manifest.name}</dd>
              <dt className="text-fg-muted">{t("common.path")}</dt>
              <dd className="selectable" data-testid="workspace-path-display">
                {workspace.path}
              </dd>
              <dt className="text-fg-muted">{t("settings.workspace.models_location")}</dt>
              <dd>{t("settings.workspace.models_in_workspace")}</dd>
              <dt className="text-fg-muted">{t("settings.workspace.created")}</dt>
              <dd>{formatDate(workspace.manifest.created_at, i18n.language)}</dd>
              <dt className="text-fg-muted">{t("settings.workspace.id")}</dt>
              <dd className="selectable text-xs text-fg-subtle">{workspace.manifest.workspace_id}</dd>
            </dl>
          ) : (
            <p className="text-fg-muted">{t("settings.workspace.none")}</p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button variant="secondary" size="sm" onClick={() => openDialog("open")} data-testid="settings-open-workspace">
              <FolderOpen />
              {t("settings.workspace.change")}
            </Button>
            <Button variant="secondary" size="sm" onClick={() => openDialog("create")}>
              {t("settings.workspace.new")}
            </Button>
            {workspace && (
              <Button variant="outline" size="sm" onClick={() => void revealPath(workspace.path)} disabled={!isTauri}>
                {t("common.open_folder")}
              </Button>
            )}
            {workspace && (
              <Button variant="ghost" size="sm" onClick={() => void close()}>
                {t("settings.workspace.close")}
              </Button>
            )}
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{t("settings.workspace.recent")}</CardTitle>
        </CardHeader>
        <CardContent>
          {settings.workspaces.length === 0 ? (
            <p className="text-sm text-fg-muted">{t("common.none")}</p>
          ) : (
            <ul className="divide-y divide-border text-sm">
              {settings.workspaces.map((w) => (
                <li key={w.path} className="flex items-center gap-2 py-2">
                  <div className="min-w-0 flex-1">
                    <div className="truncate font-medium">{w.name}</div>
                    <div className="truncate text-xs text-fg-subtle selectable">{w.path}</div>
                  </div>
                  {w.path !== workspace?.path && (
                    <Button variant="outline" size="sm" onClick={() => void open(w.path).catch((e) => toast(describeError(e, t), "danger"))}>
                      {t("common.open")}
                    </Button>
                  )}
                  <Button variant="ghost" size="sm" onClick={() => void forget(w.path)} aria-label={t("settings.workspace.forget")}>
                    <Trash2 />
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{t("settings.workspace.settings_file")}</CardTitle>
          <CardDescription>{t("settings.workspace.settings_file_hint")}</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-wrap items-center gap-3 text-sm">
          <code className="selectable rounded bg-bg-sunken px-2 py-1 text-xs">{info?.settings_path ?? "…"}</code>
          <Button variant="outline" size="sm" disabled={!isTauri || !info} onClick={() => info && void revealPath(info.settings_path)}>
            {t("settings.workspace.open_file")}
          </Button>
        </CardContent>
      </Card>
    </div>
  );
}

function ProcessingTab() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings)!;
  const update = useSettingsStore((s) => s.update);
  const reload = useSettingsStore((s) => s.load);
  const p = settings.processing;
  // The threshold is read by every new extraction and by "검증 다시 실행": drop the cached pipeline
  // queries so the status / queue pick the new value up on their next fetch (review M8).
  const commitThreshold = async (v: number) => {
    try {
      await update((s) => void (s.processing.review_threshold = v));
    } finally {
      void queryClient.invalidateQueries({ queryKey: queryKeys.pipeline });
    }
  };
  return (
    <Card>
      <CardHeader>
        <CardDescription>{t("settings.processing.hint")}</CardDescription>
      </CardHeader>
      <CardContent className="divide-y divide-border">
        <Row label={t("settings.processing.auto_level")}>
          <Select value={p.auto_level} onValueChange={(v) => void update((s) => void (s.processing.auto_level = v as AutoProcessingLevel))}>
            <SelectTrigger className="w-56">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {AUTO_PROCESSING_LEVELS.map((lvl) => (
                <SelectItem key={lvl} value={lvl}>
                  {t(`settings.processing.auto_levels.${lvl}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row label={t("settings.processing.inbox_auto_level")}>
          <Select value={p.inbox_auto_level} onValueChange={(v) => void update((s) => void (s.processing.inbox_auto_level = v as AutoProcessingLevel))}>
            <SelectTrigger className="w-56" data-testid="settings-inbox-auto-level">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {AUTO_PROCESSING_LEVELS.map((lvl) => (
                <SelectItem key={lvl} value={lvl}>
                  {t(`settings.processing.auto_levels.${lvl}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row label={t("settings.processing.ocr_digital_pdf")} hint={t("settings.processing.ocr_digital_pdf_hint")}>
          <Switch checked={p.ocr_digital_pdf} onCheckedChange={(v) => void update((s) => void (s.processing.ocr_digital_pdf = v))} data-testid="settings-ocr-digital" />
        </Row>
        <Row label={t("settings.processing.kie_max_pages")} hint={t("settings.processing.kie_max_pages_hint")} htmlFor="kie-max-pages">
          <NumberField id="kie-max-pages" value={p.kie_max_pages} min={1} max={32} step={1} onCommit={(v) => void update((s) => void (s.processing.kie_max_pages = v))} testId="settings-kie-max-pages" />
        </Row>
        <Row label={t("settings.processing.precise_mode")} hint={t("settings.processing.precise_hint")}>
          <Switch checked={p.precise_mode} onCheckedChange={(v) => void update((s) => void (s.processing.precise_mode = v))} data-testid="settings-precise-mode" />
        </Row>
        <Row label={t("settings.processing.dense_reocr")} hint={t("settings.processing.dense_reocr_hint", { cells: p.dense_cells })}>
          <Switch checked={p.dense_reocr} onCheckedChange={(v) => void update((s) => void (s.processing.dense_reocr = v))} data-testid="settings-dense-reocr" />
        </Row>
        <Row label={t("settings.processing.dense_cells")} htmlFor="dense-cells">
          <NumberField id="dense-cells" value={p.dense_cells} min={10} max={2000} step={10} onCommit={(v) => void update((s) => void (s.processing.dense_cells = v))} testId="settings-dense-cells" />
        </Row>
        <Row label={t("settings.processing.render_dpi")} htmlFor="render-dpi">
          <NumberField id="render-dpi" value={p.render_dpi} min={72} max={600} step={10} onCommit={(v) => void update((s) => void (s.processing.render_dpi = v))} testId="settings-render-dpi" />
        </Row>
        <Row
          label={t("settings.processing.review_threshold")}
          hint={
            <>
              {t("settings.processing.review_threshold_hint")}
              <span className="mt-0.5 block" data-testid="settings-review-threshold-scope">
                {t("settings.processing.review_threshold_scope")}
              </span>
            </>
          }
        >
          <Slider min={0.5} max={1} step={0.05} value={[p.review_threshold]} onValueCommit={([v]) => void commitThreshold(v ?? 0.8)} className="w-56" aria-label={t("settings.processing.review_threshold")} data-testid="settings-review-threshold" />
          <span className="w-12 text-sm tabular-nums">{p.review_threshold.toFixed(2)}</span>
        </Row>
        <Row label={t("settings.processing.audit_sample")} htmlFor="audit-sample">
          <NumberField id="audit-sample" value={p.audit_sample_percent} min={0} max={100} step={1} onCommit={(v) => void update((s) => void (s.processing.audit_sample_percent = v))} testId="settings-audit-sample" />
          <span className="text-sm text-fg-muted">%</span>
        </Row>
        <Row label={t("settings.processing.organize_reviewed_only")}>
          <Switch checked={p.organize_reviewed_only} onCheckedChange={(v) => void update((s) => void (s.processing.organize_reviewed_only = v))} />
        </Row>
        <Row label={t("settings.processing.inbox_watch")} hint={t("library.inbox_watch_hint")}>
          <Switch
            checked={p.inbox_watch}
            onCheckedChange={(v) =>
              void api
                .documentsSetInboxWatch(v)
                .then(() => reload())
                .catch(() => update((s) => void (s.processing.inbox_watch = v)))
            }
          />
        </Row>
        <Row label={t("settings.processing.cache")} htmlFor="cache-gb">
          <NumberField id="cache-gb" value={settings.cache.page_cache_max_gb} min={0.5} max={1000} step={0.5} precision={1} onCommit={(v) => void update((s) => void (s.cache.page_cache_max_gb = v))} testId="settings-cache-gb" />
          <span className="text-sm text-fg-muted">GB</span>
        </Row>
      </CardContent>
    </Card>
  );
}

/** Reply token caps of document generation by engine backend (0.3.0 item 4; user decision: GPU raised, CPU limited). */
function GenerationCard() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings)!;
  const update = useSettingsStore((s) => s.update);
  const g = settings.generation;
  return (
    <Card data-testid="settings-generation">
      <CardHeader>
        <CardTitle>{t("settings.generation.title")}</CardTitle>
        <CardDescription>{t("settings.generation.hint")}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <Row label={t("settings.generation.max_tokens_gpu")} hint={t("settings.generation.max_tokens_gpu_hint")} htmlFor="gen-cap-gpu">
          <NumberField id="gen-cap-gpu" value={g.max_tokens_gpu} min={1200} max={65536} step={1024} onCommit={(v) => void update((s) => void (s.generation.max_tokens_gpu = v))} testId="settings-gen-cap-gpu" />
        </Row>
        <Row label={t("settings.generation.max_tokens_cpu")} hint={t("settings.generation.max_tokens_cpu_hint")} htmlFor="gen-cap-cpu">
          <NumberField id="gen-cap-cpu" value={g.max_tokens_cpu} min={1200} max={12288} step={512} onCommit={(v) => void update((s) => void (s.generation.max_tokens_cpu = v))} testId="settings-gen-cap-cpu" />
        </Row>
      </CardContent>
    </Card>
  );
}

/**
 * Numeric setting edited as free text; clamped (rounded to `precision` decimals) and persisted on blur /
 * Enter, not per keystroke. Controlled: while not being edited it shows the live value, so an external
 * settings reload refreshes it.
 */
function NumberField({ id, value, min, max, step, precision = 0, onCommit, testId }: { id: string; value: number; min: number; max: number; step: number; precision?: number; onCommit: (v: number) => void; testId: string }) {
  const [draft, setDraft] = useState(String(value));
  const [editing, setEditing] = useState(false);
  const shown = editing ? draft : String(value);
  const commit = () => {
    setEditing(false);
    const n = Number(draft);
    if (!Number.isFinite(n) || draft.trim() === "") return;
    const factor = 10 ** precision;
    const clamped = Math.min(max, Math.max(min, Math.round(n * factor) / factor));
    if (clamped !== value) onCommit(clamped);
  };
  return (
    <Input
      id={id}
      type="number"
      min={min}
      max={max}
      step={step}
      value={shown}
      onFocus={() => {
        setDraft(String(value));
        setEditing(true);
      }}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
      }}
      className="w-32"
      data-testid={testId}
    />
  );
}

function KnowledgeTab() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings)!;
  const update = useSettingsStore((s) => s.update);
  const { data: registry } = useRegistry();
  const k = settings.knowledge;
  const embeddings = (registry?.models ?? []).filter((m) => m.role === "embedding");
  const DEFAULT = "__default__";
  const numberRow = (key: "context_tokens" | "top_k" | "chunk_target_tokens", label: string, min: number, max: number, step: number) => (
    <Row label={label} htmlFor={`kb-${key}`}>
      <NumberField
        id={`kb-${key}`}
        value={k[key]}
        min={min}
        max={max}
        step={step}
        onCommit={(v) => void update((s) => void (s.knowledge[key] = v))}
        testId={`kb-${key}`}
      />
    </Row>
  );
  return (
    <Card data-testid="settings-knowledge">
      <CardHeader>
        <CardTitle>{t("settings.knowledge.title")}</CardTitle>
        <CardDescription>{t("settings.knowledge.hint")}</CardDescription>
      </CardHeader>
      <CardContent className="divide-y divide-border">
        <Row label={t("settings.knowledge.auto_index")} hint={t("settings.knowledge.auto_index_hint")}>
          <Switch checked={k.auto_index} onCheckedChange={(v) => void update((s) => void (s.knowledge.auto_index = v))} data-testid="kb-auto-index" />
        </Row>
        <Row label={t("settings.knowledge.auto_embed")} hint={t("settings.knowledge.auto_embed_hint")}>
          <Switch checked={k.auto_embed} onCheckedChange={(v) => void update((s) => void (s.knowledge.auto_embed = v))} />
        </Row>
        <Row label={t("settings.knowledge.embedding_model")}>
          <Select value={k.embedding_model ?? DEFAULT} onValueChange={(v) => void update((s) => void (s.knowledge.embedding_model = v === DEFAULT ? null : v))}>
            <SelectTrigger className="w-72" aria-label={t("settings.knowledge.embedding_model")}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={DEFAULT}>{t("settings.knowledge.embedding_default", { model: registry?.default_embedding ?? "-" })}</SelectItem>
              {embeddings.map((m) => (
                <SelectItem key={m.id} value={m.id}>
                  {m.display}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row label={t("settings.knowledge.latest_only")} hint={t("settings.knowledge.latest_only_hint")}>
          <Switch checked={k.latest_only} onCheckedChange={(v) => void update((s) => void (s.knowledge.latest_only = v))} />
        </Row>
        <Row label={t("settings.knowledge.allow_general")} hint={t("settings.knowledge.allow_general_hint")}>
          <Switch checked={k.allow_general_knowledge} onCheckedChange={(v) => void update((s) => void (s.knowledge.allow_general_knowledge = v))} data-testid="kb-allow-general" />
        </Row>
        <Row label={t("settings.knowledge.min_similarity")} hint={t("settings.knowledge.min_similarity_hint")}>
          <div className="flex items-center gap-3">
            <Slider min={0.2} max={0.9} step={0.05} value={[k.min_vector_similarity]} onValueCommit={([v]) => void update((s) => void (s.knowledge.min_vector_similarity = v ?? 0.5))} className="w-56" aria-label={t("settings.knowledge.min_similarity")} />
            <span className="w-10 text-xs tabular-nums">{k.min_vector_similarity.toFixed(2)}</span>
          </div>
        </Row>
        {numberRow("context_tokens", t("settings.knowledge.context_tokens"), 1000, 24000, 500)}
        {numberRow("top_k", t("settings.knowledge.top_k"), 1, 20, 1)}
        {numberRow("chunk_target_tokens", t("settings.knowledge.chunk_target"), 120, 2000, 20)}
      </CardContent>
    </Card>
  );
}

function PrivacyTab() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings)!;
  const update = useSettingsStore((s) => s.update);
  const workspace = useWorkspaceStore((s) => s.current);
  const setRetention = useWorkspaceStore((s) => s.setRetention);
  const toast = useUiStore((s) => s.toast);
  const [draft, setDraft] = useState<RetentionPolicy | null>(workspace?.manifest.retention ?? null);
  useEffect(() => setDraft(workspace?.manifest.retention ?? null), [workspace?.manifest.retention]);

  const save = async () => {
    if (!draft) return;
    try {
      await setRetention(draft);
      toast(t("settings.privacy.saved"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  const setDays = (key: (typeof RETENTION_DAY_ITEMS)[number], value: number | null) => setDraft((d) => (d ? { ...d, [key]: value } : d));

  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardContent className="divide-y divide-border pt-2">
          <Row label={t("settings.privacy.pii_masking")} hint={t("settings.privacy.pii_masking_hint")}>
            <Switch checked={settings.privacy.pii_masking} onCheckedChange={(v) => void update((s) => void (s.privacy.pii_masking = v))} data-testid="pii-masking" />
          </Row>
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>{t("settings.privacy.retention_title")}</CardTitle>
          <CardDescription>{t("settings.privacy.retention_hint")}</CardDescription>
        </CardHeader>
        <CardContent>
          {!draft ? (
            <p className="text-sm text-fg-muted">{t("settings.privacy.requires_workspace")}</p>
          ) : (
            <div className="flex flex-col gap-3" data-testid="retention-form">
              <table className="w-full text-sm">
                <tbody className="divide-y divide-border">
                  {RETENTION_DAY_ITEMS.map((item) => {
                    const value = draft[item];
                    const unlimited = value === null;
                    const unlimitedLabel = item === "extractions_days" ? t("settings.privacy.same_as_documents") : t("settings.privacy.forever");
                    return (
                      <tr key={item}>
                        <td className="py-2 pr-3">{t(`settings.privacy.items.${item}`)}</td>
                        <td className="py-2 pr-3">
                          <div className="flex items-center gap-2">
                            <Input
                              type="number"
                              min={0}
                              max={36500}
                              className="w-24"
                              value={unlimited ? "" : value}
                              disabled={unlimited}
                              onChange={(e) => setDays(item, Math.max(0, Number(e.target.value) || 0))}
                              aria-label={t(`settings.privacy.items.${item}`)}
                              data-testid={`retention-${item}`}
                            />
                            <span className="text-fg-muted">{t("common.days")}</span>
                            {value === 0 && <Badge variant="warn">{t("settings.privacy.immediate")}</Badge>}
                          </div>
                        </td>
                        <td className="py-2">
                          <label className="flex items-center gap-2 text-fg-muted">
                            <Switch checked={unlimited} onCheckedChange={(v) => setDays(item, v ? null : 90)} aria-label={unlimitedLabel} />
                            {unlimitedLabel}
                          </label>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
              <div className="grid gap-3 md:grid-cols-3">
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="page-cache">{t("settings.privacy.page_cache")}</Label>
                  <Input id="page-cache" type="number" min={0.5} max={1000} step={0.5} value={draft.page_cache_max_gb} onChange={(e) => setDraft({ ...draft, page_cache_max_gb: Math.min(1000, Math.max(0.5, Number(e.target.value) || 0.5)) })} />
                </div>
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="purge-hour">{t("settings.privacy.purge_hour")}</Label>
                  <Input id="purge-hour" type="number" min={0} max={23} value={draft.purge_hour_local} onChange={(e) => setDraft({ ...draft, purge_hour_local: Math.min(23, Math.max(0, Number(e.target.value) || 0)) })} data-testid="purge-hour" />
                </div>
                <div className="flex flex-col gap-1.5">
                  <Label>{t("settings.privacy.purge_enabled")}</Label>
                  <Switch checked={draft.purge_enabled} onCheckedChange={(v) => setDraft({ ...draft, purge_enabled: v })} />
                </div>
              </div>
              <div className="flex items-center justify-end">
                <Button onClick={() => void save()} data-testid="retention-save">
                  {t("common.save")}
                </Button>
              </div>
            </div>
          )}
        </CardContent>
      </Card>
      {workspace && <MaintenanceCard />}
    </div>
  );
}

/** P7 retention job: last / next purge run, the last summary and "지금 파기 실행". */
function MaintenanceCard() {
  const { t, i18n } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: status } = useMaintenanceStatus();
  const [jobId, setJobId] = useState<string | null>(null);
  const job = useJob(jobId);
  const running = status?.running || (!!job && !isJobFinished(job));
  useEffect(() => {
    if (job && isJobFinished(job)) {
      setJobId(null);
      invalidateMaintenance();
      if (job.state === "done") toast(t("settings.privacy.maintenance.done"), "ok");
      else if (job.state === "failed") toast(describeError({ code: "internal", message: job.error ?? "" }, t), "danger");
    }
  }, [job, toast, t]);
  const run = async () => {
    try {
      const { job_id } = await api.maintenanceRunNow();
      setJobId(job_id);
      invalidateMaintenance();
      toast(t("settings.privacy.maintenance.started"), "info");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const summary = status?.last_summary ?? null;
  const skipped = summary ? maintenanceSkippedText(summary, t) : null;
  return (
    <Card data-testid="maintenance-card">
      <CardHeader>
        <CardTitle>{t("settings.privacy.maintenance.title")}</CardTitle>
        <CardDescription>{t("settings.privacy.maintenance.hint")}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 text-sm">
        <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1">
          <dt className="text-fg-muted">{t("settings.privacy.maintenance.last_run")}</dt>
          <dd data-testid="maintenance-last-run">{status?.last_run_at ? formatDate(status.last_run_at, i18n.language) : t("settings.privacy.maintenance.never")}</dd>
          <dt className="text-fg-muted">{t("settings.privacy.maintenance.next_run")}</dt>
          <dd data-testid="maintenance-next-run">{status?.enabled && status.next_run_at ? t("settings.privacy.maintenance.next_run_at", { date: formatDate(status.next_run_at, i18n.language), hour: status.purge_hour_local }) : t("settings.privacy.maintenance.disabled")}</dd>
        </dl>
        {summary && (
          <div className="flex flex-col gap-1 text-xs text-fg-muted">
            <p className="flex flex-wrap items-center gap-2" data-testid="maintenance-summary">
              <span>{maintenanceSummaryText(summary, t)}</span>
              {summary.cancelled && (
                <Badge variant="warn" data-testid="maintenance-cancelled">
                  {t("settings.privacy.maintenance.cancelled")}
                </Badge>
              )}
            </p>
            {skipped && <p data-testid="maintenance-skipped">{skipped}</p>}
          </div>
        )}
        {!summary && <p className="text-xs text-fg-subtle">{t("settings.privacy.purge_log_empty")}</p>}
        <div className="flex items-center gap-2" aria-live="polite">
          <Button size="sm" variant="secondary" onClick={() => void run()} disabled={running} data-testid="maintenance-run">
            <Play />
            {running ? t("settings.privacy.maintenance.running") : t("settings.privacy.maintenance.run_now")}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function ShortcutsTab() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings)!;
  return (
    <Card>
      <CardHeader>
        <CardDescription>{t("settings.shortcuts.hint")}</CardDescription>
      </CardHeader>
      <CardContent>
        <table className="w-full text-sm">
          <thead className="text-left text-xs uppercase tracking-wide text-fg-subtle">
            <tr>
              <th className="py-2">{t("settings.shortcuts.action")}</th>
              <th className="py-2">{t("settings.shortcuts.keys")}</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {Object.entries(settings.shortcuts).map(([action, chord]) => (
              <tr key={action}>
                <td className="py-2">{t(`settings.shortcuts.actions.${action}`, { defaultValue: action })}</td>
                <td className="py-2">
                  <Kbd chord={chord} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </CardContent>
    </Card>
  );
}

function AboutTab() {
  const { t } = useTranslation();
  const { data: info } = useAppInfo();
  return (
    <Card>
      <CardHeader>
        <CardTitle>
          {t("app.name")} {info?.version ?? ""}
        </CardTitle>
        <CardDescription>{t("app.tagline")}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 text-sm">
        <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1">
          <dt className="text-fg-muted">{t("settings.about.version")}</dt>
          <dd>{info?.version ?? "…"}</dd>
          <dt className="text-fg-muted">{t("settings.about.runtime")}</dt>
          <dd>{info ? `${info.os} / ${info.arch}${info.debug ? " (debug)" : ""}` : "…"}</dd>
          <dt className="text-fg-muted">{t("settings.about.settings_path")}</dt>
          <dd className="selectable text-xs">{info?.settings_path ?? "…"}</dd>
          <dt className="text-fg-muted">{t("settings.about.app_data")}</dt>
          <dd className="selectable text-xs">{info?.app_data_dir ?? "…"}</dd>
        </dl>
        <div>
          <p className="font-medium">{t("settings.about.licenses")}</p>
          <p className="text-fg-muted">{t("settings.about.licenses_hint")}</p>
          <p className="text-fg-muted">{t("settings.about.font")}</p>
          <div className="mt-2 flex flex-wrap gap-2">
            <Button variant="outline" size="sm" onClick={() => void openExternal("https://github.com/kimhershey/docHarnes/blob/main/THIRD-PARTY-NOTICES.md")}>
              <ExternalLink />
              {t("settings.about.notices")}
            </Button>
            <Button variant="outline" size="sm" onClick={() => void openExternal("https://github.com/kimhershey/docHarnes/blob/main/docs/design/doculens-design-v1.1.md")}>
              <ExternalLink />
              {t("settings.about.design_doc")}
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
