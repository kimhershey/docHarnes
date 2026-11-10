// The text a request brings along ("다듬어줘: …", "…\n위 문장을 교정해줘"): a mirror of the core's
// `planner::guard::carried_text`, shared by the mock planner (routing) and the mock knowledge base (the
// text-only answer).

/** Verbs that act on text the user supplied (mirrors `planner::guard`). */
const REWRITE_WORDS = ["다듬", "정리해", "정리 해", "교정", "맞춤법", "번역", "고쳐", "다시 써", "다시써", "윤문", "요약해", "줄여", "풀어 써", "풀어써", "바꿔 써", "존댓말", "반말", "문장으로", "개조식으로", "매끄럽", "rewrite", "polish", "proofread", "translate", "tidy", "rephrase", "paraphrase", "summarise", "summarize", "shorten", "grammar", "formal", "bullet"];
const MIN_BODY_CHARS = 60;
const MAX_INSTRUCTION_CHARS = 160;

function hasRewriteVerb(s: string): boolean {
  const lower = s.toLowerCase();
  return REWRITE_WORDS.some((w) => lower.includes(w));
}

/** The text a request brings along ("다듬어줘: …", "…\n위 문장을 교정해줘"), or null (mirrors `guard::carried_text`). */
export function carriedText(text: string): string | null {
  const t = text.trim();
  for (const sep of [":", "：", "\n", '"', "“", "「", "『"]) {
    const pos = t.indexOf(sep);
    if (pos < 0) continue;
    const head = t.slice(0, pos);
    const body = t
      .slice(pos + sep.length)
      .trim()
      .replace(/["”」』]+$/u, "")
      .trim();
    if (Array.from(head).length <= MAX_INSTRUCTION_CHARS && hasRewriteVerb(head) && Array.from(body).length >= MIN_BODY_CHARS) return body;
  }
  const nl = t.lastIndexOf("\n");
  if (nl >= 0) {
    const body = t.slice(0, nl).trim();
    const tail = t.slice(nl + 1).trim();
    if (Array.from(tail).length <= MAX_INSTRUCTION_CHARS && hasRewriteVerb(tail) && Array.from(body).length >= MIN_BODY_CHARS) return body;
  }
  return null;
}

