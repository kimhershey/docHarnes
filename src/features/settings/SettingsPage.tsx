import { ExternalLink, FolderOpen, Trash2 } from "lucide-react";
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
import { useAppInfo } from "@/lib/queries";
import { LANGUAGES, RETENTION_DAY_ITEMS, THEMES, type AutoProcessingLevel, type ReducedMotion, type RetentionPolicy } from "@/lib/types";
import { describeError } from "@/lib/errors";
import { cn, formatDate } from "@/lib/utils";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

const TABS = ["general", "theme", "workspace", "processing", "privacy", "shortcuts", "about"] as const;

function Row({ label, hint, children, htmlFor }: { label: string; hint?: string; children: React.ReactNode; htmlFor?: string }) {
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
          <ProcessingTab />
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
          <Slider min={90} max={150} step={10} value={[scale]} onValueChange={([v]) => setLocalScale(v ?? 100)} onValueCommit={([v]) => void setScale(v ?? 100)} className="w-56" aria-label={t("settings.general.scale")} />
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
  const p = settings.processing;
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
              {(["classify_only", "classify_and_ocr", "full"] as const).map((lvl) => (
                <SelectItem key={lvl} value={lvl}>
                  {t(`settings.processing.auto_levels.${lvl}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row label={t("settings.processing.render_dpi")} htmlFor="render-dpi">
          <Input id="render-dpi" type="number" min={72} max={600} step={10} className="w-28" defaultValue={p.render_dpi} onBlur={(e) => void update((s) => void (s.processing.render_dpi = Number(e.target.value) || 200))} />
        </Row>
        <Row label={t("settings.processing.review_threshold")} hint={t("settings.processing.review_threshold_hint")}>
          <Slider min={0.5} max={1} step={0.05} value={[p.review_threshold]} onValueCommit={([v]) => void update((s) => void (s.processing.review_threshold = v ?? 0.8))} className="w-56" aria-label={t("settings.processing.review_threshold")} />
          <span className="w-12 text-sm tabular-nums">{p.review_threshold.toFixed(2)}</span>
        </Row>
        <Row label={t("settings.processing.audit_sample")} htmlFor="audit-sample">
          <Input id="audit-sample" type="number" min={0} max={100} className="w-28" defaultValue={p.audit_sample_percent} onBlur={(e) => void update((s) => void (s.processing.audit_sample_percent = Math.min(100, Math.max(0, Number(e.target.value) || 0))))} />
          <span className="text-sm text-fg-muted">%</span>
        </Row>
        <Row label={t("settings.processing.organize_reviewed_only")}>
          <Switch checked={p.organize_reviewed_only} onCheckedChange={(v) => void update((s) => void (s.processing.organize_reviewed_only = v))} />
        </Row>
        <Row label={t("settings.processing.inbox_watch")}>
          <Switch checked={p.inbox_watch} onCheckedChange={(v) => void update((s) => void (s.processing.inbox_watch = v))} />
        </Row>
        <Row label={t("settings.processing.cache")} htmlFor="cache-gb">
          <Input id="cache-gb" type="number" min={0.5} max={1000} step={0.5} className="w-28" defaultValue={settings.cache.page_cache_max_gb} onBlur={(e) => void update((s) => void (s.cache.page_cache_max_gb = Number(e.target.value) || 5))} />
          <span className="text-sm text-fg-muted">GB</span>
        </Row>
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
              <div className="flex items-center justify-between">
                <span className="text-xs text-fg-subtle">{t("settings.privacy.purge_log_empty")}</span>
                <Button onClick={() => void save()} data-testid="retention-save">
                  {t("common.save")}
                </Button>
              </div>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
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
