// The 문서 작성 layout follows the width in rem: the UI 배율 narrows it like a smaller window (UI-5).
import { describe, expect, it } from "vitest";

import { generateLayout } from "./layout";

describe("generateLayout", () => {
  it("is three columns from 64 rem and roomier from 80 rem", () => {
    // 1280 px window minus the 14 rem sidebar at 100 %: 1056 px = 66 rem.
    expect(generateLayout(1056, 16)).toBe("wide");
    expect(generateLayout(1024, 16)).toBe("wide");
    expect(generateLayout(1280, 16)).toBe("xwide");
    expect(generateLayout(1023, 16)).toBe("narrow");
  });

  it("counts a higher UI 배율 like a narrower window", () => {
    // The same 1280 px window at 150 %: the sidebar is 336 px, 944 px are left = 39 rem.
    expect(generateLayout(944, 24)).toBe("narrow");
    // 110 %: 1280 − 246 = 1034 px = 58.7 rem.
    expect(generateLayout(1034, 17.6)).toBe("narrow");
    // 90 %: 1280 − 202 = 1078 px = 74.9 rem.
    expect(generateLayout(1078, 14.4)).toBe("wide");
  });

  it("falls back to the narrow layout for an unmeasured box", () => {
    expect(generateLayout(0, 16)).toBe("narrow");
    expect(generateLayout(500, 0)).toBe("narrow");
  });
});
