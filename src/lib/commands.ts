// Executes pre-routed operations (theme / language / open / help) — shared by
// the command palette, the home command bar and the assistant composer — and,
// since P5, every planner op the UI performs itself (`OpKind::runs_in_ui`:
// settings, navigation, job / engine control, reindex, inbox watch, help).

import type { TFunction } from "i18next";
import type { NavigateFunction } from "react-router";

import { describeError } from "./errors";
import { api } from "./ipc";
import { queryClient, queryKeys } from "./query-client";
import { coerceSettingValue, getSettingByPath, resolveSettingKey, setSettingByPath, settingSpec } from "./settingsCatalog";
import { SCREEN_ROUTES, isJobFinished, type AppSettings, type BackendPreference, type DocumentView, type Job, type JobOp, type JobState, type Language, type Op, type OpKind, type PreRouteResult, type RetentionPolicy, type RetentionTarget, type Screen, type Theme, type Tier, type UndoReport } from "./types";
import { errorCode, errorMessage } from "./utils";
import { chatWritesSettled } from "@/stores/assistant";
import { useDocumentsStore } from "@/stores/documents";
import { useEngineStore } from "@/stores/engine";
import { useJobsStore } from "@/stores/jobs";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

export interface ExecuteOutcome {
  /** Human-readable summary of what happened (already translated). */
  summary: string;
  /** Ops that put back what this one replaced (a settings change: the previous value). Absent when nothing changed. */
  undo?: Op[];
  /**
   * What the change left behind, as settings ops (the setting and the value it has now): `undo` only runs while
   * the settings still say so (`undoSettingsChange`). A later change of the same setting makes the undo stale —
   * replayed, it would put a value back that is no longer "the previous one".
   */
  applied?: Op[];
}

/** The setting a settings op writes and the value it sets (`null` for an op that is no settings change). */
export function settingOfOp(op: Op): { path: string; value: unknown } | null {
  switch (op.op) {
    case "set_setting":
      return op.key === "journal_days" ? null : { path: resolveSettingKey(op.key), value: op.value };
    case "set_theme":
      return { path: "ui.theme", value: op.theme };
    case "set_language":
      return { path: "ui.language", value: op.lang };
    case "set_profile":
      return { path: "models.profile", value: op.profile };
    case "set_inbox_watch":
      return { path: "processing.inbox_watch", value: op.enabled };
    default:
      return null;
  }
}

/** Whether the settings still hold every value of `applied` (see `ExecuteOutcome.applied`). */
export function settingsStillHold(applied: Op[], settings: AppSettings | null): boolean {
  if (!settings) return false;
  return applied.every((op) => {
    const s = settingOfOp(op);
    return s !== null && getSettingByPath(settings, s.path) === s.value;
  });
}

export function describePreRoute(result: PreRouteResult, t: TFunction): string {
  switch (result.op) {
    case "set_theme":
      return t("palette.set_theme", { theme: t(`theme.${result.theme}`) });
    case "set_language":
      return t("palette.set_language", { language: t(`language.${result.language}`) });
    case "open":
      return t("palette.open_screen", { screen: t(`nav.${result.screen}`) });
    case "help":
      return t("palette.help");
    default:
      return "";
  }
}

export async function executePreRoute(result: PreRouteResult, navigate: NavigateFunction, t: TFunction): Promise<ExecuteOutcome> {
  const settings = useSettingsStore.getState();
  const ui = useUiStore.getState();
  switch (result.op) {
    case "set_theme": {
      const before = settings.settings?.ui.theme;
      await settings.setTheme(result.theme);
      const summary = t("theme.changed", { theme: t(`theme.${result.theme}`) });
      ui.toast(summary, "ok");
      return { summary, ...(before && before !== result.theme ? { undo: [{ op: "set_theme", theme: before }], applied: [{ op: "set_theme", theme: result.theme }] } : {}) };
    }
    case "set_language": {
      const before = settings.settings?.ui.language;
      await settings.setLanguage(result.language);
      const summary = t("language.changed", { language: t(`language.${result.language}`) });
      ui.toast(summary, "ok");
      return { summary, ...(before && before !== result.language ? { undo: [{ op: "set_language", lang: before }], applied: [{ op: "set_language", lang: result.language }] } : {}) };
    }
    case "open": {
      navigate(SCREEN_ROUTES[result.screen]);
      return { summary: t("palette.open_screen", { screen: t(`nav.${result.screen}`) }) };
    }
    case "help": {
      ui.setPendingAssistantInput("/help");
      navigate(SCREEN_ROUTES.assistant);
      return { summary: t("palette.help") };
    }
    default:
      return { summary: "" };
  }
}

