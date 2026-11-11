/**
 * Korean particles (조사) that depend on the last sound of the word in front of them (0.7.0 audit UI-13: the UI showed
 * "문서을(를) 이동합니다", "테마를 '시스템'(으)로 변경했습니다").
 *
 * - 을/를 · 이/가 · 은/는: the first form after a final consonant (받침), the second after a vowel.
 * - 으로/로: "으로" after a final consonant other than ㄹ, "로" after a vowel or ㄹ (서울로, 시스템으로).
 * - A number is read in Korean: 1 일 · 3 삼 · 6 육 · 7 칠 · 8 팔 end in a consonant, 2 이 · 4 사 · 5 오 · 9 구 do not,
 *   and a number ending in zeros takes the unit it is read with (10 십 · 100 백 · 1,000 천 · 10,000 만 · 억 · 조 · 경).
 * - Any other ending (Latin letters, symbols, an empty word) cannot be decided: the written form "을(를)" · "이(가)" ·
 *   "은(는)" · "(으)로" stays.
 *
 * `ko.json` keeps the written forms; `josaPostProcessor` (registered in `src/i18n/index.ts`) resolves them in every
 * translated string after interpolation, so no call site changes and a new string needs no formatter. Sentences the
 * core writes itself (plan summaries, step results) are not translated here and are resolved where they are written.
 */

export type JosaPair = "을/를" | "이/가" | "은/는" | "으로/로";

/** The form shown when the ending cannot be decided — and the form `ko.json` is written in. */
const WRITTEN: Record<JosaPair, string> = { "을/를": "을(를)", "이/가": "이(가)", "은/는": "은(는)", "으로/로": "(으)로" };
const PAIR_OF: Record<string, JosaPair> = { "을(를)": "을/를", "이(가)": "이/가", "은(는)": "은/는", "(으)로": "으로/로" };
const WRITTEN_RE = /을\(를\)|이\(가\)|은\(는\)|\(으\)로/g;

/** What the last sound of a word is, as far as the particles care. */
export type Ending = "vowel" | "rieul" | "consonant" | "unknown";

const HANGUL_BASE = 0xac00;
const HANGUL_LAST = 0xd7a3;
const JONGSEONG_COUNT = 28;
const JONGSEONG_RIEUL = 8;

/** Reading of a single digit: 영 일 이 삼 사 오 육 칠 팔 구. */
const DIGIT_ENDING: Ending[] = ["consonant", "rieul", "vowel", "consonant", "vowel", "vowel", "consonant", "rieul", "rieul", "vowel"];

/** Closing quotes and brackets between a word and its particle: `'한빛 포털'을(를)` is decided by "털". */
const CLOSERS = new Set(["'", '"', "’", "”", "」", "』", "》", "〉", ")", "]", "}", ">"]);

/** A number ending in zeros is read with its unit: 십 · 백 · 천 · 만 (10⁴–10⁷) · 억 (10⁸–10¹¹) · 조 (10¹²–10¹⁵) · 경. */
function zerosEnding(zeros: number): Ending {
  if (zeros >= 20) return "unknown";
  if (zeros >= 16) return "consonant"; // 경
  if (zeros >= 12) return "vowel"; // 조
  return "consonant"; // 십 · 백 · 천 · 만 · 억
}

/** Ending of the number that `text` ends with (its last character is a digit). */
function numberEnding(text: string): Ending {
  // The digits (with thousands separators and one decimal point) the text ends with.
  const token = /[\d,]*\.?\d+$/.exec(text)?.[0] ?? "";
  const last = Number(token[token.length - 1]);
  if (!Number.isInteger(last)) return "unknown";
  if (last !== 0) return DIGIT_ENDING[last]!;
  // A zero after the decimal point is read by itself (3.0 → 삼 점 영); so is a bare 0.
  if (token.includes(".")) return DIGIT_ENDING[0]!;
  const digits = token.replace(/,/g, "");
  const zeros = digits.length - digits.replace(/0+$/, "").length;
  return zeros === digits.length ? DIGIT_ENDING[0]! : zerosEnding(zeros);
}

/** The last sound of `word`: closing quotes / brackets are skipped, Hangul and numbers are read, the rest is unknown. */
export function endingOf(word: string): Ending {
  let end = word.length;
  while (end > 0 && CLOSERS.has(word[end - 1]!)) end--;
  if (end === 0) return "unknown";
  const text = end === word.length ? word : word.slice(0, end);
  const code = text.charCodeAt(end - 1);
  if (code >= HANGUL_BASE && code <= HANGUL_LAST) {
    const jong = (code - HANGUL_BASE) % JONGSEONG_COUNT;
    return jong === 0 ? "vowel" : jong === JONGSEONG_RIEUL ? "rieul" : "consonant";
  }
  if (code >= 0x30 && code <= 0x39) return numberEnding(text);
  // "130%" is read 퍼센트.
  if (text[end - 1] === "%" && end > 1 && /\d/.test(text[end - 2]!)) return "vowel";
  return "unknown";
}

/**
 * The particle of `pair` that follows `word` — `josa("문서", "을/를")` is "를", `josa("시스템", "으로/로")` is "으로" —
 * or the written form ("을(를)", "(으)로") when the ending cannot be decided.
 */
export function josa(word: string, pair: JosaPair): string {
  const ending = endingOf(word);
  if (ending === "unknown") return WRITTEN[pair];
  const [afterConsonant, afterVowel] = pair.split("/") as [string, string];
  if (pair === "으로/로") return ending === "consonant" ? afterConsonant : afterVowel;
  return ending === "vowel" ? afterVowel : afterConsonant;
}

/** `word` with its particle attached: `withJosa("프로젝트", "을/를")` is "프로젝트를". */
export function withJosa(word: string, pair: JosaPair): string {
  return word + josa(word, pair);
}

/** Whether `text` holds a written particle form. */
function hasWritten(text: string): boolean {
  return text.includes("(") && text.search(WRITTEN_RE) >= 0;
}

/**
 * Replaces every written form in a sentence by the particle its preceding word takes:
 * "프로젝트 '신규 과제'을(를) 만들었습니다." → "프로젝트 '신규 과제'를 만들었습니다."; a form after an undecidable
 * ending ("'report.pdf'을(를)") is left as it is.
 */
export function resolveJosa(text: string): string {
  if (!hasWritten(text)) return text;
  return text.replace(WRITTEN_RE, (form: string, offset: number) => josa(text.slice(0, offset), PAIR_OF[form]!));
}

/** Private-use sentinel put inside a written form to keep `resolveJosa` away from it. */
const GUARD = "";
const guard = (text: string): string => text.replace(WRITTEN_RE, (form) => form[0] + GUARD + form.slice(1));

/**
 * i18next post-processor (`postProcess: ["josa"]`): resolves the written forms of the translated sentence. Interpolated
 * values are user data (a document title may itself contain "을(를)") and are left exactly as they are — only the
 * forms of the template change.
 */
export const josaPostProcessor = {
  type: "postProcessor" as const,
  name: "josa",
  process(value: string, _key: string | string[], options: Record<string, unknown> | undefined): string {
    if (typeof value !== "string" || !hasWritten(value)) return value;
    let text = value;
    let guarded = false;
    for (const [name, v] of Object.entries(options ?? {})) {
      if (typeof v !== "string" || name.startsWith("defaultValue") || !hasWritten(v) || !text.includes(v)) continue;
      text = text.split(v).join(guard(v));
      guarded = true;
    }
    const resolved = resolveJosa(text);
    return guarded ? resolved.split(GUARD).join("") : resolved;
  },
};
