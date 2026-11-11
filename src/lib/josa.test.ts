import i18next from "i18next";
import { describe, expect, it } from "vitest";

import { initI18n } from "@/i18n";
import ko from "@/i18n/ko.json";

import { endingOf, josa, josaPostProcessor, resolveJosa, withJosa, type JosaPair } from "./josa";

const PAIRS: JosaPair[] = ["을/를", "이/가", "은/는", "으로/로"];
const WRITTEN = /을\(를\)|이\(가\)|은\(는\)|\(으\)로/;

describe("josa", () => {
  it("picks by the final consonant of a Hangul word", () => {
    // 받침 없음
    expect(PAIRS.map((p) => josa("문서", p))).toEqual(["를", "가", "는", "로"]);
    expect(PAIRS.map((p) => josa("프로젝트", p))).toEqual(["를", "가", "는", "로"]);
    // 받침 있음
    expect(PAIRS.map((p) => josa("시스템", p))).toEqual(["을", "이", "은", "으로"]);
    expect(PAIRS.map((p) => josa("묶음", p))).toEqual(["을", "이", "은", "으로"]);
    // ㄹ 받침: 을 / 이 / 은, but 로
    expect(PAIRS.map((p) => josa("서울", p))).toEqual(["을", "이", "은", "로"]);
    expect(PAIRS.map((p) => josa("모델", p))).toEqual(["을", "이", "은", "로"]);
    expect(withJosa("프로젝트", "을/를")).toBe("프로젝트를");
    expect(withJosa("라이트", "으로/로")).toBe("라이트로");
    expect(withJosa("다크", "으로/로")).toBe("다크로");
    expect(withJosa("고대비", "으로/로")).toBe("고대비로");
    expect(withJosa("한국어", "으로/로")).toBe("한국어로");
  });

  it("reads a number in Korean", () => {
    // 영 일 이 삼 사 오 육 칠 팔 구
    expect([..."0123456789"].map((d) => josa(d, "을/를")).join("")).toBe("을을를을를를을을을를");
    // 일 · 칠 · 팔 end in ㄹ: 로; 영 · 삼 · 육 take 으로
    expect([..."0123456789"].map((d) => josa(d, "으로/로")).join(" ")).toBe("으로 로 로 으로 로 로 으로 로 로 로");
    // A number ending in zeros is read with its unit: 십 · 백 · 천 · 만 · 억 (받침), 조 (없음), 경 (받침).
    expect(josa("10", "을/를")).toBe("을");
    expect(josa("130", "으로/로")).toBe("으로");
    expect(josa("200", "으로/로")).toBe("으로");
    expect(josa("1,000", "이/가")).toBe("이");
    expect(josa("12,500,000", "은/는")).toBe("은");
    expect(josa("100000000", "을/를")).toBe("을");
    expect(josa("1,000,000,000,000", "을/를")).toBe("를");
    expect(josa(`1${"0".repeat(16)}`, "을/를")).toBe("을");
    expect(josa(`1${"0".repeat(20)}`, "을/를")).toBe("을(를)");
    // Other last digits read as themselves, whatever stands before them.
    expect(josa("2026", "으로/로")).toBe("으로");
    expect(josa("12", "으로/로")).toBe("로");
    expect(josa("문서 3", "이/가")).toBe("이");
    // After a decimal point every digit is read by itself: 3.0 삼 점 영, 2.5 이 점 오.
    expect(josa("3.0", "을/를")).toBe("을");
    expect(josa("2.5", "을/를")).toBe("를");
    expect(josa("0.8.0", "으로/로")).toBe("으로");
    // 130% → 퍼센트
    expect(josa("130%", "으로/로")).toBe("로");
    expect(endingOf("7")).toBe("rieul");
  });

  it("skips closing quotes and brackets", () => {
    expect(josa("'한빛 포털'", "을/를")).toBe("을");
    expect(josa("'신규 과제'", "을/를")).toBe("를");
    expect(josa("「계약서」", "이/가")).toBe("가");
    expect(josa("(주)한빛소프트(본사)", "은/는")).toBe("는");
    expect(josa("'10'", "으로/로")).toBe("으로");
  });

  it("keeps the written form when the ending cannot be read", () => {
    expect(PAIRS.map((p) => josa("report.pdf", p))).toEqual(["을(를)", "이(가)", "은(는)", "(으)로"]);
    expect(josa("Docs", "을/를")).toBe("을(를)");
    expect(josa("", "을/를")).toBe("을(를)");
    expect(josa("''", "을/를")).toBe("을(를)");
    expect(josa("gemma4-e4b-q6k", "으로/로")).toBe("(으)로");
    expect(josa("ㄱ", "을/를")).toBe("을(를)");
    expect(josa("%", "을/를")).toBe("을(를)");
    expect(endingOf("C:\\Work\\Docs")).toBe("unknown");
  });
});

