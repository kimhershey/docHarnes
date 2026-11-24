// The model context an evidence budget asks for — the browser mock's copy of the core's arithmetic
// (`knowledge::answer::context_needed`, `engine::presets::chat_context`; 1.0.6, user decision 2026-11-24:
// no limits on the evidence settings — what fails is reported with the advice to lower the budget).
// The desktop app never computes this in the webview: the engine bridge writes the preset file. The numbers
// here are pinned to the core's by `evidenceContext.test.ts`.

/** `knowledge.context_tokens` of a fresh install — the budget the profiles' contexts were sized for. */
export const DEFAULT_CONTEXT_TOKENS = 6000;

/** Reply limit of a summary at the default budget, the prompt scaffold and the empty request line (`answer.rs`). */
const SUMMARY_TOKENS = 1500;
const PROMPT_SCAFFOLD_TOKENS = 700;
const REQUEST_LINE_TOKENS = 12;

/**
 * The `ctx-size` one call with `contextTokens` of evidence takes: the evidence, a summary's reply (a quarter of
 * the budget), the scaffold — rounded up to a multiple of 1,024. Null up to the default budget, which leaves
 * every model's context as its profile gives it.
 */
export function contextNeeded(contextTokens: number): number | null {
  if (contextTokens <= DEFAULT_CONTEXT_TOKENS) return null;
  const reply = Math.max(SUMMARY_TOKENS, Math.floor(contextTokens / 4));
  return Math.ceil((contextTokens + PROMPT_SCAFFOLD_TOKENS + reply + REQUEST_LINE_TOKENS) / 1024) * 1024;
}

/** The context a chat model is loaded with: the profile's, or what the budget needs when that is more — within the model's maximum. */
export function chatContext(profileContext: number, contextMax: number, contextTokens: number): number {
  const needed = contextNeeded(contextTokens);
  if (needed == null) return profileContext;
  return Math.max(profileContext, contextMax > 0 ? Math.min(needed, contextMax) : needed);
}
