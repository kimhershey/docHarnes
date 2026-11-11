import type { TFunction } from "i18next";

import { OP_KINDS } from "@/features/assistant/opSpecs";
import type { Job, JobKind, JobPriority, OpKind } from "@/lib/types";
import { formatBytes } from "@/lib/utils";

/**
 * The jobs whose `bytes_done` / `bytes_total` really are bytes (model download, model import, hash check).
 * Every other job puts an item count into the same pair — documents (processing, indexing, field discovery),
 * chunks (embedding), plan steps, files (organise).
 */
const BYTE_JOB_KINDS: readonly JobKind[] = ["model_download", "model_import", "model_hash"];

/** The "done / total" counter of a job row: bytes for transfers, plain counts for everything else (CON-11). Null without a total. */
export function describeJobCounter(job: Pick<Job, "kind" | "bytes_done" | "bytes_total">, locale: string): string | null {
  if (job.bytes_total == null) return null;
  const done = job.bytes_done ?? 0;
  if (BYTE_JOB_KINDS.includes(job.kind)) return `${formatBytes(done)} / ${formatBytes(job.bytes_total)}`;
  const n = new Intl.NumberFormat(locale);
  return `${n.format(done)} / ${n.format(job.bytes_total)}`;
}

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
  // `exec.rs` reports the model call of a `draft_document` step as the "draft" stage.
  if (stage === "draft") return t("jobs.detail.drafting");
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
  // -- P6 draft job (`generate_bridge::run_draft`): "loading model" above, then "drafting"; the finishing
  // detail ("<title> · 필드 n · 검토 항목 m") and the P7 maintenance summary are already localised.
  if (d === "drafting" || d === "finishing") return t("jobs.detail.drafting");
  // Split draft (0.3.0): "outline", then "section i/n".
  if (d === "outline") return t("jobs.detail.draft_outline");
  // 0.7.0 초안 어시스턴트: one section / new section / whole-draft revision.
  if (d === "revising") return t("jobs.detail.draft_revising");
  m = d.match(/^section (\d+)\/(\d+)$/);
  if (m) return t("jobs.detail.draft_section", { index: Number(m[1]), total: Number(m[2]) });
  // -- 0.3.0 §2 schema discovery (`pipeline_bridge::run_schema_discovery`): per-document / per-page progress, then the merge.
  m = d.match(/^(.+) · discover(?: \((\d+)\/(\d+)\))?$/);
  if (m) return t("jobs.detail.discovering", { name: m[1], page: m[2] ? ` (${m[2]}/${m[3]})` : "" });
  m = d.match(/^merged (\d+) fields from (\d+) documents$/);
  if (m) return t("jobs.detail.discovery_summary", { fields: Number(m[1]), docs: Number(m[2]) });
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

/** Stable error codes the shell / workers put into `Job.error`, with their `jobs.error.*` keys. */
const ERROR_CODES: Record<string, string> = {
  engine_not_running: "jobs.error.engine_not_running",
  model_not_loaded: "jobs.error.model_not_loaded",
  "workspace closed": "jobs.error.workspace_closed",
  workspace_closed: "jobs.error.workspace_closed",
  interrupted: "jobs.error.interrupted",
  cancelled: "jobs.error.cancelled",
  no_model: "jobs.error.no_model",
  export_exists: "jobs.error.export_exists",
  path_outside: "jobs.error.path_outside",
  no_documents: "jobs.error.no_documents",
  content_empty: "jobs.error.content_empty",
  project_not_found: "jobs.error.project_not_found",
  no_check_run: "jobs.error.no_check_run",
  no_findings: "jobs.error.no_findings",
  document_not_found: "jobs.error.document_not_found",
  no_fields: "jobs.error.no_fields",
};

function describeErrorText(raw: string, t: TFunction): string {
  const text = raw.trim();
  if (!text) return text;
  const key = ERROR_CODES[text];
  if (key) return t(key);
  let m = text.match(/^schema_not_found(?::\s*(.*))?$/);
  if (m) return t("jobs.error.schema_not_found", { name: m[1]?.trim() || "?" });
  // Plan workers report "<op>: <reason>" (`planner_bridge.rs`): translate the op and recurse on the reason.
  m = text.match(/^([a-z_]+):\s*(.*)$/s);
  if (m && (OP_KINDS as readonly string[]).includes(m[1]!)) return t("jobs.error.step", { op: t(`planner.ops.${m[1] as OpKind}`), message: describeErrorText(m[2]!, t) || t("jobs.state.failed") });
  // A known code somewhere inside a longer message ("export: export_exists", "…: engine_not_running").
  for (const [code, k] of Object.entries(ERROR_CODES)) if (code.length > 6 && text.includes(code)) return t(k);
  return text;
}

/**
 * Localised `Job.error` (review L1): the stable codes of the shell / workers
 * (`engine_not_running`, `workspace closed`, `schema_not_found: …`,
 * `export_exists` …) through `jobs.error.*`; anything else verbatim. Null
 * when the job has no error.
 */
export function describeJobError(job: Pick<Job, "error">, t: TFunction): string | null {
  if (!job.error) return null;
  return describeErrorText(job.error, t);
}
