// `set_retention` helpers: the policy fields each retention item maps to and the
// "shorter period" check that makes the plan card warn first; the job a control command means; and the
// undo of a settings change (what it puts back, and when it must not).
import { beforeEach, describe, expect, it, vi } from "vitest";

// The settings ops run against the browser mock backend and the settings store, which write to
// `localStorage` and the document element: both are stood in for before anything is imported.
const env = vi.hoisted(() => {
  const store = new Map<string, string>();
  (globalThis as unknown as { localStorage: Storage }).localStorage = {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
    key: () => null,
    length: 0,
  } as Storage;
  const attributes = new Map<string, string>();
  (globalThis as unknown as { document: unknown }).document = {
    documentElement: {
      setAttribute: (k: string, v: string) => void attributes.set(k, v),
      removeAttribute: (k: string) => void attributes.delete(k),
      style: { setProperty: () => undefined },
    },
  };
  // The toast store schedules its dismissal on the window; nothing in these tests waits for it.
  (globalThis as unknown as { window: unknown }).window = { setTimeout: () => 0, clearTimeout: () => undefined, addEventListener: () => undefined, removeEventListener: () => undefined };
  return { store, attributes };
});

import i18n, { initI18n } from "@/i18n";
import ko from "@/i18n/ko.json";
import { api } from "@/lib/ipc";
import type { Job, JobState, Op, RetentionPolicy, RetentionTarget } from "@/lib/types";
import { useSettingsStore } from "@/stores/settings";

import { applyRetention, executeUiOp, executeUiOps, resolveJob, retentionShortens, settingOfOp, settingsStillHold, undoSettingsChange } from "./commands";

const policy: RetentionPolicy = {
  documents_days: null,
  extractions_days: 365,
  page_cache_max_gb: 4,
  job_logs_days: 30,
  app_logs_days: 14,
  journal_days: 90,
  chats_days: 30,
  temp_days: 7,
  golden_exports_days: null,
  purge_hour_local: 3,
  purge_enabled: true,
};

describe("applyRetention", () => {
  const table: [RetentionTarget, number, Partial<RetentionPolicy>][] = [
    ["documents", 30, { documents_days: 30 }],
    ["documents", -1, { documents_days: null }],
    ["extractions", 0, { extractions_days: 0 }],
    ["logs", 7, { job_logs_days: 7, app_logs_days: 7 }],
    ["logs", -1, { job_logs_days: null, app_logs_days: null }],
    ["journal", 45.6, { journal_days: 46 }],
    ["chats", 10, { chats_days: 10 }],
    ["temp", 3, { temp_days: 3 }],
  ];
  it.each(table)("%s → %d days sets %o", (item, days, expected) => {
    const next = applyRetention(policy, item, days);
    expect(next).toEqual({ ...policy, ...expected });
    // Everything else, the schedule included, is untouched.
    expect(next.purge_hour_local).toBe(3);
    expect(next.purge_enabled).toBe(true);
  });
  it("does not mutate the input policy", () => {
    const copy = { ...policy };
    applyRetention(policy, "chats", 1);
    expect(policy).toEqual(copy);
  });
});

describe("retentionShortens", () => {
  const table: [RetentionTarget, number, boolean][] = [
    ["chats", 10, true], // 30 → 10
    ["chats", 30, false], // unchanged
    ["chats", 60, false], // longer
    ["chats", -1, false], // forever is never shorter
    ["documents", 30, true], // forever → 30 days
    ["documents", -1, false],
    ["logs", 20, true], // app logs 14 keep, job logs 30 → 20 shortens
    ["logs", 40, false],
    ["temp", 0, true], // immediate
  ];
  it.each(table)("%s → %d days shortens: %s", (item, days, expected) => {
    expect(retentionShortens(policy, item, days)).toBe(expected);
  });
});

