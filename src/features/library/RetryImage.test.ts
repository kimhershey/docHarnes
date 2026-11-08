// Cache-busting retry URLs of purged page images (`doculens://` 404 → re-render → retry).
import { describe, expect, it } from "vitest";

import { MAX_RETRIES, RETRY_DELAY_MS, retryUrl } from "./RetryImage";

describe("retryUrl", () => {
  it("appends r=<attempt> once, after existing queries, keeping the hash", () => {
    expect(retryUrl("doculens://pages/d1/3.png", 0)).toBe("doculens://pages/d1/3.png");
    expect(retryUrl("doculens://pages/d1/3.png", 1)).toBe("doculens://pages/d1/3.png?r=1");
    expect(retryUrl("doculens://pages/d1/3.png?v=2", 2)).toBe("doculens://pages/d1/3.png?v=2&r=2");
    expect(retryUrl("doculens://pages/d1/3.png#top", 3)).toBe("doculens://pages/d1/3.png?r=3#top");
  });
  it("leaves inline sources alone and keeps the retry budget small", () => {
    expect(retryUrl("data:image/png;base64,AAAA", 2)).toBe("data:image/png;base64,AAAA");
    expect(retryUrl("blob:http://localhost/abc", 1)).toBe("blob:http://localhost/abc");
    expect(MAX_RETRIES).toBe(3);
    expect(RETRY_DELAY_MS).toBe(1500);
  });
});
