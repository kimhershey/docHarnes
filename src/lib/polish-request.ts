// 1.0.6 (user decision 2026-11-24): "다듬어줘 / 수정해줘" is not document writing. A request to polish /
// fix / revise the wording of a text is answered in the chat — over the text the sentence brings, or else
// over the assistant's last reply; a document is written only when the sentence asks for a new document or
// names one. A mirror of the core's `planner::guard::polish_words` for the mock planner.

import { carriedText } from "./carried-text";
import type { ConversationTurn } from "./types";

/** Verbs that are about wording, whatever they are said of (request forms of the nouns: "교정 일정" is no request). */
const POLISH_VERBS = ["다듬", "손봐", "윤문해", "윤문 좀", "윤문 부탁", "교정해", "교정 좀", "교정 부탁", "교정 봐", "polish*", "proofread*", "reword*", "refine*", "rephras*"];
/** "Write it again": about wording next to a word for the text or a manner, with a text, or when nothing else is said. */
const AGAIN_VERBS = ["다시 써", "다시 작성", "다시 적어", "rewrit*", "rewrote"];
/** Verbs that change anything at all: about wording only next to a word for the text, a manner, or a text. */
const CHANGE_VERBS = ["수정해", "수정하", "수정 부탁", "수정 좀", "수정 바", "고쳐", "revise*"];
/** "더 간결하게": a request on its own only when the sentence says nothing else. */
const POLISH_ADVERBS = ["자연스럽게", "간결하게", "매끄럽게"];
const DONE_FORMS = ["다듬었", "수정했", "고쳤", "교정했", "윤문했", "손봤"];
const REQUEST_NOUNS = ["수정", "교정", "윤문"];
const MANNER_WORDS = ["자연스럽", "간결", "매끄럽", "짧게", "쉽게", "정중", "공손", "부드럽게", "명확하게", "깔끔하게", "말투", "어투", "어조", "문체", "존댓말", "반말", "격식", "오타", "맞춤법", "띄어쓰기", "문법", "typo*", "grammar", "spelling", "concise*", "natural*", "formal*", "polite*", "shorter", "simpler", "tone"];
/** Words for the text of a conversation, as whole words (a particle may follow). */
const TEXT_WORDS = ["방금", "아까", "위", "앞", "이전", "직전", "마지막", "답변", "대답", "응답", "답", "내용", "글", "윗글", "문장", "문구", "표현", "문단", "단락", "부분", "요약", "요약문", "설명", "이거", "그거", "이것", "그것", "이걸", "그걸", "above", "previous", "last", "answer", "reply", "response", "text", "sentence", "sentences", "paragraph", "wording", "summary", "it", "this", "that"];
const FILLER_WORDS = ["좀", "더", "좀더", "조금", "약간", "다시", "한번", "한", "번", "훨씬", "살짝", "이", "그", "저", "please", "a", "bit", "more", "much", "little", "the", "make", "again"];
const DO_WORDS = ["해", "써", "줘"];
const DO_STEMS = ["해줘", "해주", "해줄", "해봐", "써줘", "써주", "써줄", "써봐", "작성해", "바꿔", "말해", "적어", "부탁", "주세요", "줄래", "주실", "rewrit"];
/** A document is asked for (the names of the generator's documents come with the call). */
const DOCUMENT_WORDS = ["문서", "파일", "초안", "양식", "서식", "작성해", "작성 해", "작성하", "만들", "생성", "document*", "file*", "draft*", "create", "compose", "generate", "make a", "prepare a", "write a"];
/** Things of the app a sentence may ask to change. */
const APP_WORDS = ["설정", "테마", "언어", "모델", "프로필", "프로젝트", "태그", "스키마", "규칙", "템플릿", "파일명", "이름", "제목", "폴더", "유형", "필드", "워크스페이스", "참고 자료", "원본", "페이지", "그림", "시트", "엔진", "단축키", "글꼴", "배율", "theme", "model", "profile", "project*", "tag*", "schema*", "rule*", "template*", "filename", "folder*", "field*", "setting*", "workspace", "page*", "cell*", "sheet*"];
const APP_SHORT_WORDS = ["쪽", "값", "칸", "셀"];
/** The open document, the selection, a page (`rules::SELECTION_WORDS`). */
const DOCUMENT_POINTERS = ["이문서", "이계약서", "이영수증", "이파일", "선택문서", "선택한", "선택된", "현재문서", "열린문서", "지금문서", "선택영역", "이페이지", "현재페이지", "this document", "this file", "this page", "selected", "the selection", "these documents", "current document", "this contract", "this receipt"];
const OTHER_ACTIONS = ["저장", "내보내", "메일", "인쇄", "찾아", "검색", "열어", "export*", "save", "send", "email", "print", "find", "search", "open"];
const NEGATIONS = ["다듬지 마", "다듬지 말", "수정하지 마", "수정하지 말", "고치지 마", "고치지 말", "교정하지 마", "교정하지 말", "손보지 마", "손보지 말", "쓰지 마", "쓰지 말", "don't", "do not", "never"];
const QUESTION_WORDS = ["방법", "는 법", "요령", "어떻게", "how to", "how do", "how can"];
const REQUEST_ENDINGS = ["줘", "주세요", "줄래", "줄 수", "줄수", "주실", "주시겠", "주겠", "please", "can you", "could you", "would you"];
const PARTICLES = ["에서", "으로", "에는", "에도", "까지", "을", "를", "은", "는", "이", "가", "의", "도", "만", "로", "에"];
const MAX_INSTRUCTION_CHARS = 160;
const MIN_REPLY_BODY_CHARS = 20;
const MIN_QUOTED_BODY_CHARS = 10;
const MAX_CARRIED_TEXT_CHARS = 60_000;

