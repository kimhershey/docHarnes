//! Korean particles (조사) whose form depends on the last sound of the word before them:
//! 을/를, 이/가, 은/는, 으로/로.
//!
//! Messages built from a variable used to print both forms ("문서을(를) 이동합니다",
//! "'시스템'(으)로 변경했습니다" — audit UI-13). The helpers here pick the form from the word:
//!
//! - a Hangul syllable decides by its final consonant (받침); a ㄹ final takes `로`, not `으로`;
//! - a digit is read in Korean (`3` = 삼 → 을 / 으로, `2` = 이 → 를 / 로, `7` = 칠 → 을 / 로);
//! - `%` and `°` are read 퍼센트 / 도 (no final consonant);
//! - closing quotes are skipped (`'시스템'` is decided by 템) and a trailing parenthetical is not
//!   read (`제안요청서(RFP)` is decided by 서);
//! - anything else (Latin words, symbols, an empty word) cannot be decided and keeps the
//!   neutral double form (`을(를)`, `이(가)`, `은(는)`, `(으)로`).

/// A particle pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Josa {
    /// 을 / 를 (object).
    EulReul,
    /// 이 / 가 (subject).
    IGa,
    /// 은 / 는 (topic).
    EunNeun,
    /// 으로 / 로 (direction, means, change of state).
    EuroRo,
}

/// How a word ends, as far as the particles care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Final {
    /// A vowel (no final consonant): 를 · 가 · 는 · 로.
    Vowel,
    /// The consonant ㄹ: 을 · 이 · 은, but 로.
    Rieul,
    /// Any other final consonant: 을 · 이 · 은 · 으로.
    Consonant,
    /// Not readable as Korean: both forms are printed.
    Unknown,
}

const HANGUL_FIRST: u32 = 0xAC00;
const HANGUL_LAST: u32 = 0xD7A3;
/// Final-consonant slots per syllable block (0 = none).
const JONGSEONG: u32 = 28;
const JONGSEONG_RIEUL: u32 = 8;

fn final_of(word: &str) -> Final {
    // Closing quotes and trailing blanks do not carry the sound.
    let unquote = |s: &str| -> usize {
        s.trim_end_matches(|c: char| c.is_whitespace() || "'\"’”」』》〉`*_".contains(c)).len()
    };
    let mut w = &word[..unquote(word)];
    // A trailing parenthetical is not read: "제안요청서(RFP)" is decided by 서. An unbalanced
    // bracket (or one with nothing before it) is just skipped.
    while let Some(last) = w.chars().last() {
        let open = match last {
            ')' => '(',
            ']' => '[',
            '}' => '{',
            '）' => '（',
            _ => break,
        };
        let head = w.rfind(open).map(|i| &w[..i]).filter(|h| !h.trim().is_empty());
        let rest = head.unwrap_or(&w[..w.len() - last.len_utf8()]);
        w = &rest[..unquote(rest)];
    }
    let Some(c) = w.chars().last() else { return Final::Unknown };
    let code = c as u32;
    if (HANGUL_FIRST..=HANGUL_LAST).contains(&code) {
        return match (code - HANGUL_FIRST) % JONGSEONG {
            0 => Final::Vowel,
            JONGSEONG_RIEUL => Final::Rieul,
            _ => Final::Consonant,
        };
    }
    match c {
        // 영(십·백·천·만) · 삼 · 육: a final consonant other than ㄹ.
        '0' | '3' | '6' => Final::Consonant,
        // 일 · 칠 · 팔.
        '1' | '7' | '8' => Final::Rieul,
        // 이 · 사 · 오 · 구.
        '2' | '4' | '5' | '9' => Final::Vowel,
        // 퍼센트 · 도.
        '%' | '°' => Final::Vowel,
        _ => Final::Unknown,
    }
}

/// The particle that follows `word` (the particle only, not the word).
pub fn particle(word: &str, josa: Josa) -> &'static str {
    match (josa, final_of(word)) {
        (Josa::EulReul, Final::Vowel) => "를",
        (Josa::EulReul, Final::Rieul | Final::Consonant) => "을",
        (Josa::EulReul, Final::Unknown) => "을(를)",
        (Josa::IGa, Final::Vowel) => "가",
        (Josa::IGa, Final::Rieul | Final::Consonant) => "이",
        (Josa::IGa, Final::Unknown) => "이(가)",
        (Josa::EunNeun, Final::Vowel) => "는",
        (Josa::EunNeun, Final::Rieul | Final::Consonant) => "은",
        (Josa::EunNeun, Final::Unknown) => "은(는)",
        (Josa::EuroRo, Final::Vowel | Final::Rieul) => "로",
        (Josa::EuroRo, Final::Consonant) => "으로",
        (Josa::EuroRo, Final::Unknown) => "(으)로",
    }
}

/// `word` with its particle attached (`"문서"` → `"문서를"`).
pub fn with(word: &str, josa: Josa) -> String {
    format!("{word}{}", particle(word, josa))
}