// ---------------------------------------------------------------------------
// Planner UI ops (P5)
// ---------------------------------------------------------------------------

const SCREENS = Object.keys(SCREEN_ROUTES) as Screen[];

function key(s: string): string {
  return s.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
}

/** Resolves a screen reference (`settings`, "설정", "Settings") to a screen key. */
export function resolveScreen(ref: string, t: TFunction): Screen | null {
  const k = key(ref);
  if (!k) return null;
  return SCREENS.find((s) => s === k) ?? SCREENS.find((s) => key(t(`nav.${s}`)) === k) ?? SCREENS.find((s) => key(t(`nav.${s}`)).includes(k)) ?? null;
}

/** Resolves a document reference (id, file name or title) to a document. */
export async function resolveDocument(ref: string): Promise<DocumentView | null> {
  const r = ref.trim();
  if (!r) return null;
  if (/^[0-9a-f-]{16,}$/i.test(r)) {
    try {
      return await api.documentsGet(r);
    } catch {
      /* not an id */
    }
  }
  let docs: DocumentView[] = [];
  try {
    docs = await api.documentsList({ query: r, limit: 50, sort: "imported_desc" });
  } catch {
    return null;
  }
  const k = key(r);
  const exact = docs.find((d) => key(d.original_name) === k || key(d.title ?? "") === k || key(d.original_name.replace(/\.[^.]+$/, "")) === k);
  return exact ?? docs.find((d) => key(d.original_name).includes(k) || key(d.title ?? "").includes(k)) ?? docs[0] ?? null;
}


function toNumber(v: unknown): number {
  const n = typeof v === "number" ? v : Number(String(v).replace(/[^0-9.-]/g, ""));
  if (!Number.isFinite(n)) throw new Error(`invalid number: ${String(v)}`);
  return n;
}


/**
 * Performs one UI-side op of a plan (or of an execution's `ui_actions`).
 * Returns the translated summary shown on the executed card; errors are
 * thrown for the caller to toast.
 */
