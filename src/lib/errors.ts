import type { TFunction } from "i18next";

import i18n from "@/i18n";

import { errorCode, errorMessage } from "./utils";

/** Reasons the document / pipeline commands word as a phrase instead of a code → their `errors.reasons.*` name. */
const REASON_PHRASES: Record<string, string> = {
  "nothing to import": "nothing_to_import",
  "no files": "no_files",
  "no documents": "no_documents",
  "rotation must be a multiple of 90": "rotation",
  "document_id required": "document_id_required",
  "no fields": "no_fields",
  "no split boundary inside the document": "no_split_boundary",
  "merge needs at least two documents": "merge_two",
};

const CODE_SHAPE = /^[a-z][a-z0-9_]*$/;

/**
 * The `errors.reasons.*` key of an error, wherever its stable code travels (PIP-3): as the error's own `code`
 * (the typed command errors: `{ code: "schema_name_empty" }`), as the reason of an `invalid` / `not_found`
 * error (`"invalid value: no_findings"`), behind a detail (`"schema_not_found: <id>"`) or wrapped by another
 * layer (`"database error: Invalid parameter name: schema_name_duplicate"`). A `not_found` error that names
 * what is missing (`"not found: project <id>"`) maps to `<noun>_not_found`. Null when no part of the error
 * names a reason with its own copy.
 */
export function errorReasonKey(e: unknown): string | null {
  const code = errorCode(e) ?? "";
  const parts = errorMessage(e).split(": ");
  for (const raw of [code, ...parts]) {
    const c = raw.trim();
    const name = REASON_PHRASES[c] ?? (CODE_SHAPE.test(c) ? c : null);
    if (name && i18n.exists(`errors.reasons.${name}`)) return `errors.reasons.${name}`;
  }
  if (code === "not_found") {
    const noun = (parts[parts.length - 1] ?? "").trim().match(/^([a-z_]+) \S+$/)?.[1];
    if (noun && i18n.exists(`errors.reasons.${noun}_not_found`)) return `errors.reasons.${noun}_not_found`;
  }
  return null;
}

/** Human-readable, localised description of an IPC/core error (falls back to the raw message). */
export function describeError(e: unknown, t: TFunction): string {
  const message = errorMessage(e);
  const code = errorCode(e);
  const rawDetail = (e as { detail?: unknown }).detail;
  const detail: Record<string, unknown> = typeof rawDetail === "object" && rawDetail !== null && !Array.isArray(rawDetail) ? (rawDetail as Record<string, unknown>) : {};
  // Core messages are "<context>: <reason>"; show only the reason inside the localised template.
  const reason = message.includes(": ") ? message.slice(message.indexOf(": ") + 2) : message;
  // `engine_too_old:<version>:<min>` from the engine bridge.
  const tooOld = reason.match(/^engine_too_old:([^:]*):(\d+)/);
  if (tooOld) return `${t("models.engine_card.not_found", { error: tooOld[1] || "?" })} — ${t("models.engine_card.too_old", { min: tooOld[2] })}`;
  // A stable reason code gets its own copy, whichever error class carries it.
  const key = errorReasonKey(e);
  if (key) return t(key);
  if (!code || !i18n.exists(`errors.${code}`)) return t("errors.generic", { message });
  return t(`errors.${code}`, { message: reason, ...detail });
}
