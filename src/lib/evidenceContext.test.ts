import { describe, expect, it } from "vitest";

import { chatContext, contextNeeded, DEFAULT_CONTEXT_TOKENS } from "./evidenceContext";

// The same numbers as the core's tests (`answer.rs::a_context_of_the_needed_size_holds_the_budget_and_its_reply`,
// `presets.rs::a_larger_evidence_budget_loads_the_model_with_a_context_that_holds_it`).
describe("the model context an evidence budget asks for (1.0.6)", () => {
  it("asks for nothing up to the default budget", () => {
    for (const budget of [1000, 3000, DEFAULT_CONTEXT_TOKENS]) expect(contextNeeded(budget)).toBeNull();
    for (const profile of [8192, 16384, 32768]) expect(chatContext(profile, 131072, DEFAULT_CONTEXT_TOKENS)).toBe(profile);
  });

  it("holds the budget, a summary's reply and the scaffold in a multiple of 1,024", () => {
    expect(contextNeeded(6001)).toBe(9216);
    expect(contextNeeded(8000)).toBe(11264);
    expect(contextNeeded(24000)).toBe(30720);
    expect(contextNeeded(60000)).toBe(75776);
    expect(contextNeeded(200000)).toBe(250880);
    expect(contextNeeded(262144)).toBe(328704);
  });

  it("grows a profile's context and stops at the model's maximum", () => {
    // Gemma 4 E4B (ctx_max 131,072): Balanced on a GPU (16,384) and the CPU pack (8,192).
    expect(chatContext(16384, 131072, 8000)).toBe(16384);
    expect(chatContext(8192, 131072, 8000)).toBe(11264);
    for (const profile of [8192, 16384]) {
      expect(chatContext(profile, 131072, 24000)).toBe(30720);
      expect(chatContext(profile, 131072, 60000)).toBe(75776);
      expect(chatContext(profile, 131072, 200000)).toBe(131072);
    }
    expect(chatContext(32768, 131072, 24000)).toBe(32768);
    expect(chatContext(16384, 262144, 200000)).toBe(250880);
    expect(chatContext(16384, 0, 60000)).toBe(75776);
  });
});
