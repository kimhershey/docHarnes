// Executes pre-routed operations (theme / language / open / help) — shared by
// the command palette, the home command bar and the assistant composer — and,
// since P5, every planner op the UI performs itself (`OpKind::runs_in_ui`:
// settings, navigation, job / engine control, reindex, inbox watch, help).

import type { TFunction } from "i18next";
import type { NavigateFunction } from "react-router";

import { describeError } from "./errors";
import { api } from "./ipc";
import { SCREEN_ROUTES, isJobFinished, type AutoProcessingLevel, type DocumentView, type Op, type PreRouteResult, type RetentionPolicy, type RetentionTarget, type Screen, type UndoReport } from "./types";
import { errorCode, errorMessage } from "./utils";
import { useDocumentsStore } from "@/stores/documents";
import { useEngineStore } from "@/stores/engine";
import { useJobsStore } from "@/stores/jobs";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

export interface ExecuteOutcome {
  /** Human-readable summary of what happened (already translated). */
  summary: string;
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
      await settings.setTheme(result.theme);
      const summary = t("theme.changed", { theme: t(`theme.${result.theme}`) });
      ui.toast(summary, "ok");
      return { summary };
    }
    case "set_language": {
      await settings.setLanguage(result.language);
      const summary = t("language.changed", { language: t(`language.${result.language}`) });
      ui.toast(summary, "ok");
      return { summary };
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

function toBool(v: unknown): boolean {
  if (typeof v === "boolean") return v;
  if (typeof v === "number") return v !== 0;
  const s = String(v).trim().toLowerCase();
  if (["true", "1", "on", "yes", "y", "켜", "켬", "켜기", "사용", "enable", "enabled"].includes(s)) return true;
  if (["false", "0", "off", "no", "n", "꺼", "끔", "끄기", "사용안함", "disable", "disabled"].includes(s)) return false;
  throw new Error(`invalid boolean: ${String(v)}`);
}

function toNumber(v: unknown): number {
  const n = typeof v === "number" ? v : Number(String(v).replace(/[^0-9.-]/g, ""));
  if (!Number.isFinite(n)) throw new Error(`invalid number: ${String(v)}`);
  return n;
}

function toAutoLevel(v: unknown): AutoProcessingLevel {
  const s = String(v).trim().toLowerCase();
  const map: Record<string, AutoProcessingLevel> = {
    none: "none",
    off: "none",
    classify: "classify_only",
    classify_only: "classify_only",
    ocr: "classify_and_ocr",
    classify_and_ocr: "classify_and_ocr",
    full: "full",
    extract: "full",
  };
  const level = map[s];
  if (!level) throw new Error(`invalid auto level: ${String(v)}`);
  return level;
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
      const value = op.value;
      let shown: string | number | boolean = value;
      switch (op.key) {
        case "render_dpi": {
          const n = Math.round(toNumber(value));
          shown = n;
          await settings.update((d) => void (d.processing.render_dpi = n));
          break;
        }
        case "pii_masking": {
          const b = toBool(value);
          shown = b;
          await settings.update((d) => void (d.privacy.pii_masking = b));
          break;
        }
        case "auto_pipeline": {
          const level = toAutoLevel(value);
          shown = t(`settings.processing.auto_levels.${level}`);
          await settings.update((d) => void (d.processing.auto_level = level));
          break;
        }
        case "inbox_watch": {
          const b = toBool(value);
          shown = b;
          await api.documentsSetInboxWatch(b);
          await settings.update((d) => void (d.processing.inbox_watch = b));
          break;
        }
        case "review_threshold": {
          let n = toNumber(value);
          if (n > 1) n /= 100;
          n = Math.min(1, Math.max(0, n));
          shown = n;
          await settings.update((d) => void (d.processing.review_threshold = n));
          break;
        }
        case "cache_limit_gb": {
          const n = toNumber(value);
          shown = n;
          await settings.update((d) => void (d.cache.page_cache_max_gb = n));
          break;
        }
        case "journal_days": {
          const ws = useWorkspaceStore.getState();
          if (!ws.current) throw new Error("no_workspace");
          const n = Math.max(0, Math.round(toNumber(value)));
          shown = n;
          await ws.setRetention({ ...ws.current.manifest.retention, journal_days: n === 0 ? null : n });
          break;
        }
        default:
          throw new Error(`unknown setting: ${String(op.key)}`);
      }
      return done(t("planner.ui.set_setting", { key: t(SETTING_LABELS[op.key] ?? "common.unknown"), value: String(shown) }));
    }
    case "set_model": {
      await settings.setActiveModel(op.id);
      await useEngineStore.getState().loadModel(op.id);
      return done(t("planner.ui.set_model", { id: op.id }), "info");
    }
    case "set_profile": {
      await settings.update((d) => void (d.models.profile = op.profile));
      return done(t("planner.ui.set_profile", { profile: t(`models.profiles.${op.profile}`) }));
    }
    case "open": {
      switch (op.target) {
        case "screen": {
          const screen = resolveScreen(op.ref, t);
          if (!screen) throw new Error(t("planner.ui.unknown_screen", { ref: op.ref }));
          navigate(SCREEN_ROUTES[screen]);
          return { summary: t("palette.open_screen", { screen: t(`nav.${screen}`) }) };
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
    case "set_inbox_watch": {
      await api.documentsSetInboxWatch(op.enabled);
      await settings.update((d) => void (d.processing.inbox_watch = op.enabled));
      return done(t(op.enabled ? "planner.ui.inbox_watch_on" : "planner.ui.inbox_watch_off"));
    }
    case "register_form": {
      // §3.4.0 form registration: the schema screen's dialog derives the fields and confirms them in the grid.
      const ref = (op.doc_ref ?? "").trim();
      const doc = ref && ref !== "current" ? await resolveDocument(ref) : ui.lastDocument ? await resolveDocument(ui.lastDocument.id) : (useDocumentsStore.getState().selectedIds[0] ? await resolveDocument(useDocumentsStore.getState().selectedIds[0]!) : null);
      if (!doc) throw new Error(t("planner.ui.document_not_found", { ref: ref || t("planner.params.current_document") }));
      const params = new URLSearchParams({ register: doc.id });
      if (op.name) params.set("name", op.name);
      navigate(`${SCREEN_ROUTES.schemas}?${params.toString()}`);
      return done(t("planner.ui.register_form", { name: doc.original_name }), "info");
    }
    case "set_retention": {
      const ws = useWorkspaceStore.getState();
      if (!ws.current) throw new Error("no_workspace");
      const next = applyRetention(ws.current.manifest.retention, op.item, op.days);
      await ws.setRetention(next);
      const period = op.days < 0 ? t("common.forever") : op.days === 0 ? t("settings.privacy.immediate") : t("planner.params.days", { count: op.days });
      return done(t("planner.ui.set_retention", { item: t(`planner.params.retention_${op.item}`), period }));
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

/** `job(id: "last")`: the most recent persistent job the action applies to. */
function resolveJob(jobs: ReturnType<typeof useJobsStore.getState>["jobs"], id: string | null | undefined, action: string) {
  if (id && id !== "last") return jobs.find((j) => j.id === id || j.id.startsWith(id)) ?? null;
  const persistent = jobs.filter((j) => j.persistent);
  const pool = persistent.length > 0 ? persistent : jobs;
  const candidates =
    action === "retry"
      ? pool.filter((j) => j.state === "failed" || j.state === "cancelled" || j.state === "interrupted")
      : action === "resume"
        ? pool.filter((j) => j.state === "paused")
        : action === "prioritize"
          ? pool.filter((j) => j.state === "queued" || j.state === "paused")
          : pool.filter((j) => !isJobFinished(j));
  return [...candidates].sort((a, b) => b.created_at.localeCompare(a.created_at))[0] ?? null;
}

/** Runs several UI ops in order, toasting failures; returns the summaries of the ones that ran. */
export async function executeUiOps(ops: Op[], navigate: NavigateFunction, t: TFunction): Promise<string[]> {
  const summaries: string[] = [];
  for (const op of ops) {
    try {
      const outcome = await executeUiOp(op, navigate, t);
      if (outcome.summary) summaries.push(outcome.summary);
    } catch (e) {
      useUiStore.getState().toast(describeError(e, t), "danger");
    }
  }
  return summaries;
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
