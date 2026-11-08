import type { TFunction } from "i18next";

import type { Job, JobPriority } from "@/lib/types";

/**
 * The shell reports job subjects / details as compact English strings
 * (`"3 files"`, `"imported 3 / relinked 0 / failed 0 / skipped 2"`). Known
 * shapes are rendered through i18n; anything else (model ids, plan
 * summaries, export paths, error text) is shown verbatim.
 */
export function describeJobSubject(job: Job, t: TFunction): string {
  const s = job.subject;
  let m = s.match(/^(\d+) files$/);
  if (m) return t("jobs.subject.files", { count: Number(m[1]) });
  m = s.match(/^(\d+) documents$/);
  if (m) return t("jobs.subject.documents", { count: Number(m[1]) });
  m = s.match(/^(\d+) entries$/);
  if (m) return t("jobs.subject.entries", { count: Number(m[1]) });
  if (s === "inbox") return t("jobs.subject.inbox");
  if (s === "library") return t("jobs.subject.library");
  if (s === "engine") return t("jobs.subject.engine");
  return s;
}

export function describeJobPriority(priority: JobPriority | undefined, t: TFunction): string {
  return t(`jobs.priority.${priority ?? "normal"}`);
}

/** Stage word of a plan / pipeline progress line: pipeline levels, planner ops, else verbatim. */
export function describeStage(stage: string, t: TFunction): string {
  if (stage === "classify" || stage === "ocr" || stage === "extract") return t(`pipeline.levels.${stage}`);
  return t(`planner.ops.${stage}`, { defaultValue: stage });
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
  m = d.match(/^embedded (\d+) \/ failed (\d+) \/ remaining (\d+)( \(rebuilt for a new model\))?$/);
  if (m) {
    const summary = t("jobs.detail.embed_summary", { embedded: Number(m[1]), failed: Number(m[2]), remaining: Number(m[3]) });
    return m[4] ? t("jobs.detail.embed_rebuilt", { summary }) : summary;
  }
  m = d.match(/^processed (\d+) \/ classified (\d+) \/ ocr pages (\d+) \/ extracted (\d+) \/ review (\d+) \/ failed (\d+)$/);
  if (m) return t("jobs.detail.process_summary", { processed: Number(m[1]), classified: Number(m[2]), ocr: Number(m[3]), extracted: Number(m[4]), review: Number(m[5]), failed: Number(m[6]) });
  m = d.match(/^split into (\d+)$/);
  if (m) return t("jobs.detail.split_summary", { count: Number(m[1]) });
  m = d.match(/^merged (\d+)$/);
  if (m) return t("jobs.detail.merge_summary", { count: Number(m[1]) });
  if (d === "loading model") return t("jobs.detail.loading_model");
  // -- persistent runner (P5, `planner_bridge.rs`) --
  m = d.match(/^plan (\d+)\/(\d+) · (.*)$/);
  if (m) return t("jobs.detail.plan_summary", { done: Number(m[1]), total: Number(m[2]), summary: m[3] });
  m = d.match(/^(\d+)\/(\d+) · ([a-z_]+) · (.+) \((\d+)\/(\d+)\)$/);
  if (m) return t("jobs.detail.step_progress_doc", { step: Number(m[1]), steps: Number(m[2]), stage: describeStage(m[3]!, t), name: m[4], done: Number(m[5]), total: Number(m[6]) });
  m = d.match(/^(\d+)\/(\d+) · ([a-z_]+)$/);
  if (m) return t("jobs.detail.step_progress", { step: Number(m[1]), steps: Number(m[2]), stage: describeStage(m[3]!, t) });
  m = d.match(/^exported (\d+) documents → (.+)$/);
  if (m) return t("jobs.detail.exported", { count: Number(m[1]), path: m[2] });
  m = d.match(/^moved (\d+) \/ failed (\d+) \/ skipped (\d+)$/);
  if (m) return t("jobs.detail.organize_summary", { moved: Number(m[1]), failed: Number(m[2]), skipped: Number(m[3]) });
  m = d.match(/^restored (\d+) \/ skipped (\d+)$/);
  if (m) return t("jobs.detail.undo_summary", { restored: Number(m[1]), skipped: Number(m[2]) });
  if (d === "nothing to undo") return t("jobs.detail.nothing_to_undo");
  if (d === "nothing to redo") return t("jobs.detail.nothing_to_redo");
  if (d === "interrupted") return t("jobs.detail.interrupted");
  if (d === "workspace closed") return t("jobs.detail.workspace_closed");
  if (d === "cancelled") return t("jobs.state.cancelled");
  // Per-document stage progress from `pipeline_bridge.rs`: "<doc> · classify (2/5)", "<doc> · ocr" or a bare
  // stage; the stage words are the level names, so `pipeline.levels.*` translates them.
  m = d.match(/^(?:(.+) · )?(classify|ocr|extract)(?: \((\d+)\/(\d+)\))?$/);
  if (m) {
    const stage = `${t(`pipeline.levels.${m[2]}`)}${m[3] ? ` (${m[3]}/${m[4]})` : ""}`;
    return m[1] ? `${m[1]} · ${stage}` : stage;
  }
  if (d === "done") return t("jobs.state.done");
  return d;
}