/// `word` + 을/를.
pub fn eul_reul(word: &str) -> String {
    with(word, Josa::EulReul)
}

/// `word` + 이/가.
pub fn i_ga(word: &str) -> String {
    with(word, Josa::IGa)
}

/// `word` + 은/는.
pub fn eun_neun(word: &str) -> String {
    with(word, Josa::EunNeun)
}

/// `word` + 으로/로.
pub fn euro_ro(word: &str) -> String {
    with(word, Josa::EuroRo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hangul_words_take_the_form_of_their_final_consonant() {
        // The two sentences of the audit.
        assert_eq!(eul_reul("문서"), "문서를");
        assert_eq!(euro_ro("'시스템'"), "'시스템'으로");
        // 을 / 를
        assert_eq!(eul_reul("견적서"), "견적서를");
        assert_eq!(eul_reul("영수증"), "영수증을");
        assert_eq!(eul_reul("회의록"), "회의록을");
        assert_eq!(eul_reul("선택 문서"), "선택 문서를");
        assert_eq!(eul_reul("참고 자료 묶음"), "참고 자료 묶음을");
        // 이 / 가
        assert_eq!(i_ga("템플릿"), "템플릿이");
        assert_eq!(i_ga("양식"), "양식이");
        assert_eq!(i_ga("프로젝트"), "프로젝트가");
        // 은 / 는
        assert_eq!(eun_neun("계약서"), "계약서는");
        assert_eq!(eun_neun("합계금액"), "합계금액은");
        // 으로 / 로: a ㄹ final takes 로.
        assert_eq!(euro_ro("참고 자료"), "참고 자료로");
        assert_eq!(euro_ro("업무 문서"), "업무 문서로");
        assert_eq!(euro_ro("양식"), "양식으로");
        assert_eq!(euro_ro("최신본"), "최신본으로");
        assert_eq!(euro_ro("엑셀"), "엑셀로");
        assert_eq!(euro_ro("한글 파일"), "한글 파일로");
        assert_eq!(eul_reul("엑셀"), "엑셀을");
        assert_eq!(i_ga("파일"), "파일이");
    }

    #[test]
    fn quotes_and_brackets_are_skipped() {
        assert_eq!(euro_ro("'2026 공급계약'"), "'2026 공급계약'으로");
        assert_eq!(euro_ro("“한빛 2차”"), "“한빛 2차”로");
        assert_eq!(eul_reul("「계약 묶음」"), "「계약 묶음」을");
        assert_eq!(particle("(주)한빛 ", Josa::EulReul), "을");
        assert_eq!(particle("사규)", Josa::EulReul), "를");
        // A trailing parenthetical is not read: the word before it decides.
        assert_eq!(eul_reul("제안요청서(RFP)"), "제안요청서(RFP)를");
        assert_eq!(eul_reul("일반(표/차트)"), "일반(표/차트)을");
        assert_eq!(euro_ro("'상호(법인명)'"), "'상호(법인명)'로");
        assert_eq!(eun_neun("(주)"), "(주)는");
        assert_eq!(eul_reul("WBS·일정표"), "WBS·일정표를");
    }

    #[test]
    fn digits_are_read_in_korean() {
        // 영 일 이 삼 사 오 육 칠 팔 구
        let eul: Vec<&str> = ('0'..='9').map(|d| particle(&d.to_string(), Josa::EulReul)).collect();
        assert_eq!(eul, ["을", "을", "를", "을", "를", "를", "을", "을", "을", "를"]);
        let ro: Vec<&str> = ('0'..='9').map(|d| particle(&d.to_string(), Josa::EuroRo)).collect();
        assert_eq!(ro, ["으로", "로", "로", "으로", "로", "로", "으로", "로", "로", "로"]);
        assert_eq!(euro_ro("300"), "300으로"); // 삼백
        assert_eq!(euro_ro("12"), "12로"); // 십이
        assert_eq!(euro_ro("2026"), "2026으로"); // …육
        assert_eq!(euro_ro("0.85"), "0.85로"); // 영점팔오
        assert_eq!(euro_ro("125%"), "125%로"); // 퍼센트
        assert_eq!(euro_ro("90°"), "90°로"); // 도
        assert_eq!(i_ga("문서 3"), "문서 3이");
        assert_eq!(eun_neun("7"), "7은");
    }

    #[test]
    fn undecidable_words_keep_both_forms() {
        assert_eq!(eul_reul("gemma4-12b-q4km"), "gemma4-12b-q4km을(를)");
        assert_eq!(euro_ro("dark"), "dark(으)로");
        assert_eq!(i_ga("PDF"), "PDF이(가)");
        assert_eq!(eun_neun("XLSX"), "XLSX은(는)");
        assert_eq!(eul_reul(""), "을(를)");
        assert_eq!(euro_ro("''"), "''(으)로");
        assert_eq!(eul_reul("ㅋ"), "ㅋ을(를)"); // a lone jamo is not a syllable
    }
}
