//! Korean-aware analysis for the morpheme FTS index and search queries.
//!
//! Two analyzers share one output contract:
//! * `ko-dic` — lindera with the mecab-ko-dic dictionary built by
//!   `crates/doculens-kodic` (located via `DOCULENS_KODIC`, the bundled
//!   `resources/ko-dic/` or `<AppData>/DocuLens/tokenizers/ko-dic/`).
//!   Particles/endings are dropped, inflected verbs are reduced to their stem
//!   and compound nouns are indexed as parts *and* as the joined form.
//! * `simple` — dependency-free fallback: Hangul/CJK runs plus their character
//!   bigrams, lower-cased Latin words and digit runs. Works without any asset
//!   so search degrades instead of failing.
//!
//! Both add "code" tokens that morphology would scatter: digit groups with
//! separators (`1,000,000` → `1000000`, `123-45-67890` → `1234567890`) and
//! clause references (`제 12 조` → `제12조`).

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use lindera::dictionary::load_fs_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use unicode_normalization::UnicodeNormalization;

use super::TokenizerInfo;
use crate::error::{CoreError, Result};

pub const ENV_KODIC: &str = "DOCULENS_KODIC";
pub const KODIC_DIR_NAME: &str = "ko-dic";
/// Longest text handed to the segmenter in one call (lattice memory).
const SEGMENT_WINDOW_CHARS: usize = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyzerKind {
    KoDic,
    Simple,
}

/// A content-bearing term with its char range in the analysed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// Normalised form (NFKC, lower-cased) that goes into the index / query.
    pub text: String,
    /// Part of speech (`NNG`, `SN`, `VV`…; `W` for simple words, `B` for bigrams, `C` for codes).
    pub pos: String,
    pub start: usize,
    pub end: usize,
}

pub struct Analyzer {
    kind: AnalyzerKind,
    segmenter: Option<Segmenter>,
    path: Option<PathBuf>,
    version: Option<String>,
}

impl std::fmt::Debug for Analyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analyzer").field("kind", &self.kind).field("path", &self.path).finish()
    }
}

impl Analyzer {
    pub fn simple() -> Analyzer {
        Analyzer { kind: AnalyzerKind::Simple, segmenter: None, path: None, version: None }
    }

    /// Loads a built ko-dic dictionary directory (see `doculens-kodic build`).
    pub fn load(dir: &Path) -> Result<Analyzer> {
        if !dir.join("metadata.json").is_file() || !dir.join("dict.words").is_file() {
            return Err(CoreError::NotFound(format!("{} is not a lindera dictionary directory", dir.display())));
        }
        let dictionary =
            load_fs_dictionary(dir).map_err(|e| CoreError::Invalid(format!("ko-dic {}: {e}", dir.display())))?;
        let segmenter = Segmenter::new(Mode::Normal, dictionary, None);
        let version = std::fs::read_to_string(dir.join("VERSION.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("dictionary").and_then(|d| d.as_str()).map(str::to_string));
        Ok(Analyzer { kind: AnalyzerKind::KoDic, segmenter: Some(segmenter), path: Some(dir.to_path_buf()), version })
    }

    /// `DOCULENS_KODIC` (dir) first, then the given candidates.
    pub fn candidates(extra: &[PathBuf]) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Some(p) = std::env::var_os(ENV_KODIC) {
            out.push(PathBuf::from(p));
        }
        out.extend(extra.iter().cloned());
        out
    }

    /// First candidate that loads; the simple analyzer otherwise.
    pub fn load_first(candidates: &[PathBuf]) -> Analyzer {
        for dir in candidates {
            match Analyzer::load(dir) {
                Ok(a) => {
                    log::info!("ko-dic loaded from {}", dir.display());
                    return a;
                }
                Err(e) => log::debug!("ko-dic candidate {}: {e}", dir.display()),
            }
        }
        Analyzer::simple()
    }

    pub fn kind(&self) -> AnalyzerKind {
        self.kind
    }

    pub fn info(&self) -> TokenizerInfo {
        TokenizerInfo {
            kind: match self.kind {
                AnalyzerKind::KoDic => "ko-dic".into(),
                AnalyzerKind::Simple => "simple".into(),
            },
            path: self.path.as_ref().map(|p| p.display().to_string()),
            version: self.version.clone(),
        }
    }

    /// Content terms with char offsets (sorted by position; codes may overlap words).
    pub fn analyze(&self, text: &str) -> Vec<Term> {
        let mut terms = match (&self.segmenter, self.kind) {
            (Some(seg), AnalyzerKind::KoDic) => kodic_terms(seg, text),
            _ => simple_terms(text),
        };
        terms.extend(code_terms(text));
        terms.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
        terms
    }

    /// Space-separated token string for `chunks_fts` (terms plus joined compounds).
    pub fn index_tokens(&self, text: &str) -> String {
        let terms = self.analyze(text);
        let mut out: Vec<String> = terms.iter().map(|t| t.text.clone()).collect();
        if self.kind == AnalyzerKind::KoDic {
            // Compounds and unknown words also get Hangul bigrams so that
            // sub-word queries ("정산" in "정산서") still match.
            let compounds = compound_tokens(&terms);
            for c in &compounds {
                out.extend(hangul_bigrams(c));
            }
            out.extend(compounds);
            for t in terms.iter().filter(|t| t.pos == "UNK") {
                out.extend(hangul_bigrams(&t.text));
            }
        }
        out.join(" ")
    }

    /// Distinct content terms of a query in order of appearance (no compounds:
    /// a document containing the compound also contains its parts).
    pub fn query_terms(&self, text: &str) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        let terms = self.analyze(text);
        let single = terms.len() == 1;
        for t in terms {
            if t.pos == "B" {
                // Bigrams of the simple analyzer make AND queries too strict on short
                // texts; the whole run plus the trigram channel cover them.
                continue;
            }
            // One-syllable dependent / unit nouns ("수", "것", "원") match almost
            // every chunk and would turn OR fallbacks into noise.
            let one_hangul = t.text.chars().count() == 1 && t.text.chars().all(|c| class_of(c) == CharClass::Hangul);
            if one_hangul && !single && (t.pos.starts_with("NNB") || t.pos == "XSN" || t.pos == "MM" || t.pos == "NP") {
                continue;
            }
            if seen.insert(t.text.clone()) {
                out.push(t.text);
            }
        }
        out
    }
}

