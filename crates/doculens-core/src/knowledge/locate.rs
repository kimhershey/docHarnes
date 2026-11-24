//! Which page a passage — or a match inside it — stands on (1.0.5).
//!
//! User report: "근거로 출력하는 위치와 검색의 위치가 문서에서 찾은 내용과 맞지 않음". A chunk carries
//! the pages it covers (`page_start..=page_end`) and no offsets, and the packer never cuts at a page
//! boundary: at the usual chunk size of 1,200 tokens a chunk runs over two or three pages. Every
//! label and every link said `page_start`, so a match on the chunk's last page was shown — and
//! opened — two pages early.
//!
//! Nothing is stored for this (no schema change, no re-chunking): the page is worked out from
//! `pages.text` when a hit or a source is made.
//!
//! * A chunk's text is the text of its pages, one after the other (the chunker joins the lines it
//!   read; headings and table rows keep their words). So the place where a later page begins inside
//!   the chunk is where that page's first characters stand, and the match is on the last page that
//!   begins at or before it ([`by_page_heads`]). Texts are compared without white space and case
//!   ([`compact_form`]): the chunker trims and re-joins lines.
//! * Where a page's beginning is not found in the chunk (a repeated table header, a heading the
//!   chunker rewrote, masked numbers), the match is looked for on the pages themselves — with the
//!   characters that follow it, then with those before it, then alone — and the first page that
//!   holds it is taken. Nothing found: the chunk's first page, as before.
//! * A converted document (HWPX / DOCX / PPTX / XLSX → PDF) whose printed PDF has no text layer keeps
//!   its whole text on page 1 (`documents::import`), while the viewer shows every page: "p.1" is
//!   then no place at all. For such a document no page is given (`None`).

use std::collections::HashMap;

use rusqlite::{params, Connection};

use super::repo::compact_form;

/// The characters of a page's beginning looked for in the chunk: the longer first, then a shorter
/// one (a first line the chunker changed after a few words).
const HEAD_CHARS: [usize; 2] = [24, 12];
/// A page beginning shorter than this (a bare page number) could stand anywhere.
const MIN_HEAD_CHARS: usize = 6;
/// The characters around a match that are looked for with it on the pages.
const WINDOW_CHARS: usize = 24;

/// Works out pages for the hits / sources of one request; what it learns about a document is kept.
#[derive(Default)]
pub(crate) struct PageLocator {
    reliable: HashMap<String, bool>,
}

