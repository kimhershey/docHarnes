import { describe, expect, it } from "vitest";

import { coerceInteger, coerceIntList, coerceNumber } from "./numeric";

describe("plan editor numeric coercions (Rust u32 fields)", () => {
  it("coerceInteger truncates, rejects NaN and negatives, unsets on empty", () => {
    expect(coerceInteger("")).toBeUndefined();
    expect(coerceInteger("   ")).toBeUndefined();
    expect(coerceInteger("5")).toBe(5);
    expect(coerceInteger("2.7")).toBe(2);
    expect(coerceInteger("0")).toBe(0);
    expect(coerceInteger("-3")).toBeNull();
    expect(coerceInteger("-0.5")).toBe(0);
    expect(coerceInteger("abc")).toBeNull();
    expect(coerceInteger("1e3")).toBe(1000);
  });

  it("coerceInteger takes a lower bound (set_retention.days: -1 = forever)", () => {
    expect(coerceInteger("-1", -1)).toBe(-1);
    expect(coerceInteger("-2", -1)).toBeNull();
    expect(coerceInteger("0", -1)).toBe(0);
    expect(coerceInteger("30", -1)).toBe(30);
    expect(coerceInteger("", -1)).toBeUndefined();
  });

  it("coerceNumber keeps fractions and negatives but rejects NaN", () => {
    expect(coerceNumber("")).toBeUndefined();
    expect(coerceNumber("0.75")).toBe(0.75);
    expect(coerceNumber("-2")).toBe(-2);
    expect(coerceNumber("x")).toBeNull();
  });

  it("coerceIntList parses comma / newline lists, drops NaN and negatives, truncates", () => {
    expect(coerceIntList("")).toBeUndefined();
    expect(coerceIntList("1, 2,3")).toEqual([1, 2, 3]);
    expect(coerceIntList("1，2\n3")).toEqual([1, 2, 3]);
    expect(coerceIntList("2.5, -1, x, 4")).toEqual([2, 4]);
    expect(coerceIntList("-1, x")).toBeUndefined();
  });
});