/** A 되묻기 text (`askBackText`): the sentence, the assistant's question, the user's reply. */
const ASK_BACK = /^([\s\S]*?)\n\n\[(?:어시스턴트 질문|Assistant question)\] ([\s\S]*?)\n\[(?:사용자 답변|User reply)\] ([\s\S]*)$/;

const chars = (s: string): number => Array.from(s).length;
const compactOf = (s: string): string => Array.from(s.toLowerCase()).filter((c) => /[\p{L}\p{N}]/u.test(c)).join("");
const isAscii = (s: string): boolean => /^[\x00-\x7f]*$/.test(s);

/**
 * Trigger matching the way the core's phrase rules do it (`rules::Text::has`): Korean as a part of the compact
 * text; an English word as a whole word (`"move"` is not in "remove"), with a trailing `*` as the beginning of
 * one; an English phrase or a long token as a part of the compact text.
 */
export function says(words: string, triggers: readonly string[]): boolean {
  const lower = words.toLowerCase();
  const compact = compactOf(words);
  // Lower-case, every run of punctuation / whitespace one space, a Korean particle glued to a Latin word ends it.
  const norm = ` ${lower
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .replace(/([\x00-\x7f])(?=[^\x00-\x7f\s])|([^\x00-\x7f\s])(?=[\x00-\x7f])/gu, (m) => (m === " " ? m : `${m} `))
    .replace(/\s+/g, " ")
    .trim()} `;
  return triggers.some((w) => {
    const prefix = w.endsWith("*");
    const raw = prefix ? w.slice(0, -1) : w;
    if (!isAscii(raw)) return compact.includes(compactOf(raw));
    const key = raw.trim().toLowerCase();
    if (!key) return false;
    if (/[^a-z0-9 ]/.test(key)) return lower.includes(key);
    if (key.includes(" ") || key.length >= 9) return compact.includes(compactOf(key));
    return prefix ? norm.includes(` ${key}`) : norm.includes(` ${key} `);
  });
}

/** The words of a text, lower-case, without punctuation, each without one trailing particle. */
function wordStems(text: string): string[] {
  return text
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean)
    .map((w) => {
      const particle = PARTICLES.find((p) => w.endsWith(p) && w.length > p.length);
      return particle ? w.slice(0, -particle.length) : w;
    });
}

function endsInRequestNoun(words: string): boolean {
  const w = words.replace(/[^\p{L}\p{N}]+$/u, "");
  return REQUEST_NOUNS.some((n) => w.endsWith(n));
}

