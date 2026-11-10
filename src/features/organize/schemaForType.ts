import type { SchemaRecord } from "@/lib/types";

/** The schema a document type exports with: its default (enabled) schema, else the first enabled one. */
export function schemaForType(schemas: SchemaRecord[] | undefined, docType: string): SchemaRecord | undefined {
  const candidates = (schemas ?? []).filter((s) => s.doc_type === docType && s.enabled);
  return candidates.find((s) => s.is_default) ?? candidates[0];
}
