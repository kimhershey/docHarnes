import type { DocumentStatus } from "@/lib/types";

/** Badge colour for a document status (shared by the library and the viewer). */
export function statusVariant(s: DocumentStatus): "ok" | "warn" | "danger" | "muted" | "info" {
  switch (s) {
    case "rendered":
    case "extracted":
    case "reviewed":
      return "ok";
    case "importing":
      return "info";
    case "error":
    case "missing":
      return "danger";
    case "new":
      return "warn";
    default:
      return "muted";
  }
}
