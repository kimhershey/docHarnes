// 1.0.5 (user decision C): a question the planner asks with its readings as choices.
//
// A sentence that can be read more than one way — its subject is also the name of a project or of a
// document type, or its scope names a type that documents carry by classification and by name — comes
// back as a clarification with `PlanResult.choices`: one ready-made plan per reading. The plan the user
// picks is not planned again. It is checked and previewed the way an edited plan is (`planner_execute`
// with `preview_only`), and the outcome is handled exactly like a freshly planned plan: an answer is
// asked, a UI op runs, a work plan shows its card (and runs at once when nothing needs a confirmation).

import type { PlanCheck, PlanChoice, PlanResponse } from "@/lib/types";

/** The readings a clarification offers (none for an ordinary question). */
export function choicesOf(response: PlanResponse): PlanChoice[] {
  return response.result.choices ?? [];
}

/**
 * The planner response of a reading the user picked: the checked plan, its findings and its preview stand
 * where the question was; source, model and the text the plan was made from stay those of the planning call.
 */
export function chosenResponse(asked: PlanResponse, check: PlanCheck): PlanResponse {
  const result = { ...asked.result, plan: check.plan, issues: check.issues };
  delete result.choices;
  return { result, preview: check.preview, context: asked.context };
}

/**
 * The text the question's plan was made from, when that is not the sentence on the card (the question came
 * after a 되묻기: the executor re-checks the plan against the user's words, reply included — PLN-7).
 */
export function plannedTextOf(sentence: string, asked: PlanResponse): string | undefined {
  const planned = asked.result.text;
  return planned && planned !== sentence ? planned : undefined;
}