impl PageLocator {
    /// Whether the page numbers of the document's chunks say where its text is. They do not when the
    /// document is a converted one with several pages and text on the first only: the text of the
    /// whole document was kept there because its printed pages have none.
    fn reliable(&mut self, conn: &Connection, document_id: &str) -> rusqlite::Result<bool> {
        if let Some(known) = self.reliable.get(document_id) {
            return Ok(*known);
        }
        let (converted, pages, with_text, first): (Option<i64>, i64, i64, i64) = conn.query_row(
            "SELECT MAX(d.converted), COUNT(p.page_no), COALESCE(SUM(p.has_text), 0), \
             COALESCE(SUM(CASE WHEN p.page_no = 1 THEN p.has_text ELSE 0 END), 0) \
             FROM documents d LEFT JOIN pages p ON p.document_id = d.id WHERE d.id = ?1",
            [document_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let all_on_first = converted.unwrap_or(0) != 0 && pages >= 2 && with_text == 1 && first == 1;
        self.reliable.insert(document_id.to_string(), !all_on_first);
        Ok(!all_on_first)
    }

    /// The page a passage begins on — `None` when the document's pages are not to be trusted.
    pub(crate) fn start(
        &mut self,
        conn: &Connection,
        document_id: &str,
        page_start: u32,
    ) -> rusqlite::Result<Option<u32>> {
        Ok(self.reliable(conn, document_id)?.then_some(page_start))
    }

    /// The page that holds the `len` characters at character `at` of a passage's `text` (`at = 0`,
    /// `len = 0`: where the passage begins) — `None` when the document's pages are not to be trusted.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn of_match(
        &mut self,
        conn: &Connection,
        document_id: &str,
        page_start: u32,
        page_end: u32,
        text: &str,
        at: usize,
        len: usize,
    ) -> rusqlite::Result<Option<u32>> {
        if !self.reliable(conn, document_id)? {
            return Ok(None);
        }
        if page_end <= page_start {
            return Ok(Some(page_start));
        }
        let mut st = conn.prepare_cached(
            "SELECT page_no, text FROM pages WHERE document_id = ?1 AND page_no BETWEEN ?2 AND ?3 \
             AND text IS NOT NULL ORDER BY page_no",
        )?;
        let pages: Vec<(u32, String)> = st
            .query_map(params![document_id, page_start as i64, page_end as i64], |r| {
                Ok((r.get::<_, i64>(0)?.max(0) as u32, compact_form(&r.get::<_, String>(1)?)))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(page_of(text, at, len, page_start, &pages)))
    }
}

/// The page of the `len` characters at character `at` of a chunk's `text`. `pages`: the compact
/// texts of the chunk's pages, in order.
fn page_of(text: &str, at: usize, len: usize, page_start: u32, pages: &[(u32, String)]) -> u32 {
    let chars: Vec<char> = text.chars().collect();
    let at = at.min(chars.len());
    let end = (at + len).min(chars.len());
    let piece = |from: usize, to: usize| compact_form(&chars[from..to].iter().collect::<String>());
    let pages: Vec<&(u32, String)> = pages.iter().filter(|(_, t)| !t.is_empty()).collect();
    if let Some(page) = by_page_heads(&compact_form(text), piece(0, at).len(), page_start, &pages) {
        return page;
    }
    let needles =
        [piece(at, (end + WINDOW_CHARS).min(chars.len())), piece(at.saturating_sub(WINDOW_CHARS), end), piece(at, end)];
    needles
        .iter()
        .filter(|n| n.chars().count() >= 2)
        .find_map(|n| pages.iter().find(|(_, t)| t.contains(n.as_str())).map(|(no, _)| *no))
        .unwrap_or(page_start)
}

/// The page that holds byte `offset` of the compact chunk text `whole`, from where the pages after
/// the first begin inside it. `None` when a page's beginning is not found (before the offset is
/// passed): the chunk is not the plain run of its pages' texts.
fn by_page_heads(whole: &str, offset: usize, page_start: u32, pages: &[&(u32, String)]) -> Option<u32> {
    let mut page = page_start;
    // (where the page reached so far begins in the chunk: the next one begins after it)
    let mut from = 0usize;
    for (no, text) in pages.iter().filter(|(no, _)| *no > page_start) {
        let after = from + whole[from..].chars().next().map(char::len_utf8)?;
        let found = HEAD_CHARS.iter().find_map(|n| {
            let head: String = text.chars().take(*n).collect();
            if head.chars().count() < MIN_HEAD_CHARS {
                return None;
            }
            whole[after..].find(&head).map(|i| after + i)
        })?;
        if found > offset {
            break;
        }
        page = *no;
        from = found;
    }
    Some(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(texts: &[(u32, &str)]) -> Vec<(u32, String)> {
        texts.iter().map(|(no, t)| (*no, compact_form(t))).collect()
    }

    fn at(text: &str, word: &str) -> (usize, usize) {
        let byte = text.find(word).unwrap();
        (text[..byte].chars().count(), word.chars().count())
    }

    #[test]
    fn a_match_is_on_the_page_whose_text_holds_that_place_of_the_chunk() {
        let p3 = "제3조(계약금액) 계약금액은 금 12,000,000원으로 한다.";
        let p4 = "제4조(납품 기한) 을은 계약일로부터 30일 이내에 납품한다.\n계약금액의 10%를 선급한다.";
        let p5 = "제5조(지체상금) 지체일수 1일당 계약금액의 0.1%를 부과한다.";
        // The chunk begins in the middle of page 3 and runs to the end of page 5.
        let chunk = format!("계약금액은 금 12,000,000원으로 한다.\n{}\n{}", p4.replace('\n', " "), p5);
        let all = pages(&[(3, p3), (4, p4), (5, p5)]);
        let page = |word: &str| {
            let (a, l) = at(&chunk, word);
            page_of(&chunk, a, l, 3, &all)
        };
        assert_eq!(page("12,000,000원"), 3);
        assert_eq!(page("30일 이내"), 4);
        assert_eq!(page("지체일수"), 5);
        // A word every page holds is on the page its place in the chunk belongs to, not on the first.
        let (first, len) = at(&chunk, "계약금액");
        assert_eq!(page_of(&chunk, first, len, 3, &all), 3);
        let second = chunk.find("계약금액의 10%").map(|b| chunk[..b].chars().count()).unwrap();
        assert_eq!(page_of(&chunk, second, len, 3, &all), 4);
        let third = chunk.rfind("계약금액").map(|b| chunk[..b].chars().count()).unwrap();
        assert_eq!(page_of(&chunk, third, len, 3, &all), 5);
        // Where the passage begins.
        assert_eq!(page_of(&chunk, 0, 0, 3, &all), 3);
    }

    #[test]
    fn pages_that_begin_alike_are_told_apart_by_their_order() {
        // (a running header on every page, longer than what is compared of a page's beginning)
        let p1 = "주식회사 도큐 출장비 규정 전문 (2024년 1월 1일 개정)\n제1조 목적은 다음과 같다.";
        let p2 = "주식회사 도큐 출장비 규정 전문 (2024년 1월 1일 개정)\n제2조 숙박비는 150,000원을 한도로 한다.";
        let p3 = "주식회사 도큐 출장비 규정 전문 (2024년 1월 1일 개정)\n제3조 일비는 30,000원으로 한다.";
        let chunk = format!("{p1}\n{p2}\n{p3}");
        let all = pages(&[(1, p1), (2, p2), (3, p3)]);
        for (word, page) in [("목적", 1), ("150,000원", 2), ("30,000원", 3)] {
            let (a, l) = at(&chunk, word);
            assert_eq!(page_of(&chunk, a, l, 1, &all), page, "{word}");
        }
    }

    #[test]
    fn a_chunk_that_is_not_the_run_of_its_pages_is_searched_on_them() {
        // The second page begins with a number the index masked: its beginning is not in the chunk.
        let p1 = "신청인 명단\n1. 김도큐 (개발팀)";
        let p2 = "800101-1234567 홍길동 (영업팀)\n2. 이렌즈 (기획팀) 출장 신청";
        let chunk = "신청인 명단\n1. 김도큐 (개발팀)\n800101-******* 홍길동 (영업팀)\n2. 이렌즈 (기획팀) 출장 신청";
        let all = pages(&[(1, p1), (2, p2)]);
        assert_eq!(by_page_heads(&compact_form(chunk), 0, 1, &all.iter().collect::<Vec<_>>()), None);
        for (word, page) in [("김도큐", 1), ("이렌즈", 2), ("출장 신청", 2)] {
            let (a, l) = at(chunk, word);
            assert_eq!(page_of(chunk, a, l, 1, &all), page, "{word}");
        }
        // Nothing of the chunk on any page: its first page, as before.
        let other = pages(&[(1, "전혀 다른 글"), (2, "여기도 없는 글입니다 정말로")]);
        let (a, l) = at(chunk, "이렌즈");
        assert_eq!(page_of(chunk, a, l, 1, &other), 1);
    }
}
