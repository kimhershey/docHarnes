// The note under an answer whose request text or reply was cut (`Answer.truncated`, KNO-3).
import i18next from "i18next";
import { describe, expect, it } from "vitest";

import en from "@/i18n/en.json";
import ko from "@/i18n/ko.json";

import { truncationNote } from "./AnswerCard";

const instance = i18next.createInstance();
await instance.init({ lng: "ko", fallbackLng: false, resources: { ko: { translation: ko }, en: { translation: en } }, interpolation: { escapeValue: false } });
const t = (k: string) => instance.t(k);

describe("truncationNote", () => {
  it("says which side was cut: the text that was sent, the reply, or both", () => {
    expect(truncationNote("input", t)).toBe(ko.answer.truncated_input);
    expect(truncationNote("output", t)).toBe(ko.answer.truncated_output);
    expect(truncationNote("both", t)).toBe(`${ko.answer.truncated_input} ${ko.answer.truncated_output}`);
    expect(ko.answer.truncated_input).toContain("앞부분만 처리했습니다");
    expect(ko.answer.truncated_output).toContain("길이 한도에서 끊겼습니다");
  });

  it("is absent for an answer that was not cut", () => {
    expect(truncationNote(undefined, t)).toBeNull();
    expect(truncationNote(null, t)).toBeNull();
  });

  it("has English copy", () => {
    const english = (k: string) => instance.t(k, { lng: "en" });
    expect(truncationNote("both", english)).toBe(`${en.answer.truncated_input} ${en.answer.truncated_output}`);
  });
});
