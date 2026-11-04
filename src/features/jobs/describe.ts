import type { TFunction } from "i18next";

import type { Job } from "@/lib/types";

/**
 * The shell reports job subjects / details as compact English strings
 * (`"3 files"`, `"imported 3 / relinked 0 / failed 0 / skipped 2"`). Known
 * shapes are rendered through i18n; anything else (model ids, error text) is
 * shown verbatim.
 */
export function describeJobSubject(job: Job, t: TFunction): string {
  const s = job.subject;
  let m = s.match(/^(\d+) files$/);
  if (m) return t("jobs.subject.files", { count: Number(m[1]) });
  m = s.match(/^(\d+) documents$/);
  if (m) return t("jobs.subject.documents", { count: Number(m[1]) });
  if (s === "inbox") return t("jobs.subject.inbox");
  if (s === "library") return t("jobs.subject.library");
  if (s === "engine") return t("jobs.subject.engine");
  return s;
}

export function describeJobDetail(job: Job, t: TFunction): string | null {
  const d = job.detail;
  if (!d) return null;
  let m = d.match(/^imported (\d+) \/ relinked (\d+) \/ failed (\d+) \/ skipped (\d+)$/);
  if (m) return t("jobs.detail.import_summary", { imported: Number(m[1]), relinked: Number(m[2]), failed: Number(m[3]), skipped: Number(m[4]) });
  m = d.match(/^checked (\d+) \/ missing (\d+) \/ restored (\d+) \/ changed (\d+) \/ failed (\d+)$/);
  if (m) return t("jobs.detail.rescan_summary", { checked: Number(m[1]), missing: Number(m[2]), restored: Number(m[3]), changed: Number(m[4]), failed: Number(m[5]) });
  m = d.match(/^rendered (\d+)$/);
  if (m) return t("jobs.detail.rendered", { count: Number(m[1]) });
  m = d.match(/^imported (\d+) \/ skipped (\d+)$/);
  if (m) return t("jobs.detail.import_summary", { imported: Number(m[1]), relinked: 0, failed: 0, skipped: Number(m[2]) });
  m = d.match(/^checked (\d+)$/);
  if (m) return t("jobs.detail.rescan_summary", { checked: Number(m[1]), missing: 0, restored: 0, changed: 0, failed: 0 });
  m = d.match(/^indexed (\d+) \((\d+) chunks\) \/ unchanged (\d+) \/ no text (\d+) \/ failed (\d+)$/);
  if (m) return t("jobs.detail.index_summary", { indexed: Number(m[1]), chunks: Number(m[2]), unchanged: Number(m[3]), no_text: Number(m[4]), failed: Number(m[5]) });
  m = d.match(/^embedded (\d+) \/ failed (\d+) \/ remaining (\d+)/);
  if (m) return t("jobs.detail.embed_summary", { embedded: Number(m[1]), failed: Number(m[2]), remaining: Number(m[3]) });
  if (d === "loading model") return t("jobs.detail.loading_model");
  return d;
}