export async function executeUiOp(op: Op, navigate: NavigateFunction, t: TFunction): Promise<ExecuteOutcome> {
  const settings = useSettingsStore.getState();
  const ui = useUiStore.getState();
  const done = (summary: string, kind: "ok" | "info" = "ok"): ExecuteOutcome => {
    if (summary) ui.toast(summary, kind);
    return { summary };
  };
  switch (op.op) {
    case "set_theme":
      return executePreRoute({ matched: true, source: "phrase", confidence: 1, normalized_text: "", op: "set_theme", theme: op.theme }, navigate, t);
    case "set_language":
      return executePreRoute({ matched: true, source: "phrase", confidence: 1, normalized_text: "", op: "set_language", language: op.lang }, navigate, t);
    case "help":
      return executePreRoute({ matched: true, source: "phrase", confidence: 1, normalized_text: "", op: "help" }, navigate, t);
    case "set_setting": {
      // 0.6.0: `key` is a settings-catalog path (legacy aliases resolve to one); `journal_days` is a retention period.
      if (op.key === "journal_days") {
        const ws = useWorkspaceStore.getState();
        if (!ws.current) throw new Error("no_workspace");
        const n = Math.max(0, Math.round(toNumber(op.value)));
        await ws.setRetention({ ...ws.current.manifest.retention, journal_days: n === 0 ? null : n });
        return done(t("planner.ui.set_setting", { key: t(SETTING_LABELS.journal_days!), value: String(n) }));
      }
      const path = resolveSettingKey(op.key);
      const spec = settingSpec(path);
      if (!spec) throw new Error(t("planner.ui.unknown_setting", { key: op.key }));
      const value = coerceSettingValue(spec, op.value);
      const before = settings.settings ? getSettingByPath(settings.settings, path) : undefined;
      // "티어 자동" clears the manual tier: what it was is kept here, for the undo (it is gone afterwards).
      const manualTierBefore = settings.settings?.models.manual_tier ?? null;
      switch (path) {
        case "ui.theme":
          await settings.setTheme(value as Theme);
          break;
        case "ui.language":
          await settings.setLanguage(value as Language);
          break;
        case "ui.scale_percent":
          await settings.setScale(value as number);
          break;
        case "models.tier_mode":
          await settings.setTierMode(value as "auto" | "manual", value === "manual" ? (settings.settings?.models.manual_tier ?? "vram_8gb") : null);
          break;
        case "models.manual_tier":
          await settings.setTierMode("manual", value as Tier);
          break;
        case "models.active_model":
          await settings.setActiveModel(String(value));
          // "auto" is a choice, not a model: the engine picks one, there is nothing to load by that name.
          if (String(value) !== "auto") await useEngineStore.getState().loadModel(String(value));
          break;
        case "processing.inbox_watch":
          await api.documentsSetInboxWatch(value as boolean);
          await settings.update((d) => setSettingByPath(d, path, value));
          break;
        default:
          await settings.update((d) => setSettingByPath(d, path, value));
          break;
      }
      // What was stored is what is reported (the store normalises on save), next to what it replaced.
      const stored = useSettingsStore.getState().settings;
      const after = stored ? getSettingByPath(stored, path) : value;
      const label = (v: unknown) => settingValueLabel(path, v as string | number | boolean | null, t);
      if (before === undefined || before === null) return done(t("planner.ui.set_setting", { key: t(spec.label), value: label(after) }));
      if (before === after) return done(t("planner.ui.set_setting_same", { key: t(spec.label), value: label(after) }), "info");
      // Back from "auto" to the manual mode means back to the tier that was chosen (setting the tier sets the mode).
      const undo: Op[] =
        path === "models.tier_mode" && before === "manual" && manualTierBefore
          ? [{ op: "set_setting", key: "models.manual_tier", value: manualTierBefore }]
          : [{ op: "set_setting", key: path, value: before as string | number | boolean }];
      return { ...done(t("planner.ui.set_setting_changed", { key: t(spec.label), from: label(before), to: label(after) })), undo, applied: [{ op: "set_setting", key: path, value: after as string | number | boolean }] };
    }
    case "set_model": {
      await settings.setActiveModel(op.id);
      await useEngineStore.getState().loadModel(op.id);
      return done(t("planner.ui.set_model", { id: op.id }), "info");
    }
    case "set_profile": {
      const before = settings.settings?.models.profile;
      await settings.update((d) => void (d.models.profile = op.profile));
      const said = done(t("planner.ui.set_profile", { profile: t(`models.profiles.${op.profile}`) }));
      return before && before !== op.profile ? { ...said, undo: [{ op: "set_profile", profile: before }], applied: [{ op: "set_profile", profile: op.profile }] } : said;
    }
    case "open": {
      switch (op.target) {
        case "screen": {
          const screen = resolveScreen(op.ref, t);
          if (!screen) throw new Error(t("planner.ui.unknown_screen", { ref: op.ref }));
          navigate(SCREEN_ROUTES[screen]);
          return { summary: t("palette.open_screen", { screen: t(`nav.${screen}`) }) };
        }
        case "project": {
          // The planner resolves a project's name to its id (0.8.1: "한빛 프로젝트 열어줘" opens its page).
          navigate(`/projects/${encodeURIComponent(op.ref.trim())}`);
          return { summary: t("planner.ui.open_project") };
        }
        case "folder": {
          const folder = op.ref.trim().replace(/[\\/]+$/, "");
          const target = folder === "" || folder === "all" || folder === "*" ? null : folder;
          useDocumentsStore.getState().setFolder(target);
          navigate(SCREEN_ROUTES.library);
          return { summary: t("planner.ui.open_folder", { folder: target ?? t("library.folders.all") }) };
        }
        case "page": {
          const m = op.ref.match(/^(.*?)[:：]\s*(\d+)\s*$/);
          const last = ui.lastDocument;
          let doc: DocumentView | null = null;
          let page = 1;
          if (m) {
            page = Number(m[2]);
            doc = m[1]!.trim() ? await resolveDocument(m[1]!) : last ? await resolveDocument(last.id) : null;
          } else if (/^\d+$/.test(op.ref.trim()) && last) {
            page = Number(op.ref.trim());
            doc = await resolveDocument(last.id);
          } else {
            doc = await resolveDocument(op.ref);
          }
          if (!doc) throw new Error(t("planner.ui.document_not_found", { ref: op.ref }));
          navigate(`${SCREEN_ROUTES.library}/${doc.id}?page=${Math.max(1, page)}`);
          return { summary: t("planner.ui.open_page", { name: doc.original_name, page }) };
        }
        case "document":
        default: {
          const doc = await resolveDocument(op.ref);
          if (!doc) throw new Error(t("planner.ui.document_not_found", { ref: op.ref }));
          navigate(`${SCREEN_ROUTES.library}/${doc.id}`);
          return { summary: t("planner.ui.open_document", { name: doc.original_name }) };
        }
      }
    }
    case "define_schema": {
      navigate(SCREEN_ROUTES.schemas);
      return done(t("planner.ui.define_schema", { description: op.description ?? op.name ?? op.base ?? "" }), "info");
    }
    case "job": {
      const jobs = useJobsStore.getState();
      if (op.action === "show") {
        navigate(SCREEN_ROUTES.jobs);
        return { summary: t("palette.open_screen", { screen: t("nav.jobs") }) };
      }
      const target = resolveJob(jobs.jobs, op.id, op.action);
      if (!target) return done(t("planner.ui.job_none"), "info");
      switch (op.action) {
        case "cancel":
          await jobs.cancel(target.id);
          break;
        case "pause":
          await jobs.pause(target.id);
          break;
        case "resume":
          await jobs.resume(target.id);
          break;
        case "retry":
          await jobs.retry(target.id);
          break;
        case "prioritize":
          await jobs.prioritize(target.id);
          break;
      }
      return done(t("planner.ui.job", { action: t(`planner.params.job_${op.action}`), id: target.id.slice(0, 8) }));
    }
    case "engine": {
      const engine = useEngineStore.getState();
      switch (op.action) {
        case "start":
          await engine.start();
          return done(t("planner.ui.engine_start"), "info");
        case "stop":
          await engine.stop();
          return done(t("planner.ui.engine_stop"), "info");
        case "restart":
          await engine.restart();
          return done(t("planner.ui.engine_restart"), "info");
        case "sleep": {
          const loaded = engine.status?.models.filter((m) => m.status === "loaded" || m.status === "sleeping") ?? [];
          for (const m of loaded) await engine.unloadModel(m.id);
          return done(t("planner.ui.engine_sleep", { count: loaded.length }), "info");
        }
        case "wake": {
          const state = engine.status?.state.kind ?? "stopped";
          if (state === "stopped" || state === "degraded") await engine.start();
          else await api.engineRefreshModels(true);
          return done(t("planner.ui.engine_wake"), "info");
        }
      }
      return { summary: "" };
    }
    case "reindex": {
      const scope = op.scope ?? "all";
      if (scope === "fts" || scope === "all") await api.knowledgeIndex(null, true);
      if (scope === "vec" || scope === "all") await api.knowledgeEmbed(true);
      return done(t("planner.ui.reindex"), "info");
    }
    case "set_inbox_watch":
      // "inbox 감시 꺼" (the planner's fast path) is the setting `processing.inbox_watch`: the same card —
      // before → after, "already off", 되돌리기 — as `set_setting` for it.
      return executeUiOp({ op: "set_setting", key: "processing.inbox_watch", value: op.enabled }, navigate, t);
    case "register_form": {
      // §3.4.0 form registration: the schema screen's dialog derives the fields and confirms them in the grid.
      const ref = (op.doc_ref ?? "").trim();
      // Several sample documents (0.3.0 §2) → one discovery job; otherwise the named / open / selected one.
      const many = op.doc_ids && op.doc_ids.length > 0 ? op.doc_ids : useDocumentsStore.getState().selectedIds;
      const docs = many.length >= 2 ? (await Promise.all(many.map((id) => resolveDocument(id)))).filter((d): d is NonNullable<typeof d> => !!d) : [];
      const doc = docs[0] ?? (ref && ref !== "current" ? await resolveDocument(ref) : ui.lastDocument ? await resolveDocument(ui.lastDocument.id) : (useDocumentsStore.getState().selectedIds[0] ? await resolveDocument(useDocumentsStore.getState().selectedIds[0]!) : null));
      if (!doc) throw new Error(t("planner.ui.document_not_found", { ref: ref || t("planner.params.current_document") }));
      const params = new URLSearchParams({ register: docs.length >= 2 ? docs.map((d) => d.id).join(",") : doc.id });
      if (op.name) params.set("name", op.name);
      navigate(`${SCREEN_ROUTES.schemas}?${params.toString()}`);
      return done(t("planner.ui.register_form", { name: docs.length >= 2 ? t("planner.params.sample_docs", { count: docs.length }) : doc.original_name }), "info");
    }
    case "set_retention": {
      const ws = useWorkspaceStore.getState();
      if (!ws.current) throw new Error("no_workspace");
      const next = applyRetention(ws.current.manifest.retention, op.item, op.days);
      await ws.setRetention(next);
      const period = op.days < 0 ? t("common.forever") : op.days === 0 ? t("settings.privacy.immediate") : t("planner.params.days", { count: op.days });
      return done(t("planner.ui.set_retention", { item: t(`planner.params.retention_${op.item}`), period }));
    }
    // -- 0.6.0 (plan §1.5) --
    case "manage_model": {
      const engine = useEngineStore.getState();
      const invalidateModels = () => {
        void queryClient.invalidateQueries({ queryKey: queryKeys.registry });
        void queryClient.invalidateQueries({ queryKey: queryKeys.modelIndex });
        void queryClient.invalidateQueries({ queryKey: queryKeys.assessment });
      };
      switch (op.action) {
        case "set_tier": {
          const tier = op.tier ?? "auto";
          if (tier === "auto") await settings.setTierMode("auto", null);
          else await settings.setTierMode("manual", tier);
          invalidateModels();
          return done(t("planner.ui.model_set_tier", { tier: tier === "auto" ? t("models.tier_auto") : t(`hw.tier.${tier}`) }));
        }
        case "set_backend": {
          const backend: BackendPreference = op.backend ?? "auto";
          await settings.update((d) => void (d.models.backend = backend));
          const state = engine.status?.state.kind ?? "stopped";
          if (state === "running" || state === "degraded") await engine.restart();
          return done(t("planner.ui.model_set_backend", { backend: t(`models.backends.${backend}`) }), "info");
        }
        case "import": {
          const path = (op.path ?? "").trim();
          if (!path) throw new Error(t("planner.ui.path_required"));
          const jobId = await api.modelsImportRun([path], "copy", null);
          invalidateModels();
          return done(t("planner.ui.model_import", { path, job: jobId.slice(0, 8) }), "info");
        }
        case "download": {
          const id = (op.id ?? "").trim();
          if (!id) throw new Error(t("planner.ui.model_id_required"));
          const jobId = await api.modelsDownload(id);
          return done(t("planner.ui.model_download", { id, job: jobId.slice(0, 8) }), "info");
        }
        case "delete": {
          const folder = await resolveModelFolder(op.id ?? "");
          if (!folder) throw new Error(t("planner.ui.model_not_installed", { id: op.id ?? "" }));
          await api.modelsDelete(folder);
          invalidateModels();
          return done(t("planner.ui.model_delete", { id: op.id ?? "" }));
        }
        case "verify": {
          const folder = await resolveModelFolder(op.id ?? "");
          if (!folder) throw new Error(t("planner.ui.model_not_installed", { id: op.id ?? "" }));
          const jobId = await api.modelsHash(folder);
          return done(t("planner.ui.model_verify", { id: op.id ?? "", job: jobId.slice(0, 8) }), "info");
        }
        case "benchmark": {
          const res = await api.engineChatTest(op.id ?? null, null);
          return done(t("planner.ui.model_benchmark", { model: res.model, tps: (res.tokens_per_second ?? 0).toFixed(1), ms: res.elapsed_ms }), "info");
        }
        case "load": {
          const id = (op.id ?? "").trim();
          if (!id) throw new Error(t("planner.ui.model_id_required"));
          await engine.loadModel(id);
          return done(t("planner.ui.model_load", { id }), "info");
        }
        case "unload": {
          const loaded = engine.status?.models.filter((m) => m.status === "loaded" || m.status === "sleeping") ?? [];
          const targets = op.id ? loaded.filter((m) => m.id === op.id) : loaded;
          for (const m of targets) await engine.unloadModel(m.id);
          return done(t("planner.ui.model_unload", { count: targets.length }), "info");
        }
      }
      return { summary: "" };
    }
    case "workspace": {
      const ws = useWorkspaceStore.getState();
      // The conversation so far — this very command and its plan / result cards — belongs to the workspace
      // being left: its writes must reach that database before the backend moves on (afterwards they
      // could only be dropped).
      switch (op.action) {
        case "open": {
          const path = (op.path ?? "").trim();
          if (!path) throw new Error(t("planner.ui.path_required"));
          await chatWritesSettled();
          const view = await ws.open(path);
          navigate(SCREEN_ROUTES.library);
          return done(t("planner.ui.workspace_open", { name: view.manifest.name || path }));
        }
        case "create": {
          const path = (op.path ?? "").trim();
          if (!path) throw new Error(t("planner.ui.path_required"));
          await chatWritesSettled();
          const view = await ws.create(path, null, null);
          navigate(SCREEN_ROUTES.library);
          return done(t("planner.ui.workspace_create", { name: view.manifest.name || path }));
        }
        case "close":
          await chatWritesSettled();
          await ws.close();
          return done(t("planner.ui.workspace_close"), "info");
        case "forget": {
          const path = (op.path ?? ws.current?.path ?? "").trim();
          if (!path) throw new Error(t("planner.ui.path_required"));
          await ws.forget(path);
          return done(t("planner.ui.workspace_forget", { path }), "info");
        }
      }
      return { summary: "" };
    }
    case "maintenance": {
      switch (op.action) {
        case "purge_now": {
          const res = await api.maintenanceRunNow();
          return done(t("planner.ui.maintenance_purge", { job: res.job_id.slice(0, 8) }), "info");
        }
        case "clear_finished_jobs":
          await useJobsStore.getState().clearFinished();
          return done(t("planner.ui.maintenance_clear_jobs"));
        case "rescan": {
          const jobId = await api.documentsRescan();
          return done(jobId ? t("planner.ui.maintenance_rescan") : t("planner.ui.maintenance_rescan_busy"), "info");
        }
        case "inbox_scan":
          await api.documentsInboxScan();
          return done(t("planner.ui.maintenance_inbox_scan"), "info");
      }
      return { summary: "" };
    }
    case "noop":
      return { summary: t("planner.ui.noop") };
    case "clarify":
      return { summary: "" };
    default:
      // Core ops never reach here (the executor runs them); nothing to do in the UI.
      return { summary: "" };
  }
}

