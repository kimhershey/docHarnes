// Names of the validation rules that have one (the review queue shows the name before the core's message).
import i18next from "i18next";
import { describe, expect, it } from "vitest";

import en from "@/i18n/en.json";
import ko from "@/i18n/ko.json";

import { validationRuleLabel } from "./labels";

const instance = i18next.createInstance();
await instance.init({ lng: "ko", fallbackLng: false, resources: { ko: { translation: ko }, en: { translation: en } }, interpolation: { escapeValue: false } });
const t = instance.t.bind(instance);

describe("validationRuleLabel", () => {
  it("names the 0.8.0 rules: a partially extracted page, a date whose day / month order is a guess", () => {
    expect(validationRuleLabel("partial_page", t)).toBe("일부 쪽만 추출됨");
    expect(validationRuleLabel("date_ambiguous", t)).toBe("일·월 순서가 모호한 날짜");
    const english = ((k: string, o?: Record<string, unknown>) => instance.t(k, { ...o, lng: "en" })) as typeof t;
    expect(validationRuleLabel("partial_page", english)).toBe(en.review.rules.partial_page);
    expect(validationRuleLabel("date_ambiguous", english)).toBe(en.review.rules.date_ambiguous);
    expect(Object.keys(en.review.rules).sort()).toEqual(Object.keys(ko.review.rules).sort());
  });

  it("has no name for the other rules: their message is shown as it is", () => {
    for (const rule of ["brn_checksum", "amount_parse", "sum_rule", "", "not a code", "review.rules"]) expect(validationRuleLabel(rule, t), rule).toBeNull();
  });
});
