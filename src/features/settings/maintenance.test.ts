// Purge-job summary copy against the real Korean strings: the counts line with the cache size
// and the trash count, the skipped steps and the cancelled badge text.
import i18next from "i18next";
import { describe, expect, it } from "vitest";

import en from "@/i18n/en.json";
import ko from "@/i18n/ko.json";
import type { MaintenanceSummary } from "@/lib/types";

import { maintenanceSkippedText, maintenanceSummaryText } from "./maintenance";

const instance = i18next.createInstance();
await instance.init({ lng: "ko", fallbackLng: false, resources: { ko: { translation: ko }, en: { translation: en } }, interpolation: { escapeValue: false } });
const t = instance.t.bind(instance);

const summary: MaintenanceSummary = {
  started_at: "2026-11-08T03:00:00Z",
  elapsed_ms: 1200,
  purged: { journal: 3, jobs: 12, answers: 0, cache_bytes: 1.5 * 1024 * 1024 * 1024, trash: 4, logs: 2, temp: 1, documents: 0, extractions: 5 },
  skipped: ["documents", "chats", "disabled", "extractions: disk full"],
  cancelled: false,
};

describe("maintenance summary copy", () => {
  it("lists every purged count including the cache size and the trash", () => {
    expect(maintenanceSummaryText(summary, t)).toBe("저널 3 · 작업 12 · 대화 0 · 문서 0 · 추출 5 · 로그 2 · 임시 1 · 캐시 1.5 GiB · 휴지통 4");
  });
  it("names the skipped steps and keeps an error text", () => {
    expect(maintenanceSkippedText(summary, t)).toBe("건너뜀: 문서, 대화, 매일 실행 꺼짐, 추출 결과 (disk full)");
    expect(maintenanceSkippedText({ ...summary, skipped: [] }, t)).toBeNull();
    // Older shells without the field.
    expect(maintenanceSkippedText({ ...summary, skipped: undefined as unknown as string[] }, t)).toBeNull();
  });
});