describe("resolveJosa", () => {
  it("replaces every written form by the particle its word takes", () => {
    expect(resolveJosa("문서을(를) 유형별 폴더로 이동합니다")).toBe("문서를 유형별 폴더로 이동합니다");
    expect(resolveJosa("테마를 '시스템'(으)로 변경했습니다")).toBe("테마를 '시스템'으로 변경했습니다");
    expect(resolveJosa("테마를 '다크'(으)로 변경했습니다")).toBe("테마를 '다크'로 변경했습니다");
    expect(resolveJosa("'요구사항 정의서'을(를) 참고 자료(으)로 바꿨습니다.")).toBe("'요구사항 정의서'를 참고 자료로 바꿨습니다.");
    expect(resolveJosa("새 유형 '견적'이(가) 추가되었습니다.")).toBe("새 유형 '견적'이 추가되었습니다.");
    expect(resolveJosa("'회의록'은(는) 이미 쓰는 이름입니다.")).toBe("'회의록'은 이미 쓰는 이름입니다.");
    expect(resolveJosa("모델 티어를 8 GB(으)로 두었습니다.")).toBe("모델 티어를 8 GB(으)로 두었습니다.");
    // Undecidable endings keep the written form; decidable ones in the same sentence are still resolved.
    expect(resolveJosa("'정보보안지침.pdf'을(를) 업무 문서(으)로 바꿨습니다.")).toBe("'정보보안지침.pdf'을(를) 업무 문서로 바꿨습니다.");
    expect(resolveJosa("Nothing to do (yet)")).toBe("Nothing to do (yet)");
    expect(resolveJosa("")).toBe("");
  });
});

/** Every string of ko.json (flattened). */
function strings(tree: unknown, prefix = "", out: [string, string][] = []): [string, string][] {
  if (typeof tree === "string") out.push([prefix, tree]);
  else if (tree && typeof tree === "object") for (const [k, v] of Object.entries(tree)) strings(v, prefix ? `${prefix}.${k}` : k, out);
  return out;
}

describe("josaPostProcessor", () => {
  const instance = i18next.createInstance();
  void instance.use(josaPostProcessor).init({ lng: "ko", fallbackLng: false, resources: { ko: { translation: ko } }, interpolation: { escapeValue: false }, postProcess: [josaPostProcessor.name] });

  it("resolves the particles of translated sentences after interpolation", () => {
    expect(instance.t("projects.put.created", { name: "신규 과제" })).toBe("프로젝트 '신규 과제'를 만들었습니다.");
    expect(instance.t("projects.put.created", { name: "한빛 포털" })).toBe("프로젝트 '한빛 포털'을 만들었습니다.");
    expect(instance.t("theme.changed", { theme: "시스템" })).toBe("테마를 '시스템'으로 변경했습니다");
    expect(instance.t("theme.changed", { theme: "다크" })).toBe("테마를 '다크'로 변경했습니다");
    expect(instance.t("language.changed", { language: "한국어" })).toBe("언어를 '한국어'로 변경했습니다");
    expect(instance.t("projects.documents.kind_changed", { name: "요구사항정의서", kind: "참고 자료" })).toBe("'요구사항정의서'를 참고 자료로 바꿨습니다.");
    expect(instance.t("projects.documents.kind_switch_hint", { kind: "업무 문서" })).toBe("업무 문서로 전환");
    expect(instance.t("schemas.reclassify.banner", { name: "견적", count: 3 })).toContain("새 유형 '견적'이 추가되었습니다.");
    expect(instance.t("pipeline.process.model_not_loaded", { model: "gemma4-e4b-q4km" })).toContain("gemma4-e4b-q4km이(가) 로드되지");
    // A file name ends in Latin letters: the written form stays for it, the rest of the sentence is resolved.
    expect(instance.t("projects.documents.kind_changed", { name: "정보보안지침.pdf", kind: "업무 문서" })).toBe("'정보보안지침.pdf'을(를) 업무 문서로 바꿨습니다.");
  });

  it("leaves interpolated values exactly as they are", () => {
    // A title that itself contains a written form is user data: only the template's particle follows the title's last sound.
    expect(instance.t("generate.drafts_list.delete_confirm", { title: "갑은(는) 을에게 지급한다" })).toBe("초안 '갑은(는) 을에게 지급한다'를 삭제할까요? 저장한 파일은 남습니다.");
    expect(instance.t("projects.put.created", { name: "보고서을(를) 검토" })).toBe("프로젝트 '보고서을(를) 검토'를 만들었습니다.");
    expect(instance.t("knowledge.glossary.delete_confirm", { term: "(으)로" })).toContain("'(으)로'");
  });

  it("covers every string of ko.json that is written with a particle form", () => {
    const written = strings(ko).filter(([, s]) => WRITTEN.test(s));
    // The 22 strings of the 0.7.0 audit (UI-13); a new one is resolved without further work.
    expect(written.length).toBeGreaterThanOrEqual(22);
    for (const [key, template] of written) {
      const vars: Record<string, string> = {};
      for (const m of template.matchAll(/\{\{(\w+)\}\}/g)) vars[m[1]!] = "견적서";
      const consonant = Object.fromEntries(Object.keys(vars).map((k) => [k, "계약"]));
      expect(instance.t(key, vars), key).not.toMatch(WRITTEN);
      expect(instance.t(key, consonant), key).not.toMatch(WRITTEN);
      // 견적서 (no final consonant) and 계약 (final consonant) take different particles.
      expect(instance.t(key, vars).replaceAll("견적서", "계약"), key).not.toBe(instance.t(key, consonant));
    }
  });

  it("is switched on in the app's i18n instance", () => {
    const app = initI18n("ko");
    expect(app.t("projects.put.created", { name: "신규 과제" })).toBe("프로젝트 '신규 과제'를 만들었습니다.");
    expect(app.t("theme.changed", { theme: app.t("theme.system") })).not.toMatch(WRITTEN);
  });
});
