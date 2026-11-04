import type { TFunction } from "i18next";

import i18n from "@/i18n";

import { errorCode, errorMessage } from "./utils";

/** Human-readable, localised description of an IPC/core error (falls back to the raw message). */
export function describeError(e: unknown, t: TFunction): string {
  const message = errorMessage(e);
  const code = errorCode(e);
  const detail = (e as { detail?: Record<string, unknown> }).detail ?? {};
  // Core messages are "<context>: <reason>"; show only the reason inside the localised template.
  const reason = message.includes(": ") ? message.slice(message.indexOf(": ") + 2) : message;
  // `engine_too_old:<version>:<min>` from the engine bridge.
  const tooOld = reason.match(/^engine_too_old:([^:]*):(\d+)/);
  if (tooOld) return `${t("models.engine_card.not_found", { error: tooOld[1] || "?" })} — ${t("models.engine_card.too_old", { min: tooOld[2] })}`;
  if (!code || !i18n.exists(`errors.${code}`)) return t("errors.generic", { message });
  return t(`errors.${code}`, { message: reason, ...detail });
}
