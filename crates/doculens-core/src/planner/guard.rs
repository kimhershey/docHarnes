//! Post-model sanity rule for sentences that carry their own text.
//!
//! "다음 문장을 다듬어 정리해줘: …" asks about the text inside the sentence, not about the library, yet a
//! planner model that only knows document ops answers it with `summarize` over every document (one
//! retrieval per document, 55 of them on the first report). The model's reading stays authoritative for
//! everything else; this only re-targets a knowledge-base op without an explicit folder / filter when the
//! sentence demonstrably brought the material along.

use super::{Op, OpKind, Plan, PlanIssue, Scope};

/// A folder or a content filter names documents on purpose. A date range alone does not: the date
/// resolver fills it from any date in the sentence, including dates inside the pasted text.
fn explicit_scope(plan: &Plan) -> bool {
    let f = &plan.scope.filter;
    plan.scope.folder.is_some()
        || f.doc_type.is_some()
        || f.keyword.is_some()
        || f.vendor.is_some()
        || f.status.is_some()
        || !f.tags.is_empty()
        || f.min_confidence.is_some()
        || f.review_status.is_some()
}

/// Verbs that act on text the user supplied.
const REWRITE_KO: &[&str] = &[
    "다듬",
    "정리해",
    "정리 해",
    "교정",
    "맞춤법",
    "번역",
    "고쳐",
    "다시 써",
    "다시써",
    "윤문",
    "요약해",
    "줄여",
    "풀어 써",
    "풀어써",
    "바꿔 써",
    "존댓말",
    "반말",
    "문장으로",
    "개조식으로",
    "매끄럽",
];
const REWRITE_EN: &[&str] = &[
    "rewrite",
    "polish",
    "proofread",
    "translate",
    "tidy",
    "rephrase",
    "paraphrase",
    "summarise",
    "summarize",
    "shorten",
    "grammar",
    "formal",
    "bullet",
];
/// Shortest body that counts as "the text came with the request".
const MIN_BODY_CHARS: usize = 60;
/// Longest instruction part; anything longer is a question in its own right.
const MAX_INSTRUCTION_CHARS: usize = 160;

fn has_rewrite_verb(s: &str) -> bool {
    let lower = s.to_lowercase();
    REWRITE_KO.iter().chain(REWRITE_EN).any(|w| lower.contains(w))
}

/// The text a request brings along: "instruction + separator + body" (colon, newline or quotes) or
/// "body + newline + instruction", with a rewrite verb in the instruction and a body of at least
/// `MIN_BODY_CHARS` characters.
pub fn carried_text(text: &str) -> Option<&str> {
    let text = text.trim();
    for sep in [':', '：', '\n', '"', '“', '「', '『'] {
        if let Some(pos) = text.find(sep) {
            let (head, tail) = text.split_at(pos);
            let body = tail
                .trim_start_matches(sep)
                .trim()
                .trim_start_matches(['"', '“', '「', '『'])
                .trim_end_matches(['"', '”', '」', '』'])
                .trim();
            if head.chars().count() <= MAX_INSTRUCTION_CHARS
                && has_rewrite_verb(head)
                && body.chars().count() >= MIN_BODY_CHARS
            {
                return Some(body);
            }
        }
    }
    if let Some(pos) = text.rfind('\n') {
        let (body, tail) = text.split_at(pos);
        let tail = tail.trim();
        if tail.chars().count() <= MAX_INSTRUCTION_CHARS
            && has_rewrite_verb(tail)
            && body.trim().chars().count() >= MIN_BODY_CHARS
        {
            return Some(body.trim());
        }
    }
    None
}

