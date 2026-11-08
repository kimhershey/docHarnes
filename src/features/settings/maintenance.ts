// Pure helpers of the P7 purge (maintenance) job's summary, shared by the
// settings card and the jobs page.

import type { TFunction } from "i18next";

import type { MaintenanceSummary } from "@/lib/types";
import { formatBytes } from "@/lib/utils";

/** "저널 3 · 작업 12 · … · 캐시 1.20 GB · 휴지통 4" from `MaintenanceSummary.purged`. */
export function maintenanceSummaryText(summary: MaintenanceSummary, t: TFunction): string {
  const p = summary.purged;
  return t("settings.privacy.maintenance.summary", {
    journal: p.journal,
    jobs: p.jobs,
    answers: p.answers,
    documents: p.documents,
    extractions: p.extractions,
    logs: p.logs,
    temp: p.temp,
    cache: formatBytes(p.cache_bytes, 1),
    trash: p.trash,
  });
}

/** `maintenance.rs` names the steps by their policy (`chats`); the item labels use the purge counter's name. */
const STEP_ITEM_KEYS: Record<string, string> = { chats: "answers" };

/**
 * Skipped steps of a purge run: the step codes (`journal`, `jobs`, `chats`, …, `disabled`) through
 * `settings.privacy.maintenance.items.*`; an `<step>: <error>` entry keeps its error text.
 */
export function maintenanceSkippedText(summary: MaintenanceSummary, t: TFunction): string | null {
  const skipped = summary.skipped ?? [];
  if (skipped.length === 0) return null;
  const items = skipped.map((s) => {
    const m = s.match(/^([a-z_]+)(?::\s*(.*))?$/s);
    const code = m?.[1] ?? s;
    const label = code === "disabled" ? t("settings.privacy.maintenance.skipped_disabled") : t(`settings.privacy.maintenance.items.${STEP_ITEM_KEYS[code] ?? code}`, { defaultValue: code });
    return m?.[2] ? `${label} (${m[2]})` : label;
  });
  return t("settings.privacy.maintenance.skipped", { items: items.join(", ") });
}
