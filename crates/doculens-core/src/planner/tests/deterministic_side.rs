//! 0.8.0 audit fixes on the planner's deterministic side: validation, scope resolution and the
//! phrase rules (the executor cases live in `exec_tests/audit_fixes.rs`).

use super::*;

/// UI-13: summaries pick the particle from the word instead of printing "을(를)" / "(으)로".
#[test]
fn rule_summaries_pick_the_korean_particle() {
    let summary = |text: &str| planned(text).plan.summary.unwrap_or_default();
    // The sentence of the audit ("문서을(를) 유형별 폴더로 이동합니다").
    assert_eq!(summary("전체 문서를 유형별로 정리해줘"), "문서를 유형별 폴더로 이동합니다");
    assert_eq!(summary("세금계산서 → 엑셀"), "세금계산서를 XLSX 파일로 내보냅니다");
    assert_eq!(summary("영수증 csv로 내보내"), "영수증을 CSV 파일로 내보냅니다");
    assert_eq!(summary("작년 계약서 전부 삭제"), "계약서를 휴지통으로 보냅니다");
    assert_eq!(summary("이 문서를 참고 자료로 표시"), "참고 자료로 표시합니다");
    assert_eq!(summary("이 문서를 양식으로 표시"), "양식으로 표시합니다");
    assert_eq!(summary("제목을 '2026 공급계약'으로 바꿔"), "제목을 '2026 공급계약'으로 바꿉니다");
    assert_eq!(summary("제목을 '한빛 2차'로 바꿔"), "제목을 '한빛 2차'로 바꿉니다");
    assert_eq!(summary("'사규' 참고 자료 묶음 만들어"), "참고 자료 묶음 '사규'를 만듭니다");
    assert_eq!(summary("'규정집' 참고 자료 묶음 만들어"), "참고 자료 묶음 '규정집'을 만듭니다");
    assert_eq!(summary("'㈜한빛'을 '(주)한빛'으로 바꿔줘"), "'㈜한빛'을 '(주)한빛'으로 바꿉니다");
    assert_eq!(summary("선택 문서에 태그 경비, 10월 추가해줘"), "태그 경비, 10월을 추가합니다");
    assert_eq!(summary("원본 문서는 무기한 보존"), "원본 문서 보존 기간을 무기한으로 설정합니다");
    assert_eq!(summary("저널은 30일만 보관해"), "파일 조작 저널 보존 기간을 30일로 설정합니다");
    // A Latin id never carries the particle: it follows a Korean noun instead.
    assert_eq!(summary("12B 모델 받아줘"), "gemma4-12b-q4km 모델을 내려받습니다");
    for text in [
        "inbox 폴더의 영수증을 전부 처리하고 월별 폴더로 정리해줘",
        "검토 완료된 영수증 거래처별 폴더로 옮겨",
        "이 문서 요약해줘",
        "거래처명이 ㈜한빛인 문서 찾아줘",
        "영수증 양식 기본으로",
        "지난달 영수증 집계 결과로 경비 보고서 초안 작성해줘 pdf로",
    ] {
        let s = summary(text);
        assert!(!s.is_empty() && !s.contains("을(를)") && !s.contains("(으)로"), "{text} → {s}");
    }
}
