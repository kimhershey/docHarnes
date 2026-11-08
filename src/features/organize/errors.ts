import type { TFunction } from "i18next";

import { describeError } from "@/lib/errors";
import { errorCode, errorMessage } from "@/lib/utils";

/** Stable codes the planner bridge / core raise for organise, journal and export requests. */
const ORGANIZE_CODES = ["organize_nothing", "template_variable", "nothing_to_undo", "nothing_to_redo", "batch_already_undone", "no_documents"] as const;
const EXPORT_CODES = ["export_exists", "path_outside", "no_documents"] as const;

export type OrganizeErrorCode = (typeof ORGANIZE_CODES)[number];
export type ExportErrorCode = (typeof EXPORT_CODES)[number];

/**
 * The known code carried by an IPC error: either as `code` (`typed()` in planner_bridge.rs) or
 * inside the message of an `invalid` error (`UiError::invalid("organize_nothing")`,
 * `"template_variable: bogus"`).
 */
export function knownCode<C extends string>(e: unknown, codes: readonly C[]): C | null {
  const code = errorCode(e);
  const message = errorMessage(e);
  return codes.find((c) => c === code || message.includes(c)) ?? null;
}

/** Variable names after `template_variable:` in the error message. */
export function templateVariablesOf(e: unknown): string[] {
  const m = errorMessage(e).match(/template_variable:\s*([^\n]+)/);
  return m ? m[1]!.split(",").map((s) => s.trim()).filter(Boolean) : [];
}

export function describeOrganizeError(e: unknown, t: TFunction): string {
  const code = knownCode(e, ORGANIZE_CODES);
  if (code === "template_variable") return t("organize.errors.template_variable", { vars: templateVariablesOf(e).join(", ") || "?" });
  if (code) return t(`organize.errors.${code}`);
  return describeError(e, t);
}

export function describeExportError(e: unknown, t: TFunction): string {
  const code = knownCode(e, EXPORT_CODES);
  if (code) return t(`export.errors.${code}`);
  return describeError(e, t);
}