/// NFKC + lower-case.
pub fn normalize(s: &str) -> String {
    s.nfkc().collect::<String>().to_lowercase()
}

fn is_content_pos(pos: &str) -> bool {
    let p = pos.split('+').next().unwrap_or(pos);
    p.starts_with("NN")
        || p.starts_with("NR")
        || p.starts_with("NP")
        || p == "SL"
        || p == "SH"
        || p == "SN"
        || p == "XR"
        || p == "XSN"
        || p.starts_with("VV")
        || p.starts_with("VA")
        || p == "MAG"
        || p == "MM"
}

fn is_noun_pos(pos: &str) -> bool {
    let p = pos.split('+').next().unwrap_or(pos);
    p.starts_with("NN") || p == "SL" || p == "SH" || p == "XR"
}

fn kodic_terms(seg: &Segmenter, text: &str) -> Vec<Term> {
    let mut out = Vec::new();
    // Segment in windows to bound lattice memory on very long chunks; windows
    // end at whitespace so no word is cut.
    let mut window_start = 0usize;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut char_at_byte = std::collections::HashMap::with_capacity(chars.len() + 1);
    for (ci, (bi, _)) in chars.iter().enumerate() {
        char_at_byte.insert(*bi, ci);
    }
    char_at_byte.insert(text.len(), chars.len());
    while window_start < text.len() {
        let mut end = text.len();
        if chars.len() - char_at_byte[&window_start] > SEGMENT_WINDOW_CHARS {
            let limit_char = char_at_byte[&window_start] + SEGMENT_WINDOW_CHARS;
            let mut cut = chars[limit_char].0;
            while cut > window_start && !text[cut..].starts_with(char::is_whitespace) {
                cut = text[..cut].char_indices().next_back().map(|(i, _)| i).unwrap_or(window_start);
            }
            if cut > window_start {
                end = cut;
            } else {
                end = chars[limit_char].0;
            }
        }
        let slice = &text[window_start..end];
        if let Ok(mut tokens) = seg.segment(Cow::Borrowed(slice)) {
            for tok in tokens.iter_mut() {
                let pos = tok.get_detail(0).unwrap_or("UNK").to_string();
                let kind = tok.get_detail(4).unwrap_or("*").to_string();
                let expression = tok.get_detail(7).unwrap_or("*").to_string();
                let surface = tok.surface.to_string();
                let bs = window_start + tok.byte_start;
                let be = window_start + tok.byte_end;
                let (start, end) = (char_at_byte.get(&bs).copied(), char_at_byte.get(&be).copied());
                let (Some(start), Some(end)) = (start, end) else { continue };
                if pos.starts_with('S') && pos != "SL" && pos != "SH" && pos != "SN" {
                    continue; // punctuation / symbols
                }
                if pos == "UNK" || pos == "*" {
                    // Unknown word: keep it if it carries letters or digits.
                    if surface.chars().any(|c| c.is_alphanumeric()) {
                        out.push(Term { text: normalize(&surface), pos: "UNK".into(), start, end });
                    }
                    continue;
                }
                if kind == "Inflect" && pos.contains('+') {
                    // "맞춰/VV+EC" with expression "맞추/VV/*+어/EC/*": index the stem(s).
                    let mut pushed = false;
                    for part in expression.split('+') {
                        let mut it = part.split('/');
                        let (Some(form), Some(ppos)) = (it.next(), it.next()) else { continue };
                        if is_content_pos(ppos) && !form.is_empty() && form != "*" {
                            out.push(Term { text: normalize(form), pos: ppos.to_string(), start, end });
                            pushed = true;
                        }
                    }
                    if !pushed && is_content_pos(&pos) {
                        out.push(Term { text: normalize(&surface), pos, start, end });
                    }
                    continue;
                }
                if !is_content_pos(&pos) {
                    continue;
                }
                if kind == "Compound" && expression != "*" {
                    // Dictionary compound ("사업자등록번호"): parts are searchable too.
                    out.push(Term { text: normalize(&surface), pos: pos.clone(), start, end });
                    for part in expression.split('+') {
                        let mut it = part.split('/');
                        let (Some(form), Some(ppos)) = (it.next(), it.next()) else { continue };
                        if is_content_pos(ppos) && !form.is_empty() && form != "*" {
                            out.push(Term { text: normalize(form), pos: ppos.to_string(), start, end });
                        }
                    }
                    continue;
                }
                out.push(Term { text: normalize(&surface), pos, start, end });
            }
        }
        window_start = end;
    }
    out
}

