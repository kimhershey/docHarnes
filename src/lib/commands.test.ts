// `set_retention` helpers: the policy fields each retention item maps to and the
// "shorter period" check that makes the plan card warn first.
import { describe, expect, it } from "vitest";

import type { RetentionPolicy, RetentionTarget } from "@/lib/types";

import { applyRetention, retentionShortens } from "./commands";

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