// PLN-15: "작업 일시정지" / "작업 취소" without an id took the newest unfinished job — usually one still
// queued behind the job that is actually running.
describe("resolveJob", () => {
  let clock = 0;
  const job = (id: string, state: JobState, persistent = true): Job => ({
    id,
    kind: "plan",
    subject: id,
    state,
    progress: null,
    detail: null,
    bytes_done: null,
    bytes_total: null,
    error: null,
    cancel_requested: false,
    // Later calls are newer jobs.
    created_at: new Date(Date.UTC(2026, 10, 11, 9, 0, clock++)).toISOString(),
    updated_at: "2026-11-11T09:00:00Z",
    finished_at: null,
    priority: "normal",
    persistent,
    attempts: 1,
  });
  const running = job("running-old", "running");
  const paused = job("paused-mid", "paused");
  const queued = job("queued-new", "queued");
  const failed = job("failed-newest", "failed");

  it("pause / cancel pick the running job, not the newest waiting one", () => {
    const jobs = [queued, paused, running, failed];
    expect(resolveJob(jobs, null, "pause")?.id).toBe("running-old");
    expect(resolveJob(jobs, "last", "cancel")?.id).toBe("running-old");
  });

  it("falls back in order: cancel → paused → queued; pause skips what is already paused", () => {
    expect(resolveJob([queued, paused, failed], null, "cancel")?.id).toBe("paused-mid");
    expect(resolveJob([queued, failed], null, "cancel")?.id).toBe("queued-new");
    expect(resolveJob([queued, paused, failed], null, "pause")?.id).toBe("queued-new");
    expect(resolveJob([paused, failed], null, "pause")).toBeNull();
    expect(resolveJob([failed], null, "cancel")).toBeNull();
  });

  it("takes the newest of several jobs in the same state", () => {
    const older = job("running-a", "running");
    const newer = job("running-b", "running");
    expect(resolveJob([older, newer], null, "cancel")?.id).toBe("running-b");
  });

  it("keeps the other actions: resume → paused, retry → newest failed / cancelled / interrupted, prioritize → waiting", () => {
    const jobs = [running, paused, queued, failed, job("cancelled-last", "cancelled")];
    expect(resolveJob(jobs, null, "resume")?.id).toBe("paused-mid");
    expect(resolveJob(jobs, null, "retry")?.id).toBe("cancelled-last");
    expect(resolveJob(jobs, null, "prioritize")?.id).toBe("queued-new");
  });

  it("an explicit id (or its prefix) names the job whatever its state", () => {
    expect(resolveJob([running, queued], "queued-new", "cancel")?.id).toBe("queued-new");
    expect(resolveJob([running, queued], "queued", "pause")?.id).toBe("queued-new");
    expect(resolveJob([running, queued], "nope", "cancel")).toBeNull();
  });

  it("prefers persistent jobs when there are any (the others cannot be paused / retried)", () => {
    const download = job("download", "running", false);
    expect(resolveJob([download, queued], null, "cancel")?.id).toBe("queued-new");
    expect(resolveJob([download], null, "cancel")?.id).toBe("download");
  });
});

