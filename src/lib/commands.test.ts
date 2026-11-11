// `set_retention` helpers: the policy fields each retention item maps to and the
// "shorter period" check that makes the plan card warn first.
import { describe, expect, it } from "vitest";

import type { Job, JobState, RetentionPolicy, RetentionTarget } from "@/lib/types";

import { applyRetention, resolveJob, retentionShortens } from "./commands";

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