/** Human form of a coerced setting value (enum labels, booleans, numbers). */
function settingValueLabel(path: string, value: string | number | boolean | null, t: TFunction): string {
  if (value === null) return t("planner.params.default_value");
  if (typeof value === "boolean") return t(value ? "planner.params.on" : "planner.params.off");
  switch (path) {
    case "ui.theme":
      return t(`theme.${value}`);
    case "ui.language":
      return t(`language.${value}`);
    case "processing.auto_level":
    case "processing.inbox_auto_level":
      return t(`settings.processing.auto_levels.${value}`);
    case "models.profile":
      return t(`models.profiles.${value}`);
    case "models.backend":
      return t(`models.backends.${value}`);
    case "models.manual_tier":
      return t(`hw.tier.${value}`);
    case "models.tier_mode":
      return value === "auto" ? t("models.tier_auto") : t("models.tier_manual");
    case "ui.scale_percent":
      return `${value}%`;
    default:
      return String(value);
  }
}

/** Installed folder of a model id (`manage_model delete / verify`), from the discovered index. */
async function resolveModelFolder(id: string): Promise<string | null> {
  const key = id.trim().toLowerCase();
  if (!key) return null;
  const index = await api.modelsList();
  const hit = index.discovered.find((m) => m.id.toLowerCase() === key) ?? index.discovered.find((m) => m.folder_name.toLowerCase() === key || m.id.toLowerCase().includes(key));
  return hit?.folder ?? null;
}

