import type { CheckRun, RuleResult, Verdict } from "@/lib/types";

/** Finding id of a missing document type in `finding_ids` — `report::missing_doc_finding_id`. */
export function missingDocFindingId(docType: string): string {
  return `missing_doc:${docType}`;
}

/** One row of the 보완요청서 dialog: a required document the project lacks, or a rule result that did not pass. */
export interface SupplementRow {
  /** The `finding_ids` entry. */
  id: string;
  verdict: Verdict;
  /** The missing document type (a missing-document row) … */
  docType: string | null;
  /** … or the rule result. */
  result: RuleResult | null;
  /** The request builder turns only 불일치 / 누락 into findings; a 판단불가 result cannot be requested. */
  selectable: boolean;
  /** A 누락 rule whose document type is itself one of the missing documents: that row already asks for it. */
  covered: boolean;
}

/**
 * The rows of a check run in the order the request lists them (`report::supplement_request`): the missing
 * required documents first, then every rule result that did not pass.
 */
export function supplementRows(run: CheckRun | null | undefined): SupplementRow[] {
  if (!run) return [];
  const missingDocs: SupplementRow[] = run.missing_doc_types.map((docType) => ({ id: missingDocFindingId(docType), verdict: "missing", docType, result: null, selectable: true, covered: false }));
  const results: SupplementRow[] = run.results
    .filter((r) => r.verdict !== "pass")
    .map((r) => ({
      id: r.id,
      verdict: r.verdict,
      docType: null,
      result: r,
      selectable: r.verdict === "mismatch" || r.verdict === "missing",
      covered: r.verdict === "missing" && run.missing_doc_types.includes(r.left?.doc_type ?? ""),
    }));
  return [...missingDocs, ...results];
}

/**
 * The rows ticked when the dialog opens — what the request builder takes for "everything": the missing documents
 * and every 불일치 / 누락 result, except a 누락 that its missing-document row already covers.
 */
export function defaultSupplementSelection(rows: SupplementRow[]): string[] {
  return rows.filter((r) => r.selectable && !r.covered).map((r) => r.id);
}

/** Every row that can go into the request ("모두 선택"). */
export function selectableSupplementIds(rows: SupplementRow[]): string[] {
  return rows.filter((r) => r.selectable).map((r) => r.id);
}