function hasPolishWord(words: string): boolean {
  return says(words, POLISH_VERBS) || says(words, AGAIN_VERBS) || says(words, CHANGE_VERBS) || says(words, POLISH_ADVERBS) || endsInRequestNoun(words);
}

/** Every quoted segment in order (`rules::quoted_all`). */
function quotes(raw: string): string[] {
  return [...raw.matchAll(/["'“‘「『]([^"'”’」』]*)["'”’」』]/gu)].map((m) => m[1]!.trim()).filter(Boolean);
}

function outsideQuotes(raw: string): string {
  return raw.replace(/["'“‘「『][^"'”’」』]*["'”’」』]/gu, " ");
}

/** "3절", "결론 절", "section 2" (`rules::names_section`). */
function namesSection(words: string): boolean {
  return /\d\s*절/u.test(words) || /(^|[\s\d])절($|[^가-힣]|[을를이가은는도만의])/u.test(words) || /section/i.test(words);
}

/** The instruction and the text of a sentence that brings its text along, however short the text. */
function broughtText(text: string): [string, string] | null {
  const asked = text.match(ASK_BACK);
  const command = asked ? asked[1]!.trimEnd() : text;
  const said = asked ? `${command}\n${asked[3]!.trim()}` : text;
  const carried = carriedText(command);
  if (carried) return [said.split(carried).join(" "), carried];
  if (asked) {
    const reply = asked[3]!.trim();
    return chars(reply) >= MIN_REPLY_BODY_CHARS && !hasPolishWord(reply) && hasPolishWord(command) ? [command.trim(), reply] : null;
  }
  const t = command.trim();
  for (const sep of [":", "：", "\n"]) {
    let pos = -1;
    for (let i = t.indexOf(sep); i >= 0; i = t.indexOf(sep, i + 1)) {
      if (/\d$/.test(t.slice(0, i)) && /^\d/.test(t.slice(i + sep.length))) continue; // "10:30"
      pos = i;
      break;
    }
    if (pos < 0) continue;
    const head = t.slice(0, pos).trim();
    const tail = t.slice(pos + sep.length).trim();
    if (chars(head) <= MAX_INSTRUCTION_CHARS && hasPolishWord(head) && chars(tail) >= 2) return [head, tail];
  }
  const nl = t.lastIndexOf("\n");
  if (nl >= 0) {
    const body = t.slice(0, nl).trim();
    const tail = t.slice(nl + 1).trim();
    if (chars(tail) <= MAX_INSTRUCTION_CHARS && hasPolishWord(tail) && chars(body) >= 2) return [tail, body];
  }
  const quoted = quotes(t);
  if (quoted.length === 1 && chars(quoted[0]!) >= MIN_QUOTED_BODY_CHARS && hasPolishWord(outsideQuotes(t))) {
    // (a passage opens at the beginning of a word: the apostrophes of "it's … doesn't" quote nothing)
    const before = t.slice(0, t.indexOf(quoted[0]!)).trimEnd().slice(0, -1);
    if (before === "" || /\s$/.test(before)) return [outsideQuotes(t).trim(), quoted[0]!];
  }
  return null;
}

/** What the planner knows of the conversation and the workspace (`PlanContext`). */
export interface PolishContext {
  doc_types: { label_ko: string; label_en: string }[];
  schemas: { name: string }[];
  history: ConversationTurn[];
  /** The assistant's last reply in full: `""` = there is none, absent = the last assistant turn of `history`. */
  last_reply?: string | null;
}

/** The assistant's last reply — never the question a reply to a question back answers (`guard::last_reply`). */
function lastReply(text: string, ctx: PolishContext): string | null {
  if (ctx.last_reply != null) return ctx.last_reply.trim() || null;
  const asked = compactOf(text.match(ASK_BACK)?.[2] ?? "");
  for (const turn of [...ctx.history].reverse()) {
    if (turn.role !== "assistant") continue;
    const key = compactOf(turn.text.trim().replace(/…+$/u, ""));
    if (key && !(asked && asked.includes(key))) return turn.text.trim();
  }
  return null;
}