/** `set_retention` → the retention policy fields of the item (`logs` covers job and app logs); days < 0 = forever. */
export function applyRetention(policy: RetentionPolicy, item: RetentionTarget, days: number): RetentionPolicy {
  const value = days < 0 ? null : Math.max(0, Math.round(days));
  const next: RetentionPolicy = { ...policy };
  switch (item) {
    case "documents":
      next.documents_days = value;
      break;
    case "extractions":
      next.extractions_days = value;
      break;
    case "logs":
      next.job_logs_days = value;
      next.app_logs_days = value;
      break;
    case "journal":
      next.journal_days = value;
      break;
    case "chats":
      next.chats_days = value;
      break;
    case "temp":
      next.temp_days = value;
      break;
  }
  return next;
}

/** True when the new period is shorter than the current one (null = forever); shorter periods are confirmed first. */
export function retentionShortens(policy: RetentionPolicy, item: RetentionTarget, days: number): boolean {
  const keys: (keyof RetentionPolicy)[] = item === "documents" ? ["documents_days"] : item === "extractions" ? ["extractions_days"] : item === "logs" ? ["job_logs_days", "app_logs_days"] : item === "journal" ? ["journal_days"] : item === "chats" ? ["chats_days"] : ["temp_days"];
  if (days < 0) return false;
  return keys.some((k) => {
    const cur = policy[k];
    return cur === null || (typeof cur === "number" && days < cur);
  });
}

