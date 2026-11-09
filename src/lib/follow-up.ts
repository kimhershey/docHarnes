// Mirror of the Rust `knowledge::answer::is_follow_up`: a question that leans on the conversation
// ("그 문서", "아까 금액", "that one") keeps the previous evidence and is retrieved together with the
// previous question.

const KO = [
  "그", "그것", "그거", "그건", "그걸", "그게", "그중", "그런", "그럼", "그러면", "그래서", "그쪽", "그때", "거기",
  "이", "이것", "이거", "이건", "이걸", "해당", "위", "아까", "방금", "앞서", "앞의", "이전", "다시", "또", "저거", "저것",
  "둘", "둘째", "첫째", "마지막", "나머지", "마찬가지로", "같은",
];
const KO_SUFFIX = "은는이가을를의도에서로과와중만";
const EN = new Set(["it", "its", "that", "this", "those", "these", "they", "them", "same", "above", "previous", "earlier", "again", "also", "then"]);

export function looksLikeFollowUp(text: string): boolean {
  const q = text.trim();
  if (!q) return false;
  for (const raw of q.split(/\s+/)) {
    const token = raw.replace(/^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu, "");
    if (!token) continue;
    for (const w of KO) {
      if (token === w) return true;
      // "그건", "그것은", "그 문서의" …: the demonstrative plus one particle.
      if (token.startsWith(w) && [...token].length === [...w].length + 1 && KO_SUFFIX.includes(token.slice(w.length))) return true;
    }
    if (EN.has(token.toLowerCase())) return true;
  }
  // A very short question after a conversation is a continuation ("왜?", "언제?").
  return [...q].length <= 4 && /[?？]$/.test(q);
}
