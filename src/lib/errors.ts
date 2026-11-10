import type { TFunction } from "i18next";

import i18n from "@/i18n";

import { errorCode, errorMessage } from "./utils";

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
  // Well-known `invalid` reasons from the document commands get their own copy.
  if (code === "invalid" || code === "not_found") {
    const known: Record<string, string> = {
      "nothing to import": "errors.reasons.nothing_to_import",
      "no files": "errors.reasons.no_files",
      "no documents": "errors.reasons.no_documents",
      "rotation must be a multiple of 90": "errors.reasons.rotation",
      collection_name_taken: "errors.reasons.collection_name_taken",
      collection_name_empty: "errors.reasons.collection_name_empty",
      glossary_term_empty: "errors.reasons.glossary_term_empty",
      glossary_term_taken: "errors.reasons.glossary_term_taken",
      collection_not_found: "errors.reasons.collection_not_found",
      document_not_found: "errors.reasons.document_not_found",
      feedback_invalid: "errors.reasons.feedback_invalid",
      schema_name_duplicate: "errors.reasons.schema_name_duplicate",
      version_group_empty: "errors.reasons.version_group_empty",
      no_active_model: "errors.reasons.no_active_model",
      embedding_model_not_installed: "errors.reasons.embedding_model_not_installed",
      no_text: "errors.reasons.no_text",
      "document_id required": "errors.reasons.document_id_required",
      schema_name_empty: "errors.reasons.schema_name_empty",
      schema_doc_type_invalid: "errors.reasons.schema_doc_type_invalid",
      schema_invalid: "errors.reasons.schema_invalid",
      schema_not_found: "errors.reasons.schema_not_found",
      schema_builtin: "errors.reasons.schema_builtin",
      field_not_found: "errors.reasons.field_not_found",
      field_value_empty: "errors.reasons.field_value_empty",
      change_not_found: "errors.reasons.change_not_found",
      description_empty: "errors.reasons.description_empty",
      no_model: "errors.reasons.no_model",
      engine_not_running: "errors.reasons.engine_not_running",
      model_not_loaded: "errors.reasons.model_not_loaded",
      "no fields": "errors.reasons.no_fields",
      "no split boundary inside the document": "errors.reasons.no_split_boundary",
      "merge needs at least two documents": "errors.reasons.merge_two",
      table_not_found: "errors.reasons.table_not_found",
      plan_clarify: "errors.reasons.plan_clarify",
    };
    const key = known[reason.trim()];
    if (key) return t(key);
  }
  if (!code || !i18n.exists(`errors.${code}`)) return t("errors.generic", { message });
  return t(`errors.${code}`, { message: reason, ...detail });
}