const SETTING_LABELS: Record<string, string> = {
  render_dpi: "settings.processing.render_dpi",
  pii_masking: "settings.privacy.pii_masking",
  auto_pipeline: "settings.processing.auto_level",
  inbox_watch: "settings.processing.inbox_watch",
  review_threshold: "settings.processing.review_threshold",
  cache_limit_gb: "planner.params.cache_limit_gb",
  journal_days: "settings.privacy.items.journal_days",
};

/**
 * `job(id: "last")` / no id: the persistent job the action applies to. "작업 일시정지" / "작업 취소" mean
 * the job that is at work — running first, then (cancel only: a paused job cannot be paused again) a
 * paused one, then one still waiting — not simply the newest unfinished job, which is usually a queued
 * one behind it (PLN-15). The other actions take the most recent job in a state they can act on.
 */
export function resolveJob(jobs: Job[], id: string | null | undefined, action: JobOp): Job | null {
  if (id && id !== "last") return jobs.find((j) => j.id === id || j.id.startsWith(id)) ?? null;
  const persistent = jobs.filter((j) => j.persistent);
  const pool = persistent.length > 0 ? persistent : jobs;
  const newest = (list: Job[]): Job | null => [...list].sort((a, b) => b.created_at.localeCompare(a.created_at))[0] ?? null;
  if (action === "pause" || action === "cancel") {
    const order: JobState[] = action === "pause" ? ["running", "queued"] : ["running", "paused", "queued"];
    for (const state of order) {
      const hit = newest(pool.filter((j) => j.state === state));
      if (hit) return hit;
    }
    return null;
  }
  const candidates =
    action === "retry"
      ? pool.filter((j) => j.state === "failed" || j.state === "cancelled" || j.state === "interrupted")
      : action === "resume"
        ? pool.filter((j) => j.state === "paused")
        : action === "prioritize"
          ? pool.filter((j) => j.state === "queued" || j.state === "paused")
          : pool.filter((j) => !isJobFinished(j));
  return newest(candidates);
}