/** A request to polish / fix / revise the wording of a text (`guard::Polish`). */
export interface Polish {
  /** The user's words without the text they bring. */
  instruction: string;
  /** The request and the text to work on ("instruction:\ntext"); null when there is no text. */
  question: string | null;
  /** Whether the text is the assistant's last reply (else the sentence brought it). */
  fromReply: boolean;
}

/**
 * Whether the user's words ask for the wording of a text to be polished / fixed / revised, and for nothing
 * else (`guard::polish_words`). When in doubt, they do not. `templateWords`: the names of the documents the
 * generator writes ("보고서", "회의록" …).
 */
export function polishRequest(text: string, ctx: PolishContext, templateWords: readonly string[]): Polish | null {
  const asked = text.match(ASK_BACK);
  const brought = broughtText(text);
  const instruction = (brought ? brought[0] : asked ? `${asked[1]!.trimEnd()}\n${asked[3]!.trim()}` : text).trim();
  if (!instruction || chars(instruction) > MAX_INSTRUCTION_CHARS) return null;
  const stems = wordStems(instruction);
  // 1. What asks for it.
  const wording = says(instruction, POLISH_VERBS);
  const again = says(instruction, AGAIN_VERBS);
  const change = says(instruction, CHANGE_VERBS) || endsInRequestNoun(instruction);
  const adverb = says(instruction, POLISH_ADVERBS);
  const aboutText = brought !== null || stems.some((s) => TEXT_WORDS.includes(s)) || says(instruction, MANNER_WORDS);
  const nothingElse = stems.every((s) => FILLER_WORDS.includes(s) || TEXT_WORDS.includes(s) || DO_WORDS.includes(s) || DO_STEMS.some((d) => s.startsWith(d)) || says(s, MANNER_WORDS));
  if (!(wording || ((again || change) && aboutText) || ((again || adverb) && nothingElse))) return null;
  // 2. What it is not. ("다시 써줘" and "다시 작성해줘" write the text again, not a document.)
  const compact = compactOf(instruction);
  const lower = instruction.toLowerCase();
  const withoutAgain = instruction.split("다시 작성").join(" ").split("다시작성").join(" ");
  const names = [...ctx.doc_types.flatMap((d) => [d.label_ko, d.label_en]), ...ctx.schemas.map((s) => s.name)].map(compactOf).filter((k) => chars(k) >= 3);
  const asksDocument = says(withoutAgain, DOCUMENT_WORDS) || says(instruction, templateWords) || namesSection(instruction) || names.some((k) => compact.includes(k));
  const namesAppThing = says(instruction, APP_WORDS) || says(instruction, DOCUMENT_POINTERS) || stems.some((s) => APP_SHORT_WORDS.includes(s.replace(/^\d+/, "")));
  const replaces = quotes(instruction).length >= 2;
  const question = says(instruction, QUESTION_WORDS) || (/[?？]$/.test(instruction) && !REQUEST_ENDINGS.some((w) => lower.includes(w)));
  if (asksDocument || namesAppThing || replaces || says(instruction, OTHER_ACTIONS) || says(instruction, NEGATIONS) || says(instruction, DONE_FORMS) || question) return null;
  // 3. The text to work on.
  const head = instruction
    .replace(/[^\p{L}\p{N}]+$/u, "")
    .split(/\s+/)
    .filter(Boolean)
    .join(" ");
  const layout = (body: string) =>
    `${head}:\n${Array.from(body.trim())
      .slice(0, Math.max(0, MAX_CARRIED_TEXT_CHARS - chars(head) - 2))
      .join("")}`;
  if (brought) return { instruction: head, question: asked ? layout(brought[1]) : text.trim(), fromReply: false };
  const reply = lastReply(text, ctx);
  return { instruction: head, question: reply ? layout(reply) : null, fromReply: reply !== null };
}

/** What is asked when a polish request has no text to work on (`guard::polish_what`). */
export function polishWhat(ko: boolean): string {
  return ko ? "어떤 글을 다듬을까요? 글을 함께 보내 주시거나, 답변을 받은 뒤 '방금 답변 다듬어줘'라고 해 주세요." : 'Which text? Send it along with the request, or ask again after an answer ("polish the last answer").';
}
