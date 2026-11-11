import i18next from "i18next";
import { describe, expect, it } from "vitest";

import en from "@/i18n/en.json";
import ko from "@/i18n/ko.json";

import registry from "../../../models/registry.json";
import { localizeModelDisplay } from "./useModelDisplay";

const instance = i18next.createInstance();
void instance.init({ lng: "en", fallbackLng: false, resources: { ko: { translation: ko }, en: { translation: en } }, interpolation: { escapeValue: false } });
const tEn = instance.getFixedT("en");
const tKo = instance.getFixedT("ko");

describe("localizeModelDisplay", () => {
  const displays = (registry as { models: { id: string; display: string }[] }).models.map((m) => m.display);

  it("translates the descriptor of every registry model in the English UI", () => {
    expect(displays.length).toBeGreaterThan(0);
    // The registry really carries Korean descriptors (otherwise this test checks nothing).
    expect(displays.some((d) => /[가-힣]/.test(d))).toBe(true);
    for (const display of displays) {
      const shown = localizeModelDisplay(display, "en", tEn);
      expect(shown, display).not.toMatch(/[가-힣]/);
      // Name and quantisation in front of the descriptor are untouched.
      expect(shown.slice(0, shown.indexOf("(") + 1), display).toBe(display.slice(0, display.indexOf("(") + 1));
    }
    expect(localizeModelDisplay("Gemma 4 E4B (Q6_K, 정확도)", "en", tEn)).toBe("Gemma 4 E4B (Q6_K, accuracy)");
    expect(localizeModelDisplay("Gemma 4 E2B (Q4_K_M, CPU·저여유)", "en", tEn)).toBe("Gemma 4 E2B (Q4_K_M, CPU·low memory)");
    expect(localizeModelDisplay("Gemma 4 E4B (Q5_K_M, 균형)", "en", tEn)).toBe("Gemma 4 E4B (Q5_K_M, balanced)");
    expect(localizeModelDisplay("EmbeddingGemma 2 (Q8_0, 임베딩 · CPU)", "en", tEn)).toBe("EmbeddingGemma 2 (Q8_0, embedding · CPU)");
  });

  it("shows the registry name as it is in the Korean UI", () => {
    for (const display of displays) expect(localizeModelDisplay(display, "ko", tKo)).toBe(display);
    expect(localizeModelDisplay("Gemma 4 E4B (Q6_K, 정확도)", "ko-KR", tKo)).toBe("Gemma 4 E4B (Q6_K, 정확도)");
  });

  it("leaves names without a descriptor and unknown words alone", () => {
    expect(localizeModelDisplay("gemma-4-E4B-it-Q4_K_M", "en", tEn)).toBe("gemma-4-E4B-it-Q4_K_M");
    // A dropped-in model named by its user: nothing in front of the descriptor is translated, unknown words stay.
    expect(localizeModelDisplay("균형 잡힌 모델 (실험용)", "en", tEn)).toBe("균형 잡힌 모델 (실험용)");
    expect(localizeModelDisplay("내 모델 (Q4_K_M, 정확도)", "en", tEn)).toBe("내 모델 (Q4_K_M, accuracy)");
  });

  it("has the same terms in both languages", () => {
    expect(Object.keys(en.models.display_terms).sort()).toEqual(Object.keys(ko.models.display_terms).sort());
    // The Korean values are the words the registry uses.
    for (const term of Object.values(ko.models.display_terms)) expect(displays.some((d) => d.includes(term)), term).toBe(true);
  });
});