/** What became of one UI-side op: `executeUiOps` returns one per op, in the order of the ops. */
export interface UiOpResult {
  op: OpKind;
  ok: boolean;
  /** Translated summary of what was done (may be empty; empty for a failed op). */
  summary: string;
  /** Translated error of a failed op. */
  error: string | null;
  /** Ops that put back what this op replaced (settings changes). */
  undo?: Op[];
  /** What the op left behind (`ExecuteOutcome.applied`). */
  applied?: Op[];
}

/**
 * Runs several UI ops in order. A failure is toasted and does not stop the ops after it; the result
 * says for every op whether it ran, so the card that asked for them can show a failure as one (a toast
 * alone left the card saying "실행됨", PLN-17).
 */
export async function executeUiOps(ops: Op[], navigate: NavigateFunction, t: TFunction): Promise<UiOpResult[]> {
  const results: UiOpResult[] = [];
  for (const op of ops) {
    try {
      const outcome = await executeUiOp(op, navigate, t);
      results.push({ op: op.op, ok: true, summary: outcome.summary, error: null, ...(outcome.undo ? { undo: outcome.undo } : {}), ...(outcome.applied ? { applied: outcome.applied } : {}) });
    } catch (e) {
      const error = describeError(e, t);
      useUiStore.getState().toast(error, "danger");
      results.push({ op: op.op, ok: false, summary: "", error });
    }
  }
  return results;
}