/// Character bigrams of a Hangul-only word of ≥ 3 syllables.
fn hangul_bigrams(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    if chars.len() < 3 || !chars.iter().all(|c| class_of(*c) == CharClass::Hangul) {
        return Vec::new();
    }
    chars.windows(2).map(|w| w.iter().collect()).collect()
}

/// Joined forms of adjacent nouns written without a space ("사업자" + "등록" + "번호").
pub fn compound_tokens(terms: &[Term]) -> Vec<String> {
    let mut out = Vec::new();
    // One term per span: dictionary compounds push the whole surface first and
    // their parts after it (same span); only the whole form may join neighbours.
    let mut nouns: Vec<&Term> = Vec::new();
    for t in terms.iter().filter(|t| is_noun_pos(&t.pos)) {
        if nouns.last().is_some_and(|p| p.start == t.start || t.end <= p.end) {
            continue;
        }
        nouns.push(t);
    }
    let mut i = 0;
    while i < nouns.len() {
        let mut j = i;
        let mut joined = nouns[i].text.clone();
        while j + 1 < nouns.len() && nouns[j + 1].start == nouns[j].end && j + 1 - i < 4 {
            joined.push_str(&nouns[j + 1].text);
            j += 1;
            out.push(joined.clone());
        }
        i = if j > i { j + 1 } else { i + 1 };
    }
    out.dedup();
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CharClass {
    Hangul,
    Cjk,
    Latin,
    Digit,
    Other,
}

pub(crate) fn class_of(c: char) -> CharClass {
    let u = c as u32;
    if (0xAC00..=0xD7A3).contains(&u) || (0x1100..=0x11FF).contains(&u) || (0x3130..=0x318F).contains(&u) {
        CharClass::Hangul
    } else if (0x4E00..=0x9FFF).contains(&u)
        || (0x3400..=0x4DBF).contains(&u)
        || (0x3040..=0x30FF).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
    {
        CharClass::Cjk
    } else if c.is_ascii_digit() {
        CharClass::Digit
    } else if c.is_alphanumeric() {
        CharClass::Latin
    } else {
        CharClass::Other
    }
}

fn simple_terms(text: &str) -> Vec<Term> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let cls = class_of(chars[i]);
        if cls == CharClass::Other {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && class_of(chars[j]) == cls {
            j += 1;
        }
        let run: String = chars[i..j].iter().collect();
        match cls {
            CharClass::Hangul | CharClass::Cjk => {
                if run.chars().count() <= 24 {
                    out.push(Term { text: normalize(&run), pos: "W".into(), start: i, end: j });
                }
                if j - i >= 3 {
                    for k in i..j - 1 {
                        let bg: String = chars[k..k + 2].iter().collect();
                        out.push(Term { text: normalize(&bg), pos: "B".into(), start: k, end: k + 2 });
                    }
                }
            }
            CharClass::Latin | CharClass::Digit => out.push(Term {
                text: normalize(&run),
                pos: if cls == CharClass::Digit { "SN" } else { "SL" }.into(),
                start: i,
                end: j,
            }),
            CharClass::Other => {}
        }
        i = j;
    }
    out
}

