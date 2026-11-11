// Pure coercions for the plan editor's numeric fields (review M4): the Rust
// op types use `u32` for `search.top_k` and `ocr.pages`, so fractions,
// negatives and NaN must never reach `planner_execute` (a serde error there
// is a plain string, not a `UiError` the card could attach to a field).

/**
 * Integer field: "" → undefined (unset); "2.7" → 2; NaN or below `min` → null (rejected). `min` is 0 for
 * the core's unsigned fields; `set_retention.days` takes -1 (forever).
 */
export function coerceInteger(raw: string, min = 0): number | null | undefined {
  const s = raw.trim();
  if (s === "") return undefined;
  const n = Number(s);
  if (!Number.isFinite(n)) return null;
  const i = Math.trunc(n);
  if (i < min) return null;
  return i === 0 ? 0 : i; // Math.trunc(-0.5) is -0
}

/** Number field: "" → undefined (unset); NaN → null (rejected). */
export function coerceNumber(raw: string): number | null | undefined {
  const s = raw.trim();
  if (s === "") return undefined;
  const n = Number(s);
  return Number.isFinite(n) ? n : null;
}

/** Comma / newline separated integers: fractions truncated, NaN and negatives dropped; none → undefined. */
export function coerceIntList(raw: string): number[] | undefined {
  const items = raw
    .split(/[,，\n]/)
    .map((s) => coerceInteger(s))
    .filter((n): n is number => typeof n === "number");
  return items.length > 0 ? items : undefined;
}