/**
 * 되돌리기 of a settings change: runs `undo` — unless the settings no longer hold what the change set
 * (`applied`). "배율 120", then "배율 150", then 되돌리기 on the first card must not set 100: 120 is not what
 * would be undone any more. The current values are read from the backend first (the card may be from before a
 * restart, the setting may have been changed on the settings screen). `stale`: nothing was changed.
 * A card without `applied` (stored by an earlier build) is undone as before.
 */
export async function undoSettingsChange(undo: Op[], applied: Op[] | undefined, navigate: NavigateFunction, t: TFunction): Promise<"undone" | "stale" | "failed"> {
  if (undo.length === 0) return "failed";
  if (applied && applied.length > 0) {
    await useSettingsStore.getState().load();
    if (!settingsStillHold(applied, useSettingsStore.getState().settings)) return "stale";
  }
  const results = await executeUiOps(undo, navigate, t);
  return results.every((r) => r.ok) ? "undone" : "failed";
}

// ---------------------------------------------------------------------------
// Journal undo / redo (Ctrl+Z / Ctrl+Y, result card "↶ 되돌리기")
// ---------------------------------------------------------------------------

function isNothingTo(e: unknown, code: "nothing_to_undo" | "nothing_to_redo"): boolean {
  return errorCode(e) === code || errorMessage(e).includes(code);
}

/**
 * Undoes a journal batch (`null` = the last undoable one) or redoes the last
 * undo, with the toasts of design §3.5 ("N건 되돌림 (불가 N건)"). Resolves to
 * the report when the batch was reverted inline, null otherwise.
 */
export async function runJournal(t: TFunction, { batchId = null, redo = false }: { batchId?: string | null; redo?: boolean } = {}): Promise<UndoReport | null> {
  const ui = useUiStore.getState();
  try {
    const res = redo ? await api.journalRedo() : await api.journalUndo(batchId);
    if (res.report) {
      ui.toast(t(redo ? "palette.redo_done" : "palette.undo_done", { restored: res.report.restored, skipped: res.report.skipped.length }), "ok");
      return res.report;
    }
    if (res.job_id) ui.toast(t(redo ? "palette.redo_started" : "palette.undo_started"), "info");
    return null;
  } catch (e) {
    if (isNothingTo(e, "nothing_to_undo")) ui.toast(t("palette.nothing_to_undo"), "info");
    else if (isNothingTo(e, "nothing_to_redo")) ui.toast(t("palette.nothing_to_redo"), "info");
    else ui.toast(describeError(e, t), "danger");
    return null;
  }
}
