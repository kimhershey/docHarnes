// Job row copy against the real Korean strings: the P5 progress / summary
// patterns of the persistent runner and the localisation of `Job.error`.
import i18next from "i18next";
import { describe, expect, it } from "vitest";

import en from "@/i18n/en.json";
import ko from "@/i18n/ko.json";
import type { Job } from "@/lib/types";

import { describeJobDetail, describeJobError } from "./describe";

const instance = i18next.createInstance();
await instance.init({ lng: "ko", fallbackLng: false, resources: { ko: { translation: ko }, en: { translation: en } }, interpolation: { escapeValue: false } });
const t = instance.t.bind(instance);

function job(patch: Partial<Job>): Job {
  return {
    id: "job1",
    kind: "plan",
    subject: "plan",
    state: "done",
    progress: 1,
    detail: null,
    bytes_done: null,
    bytes_total: null,
    error: null,
    cancel_requested: false,
    created_at: "2026-11-04T00:00:00Z",
    updated_at: "2026-11-04T00:00:00Z",
    finished_at: null,
    priority: "normal",
    persistent: true,
    attempts: 1,
    ...patch,
  };
}

describe("describeJobDetail (P5 persistent runner patterns)", () => {
  it("translates the plan summary and step progress lines", () => {
    expect(describeJobDetail(job({ detail: "plan 2/3 · 문서 3건 내보냄" }), t)).toBe("플랜 2/3 · 문서 3건 내보냄");
    expect(describeJobDetail(job({ detail: "1/3 · export" }), t)).toBe("1/3 · 내보내기");
    expect(describeJobDetail(job({ detail: "2/3 · ocr · 영수증-1p.jpg (1/2)" }), t)).toBe("2/3 · OCR · 영수증-1p.jpg (1/2)");
    expect(describeJobDetail(job({ detail: "3/3 · classify · 세금계산서.pdf (2/2)" }), t)).toBe(`3/3 · ${ko.pipeline.levels.classify} · 세금계산서.pdf (2/2)`);
  });

  it("translates the export / organise / undo summaries", () => {
    expect(describeJobDetail(job({ detail: "exported 3 documents → exports/2026-11-04_영수증.xlsx" }), t)).toBe("문서 3건 내보냄 → exports/2026-11-04_영수증.xlsx");
    expect(describeJobDetail(job({ detail: "moved 2 / failed 0 / skipped 1" }), t)).toBe("이동 2 · 실패 0 · 건너뜀 1");
    expect(describeJobDetail(job({ detail: "restored 1 / skipped 0" }), t)).toBe("복구 1 · 불가 0");
    expect(describeJobDetail(job({ detail: "nothing to undo" }), t)).toBe(ko.jobs.detail.nothing_to_undo);
    expect(describeJobDetail(job({ detail: "nothing to redo" }), t)).toBe(ko.jobs.detail.nothing_to_redo);
  });

  it("translates the terminal states and keeps unknown text verbatim", () => {
    expect(describeJobDetail(job({ detail: "interrupted" }), t)).toBe(ko.jobs.detail.interrupted);
    expect(describeJobDetail(job({ detail: "workspace closed" }), t)).toBe(ko.jobs.detail.workspace_closed);
    expect(describeJobDetail(job({ detail: "cancelled" }), t)).toBe(ko.jobs.state.cancelled);
    expect(describeJobDetail(job({ detail: "loading model" }), t)).toBe(ko.jobs.detail.loading_model);
    expect(describeJobDetail(job({ detail: "something else" }), t)).toBe("something else");
  });

  it("translates the draft job's stage words and keeps its localised finishing detail", () => {
    // `generate_bridge::run_draft` emits exactly "loading model" and "drafting".
    expect(describeJobDetail(job({ kind: "draft", detail: "loading model" }), t)).toBe(ko.jobs.detail.loading_model);
    expect(describeJobDetail(job({ kind: "draft", detail: "drafting" }), t)).toBe(ko.jobs.detail.drafting);
    expect(describeJobDetail(job({ kind: "draft", detail: "견적서 · 필드 7 · 검토 항목 1" }), t)).toBe("견적서 · 필드 7 · 검토 항목 1");
    // The maintenance job reports its summary already localised (no stage words).
    expect(describeJobDetail(job({ kind: "maintenance", detail: "저널 3 · 작업 12 · 대화 0 · 추출 0 · 문서 0 · 캐시 0 MB" }), t)).toBe("저널 3 · 작업 12 · 대화 0 · 추출 0 · 문서 0 · 캐시 0 MB");
    expect(describeJobDetail(job({ detail: null }), t)).toBeNull();
  });
});

describe("describeJobError", () => {
  it("is null without an error", () => {
    expect(describeJobError(job({ error: null }), t)).toBeNull();
  });

  it("maps the stable shell / worker codes to jobs.error.*", () => {
    expect(describeJobError(job({ error: "engine_not_running" }), t)).toBe(ko.jobs.error.engine_not_running);
    expect(describeJobError(job({ error: "model_not_loaded" }), t)).toBe(ko.jobs.error.model_not_loaded);
    expect(describeJobError(job({ error: "workspace closed" }), t)).toBe(ko.jobs.error.workspace_closed);
    expect(describeJobError(job({ error: "interrupted" }), t)).toBe(ko.jobs.error.interrupted);
    expect(describeJobError(job({ error: "cancelled" }), t)).toBe(ko.jobs.error.cancelled);
    expect(describeJobError(job({ error: "no_model" }), t)).toBe(ko.jobs.error.no_model);
    expect(describeJobError(job({ error: "export_exists" }), t)).toBe(ko.jobs.error.export_exists);
    expect(describeJobError(job({ error: "path_outside" }), t)).toBe(ko.jobs.error.path_outside);
    expect(describeJobError(job({ error: "no_documents" }), t)).toBe(ko.jobs.error.no_documents);
  });

  it("carries the schema name and translates the op of a failed plan step", () => {
    expect(describeJobError(job({ error: "schema_not_found: 영수증 v2" }), t)).toBe("스키마를 찾을 수 없습니다: 영수증 v2");
    expect(describeJobError(job({ error: "export: export_exists" }), t)).toBe(`${ko.planner.ops.export}: ${ko.jobs.error.export_exists}`);
    expect(describeJobError(job({ error: "extract: schema_not_found: 계약서" }), t)).toBe(`${ko.planner.ops.extract}: 스키마를 찾을 수 없습니다: 계약서`);
  });

  it("finds a known code inside a longer message and keeps unknown text verbatim", () => {
    expect(describeJobError(job({ error: "worker: engine_not_running (retry)" }), t)).toBe(ko.jobs.error.engine_not_running);
    expect(describeJobError(job({ error: "disk full" }), t)).toBe("disk full");
  });

  it("uses the English strings in English", () => {
    expect(describeJobError(job({ error: "workspace closed" }), ((k: string, o?: Record<string, unknown>) => instance.t(k, { ...o, lng: "en" })) as typeof t)).toBe(en.jobs.error.workspace_closed);
  });
});