/// Digit groups with separators and clause references, as single tokens.
fn code_terms(text: &str) -> Vec<Term> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let mut j = i;
            let mut last_digit = i;
            while j < chars.len() && (chars[j].is_ascii_digit() || matches!(chars[j], ',' | '.' | '-' | '/')) {
                if chars[j].is_ascii_digit() {
                    last_digit = j;
                }
                j += 1;
            }
            let end = last_digit + 1;
            let raw: String = chars[i..end].iter().collect();
            let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
            if raw.len() != digits.len() && digits.len() >= 4 {
                out.push(Term { text: digits, pos: "C".into(), start: i, end });
            }
            i = end.max(i + 1);
            continue;
        }
        if chars[i] == '제' {
            // 제 12 조 / 제3항 / 제2호 / 제1장 / 제2절
            let mut j = i + 1;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            let ds = j;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j > ds && j - ds <= 4 {
                let mut k = j;
                while k < chars.len() && chars[k] == ' ' {
                    k += 1;
                }
                if k < chars.len() && matches!(chars[k], '조' | '항' | '호' | '장' | '절' | '관' | '편') {
                    let num: String = chars[ds..j].iter().collect();
                    out.push(Term { text: format!("제{num}{}", chars[k]), pos: "C".into(), start: i, end: k + 1 });
                    i = k + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Quotes a term for an FTS5 MATCH expression.
pub fn fts_quote(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_analyzer_words_bigrams_and_codes() {
        let a = Analyzer::simple();
        let tokens = a.index_tokens("공급가액은 1,000,000원 (Invoice No. 123-45-67890) 제 12 조");
        for expected in
            ["공급가액은", "공급", "급가", "가액", "액은", "1000000", "invoice", "no", "1234567890", "제12조", "원"]
        {
            assert!(tokens.split(' ').any(|t| t == expected), "{expected} missing in {tokens}");
        }
        let q = a.query_terms("공급가액");
        assert_eq!(q, vec!["공급가액".to_string()]);
    }

    #[test]
    fn code_terms_cover_amounts_and_clauses() {
        let t = code_terms("합계 12,345,678원, 제3항, 제 2 호, 2024-11-04");
        let texts: Vec<&str> = t.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, vec!["12345678", "제3항", "제2호", "20241104"]);
    }

    #[test]
    fn compounds_join_adjacent_nouns_only() {
        let terms = vec![
            Term { text: "사업자".into(), pos: "NNG".into(), start: 0, end: 3 },
            Term { text: "등록".into(), pos: "NNG".into(), start: 3, end: 5 },
            Term { text: "번호".into(), pos: "NNG".into(), start: 5, end: 7 },
            Term { text: "계약".into(), pos: "NNG".into(), start: 8, end: 10 },
        ];
        assert_eq!(compound_tokens(&terms), vec!["사업자등록".to_string(), "사업자등록번호".to_string()]);
    }

    #[test]
    fn fts_quote_escapes_quotes() {
        assert_eq!(fts_quote("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn kodic_analyzer_when_available() {
        let Some(dir) = std::env::var_os(ENV_KODIC) else {
            eprintln!("kodic_analyzer_when_available: skipped (set DOCULENS_KODIC)");
            return;
        };
        let a = Analyzer::load(Path::new(&dir)).expect("ko-dic loads");
        assert_eq!(a.kind(), AnalyzerKind::KoDic);
        let tokens = a.index_tokens(
            "공급가액은 1,000,000원이고 사업자등록번호는 123-45-67890입니다. 출장비 규정에 맞춰 정산서를 작성했습니다.",
        );
        let set: std::collections::HashSet<&str> = tokens.split(' ').collect();
        for expected in [
            "공급",
            "가액",
            "공급가액",
            "사업자",
            "등록",
            "번호",
            "사업자등록번호",
            "1000000",
            "1234567890",
            "출장비",
            "규정",
            "맞추",
            "정산",
            "작성",
        ] {
            assert!(set.contains(expected), "{expected} missing in {tokens}");
        }
        for particle in ["은", "는", "이고", "입니다", "를", "에"] {
            assert!(!set.contains(particle), "particle {particle} indexed: {tokens}");
        }
        assert_eq!(
            a.query_terms("공급가액은 얼마야?"),
            vec!["공급".to_string(), "가액".to_string(), "얼마".to_string()]
        );
        // Long text is segmented in windows without losing words.
        let long = "세금계산서 발행 ".repeat(1_500);
        let n = a.index_tokens(&long).split(' ').filter(|t| *t == "세금").count();
        assert!(n >= 1_500, "{n}");
    }
}