/// Re-targets a plan made of knowledge-base ops (summarize / answer / explain / compare / search) with
/// no folder or filter to one text-only `answer` when the sentence carries the text it talks about.
/// Returns the note shown on the card when it did.
pub fn prefer_text_only_answer(text: &str, plan: &mut Plan, ko: bool) -> Option<PlanIssue> {
    carried_text(text)?;
    if plan.operations.is_empty() || explicit_scope(plan) {
        return None;
    }
    let knowledge_only = plan.operations.iter().all(|op| match op {
        Op::Summarize { .. } | Op::Explain { .. } | Op::Compare { .. } | Op::Search { .. } => true,
        Op::Answer { knowledge, .. } => knowledge.unwrap_or(true),
        _ => false,
    });
    if !knowledge_only {
        return None;
    }
    plan.intent = OpKind::Answer;
    plan.scope = Scope::default();
    plan.operations = vec![Op::Answer { question: text.trim().to_string(), cite: Some(false), knowledge: Some(false) }];
    plan.requires_confirmation = false;
    plan.clarification_question = None;
    plan.summary = Some(
        if ko {
            "보내 주신 글을 문서 검색 없이 요청대로 다듬습니다"
        } else {
            "Works on the text you sent, without looking documents up"
        }
        .to_string(),
    );
    Some(PlanIssue::warn(
        "operations",
        "text_only_answer",
        if ko {
            "문장에 글이 들어 있어 문서를 찾는 대신 그 글로 답합니다"
        } else {
            "the sentence carries its own text; answering from it instead of searching documents"
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::SummarizeTarget;

    const BODY: &str = "금번 과업은 당사가 수행하며 기한은 12월말까지로 하고 산출물은 설계서와 소스코드로 함. 검수는 발주처가 납품 후 2주 내에 완료하는 것으로 한다.";

    fn kb_plan(op: Op) -> Plan {
        Plan {
            intent: OpKind::Summarize,
            scope: Scope::default(),
            operations: vec![op],
            requires_confirmation: true,
            confidence: 0.9,
            clarification_question: None,
            summary: None,
        }
    }

    #[test]
    fn a_pasted_body_after_a_rewrite_instruction_is_recognised() {
        assert_eq!(carried_text(&format!("다음 문장을 다듬어 정리해줘: {BODY}")), Some(BODY));
        assert_eq!(carried_text(&format!("교정해줘\n\"{BODY}\"")), Some(BODY));
        assert_eq!(carried_text(&format!("{BODY}\n위 문장을 매끄럽게 다듬어 줘")), Some(BODY));
        assert_eq!(carried_text(&format!("Please polish this: {BODY}")), Some(BODY));
        // No verb, a short body, or a plain question: nothing carried.
        assert_eq!(carried_text(&format!("이 내용 알려줘: {BODY}")), None);
        assert_eq!(carried_text("다듬어줘: 짧은 글"), None);
        assert_eq!(carried_text("이 계약서의 계약기간은?"), None);
    }

    #[test]
    fn a_library_wide_summary_of_a_pasted_text_becomes_a_text_only_answer() {
        let text = format!("다음 문장을 다듬어 정리해줘: {BODY}");
        let mut plan = kb_plan(Op::Summarize { target: Some(SummarizeTarget::EachDoc), length: None });
        let note = prefer_text_only_answer(&text, &mut plan, true).expect("re-targeted");
        assert_eq!(note.code, "text_only_answer");
        assert!(!note.blocking);
        assert_eq!(plan.intent, OpKind::Answer);
        assert!(!plan.requires_confirmation);
        assert!(
            matches!(&plan.operations[0], Op::Answer { knowledge: Some(false), cite: Some(false), question } if question == &text)
        );
    }

    #[test]
    fn explicit_scopes_and_non_knowledge_ops_are_left_alone() {
        let text = format!("다음 문장을 다듬어 정리해줘: {BODY}");
        let mut scoped = kb_plan(Op::Summarize { target: None, length: None });
        scoped.scope.folder = Some("documents/2026".into());
        assert!(prefer_text_only_answer(&text, &mut scoped, true).is_none());
        let mut validate = kb_plan(Op::Validate {});
        assert!(prefer_text_only_answer(&text, &mut validate, true).is_none());
        let mut plain = kb_plan(Op::Summarize { target: None, length: None });
        assert!(prefer_text_only_answer("지난달 계약서 요약해줘", &mut plain, true).is_none());
        // A date range the resolver filled from "12월말" inside the body is not an explicit scope.
        let mut dated = kb_plan(Op::Summarize { target: Some(SummarizeTarget::EachDoc), length: None });
        dated.scope.filter.date_from = Some("2025-12-01".into());
        dated.scope.filter.date_to = Some("2025-12-31".into());
        assert!(prefer_text_only_answer(&text, &mut dated, true).is_some());
        assert!(dated.scope.is_empty());
    }
}