// 0.8.0 review: 되돌리기 on an executed card replayed the value from before the change unconditionally —
// "배율 120", "배율 150", then 되돌리기 on the first card set 100 and said "이전 값으로 되돌렸습니다".
describe("undo of a settings change", () => {
  initI18n("ko");
  const t = i18n.t.bind(i18n);
  const navigate = vi.fn();
  const settings = () => useSettingsStore.getState().settings!;
  const run = (op: Op) => executeUiOp(op, navigate, t);
  const undo = (change: { undo?: Op[]; applied?: Op[] }) => undoSettingsChange(change.undo ?? [], change.applied, navigate, t);
  const scale = (value: number): Op => ({ op: "set_setting", key: "ui.scale_percent", value });

  beforeEach(async () => {
    env.store.clear();
    // The inbox watch is a command of the open workspace.
    await api.workspaceCreate("C:\\Work\\Docs", null, null);
    await useSettingsStore.getState().load();
    expect(settings().ui.scale_percent).toBe(100);
  });

  it("names the setting an op writes", () => {
    expect(settingOfOp(scale(120))).toEqual({ path: "ui.scale_percent", value: 120 });
    // Legacy keys resolve to their catalog path.
    expect(settingOfOp({ op: "set_setting", key: "render_dpi", value: 300 })).toEqual({ path: "processing.render_dpi", value: 300 });
    expect(settingOfOp({ op: "set_theme", theme: "dark" })).toEqual({ path: "ui.theme", value: "dark" });
    expect(settingOfOp({ op: "set_language", lang: "en" })).toEqual({ path: "ui.language", value: "en" });
    expect(settingOfOp({ op: "set_profile", profile: "accuracy" })).toEqual({ path: "models.profile", value: "accuracy" });
    expect(settingOfOp({ op: "set_inbox_watch", enabled: false })).toEqual({ path: "processing.inbox_watch", value: false });
    // A retention period is not a settings path; other ops change no setting.
    expect(settingOfOp({ op: "set_setting", key: "journal_days", value: 30 })).toBeNull();
    expect(settingOfOp({ op: "help" })).toBeNull();
    expect(settingsStillHold([scale(100)], settings())).toBe(true);
    expect(settingsStillHold([scale(120)], settings())).toBe(false);
    expect(settingsStillHold([scale(100)], null)).toBe(false);
  });

  it("(a) a change that was changed again is not undone: the value stays and the undo says `stale`", async () => {
    const first = await run(scale(120));
    expect(first.undo).toEqual([scale(100)]);
    expect(first.applied).toEqual([scale(120)]);
    const second = await run(scale(150));
    expect(second).toMatchObject({ undo: [scale(120)], applied: [scale(150)] });

    // 되돌리기 on the first card: 120 is not what the setting holds any more — 100 must not come back.
    expect(await undo(first)).toBe("stale");
    expect(settings().ui.scale_percent).toBe(150);
    expect((await api.settingsGet()).ui.scale_percent).toBe(150);
    // The last change can be undone; after that the first one is current again.
    expect(await undo(second)).toBe("undone");
    expect(settings().ui.scale_percent).toBe(120);
    expect(await undo(first)).toBe("undone");
    expect(settings().ui.scale_percent).toBe(100);
    // Undone twice: the second time there is nothing of that change left to undo.
    expect(await undo(first)).toBe("stale");
    expect(settings().ui.scale_percent).toBe(100);
  });

  it("(a) the current value is read from the backend first: a change made elsewhere (the settings screen, before a restart) makes the undo stale", async () => {
    const change = await run(scale(120));
    // Behind the store's back, as another window or an earlier session would.
    await api.settingsSetUiScale(130);
    expect(settings().ui.scale_percent).toBe(120);
    expect(await undo(change)).toBe("stale");
    expect(settings().ui.scale_percent).toBe(130);
  });

  it("(a) theme, language and profile undo check the same", async () => {
    const theme = await run({ op: "set_theme", theme: "dark" });
    expect(theme).toMatchObject({ undo: [{ op: "set_theme", theme: "system" }], applied: [{ op: "set_theme", theme: "dark" }] });
    await run({ op: "set_theme", theme: "light" });
    expect(await undo(theme)).toBe("stale");
    expect(settings().ui.theme).toBe("light");

    const profileBefore = settings().models.profile;
    const profile = await run({ op: "set_profile", profile: "fine_print" });
    expect(profile).toMatchObject({ undo: [{ op: "set_profile", profile: profileBefore }], applied: [{ op: "set_profile", profile: "fine_print" }] });
    expect(await undo(profile)).toBe("undone");
    expect(settings().models.profile).toBe(profileBefore);
    expect(await undo(profile)).toBe("stale");

    const language = await run({ op: "set_language", lang: "en" });
    expect(language).toMatchObject({ undo: [{ op: "set_language", lang: "ko" }], applied: [{ op: "set_language", lang: "en" }] });
    await run({ op: "set_language", lang: "ko" });
    expect(await undo(language)).toBe("stale");
    expect(settings().ui.language).toBe("ko");
  });

  it("a card stored without `applied` (an earlier build) is undone as before; no undo ops is a failure", async () => {
    await run(scale(120));
    expect(await undoSettingsChange([scale(100)], undefined, navigate, t)).toBe("undone");
    expect(settings().ui.scale_percent).toBe(100);
    expect(await undoSettingsChange([], [scale(100)], navigate, t)).toBe("failed");
  });

  it("several changes of one sentence are undone together or not at all; the results carry what was applied", async () => {
    const results = await executeUiOps([scale(120), { op: "set_theme", theme: "dark" }], navigate, t);
    expect(results.map((r) => r.applied)).toEqual([[scale(120)], [{ op: "set_theme", theme: "dark" }]]);
    const undoOps = results.flatMap((r) => r.undo ?? []).reverse();
    const applied = results.flatMap((r) => r.applied ?? []);
    await run({ op: "set_theme", theme: "light" });
    // One of the two was changed again: neither is put back.
    expect(await undoSettingsChange(undoOps, applied, navigate, t)).toBe("stale");
    expect(settings().ui).toMatchObject({ scale_percent: 120, theme: "light" });
  });

  it("(b) '티어 자동' clears the manual tier — its undo brings back the tier that was chosen, not a default", async () => {
    await useSettingsStore.getState().setTierMode("manual", "vram_12gb");
    const change = await run({ op: "set_setting", key: "models.tier_mode", value: "auto" });
    expect(settings().models).toMatchObject({ tier_mode: "auto", manual_tier: null });
    expect(change.undo).toEqual([{ op: "set_setting", key: "models.manual_tier", value: "vram_12gb" }]);
    expect(change.applied).toEqual([{ op: "set_setting", key: "models.tier_mode", value: "auto" }]);
    expect(await undo(change)).toBe("undone");
    expect(settings().models).toMatchObject({ tier_mode: "manual", manual_tier: "vram_12gb" });
    // The other direction puts "auto" back.
    await useSettingsStore.getState().setTierMode("auto", null);
    const manual = await run({ op: "set_setting", key: "models.tier_mode", value: "manual" });
    expect(manual.undo).toEqual([{ op: "set_setting", key: "models.tier_mode", value: "auto" }]);
    expect(await undo(manual)).toBe("undone");
    expect(settings().models.tier_mode).toBe("auto");
  });

  it("(c) `set_inbox_watch` reads like the setting it is: before → after, 'already', and an undo", async () => {
    expect(settings().processing.inbox_watch).toBe(true);
    const label = ko.planner.settings.processing_inbox_watch;
    const off = await run({ op: "set_inbox_watch", enabled: false });
    expect(settings().processing.inbox_watch).toBe(false);
    expect((await api.settingsGet()).processing.inbox_watch).toBe(false);
    expect(off.summary).toBe(ko.planner.ui.set_setting_changed.replace("{{key}}", label).replace("{{from}}", ko.planner.params.on).replace("{{to}}", ko.planner.params.off));
    expect(off.undo).toEqual([{ op: "set_setting", key: "processing.inbox_watch", value: true }]);
    expect(off.applied).toEqual([{ op: "set_setting", key: "processing.inbox_watch", value: false }]);
    // Already off: the card says so and offers nothing to undo.
    const again = await run({ op: "set_inbox_watch", enabled: false });
    expect(again.summary).toBe(ko.planner.ui.set_setting_same.replace("{{key}}", label).replace("{{value}}", ko.planner.params.off));
    expect(again.undo).toBeUndefined();
    expect(await undo(off)).toBe("undone");
    expect(settings().processing.inbox_watch).toBe(true);
    // The same op and the catalog setting are one and the same switch.
    const viaSetting = await run({ op: "set_setting", key: "processing.inbox_watch", value: false });
    expect(viaSetting.summary).toBe(off.summary);
  });
});
