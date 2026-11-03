import type { TFunction } from "i18next";

import { errorCode, errorMessage } from "./utils";

const KNOWN_CODES = new Set(["io", "json", "workspace_locked", "invalid_workspace", "no_workspace", "unknown_model", "invalid", "internal"]);

/** Human-readable, localised description of an IPC/core error (falls back to the raw message). */
export function describeError(e: unknown, t: TFunction): string {
  const message = errorMessage(e);
  const code = errorCode(e);
  if (!code || !KNOWN_CODES.has(code)) return t("errors.generic", { message });
  const detail = (e as { detail?: Record<string, unknown> }).detail ?? {};
  // Core messages are "<context>: <reason>"; show only the reason inside the localised template.
  const reason = message.includes(": ") ? message.slice(message.indexOf(": ") + 2) : message;
  return t(`errors.${code}`, { message: reason, ...detail });
}
